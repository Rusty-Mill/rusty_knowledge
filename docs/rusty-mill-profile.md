# Rusty Knowledge repository standards profile

**Status:** Draft — not yet Accepted. Per [`repository-profile.md`](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/software-development/repository-profile.md) (`RM-DEV-PROFILE-0001`–`0005`) in `rusty_foundation_akb`, a repository without a current valid (i.e. Accepted) profile cannot host an authorized implementation trial or publish a Rusty Mill conformance/release claim. This document exists to make the profile's required fields visible and honestly incomplete — not to claim readiness it doesn't have. Disclosing a gap here is the point, not a defect to hide, per this ecosystem's evidence culture.

| Field | Value |
|---|---|
| Profile identity/version | `rusty-knowledge-profile` v0 (Draft, first revision) |
| Repository/components | `rusty-mill/rusty_knowledge` — entire repository; no components exist yet (pre-code) |
| Architecture/domain inputs | Architecture model 1.99.0; [RFC-0003](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/rfc/0003-rusty-knowledge-domain-framework.md) (Draft); [ADR-0164](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0164-rusty-knowledge-is-a-domain-framework.md), [ADR-0165](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0165-knowledge-layered-authority-carries-over-as-a-requirement.md) (Proposed); `knowledge` domain has no accepted capability contract |
| Trial/maturity | No trial authorized. [TRIAL-0003](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/implementation-trials/rusty-knowledge-trial-proposal.md) entry review is **Not authorized** (Subject, Repository gates fail). Nonclaims: no conformance, no release, no production readiness, no API stability — none apply because none exist yet |
| Toolchain | Not yet selected. Candidate research (not decision) exists in [`platform-research.md`](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/02-capabilities/knowledge/platform-research.md): Rust edition/MSRV, stable-channel policy, and target triples are all open questions for the trial's entry review, not this profile |
| Rules | Inherits [Rusty Mill software development standards](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/software-development/README.md) in full; no local strengthening or exception exists yet |
| Unsafe/FFI | None yet — no code exists. `rusqlite`'s `bundled` feature and `sqlite-vec`'s FFI bindings (both researched, not selected, in `platform-research.md`) would be the likely unsafe/FFI surface if a trial is authorized; no budget, owner, or invariant is defined until that happens |
| Dependencies | None selected. Candidates researched, not chosen: `rusqlite` (MIT), `sqlite-vec` (MIT/Apache-2.0, **pre-1.0/alpha**, disclosed), `rmcp` (Apache-2.0, exact current version unresolved — see `platform-research.md`'s disclosed version discrepancy). No lock/vendor strategy, license-compatibility review, or advisory/update-cadence policy exists yet |
| Verification | None. No test suite, no conformance harness, no fuzz/model tests — this repository has no code to verify |
| Performance | None. No benchmark scenario, baseline, or budget exists; [`benchmarks.md`](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/02-capabilities/knowledge/benchmarks.md) in the domain docs states plainly that no benchmark has been run |
| Cross-cutting | None executed. [`cross-cutting.md`](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/02-capabilities/knowledge/cross-cutting.md) in the domain docs records planned evidence only; Review status Unknown |
| CI/release | No CI configured in this repository yet. No runner trust, artifact, provenance, or publication authority decision has been made. No release channel exists |
| Exceptions | None active. If solo-maintainer review sufficiency needs to be invoked for this repository's own future decisions, it is covered by [RFC-0004](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/rfc/0004-solo-maintainer-review-sufficiency.md) in `rusty_foundation_akb`, not a repository-local waiver |

## What this profile does and does not authorize

This profile's existence resolves TRIAL-0003's Repository gate from "no standards profile exists" toward evaluable — it does **not** itself flip that gate to Pass (the gate also requires the profile to be **Accepted**, which this Draft is not), and it does not authorize a dependency, a line of code, a CI workflow, or a first substantive commit beyond this bootstrap. Per [RM-DEV-PROFILE-0005](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/software-development/repository-profile.md), a Draft profile still cannot host an authorized trial.

## Change log

| Revision | Date | Change |
|---|---|---|
| 0 | 2026-08-10 | Initial Draft profile — bootstrap only, every substantive field disclosed as not-yet-decided |
