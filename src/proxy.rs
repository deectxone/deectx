use crate::config::Config;
use crate::detect::{regex::RegexDetector, secrets::SecretsDetector, DetectorChain};
use crate::ledger::{sha256_hex, Ledger, LedgerEntry, LedgerEvent};
use crate::masker::Masker;
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
}

pub async fn run_proxy(cfg: Config) -> Result<()> {
    let listener = TcpListener::bind(&cfg.listen).await?;
    tracing::info!("deectx listening on {}", cfg.listen);
    serve_with_listener(cfg, listener).await
}

pub async fn serve_with_listener(cfg: Config, listener: TcpListener) -> Result<()> {
    let state = Arc::new(AppState {
        upstream: cfg.upstream.trim_end_matches('/').to_string(),
        chain: DetectorChain::new(vec![Box::new(RegexDetector::new()), Box::new(SecretsDetector::new())]),
        masker: Masker::new(),
        ledger: Ledger::new(cfg.ledger_path)?,
        http: reqwest::Client::new(),
    });
    let app = Router::new().route("/v1/chat/completions", post(handle_chat)).with_state(state);
    axum::serve(listener, app).await?;
    Ok(())
}

fn session_id(body: &serde_json::Value) -> String {
    let first = body["messages"][0]["content"].as_str().unwrap_or("");
    format!("s_{}", &sha256_hex(first)[..8])
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
            let mut resp = Response::builder().status(up.status());
            for (k, v) in up.headers() {
                if k != "content-length" {
                    resp = resp.header(k, v);
                }
            }
            let bytes = up.bytes().await.unwrap_or_default();
            let body: Vec<u8> = match session {
                Some(sess) => rehydrate_response(st, sess, &bytes),
                None => bytes.to_vec(),
            };
            resp.body(axum::body::Body::from(body)).unwrap()
        }
        Err(e) => {
            tracing::warn!("upstream error: {e}");
            Response::builder().status(502).body(axum::body::Body::from("upstream error")).unwrap()
        }
    }
}

/// Rewrite masked placeholders back to originals in an upstream JSON response.
/// Walks choices[].message.content and choices[].delta.content. If the body is
/// not valid JSON, falls back to a raw placeholder->original replace so
/// plain-text / partially-broken streams are still best-effort rehydrated.
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
                    if let Some(content) = c["delta"]["content"].as_str() {
                        c["delta"]["content"] =
                            serde_json::Value::String(st.masker.rehydrate(session, content));
                    }
                }
            }
            serde_json::to_vec(&v).unwrap_or_else(|_| bytes.to_vec())
        }
        Err(_) => st.masker.rehydrate(session, &text).into_bytes(),
    }
}
