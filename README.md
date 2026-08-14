# Rusty Knowledge

An MCP (Model Context Protocol) server, via [`rmcp`](https://docs.rs/rmcp) over
stdio, over a domain-agnostic authority model: **Sources** (anything that can
issue a rule — a standard, an org's implementation of it, a subordinate org's
implementation under that) form a DAG of "who answers to whom," **Subjects**
are the canonical, exact-lookup identity for what a rule is about, and
**Rules** are the ground-truth statements a Source makes about a Subject —
including relationship claims between two Subjects, and structured
`machine_check` logic for rules that need to be checked against a real
system's state, not just read by a human.

This replaces an earlier fixed 4-layer authority model (Standard → Tool
Implementation → Conventions → Process), which categorized rules by *type* of
authority and didn't fit domains whose authority nests *organizationally*
instead — e.g. UDRA: a data-mesh standard → the Army's UDRA construct → an
org's implementation under it → a subordinate org's implementation under
that. See `src/store.rs`'s module doc comment for the full account of what
forced the redesign and what each piece of the model is for.

It started as a Rust reimplementation of
[`baileyrd/knowledge-mcp`](https://github.com/baileyrd/knowledge-mcp), but the
current model is no longer parity-shaped with that Python server — the two
diverged once the fixed 4-layer model stopped fitting the domains this needs
to represent. A one-way importer still exists to bring a `knowledge-mcp`
SQLite file's data across (see below); there is no ongoing behavioral parity
goal.

## What's implemented

This is a vertical slice proving the model end-to-end, not a full tool
surface. Two MCP tools:

- `lookup_subject` — everything the full authority chain says about a
  subject: every `Rule` that names it (directly, or as the target of a
  relationship claim), each labeled with the `Source` that issued it.
- `crosscut_conflicts` — conflict-registry status for a subject: confirmed,
  active `conflicts_with` relations between its rules, plus candidate pairs
  (same subject, different Sources, no confirmed relation yet) needing human
  review. Correlates by exact subject identity first, not just an ancestor
  walk, so it catches disagreement between sibling organizations under the
  same parent — not only parent/child contradictions.

See `src/main.rs`'s module doc comment for the authoritative, current
breakdown — it's kept up to date as tools land, this file summarizes it.

## Getting started

```sh
cargo build
cargo test
cargo run   # starts the MCP server on stdio
```

No Cargo features to opt into — the default build is everything there is.

### Configuration

| Variable | Effect |
| --- | --- |
| `KNOWLEDGE_MCP_IMPORT_PATH` | Path to a real `knowledge-mcp` SQLite file. Imports it via `knowledge_mcp_import_v2` instead of the small hand-seeded illustrative UDRA dataset (`store::seed_udra`) that's used otherwise. The two aren't run together — the reference data's real `udra` domain and the hand-seeded one use overlapping ids. See `knowledge_mcp_import_v2`'s module doc for exactly what does and doesn't translate, and the one inferred `SourceAuthority` edge it adds (disclosed, not silent). |

Unset, the server starts with the hand-seeded illustrative UDRA dataset. The
store is in-memory only — nothing persists across restarts; there is no
file-backed storage option in this model yet.

## Architecture

See [ARCHITECTURE.md](./ARCHITECTURE.md) for boundaries, structure, and key
decisions, and [`docs/adr/`](./docs/adr/) for the individual decision records.

This repo's placement and layered-authority requirements were originally
specified in [`Rusty-Mill/rusty_foundation_akb`](https://github.com/Rusty-Mill/rusty_foundation_akb)
([RFC-0003](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/rfc/0003-rusty-knowledge-domain-framework.md),
[ADR-0164](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0164-rusty-knowledge-is-a-domain-framework.md),
[ADR-0165](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0165-knowledge-layered-authority-carries-over-as-a-requirement.md)) —
background for *why* a layered-authority requirement existed in the first
place, not a description of current implementation state. **The current
model has since diverged from the fixed 4-layer shape those documents
describe** (see above) — whether that divergence needs its own RFC/ADR in
`rusty_foundation_akb` is a call for that repo's own governance process, not
this one.

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
