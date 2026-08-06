# AGENTS.md — deectx

Reference for coding agents (Claude Code, GitHub Copilot, opencode) working in this repo.
Deep design discussion lives in `ARCHITECTURE.md`.

## What this is

`deectx` (crate `deectx` v0.1.0, Apache-2.0, Rust edition 2021) is a **local-first PII-masking
reverse HTTPS proxy** for AI coding tools. It sits between the tool (Cursor, opencode, Claude
Code) and an upstream AI API (OpenAI / Anthropic), masks secrets and personal data before they
leave the machine, stores a privacy-safe ledger locally, and lets the user audit what was masked.

Must stay **OOA pure**: this is the open-source, privacy-preserving core. No org/telemetry/user
identity features here — those live in the commercial `deectx-pro` companion repo.

## Commands

```bash
deectx serve            # run the masking proxy (default http://127.0.0.1:8787)
deectx serve --config config.toml
deectx audit            # human-readable summary of the local ledger
deectx audit --today --export -    # JSON for today to stdout
```

Build / test / lint (all from repo root):

```bash
cargo build --release
cargo test                       # 62 tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
# Windows GNU toolchain: prepend "$env:PATH='C:\msys64\mingw64\bin;'" before cargo
```

## Project layout

| Path | Responsibility |
|------|----------------|
| `src/lib.rs` | Public library surface; 11 `pub mod` exports |
| `src/main.rs` | CLI (`serve`, `audit`) via clap derive |
| `src/config.rs` | `Config` struct + TOML loading, serde defaults |
| `src/proxy.rs` | axum routes, masking walk, upstream forwarding, rehydration |
| `src/ledger.rs` | Append-only JSONL ledger, daily rotation, retention |
| `src/audit.rs` | `AuditSummary` aggregation |
| `src/masker.rs` | Session-scoped reversible masking (`[EMAIL_1]`) / redaction |
| `src/sse.rs` | Streaming SSE rehydration (bounded buffers) |
| `src/span.rs` | Detected `Span{start,end,entity,action,text,alert}` |
| `src/allowlist.rs` | Case-insensitive allowlist filter |
| `src/chunk.rs` | Text chunking for NER |
| `src/detect/` | `mod.rs` (chain), `regex.rs`, `secrets.rs`, `ner.rs` (feature `ner`) |
| `src/packs/` | Pack YAML definitions: `default`, `gdpr`, `cdr-au` + loader |
| `config.example.toml` | Documented example config |
| `shims/` | Integration shims for Cursor + opencode |
| `install/` | Homebrew formula + Scoop manifest |
| `scripts/` | `release.ps1`, `release.sh` (build + package) |
| `tests/` | `proxy_integration.rs`, `golden_set.rs`, `installers.rs` |
| `.github/workflows/` | `ci.yml` (fmt+clippy+test matrix), `release.yml` (tag builds) |

## Core concepts

### Config (`src/config.rs`)
TOML file (`config.toml` default). Fields: `listen` (127.0.0.1:8787), `upstream`
(`https://api.openai.com`), `upstream_anthropic`, `ledger_path` (`./ledger.jsonl`),
`ledger_retention_days` (90), `active_packs`, `packs_dir`, `model_dir`, `allowlist`, `ner`.
Partial files fill unspecified fields from serde defaults. **No env-var config.**

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

### Proxy (`src/proxy.rs`)
Routes: `GET /healthz`, `POST /v1/chat/completions` (OpenAI), `POST /v1/messages` (Anthropic).
`mask_walk` walks JSON (recursing into JSON-encoded tool args byte-preserving); streams via
`SseRehydrator`; non-streams rehydrated via `rehydrate_response`.

## Conventions
- Rust 2021, `anyhow` for errors, serde derive, `to_utc` + chrono. Tests live next to code
  (and in `tests/`); TDD. `cargo fmt` + `clippy -D warnings` must be clean.
- No env-var config (TOML only). No raw PII written anywhere; hashes only.
- Match existing module boundaries — add features as new focused `src/` files, exported from
  `lib.rs`, never pile into `proxy.rs`.

## Where to make a change
- New scanner type → add to `src/detect/` + wire into `detect/mod.rs` chain.
- New built-in pack → new YAML in `src/packs/` + registration in `packs/mod.rs`.
- New route / upstream format → `src/proxy.rs` (add `ApiFormat` arm).
- Masking semantics → `src/masker.rs`. Ledger shape → `src/ledger.rs`. Reporting → `src/audit.rs`.
- CLI surface → `src/main.rs`. Defaults → `src/config.rs`.