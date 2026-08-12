# Gap analysis: rusty_knowledge vs. knowledge-mcp

Assessment path: **spec** (path 3 — no `cargo public-api`-diffable surface exists
because the reference, `baileyrd/knowledge-mcp`, is Python; extracted directly from
its `knowledge_mcp/server.py` tool definitions, `docs/02-capabilities/knowledge/model.md`
in `rusty_foundation_akb`, and `rusty_knowledge`'s current `src/`).

Reference pinned at `baileyrd/knowledge-mcp` default branch, `pyproject.toml` version
`0.1.0` (per TRIAL-0003's own citation). Target: `Rusty-Mill/rusty_knowledge` at
`main` (`599e010` + merge `2c1b55c`).

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

## Current state of rusty_knowledge

One MCP tool implemented (`search_knowledge`, `src/main.rs`): FTS5 keyword match
only, no domain/layer scoping, no rank, no vector component, no hybrid-mode
indicator. No domain concept at all — `store::seed` hardcodes three rows into a
single unnamed in-memory table. No conflict registry. No relationship/validation
data model.

## Tool-surface gaps

| Symbol | Category | Source | Platforms | Reference | Breaking? | Est. size | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Multi-domain store (`Domain`/`Construct`/`Rule`/`Relationship` schema) | infra | spec | both | `model.md` Entities; `knowledge_mcp/db/schema.py` | no | L | Prerequisite for nearly every row below — `store.rs` currently has one flat `rules_fts` table with no domain/construct/relationship modeling at all. `RM-KNOWLEDGE-MODEL-0001` (no cross-domain leakage) can't even be tested until this exists. Should be its own issue(s), split before filing, not lumped with a tool. |
| `search.knowledge` (upgrade existing) | fn (existing) | spec | both | `server.py:500-529`; `RM-KNOWLEDGE-MODEL-0005` | **yes** | M | Rust's `search_knowledge` already exists but is missing domain/layer filtering, rank, and — per `RM-KNOWLEDGE-MODEL-0005` — a required hybrid-vs-lexical-only indicator in the response. Widening its response shape is a behavior/signature change on an existing public tool — flagged, not auto-implemented. |
| `lookup.construct` | tool | spec | both | `server.py:179-224` | no | M | Needs the domain store first (row above). |
| `lookup.rules` | tool | spec | both | `server.py:226-258` | no | M | Depends on domain store; also depends on `AuthorityLayer` gaining per-rule-type (MUST/SHALL/SHOULD/MAY/MUST_NOT) metadata beyond the four layers already modeled in `store.rs`. |
| `lookup.relationships` | tool | spec | both | `server.py:260-298` | no | M | Needs a `Relationship` entity (typed, directional, cardinality) — none exists yet. |
| `lookup.valid_relationships` | tool | spec | both | `server.py:300-318` | no | S | Depends on the relationship entity above; mostly a query over its "valid-relationship set" once that exists. |
| `lookup.domain_summary` | tool | spec | both | `server.py:320-354` | no | S | Depends on domain store. |
| `validate.element` | tool | spec | both | `server.py:356-425` | no | L | Rule-evaluation engine (`_evaluate_machine_rule` in Python, `server.py:753`) has no Rust counterpart yet; nontrivial logic, not a thin wrapper. |
| `validate.relationship` | tool | spec | both | `server.py:427-476` | no | M | Depends on relationship entity + valid-relationship set. |
| `validate.completeness` | tool | spec | both | `server.py:478-494` | no | M | Depends on domain store + construct/element-type modeling. |
| `search.constructs` | tool | spec | both | `server.py:532-562` | no | S | List/filter, no ranking — straightforward once domain store exists. |
| `crosscut.traceability` | tool | spec | both | `server.py:564-605` | no | M | Depends on relationship entity. |
| `crosscut.conflicts` | tool | spec | both | `server.py:607-643`; `RM-KNOWLEDGE-MODEL-0003`; `ADR-0165` | no | L | Conflict registry (`RK-002` in TRIAL-0003) doesn't exist in Rust yet — this is the specific hypothesis TRIAL-0003 itself flags as highest-risk (silent precedence resolution vs. genuine queryable registry). Worth extra scrutiny on the implementation, not just the interface. |
| `crosscut.cross_domain` | tool | spec | both | `server.py:645-671` | no | M | Depends on multi-domain store; exercises `RM-KNOWLEDGE-MODEL-0001` directly. |
| `meta.list_domains` | tool | spec | both | `server.py:673-684`; `RM-KNOWLEDGE-MODEL-0001` | no | S | Depends on domain store; the isolation test named in `RK-003`. |
| `meta.routing_guide` | tool | spec | both | `server.py:686-751` | no | S | Static/near-static guidance text — likely the smallest real gap once tool names stabilize. |
| Vector retrieval wired into search | infra | spec | both | `store.rs`'s `vec0` table (unused); `RM-KNOWLEDGE-MODEL-0005`; `RK-004` | no | M | The `sqlite-vec` virtual table is already created in `store.rs` but nothing inserts into or queries it — this is explicitly deferred scope per `main.rs`'s own doc comment, not an oversight. |

15 tools total in the reference; 1 partially present in the target, 14 absent.
Two rows above (`search.knowledge` upgrade, and the domain-store prerequisite) are
marked or sized in a way that means **most of the tool rows can't be started
before those land** — worth sequencing rather than filing all 16 rows as
independent, immediately-parallel issues.

## Reusable crates / capabilities across our repos

Checked the wider `rusty_*` ecosystem (50+ repos under `baileyrd`/`Rusty-Mill`) for
anything `rusty_knowledge` could depend on instead of hand-rolling:

- **`baileyrd/rusty_search`** — a real, tested, pluggable `SearchBackend` trait
  crate (`rusty-search-core` + 8 backend crates: memory, Tantivy, Elasticsearch,
  OpenSearch, Meilisearch, Solr, Algolia, Azure AI Search). **Directly relevant,
  but not a drop-in today**: its own README lists both of what `rusty_knowledge`
  needs — a **SQLite FTS5 backend** and **vector/hybrid search** — under "Planned
  backends," explicitly unimplemented, with the vector-search entry noting it
  doesn't fit the current `Query` DSL without a design decision on adding a
  similarity-query variant. Closing that gap in `rusty_search` (contributing a
  `rusty-search-sqlite-fts5` backend, and settling the vector-query DSL question)
  would benefit both repos rather than `rusty_knowledge` duplicating FTS5/vector
  query-building logic that `rusty_search` was built to abstract over. Flagged as
  a candidate cross-repo issue, not filed against `rusty_knowledge` itself since
  it isn't this repo's gap to close unilaterally.
- **`baileyrd/rusty_sqlite`** — empty repository, nothing to reuse yet.
- **`baileyrd/rusty_mcp`** — a `cargo-generate` project **template** for
  scaffolding new Rust MCP servers, not a runtime library `rusty_knowledge` could
  depend on (it's already past the scaffolding stage).
- No `rusty_persistence`, `rusty_networking`, or `rusty_ipc` capability crate
  exists under either org — `rusty_knowledge` depending on raw `rusqlite`,
  `sqlite-vec`, and `rmcp` directly (as it does today) is the only real option
  right now, matching what `platform-research.md` already researched.

## Deliberately not filed as issues this round

- The domain-store and `search.knowledge`-upgrade rows above are prerequisites,
  not independent gaps — filing all 16 as parallel issues would let 14 of them
  sit blocked on the other 2. Recommend filing the domain-store infra issue
  first, `search.knowledge`'s upgrade as a `breaking-change`-labeled issue for
  explicit sign-off (per this skill's own rule), and holding the rest until the
  store lands.
- The `rusty_search` SQLite-FTS5/vector-backend gap is real but lives in a
  different repo (`baileyrd/rusty_search`) — noted above, not filed here.
