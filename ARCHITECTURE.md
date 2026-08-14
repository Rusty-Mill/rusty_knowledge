# Architecture

## Overview
Rusty Knowledge is an MCP (Model Context Protocol) server, over stdio, over a
domain-agnostic authority model: `Source` (anything that can issue a rule),
`SourceAuthority` (a DAG of "child answers to parent" edges — a Source can
answer to more than one independent parent), `Subject` (canonical,
exact-lookup identity for what a rule is about, independent of who's making
claims about it), `Rule` (the ground-truth statement — including
relationship claims between two Subjects, and optional structured
`machine_check` logic), `RuleRelation` (the human-confirmed conflict gate),
`SelectionGroup` (a cardinality constraint over a set of relationship-shaped
Rules), and `RuleDerivation` (a firewalled, non-authoritative rollup
summary). This started as a vertical slice proving that model end-to-end
with two MCP tools and has since grown to the previous model's full 16-tool
surface (tracked in
[rusty_knowledge#55](https://github.com/Rusty-Mill/rusty_knowledge/issues/55),
now closed), plus `lookup_derived_summary`. See `src/store.rs`'s own module
doc comment for the full account of what forced this design and
`src/main.rs`'s for the exact, currently-maintained tool list — it's kept
current as tools land; this file summarizes it rather than duplicating it
in detail.

This replaces an earlier fixed 4-layer `AuthorityLayer`/`Construct` model
(Standard / Tool Implementation / Conventions / Process), which categorized
rules by *type* of authority and didn't fit domains — like UDRA — whose
authority nests *organizationally* instead. That model's full 15-tool
surface and hybrid FTS5/`sqlite-vec` search were **not carried forward
as-is** — they were built around the schema this replaces; the tool
surface has since been re-ported in full (see above), and lexical search
was reintroduced (see Boundaries below), deliberately without the
vector/hybrid component.

The store is SQLite, either in-memory (default, fresh every run) or
file-backed at `KNOWLEDGE_DB_PATH` (created if missing; reopening an
existing file is safe since the schema DDL is idempotent). Seed/import
only ever runs against an empty store (`store::is_empty`) — reopening a
file that already has data from a previous run leaves it alone rather
than re-seeding into primary-key conflicts. A real `knowledge-mcp` SQLite
file can be imported via `knowledge_mcp_import_v2` — its schema isn't
on-disk compatible with this crate's (see that module's doc comment for
exactly what does and doesn't translate, and the one inferred cross-domain
`SourceAuthority` edge it adds, explicitly disclosed rather than silently
fabricated).

## Boundaries
`main.rs`'s `KnowledgeServer` depends on `store::Store`, a trait covering
exactly the read-only query surface its 16 tools need (`resolve_subject`,
`rules_for_subject`, `search_knowledge`, etc.) — `Arc<Mutex<dyn Store +
Send>>`, not a concrete type. `store::SqliteStore` (a thin `Connection`
newtype whose trait methods delegate to `store.rs`'s existing free
functions) is the only implementation. Extracted because a caller
explicitly asked for the abstraction, not because a second backend exists
yet.

Writes (`insert_*`) and bootstrap (`seed_udra`, `open_store`/
`open_store_at`, `is_empty`) deliberately stay **outside** the trait: they
run once at startup, before a `Store` is even constructed, operating on
the raw `Connection` instead (see Data flow below). `knowledge_mcp_import_v2`
also stays on the raw `Connection` rather than the trait, since one of its
paths does a raw `dest.execute` that doesn't map onto a structured port at
all.

| Port | Adapter(s) | Notes |
| ---- | ---------- | ----- |
| `store::Store` (read-only query surface `KnowledgeServer` needs) | `store::SqliteStore` — wraps a `rusqlite::Connection`, in-memory or file-backed (`KNOWLEDGE_DB_PATH`) | single implementation; writes/bootstrap stay on the raw `Connection`, outside the trait |

There is no embedder boundary in this model -- the previous crate's
`Embedder` trait (`rusty_embedder_core`, `NullEmbedder`/local/http backends)
and its `sqlite-vec`-backed hybrid/vector search were built around the old
schema and were not carried forward; their `Cargo.toml` dependencies and the
`local-embeddings`/`http-embeddings` features were removed once nothing in
`src/` referenced them anymore. `search_knowledge` is real, but
lexical-only: a SQLite FTS5 virtual table (`search_index`) kept in sync
incrementally by `insert_rule`/`insert_subject`, with no vector component
and no embedder pluggability -- a deliberate scope decision, documented in
Non-goals below, not a gap.

## Structure
Single binary crate, three modules:
- `main.rs` — MCP transport and tool surface (`rmcp` over stdio): defines
  `KnowledgeServer` and its tools (`#[tool_router]`), and startup
  seed/import selection.
- `store.rs` — everything persistence and domain-logic: schema setup
  (`open_store`/`open_store_at`), every `insert_*`/query function, the
  `Source`/`Subject`/`Rule`/`RuleRelation`/`SelectionGroup`/`RuleDerivation`/
  etc. types, DAG traversal and cycle rejection (`ancestors_of`,
  `insert_source_authority_edge`), the two-tier conflict-candidate query,
  supersession-cascade logic, machine-check evaluation
  (`evaluate_machine_check`), completeness evaluation
  (`evaluate_completeness`), and the hand-seeded illustrative UDRA dataset
  (`seed_udra`).
- `knowledge_mcp_import_v2.rs` — translates a real `knowledge-mcp` SQLite
  file's rows into `store.rs`'s own `insert_*` functions, since the two
  on-disk schemas don't match (the old schema's `domain_layers` has no
  `SourceAuthority` DAG equivalent, its `constructs`/`rules`/`relationships`
  reference a bare `layer_num` integer rather than a `Source` id, and its
  `conflicts` are layer-vs-layer observations rather than the specific
  rule-to-rule pairs `RuleRelation` requires). Returns an `ImportReport`
  disclosing every skipped row and every non-literal inference (like the
  cross-domain lineage edge) rather than a silent partial import.

Modular-monolith default holds; the crate hasn't grown enough surface area to
need splitting further.

## Data flow

**A subject lookup** (`lookup_subject`):
1. An MCP client connects over stdio and calls `tools/call` with
   `lookup_subject`.
2. `KnowledgeServer::lookup_subject` locks the shared `Connection` and calls
   `store::resolve_subject` (exact short-name match within the domain tag,
   falling back to a direct ID match).
3. `store::rules_for_subject` returns every `Rule` naming that Subject —
   either as its primary subject or as the target of a relationship claim —
   joined with the issuing `Source`, regardless of where that Source sits in
   the authority DAG.
4. The response lists every rule with its binding strength, issuing Source
   (and steward, if recorded), any relationship target, and whether it
   carries a `machine_check`.

**A conflict check** (`crosscut_conflicts`):
1. Same subject resolution as above.
2. `store::confirmed_conflicts_for_subject` returns already-confirmed,
   active `conflicts_with` `RuleRelation`s among that subject's rules.
3. `store::conflict_candidates_for_subject` finds same-subject rule pairs
   from different Sources with no confirmed relation yet — correlating by
   exact `subject_id` match first (catching sibling/cousin conflicts a pure
   ancestor-chain walk can't see), excluding pairs where one rule is
   `DELEGATED` and the other's Source is a descendant of the delegating
   Source (the authority working as intended, not an ambiguity).

**Startup:**
1. `KNOWLEDGE_DB_PATH` set → `store::open_store_at` opens (or creates) that
   file, with idempotent schema DDL. Unset → `store::open_store` creates a
   fresh in-memory schema.
2. `store::is_empty` decides whether to seed/import at all — a file
   carrying data from a previous run is left alone. When the store is
   empty: `KNOWLEDGE_MCP_IMPORT_PATH` set →
   `knowledge_mcp_import_v2::import_knowledge_mcp_db` runs against that
   file, falling back to `store::seed_udra` on failure; unset →
   `store::seed_udra` runs directly.
3. The MCP server starts serving over stdio.

## Key decisions
See [docs/adr/](./docs/adr/) for the record of individual decisions and their
tradeoffs.

The fuller seven-table design this model was built from (`Source`,
`SourceAuthority`, `Subject`, `Rule`, `RuleRelation`, `SelectionGroup`,
`RuleDerivation`) is now fully implemented -- nothing from that design is
deferred anymore.

## Non-goals
Deliberately out of scope, not silently dropped:
- Vector/hybrid search (the `Embedder` trait, `sqlite-vec`) — the previous
  model's hybrid FTS5+vector retrieval was built around the old schema and
  was not carried forward. `search_knowledge` is real in the current
  model, but deliberately lexical-only (FTS5, no vector component, no
  embedder pluggability); reintroducing a vector/hybrid layer is follow-up
  work for if a real case needs it, not assumed to happen automatically.
- A second `Store` implementation — the trait now exists (see Boundaries
  above), but `SqliteStore` is still its only implementer; a real second
  backend would be the trigger for anything further (e.g. reconsidering
  which methods belong on the trait, or how bootstrap/writes should be
  exposed to it), not assumed to happen automatically.
