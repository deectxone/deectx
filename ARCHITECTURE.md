# deeCtx Architecture

This document describes how deeCtx works internally — the component model, data
flow, detection pipeline, lifecycle, and threat model. Read it to understand
what deeCtx does, how to extend it (packs, detectors, shims), and the exact
guarantees it can and cannot make. A shorter, user-facing overview lives in
[`README.md`](README.md).

---

## 1. At a glance

```
             ┌────────────────────────────────────────────────────────────┐
             │                        Your machine                        │
             │                                                            │
  AI coding  │  ┌─────────────┐        ┌─────────────────────────┐         │
  tool       │  │             │  HTTP  │  deeCtx proxy (axum)     │         │
 (Cursor,    │  │   Client    │────────▶│  ┌───────────────────┐  │         │
  opencode,  │  │             │        │  │ 1. Ingress        │  │         │
  any SDK)   │  └─────────────┘        │  │   /v1/chat/compl.  │  │         │
     │       │                        │  │   /v1/messages     │  │         │
     │       │                        │  │   /healthz         │  │         │
     │       │                        │  └───────────────────┘  │         │
     │       │                        │            │            │         │
     │       │                        │            ▼            │         │
     │       │                        │  ┌───────────────────┐  │         │
     │       │                        │  │ 2. Mask / audit    │  │         │
     │       │                        │  │   DetectorChain   │  │         │
     │       │                        │  │   Allowlist       │  │         │
     │       │                        │  │   Masker          │  │         │
     │       │                        │  │   Ledger (hash)   │  │         │
     │       │                        │  └───────────────────┘  │         │
     │       │                        │            │            │         │
     │       │                        └────────────┼────────────┘         │
     │       │                                     ▼                      │
     └───────┴─────────────────────────►  Real model API                 │
     prompt/reply                          (OpenAI | Anthropic)          │
```

The proxy never stores model keys or secrets; placeholders are kept only in
memory, keyed by a per-session id.

---

## 2. Core concepts

| Concept | Meaning |
|--------|---------|
| **Span** | A detected sensitive region: byte `start`, byte `end`, entity name, action, matched text, and whether it is an *alert*. |
| **Action** | `mask` (reversible, replaced by `[ENTITY_N]`) or `redact` (irreversible, replaced by `[REDACTED_SECRET]`). |
| **Detector** | A `Detector` trait impl returning `Vec<Span>` for a `&str`. |
| **DetectorChain** | Composes multiple detectors, resolves overlapping spans (longest at same start, sorted by start). |
| **Pack** | A YAML unit that declares entities (one or more named detectors/patterns), and per-pack settings (`failClosed`, `allow`). |
| **Masker** | In-memory session-scoped bidirectional map original↔placeholder. |
| **Ledger** | Append-only hash-only JSONL log of masking events (never raw PII). |
| **SseRehydrator** | Streaming rehydrator that restores placeholders to originals in SSE chunks without corrupting byte/multibyte boundaries or splitting placeholders. |

---

## 3. Module map (`src/`)

| Module | Responsibility |
|--------|----------------|
| `main.rs` | CLI: `serve`, `audit`. |
| `lib.rs` | Module root; public crate surface. |
| `proxy.rs` | The axum HTTP server, ingest routing, masking walk, streaming rehydration wiring, upstream forwarding, ledger append. |
| `config.rs` | TOML configuration and defaults. |
| `span.rs` | `Span` and the `Action` enum (`mask`/`redact`). |
| `detect/mod.rs` | `Detector` trait, `DetectorChain`. |
| `detect/regex.rs` | Regex-based detector with optional checksum validators (Luhn, Mod97, ATO TFN). |
| `detect/secrets.rs` | Secret-like scanning with Shannon-entropy filter. |
| `detect/ner.rs` | (feature-gated) GLiNER-onnx batch/word-span NER detection, fail-open. |
| `packers` | `packs/` model loading, built-in packs, active-set resolution, allowlist aggregation, chain construction. |
| `masker.rs` | Session-scoped masking + rehydration. |
| `sse.rs` | Streaming chunk rehydrator with bounded memory and `[`-prefix hold logic. |
| `ledger.rs` | Hash-only append-only JSONL, dated rotation, retention pruning, `read_all`. |
| `audit.rs` | Aggregation of ledger entries into `AuditSummary`. |
| `allowlist.rs` | Exact, case-insensitive allow-list filter applied after detection. |
| `chunk.rs` | Chunking helpers for large content. |

> Note: the crate also ships `src/bin/ner_spike.rs` (a standalone NER model
> spike) and a `models/` gitignored directory where a GLiNER ONNX model would live.

---

## 4. Request lifecycle (the masking cycle)

A single `POST` to `/v1/chat/completions` (OpenAI) or `/v1/messages` (Anthropic):

1. **Ingress** — replace body bytes, identify the calling `user-agent` as `tool`.
2. **Session id** — derived from the first user message(s) (`sha256(...)[..8]` prefixed `s_`) so each conversation is isolated for masking.
3. **Fail-closed gate** — if any active pack sets `failClosed: true` and `DetectorChain.ready()` is false (e.g. NER model unavailable), return HTTP **503** and refuse the request rather than leak.
4. **Mask walk** — recursively walk the JSON body (`mask_walk`). Each string is handed to the with each detector, allowlist-filtered, then masked:
   - Reversible values (`Action::Mask`) → `[EMAIL_1]`, `[NAME_2]`, … tracked per session.
   - Secrets (`Action::Redact` for API keys) → `[REDACTED_SECRET]`, never stored.
   - JSON-encoded tool `arguments` strings are recursively descended so nested PII is caught. A *byte-preserving* round-trip is required; otherwise it masks the raw string bytes to avoid silver/double-conversion of numbers.
5. **Forward** — the masked JSON is POSTed to the resolved upstream (OpenAI or Anthropic base URL per format), forwarding headers except `host`/`content-length`/`accept-encoding`.
6. **Response / rehydration**:
   - **Non-stream**: buffers the body; if it's JSON, walks `choices[].message`/`.delta` and `content[].text`, then a final raw placeholder→original pass; if it is compressed/binary, forwarded verbatim.
   - **Stream (SSE)**: `SseRehydrator` incrementally restores placeholders across chunk boundaries while holding partial `[ENTITY_` prefixes so a token split across chunks isn't rehydrated too early. Hard memory cap (64 KB pending).
   - `gzip`-like bodies stream/forward through raw—rehydration is only for UTF-8 text.
7. **Ledger** — one append-only `LedgerEntry` (timestamp, tool, session, latency, affected entities, events) is written. Events carry entity, action, an optional placeholder, a **hash** of the placeholder (never the placeholder), and the `alert` flag.
8. **Return** — the rehydrated response is returned to the tool.

---

## 5. Detection pipeline

```
                 text
                   │
        ┌──────────▼──────────┐
        │  DetectorChain       │   (all detectors run, then merged)
        │  ├─ RegexDetector    │   patterns + checksums (Luhn/Mod97/Ato-TFN)
        │  └─ SecretsDetector  │   entropy >= entropy_min
        │  └─ NerDetector      │   (feature `ner`) chunked GLiNER inference
        └──────────┬──────────┘
          merge: de-overlaps, longest wins, sort by start
                   │
        ┌──────────▼──────────┐
        │  Allowlist          │   drop spans equal to allowlisted values
        └──────────┬──────────┘
                   ▼
        ┌───────────────────┐
        │   spans + Masker   │   → masked text + ledger events
        └───────────────────┘
```

Detectors implement `Detector { fn detect(text) -> Vec<Span>; fn ready() -> bool }`.
A chain's `ready()` is all detectors ready; used for the fail-closed gate.

---

## 6. Packs and built-in coverage

Packs are YAML files declaring entities. Built-ins are compiled in
(`src/packs/*.yaml`), or custom ones load from `packs_dir`.

Entity fields: `id`, `detector` (`regex`|`secrets`|`ner`), `pattern`/`patterns`,
`labels` (NER labels list), `checksum` (`luhn`|`mod97`|`ato_tfn`),
`entropy_min`, `action` (`mask`|`redact`), `alert` (bool).

| Pack | Entities (examples) | Notable settings |
|------|--------------------|------------------|
| `default` (always active) | `email` (regex), `credit_card` (regex+Luhn), `api_key` (secrets+entropy, redact) | `failClosed: false` |
| `gdpr` | NER: `person`, `address`; Art. 9 (health, biometric, ethnicity, religion, politics, union, sex, race) NER with `alert: true`; regex: `email`, `phone`, `iban` (Mod97) | Art 9 → notify |
| `cdr-au` (Australian CDR) | `tfn` (Ato-TFN), `medicare_number`, `bsb_account`, `driver_licence_au`, `passport_au`, `centrelink_crn` | alert on sensitive |

Active packs are loaded from `active_packs` (names of built-ins) plus any
`packs_dir`, deduplicated by name.

---

## 7. Masking & rehydration (detail)

- **Session isolation**. The `Masker` key maps by `session` id, so the same email in two sessions never shares a counter.
- **Consistency**: the same original value in a session always maps to the same placeholder.
- **Ordering**: replacements applied right-to-left (offsets stay valid); rehydration replaces by descending placeholder length to protect `[EMAIL_10]` from `[EMAIL_1]`.
- **Redaction**: redacted secrets store no mapping; they are permanently removed and never rehydrated.
- **SSE streaming**: `SseRehydrator` buffers a tail (up to 128 KB) to stay within UTF-8/placeholder boundaries, emits complete non-placeholder bytes as they arrive, holds while a `[ENTITY...` prefix is incomplete, and flushes pending at the finish.

---

## 8. Ledger & audit

- **Append-only JSONL** with daily rotation to `ledger-YYYY-MM-DD.jsonl`; retention pruning after `ledger_retention_days`.
- **Hash-only**: events store a SHA-256 of the placeholder, **no raw PII**, no reversible mapping in the log.
- **`audit` mode** (`deectx audit --today [--export report.json]`) aggregates: total requests, masked/redacted events, alerts, distinct sessions, per-tool, per-entity, per-pack — for DPIA reporting without exposing personal data.

---

## 9. Threat model

**deectx is a risk-reduction layer, not full security.**

What it does:
- Prevents specific PII/secret types from being sent to 3rd-party model APIs over the wire.
- Provides an audit trail for compliance (DPIA/GDPR/CDR) without storing raw personal data.
- Fails closed by default when a required detector isn't ready and `failClosed` is set in the pack(s).

What it does NOT do:
- Does **not** store your API keys with you proxy; keys are forwarded to the upstream API as normal. It never *reads* them.
- Latency-model is in-process detection only; it cannot empty a compromised host, a warm malware/trojan, or exfiltration by other software.
- Masking is best-effort on text; a novel entity type not declared in a pack is not detected. It is *not* ML whiz -- NER is optional and fail-open.
- Streaming rehydration is best-effort for UTF-8; compressed/binary streams pass through already masked bytes (they were masked before forwarding upstream).
- Reversibility means model responses *do* contain original values again once rehydrated; if you need to keep data out of *your* logs, redact rather than mask.

---

## 10. Extending

- **Add an entity**: edit or add to a YAML pack (regex + optional checksum for high-confidence matches; `secrets` + `entropy_min` for like-key materials; `ner` for semantic entity types with `labels` representing the GLiNER label list).
- **Add a pack**: drop a YAML into `packs_dir` and start it via `active_packs`.
- **Add a detector**: implement the `Detector` trait and register it in `pack/build_chain` (and opt it under a feature gate if it pulls heavy deps like ONNX).
- **Add a shim**: any tool that lets you override its model API base URL to `http://127.0.0.1:8787` and uses OpenAI/Anthropic request shapes works. The repo ships `shims/cursor` and `shims/opencode` examples.

---

## 11. Configuration reference

| Field | Default | Purpose |
|-------|---------|---------|
| `listen` | `127.0.0.1:8787` | Proxy listen addr. |
| `upstream` | `https://api.openai.com` | OpenAI-style upstream. |
| `upstream_anthropic` | `https://api.anthropic.com` | Anthropic-style upstream. |
| `ledger_path` | `./ledger.jsonl` | Ledger base path. |
| `ledger_retention_days` | `90` | Days to keep rotated ledger files. |
| `active_packs` | `[]` | Built-in pack names to enable. |
| `packs_dir` | none | Directory of custom packs. |
| `model_dir` | `./models` | GLiNER ONNX model dir (`model.onnx` + `tokenizer.json`). |
| `allowlist` | `[]` | Values never masked (case-insensitive). |
| `ner` | `false` | Enable NER (requires `ner` feature + model). |