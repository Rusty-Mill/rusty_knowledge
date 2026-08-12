# Release Notes

No version tags exist yet (pre-1.0, nothing published). One entry per merged PR
against `main`, reverse chronological, each linking to its PR.

---

## PR #1 — Apply repo-config governance file set
**2026-08-12** · [#1](https://github.com/Rusty-Mill/rusty_knowledge/pull/1)

- **Added:** CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, CHANGELOG, RELEASE_NOTES,
  ARCHITECTURE, and an ADR log seed, via the `repo-config` skill. ARCHITECTURE's
  overview/boundaries/structure/data-flow sections were hand-written from the
  actual `main.rs`/`store.rs` vertical slice rather than left as scaffold.
- **Added:** `.github/PULL_REQUEST_TEMPLATE/` (feature, bug_fix, docs, chore),
  `.github/ISSUE_TEMPLATE/` (bug_report, feature_request, config.yml), and
  `.github/workflows/ci-rust.yml` (fmt --check, clippy -D warnings, cargo test).
- **Fixed:** the two items above were initially missing — this session's locally
  synced copy of the `repo-config` skill was missing its `.github/` template
  directory entirely (and had lost the executable bit on `apply.sh`/`audit.sh`).
  Root-caused against the skill's actual source at `github.com/baileyrd/skill_pack`,
  which was intact — the gap was in this environment's skill sync, not the skill's
  content. Fixed the local sync and re-applied; audit score is now 10/10.
- **Fixed:** `cargo fmt --all` on `src/main.rs`/`src/store.rs` — purely mechanical
  line-wrapping, no logic changes — required to make the newly-added CI workflow's
  `fmt --check` step pass; verified `fmt --check`, `clippy -D warnings`, and
  `cargo test` all pass locally before pushing.
