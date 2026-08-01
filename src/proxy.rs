use crate::config::Config;
use crate::detect::DetectorChain;
use crate::ledger::{sha256_hex, Ledger, LedgerEntry, LedgerEvent};
use crate::masker::Masker;
use crate::packs;
use crate::span::Action;
use anyhow::Result;
use axum::{body::Bytes, extract::State, http::HeaderMap, response::Response, routing::post, Router};
use chrono::Utc;
use std::sync::Arc;
use tokio::net::TcpListener;

struct AppState {
    upstream: String,
    chain: DetectorChain,
    masker: Masker,
    ledger: Ledger,
    http: reqwest::Client,
    packs: Vec<String>,
}

pub async fn run_proxy(cfg: Config) -> Result<()> {
    let listener = TcpListener::bind(&cfg.listen).await?;
    tracing::info!("deectx listening on {}", cfg.listen);
    serve_with_listener(cfg, listener).await
}

pub async fn serve_with_listener(cfg: Config, listener: TcpListener) -> Result<()> {
    let packs = packs::load_active(&cfg);
    let pack_names: Vec<String> = packs.iter().map(|p| p.name.clone()).collect();
    let state = Arc::new(AppState {
        upstream: cfg.upstream.trim_end_matches('/').to_string(),
        chain: packs::build_chain(&packs),
        masker: Masker::new(),
        ledger: Ledger::new(cfg.ledger_path)?,
        http: reqwest::Client::new(),
        packs: pack_names,
    });
    let app = Router::new().route("/v1/chat/completions", post(handle_chat)).with_state(state);
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

async fn handle_chat(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let start = std::time::Instant::now();
    let mut json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return forward_raw(&*st, &headers, body, None).await,
    };
    let session = session_id(&json);
    let mut events = Vec::new();

    let mask_content = |content: &str, session: &str, st: &AppState, events: &mut Vec<LedgerEvent>| -> Option<String> {
        let spans = st.chain.detect(content);
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
                action: match s.action { Action::Mask => "mask", Action::Redact => "redact" }.into(),
            });
        }
        Some(masked)
    };

    if let Some(messages) = json["messages"].as_array_mut() {
        for msg in messages {
            if let Some(content) = msg["content"].as_str() {
                if let Some(masked) = mask_content(content, &session, &st, &mut events) {
                    msg["content"] = serde_json::Value::String(masked);
                }
            } else if let Some(parts) = msg["content"].as_array_mut() {
                for part in parts {
                    if let Some(text) = part["text"].as_str() {
                        if let Some(masked) = mask_content(text, &session, &st, &mut events) {
                            part["text"] = serde_json::Value::String(masked);
                        }
                    }
                }
            }
        }
    }

    let entry = LedgerEntry {
        ts: Utc::now(),
        tool: headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("unknown").to_string(),
        session: session.clone(),
        events,
        latency_ms: start.elapsed().as_millis(),
        packs: st.packs.clone(),
    };
    if let Err(e) = st.ledger.append(&entry) {
        tracing::warn!("ledger append failed: {e}");
    }

    let out = serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec());
    forward_raw(&*st, &headers, Bytes::from(out), Some(&session)).await
}

async fn forward_raw(st: &AppState, headers: &HeaderMap, body: Bytes, session: Option<&str>) -> Response {
    let url = format!("{}/v1/chat/completions", st.upstream);
    let mut req = st.http.post(&url).body(body);
    for (k, v) in headers {
        if k != "host" && k != "content-length" {
            req = req.header(k, v);
        }
    }
    match req.send().await {
        Ok(up) => {
            let up_headers = up.headers().clone();
            let status = up.status();
            let bytes = match up.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("upstream body read error: {e}");
                    return Response::builder().status(502)
                        .body(axum::body::Body::from("upstream read error"))
                        .unwrap();
                }
            };
            let mut resp = Response::builder().status(status);
            for (k, v) in &up_headers {
                if k != "content-length" && k != "transfer-encoding" {
                    resp = resp.header(k, v);
                }
            }
            let body: Vec<u8> = match session {
                Some(sess) if !is_gzip_like(&bytes, &up_headers) => rehydrate_response(st, sess, &bytes),
                _ => bytes.to_vec(),
            };
            resp.body(axum::body::Body::from(body)).unwrap()
        }
        Err(e) => {
            tracing::warn!("upstream error: {e}");
            Response::builder().status(502).body(axum::body::Body::from("upstream error")).unwrap()
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
/// Walks choices[].message.content, choices[].message.tool_calls,
/// choices[].delta.content and choices[].delta.tool_calls, then runs a raw
/// placeholder->original pass over the re-serialized JSON. If the body is not
/// valid JSON, falls back to a raw placeholder->original replace so plain-text
/// or partially-broken streams are still best-effort rehydrated.
fn rehydrate_response(st: &AppState, session: &str, bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(mut v) => {
            if let Some(choices) = v["choices"].as_array_mut() {
                for c in choices {
                    if let Some(content) = c["message"]["content"].as_str() {
                        c["message"]["content"] =
                            serde_json::Value::String(st.masker.rehydrate(session, content));
                    }
                    if let Some(tool_calls) = c["message"]["tool_calls"].as_array_mut() {
                        for tc in tool_calls {
                            if let Some(args) = tc["function"]["arguments"].as_str() {
                                tc["function"]["arguments"] =
                                    serde_json::Value::String(st.masker.rehydrate(session, args));
                            }
                        }
                    }
                    if let Some(content) = c["delta"]["content"].as_str() {
                        c["delta"]["content"] =
                            serde_json::Value::String(st.masker.rehydrate(session, content));
                    }
                    if let Some(tool_calls) = c["delta"]["tool_calls"].as_array_mut() {
                        for tc in tool_calls {
                            if let Some(args) = tc["function"]["arguments"].as_str() {
                                tc["function"]["arguments"] =
                                    serde_json::Value::String(st.masker.rehydrate(session, args));
                            }
                        }
                    }
                }
            }
            match serde_json::to_vec(&v) {
                Ok(serialized) => {
                    let serialized = String::from_utf8_lossy(&serialized);
                    st.masker.rehydrate(session, &serialized).into_bytes()
                }
                Err(_) => bytes.to_vec(),
            }
        }
        Err(_) => st.masker.rehydrate(session, &text).into_bytes(),
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
