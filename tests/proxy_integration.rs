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
