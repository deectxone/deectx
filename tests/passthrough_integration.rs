use serde_json::json;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// One-shot mock upstream that records the request line (method + path) and the
/// body, then returns a fixed JSON body. Returns (base_url, request_line, body).
fn mock_capture() -> (String, Arc<Mutex<String>>, Arc<Mutex<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let line = Arc::new(Mutex::new(String::new()));
    let body = Arc::new(Mutex::new(String::new()));
    let (l2, b2) = (line.clone(), body.clone());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf).unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        *l2.lock().unwrap() = req.lines().next().unwrap_or("").to_string();
        *b2.lock().unwrap() = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        let resp_body = r#"{"input_tokens":5}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            resp_body.len(),
            resp_body
        );
        stream.write_all(resp.as_bytes()).unwrap();
    });
    (format!("http://127.0.0.1:{}", port), line, body)
}

#[tokio::test]
async fn unknown_get_path_forwards_verbatim() {
    let (upstream, line, _body) = mock_capture();
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: upstream.clone(),
        ledger_path: std::env::temp_dir()
            .join(format!("deectx_pt_get_{}.jsonl", std::process::id())),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/v1/models", port))
        .header("authorization", "Bearer sk-test")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let request_line = line.lock().unwrap().clone();
    assert!(
        request_line.starts_with("GET /v1/models"),
        "method+path must be preserved: {request_line}"
    );
}

#[tokio::test]
async fn count_tokens_body_is_masked_before_forward() {
    let (upstream, line, body) = mock_capture();
    let cfg = deectx::config::Config {
        listen: "127.0.0.1:0".into(),
        upstream: upstream.clone(),
        upstream_anthropic: Some(upstream.clone()),
        ledger_path: std::env::temp_dir()
            .join(format!("deectx_pt_ct_{}.jsonl", std::process::id())),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let resp = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/v1/messages/count_tokens",
            port
        ))
        .header("x-api-key", "test-key")
        .json(&json!({"model":"claude-3-7-sonnet","messages":[{"role":"user","content":"my email is jane.doe@example.com"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let request_line = line.lock().unwrap().clone();
    assert!(
        request_line.starts_with("POST /v1/messages/count_tokens"),
        "path preserved: {request_line}"
    );
    let upstream_body = body.lock().unwrap().clone();
    assert!(
        upstream_body.contains("[EMAIL_1]"),
        "count_tokens body must be masked upstream: {upstream_body}"
    );
    assert!(
        !upstream_body.contains("jane.doe@example.com"),
        "raw email must not leak via count_tokens: {upstream_body}"
    );
}
