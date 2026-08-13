# AGENTS.md — deectx

Reference for coding agents (Claude Code, GitHub Copilot, opencode) working in this repo.
Deep design discussion lives in `ARCHITECTURE.md`. Keep this file current — see
[Maintenance](#maintenance).

## What this is

`deectx` (crate `deectx`, Apache-2.0, Rust edition 2021) is a **local-first PII-masking
reverse proxy** for AI coding tools. It sits between the tool (Cursor, opencode, Claude
Code, Codex, Copilot CLI) and an upstream AI API (OpenAI / Anthropic), masks secrets and
personal data before they leave the machine, stores a privacy-safe ledger locally, and lets
the user audit what was masked. It is a **transparent** proxy: endpoints it doesn't handle
explicitly are forwarded verbatim, so wired tools never break.

Must stay **OSS-pure**: this is the open-source, privacy-preserving core. No org/telemetry/user
identity features here — those live in the commercial `deectx-pro` companion repo.

## Commands

Lifecycle (the guided surface — states are ACTIVE ⇄ OFF):

```bash
deectx                  # status dashboard (running? tools wired? warnings?)
deectx start            # turn ON: wire tools + install autostart + start masking (idempotent)
deectx stop             # turn OFF: restore tools to direct API + stop the proxy
deectx uninstall        # stop + restore tools; prompts to delete data; never removes the binary
```

Other:

```bash
deectx serve            # run the masking proxy (spawned by the daemon / `start`); uses ~/.deectx/config.toml
deectx audit --today [--export -]   # ledger summary (JSON to stdout with `-`)
deectx status [--json]  # live masked/redacted counters from the running proxy's /stats
deectx doctor           # per-tool wiring status
# aliases: `setup` -> `start`, `unwrap` -> `stop`; `daemon-install`/`daemon-uninstall` remain
```

Runtime state lives in **`~/.deectx/`** (`$DEECTX_HOME` overrides): `config.toml`,
`ledger.jsonl`, `deectx.pid` — absolute so the daemon and CLI agree regardless of CWD.

Build / test / lint (all from repo root):

```bash
cargo build --release
cargo test                       # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
# Windows without MSVC: build via the GNU toolchain + MSYS2 mingw64 —
#   CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=C:/msys64/mingw64/bin/gcc.exe
#   RUSTFLAGS="-C dlltool=C:/msys64/mingw64/bin/dlltool.exe"
#   cargo +stable-x86_64-pc-windows-gnu test
# (machine-local only; CI's windows-latest has MSVC and needs none of this.)
```

Install paths ship prebuilt binaries first (Scoop / `cargo binstall` / prebuilt Homebrew);
`cargo install` builds from source and needs a C/C++ linker. See `README.md` §Install.

## Project layout

| Path | Responsibility |
|------|----------------|
| `src/lib.rs` | Public library surface; `pub mod` exports |
| `src/main.rs` | CLI (`start`, `stop`, `uninstall`, bare = status, `serve`, `audit`, `status`, `doctor`, `setup`/`unwrap` aliases, `daemon-*`) via clap derive |
| `src/lifecycle.rs` | `start`/`stop`/`uninstall`/`status` + `StatusReport`/`render_status`; `ProcessManager` trait (+ `OsProcessManager`) |
| `src/home.rs` | Runtime home `~/.deectx/` (config/ledger/pidfile paths) + `Pidfile` |
| `src/config.rs` | `Config` struct + TOML loading, serde defaults |
| `src/proxy.rs` | axum routes, transparent fallback, masking walk, method-agnostic forward, rehydration, local dashboard (`GET /`, `/dashboard`, `/audit/today`) |
| `src/dashboard.html` | Self-contained branded HTML/CSS/JS for the local dashboard; embedded via `include_str!`, polls `/stats` + `/audit/today` |
| `src/tray/` | `mod.rs` (event loop, menu, `TrayState`), `icon.rs` (pure RGBA rendering), `proxy_handle.rs` (in-process start/stop) — Windows/macOS only, `#[cfg]`-gated out of Linux builds entirely |
| `src/upstream.rs` | Upstream routing by API-key shape (`sk-ant-…` → Anthropic, `sk-…` → OpenAI) |
| `src/responses_ws.rs` | `/v1/responses` WebSocket proxy (Codex / Copilot CLI) — masked + rehydrated per frame |
| `src/sse.rs` | Streaming SSE rehydration (bounded buffers) |
| `src/ledger.rs` | Append-only JSONL ledger, daily rotation, retention |
| `src/audit.rs` | `AuditSummary` aggregation |
| `src/stats.rs` | Live in-memory counters served at `GET /stats` |
| `src/status.rs` | Formats `/stats` output for `deectx status` |
| `src/masker.rs` | Session-scoped reversible masking (`[EMAIL_1]`) / redaction |
| `src/span.rs` | Detected `Span{start,end,entity,action,text,alert}` |
| `src/allowlist.rs` | Case-insensitive allowlist filter |
| `src/chunk.rs` | Text chunking for NER |
| `src/setup.rs` | Tool discovery + config patching + autostart daemon install/uninstall |
| `src/detect/` | `mod.rs` (chain), `regex.rs`, `secrets.rs`, `ner.rs` (feature `ner`) |
| `src/packs/` | Pack YAML definitions: `default`, `gdpr`, `cdr-au` + loader |
| `config.example.toml` | Documented example config |
| `shims/` | Integration shims for Cursor + opencode |
| `install/` | Homebrew formula + Scoop manifest (`release.yml` refreshes hashes on tag and pushes to the `homebrew-deectx`/`scoop-deectx` repos — see `install/brew/README.md`, `install/scoop/README.md`) |
| `scripts/` | `release.ps1`, `release.sh` (build + package) |
| `tests/` | `proxy_integration.rs`, `passthrough_integration.rs`, `golden_set.rs`, `installers.rs` |
| `.github/workflows/` | `ci.yml` (fmt+clippy+test), `release.yml` (tag builds + publish), `agents-doc-freshness.yml` |

## Core concepts

### Config (`src/config.rs`)
TOML file (`~/.deectx/config.toml` default). Fields: `listen` (127.0.0.1:8787), `upstream`
(`https://api.openai.com`), `upstream_anthropic`, `upstream_responses`, `ledger_path`
(default `~/.deectx/ledger.jsonl`), `ledger_retention_days` (90), `active_packs`, `packs_dir`,
`model_dir`, `allowlist`, `ner`, `stats_enabled`. Partial files fill unspecified fields from
serde defaults. **No env-var config** (the only env override is `DEECTX_HOME`, a path).

### Detection pipeline (`src/detect/`, `src/packs/`)
Packs compose detectors. `DetectorChain` runs all detectors, merges spans (longest wins at same
start), allowlist-filters. `ready()` false → the fail-closed 503 gate can fire.
- `regex.rs`: patterns + optional checksum (`Luhn` cards, `Mod97` IBAN, `AtoTfn` AU TFN).
- `secrets.rs` API keys with Shannon-entropy gate (`entropy_min`).
- `ner.rs` GLiNER-onnx word spans under the `ner` feature.

Built-in packs: `default` (email, credit_card+Luhn, api_key redact), `gdpr` (person/address +
Art.9 special-category all `alert`), `cdr-au` (TFN, Medicare, BSB, driver licence, passport,
Centrelink CRN).

### Masking & reversibility (`src/masker.rs`, `src/span.rs`)
`Mask` → reversible placeholder `[ENTITY_N]`, mapped per-session in-memory.
`Redact` → `[REDACTED_SECRET]`, never stored. `rehydrate` restores placeholders to the client by
descending placeholder length (protects `[EMAIL_10]` vs `[EMAIL_1]`). **Originals are masked
outbound to the model but rehydrated to the local tool.**

### Ledger (`src/ledger.rs`)
Append-only JSONL, daily rotation to `ledger-YYYY-MM-DD.jsonl`, prune after retention days.
Stores only hashes — never raw PII. `LedgerEvent{entity, placeholder, ph_hash, action, alert}`;
`LedgerEntry{ ts, tool, session, events, latency_ms, packs }`. `tool` = user-agent; `session` =
`"s_"+sha256(first_msg)[..8]`. `Ledger::path()` exposes the base path so callers (dashboard,
`audit --today`) can re-read today's entries without duplicating it.

### Proxy (`src/proxy.rs`, `src/upstream.rs`, `src/responses_ws.rs`)
Routes: `GET /healthz`, `GET /stats`, `GET /audit/today` (today's hash-only `AuditSummary` as
JSON), `GET /` + `GET /dashboard` (local dashboard, `src/dashboard.html`), `POST
/v1/chat/completions` (OpenAI), `POST /v1/messages` (Anthropic), the `/v1/responses` WebSocket,
and a **catch-all `fallback`** (`handle_passthrough`) for everything else. `/stats`,
`/audit/today`, and the dashboard are only mounted when `stats_enabled`.
`passthrough_should_mask` masks prompt-bearing endpoints (`/v1/messages/count_tokens`) via the
same `mask_walk`; other paths (`/v1/models`, …) forward verbatim. Completions and passthrough
share the method-agnostic `forward`. Streams via `SseRehydrator`; non-streams rehydrated via
`rehydrate_response`; WS frames per frame.

`mask_walk` skips `tool_use_id`/`tool_call_id`/`call_id`, and bare `id` when the enclosing object
is a `tool_use`/`tool_result`/`function` block (`is_protocol_id_field`) — these are high-entropy
protocol ids the secrets detector would otherwise flag as an `api_key` and mask, corrupting the id
and breaking the upstream schema. It also skips `thinking`/`redacted_thinking` blocks entirely:
the `signature` on a `thinking` block is cryptographically bound to the exact `thinking` text, so
masking either one invalidates the signature and upstream rejects the whole request with 400 — same
failure mode as the protocol-id case. `mask_or_forward_unmasked` wraps `mask_walk` + serialization in
`catch_unwind`: any panic or serialize failure forwards the original body unmasked (never blocks
or corrupts the request) and increments `stats.errors`, surfaced in `/stats`, `deectx status`, and
the dashboard. `handle_completion`/`handle_passthrough` also retry once, unmasked, whenever a
*masked* request comes back 400 from upstream — that status only means "the masker corrupted
something," never "the client sent a bad request," so it fails open with a `tracing::error!` and a
`stats.errors` count rather than surfacing the bug as a broken tool session.

### Lifecycle & autostart (`src/lifecycle.rs`, `src/setup.rs`)
`lifecycle::start` (idempotent) stops any running/stale proxy (via `deectx.pid` + `ProcessManager`),
wires tools, installs the login autostart, and spawns the resident process — `tray` on
Windows/macOS, `serve` on Linux (`ProcessManager::spawn_serve`), matching what `install_daemon`
wires for next login; `stop` unwraps tools + kills the
proxy + removes autostart; `uninstall` = stop + optional data delete. `stop`/`uninstall` return
`Vec<String>` restore warnings (never silently swallowed) so `deectx stop` tells you when a tool
config couldn't be restored instead of claiming success. `setup.rs` still owns the low-level tool
`discover`/`patch_config`/`unwrap` (which now attempts every tool even if one fails, returning
`Vec<(Tool, Error)>`) and the per-OS daemon artifacts. `unwrap` also removes each tool's
`extra_artifacts` — currently opencode's fail-closed plugin (`shims/opencode/deectx-plugin.ts`,
hand-installed per `shims/README.md`) — since those live outside `patch_config`'s `.bak` tracking
entirely; leaving that plugin behind after `stop` turns "restore direct access" into "opencode
throws on every tool call." A session that already loaded the plugin still needs a restart —
that part can't be fixed from the filesystem side.

### Tray (`src/tray/`, Windows/macOS only)
`deectx tray` hosts the masking proxy inside the same process as the tray
icon (`ProxyHandle` wraps `proxy::run_proxy_with_shutdown`) rather than
spawning a separate `serve` — so "Stop Masking" from the tray closes the
listener but keeps the icon (and the ability to "Start Masking" again)
alive. `daemon_artifact_for` now points Windows/macOS autostart at `<exe>
tray` instead of `<exe> serve`; Linux is untouched and gets no tray code at
all (`#[cfg(any(target_os = "windows", target_os = "macos"))]` on the module
in `lib.rs`, target-gated deps in `Cargo.toml`). See
`docs/superpowers/specs/2026-08-12-tray-icon-design.md` for the full design
and its documented limitations (no coordination between the tray's internal
start/stop and CLI `start`/`stop` run concurrently; tray-icon/muda/tao API
correctness verified only on Windows, not macOS).

## Conventions
- Rust 2021, `anyhow` for errors, serde derive, chrono UTC. Tests live next to code
  (and in `tests/`); TDD. `cargo fmt` + `clippy -D warnings` must be clean.
- No env-var config (TOML only); `DEECTX_HOME` is the sole path override. No raw PII written
  anywhere; hashes only — this extends to the dashboard, which shows entity types/counts, never
  masked values.
- Match existing module boundaries — add features as new focused `src/` files, exported from
  `lib.rs`, never pile into `proxy.rs`.
- Masking must fail open, never closed, on an internal bug (panic/serialize error): forward the
  request unmasked and record it to `stats.errors` rather than corrupting the request or dropping
  the connection. This is distinct from the deliberate `fail_closed` config gate (503 when a
  required detector like NER is unavailable), which is an intentional, opt-in compliance control
  and must keep blocking.

## Where to make a change
- New scanner type → add to `src/detect/` + wire into `detect/mod.rs` chain.
- New built-in pack → new YAML in `src/packs/` + registration in `packs/mod.rs`.
- New route / upstream format → `src/proxy.rs` (+ `upstream.rs` for routing, `responses_ws.rs` for WS).
- Masking semantics → `src/masker.rs`. Ledger shape → `src/ledger.rs`. Reporting → `src/audit.rs`.
- Live counters → `src/stats.rs` / `src/status.rs`.
- Lifecycle / start-stop / process mgmt → `src/lifecycle.rs`. Runtime paths / pidfile → `src/home.rs`.
- Tool wiring / autostart → `src/setup.rs`. CLI surface → `src/main.rs`. Defaults → `src/config.rs`.

## Maintenance
Update this file whenever you add or move a **module, route, CLI command, config field, or pack**.
Keep the layout table and Commands section in sync with `src/` and `src/main.rs`. A CI check
(`.github/workflows/agents-doc-freshness.yml`) posts a warning annotation when `src/**` or
`Cargo.toml` change in a commit/PR without a matching `AGENTS.md` edit — treat it as a nudge, not
a gate.
