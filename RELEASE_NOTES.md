# Release Notes

No version tags exist yet (pre-1.0, nothing published). One entry per merged PR
against `main`, reverse chronological, each linking to its PR.

---

## PR TBD — Live-verify OllamaEmbedder against a real Ollama server
**2026-08-14** · PR TBD

- **Verified, not just implemented:** installed a real Ollama server in
  this development environment, pulled `all-minilm`, and confirmed
  `store::OllamaEmbedder` end-to-end — both directly (a new
  `ollama_embedder_live_semantic_similarity` test) and through the actual
  MCP binary's `search_knowledge` tool over stdio, which returned real
  results correctly labeled `"Ollama local embedding -- a real trained
  semantic model"`.
- New `ollama_embedder_live_semantic_similarity` test (`#[ignore]`d by
  default, since CI and most dev environments don't have Ollama running):
  embeds a sentence, a paraphrase, and an unrelated sentence, and asserts
  the paraphrase scores a higher cosine similarity than the unrelated one
  — a property `HashingEmbedder` structurally cannot have, and the actual
  reason this backend exists. Opt in with `OLLAMA_EMBEDDING_MODEL=all-minilm
  cargo test --all-features -- --ignored ollama_embedder_live`.
- Doc comments (`OllamaEmbedder`'s own, and `ARCHITECTURE.md`'s former
  "live-unverified" non-goal) updated to reflect that this has now
  actually been proven to work, while staying honest that CI itself still
  doesn't automate this check — the ignored test is a repeatable manual
  verification, not a standing per-commit guarantee.

## PR #68 — Replace OnyxEmbedder with OllamaEmbedder (no API key needed)
**2026-08-14** · [#68](https://github.com/Rusty-Mill/rusty_knowledge/pull/68)

- **Corrected:** PR #67's `store::OnyxEmbedder` targeted Onyx's *cloud*
  embeddings API (`ai.onyx.dev`), which turned out to require
  authentication (a Bearer token, or `x-onyx-key`/`x-onyx-secret`) with no
  unauthenticated tier — exactly the credential this environment doesn't
  have. Replaced with `store::OllamaEmbedder`, calling a local (or
  otherwise self-hosted) Ollama server's `POST /api/embed` instead. A
  local Ollama server has nothing to authenticate to by default, so no API
  key is needed at all.
- `EMBEDDING_BACKEND=ollama` replaces `EMBEDDING_BACKEND=onyx`;
  `OLLAMA_EMBEDDING_MODEL` (required — Ollama has no documented default
  embedding model) and `OLLAMA_API_BASE_URL` (optional, defaults to
  `http://localhost:11434`) replace `ONYX_API_KEY`/`ONYX_EMBEDDING_MODEL`/
  `ONYX_API_BASE_URL`. `HashingEmbedder` remains the unchanged default.
- Request/response shape changed to match Ollama's actual documented
  contract: `{"model", "input"}` → `{"embeddings": [[...]]}` (a batched
  shape, one vector array per input, even for a single string), rather
  than Onyx's `{"model", "prompt"}` → `{"embedding": [...]}`.
- **Still honestly disclosed:** this environment has no Ollama
  installation either, so `OllamaEmbedder` remains live-unverified —
  tested only against a local mock HTTP server exercising the documented
  request/response shape. Swapping which real backend is unverified
  doesn't change that status; it's tracked the same way `OnyxEmbedder`'s
  was.

## PR #67 — Real (Onyx) semantic embedder backend for search_knowledge
**2026-08-14** · [#67](https://github.com/Rusty-Mill/rusty_knowledge/pull/67)

- **Added:** `store::OnyxEmbedder`, a second, real `Embedder` implementation
  calling Onyx's cloud embeddings API (`POST /api/embeddings`, an
  Ollama-compatible shape), closing the one remaining gap noted in PR #65
  and tracked as [#66](https://github.com/Rusty-Mill/rusty_knowledge/issues/66)
  — a real semantic model has no network access rationale to stay
  deferred once network access is confirmed available. Opt-in via
  `EMBEDDING_BACKEND=onyx`; unset, or any other value, keeps the existing
  `HashingEmbedder` default unchanged.
- New `store::active_embedder()`: a `OnceLock`-cached, env-var-gated
  selector run once per process. A misconfigured `onyx` backend (missing
  `ONYX_API_KEY`/`ONYX_EMBEDDING_MODEL`, or any other `OnyxEmbedder::from_env`
  failure) fails loudly to stderr and falls back to `HashingEmbedder` rather
  than silently doing nothing or panicking. `store::RETRIEVAL_MODE_DESCRIPTION`
  (a const) is replaced with `store::retrieval_mode_description()` (a
  function), since the description must now reflect whichever backend is
  actually active rather than a single hardcoded string.
- New `rusty_request` git dependency (`baileyrd/rusty_request`, `tokio`
  feature) for the HTTP transport, chosen over `reqwest`/`ureq` to match
  this ecosystem's own sovereign-HTTP-client convention. `Embedder::embed`
  stays synchronous (dozens of existing sync call sites); `OnyxEmbedder`
  bridges into `rusty_request`'s async API via `tokio::task::block_in_place`
  + `Handle::current().block_on(...)`, safe because it's only ever reached
  from within `main()`'s multi-threaded tokio runtime.
- **Honestly disclosed, not fabricated:** no Onyx API key is available in
  this environment, so `OnyxEmbedder` has never been exercised against a
  live endpoint. Its tests (request/response shape, vector normalization,
  malformed-response fallback) run against a small hand-rolled local mock
  HTTP server instead. This is called out in the type's own doc comment,
  in `ARCHITECTURE.md`, and here: implemented and CI-gate-clean, but
  live-unverified until real credentials are supplied.
- New config: `EMBEDDING_BACKEND`, `ONYX_API_KEY`, `ONYX_EMBEDDING_MODEL`,
  `ONYX_API_BASE_URL` (optional, defaults to `https://ai.onyx.dev`).

## PR #65 — Vector/hybrid search for search_knowledge
**2026-08-14** · [#65](https://github.com/Rusty-Mill/rusty_knowledge/pull/65)

- **Added:** `search_knowledge` now fuses lexical (FTS5 `bm25`, min-max
  normalized to `[0, 1]`) and vector (cosine similarity) signals in equal
  weight, instead of lexical-only. A hit found by only one signal is
  still ranked -- the other contributes 0, not exclusion -- so a
  near-duplicate phrasing FTS5's exact tokenizer misses can still surface.
  `score`'s semantics flip accordingly: now `[0, 1]`, higher is more
  relevant (previously raw `bm25`, lower was more relevant).
- New `store::Embedder` trait + `store::HashingEmbedder`: a real,
  zero-dependency, zero-network "hashing trick" bag-of-words vector --
  **deliberately not a trained semantic embedding**, since this crate has
  no network access and bundles no model weights. Disclosed honestly as
  syntactic (token-overlap-driven) rather than semantic, both in code
  comments and in the tool's own output
  (`store::RETRIEVAL_MODE_DESCRIPTION`, the single source of truth for
  how `search_knowledge` describes its own retrieval mode). A pluggable
  trait, not a hardcoded function, so a real embedder is a drop-in swap
  if this crate ever gets network/model access.
- New `search_vectors` table, kept in sync incrementally by
  `index_for_search` (same write path as the FTS5 index) -- every
  existing seed/import path gets vectors for free. A similarity floor
  (`MIN_VECTOR_SIMILARITY`) filters out spurious hash-bucket-collision
  noise on unrelated queries, and `EMBEDDING_DIM = 256` keeps genuine
  single-token accidental collisions rare at this dataset's scale.
- This closes the last item from `ARCHITECTURE.md`'s non-goals list that
  was about *capability* rather than *implementation count* -- a trained/
  semantic embedder remains the one deliberately deferred piece, since
  building one is out of this crate's reach in this environment, not out
  of scope on principle.

## PR #64 — Introduce a Store trait / port-adapter abstraction
**2026-08-14** · [#64](https://github.com/Rusty-Mill/rusty_knowledge/pull/64)

- **Added:** `store::Store`, a trait covering exactly the read-only query
  surface `KnowledgeServer`'s 16 MCP tools need (`resolve_subject`,
  `rules_for_subject`, `search_knowledge`, etc.). `KnowledgeServer` now
  holds `Arc<Mutex<dyn Store + Send>>` instead of a raw
  `Arc<Mutex<Connection>>`. `store::SqliteStore` -- a thin `Connection`
  newtype whose trait methods delegate to `store.rs`'s existing free
  functions -- is the only implementation.
- **Deliberately scoped, not exhaustive:** writes (`insert_*`) and
  bootstrap (`seed_udra`, `open_store`/`open_store_at`, `is_empty`) stay
  *outside* the trait -- they run once at startup, before a `Store` is
  even constructed, and continue operating on the raw `Connection`
  exactly as before. `knowledge_mcp_import_v2` also stays on the raw
  `Connection` rather than the trait, since one of its paths does a raw
  `dest.execute` that doesn't map onto a structured port at all.
  Every free function in `store.rs` is unchanged; every existing test
  keeps calling them directly. This is a dependency-inversion layer
  `KnowledgeServer` sits behind, not a rewrite.
- README/ARCHITECTURE.md updated: `ARCHITECTURE.md`'s Boundaries section
  now documents the real port/adapter split; a second `Store`
  implementation is the one remaining non-goal.

## PR #63 — Implement RuleDerivation (firewalled, non-authoritative rollups)
**2026-08-14** · [#63](https://github.com/Rusty-Mill/rusty_knowledge/pull/63)

- **Added:** `RuleDerivation` -- the last piece of the fuller seven-table
  design this model was built from, and a new `lookup_derived_summary`
  tool exposing it. A `RuleDerivation` is a synthesized rollup over a set
  of Rules about one Subject (e.g. "the combined effective guidance"),
  **firewalled from authority by construction**: it's never returned by
  `rules_for_subject` or any other Rule-returning query, never indexed
  for `search_knowledge`, never a `RuleRelation` participant, and every
  `lookup_derived_summary` response is explicitly labeled
  NON-AUTHORITATIVE and lists exactly which Rules it was synthesized
  from, so a reader can go verify against ground truth rather than
  citing the rollup itself.
- New `store.rs` schema: `rule_derivations`/`rule_derivation_sources`
  tables, `insert_rule_derivation` (inserts the derivation and its
  source-rule links in one call), `rule_derivations_for_subject`.
- `seed_udra` gained one illustrative derivation on `udra.DataProduct`,
  rolling up the three separate ownership/registration rules spread
  across the authority chain (data mesh principle -> Army UDRA -> org
  implementation) into one orientation summary.
- README/ARCHITECTURE.md/module doc comments updated: the fuller
  seven-table design is now fully implemented -- nothing from it is
  deferred anymore. Only vector/hybrid search and a `Store` trait
  abstraction remain as deliberate non-goals.

## PR #62 — File-backed persistence (KNOWLEDGE_DB_PATH)
**2026-08-14** · [#62](https://github.com/Rusty-Mill/rusty_knowledge/pull/62)

- **Added:** `KNOWLEDGE_DB_PATH` env var -- when set, the server opens
  (or creates) a SQLite file at that path instead of the default
  in-memory store, so data survives process restarts. Unset behavior is
  unchanged: fresh in-memory store every run.
- New `store.rs` functions: `open_store_at(path)` (schema DDL is now
  entirely `IF NOT EXISTS`, so reopening an existing file is safe and
  doesn't touch data already there) and `is_empty(conn)`. Seed/import
  only ever runs against an empty store -- reopening a file that already
  has data from a previous run leaves it alone instead of re-seeding into
  primary-key conflicts on `seed_udra`'s fixed illustrative ids.
- README/ARCHITECTURE.md updated; this closes one of the two
  previous-model capabilities `ARCHITECTURE.md`'s non-goals listed as
  "not carried forward yet" (the other, vector/hybrid search, remains a
  deliberate non-goal).

## PR #61 — Multi-parent-authority DAG stress test
**2026-08-14** · [#61](https://github.com/Rusty-Mill/rusty_knowledge/pull/61)

- **Added:** three `store.rs` tests exercising a deeper multi-parent
  `SourceAuthority` DAG than the existing single-extra-edge case: two
  entirely independent, two-level-deep root lineages (no shared ancestor
  at all) converging only at a shared descendant. Verifies `ancestors_of`
  correctly walks the full transitive closure across both lineages,
  verifies the two roots/mids don't spuriously see each other as
  ancestors, and verifies `rules_for_subject`/`conflict_candidates_for_subject`
  surface and flag disagreeing rules from the two unrelated lineages about
  a shared Subject -- the real-world shape this exists for (e.g. two
  independent standards both making claims about the same system
  boundary). Test-only; no production code changed.

## PR #60 — search_knowledge: lexical FTS5 search (rusty_knowledge#55)
**2026-08-14** · [#60](https://github.com/Rusty-Mill/rusty_knowledge/pull/60)

- **Added:** `search_knowledge` -- lexical (FTS5) keyword search over
  every `Rule.statement` and `Subject.name`/`short_name`/`description`.
  This is the last of the 16 tools tracked by #55, which is now closed.
  Tool surface goes from 15 to 16.
- New `store.rs` schema: a `search_index` FTS5 virtual table, kept in
  sync incrementally by `insert_rule`/`insert_subject` (via a new private
  `index_for_search` helper) -- never rebuilt per call, and every
  existing seed/import path gets indexed for free since both go through
  those same `insert_*` functions.
- New `store.rs` types/functions: `SearchRefType` (`Rule`/`Subject`),
  `SearchResult`, `search_knowledge`. A `fts5_safe_query` helper quotes
  each whitespace-separated query token as an FTS5 string literal before
  building the `MATCH` expression -- otherwise a query like
  `"data-product"` would be silently reinterpreted by FTS5's query syntax
  as `data NOT product` (a leading `-` is the FTS5 NOT operator), and
  more generally any FTS5 syntax character in a caller's free-text query
  would change the search instead of being searched for literally.
- **Deliberately lexical-only**, not the previous model's hybrid
  FTS5+vector search: the `Embedder` trait and `sqlite-vec` retrieval
  were removed entirely along with the schema this replaces and are not
  reintroduced here (documented in `ARCHITECTURE.md`'s non-goals). The
  tool always declares its retrieval mode (`lexical-only`) in its
  response rather than leaving that undiscoverable.
- README/ARCHITECTURE.md/module doc comments updated for the full
  16-tool surface; #55 closed.

## PR #59 — SelectionGroup + validate_completeness (rusty_knowledge#55)
**2026-08-14** · [#59](https://github.com/Rusty-Mill/rusty_knowledge/pull/59)

- **Added:** `validate_completeness` -- evaluates every `SelectionGroup`
  defined on a container/viewpoint subject against a caller-supplied set
  of element types actually present, reporting per-group and per-member
  satisfaction plus an overall COMPLETE/INCOMPLETE verdict. Tool surface
  goes from 14 to 15; only `search_knowledge` remains of #55's original
  16-tool scope, and it needs a fresh design decision rather than a
  direct re-port.
- New `store.rs` type/functions: `SelectionConstraint` (`All` or
  `AtLeast(n)`), `SelectionGroup` (a cardinality constraint over a set of
  relationship-shaped Rules on one Subject -- distinct from a single
  Rule's own `cardinality` field, which constrains instance count within
  *one* relationship rather than which subset of *several* rules must
  hold), `insert_selection_group`, `selection_groups_for_subject`,
  `evaluate_completeness` (+ `CompletenessFinding`). A member rule with no
  `related_subject_id` can't be checked against the presence set and
  always counts as satisfied, since there's nothing external for the
  caller to have supplied.
- `seed_udra` gained one `SelectionGroup` on `udra.DataProduct`
  (`selgrp.data-product-complete`, `All`, reusing the existing
  `exposes`/`realizes` relationship rules) -- needed to make
  `validate_completeness` demonstrable and testable against real seed
  data rather than synthetic test-only fixtures.
- README/ARCHITECTURE.md/module doc comments updated for the 15-tool
  surface; `SelectionGroup` moved out of the "specified but not
  implemented" non-goals list (`RuleDerivation` is the one construct from
  the fuller seven-table design still deferred).

## PR #58 — machine_check evaluator + validate_element (rusty_knowledge#55)
**2026-08-14** · [#58](https://github.com/Rusty-Mill/rusty_knowledge/pull/58)

- **Added:** `validate_element` -- checks a real element's properties
  against a subject's machine-checkable rules and reports PASS/FAIL/WARNING
  per rule, with citations. Rules with no `machine_check` are listed as not
  machine-checkable, never silently skipped. Tool surface goes from 13 to
  14; only `validate_completeness` (blocked on `SelectionGroup`) and
  `search_knowledge` (needs a fresh design decision) remain of #55's
  original 16-tool scope.
- New `store.rs` types/function backing it: `MachineCheck` (serde-tagged
  enum over the `required_property`/`enum_value`/`pattern`/`range`/`custom`
  shapes a `Rule.machine_check` JSON blob can take), `CheckResult`
  (`Pass`/`Fail`/`Warning`), `evaluate_machine_check`. Design decision: a
  missing property is always `Fail` regardless of check type (the check
  literally couldn't run); a `pattern` mismatch on a present property is
  `Warning`, not `Fail` -- format/style guidance is advisory, unlike
  structural presence, enum membership, or numeric range, which are hard
  violations. Invalid JSON or an invalid regex pattern is `Warning`, never
  a panic.
- Re-added the `rusty_regx` dependency (removed in #54 as unused) -- it
  now has a real caller in the `pattern` check.

## PR #57 — Crosscut + validate_relationship tools ported (rusty_knowledge#55, part 2)
**2026-08-14** · [#57](https://github.com/Rusty-Mill/rusty_knowledge/pull/57)

- **Added:** `lookup_valid_relationships`, `crosscut_traceability`,
  `crosscut_cross_domain`, `crosscut_valid_relationship_candidates`,
  `validate_relationship` -- the remaining five "direct re-port, no
  blocker" tools from #55. Tool surface goes from 8 to 13; only
  `validate_element`, `validate_completeness` (blocked on model
  capability), and `search_knowledge` (needs a fresh design decision)
  remain.
- New `store.rs` query functions: `valid_relationship_types`,
  `traceability` (outgoing/incoming `traces_to` rules, `MUST`-only unless
  `include_optional`), `cross_domain_relationships`,
  `candidate_valid_relationships` (+ `ValidRelationshipCandidate`,
  grouping relationship instances by `(from_type, to_type,
  relationship_type)` with deterministic majority-cardinality tie-breaking
  -- never auto-committed, always a suggestion for human review),
  `validate_relationship`.
- `seed_udra` gained a `data_mesh` domain and Subject plus two more rules
  (a `traces_to` claim and a cross-domain `realizes` claim) -- needed to
  make `crosscut_traceability`/`crosscut_cross_domain` demonstrable and
  testable; previously seed data had zero of either.
- README/ARCHITECTURE.md updated for the 13-tool surface.

## PR #56 — Six more tools ported onto the v2 model (rusty_knowledge#55, part 1)
**2026-08-14** · [#56](https://github.com/Rusty-Mill/rusty_knowledge/pull/56)

- **Added:** `lookup_rules`, `lookup_relationships`, `lookup_domain_summary`,
  `search_constructs`, `meta_list_domains`, `meta_routing_guide` — six of
  the fourteen tools tracked by #55 as unblocked by the v2 model. Tool
  surface goes from 2 to 8.
- New `store.rs` query functions backing them: `all_sources`,
  `list_domains`, `subjects_in_domain`, `domain_summary`,
  `statement_rules_for_subject` (plain statement rules, excludes
  relationship claims), `outgoing_relationships` (relationship claims
  only, the complementary half).
- `lookup_rules` filters by `binding_strength`, reusing the
  `BindingStrength::parse`/`from_str` split already in `store.rs` (added
  back after being trimmed for having no caller in the original vertical
  slice — now it has one).
- `seed_udra`'s illustrative dataset gained one relationship-shaped rule
  (`udra.DataProduct` --exposes--> `udra.DataContract`) — previously zero
  existed, which meant `lookup_relationships` would have had nothing real
  to demonstrate or test against.
- README/ARCHITECTURE.md updated for the 8-tool surface and to point at
  #55 as the tracking issue for the rest.

## PR #54 — Remove unused Cargo.toml dependencies
**2026-08-14** · [#54](https://github.com/Rusty-Mill/rusty_knowledge/pull/54)

- **Removed:** `rusty-embedder-core`/`-http`/`-local`, `rusty_regx`,
  `sqlite-vec`, and the `local-embeddings`/`http-embeddings` Cargo
  features -- all built around the search/embedding infrastructure the
  v2 model (#51) replaced. Nothing in `src/` has referenced any of them
  since that PR; flagged as known cleanup debt in #53, now done.
- No behavior change -- `cargo build --all-features` and default now
  build identically, since no features remain.
- `Cargo.lock` regenerated accordingly.

## PR #53 — README/ARCHITECTURE.md updated for the v2 model
**2026-08-13** · [#53](https://github.com/Rusty-Mill/rusty_knowledge/pull/53)

- **Fixed:** both docs still described the pre-#51 model (15-tool surface,
  `AuthorityLayer`/`Construct`, hybrid FTS5/`sqlite-vec` search,
  `KNOWLEDGE_DB_PATH` file-backed persistence). Rewritten to describe
  current reality: two tools (`lookup_subject`, `crosscut_conflicts`),
  the five-table `Source`/`SourceAuthority`/`Subject`/`Rule`/
  `RuleRelation` schema, in-memory-only storage, and
  `knowledge_mcp_import_v2`.
- **Fixed:** `store.rs`/`main.rs`'s own module doc comments overclaimed
  "seven tables" -- `SelectionGroup`/`RuleDerivation` were specified in
  the fuller design but never implemented. Comment-only, no behavior
  change.
- **Flagged, not fixed:** `Cargo.toml` still lists now-unused embedding/
  search dependencies (`rusty-embedder-*`, `rusty_regx`, `sqlite-vec`)
  and Cargo features from the model this replaces -- noted in
  `ARCHITECTURE.md` as known cleanup debt.
- Docs (plus two doc-comment-only `.rs` edits), no behavior change.

## PR #52 — knowledge-mcp importer for the v2 model, seeded with real data
**2026-08-13** · [#52](https://github.com/Rusty-Mill/rusty_knowledge/pull/52)

- **Added:** `knowledge_mcp_import_v2` -- a row-by-row translation of a
  real `knowledge-mcp` (Python) SQLite file into the v2 store. `domains` +
  `domain_layers` become a straight `Source`/`SourceAuthority` chain per
  domain (layer N answers to layer N-1); `constructs` become `Subject`
  (a null `short_name` falls back to the id's suffix rather than being
  dropped); `rules` become `Rule`; `relationships` become `Rule` with
  `related_subject_id`/`relationship_type`/`cardinality` set (a null
  `rule_type` becomes `binding_strength: May`, the weakest level, not a
  guess at MUST/SHOULD).
- **Disclosed, not force-fit:** the old schema's `conflicts` are
  layer-vs-layer or domain-wide observations, never tied to two specific
  rule ids the way `RuleRelation` requires -- counted and disclosed per
  domain rather than guessed at. `properties`, `embeddings`,
  `ingestion_log`, `schema_version`, `knowledge_fts` have no destination
  concept in the current model and are disclosed as not imported.
- **One inferred addition, clearly disclosed:** if both a `udra` and a
  `data_mesh` domain are present, one `SourceAuthority` edge is added
  from `udra`'s root Source to `data_mesh`'s root Source. The old schema
  has no way to express "this whole domain builds on that whole domain"
  (no `cross_domain_relationships` row exists for it in the reference
  data), but UDRA's own domain description says exactly this
  ("introduces data mesh principles..."). This is the one thing in the
  import that isn't a literal transcription of source data -- reported in
  `ImportReport.disclosures` so it's never mistaken for one.
- **`KNOWLEDGE_MCP_IMPORT_PATH`** now runs this importer instead of the
  small hand-seeded illustrative UDRA dataset (`store::seed_udra`) added
  in #51 -- the two aren't run together, since the reference data's real
  `udra` domain and the hand-seeded one use overlapping ids (both define
  `udra.DataProduct`). Omit it and nothing changes from the hand-seeded
  default.
- Verified against real reference data (3 domains -- UAF 1.3, Data Mesh,
  UDRA -- 214 constructs, 124 rules, 38 relationships): imports cleanly
  with 0 rows skipped, correct multi-level `SourceAuthority` ancestry,
  and the `udra` -> `data_mesh` lineage edge confirmed reachable via
  `ancestors_of`.
- `Subject` gained one field driven by real data: `source_section`
  (populated on 177 of 214 reference constructs, e.g. UAF spec section
  references) -- dropped from #51's minimal vertical slice, but real
  content here, so it's back and now surfaced in `lookup_subject`'s
  output.

## PR #51 — Knowledge model v2: Source/Subject/Rule replaces AuthorityLayer/Construct
**2026-08-13** · [#51](https://github.com/Rusty-Mill/rusty_knowledge/pull/51)

- **Replaced** the fixed 4-layer `AuthorityLayer` (Standard/Tool
  Implementation/Conventions/Process) and `Construct`-based model with a
  seven-table design: `Source` (an authority node -- anything that can
  issue a rule), `SourceAuthority` (a DAG of "child answers to parent"
  edges -- a Source can answer to more than one independent parent),
  `Subject` (canonical, exact-lookup identity for what a rule is about,
  independent of who's making claims about it), `Rule` (the ground-truth
  statement, now carrying `binding_strength` including `DELEGATED`, and
  an optional `machine_check` for rules that need to be checked against a
  real system's state, not just read by a human), `RuleRelation` (the
  human-confirmed conflict gate, with a `status` that goes `stale`
  automatically when a superseded rule invalidates a prior confirmation),
  `SelectionGroup`, and `RuleDerivation`.
- **Why:** the old model's fixed Standard/Tool/Convention/Process
  taxonomy categorizes rules by *type* of authority and doesn't fit
  domains whose authority is nested *organizationally* instead (e.g. a
  data-mesh architecture implemented by a service, then an org within it,
  then a subordinate org within that) -- forcing that shape into 4 fixed
  slots either mislabels content or runs out of room past 4 levels. The
  new model separates authority-type (`Source.kind`, a tag),
  authority-scope (`Source`/`SourceAuthority`, arbitrary-depth and
  -breadth), subject identity (`Subject`), and derivation
  (`RuleDerivation`) as independent axes.
- **This is a vertical slice**, not a full port: two MCP tools
  (`lookup_subject`, `crosscut_conflicts`) are wired end-to-end against
  real seeded UDRA data (a four-level authority chain, a `DELEGATED` rule
  with a confirmed fulfillment, and a genuine sibling conflict between
  two subordinate orgs that only the two-tier conflict gate catches,
  since neither is an ancestor of the other). The previous 15-tool
  surface, the `knowledge-mcp` importer, file-backed persistence
  (`KNOWLEDGE_DB_PATH`), and search (`search_knowledge`,
  `EMBEDDING_BACKEND`) were all built around the model this replaces and
  are removed in this pass, not carried forward -- re-porting them onto
  the new schema is follow-up work, tracked separately.
- **Breaking**: every existing MCP tool name from the previous surface is
  gone in this build. Anything integrating against `rusty_knowledge`
  today will need to move to the new two-tool surface (or wait for the
  rest of the old surface to be re-ported).

## PR #50 — RuleType gains RECOMMENDED/FORBIDDEN
**2026-08-12** · [#50](https://github.com/Rusty-Mill/rusty_knowledge/pull/50)

- **Fixed:** (closes
  [rusty_knowledge#46](https://github.com/Rusty-Mill/rusty_knowledge/issues/46))
  `RuleType` only modeled five of `knowledge-mcp`'s seven `rule_type`
  values (`MUST`/`SHALL`/`SHOULD`/`MAY`/`MUST_NOT`). Rules or
  valid-relationship rules using `RECOMMENDED`/`FORBIDDEN` were silently
  unimportable — the `knowledge-mcp` importer skipped them row-by-row
  with a disclosure, and the MCP tools' `rule_type` filter rejected them
  outright. Added both as full `RuleType` variants (`as_str`/`from_str`
  parity update included, so a row written with either value can also be
  read back without panicking).
- **Decision:** `RECOMMENDED`/`FORBIDDEN` are accepted as MCP-tool-facing
  `rule_type` filter values too, via the same shared `RuleType::parse()`
  path every other variant already uses — no separate, more restrictive
  parsing surface for just these two.
- The `knowledge-mcp` importer now imports rules using either value
  instead of skipping them; a new test proves both round-trip correctly
  through storage.

## PR #49 — Documentation refresh: ARCHITECTURE.md, gap-analysis.md, governance profile
**2026-08-12** · [#49](https://github.com/Rusty-Mill/rusty_knowledge/pull/49)

- **Fixed:** (closes
  [rusty_knowledge#48](https://github.com/Rusty-Mill/rusty_knowledge/issues/48))
  `ARCHITECTURE.md` was unchanged since the first vertical slice — still
  described one tool, two modules, no persistence, and listed the
  conflict registry/multi-domain hosting/vector retrieval as deliberate
  non-goals when all three had since been implemented. Rewritten to
  match current reality: the full tool surface, all three modules
  (`main.rs`/`store.rs`/`knowledge_mcp_import.rs`), the search and
  startup data flows, and a non-goals list that no longer contradicts
  the codebase.
- **Updated:** `gap-analysis.md`'s status banner — it previously said
  the `knowledge-mcp` import gap (#38) "remains open"; it and its three
  follow-ups (#41, #43, #45) have since closed, and #46 (`RuleType`
  extension) is noted as the one still-open item.
- **Updated:** `docs/rusty-mill-profile.md`'s field *values* — several
  claimed "no code exists," "no dependencies selected," "no CI," "no
  tests," all false after implementation proceeded. Corrected to
  describe this repository's actual state. The governance **status**
  itself (Draft, not Accepted; `TRIAL-0003` not formally authorized) is
  deliberately left unchanged — that determination belongs to
  `rusty_foundation_akb`, not something this repository can declare for
  itself. A note explains the gap between "formally authorized" and
  "actually implemented" rather than papering over it.
- Docs only, no code changes.

## PR #47 — Rewrite the README to reflect current reality
**2026-08-12** · [#47](https://github.com/Rusty-Mill/rusty_knowledge/pull/47)

- **Fixed:** (closes
  [rusty_knowledge#45](https://github.com/Rusty-Mill/rusty_knowledge/issues/45))
  `README.md` had been unchanged since the original bootstrap commit —
  still claiming `**Status: pre-code**`, "not yet authorized to contain
  implementation code," and unmet `TRIAL-0003` governance gates, none of
  which reflected reality after dozens of merged PRs.
- Rewritten to describe what's actually implemented: the full 15-tool
  `knowledge-mcp` parity surface, the two tools with no `knowledge-mcp`
  equivalent (`crosscut_valid_relationship_candidates`, the SQLite
  importer), and a documented list of every environment variable that
  configures runtime behavior (`EMBEDDING_BACKEND`/`EMBEDDING_MODEL`/
  `EMBEDDING_DIMENSION`/`OPENAI_API_KEY`, `KNOWLEDGE_MCP_IMPORT_PATH`,
  `KNOWLEDGE_DB_PATH`) — none of which were documented anywhere outside
  source comments before this.
- Docs only, no code changes.

## PR #44 — Suggest candidate valid-relationship rules
**2026-08-12** · [#44](https://github.com/Rusty-Mill/rusty_knowledge/pull/44)

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
