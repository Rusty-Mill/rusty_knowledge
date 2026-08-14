//! Rusty Knowledge — an MCP server (via `rmcp`, stdio transport) over the
//! knowledge-model-v2 store (`Source`/`SourceAuthority`/`Subject`/`Rule`/
//! `RuleRelation`/`SelectionGroup`/`RuleDerivation` -- every table the
//! fuller seven-table design specifies; see `store`'s module doc for the
//! full account).
//!
//! `KNOWLEDGE_DB_PATH` set at startup opens a file-backed store instead of
//! the default in-memory one (see `store::open_store_at`); either way,
//! `KNOWLEDGE_MCP_IMPORT_PATH` set imports a real `knowledge-mcp` SQLite
//! file (see `knowledge_mcp_import_v2`'s module doc for exactly what does
//! and doesn't translate) instead of the small hand-seeded illustrative
//! UDRA dataset (`store::seed_udra`) -- the two aren't run together, since
//! the reference data's `udra` domain and the hand-seeded one use
//! overlapping ids (e.g. both define `udra.DataProduct`). Neither runs at
//! all against a store that already has data from a previous run
//! (`store::is_empty`). Omit both env vars and nothing changes from the
//! in-memory hand-seeded default.
//!
//! All 16 tools tracked by rusty_knowledge#55 are wired end-to-end, plus
//! `lookup_derived_summary` (below).
//! - `lookup_subject` — everything a Subject's authority chain says about
//!   it, across every Source that makes a claim, with provenance.
//! - `lookup_rules` — plain statement rules for a subject (excludes
//!   relationship claims), optionally filtered by binding strength.
//! - `lookup_relationships` — outgoing relationship claims from a
//!   subject, optionally filtered by relationship type.
//! - `lookup_valid_relationships` — declared relationship types between
//!   two subject_types within a domain.
//! - `lookup_domain_summary` — subject counts (overall and by type) and
//!   root Source(s) for a domain.
//! - `search_constructs` — list/filter subjects within a domain by type.
//! - `meta_list_domains` — every domain tag in use, with counts and
//!   root Sources.
//! - `meta_routing_guide` — query routing guidance.
//! - `crosscut_traceability` — outgoing/incoming `traces_to` relationship
//!   claims for a subject.
//! - `crosscut_cross_domain` — relationship claims whose target sits in a
//!   different domain.
//! - `crosscut_valid_relationship_candidates` — suggests candidate
//!   valid-relationship rules from existing relationship instances, for a
//!   human to review (no `knowledge-mcp` equivalent; never auto-commits).
//! - `validate_relationship` — whether a relationship between two
//!   subjects is declared by an existing rule.
//! - `validate_element` — PASS/FAIL/WARNING per machine-checkable rule
//!   against a set of real property values (`Rule.machine_check`, e.g.
//!   `required_property`/`enum_value`/`pattern`/`range`/`custom`). A
//!   missing property always fails; a pattern *mismatch* (value present,
//!   just doesn't match) is a warning, not a fail -- pattern/format
//!   guidance is advisory, structural presence isn't.
//! - `crosscut_conflicts` — confirmed conflicts plus unconfirmed
//!   candidates needing review, via the two-tier conflict gate (exact
//!   `subject_id` correlation catches sibling/cousin conflicts a pure
//!   ancestor-chain walk can't see; `DELEGATED` parent/fulfilling-child
//!   pairs are excluded from the review queue, since that's the
//!   authority working as intended, not an ambiguity).
//! - `validate_completeness` — evaluates every `SelectionGroup` (a
//!   cardinality constraint over a set of relationship-shaped rules, e.g.
//!   "must have both X and Y") defined on a container/viewpoint subject
//!   against a caller-supplied set of element types actually present,
//!   reporting which groups are satisfied, which member rules are
//!   missing, and the overall verdict.
//! - `search_knowledge` — hybrid search over every `Rule.statement` and
//!   `Subject.name`/`short_name`/`description`, fusing lexical (FTS5
//!   `bm25`) and vector (`store::HashingEmbedder` cosine similarity)
//!   signals in equal weight. Always declares its retrieval mode
//!   honestly: the vector half is a zero-dependency, zero-network
//!   "hashing trick" bag-of-words embedding, not a trained semantic
//!   model -- see `store::RETRIEVAL_MODE_DESCRIPTION`.
//! - `lookup_derived_summary` — any `RuleDerivation` rollups recorded for
//!   a subject, always labeled NON-AUTHORITATIVE and listing exactly
//!   which Rules each one was synthesized from. Firewalled from
//!   authority: never returned by `lookup_subject`/`lookup_rules`, never
//!   indexed for `search_knowledge`, never a `RuleRelation` participant.

mod knowledge_mcp_import_v2;
mod store;

use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use store::Source;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SubjectLookupParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// Short name (e.g. "DataProduct") or full subject ID.
    subject_ref: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct DomainParams {
    /// Domain tag, e.g. "udra".
    domain_tag: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchConstructsParams {
    /// Domain tag to search within, e.g. "udra".
    domain_tag: String,
    /// Restrict to one subject_type (e.g. "concept"). Omit for all types.
    #[serde(default)]
    subject_type: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LookupRulesParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// Short name or full subject ID.
    subject_ref: String,
    /// Restrict to one binding strength (MUST/MUST_NOT/SHOULD/SHOULD_NOT/
    /// MAY/DELEGATED). Omit for all. An unrecognized value is reported as
    /// an error, not silently ignored.
    #[serde(default)]
    binding_strength: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LookupRelationshipsParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// Source subject of the relationship (short name or full ID).
    subject_ref: String,
    /// Restrict to one relationship_type (e.g. "contains"). Omit for all
    /// outgoing relationships.
    #[serde(default)]
    relationship_type: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidRelationshipsParams {
    /// Domain tag, e.g. "udra".
    domain_tag: String,
    /// Source subject_type, e.g. "concept".
    from_type: String,
    /// Target subject_type, e.g. "concept".
    to_type: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TraceabilityParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// Short name or full subject ID.
    subject_ref: String,
    /// Include SHOULD traces, not just MUST.
    #[serde(default)]
    include_optional: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct CrossDomainParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// Short name or full subject ID.
    subject_ref: String,
    /// Restrict results to one target domain. Omit for all domains.
    #[serde(default)]
    to_domain_tag: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidRelationshipCandidatesParams {
    /// Domain tag to derive candidates for, e.g. "udra".
    domain_tag: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidateRelationshipParams {
    /// Domain tag both subjects belong to, e.g. "udra".
    domain_tag: String,
    /// Source subject (short name or full ID).
    from_subject_ref: String,
    /// Target subject (short name or full ID).
    to_subject_ref: String,
    /// The relationship type to validate, e.g. "exposes".
    relationship_type: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidateElementParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// The construct type being validated (short name or full ID).
    subject_ref: String,
    /// Property name -> value pairs of the actual element being checked.
    #[serde(default)]
    properties: HashMap<String, String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidateCompletenessParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// The container/viewpoint subject being validated (short name or
    /// full ID), e.g. "DataProduct".
    subject_ref: String,
    /// The subject IDs or short names actually present in the model being
    /// checked, e.g. ["DataContract"].
    present_element_types: Vec<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchKnowledgeParams {
    /// Keyword search query. Whitespace-separated terms are OR-matched
    /// (each term treated as literal text, never as FTS query syntax).
    query: String,
    /// Restrict results to one domain tag, e.g. "udra". Omit to search
    /// every domain.
    #[serde(default)]
    domain_tag: Option<String>,
    /// Max results to return. Omit for the default of 10.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LookupDerivedSummaryParams {
    /// Domain tag the subject belongs to, e.g. "udra".
    domain_tag: String,
    /// Short name or full subject ID.
    subject_ref: String,
}

#[derive(Clone)]
struct KnowledgeServer {
    store: Arc<Mutex<dyn store::Store + Send>>,
}

fn format_source(source: &Source) -> String {
    match &source.steward {
        Some(steward) => format!("{} [{}] (steward: {})", source.name, source.kind, steward),
        None => format!("{} [{}]", source.name, source.kind),
    }
}

/// Shared by every tool that accepts an optional binding-strength filter
/// string: `Ok(None)` for "no filter", `Ok(Some(_))` for a recognized
/// value, `Err` (never silently ignored) for anything else.
fn parse_binding_strength_filter(
    value: &Option<String>,
) -> Result<Option<store::BindingStrength>, String> {
    match value.as_deref().map(store::BindingStrength::parse) {
        Some(None) => Err(format!(
            "{:?} is not a known binding strength (expected one of MUST, MUST_NOT, SHOULD, \
             SHOULD_NOT, MAY, DELEGATED).",
            value.as_ref().unwrap()
        )),
        Some(Some(parsed)) => Ok(Some(parsed)),
        None => Ok(None),
    }
}

fn routing_guide() -> String {
    "Routing guidance:\n\
     - \"What does X mean?\" -> lookup_subject\n\
     - \"What are the rules for X?\" -> lookup_rules (optionally filter by binding_strength)\n\
     - \"What does X relate to / contain / extend?\" -> lookup_relationships\n\
     - \"What relationship types are valid between type A and type B?\" -> \
       lookup_valid_relationships\n\
     - \"I can't find the right subject\" -> search_constructs\n\
     - \"What domains are loaded?\" -> meta_list_domains\n\
     - \"Give me an overview of domain X\" -> lookup_domain_summary\n\
     - \"Does X trace to Y?\" -> crosscut_traceability\n\
     - \"How does X relate to a subject in another domain?\" -> crosscut_cross_domain\n\
     - \"What valid-relationship rules should I declare for this domain?\" -> \
       crosscut_valid_relationship_candidates\n\
     - \"Is this relationship permitted?\" -> validate_relationship\n\
     - \"Is X valid/conformant?\" -> validate_element\n\
     - \"Is this model/container complete? What's missing?\" -> validate_completeness\n\
     - \"Where do sources disagree about X?\" -> crosscut_conflicts\n\
     - \"Search for X across everything\" -> search_knowledge (hybrid: lexical + hashing-vector)\n\
     - \"Give me a quick summary of X\" -> lookup_derived_summary (NON-AUTHORITATIVE -- \
       still verify against lookup_subject)"
        .to_string()
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
        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules = match db.rules_for_subject(&subject.id) {
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
        description = "Get plain statement rules for a subject, optionally filtered by binding strength (MUST/MUST_NOT/SHOULD/SHOULD_NOT/MAY/DELEGATED). Excludes relationship claims -- see lookup_relationships for those."
    )]
    fn lookup_rules(
        &self,
        Parameters(LookupRulesParams {
            domain_tag,
            subject_ref,
            binding_strength,
        }): Parameters<LookupRulesParams>,
    ) -> String {
        let filter = match parse_binding_strength_filter(&binding_strength) {
            Ok(filter) => filter,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules = match db.statement_rules_for_subject(&subject.id, filter) {
            Ok(rules) => rules,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if rules.is_empty() {
            return format!("No rules found for {} ({}).", subject.name, subject.id);
        }
        rules
            .iter()
            .map(|(rule, source)| {
                format!(
                    "[{}] {} -- {}",
                    rule.binding_strength.as_str(),
                    rule.statement,
                    format_source(source)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tool(
        description = "Get outgoing relationship claims from a subject (e.g. what it contains, extends, or must trace to), optionally filtered by relationship_type."
    )]
    fn lookup_relationships(
        &self,
        Parameters(LookupRelationshipsParams {
            domain_tag,
            subject_ref,
            relationship_type,
        }): Parameters<LookupRelationshipsParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules = match db.outgoing_relationships(&subject.id, relationship_type.as_deref()) {
            Ok(rules) => rules,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if rules.is_empty() {
            return format!(
                "No outgoing relationships found for {} ({}).",
                subject.name, subject.id
            );
        }
        rules
            .iter()
            .map(|(rule, source)| {
                let cardinality = rule.cardinality.as_deref().unwrap_or("(no cardinality)");
                format!(
                    "[{}] {} --{}--> {} ({}) -- {}",
                    rule.binding_strength.as_str(),
                    subject.id,
                    rule.relationship_type.as_deref().unwrap_or("relates_to"),
                    rule.related_subject_id.as_deref().unwrap_or("?"),
                    cardinality,
                    format_source(source)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tool(
        description = "Get a summary of a domain: subject counts (overall and by subject_type) and which Source(s) root it."
    )]
    fn lookup_domain_summary(
        &self,
        Parameters(DomainParams { domain_tag }): Parameters<DomainParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let summary = match db.domain_summary(&domain_tag) {
            Ok(summary) => summary,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if summary.subject_count == 0 && summary.root_sources.is_empty() {
            return format!("Domain {domain_tag:?} not found.");
        }

        let by_type = summary
            .subject_count_by_type
            .iter()
            .map(|(subject_type, count): &(String, i64)| format!("  {subject_type}: {count}"))
            .collect::<Vec<_>>()
            .join("\n");
        let roots = if summary.root_sources.is_empty() {
            "  (none recorded)".to_string()
        } else {
            summary
                .root_sources
                .iter()
                .map(|source| format!("  {}", format_source(source)))
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "{} -- {} subject(s) total\nBy type:\n{by_type}\nRooted by:\n{roots}",
            summary.domain_tag, summary.subject_count
        )
    }

    #[tool(description = "List and filter subjects within a domain by subject_type.")]
    fn search_constructs(
        &self,
        Parameters(SearchConstructsParams {
            domain_tag,
            subject_type,
        }): Parameters<SearchConstructsParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let subjects = match db.subjects_in_domain(&domain_tag, subject_type.as_deref()) {
            Ok(subjects) => subjects,
            Err(err) => return format!("Search failed: {err}"),
        };

        if subjects.is_empty() {
            return format!("No subjects found in domain {domain_tag:?}.");
        }
        subjects
            .iter()
            .map(|subject| {
                format!(
                    "{} ({}) [{}]",
                    subject.name, subject.id, subject.subject_type
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tool(
        description = "List every domain tag in use, with its subject count and the Source(s) that root it."
    )]
    fn meta_list_domains(&self) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let domains = match db.list_domains() {
            Ok(domains) => domains,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if domains.is_empty() {
            return "(no domains loaded)".to_string();
        }
        domains
            .iter()
            .map(|domain| {
                let roots = if domain.root_sources.is_empty() {
                    "(no root Source recorded)".to_string()
                } else {
                    domain
                        .root_sources
                        .iter()
                        .map(|source| source.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                format!(
                    "{} -- {} subject(s), rooted by: {}",
                    domain.domain_tag, domain.subject_count, roots
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tool(
        description = "Query routing guidance -- which tools to use for which question types. Call this when unsure how to decompose a task."
    )]
    fn meta_routing_guide(&self) -> String {
        routing_guide()
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
        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let confirmed = match db.confirmed_conflicts_for_subject(&subject.id) {
            Ok(confirmed) => confirmed,
            Err(err) => return format!("Conflict lookup failed: {err}"),
        };
        let candidates = match db.conflict_candidates_for_subject(&subject.id) {
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

    #[tool(
        description = "Given two subject_types within a domain, list every declared relationship_type between them (with cardinality and binding strength), derived from existing relationship-shaped rules."
    )]
    fn lookup_valid_relationships(
        &self,
        Parameters(ValidRelationshipsParams {
            domain_tag,
            from_type,
            to_type,
        }): Parameters<ValidRelationshipsParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let rules = match db.valid_relationship_types(&domain_tag, &from_type, &to_type) {
            Ok(rules) => rules,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if rules.is_empty() {
            return format!(
                "No declared relationship types found from {from_type:?} to {to_type:?} in \
                 domain {domain_tag:?}."
            );
        }
        rules
            .iter()
            .map(|(rule, source)| {
                let cardinality = rule.cardinality.as_deref().unwrap_or("(no cardinality)");
                format!(
                    "[{}] {} ({}) -- {}",
                    rule.binding_strength.as_str(),
                    rule.relationship_type.as_deref().unwrap_or("?"),
                    cardinality,
                    format_source(source)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tool(
        description = "Get traceability for a subject -- what it must (and optionally should) trace to, and what must (and optionally should) trace to it, via relationship_type=\"traces_to\" rules."
    )]
    fn crosscut_traceability(
        &self,
        Parameters(TraceabilityParams {
            domain_tag,
            subject_ref,
            include_optional,
        }): Parameters<TraceabilityParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let (outgoing, incoming) = match db.traceability(&subject.id, include_optional) {
            Ok(result) => result,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let outgoing_block = if outgoing.is_empty() {
            "(none)".to_string()
        } else {
            outgoing
                .iter()
                .map(|(rule, source)| {
                    format!(
                        "  [{}] -> {} -- {}",
                        rule.binding_strength.as_str(),
                        rule.related_subject_id.as_deref().unwrap_or("?"),
                        format_source(source)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let incoming_block = if incoming.is_empty() {
            "(none)".to_string()
        } else {
            incoming
                .iter()
                .map(|(rule, source)| {
                    format!(
                        "  [{}] <- {} -- {}",
                        rule.binding_strength.as_str(),
                        rule.subject_id,
                        format_source(source)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "Traceability for {} ({}):\nMust trace to:\n{outgoing_block}\nMust be traced from:\n{incoming_block}",
            subject.name, subject.id
        )
    }

    #[tool(
        description = "Find cross-domain relationships from a subject -- relationship claims whose target subject sits in a different domain, optionally narrowed to one to_domain_tag."
    )]
    fn crosscut_cross_domain(
        &self,
        Parameters(CrossDomainParams {
            domain_tag,
            subject_ref,
            to_domain_tag,
        }): Parameters<CrossDomainParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let relationships =
            match db.cross_domain_relationships(&subject.id, to_domain_tag.as_deref()) {
                Ok(relationships) => relationships,
                Err(err) => return format!("Lookup failed: {err}"),
            };

        if relationships.is_empty() {
            return format!(
                "No cross-domain relationships found for {} ({}).",
                subject.name, subject.id
            );
        }
        relationships
            .iter()
            .map(|(rule, related, source)| {
                format!(
                    "[{}] {} --{}--> {} ({}) -- {}",
                    rule.binding_strength.as_str(),
                    subject.id,
                    rule.relationship_type.as_deref().unwrap_or("relates_to"),
                    related.id,
                    related.domain_tag,
                    format_source(source)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tool(
        description = "Suggest candidate valid-relationship rules for a domain, derived from existing relationship-shaped rules grouped by (from_type, to_type, relationship_type). Never auto-committed -- a human decides whether to declare these."
    )]
    fn crosscut_valid_relationship_candidates(
        &self,
        Parameters(ValidRelationshipCandidatesParams { domain_tag }): Parameters<
            ValidRelationshipCandidatesParams,
        >,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let candidates = match db.candidate_valid_relationships(&domain_tag) {
            Ok(candidates) => candidates,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if candidates.is_empty() {
            return format!(
                "No relationship-shaped rules found in domain {domain_tag:?} to derive \
                 candidates from."
            );
        }
        candidates
            .iter()
            .map(|candidate| {
                let disagreement = if candidate.other_cardinalities_seen.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (other cardinalities seen: {})",
                        candidate.other_cardinalities_seen.join(", ")
                    )
                };
                format!(
                    "{} --{}--> {} : cardinality {}{disagreement} ({} instance(s))",
                    candidate.from_type,
                    candidate.relationship_type,
                    candidate.to_type,
                    candidate
                        .majority_cardinality
                        .as_deref()
                        .unwrap_or("(none declared)"),
                    candidate.instance_count
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tool(
        description = "Check whether a relationship between two subjects is declared by an existing rule -- VALID with the matching rule(s), or INVALID if no such declaration exists."
    )]
    fn validate_relationship(
        &self,
        Parameters(ValidateRelationshipParams {
            domain_tag,
            from_subject_ref,
            to_subject_ref,
            relationship_type,
        }): Parameters<ValidateRelationshipParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let from_subject = match db.resolve_subject(&domain_tag, &from_subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {from_subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };
        let to_subject = match db.resolve_subject(&domain_tag, &to_subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {to_subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let matches =
            match db.validate_relationship(&from_subject.id, &to_subject.id, &relationship_type) {
                Ok(matches) => matches,
                Err(err) => return format!("Validation failed: {err}"),
            };

        if matches.is_empty() {
            return format!(
                "INVALID: no rule declares {} --{}--> {}.",
                from_subject.id, relationship_type, to_subject.id
            );
        }
        let rules_block = matches
            .iter()
            .map(|(rule, source)| {
                format!(
                    "  [{}] {} -- {}",
                    rule.binding_strength.as_str(),
                    rule.statement,
                    format_source(source)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "VALID: {} --{}--> {}\n{rules_block}",
            from_subject.id, relationship_type, to_subject.id
        )
    }

    #[tool(
        description = "Validate an element's properties against a subject's machine-checkable rules. Returns PASS/FAIL/WARNING per rule with citations -- rules with no machine_check are listed as not machine-checkable, not silently skipped."
    )]
    fn validate_element(
        &self,
        Parameters(ValidateElementParams {
            domain_tag,
            subject_ref,
            properties,
        }): Parameters<ValidateElementParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules = match db.statement_rules_for_subject(&subject.id, None) {
            Ok(rules) => rules,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let checkable: Vec<_> = rules
            .iter()
            .filter(|(rule, _)| rule.machine_check.is_some())
            .collect();
        if checkable.is_empty() {
            return format!(
                "{} ({}) has no machine-checkable rules; nothing to validate.",
                subject.name, subject.id
            );
        }

        let mut fail_count = 0;
        let mut warning_count = 0;
        let findings = checkable
            .iter()
            .map(|(rule, source)| {
                let check_json = rule.machine_check.as_deref().unwrap();
                let result = store::evaluate_machine_check(check_json, &properties);
                let (label, detail) = match &result {
                    store::CheckResult::Pass => ("PASS", None),
                    store::CheckResult::Fail(reason) => {
                        fail_count += 1;
                        ("FAIL", Some(reason.clone()))
                    }
                    store::CheckResult::Warning(reason) => {
                        warning_count += 1;
                        ("WARNING", Some(reason.clone()))
                    }
                };
                match detail {
                    Some(detail) => format!(
                        "  [{label}] {} -- {detail} ({})",
                        rule.statement,
                        format_source(source)
                    ),
                    None => format!(
                        "  [{label}] {} -- ({})",
                        rule.statement,
                        format_source(source)
                    ),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let overall = if fail_count > 0 {
            "FAIL"
        } else if warning_count > 0 {
            "WARNING"
        } else {
            "PASS"
        };

        format!(
            "{} ({}): {overall} ({fail_count} fail, {warning_count} warning)\n{findings}",
            subject.name, subject.id
        )
    }

    #[tool(
        description = "Check whether a container/viewpoint subject is complete, given the element types actually present in the model. Evaluates every SelectionGroup defined on the subject (a cardinality constraint over a set of relationship-shaped rules, e.g. \"must have both a DataContract and realize DataProduct\") and reports which are satisfied, which are missing, and the overall verdict."
    )]
    fn validate_completeness(
        &self,
        Parameters(ValidateCompletenessParams {
            domain_tag,
            subject_ref,
            present_element_types,
        }): Parameters<ValidateCompletenessParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let present: HashSet<String> = present_element_types.into_iter().collect();
        let findings = match db.evaluate_completeness(&subject.id, &present) {
            Ok(findings) => findings,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if findings.is_empty() {
            return format!(
                "{} ({}) has no completeness constraints defined; nothing to validate.",
                subject.name, subject.id
            );
        }

        let mut findings_report = String::new();
        for finding in &findings {
            let group_label = if finding.is_satisfied {
                "COMPLETE"
            } else {
                "INCOMPLETE"
            };
            findings_report.push_str(&format!(
                "  [{group_label}] {} ({}/{} satisfied)\n",
                finding.group.description,
                finding.satisfied_count,
                finding.members.len()
            ));
            for (rule, source, satisfied) in &finding.members {
                let member_label = if *satisfied { "present" } else { "MISSING" };
                findings_report.push_str(&format!(
                    "    [{member_label}] {} -- ({})\n",
                    rule.statement,
                    format_source(source)
                ));
            }
        }

        let overall = if findings.iter().all(|f| f.is_satisfied) {
            "COMPLETE"
        } else {
            "INCOMPLETE"
        };
        format!(
            "{} ({}): {overall}\n{}",
            subject.name,
            subject.id,
            findings_report.trim_end()
        )
    }

    #[tool(
        description = "Hybrid search over every Rule statement and Subject name/short_name/description, fusing lexical (FTS5) and vector (local hashing-trick embedding, not a trained semantic model) signals. Always declares its retrieval mode honestly and returns ranked hits identified well enough to look each one up via lookup_subject."
    )]
    fn search_knowledge(
        &self,
        Parameters(SearchKnowledgeParams {
            query,
            domain_tag,
            limit,
        }): Parameters<SearchKnowledgeParams>,
    ) -> String {
        if query.trim().is_empty() {
            return "Query is empty; nothing to search.".to_string();
        }
        let db = self.store.lock().expect("store mutex poisoned");
        let results = match db.search_knowledge(&query, domain_tag.as_deref(), limit.unwrap_or(10))
        {
            Ok(results) => results,
            Err(err) => return format!("Search failed: {err}"),
        };

        if results.is_empty() {
            return format!(
                "No results for {query:?} (retrieval mode: {}).",
                store::RETRIEVAL_MODE_DESCRIPTION
            );
        }

        let lines = results
            .iter()
            .map(|r| {
                let kind = match r.ref_type {
                    store::SearchRefType::Rule => "rule",
                    store::SearchRefType::Subject => "subject",
                };
                format!(
                    "  [{kind}] {} ({}) -- {} (score {:.3})",
                    r.ref_id, r.domain_tag, r.text, r.score
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "{} result(s) for {query:?} (retrieval mode: {}; higher score = more relevant):\n{lines}",
            results.len(),
            store::RETRIEVAL_MODE_DESCRIPTION
        )
    }

    #[tool(
        description = "Look up any synthesized, non-authoritative rollup summaries recorded for a subject (RuleDerivation). Every response is explicitly labeled NON-AUTHORITATIVE and lists the Rules it was derived from -- use lookup_subject for ground truth, this tool only for a quick orientation summary."
    )]
    fn lookup_derived_summary(
        &self,
        Parameters(LookupDerivedSummaryParams {
            domain_tag,
            subject_ref,
        }): Parameters<LookupDerivedSummaryParams>,
    ) -> String {
        let db = self.store.lock().expect("store mutex poisoned");
        let subject = match db.resolve_subject(&domain_tag, &subject_ref) {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                return format!("Subject {subject_ref:?} not found in domain {domain_tag:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let derivations = match db.rule_derivations_for_subject(&subject.id) {
            Ok(derivations) => derivations,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if derivations.is_empty() {
            return format!(
                "{} ({}) has no recorded derived summaries.",
                subject.name, subject.id
            );
        }

        let body = derivations
            .iter()
            .map(|d| {
                format!(
                    "  [NON-AUTHORITATIVE] {}: {}\n    Derived from: {}",
                    d.label,
                    d.summary,
                    d.source_rule_ids.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "{} ({}) -- {} derived summary/summaries (NON-AUTHORITATIVE -- refer to \
             lookup_subject for ground truth):\n{body}",
            subject.name,
            subject.id,
            derivations.len()
        )
    }
}

/// `KNOWLEDGE_DB_PATH` set -> file-backed store at that path (created if
/// it doesn't exist; reopening an existing file is safe, since
/// `open_store_at`'s schema DDL is idempotent). Unset -> in-memory,
/// exactly the previous behavior.
fn open_store() -> rusqlite::Result<Connection> {
    match std::env::var("KNOWLEDGE_DB_PATH") {
        Ok(path) => {
            let conn = store::open_store_at(std::path::Path::new(&path))?;
            eprintln!("Using file-backed store at {path:?}.");
            Ok(conn)
        }
        Err(_) => store::open_store(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let conn = open_store()?;

    if store::is_empty(&conn)? {
        match std::env::var("KNOWLEDGE_MCP_IMPORT_PATH") {
            Ok(path) => {
                match knowledge_mcp_import_v2::import_knowledge_mcp_db(
                    &conn,
                    std::path::Path::new(&path),
                ) {
                    Ok(report) => {
                        eprintln!(
                            "Imported {path:?}: {} source(s), {} authority edge(s), {} \
                             subject(s), {} rule(s), {} row(s) skipped.",
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
    } else {
        eprintln!(
            "Store already has data (file-backed persistence carried it over from a \
             previous run); skipping seed/import."
        );
    }

    eprintln!(
        "rusty-knowledge MCP server starting on stdio (tools: lookup_subject, lookup_rules, \
         lookup_relationships, lookup_valid_relationships, lookup_domain_summary, \
         search_constructs, meta_list_domains, meta_routing_guide, crosscut_traceability, \
         crosscut_cross_domain, crosscut_valid_relationship_candidates, validate_relationship, \
         validate_element, validate_completeness, crosscut_conflicts, search_knowledge, \
         lookup_derived_summary; knowledge-model-v2)"
    );

    let store: Arc<Mutex<dyn store::Store + Send>> =
        Arc::new(Mutex::new(store::SqliteStore::from(conn)));
    let server = KnowledgeServer { store };
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
        let store: Arc<Mutex<dyn store::Store + Send>> =
            Arc::new(Mutex::new(store::SqliteStore::from(conn)));
        KnowledgeServer { store }
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

    #[test]
    fn meta_routing_guide_references_new_tools() {
        let response = routing_guide();
        assert!(response.contains("lookup_subject"));
        assert!(response.contains("lookup_rules"));
        assert!(response.contains("lookup_relationships"));
        assert!(response.contains("lookup_valid_relationships"));
        assert!(response.contains("search_constructs"));
        assert!(response.contains("meta_list_domains"));
        assert!(response.contains("lookup_domain_summary"));
        assert!(response.contains("crosscut_traceability"));
        assert!(response.contains("crosscut_cross_domain"));
        assert!(response.contains("crosscut_valid_relationship_candidates"));
        assert!(response.contains("validate_relationship"));
        assert!(response.contains("crosscut_conflicts"));
    }

    #[test]
    fn meta_list_domains_reports_seeded_udra_domain() {
        let server = test_server();
        let response = server.meta_list_domains();
        assert!(response.contains("udra -- 2 subject(s)"));
        assert!(response.contains("Data Mesh Principles"));
    }

    #[test]
    fn search_constructs_filters_by_subject_type() {
        let server = test_server();
        let response = server.search_constructs(Parameters(SearchConstructsParams {
            domain_tag: "udra".into(),
            subject_type: Some("concept".into()),
        }));
        assert!(response.contains("Data Product"));
        assert!(response.contains("Data Contract"));

        let empty = server.search_constructs(Parameters(SearchConstructsParams {
            domain_tag: "udra".into(),
            subject_type: Some("no-such-type".into()),
        }));
        assert!(empty.contains("No subjects found"));
    }

    #[test]
    fn lookup_domain_summary_reports_counts_and_roots() {
        let server = test_server();
        let response = server.lookup_domain_summary(Parameters(DomainParams {
            domain_tag: "udra".into(),
        }));
        assert!(response.contains("2 subject(s) total"));
        assert!(response.contains("concept: 2"));
        assert!(response.contains("Data Mesh Principles"));
    }

    #[test]
    fn lookup_domain_summary_unknown_domain_reports_not_found() {
        let server = test_server();
        let response = server.lookup_domain_summary(Parameters(DomainParams {
            domain_tag: "no-such-domain".into(),
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn lookup_rules_excludes_relationship_claims_and_filters_by_binding_strength() {
        let server = test_server();
        let all = server.lookup_rules(Parameters(LookupRulesParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            binding_strength: None,
        }));
        assert!(!all.contains("exposes"));

        let must_only = server.lookup_rules(Parameters(LookupRulesParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            binding_strength: Some("MUST".into()),
        }));
        assert!(must_only.contains("[MUST]"));
        assert!(!must_only.contains("[SHOULD]"));
    }

    #[test]
    fn lookup_rules_rejects_unknown_binding_strength() {
        let server = test_server();
        let response = server.lookup_rules(Parameters(LookupRulesParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            binding_strength: Some("MAYBE".into()),
        }));
        assert!(response.contains("not a known binding strength"));
    }

    #[test]
    fn lookup_relationships_returns_seeded_relationship() {
        let server = test_server();
        let response = server.lookup_relationships(Parameters(LookupRelationshipsParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            relationship_type: None,
        }));
        assert!(response.contains("exposes"));
        assert!(response.contains("udra.DataContract"));
    }

    #[test]
    fn lookup_relationships_filters_by_relationship_type() {
        let server = test_server();
        let matching = server.lookup_relationships(Parameters(LookupRelationshipsParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            relationship_type: Some("exposes".into()),
        }));
        assert!(matching.contains("udra.DataContract"));

        let non_matching = server.lookup_relationships(Parameters(LookupRelationshipsParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            relationship_type: Some("no-such-type".into()),
        }));
        assert!(non_matching.contains("No outgoing relationships"));
    }

    #[test]
    fn lookup_valid_relationships_scoped_within_domain() {
        let server = test_server();
        let response = server.lookup_valid_relationships(Parameters(ValidRelationshipsParams {
            domain_tag: "udra".into(),
            from_type: "concept".into(),
            to_type: "concept".into(),
        }));
        assert!(response.contains("exposes"));
        assert!(response.contains("traces_to"));
        // rule.dm.003 crosses into data_mesh -- shouldn't show up here.
        assert!(!response.contains("realizes"));
    }

    #[test]
    fn crosscut_traceability_reports_outgoing_and_incoming() {
        let server = test_server();
        let response = server.crosscut_traceability(Parameters(TraceabilityParams {
            domain_tag: "udra".into(),
            subject_ref: "DataContract".into(),
            include_optional: false,
        }));
        assert!(response.contains("Must trace to:"));
        assert!(response.contains("udra.DataProduct"));
    }

    #[test]
    fn crosscut_cross_domain_finds_seeded_cross_domain_relationship() {
        let server = test_server();
        let response = server.crosscut_cross_domain(Parameters(CrossDomainParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            to_domain_tag: None,
        }));
        assert!(response.contains("realizes"));
        assert!(response.contains("data_mesh.DataProduct"));

        let non_matching = server.crosscut_cross_domain(Parameters(CrossDomainParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            to_domain_tag: Some("no-such-domain".into()),
        }));
        assert!(non_matching.contains("No cross-domain relationships"));
    }

    #[test]
    fn crosscut_valid_relationship_candidates_reports_seeded_candidates() {
        let server = test_server();
        let response = server.crosscut_valid_relationship_candidates(Parameters(
            ValidRelationshipCandidatesParams {
                domain_tag: "udra".into(),
            },
        ));
        assert!(response.contains("exposes"));
        assert!(response.contains("cardinality 1..*"));
    }

    #[test]
    fn validate_relationship_reports_valid_and_invalid() {
        let server = test_server();
        let valid = server.validate_relationship(Parameters(ValidateRelationshipParams {
            domain_tag: "udra".into(),
            from_subject_ref: "DataProduct".into(),
            to_subject_ref: "DataContract".into(),
            relationship_type: "exposes".into(),
        }));
        assert!(valid.starts_with("VALID"));

        let invalid = server.validate_relationship(Parameters(ValidateRelationshipParams {
            domain_tag: "udra".into(),
            from_subject_ref: "DataProduct".into(),
            to_subject_ref: "DataContract".into(),
            relationship_type: "no-such-type".into(),
        }));
        assert!(invalid.starts_with("INVALID"));
    }

    #[test]
    fn validate_element_passes_when_property_matches_pattern() {
        let server = test_server();
        let mut properties = HashMap::new();
        properties.insert("owner_email".to_string(), "alice@example.org".to_string());
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            properties,
        }));
        assert!(response.contains("PASS"));
        assert!(!response.contains("FAIL"));
    }

    #[test]
    fn validate_element_fails_when_required_property_missing() {
        let server = test_server();
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            properties: HashMap::new(),
        }));
        assert!(response.starts_with("Data Product (udra.DataProduct): FAIL"));
    }

    #[test]
    fn validate_element_warns_on_pattern_mismatch_not_fail() {
        let server = test_server();
        let mut properties = HashMap::new();
        properties.insert("owner_email".to_string(), "not-an-email".to_string());
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            properties,
        }));
        assert!(response.starts_with("Data Product (udra.DataProduct): WARNING"));
    }

    #[test]
    fn validate_element_no_machine_checkable_rules_says_so() {
        let server = test_server();
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_tag: "udra".into(),
            subject_ref: "DataContract".into(),
            properties: HashMap::new(),
        }));
        assert!(response.contains("no machine-checkable rules"));
    }

    #[test]
    fn validate_completeness_reports_complete_when_all_members_present() {
        let server = test_server();
        let response = server.validate_completeness(Parameters(ValidateCompletenessParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            present_element_types: vec!["DataContract".into(), "DataProduct".into()],
        }));
        assert!(response.starts_with("Data Product (udra.DataProduct): COMPLETE"));
        assert!(!response.contains("MISSING"));
    }

    #[test]
    fn validate_completeness_reports_incomplete_and_names_the_missing_rule() {
        let server = test_server();
        let response = server.validate_completeness(Parameters(ValidateCompletenessParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
            present_element_types: vec!["DataContract".into()],
        }));
        assert!(response.starts_with("Data Product (udra.DataProduct): INCOMPLETE"));
        assert!(response.contains("MISSING"));
        assert!(response.contains("Data Mesh data product concept"));
    }

    #[test]
    fn validate_completeness_no_constraints_defined_says_so() {
        let server = test_server();
        let response = server.validate_completeness(Parameters(ValidateCompletenessParams {
            domain_tag: "udra".into(),
            subject_ref: "DataContract".into(),
            present_element_types: vec![],
        }));
        assert!(response.contains("no completeness constraints defined"));
    }

    #[test]
    fn validate_completeness_unknown_subject_reports_not_found() {
        let server = test_server();
        let response = server.validate_completeness(Parameters(ValidateCompletenessParams {
            domain_tag: "udra".into(),
            subject_ref: "NoSuchSubject".into(),
            present_element_types: vec![],
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn search_knowledge_finds_matches_and_declares_retrieval_mode_honestly() {
        let server = test_server();
        let response = server.search_knowledge(Parameters(SearchKnowledgeParams {
            query: "accountable".into(),
            domain_tag: None,
            limit: None,
        }));
        assert!(response.contains("retrieval mode: hybrid"));
        assert!(response.contains("not a trained semantic embedding"));
        assert!(response.contains("rule.dm.001"));
    }

    #[test]
    fn search_knowledge_filters_by_domain_tag() {
        let server = test_server();
        let response = server.search_knowledge(Parameters(SearchKnowledgeParams {
            query: "Product".into(),
            domain_tag: Some("data_mesh".into()),
            limit: None,
        }));
        assert!(response.contains("data_mesh.DataProduct"));
        assert!(!response.contains("udra.DataProduct"));
    }

    #[test]
    fn search_knowledge_no_match_says_so_not_an_error() {
        let server = test_server();
        let response = server.search_knowledge(Parameters(SearchKnowledgeParams {
            query: "zzzznosuchword".into(),
            domain_tag: None,
            limit: None,
        }));
        assert!(response.starts_with("No results"));
    }

    #[test]
    fn search_knowledge_empty_query_says_so_not_an_error() {
        let server = test_server();
        let response = server.search_knowledge(Parameters(SearchKnowledgeParams {
            query: "   ".into(),
            domain_tag: None,
            limit: None,
        }));
        assert!(response.contains("Query is empty"));
    }

    #[test]
    fn lookup_derived_summary_reports_seeded_derivation_as_non_authoritative() {
        let server = test_server();
        let response = server.lookup_derived_summary(Parameters(LookupDerivedSummaryParams {
            domain_tag: "udra".into(),
            subject_ref: "DataProduct".into(),
        }));
        assert!(response.contains("NON-AUTHORITATIVE"));
        assert!(response.contains("Effective ownership & registration guidance"));
        assert!(response.contains("rule.dm.001"));
        assert!(response.contains("rule.army-udra.001"));
        assert!(response.contains("rule.org.001"));
    }

    #[test]
    fn lookup_derived_summary_no_derivations_says_so() {
        let server = test_server();
        let response = server.lookup_derived_summary(Parameters(LookupDerivedSummaryParams {
            domain_tag: "udra".into(),
            subject_ref: "DataContract".into(),
        }));
        assert!(response.contains("no recorded derived summaries"));
    }

    #[test]
    fn lookup_derived_summary_unknown_subject_reports_not_found() {
        let server = test_server();
        let response = server.lookup_derived_summary(Parameters(LookupDerivedSummaryParams {
            domain_tag: "udra".into(),
            subject_ref: "NoSuchSubject".into(),
        }));
        assert!(response.contains("not found"));
    }
}
