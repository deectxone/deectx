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
    assert!(resp_body.contains("jane.doe@example.com"),
            "placeholder was not rehydrated in response: {}", resp_body);
    let resp_json: serde_json::Value = serde_json::from_str(&resp_body).unwrap();
    let args = resp_json["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"].as_str().unwrap();
    assert!(args.contains("jane.doe@example.com"),
            "tool_calls arguments were not rehydrated: {}", args);
    assert!(!resp_body.contains("[EMAIL_1]"),
            "masked placeholder leaked to client: {}", resp_body);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream_body = received.lock().unwrap().clone();
    assert!(!upstream_body.contains("jane.doe@example.com"), "PII leaked upstream: {}", upstream_body);
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
    assert!(text.contains("jane.doe@example.com"), "response must be rehydrated: {text}");
    assert!(!text.contains("[EMAIL_1]"), "placeholder leaked: {text}");

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let upstream = seen.lock().unwrap().clone();
    assert!(upstream.contains("[EMAIL_1]"), "upstream must see masked email: {upstream}");
    assert!(!upstream.contains("jane.doe@example.com"), "upstream must not see raw email: {upstream}");
}
