# deeCtx

**Local-first PII-masking proxy for AI coding tools.**

<div>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="Apache-2.0"></a>
<a href="https://crates.io/crates/deectx"><img src="https://img.shields.io/crates/v/deectx" alt="crates.io"></a>
<a href="https://github.com/deectxone/deectx/releases"><img src="https://img.shields.io/github/v/release/deectxone/deectx" alt="GitHub release"></a>
</div>

deeCtx sits between your AI coding agent and the model API. It scans every prompt
you send, masks personally-identifiable information (PII) and secrets with
reversible, session-scoped placeholders *before* anything reaches the model
service, forwards the masked request, then rehydrates the response — so you get
your real data back with full context, and third parties never see it.

```
tool ──▶ [ deeCtx /v1 ] ──mask──▶ upstream API (OpenAI | Anthropic)
        [      /\      ]
        └──────┘ rehydrate
```

**Why this matters.** AI coding tools (Cursor, opencode, Claude Code, custom
SDK clients, …) send your code, prompts, and local context to remote model
APIs. If that context contains emails, phone numbers, IBANs, medical terms,
Australian CDR fields, or API keys, it leaves your machine. deeCtx minimizes
what actually leaves — while preserving your workflow.

For a deep dive (data flow, component map, threat model, how to extend), see
[`ARCHITECTURE.md`](ARCHITECTURE.md).

---

## What it solves

| Problem | deeCtx |
|---------|--------|
| Sensitive data uploaded to 3rd-party model APIs | Detects + masks PII/secrets before the request is forwarded |
| Conversation context loss when you redact yourself | **Reversible** in-session masking (`[EMAIL_1]` ↔ `jane@example.com`) |
| Ability to prove what data was processed (DPIA/compliance) | **Hash-only** append-only ledger + `audit` reporting — no raw PII stored |
| Detector misses / false positives | Pluggable **packs** (regex + checksum, entropy heuristics, optional NER) |
| A required classifier being down | **Fail-closed** gate: request is refused (HTTP 503) rather than leaking |

### Detection pipeline

```
text → DetectorChain ─┬─ Regex (email, IBAN, cards, … + Luhn/Mod97/Ato-TFN checks)
                      ├─ Secrets (API keys, … + entropy filter) ──► Allowlist ─► Masker ─► tuned text
                      └─ NER (optional, GLiNER: names, addresses, Art.9 health…) ─┘      └► ledger
```

---

## Install

Builds are available for **Windows (x86_64)**, **macOS (Intel + Apple Silicon)**,
and **Linux (x86_64)**. Choose whichever is easiest:

- **Cargo (recommended, all platforms)**: `cargo install deectx`
- **Homebrew (macOS/Linux)**: `brew install deectx`
- **Scoop (Windows)**: `scoop install deectx`
- **Binary zip/tarball**: download from
  [GitHub Releases](https://github.com/deectxone/deectx/releases) — each release
  is built for all four targets by CI.

All three package paths ship and are refreshed automatically on release.
> Prefer building from source? `cargo install --path .` or `cargo build --release`
> (see [Development](#development)).

## Quick start (2 minutes)

```bash
# 1. Copy the example config and start the proxy
cp config.example.toml config.toml
deectx serve --config config.toml
# listening on http://127.0.0.1:8787

# 2. Point your AI tool at the proxy
#    OpenAI-compatible base:  http://127.0.0.1:8787/v1
#    Anthropic-compatible:    http://127.0.0.1:8787/v1/messages
```

To enable methods, set `active_packs = ["gdpr"]` (or `["cdr-au"]`) in
`config.toml`. NER (semantic detection of people, addresses, health terms) is
optional and requires a GLiNER ONNX model — see both files and
[`ARCHITECTURE.md`](ARCHITECTURE.md) §5–6.

Tool integration is turnkey for **Cursor** and **opencode** via the shipped
shims — see [`shims/README.md`](shims/README.md). Any tool that lets you override
its model base URL to the proxy works too (see “AI tool support” below).

## Verify it's working

```bash
curl -fsS http://127.0.0.1:8787/healthz   # → ok
```

Send a prompt with a test email; check the audit:

```bash
deectx audit --config config.toml --today --export report.json
```

---

## Configuration

See [`ARCHITECTURE.md`](ARCHITECTURE.md) §11 for the full field reference. The
defaults work out of the box; the notable knobs are:

| Setting | What it controls |
|---------|------------------|
| `upstream` / `upstream_anthropic` | Where masked requests are sent. |
| `active_packs` | Built-in PII packs to turn on (`default`(always) , `gdpr`, `cdr-au`). |
| `ner` + `model_dir` | Optional semantic NER via a local GLiNER ONNX model. |
| `allowlist` | Values never masked (case-insensitive). |
| `ledger_path` / `ledger_retention_days` | Audit log location and retention. |

---

## AI product support

### What works

Any client that can be pointed at deeCtx's `:8787` base URL and uses one of the
two supported request schemas:

- **OpenAI-compatible** chat: `/v1/chat/completions` — including **streaming**
  (SSE) responses rehydrated in real time.
- **Anthropic-compatible** messages: `/v1/messages`.
- Concretely supported examples (shims included): **Cursor** and **opencode**,
  plus any LangChain/OpenAI SDK caller with a configurable `base_url`/`OPENAI_BASE_URL`.

### Not supported (know the limits)

- Model APIs with a **non‑OpenAI/Anthropic wire format** (e.g. Gemini native
  REST, Bedrock-native) **as-is** — they'd need a new request/response adapter.
- **Non‑text / binary** outputs (images, blobs) are not rehydrated; they pass
  through as masked (they were masked before leaving your machine).
- deeCtx is not a remote proxy or filtering firewall — it is specific to your
  local model traffic. Full guarantees and caveats: [`ARCHITECTURE.md`](ARCHITECTURE.md) §9 (threat model).

---

## Commands

```bash
deectx serve --config config.toml    # run the masking proxy
deectx audit --config config.toml --today          # console summary
deectx audit --config config.toml --today --export report.json   # JSON export
```

---

## Audit for compliance

`audit` aggregates the hash-only ledger into totals — masked vs. redacted
events, alerts, distinct sessions, per-tool / per-entity / per-pack — for DPIA,
GDPR, or Australian CDR reporting **without exposing personal data**.

---

## Development

```bash
cargo test        # unit + integration tests
cargo clippy      # lint
scripts/release.ps1  (Windows) / scripts/release.sh  → builds zip + computes SHA-256
```

---

## More

- [ARCHITECTURE.md](ARCHITECTURE.md) — the full engineering deep-dive.
- [shims/README.md](shims/README.md) — Cursor & opencode integration.
- `scripts/release.*` — release packaging (zip + brew/scoop hashes).