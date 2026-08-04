# Install-and-forget Transparent Interception — Design Spec

**Date:** 2026-08-04
**Status:** Approved (design), pending plan
**Scope:** Open-source `deectx` engine. Additive only — the masking pipeline, ledger, and audit are unchanged.

## Problem

`deectx serve` works as a manual reverse proxy, but it is not plug-and-play: the user must
point each AI tool at `127.0.0.1:8787` by hand, and must configure an upstream. The user's
intent:

> "I want the user to just install and forget that it's there, and in the background it just
> continues working as LeanCTX works... intercept any prompt and scan for data privacy rules."

Plus a hard performance constraint: no compromise on latency; interception is local so it
must feel instant.

## Goals

- One-time `deectx setup` makes interception work by default across tools and models.
- Zero user configuration after setup; tool configs are auto-rewired and reversible.
- Dynamic upstream routing so the user does not have to "switch models" or configure a backend.
- Support Codex CLI / GitHub Copilot CLI (OpenAI Responses API over WebSocket).
- A local `/stats` surface confirming the proxy is actively securing prompts.
- Local overhead < 10ms p99 on non-NER packs; no change to time-to-first-token (TTFT).

## Non-Goals

- Universal TLS MITM capture / root CA. Interception is LeanCTX-style config auto-wiring,
  never a system-level TLS proxy.
- Weighted exposure scoring, per-user identity, regulation buckets — these stay in the
  commercial `deectx-pro` (gated by engine purity).
- Hijacking locked-subscription OAuth tools (Claude Pro/Max, Copilot built-in); unsupported
  and explicitly skipped with a reason.

## Constraints

- Engine (`src/`) stays a pure masking core; `deectx-pro` keeps its separate commercial
  workspace. All new behavior here is additive to the open engine.
- Performance non-negotiable: persistent warm daemon (no cold start), pooled upstream
  connections, stream-in/stream-out (never buffer whole payloads), single loopback hop.

## Architecture

```
              ┌─────────────── deectx engine (unchanged) ───────────────┐
              │  reverse proxy 127.0.0.1:8787                          │
              │   ├─ /v1/chat/completions   (OpenAI-compatible)        │
              │   ├─ /v1/messages           (Anthropic)                │
              │   ├─ /v1/responses  (WebSocket, NEW)                   │
              │   └─ /stats          (local JSON, NEW)                 │
              │  mask_walk → detect chain → mask → forward → rehydrate │
              └───────────────────────────────┬───────────────────────┘
        ▲ intercepted                           │ routed dynamically
┌───────┴────────┐                  ┌───────────▼───────────┐
│ AI tools        │                  │ real upstream         │
│ Cursor, Claude │                  │ OpenAI / Anthropic /  │
│ Codex, Copilot │                  │ fallback (defaults)   │
└────────────────┘                  └───────────────────────┘

   NEW — deectx setup (auto-wiring):  detect tools → rewrite base URLs (.bak) →
        install warm daemon → doctor / unwrap
   NEW — dynamic upstream router (in proxy): key-shape classification
   NEW — GET /stats + deectx status: lightweight live tracker
```

## Components

### A. `deectx setup` — auto-wiring installer (new subcommand)

- Automatically detects installed AI tools (Cursor, Claude Code, Codex CLI, opencode, Copilot
  CLI), scanning known config locations plus `PATH`.
- For each supported tool, rewrites its provider config to point at
  `http://127.0.0.1:8787`, keeping a `.bak` copy of every file it touches. Idempotent —
  re-running never double-patches.
- Installs the daemon for auto-start (macOS LaunchAgent, Linux systemd user unit, Windows
  scheduled task/service) so the proxy stays warm.
- `deectx doctor`: verifies each tool's wiring and the daemon state; reports and fixes.
- `deectx unwrap`: restores every `.bak`, removing all patches.
- Skips locked-OAuth tools with an explanation (Claude Pro/Max, Copilot built-in).

### B. Dynamic upstream router (additive, inside proxy)

- Per request, inspect `Authorization` key shape: `sk-ant-` → Anthropic; `sk-` (OpenAI
  format) → OpenAI; otherwise heuristic or configured fallback.
- `upstream` config field remains as the documented fallback; default keeps today's OpenAI
  endpoint so zero-config holds.
- Router result cached per client to avoid re-classifying repeated requests from one tool.

### C. Responses API WebSocket endpoint (new)

- New handler for `POST /v1/responses` supporting OpenAI's Responses API over WebSocket, used
  by Codex CLI / GitHub Copilot CLI.
- Intercepts the inbound prompt, masks, forwards over a single WebSocket to the real provider,
  streams masked chunks back rehydrated to original placeholders.
- Reuses the existing mask/rehydrate pipeline; only frame handling is new.

### D. Warm daemon

- Auto-start service runs the existing `deectx` static binary with a warm reqwest
  connection pool. Request path: tool → persistent loopback → pooled upstream TLS session.

### E. Lightweight activity tracker (new, local-only)

- In-memory live counters on the proxy, since daemon start: total requests, masked events,
  redacted events, alerts. Cheap atomic counters; no per-request IO.
- `GET http://127.0.0.1:8787/stats` (new) returns these as JSON; loopback-only by default.
- `deectx status` reads the same endpoint for a terminal-friendly glance.
- Ledger stays the durable record; the tracker is a live signal with no identity/weighting.
- Explicitly out of Pro scope: no user identity, no regulation buckets, no weighted scores.

## Data Flow (end-to-end)

1. Tool starts, reads its rewired config → sends requests to `127.0.0.1:8787`.
2. Proxy classifies the upstream by the request's key shape → picks real provider.
3. Prompt runs mask_walk → detect chain → mask → forward to real provider over pooled conn.
4. Response streams back; masked chunks are rehydrated to original values before the tool
   sees them (provider saw `[EMAIL_1]`; tool sees `name@example.com`).
5. Ledger entry appended (hashes only); live counters bumped.

## Error Handling

- Fail-closed default preserved: if required detectors are not ready, the proxy returns 503
  rather than leaking; `doctor` explains why.
- Unclassifiable upstream → fall back to configured `upstream`; never drop the request.
- Unsupported/untracked tool → `setup` reports it and leaves it untouched; the rest of the
  setup still completes.
- Rewire reversal always available via `unwrap`; idempotent re-runs never double-patch.

## Performance Budget (non-negotiable)

- Local overhead < 10ms p99 on default (non-NER) packs.
- No regression to time-to-first-byte / TTFT for streaming paths.
- Warm daemon: no per-request process spawn; pooled connections save ~1 RTT.
- Streaming forward, never buffer whole payloads.

## Testing

- Unit: router key-shape classification; setup config patcher against golden config files;
  rehydration round-trips.
- Integration: each supported tool config patched → served through proxy → masked → rehydrated
  (reuse `proxy_integration.rs` harness).
- E2E: real WebSocket Responses stream through the proxy against a mock upstream; assert TTFT
  not degraded.
- Perf smoke test: assert local overhead < 10ms p99 on default packs.

## Documentation Updates (post-implementation)

- `README.md`: new `deectx setup` quick-start, supported tools, `/stats` preview, unwrap/doctor
  usage.
- `ARCHITECTURE.md`: add router, `/v1/responses` WS, `/stats`, setup auto-wiring; refresh the
  interception diagram and document the setup flow.
- `config.example.toml`: document the fallback `upstream` semantics and new default.
- Refactoring & design docs in `docs/superpowers/specs/` and `docs/superpowers/plans/`.

## Rollout

Single implementation plan (sections above are one cohesive feature, not independent
subsystems). Written next via the writing-plans skill. Then docs updated as a final task.