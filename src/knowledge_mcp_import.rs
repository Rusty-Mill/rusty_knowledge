//! Imports a `knowledge-mcp` (Python) SQLite database into an already-open
//! `rusty_knowledge` store. See rusty_knowledge#38 for the schema-mapping
//! design this follows.
//!
//! `knowledge-mcp`'s and `rusty_knowledge`'s on-disk schemas are not
//! compatible -- different column sets, `layer_num` INTEGER vs
//! `AuthorityLayer` TEXT, no shared `rules` table design -- so this module
//! translates row by row into this crate's existing `store::insert_*`
//! functions rather than reading the source file's tables directly into
//! `rusty_knowledge`'s own tables.
//!
//! Deliberately **not** imported, per the design discussion on #38:
//! - `knowledge_fts`: redundant with `rules`/`constructs` (confirmed by
//!   reading `knowledge-mcp`'s ingestion pipeline -- its `"rule"` rows are
//!   copies of `rules.rule_text`, its `"definition"` rows are copies of
//!   `constructs.description`). `rusty_knowledge`'s own `insert_rule`
//!   already rebuilds `rules_fts` the normal way.
//! - `valid_relationships`: `knowledge-mcp` has no declared-rule table for
//!   this at all -- its `lookup.valid_relationships` infers validity from
//!   existing relationship instances, which is exactly what
//!   `RM-KNOWLEDGE-MODEL-0004` requires `rusty_knowledge` *not* to do.
//!   Left empty on import; disclosed in the returned [`ImportReport`].
//! - `properties`, `domain_layers`, `ingestion_log`, `schema_version`: no
//!   `rusty_knowledge` equivalent.

use crate::store::{
    self, AuthorityLayer, Conflict, Construct, CrossDomainRelationship, Domain, MachineRule,
    Relationship, Rule, RuleType,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

/// Per-table counts, plus every dropped column, unmapped value, or
/// unimportable row -- surfaced explicitly rather than silently discarded,
/// matching this crate's existing "declare, don't silently substitute"
/// discipline (`RM-KNOWLEDGE-MODEL-0005` for search; the same principle
/// applied to ingestion here).
///
/// `rows_skipped` counts rows that failed to import *at all* (e.g. a rule
/// with an unrecognized `layer_num`). A row that imported successfully but
/// lost a field along the way (e.g. a construct's `is_abstract` flag, or a
/// rule whose `machine_rule` didn't parse) is *not* a skipped row -- it's
/// still counted as imported, with the dropped field noted in
/// `disclosures` instead.
#[derive(Debug, Default)]
pub struct ImportReport {
    pub domains_imported: usize,
    pub constructs_imported: usize,
    pub rules_imported: usize,
    pub relationships_imported: usize,
    pub cross_domain_relationships_imported: usize,
    pub conflicts_imported: usize,
    pub embeddings_imported: usize,
    pub rows_skipped: usize,
    pub disclosures: Vec<String>,
}

/// Errors from the import path -- distinguishes a problem reading the
/// source file from a problem writing to the destination store, since
/// they call for different fixes (a bad/foreign source file vs. an
/// ID collision or schema issue in the destination).
#[derive(Debug)]
pub enum ImportError {
    Source(rusqlite::Error),
    Dest(rusqlite::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Source(err) => write!(f, "reading source database: {err}"),
            ImportError::Dest(err) => write!(f, "writing to destination store: {err}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// `knowledge-mcp`'s `layer_num` (1-4) -> `AuthorityLayer`. Distinct from
/// `AuthorityLayer::parse`, which only accepts the Rust string names (for
/// an MCP caller's layer filter) -- the source database stores layers as
/// integers, not strings.
fn authority_layer_from_num(layer_num: i64) -> Option<AuthorityLayer> {
    match layer_num {
        1 => Some(AuthorityLayer::Standard),
        2 => Some(AuthorityLayer::ToolImplementation),
        3 => Some(AuthorityLayer::Conventions),
        4 => Some(AuthorityLayer::Process),
        _ => None,
    }
}

/// Translates `knowledge-mcp`'s `machine_rule` JSON column (per
/// `knowledge_mcp/server.py`'s `_evaluate_machine_rule`:
/// `{"check": "required_property"|"enum_value"|"pattern"|"range"|"custom",
/// "property", "values", "pattern", "min", "max"}`) into a `MachineRule`.
/// `Err` for JSON that doesn't parse, has no `"check"` field, or names a
/// `check` kind with no `rusty_knowledge` equivalent (`"custom"`) -- the
/// caller discloses these rather than silently dropping the check.
fn parse_machine_rule(json: &str) -> Result<MachineRule, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|err| format!("unparseable machine_rule JSON: {err}"))?;

    let property = value
        .get("property")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    match value.get("check").and_then(|v| v.as_str()) {
        Some("required_property") => Ok(MachineRule::RequiredProperty { property }),
        Some("enum_value") => {
            let values = value
                .get("values")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            Ok(MachineRule::EnumValue { property, values })
        }
        Some("pattern") => {
            let pattern = value
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Ok(MachineRule::Pattern { property, pattern })
        }
        Some("range") => Ok(MachineRule::Range {
            property,
            min: value.get("min").and_then(|v| v.as_f64()),
            max: value.get("max").and_then(|v| v.as_f64()),
        }),
        Some(other) => Err(format!(
            "unsupported machine_rule check {other:?} -- no rusty_knowledge equivalent, check dropped"
        )),
        None => Err("machine_rule JSON has no \"check\" field, check dropped".to_string()),
    }
}

/// Imports a `knowledge-mcp` SQLite file (opened read-only) into `dest`.
/// `dest` can be freshly opened (typical) or already contain data -- rows
/// are inserted as-is, so an ID collision with existing data in `dest`
/// surfaces as an `Err`, not a silent overwrite or merge.
pub fn import_knowledge_mcp_db(
    dest: &Connection,
    source_path: &Path,
) -> Result<ImportReport, ImportError> {
    let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(ImportError::Source)?;

    let mut report = ImportReport::default();
    import_domains(&source, dest, &mut report)?;
    let construct_names = import_constructs(&source, dest, &mut report)?;
    import_rules(&source, dest, &construct_names, &mut report)?;
    import_relationships(&source, dest, &mut report)?;
    import_cross_domain_relationships(&source, dest, &mut report)?;
    import_conflicts(&source, dest, &mut report)?;
    import_embeddings(&source, dest, &mut report)?;

    report.disclosures.push(
        "valid_relationships: 0 imported -- knowledge-mcp has no declared-rule table for this; \
         its lookup.valid_relationships infers validity from relationship instances, which \
         rusty_knowledge deliberately does not do (RM-KNOWLEDGE-MODEL-0004)."
            .to_string(),
    );

    Ok(report)
}

fn import_domains(
    source: &Connection,
    dest: &Connection,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare("SELECT id, name FROM domains")
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(ImportError::Source)?;

    let mut count = 0;
    for row in rows {
        let (id, name) = row.map_err(ImportError::Source)?;
        store::insert_domain(dest, &Domain { id, name }).map_err(ImportError::Dest)?;
        count += 1;
    }
    report.domains_imported = count;
    if count > 0 {
        report.disclosures.push(
            "domains: description/standard_body/domain_type dropped -- no destination field."
                .to_string(),
        );
    }
    Ok(())
}

fn import_constructs(
    source: &Connection,
    dest: &Connection,
    report: &mut ImportReport,
) -> Result<HashMap<String, String>, ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT id, domain_id, short_name, name, construct_type, description FROM constructs",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let domain_id: String = row.get(1)?;
            let short_name: Option<String> = row.get(2)?;
            let name: String = row.get(3)?;
            let construct_type: String = row.get(4)?;
            let description: Option<String> = row.get(5)?;
            Ok((id, domain_id, short_name, name, construct_type, description))
        })
        .map_err(ImportError::Source)?;

    let mut names = HashMap::new();
    let mut count = 0;
    for row in rows {
        let (id, domain_id, short_name, name, construct_type, description) =
            row.map_err(ImportError::Source)?;
        // `short_name` is optional in knowledge-mcp; rusty_knowledge's
        // `short_name` is NOT NULL and doubles as the display name
        // everywhere, so fall back to `name` when absent.
        let short_name = short_name.unwrap_or(name);
        names.insert(id.clone(), short_name.clone());
        store::insert_construct(
            dest,
            &Construct {
                id,
                domain_id,
                short_name,
                construct_type,
                description: description.unwrap_or_default(),
            },
        )
        .map_err(ImportError::Dest)?;
        count += 1;
    }
    report.constructs_imported = count;
    if count > 0 {
        report.disclosures.push(
            "constructs: layer_num, is_abstract, is_deprecated, parent_id, source_section, \
             metadata dropped -- no destination field."
                .to_string(),
        );
    }
    Ok(names)
}

fn import_rules(
    source: &Connection,
    dest: &Connection,
    construct_names: &HashMap<String, String>,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT id, construct_id, domain_id, layer_num, rule_type, rule_text, machine_rule FROM rules",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let construct_id: String = row.get(1)?;
            let domain_id: String = row.get(2)?;
            let layer_num: i64 = row.get(3)?;
            let rule_type: String = row.get(4)?;
            let rule_text: String = row.get(5)?;
            let machine_rule: Option<String> = row.get(6)?;
            Ok((
                id,
                construct_id,
                domain_id,
                layer_num,
                rule_type,
                rule_text,
                machine_rule,
            ))
        })
        .map_err(ImportError::Source)?;

    for row in rows {
        let (id, construct_id, domain_id, layer_num, rule_type_str, text, machine_rule_json) =
            row.map_err(ImportError::Source)?;

        let Some(layer) = authority_layer_from_num(layer_num) else {
            report.rows_skipped += 1;
            report.disclosures.push(format!(
                "rules: skipped rule {id:?} -- unrecognized layer_num {layer_num} (expected 1-4)"
            ));
            continue;
        };
        let Some(rule_type) = RuleType::parse(&rule_type_str) else {
            report.rows_skipped += 1;
            report.disclosures.push(format!(
                "rules: skipped rule {id:?} -- unrecognized rule_type {rule_type_str:?}"
            ));
            continue;
        };
        let construct = construct_names
            .get(&construct_id)
            .cloned()
            .unwrap_or_else(|| construct_id.clone());

        let rule_rowid = store::insert_rule(
            dest,
            &Rule {
                domain_id,
                construct_id,
                construct,
                text,
                layer,
                rule_type,
            },
        )
        .map_err(ImportError::Dest)?;
        report.rules_imported += 1;

        if let Some(json) = machine_rule_json.filter(|s| !s.is_empty() && s != "null") {
            match parse_machine_rule(&json) {
                Ok(check) => {
                    store::insert_machine_check(dest, rule_rowid, &check)
                        .map_err(ImportError::Dest)?;
                }
                Err(msg) => report.disclosures.push(format!("rules ({id:?}): {msg}")),
            }
        }
    }
    Ok(())
}

fn import_relationships(
    source: &Connection,
    dest: &Connection,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT id, domain_id, from_construct_id, to_construct_id, relationship_type, \
             layer_num, cardinality, rule_type FROM relationships",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let domain_id: String = row.get(1)?;
            let from_construct_id: String = row.get(2)?;
            let to_construct_id: String = row.get(3)?;
            let relationship_type: String = row.get(4)?;
            let layer_num: i64 = row.get(5)?;
            let cardinality: Option<String> = row.get(6)?;
            let rule_type: Option<String> = row.get(7)?;
            Ok((
                id,
                domain_id,
                from_construct_id,
                to_construct_id,
                relationship_type,
                layer_num,
                cardinality,
                rule_type,
            ))
        })
        .map_err(ImportError::Source)?;

    let mut null_rule_type_count = 0;
    for row in rows {
        let (
            id,
            domain_id,
            from_construct_id,
            to_construct_id,
            relationship_type,
            layer_num,
            cardinality,
            rule_type_str,
        ) = row.map_err(ImportError::Source)?;

        let Some(layer) = authority_layer_from_num(layer_num) else {
            report.rows_skipped += 1;
            report.disclosures.push(format!(
                "relationships: skipped {id:?} -- unrecognized layer_num {layer_num} (expected 1-4)"
            ));
            continue;
        };
        // knowledge-mcp's relationships.rule_type is nullable ("is this
        // trace required?"); rusty_knowledge's Relationship.rule_type is
        // not optional. Defaulting to May (the weakest requirement level)
        // rather than guessing MUST/SHALL -- never claim a relationship is
        // required when the source didn't say so.
        let rule_type = match rule_type_str.as_deref() {
            Some(s) => match RuleType::parse(s) {
                Some(rule_type) => rule_type,
                None => {
                    report.rows_skipped += 1;
                    report.disclosures.push(format!(
                        "relationships: skipped {id:?} -- unrecognized rule_type {s:?}"
                    ));
                    continue;
                }
            },
            None => {
                null_rule_type_count += 1;
                RuleType::May
            }
        };

        store::insert_relationship(
            dest,
            &Relationship {
                id,
                domain_id,
                from_construct_id,
                to_construct_id,
                relationship_type,
                cardinality: cardinality.unwrap_or_default(),
                layer,
                rule_type,
            },
        )
        .map_err(ImportError::Dest)?;
        report.relationships_imported += 1;
    }
    if null_rule_type_count > 0 {
        report.disclosures.push(format!(
            "relationships: {null_rule_type_count} relationship(s) had a null rule_type, \
             defaulted to MAY (the weakest requirement level) -- rusty_knowledge's \
             Relationship.rule_type is not optional."
        ));
    }
    Ok(())
}

fn import_cross_domain_relationships(
    source: &Connection,
    dest: &Connection,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT id, from_domain_id, from_construct_id, to_domain_id, to_construct_id, \
             relationship_type, description, rationale FROM cross_domain_relationships",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CrossDomainRelationship {
                id: row.get(0)?,
                from_domain_id: row.get(1)?,
                from_construct_id: row.get(2)?,
                to_domain_id: row.get(3)?,
                to_construct_id: row.get(4)?,
                relationship_type: row.get(5)?,
                description: row.get(6)?,
                rationale: row.get(7)?,
            })
        })
        .map_err(ImportError::Source)?;

    let mut count = 0;
    for row in rows {
        let rel = row.map_err(ImportError::Source)?;
        store::insert_cross_domain_relationship(dest, &rel).map_err(ImportError::Dest)?;
        count += 1;
    }
    report.cross_domain_relationships_imported = count;
    Ok(())
}

fn import_conflicts(
    source: &Connection,
    dest: &Connection,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT id, domain_id, construct_id, layer_a, layer_b, conflict_type, description, \
             resolution, rationale, review_date FROM conflicts",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let domain_id: String = row.get(1)?;
            let construct_id: Option<String> = row.get(2)?;
            let layer_a: i64 = row.get(3)?;
            let layer_b: i64 = row.get(4)?;
            let conflict_type: String = row.get(5)?;
            let description: String = row.get(6)?;
            let resolution: String = row.get(7)?;
            let rationale: Option<String> = row.get(8)?;
            let review_date: Option<String> = row.get(9)?;
            Ok((
                id,
                domain_id,
                construct_id,
                layer_a,
                layer_b,
                conflict_type,
                description,
                resolution,
                rationale,
                review_date,
            ))
        })
        .map_err(ImportError::Source)?;

    for row in rows {
        let (
            id,
            domain_id,
            construct_id,
            layer_a,
            layer_b,
            conflict_type,
            description,
            resolution,
            rationale,
            review_date,
        ) = row.map_err(ImportError::Source)?;

        let (Some(layer_a), Some(layer_b)) = (
            authority_layer_from_num(layer_a),
            authority_layer_from_num(layer_b),
        ) else {
            report.rows_skipped += 1;
            report.disclosures.push(format!(
                "conflicts: skipped {id:?} -- unrecognized layer_a/layer_b ({layer_a}/{layer_b}, expected 1-4)"
            ));
            continue;
        };

        store::insert_conflict(
            dest,
            &Conflict {
                id,
                domain_id,
                construct_id,
                layer_a,
                layer_b,
                conflict_type,
                description,
                resolution,
                rationale,
                review_date,
            },
        )
        .map_err(ImportError::Dest)?;
        report.conflicts_imported += 1;
    }
    Ok(())
}

fn import_embeddings(
    source: &Connection,
    dest: &Connection,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare("SELECT construct_id, domain_id, model, embedding FROM embeddings WHERE embedding IS NOT NULL")
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            let construct_id: String = row.get(0)?;
            let domain_id: String = row.get(1)?;
            let model: String = row.get(2)?;
            let embedding: Vec<u8> = row.get(3)?;
            Ok((construct_id, domain_id, model, embedding))
        })
        .map_err(ImportError::Source)?;

    let mut count = 0;
    for row in rows {
        let (construct_id, domain_id, model, embedding) = row.map_err(ImportError::Source)?;
        store::insert_construct_embedding(dest, &construct_id, &domain_id, &model, &embedding)
            .map_err(ImportError::Dest)?;
        count += 1;
    }
    report.embeddings_imported = count;
    if count > 0 {
        report
            .disclosures
            .push("embeddings: generated_at dropped -- no destination field.".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh path in the OS temp dir, unique per test (by test name +
    /// thread ID, since `cargo test` runs each test on its own thread) so
    /// concurrently-running tests never collide on the same file.
    fn fixture_path(test_name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "rusty_knowledge_import_test_{test_name}_{:?}.db",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Creates a `knowledge-mcp`-shaped schema (a representative subset --
    /// only the columns this importer reads) at `path`, returning a write
    /// connection for the test to seed with rows. Callers must drop the
    /// returned connection before calling `import_knowledge_mcp_db` on the
    /// same path, so the import's read-only open doesn't race a still-open
    /// writer.
    fn create_fixture_schema(path: &std::path::Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE domains (
                 id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT,
                 standard_body TEXT, domain_type TEXT
             );
             CREATE TABLE constructs (
                 id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, layer_num INTEGER,
                 construct_type TEXT NOT NULL, name TEXT NOT NULL, short_name TEXT,
                 description TEXT, is_abstract INTEGER, is_deprecated INTEGER,
                 parent_id TEXT, source_section TEXT, metadata TEXT
             );
             CREATE TABLE rules (
                 id TEXT PRIMARY KEY, construct_id TEXT NOT NULL, domain_id TEXT NOT NULL,
                 layer_num INTEGER NOT NULL, rule_type TEXT NOT NULL, rule_text TEXT NOT NULL,
                 machine_rule TEXT, source_section TEXT, tags TEXT
             );
             CREATE TABLE relationships (
                 id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, from_construct_id TEXT NOT NULL,
                 to_construct_id TEXT NOT NULL, relationship_type TEXT NOT NULL,
                 layer_num INTEGER NOT NULL, cardinality TEXT, rule_type TEXT,
                 description TEXT, source_section TEXT
             );
             CREATE TABLE cross_domain_relationships (
                 id TEXT PRIMARY KEY, from_domain_id TEXT NOT NULL, from_construct_id TEXT NOT NULL,
                 to_domain_id TEXT NOT NULL, to_construct_id TEXT NOT NULL,
                 relationship_type TEXT NOT NULL, description TEXT, rationale TEXT
             );
             CREATE TABLE conflicts (
                 id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, construct_id TEXT,
                 layer_a INTEGER NOT NULL, layer_b INTEGER NOT NULL, conflict_type TEXT NOT NULL,
                 description TEXT NOT NULL, resolution TEXT NOT NULL, rationale TEXT, review_date TEXT
             );
             CREATE TABLE embeddings (
                 construct_id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, model TEXT NOT NULL,
                 embedding BLOB, generated_at TEXT
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn import_knowledge_mcp_db_happy_path() {
        let path = fixture_path("happy_path");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name, description) VALUES ('d1', 'Demo', 'A demo domain');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name, short_name, description)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'Construct One', 'C1', 'First construct');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text, machine_rule)
                     VALUES ('r1', 'd1:c1', 'd1', 1, 'MUST', 'C1 must have a name.',
                             '{\"check\": \"required_property\", \"property\": \"name\"}');
                 INSERT INTO relationships
                     (id, domain_id, from_construct_id, to_construct_id, relationship_type, layer_num, cardinality, rule_type)
                     VALUES ('rel1', 'd1', 'd1:c1', 'd1:c1', 'self_refs', 1, '0..1', 'MAY');
                 INSERT INTO cross_domain_relationships
                     (id, from_domain_id, from_construct_id, to_domain_id, to_construct_id, relationship_type, description, rationale)
                     VALUES ('cdr1', 'd1', 'd1:c1', 'd1', 'd1:c1', 'governs', 'desc', 'why');
                 INSERT INTO conflicts
                     (id, domain_id, construct_id, layer_a, layer_b, conflict_type, description, resolution)
                     VALUES ('cf1', 'd1', 'd1:c1', 1, 2, 'contradiction', 'they disagree', 'standard wins');
                 INSERT INTO embeddings (construct_id, domain_id, model, embedding)
                     VALUES ('d1:c1', 'd1', 'test-model', X'0000803F');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.domains_imported, 1);
        assert_eq!(report.constructs_imported, 1);
        assert_eq!(report.rules_imported, 1);
        assert_eq!(report.relationships_imported, 1);
        assert_eq!(report.cross_domain_relationships_imported, 1);
        assert_eq!(report.conflicts_imported, 1);
        assert_eq!(report.embeddings_imported, 1);
        assert_eq!(report.rows_skipped, 0);
        assert!(
            report
                .disclosures
                .iter()
                .any(|d| d.contains("valid_relationships"))
        );

        let construct = store::resolve_construct(&dest, "d1", "C1")
            .unwrap()
            .unwrap();
        assert_eq!(construct.id, "d1:c1");
        let rules = store::rules_with_checks_for_construct(&dest, "d1:c1", None).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].1,
            Some(MachineRule::RequiredProperty {
                property: "name".into()
            })
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_knowledge_mcp_db_construct_short_name_falls_back_to_name() {
        let path = fixture_path("short_name_fallback");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name, short_name)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'Full Name Only', NULL);",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        import_knowledge_mcp_db(&dest, &path).unwrap();

        let construct = store::resolve_construct(&dest, "d1", "d1:c1")
            .unwrap()
            .unwrap();
        assert_eq!(construct.short_name, "Full Name Only");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_knowledge_mcp_db_skips_rule_with_unrecognized_layer_num() {
        let path = fixture_path("bad_rule_layer");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'C1');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text)
                     VALUES ('r1', 'd1:c1', 'd1', 9, 'MUST', 'bad layer');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.rules_imported, 0);
        assert_eq!(report.rows_skipped, 1);
        assert!(report.disclosures.iter().any(|d| d.contains("layer_num 9")));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_knowledge_mcp_db_skips_rule_with_unrecognized_rule_type() {
        let path = fixture_path("bad_rule_type");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'C1');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text)
                     VALUES ('r1', 'd1:c1', 'd1', 1, 'MAYBE', 'not a real rule_type value');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.rules_imported, 0);
        assert_eq!(report.rows_skipped, 1);
        assert!(report.disclosures.iter().any(|d| d.contains("MAYBE")));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_knowledge_mcp_db_imports_recommended_and_forbidden_rule_types() {
        let path = fixture_path("recommended_forbidden");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'C1');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text)
                     VALUES ('r1', 'd1:c1', 'd1', 1, 'RECOMMENDED', 'a recommended practice');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text)
                     VALUES ('r2', 'd1:c1', 'd1', 1, 'FORBIDDEN', 'a forbidden practice');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.rules_imported, 2);
        assert_eq!(report.rows_skipped, 0);
        let rules = store::rules_with_checks_for_construct(&dest, "d1:c1", None).unwrap();
        assert!(
            rules
                .iter()
                .any(|(r, _)| r.rule_type == RuleType::Recommended)
        );
        assert!(
            rules
                .iter()
                .any(|(r, _)| r.rule_type == RuleType::Forbidden)
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_knowledge_mcp_db_reports_unparseable_machine_rule_but_still_imports_the_rule() {
        let path = fixture_path("bad_machine_rule_json");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'C1');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text, machine_rule)
                     VALUES ('r1', 'd1:c1', 'd1', 1, 'MUST', 'has bad json', 'not valid json');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.rules_imported, 1);
        assert_eq!(report.rows_skipped, 0);
        assert!(
            report
                .disclosures
                .iter()
                .any(|d| d.contains("unparseable machine_rule"))
        );
        let rules = store::rules_with_checks_for_construct(&dest, "d1:c1", None).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].1, None);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_knowledge_mcp_db_reports_unsupported_machine_rule_check() {
        let path = fixture_path("custom_machine_rule");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'C1');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text, machine_rule)
                     VALUES ('r1', 'd1:c1', 'd1', 1, 'MUST', 'has a custom check',
                             '{\"check\": \"custom\", \"property\": \"x\"}');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.rules_imported, 1);
        assert!(
            report
                .disclosures
                .iter()
                .any(|d| d.contains("unsupported machine_rule check \"custom\""))
        );
        let rules = store::rules_with_checks_for_construct(&dest, "d1:c1", None).unwrap();
        assert_eq!(rules[0].1, None);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_knowledge_mcp_db_defaults_null_relationship_rule_type_to_may() {
        let path = fixture_path("null_relationship_rule_type");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'C1');
                 INSERT INTO relationships
                     (id, domain_id, from_construct_id, to_construct_id, relationship_type, layer_num, cardinality, rule_type)
                     VALUES ('rel1', 'd1', 'd1:c1', 'd1:c1', 'self_refs', 1, '0..1', NULL);",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.relationships_imported, 1);
        assert!(
            report
                .disclosures
                .iter()
                .any(|d| d.contains("defaulted to MAY"))
        );
        let rels = store::relationships_from(&dest, "d1:c1", None, None, None, None).unwrap();
        assert_eq!(rels[0].rule_type, RuleType::May);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_knowledge_mcp_db_skips_conflict_with_unrecognized_layer() {
        let path = fixture_path("bad_conflict_layer");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO constructs (id, domain_id, layer_num, construct_type, name)
                     VALUES ('d1:c1', 'd1', 1, 'entity', 'C1');
                 INSERT INTO conflicts
                     (id, domain_id, construct_id, layer_a, layer_b, conflict_type, description, resolution)
                     VALUES ('cf1', 'd1', 'd1:c1', 1, 0, 'gap', 'bad layer_b', 'n/a');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.conflicts_imported, 0);
        assert_eq!(report.rows_skipped, 1);

        let _ = std::fs::remove_file(&path);
    }
}
