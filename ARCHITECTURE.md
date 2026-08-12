# Architecture

## Overview
Rusty Knowledge is an MCP (Model Context Protocol) server, over stdio, that answers
layered domain-knowledge queries. It implements the full 15-tool `knowledge-mcp`
parity surface — search, lookup, validation, and cross-cutting tools (traceability,
the layered-authority conflict registry, cross-domain relationships) — plus one tool
with no `knowledge-mcp` equivalent, `crosscut_valid_relationship_candidates`. Every
rule returned carries an `AuthorityLayer` (Standard / Tool Implementation /
Conventions / Process) as a Rust enum, not an optional field — an unlabeled rule is
structurally unrepresentable (RK-001 / RM-KNOWLEDGE-MODEL-0002). See
`src/main.rs`'s own module doc comment for the exact, currently-maintained tool list
and configuration surface — it's kept current as tools land; this file summarizes it
rather than duplicating it in detail.

The store is SQLite (FTS5 full-text + `sqlite-vec` for hybrid retrieval), in-memory
by default and optionally file-backed (`KNOWLEDGE_DB_PATH`). Multiple domains coexist
in one store with no cross-domain leakage (RM-KNOWLEDGE-MODEL-0001) — the seeded demo
data alone spans two (`uaf-1.3`, `data-mesh`), and a `knowledge-mcp` SQLite file can
be imported on top via `knowledge_mcp_import` (its own schemas aren't on-disk
compatible with this crate's, so this is a row-by-row translation, not a raw file
open — see that module's doc comment for exactly what does and doesn't translate).

## Boundaries
No port/adapter abstraction exists — `main.rs`'s `KnowledgeServer` calls `store`'s
functions directly, and `store.rs` has exactly one implementation (SQLite via
`rusqlite` + `sqlite-vec`). Disclosed honestly rather than inventing an interface
with a single implementer: a `Store` trait is worth introducing once a second backend
(e.g. a different embedded database, or a test double beyond an in-memory SQLite
connection) actually needs one — none has, yet.

| Port | Adapter(s) | Notes |
| ---- | ---------- | ----- |
| *(none yet)* | `store.rs` — SQLite (FTS5 + `sqlite-vec`), in-memory or file-backed | single implementation; not behind a trait |
| `Embedder` (from `rusty_embedder_core`) | `NullEmbedder` (default); `rusty-embedder-local`/`rusty-embedder-http` (opt-in Cargo features) | the one real trait boundary in this crate — chosen at startup via `EMBEDDING_BACKEND`, not compiled-in-only |

## Structure
Single binary crate, three modules:
- `main.rs` — MCP transport and tool surface (`rmcp` over stdio): defines
  `KnowledgeServer` and every tool (`#[tool_router]`), startup store/embedder
  selection, and the routing guide.
- `store.rs` — everything persistence and domain-logic: schema setup and the two
  store-opening entry points (`open_store`, `open_store_at_path`), seeding, every
  `insert_*`/query function, the `AuthorityLayer`/`Rule`/`Construct`/`Relationship`/
  `Conflict`/etc. types, hybrid search (FTS5 + vector fusion), and machine-rule
  evaluation.
- `knowledge_mcp_import.rs` — translates a `knowledge-mcp` SQLite file's rows into
  `store.rs`'s own `insert_*` functions, since the two on-disk schemas don't match
  column-for-column (different column sets, `layer_num` INTEGER vs `AuthorityLayer`
  TEXT, no shared `rules` table design). Returns an `ImportReport` disclosing every
  dropped column, unmapped value, or unimportable row rather than a silent partial
  import.

Modular-monolith default holds; the crate hasn't grown enough surface area to need
splitting further.

## Data flow

**A search call** (`search_knowledge`):
1. An MCP client connects over stdio and calls `tools/call` with `search_knowledge`.
2. `KnowledgeServer::search_knowledge` locks the shared `Connection` and calls
   `store::hybrid_search`.
3. `hybrid_search` always runs an FTS5 query first. If a real (non-null) embedder is
   configured and a vector index exists, it also runs a `sqlite-vec` KNN query over
   construct-description embeddings and fuses the two result sets via Reciprocal Rank
   Fusion; on any vector-search error it falls back to the FTS-only result instead of
   failing the whole call.
4. The response always declares which retrieval mode actually produced it
   (`lexical-only` or `hybrid`) — RM-KNOWLEDGE-MODEL-0005 requires this be stated, not
   silently substituted.

**Startup:**
1. `KNOWLEDGE_DB_PATH` set → `store::open_store_at_path`, which reuses an
   already-initialized file as-is (skipping the next two steps entirely) or creates
   a fresh schema. Unset → `store::open_store` (in-memory, always fresh).
2. If the store is fresh: `seed()` populates the built-in demo data, then
   `KNOWLEDGE_MCP_IMPORT_PATH`, if set, imports a `knowledge-mcp` file on top.
3. `EMBEDDING_BACKEND` selects the search embedder (`null` by default; `local`/`http`
   need their matching Cargo feature compiled in). `build_construct_embeddings` runs
   unconditionally — a no-op when the embedder is `NullEmbedder`.
4. The MCP server starts serving over stdio.

## Key decisions
See [docs/adr/](./docs/adr/) for the record of individual decisions and their tradeoffs.

## Non-goals
Deliberately out of scope, not silently dropped:
- Streamable HTTP transport — stdio only, `rmcp`'s simplest documented starting
  point (per `main.rs`'s own module doc).
- A `Store` trait / port-adapter abstraction for persistence — see Boundaries above;
  introduce one when a second real implementation actually needs it, not before.
- Committing a `crosscut_valid_relationship_candidates` suggestion into
  `valid_relationships` automatically, or via any MCP tool — `RM-KNOWLEDGE-MODEL-0004`
  requires that set be declared, not inferred; turning a candidate into a real rule
  is a deliberate, separate `insert_valid_relationship` call outside the MCP surface
  today (rusty_knowledge#43).
