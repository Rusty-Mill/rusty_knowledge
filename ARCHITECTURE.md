# Architecture

## Overview
Rusty Knowledge is an MCP (Model Context Protocol) server, over stdio, that answers
layered domain-knowledge queries. The current vertical slice exposes one tool,
`search_knowledge`, backed by an in-memory SQLite store combining FTS5 full-text
search with a `sqlite-vec` virtual table. Every rule returned carries an
`AuthorityLayer` (Standard / Tool Implementation / Conventions / Process) as a Rust
enum, not an optional field — an unlabeled rule is structurally unrepresentable
(RK-001 / RM-KNOWLEDGE-MODEL-0002).

It is not yet the full domain framework: no conflict registry, no multi-domain
hosting, no vector retrieval in the tool surface, and no persistence beyond an
in-memory connection seeded at startup. See Non-goals below for what's deliberately
deferred, and `docs/rusty-mill-profile.md` for the governance state this repository
is bootstrapping under.

## Boundaries
No port/adapter abstraction exists yet — `main.rs`'s `KnowledgeServer` calls
`store::search` directly, and `store.rs` has exactly one implementation (in-memory
SQLite via `rusqlite` + `sqlite-vec`). Disclosed honestly rather than inventing an
interface with a single implementer: a `Store` trait is worth introducing once a
second backend (e.g. a persisted connection, or a test double) actually needs one.

| Port | Adapter(s) | Notes |
| ---- | ---------- | ----- |
| *(none yet)* | `store.rs` — in-memory SQLite (FTS5 + `sqlite-vec`) | single implementation; not behind a trait |

## Structure
Single binary crate, two modules:
- `main.rs` — MCP transport and tool surface (`rmcp` over stdio); defines
  `KnowledgeServer` and the `search_knowledge` tool.
- `store.rs` — persistence: schema setup (`open_store`), seeding (`seed`), and the
  FTS5 query (`search`), plus the `AuthorityLayer` enum and `Rule` type.

Modular-monolith default holds; the crate hasn't grown enough surface area to need
splitting.

## Data flow
1. An MCP client connects over stdio and calls `tools/call` with `search_knowledge`
   and a `query` string.
2. `KnowledgeServer::search_knowledge` locks the shared `Connection` and calls
   `store::search`.
3. `store::search` runs an FTS5 `MATCH` query against `rules_fts`, mapping each row
   back to a `Rule` (construct, text, `AuthorityLayer`).
4. Results are formatted as `[layer] construct: text` lines and returned as the
   tool's text response; an empty match set returns a "no rules matched" message.

## Key decisions
See [docs/adr/](./docs/adr/) for the record of individual decisions and their tradeoffs.

## Non-goals
Deliberately out of scope for the current slice (per `main.rs`'s module doc), not
silently dropped:
- Streamable HTTP transport — stdio only, `rmcp`'s simplest documented starting point.
- The layered-authority conflict registry (RK-002).
- Multi-domain hosting (RK-003).
- Vector retrieval in the tool surface — the `vec0` table exists in the store but
  isn't queried by `search_knowledge` yet (RK-004).
- Any implementation beyond this bounded vertical slice: per
  `docs/rusty-mill-profile.md`, `TRIAL-0003`'s full entry review is not yet
  authorized, so broader scope isn't this repository's call to make unilaterally.
