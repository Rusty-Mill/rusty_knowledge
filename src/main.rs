//! Rusty Knowledge — an MCP server (via `rmcp`, stdio transport) over the
//! knowledge-model-v2 store (`Source`/`SourceAuthority`/`Subject`/`Rule`/
//! `RuleRelation` -- see `store`'s module doc for the two tables the
//! fuller design specifies but this doesn't implement yet).
//!
//! This is a vertical slice proving the redesigned model end-to-end, not
//! a full port of the previous 15-tool surface. That surface was built
//! around the model this replaces (`AuthorityLayer`/`Construct`/a fixed
//! 4-layer taxonomy) and is deferred to follow-up work, not silently
//! dropped.
//!
//! Setting `KNOWLEDGE_MCP_IMPORT_PATH` at startup imports a real
//! `knowledge-mcp` SQLite file (see `knowledge_mcp_import_v2`'s module
//! doc for exactly what does and doesn't translate) instead of the small
//! hand-seeded illustrative UDRA dataset (`store::seed_udra`) -- the two
//! aren't run together, since the reference data's `udra` domain and the
//! hand-seeded one use overlapping ids (e.g. both define
//! `udra.DataProduct`). Omit it and nothing changes from the hand-seeded
//! default.
//!
//! Two tools are wired end-to-end:
//! - `lookup_subject` — everything a Subject's authority chain says about
//!   it, across every Source that makes a claim, with provenance.
//! - `crosscut_conflicts` — confirmed conflicts plus unconfirmed
//!   candidates needing review, via the two-tier conflict gate (exact
//!   `subject_id` correlation catches sibling/cousin conflicts a pure
//!   ancestor-chain walk can't see; `DELEGATED` parent/fulfilling-child
//!   pairs are excluded from the review queue, since that's the
//!   authority working as intended, not an ambiguity).

mod knowledge_mcp_import_v2;
mod store;

use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use store::Source;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SubjectLookupParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// Short name (e.g. "DataProduct") or full subject ID.
    subject_ref: String,
}

#[derive(Clone)]
struct KnowledgeServer {
    conn: Arc<Mutex<Connection>>,
}

fn format_source(source: &Source) -> String {
    match &source.steward {
        Some(steward) => format!("{} [{}] (steward: {})", source.name, source.kind, steward),
        None => format!("{} [{}]", source.name, source.kind),
    }
}

#[tool_router(server_handler)]
impl KnowledgeServer {
    #[tool(
        description = "Look up everything the full authority chain says about a subject -- every Rule that names it (directly, or as the target of a relationship claim), each labeled with the Source that issued it. Resolves subject_ref by short name first, then falls back to a direct ID match within the domain."
    )]
    fn lookup_subject(
        &self,
        Parameters(SubjectLookupParams {
            domain_tag,
            subject_ref,
        }): Parameters<SubjectLookupParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let subject = match store::resolve_subject(&conn, &domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules = match store::rules_for_subject(&conn, &subject.id) {
            Ok(rules) => rules,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules_block = if rules.is_empty() {
            "(no rules)".to_string()
        } else {
            rules
                .iter()
                .map(|(rule, source)| {
                    let relation = match (&rule.related_subject_id, &rule.relationship_type) {
                        (Some(target), Some(rel_type)) => {
                            format!(" [{rel_type} -> {target}]")
                        }
                        _ => String::new(),
                    };
                    let check = if rule.machine_check.is_some() {
                        " (machine-checkable)"
                    } else {
                        ""
                    };
                    format!(
                        "  [{}] {} -- {}{relation}{check}",
                        rule.binding_strength.as_str(),
                        rule.statement,
                        format_source(source)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let section_note = match &subject.source_section {
            Some(section) => format!(" (source: Section {section})"),
            None => String::new(),
        };
        format!(
            "{} ({}) [{}]{section_note}\n{}\nRules:\n{rules_block}",
            subject.name,
            subject.id,
            subject.subject_type,
            subject.description.as_deref().unwrap_or("(no description)")
        )
    }

    #[tool(
        description = "Show conflict-registry status for a subject: confirmed, active conflicts_with relations between its rules, plus candidate pairs (same subject, different Sources, no confirmed relation yet) that need human review. A DELEGATED parent rule and a descendant Source's fulfilling rule are not surfaced as a candidate -- that's the authority working as intended, not an ambiguity."
    )]
    fn crosscut_conflicts(
        &self,
        Parameters(SubjectLookupParams {
            domain_tag,
            subject_ref,
        }): Parameters<SubjectLookupParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let subject = match store::resolve_subject(&conn, &domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let confirmed = match store::confirmed_conflicts_for_subject(&conn, &subject.id) {
            Ok(confirmed) => confirmed,
            Err(err) => return format!("Conflict lookup failed: {err}"),
        };
        let candidates = match store::conflict_candidates_for_subject(&conn, &subject.id) {
            Ok(candidates) => candidates,
            Err(err) => return format!("Conflict lookup failed: {err}"),
        };

        let confirmed_block = if confirmed.is_empty() {
            "(none)".to_string()
        } else {
            confirmed
                .iter()
                .map(|(a, b, relation)| {
                    format!(
                        "  {} vs {} -- confirmed by {}",
                        a.id, b.id, relation.confirmed_by
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let candidates_block = if candidates.is_empty() {
            "(none)".to_string()
        } else {
            candidates
                .iter()
                .map(|(a, b)| {
                    format!(
                        "  {} ({}) vs {} ({}) -- no relation confirmed yet",
                        a.id, a.source_id, b.id, b.source_id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "Conflicts for {} ({}):\nConfirmed:\n{confirmed_block}\nNeeds review:\n{candidates_block}",
            subject.name, subject.id
        )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let conn = store::open_store()?;

    match std::env::var("KNOWLEDGE_MCP_IMPORT_PATH") {
        Ok(path) => {
            match knowledge_mcp_import_v2::import_knowledge_mcp_db(
                &conn,
                std::path::Path::new(&path),
            ) {
                Ok(report) => {
                    eprintln!(
                        "Imported {path:?}: {} source(s), {} authority edge(s), {} subject(s), \
                         {} rule(s), {} row(s) skipped.",
                        report.sources_imported,
                        report.source_authority_edges_imported,
                        report.subjects_imported,
                        report.rules_imported,
                        report.rows_skipped,
                    );
                    for disclosure in &report.disclosures {
                        eprintln!("  {disclosure}");
                    }
                }
                Err(err) => {
                    eprintln!(
                        "Failed to import {path:?} ({err}); falling back to seeded UDRA data."
                    );
                    store::seed_udra(&conn).map_err(|err| anyhow::anyhow!(err))?;
                }
            }
        }
        Err(_) => {
            store::seed_udra(&conn).map_err(|err| anyhow::anyhow!(err))?;
        }
    }

    eprintln!(
        "rusty-knowledge MCP server starting on stdio (tools: lookup_subject, crosscut_conflicts; \
         knowledge-model-v2 vertical slice)"
    );

    let server = KnowledgeServer {
        conn: Arc::new(Mutex::new(conn)),
    };
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_server() -> KnowledgeServer {
        let conn = store::open_store().unwrap();
        store::seed_udra(&conn).unwrap();
        KnowledgeServer {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    #[test]
    fn lookup_subject_resolves_by_short_name_and_lists_rules_from_the_whole_chain() {
        let server = test_server();
        let response = server.lookup_subject(Parameters(SubjectLookupParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
        }));
        assert!(response.contains("Data Product"));
        assert!(response.contains("Data Mesh Principles"));
        assert!(response.contains("Army Unified Data Reference Architecture"));
        assert!(response.contains("Our Org's UDRA Implementation"));
    }

    #[test]
    fn lookup_subject_marks_machine_checkable_rules() {
        let server = test_server();
        let response = server.lookup_subject(Parameters(SubjectLookupParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
        }));
        assert!(response.contains("(machine-checkable)"));
    }

    #[test]
    fn lookup_subject_unknown_ref_reports_not_found() {
        let server = test_server();
        let response = server.lookup_subject(Parameters(SubjectLookupParams {
            domain_tag: "udra".into(),
            subject_ref: "NoSuchSubject".into(),
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn crosscut_conflicts_surfaces_sibling_disagreement_as_needing_review() {
        let server = test_server();
        let response = server.crosscut_conflicts(Parameters(SubjectLookupParams {
            domain_tag: "udra".into(),
            subject_ref: "DataContract".into(),
        }));
        assert!(response.contains("rule.suborg-a.001"));
        assert!(response.contains("rule.suborg-b.001"));
        assert!(response.contains("Needs review"));
    }

    #[test]
    fn crosscut_conflicts_does_not_flag_delegated_fulfillment() {
        let server = test_server();
        let response = server.crosscut_conflicts(Parameters(SubjectLookupParams {
            domain_tag: "udra".into(),
            subject_ref: "DataContract".into(),
        }));
        assert!(!response.contains("rule.org.002 (src.org-udra-impl) vs rule.suborg-a.001"));
        assert!(!response.contains("rule.suborg-a.001 (src.suborg-a-impl) vs rule.org.002"));
    }
}
