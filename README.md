# Rusty Knowledge

An MCP (Model Context Protocol) server, via [`rmcp`](https://docs.rs/rmcp) over
stdio, that answers structured, layered domain-knowledge queries — constructs,
rules, relationships, cross-domain traceability, and conflicts — over a
four-layer authority model (Standard → Tool Implementation → Conventions →
Process), backed by SQLite (FTS5 full-text search plus `sqlite-vec` for hybrid
retrieval).

It's a Rust reimplementation of [`baileyrd/knowledge-mcp`](https://github.com/baileyrd/knowledge-mcp),
built for parity with that Python server's behavior — not its on-disk schema
(see `knowledge_mcp_import`'s module doc for exactly where the two schemas
diverge and why).

## What's implemented

The full 15-tool `knowledge-mcp` parity surface: `search_knowledge`,
`meta_routing_guide`, `lookup_construct`, `lookup_rules`,
`lookup_relationships`, `lookup_valid_relationships`, `lookup_domain_summary`,
`validate_element`, `validate_relationship`, `validate_completeness`,
`search_constructs`, `crosscut_traceability`, `crosscut_conflicts`,
`crosscut_cross_domain`, and `meta_list_domains` — plus two tools with no
`knowledge-mcp` equivalent:

- `crosscut_valid_relationship_candidates` — suggests candidate declared
  valid-relationship rules from a domain's existing relationship instances,
  for a human to review (never auto-populated; see its own doc comment for
  why).
- A `knowledge-mcp` SQLite database importer (`knowledge_mcp_import`), plus
  optional persistent, file-backed storage — see Configuration below.

`search_knowledge` always declares how it produced its results
(`lexical-only` or `hybrid`) rather than ever silently claiming a retrieval
mode it didn't actually use.

See `src/main.rs`'s module doc comment for the authoritative, current
breakdown — it's kept up to date as tools land, this file summarizes it.

## Getting started

```sh
cargo build
cargo test
cargo run   # starts the MCP server on stdio
```

Real embedding backends (for hybrid search) and the `http` backend's HTTP
client are opt-in Cargo features, kept out of the default build so
`cargo build`/`cargo test` stay fast and dependency-light:

```sh
cargo build --features local-embeddings   # fastembed-rs, no network at runtime after model download
cargo build --features http-embeddings    # reqwest client for any OpenAI-compatible endpoint
```

### Configuration

All optional; every one of these is unset by default, which reproduces the
exact behavior described above with no external dependencies or state.

| Variable | Effect |
| --- | --- |
| `EMBEDDING_BACKEND` | `local` or `http`/`openai` to enable hybrid search with a real embedder (requires the matching Cargo feature above). Unset or unrecognized falls back to `NullEmbedder` — lexical-only search, same as today. |
| `EMBEDDING_MODEL` | Model name for the `http` backend. Defaults to `text-embedding-3-small`. |
| `EMBEDDING_DIMENSION` | Vector dimension for the `http` backend. Defaults to `1536`. |
| `OPENAI_API_KEY` | API key for the `http`/`openai` backend. |
| `KNOWLEDGE_MCP_IMPORT_PATH` | Path to a `knowledge-mcp` SQLite file to import at startup, on top of the seeded demo data. |
| `KNOWLEDGE_DB_PATH` | Path to a file-backed store instead of the in-memory default. A previously-initialized file is reused as-is (seeding/import are skipped on that run); a new or empty file is seeded/imported normally. |

## Architecture

See [ARCHITECTURE.md](./ARCHITECTURE.md) for boundaries, structure, and key
decisions, and [`docs/adr/`](./docs/adr/) for the individual decision records.

This repo's placement and layered-authority requirements were originally
specified in [`Rusty-Mill/rusty_foundation_akb`](https://github.com/Rusty-Mill/rusty_foundation_akb)
([RFC-0003](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/rfc/0003-rusty-knowledge-domain-framework.md),
[ADR-0164](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0164-rusty-knowledge-is-a-domain-framework.md),
[ADR-0165](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0165-knowledge-layered-authority-carries-over-as-a-requirement.md)) —
background for *why* the domain model looks the way it does, not a
description of current implementation state.

## Development

```sh
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md).

## Security

See [SECURITY.md](./SECURITY.md) to report a vulnerability.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT),
matching Rusty Mill's ecosystem convention.
