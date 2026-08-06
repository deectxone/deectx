# AGENTS.md — deectx

Reference for coding agents (Claude Code, GitHub Copilot, opencode) working in this repo.
Deep design discussion lives in `ARCHITECTURE.md`. Keep this file current — see
[Maintenance](#maintenance).

## What this is

`deectx` (crate `deectx`, Apache-2.0, Rust edition 2021) is a **local-first PII-masking
reverse proxy** for AI coding tools. It sits between the tool (Cursor, opencode, Claude
Code, Codex, Copilot CLI) and an upstream AI API (OpenAI / Anthropic), masks secrets and
personal data before they leave the machine, stores a privacy-safe ledger locally, and lets
the user audit what was masked.

Must stay **OSS-pure**: this is the open-source, privacy-preserving core. No org/telemetry/user
identity features here — those live in the commercial `deectx-pro` companion repo.

## Commands

```bash
deectx serve --config config.toml   # run the masking proxy (default http://127.0.0.1:8787)
deectx audit --config config.toml --today            # human-readable ledger summary
deectx audit --config config.toml --today --export - # JSON to stdout (path or - )
deectx status [--json]              # live masked/redacted counters from the proxy's /stats
deectx setup                        # auto-wire installed tools + install autostart daemon
deectx doctor                       # per-tool wiring status
deectx unwrap                       # restore original tool configs from .bak backups
deectx daemon-install               # start the proxy at login (autostart)
deectx daemon-uninstall             # remove the autostart entry
```

Build / test / lint (all from repo root):

```bash
cargo build --release
cargo test                       # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
# Windows GNU toolchain: prepend "$env:PATH='C:\msys64\mingw64\bin;'" before cargo (dlltool)
```

Install paths ship prebuilt binaries first (Scoop / `cargo binstall` / prebuilt Homebrew);
`cargo install` builds from source and needs a C/C++ linker. See `README.md` §Install.

## Project layout

| Path | Responsibility |
|------|----------------|
| `src/lib.rs` | Public library surface; `pub mod` exports |
| `src/main.rs` | CLI (`serve`, `audit`, `status`, `setup`, `doctor`, `unwrap`, `daemon-install/uninstall`) via clap derive |
| `src/config.rs` | `Config` struct + TOML loading, serde defaults |
| `src/proxy.rs` | axum routes, masking walk, upstream forwarding, rehydration |
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
| `src/setup.rs` | Tool discovery + config patching (`setup`/`doctor`/`unwrap`) + autostart daemon install/uninstall |
| `src/detect/` | `mod.rs` (chain), `regex.rs`, `secrets.rs`, `ner.rs` (feature `ner`) |
| `src/packs/` | Pack YAML definitions: `default`, `gdpr`, `cdr-au` + loader |
| `config.example.toml` | Documented example config |
| `shims/` | Integration shims for Cursor + opencode |
| `install/` | Homebrew formula + Scoop manifest (`release.yml` refreshes hashes on tag) |
| `scripts/` | `release.ps1`, `release.sh` (build + package) |
| `tests/` | `proxy_integration.rs`, `golden_set.rs`, `installers.rs` |
| `.github/workflows/` | `ci.yml` (fmt+clippy+test), `release.yml` (tag builds + publish), `agents-doc-freshness.yml` |

## Core concepts

### Config (`src/config.rs`)
TOML file (`config.toml` default). Fields: `listen` (127.0.0.1:8787), `upstream`
(`https://api.openai.com`), `upstream_anthropic`, `upstream_responses`, `ledger_path`
(`./ledger.jsonl`), `ledger_retention_days` (90), `active_packs`, `packs_dir`, `model_dir`,
`allowlist`, `ner`, `stats_enabled`. Partial files fill unspecified fields from serde
defaults. **No env-var config.**

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
`"s_"+sha256(first_msg)[..8]`.

### Proxy (`src/proxy.rs`, `src/upstream.rs`, `src/responses_ws.rs`)
Routes: `GET /healthz`, `GET /stats`, `POST /v1/chat/completions` (OpenAI), `POST /v1/messages`
(Anthropic), and the `/v1/responses` WebSocket. Routing picks the upstream by API-key shape.
`mask_walk` walks JSON (recursing into JSON-encoded tool args byte-preserving); streams via
`SseRehydrator`; non-streams rehydrated via `rehydrate_response`; WS frames masked/rehydrated
per frame in `responses_ws.rs`.

### Setup / autostart (`src/setup.rs`)
`discover()` finds installed tools; `patch_config` rewrites their base URL to the proxy (backing
up to `<path>.bak`, idempotent); OAuth-locked tools are skipped. `install_daemon` /
`uninstall_daemon` manage the per-OS login autostart entry. `doctor()` reports wiring; `unwrap()`
restores from `.bak`.

## Conventions
- Rust 2021, `anyhow` for errors, serde derive, chrono UTC. Tests live next to code
  (and in `tests/`); TDD. `cargo fmt` + `clippy -D warnings` must be clean.
- No env-var config (TOML only). No raw PII written anywhere; hashes only.
- Match existing module boundaries — add features as new focused `src/` files, exported from
  `lib.rs`, never pile into `proxy.rs`.

## Where to make a change
- New scanner type → add to `src/detect/` + wire into `detect/mod.rs` chain.
- New built-in pack → new YAML in `src/packs/` + registration in `packs/mod.rs`.
- New route / upstream format → `src/proxy.rs` (+ `upstream.rs` for routing, `responses_ws.rs` for WS).
- Masking semantics → `src/masker.rs`. Ledger shape → `src/ledger.rs`. Reporting → `src/audit.rs`.
- Live counters → `src/stats.rs` / `src/status.rs`.
- Tool wiring / autostart → `src/setup.rs`. CLI surface → `src/main.rs`. Defaults → `src/config.rs`.

## Maintenance
Update this file whenever you add or move a **module, route, CLI command, config field, or pack**.
Keep the layout table and Commands section in sync with `src/` and `src/main.rs`. A CI check
(`.github/workflows/agents-doc-freshness.yml`) posts a warning annotation when `src/**` or
`Cargo.toml` change in a commit/PR without a matching `AGENTS.md` edit — treat it as a nudge, not
a gate.
