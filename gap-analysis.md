# Gap analysis: rusty_knowledge vs. knowledge-mcp

> **Status update (2026-08-12):** every gap identified below has been closed.
> All 15 reference tools are implemented, the domain/construct/relationship
> data model landed, and vector retrieval is wired end-to-end (storage,
> query, fusion into `search_knowledge`, and an opt-in real embedder via
> `baileyrd/rusty_embedder`). This file is kept as the historical record of
> the original assessment — see the **Status** column added to each row and
> the notes below rather than treating the narrative prose above each table
> as still-current. New gaps not covered by the original assessment surfaced
> afterward and are tracked directly via their issues, not re-added to the
> table below: a `knowledge-mcp` SQLite importer (#38), a configurable/
> persistent store path (#41), and a valid-relationship candidate-suggestion
> tool (#43) — all closed. `RuleType` still has no equivalent for
> `knowledge-mcp`'s `RECOMMENDED`/`FORBIDDEN` rule types (#46) — open.

Assessment path: **spec** (path 3 — no `cargo public-api`-diffable surface exists
because the reference, `baileyrd/knowledge-mcp`, is Python; extracted directly from
its `knowledge_mcp/server.py` tool definitions, `docs/02-capabilities/knowledge/model.md`
in `rusty_foundation_akb`, and `rusty_knowledge`'s current `src/`).

Reference pinned at `baileyrd/knowledge-mcp` default branch, `pyproject.toml` version
`0.1.0` (per TRIAL-0003's own citation). Original target snapshot:
`Rusty-Mill/rusty_knowledge` at `main` (`599e010` + merge `2c1b55c`). **Superseded** —
current `main` is `64d92bd` (52 commits ahead of that snapshot; every row below
closed somewhere in that range).

**Scope note, checked per this skill's step 0:** `rusty_knowledge` has no
hand-curated roadmap of its own yet — its README points at `rusty_foundation_akb`'s
`docs/02-capabilities/knowledge/` docs as the governing spec (`model.md`'s
`RM-KNOWLEDGE-MODEL-000N` requirements and the 15-tool query surface). Those docs
were treated as the roadmap for this table. **Authorization caveat:** those same
docs record `TRIAL-0003` (the implementation trial covering this exact
re-implementation work) as **Not authorized**, and the two prior implementation
commits in `rusty_knowledge` cite an `ADR-0166`/`RFC-0005` fast-lane authorization
that does not exist on `rusty_foundation_akb`'s `main` branch. Per explicit
direction from the repo owner, this run proceeds without blocking on that gate —
noted here for the record, not re-litigated per issue.

## Current state of rusty_knowledge (as of `main` @ `64d92bd`)

All 15 reference MCP tools are implemented (`src/main.rs`), backed by a real
`Domain`/`Construct`/`Rule`/`Relationship` data model (`src/store.rs`) — the
single-flat-FTS-table, no-domain-concept state described below is no longer
current. `search_knowledge` reports rank/score and declares its retrieval mode
(`lexical-only` vs. `hybrid`) per result set, per `RM-KNOWLEDGE-MODEL-0005`.
Vector retrieval is wired: `rule_vectors`/`construct_embeddings` are populated
at startup via a pluggable `Embedder` (`rusty_embedder_core::Embedder`,
defaulting to the zero-dependency `NullEmbedder`; `local-embeddings` and
`http-embeddings` Cargo features opt in to `rusty-embedder-local`/
`rusty-embedder-http` real backends, selected at runtime via `EMBEDDING_BACKEND`)
and fused into `search_scoped`'s results, with discoverable degradation to
lexical-only when no real embedder is configured — never silent substitution.

The paragraph originally here (superseded, kept for record): *"One MCP tool
implemented (`search_knowledge`, `src/main.rs`): FTS5 keyword match only, no
domain/layer scoping, no rank, no vector component, no hybrid-mode indicator.
No domain concept at all — `store::seed` hardcodes three rows into a single
unnamed in-memory table. No conflict registry. No relationship/validation
data model."*

## Tool-surface gaps

| Symbol | Category | Source | Platforms | Reference | Breaking? | Est. size | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Multi-domain store (`Domain`/`Construct`/`Rule`/`Relationship` schema) | infra | spec | both | `model.md` Entities; `knowledge_mcp/db/schema.py` | no | L | ✅ Closed — [#2](https://github.com/Rusty-Mill/rusty_knowledge/issues/2) | Prerequisite for nearly every row below — landed first, unblocking the rest. |
| `search.knowledge` (upgrade existing) | fn (existing) | spec | both | `server.py:500-529`; `RM-KNOWLEDGE-MODEL-0005` | **yes** | M | ✅ Closed — [#3](https://github.com/Rusty-Mill/rusty_knowledge/issues/3) | Breaking-change sign-off was obtained before implementing, per the issue's own gate. |
| `lookup.construct` | tool | spec | both | `server.py:179-224` | no | M | ✅ Closed — [#4](https://github.com/Rusty-Mill/rusty_knowledge/issues/4) | |
| `lookup.rules` | tool | spec | both | `server.py:226-258` | no | M | ✅ Closed — [#5](https://github.com/Rusty-Mill/rusty_knowledge/issues/5) | |
| `lookup.relationships` | tool | spec | both | `server.py:260-298` | no | M | ✅ Closed — [#6](https://github.com/Rusty-Mill/rusty_knowledge/issues/6) | |
| `lookup.valid_relationships` | tool | spec | both | `server.py:300-318` | no | S | ✅ Closed — [#7](https://github.com/Rusty-Mill/rusty_knowledge/issues/7) | |
| `lookup.domain_summary` | tool | spec | both | `server.py:320-354` | no | S | ✅ Closed — [#8](https://github.com/Rusty-Mill/rusty_knowledge/issues/8) | |
| `validate.element` | tool | spec | both | `server.py:356-425` | no | L | ✅ Closed — [#9](https://github.com/Rusty-Mill/rusty_knowledge/issues/9) | Rule-evaluation engine landed; `62fd049` additionally wired `rusty_regx` into the `Pattern` check. |
| `validate.relationship` | tool | spec | both | `server.py:427-476` | no | M | ✅ Closed — [#10](https://github.com/Rusty-Mill/rusty_knowledge/issues/10) | |
| `validate.completeness` | tool | spec | both | `server.py:478-494` | no | M | ✅ Closed — [#11](https://github.com/Rusty-Mill/rusty_knowledge/issues/11) | |
| `search.constructs` | tool | spec | both | `server.py:532-562` | no | S | ✅ Closed — [#12](https://github.com/Rusty-Mill/rusty_knowledge/issues/12) | |
| `crosscut.traceability` | tool | spec | both | `server.py:564-605` | no | M | ✅ Closed — [#13](https://github.com/Rusty-Mill/rusty_knowledge/issues/13) | |
| `crosscut.conflicts` | tool | spec | both | `server.py:607-643`; `RM-KNOWLEDGE-MODEL-0003`; `ADR-0165` | no | L | ✅ Closed — [#14](https://github.com/Rusty-Mill/rusty_knowledge/issues/14) | Conflict registry (`RK-002`) implemented as a genuine queryable registry, not silent precedence resolution. |
| `crosscut.cross_domain` | tool | spec | both | `server.py:645-671` | no | M | ✅ Closed — [#15](https://github.com/Rusty-Mill/rusty_knowledge/issues/15) | |
| `meta.list_domains` | tool | spec | both | `server.py:673-684`; `RM-KNOWLEDGE-MODEL-0001` | no | S | ✅ Closed — [#16](https://github.com/Rusty-Mill/rusty_knowledge/issues/16) | |
| `meta.routing_guide` | tool | spec | both | `server.py:686-751` | no | S | ✅ Closed — [#17](https://github.com/Rusty-Mill/rusty_knowledge/issues/17) | |
| Vector retrieval wired into search | infra | spec | both | `store.rs`'s `vec0` table; `RM-KNOWLEDGE-MODEL-0005`; `RK-004` | no | M | ✅ Closed — [#18](https://github.com/Rusty-Mill/rusty_knowledge/issues/18) (storage/query) + [#37](https://github.com/Rusty-Mill/rusty_knowledge/issues/37) (fused into `search_knowledge`, real embedder wiring) | Split into two issues during the loop: #18 covered `rule_vectors` insert/query wiring only (by its own acceptance criteria); #37 covered fusing those results into `search_scoped`'s response, a real hybrid `RetrievalMode` variant, and selecting a real (non-null) embedder backend in `main()`. Both closed. |

All 15 reference tools are now implemented (was 1 partial / 14 absent at the
original snapshot), plus the domain-store and vector-retrieval infra rows.
Sequencing worked as originally recommended: domain-store landed first,
`search.knowledge`'s breaking upgrade got explicit sign-off, and the
remaining 14 tool rows landed once unblocked.

## Reusable crates / capabilities across our repos

Checked the wider `rusty_*` ecosystem (50+ repos under `baileyrd`/`Rusty-Mill`) for
anything `rusty_knowledge` could depend on instead of hand-rolling:

- **`baileyrd/rusty_search`** — a real, tested, pluggable `SearchBackend` trait
  crate (`rusty-search-core` + 8 backend crates: memory, Tantivy, Elasticsearch,
  OpenSearch, Meilisearch, Solr, Algolia, Azure AI Search). At the time of the
  original assessment its README listed both of what `rusty_knowledge` needed —
  a SQLite FTS5 backend and vector/hybrid search — as unimplemented "Planned
  backends," flagged here as a candidate cross-repo issue rather than filed.
  **Since resolved**: filed as [`rusty_search#14`](https://github.com/baileyrd/rusty_search/issues/14)
  and closed by [PR #15](https://github.com/baileyrd/rusty_search/pull/15) —
  `rusty-search-sqlite-fts5` now exists, and the vector-query DSL question is
  settled (`rusty-search-core` gained a standalone `VectorQuery { field, vector,
  k }` type plus an additive `SearchRequest::vector` field, documented in
  ADR-0008; `rusty_search`'s own `Query` enum stays boolean-predicate-only).
  **Not yet consumed here**: `rusty_knowledge` still depends on `rusqlite` +
  `sqlite-vec` directly for its FTS5/vector storage (see PR #36) rather than on
  `rusty-search-sqlite-fts5` — migrating onto it would be a separate, later
  change, not something this assessment round did.
- **`baileyrd/rusty_embedder`** *(new since the original assessment)* — a
  pluggable text-embedding crate (`rusty-embedder-core`'s `Embedder` trait +
  `NullEmbedder`, `rusty-embedder-local` for ONNX-via-`fastembed-rs`,
  `rusty-embedder-http` for OpenAI-compatible REST endpoints), merged via
  [PR #2](https://github.com/baileyrd/rusty_embedder/pull/2). Filed as the
  prior-art gap `rusty_knowledge#18` called out (Python's
  `knowledge_mcp/embeddings/` protocol). **Already consumed**: `Cargo.toml`
  depends on `rusty-embedder-core`/`-local`/`-http` at rev `6add27a`, and
  `main()`'s `embedder_from_env()` selects `rusty-embedder-local` or
  `rusty-embedder-http` at runtime behind the `local-embeddings`/
  `http-embeddings` feature flags, falling back to `NullEmbedder` (degrading
  search to lexical-only, discoverably) when unset or misconfigured.
- **`baileyrd/rusty_sqlite`** — empty repository, nothing to reuse yet
  (unchecked since the original assessment).
- **`baileyrd/rusty_mcp`** — a `cargo-generate` project **template** for
  scaffolding new Rust MCP servers, not a runtime library `rusty_knowledge` could
  depend on (it's already past the scaffolding stage).
- No `rusty_persistence`, `rusty_networking`, or `rusty_ipc` capability crate
  exists under either org (unchecked since the original assessment) —
  `rusty_knowledge` depends on raw `rusqlite`, `sqlite-vec`, and `rmcp`
  directly, matching what `platform-research.md` already researched.

## Deliberately not filed as issues this round

- The domain-store and `search.knowledge`-upgrade rows above were treated as
  prerequisites, not independent gaps, and filed/sequenced first as
  recommended — see Status column above; this played out as planned.
- ~~The `rusty_search` SQLite-FTS5/vector-backend gap is real but lives in a
  different repo (`baileyrd/rusty_search`) — noted above, not filed here.~~
  **Superseded**: filed as `rusty_search#14` and closed by `rusty_search#15`
  (see Reusable crates section above).
- **New gap surfaced after this file's original scope, not retroactively
  added to the table above**: [`rusty_knowledge#38`](https://github.com/Rusty-Mill/rusty_knowledge/issues/38),
  "Import a knowledge-mcp SQLite database (schemas are not currently
  compatible)" — open. `rusty_knowledge` has no file-backed storage or
  ingestion path at all today, and even with one, the two schemas encode
  layers/constructs/rules differently enough that a straight file-open
  wouldn't work; needs a translating importer instead. Track directly on that
  issue rather than here, since it wasn't part of the original knowledge-mcp
  tool-surface/spec comparison this file assessed.
