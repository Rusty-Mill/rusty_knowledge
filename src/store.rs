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

/// A single rule, always carrying its authority layer — RM-KNOWLEDGE-MODEL-0002.
pub struct Rule {
    pub construct: String,
    pub text: String,
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
    conn.execute_batch(
        "CREATE VIRTUAL TABLE rules_fts USING fts5(construct, text, layer UNINDEXED);
         CREATE VIRTUAL TABLE rule_vectors USING vec0(embedding float[4]);",
    )?;
    Ok(conn)
}

pub fn insert_rule(conn: &Connection, rule: &Rule) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO rules_fts (construct, text, layer) VALUES (?1, ?2, ?3)",
        (&rule.construct, &rule.text, rule.layer.as_str()),
    )?;
    Ok(())
}

pub fn search(conn: &Connection, query: &str) -> rusqlite::Result<Vec<Rule>> {
    let mut stmt =
        conn.prepare("SELECT construct, text, layer FROM rules_fts WHERE rules_fts MATCH ?1")?;
    let rows = stmt.query_map([query], |row| {
        let layer_text: String = row.get(2)?;
        Ok(Rule {
            construct: row.get(0)?,
            text: row.get(1)?,
            layer: AuthorityLayer::from_str(&layer_text),
        })
    })?;
    rows.collect()
}

/// Seed data matching the previous slice's proof-of-concept rows, plus
/// one more construct so `meta.list_domains`-style breadth is visible
/// once that tool exists — not added yet, deliberately, per RK-003
/// (multi-domain hosting) being out of scope for this slice.
pub fn seed(conn: &Connection) -> rusqlite::Result<()> {
    insert_rule(
        conn,
        &Rule {
            construct: "AuthorityGrant".into(),
            text: "An AuthorityGrant MUST declare an explicit scope and expiry.".into(),
            layer: AuthorityLayer::Standard,
        },
    )?;
    insert_rule(
        conn,
        &Rule {
            construct: "AuthorityGrant".into(),
            text: "In practice, teams often omit expiry for internal-only grants.".into(),
            layer: AuthorityLayer::Conventions,
        },
    )?;
    insert_rule(
        conn,
        &Rule {
            construct: "ConflictRegistryEntry".into(),
            text: "A ConflictRegistryEntry MUST record both contradicting rules' layers.".into(),
            layer: AuthorityLayer::Standard,
        },
    )?;
    Ok(())
}
