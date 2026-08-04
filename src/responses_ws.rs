use crate::ledger::{LedgerEntry, LedgerEvent};
use crate::masker::Masker;
use crate::proxy::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Monotonic per-connection id so concurrent connections get distinct masking
/// sessions (the Masker's placeholder map is keyed by session string).
static CONN_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, st, headers))
}

async fn handle_socket(mut socket: WebSocket, st: Arc<AppState>, headers: HeaderMap) {
    let session = format!("ws_{}", CONN_ID.fetch_add(1, Ordering::Relaxed));
    st.stats.record_request();

    // Build the upstream request via tungstenite's IntoClientRequest so it
    // generates the WebSocket handshake headers (sec-websocket-key/version),
    // then forward the client's authorization (and user-agent): without the
    // real API key the upstream refuses the upgrade.
    let mut request = match st.upstream_responses.as_str().into_client_request() {
        Ok(req) => req,
        Err(e) => {
            let _ = socket
                .send(Message::Text(error_frame(&format!(
                    "invalid upstream request: {e}"
                ))))
                .await;
            return;
        }
    };
    for name in ["authorization", "user-agent"] {
        if let Some(v) = headers.get(name) {
            if let Ok(v) = v.to_str() {
                if let Ok(hv) = axum::http::HeaderValue::from_str(v) {
                    request.headers_mut().insert(name, hv);
                }
            }
        }
    }
    let mut upstream = match tokio_tungstenite::connect_async(request).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            tracing::warn!("upstream responses connect failed: {e}");
            let _ = socket
                .send(Message::Text(error_frame(&format!(
                    "upstream connect failed: {e}"
                ))))
                .await;
            return;
        }
    };

    let mut events: Vec<LedgerEvent> = Vec::new();

    // Bidirectional pump: client -> upstream is masked; upstream -> client is
    // rehydrated. Each Responses API frame is a complete JSON event object, so
    // masking/rehydration happen per-frame on the JSON values.
    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let out = mask_outbound(&text, &st, &session, &mut events);
                        let _ = upstream
                            .send(tokio_tungstenite::tungstenite::Message::Text(out.into()))
                            .await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            msg = upstream.next() => {
                match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                        let out = rehydrate_inbound(text.as_str(), &st, &session);
                        let _ = socket.send(Message::Text(out)).await;
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    let entry = LedgerEntry {
        ts: Utc::now(),
        tool: "responses-ws".into(),
        session,
        events,
        latency_ms: 0,
        packs: st.packs.clone(),
    };
    if let Err(e) = st.ledger.append(&entry) {
        tracing::warn!("ledger append failed: {e}");
    }
}

fn error_frame(message: &str) -> String {
    serde_json::json!({
        "type": "error",
        "error": { "message": message }
    })
    .to_string()
}

/// Mask a client->upstream Responses API JSON frame. Returns the text to
/// forward: the re-serialized masked JSON when anything changed, otherwise the
/// original bytes untouched (unparseable frames flow through verbatim).
fn mask_outbound(
    text: &str,
    st: &AppState,
    session: &str,
    events: &mut Vec<LedgerEvent>,
) -> String {
    let mut value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return text.to_string(),
    };
    let mut frame_events = Vec::new();
    let changed = crate::proxy::mask_walk(st, session, &mut frame_events, &mut value);
    for ev in &frame_events {
        st.stats.record_event(&ev.action, ev.alert);
    }
    events.extend(frame_events);
    if changed {
        serde_json::to_string(&value).unwrap_or_else(|_| text.to_string())
    } else {
        text.to_string()
    }
}

/// Rehydrate an upstream->client Responses API JSON frame. Each frame is a
/// complete JSON object (e.g. response.output_text.delta), so text-bearing
/// string fields are rehydrated directly via the session Masker and the frame
/// re-serialized only when a placeholder was actually restored. Non-JSON
/// frames get a raw placeholder->original pass.
fn rehydrate_inbound(text: &str, st: &AppState, session: &str) -> String {
    let mut value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return st.masker.rehydrate(session, text),
    };
    if rehydrate_walk(&st.masker, session, &mut value) {
        serde_json::to_string(&value).unwrap_or_else(|_| st.masker.rehydrate(session, text))
    } else {
        text.to_string()
    }
}

/// Walk a JSON value rehydrating every string field. Returns true if any field
/// changed. Rehydration is a no-op on fields without placeholders, so applying
/// it to all string fields is safe regardless of the event type.
fn rehydrate_walk(masker: &Masker, session: &str, value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => {
            let out = masker.rehydrate(session, s);
            if out != *s {
                *s = out;
                true
            } else {
                false
            }
        }
        serde_json::Value::Array(arr) => {
            let mut changed = false;
            for item in arr.iter_mut() {
                changed |= rehydrate_walk(masker, session, item);
            }
            changed
        }
        serde_json::Value::Object(obj) => {
            let mut changed = false;
            for v in obj.values_mut() {
                changed |= rehydrate_walk(masker, session, v);
            }
            changed
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ledger::Ledger;
    use crate::masker::Masker;
    use crate::packs;
    use crate::stats::LiveStats;

    fn test_state() -> Arc<AppState> {
        let cfg = Config {
            ledger_path: std::env::temp_dir()
                .join(format!("deectx_ws_unit_{}.jsonl", std::process::id())),
            ..Default::default()
        };
        let packs = packs::load_active(&cfg);
        let pack_names: Vec<String> = packs.iter().map(|p| p.name.clone()).collect();
        let allowlist = crate::allowlist::Allowlist::new(packs::allow_entries(&cfg, &packs));
        Arc::new(AppState {
            upstream: cfg.upstream.trim_end_matches('/').to_string(),
            anthropic_upstream: cfg
                .upstream_anthropic
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".to_string()),
            upstream_responses: cfg.upstream_responses.clone(),
            chain: packs::build_chain(&packs, false, std::path::PathBuf::from("./models")),
            masker: Arc::new(Masker::new()),
            ledger: Ledger::new(cfg.ledger_path, cfg.ledger_retention_days).unwrap(),
            http: reqwest::Client::new(),
            packs: pack_names,
            allowlist,
            fail_closed: false,
            stats: Arc::new(LiveStats::new()),
        })
    }

    #[test]
    fn masks_email_outbound_and_rehydrates_it_inbound() {
        let st = test_state();
        let session = "ws_unit_1".to_string();
        let mut events = Vec::new();
        let out = mask_outbound(
            r#"{"type":"response.create","input":"my email is jane.doe@example.com"}"#,
            &st,
            &session,
            &mut events,
        );
        assert!(
            out.contains("[EMAIL_1]"),
            "email must be masked outbound: {out}"
        );
        assert!(
            !out.contains("jane.doe@example.com"),
            "raw email must not leave outbound: {out}"
        );
        assert!(events.iter().any(|e| e.entity == "email"));

        let client = rehydrate_inbound(
            r#"{"type":"response.output_text.delta","delta":"sent report to [EMAIL_1]","item_id":"msg_1","output_index":0,"content_index":0}"#,
            &st,
            &session,
        );
        assert!(
            client.contains("jane.doe@example.com"),
            "delta must be rehydrated: {client}"
        );
        assert!(
            !client.contains("[EMAIL_1]"),
            "placeholder must not leak to client: {client}"
        );
    }

    #[test]
    fn rehydrate_walk_restores_placeholders_in_nested_response_item() {
        let st = test_state();
        let session = "ws_unit_2".to_string();
        let mut events = Vec::new();
        mask_outbound(
            r#"{"type":"response.create","input":"notify jane.doe@example.com"}"#,
            &st,
            &session,
            &mut events,
        );
        let mut value = serde_json::json!({
            "type": "response.output_item.done",
            "item": {
                "type": "message",
                "content": [{"type": "output_text", "text": "notified jane.doe@example.com [EMAIL_1]"}]
            }
        });
        assert!(
            rehydrate_walk(&st.masker, &session, &mut value),
            "placeholder must be detected and restored"
        );
        let text = serde_json::to_string(&value).unwrap();
        assert!(
            text.contains("jane.doe@example.com"),
            "nested text must be rehydrated: {text}"
        );
        assert!(!text.contains("[EMAIL_1]"), "placeholder leaked: {text}");
    }

    #[test]
    fn mask_outbound_passes_through_unparseable_frames_unchanged() {
        let st = test_state();
        let session = "ws_unit_3".to_string();
        let mut events = Vec::new();
        let frame = "not json at all";
        assert_eq!(
            mask_outbound(frame, &st, &session, &mut events),
            frame,
            "unparseable frames must be forwarded verbatim"
        );
        assert!(events.is_empty());
    }

    #[test]
    fn frames_without_pii_are_forwarded_byte_for_byte() {
        let st = test_state();
        let session = "ws_unit_4".to_string();
        let frame =
            r#"{"type":"response.create","input":"just a harmless question","model":"gpt-4o"}"#;
        let mut events = Vec::new();
        assert_eq!(
            mask_outbound(frame, &st, &session, &mut events),
            frame,
            "no-change frames must keep exact original bytes"
        );
        assert!(events.is_empty());
    }
}
