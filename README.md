# Rusty Knowledge

**Status: pre-code.** This repository is the designated target for the [Rusty Knowledge domain-framework implementation trial](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/implementation-trials/rusty-knowledge-trial-proposal.md) (`TRIAL-0003`) — it is **not yet authorized to contain implementation code**, and this bootstrap commit does not authorize it either.

## What this is

Rusty Knowledge is the proposed Rust reimplementation of [`baileyrd/knowledge-mcp`](https://github.com/baileyrd/knowledge-mcp) — a working Python MCP (Model Context Protocol) server that answers structured, layered domain-knowledge queries (constructs, rules, relationships, cross-domain traceability, conflicts) over a four-layer authority model (Standard → Tool Implementation → Conventions → Process), backed by SQLite with hybrid full-text and vector search.

Its architecture is specified, not implemented, in [`Rusty-Mill/rusty_foundation_akb`](https://github.com/Rusty-Mill/rusty_foundation_akb):

- [RFC-0003](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/rfc/0003-rusty-knowledge-domain-framework.md) — placement as a domain framework, not a base capability
- [ADR-0164](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0164-rusty-knowledge-is-a-domain-framework.md), [ADR-0165](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0165-knowledge-layered-authority-carries-over-as-a-requirement.md) — placement and carry-over decisions
- [`docs/02-capabilities/knowledge/`](https://github.com/Rusty-Mill/rusty_foundation_akb/tree/main/docs/02-capabilities/knowledge) — domain model, platform/crate research, composition register, promotion review
- [`TRIAL-0003`](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/05-governance/implementation-trials/rusty-knowledge-trial-proposal.md) — the bounded implementation trial's entry review (currently **not authorized**)

## Why this repository is (almost) empty

Rusty Mill's governance is specification-before-implementation ([ADR-0002](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/adr/0002-specification-before-implementation.md), [RFC-0002](https://github.com/Rusty-Mill/rusty_foundation_akb/blob/main/docs/rfc/0002-implementation-trial-governance.md)): an implementation trial needs every entry gate to pass — Subject, Learning value, Bounds, Ownership, Repository, Verification, Cross-cutting, Operations — before code is authorized. As of this commit, `knowledge` (the domain this repository would implement) is still `Draft domain analysis` with no accepted Experimental promotion decision, so the Subject gate fails, and this repository having no standards profile was itself the reason the Repository gate failed. This commit exists to fix exactly that second gap — nothing more.

This is ordinary repository bootstrap (license, README, standards profile), not trial-authorized implementation work. No source code, dependency, crate, or workspace layout is selected by this commit.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), matching Rusty Mill's ecosystem convention.

## Repository standards profile

See [`docs/rusty-mill-profile.md`](docs/rusty-mill-profile.md) — Draft, not yet Accepted.
