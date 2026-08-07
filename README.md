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

deeCtx is a **transparent** proxy: endpoints it doesn't handle explicitly are
forwarded verbatim, so your wired tools keep working.

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

Prebuilt binaries are published for **Windows (x86_64)**, **macOS (Intel + Apple
Silicon)**, and **Linux (x86_64)**. The paths below **download a prebuilt binary
— no compiler or C/C++ linker required** — and are the recommended way in:

- **Scoop (Windows, recommended)**:
  ```powershell
  scoop bucket add deectx https://github.com/deectxone/scoop-deectx
  scoop install deectx
  ```
- **cargo-binstall (all platforms)** — cargo-native, fetches the release archive
  instead of compiling (one-time: `cargo install cargo-binstall`):
  ```bash
  cargo binstall deectx
  ```
- **Homebrew (macOS/Linux)** — installs the prebuilt release binary, no linker:
  ```bash
  brew tap deectxone/deectx && brew install deectx
  ```
- **Binary zip/tarball**: download from
  [GitHub Releases](https://github.com/deectxone/deectx/releases) — one archive
  per target, built by CI.

Building **from source** instead (needs a working C/C++ linker — see
[Troubleshooting](#troubleshooting) if the build fails):

- **Cargo**: `cargo install deectx`
- **Local checkout**: `cargo install --path .` or `cargo build --release`
  (see [Development](#development)).

All package paths ship and are refreshed automatically on release. Scoop
requires adding the bucket once before `scoop install deectx` will work —
deectx isn't in Scoop's main bucket.

### First time? Install the package manager first

Each path needs its tool present. If a command errors with *"not recognized"* /
*"command not found"*, install the prerequisite, open a **new** terminal, then retry:

- **Scoop** (Windows) — in PowerShell:
  ```powershell
  Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
  Invoke-RestMethod -Uri https://get.scoop.sh | Invoke-Expression
  ```
- **Rust / Cargo** (for `cargo binstall` or `cargo install`) — install via
  [rustup.rs](https://rustup.rs). On Windows that's `winget install Rustlang.Rustup`;
  on macOS/Linux, `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`.
  Then enable binstall once: `cargo install cargo-binstall`.
- **Homebrew** (macOS/Linux) — from [brew.sh](https://brew.sh):
  `/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"`

## Quick start: install-and-forget

The fastest path — one command turns deeCtx **on**:

```bash
deectx start
```

It discovers your installed tools, points them at the local proxy
(`http://127.0.0.1:8787`), and starts masking:

- **Wires Claude Code, Codex, and opencode** to the proxy, backing up every
  patched file to `<path>.bak` first (existing backups are never overwritten;
  re-running is idempotent). Tools locked to OAuth accounts (Claude Pro/Max,
  Copilot built-in) are skipped with a message.
- **Installs the autostart daemon** so the proxy starts at login, and starts it
  now. Re-running `deectx start` after an update replaces a stale proxy.

Then live with it:

```bash
deectx                 # status dashboard: on/off, wired tools, warnings
deectx stop            # turn OFF — restore tools to direct API + stop the proxy
deectx uninstall       # remove deeCtx (prompts before deleting your data)
deectx audit --today   # hash-only ledger summary
deectx status          # live masked/redacted counts from the running proxy's /stats
```

deeCtx stores its config and audit ledger in **`~/.deectx/`** (`config.toml`,
`ledger.jsonl`; set `$DEECTX_HOME` to relocate). `setup` and `unwrap` remain as
aliases of `start`/`stop`.

The proxy routes by API-key shape — Anthropic keys (`sk-ant-…`) go to the
Anthropic upstream, OpenAI keys (`sk-…`) to the OpenAI upstream — so one proxy
serves both tools. Codex / Copilot CLI traffic rides the `/v1/responses`
WebSocket, masked and rehydrated per frame.

## Quick start (manual)

Prefer to run the proxy yourself instead of `deectx start`?

```bash
# 1. (optional) seed a config — start/serve auto-create ~/.deectx/config.toml
cp config.example.toml ~/.deectx/config.toml

# 2. Run the proxy (reads ~/.deectx/config.toml by default)
deectx serve
# listening on http://127.0.0.1:8787

# 3. Point your AI tool at the proxy
#    OpenAI-compatible base:  http://127.0.0.1:8787/v1
#    Anthropic-compatible:    http://127.0.0.1:8787/v1/messages
```

To enable methods, set `active_packs = ["gdpr"]` (or `["cdr-au"]`) in
`~/.deectx/config.toml`. NER (semantic detection of people, addresses, health
terms) is optional and requires a GLiNER ONNX model — see both files and
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
deectx audit --today --export report.json
```

---

## Configuration

See [`ARCHITECTURE.md`](ARCHITECTURE.md) §11 for the full field reference. Config
lives at `~/.deectx/config.toml` by default; the defaults work out of the box.
The notable knobs are:

| Setting | What it controls |
|---------|------------------|
| `upstream` / `upstream_anthropic` | Where masked requests are sent. |
| `active_packs` | Built-in PII packs to turn on (`default`(always) , `gdpr`, `cdr-au`). |
| `ner` + `model_dir` | Optional semantic NER via a local GLiNER ONNX model. |
| `allowlist` | Values never masked (case-insensitive). |
| `ledger_path` / `ledger_retention_days` | Audit log location (default `~/.deectx/ledger.jsonl`) and retention. |

---

## AI product support

### What works

Any client that can be pointed at deeCtx's `:8787` base URL and uses one of the
two supported request schemas:

- **OpenAI-compatible** chat: `/v1/chat/completions` — including **streaming**
  (SSE) responses rehydrated in real time.
- **Anthropic-compatible** messages: `/v1/messages` (and `/v1/messages/count_tokens`).
- Concretely supported examples (shims included): **Cursor** and **opencode**,
  plus any LangChain/OpenAI SDK caller with a configurable `base_url`/`OPENAI_BASE_URL`.

Endpoints deeCtx doesn't handle explicitly (e.g. `/v1/models`) are **forwarded
verbatim**, so tools that call them keep working.

### Not supported (know the limits)

- Model APIs with a **non‑OpenAI/Anthropic wire format** (e.g. Gemini native
  REST, Bedrock-native) **as-is** — they'd need a new request/response adapter.
- **Non‑text / binary** outputs (images, blobs) are not rehydrated; they pass
  through as masked (they were masked before leaving your machine).
- **Batch / legacy-completions / embeddings** requests forwarded via the
  transparent fallback are currently passed **verbatim** (not masked) — their
  asynchronous results can't be rehydrated by a stateless proxy. Prefer the
  masked `/v1/chat/completions` and `/v1/messages` paths for sensitive prompts.
- deeCtx is not a remote proxy or filtering firewall — it is specific to your
  local model traffic. Full guarantees and caveats: [`ARCHITECTURE.md`](ARCHITECTURE.md) §9 (threat model).

---

## Commands

Lifecycle (the guided surface — states are ACTIVE ⇄ OFF):

```bash
deectx                 # status dashboard (on/off, wired tools, warnings)
deectx start           # turn ON: wire tools + install autostart + start masking
deectx stop            # turn OFF: restore tools to direct API + stop the proxy
deectx uninstall       # stop + restore tools; prompts to delete data (never removes the binary)
```

Operate & inspect:

```bash
deectx serve                                  # run the proxy directly (uses ~/.deectx/config.toml)
deectx audit --today                          # console summary of the hash-only ledger
deectx audit --today --export report.json     # JSON export
deectx status [--json]                        # live counters from the running proxy's /stats
deectx doctor                                 # check which tools are wired
```

`setup` → alias of `start`, `unwrap` → alias of `stop`; `daemon-install` /
`daemon-uninstall` manage the login autostart entry directly.

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

Building on Windows without the MSVC toolchain? Use the GNU toolchain + MSYS2
mingw64: set `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=C:/msys64/mingw64/bin/gcc.exe`
and `RUSTFLAGS="-C dlltool=C:/msys64/mingw64/bin/dlltool.exe"`, then
`cargo +stable-x86_64-pc-windows-gnu test`. (CI's `windows-latest` has MSVC and
needs none of this.)

---

## Troubleshooting

> **Fastest fix for any build error below:** don't build at all. `scoop install`
> (Windows) or `cargo binstall deectx` (all platforms) download a prebuilt binary
> and need no compiler or linker. See [Install](#install).

### Runtime: `deectx setup` says "unrecognized subcommand", or "Prompt is too long" after wiring

Both mean your **installed binary is older than your wiring**. `deectx setup`
(now an alias of `start`) and the transparent proxy that fixes "Prompt is too
long" ship in newer builds. Update to the latest release, then re-run:

```powershell
scoop update deectx      # or: cargo binstall deectx / brew upgrade deectx
deectx start             # re-wires tools and replaces any stale proxy
```

A stale proxy from a previous version keeps running until `deectx start` (or a
reboot) replaces it — `deectx start` stops the old one first.

### Runtime: `deectx serve` fails with "address in use" (os error 10048)

A proxy is already listening on `127.0.0.1:8787` — usually the autostart daemon
`deectx start` installed. You don't need `deectx serve` when the daemon is
running; check state with `deectx` (the dashboard). To take over the port,
`deectx stop` first, or reconfigure `listen` in `~/.deectx/config.toml`.

### `cargo install deectx` fails on Windows with `link.exe not found`

If your default Rust toolchain is `x86_64-pc-windows-msvc` (the Windows default)
and Visual C++ build tools aren't installed, `cargo install` fails with:

```text
error: linker `link.exe` not found
note: the msvc targets depend on the msvc linker but `link.exe` was not found
note: please ensure that Visual Studio 2017 or later, or Build Tools for Visual
      Studio were installed with the Visual C++ option
```

**Cause.** `cargo install` compiles from source, and the MSVC target links with
`link.exe`, which ships only with the Visual C++ toolchain — not with Rust, and
not with VS Code. This is an environment prerequisite, not a deeCtx bug.

**Fix (pick one):**

1. **Skip the build entirely (recommended)** — install a prebuilt binary:
   ```powershell
   scoop bucket add deectx https://github.com/deectxone/scoop-deectx
   scoop install deectx
   # or, cargo-native:
   cargo binstall deectx
   ```
2. **Install the MSVC linker** — Build Tools for Visual Studio with the
   **"Desktop development with C++"** workload (provides `link.exe`), then retry
   `cargo install deectx`.
3. **Use the GNU toolchain**, which bundles its own linker (no Visual Studio),
   but see the `dlltool` note below:
   ```powershell
   rustup toolchain install stable-x86_64-pc-windows-gnu
   rustup default stable-x86_64-pc-windows-gnu
   cargo install deectx
   ```

### `cargo install deectx` fails on Windows with a `dlltool` error

If your default Rust toolchain is `x86_64-pc-windows-gnu`, building from source
can fail with:

```text
error: error calling dlltool 'dlltool.exe': program not found
error: dlltool could not create import library ... CreateProcess
```

**Cause.** rustup's bundled MinGW is incomplete — it ships `dlltool` but not the
`as`/`ar` binaries it spawns. The `windows-sys`/`windows-link` crates (transitive
dependencies via `parking_lot`, `reqwest`, …) generate Windows import libraries
at link time, and that generation fails on this toolchain. This is an
environment issue, not a deeCtx bug.

**Fix (pick one):**

1. **Install a full MinGW-w64 (recommended)** — e.g. MSYS2:
   ```powershell
   winget install MSYS2.MSYS2
   ```
   then add `C:\msys64\mingw64\bin` to your `PATH` (System Settings → Environment
   Variables, then open a new terminal) and retry:
   ```powershell
   cargo install deectx
   ```
2. **Switch to the MSVC toolchain** — install Visual Studio Build Tools, then:
   ```powershell
   rustup default stable-x86_64-pc-windows-msvc
   cargo install deectx
   ```
3. **Skip the build entirely** — use a prebuilt release binary, or Scoop:
   ```powershell
   scoop bucket add deectx https://github.com/deectxone/scoop-deectx
   scoop install deectx
   ```
   (see [Install](#install)).

### `scoop install deectx` fails with `Couldn't find manifest for 'deectx'`

deectx isn't in Scoop's main bucket, so `scoop install deectx` on its own will
always fail with this error. Add the deectx bucket first, then install:

```powershell
scoop bucket add deectx https://github.com/deectxone/scoop-deectx
scoop install deectx
```

---

## Updating & uninstalling

Update in place — same tool you installed with:

| Installed with | Update | Uninstall the binary |
|----------------|--------|----------------------|
| Scoop | `scoop update deectx` | `scoop uninstall deectx` |
| cargo-binstall | `cargo binstall deectx` (re-run) | `cargo uninstall deectx` |
| Cargo | `cargo install deectx` (re-run) | `cargo uninstall deectx` |
| Homebrew | `brew upgrade deectx` | `brew uninstall deectx` |

**To remove deeCtx cleanly, run `deectx uninstall` first** — it stops the proxy,
restores every tool config from its `.bak` backup, removes the login autostart
entry, and prompts before deleting your data:

```bash
deectx uninstall          # unwire tools + stop + remove autostart; asks about data
```

Then remove the **binary** with the uninstall command for your install path
above. Your config and ledger in `~/.deectx/` (`config.toml`, `ledger.jsonl`)
are kept unless you answered "yes" to the delete prompt (or ran
`deectx uninstall --purge`) — delete `~/.deectx/` by hand if you want them gone.

---

## More

- [ARCHITECTURE.md](ARCHITECTURE.md) — the full engineering deep-dive.
- [shims/README.md](shims/README.md) — Cursor & opencode integration.
- `scripts/release.*` — release packaging (zip + brew/scoop hashes).
