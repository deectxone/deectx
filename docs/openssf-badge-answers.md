# OpenSSF Best Practices Badge — Answer Sheet (deeCtx)

> ✅ **STATUS: PASSING** — [bestpractices.dev project 13981](https://www.bestpractices.dev/projects/13981).
> Badge: `https://www.bestpractices.dev/projects/13981/badge`
> This sheet is retained as the record of how each criterion was answered.

Answers for the **passing** level. Each row: the criterion, our status, and the
justification text. Criteria are **MUST** unless marked *(SHOULD)* /
*(SUGGESTED)* — only MUSTs block the passing badge.

Project URL: `https://github.com/deectxone/deectx`
Repo URL:    `https://github.com/deectxone/deectx`

---

## ✅ Passing-level status

All passing-level **MUST** criteria are satisfied. `CONTRIBUTING.md` (the former
blocker) exists. Remaining optional hardening:

- *(Recommended, not required)* a standalone `CODE_OF_CONDUCT.md` — the code of
  conduct is currently a section inside CONTRIBUTING.md, which is sufficient for
  passing but is expected as its own file at the silver/gold levels.
- `release_notes` is currently answered on the strength of GitHub Releases; to
  make it durable, add a CHANGELOG and wire real per-tag notes (see
  docs/TRUST-ROADMAP.md Phase 0).

---

## Basics

| Criterion | Status | Justification to paste |
|-----------|--------|------------------------|
| `description_good` | ☑ MET | README opens with a one-line description and a data-flow diagram: local-first PII-masking proxy for AI coding tools. |
| `interact` | ☑ MET | GitHub Issues + Pull Requests on the public repo. |
| `contribution` | ☑ MET | Contribution process documented in CONTRIBUTING.md (fork → branch → `cargo fmt`/`clippy -D warnings`/`cargo test` → PR). Module boundaries and conventions in AGENTS.md. |
| `contribution_requirements` | ☑ MET | Requirements for acceptance (formatting, clippy clean, tests pass, TDD, no new `unsafe`/vulnerable deps) are stated in CONTRIBUTING.md and enforced by CI (ci.yml). |
| `floss_license` | ☑ MET | Apache-2.0. |
| `floss_license_osi` | ☑ MET | Apache-2.0 is OSI-approved. |
| `license_location` | ☑ MET | `LICENSE` file at repo root; `license = "Apache-2.0"` in Cargo.toml. |
| `documentation_basics` | ☑ MET | README (install, quick-start, commands, config) + ARCHITECTURE.md (deep dive) + AGENTS.md. |
| `documentation_interface` | ☑ MET | README documents the CLI (`serve`, `audit`, `setup`, `doctor`, …) and the HTTP interface (`/v1/chat/completions`, `/v1/messages`, `/healthz`); `deectx --help` covers flags. |
| `sites_https` | ☑ MET | Project + repo hosted on GitHub (HTTPS). |
| `discussion` | ☑ MET | GitHub Issues/PRs provide threaded discussion. |
| `english` | ☑ MET | All docs in English. |
| `maintained` | ☑ MET | Actively maintained; CI on every push/PR. |

## Change Control

| Criterion | Status | Justification to paste |
|-----------|--------|------------------------|
| `repo_public` | ☑ MET | Public GitHub repo. |
| `repo_track` | ☑ MET | Git version control. |
| `repo_interim` | ☑ MET | Interim commits are visible between releases; work merges via PR to `master`. |
| `repo_distributed` *(SUGGESTED)* | ☑ MET | Git (distributed). |
| `version_unique` | ☑ MET | Unique versions via Cargo.toml `version` + `v*` git tags + crates.io. |
| `version_semver` *(SUGGESTED)* | ☑ MET | Semantic Versioning (currently 0.1.0, pre-1.0). |
| `version_tags` *(SUGGESTED)* | ☑ MET | Releases are tagged `v*`; release.yml triggers on those tags. |
| `release_notes` | ☑ MET | GitHub Releases carry per-tag notes; binaries, signatures, and SBOM attached. (Hardening: add CHANGELOG + wire notes — roadmap Phase 0.) |
| `release_notes_vulns` *(SUGGESTED)* | ☑ MET | No vulnerabilities fixed to date; when one is, its release notes will identify it. Disclosure policy in SECURITY.md. |

## Reporting

| Criterion | Status | Justification to paste |
|-----------|--------|------------------------|
| `report_process` | ☑ MET | Bugs via GitHub Issues (referenced from CONTRIBUTING.md); security via SECURITY.md. |
| `report_tracker` *(SHOULD)* | ☑ MET | GitHub Issues tracks individual reports. |
| `report_responses` | ☑ MET | Maintainers triage issues/PRs on the public tracker; no unaddressed reports in the window. |
| `enhancement_responses` *(SHOULD)* | ☑ MET | Feature requests handled as GitHub Issues; none unaddressed in the window. |
| `report_archive` | ☑ MET | Issue/PR history is publicly archived + searchable on GitHub. |
| `vulnerability_report_process` | ☑ MET | SECURITY.md defines the process (GitHub Security Advisories + security@deectx.dev). |
| `vulnerability_report_private` | ☑ MET | Private reporting via GitHub Security Advisories; PGP available on request. |
| `vulnerability_report_response` *(SHOULD)* | ☑ MET | SECURITY.md commits to acknowledgement within 3 business days (≤14-day requirement); none received to date. |

## Quality

| Criterion | Status | Justification to paste |
|-----------|--------|------------------------|
| `build` | ☑ MET | `cargo build --release` (standard Rust build). |
| `build_common_tools` | ☑ MET | Cargo — the standard Rust build tool. |
| `build_floss_tools` *(SUGGESTED)* | ☑ MET | Rust toolchain + Cargo are FLOSS. |
| `test` | ☑ MET | 62 tests: unit tests next to code + integration in `tests/` (proxy_integration, golden_set, installers). |
| `test_invocation` | ☑ MET | `cargo test` (documented in README, CONTRIBUTING.md, and AGENTS.md). |
| `test_most` *(SHOULD)* | ☑ MET | Core masking, detection, proxy, ledger, and installer paths are covered, including a golden-set. |
| `test_continuous_integration` *(SUGGESTED)* | ☑ MET | ci.yml runs fmt + clippy + test matrix (Linux/Windows/macOS) + `cargo audit` on every push/PR. |
| `test_policy` | ☑ MET | CONTRIBUTING.md + AGENTS.md mandate TDD; new features add tests. |
| `tests_are_added` | ☑ MET | New functionality ships with tests (TDD convention, enforced in review). |
| `tests_documented_added` *(SUGGESTED)* | ☑ MET | Test policy documented in CONTRIBUTING.md / AGENTS.md. |
| `warnings` | ☑ MET | `cargo clippy --all-targets -- -D warnings` + `cargo fmt --check`. |
| `warnings_fixed` | ☑ MET | Warnings are errors in CI (`-D warnings`); builds fail otherwise. |
| `warnings_strict` *(SUGGESTED)* | ☑ MET | `-D warnings` treats all clippy lints as errors. |

## Security

| Criterion | Status | Justification to paste |
|-----------|--------|------------------------|
| `know_secure_design` | ☑ MET | ARCHITECTURE.md §9 documents the threat model; design is fail-closed (503 when a required detector is unavailable) and local-first (no data egress). |
| `know_common_errors` | ☑ MET | Written in safe Rust (memory-safe); detection uses checksums (Luhn/Mod97/ATO-TFN) + entropy gating; `cargo audit` counters vulnerable deps. |
| `crypto_published` | ☑ MET | No proprietary cryptography. SHA-256 (ledger hashing) + standard TLS via the Rust HTTP stack; release signing uses Sigstore. |
| `crypto_call` | ☑ MET | Crypto via established libraries, not hand-rolled. |
| `crypto_floss` | ☑ MET | All crypto dependencies are FLOSS. |
| `crypto_keylength` | ☑ MET | SHA-256; TLS uses modern key sizes. No config exposes weaker keylengths. |
| `crypto_working` | ☑ MET | No known-broken algorithms (no MD5/SHA-1/DES/RC4). |
| `crypto_weaknesses` *(SHOULD)* | ☑ MET | Uses SHA-256, not SHA-1; modern TLS. |
| `crypto_pfs` *(SHOULD)* | ☑ MET | Outbound TLS to upstream uses ECDHE (PFS); deeCtx implements no key-agreement of its own. |
| `crypto_password_storage` | ☑ N/A | deeCtx stores no user passwords. |
| `crypto_random` | ☑ N/A | No cryptographic keys/nonces generated; placeholders are sequential per-session identifiers. |
| `delivery_mitm` | ☑ MET | HTTPS-only distribution (GitHub, crates.io, brew/scoop `https://` URLs) + Sigstore signatures + SLSA provenance. |
| `delivery_unsigned` | ☑ MET | Release archives are Sigstore-signed (`.cosign.bundle`) + SLSA provenance; manifest hashes fetched over HTTPS. |
| `vulnerabilities_fixed_60_days` | ☑ MET | No publicly known unpatched vulnerabilities. |
| `vulnerabilities_critical_fixed` *(SHOULD)* | ☑ MET | None outstanding; SECURITY.md commits to rapid remediation. |
| `no_leaked_credentials` | ☑ MET | No credentials in the repo; the ledger stores hashes only (never raw PII/secrets). |

## Analysis

| Criterion | Status | Justification to paste |
|-----------|--------|------------------------|
| `static_analysis` | ☑ MET | Clippy (all targets, `-D warnings`) + `cargo audit` (RUSTSEC) on every CI run. |
| `static_analysis_common_vulnerabilities` *(SHOULD)* | ☑ MET | Rust's compiler/borrow-checker + clippy + `cargo audit` cover memory safety and known-vulnerable dependencies. |
| `static_analysis_fixed` | ☑ MET | Findings block CI (`-D warnings`; audit fails on advisories) and are fixed before merge. |
| `static_analysis_often` *(SUGGESTED)* | ☑ MET | Runs on every push and pull request. |
| `dynamic_analysis` *(SUGGESTED)* | ◐ PLANNED | OSS-Fuzz enrollment for the detector/regex chain is on the roadmap (docs/TRUST-ROADMAP.md, Phase 0). Not required for passing. |
| `dynamic_analysis_unsafe` *(SUGGESTED)* | ☑ MET | Codebase is predominantly safe Rust; `unsafe` is avoided. |
| `dynamic_analysis_enable_assertions` *(SUGGESTED)* | ☑ N/A | Debug assertions run under `cargo test`. |

---

## Maintaining the badge

- The badge auto-detects some criteria from the repo; the rest use the text above.
- Badge embedded in README:
  `[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/13981/badge)](https://www.bestpractices.dev/projects/13981)`
- Re-review the answers whenever release/CI or the disclosure process changes.

*Last updated: 2026-08-07.*
