# deeCtx Lifecycle UX & Transparent Proxy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make deeCtx a transparent reverse proxy (never breaks tools) with a guided `start`/`stop`/`status`/`uninstall` lifecycle, backed by a stable `~/.deectx/` home.

**Architecture:** A thin lifecycle layer over the existing, tested `proxy.rs` and `setup.rs`. New `home.rs` centralizes runtime state; `proxy.rs` gains a catch-all fallback route; new `lifecycle.rs` composes tool-wiring + process management behind a `ProcessManager` trait so it's testable without spawning.

**Tech Stack:** Rust 2021, axum 0.7, reqwest 0.12, tokio, serde/serde_json, anyhow, clap 4.

## Global Constraints

- Rust edition 2021; errors via `anyhow`; serde derive. Copy exact values.
- `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings` MUST stay clean; `cargo test` MUST pass.
- Never write raw PII anywhere; ledger stays hash-only.
- Config is TOML-only. The ONE allowed environment override is `DEECTX_HOME` (a path for the runtime home, not config values).
- Crate version string is always `env!("CARGO_PKG_VERSION")`.
- Add new features as focused `src/` files exported from `lib.rs`; do not pile into `proxy.rs`.
- Tests must not spawn OS processes or hit the network except the existing loopback mock-upstream pattern in `tests/proxy_integration.rs`.

---

### Task 1: `home.rs` — runtime home + path helpers

**Files:**
- Create: `src/home.rs`
- Modify: `src/lib.rs` (add `pub mod home;`)

**Interfaces:**
- Produces: `deectx_home() -> PathBuf`, `config_path() -> PathBuf`, `ledger_path() -> PathBuf`, `pidfile_path() -> PathBuf`, `ensure_home() -> std::io::Result<()>`.

- [ ] **Step 1: Add the module declaration**

In `src/lib.rs`, add after `pub mod detect;`:

```rust
pub mod home;
```

- [ ] **Step 2: Write the failing test**

Create `src/home.rs`:

```rust
use std::path::PathBuf;

/// deeCtx runtime home: `$DEECTX_HOME` or `~/.deectx`. All runtime state
/// (config, ledger, pidfile) lives here so the daemon and CLI agree on paths
/// regardless of the process working directory.
pub fn deectx_home() -> PathBuf {
    if let Ok(h) = std::env::var("DEECTX_HOME") {
        return PathBuf::from(h);
    }
    let base = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".deectx")
}

/// Default config location: `<home>/config.toml`.
pub fn config_path() -> PathBuf {
    deectx_home().join("config.toml")
}

/// Default ledger location: `<home>/ledger.jsonl`.
pub fn ledger_path() -> PathBuf {
    deectx_home().join("ledger.jsonl")
}

/// Pidfile location: `<home>/deectx.pid`.
pub fn pidfile_path() -> PathBuf {
    deectx_home().join("deectx.pid")
}

/// Create the home directory if it does not exist.
pub fn ensure_home() -> std::io::Result<()> {
    std::fs::create_dir_all(deectx_home())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize env mutation across tests in this file.
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn deectx_home_env_override_wins() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("DEECTX_HOME", "/tmp/deectx-test-home");
        assert_eq!(deectx_home(), PathBuf::from("/tmp/deectx-test-home"));
        assert_eq!(
            config_path(),
            PathBuf::from("/tmp/deectx-test-home").join("config.toml")
        );
        assert_eq!(
            ledger_path(),
            PathBuf::from("/tmp/deectx-test-home").join("ledger.jsonl")
        );
        std::env::remove_var("DEECTX_HOME");
    }

    #[test]
    fn deectx_home_defaults_under_profile() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("DEECTX_HOME");
        let home = deectx_home();
        assert!(
            home.ends_with(".deectx"),
            "default home must end with .deectx: {home:?}"
        );
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib home::`
Expected: PASS (2 tests).

- [ ] **Step 4: Verify lint/format**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/home.rs src/lib.rs
git commit -m "feat(home): add ~/.deectx runtime home + path helpers"
```

---

### Task 2: `home.rs` — pidfile encode/parse + read/write/clear

**Files:**
- Modify: `src/home.rs`

**Interfaces:**
- Consumes: `pidfile_path()`, `ensure_home()` (Task 1).
- Produces: `pub struct Pidfile { pub pid: u32, pub listen: String, pub version: String }` with `encode()`, `parse(&str) -> Option<Pidfile>`, `write() -> std::io::Result<()>`, `read() -> Option<Pidfile>`, `clear()`.

- [ ] **Step 1: Write the failing test**

Append inside the `tests` module in `src/home.rs`:

```rust
    #[test]
    fn pidfile_roundtrips() {
        let pf = Pidfile {
            pid: 4321,
            listen: "127.0.0.1:8787".into(),
            version: "0.2.0".into(),
        };
        let parsed = Pidfile::parse(&pf.encode()).unwrap();
        assert_eq!(parsed, pf);
    }

    #[test]
    fn pidfile_parse_rejects_garbage() {
        assert!(Pidfile::parse("not-a-number\n").is_none());
        assert!(Pidfile::parse("").is_none());
    }

    #[test]
    fn pidfile_write_read_clear() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("deectx_pf_{}", std::process::id()));
        std::env::set_var("DEECTX_HOME", &dir);
        let pf = Pidfile {
            pid: 7,
            listen: "127.0.0.1:9999".into(),
            version: "9.9.9".into(),
        };
        pf.write().unwrap();
        assert_eq!(Pidfile::read().unwrap(), pf);
        Pidfile::clear();
        assert!(Pidfile::read().is_none());
        std::env::remove_var("DEECTX_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib home::tests::pidfile`
Expected: FAIL — `cannot find type Pidfile`.

- [ ] **Step 3: Add the implementation**

Insert before the `#[cfg(test)]` block in `src/home.rs`:

```rust
/// Contents of `<home>/deectx.pid`, written by `serve` on start and read by
/// the lifecycle commands. Three newline-separated lines: pid, listen, version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pidfile {
    pub pid: u32,
    pub listen: String,
    pub version: String,
}

impl Pidfile {
    pub fn encode(&self) -> String {
        format!("{}\n{}\n{}\n", self.pid, self.listen, self.version)
    }

    pub fn parse(s: &str) -> Option<Pidfile> {
        let mut lines = s.lines();
        let pid = lines.next()?.trim().parse().ok()?;
        let listen = lines.next()?.trim().to_string();
        let version = lines.next().unwrap_or("").trim().to_string();
        Some(Pidfile { pid, listen, version })
    }

    pub fn write(&self) -> std::io::Result<()> {
        ensure_home()?;
        std::fs::write(pidfile_path(), self.encode())
    }

    pub fn read() -> Option<Pidfile> {
        std::fs::read_to_string(pidfile_path())
            .ok()
            .and_then(|s| Pidfile::parse(&s))
    }

    pub fn clear() {
        let _ = std::fs::remove_file(pidfile_path());
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib home::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/home.rs
git commit -m "feat(home): add Pidfile encode/parse/read/write/clear"
```

---

### Task 3: `config.rs` — ledger defaults to the home dir

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: `crate::home::ledger_path()` (Task 1).
- Produces: `Config::default().ledger_path == home::ledger_path()`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/config.rs`:

```rust
    #[test]
    fn default_ledger_is_under_home() {
        std::env::set_var("DEECTX_HOME", "/tmp/deectx-cfg-home");
        assert_eq!(
            Config::default().ledger_path,
            std::path::PathBuf::from("/tmp/deectx-cfg-home").join("ledger.jsonl")
        );
        std::env::remove_var("DEECTX_HOME");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib config::tests::default_ledger_is_under_home`
Expected: FAIL — default is `./ledger.jsonl`.

- [ ] **Step 3: Point the default at home**

In `src/config.rs`, replace `default_ledger()`:

```rust
fn default_ledger() -> PathBuf {
    crate::home::ledger_path()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config::`
Expected: PASS (existing `loads_partial_toml_with_defaults` still passes — it only checks `upstream`/`listen`).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): default ledger_path to ~/.deectx/ledger.jsonl"
```

---

### Task 4: `proxy.rs` — extract a method-agnostic `forward` + passthrough decision

**Files:**
- Modify: `src/proxy.rs`

**Interfaces:**
- Produces:
  - `async fn forward(st, base: &str, method: reqwest::Method, path_and_query: &str, headers: HeaderMap, body: Bytes, session: Option<&str>, format: ApiFormat, stream: bool) -> Response`
  - `fn passthrough_should_mask(path: &str) -> bool`
- Consumes: existing `mask_walk`, `rehydrate_response`, `is_gzip_like*`, `SseRehydrator`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/proxy.rs`:

```rust
    #[test]
    fn count_tokens_is_masked_others_are_not() {
        assert!(passthrough_should_mask("/v1/messages/count_tokens"));
        assert!(!passthrough_should_mask("/v1/models"));
        assert!(!passthrough_should_mask("/v1/messages/batches"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib proxy::tests::count_tokens_is_masked_others_are_not`
Expected: FAIL — `passthrough_should_mask` not found.

- [ ] **Step 3: Add the decision function**

In `src/proxy.rs`, add near `mask_content`:

```rust
/// Unmatched endpoints that still carry the full prompt and MUST be masked
/// before forwarding. `count_tokens` sends the same `messages` array as a
/// completion; everything else (models, batches, files) carries no maskable
/// user content and is forwarded verbatim.
pub(crate) fn passthrough_should_mask(path: &str) -> bool {
    path.ends_with("/messages/count_tokens")
}
```

- [ ] **Step 4: Extract the shared forwarder**

In `src/proxy.rs`, replace the body of `forward_raw` so it delegates to a new `forward`. Add `forward` and rewrite `forward_raw`:

```rust
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
```

- [ ] **Step 5: Run the full test suite (behavior preserved)**

Run: `cargo test`
Expected: PASS — all existing `proxy_integration.rs` tests (mask/rehydrate/SSE/stats/anthropic) still pass; new decision test passes.

- [ ] **Step 6: Verify lint/format, then commit**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings`

```bash
git add src/proxy.rs
git commit -m "refactor(proxy): method-agnostic forward + passthrough mask decision"
```

---

### Task 5: `proxy.rs` — transparent fallback route

**Files:**
- Modify: `src/proxy.rs`
- Test: `tests/passthrough_integration.rs`

**Interfaces:**
- Consumes: `forward`, `passthrough_should_mask`, `mask_walk`, `session_id`, `upstream::classify`, `ApiFormat`.
- Produces: router `.fallback(handle_passthrough)`.

- [ ] **Step 1: Write the failing integration test**

Create `tests/passthrough_integration.rs`:

```rust
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
        ledger_path: std::env::temp_dir().join(format!("deectx_pt_get_{}.jsonl", std::process::id())),
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
        ledger_path: std::env::temp_dir().join(format!("deectx_pt_ct_{}.jsonl", std::process::id())),
        ..Default::default()
    };
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(deectx::proxy::serve_with_listener(cfg, listener));

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/v1/messages/count_tokens", port))
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test passthrough_integration`
Expected: FAIL — unmatched routes currently 404 (no fallback), so status is 404 not 200.

- [ ] **Step 3: Add the fallback handler**

In `src/proxy.rs`, add the handler:

```rust
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

    let method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::POST);

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
    forward(&st, base, method, &path_and_query, headers, out_body, None, format, false).await
}
```

- [ ] **Step 4: Register the fallback**

In `serve_with_listener`, change the router build so the fallback is attached before `.with_state`:

```rust
    if stats_enabled {
        app = app.route("/stats", axum::routing::get(handle_stats));
    }
    let app = app.fallback(handle_passthrough).with_state(state);
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test passthrough_integration && cargo test`
Expected: PASS (new + all existing).

- [ ] **Step 6: Verify lint/format, then commit**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings`

```bash
git add src/proxy.rs tests/passthrough_integration.rs
git commit -m "feat(proxy): transparent fallback route (fixes count_tokens/models 404 -> 'prompt too long')"
```

---

### Task 6: `proxy.rs` — write/clear the pidfile in `run_proxy`

**Files:**
- Modify: `src/proxy.rs`

**Interfaces:**
- Consumes: `crate::home::Pidfile` (Task 2).
- Produces: `run_proxy` writes `<home>/deectx.pid` on bind, clears it on exit. `serve_with_listener` is unchanged (tests never write a pidfile).

- [ ] **Step 1: Update `run_proxy`**

In `src/proxy.rs`, replace `run_proxy`:

```rust
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
```

- [ ] **Step 2: Verify existing tests still pass**

Run: `cargo test`
Expected: PASS (integration tests use `serve_with_listener` directly, so no pidfile side effects).

- [ ] **Step 3: Verify lint/format, then commit**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings`

```bash
git add src/proxy.rs
git commit -m "feat(proxy): run_proxy writes/clears ~/.deectx/deectx.pid"
```

---

### Task 7: `lifecycle.rs` — ProcessManager trait, fake, and StatusReport renderer

**Files:**
- Create: `src/lifecycle.rs`
- Modify: `src/lib.rs` (add `pub mod lifecycle;`)

**Interfaces:**
- Consumes: `crate::home`, `crate::setup::Tool`.
- Produces:
  - `pub trait ProcessManager { fn spawn_serve(&self) -> anyhow::Result<u32>; fn is_alive(&self, pid: u32) -> bool; fn kill(&self, pid: u32) -> anyhow::Result<()>; fn port_in_use(&self, addr: &str) -> bool; }`
  - `pub struct StatusReport { pub running: bool, pub listen: Option<String>, pub running_version: Option<String>, pub current_version: String, pub tools: Vec<(crate::setup::Tool, bool)>, pub warnings: Vec<String> }`
  - `pub fn render_status(r: &StatusReport) -> String`

- [ ] **Step 1: Add the module declaration**

In `src/lib.rs`, add after `pub mod ledger;`:

```rust
pub mod lifecycle;
```

- [ ] **Step 2: Write the failing test**

Create `src/lifecycle.rs`:

```rust
use crate::setup::Tool;

/// Abstraction over OS process operations so the lifecycle logic is testable
/// without spawning or killing anything.
pub trait ProcessManager {
    /// Spawn `<current_exe> serve` detached; return the child pid.
    fn spawn_serve(&self) -> anyhow::Result<u32>;
    /// True if a process with `pid` is currently alive.
    fn is_alive(&self, pid: u32) -> bool;
    /// Terminate `pid`.
    fn kill(&self, pid: u32) -> anyhow::Result<()>;
    /// True if `addr` (host:port) is already bound by some process.
    fn port_in_use(&self, addr: &str) -> bool;
}

/// A snapshot of deeCtx's state for the status dashboard.
#[derive(Debug, Clone)]
pub struct StatusReport {
    pub running: bool,
    pub listen: Option<String>,
    pub running_version: Option<String>,
    pub current_version: String,
    pub tools: Vec<(Tool, bool)>,
    pub warnings: Vec<String>,
}

/// Render the status dashboard. Pure: no I/O, so it is unit-testable.
pub fn render_status(r: &StatusReport) -> String {
    let mut out = String::new();
    if r.running {
        out.push_str("deeCtx — ACTIVE ✓ masking\n");
        let listen = r.listen.clone().unwrap_or_default();
        let ver = r.running_version.clone().unwrap_or_default();
        out.push_str(&format!("  proxy    running · {listen} · v{ver}\n"));
    } else {
        out.push_str("deeCtx — OFF (tools talk directly to the API)\n");
        out.push_str("  proxy    not running\n");
    }
    let tools: Vec<String> = r
        .tools
        .iter()
        .map(|(t, ok)| format!("{t:?} {}", if *ok { "✓" } else { "✗" }))
        .collect();
    out.push_str(&format!("  tools    {}\n", tools.join("   ")));
    for w in &r.warnings {
        out.push_str(&format!("  ⚠ {w}\n"));
    }
    out.push_str(if r.running {
        "\nNext: you're protected. `deectx stop` to turn off.\n"
    } else {
        "\nNext: `deectx start` to protect your tools.\n"
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_active_mentions_stop() {
        let r = StatusReport {
            running: true,
            listen: Some("127.0.0.1:8787".into()),
            running_version: Some("0.2.0".into()),
            current_version: "0.2.0".into(),
            tools: vec![(Tool::ClaudeCode, true), (Tool::Codex, false)],
            warnings: vec![],
        };
        let s = render_status(&r);
        assert!(s.contains("ACTIVE"));
        assert!(s.contains("127.0.0.1:8787"));
        assert!(s.contains("deectx stop"));
        assert!(s.contains("ClaudeCode ✓"));
        assert!(s.contains("Codex ✗"));
    }

    #[test]
    fn render_off_mentions_start() {
        let r = StatusReport {
            running: false,
            listen: None,
            running_version: None,
            current_version: "0.2.0".into(),
            tools: vec![],
            warnings: vec!["port 8787 in use by another app".into()],
        };
        let s = render_status(&r);
        assert!(s.contains("OFF"));
        assert!(s.contains("deectx start"));
        assert!(s.contains("⚠ port 8787 in use by another app"));
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib lifecycle::`
Expected: PASS.

- [ ] **Step 4: Verify lint/format, then commit**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings`

```bash
git add src/lifecycle.rs src/lib.rs
git commit -m "feat(lifecycle): ProcessManager trait + StatusReport renderer"
```

---

### Task 8: `lifecycle.rs` — `start` / `stop` / `uninstall` / `status` composition

**Files:**
- Modify: `src/lifecycle.rs`

**Interfaces:**
- Consumes: `ProcessManager` (Task 7), `crate::home`, `crate::setup` (`discover`, `is_locked`, `patch_config`, `unwrap`, `install_daemon`, `uninstall_daemon`, `wired`), `crate::config::Config`.
- Produces: `pub fn start<P: ProcessManager>(pm: &P) -> anyhow::Result<StatusReport>`, `pub fn stop<P: ProcessManager>(pm: &P) -> anyhow::Result<()>`, `pub fn uninstall<P: ProcessManager>(pm: &P, delete_data: bool) -> anyhow::Result<()>`, `pub fn status<P: ProcessManager>(pm: &P) -> anyhow::Result<StatusReport>`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/lifecycle.rs`:

```rust
    use std::cell::RefCell;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct FakePm {
        alive: RefCell<Vec<u32>>,
        killed: RefCell<Vec<u32>>,
        spawned: RefCell<bool>,
    }
    impl ProcessManager for FakePm {
        fn spawn_serve(&self) -> anyhow::Result<u32> {
            *self.spawned.borrow_mut() = true;
            Ok(4242)
        }
        fn is_alive(&self, pid: u32) -> bool {
            self.alive.borrow().contains(&pid)
        }
        fn kill(&self, pid: u32) -> anyhow::Result<()> {
            self.killed.borrow_mut().push(pid);
            Ok(())
        }
        fn port_in_use(&self, _addr: &str) -> bool {
            false
        }
    }

    fn isolated_home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("deectx_life_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Point both DEECTX_HOME and the profile at the temp dir so setup::discover
        // (which reads USERPROFILE/HOME) finds no real tool configs.
        std::env::set_var("DEECTX_HOME", &dir);
        std::env::set_var("USERPROFILE", &dir);
        std::env::set_var("HOME", &dir);
        dir
    }

    #[test]
    fn start_kills_stale_proxy_then_spawns() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = isolated_home("start");
        // Seed a stale pidfile whose pid is "alive".
        crate::home::Pidfile { pid: 999, listen: "127.0.0.1:8787".into(), version: "0.1.0".into() }
            .write()
            .unwrap();
        let pm = FakePm::default();
        pm.alive.borrow_mut().push(999);

        let report = start(&pm).unwrap();

        assert!(pm.killed.borrow().contains(&999), "stale proxy must be killed");
        assert!(*pm.spawned.borrow(), "a new proxy must be spawned");
        assert!(crate::home::config_path().exists(), "config.toml must be created");
        assert_eq!(report.current_version, env!("CARGO_PKG_VERSION"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_clears_pidfile_and_kills() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = isolated_home("stop");
        crate::home::Pidfile { pid: 555, listen: "127.0.0.1:8787".into(), version: "0.2.0".into() }
            .write()
            .unwrap();
        let pm = FakePm::default();
        pm.alive.borrow_mut().push(555);

        stop(&pm).unwrap();

        assert!(pm.killed.borrow().contains(&555));
        assert!(crate::home::Pidfile::read().is_none(), "pidfile must be cleared");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib lifecycle::tests::start_kills_stale_proxy_then_spawns`
Expected: FAIL — `start` not found.

- [ ] **Step 3: Implement the lifecycle functions**

In `src/lifecycle.rs`, add before the `#[cfg(test)]` block:

```rust
use crate::home::Pidfile;

const DEFAULT_LISTEN: &str = "127.0.0.1:8787";

/// Kill a running proxy recorded in the pidfile (if its pid is alive), then
/// clear the pidfile. Safe when nothing is running.
fn stop_running_proxy<P: ProcessManager>(pm: &P) {
    if let Some(pf) = Pidfile::read() {
        if pm.is_alive(pf.pid) {
            let _ = pm.kill(pf.pid);
        }
        Pidfile::clear();
    }
}

/// Ensure `<home>/config.toml` exists; write an empty file (all defaults) if not.
fn ensure_config() -> anyhow::Result<()> {
    crate::home::ensure_home()?;
    let path = crate::home::config_path();
    if !path.exists() {
        std::fs::write(&path, "# deeCtx config — see ARCHITECTURE.md §11\n")?;
    }
    Ok(())
}

/// Wire every installed, non-locked tool to the proxy. Returns per-tool wired
/// state for the status report; a single tool failing never aborts the rest.
fn wire_tools() -> Vec<(Tool, bool)> {
    let mut out = Vec::new();
    for (tool, path) in crate::setup::discover() {
        if crate::setup::is_locked(tool, &path) {
            out.push((tool, false));
            continue;
        }
        let ok = crate::setup::patch_config(tool, &path).is_ok();
        out.push((tool, ok));
    }
    out
}

/// Turn deeCtx ON. Idempotent: replaces any running/stale proxy with the
/// current binary, wires tools, installs autostart, and starts serving.
pub fn start<P: ProcessManager>(pm: &P) -> anyhow::Result<StatusReport> {
    ensure_config()?;
    stop_running_proxy(pm);
    let _tools = wire_tools();
    let _ = crate::setup::install_daemon();
    let _pid = pm.spawn_serve()?;
    status(pm)
}

/// Turn deeCtx OFF: restore tool configs (direct to API), stop the proxy, and
/// remove the login autostart so it stays off until `start`.
pub fn stop<P: ProcessManager>(pm: &P) -> anyhow::Result<()> {
    let _ = crate::setup::unwrap();
    stop_running_proxy(pm);
    let _ = crate::setup::uninstall_daemon();
    Ok(())
}

/// Full teardown: stop, then optionally delete config + ledger. Never removes
/// the binary.
pub fn uninstall<P: ProcessManager>(pm: &P, delete_data: bool) -> anyhow::Result<()> {
    stop(pm)?;
    if delete_data {
        let _ = std::fs::remove_file(crate::home::config_path());
        let _ = std::fs::remove_file(crate::home::ledger_path());
    }
    Ok(())
}

/// Build the current status snapshot from the pidfile, process liveness, tool
/// wiring, and version comparison.
pub fn status<P: ProcessManager>(pm: &P) -> anyhow::Result<StatusReport> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let pf = Pidfile::read();
    let running = pf.as_ref().map(|p| pm.is_alive(p.pid)).unwrap_or(false);

    let mut tools = Vec::new();
    for (tool, path) in crate::setup::discover() {
        let wired = std::fs::read_to_string(&path)
            .map(|c| crate::setup::wired(tool, &c))
            .unwrap_or(false);
        tools.push((tool, wired));
    }

    let mut warnings = Vec::new();
    if let Some(pf) = &pf {
        if running && pf.version != current_version {
            warnings.push(format!(
                "update installed (running v{}, have v{current_version}) — run `deectx start` to apply",
                pf.version
            ));
        }
    }
    if !running && pm.port_in_use(DEFAULT_LISTEN) {
        warnings.push(format!("port {DEFAULT_LISTEN} in use by another app"));
    }

    Ok(StatusReport {
        running,
        listen: pf.as_ref().map(|p| p.listen.clone()),
        running_version: pf.as_ref().map(|p| p.version.clone()),
        current_version,
        tools,
        warnings,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib lifecycle::`
Expected: PASS.

- [ ] **Step 5: Verify lint/format, then commit**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings`

```bash
git add src/lifecycle.rs
git commit -m "feat(lifecycle): start/stop/uninstall/status composition over setup + ProcessManager"
```

---

### Task 9: `lifecycle.rs` — real OS `ProcessManager`

**Files:**
- Modify: `src/lifecycle.rs`

**Interfaces:**
- Produces: `pub struct OsProcessManager;` implementing `ProcessManager`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/lifecycle.rs`:

```rust
    #[test]
    fn os_pm_reports_self_alive() {
        let pm = OsProcessManager;
        assert!(pm.is_alive(std::process::id()), "our own pid must be alive");
        assert!(!pm.is_alive(0), "pid 0 is never a live user process");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib lifecycle::tests::os_pm_reports_self_alive`
Expected: FAIL — `OsProcessManager` not found.

- [ ] **Step 3: Implement `OsProcessManager`**

In `src/lifecycle.rs`, add before the `#[cfg(test)]` block:

```rust
/// Real process operations. `is_alive`/`kill` shell out to platform tools to
/// avoid a new native dependency; `port_in_use` probes with a bind.
pub struct OsProcessManager;

impl ProcessManager for OsProcessManager {
    fn spawn_serve(&self) -> anyhow::Result<u32> {
        let exe = std::env::current_exe()?;
        let child = std::process::Command::new(exe).arg("serve").spawn()?;
        Ok(child.id())
    }

    fn is_alive(&self, pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        #[cfg(windows)]
        {
            let out = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output();
            match out {
                Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
                Err(_) => false,
            }
        }
        #[cfg(not(windows))]
        {
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    fn kill(&self, pid: u32) -> anyhow::Result<()> {
        #[cfg(windows)]
        let status = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status()?;
        #[cfg(not(windows))]
        let status = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("failed to kill pid {pid}")
        }
    }

    fn port_in_use(&self, addr: &str) -> bool {
        std::net::TcpListener::bind(addr).is_err()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib lifecycle::tests::os_pm_reports_self_alive`
Expected: PASS.

- [ ] **Step 5: Verify lint/format, then commit**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings`

```bash
git add src/lifecycle.rs
git commit -m "feat(lifecycle): OsProcessManager (spawn/is_alive/kill/port_in_use)"
```

---

### Task 10: `main.rs` — CLI verbs, default status, back-compat aliases

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `deectx::lifecycle::{start, stop, uninstall, status, render_status, OsProcessManager}`, `deectx::home`.

- [ ] **Step 1: Add the new subcommands + default to the `Cmd` enum**

In `src/main.rs`, add these variants to `enum Cmd` (keep all existing ones):

```rust
    /// Turn deeCtx on: wire tools, install autostart, start masking
    Start,
    /// Turn deeCtx off: restore tools to direct API, stop the proxy
    Stop,
    /// Remove deeCtx: stop + restore tools + optionally delete data
    Uninstall {
        /// Also delete config + ledger without prompting
        #[arg(long)]
        purge: bool,
    },
```

Make the subcommand optional so bare `deectx` shows status. Change the `Cli` struct:

```rust
#[derive(Parser)]
#[command(name = "deectx", about = "Local PII-masking proxy for AI tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}
```

- [ ] **Step 2: Default `--config` to the home config**

In `src/main.rs`, change the three `#[arg(long, default_value = "config.toml")]` occurrences (Serve, Audit, Status) to resolve the home path. Replace each `config: PathBuf` arg default with a value parser using `deectx::home::config_path()` — simplest is to drop the literal default and fall back in code. For `Serve`, `Audit`, `Status`, change the field to:

```rust
        #[arg(long)]
        config: Option<PathBuf>,
```

and at each use site resolve it:

```rust
let config = config.unwrap_or_else(deectx::home::config_path);
```

- [ ] **Step 3: Handle the new verbs + default in `main`**

In `src/main.rs`, replace `let cli = Cli::parse();` and the `match cli.cmd { … }` opening so a missing subcommand renders status:

```rust
    let cli = Cli::parse();
    let pm = deectx::lifecycle::OsProcessManager;
    match cli.cmd {
        None => {
            let report = deectx::lifecycle::status(&pm)?;
            println!("{}", deectx::lifecycle::render_status(&report));
        }
        Some(Cmd::Start) => {
            let report = deectx::lifecycle::start(&pm)?;
            println!("{}", deectx::lifecycle::render_status(&report));
        }
        Some(Cmd::Stop) => {
            deectx::lifecycle::stop(&pm)?;
            println!("deeCtx stopped; tools restored to direct API access.");
        }
        Some(Cmd::Uninstall { purge }) => {
            let delete = purge || {
                use std::io::Write;
                print!("Also delete config + ledger (audit data)? [y/N] ");
                std::io::stdout().flush().ok();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
            };
            deectx::lifecycle::uninstall(&pm, delete)?;
            println!("deeCtx removed. To remove the binary: `scoop uninstall deectx` (or brew/cargo).");
        }
```

Then wrap the remaining existing arms (`Serve`, `Audit`, `Status`, `Setup`, `Doctor`, `Unwrap`, `DaemonInstall`, `DaemonUninstall`) in `Some(Cmd::…)` and apply the `config.unwrap_or_else` resolution from Step 2 inside `Serve`/`Audit`/`Status`.

- [ ] **Step 4: Keep back-compat aliases**

`Setup` should now delegate to `start`, `Unwrap` to `stop`. Replace the existing `Cmd::Setup => { … }` and `Cmd::Unwrap => { … }` arms:

```rust
        Some(Cmd::Setup) => {
            let report = deectx::lifecycle::start(&pm)?;
            println!("{}", deectx::lifecycle::render_status(&report));
        }
        Some(Cmd::Unwrap) => {
            deectx::lifecycle::stop(&pm)?;
            println!("restored all original configs");
        }
```

Leave `Doctor`, `DaemonInstall`, `DaemonUninstall`, `Status` arms working as before (Status may keep its existing `/stats` output, or call `render_status` — keep existing behavior to avoid churn).

- [ ] **Step 5: Build + smoke test**

Run: `cargo build && cargo run -- --help`
Expected: build succeeds; help lists `start`, `stop`, `uninstall` plus the existing commands.

Run: `cargo run` (no args, with `DEECTX_HOME` set to a temp dir)
Expected: prints the OFF dashboard (no proxy running), exit 0.

- [ ] **Step 6: Run the full suite + lint, then commit**

Run: `cargo test && cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings`

```bash
git add src/main.rs
git commit -m "feat(cli): start/stop/uninstall verbs, bare deectx = status, setup/unwrap aliases"
```

---

### Task 11: Docs — AGENTS.md, README, release note

**Files:**
- Modify: `AGENTS.md`
- Modify: `README.md`

**Interfaces:** none (documentation).

- [ ] **Step 1: Update the AGENTS.md command list**

In `AGENTS.md`, under `## Commands`, replace the command block's top lines so the primary lifecycle is `start`/`stop`/`status`/`uninstall`, and note `setup`→`start`, `unwrap`→`stop` are aliases. Add one line: "Runtime state lives in `~/.deectx/` (`config.toml`, `ledger.jsonl`, `deectx.pid`)." Update the `src/` layout table to add `home.rs` (runtime home + pidfile) and `lifecycle.rs` (start/stop/uninstall/status).

- [ ] **Step 2: Update README install/quick-start**

In `README.md`, replace the "Quick start: install-and-forget" section's commands: `deectx start` (turn on), `deectx` or `deectx status` (dashboard), `deectx stop` (turn off), `deectx uninstall`. Add a one-line note: "deeCtx stores its config and audit ledger in `~/.deectx/`."

- [ ] **Step 3: Commit**

```bash
git add AGENTS.md README.md
git commit -m "docs: document start/stop/status/uninstall lifecycle + ~/.deectx home"
```

---

## Self-Review

**Spec coverage:**
- Runtime home `~/.deectx/` (config/ledger/pid) → Tasks 1, 2, 3, 6. ✓
- Transparent passthrough (mask count_tokens, forward the rest) → Tasks 4, 5. ✓
- Stale-daemon self-replace on update → Task 8 (`start` → `stop_running_proxy`) + Task 9 (`kill`/`is_alive`). ✓
- Lifecycle verbs `start`/`stop`/`uninstall`/`status` + bare `deectx` + aliases → Tasks 8, 10. ✓
- Status dashboard with version-mismatch / port-in-use warnings → Tasks 7, 8. ✓
- Uninstall prompts for data, never removes binary → Tasks 8, 10. ✓
- Error handling (stale pidfile, foreign port owner, partial wiring) → Tasks 8 (`stop_running_proxy`, `wire_tools`, `status` warnings). ✓
- Testing split (pure units + fake ProcessManager + integration) → Tasks 1–9. ✓
- Docs/back-compat → Tasks 10, 11. ✓

**Placeholder scan:** none — every code step contains complete code.

**Type consistency:** `ProcessManager` methods (`spawn_serve`/`is_alive`/`kill`/`port_in_use`), `StatusReport` fields (`running`/`listen`/`running_version`/`current_version`/`tools`/`warnings`), `Pidfile` fields (`pid`/`listen`/`version`), and `forward(...)` signature are used identically across Tasks 4–10. ✓

**Note for the implementer:** `Tool` derives `Copy` (see `setup.rs`), so `(tool, path)` iteration and `tool` reuse compile without clones.
