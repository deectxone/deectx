# deeCtx

Local-first PII-masking proxy for AI coding tools. Sits between your agent and the
model API: it masks PII and secrets with reversible session-scoped placeholders,
rehydrates responses, and writes a hash-only audit ledger.

## Install

- Cargo: `cargo install deectx`
- Homebrew: `brew install deectx`
- Scoop (Windows): `scoop install deectx` (manifest in `install/scoop/`)
- Binary releases: GitHub Releases (`scripts/release.ps1` / `scripts/release.sh`)

## Run

```bash
cp config.example.toml config.toml   # or start with defaults
deectx serve --config config.toml
```

Point your tool at `http://127.0.0.1:8787/v1` (OpenAI) or `/v1/messages` (Anthropic).
See `shims/` for Cursor and opencode integration.

## Audit

```bash
deectx audit --config config.toml --today --export report.json
```

## Latency budget

deeCtx sits on the hot path between your coding tool and the model API, so latency
is budgeted per-phase. Measured on a local machine; absolute numbers vary with CPU
and traffic, but the split and ceilings hold:

| Phase | Budget | Notes |
|-------|--------|-------|
| Rule/detection pass (regex + secrets) | ms-scale | In-process, no I/O |
| NER inference (when enabled) | per-chunk, fail-open | OnnxRuntime; disabled if model/proxy can't load |
| Masking + ledger write | async | Never blocks the response path |
| Response rehydration | in-memory | No network round-trip |

If the proxy starts to exceed its budget — especially in the detection phase with
NER enabled — it fails open rather than stalling requests, trading a missed mask
for not blocking your workflow. See the `tracing` output for per-phase timings.