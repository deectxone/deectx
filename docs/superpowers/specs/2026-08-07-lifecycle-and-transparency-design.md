# deeCtx — Lifecycle UX & Transparent Proxy — Design

*Date: 2026-08-07 · Status: approved design, pre-implementation*

## Problem

Field testing of 0.2.0 surfaced three failures that make deeCtx *block* the user
instead of transparently masking:

1. **"Prompt is too long" / API errors in Claude Code.** `deectx setup` points
   `ANTHROPIC_BASE_URL` at the proxy, so *all* of Claude Code's Anthropic traffic
   flows through deeCtx. But the router only handles `/v1/messages`,
   `/v1/chat/completions`, `/v1/responses`, `/healthz`, `/stats`
   (`src/proxy.rs`). Claude Code also calls `POST /v1/messages/count_tokens`
   (and `/v1/models`) for context management; those hit **no route → 404**, so
   the tool's token math breaks.
2. **Stale daemon after update.** `scoop update` replaced the binary on disk but
   the *running* 0.1.0 autostart daemon kept port 8787. Result: `deectx status`
   got a 404 on `/stats` (old proxy), and `deectx serve` failed with "address in
   use" (os error 10048).
3. **`deectx audit` shows 0.** The daemon runs `deectx serve` from the Startup
   folder, writing `./ledger.jsonl` there; `audit` reads `./ledger.jsonl` from
   the user's shell CWD — different files. Relative `ledger_path`
   (`src/config.rs` `default_ledger()`) is the culprit.

Beyond the bugs, the lifecycle is a pile of commands (`setup`, `doctor`,
`unwrap`, `daemon-install`, `daemon-uninstall`, `serve`, `status`) with no
coherent "turn it on / off / remove it" story. Users juggle commands even to
uninstall cleanly.

## Goals

- deeCtx is a **transparent reverse proxy**: unhandled endpoints forward
  cleanly; prompt-bearing endpoints are still masked. Tools never break.
- A **guided, minimal lifecycle**: `deectx` (status) · `start` · `stop` ·
  `uninstall`, each doing the whole job (daemon + wiring) for the user.
- Updates "just work": re-running `start` self-replaces a stale daemon.
- `audit`/`status` always read the daemon's ledger.

## Non-goals

- No long-running supervisor/IPC subsystem (that lives in `deectx-pro`).
- No OS-service registration (systemd/launchd/Windows Service) as the primary
  mechanism — the login-autostart artifact stays the source of truth.
- `uninstall` never removes the binary (that's scoop/brew/cargo's job).
- No `pause` verb — the model is just ACTIVE ⇄ OFF.

## UX model

States: **ACTIVE** ⇄ **OFF**, plus uninstalled.

| Command | Behavior |
|---------|----------|
| `deectx` / `deectx status` | Status dashboard (below). |
| `deectx start` | Turn ON (idempotent): ensure config → stop any running/stale proxy → wire non-locked tools → install login autostart → spawn `serve` → health-check → print status. |
| `deectx stop` | Turn OFF: unwrap tools (direct to API) → kill proxy via pidfile → remove autostart. Stays off until `start`. |
| `deectx uninstall` | `stop`, then prompt "also delete config + ledger?" (default **no**), then print the binary-removal command. |

Back-compat aliases (kept, some hidden): `setup`→`start`, `unwrap`→`stop`;
`serve` (internal, spawned by the daemon/`start`), `audit`, `doctor`,
`daemon-install`, `daemon-uninstall` remain.

### Status dashboard

```
deeCtx — ACTIVE ✓ masking
  proxy    running · 127.0.0.1:8787 · v0.2.0 (pid 12345)
  tools    Claude Code ✓   opencode ✓   Codex ✗ (not wired)
  today    418 requests · 37 masked · 3 redacted · 1 alert
  ledger   ~/.deectx/ledger.jsonl

Next: you're protected. `deectx stop` to turn off.
```

Warnings shown with the one command to fix:
- running proxy version ≠ installed binary → `⚠ update installed — run 'deectx start' to apply`
- port 8787 held by a non-deectx process → `⚠ port 8787 in use by another app`
- OFF → tools are direct; `deectx start` to protect.

## Architecture

Approach: **thin lifecycle layer over the existing proxy** — reuse the tested
`setup.rs` and `proxy.rs`; add focused modules.

### 1. `home.rs` — runtime home & state

- `deectx_home() -> PathBuf` = `$DEECTX_HOME` or `~/.deectx`
  (Windows `%USERPROFILE%\.deectx`), created on demand.
- Files (absolute, CWD-independent):
  - `config.toml` — the one config `serve`/`start` read/write.
  - `ledger.jsonl` — new default for `ledger_path`.
  - `deectx.pid` — `{pid, listen, version, started_at}` written by `serve`.
- Changes:
  - `config.rs` `default_ledger()` → `deectx_home().join("ledger.jsonl")`;
    relative `ledger_path` values resolve against home.
  - `main.rs` `serve`/`audit`/`status` default `--config` →
    `<home>/config.toml`. The daemon artifact launches plain `deectx serve`.

### 2. Transparent proxy — `proxy.rs`

- Add `.fallback(handle_passthrough)` to the router.
- Refactor the request-forwarding half of `forward_raw` into a shared,
  **method-agnostic** `forward(method, path, query, headers, body, mask?)` used
  by both the completion handlers and the fallback.
- `handle_passthrough(method, uri, headers, body)`:
  - Choose upstream via existing `upstream::classify` (auth/key shape) + the
    OpenAI/Anthropic format heuristic already in `forward_raw`.
  - **Masking policy by path:**
    - `/v1/messages/count_tokens` → carries the full prompt → run body through
      `mask_walk` before forwarding; honor the fail-closed gate (503 when a
      required detector is unavailable), exactly like completions. Response is a
      token count — no rehydration needed.
    - any other unmatched path (`/v1/models`, batches, …) → forward **verbatim**,
      unmasked.
  - Preserve method, query string, and headers (minus `host`,
    `content-length`, `accept-encoding`).

### 3. `lifecycle.rs` — orchestration

Composes `setup.rs` (`patch_config`/`unwrap`/`install_daemon`/`uninstall_daemon`,
`discover`, `is_locked`, `wired`) with a `ProcessManager`.

- `ProcessManager` trait: `spawn_serve()`, `kill(pid)`, `is_alive(pid)`,
  `port_owner(addr)`. Real impl shells out (`taskkill /F /PID` on Windows,
  `kill` on unix) / checks the pidfile; a fake impl backs tests.
- `start()`, `stop()`, `uninstall(delete_data: bool)`, `status() -> StatusReport`
  as described in the UX table. `start` is idempotent and self-replacing.
- Pidfile read/write/stale-detection helpers.
- `StatusReport` is a plain struct (proxy state, per-tool wiring, today's
  counts, warnings) rendered by a pure formatter — so status is unit-testable.

### 4. `main.rs` — CLI

Add `Start`, `Stop`, `Uninstall`; default (no subcommand) → status. Keep
`Serve`, `Audit`, `Status`, plus the back-compat aliases. Verbs are thin calls
into `lifecycle`.

## Data flow

- **Request (ACTIVE):** tool → `http://127.0.0.1:8787/<path>` → router. Known
  completion path → mask → forward → rehydrate. `count_tokens` → mask → forward.
  Any other path → forward verbatim. Upstream chosen by key shape.
- **Lifecycle:** `start` writes `config.toml` (if absent), kills any pid in
  `deectx.pid`, wires tools, installs autostart, spawns `serve` (which writes a
  fresh pidfile), health-checks. `stop` unwraps tools, kills the pid, removes
  autostart. `status` reads the pidfile + probes `/healthz` and `/stats` +
  checks tool wiring + compares versions.

## Error handling

- Stale pidfile (pid dead) → "not running"; `start` proceeds, `stop` clears it.
- Port 8787 held by a **non-deectx** process → detected and reported; never
  blind-kill a foreign process.
- Partial wiring → per-tool result (`wired` / `skipped: OAuth-locked` /
  `error`); one failure never aborts the rest.
- Spawn/daemon failure (permissions, unsupported OS) → reported; tools + config
  are still left in a coherent state.
- Passthrough upstream error → 502 with a readable body; fail-closed still
  refuses prompt-bearing endpoints when a required detector is down.
- `uninstall` data prompt defaults to **keep** — never silently deletes the
  ledger.

## Testing

- **Pure unit:** `deectx_home()` (env override + default); pidfile
  encode/parse + stale detection; passthrough decision table (path → upstream +
  mask?), incl. `count_tokens`→mask and `/v1/models`→verbatim; method/query/
  header preservation.
- **Masking golden:** a `/v1/messages/count_tokens` body has its PII masked
  before forward.
- **Lifecycle:** inject a fake `ProcessManager`; test `start`/`stop`/`uninstall`
  by composition with no spawning; reuse `setup.rs` wire/unwrap/daemon tests.
- **Integration:** unknown path (`/v1/models`) forwards through the fallback to
  a mock upstream; `stop` after `start` restores tool configs byte-identically.
- CI unchanged: `cargo fmt` + `clippy -D warnings` + `cargo test`.

## Backward compatibility

- Existing docs/muscle-memory (`setup`, `unwrap`, `doctor`, `daemon-*`, `serve`,
  `audit`, `status`) keep working via aliases.
- Ledger location moves to `~/.deectx/ledger.jsonl`. A one-line note in the
  release notes; no migration of old ledgers (hash-only audit data, safe to
  leave).

## Out of scope (tracked elsewhere)

- Homebrew **tap** auto-sync in the release workflow (separate follow-up).
- Any `deectx-pro` supervisor/telemetry behavior.
