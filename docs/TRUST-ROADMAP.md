# deeCtx Trust & Certification Roadmap

The path from "an open-source tool" to "a tool an enterprise security team will
approve." There is **no single authority** that certifies a tool — enterprise
trust is a *stack* of independent signals. This is the tracker for assembling
that stack, cheapest-and-highest-leverage first.

deeCtx is a **security/privacy tool itself**, so trust is doubly load-bearing:
we must prove both that we're safe to run *and* that the masking actually works.

## How to read this

- **Status:** ☐ not started · ◐ in progress · ☑ done
- **Cost:** one-time or annual, rough. $0 = free.
- **Track A (OSS core):** artifact- and code-level trust — cheap, do now.
- **Track B (deectx-pro / company):** organizational certs — for enterprise sales.

---

## Phase 0 — Free, now (Track A)

These fill a security engineer's checklist for "is this binary safe to run?"

| ☑/☐ | Item | What it proves | Authority / tool | Cost |
|-----|------|----------------|------------------|------|
| ☑ | **Sigstore/cosign signed releases** | Each binary is authentic & tamper-evident | Sigstore (keyless OIDC) — `release.yml` | $0 |
| ☑ | **SLSA build provenance** | Binary came from *this* commit via *this* CI run | `actions/attest-build-provenance` (SLSA L2) — `release.yml` | $0 |
| ☑ | **SBOM (CycloneDX)** | Every dependency is disclosed | anchore/syft, CycloneDX — `release.yml` | $0 |
| ☑ | **SECURITY.md + disclosure policy** | A defined way to report vulnerabilities | GitHub Security Advisories (CNA) — `SECURITY.md` | $0 |
| ☑ | **OpenSSF Scorecard** | Automated repo security posture score | OpenSSF — `scorecard.yml` | $0 |
| ☑ | **Dependency & vuln scanning** | No known-vulnerable deps ship | `rustsec/audit-check` in CI — `ci.yml` | $0 |
| ☑ | **OpenSSF Best Practices Badge** | Self-certified secure-dev checklist, public badge — **PASSING** | OpenSSF — [project 13981](https://www.bestpractices.dev/projects/13981) | $0 |
| ☐ | **Pin GitHub Actions by SHA** | CI supply chain can't be swapped under us | (repo hygiene) | $0 |
| ☐ | **OSS-Fuzz enrollment** | Detector/regex chain is continuously fuzzed | Google OSS-Fuzz (free for OSS) | $0 |
| ☐ | **Published golden-set benchmark** | Detection efficacy (false-negative rate) is measurable | own `tests/golden_set.rs`, versioned | $0 |
| ☐ | **CHANGELOG + real release notes** | Users can judge upgrade impact (OpenSSF `release_notes`) | Keep a Changelog + `release.yml` wiring | $0 |
| ☐ | **Threat-model whitepaper** | Honest limits & guarantees, promoted from ARCHITECTURE.md §9 | own doc, signed | $0 |
| ☐ | **Reproducible builds** (stretch) | Anyone can rebuild the exact published binary | `--locked`, pinned toolchain | $0 |

> **Validation note:** the four shipped CI/release items are wired but only
> fully exercised on the **next `v*` tag** (release) and next push to `master`
> (Scorecard). Cut a test tag to confirm attestation + SBOM upload succeed.

**Verification story to advertise** (README + SECURITY.md):
`gh attestation verify <artifact> --repo deectxone/deectx` and
`cosign verify-blob --bundle <artifact>.cosign.bundle <artifact>`.

---

## Phase 1 — Low cost (Track A)

| ☑/☐ | Item | What it proves | Authority / tool | Cost |
|-----|------|----------------|------------------|------|
| ☐ | **Code-signing certificate** | Windows SmartScreen / macOS Gatekeeper won't flag the binary or the `deectx daemon-install` autostart entry | DigiCert / Sectigo / SSL.com (CA) | ~$100–500/yr (EV higher) |
| ☐ | **Apple notarization** | macOS Gatekeeper trusts the binary | Apple Developer Program | $99/yr |
| ☐ | **Security whitepaper (public)** | Data-flow + threat model as a standalone artifact | own | $0–low |

> **Why code signing matters specifically for deeCtx:** `deectx setup` /
> `deectx daemon-install` write a login autostart entry and patch tool configs.
> Unsigned, enterprise EDR flags that immediately. Signing is the difference
> between "auto-blocked" and "runs clean."

---

## Phase 2 — External validation (Track A/B) — *the credibility unlock*

For a **masking** tool, the single most persuasive signal is an independent
party confirming the masking actually catches what we claim.

| ☑/☐ | Item | What it proves | Authority / firm | Cost |
|-----|------|----------------|------------------|------|
| ☐ | **Independent security audit** | Detection pipeline + fail-closed gate work; no code backdoors | Cure53 / Trail of Bits / NCC Group / Radically Open Security | ~$15k–60k (scoped) |
| ☐ | **Penetration test (report)** | No exploitable proxy/daemon surface | reputable pentest firm | ~$8k–30k |
| ☐ | **Privacy / DPIA legal review** | GDPR + AU CDR claims hold up | privacy counsel / DPO consultancy | legal fees |

---

## Phase 3 — Organizational certifications (Track B — deectx-pro)

Certify the *company and processes*, not the code. These are procurement gates
for enterprise SaaS. Pick by target market; you don't need all at once.

| ☑/☐ | Item | What it proves | Authority | Cost / time |
|-----|------|----------------|-----------|-------------|
| ☐ | **SOC 2 Type II** | Operating security controls over a period | AICPA-licensed CPA auditor | $15k–60k+, 6–12 mo |
| ☐ | **ISO/IEC 27001** | Information Security Management System | Accredited body (BSI, Bureau Veritas, …) | $20k–50k, 6–12 mo |
| ☐ | **ISO/IEC 42001** | AI Management System — apt: we sit in AI pipelines | Accredited body | emerging, similar |
| ☐ | **CSA STAR / CAIQ** | Cloud security self- or 3rd-party assessment | Cloud Security Alliance | $0 (L1 self) → paid |
| ☐ | **GDPR / CDR alignment record** | Documented regulatory posture (DPIA, RoPA) | internal + counsel | legal fees |

---

## Cross-cutting — the packet that actually closes deals

Most enterprise reviews unblock on a **vendor packet**, not a single cert number.

| ☑/☐ | Item | Notes |
|-----|------|-------|
| ☐ | **Trust Center page** | Central hub: SOC 2 report, pen-test summary, SBOM, `SECURITY.md`, data-flow diagram |
| ☐ | **Completed CAIQ questionnaire** | Cloud Security Alliance's standard vendor questionnaire |
| ☐ | **Completed VSA questionnaire** | Vendor Security Alliance questionnaire |
| ☐ | **Data-flow / architecture diagram** | Lead with the strongest line: *local-first, no data egress — source never leaves the machine* |

> deeCtx's strongest single argument preempts the #1 objection before any cert:
> **local-first, no egress.** Prove it with a network-capture demo and put it
> at the top of the Trust Center.

---

## Sequencing summary

1. **Done:** cosign + SLSA + SBOM in `release.yml`, `SECURITY.md`, Scorecard workflow, `cargo audit` in CI, **OpenSSF Best Practices Badge (passing, project 13981)**.
2. **This month (free):** pin actions by SHA, add CHANGELOG + wire real release notes, publish golden-set benchmark, OSS-Fuzz application, threat-model whitepaper.
3. **Low budget:** code-signing cert + Apple notarization; public security whitepaper.
4. **On first enterprise interest:** independent masking audit → pen test.
5. **Selling deectx-pro:** SOC 2 Type II **or** ISO 27001 (+ ISO 42001); build Trust Center; answer CAIQ/VSA.

*Last updated: 2026-08-07.*
