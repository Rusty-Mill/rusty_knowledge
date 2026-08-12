# Release Notes

No version tags exist yet (pre-1.0, nothing published). One entry per merged PR
against `main`, reverse chronological, each linking to its PR.

---

## PR TBD — Apply repo-config governance file set
**2026-08-12**

- **Added:** CONTRIBUTING, CODE_OF_CONDUCT, SECURITY, CHANGELOG, RELEASE_NOTES,
  ARCHITECTURE, and an ADR log seed, via the `repo-config` skill. ARCHITECTURE's
  overview/boundaries/structure/data-flow sections were hand-written from the
  actual `main.rs`/`store.rs` vertical slice rather than left as scaffold.
- **Known limitation, disclosed rather than silently worked around:** the installed
  `repo-config` skill's template payload is missing its entire `.github/` folder
  (PR templates, issue templates, and the stack-selected CI workflow) — those
  couldn't be generated from the skill's assets as designed. Flagged to the repo
  owner as a skill-packaging gap; not fabricated from scratch to avoid diverging
  from the skill's intended content.
- This entry's own PR link will be filled in once the PR is opened.
