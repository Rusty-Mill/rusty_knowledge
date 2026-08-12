# Rusty Knowledge repository standards profile

**Status:** Draft — not yet formally Accepted through `rusty_foundation_akb`'s
process. Per [`repository-profile.md`](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/software-development/repository-profile.md)
(`RM-DEV-PROFILE-0001`–`0005`), a repository without a current valid (i.e.
Accepted) profile cannot host a formally authorized implementation trial or
publish a Rusty Mill conformance/release claim. **That governance status is
unchanged by this repository's own actions** — it can only change via a
decision in `rusty_foundation_akb`, which is outside this repository's
control. What *has* changed since this profile's original Draft (2026-08-10):
substantial implementation work proceeded anyway, per explicit direction from
the repository owner overriding the Subject/Repository gate blockers below
(see `gap-analysis.md`'s "Authorization caveat" for the record). The field
values below are corrected to describe this repository's actual current
state; the governance status line above is deliberately left as-is, since
asserting "Accepted"/"Authorized" here would be a claim this document has no
authority to make.

| Field | Value |
|---|---|
| Profile identity/version | `rusty-knowledge-profile` v0 (Draft, first revision — no formal revision to the profile itself since 2026-08-10; see status note above) |
| Repository/components | `rusty-mill/rusty_knowledge` — a working MCP server: the full 15-tool `knowledge-mcp` parity surface plus a candidate-suggestion tool, hybrid search, a `knowledge-mcp` SQLite importer, and optional persistent storage. See `README.md`/`ARCHITECTURE.md` for current capabilities |
| Architecture/domain inputs | Architecture model 1.99.0; [RFC-0003](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/rfc/0003-rusty-knowledge-domain-framework.md) (Draft); [ADR-0164](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0164-rusty-knowledge-is-a-domain-framework.md), [ADR-0165](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0165-knowledge-layered-authority-carries-over-as-a-requirement.md) (Proposed); `knowledge` domain still has no accepted capability contract in `rusty_foundation_akb` — unchanged by implementation proceeding here |
| Trial/maturity | [TRIAL-0003](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/implementation-trials/rusty-knowledge-trial-proposal.md)'s entry review was never formally passed and remains, as far as this repository can verify, **Not authorized** in `rusty_foundation_akb`'s own record. Implementation proceeded regardless, per explicit repository-owner direction (`gap-analysis.md`). Nonclaims stand: no formal conformance, release, production-readiness, or API-stability claim is made by this profile |
| Toolchain | Rust 2024 edition, selected pragmatically for implementation (see `Cargo.toml`) rather than through the trial's entry review, which never occurred. MSRV/stable-channel policy/target triples remain undecided as formal commitments |
| Rules | Inherits [Rusty Mill software development standards](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/software-development/README.md) in full; no local strengthening or exception exists |
| Unsafe/FFI | In active use: `rusqlite`'s `bundled` feature and `sqlite-vec`'s FFI bindings, via one direct `unsafe` block (`store::register_vec_extension`'s `sqlite3_auto_extension` registration, matching `sqlite-vec`'s own documented usage). No formal budget/owner/invariant review occurred through the trial process, since that process was never formally entered — disclosed as a standing gap, not backfilled retroactively |
| Dependencies | In use: `rusqlite` (MIT, `bundled` feature), `sqlite-vec` (MIT/Apache-2.0, pre-1.0), `rmcp` (Apache-2.0), `serde`/`serde_json`, `tokio`, `anyhow` — all from crates.io — plus two git dependencies pinned to a commit SHA rather than crates.io: `rusty_regx` and `rusty_embedder` (`baileyrd/rusty_regx`, `baileyrd/rusty_embedder`). No formal license-compatibility review, lock/vendor strategy, or advisory/update-cadence policy exists — same disclosed gap as the original Draft, not resolved by dependencies now existing |
| Verification | A test suite covering every module (`cargo test`), `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo fmt --all -- --check` all run in CI on every PR and push to `main`. No formal conformance harness or fuzz/model testing beyond that |
| Performance | None. No benchmark scenario, baseline, or budget exists; [`benchmarks.md`](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/02-capabilities/knowledge/benchmarks.md) in the domain docs states plainly that no benchmark has been run |
| Cross-cutting | None executed. [`cross-cutting.md`](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/02-capabilities/knowledge/cross-cutting.md) in the domain docs records planned evidence only; Review status Unknown |
| CI/release | `.github/workflows/ci-rust.yml` runs fmt/clippy/test on every PR and push to `main` — the required status check for this repository's "green CI, then merge with a merge commit" convention. No runner-trust/artifact/provenance/publication-authority decision has been formally made beyond that. No release channel exists (pre-1.0, nothing published — see `RELEASE_NOTES.md`) |
| Exceptions | None active. If solo-maintainer review sufficiency needs to be invoked for this repository's own future decisions, it is covered by [RFC-0004](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/rfc/0004-solo-maintainer-review-sufficiency.md) in `rusty_foundation_akb`, not a repository-local waiver |

## What this profile does and does not authorize

This profile's existence resolves TRIAL-0003's Repository gate from "no standards
profile exists" toward evaluable — it does **not** itself flip that gate to Pass
(the gate also requires the profile to be **Accepted**, which this Draft still is
not), and it does not retroactively authorize the implementation work that
proceeded anyway. Per [RM-DEV-PROFILE-0005](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/software-development/repository-profile.md),
a Draft profile still cannot host a formally authorized trial — that remains true
regardless of how much code exists in this repository today. The gap between
"formally authorized" and "actually implemented" is real and disclosed, not
resolved by this document.

## Change log

| Revision | Date | Change |
|---|---|---|
| 0 | 2026-08-10 | Initial Draft profile — bootstrap only, every substantive field disclosed as not-yet-decided |
| 0 (field update, no formal revision bump) | 2026-08-12 | Field *values* corrected to describe this repository's actual state (real dependencies, CI, tests, unsafe/FFI usage) after implementation proceeded per explicit repository-owner override. The governance **status** itself (Draft, not Accepted; trial not authorized) is unchanged — that determination belongs to `rusty_foundation_akb`, not this repository |
