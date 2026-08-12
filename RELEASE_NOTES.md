# Release Notes

No version tags exist yet (pre-1.0, nothing published). One entry per merged PR
against `main`, reverse chronological, each linking to its PR.

---

## PR #22 — Implement lookup.construct
**2026-08-12** · [#22](https://github.com/Rusty-Mill/rusty_knowledge/pull/22)

- **Added:** `lookup_construct` MCP tool (closes
  [rusty_knowledge#4](https://github.com/Rusty-Mill/rusty_knowledge/issues/4)) —
  full construct definition (short name, ID, type, description) plus its
  rules, optionally filtered by authority layer. Resolves `construct_ref` by
  short name first, falling back to a direct ID match within the domain,
  matching `knowledge-mcp`'s `_resolve` order.
- **Added:** `description` field on `Construct`; `resolve_construct` and
  `rules_for_construct` queries in `store.rs`.
- **Deliberately not included:** `is_abstract`/`is_deprecated`/`parent_id`/
  `metadata`, conflict-registry counts, and "properties" — `knowledge-mcp`'s
  fuller `Construct` model and its conflict/properties concepts aren't
  modeled in this crate yet (conflicts land with
  [rusty_knowledge#14](https://github.com/Rusty-Mill/rusty_knowledge/issues/14);
  properties/metadata aren't filed anywhere yet). Not fabricated to look
  more complete than it is.
- 6 new tests (short-name resolution, ID resolution, layer filter, cross-domain
  isolation, unknown-ref handling, unknown-layer error path).

## PR #21 — search_knowledge: domain/layer filtering, rank, retrieval-mode
**2026-08-12** · [#21](https://github.com/Rusty-Mill/rusty_knowledge/pull/21)

- **Changed (breaking, explicitly signed off):** `search_knowledge`'s response
  now always declares its retrieval mode (`lexical-only` until
  [rusty_knowledge#18](https://github.com/Rusty-Mill/rusty_knowledge/issues/18)
  wires vector retrieval in, per `RM-KNOWLEDGE-MODEL-0005`), includes each
  hit's FTS5 rank, and accepts optional `domain_id`/`layer` filter params
  (closes [rusty_knowledge#3](https://github.com/Rusty-Mill/rusty_knowledge/issues/3)).
  Confirmed low-risk before implementing: no external consumer of this tool
  exists yet besides this crate itself.
- **Added:** `store::search_scoped`, `store::RetrievalMode`, `store::SearchHit`,
  and `AuthorityLayer::parse` (a fallible parser for untrusted caller input,
  separate from the existing panic-on-corruption `from_str` used for trusted
  storage reads).
- **Removed:** `store::search` — superseded by `search_scoped` called with no
  filters; kept no dead code around once nothing needed it.
- An unrecognized `layer` value is reported as a tool error, not silently
  ignored or defaulted.
- 7 new/updated tests (mode declaration, domain filter, layer filter,
  unknown-layer error path); all pass.

## PR #20 — Implement meta.routing_guide
**2026-08-12** · [#20](https://github.com/Rusty-Mill/rusty_knowledge/pull/20)

- **Added:** `meta_routing_guide` MCP tool (closes
  [rusty_knowledge#17](https://github.com/Rusty-Mill/rusty_knowledge/issues/17)),
  matching `knowledge-mcp`'s `meta.routing_guide` in shape. Deliberately
  limited to tools that actually exist in this crate today — just
  `search_knowledge` — rather than advertising routing guidance for
  `lookup.*`/`validate.*`/`crosscut.*` tools that don't exist yet and would
  fail if called. Grows as those tools land (rusty_knowledge#4-#16).
- 1 new test asserting the guide only references tools that actually exist.

## PR #19 — Domain/construct/relationship data model
**2026-08-12** · [#19](https://github.com/Rusty-Mill/rusty_knowledge/pull/19)

- **Added:** `Domain`, `Construct`, and `Relationship` entities in `store.rs`,
  alongside the existing `Rule`/`AuthorityLayer` types — `rules_fts` now carries
  `domain_id`/`construct_id` columns (UNINDEXED, so exact-match filtering stays
  possible without joining them into the full-text index). Prerequisite for
  nearly every other parity-gap tool against `knowledge-mcp` (closes
  [rusty_knowledge#2](https://github.com/Rusty-Mill/rusty_knowledge/issues/2)).
- **Added:** `constructs_in_domain` query and a cross-domain-leakage test,
  proving `RM-KNOWLEDGE-MODEL-0001` (no domain leakage) at the store level —
  seed data now spans two domains (`uaf-1.3`, `data-mesh`) instead of one
  flat, domain-less table.
- **Unchanged, verified:** `search_knowledge`'s public tool contract (params,
  response format) — this slice is additive only; the existing seeded rows'
  text is untouched, so any query that matched before still matches the same
  way. The richer domain/layer-filtered, ranked, hybrid-mode-declaring version
  RM-KNOWLEDGE-MODEL-0005 requires is a separate, breaking-change-flagged issue
  ([rusty_knowledge#3](https://github.com/Rusty-Mill/rusty_knowledge/issues/3)),
  not bundled into this one.
- **Added:** `gap-analysis.md` — the full 17-row parity assessment against
  `knowledge-mcp`'s 15 MCP tools, produced by the `parity-loop` skill, plus a
  survey of the wider `rusty_*` ecosystem for reusable crates (`rusty_search`
  and `rusty_sqlite` don't yet cover what this repo needs — filed as
  [`rusty_search#14`](https://github.com/baileyrd/rusty_search/issues/14) and
  [`rusty_sqlite#1`](https://github.com/baileyrd/rusty_sqlite/issues/1)).
- 4 new unit tests (domain isolation, unknown-domain empty result, search
  still matches across domains, relationship round-trip); all pass, plus the
  existing suite.

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
