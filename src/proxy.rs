use crate::config::Config;
use crate::detect::DetectorChain;
use crate::ledger::{sha256_hex, Ledger, LedgerEntry, LedgerEvent};
use crate::masker::Masker;
use crate::packs;
use crate::span::Action;
use anyhow::Result;
use axum::body::Body;
use axum::{body::Bytes, extract::State, http::HeaderMap, response::Response, Router};
use chrono::Utc;
use std::sync::Arc;
use tokio::net::TcpListener;

struct AppState {
    upstream: String,
    anthropic_upstream: String,
    chain: DetectorChain,
    masker: Masker,
    ledger: Ledger,
    http: reqwest::Client,
    packs: Vec<String>,
    allowlist: crate::allowlist::Allowlist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiFormat {
    OpenAI,
    Anthropic,
}

pub async fn run_proxy(cfg: Config) -> Result<()> {
    let listener = TcpListener::bind(&cfg.listen).await?;
    tracing::info!("deectx listening on {}", cfg.listen);
    serve_with_listener(cfg, listener).await
}

pub async fn serve_with_listener(cfg: Config, listener: TcpListener) -> Result<()> {
    let packs = packs::load_active(&cfg);
    let pack_names: Vec<String> = packs.iter().map(|p| p.name.clone()).collect();
    let allowlist = crate::allowlist::Allowlist::new(packs::allow_entries(&cfg, &packs));
    let anthropic_upstream = cfg
        .upstream_anthropic
        .clone()
        .unwrap_or_else(|| "https://api.anthropic.com".to_string());
    let state = Arc::new(AppState {
        upstream: cfg.upstream.trim_end_matches('/').to_string(),
        anthropic_upstream,
        chain: packs::build_chain(&packs, cfg.ner, cfg.model_dir.clone().unwrap_or_else(|| std::path::PathBuf::from("./models"))),
        masker: Masker::new(),
        ledger: Ledger::new(cfg.ledger_path)?,
        http: reqwest::Client::new(),
        packs: pack_names,
        allowlist,
    });
    let app = Router::new()
        .route("/v1/chat/completions", axum::routing::post(handle_chat_openai))
        .route("/v1/messages", axum::routing::post(handle_chat_anthropic))
        .with_state(state);
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
    let mut events = Vec::new();
    match format {
        ApiFormat::OpenAI => {
            if let Some(msgs) = json["messages"].as_array_mut() {
                for m in msgs {
                    if let Some(c) = m.get_mut("content") {
                        mask_walk(&st, &session, &mut events, c);
                    }
                }
            }
        }
        ApiFormat::Anthropic => {
            if let Some(sys) = json.get_mut("system") {
                mask_walk(&st, &session, &mut events, sys);
            }
            if let Some(msgs) = json["messages"].as_array_mut() {
                for m in msgs {
                    if let Some(c) = m.get_mut("content") {
                        mask_walk(&st, &session, &mut events, c);
                    }
                }
            }
        }
    }
    let stream = json.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let out = serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec());
    let resp = forward_raw(&st, format, headers, Bytes::from(out), Some(&session), stream).await;
    // Streaming latency is measured in Task 3; non-stream requests buffer the
    // full body in forward_raw, so elapsed() here is the true request latency.
    let latency_ms = if stream { 0 } else { start.elapsed().as_millis() };
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

fn mask_walk(
    st: &AppState,
    session: &str,
    events: &mut Vec<LedgerEvent>,
    value: &mut serde_json::Value,
) -> bool {
    let mut changed = false;
    if let Some(s) = value.as_str() {
        if let Some(masked) = mask_content(st, session, s, events) {
            *value = serde_json::Value::String(masked);
            changed = true;
        }
    } else if let Some(arr) = value.as_array_mut() {
        for item in arr.iter_mut() {
            if let Some(obj) = item.as_object_mut() {
                if let Some(txt) = obj.get_mut("text") {
                    changed |= mask_walk(st, session, events, txt);
                }
            }
        }
    }
    changed
}

fn mask_content(
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
        let ph = match s.action {
            Action::Mask => st.masker.placeholder_for(session, &s.text),
            Action::Redact => Some("[REDACTED_SECRET]".to_string()),
        };
        events.push(LedgerEvent {
            entity: s.entity.clone(),
            placeholder: ph.clone(),
            ph_hash: ph.as_ref().map(|p| sha256_hex(p)),
            action: if matches!(s.action, Action::Mask) { "mask".into() } else { "redact".into() },
        });
    }
    Some(masked)
}

async fn forward_raw(
    st: &AppState,
    format: ApiFormat,
    headers: HeaderMap,
    body: Bytes,
    session: Option<&str>,
    _stream: bool,
) -> Response {
    let base = match format {
        ApiFormat::OpenAI => &st.upstream,
        ApiFormat::Anthropic => &st.anthropic_upstream,
    };
    let path = match format {
        ApiFormat::OpenAI => "/v1/chat/completions",
        ApiFormat::Anthropic => "/v1/messages",
    };
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let mut req = st.http.post(url).body(body);
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
            // NOTE: `stream` handling lands in Task 3. For now, always buffer.
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
                    builder.status(502).body(Body::from("upstream read error")).unwrap()
                }
            }
        }
        Err(e) => {
            tracing::warn!("upstream error: {e}");
            Response::builder().status(502).body(Body::from("upstream error")).unwrap()
        }
    }
}

/// Rehydration only makes sense for UTF-8 text bodies. Gzip/compressed or
/// binary bodies are forwarded verbatim to avoid corrupting them.
fn is_gzip_like(bytes: &[u8], headers: &HeaderMap) -> bool {
    let ce = headers
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    ce != "" && ce != "identity" || std::str::from_utf8(bytes).is_err()
}

/// Rewrite masked placeholders back to originals in an upstream JSON response.
/// Walks the format-specific fields (OpenAI: choices[].message/delta content and
/// tool_calls; Anthropic: content[].text), then runs a raw placeholder->original
/// pass over the re-serialized JSON. If the body is not valid JSON, falls back to
/// a raw placeholder->original replace so plain-text or partially-broken streams
/// are still best-effort rehydrated.
fn rehydrate_response(st: &AppState, session: &str, bytes: &[u8], format: ApiFormat) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) {
        match format {
            ApiFormat::OpenAI => {
                if let Some(choices) = json["choices"].as_array_mut() {
                    for c in choices {
                        if let Some(msg) = c["message"].as_object_mut() {
                            if let Some(txt) = msg.get_mut("content") {
                                if let Some(s) = txt.as_str() {
                                    *txt = serde_json::Value::String(st.masker.rehydrate(session, s));
                                }
                            }
                            if let Some(calls) = msg.get_mut("tool_calls").and_then(|t| t.as_array_mut()) {
                                for tc in calls {
                                    if let Some(args) = tc["function"].get_mut("arguments") {
                                        if let Some(s) = args.as_str() {
                                            *args = serde_json::Value::String(st.masker.rehydrate(session, s));
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(delta) = c["delta"].as_object_mut() {
                            if let Some(txt) = delta.get_mut("content") {
                                if let Some(s) = txt.as_str() {
                                    *txt = serde_json::Value::String(st.masker.rehydrate(session, s));
                                }
                            }
                            if let Some(calls) = delta.get_mut("tool_calls").and_then(|t| t.as_array_mut()) {
                                for tc in calls {
                                    if let Some(args) = tc["function"].get_mut("arguments") {
                                        if let Some(s) = args.as_str() {
                                            *args = serde_json::Value::String(st.masker.rehydrate(session, s));
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
    use super::*;

    #[test]
    fn session_id_is_distinct_for_array_first_messages() {
        let a = serde_json::json!({"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]});
        let b = serde_json::json!({"messages":[{"role":"user","content":[{"type":"text","text":"world"}]}]});
        assert_ne!(session_id(&a), session_id(&b));
        assert!(session_id(&a).starts_with("s_"));
    }
}
