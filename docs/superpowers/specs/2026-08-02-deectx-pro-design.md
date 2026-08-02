# deectx-pro — Enterprise Compliance & Governance Control Plane

**Date:** 2026-08-02
**Status:** Approved design (brainstorming). Not yet implemented.
**Related:** Open-source engine `deectx` (https://github.com/deectxone/deectx)

---

## 1. Summary / Problem

We are building a commercial, enterprise-targeted edition of the open-source
`deectx` PII-masking reverse proxy, called **`deectx-pro`**, in a **separate
repo**. Its job: let an organization **track how many AI-tool "hits" (LLM
requests) could have exposed PII, and quantify what was saved** toward
**EU GDPR** (Art. 9 special categories) and **AU CDR** (Consumer Data Right)
obligations — per user, per entity, live via Prometheus/Grafana, and with an
**independently verifiable, tamper-resistant audit ledger**.

The model is patterned on Leanctx's Pro/Team/Enterprise tiers, re-scoped for the
PII/compliance monitoring domain rather than context compression.

## 2. Confirmed requirements (from user decisions)

1. **Topology:** Self-hosted org server; local agents report masked/compliance
   telemetry. Raw PII never leaves the edge.
2. **"Saved" metric:** Both raw counts and weighted compliance scores per
   GDPR Art.9 and AU CDR buckets.
3. **User identity:** OS username / config-bound `user_id`, attached to every
   telemetry event, evaluated at the pairing client (not the OSS agent).
4. **Observability:** Org server exposes **Prometheus `/metrics`**; dashboards
   via Grafana.
5. **Verifiability:** **Ed25519-signed, hash-chained ledger**, verifiable
   offline by an independent tool.
6. **Repo separation:** separate repo. Shared engine (open deectx) stays 100%
   pure — telemetry lives in a **separate pairing client**.
7. **Storage:** SQLite by default (self-hosted simplicity), optional Postgres.
8. **Weighting:** configurable severity weights, default goals documented in a
   `methodology.md` (auditor defensibility).
9. **Nomination:** git identity "Dee Srivastava", noreply email
   `deectxone@users.noreply.github.com` for author commits.

## 3. Architecture

```
 Employee laptop                          Org cloud / on-prem
 ┌──────────────────┐   mTLS/HTTPS   ┌──────────────────────────────┐
 │  deectx (OSS)    │  signed events │  deectx-pro server          │
 │  - masking engine│ ─────────────→ │  - ingest endpoint           │
 │  - ledger.jsonl  │                │  - Ed25519 ledger (hash-chain)│
 │                  │                │  - compliance scoring engine  │
 │  deectx-ship (client)             │  - Prometheus /metrics        │
 │  reads ledger, adds user_id,      │  - REST API                   │
 │  signs + spools + ships           └───────────┬───────────────────┘
 └──────────────────┘                            │ scrape
                                        ┌────────▼────────┐  ┌──────────┐
                                        │  Prometheus        │──▶│  Grafana │
                                        └────────┬────────┘  └──────────┘
                                                 │ offline verify
                                        ┌────────▼──────────────┐
                                        │ deectx-verify (standalone) │
                                        └───────────────────────┘
```

**Data flow (per request at the edge):**
1. `deectx` (OSS) detects/masks/redacts PII as today, writing `ledger.jsonl` —
   unchanged, no new code, no raw PII.
2. **`deectx-ship`** (pairing client, subsystem A) tails the ledger, wraps each
   entry with `user_id` (from its own config) and a derived regulation bucketing,
   signs each batch with its Ed25519 key, and ships to the server over TLS+mTLS.
   On network outage it spools signed events locally and retries with backoff.
3. **`deectx-server`** verifies signatures, appends to the hash-chained SQLite
   ledger, updates Prometheus counters, and aggregates into org + per-user
   reports.
4. **`deectx-verify`** (offline, independent, ~0 deps) replays chain and sigs;
   reports VALID/INVALID.

## 4. Subsystem A — Edge pairing client (`deectx-ship`)

- Rust binary/crate in the `deectx-pro` repo.
- Tails `ledger.jsonl` (poll/inotify) — reads existing fields only
  (`ts, tool, session, events[{entity,placeholder,ph_hash,action,alert}],
  latency_ms, packs`).
- `user_id` resolution (config-first): `report[].user_id` → env `DEE_USER_ID` →
  OS username (`USER`/`USERNAME`) → fallback `"local"`.
- Derives `regulatory_bucket` from the `entity` name (mapping table below) —
  server-independent.
- Ed25519 signing per batch; holds a local spool buffer on failure; flush on
  reconnect.
- Ships to `POST /v1/teleport` with an agent cert (mTLS).
- Config:
  ```toml
  [report]
  enabled = false
  server = "https://compliance.example.org"
  agent_key = "path/to/ed25519.pem"
  batch_ms = 5000
  ledger_path = "./ledger.jsonl"   # local OSS ledger to tail
  ```

## 5. Subsystem B — Org server (`deectx-serve` / `deectx-pro-server`)

- Rust binary, axum-based.
- Storage: **SQLite** (default) via sqlx/rusqlite, optional Postgres via env.
- **Ingest** `POST /v1/teleport`: mTLS bearer agent cert; body = batch of
  `SignedEvent { event, signature }`. Verify Ed25519 vs. enrolled agent pubkey;
  idempotency by `(id, agent_id)`; malformed/mismatch → HTTP 400 + metric.
- **Ledger** (core trust primitive) rows: `seq, prev_hash, msg_hash, signature,
  payload(JSON), root_hash`; `root_hash = sha256(prev_root || sha256(payload))`;
  periodic signed **checkpoints** `(root_hash, seq)`.
- **Prometheus `/metrics`** counters (labels: user, entity, bucket):
  - `deectx_hits_total`
  - `deectx_at_risk_total`
  - `deectx_saved_total`
  - `deectx_leaked_total`  (fail-open / allowlisted PII, if any)
  - `deectx_alert_total`   (highest-severity Art.9 / CDR-AU)
- **REST API:**
  - `GET /org/summary`
  - `GET /users/:id`
  - `GET /report/gdpr`
  - `GET /report/cdrua`

## 6. Subsystem C — Compliance scoring & verification

**Entity → regulation mapping (deterministic, server-side):**

| Bucket | Entities |
|---|---|
| `gdpr-art9` | `person`, `art9_health`, `art9_biometric`, `art9_racial_ethnic`, `art9_religious`, `art9_political`, `art9_trade_union`, `art9_sex_life`, ... |
| `cdr-au` | `tfn`, `medicare_number`, `bsb_account`, `passport_au`, `centrelink_crn`, `driver_licence_au` |
| `none` | `email`, `credit_card`, `api_key`, `phone`, `iban`, others |

**Weighting ("saved" score):**
- `saved_count +1` per masked/redacted detection; `at_risk_count +1` per detection
  that would have leaked.
- Weighted `exposure_score`: per hit `w(bucket) × entity_severity`.
  Defaults configurable via a `weights` table; documented in `methodology.md`.
- `threat_ratio[user] = at_risk / total_hits`.

**Verifier (`deectx-verify`, offline, standalone, minimal deps):**
- `--ledger path` + `--root <checkpoint>`: recompute chain anchor→head, verify
  each Ed25519 sig. Output VALID/INVALID + exit code.
- `--bundle`: deterministic-signed ZIP export (ledger segment + config +
  coverage) — analogous to an EvidenceBundle.

**Reports:**
- `deectx disclosures report --from --to --bucket gdpr|cdru|all` (human + `--json`).
- `deectx users --csv`: `{user, total, at_risk, saved, alert, top_entity}`.
- Grafana dashboards over Prometheus `/metrics`.

## 7. Security / trust

- mTLS agent↔server; Ed25519 per-agent keys.
- No raw PII anywhere — only entity class + hashes (consistent with OSS).
- Ledger append-only; hash chain + signatures detect any tampering.
- Server supports `--secret` CA generation or BYO certs.

## 8. Error handling / resilience

- Agent spools to local buffer on network fail; backoff; never drop audit rows.
- Server idempotent ingest (dedupe by id).
- Malformed/mismatched payloads rejected → HTTP 400 + metric.

## 9. Testing

- Unit: mapping table, weight sums, hash-chain helpers, signing.
- Integration: agent→server→batch, chain-recompute, **tamper test** (flip byte →
  INVALID).
- E2E against the real OSS `deectx` agent (real ledger.jsonl produced).

## 10. Distribution / licensing

- `deectx-pro` ships `deectx-server`, `deectx-ship`, `deectx-verify`, a Dockerfile
  for the server, install scripts. Binaries via GitHub Releases (CI-built).
- License: **proprietary / commercial** (closed source); NOT Apache-2.0 (unlike OSS).
- The open `deectx` repo remains Apache-2.0 and free forever; CI gate ensures no
  OSS local capability is ever paywalled.

## 11. Rollout order (each = own spec → plan → implement)

1. **Spec A:** pairing client (tail ledger, add user_id, bucket, sign, spool,
   ship) + mapping util.
2. **Spec B:** server (ingest, SQLite ledger + hash-chain checkpoints, /metrics,
   minimal REST API).
3. **Spec C:** compliance engine (weights/config/methodology), reports, offline
   verifier, Docker, CI release.

## 12. Open questions / deferrals

- Exact default weight numbers (to be fixed in `methodology.md` during Spec C).
- Whether to auto-insert the derived bucket by the shipper or recompute
  server-side (currently server-side).
- Postgres path is a stretch; initial scope is SQLite only.