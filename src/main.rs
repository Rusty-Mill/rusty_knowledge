//! Rusty Knowledge — vertical-slice proof of concept.
//!
//! This is not the MCP server yet (rmcp integration is the next slice —
//! the dependency is declared and version-resolved in Cargo.toml, but
//! wiring the Streamable HTTP transport is deliberately out of scope
//! here so this slice stays reviewable). This slice exists to test the
//! two riskiest architectural claims from `rusty_foundation_akb`'s
//! knowledge domain research before building anything else on top of
//! them:
//!
//! - RK-001 / RM-KNOWLEDGE-MODEL-0002: an authority layer is a Rust
//!   type, not an optional string — a rule without one must not compile.
//! - RK-004 / RM-KNOWLEDGE-MODEL-0005: FTS5 (via rusqlite's `bundled`
//!   feature) and sqlite-vec both work in the same SQLite connection,
//!   matching the Python `knowledge-mcp` server's own storage design.

use rusqlite::Connection;

/// The four-layer authority model from ADR-0165. Not `Option<Layer>` —
/// per RK-001, an unlabeled rule must be structurally unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthorityLayer {
    Standard,
    ToolImplementation,
    Conventions,
    Process,
}

impl AuthorityLayer {
    fn as_str(self) -> &'static str {
        match self {
            AuthorityLayer::Standard => "Standard",
            AuthorityLayer::ToolImplementation => "Tool Implementation",
            AuthorityLayer::Conventions => "Conventions",
            AuthorityLayer::Process => "Process",
        }
    }
}

/// A single rule, always carrying its authority layer — RM-KNOWLEDGE-MODEL-0002.
struct Rule {
    construct: String,
    text: String,
    layer: AuthorityLayer,
}

fn open_store() -> rusqlite::Result<Connection> {
    // sqlite-vec registers itself as an auto-extension: every connection
    // opened after this call gets vec0 virtual tables. This is the exact
    // mechanism platform-research.md described, now proven to link and run.
    // `sqlite3_auto_extension` lives in rusqlite::ffi, not sqlite_vec itself
    // (sqlite_vec only exports the init entrypoint) -- confirmed against
    // the crate's own usage guide, not guessed.
    #[allow(clippy::missing_transmute_annotations)] // matches sqlite-vec's own documented usage verbatim
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

fn insert_rule(conn: &Connection, rule: &Rule) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO rules_fts (construct, text, layer) VALUES (?1, ?2, ?3)",
        (&rule.construct, &rule.text, rule.layer.as_str()),
    )?;
    Ok(())
}

fn search(conn: &Connection, query: &str) -> rusqlite::Result<Vec<Rule>> {
    let mut stmt =
        conn.prepare("SELECT construct, text, layer FROM rules_fts WHERE rules_fts MATCH ?1")?;
    let rows = stmt.query_map([query], |row| {
        let layer_text: String = row.get(2)?;
        let layer = match layer_text.as_str() {
            "Standard" => AuthorityLayer::Standard,
            "Tool Implementation" => AuthorityLayer::ToolImplementation,
            "Conventions" => AuthorityLayer::Conventions,
            "Process" => AuthorityLayer::Process,
            other => panic!("stored layer {other:?} is not one of the four known layers"),
        };
        Ok(Rule {
            construct: row.get(0)?,
            text: row.get(1)?,
            layer,
        })
    })?;
    rows.collect()
}

fn main() -> rusqlite::Result<()> {
    let conn = open_store()?;

    insert_rule(
        &conn,
        &Rule {
            construct: "AuthorityGrant".into(),
            text: "An AuthorityGrant MUST declare an explicit scope and expiry.".into(),
            layer: AuthorityLayer::Standard,
        },
    )?;
    insert_rule(
        &conn,
        &Rule {
            construct: "AuthorityGrant".into(),
            text: "In practice, teams often omit expiry for internal-only grants.".into(),
            layer: AuthorityLayer::Conventions,
        },
    )?;

    println!("RK-001 check: every stored rule has a non-optional AuthorityLayer field.");
    println!("RK-004 check: FTS5 + sqlite-vec both live in the same connection.\n");

    for rule in search(&conn, "AuthorityGrant")? {
        println!(
            "[{}] {}: {}",
            rule.layer.as_str(),
            rule.construct,
            rule.text
        );
    }

    Ok(())
}
