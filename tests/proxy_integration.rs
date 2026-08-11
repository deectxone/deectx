use serde_json::json;
use std::io::Read;
use std::sync::{Arc, Mutex};

/// Spawn a one-shot mock upstream on an ephemeral port; returns (base_url, received_body).
fn mock_upstream() -> (String, Arc<Mutex<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(String::new()));
    let received2 = received.clone();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf).unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        *received2.lock().unwrap() = body;
        let resp_body = r#"{"id":"chatcmpl-1","choices":[{"message":{"content":"ok, sending report to [EMAIL_1]","tool_calls":[{"id":"call_1","type":"function","function":{"name":"send_report","arguments":"{\"email\":\"[EMAIL_1]\"}"}}]}}]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            resp_body.len(), resp_body
        );
        use std::io::Write;
        stream.write_all(resp.as_bytes()).unwrap();
    });
    (format!("http://127.0.0.1:{}", port), received)
}

#[tokio::test]
async fn masks_email_before_forwarding() {
    let (upstream, received) = mock_upstream();
    let ledger_path = std::env::temp_dir().join(format!("deectx_it_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);

    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: upstream.clone(),
        ledger_path: ledger_path.clone(),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .json(&json!({"model":"gpt-4","messages":[{"role":"user","content":"my email is jane.doe@example.com and card 4111 1111 1111 1111"}]}))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let resp_body = resp.text().await.unwrap();
    assert!(
        resp_body.contains("jane.doe@example.com"),
        "placeholder was not rehydrated in response: {}",
        resp_body
    );
    let resp_json: serde_json::Value = serde_json::from_str(&resp_body).unwrap();
    let args = resp_json["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    assert!(
        args.contains("jane.doe@example.com"),
        "tool_calls arguments were not rehydrated: {}",
        args
    );
    assert!(
        !resp_body.contains("[EMAIL_1]"),
        "masked placeholder leaked to client: {}",
        resp_body
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream_body = received.lock().unwrap().clone();
    assert!(
        !upstream_body.contains("jane.doe@example.com"),
        "PII leaked upstream: {}",
        upstream_body
    );
    assert!(upstream_body.contains("[EMAIL_1]"));
    assert!(upstream_body.contains("[CARD_1]") || upstream_body.contains("[CREDIT_CARD_1]"));

    let ledger = std::fs::read_to_string(&ledger_path).unwrap();
    assert!(ledger.contains("\"email\""));
    assert!(!ledger.contains("jane.doe@example.com"));
}

/// Spawn a one-shot mock upstream on an ephemeral port; returns (addr, received_body).
async fn mock_upstream_anthropic() -> (std::net::SocketAddr, Arc<Mutex<String>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body_seen = Arc::new(Mutex::new(String::new()));
    let seen = body_seen.clone();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        let n = sock.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let idx = req.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        *seen.lock().unwrap() = req[idx..].to_string();
        let resp_body = r#"{"content":[{"type":"text","text":"ok, sending report to [EMAIL_1]"}],"stop_reason":"end_turn","model":"claude-3-7-sonnet"}"#;
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", resp_body.len(), resp_body);
        sock.write_all(resp.as_bytes()).await.unwrap();
    });
    (addr, body_seen)
}

#[tokio::test]
async fn masks_and_rehydrates_anthropic_messages() {
    let (up_addr, seen) = mock_upstream_anthropic().await;
    let ledger_path = std::env::temp_dir().join(format!("deectx_an_{}.jsonl", std::process::id()));
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: format!("http://{up_addr}"),
        ledger_path,
        upstream_anthropic: Some(format!("http://{up_addr}")),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{local}/v1/messages"))
        .header("x-api-key", "test-key")
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 64,
            "system": "contact jane.doe@example.com",
            "messages": [{"role": "user", "content": "my email is jane.doe@example.com"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("jane.doe@example.com"),
        "response must be rehydrated: {text}"
    );
    assert!(!text.contains("[EMAIL_1]"), "placeholder leaked: {text}");

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream = seen.lock().unwrap().clone();
    assert!(
        upstream.contains("[EMAIL_1]"),
        "upstream must see masked email: {upstream}"
    );
    assert!(
        !upstream.contains("jane.doe@example.com"),
        "upstream must not see raw email: {upstream}"
    );
}

#[tokio::test]
async fn masks_tool_arguments_on_request() {
    let (upstream, received) = mock_upstream();
    let ledger_path =
        std::env::temp_dir().join(format!("deectx_tools_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);

    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: upstream.clone(),
        ledger_path,
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .json(&json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "send the report"},
                {"role": "assistant", "content": null, "tool_calls": [{"id":"c1","type":"function","function":{"name":"send_report","arguments":"{\"email\":\"jane.doe@example.com\"}"}}]}
            ]
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream_body = received.lock().unwrap().clone();
    assert!(
        upstream_body.contains("[EMAIL_1]"),
        "tool argument must be masked upstream: {}",
        upstream_body
    );
    assert!(
        !upstream_body.contains("jane.doe@example.com"),
        "raw email must not leak upstream: {}",
        upstream_body
    );
}

#[tokio::test]
async fn masking_a_tool_arg_preserves_sibling_numeric_fields() {
    let (upstream, received) = mock_upstream();
    let ledger_path =
        std::env::temp_dir().join(format!("deectx_fc_sib_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);

    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: upstream.clone(),
        ledger_path,
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let client = reqwest::Client::new();
    let body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "bill the user"},
            {"role": "assistant", "content": null, "tool_calls": [{"id":"c1","type":"function","function":{"name":"charges",
                "arguments": "{\"email\":\"jane.doe@example.com\",\"amount\":1.50,\"big\":9007199254740993}"}}]}
        ]
    });
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream = received.lock().unwrap().clone();
    assert!(
        upstream.contains("[EMAIL_1]"),
        "email must be masked upstream: {upstream}"
    );
    assert!(
        !upstream.contains("jane.doe@example.com"),
        "raw email leaked upstream: {upstream}"
    );
    // Parse the forwarded body down to the tool-call arguments (the arguments
    // arrive as an escaped JSON string) and verify masking the email did not
    // re-encode the unmasked sibling numbers.
    let body: serde_json::Value = serde_json::from_str(&upstream).unwrap();
    let args_raw = body["messages"][1]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    assert!(
        args_raw.contains("\"amount\":1.50"),
        "float trailing zero dropped despite sibling masked: {args_raw}"
    );
    assert!(
        args_raw.contains("9007199254740993"),
        "large integer precision lost despite sibling masked: {args_raw}"
    );
    let args: serde_json::Value = serde_json::from_str(args_raw).unwrap();
    assert_eq!(
        args["email"], "[EMAIL_1]",
        "email must resolve to placeholder"
    );
    assert_eq!(
        args["big"], 9007199254740993i64,
        "big int must round-trip exactly"
    );
}

#[tokio::test]
async fn masks_anthropic_tool_use_input() {
    let (up_addr, seen) = mock_upstream_anthropic().await;
    let ledger_path =
        std::env::temp_dir().join(format!("deectx_tools_an_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: format!("http://{up_addr}"),
        ledger_path,
        upstream_anthropic: Some(format!("http://{up_addr}")),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{local}/v1/messages"))
        .header("x-api-key", "test-key")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "tool_use", "id": "t1", "name": "send_report", "input": {"email": "jane.doe@example.com"}}
                ]
            }]
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream = seen.lock().unwrap().clone();
    assert!(
        upstream.contains("[EMAIL_1]"),
        "tool_use.input must be masked: {upstream}"
    );
    assert!(
        !upstream.contains("jane.doe@example.com"),
        "tool_use.input leaked: {upstream}"
    );
}

/// Regression test: Anthropic tool_use/tool_result ids are long, high-entropy
/// tokens (e.g. `toolu_01A9kF2mQ8vLp3nR7sW1yZ4bN6`) that the secrets entropy
/// detector would otherwise flag as an api_key and mask into
/// `[REDACTED_SECRET]`/`[API_KEY_N]` — which breaks Anthropic's
/// `^[a-zA-Z0-9_-]+$` id schema and the API rejects the request with a 400.
/// These fields must pass through byte-for-byte.
#[tokio::test]
async fn anthropic_tool_use_id_survives_masking() {
    let (up_addr, seen) = mock_upstream_anthropic().await;
    let ledger_path =
        std::env::temp_dir().join(format!("deectx_tool_id_an_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: format!("http://{up_addr}"),
        ledger_path,
        upstream_anthropic: Some(format!("http://{up_addr}")),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let tool_use_id = "toolu_01A9kF2mQ8vLp3nR7sW1yZ4bN6";
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{local}/v1/messages"))
        .header("x-api-key", "test-key")
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 64,
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {"type": "tool_use", "id": tool_use_id, "name": "send_report", "input": {"email": "jane.doe@example.com"}}
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {"type": "tool_result", "tool_use_id": tool_use_id, "content": "sent"}
                    ]
                }
            ]
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream = seen.lock().unwrap().clone();
    assert!(
        upstream.contains(&format!("\"id\":\"{tool_use_id}\"")),
        "tool_use.id must survive masking unchanged: {upstream}"
    );
    assert!(
        upstream.contains(&format!("\"tool_use_id\":\"{tool_use_id}\"")),
        "tool_result.tool_use_id must survive masking unchanged: {upstream}"
    );
    assert!(
        upstream.contains("[EMAIL_1]"),
        "the email inside tool_use.input must still be masked: {upstream}"
    );
}

/// Same regression as `anthropic_tool_use_id_survives_masking` but for
/// OpenAI-shaped `tool_calls[].id` / `tool_call_id`.
#[tokio::test]
async fn openai_tool_call_id_survives_masking() {
    let (upstream_addr, received) = mock_upstream();
    let ledger_path =
        std::env::temp_dir().join(format!("deectx_tool_id_oai_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);

    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: upstream_addr.clone(),
        ledger_path,
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let call_id = "call_01A9kF2mQ8vLp3nR7sW1yZ4bN6";
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .json(&json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "send the report"},
                {"role": "assistant", "content": null, "tool_calls": [{"id": call_id, "type": "function", "function": {"name":"send_report","arguments":"{\"email\":\"jane.doe@example.com\"}"}}]},
                {"role": "tool", "tool_call_id": call_id, "content": "sent"}
            ]
        }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream_body = received.lock().unwrap().clone();
    assert!(
        upstream_body.contains(&format!("\"id\":\"{call_id}\"")),
        "tool_calls[].id must survive masking unchanged: {upstream_body}"
    );
    assert!(
        upstream_body.contains(&format!("\"tool_call_id\":\"{call_id}\"")),
        "tool_call_id must survive masking unchanged: {upstream_body}"
    );
    assert!(
        upstream_body.contains("[EMAIL_1]"),
        "the email inside the tool arguments must still be masked: {upstream_body}"
    );
}

#[tokio::test]
async fn streams_sse_with_split_placeholder_rehydrated() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        let _ = sock.read(&mut buf).await.unwrap();
        sock.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
        sock.flush().await.unwrap();
        sock.write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"hello [EMA")
            .await
            .unwrap();
        sock.flush().await.unwrap();
        sock.write_all(b"IL_1]\"}}]}\n\ndata: [DONE]\n\n")
            .await
            .unwrap();
        sock.flush().await.unwrap();
    });

    let ledger_path = std::env::temp_dir().join(format!("deectx_sse_{}.jsonl", std::process::id()));
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: format!("http://{up_addr}"),
        ledger_path,
        ..Default::default()
    };
    let proxy_listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let local = proxy_listener.local_addr().unwrap();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, proxy_listener));

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{local}/v1/chat/completions"))
        .json(&serde_json::json!({
            "model": "gpt-4",
            "stream": true,
            "messages": [{"role": "user", "content": "my email is jane.doe@example.com"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().contains("event-stream"))
            .unwrap_or(false),
        "must preserve event-stream content-type"
    );
    let text = resp.text().await.unwrap();
    assert!(
        text.contains("jane.doe@example.com"),
        "split placeholder must be rehydrated: {text}"
    );
    assert!(
        !text.contains("[EMAIL_1]"),
        "placeholder leaked into stream: {text}"
    );
}

#[tokio::test]
async fn healthz_returns_ok() {
    let ledger_path = std::env::temp_dir().join(format!("deectx_hz_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        ledger_path,
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/healthz", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

/// Spawn a deectx proxy on an ephemeral port with default config; returns (addr, handle).
/// /stats does not hit upstream, so no mock upstream is needed here.
async fn spawn_proxy() -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        ledger_path: std::env::temp_dir()
            .join(format!("deectx_stats_{}.jsonl", std::process::id())),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));
    (addr, handle)
}

#[tokio::test]
async fn stats_endpoint_returns_json() {
    let (addr, _guard) = spawn_proxy().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/stats"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["requests"], 0);
    assert!(body.get("masked").is_some());
}

#[tokio::test]
async fn stats_counters_reflect_real_requests() {
    let (upstream, _received) = mock_upstream();
    let ledger_path =
        std::env::temp_dir().join(format!("deectx_stats_cnt_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream,
        ledger_path,
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    // A real completion request carrying an email: drives requests + masked.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .json(&json!({"model":"gpt-4","messages":[{"role":"user","content":"contact jane.doe@example.com please"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let stats = client
        .get(format!("http://127.0.0.1:{}/stats", port))
        .send()
        .await
        .unwrap();
    assert_eq!(stats.status(), 200);
    let body: serde_json::Value = stats.json().await.unwrap();
    assert!(
        body["requests"].as_u64().unwrap_or(0) >= 1,
        "requests must be counted: {body}"
    );
    assert!(
        body["masked"].as_u64().unwrap_or(0) >= 1,
        "masked events must be counted: {body}"
    );
}

#[tokio::test]
async fn stats_endpoint_disabled_returns_404() {
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        ledger_path: std::env::temp_dir()
            .join(format!("deectx_stats_off_{}.jsonl", std::process::id())),
        stats_enabled: false,
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/stats", port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[cfg(feature = "ner")]
#[tokio::test]
async fn fail_closed_pack_blocks_when_ner_model_missing() {
    let pack_dir = std::env::temp_dir().join(format!("deectx_strict_{}", std::process::id()));
    std::fs::create_dir_all(&pack_dir).unwrap();
    std::fs::write(
        pack_dir.join("strict.yaml"),
        r#"name: strict
version: 0.1.0
entities:
  - id: person
    detector: ner
    labels: [person]
    action: mask
settings:
  failClosed: true
"#,
    )
    .unwrap();

    let (upstream, _received) = mock_upstream();
    let ledger_path = std::env::temp_dir().join(format!("deectx_fc_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream,
        ledger_path,
        active_packs: vec!["strict".into()],
        packs_dir: Some(pack_dir.clone()),
        model_dir: Some(std::path::PathBuf::from("./definitely-missing-models")),
        ner: true,
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .json(&json!({"model":"gpt-4","messages":[{"role":"user","content":"hello"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        503,
        "failClosed must block when NER model is unavailable"
    );
    let _ = std::fs::remove_dir_all(&pack_dir);
}

/// Loopback WebSocket upstream: accepts a masked frame, echoes a Responses
/// event containing a masked placeholder, then closes. Returns (ws_url, masked_texts_seen).
async fn mock_responses_ws() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as Ws;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(msg)) = ws.next().await {
            if let Ws::Text(text) = msg {
                seen2.lock().unwrap().push(text.to_string());
                let reply = r#"{"type":"response.output_text.delta","delta":"sent report to [EMAIL_1]","item_id":"msg_1","output_index":0,"content_index":0}"#;
                ws.send(Ws::Text(reply.into())).await.unwrap();
                ws.send(Ws::Close(None)).await.unwrap();
                break;
            }
        }
    });
    (format!("ws://{addr}/v1/responses"), seen)
}

#[tokio::test]
async fn responses_ws_masks_outbound_and_rehydrates_inbound() {
    let (upstream, seen) = mock_responses_ws().await;
    let ledger_path = std::env::temp_dir().join(format!("deectx_ws_{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&ledger_path);
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream_responses: upstream.clone(),
        ledger_path: ledger_path.clone(),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as Ws;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{local}/v1/responses"))
        .await
        .unwrap();
    ws.send(Ws::Text(
        r#"{"type":"response.create","input":"my email is jane.doe@example.com"}"#.into(),
    ))
    .await
    .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    let text = match reply {
        Ws::Text(t) => t.to_string(),
        other => panic!("expected a text frame, got {other:?}"),
    };
    assert!(
        text.contains("jane.doe@example.com"),
        "inbound delta must be rehydrated to the client: {text}"
    );
    assert!(
        !text.contains("[EMAIL_1]"),
        "masked placeholder leaked to client: {text}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let sent = seen.lock().unwrap().clone();
    assert!(
        sent.iter().any(|s| s.contains("[EMAIL_1]")),
        "upstream must see the masked email: {sent:?}"
    );
    assert!(
        sent.iter().all(|s| !s.contains("jane.doe@example.com")),
        "raw email must not leave outbound: {sent:?}"
    );

    let ledger = std::fs::read_to_string(&ledger_path).unwrap();
    assert!(
        ledger.contains("\"tool\":\"responses-ws\""),
        "WS connection must log a responses-ws ledger entry: {ledger}"
    );
    assert!(ledger.contains("\"entity\":\"email\""));
    assert!(
        !ledger.contains("jane.doe@example.com"),
        "ledger must never store raw PII"
    );
}
