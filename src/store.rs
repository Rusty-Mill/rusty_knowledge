//! The knowledge store: FTS5 + sqlite-vec over one SQLite connection,
//! with the authority layer as a Rust type rather than an optional field.
//!
//! See `main.rs`'s module doc for what this slice tests and why.

use rusqlite::Connection;

/// The four-layer authority model from ADR-0165. Not `Option<Layer>` —
/// per RK-001, an unlabeled rule must be structurally unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityLayer {
    Standard,
    ToolImplementation,
    Conventions,
    Process,
}

impl AuthorityLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthorityLayer::Standard => "Standard",
            AuthorityLayer::ToolImplementation => "Tool Implementation",
            AuthorityLayer::Conventions => "Conventions",
            AuthorityLayer::Process => "Process",
        }
    }

    /// Trusted-storage parse: panics on a value that isn't one of the four
    /// layers, since a row already in `rules_fts` was only ever written by
    /// `insert_rule` with a valid `as_str()` output.
    fn from_str(text: &str) -> Self {
        match text {
            "Standard" => AuthorityLayer::Standard,
            "Tool Implementation" => AuthorityLayer::ToolImplementation,
            "Conventions" => AuthorityLayer::Conventions,
            "Process" => AuthorityLayer::Process,
            other => panic!("stored layer {other:?} is not one of the four known layers"),
        }
    }

    /// Fallible parse for untrusted input (e.g. an MCP tool caller's layer
    /// filter) -- `None` rather than a panic on anything that isn't one of
    /// the four known layers.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "Standard" => Some(AuthorityLayer::Standard),
            "Tool Implementation" => Some(AuthorityLayer::ToolImplementation),
            "Conventions" => Some(AuthorityLayer::Conventions),
            "Process" => Some(AuthorityLayer::Process),
            _ => None,
        }
    }
}

/// A namespaced body of knowledge (for example UAF 1.3) — RM-KNOWLEDGE-MODEL-0001.
/// A server instance hosts one or more domains concurrently.
pub struct Domain {
    pub id: String,
    pub name: String,
}

/// A named element within a domain (an entity type, artifact, or modeling
/// concept) with rules and relationships attached.
///
/// A subset of `knowledge-mcp`'s `Construct` model: `name`, `is_abstract`,
/// `is_deprecated`, `parent_id`, `source_section`, and `metadata` aren't
/// modeled yet, and aren't invented here -- they land with whichever
/// parity-gap issue actually needs them, not speculatively.
pub struct Construct {
    pub id: String,
    pub domain_id: String,
    pub short_name: String,
    pub construct_type: String,
    pub description: String,
}

/// A rule's normative strength, matching `knowledge-mcp`'s five rule types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleType {
    Must,
    Shall,
    Should,
    May,
    MustNot,
}

impl RuleType {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleType::Must => "MUST",
            RuleType::Shall => "SHALL",
            RuleType::Should => "SHOULD",
            RuleType::May => "MAY",
            RuleType::MustNot => "MUST_NOT",
        }
    }

    fn from_str(text: &str) -> Self {
        match text {
            "MUST" => RuleType::Must,
            "SHALL" => RuleType::Shall,
            "SHOULD" => RuleType::Should,
            "MAY" => RuleType::May,
            "MUST_NOT" => RuleType::MustNot,
            other => panic!("stored rule_type {other:?} is not one of the five known types"),
        }
    }

    /// Fallible parse for untrusted input (e.g. an MCP tool caller's
    /// rule_type filter) -- see `AuthorityLayer::parse`'s doc comment for
    /// why this is kept separate from the trusted-storage `from_str` above.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "MUST" => Some(RuleType::Must),
            "SHALL" => Some(RuleType::Shall),
            "SHOULD" => Some(RuleType::Should),
            "MAY" => Some(RuleType::May),
            "MUST_NOT" => Some(RuleType::MustNot),
            _ => None,
        }
    }
}

/// A single rule, always carrying its authority layer — RM-KNOWLEDGE-MODEL-0002.
/// Scoped to the domain and construct it belongs to, so a query can be
/// restricted to one domain without touching another's rules.
pub struct Rule {
    pub domain_id: String,
    pub construct_id: String,
    pub construct: String,
    pub text: String,
    pub layer: AuthorityLayer,
    pub rule_type: RuleType,
}

/// A typed, directional link between two constructs in the same domain.
/// Queried by `lookup_relationships`; `lookup.valid_relationships` and
/// `crosscut.traceability` (rusty_knowledge#7/#13) land in later issues.
pub struct Relationship {
    pub id: String,
    pub domain_id: String,
    pub from_construct_id: String,
    pub to_construct_id: String,
    pub relationship_type: String,
    pub cardinality: String,
    pub layer: AuthorityLayer,
}

pub fn open_store() -> rusqlite::Result<Connection> {
    // sqlite-vec registers itself as an auto-extension: every connection
    // opened after this call gets vec0 virtual tables. This is the exact
    // mechanism platform-research.md described, now proven to link and run.
    // `sqlite3_auto_extension` lives in rusqlite::ffi, not sqlite_vec itself
    // (sqlite_vec only exports the init entrypoint) -- confirmed against
    // the crate's own usage guide, not guessed.
    #[allow(clippy::missing_transmute_annotations)]
    // matches sqlite-vec's own documented usage verbatim
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }
    let conn = Connection::open_in_memory()?;

    // FTS5: no rusqlite-side feature flag needed (confirmed in
    // platform-research.md) — it's compiled into the bundled SQLite
    // binary via libsqlite3-sys's build.rs, used here via plain SQL.
    //
    // `domain_id`/`construct_id` are UNINDEXED: FTS5 still allows an exact-match
    // predicate on them alongside a MATCH clause, which is what domain-scoped
    // search (a later slice) needs, without folding them into the full-text index.
    conn.execute_batch(
        "CREATE TABLE domains (
             id   TEXT PRIMARY KEY,
             name TEXT NOT NULL
         );
         CREATE TABLE constructs (
             id             TEXT PRIMARY KEY,
             domain_id      TEXT NOT NULL REFERENCES domains(id),
             short_name     TEXT NOT NULL,
             construct_type TEXT NOT NULL,
             description    TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE rules_fts USING fts5(
             domain_id UNINDEXED,
             construct_id UNINDEXED,
             construct,
             text,
             layer UNINDEXED,
             rule_type UNINDEXED
         );
         CREATE TABLE relationships (
             id                TEXT PRIMARY KEY,
             domain_id         TEXT NOT NULL REFERENCES domains(id),
             from_construct_id TEXT NOT NULL REFERENCES constructs(id),
             to_construct_id   TEXT NOT NULL REFERENCES constructs(id),
             relationship_type TEXT NOT NULL,
             cardinality       TEXT NOT NULL,
             layer             TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE rule_vectors USING vec0(embedding float[4]);",
    )?;
    Ok(conn)
}

pub fn insert_domain(conn: &Connection, domain: &Domain) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO domains (id, name) VALUES (?1, ?2)",
        (&domain.id, &domain.name),
    )?;
    Ok(())
}

pub fn insert_construct(conn: &Connection, construct: &Construct) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO constructs (id, domain_id, short_name, construct_type, description)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            &construct.id,
            &construct.domain_id,
            &construct.short_name,
            &construct.construct_type,
            &construct.description,
        ),
    )?;
    Ok(())
}

/// Resolve a construct reference within a domain: tries an exact `short_name`
/// match first, then falls back to a direct ID lookup verified against the
/// domain (matching `knowledge-mcp`'s `_resolve` fallback order in `server.py`).
pub fn resolve_construct(
    conn: &Connection,
    domain_id: &str,
    construct_ref: &str,
) -> rusqlite::Result<Option<Construct>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain_id, short_name, construct_type, description
         FROM constructs WHERE domain_id = ?1 AND short_name = ?2",
    )?;
    let by_name = stmt
        .query_map((domain_id, construct_ref), construct_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if let Some(construct) = by_name.into_iter().next() {
        return Ok(Some(construct));
    }

    let mut stmt = conn.prepare(
        "SELECT id, domain_id, short_name, construct_type, description
         FROM constructs WHERE id = ?1 AND domain_id = ?2",
    )?;
    let by_id = stmt
        .query_map((construct_ref, domain_id), construct_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(by_id.into_iter().next())
}

fn construct_from_row(row: &rusqlite::Row) -> rusqlite::Result<Construct> {
    Ok(Construct {
        id: row.get(0)?,
        domain_id: row.get(1)?,
        short_name: row.get(2)?,
        construct_type: row.get(3)?,
        description: row.get(4)?,
    })
}

fn rule_from_row(row: &rusqlite::Row) -> rusqlite::Result<Rule> {
    let layer_text: String = row.get(4)?;
    let rule_type_text: String = row.get(5)?;
    Ok(Rule {
        domain_id: row.get(0)?,
        construct_id: row.get(1)?,
        construct: row.get(2)?,
        text: row.get(3)?,
        layer: AuthorityLayer::from_str(&layer_text),
        rule_type: RuleType::from_str(&rule_type_text),
    })
}

/// Rules attached to one construct, optionally filtered by authority layer
/// and/or rule type (MUST/SHALL/SHOULD/MAY/MUST_NOT).
pub fn rules_for_construct(
    conn: &Connection,
    construct_id: &str,
    layer: Option<AuthorityLayer>,
    rule_type: Option<RuleType>,
) -> rusqlite::Result<Vec<Rule>> {
    let layer_str = layer.map(AuthorityLayer::as_str);
    let rule_type_str = rule_type.map(RuleType::as_str);
    let mut stmt = conn.prepare(
        "SELECT domain_id, construct_id, construct, text, layer, rule_type
         FROM rules_fts
         WHERE construct_id = ?1
           AND (?2 IS NULL OR layer = ?2)
           AND (?3 IS NULL OR rule_type = ?3)",
    )?;
    let rows = stmt.query_map((construct_id, layer_str, rule_type_str), rule_from_row)?;
    rows.collect()
}

pub fn insert_rule(conn: &Connection, rule: &Rule) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO rules_fts (domain_id, construct_id, construct, text, layer, rule_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            &rule.domain_id,
            &rule.construct_id,
            &rule.construct,
            &rule.text,
            rule.layer.as_str(),
            rule.rule_type.as_str(),
        ),
    )?;
    Ok(())
}

pub fn insert_relationship(conn: &Connection, rel: &Relationship) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO relationships
             (id, domain_id, from_construct_id, to_construct_id, relationship_type, cardinality, layer)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            &rel.id,
            &rel.domain_id,
            &rel.from_construct_id,
            &rel.to_construct_id,
            &rel.relationship_type,
            &rel.cardinality,
            rel.layer.as_str(),
        ),
    )?;
    Ok(())
}

/// Relationships originating from one construct, optionally narrowed to a
/// specific target construct and/or relationship type. Unlike
/// `knowledge-mcp`'s `lookup.relationships` (which silently drops an
/// unresolvable `to_construct_ref` filter rather than erroring), an
/// unresolvable `to_construct_id` here is the caller's job to resolve first
/// -- this function takes an already-resolved ID, not a ref string.
pub fn relationships_from(
    conn: &Connection,
    from_construct_id: &str,
    to_construct_id: Option<&str>,
    relationship_type: Option<&str>,
) -> rusqlite::Result<Vec<Relationship>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain_id, from_construct_id, to_construct_id, relationship_type, cardinality, layer
         FROM relationships
         WHERE from_construct_id = ?1
           AND (?2 IS NULL OR to_construct_id = ?2)
           AND (?3 IS NULL OR relationship_type = ?3)",
    )?;
    let rows = stmt.query_map(
        (from_construct_id, to_construct_id, relationship_type),
        |row| {
            let layer_text: String = row.get(6)?;
            Ok(Relationship {
                id: row.get(0)?,
                domain_id: row.get(1)?,
                from_construct_id: row.get(2)?,
                to_construct_id: row.get(3)?,
                relationship_type: row.get(4)?,
                cardinality: row.get(5)?,
                layer: AuthorityLayer::from_str(&layer_text),
            })
        },
    )?;
    rows.collect()
}

/// How a search response was produced. `RM-KNOWLEDGE-MODEL-0005` requires
/// this be declared on every search response, not silently omitted or
/// substituted -- there is deliberately no `Default` impl, so a caller can't
/// forget to pick one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalMode {
    /// FTS5 keyword match only. The only mode this crate can produce until
    /// rusty_knowledge#18 wires the existing (but unused) `vec0` table into
    /// search.
    LexicalOnly,
}

impl RetrievalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RetrievalMode::LexicalOnly => "lexical-only",
        }
    }
}

/// One ranked search hit: a rule plus its FTS5 rank (more negative = better
/// match, per SQLite's bm25-derived `rank` auxiliary column -- ascending
/// order is already best-first).
pub struct SearchHit {
    pub rule: Rule,
    pub rank: f64,
}

/// Domain/layer-scoped, ranked search — the `RM-KNOWLEDGE-MODEL-0005`-conforming
/// upgrade to plain `search`. `domain_id`/`layer` are optional filters; `None`
/// means unfiltered on that axis, matching the previous unscoped behavior when
/// both are `None`. Always returns `RetrievalMode::LexicalOnly` today; a caller
/// combining this with vector similarity (once rusty_knowledge#18 lands) would
/// report a different mode rather than this function silently claiming hybrid.
pub fn search_scoped(
    conn: &Connection,
    query: &str,
    domain_id: Option<&str>,
    layer: Option<AuthorityLayer>,
) -> rusqlite::Result<(Vec<SearchHit>, RetrievalMode)> {
    let layer_str = layer.map(AuthorityLayer::as_str);
    let mut stmt = conn.prepare(
        "SELECT domain_id, construct_id, construct, text, layer, rule_type, rank
         FROM rules_fts
         WHERE rules_fts MATCH ?1
           AND (?2 IS NULL OR domain_id = ?2)
           AND (?3 IS NULL OR layer = ?3)
         ORDER BY rank",
    )?;
    let rows = stmt.query_map((query, domain_id, layer_str), |row| {
        Ok(SearchHit {
            rule: rule_from_row(row)?,
            rank: row.get(6)?,
        })
    })?;
    Ok((
        rows.collect::<rusqlite::Result<Vec<_>>>()?,
        RetrievalMode::LexicalOnly,
    ))
}

/// Constructs belonging to exactly one domain — proves `RM-KNOWLEDGE-MODEL-0001`
/// (no cross-domain leakage) at the store level, ahead of the `meta.list_domains`
/// and `search.constructs` tools that will eventually wrap this query.
// Exercised by tests today; by `meta.list_domains`/`search.constructs`
// (rusty_knowledge#12/#16) next.
#[allow(dead_code)]
pub fn constructs_in_domain(
    conn: &Connection,
    domain_id: &str,
) -> rusqlite::Result<Vec<Construct>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain_id, short_name, construct_type, description
         FROM constructs WHERE domain_id = ?1",
    )?;
    let rows = stmt.query_map([domain_id], construct_from_row)?;
    rows.collect()
}

/// Seed data spanning two domains — matching `knowledge-mcp`'s own real (UAF 1.3)
/// plus stub (data mesh) domains — specifically so a cross-domain-leakage test has
/// more than one domain to exercise, per `RM-KNOWLEDGE-MODEL-0001`.
pub fn seed(conn: &Connection) -> rusqlite::Result<()> {
    insert_domain(
        conn,
        &Domain {
            id: "uaf-1.3".into(),
            name: "UAF 1.3".into(),
        },
    )?;
    insert_domain(
        conn,
        &Domain {
            id: "data-mesh".into(),
            name: "Data Mesh".into(),
        },
    )?;

    insert_construct(
        conn,
        &Construct {
            id: "uaf-1.3:AuthorityGrant".into(),
            domain_id: "uaf-1.3".into(),
            short_name: "AuthorityGrant".into(),
            construct_type: "entity".into(),
            description: "A scoped, time-bounded grant of authority to act within a domain.".into(),
        },
    )?;
    insert_construct(
        conn,
        &Construct {
            id: "uaf-1.3:ConflictRegistryEntry".into(),
            domain_id: "uaf-1.3".into(),
            short_name: "ConflictRegistryEntry".into(),
            construct_type: "entity".into(),
            description: "A recorded contradiction between two rules across authority layers."
                .into(),
        },
    )?;
    insert_construct(
        conn,
        &Construct {
            id: "data-mesh:DataProduct".into(),
            domain_id: "data-mesh".into(),
            short_name: "DataProduct".into(),
            construct_type: "entity".into(),
            description: "A discoverable, owned unit of data published by a domain team.".into(),
        },
    )?;

    insert_rule(
        conn,
        &Rule {
            domain_id: "uaf-1.3".into(),
            construct_id: "uaf-1.3:AuthorityGrant".into(),
            construct: "AuthorityGrant".into(),
            text: "An AuthorityGrant MUST declare an explicit scope and expiry.".into(),
            layer: AuthorityLayer::Standard,
            rule_type: RuleType::Must,
        },
    )?;
    insert_rule(
        conn,
        &Rule {
            domain_id: "uaf-1.3".into(),
            construct_id: "uaf-1.3:AuthorityGrant".into(),
            construct: "AuthorityGrant".into(),
            text: "In practice, teams often omit expiry for internal-only grants.".into(),
            layer: AuthorityLayer::Conventions,
            rule_type: RuleType::May,
        },
    )?;
    insert_rule(
        conn,
        &Rule {
            domain_id: "uaf-1.3".into(),
            construct_id: "uaf-1.3:ConflictRegistryEntry".into(),
            construct: "ConflictRegistryEntry".into(),
            text: "A ConflictRegistryEntry MUST record both contradicting rules' layers.".into(),
            layer: AuthorityLayer::Standard,
            rule_type: RuleType::Must,
        },
    )?;
    insert_rule(
        conn,
        &Rule {
            domain_id: "data-mesh".into(),
            construct_id: "data-mesh:DataProduct".into(),
            construct: "DataProduct".into(),
            text: "A DataProduct MUST declare an owning domain team.".into(),
            layer: AuthorityLayer::Standard,
            rule_type: RuleType::Must,
        },
    )?;

    insert_relationship(
        conn,
        &Relationship {
            id: "uaf-1.3:AuthorityGrant-records-ConflictRegistryEntry".into(),
            domain_id: "uaf-1.3".into(),
            from_construct_id: "uaf-1.3:AuthorityGrant".into(),
            to_construct_id: "uaf-1.3:ConflictRegistryEntry".into(),
            relationship_type: "records".into(),
            cardinality: "0..*".into(),
            layer: AuthorityLayer::Standard,
        },
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_in_domain_do_not_leak_across_domains() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let uaf = constructs_in_domain(&conn, "uaf-1.3").unwrap();
        assert_eq!(uaf.len(), 2);
        assert!(uaf.iter().all(|c| c.domain_id == "uaf-1.3"));
        assert!(uaf.iter().any(|c| c.short_name == "AuthorityGrant"));
        assert!(uaf.iter().any(|c| c.short_name == "ConflictRegistryEntry"));
        assert!(!uaf.iter().any(|c| c.short_name == "DataProduct"));

        let data_mesh = constructs_in_domain(&conn, "data-mesh").unwrap();
        assert_eq!(data_mesh.len(), 1);
        assert_eq!(data_mesh[0].short_name, "DataProduct");
    }

    #[test]
    fn constructs_in_domain_unknown_domain_returns_empty() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let none = constructs_in_domain(&conn, "does-not-exist").unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn relationships_from_returns_seeded_relationship() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels = relationships_from(&conn, "uaf-1.3:AuthorityGrant", None, None).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].to_construct_id, "uaf-1.3:ConflictRegistryEntry");
        assert_eq!(rels[0].relationship_type, "records");
    }

    #[test]
    fn relationships_from_filters_by_to_construct_and_type() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels = relationships_from(
            &conn,
            "uaf-1.3:AuthorityGrant",
            Some("uaf-1.3:ConflictRegistryEntry"),
            Some("records"),
        )
        .unwrap();
        assert_eq!(rels.len(), 1);

        let none = relationships_from(
            &conn,
            "uaf-1.3:AuthorityGrant",
            None,
            Some("does-not-exist"),
        )
        .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn relationships_from_construct_with_no_relationships_is_empty() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels = relationships_from(&conn, "data-mesh:DataProduct", None, None).unwrap();
        assert!(rels.is_empty());
    }

    #[test]
    fn search_scoped_unfiltered_matches_plain_search() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let (hits, mode) = search_scoped(&conn, "AuthorityGrant", None, None).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(mode, RetrievalMode::LexicalOnly);
    }

    #[test]
    fn search_scoped_filters_by_domain() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let (hits, _) = search_scoped(&conn, "DataProduct", Some("uaf-1.3"), None).unwrap();
        assert!(
            hits.is_empty(),
            "data-mesh's DataProduct must not leak into a uaf-1.3-scoped search"
        );

        let (hits, _) = search_scoped(&conn, "DataProduct", Some("data-mesh"), None).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn search_scoped_filters_by_layer() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let (hits, _) = search_scoped(
            &conn,
            "AuthorityGrant",
            None,
            Some(AuthorityLayer::Conventions),
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule.layer, AuthorityLayer::Conventions);
    }

    #[test]
    fn search_scoped_always_declares_lexical_only_today() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        // RM-KNOWLEDGE-MODEL-0005: the mode must be declared, never silently
        // substituted -- until rusty_knowledge#18 wires vector retrieval in,
        // every response is lexical-only, and this test pins that down.
        let (_, mode) = search_scoped(&conn, "AuthorityGrant", None, None).unwrap();
        assert_eq!(mode, RetrievalMode::LexicalOnly);
    }
}
