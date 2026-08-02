# deeCtx — Design Spec

**Date:** 2026-07-30
**Status:** Approved (design phase)
**Type:** Open-source, local-first PII-masking proxy for AI coding tools

## 1. Vision

deeCtx intercepts prompts from AI coding tools, masks PII and secrets before they leave the machine, forwards to the LLM provider, and rehydrates the originals back into the streamed response. Compliance policy is expressed as pluggable rule packs (GDPR + Australian CDR ship in-tree). All state stays local; zero telemetry.

Analogue: what LeanCTX does for context compression, deeCtx does for privacy/compliance.

## 2. Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Project type | Open-source project, community trust is the moat | User direction |
| v1 wedge | Coding-agent proxy (custom base URL) | Best coverage-per-effort; dodges MITM/cert-install; APIs exist |
| Language | Rust + ONNX (GLiNER NER) | Single-binary distribution; no Python runtime needed |
| Detection scope | PII + secrets | PII reversibly masked; secrets hard-redacted |
| Rule packs v1 | GDPR + CDR Australia, extensible YAML format | CDR is a differentiator — zero competitors ship AU packs |
| Masking strategy | Reversible pseudonymization, consistent placeholders + SSE rehydration | Preserves LLM quality; research-verified winning pattern |
| Build/execution | Solo part-time maintainer; implementation delegated to AI agents | User direction; subagent-driven-development workflow |
| Failure default | Fail-open (log + continue); `failClosed` per-pack opt-in | Availability first; Cursor hook uses fail-closed |

## 3. Architecture

```
┌─────────────────────────────────────────────────────────┐
│  deeCtx (single Rust binary, localhost:8787)            │
│                                                         │
│  ┌───────────┐   ┌──────────────┐   ┌────────────────┐  │
│  │ HTTP      │──▶│ Pipeline     │──▶│ Upstream client│──▶ api.openai.com /
│  │ proxy     │   │ (detectors)  │   │ (OpenAI+       │    api.anthropic.com
│  │ (axum)    │◀──│              │◀──│  Anthropic)    │◀───
│  └───────────┘   └──────────────┘   └────────────────┘  │
│       │                 │                               │
│       ▼                 ▼                               │
│  ┌───────────┐   ┌──────────────┐   ┌────────────────┐  │
│  │ Rehydrator│   │ Mapping store│   │ Audit ledger   │  │
│  │ (SSE      │   │ (session-    │   │ (append-only,  │  │
│  │  stream)  │   │  scoped, RAM │   │  hash-only,    │  │
│  └───────────┘   └──────────────┘   │  JSONL on disk)│  │
│                                     └────────────────┘  │
│  Config: config.toml + packs/*.yaml (gdpr, cdr-au)      │
└─────────────────────────────────────────────────────────┘
        ▲                    ▲
  Cursor hooks.json    opencode plugin  (thin shims: enforce base_url, fail-closed)
```

### Components (one Rust module each)

- **proxy** — axum HTTP server; OpenAI `/v1/chat/completions` + Anthropic `/v1/messages`; forwards upstream with user's API key (never logged)
- **pipeline** — detector chain: regex/checksum → secrets (entropy) → GLiNER ONNX NER; <100ms budget; chunking with overlap for >512 tokens
- **masker** — spans → consistent placeholders (`[PERSON_1]`); secrets hard-redacted
- **mapper** — session-scoped placeholder↔original map, in-memory only, dropped at session end
- **rehydrator** — SSE stream interception; restores originals; handles placeholders split across chunks via lookahead buffer
- **ledger** — append-only JSONL: timestamp, tool, entity types, actions, SHA-256 of placeholders — never raw PII
- **packs** — YAML rule packs: entities, patterns, per-entity action (mask/redact/warn/block)

## 4. Data Flow

**Outbound:**
1. Request hits `localhost:8787`
2. Session ID from header or hash of first message; mapping store retrieved/created
3. Content extracted per API format
4. Pipeline: regex/checksum (µs) → secrets → GLiNER NER (~10–50ms/chunk)
5. Span dedup/overlap-resolve: longest match wins; checksum-validated beats NER
6. Mask: new entity → next placeholder; repeat entity → same placeholder (referential consistency)
7. Ledger entry (types + placeholder hashes only)
8. Forward upstream

**Inbound:**
9. Rehydrator scans SSE chunks; lookahead buffer for split placeholders
10. Swap originals back; non-streaming = plain string replace

**Failure modes:**
- NER missing/slow → degrade to regex+secrets, warn, never block (unless pack sets `failClosed: true`)
- Upstream error → passthrough
- Hallucinated placeholder (no mapping) → left as-is, harmless

**Latency budget:** regex+secrets <5ms; NER <50ms typical; p95 added <100ms.

## 5. Detection Engine & Rule Packs

| Layer | Tech | Catches | Action |
|---|---|---|---|
| Regex+checksum | hand patterns, Luhn/mod-97/ATO checksums | email, phone, IBAN, cards, TFN, Medicare, BSB/acct | mask |
| Secrets | gitleaks regexes + entropy (>4.5, len>20) | API keys, tokens, private keys, conn strings | redact |
| NER | GLiNER multi-PII, ONNX INT8, zero-shot | person, address, org, DOB, free-text PII | mask |

- Allow-lists: per-pack + per-user; recall-first tuning, precision via allow-lists
- `Detector` trait, one impl per layer

### Pack format (example: packs/gdpr.yaml)

```yaml
name: gdpr
version: 0.1.0
entities:
  - id: person
    detector: ner
    labels: [person, name]
    action: mask
  - id: art9_health
    detector: ner
    labels: [medical_condition, health_data]
    action: mask
    alert: true
  - id: iban
    detector: regex
    pattern: '\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){3,7}\b'
    checksum: mod97
    action: mask
settings:
  failClosed: false
```

### Packs shipped in-tree

- **gdpr** — person/address/email/phone/IBAN/national IDs + Art 9 special categories (health, biometric, racial, religious, political, union, sexual orientation) with `alert: true`
- **cdr-au** — TFN (ATO checksum), Medicare, BSB+account, driver licence, passport, Centrelink CRN, CDR Privacy Safeguard entities

HIPAA/PCI possible via same YAML format; not shipped/tested in v1.

### Config

`~/.config/deeCtx/config.toml`: upstream URL, active packs, API key env refs, ledger path, NER on/off, per-tool overrides.

## 6. Audit Ledger

`~/.local/share/deeCtx/ledger.jsonl`:

```json
{"ts":"2026-07-30T10:15:00Z","tool":"opencode","session":"s_a91f",
 "events":[{"entity":"person","ph":"[PERSON_1]","ph_hash":"sha256:…","action":"mask"},
           {"entity":"api_key","action":"redact"}],
 "packs":["gdpr","cdr-au"],"latency_ms":42}
```

- Append-only, file-locked, daily rotation, 90-day retention (configurable)
- Never raw PII
- `deeCtx audit --today --export` → DPIA/Art 30-friendly summary (compliance-evidence differentiator)

## 7. IDE/Tool Coverage

Interception depends on the tool supporting a custom API endpoint, not the IDE.

| IDE | Tool | Covered |
|---|---|---|
| VS Code | Continue, Cline, Cody, Roo Code | yes |
| VS Code | GitHub Copilot Chat/inline | no (hardcoded, no API) |
| Visual Studio | Copilot | no |
| Visual Studio | OpenAI-compatible extensions | yes |
| JetBrains | AI Assistant | no |
| JetBrains | Continue/OSS plugins | yes |
| Eclipse | Copilot4Eclipse | no |
| Eclipse | OpenAI-compatible plugins | yes |
| Any terminal | Claude Code, opencode, Aider | yes (env var) |

Vendor-locked assistants (Copilot, JetBrains AI) are an accepted v1 gap. Post-v1 paths: optional MITM mode (root cert) or per-IDE extension shims.

## 8. Testing

- **Unit**: detectors vs fixture corpus (Unicode names, typos, code identifiers that must NOT match)
- **Golden set**: 200+ labeled prompts; precision/recall in CI; recall regression gate
- **Integration**: mock upstream; assert zero PII out, rehydration restores, split-placeholder SSE case
- **E2E**: scripted opencode + curl against binary
- **Leak test**: garak-style replay probes for residual identifiers

## 9. Milestones (solo, ~8–10h/wk, agent-executed)

| M | Deliverable | ~Time |
|---|---|---|
| M1 | Proxy + regex/checksum + secrets + masking (no NER), OpenAI format, ledger | 4–6 wks |
| M2 | GLiNER ONNX layer, chunking, allow-lists, golden-set CI | +4 wks |
| M3 | Anthropic format, SSE rehydration, GDPR + CDR packs, audit cmd | +4 wks |
| M4 | Cursor hook + opencode plugin shims, installers (brew/scoop/cargo), v0.1 release | +3 wks |

## 10. Out of Scope (v1)

- MITM/root-cert interception of hardcoded tools (Copilot etc.)
- Image/attachment PII redaction
- Multi-language NER beyond GLiNER multi-PII defaults
- Cloud/team dashboards, SSO, fleet management
- HIPAA/PCI/DORA packs (community territory)
- Clipboard/keyboard OS-level hooks

## 11. Known Risks

- False positives on code identifiers → allow-lists + per-domain tuning are mandatory
- Masking ≠ anonymization under GDPR (quasi-identifiers survive) — documented as risk reduction
- Copilot gap is permanent until Microsoft opens an API
- NER context window (chunking overlap mitigates, imperfectly)
