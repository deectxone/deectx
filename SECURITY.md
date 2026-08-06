# Security Policy

deeCtx is a local-first, privacy-preserving tool: it masks PII and secrets
before they leave your machine. Because it is a security tool, we hold its own
supply chain and disclosure process to a high bar. This document explains how to
report a vulnerability and how to verify that the binary you run is authentic.

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Report privately via GitHub Security Advisories:
<https://github.com/deectxone/deectx/security/advisories/new>

If you cannot use GitHub advisories, email **security@deectx.dev** (PGP key on
request).

Please include:

- A description of the issue and its impact (e.g. a class of PII that bypasses
  masking, a way to make the fail-closed gate fail *open*, or a proxy/daemon
  exposure).
- Steps to reproduce, ideally against a test config and synthetic data.
- The deeCtx version (`deectx --version`) and platform.

### What to expect

- **Acknowledgement:** within 3 business days.
- **Assessment & triage:** within 10 business days.
- **Coordinated disclosure:** we aim to release a fix and advisory within 90
  days, crediting you unless you prefer to remain anonymous.

### Scope — high-priority classes

Because of what deeCtx does, we treat these as especially serious:

- **Masking bypass** — real PII/secrets reaching the upstream model despite an
  active pack that should have caught it.
- **Fail-open** — the fail-closed gate (HTTP 503) not firing when a required
  detector is unavailable.
- **Ledger leakage** — raw PII written anywhere on disk (the ledger must store
  hashes only).
- **Local exposure** — the proxy or autostart daemon reachable or exploitable
  beyond `127.0.0.1`, or `deectx setup`'s config patching being abusable.

Detection *false negatives* on adversarial/novel inputs are expected in any
detector and are tracked as accuracy issues, not security vulnerabilities —
though we still want to hear about systematic gaps.

## Supply-chain integrity — verify what you run

Every released binary is **signed with Sigstore (keyless)** and carries **SLSA
build provenance**, so you can prove it was built from this repository's source
by our CI, and was not tampered with in transit or in a package mirror.

### Verify SLSA provenance (recommended)

```bash
gh attestation verify <artifact> --repo deectxone/deectx
```

This confirms the artifact's provenance attestation was produced by this repo's
release workflow.

### Verify the Sigstore signature

Each release archive ships with a `<artifact>.cosign.bundle`:

```bash
cosign verify-blob \
  --bundle <artifact>.cosign.bundle \
  --certificate-identity-regexp 'https://github.com/deectxone/deectx/.github/workflows/release.yml@.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  <artifact>
```

### Software Bill of Materials (SBOM)

A CycloneDX SBOM (`deectx-sbom.cyclonedx.json`) is attached to each release,
listing every dependency for your own vulnerability review.

## Supported versions

deeCtx is pre-1.0. Security fixes are released against the latest published
version. Please upgrade to the latest release before reporting.
