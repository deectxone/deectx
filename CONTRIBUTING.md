# Contributing to deeCtx

Thanks for helping improve deeCtx — a local-first PII-masking proxy for AI
coding tools. This guide covers how to propose changes and what we require
before a pull request can be merged.

> deeCtx (this repo) is the **open-source, privacy-preserving core** and must
> stay OOA-pure: no org/telemetry/user-identity features here — those live in
> the separate commercial `deectx-pro` repo. See AGENTS.md.

## Ways to contribute

- **Report a bug** — open a [GitHub Issue](https://github.com/deectxone/deectx/issues).
  Include your `deectx --version`, platform, config (redacted), and repro steps.
- **Report a security vulnerability** — **do not** use public issues. Follow
  [SECURITY.md](SECURITY.md) (GitHub Security Advisories / security@deectx.dev).
- **Request a feature** — open an Issue describing the problem before the
  solution.
- **Submit code** — see below.

## Development setup

```bash
git clone https://github.com/deectxone/deectx
cd deectx
cargo build
cargo test          # 62 tests
```

On Windows, prefer a prebuilt binary for *using* deeCtx; for *building* see the
Troubleshooting section in the README (MSVC vs GNU toolchain notes).

## Making a change

1. **Fork** and create a topic branch off `master`.
2. Follow **test-driven development** — add or update tests first; tests live
   next to the code and in `tests/`.
3. Keep to the existing module boundaries (see AGENTS.md → *Where to make a
   change*). Add features as new focused files under `src/`, exported from
   `lib.rs` — don't pile into `proxy.rs`.
4. No raw PII written anywhere — **hashes only**. No env-var config (TOML only).

## Requirements for acceptance

A pull request must pass all of the following — CI (`.github/workflows/ci.yml`)
enforces them on every push and PR:

```bash
cargo fmt --all -- --check          # formatting
cargo clippy --all-targets -- -D warnings   # zero warnings
cargo test --workspace              # all tests green
```

Also expected:

- **New behavior ships with tests.** Bug fixes include a regression test.
- **No new `unsafe`** without a clear, documented justification.
- **No new known-vulnerable dependencies** (`cargo audit` runs in CI).
- Commits are focused; PR description explains the *why*, not just the *what*.
- Public API or CLI changes are reflected in the README / ARCHITECTURE.md.

## Detection / masking changes

Because deeCtx's value is detection accuracy, changes to detectors, packs, or
masking must:

- Add cases to the golden set (`tests/golden_set.rs`) demonstrating the new
  detection (and any false-positive it must *not* trigger).
- Preserve the **fail-closed** guarantee — a detector that can't run must lead
  to a refused request (HTTP 503), never a silent pass-through.

## Licensing

By contributing, you agree your contributions are licensed under the project's
**Apache-2.0** license (see [LICENSE](LICENSE)).

## Code of conduct

Be respectful and constructive. Harassment or abuse is not tolerated;
maintainers may remove comments, commits, or contributors that violate this.
