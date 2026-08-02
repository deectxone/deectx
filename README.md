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