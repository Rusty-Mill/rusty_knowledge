# Release Notes

No version tags exist yet (pre-1.0, nothing published). One entry per merged PR
against `main`, reverse chronological, each linking to its PR.

---

## PR TBD — Suggest candidate valid-relationship rules
**2026-08-12** · [#TBD](https://github.com/Rusty-Mill/rusty_knowledge/pull/TBD)

- **Added:** `crosscut_valid_relationship_candidates` MCP tool (closes
  [rusty_knowledge#43](https://github.com/Rusty-Mill/rusty_knowledge/issues/43))
  — derives *candidate* declared valid-relationship rules from a domain's
  existing `relationships` instances, grouped by `(from_type, to_type,
  relationship_type)` via each endpoint's `construct_type`. Read-only:
  never writes to `valid_relationships` itself.
- **Has no `knowledge-mcp` equivalent** — the first tool in this crate
  that doesn't. Follows on from the decision on rusty_knowledge#38 (the
  importer leaves `valid_relationships` empty rather than inferring it,
  per `RM-KNOWLEDGE-MODEL-0004`): this tool exists so a human still has a
  way to populate that table afterward, without the crate ever silently
  promoting an inferred candidate into the declared set itself. Turning a
  candidate into a real rule is a separate, explicit
  `insert_valid_relationship` call outside this tool.
- **Added:** `store::candidate_valid_relationships` and
  `ValidRelationshipCandidate` (`rule`, `instance_count`,
  `other_cardinalities_seen`). When multiple instances of the same type
  triple disagree on cardinality, the most common one is chosen and the
  rest are disclosed via `other_cardinalities_seen` rather than silently
  resolved.
- 6 new tests (4 store-level: seeded relationship produces one candidate,
  majority-cardinality selection with disagreement disclosed, distinct
  relationship types produce distinct candidates, a domain with no
  relationships is empty; 2 tool-level: seeded candidate reported,
  no-relationships domain reports none).

## PR #42 — Make the store's database path configurable
**2026-08-12** · [#42](https://github.com/Rusty-Mill/rusty_knowledge/pull/42)

- **Added:** `store::open_store_at_path` (closes
  [rusty_knowledge#41](https://github.com/Rusty-Mill/rusty_knowledge/issues/41))
  — a file-backed alternative to `open_store`'s in-memory default. Selected
  at startup via a new `KNOWLEDGE_DB_PATH` env var; unset means unchanged
  behavior (in-memory, same as before this PR).
- **Decision:** a brand new or still-empty file gets the schema created
  and is seeded/imported exactly like the in-memory default. A file a
  previous run already initialized (its `domains` table already exists)
  is reused as-is — `seed()` and `KNOWLEDGE_MCP_IMPORT_PATH` are both
  skipped on that run. Chosen over re-seeding on top of existing data
  (existing `insert_*` functions use plain `INSERT`, not `INSERT OR
  REPLACE`, so that would fail on ID collisions) and over erroring out
  (would make the ordinary "first run against a path that doesn't exist
  yet" case require an extra opt-in flag for no benefit).
- **Refactored, not changed:** `open_store`'s own behavior and signature
  are unchanged — the `CREATE TABLE` batch and the `sqlite-vec`
  auto-extension registration were factored into shared private helpers
  (`create_schema`, `register_vec_extension`) so both entry points use the
  same schema, not duplicated SQL. All 109 pre-existing tests pass
  unmodified.
- 3 new tests: a fresh/nonexistent path is fresh and seeds normally;
  reopening the same path a second time is not fresh and the first run's
  data is still there (real persistence, not just "didn't error twice");
  a zero-byte file already at the path (simulating `touch`) is still
  treated as fresh.

## PR #39 — Import a knowledge-mcp SQLite database
**2026-08-12** · [#39](https://github.com/Rusty-Mill/rusty_knowledge/pull/39)

- **Added:** `knowledge_mcp_import::import_knowledge_mcp_db` (closes
  [rusty_knowledge#38](https://github.com/Rusty-Mill/rusty_knowledge/issues/38))
  — reads a `knowledge-mcp` (Python) SQLite file and translates its rows
  into this crate's existing `store::insert_*` functions. The two schemas
  aren't on-disk compatible (different column sets, `layer_num` INTEGER vs
  `AuthorityLayer` TEXT, no shared `rules` table design), so this is a
  row-by-row translation, not a raw file open — see the design discussion
  on #38 for the full schema comparison.
- **Added:** an optional `KNOWLEDGE_MCP_IMPORT_PATH` startup env var —
  when set, imports that file on top of the seeded demo data before the
  server starts serving. Omit it and nothing changes from today's
  seed-data-only behavior.
- **New dependency:** `serde_json`, for parsing `machine_rule`'s JSON
  column — added after explicit sign-off (this crate previously had no
  JSON dependency, only `serde` itself).
- **Deliberately not imported:**
  - `knowledge_fts` — confirmed redundant by reading `knowledge-mcp`'s own
    ingestion pipeline: its `"rule"` rows are copies of `rules.rule_text`,
    its `"definition"` rows are copies of `constructs.description`.
    `insert_rule` already rebuilds `rules_fts` the normal way.
  - `valid_relationships` — `knowledge-mcp` has no declared-rule table for
    this at all; its `lookup.valid_relationships` infers validity from
    relationship instances, which is exactly what `RM-KNOWLEDGE-MODEL-0004`
    requires `rusty_knowledge` not to do. Left empty on import, disclosed
    in the returned `ImportReport`.
  - `properties`, `domain_layers`, `ingestion_log`, `schema_version` — no
    `rusty_knowledge` equivalent.
- **Added:** `store::insert_construct_embedding`, for pre-existing vectors
  from an import (distinct from `build_construct_embeddings`, which only
  generates fresh ones via an `Embedder`).
- **`ImportReport`** carries per-table counts plus every dropped column,
  unmapped value, or unimportable row as a human-readable disclosure —
  never a silent partial import. A row that fails to translate at all
  (e.g. an unrecognized `layer_num`) is skipped and disclosed; a row that
  imports but loses a field along the way (e.g. a rule whose
  `machine_rule` didn't parse) still counts as imported, with the drop
  noted separately.
- 8 new tests: happy path across every table; a construct's `short_name`
  falling back to `name` when null; a rule with an unrecognized
  `layer_num`/`rule_type` (skipped, disclosed); a rule whose `machine_rule`
  is unparseable JSON or names an unsupported check kind (`"custom"`) --
  both still import the rule itself, just without a `MachineRule` attached;
  a relationship with a null `rule_type` (defaults to `MAY`, disclosed); a
  conflict with an unrecognized `layer_a`/`layer_b`.

## PR #36 — Wire sqlite-vec vec0 table into search
**2026-08-12** · [#36](https://github.com/Rusty-Mill/rusty_knowledge/pull/36)

- **Added:** hybrid search (closes
  [rusty_knowledge#18](https://github.com/Rusty-Mill/rusty_knowledge/issues/18))
  — `search_knowledge` now fuses FTS5 with `sqlite-vec` cosine-similarity
  search over construct descriptions (RK-004) via Reciprocal Rank Fusion,
  matching `knowledge-mcp`'s `hybrid_search`/`_reciprocal_rank_fusion`
  exactly in shape: same RRF formula and `k=60` constant, embeddings keyed
  by construct (not rule), a lazily-sized `vec0` table detected from the
  first stored vector's byte length rather than a fixed dimension baked
  into the schema, and a silent-but-honest fallback to `RetrievalMode::LexicalOnly`
  on any vector-search error.
- **New dependency:** [`rusty_embedder`](https://github.com/baileyrd/rusty_embedder)
  (pinned to `6add27a`), the `Embedder` trait + `NullEmbedder` this crate
  needed to close #18 — built earlier in this same parity-loop session
  specifically for this gap, after confirming no existing org crate covered
  it. Only `rusty-embedder-core` (zero-dependency: the trait, `NullEmbedder`,
  and `serialize_f32`/`deserialize_f32`) is a mandatory dependency; the real
  backends are opt-in Cargo features (`local-embeddings` via `fastembed-rs`,
  no network at runtime after model download; `http-embeddings`, any
  OpenAI-compatible endpoint via `reqwest`).
- **Default behavior is unchanged:** the server still runs `NullEmbedder`
  (dimension 0) unless both a real-backend feature is compiled in *and*
  `EMBEDDING_BACKEND` (`local` / `http` / `openai`) selects it at startup --
  matching `knowledge-mcp`'s own `EMBEDDING_BACKEND` env var and its
  `NullEmbedder`-by-default posture. `cargo build`/`cargo test` with no
  extra features stays exactly as fast and dependency-light as before;
  every existing test keeps passing unmodified.
- **Changed:** `store::RetrievalMode` gained a `Hybrid` variant. The
  placeholder `rule_vectors` vec0 table (declared in schema since an early
  slice, never populated, fixed at a meaningless `float[4]`) is replaced by
  a real `construct_embeddings` table plus a `vec_constructs` vec0 table
  built on demand.
- 11 new tests (9 store-level: embedding storage/no-op with `NullEmbedder`,
  vec0 index sync and dimension detection, KNN vector search and its domain
  filter, hybrid fusion producing both a lexical+vector hit and a
  vector-only hit, lexical-only fallback with `NullEmbedder`; 2 tool-level:
  hybrid-mode search response formatting, semantic-only-match annotation).
  All pre-existing tests continue to pass unmodified against the
  `NullEmbedder` default (101 total, up from 90).

## PR #34 — Implement meta.list_domains
**2026-08-12** · [#34](https://github.com/Rusty-Mill/rusty_knowledge/pull/34)

- **Added:** `meta_list_domains` MCP tool (closes
  [rusty_knowledge#16](https://github.com/Rusty-Mill/rusty_knowledge/issues/16))
  — lists all loaded domains. This is the last of `knowledge-mcp`'s 15
  tools; the full surface is now implemented.
- **Matches `knowledge-mcp`'s actual behavior, not its tool description:**
  the description claims "layer counts and coverage summary," but
  `knowledge-mcp`'s own implementation just returns bare domain rows (id,
  name, ...) -- the counts live in a separate tool
  (`lookup.domain_summary`, our `lookup_domain_summary`, rusty_knowledge#8)
  instead. This crate's version does the same: per-domain layer/construct
  counts stay in `lookup_domain_summary`, not duplicated here.
- **Added:** `store::list_domains`, returning all domains ordered by name.
- 2 new tests (1 store-level: both seeded domains in name order; 1
  tool-level: both domains listed by `meta_list_domains`). The existing
  routing-guide test is updated to assert the now-complete 15-tool surface
  instead of checking for not-yet-implemented tools.

## PR #33 — Implement crosscut.cross_domain
**2026-08-12** · [#33](https://github.com/Rusty-Mill/rusty_knowledge/pull/33)

- **Added:** `crosscut_cross_domain` MCP tool (closes
  [rusty_knowledge#15](https://github.com/Rusty-Mill/rusty_knowledge/issues/15))
  — finds typed relationships from a construct to constructs in *other*
  domains (e.g. a UAF capability tracing to an RMF control family),
  optionally narrowed to one target domain.
- **Added:** `CrossDomainRelationship` struct, `cross_domain_relationships`
  table, `insert_cross_domain_relationship`, and
  `cross_domain_relationships_from` in `store.rs`. Distinct from
  `Relationship`, which only ever connects two constructs in the *same*
  domain.
- **Not modeled:** the target construct is never resolved against a live
  `constructs` row, matching `knowledge-mcp`'s own behavior — the target
  domain (an external framework) may not be loaded at all.
- **Seed data:** one real cross-domain relationship linking the existing
  `uaf-1.3:AuthorityGrant` and `data-mesh:DataProduct` seed constructs
  (`governs`), reusing the two domains already present rather than adding
  new seed constructs just for this table.
- 7 new tests (3 store-level: seeded relationship, `to_domain_id` filter,
  a construct with none; 4 tool-level: seeded relationship reported,
  `to_domain_id` filter, construct with none, unknown construct).

## PR #32 — Implement crosscut.conflicts
**2026-08-12** · [#32](https://github.com/Rusty-Mill/rusty_knowledge/pull/32)

- **Added:** `crosscut_conflicts` MCP tool (closes
  [rusty_knowledge#14](https://github.com/Rusty-Mill/rusty_knowledge/issues/14),
  the layered-authority conflict registry, RK-002) — lists conflict-registry
  entries for a domain, optionally narrowed to one construct. Matching
  `knowledge-mcp`'s `get_conflicts`, a construct-scoped query returns both
  that construct's own conflicts *and* the domain's construct-independent
  ones (a domain-level conflict applies no matter which construct you asked
  about).
- **Deliberately different from `knowledge-mcp`:** an unresolvable
  `construct_ref` is a hard error here, not a silently dropped filter that
  falls back to the whole domain — the same divergence already made for
  `lookup_relationships`' `to_construct_ref` (rusty_knowledge#6), for the
  same reason: silently widening the result set on a typo'd reference is
  more surprising than failing loudly.
- **Added:** `Conflict` struct, `conflicts` table, `insert_conflict`, and
  `conflicts_for` in `store.rs`. `layer_a`/`layer_b` are `AuthorityLayer`
  (matching how `Rule`/`Relationship` already model layers), not raw
  integers as in `knowledge-mcp`'s `layer_num`.
- **Seed data:** one real conflict entry, documenting the exact
  Standard-vs-Conventions contradiction the two existing seeded
  `AuthorityGrant` rules already imply (expiry required vs. often omitted).
- 8 new tests (3 store-level: seeded conflict, construct + domain-level
  scoping, a domain with none; 5 tool-level: seeded conflict reported,
  construct + domain-level listing, construct-scoped filter, no conflicts,
  unknown construct).

## PR #31 — Implement crosscut.traceability
**2026-08-12** · [#31](https://github.com/Rusty-Mill/rusty_knowledge/pull/31)

- **Added:** `crosscut_traceability` MCP tool (closes
  [rusty_knowledge#13](https://github.com/Rusty-Mill/rusty_knowledge/issues/13)) —
  given a construct, reports what it must/should trace to (`traces_to`
  relationships outgoing) and what must/should trace to it (`traces_to`
  relationships incoming). MUST/SHALL-typed traces only by default;
  `include_optional` widens to SHOULD/MAY as well, matching `knowledge-mcp`.
  Traceability is always evaluated at the Standard authority layer, matching
  `knowledge-mcp`'s hardcoded `layer_num=1`.
- **Added:** `store::relationships_to` — mirrors `relationships_from`, keyed
  by the target construct instead of the source, needed for the
  "traced from" side. Both now share a `layer` filter parameter and a
  factored-out `relationship_from_row` mapper.
- 6 new tests (2 store-level for `relationships_from`/`relationships_to`'s
  new layer filter and the "to" direction, 4 tool-level: outgoing + incoming
  MUST traces, SHOULD traces excluded/included via `include_optional`, no
  requirements, unknown construct).

## PR #30 — Implement search.constructs
**2026-08-12** · [#30](https://github.com/Rusty-Mill/rusty_knowledge/pull/30)

- **Added:** `search_constructs` MCP tool (closes
  [rusty_knowledge#12](https://github.com/Rusty-Mill/rusty_knowledge/issues/12)) —
  lists constructs in a domain, optionally narrowed to one `construct_type`.
- **Changed:** `store::constructs_in_domain` gained an optional
  `construct_type` filter parameter; existing call sites pass `None`
  (unchanged behavior).
- **Not modeled:** `knowledge-mcp`'s `search.constructs` also filters by
  `layer_num`, but this crate's `Construct` doesn't carry an authority
  layer (only `Rule` does) — a construct itself isn't layered, only the
  rules attached to it are.
- 4 new tests (1 store-level filter test, 3 tool-level: list all, filter by
  type, unknown domain reports none found).

## PR #29 — Implement validate.completeness
**2026-08-12** · [#29](https://github.com/Rusty-Mill/rusty_knowledge/pull/29)

- **Added:** `validate_completeness` MCP tool (closes
  [rusty_knowledge#11](https://github.com/Rusty-Mill/rusty_knowledge/issues/11)) —
  given a container/viewpoint construct and the element types present in a
  model, reports required/present/missing/extra element types plus the
  construct's required (MUST/SHALL) and recommended (SHOULD) rule texts, and
  an overall complete/incomplete verdict.
- **Added:** `rule_type` field on `Relationship` (mirrors `Rule`'s) — a
  `MUST`-typed relationship is what "required child element type" means here,
  matching `knowledge-mcp`'s own `evaluate_completeness`, which filters its
  relationship store the same way `validate.relationship` does. `relationships_from`
  gained a matching optional `rule_type` filter parameter (existing call
  sites pass `None`, unchanged behavior). Seed data's existing relationship
  now carries `rule_type: RuleType::Must`.
- **Added:** `CompletenessReport` and `evaluate_completeness` in `store.rs`.
- 8 new tests (4 tool-level: complete when required present, missing required
  reported, extra-present doesn't block completeness, unknown construct; 4
  store-level: same coverage plus a construct with no required relationships
  at all, and the new `relationships_from` rule_type filter).

## PR #28 — Implement validate.relationship
**2026-08-12** · [#28](https://github.com/Rusty-Mill/rusty_knowledge/pull/28)

- **Added:** `validate_relationship` MCP tool (closes
  [rusty_knowledge#10](https://github.com/Rusty-Mill/rusty_knowledge/issues/10)) —
  VALID if at least one recorded relationship matches the given source
  construct, target construct, and relationship type; INVALID otherwise.
- **No new store schema** — `knowledge-mcp`'s `validate.relationship` calls
  the exact same store query as its `lookup.relationships` (both keyed by
  specific construct instances, not types), so this reuses
  `relationships_from` and `resolve_construct` from #6 as-is.
- 4 new tests (recorded relationship is valid, unrecorded type is invalid,
  wrong direction is invalid, unknown construct reports not found).

## PR #27 — Implement validate.element
**2026-08-12** · [#27](https://github.com/Rusty-Mill/rusty_knowledge/pull/27)

- **Added:** `validate_element` MCP tool (closes
  [rusty_knowledge#9](https://github.com/Rusty-Mill/rusty_knowledge/issues/9)) —
  validates a caller-supplied element's properties against a construct's
  machine-checkable rules, returning PASS/FAIL/WARNING per rule plus an
  overall result. This is the rule-evaluation engine `knowledge-mcp`'s
  `_evaluate_machine_rule` provides — the biggest real gap in this list.
- **Added:** `MachineRule` (a structured, machine-checkable rule attached to
  a `Rule` row — required-property, enum-value, pattern, and range checks,
  matching `knowledge-mcp`'s `machine_rule` schema), `ValidationOutcome`, and
  `evaluate_machine_rule`. A new `rule_machine_checks` side table keyed by
  `rules_fts`'s `rowid` stores them, since most rules are free text only and
  don't need one. `insert_rule` now returns that `rowid` so a caller can
  attach a check afterward.
- **New dependency, added only after explicit sign-off:** `Pattern` (regex)
  checks are evaluated via [`rusty_regx`](https://github.com/baileyrd/rusty_regx),
  a zero-runtime-dependency POSIX-ERE engine from this same GitHub account —
  deliberately chosen over the `regex` crate to avoid its several transitive
  dependencies. `find`'s unanchored match is constrained to `start() == 0` to
  replicate Python's `re.match` (anchored-at-start) semantics; an invalid
  pattern or a mismatch reports `WARNING`, matching `_evaluate_machine_rule`'s
  own behavior — never a silent PASS or a panic. Pinned to a commit SHA (no
  tags exist upstream), same reproducibility standard as everything else this
  repo depends on.
- **Deliberately not included:** the separate "required-property schema per
  construct type" completeness check `knowledge-mcp` also runs in
  `validate.element` (distinct from per-rule machine checks) isn't modeled —
  it's a different concept (a declared property schema) that would double
  this issue's scope. Also skipped: `known_conflicts` (needs #14).
- Seed data: two existing rules now carry machine checks —
  `AuthorityGrant`'s "MUST declare an explicit scope and expiry"
  (`RequiredProperty { property: "scope" }`) and `DataProduct`'s "MUST
  declare an owning domain team" (`Pattern` matching a team-slug format) —
  so there's something real to validate against for each check kind. No new
  rule rows added, so existing rule-count assertions elsewhere are untouched.
- 14 new tests (7 tool-level: fail/pass on the required-property check,
  pass/warning on the pattern check, no-machine-checks construct, unknown
  construct, layer filter excluding the check; 7 store-level: joined query
  returns the seeded check, and `evaluate_machine_rule` for
  required-property/enum/range/pattern-match/pattern-mismatch/invalid-pattern).

## PR #26 — Implement lookup.domain_summary
**2026-08-12** · [#26](https://github.com/Rusty-Mill/rusty_knowledge/pull/26)

- **Added:** `lookup_domain_summary` MCP tool (closes
  [rusty_knowledge#8](https://github.com/Rusty-Mill/rusty_knowledge/issues/8)) —
  domain name, authority layers present, and construct counts (total and by
  type).
- **Added:** `domain_by_id` and `layers_present_in_domain` queries in
  `store.rs`. Layers-present is derived from `rules_fts` rather than tracked
  as separate state, since it's fully determined by what rules already exist.
- **Deliberately not included:** `conflict_count` — `knowledge-mcp`'s summary
  includes a count from its conflict registry, which doesn't exist in this
  crate yet (lands with
  [rusty_knowledge#14](https://github.com/Rusty-Mill/rusty_knowledge/issues/14)).
- 6 new tests (layers + counts for a populated domain, no cross-domain count
  leakage, unknown domain, plus 2 new store-level tests).

## PR #25 — Implement lookup.valid_relationships
**2026-08-12** · [#25](https://github.com/Rusty-Mill/rusty_knowledge/pull/25)

- **Added:** `lookup_valid_relationships` MCP tool (closes
  [rusty_knowledge#7](https://github.com/Rusty-Mill/rusty_knowledge/issues/7)) —
  given two construct types, all valid relationship types between them.
- **Added:** `ValidRelationshipRule` entity and a new `valid_relationships`
  table — a *declared* rule about which relationship types are valid between
  two construct *types*, distinct from `Relationship` (an actual link between
  two construct *instances*). `RM-KNOWLEDGE-MODEL-0004` requires validation
  to check against this declared set rather than inferring validity from
  whatever relationship instances happen to already exist, so this couldn't
  just reuse the `relationships` table from #6. Seed data gets one declared
  rule matching the relationship instance already seeded in #6.
- 4 new tests (seeded rule returned at both the store and tool layer, unknown
  type-pair reports none at both layers).

## PR #24 — Implement lookup.relationships
**2026-08-12** · [#24](https://github.com/Rusty-Mill/rusty_knowledge/pull/24)

- **Added:** `lookup_relationships` MCP tool (closes
  [rusty_knowledge#6](https://github.com/Rusty-Mill/rusty_knowledge/issues/6)) —
  relationships from a construct, with cardinality and layer provenance,
  optionally narrowed to a target construct and/or relationship type.
- **Added:** `relationships_from` query in `store.rs`; `insert_relationship`
  and `Relationship` are no longer `#[allow(dead_code)]` now that a real tool
  consumes them. Seed data gains one real relationship
  (`AuthorityGrant --records--> ConflictRegistryEntry`) instead of only the
  test-only ad hoc row from the previous PR.
- **Deliberately different from `knowledge-mcp`:** an unresolvable
  `to_construct_ref` is a tool error here, not a silently dropped filter —
  `knowledge-mcp`'s Python `lookup.relationships` falls back to unfiltered
  results if the target ref doesn't resolve, which reads as a bug worth not
  replicating rather than a behavior to preserve for parity's own sake.
- 5 new tool-level tests plus 3 new store-level tests (seeded relationship
  returned, `to_construct_ref`/`relationship_type` filtering, empty result
  for a construct with none, unresolvable-ref error path, unknown
  `from_construct_ref` error path).

## PR #23 — Implement lookup.rules
**2026-08-12** · [#23](https://github.com/Rusty-Mill/rusty_knowledge/pull/23)

- **Added:** `lookup_rules` MCP tool (closes
  [rusty_knowledge#5](https://github.com/Rusty-Mill/rusty_knowledge/issues/5)) —
  rules for a construct, filterable by authority layer and/or rule type
  (MUST/SHALL/SHOULD/MAY/MUST_NOT).
- **Added:** `RuleType` enum (mirrors `AuthorityLayer`'s trusted-storage
  `from_str` / untrusted-input `parse` split) and a `rule_type` field on
  `Rule`; `rules_fts` gained a `rule_type` UNINDEXED column. Seed data now
  assigns a rule type to each of the 4 existing rules based on their actual
  text, not arbitrarily.
- **Refactored:** the layer-filter-parsing block was duplicated three times
  across `search_knowledge`, `lookup_construct`, and now `lookup_rules` —
  factored into shared `parse_layer_filter`/`parse_rule_type_filter`
  functions rather than copy-pasting a fourth time. No behavior change.
- 4 new tests (all rules for a construct, combined layer+rule_type filter,
  unknown rule_type error path, unknown construct).

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
