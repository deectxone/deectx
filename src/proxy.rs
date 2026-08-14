use crate::config::Config;
use crate::detect::DetectorChain;
use crate::ledger::{sha256_hex, Ledger, LedgerEntry, LedgerEvent};
use crate::masker::Masker;
use crate::packs;
use crate::span::Action;
use crate::sse::SseRehydrator;
use crate::stats::LiveStats;
use anyhow::Result;
use axum::body::Body;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use chrono::Utc;
use futures_util::StreamExt;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;

/// The parts of the masking pipeline that depend on which packs are active.
/// Held behind a lock so `POST /packs` can swap in a freshly-built chain
/// without restarting the proxy or interrupting in-flight requests.
pub(crate) struct PackRuntime {
    pub(crate) chain: DetectorChain,
    pub(crate) packs: Vec<String>,
    pub(crate) allowlist: crate::allowlist::Allowlist,
    pub(crate) fail_closed: bool,
}

/// Build a [`PackRuntime`] from `cfg.active_packs` (plus the mandatory
/// `default` pack and any packs in `cfg.packs_dir`). Shared by initial
/// startup and by `POST /packs` reloading with a different active set.
fn build_pack_runtime(cfg: &Config) -> PackRuntime {
    let packs = packs::load_active(cfg);
    let pack_names: Vec<String> = packs.iter().map(|p| p.name.clone()).collect();
    let allowlist = crate::allowlist::Allowlist::new(packs::allow_entries(cfg, &packs));
    let fail_closed = packs.iter().any(|p| p.settings.fail_closed);
    let chain = packs::build_chain(
        &packs,
        cfg.ner,
        cfg.model_dir
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("./models")),
    );
    PackRuntime {
        chain,
        packs: pack_names,
        allowlist,
        fail_closed,
    }
}

pub(crate) struct AppState {
    pub(crate) upstream: String,
    pub(crate) anthropic_upstream: String,
    pub(crate) upstream_responses: String,
    pub(crate) runtime: RwLock<PackRuntime>,
    pub(crate) masker: std::sync::Arc<Masker>,
    pub(crate) ledger: Ledger,
    pub(crate) http: reqwest::Client,
    /// Snapshot of the config the proxy was started with — reused as the base
    /// when `POST /packs` rebuilds the runtime with a new `active_packs`.
    pub(crate) cfg: Config,
    pub(crate) stats: std::sync::Arc<LiveStats>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiFormat {
    OpenAI,
    Anthropic,
}

pub async fn run_proxy(cfg: Config) -> Result<()> {
    run_proxy_with_shutdown(cfg, std::future::pending()).await
}

/// Like [`run_proxy`] (binds, writes the pidfile, serves, clears the pidfile
/// on exit), but stops as soon as `shutdown` resolves instead of running
/// forever. Used by the tray to stop masking without exiting its process —
/// the pidfile write/clear happens here exactly as it does for plain
/// `serve`, so `deectx status`/`doctor` see a tray-hosted proxy the same way
/// they'd see a standalone one.
pub async fn run_proxy_with_shutdown(
    cfg: Config,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    run_proxy_with_shutdown_and_ready(cfg, shutdown, None).await
}

/// Like [`run_proxy_with_shutdown`], but additionally signals `ready` (if
/// given) the moment the listener has successfully bound — before that,
/// `bind` may still fail (port in use, etc.). Lets a caller like the tray's
/// `ProxyHandle` distinguish "genuinely bound and serving" from "assumed
/// success the instant the call was made," which is what `is_running()`
/// needs to avoid showing an "active" icon over a dead listener.
pub async fn run_proxy_with_shutdown_and_ready(
    cfg: Config,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ready: Option<std::sync::mpsc::Sender<()>>,
) -> Result<()> {
    let listener = TcpListener::bind(&cfg.listen).await?;
    if let Some(tx) = ready {
        let _ = tx.send(());
    }
    let actual = listener.local_addr()?.to_string();
    tracing::info!("deectx listening on {}", actual);
    let pf = crate::home::Pidfile {
        pid: std::process::id(),
        listen: actual,
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    if let Err(e) = pf.write() {
        tracing::warn!("could not write pidfile: {e}");
    }
    let res = serve_with_listener_and_shutdown(cfg, listener, shutdown).await;
    crate::home::Pidfile::clear();
    res
}

pub async fn serve_with_listener(cfg: Config, listener: TcpListener) -> Result<()> {
    serve_with_listener_and_shutdown(cfg, listener, std::future::pending()).await
}

/// Like [`serve_with_listener`], but stops serving as soon as `shutdown`
/// resolves instead of running forever. Used by the tray (`src/tray/`) to
/// stop masking without exiting the process it's hosting the icon in.
pub async fn serve_with_listener_and_shutdown(
    cfg: Config,
    listener: TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let stats_enabled = cfg.stats_enabled;
    let anthropic_upstream = cfg
        .upstream_anthropic
        .clone()
        .unwrap_or_else(|| "https://api.anthropic.com".to_string());
    let runtime = build_pack_runtime(&cfg);
    let state = Arc::new(AppState {
        upstream: cfg.upstream.trim_end_matches('/').to_string(),
        anthropic_upstream,
        upstream_responses: cfg.upstream_responses.clone(),
        runtime: RwLock::new(runtime),
        masker: std::sync::Arc::new(Masker::new()),
        ledger: Ledger::new(cfg.ledger_path.clone(), cfg.ledger_retention_days)?,
        http: reqwest::Client::new(),
        cfg,
        stats: std::sync::Arc::new(LiveStats::new()),
    });
    let mut app = Router::new()
        .route("/healthz", axum::routing::get(|| async { "ok" }))
        .route(
            "/v1/chat/completions",
            axum::routing::post(handle_chat_openai),
        )
        .route("/v1/messages", axum::routing::post(handle_chat_anthropic))
        .route(
            "/v1/responses",
            axum::routing::get(crate::responses_ws::ws_handler),
        );
    if stats_enabled {
        app = app
            .route("/stats", axum::routing::get(handle_stats))
            .route("/audit/today", axum::routing::get(handle_audit_today))
            .route("/audit/day", axum::routing::get(handle_audit_day))
            .route("/audit/days", axum::routing::get(handle_audit_days))
            .route("/audit/clear", axum::routing::post(handle_audit_clear))
            .route(
                "/packs",
                axum::routing::get(handle_packs_get).post(handle_packs_post),
            )
            .route("/preview", axum::routing::post(handle_preview))
            .route("/tools", axum::routing::get(handle_tools))
            .route("/version", axum::routing::get(handle_version))
            .route("/", axum::routing::get(handle_dashboard))
            .route("/dashboard", axum::routing::get(handle_dashboard));
    }
    let app = app.fallback(handle_passthrough).with_state(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

fn session_id(body: &serde_json::Value) -> String {
    let first = &body["messages"][0]["content"];
    let seed = match first {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(_) => first.to_string(),
        _ => String::new(),
    };
    format!("s_{}", &sha256_hex(&seed)[..8])
}

async fn handle_chat_openai(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_completion(st, headers, body, ApiFormat::OpenAI).await
}

async fn handle_chat_anthropic(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_completion(st, headers, body, ApiFormat::Anthropic).await
}

async fn handle_completion(
    st: Arc<AppState>,
    headers: HeaderMap,
    body: Bytes,
    format: ApiFormat,
) -> Response {
    let start = std::time::Instant::now();
    st.stats.record_request();
    let tool = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let mut json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(j) => j,
        Err(_) => return forward_raw(&st, format, headers, body, None, false).await,
    };
    let session = session_id(&json);
    if pack_runtime_blocked(&st) {
        tracing::warn!("failClosed enforcement: a required detector (NER model) is unavailable; refusing request");
        return Response::builder()
            .status(503)
            .body(Body::from(
                "deeCtx: failClosed enforcement — masking cannot be guaranteed",
            ))
            .unwrap();
    }
    let mut events = Vec::new();
    let out = mask_or_forward_unmasked(&st, &session, &mut events, &mut json, &body);
    let was_masked = out != body.as_ref();
    let stream = json
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let mut resp = forward_raw(
        &st,
        format,
        headers.clone(),
        Bytes::from(out),
        Some(&session),
        stream,
    )
    .await;
    // A masked request that upstream rejects with 400 is far more likely to be
    // deeCtx corrupting something it shouldn't have touched than a genuinely
    // malformed client request. Retry once with the untouched original body so
    // a masking bug degrades to "this one request went out unmasked" instead
    // of a broken coding session — logged as an error (and counted in
    // `/stats`) so it's visible in `deectx status`/`audit` rather than
    // silently biting the next request too.
    if was_masked && resp.status() == StatusCode::BAD_REQUEST {
        tracing::error!(
            "upstream rejected a masked request with 400; retrying unmasked so traffic keeps \
             flowing — this most likely means a masking bug corrupted the request"
        );
        st.stats.record_error();
        events.clear();
        resp = forward_raw(&st, format, headers, body.clone(), Some(&session), stream).await;
    }
    // Streaming latency is measured in Task 3; non-stream requests buffer the
    // full body in forward_raw, so elapsed() here is the true request latency.
    let latency_ms = if stream {
        0
    } else {
        start.elapsed().as_millis()
    };
    let entry = LedgerEntry {
        ts: Utc::now(),
        tool,
        session: session.clone(),
        events,
        latency_ms,
        packs: st.runtime.read().unwrap().packs.clone(),
    };
    if let Err(e) = st.ledger.append(&entry) {
        tracing::warn!("ledger append failed: {e}");
    }
    resp
}

async fn handle_stats(State(st): State<Arc<AppState>>) -> axum::Json<crate::stats::StatsSnapshot> {
    axum::Json(st.stats.snapshot())
}

/// Today's ledger summary as JSON — entity **types** and counts only (e.g.
/// `{"email": 4, "api_key": 1}`), the same hash-only aggregation `deectx
/// audit --today` prints. Never the masked values themselves: the ledger
/// never stores raw PII, so there is nothing more revealing to serve.
async fn handle_audit_today(State(st): State<Arc<AppState>>) -> Response {
    let today = crate::ledger::local_date(Utc::now());
    audit_response(&st, today)
}

/// A specific local calendar day's ledger summary, e.g. `/audit/day?date=2026-08-13`
/// — backs the dashboard's day picker. Same shape/semantics as
/// [`handle_audit_today`], just for an arbitrary day instead of today.
async fn handle_audit_day(
    State(st): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(date_str) = params.get("date") else {
        return Response::builder()
            .status(400)
            .body(Body::from("deeCtx: missing ?date=YYYY-MM-DD"))
            .unwrap();
    };
    match chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        Ok(date) => audit_response(&st, date),
        Err(_) => Response::builder()
            .status(400)
            .body(Body::from("deeCtx: invalid date, expected YYYY-MM-DD"))
            .unwrap(),
    }
}

fn audit_response(st: &AppState, date: chrono::NaiveDate) -> Response {
    match crate::audit::summarize_for_date(st.ledger.path(), date) {
        Ok(summary) => axum::Json(summary).into_response(),
        Err(e) => {
            tracing::warn!("dashboard: could not read audit summary for {date}: {e}");
            Response::builder()
                .status(500)
                .body(Body::from("deeCtx: could not read audit summary"))
                .unwrap()
        }
    }
}

/// The local calendar days that have ledger data on disk, most recent first
/// — populates the dashboard's day picker.
async fn handle_audit_days(State(st): State<Arc<AppState>>) -> Response {
    match st.ledger.available_dates() {
        Ok(dates) => {
            let dates: Vec<String> = dates.iter().map(|d| d.to_string()).collect();
            axum::Json(serde_json::json!({ "dates": dates })).into_response()
        }
        Err(e) => {
            tracing::warn!("dashboard: could not list ledger dates: {e}");
            Response::builder()
                .status(500)
                .body(Body::from("deeCtx: could not list ledger dates"))
                .unwrap()
        }
    }
}

/// Wipes all ledger files and resets the live counters — the dashboard's
/// "clear logs" action, for starting a fresh tracking window on demand.
async fn handle_audit_clear(State(st): State<Arc<AppState>>) -> Response {
    match st.ledger.clear_all() {
        Ok(()) => {
            st.stats.reset();
            axum::Json(serde_json::json!({ "cleared": true })).into_response()
        }
        Err(e) => {
            tracing::warn!("dashboard: could not clear ledger: {e}");
            Response::builder()
                .status(500)
                .body(Body::from("deeCtx: could not clear ledger"))
                .unwrap()
        }
    }
}

/// Which packs are currently active, plus every pack the dashboard can toggle
/// on. `default` is always active and cannot be turned off — it's loaded
/// unconditionally by [`packs::load_active`].
async fn handle_packs_get(State(st): State<Arc<AppState>>) -> Response {
    let active: std::collections::HashSet<String> =
        st.runtime.read().unwrap().packs.iter().cloned().collect();
    let catalog = [
        ("default", "Baseline PII detectors — always on"),
        ("gdpr", "GDPR-relevant entities (IBAN, EU-style PII)"),
        ("cdr-au", "Australian Consumer Data Right entities"),
    ];
    let packs: Vec<serde_json::Value> = catalog
        .iter()
        .map(|(name, description)| {
            serde_json::json!({
                "name": name,
                "description": description,
                "active": active.contains(*name),
                "locked": *name == "default",
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "packs": packs })).into_response()
}

/// Sets which packs are active from the dashboard: persists `active_packs` to
/// `config.toml` (so it survives a restart) and immediately swaps in a
/// freshly-built [`PackRuntime`] — no restart needed for the change to take
/// effect on the next request.
async fn handle_packs_post(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    #[derive(serde::Deserialize)]
    struct Req {
        active_packs: Vec<String>,
    }
    let req: Req = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return Response::builder()
                .status(400)
                .body(Body::from(
                    "deeCtx: expected JSON body {\"active_packs\": [\"gdpr\", ...]}",
                ))
                .unwrap()
        }
    };
    // Only known, toggleable built-in packs are accepted — "default" is
    // implicit and unrecognized names are silently dropped rather than
    // erroring, so a stale/typo'd entry in the request never blocks the rest.
    let known = ["gdpr", "cdr-au"];
    let filtered: Vec<String> = req
        .active_packs
        .into_iter()
        .filter(|p| known.contains(&p.as_str()))
        .collect();

    if let Err(e) = crate::config::write_active_packs(&crate::home::config_path(), &filtered) {
        tracing::warn!("could not persist active_packs to config.toml: {e}");
        return Response::builder()
            .status(500)
            .body(Body::from("deeCtx: could not save config"))
            .unwrap();
    }

    let mut cfg = st.cfg.clone();
    cfg.active_packs = filtered;
    let runtime = build_pack_runtime(&cfg);
    *st.runtime.write().unwrap() = runtime;

    handle_packs_get(State(st)).await
}

/// The running binary's version — lets the dashboard show what's actually
/// deployed without the user having to run `deectx --version` separately.
async fn handle_version() -> Response {
    axum::Json(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") })).into_response()
}

/// Dry-run detection for the dashboard's "Test your prompt" box: runs the
/// active packs' detectors over arbitrary text and returns the spans that
/// would be masked/redacted, so the user can see risk before ever sending it
/// anywhere. Unlike real traffic, this never touches the ledger or `/stats`
/// counters and never leaves the machine — it's a preview, not a request.
async fn handle_preview(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    #[derive(serde::Deserialize)]
    struct Req {
        text: String,
    }
    let req: Req = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return Response::builder()
                .status(400)
                .body(Body::from("deeCtx: expected JSON body {\"text\": \"...\"}"))
                .unwrap()
        }
    };
    let rt = st.runtime.read().unwrap();
    let spans = rt.allowlist.filter(rt.chain.detect(&req.text));
    let out: Vec<serde_json::Value> = spans
        .iter()
        .map(|s| {
            serde_json::json!({
                "start": s.start,
                "end": s.end,
                "entity": s.entity,
                "action": match s.action {
                    Action::Mask => "mask",
                    Action::Redact => "redact",
                },
                "alert": s.alert,
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "spans": out })).into_response()
}

/// Every AI tool deeCtx knows how to wire, and whether each one is currently
/// installed and pointed at this proxy — the same check as `deectx doctor`,
/// for the dashboard. Unlike `setup::discover()` (which only lists tools
/// whose config file exists), this always reports all three so "not
/// installed" and "installed but not wired" read differently to the user.
async fn handle_tools() -> Response {
    use crate::setup::Tool;
    let tools: Vec<serde_json::Value> = [Tool::ClaudeCode, Tool::Codex, Tool::Opencode]
        .into_iter()
        .map(|tool| {
            let path = tool.config_path();
            let (installed, wired) = match &path {
                Some(p) if p.exists() => {
                    let content = std::fs::read_to_string(p).unwrap_or_default();
                    (true, crate::setup::wired(tool, &content))
                }
                _ => (false, false),
            };
            serde_json::json!({
                "name": format!("{tool:?}"),
                "config_path": path.map(|p| p.display().to_string()),
                "installed": installed,
                "wired": wired,
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "tools": tools })).into_response()
}

/// The local status dashboard (`GET /` and `/dashboard`): live counters,
/// today's masked/redacted breakdown by entity type, and which tools are
/// wired in — served from the same process that's already doing the masking,
/// so it needs no separate auth story beyond "you can reach 127.0.0.1".
async fn handle_dashboard() -> impl IntoResponse {
    axum::response::Html(include_str!("dashboard.html"))
}

/// Transparent reverse-proxy fallback for any endpoint the router does not
/// explicitly handle (e.g. `/v1/messages/count_tokens`, `/v1/models`). Masks
/// the request body for prompt-bearing endpoints, forwards everything else
/// verbatim, so tools that call auxiliary endpoints never break.
async fn handle_passthrough(
    State(st): State<Arc<AppState>>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let path_and_query = uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| path.clone());

    let provider = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(crate::upstream::classify)
        .unwrap_or(crate::upstream::Provider::Unknown);
    let anthropic_shaped = path.starts_with("/v1/messages")
        || headers.contains_key("x-api-key")
        || headers.contains_key("anthropic-version");
    let (base, format) = match provider {
        crate::upstream::Provider::Anthropic => (&st.anthropic_upstream, ApiFormat::Anthropic),
        crate::upstream::Provider::OpenAI => (&st.upstream, ApiFormat::OpenAI),
        crate::upstream::Provider::Unknown if anthropic_shaped => {
            (&st.anthropic_upstream, ApiFormat::Anthropic)
        }
        crate::upstream::Provider::Unknown => (&st.upstream, ApiFormat::OpenAI),
    };

    let method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::POST);

    let mut out_body = body.clone();
    if passthrough_should_mask(&path) {
        st.stats.record_request();
        if pack_runtime_blocked(&st) {
            return Response::builder()
                .status(503)
                .body(Body::from(
                    "deeCtx: failClosed enforcement — masking cannot be guaranteed",
                ))
                .unwrap();
        }
        if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&body) {
            let session = session_id(&json);
            let mut events = Vec::new();
            out_body = Bytes::from(mask_or_forward_unmasked(
                &st,
                &session,
                &mut events,
                &mut json,
                &body,
            ));
            // Record the count_tokens masking to the hash-only ledger, like a
            // completion, so `deectx audit` counts it.
            let tool = headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();
            let entry = crate::ledger::LedgerEntry {
                ts: chrono::Utc::now(),
                tool,
                session,
                events,
                latency_ms: 0,
                packs: st.runtime.read().unwrap().packs.clone(),
            };
            if let Err(e) = st.ledger.append(&entry) {
                tracing::warn!("ledger append failed: {e}");
            }
        }
    }

    let was_masked = out_body != body;
    // session=None: passthrough responses are returned verbatim (count_tokens
    // returns a count; other endpoints carry no placeholders to rehydrate).
    let resp = forward(
        &st,
        base,
        method.clone(),
        &path_and_query,
        headers.clone(),
        out_body,
        None,
        format,
        false,
    )
    .await;
    // See handle_completion: a masked request upstream rejects with 400 is
    // treated as a masking bug, not a bad client request — retry unmasked and
    // log it as an error rather than breaking the tool's call.
    if was_masked && resp.status() == StatusCode::BAD_REQUEST {
        tracing::error!(
            "upstream rejected a masked passthrough request with 400; retrying unmasked so \
             traffic keeps flowing — this most likely means a masking bug corrupted the request"
        );
        st.stats.record_error();
        return forward(
            &st,
            base,
            method,
            &path_and_query,
            headers,
            body,
            None,
            format,
            false,
        )
        .await;
    }
    resp
}

/// Run `mask_walk` and re-serialize, but never let a masking bug block or
/// corrupt a user's request. On a panic or serialization failure, discard
/// whatever partial masking happened, record it to `/stats` `errors` (so
/// `deectx status`/`audit` surface it instead of it silently biting the next
/// user), and forward the original body unmasked — a seamless session beats a
/// hard failure or a corrupted upstream call over a bug in this proxy.
pub(crate) fn mask_or_forward_unmasked(
    st: &AppState,
    session: &str,
    events: &mut Vec<LedgerEvent>,
    json: &mut serde_json::Value,
    body: &Bytes,
) -> Vec<u8> {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mask_walk(st, session, events, json);
        serde_json::to_vec(json)
    }));
    match outcome {
        Ok(Ok(bytes)) => bytes,
        _ => {
            events.clear();
            st.stats.record_error();
            tracing::warn!(
                "masking failed for this request; forwarding it unmasked rather than \
                 blocking the session or sending a corrupted request upstream"
            );
            body.to_vec()
        }
    }
}

pub(crate) fn mask_walk(
    st: &AppState,
    session: &str,
    events: &mut Vec<LedgerEvent>,
    value: &mut serde_json::Value,
) -> bool {
    match value {
        serde_json::Value::String(s) => {
            let mut changed = false;
            // Tool arguments are JSON-encoded strings (OpenAI function.arguments);
            // recurse into the decoded value so nested PII is caught too.
            if let Ok(mut inner) = serde_json::from_str::<serde_json::Value>(s) {
                // Only rebuild via a whole-Value round-trip when that is
                // byte-preserving for unmasked siblings (e.g. no float `1.50` ->
                // `1.5` drift, no whitespace collapsing). Otherwise fall back to
                // masking the raw string bytes so every unaffected node keeps its
                // exact original text and a masked sibling can't corrupt numbers.
                #[allow(clippy::cmp_owned)]
                let byte_preserving = inner.to_string() == *s;
                if byte_preserving && mask_walk(st, session, events, &mut inner) {
                    *s = inner.to_string();
                    changed = true;
                }
            }
            if !changed {
                if let Some(masked) = mask_content(st, session, s, events) {
                    *s = masked;
                    changed = true;
                }
            }
            changed
        }
        serde_json::Value::Array(arr) => {
            let mut changed = false;
            for item in arr.iter_mut() {
                changed |= mask_walk(st, session, events, item);
            }
            changed
        }
        serde_json::Value::Object(obj) => {
            // tool_use / tool_call blocks carry a protocol id (e.g. Anthropic
            // `toolu_...`, OpenAI `call_...`) alongside their `type`. These are
            // high-entropy strings that the secrets detector happily flags as
            // an api_key, but masking them corrupts the id — upstream APIs
            // require it to match `^[a-zA-Z0-9_-]+$` and reject the request
            // with a 400 otherwise. Skip id fields, never their content.
            let owns_tool_id = matches!(
                obj.get("type").and_then(|t| t.as_str()),
                Some("tool_use") | Some("tool_result") | Some("function")
            );
            // Extended-thinking blocks carry a `signature` cryptographically bound
            // to the exact `thinking` text (and `redacted_thinking` blocks carry
            // opaque `data`). The secrets detector flags these high-entropy fields
            // as an api_key the same way it does tool_use ids; masking either the
            // signature or the thinking text it covers invalidates the signature,
            // and upstream rejects the whole request with a 400. Skip the block
            // entirely rather than trying to pick individual safe fields out of it.
            let block_type = obj.get("type").and_then(|t| t.as_str());
            if is_opaque_content_block(block_type) {
                return false;
            }
            let mut changed = false;
            for (k, v) in obj.iter_mut() {
                if is_protocol_id_field(k, owns_tool_id) {
                    continue;
                }
                changed |= mask_walk(st, session, events, v);
            }
            changed
        }
        _ => false,
    }
}

/// True when a content block's `type` marks it as carrying opaque data that
/// must never be walked into and masked as text.
///
/// - `thinking`/`redacted_thinking`: the `signature`/`data` field is
///   cryptographically bound to the exact block content; any edit (including
///   masking) invalidates it and upstream rejects the whole request.
/// - `image`/`document` (Anthropic) and `image_url`/`input_audio` (OpenAI):
///   the payload is base64 — exactly what the entropy-based secrets detector
///   is tuned to catch, so every ~21-char run of it reads as a plausible API
///   key and gets redacted piecemeal, corrupting the payload the same way.
///
/// None of these block types carry human-authored text worth masking, so
/// the whole block is skipped rather than picking individual safe fields
/// out of it.
fn is_opaque_content_block(block_type: Option<&str>) -> bool {
    matches!(
        block_type,
        Some("thinking")
            | Some("redacted_thinking")
            | Some("image")
            | Some("document")
            | Some("image_url")
            | Some("input_audio")
    )
}

/// True when `key` in this object holds a tool_use/tool_call protocol id
/// rather than free-text content. `tool_use_id`/`tool_call_id`/`call_id` are
/// unambiguous field names; bare `id` is only excluded when the enclosing
/// object is itself a tool_use/tool_result/function block (Anthropic content
/// blocks and OpenAI `tool_calls[]` entries), so an arbitrary tool argument
/// schema that happens to use the key `id` is still masked normally.
fn is_protocol_id_field(key: &str, owns_tool_id: bool) -> bool {
    match key {
        "tool_use_id" | "tool_call_id" | "call_id" => true,
        "id" => owns_tool_id,
        _ => false,
    }
}

/// True when failClosed is set for the active pack set and the detector chain
/// (e.g. an NER model) isn't ready — masking can't be guaranteed, so the
/// request must be refused rather than forwarded unmasked.
pub(crate) fn pack_runtime_blocked(st: &AppState) -> bool {
    let rt = st.runtime.read().unwrap();
    rt.fail_closed && !rt.chain.ready()
}

pub(crate) fn mask_content(
    st: &AppState,
    session: &str,
    content: &str,
    events: &mut Vec<LedgerEvent>,
) -> Option<String> {
    let rt = st.runtime.read().unwrap();
    let spans = rt.allowlist.filter(rt.chain.detect(content));
    if spans.is_empty() {
        return None;
    }
    let masked = st.masker.mask_text(session, content, &spans);
    for s in &spans {
        st.stats.record_event(
            if matches!(s.action, Action::Mask) {
                "mask"
            } else {
                "redact"
            },
            s.alert,
        );
        if s.alert {
            tracing::warn!(
                "deeCtx ALERT: '{}' entity detected (masked/redacted; see ledger)",
                s.entity
            );
        }
        let ph = match s.action {
            Action::Mask => st.masker.placeholder_for(session, &s.text),
            Action::Redact => Some("[REDACTED_SECRET]".to_string()),
        };
        events.push(LedgerEvent {
            entity: s.entity.clone(),
            placeholder: ph.clone(),
            ph_hash: ph.as_ref().map(|p| sha256_hex(p)),
            action: if matches!(s.action, Action::Mask) {
                "mask".into()
            } else {
                "redact".into()
            },
            alert: s.alert,
        });
    }
    Some(masked)
}

/// Whether an unmatched, prompt-bearing endpoint's request body must be masked
/// before forwarding. `/v1/messages/count_tokens` carries the same `messages`
/// array as a completion, so it is masked here.
///
/// KNOWN LIMITATION: other prompt-bearing endpoints the fallback forwards —
/// `/v1/messages/batches`, legacy `/v1/completions`, `/v1/embeddings` — are
/// currently forwarded VERBATIM (their bodies are not masked, and their later
/// results can't be rehydrated by this stateless proxy). Non-prompt endpoints
/// (`/v1/models`, files) carry no maskable content.
pub(crate) fn passthrough_should_mask(path: &str) -> bool {
    path.ends_with("/messages/count_tokens")
}

async fn forward_raw(
    st: &AppState,
    format: ApiFormat,
    headers: HeaderMap,
    body: Bytes,
    session: Option<&str>,
    stream: bool,
) -> Response {
    let provider = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(crate::upstream::classify)
        .unwrap_or(crate::upstream::Provider::Unknown);
    let base = match (provider, format) {
        (crate::upstream::Provider::Anthropic, _) => &st.anthropic_upstream,
        (crate::upstream::Provider::OpenAI, _) => &st.upstream,
        (crate::upstream::Provider::Unknown, ApiFormat::Anthropic) => &st.anthropic_upstream,
        (crate::upstream::Provider::Unknown, _) => &st.upstream,
    };
    let path = match format {
        ApiFormat::OpenAI => "/v1/chat/completions",
        ApiFormat::Anthropic => "/v1/messages",
    };
    forward(
        st,
        base,
        reqwest::Method::POST,
        path,
        headers,
        body,
        session,
        format,
        stream,
    )
    .await
}

/// Method-agnostic reverse-proxy forward. `base` is the upstream origin,
/// `path_and_query` the path (optionally with `?query`). When `session` is
/// `Some`, non-stream/stream responses are rehydrated for `format`; when
/// `None`, the response is returned verbatim (transparent passthrough).
#[allow(clippy::too_many_arguments)]
async fn forward(
    st: &AppState,
    base: &str,
    method: reqwest::Method,
    path_and_query: &str,
    headers: HeaderMap,
    body: Bytes,
    session: Option<&str>,
    format: ApiFormat,
    stream: bool,
) -> Response {
    let url = format!("{}{}", base.trim_end_matches('/'), path_and_query);
    let mut req = st.http.request(method, url).body(body);
    for (k, v) in headers.iter() {
        let name = k.as_str();
        if name != "host" && name != "content-length" && name != "accept-encoding" {
            req = req.header(k, v);
        }
    }
    match req.send().await {
        Ok(up) => {
            let status = up.status();
            let up_headers = up.headers().clone();
            let mut builder = Response::builder().status(status);
            for (k, v) in &up_headers {
                let name = k.as_str();
                if name != "content-length" && name != "transfer-encoding" {
                    builder = builder.header(k, v);
                }
            }
            if stream {
                if is_gzip_like_headers(&up_headers) {
                    let raw = up.bytes_stream().map(|r| r.map_err(std::io::Error::other));
                    builder.body(Body::from_stream(raw)).unwrap()
                } else {
                    let masker = st.masker.clone();
                    let sess = session.unwrap_or("").to_string();
                    let reh = Arc::new(Mutex::new(SseRehydrator::new(64)));
                    let reh2 = reh.clone();
                    let masker2 = masker.clone();
                    let sess2 = sess.clone();
                    let byte_stream = up.bytes_stream().map(|r| r.map_err(std::io::Error::other));
                    let rehydrated = byte_stream
                        .scan((), move |_, chunk| {
                            futures_util::future::ready(match chunk {
                                Ok(bytes) => Some(Ok(Bytes::from(
                                    reh.lock()
                                        .unwrap_or_else(|p| p.into_inner())
                                        .push(&bytes, &sess, &masker),
                                ))),
                                Err(e) => Some(Err(e)),
                            })
                        })
                        .chain(futures_util::stream::once(futures_util::future::ready(Ok(
                            Bytes::from(
                                reh2.lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .finish(&sess2, &masker2),
                            ),
                        ))));
                    builder.body(Body::from_stream(rehydrated)).unwrap()
                }
            } else {
                match up.bytes().await {
                    Ok(bytes) => {
                        let body_bytes = if let Some(sess) = session {
                            if is_gzip_like(&bytes, &up_headers) {
                                bytes.to_vec()
                            } else {
                                rehydrate_response(st, sess, &bytes, format)
                            }
                        } else {
                            bytes.to_vec()
                        };
                        builder.body(Body::from(body_bytes)).unwrap()
                    }
                    Err(e) => {
                        tracing::warn!("upstream read error: {e}");
                        builder
                            .status(502)
                            .body(Body::from("upstream read error"))
                            .unwrap()
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!("upstream error: {e}");
            Response::builder()
                .status(502)
                .body(Body::from("upstream error"))
                .unwrap()
        }
    }
}

/// True when a content-encoding other than identity is declared. Used by the
/// streaming path where the body can't be inspected upfront.
fn is_gzip_like_headers(headers: &HeaderMap) -> bool {
    let ce = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    !ce.is_empty() && ce != "identity"
}

/// Rehydration only makes sense for UTF-8 text bodies. Gzip/compressed or
/// binary bodies are forwarded verbatim to avoid corrupting them.
fn is_gzip_like(bytes: &[u8], headers: &HeaderMap) -> bool {
    is_gzip_like_headers(headers) || std::str::from_utf8(bytes).is_err()
}

/// Rewrite masked placeholders back to originals in an upstream JSON response.
/// Walks the format-specific fields (OpenAI: choices[].message/delta content and
/// tool_calls; Anthropic: content[].text), then runs a raw placeholder->original
/// pass over the re-serialized JSON. If the body is not valid JSON, falls back to
/// a raw placeholder->original replace so plain-text or partially-broken streams
/// are still best-effort rehydrated.
pub(crate) fn rehydrate_response(
    st: &AppState,
    session: &str,
    bytes: &[u8],
    format: ApiFormat,
) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) {
        match format {
            ApiFormat::OpenAI => {
                if let Some(choices) = json["choices"].as_array_mut() {
                    for c in choices {
                        if let Some(msg) = c["message"].as_object_mut() {
                            if let Some(txt) = msg.get_mut("content") {
                                if let Some(s) = txt.as_str() {
                                    *txt =
                                        serde_json::Value::String(st.masker.rehydrate(session, s));
                                }
                            }
                            if let Some(calls) =
                                msg.get_mut("tool_calls").and_then(|t| t.as_array_mut())
                            {
                                for tc in calls {
                                    if let Some(args) = tc["function"].get_mut("arguments") {
                                        if let Some(s) = args.as_str() {
                                            *args = serde_json::Value::String(
                                                st.masker.rehydrate(session, s),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(delta) = c["delta"].as_object_mut() {
                            if let Some(txt) = delta.get_mut("content") {
                                if let Some(s) = txt.as_str() {
                                    *txt =
                                        serde_json::Value::String(st.masker.rehydrate(session, s));
                                }
                            }
                            if let Some(calls) =
                                delta.get_mut("tool_calls").and_then(|t| t.as_array_mut())
                            {
                                for tc in calls {
                                    if let Some(args) = tc["function"].get_mut("arguments") {
                                        if let Some(s) = args.as_str() {
                                            *args = serde_json::Value::String(
                                                st.masker.rehydrate(session, s),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ApiFormat::Anthropic => {
                if let Some(content) = json["content"].as_array_mut() {
                    for block in content {
                        if let Some(txt) = block.get_mut("text") {
                            if let Some(s) = txt.as_str() {
                                *txt = serde_json::Value::String(st.masker.rehydrate(session, s));
                            }
                        }
                    }
                }
            }
        }
        // Final raw pass over the re-serialized JSON catches tool_use.input etc.
        if let Ok(serialized) = serde_json::to_string(&json) {
            return st.masker.rehydrate(session, &serialized).into_bytes();
        }
        st.masker.rehydrate(session, &text).into_bytes()
    } else {
        st.masker.rehydrate(session, &text).into_bytes()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn count_tokens_is_masked_others_are_not() {
        assert!(passthrough_should_mask("/v1/messages/count_tokens"));
        assert!(!passthrough_should_mask("/v1/models"));
        assert!(!passthrough_should_mask("/v1/messages/batches"));
    }
    use super::*;

    #[test]
    fn gzip_like_headers_guards_streaming_rehydration() {
        let mut h = HeaderMap::new();
        assert!(
            !is_gzip_like_headers(&h),
            "absent content-encoding is not gzip"
        );
        h.insert(
            "content-encoding",
            axum::http::HeaderValue::from_static("identity"),
        );
        assert!(!is_gzip_like_headers(&h), "identity is not gzip");
        h.insert(
            "content-encoding",
            axum::http::HeaderValue::from_static("gzip"),
        );
        assert!(is_gzip_like_headers(&h), "gzip is gzip");
        h.insert(
            "content-encoding",
            axum::http::HeaderValue::from_static("br"),
        );
        assert!(is_gzip_like_headers(&h), "br is gzip-like");
    }

    #[test]
    fn protocol_id_fields_are_never_masked() {
        assert!(is_protocol_id_field("tool_use_id", false));
        assert!(is_protocol_id_field("tool_call_id", false));
        assert!(is_protocol_id_field("call_id", false));
        assert!(
            is_protocol_id_field("id", true),
            "bare `id` is excluded only inside a tool_use/tool_result/function block"
        );
        assert!(
            !is_protocol_id_field("id", false),
            "bare `id` elsewhere (e.g. a user-defined tool argument) must still be masked"
        );
        assert!(!is_protocol_id_field("email", true));
    }

    #[test]
    fn opaque_content_blocks_are_never_masked() {
        for t in [
            "thinking",
            "redacted_thinking",
            "image",
            "document",
            "image_url",
            "input_audio",
        ] {
            assert!(
                is_opaque_content_block(Some(t)),
                "{t} must be treated as opaque"
            );
        }
        assert!(!is_opaque_content_block(Some("text")));
        assert!(!is_opaque_content_block(Some("tool_use")));
        assert!(!is_opaque_content_block(None));
    }

    fn test_app_state() -> AppState {
        let cfg = crate::config::Config {
            ledger_path: std::env::temp_dir()
                .join(format!("deectx_test_ledger_{}.jsonl", std::process::id())),
            ..crate::config::Config::default()
        };
        let runtime = build_pack_runtime(&cfg);
        AppState {
            upstream: cfg.upstream.clone(),
            anthropic_upstream: "https://api.anthropic.com".to_string(),
            upstream_responses: cfg.upstream_responses.clone(),
            runtime: RwLock::new(runtime),
            masker: std::sync::Arc::new(Masker::new()),
            ledger: Ledger::new(cfg.ledger_path.clone(), cfg.ledger_retention_days).unwrap(),
            http: reqwest::Client::new(),
            cfg,
            stats: std::sync::Arc::new(LiveStats::new()),
        }
    }

    #[test]
    fn base64_image_payload_survives_mask_walk_untouched() {
        // A base64 image payload is exactly the shape the entropy-based
        // secrets detector flags as an api_key; mask_walk must leave the
        // whole image block alone rather than redacting chunks of it.
        let st = test_app_state();
        let base64_payload = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
        let mut json = serde_json::json!({
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "image",
                    "source": { "type": "base64", "media_type": "image/png", "data": base64_payload }
                }]
            }]
        });
        let mut events = Vec::new();
        mask_walk(&st, "s_test", &mut events, &mut json);
        assert_eq!(
            json["messages"][0]["content"][0]["source"]["data"], base64_payload,
            "image payload must be byte-identical after mask_walk"
        );
        assert!(
            events.is_empty(),
            "no masking events should fire on an opaque image block"
        );
    }

    #[test]
    fn session_id_is_distinct_for_array_first_messages() {
        let a = serde_json::json!({"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]});
        let b = serde_json::json!({"messages":[{"role":"user","content":[{"type":"text","text":"world"}]}]});
        assert_ne!(session_id(&a), session_id(&b));
        assert!(session_id(&a).starts_with("s_"));
    }
}
