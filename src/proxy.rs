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
        Err(_) => return forward_raw(&*st, &headers, body).await,
    };
    let session = session_id(&json);
    let mut events = Vec::new();

    if let Some(messages) = json["messages"].as_array_mut() {
        for msg in messages {
            if let Some(content) = msg["content"].as_str() {
                let spans = st.chain.detect(content);
                if !spans.is_empty() {
                    for s in &spans {
                        events.push(LedgerEvent {
                            entity: s.entity.clone(),
                            placeholder: None,
                            ph_hash: Some(sha256_hex(&format!("{}:{}", session, s.entity))),
                            action: match s.action { Action::Mask => "mask", Action::Redact => "redact" }.into(),
                        });
                    }
                    let masked = st.masker.mask_text(&session, content, &spans);
                    msg["content"] = serde_json::Value::String(masked);
                }
            }
        }
    }

    let entry = LedgerEntry {
        ts: Utc::now(),
        tool: headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("unknown").to_string(),
        session,
        events,
        latency_ms: start.elapsed().as_millis(),
    };
    if let Err(e) = st.ledger.append(&entry) {
        tracing::warn!("ledger append failed: {e}");
    }

    let out = serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec());
    forward_raw(&*st, &headers, Bytes::from(out)).await
}

async fn forward_raw(st: &AppState, headers: &HeaderMap, body: Bytes) -> Response {
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
                resp = resp.header(k, v);
            }
            let bytes = up.bytes().await.unwrap_or_default();
            resp.body(axum::body::Body::from(bytes)).unwrap()
        }
        Err(e) => {
            tracing::warn!("upstream error: {e}");
            Response::builder().status(502).body(axum::body::Body::from("upstream error")).unwrap()
        }
    }
}
