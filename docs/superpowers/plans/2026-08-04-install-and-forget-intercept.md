# Install-and-forget Transparent Interception — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `deectx` zero-config: a one-time `deectx setup` auto-wires installed AI tools to the local proxy, the proxy routes each request to the correct real upstream by API-key shape, serves Codex/Copilot's Responses WebSocket protocol, and exposes live `/stats`.

**Architecture:** Additive changes to the pure masking engine. A new `upstream` module classifies providers by `Authorization` key shape; the proxy uses it to pick the upstream base URL. A new `responses_ws` module terminates the OpenAI Responses API WebSocket (`/v1/responses`) and pipes it through the existing mask/rehydrate pipeline. A new `stats` module holds live atomic counters exposed at `/stats` and via `deectx status`. A new `setup` module detects installed tools, rewrites their configs (with `.bak`) to point at `127.0.0.1:8787`, installs a warm autostart daemon, and provides `doctor`/`unwrap`.

**Tech Stack:** Rust 2021, axum 0.7 (+ `ws`), tokio, tokio-tungstenite, serde, serde_json, toml, reqwest, clap 4 derive, sha2. Configs patched: Claude Code `~/.claude/settings.json`, Codex `~/.codex/config.toml`, opencode `~/.config/opencode/opencode.json` (v1 schema).

## Global Constraints

- Engine must stay a pure masking core. All new behavior is additive; `src/proxy.rs`'s mask→forward→rehydrate pipeline and the ledger format are unchanged.
- Performance non-negotiable: no per-request process spawn, pooled upstream connections, stream-in/stream-out (never buffer whole payloads). Local overhead < 10ms p99 on default packs; no regression to TTFT.
- No raw PII anywhere: ledger stores hashes only; outbound masking, inbound rehydration identical to today.
- Zero-config after `setup`: the `upstream` config field remains the fallback with today's default (`https://api.openai.com`); no new required config fields. All new config fields have working defaults.
- Locked-OAuth tools (Claude Pro/Max, Copilot built-in) are skipped by `setup` with an explanatory message, never silently broken.
- Every config file the patcher touches gets a `.bak` copy; `unwrap` restores all. Patching is idempotent.
- Fail-closed default preserved: if a required detector is unavailable, proxy returns 503, never leaks.
- Windows builds need `$env:PATH` prefixed with `C:\msys64\mingw64\bin;` before cargo (dlltool). Run cargo from `C:\self\deectx`.
- Follow the repo conventions (AGENTS.md): Rust 2021, `anyhow`, serde derive, tests next to code, `cargo fmt` + `cargo clippy --all-targets -- -D warnings` clean. Add features as new focused `src/` files exported from `lib.rs`; never pile into `proxy.rs`.
- The 3 HTTP routes must all go through the same warm `reqwest::Client`; the WS handler uses its own tokio-tungstenite connection.

---

### Task 1: Dynamic upstream router (`src/upstream.rs`)

**Files:**
- Create: `src/upstream.rs`
- Modify: `src/lib.rs` (add `pub mod upstream;`)
- Modify: `src/proxy.rs` (use router to select base URL)

**Interfaces:**
- Produces: `pub enum Provider { OpenAI, Anthropic, Unknown }` and `pub fn classify(auth: &str) -> Provider`.
- Consumes (from `proxy.rs`): existing `AppState` fields `upstream: String`, `anthropic_upstream: String`.

- [ ] **Step 1: Write the failing test**

Create `src/upstream.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAI,
    Anthropic,
    Unknown,
}

/// Classify the upstream provider from an Authorization header value.
pub fn classify(auth: &str) -> Provider {
    let key = auth.trim();
    if key.starts_with("sk-ant-") {
        Provider::Anthropic
    } else if key.starts_with("sk-") {
        Provider::OpenAI
    } else {
        Provider::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_key_shape_is_anthropic() {
        assert_eq!(classify("sk-ant-api03-abcdef"), Provider::Anthropic);
    }

    #[test]
    fn openai_key_shape_is_openai() {
        assert_eq!(classify("sk-proj-abcdef"), Provider::OpenAI);
    }

    #[test]
    fn bare_bearer_is_unknown() {
        assert_eq!(classify("Bearer somelongtoken"), Provider::Unknown);
    }

    #[test]
    fn leading_whitespace_is_trimmed() {
        assert_eq!(classify("  sk-proj-x  "), Provider::OpenAI);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test upstream`
Expected: FAIL — compile error `unresolved import crate::upstream` (no `pub mod` yet) or no tests run.

- [ ] **Step 3: Register module and wire into proxy**

In `src/lib.rs`, add the module line (keep alphabetical order):

```rust
pub mod upstream;
```

In `src/proxy.rs`, change the base-URL selection in `forward_raw` (currently `let base = match format {...}`) to classify from the request headers:

```rust
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
```

The path (`/v1/chat/completions` vs `/v1/messages`) is still chosen by `format` unchanged. Key-shape only picks the base host; an Anthropic-keyed request hitting the OpenAI route still uses the Anthropic base (path stays per-route — the known-good configuration is route-matching key shape, so this is a safe superset).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test upstream && cargo test proxy`
Expected: PASS — 4 upstream tests; existing proxy tests still pass.

- [ ] **Step 5: Run lint**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/upstream.rs src/lib.rs src/proxy.rs
git commit -m "feat: dynamic upstream router by API-key shape"
```

---

### Task 2: Live `/stats` counters + route

**Files:**
- Create: `src/stats.rs`
- Modify: `src/lib.rs` (add `pub mod stats;`)
- Modify: `src/proxy.rs` (AppState field, counter bumps, route)
- Modify: `src/config.rs` (optional `stats_enabled`, default true)

**Interfaces:**
- Produces: `LiveStats` (methods `record_request()`, `record_event(action: &str, alert: bool)`, `snapshot() -> StatsSnapshot`); `StatsSnapshot` derives `serde::Serialize`.
- Consumes: `proxy::mask_content` (where each `LedgerEvent` is pushed) and `proxy::handle_completion` (per request).

- [ ] **Step 1: Write the failing test**

Create `src/stats.rs`:

```rust
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};

/// Live, process-scoped counters. Cheap atomic increments; never read from disk.
#[derive(Default)]
pub struct LiveStats {
    requests: AtomicU64,
    masked: AtomicU64,
    redacted: AtomicU64,
    alerts: AtomicU64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct StatsSnapshot {
    pub requests: u64,
    pub masked: u64,
    pub redacted: u64,
    pub alerts: u64,
}

impl LiveStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_request(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_event(&self, action: &str, alert: bool) {
        if action == "redact" {
            self.redacted.fetch_add(1, Ordering::Relaxed);
        } else {
            self.masked.fetch_add(1, Ordering::Relaxed);
        }
        if alert {
            self.alerts.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            requests: self.requests.load(Ordering::Relaxed),
            masked: self.masked.load(Ordering::Relaxed),
            redacted: self.redacted.load(Ordering::Relaxed),
            alerts: self.alerts.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_is_zero() {
        let s = LiveStats::new();
        assert_eq!(
            s.snapshot(),
            StatsSnapshot { requests: 0, masked: 0, redacted: 0, alerts: 0 }
        );
    }

    #[test]
    fn records_requests_and_mask_events() {
        let s = LiveStats::new();
        s.record_request();
        s.record_event("mask", false);
        s.record_event("redact", true);
        let snap = s.snapshot();
        assert_eq!(snap.requests, 1);
        assert_eq!(snap.masked, 1);
        assert_eq!(snap.redacted, 1);
        assert_eq!(snap.alerts, 1);
    }

    #[test]
    fn serializes_to_json() {
        let s = LiveStats::new();
        s.record_request();
        let json = serde_json::to_string(&s.snapshot()).unwrap();
        assert!(json.contains("\"requests\":1"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test stats`
Expected: FAIL — `unresolved import crate::stats` (not registered yet).

- [ ] **Step 3: Add config flag**

In `src/config.rs`, add the field to the struct (after `upstream_anthropic`) and to `impl Default`:

```rust
    #[serde(default = "default_stats_enabled")]
    pub stats_enabled: bool,
```

And in the defaults helpers:

```rust
fn default_stats_enabled() -> bool {
    true
}
```

In `impl Default for Config`, add `stats_enabled: default_stats_enabled(),`.

- [ ] **Step 4: Wire counters and route into proxy**

In `src/proxy.rs`:

1. Add `use crate::stats::LiveStats;`.
2. In `AppState`, add `stats: std::sync::Arc<LiveStats>,`.
3. In `serve_with_listener`, when constructing `AppState`, add `stats: std::sync::Arc::new(LiveStats::new()),`.
4. In `handle_completion`, right before the ledger append, add `st.stats.record_request();`.
5. In `mask_content`, in the loop that pushes each `LedgerEvent`, add a counter bump using the same strings already computed:

```rust
        st.stats.record_event(
            if matches!(s.action, Action::Mask) { "mask" } else { "redact" },
            s.alert,
        );
```

6. Register the route in `serve_with_listener`, guarded by the config flag:

```rust
    let app = Router::new()
        .route("/healthz", axum::routing::get(|| async { "ok" }))
        .route("/v1/chat/completions", axum::routing::post(handle_chat_openai))
        .route("/v1/messages", axum::routing::post(handle_chat_anthropic));

    let app = if cfg.stats_enabled {
        app.route("/stats", axum::routing::get(handle_stats))
    } else {
        app
    };
    let app = app.with_state(state);
```

7. Add the handler:

```rust
async fn handle_stats(State(st): State<Arc<AppState>>) -> axum::Json<crate::stats::StatsSnapshot> {
    axum::Json(st.stats.snapshot())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test stats && cargo test proxy`
Expected: PASS.

- [ ] **Step 6: Add an integration check that `/stats` is served**

Append to `tests/proxy_integration.rs` (reuse the existing server-spawn helper there — bind `127.0.0.1:0`, return `(SocketAddr, JoinHandle)`):

```rust
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
```

Match the existing `spawn_proxy` helper's signature; if it does not exist or takes different args, create a minimal helper mirroring the file's existing pattern.

- [ ] **Step 7: Run lint**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/stats.rs src/lib.rs src/proxy.rs src/config.rs tests/proxy_integration.rs
git commit -m "feat: live /stats counters and endpoint"
```

---

### Task 3: `deectx status` CLI

**Files:**
- Modify: `src/main.rs` (add `Cmd::Status` + `format_status`)
- Modify: `src/lib.rs` (export `format_status` or keep in `main`)
- Modify: `Cargo.toml` (add `blocking` feature to `reqwest`)

**Interfaces:**
- Produces: `Cmd::Status { config: PathBuf }` parsed by clap; prints JSON (or human-readable) summary of live stats.
- Consumes: `GET /stats` on the proxy's listen address (default `127.0.0.1:8787`).

- [ ] **Step 1: Add `blocking` feature to reqwest**

In `Cargo.toml`:

```toml
reqwest = { version = "0.12", features = ["json", "stream", "blocking"], default-features = false }
```

(The `blocking` feature pulls in `tokio`'s runtime; the binary already builds under tokio. If `default-features = false` is not already set, set it.)

- [ ] **Step 2: Add the CLI subcommand**

In `src/main.rs`, extend the clap enum:

```rust
#[derive(clap::Parser)]
pub enum Cmd {
    Serve { ... },
    Audit { ... },
    Status {
        #[arg(long, default_value = "config.toml")]
        config: std::path::PathBuf,
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
}
```

In `main()`, add the arm (place before `Cmd::Audit`):

```rust
Cmd::Status { config, json } => {
    let cfg = deectx::config::Config::load(&config)?;
    let url = format!("http://{}/stats", cfg.listen);
    let body = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?
        .get(&url)
        .send()?
        .error_for_status()?
        .text()?;
    if *json {
        println!("{body}");
    } else {
        println!("{}", format_status(&body)?);
    }
}
```

- [ ] **Step 3: Add `format_status`**

In `src/main.rs` (or a new `src/status.rs` — prefer keeping main lean; put it in `src/status.rs` if it grows):

```rust
fn format_status(json: &str) -> anyhow::Result<String> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let reqs = v["requests"].as_u64().unwrap_or(0);
    let masked = v["masked"].as_u64().unwrap_or(0);
    let redacted = v["redacted"].as_u64().unwrap_or(0);
    let alerts = v["alerts"].as_u64().unwrap_or(0);
    Ok(format!(
        "proxy: up\nrequests: {reqs}\nmasked: {masked}\nredacted: {redacted}\nalerts: {alerts}"
    ))
}
```

- [ ] **Step 4: Test manually**

Run from `C:\self\deectx`: `cargo build` then `cargo run -- status`. If proxy not running, expect a connect error (acceptable; `status` fails fast with a clear message).

- [ ] **Step 5: Add a unit test for `format_status`**

```rust
#[test]
fn format_status_renders_json() {
    let out = format_status(r#"{"requests":3,"masked":2,"redacted":1,"alerts":1}"#).unwrap();
    assert!(out.contains("requests: 3"));
    assert!(out.contains("redacted: 1"));
}
```

- [ ] **Step 6: Run lint + tests**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test status`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs Cargo.toml
git commit -m "feat: deectx status CLI wrapping live /stats"
```

---

### Task 4: Responses API WebSocket endpoint (`src/responses_ws.rs`)

**Files:**
- Create: `src/responses_ws.rs`
- Modify: `src/lib.rs` (add `pub mod responses_ws;`)
- Modify: `src/config.rs` (add `upstream_responses` field)
- Modify: `src/proxy.rs` (make internals `pub(crate)`, add field + route)
- Modify: `Cargo.toml` (axum `ws` feature, `tokio-tungstenite`)

**Interfaces:**
- Produces: `ws_handler(State, ws::WebSocketUpgrade, headers) -> Response`.
- Consumes: `AppState` fields `upstream`, `anthropic_upstream`, `upstream_responses`, `client`, `stats`, `ledger`; `mask_walk`; `SseRehydrator`.

- [ ] **Step 1: Add dependencies**

In `Cargo.toml`:

```toml
axum = { version = "0.7", features = ["ws", "tokio"] }
tokio-tungstenite = "0.26"
```

- [ ] **Step 2: Add config field**

In `src/config.rs`, add to `Config` struct and `impl Default`:

```rust
    #[serde(default = "default_upstream_responses")]
    pub upstream_responses: String,
```

```rust
fn default_upstream_responses() -> String {
    "https://api.openai.com/v1/responses".to_string()
}
```

- [ ] **Step 3: Make proxy internals `pub(crate)`**

In `src/proxy.rs`:
- Change `struct AppState` to `pub(crate) struct AppState`.
- Make `mask_walk`, `mask_content`, `rehydrate_response` and their types `pub(crate)`.
- Add to `AppState`:

```rust
    pub(crate) upstream_responses: String,
    pub(crate) client: reqwest::Client,
    pub(crate) stats: std::sync::Arc<crate::stats::LiveStats>,
```

(If `client` already exists in `AppState`, just make it `pub(crate)`.)

- [ ] **Step 4: Register route**

In `serve_with_listener`, add to the `Router`:

```rust
        .route("/v1/responses", axum::routing::get(crate::responses_ws::ws_handler))
```

Pass `upstream_responses: cfg.upstream_responses.clone()` when constructing `AppState`.

- [ ] **Step 5: Write the WebSocket handler**

Create `src/responses_ws.rs`:

```rust
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use crate::proxy::AppState;
use crate::sse::SseRehydrator;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(st): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, st))
}

async fn handle_socket(mut socket: WebSocket, st: Arc<AppState>) {
    // Upgrade our end to the upstream Responses WS.
    let mut upstream = match tokio_tungstenite::connect_async(&st.upstream_responses).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    serde_json::json!({"type": "error", "error": {"message": format!("upstream connect failed: {e}")}}).to_string(),
                ))
                .await;
            return;
        }
    };

    let session = "ws_session".to_string();
    let rehydrator = Arc::new(Mutex::new(SseRehydrator::new(64)));

    // Bidirectional pump.
    // Client -> upstream: mask via mask_walk; Server -> client: rehydrate via SseRehydrator.
    loop {
        tokio::select! {
            msg = socket.recv() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(text) => {
                        if let Some(out) = mask_outbound(&text, &st) {
                            let _ = upstream.send(tokio_tungstenite::tungstenite::Message::Text(out)).await;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            msg = upstream.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    tokio_tungstenite::tungstenite::Message::Text(text) => {
                        let out = rehydrate_inbound(&text, &st, &rehydrator);
                        let _ = socket.send(Message::Text(out)).await;
                    }
                    tokio_tungstenite::tungstenite::Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    st.ledger.append(/* LedgerEntry for this WS session, tool "responses-ws" */);
}
```

- [ ] **Step 6: Implement masking + rehydration helpers**

Add to `src/responses_ws.rs`:

```rust
fn mask_outbound(text: &str, st: &Arc<AppState>) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let masked = crate::proxy::mask_walk(value, &st.masker, &st, true)?;
    Some(masked.to_string())
}

fn rehydrate_inbound(
    text: &str,
    st: &Arc<AppState>,
    rehydrator: &Arc<Mutex<SseRehydrator>>,
) -> String {
    let mut r = rehydrator.lock().unwrap();
    let bytes = r.push(text.as_bytes(), &session(), &st.masker);
    String::from_utf8(bytes).unwrap_or_default()
}
```

(These reference `mask_walk` and `SseRehydrator` APIs as they exist in `src/proxy.rs` and `src/sse.rs`. Adjust signatures to the actual current code.)

- [ ] **Step 7: Verify builds**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 8: Add an integration test (optional but encouraged)**

In `tests/proxy_integration.rs`, connect a WS client to `{addr}/v1/responses`, send a minimal `response.create`, assert a `response.output_text.delta` or `response.completed` event arrives.

- [ ] **Step 9: Run lint**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add src/responses_ws.rs src/lib.rs src/config.rs src/proxy.rs Cargo.toml tests/proxy_integration.rs
git commit -m "feat: Responses API WebSocket endpoint with mask/rehydrate"
```

---

### Task 5: Auto-wiring installer (`src/setup.rs`)

**Files:**
- Create: `src/setup.rs`
- Modify: `src/lib.rs` (add `pub mod setup;`)
- Modify: `src/main.rs` (add `Cmd::Setup`, `Cmd::Doctor`, `Cmd::Unwrap`)
- Modify: `src/config.rs` (add `setup` toggle / daemon settings, all defaulted)

**Interfaces:**
- Produces:
  - `enum Tool { ClaudeCode, Codex, Opencode }`
  - `fn discover() -> Vec<(Tool, PathBuf)>` — find installed tools' config paths on the current OS.
  - `fn patch_config(tool: Tool, path: PathBuf) -> anyhow::Result<PatchResult>` — rewrite to point at `127.0.0.1:8787`, backing up original to `path + ".bak"` (idempotent).
  - `fn doctorate() -> anyhow::Result<String>` — verify wiring, report missing/misconfigured tools.
  - `fn unwrap() -> anyhow::Result<()>` — restore all `.bak` files.
- Consumes: none external; writes to `~/.claude/settings.json`, `~/.codex/config.toml`, `~/.config/opencode/opencode.json`.

- [ ] **Step 1: Define the tool model + discovery**

Create `src/setup.rs`:

```rust
use std::path::PathBuf;
use anyhow::{anyhow, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ClaudeCode,
    Codex,
    Opencode,
}

impl Tool {
    /// Where the tool stores its user-level config.
    pub fn config_path(env: &std::env::Vars) -> Result<Option<PathBuf>> {
        let home = env::home()?;
        Ok(match self {
            Tool::ClaudeCode => Some(home.join(".claude").join("settings.json")),
            Tool::Codex => Some(home.join(".codex").join("config.toml")),
            Tool::Opencode => Some(home.join(".config").join("opencode").join("opencode.json")),
        })
    }
}

/// Detect which tools are installed and have a config we can patch.
pub fn discover() -> Vec<(Tool, PathBuf)> {
    let mut out = Vec::new();
    for tool in [Tool::ClaudeCode, Tool::Codex, Tool::Opencode] {
        if let Ok(Some(p)) = tool.config_path() {
            if p.exists() {
                out.push((tool, p));
            }
        }
    }
    out
}
```

(Add a small `fn config_path(&self)` helper. Adjust for Windows where home is `%USERPROFILE%` via `dirs::home_dir()` or `env::var("USERPROFILE")`.)

- [ ] **Step 2: Write the patcher (idempotent, with `.bak`)**

```rust
pub enum PatchResult {
    AlreadyPatched,
    Patched,        // created .bak and wrote new config
}

pub fn patch_config(tool: Tool, path: &PathBuf) -> Result<PatchResult> {
    let backup = path.with_extension(format!("{}.bak", path.extension().unwrap_or_default().to_string_lossy()));

    // Early-out if already pointing at the proxy.
    let original = std::fs::read_to_string(path)?;
    if original.contains("127.0.0.1:8787") {
        return Ok(PatchResult::AlreadyPatched);
    }

    // Back up only once (never clobber an existing .bak the user made).
    if !backup.exists() {
        std::fs::copy(path, &backup)?;
    }

    let patched = rewrite_for(tool, &original)?;
    std::fs::write(path, patched)?;
    Ok(PatchResult::Patched)
}
```

- [ ] **Step 3: Provider-specific rewriting**

```rust
fn patch_for(tool: Tool, original: &str) -> Result<String> {
    match tool {
        // Claude Code: settings.json -> {"env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:8787"}}
        Tool::ClaudeCode => {
            let mut v: serde_json::Value = serde_json::from_str(original)?;
            v["env"]["ANTHROPIC_BASE_URL"] = serde_json::json!("http://127.0.0.1:8787");
            serde_json::to_string_pretty(&v).map_err(Into::into)
        }
        // Codex: config.toml -> [model_providers.proxy] base_url/codex_autouse; or openai_base_url
        Tool::Codex => Ok(format!("{original}\n\n# deectx auto-wiring\nopenai_base_url = \"http://127.0.0.1:8787\"\n")),
        // opencode: opencode.json -> {"provider":{"anthropic":{"options":{"baseURL":"http://127.0.0.1:8787"}}}}
        Tool::Opencode => {
            let mut v: serde_json::Value = serde_json::from_str(original)?;
            v["provider"]["anthropic"]["options"]["baseURL"] = serde_json::json!("http://127.0.0.1:8787");
            serde_json::to_string_pretty(&v).map_err(Into::into)
        }
    }
}
```

Verify these against the real config schemas during implementation (see research notes): Codex's `openai_base_url` vs `[model_providers.<name>]` (use the custom provider block, not the legacy key, to avoid heap/URN mismatches); Claude Code uses `ANTHROPIC_BASE_URL`; opencode may be `opencode.json` or `.opencode.json`. Adjust the patch blocks to match whatever actual key names the installed tool version reads.

- [ ] **Step 4: Add CLI subcommands**

In `src/main.rs`, extend the clap enum and dispatch:

```rust
Cmd::Setup => {
    let found = deectx::setup::discover();
    for (tool, path) in &found {
        match deectx::setup::patch_config(*tool, path)? {
            PatchResult::AlreadyPatched => println!("{tool:?}: already wired"),
            PatchResult::Patched => println!("{tool:?}: patched -> {path}"),
        }
    }
    println!("done; start the proxy: deectx serve");
}
Cmd::Doctor => {
    println!("{}", deectx::setup::doctor()?);
}
Cmd::Unwrap => {
    deectx::setup::unwrap()?;
    println!("restored all original configs");
}
```

- [ ] **Step 5: Doctor + unwrap**

```rust
pub fn doctor() -> Result<String> {
    let mut lines = Vec::new();
    for (tool, path) in discover() {
        let content = std::fs::read_to_string(&path)
            .map_err(|_| anyhow!("cannot read {tool} config at {path}"))?;
        let ok = content.contains("127.0.0.1:8787");
        lines.push(format!("{tool:?}: {}", if ok { "OK (wired)" } else { "NOT WIRED" }));
    }
    Ok(lines.join("\n"))
}

pub fn unwrap() -> Result<()> {
    for (tool, path) in discover() {
        let backup = format!("{path}.bak");
        if std::path::Path::new(&backup).exists() {
            std::fs::rename(&backup, &path)?;
        }
    }
    Ok(())
}
```

- [ ] **Step 6: Handle locked-OAuth tools gracefully**

When `discover()` finds a tool whose traffic goes through locked OAuth (Claude Pro/Max, Copilot built-in), skip it: `println!("{tool}: locked provider, cannot intercept (OAuth-managed)")` instead of patching. Detect via config markers (e.g. `"oauth_account"` present, or absence of a writable API-key base URL).

- [ ] **Step 7: Tests + lint + commit**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test setup`
Add unit tests for `patch_for` round-trips (patch → unwrap → byte-identical) and for `discover` on a temp home dir.
Commit: `git commit -m "feat: auto-wiring setup, doctor, unwrap for Claude/Codex/opencode"`

---

### Task 6: Warm autostart daemon

**Files:**
- Modify: `src/main.rs` (hook into `Cmd::Setup` to install a daemon; add `Cmd::DaemonInstall`/`Cmd::DaemonUninstall`)

**Interfaces:**
- Produces: install/uninstall hooks for the platform's autostart mechanism.
- Consumes: the `deectx serve` binary path and current config.

- [ ] **Step 1: Define the daemon-install function (per-OS)**

```rust
pub fn install_daemon() -> Result<()> {
    let exe = std::env::current_exe()?;
    let home = /* user home */;
    match std::env::consts::OS {
        "macos" => {
            // Write ~/Library/LaunchAgents/com.deectx.proxy.plist that runs `exe serve`
        }
        "linux" => {
            // Write ~/.config/systemd/user/deectx.service + `systemctl --user daemon-reload`
        }
        "windows" => {
            // Register a Task Scheduler task / startup shortcut pointing at `exe serve`
        }
        _ => return Err(anyhow!("unsupported OS")),
    }
    Ok(())
}
```

- [ ] **Step 2: Wire into `Cmd::Setup`**

After patching configs, call `deectx::setup::install_daemon()` and start the proxy once (warm start). Report the listen URL.

- [ ] **Step 3: `Cmd::DaemonUninstall`**

Remove the plist/service/task created in step 1. Idempotent (no-op if absent).

- [ ] **Step 4: Test + lint + commit**

Verify the daemon file is written and points at the current binary with `serve`. Note: don't actually `systemctl enable` in CI.
Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Commit: `git commit -m "feat: warm autostart daemon install/uninstall per OS"`

---

### Task 7: Documentation + example config update (final)

- [ ] **Step 1: Update `config.example.toml`**

Add the new fields with defaults and comments:
```toml
# Whether the live /stats endpoint is served (default true).
# stats_enabled = true

# Responses API WebSocket upstream (used by Codex / Copilot CLI).
# upstream_responses = "https://api.openai.com/v1/responses"
```

- [ ] **Step 2: Update `README.md`**

Add a "Quick start: install-and-forget" section: `deectx setup`, what it wires (Claude Code, Codex, opencode; skips locked-OAuth), `doctor`, `unwrap`, `deectx status`, and `/stats`.

- [ ] **Step 3: Update `ARCHITECTURE.md`**

Document the new modules: `upstream`, `stats`, `responses_ws`, `setup`. Note the daemon, the WS protocol handling, and the key-shape routing with the fallback default.

- [ ] **Step 4: Final full verification**

Run (from `C:\self\deectx`, with the mingw PATH prefix on Windows):
```
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```
Expected: all green; the whole test suite (62 existing + new) passes. Then commit:
```bash
git commit -m "docs: install-and-forget quick start, config, ARCHITECTURE"
```

---

## Done / Definition of Done

- [ ] `cargo build` and `cargo test` pass from a clean checkout.
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check` clean.
- [ ] `deectx setup` wires Claude Code, Codex, and opencode configs to the local proxy with `.bak` backups; idempotent.
- [ ] `deectx doctor` reports per-tool wiring status; `deectx unwrap` restores all originals byte-for-byte.
- [ ] The proxy routes Anthropic-shaped keys to `anthropic_upstream` and OpenAI-shaped keys to `upstream`, with `upstream_server`-default fallback preserved.
- [ ] `/v1/responses` WebSocket pipes through mask→forward→rehydrate without buffering whole payloads.
- [ ] A warm daemon keeps the proxy up from login; `DaemonUninstall` removes it.
- [ ] `/stats` and `deectx status` report live counts; JSON and human-readable both correct.
- [ ] Documentation reflects all changes (README, ARCHITECTURE, config.example.toml).
- [ ] No raw PII written to any ledger or log; existing 62 tests still pass; zero new warnings.
