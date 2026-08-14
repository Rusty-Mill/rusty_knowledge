# Architecture

## Overview
Rusty Knowledge is an MCP (Model Context Protocol) server, over stdio, over a
domain-agnostic authority model: `Source` (anything that can issue a rule),
`SourceAuthority` (a DAG of "child answers to parent" edges — a Source can
answer to more than one independent parent), `Subject` (canonical,
exact-lookup identity for what a rule is about, independent of who's making
claims about it), `Rule` (the ground-truth statement — including
relationship claims between two Subjects, and optional structured
`machine_check` logic), and `RuleRelation` (the human-confirmed conflict
gate). This started as a vertical slice proving that model end-to-end with
two MCP tools and is growing incrementally toward the previous model's full
surface (tracked in
[rusty_knowledge#55](https://github.com/Rusty-Mill/rusty_knowledge/issues/55)) —
15 tools so far. See `src/store.rs`'s own module doc comment for the full
account of what forced this design and `src/main.rs`'s for the exact,
currently-maintained tool list — it's kept current as tools land; this file
summarizes it rather than duplicating it in detail.

This replaces an earlier fixed 4-layer `AuthorityLayer`/`Construct` model
(Standard / Tool Implementation / Conventions / Process), which categorized
rules by *type* of authority and didn't fit domains — like UDRA — whose
authority nests *organizationally* instead. That model's full 15-tool
surface, file-backed persistence, and hybrid FTS5/`sqlite-vec` search are
**not carried forward** — they were built around the schema this replaces
and are deferred to follow-up work, not silently dropped.

The store is SQLite, in-memory only (no file-backed persistence option in
this model yet). A real `knowledge-mcp` SQLite file can be imported via
`knowledge_mcp_import_v2` — its schema isn't on-disk compatible with this
crate's (see that module's doc comment for exactly what does and doesn't
translate, and the one inferred cross-domain `SourceAuthority` edge it adds,
explicitly disclosed rather than silently fabricated).

## Boundaries
No port/adapter abstraction exists — `main.rs`'s `KnowledgeServer` calls
`store`'s functions directly, and `store.rs` has exactly one implementation
(SQLite via `rusqlite`). Disclosed honestly rather than inventing an
interface with a single implementer: a `Store` trait is worth introducing
once a second backend actually needs one — none has, yet.

| Port | Adapter(s) | Notes |
| ---- | ---------- | ----- |
| *(none)* | `store.rs` — SQLite via `rusqlite`, in-memory only | single implementation; not behind a trait |

There is no embedder/search boundary in this model — the previous crate's
`Embedder` trait (`rusty_embedder_core`, `NullEmbedder`/local/http backends)
and its `sqlite-vec`-backed hybrid search were built around the old schema
and were not carried forward. Their `Cargo.toml` dependencies and the
`local-embeddings`/`http-embeddings` features were removed once nothing in
`src/` referenced them anymore.

## Structure
Single binary crate, three modules:
- `main.rs` — MCP transport and tool surface (`rmcp` over stdio): defines
  `KnowledgeServer` and its tools (`#[tool_router]`), and startup
  seed/import selection.
- `store.rs` — everything persistence and domain-logic: schema setup
  (`open_store`), every `insert_*`/query function, the `Source`/`Subject`/
  `Rule`/`RuleRelation`/`SelectionGroup`/etc. types, DAG traversal and cycle
  rejection (`ancestors_of`, `insert_source_authority_edge`), the two-tier
  conflict-candidate query, supersession-cascade logic, machine-check
  evaluation (`evaluate_machine_check`), completeness evaluation
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
1. `store::open_store` creates a fresh in-memory schema.
2. `KNOWLEDGE_MCP_IMPORT_PATH` set → `knowledge_mcp_import_v2::import_knowledge_mcp_db`
   runs against that file; on failure, falls back to `store::seed_udra`.
   Unset → `store::seed_udra` runs directly.
3. The MCP server starts serving over stdio.

## Key decisions
See [docs/adr/](./docs/adr/) for the record of individual decisions and their
tradeoffs.

## Non-goals
Deliberately out of scope, not silently dropped:
- The previous model's full 15-tool surface (`lookup_construct`,
  `search_knowledge`, etc.) — built around the schema this replaces;
  re-porting individual tools onto the new model is follow-up work, tracked
  separately, not assumed to happen automatically. `search_knowledge` is the
  one tool left in #55, needing a fresh design decision rather than a
  direct re-port, since its old FTS5/`sqlite-vec` infrastructure was removed
  entirely along with the schema this replaces.
- File-backed persistence (`KNOWLEDGE_DB_PATH`) — the previous model had
  this; the current one is in-memory only until a real case needs it back.
- Search (FTS5/`sqlite-vec` hybrid retrieval, the `Embedder` trait) — same
  reasoning; nothing in the current tool surface needs it yet.
- `RuleDerivation` (firewalled, non-authoritative rollup views) — specified
  in the fuller seven-table design this was built from, not implemented,
  since nothing in the current tool surface needs it. (`SelectionGroup`,
  specified alongside it, *is* now implemented — it backs
  `validate_completeness`.) Not a gap discovered late — a deliberate "don't
  add a construct before a real case needs it" call, same as everything
  else in this model's design history.
- A `Store` trait / port-adapter abstraction for persistence — see
  Boundaries above; introduce one when a second real implementation
  actually needs it, not before.
