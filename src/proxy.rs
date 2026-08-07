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
use axum::{body::Bytes, extract::State, http::HeaderMap, response::Response, Router};
use chrono::Utc;
use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

pub(crate) struct AppState {
    pub(crate) upstream: String,
    pub(crate) anthropic_upstream: String,
    pub(crate) upstream_responses: String,
    pub(crate) chain: DetectorChain,
    pub(crate) masker: std::sync::Arc<Masker>,
    pub(crate) ledger: Ledger,
    pub(crate) http: reqwest::Client,
    pub(crate) packs: Vec<String>,
    pub(crate) allowlist: crate::allowlist::Allowlist,
    pub(crate) fail_closed: bool,
    pub(crate) stats: std::sync::Arc<LiveStats>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiFormat {
    OpenAI,
    Anthropic,
}

pub async fn run_proxy(cfg: Config) -> Result<()> {
    let listener = TcpListener::bind(&cfg.listen).await?;
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
    let res = serve_with_listener(cfg, listener).await;
    crate::home::Pidfile::clear();
    res
}

pub async fn serve_with_listener(cfg: Config, listener: TcpListener) -> Result<()> {
    let packs = packs::load_active(&cfg);
    let pack_names: Vec<String> = packs.iter().map(|p| p.name.clone()).collect();
    let allowlist = crate::allowlist::Allowlist::new(packs::allow_entries(&cfg, &packs));
    let stats_enabled = cfg.stats_enabled;
    let anthropic_upstream = cfg
        .upstream_anthropic
        .clone()
        .unwrap_or_else(|| "https://api.anthropic.com".to_string());
    let state = Arc::new(AppState {
        upstream: cfg.upstream.trim_end_matches('/').to_string(),
        anthropic_upstream,
        upstream_responses: cfg.upstream_responses.clone(),
        chain: packs::build_chain(
            &packs,
            cfg.ner,
            cfg.model_dir
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("./models")),
        ),
        masker: std::sync::Arc::new(Masker::new()),
        ledger: Ledger::new(cfg.ledger_path, cfg.ledger_retention_days)?,
        http: reqwest::Client::new(),
        packs: pack_names,
        allowlist,
        fail_closed: packs.iter().any(|p| p.settings.fail_closed),
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
        app = app.route("/stats", axum::routing::get(handle_stats));
    }
    let app = app.fallback(handle_passthrough).with_state(state);
    axum::serve(listener, app).await?;
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
    if st.fail_closed && !st.chain.ready() {
        tracing::warn!("failClosed enforcement: a required detector (NER model) is unavailable; refusing request");
        return Response::builder()
            .status(503)
            .body(Body::from(
                "deeCtx: failClosed enforcement — masking cannot be guaranteed",
            ))
            .unwrap();
    }
    let mut events = Vec::new();
    mask_walk(&st, &session, &mut events, &mut json);
    let stream = json
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let out = serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec());
    let resp = forward_raw(
        &st,
        format,
        headers,
        Bytes::from(out),
        Some(&session),
        stream,
    )
    .await;
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
        packs: st.packs.clone(),
    };
    if let Err(e) = st.ledger.append(&entry) {
        tracing::warn!("ledger append failed: {e}");
    }
    resp
}

async fn handle_stats(State(st): State<Arc<AppState>>) -> axum::Json<crate::stats::StatsSnapshot> {
    axum::Json(st.stats.snapshot())
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
        if st.fail_closed && !st.chain.ready() {
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
            mask_walk(&st, &session, &mut events, &mut json);
            if let Ok(v) = serde_json::to_vec(&json) {
                out_body = Bytes::from(v);
            }
        }
    }

    // session=None: passthrough responses are returned verbatim (count_tokens
    // returns a count; other endpoints carry no placeholders to rehydrate).
    forward(
        &st,
        base,
        method,
        &path_and_query,
        headers,
        out_body,
        None,
        format,
        false,
    )
    .await
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
            let mut changed = false;
            for (_, v) in obj.iter_mut() {
                changed |= mask_walk(st, session, events, v);
            }
            changed
        }
        _ => false,
    }
}

pub(crate) fn mask_content(
    st: &AppState,
    session: &str,
    content: &str,
    events: &mut Vec<LedgerEvent>,
) -> Option<String> {
    let spans = st.allowlist.filter(st.chain.detect(content));
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

/// Unmatched endpoints that still carry the full prompt and MUST be masked
/// before forwarding. `count_tokens` sends the same `messages` array as a
/// completion; everything else (models, batches, files) carries no maskable
/// user content and is forwarded verbatim.
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
    fn session_id_is_distinct_for_array_first_messages() {
        let a = serde_json::json!({"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]});
        let b = serde_json::json!({"messages":[{"role":"user","content":[{"type":"text","text":"world"}]}]});
        assert_ne!(session_id(&a), session_id(&b));
        assert!(session_id(&a).starts_with("s_"));
    }
}
