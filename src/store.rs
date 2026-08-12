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

    fn from_str(text: &str) -> Self {
        match text {
            "Standard" => AuthorityLayer::Standard,
            "Tool Implementation" => AuthorityLayer::ToolImplementation,
            "Conventions" => AuthorityLayer::Conventions,
            "Process" => AuthorityLayer::Process,
            other => panic!("stored layer {other:?} is not one of the four known layers"),
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
pub struct Construct {
    pub id: String,
    pub domain_id: String,
    pub short_name: String,
    pub construct_type: String,
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
}

/// A typed, directional link between two constructs in the same domain.
// Not read from yet outside tests -- the tools that query relationships
// (lookup.relationships, lookup.valid_relationships, crosscut.traceability;
// rusty_knowledge#6/#7/#13) land in later, separately-scoped issues. This
// type and `insert_relationship` exist now so the schema and its round-trip
// are proven ahead of those tools, not unused/abandoned code.
#[allow(dead_code)]
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
             construct_type TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE rules_fts USING fts5(
             domain_id UNINDEXED,
             construct_id UNINDEXED,
             construct,
             text,
             layer UNINDEXED
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
        "INSERT INTO constructs (id, domain_id, short_name, construct_type) VALUES (?1, ?2, ?3, ?4)",
        (
            &construct.id,
            &construct.domain_id,
            &construct.short_name,
            &construct.construct_type,
        ),
    )?;
    Ok(())
}

pub fn insert_rule(conn: &Connection, rule: &Rule) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO rules_fts (domain_id, construct_id, construct, text, layer) VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            &rule.domain_id,
            &rule.construct_id,
            &rule.construct,
            &rule.text,
            rule.layer.as_str(),
        ),
    )?;
    Ok(())
}

// See `Relationship`'s doc comment: exercised by tests today, by
// relationship-querying tools (rusty_knowledge#6/#7/#13) next.
#[allow(dead_code)]
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

/// Unscoped full-text search across every domain's rules. Deliberately not
/// domain/layer-filtered yet, and deliberately not exposed with a wider
/// response shape — RM-KNOWLEDGE-MODEL-0005-conforming hybrid/rank/mode
/// output is tracked separately (rusty_knowledge#3) as a breaking change to
/// the existing `search_knowledge` tool contract, not bundled into this slice.
pub fn search(conn: &Connection, query: &str) -> rusqlite::Result<Vec<Rule>> {
    let mut stmt = conn.prepare(
        "SELECT domain_id, construct_id, construct, text, layer
         FROM rules_fts WHERE rules_fts MATCH ?1",
    )?;
    let rows = stmt.query_map([query], |row| {
        let layer_text: String = row.get(4)?;
        Ok(Rule {
            domain_id: row.get(0)?,
            construct_id: row.get(1)?,
            construct: row.get(2)?,
            text: row.get(3)?,
            layer: AuthorityLayer::from_str(&layer_text),
        })
    })?;
    rows.collect()
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
        "SELECT id, domain_id, short_name, construct_type FROM constructs WHERE domain_id = ?1",
    )?;
    let rows = stmt.query_map([domain_id], |row| {
        Ok(Construct {
            id: row.get(0)?,
            domain_id: row.get(1)?,
            short_name: row.get(2)?,
            construct_type: row.get(3)?,
        })
    })?;
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
        },
    )?;
    insert_construct(
        conn,
        &Construct {
            id: "uaf-1.3:ConflictRegistryEntry".into(),
            domain_id: "uaf-1.3".into(),
            short_name: "ConflictRegistryEntry".into(),
            construct_type: "entity".into(),
        },
    )?;
    insert_construct(
        conn,
        &Construct {
            id: "data-mesh:DataProduct".into(),
            domain_id: "data-mesh".into(),
            short_name: "DataProduct".into(),
            construct_type: "entity".into(),
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
    fn search_still_matches_seeded_rules_across_all_domains() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let results = search(&conn, "AuthorityGrant").unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.domain_id == "uaf-1.3"));

        let cross_domain = search(&conn, "DataProduct").unwrap();
        assert_eq!(cross_domain.len(), 1);
        assert_eq!(cross_domain[0].domain_id, "data-mesh");
    }

    #[test]
    fn insert_relationship_round_trips() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        insert_relationship(
            &conn,
            &Relationship {
                id: "rel-1".into(),
                domain_id: "uaf-1.3".into(),
                from_construct_id: "uaf-1.3:AuthorityGrant".into(),
                to_construct_id: "uaf-1.3:ConflictRegistryEntry".into(),
                relationship_type: "records".into(),
                cardinality: "0..*".into(),
                layer: AuthorityLayer::Standard,
            },
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM relationships", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
