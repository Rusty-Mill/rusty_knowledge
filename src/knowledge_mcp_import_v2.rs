//! Imports a `knowledge-mcp` (Python) SQLite database into the
//! knowledge-model-v2 store, read-only against the source file. The two
//! on-disk schemas are not compatible -- `knowledge-mcp`'s `domain_layers`
//! (a flat, fixed-depth layer list per domain) has no `Source`/
//! `SourceAuthority` DAG equivalent, its `constructs`/`rules`/
//! `relationships` reference a bare `layer_num` integer rather than a
//! `Source` id, and its `conflicts` are layer-vs-layer or domain-wide
//! observations rather than the specific rule-to-rule pairs
//! `RuleRelation` requires -- so this is a row-by-row translation, not a
//! raw file open.
//!
//! What imports cleanly:
//! - `domains` + `domain_layers` -> `Source` + `SourceAuthority`: each
//!   domain's layer stack becomes a straight chain (layer N answers to
//!   layer N-1), matching `knowledge-mcp`'s own implicit layer-number
//!   ordering. Layer 1 (the root) carries the domain id as its
//!   `domain_tags`; deeper layers inherit it per the model's own rule.
//! - `constructs` -> `Subject`: a near-direct mapping (both use the same
//!   `"{domain_id}.{short_name}"` id scheme). A null `short_name` (37 of
//!   214 rows in the reference data, all `pattern`/`service_activity`/
//!   `role`/`component`/`construct` types) falls back to the id's suffix
//!   rather than being dropped.
//! - `rules` -> `Rule`.
//! - `relationships` -> `Rule` with `related_subject_id`/
//!   `relationship_type`/`cardinality` set, per the model's design (a
//!   relationship claim has the same shape as any other rule). A null
//!   `rule_type` becomes `binding_strength: May` -- the weakest level,
//!   not a guess at MUST/SHOULD.
//!
//! What doesn't, and is disclosed rather than silently dropped or
//! force-fit:
//! - `conflicts`: layer-vs-layer or domain-wide observations in the old
//!   schema, never tied to two specific rule ids the way `RuleRelation`
//!   requires. Counted and disclosed per domain, not imported -- a human
//!   needs to read each one and decide which specific `Rule`s it's
//!   actually about before confirming a `RuleRelation(conflicts_with)`.
//! - `properties`, `embeddings`, `ingestion_log`, `schema_version`,
//!   `knowledge_fts`: no destination concept in the current model for a
//!   properties/attribute schema, embeddings, or import audit log.
//!
//! One thing this importer adds that isn't literally present in the
//! source data: if both a `udra` and a `data_mesh` domain are found, one
//! `SourceAuthority` edge is added from `udra`'s root Source to
//! `data_mesh`'s root Source. The old schema has no way to express "this
//! whole domain builds on that whole domain" (no `cross_domain_relationships`
//! row exists for it in the reference data), but UDRA's own domain
//! description says exactly this ("introduces data mesh principles...").
//! Disclosed explicitly, not silently inferred -- delete the disclosed
//! edge after import if it doesn't hold for a given source file.

use crate::store::{self, BindingStrength, Rule, Source, Subject};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct ImportReport {
    pub sources_imported: usize,
    pub source_authority_edges_imported: usize,
    pub subjects_imported: usize,
    pub rules_imported: usize,
    pub rows_skipped: usize,
    pub disclosures: Vec<String>,
}

#[derive(Debug)]
pub enum ImportError {
    /// Reading the source `knowledge-mcp` file failed.
    Source(rusqlite::Error),
    /// Writing to the destination store failed.
    Dest(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Source(err) => write!(f, "reading source knowledge-mcp database: {err}"),
            ImportError::Dest(err) => write!(f, "writing to destination store: {err}"),
        }
    }
}

impl std::error::Error for ImportError {}

fn dest_err(err: rusqlite::Error) -> ImportError {
    ImportError::Dest(err.to_string())
}

pub fn import_knowledge_mcp_db(
    dest: &Connection,
    source_path: &Path,
) -> Result<ImportReport, ImportError> {
    let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(ImportError::Source)?;
    let mut report = ImportReport::default();

    let (source_id_by_domain_layer, root_source_by_domain) =
        import_sources(dest, &source, &mut report)?;
    import_subjects(dest, &source, &mut report)?;
    import_rules(dest, &source, &source_id_by_domain_layer, &mut report)?;
    import_relationships(dest, &source, &source_id_by_domain_layer, &mut report)?;
    disclose_unsupported(dest, &source, &mut report)?;
    add_udra_data_mesh_lineage_edge(dest, &root_source_by_domain, &mut report)?;

    Ok(report)
}

type DomainLayerKey = (String, i64);
type SourceImportResult = (HashMap<DomainLayerKey, String>, HashMap<String, String>);

fn import_sources(
    dest: &Connection,
    source: &Connection,
    report: &mut ImportReport,
) -> Result<SourceImportResult, ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT domain_id, layer_num, layer_name, authority, source_name, source_url, \
             source_version, owner FROM domain_layers ORDER BY domain_id, layer_num",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })
        .map_err(ImportError::Source)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ImportError::Source)?;

    let mut source_id_by_domain_layer = HashMap::new();
    let mut root_source_by_domain = HashMap::new();

    for (
        domain_id,
        layer_num,
        layer_name,
        authority,
        source_name,
        source_url,
        source_version,
        owner,
    ) in &rows
    {
        let source_id = format!("src.{domain_id}.{layer_num}");
        let citation = match (source_name, source_version) {
            (Some(name), Some(version)) => Some(format!("{name} (v{version})")),
            (Some(name), None) => Some(name.clone()),
            (None, _) => source_url.clone(),
        };
        let domain_tags = if *layer_num == 1 {
            vec![domain_id.clone()]
        } else {
            vec![]
        };
        store::insert_source(
            dest,
            &Source {
                id: source_id.clone(),
                name: layer_name.clone(),
                kind: authority.clone(),
                domain_tags,
                steward: owner.clone(),
                citation,
                supersedes_source_id: None,
            },
        )
        .map_err(dest_err)?;
        report.sources_imported += 1;

        source_id_by_domain_layer.insert((domain_id.clone(), *layer_num), source_id.clone());
        if *layer_num == 1 {
            root_source_by_domain.insert(domain_id.clone(), source_id.clone());
        }
    }

    for (domain_id, layer_num, ..) in &rows {
        if *layer_num <= 1 {
            continue;
        }
        let child_id = &source_id_by_domain_layer[&(domain_id.clone(), *layer_num)];
        match source_id_by_domain_layer.get(&(domain_id.clone(), layer_num - 1)) {
            Some(parent_id) => {
                store::insert_source_authority_edge(dest, child_id, parent_id)
                    .map_err(ImportError::Dest)?;
                report.source_authority_edges_imported += 1;
            }
            None => {
                report.disclosures.push(format!(
                    "domain_layers: {domain_id:?} layer {layer_num} has no layer {} to answer \
                     to -- left as its own root",
                    layer_num - 1
                ));
            }
        }
    }

    Ok((source_id_by_domain_layer, root_source_by_domain))
}

fn import_subjects(
    dest: &Connection,
    source: &Connection,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT id, domain_id, construct_type, name, short_name, description, \
             is_deprecated, parent_id, source_section FROM constructs",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(ImportError::Source)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ImportError::Source)?;

    // Two passes: insert every Subject with parent_subject_id left NULL
    // first, then backfill it -- a construct's `parent_id` can point at a
    // sibling not yet inserted, and `subjects.parent_subject_id` is a
    // foreign key `dest` enforces.
    let mut pending_parents = Vec::new();
    for (
        id,
        domain_id,
        subject_type,
        name,
        short_name,
        description,
        is_deprecated,
        parent_id,
        source_section,
    ) in rows
    {
        let short_name = short_name.unwrap_or_else(|| {
            id.strip_prefix(&format!("{domain_id}."))
                .unwrap_or(&id)
                .to_string()
        });
        store::insert_subject(
            dest,
            &Subject {
                id: id.clone(),
                domain_tag: domain_id,
                subject_type,
                name,
                short_name,
                description,
                is_deprecated: is_deprecated != 0,
                parent_subject_id: None,
                supersedes_subject_id: None,
                source_section,
            },
        )
        .map_err(dest_err)?;
        report.subjects_imported += 1;
        if let Some(parent_id) = parent_id {
            pending_parents.push((id, parent_id));
        }
    }

    for (id, parent_id) in pending_parents {
        dest.execute(
            "UPDATE subjects SET parent_subject_id = ?1 WHERE id = ?2",
            rusqlite::params![parent_id, id],
        )
        .map_err(dest_err)?;
    }

    Ok(())
}

fn import_rules(
    dest: &Connection,
    source: &Connection,
    source_id_by_domain_layer: &HashMap<DomainLayerKey, String>,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT id, construct_id, domain_id, layer_num, rule_type, rule_text, machine_rule \
             FROM rules",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(ImportError::Source)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ImportError::Source)?;

    for (id, construct_id, domain_id, layer_num, rule_type, rule_text, machine_rule) in rows {
        let Some(source_id) = source_id_by_domain_layer.get(&(domain_id.clone(), layer_num)) else {
            report.rows_skipped += 1;
            report.disclosures.push(format!(
                "rules: skipped {id:?} -- no Source for domain {domain_id:?} layer {layer_num}"
            ));
            continue;
        };
        let Some(binding_strength) = BindingStrength::parse(&rule_type) else {
            report.rows_skipped += 1;
            report.disclosures.push(format!(
                "rules: skipped {id:?} -- unrecognized rule_type {rule_type:?}"
            ));
            continue;
        };
        let machine_check = machine_rule.filter(|value| value != "null");

        store::insert_rule(
            dest,
            &Rule {
                id,
                source_id: source_id.clone(),
                subject_id: construct_id,
                related_subject_id: None,
                relationship_type: None,
                cardinality: None,
                statement: rule_text,
                machine_check,
                binding_strength,
                supersedes_rule_id: None,
            },
        )
        .map_err(dest_err)?;
        report.rules_imported += 1;
    }

    Ok(())
}

fn import_relationships(
    dest: &Connection,
    source: &Connection,
    source_id_by_domain_layer: &HashMap<DomainLayerKey, String>,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let mut stmt = source
        .prepare(
            "SELECT id, domain_id, from_construct_id, to_construct_id, relationship_type, \
             layer_num, cardinality, rule_type, description FROM relationships",
        )
        .map_err(ImportError::Source)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(ImportError::Source)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ImportError::Source)?;

    for (
        id,
        domain_id,
        from_construct_id,
        to_construct_id,
        relationship_type,
        layer_num,
        cardinality,
        rule_type,
        description,
    ) in rows
    {
        let Some(source_id) = source_id_by_domain_layer.get(&(domain_id.clone(), layer_num)) else {
            report.rows_skipped += 1;
            report.disclosures.push(format!(
                "relationships: skipped {id:?} -- no Source for domain {domain_id:?} layer \
                 {layer_num}"
            ));
            continue;
        };
        // A null rule_type in the source schema means "no declared
        // requirement level" -- mapped to the weakest binding strength
        // (MAY) rather than guessed as MUST/SHOULD.
        let binding_strength = match rule_type {
            None => BindingStrength::May,
            Some(s) => match BindingStrength::parse(&s) {
                Some(parsed) => parsed,
                None => {
                    report.disclosures.push(format!(
                        "relationships: {id:?} had unrecognized rule_type {s:?} -- imported as \
                         MAY rather than skipped"
                    ));
                    BindingStrength::May
                }
            },
        };
        let statement = description.unwrap_or_else(|| {
            format!("{from_construct_id} {relationship_type} {to_construct_id}")
        });

        store::insert_rule(
            dest,
            &Rule {
                id,
                source_id: source_id.clone(),
                subject_id: from_construct_id,
                related_subject_id: Some(to_construct_id),
                relationship_type: Some(relationship_type),
                cardinality,
                statement,
                machine_check: None,
                binding_strength,
                supersedes_rule_id: None,
            },
        )
        .map_err(dest_err)?;
        report.rules_imported += 1;
    }

    Ok(())
}

fn disclose_unsupported(
    _dest: &Connection,
    source: &Connection,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let conflicts_by_domain: Vec<(String, i64)> = {
        let mut stmt = source
            .prepare("SELECT domain_id, COUNT(*) FROM conflicts GROUP BY domain_id")
            .map_err(ImportError::Source)?;
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(ImportError::Source)?
        .collect::<rusqlite::Result<_>>()
        .map_err(ImportError::Source)?
    };
    for (domain_id, count) in conflicts_by_domain {
        report.disclosures.push(format!(
            "conflicts: {count} row(s) in domain {domain_id:?} not imported -- these are \
             layer-vs-layer or domain-wide observations in the old schema, never tied to two \
             specific Rule ids the way RuleRelation requires. Read them and confirm as \
             RuleRelation(conflicts_with) manually if still relevant."
        ));
    }

    for table in ["properties", "embeddings"] {
        let count: i64 = source
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(ImportError::Source)?;
        if count > 0 {
            report.disclosures.push(format!(
                "{table}: {count} row(s) not imported -- no destination table for this in the \
                 current model."
            ));
        }
    }
    report.disclosures.push(
        "ingestion_log, schema_version, knowledge_fts: not imported -- old-schema-specific \
         bookkeeping with no destination concept."
            .to_string(),
    );

    Ok(())
}

fn add_udra_data_mesh_lineage_edge(
    dest: &Connection,
    root_source_by_domain: &HashMap<String, String>,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    if let (Some(udra_root), Some(data_mesh_root)) = (
        root_source_by_domain.get("udra"),
        root_source_by_domain.get("data_mesh"),
    ) {
        store::insert_source_authority_edge(dest, udra_root, data_mesh_root)
            .map_err(ImportError::Dest)?;
        report.source_authority_edges_imported += 1;
        report.disclosures.push(
            "SourceAuthority: added an edge from udra's root Source to data_mesh's root Source \
             -- not present in source data (the old schema has no domain-lineage table), \
             inferred from udra's own domain description (\"introduces data mesh \
             principles...\"). Remove this edge if it doesn't hold for this source file."
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    /// Builds a temp SQLite file with the old `knowledge-mcp` schema (just
    /// the tables/columns this importer reads) so tests don't depend on a
    /// real checkout of that repo being present -- CI won't have one.
    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rusty_knowledge_import_v2_test_{name}.sqlite"))
    }

    fn create_fixture_schema(path: &std::path::Path) -> Connection {
        let _ = std::fs::remove_file(path);
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE domains (id TEXT PRIMARY KEY, name TEXT);
             CREATE TABLE domain_layers (
                 domain_id TEXT NOT NULL, layer_num INTEGER NOT NULL, layer_name TEXT NOT NULL,
                 authority TEXT NOT NULL, source_name TEXT, source_url TEXT, source_version TEXT,
                 owner TEXT
             );
             CREATE TABLE constructs (
                 id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, construct_type TEXT NOT NULL,
                 name TEXT NOT NULL, short_name TEXT, description TEXT,
                 is_deprecated INTEGER NOT NULL DEFAULT 0, parent_id TEXT, source_section TEXT
             );
             CREATE TABLE rules (
                 id TEXT PRIMARY KEY, construct_id TEXT NOT NULL, domain_id TEXT NOT NULL,
                 layer_num INTEGER NOT NULL, rule_type TEXT NOT NULL, rule_text TEXT NOT NULL,
                 machine_rule TEXT
             );
             CREATE TABLE relationships (
                 id TEXT PRIMARY KEY, domain_id TEXT NOT NULL, from_construct_id TEXT NOT NULL,
                 to_construct_id TEXT NOT NULL, relationship_type TEXT NOT NULL,
                 layer_num INTEGER NOT NULL, cardinality TEXT, rule_type TEXT, description TEXT
             );
             CREATE TABLE conflicts (id TEXT PRIMARY KEY, domain_id TEXT NOT NULL);
             CREATE TABLE properties (id TEXT PRIMARY KEY);
             CREATE TABLE embeddings (construct_id TEXT PRIMARY KEY);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn imports_domain_layer_chain_as_source_authority_edges() {
        let path = fixture_path("layer_chain");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority, owner)
                     VALUES ('d1', 1, 'Standard', 'normative', 'Team A');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority, owner)
                     VALUES ('d1', 2, 'Tool Guidance', 'guidance', 'Team B');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority, owner)
                     VALUES ('d1', 3, 'Org Convention', 'prescriptive', 'Team C');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.sources_imported, 3);
        assert_eq!(report.source_authority_edges_imported, 2);
        let ancestors = store::ancestors_of(&dest, "src.d1.3").unwrap();
        assert!(ancestors.contains("src.d1.2"));
        assert!(ancestors.contains("src.d1.1"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn null_short_name_falls_back_to_id_suffix() {
        let path = fixture_path("null_short_name");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority)
                     VALUES ('d1', 1, 'Standard', 'normative');
                 INSERT INTO constructs (id, domain_id, construct_type, name, short_name)
                     VALUES ('d1.some_pattern', 'd1', 'pattern', 'Some Pattern', NULL);",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        import_knowledge_mcp_db(&dest, &path).unwrap();

        let subject = store::resolve_subject(&dest, "d1", "some_pattern")
            .unwrap()
            .expect("should resolve by the derived short name");
        assert_eq!(subject.id, "d1.some_pattern");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn imports_rules_and_relationships_with_null_rule_type_as_may() {
        let path = fixture_path("rules_and_relationships");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority)
                     VALUES ('d1', 1, 'Standard', 'normative');
                 INSERT INTO constructs (id, domain_id, construct_type, name, short_name)
                     VALUES ('d1.A', 'd1', 'entity', 'A', 'A');
                 INSERT INTO constructs (id, domain_id, construct_type, name, short_name)
                     VALUES ('d1.B', 'd1', 'entity', 'B', 'B');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text)
                     VALUES ('d1.A.rule.1', 'd1.A', 'd1', 1, 'MUST', 'A must exist.');
                 INSERT INTO relationships
                     (id, domain_id, from_construct_id, to_construct_id, relationship_type,
                      layer_num, cardinality, rule_type, description)
                     VALUES ('d1.rel.1', 'd1', 'd1.A', 'd1.B', 'contains', 1, '1..*', NULL, NULL);",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.rules_imported, 2);
        let rules = store::rules_for_subject(&dest, "d1.A").unwrap();
        let relationship_rule = rules
            .iter()
            .find(|(r, _)| r.id == "d1.rel.1")
            .expect("relationship should import as a Rule");
        assert_eq!(
            relationship_rule.0.related_subject_id.as_deref(),
            Some("d1.B")
        );
        assert_eq!(
            relationship_rule.0.relationship_type.as_deref(),
            Some("contains")
        );
        assert_eq!(
            relationship_rule.0.binding_strength,
            store::BindingStrength::May
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn conflicts_are_disclosed_not_imported() {
        let path = fixture_path("conflicts");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority)
                     VALUES ('d1', 1, 'Standard', 'normative');
                 INSERT INTO conflicts (id, domain_id) VALUES ('d1.conflict.1', 'd1');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert!(
            report
                .disclosures
                .iter()
                .any(|d| d.contains("conflicts") && d.contains("not imported"))
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn adds_udra_data_mesh_lineage_edge_only_when_both_domains_present() {
        let path = fixture_path("udra_lineage");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('udra', 'UDRA');
                 INSERT INTO domains (id, name) VALUES ('data_mesh', 'Data Mesh');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority)
                     VALUES ('udra', 1, 'UDRA Standard', 'normative');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority)
                     VALUES ('data_mesh', 1, 'Data Mesh Principles', 'normative');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        let ancestors = store::ancestors_of(&dest, "src.udra.1").unwrap();
        assert!(ancestors.contains("src.data_mesh.1"));
        assert!(
            report
                .disclosures
                .iter()
                .any(|d| d.contains("udra's root Source to data_mesh's root Source"))
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn no_lineage_edge_added_when_data_mesh_domain_absent() {
        let path = fixture_path("no_data_mesh");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('udra', 'UDRA');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority)
                     VALUES ('udra', 1, 'UDRA Standard', 'normative');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        import_knowledge_mcp_db(&dest, &path).unwrap();

        let ancestors = store::ancestors_of(&dest, "src.udra.1").unwrap();
        assert!(ancestors.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn skips_and_discloses_row_with_no_matching_source() {
        let path = fixture_path("no_matching_source");
        let fixture = create_fixture_schema(&path);
        fixture
            .execute_batch(
                "INSERT INTO domains (id, name) VALUES ('d1', 'Demo');
                 INSERT INTO domain_layers (domain_id, layer_num, layer_name, authority)
                     VALUES ('d1', 1, 'Standard', 'normative');
                 INSERT INTO constructs (id, domain_id, construct_type, name, short_name)
                     VALUES ('d1.A', 'd1', 'entity', 'A', 'A');
                 INSERT INTO rules (id, construct_id, domain_id, layer_num, rule_type, rule_text)
                     VALUES ('d1.A.rule.1', 'd1.A', 'd1', 2, 'MUST', 'Orphaned at layer 2.');",
            )
            .unwrap();
        drop(fixture);

        let dest = store::open_store().unwrap();
        let report = import_knowledge_mcp_db(&dest, &path).unwrap();

        assert_eq!(report.rows_skipped, 1);
        assert!(report.disclosures.iter().any(|d| d.contains("d1.A.rule.1")));

        let _ = std::fs::remove_file(&path);
    }
}
