//! Rusty Knowledge — an MCP server (via `rmcp`, stdio transport) over an
//! FTS5/sqlite-vec store, working toward parity with `knowledge-mcp`'s
//! 15-tool surface (tracked as rusty_knowledge#2-#18).
//!
//! Authorized by `rusty_foundation_akb`'s ADR-0166 (RFC-0005 fast-lane
//! entry): `knowledge` doesn't author unsafe/FFI, a native platform
//! backend, or authority/crypto primitives, so implementation proceeds
//! without TRIAL-0003's full entry-gate process.
//!
//! Tools implemented so far: `search_knowledge` (domain/layer-scoped,
//! ranked, always declares its retrieval mode per RM-KNOWLEDGE-MODEL-0005),
//! `meta_routing_guide`, `lookup_construct`, `lookup_rules`,
//! `lookup_relationships`, `lookup_valid_relationships`,
//! `lookup_domain_summary`, `validate_element` (required-property,
//! enum-value, range, and pattern machine checks -- pattern matching via
//! `rusty_regx`, a zero-dependency in-ecosystem regex engine),
//! `validate_relationship`, `validate_completeness`, and `search_constructs`.
//!
//! What this does *not* yet do: Streamable HTTP transport (stdio only,
//! since that's rmcp's simplest documented starting point), the
//! layered-authority conflict registry (RK-002), or hybrid vector
//! retrieval in the tool surface itself (RK-004's vec0 table exists in
//! the store but isn't queried by this tool yet). Each is a candidate
//! for a later slice, not silently dropped.

mod store;

use rmcp::{
    ServiceExt, handler::server::wrapper::Parameters, schemars, tool, tool_router, transport::stdio,
};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use store::{AuthorityLayer, RuleType, ValidationOutcome};

/// Shared by every tool that accepts an optional authority-layer filter
/// string: `Ok(None)` for "no filter", `Ok(Some(_))` for a recognized layer,
/// `Err` (never silently ignored) for anything else.
fn parse_layer_filter(layer: &Option<String>) -> Result<Option<AuthorityLayer>, String> {
    match layer.as_deref().map(AuthorityLayer::parse) {
        Some(None) => Err(format!(
            "{:?} is not a known authority layer (expected one of Standard, Tool Implementation, \
             Conventions, Process).",
            layer.as_ref().unwrap()
        )),
        Some(Some(parsed)) => Ok(Some(parsed)),
        None => Ok(None),
    }
}

/// Same contract as `parse_layer_filter`, for the rule-type filter.
fn parse_rule_type_filter(rule_type: &Option<String>) -> Result<Option<RuleType>, String> {
    match rule_type.as_deref().map(RuleType::parse) {
        Some(None) => Err(format!(
            "{:?} is not a known rule type (expected one of MUST, SHALL, SHOULD, MAY, MUST_NOT).",
            rule_type.as_ref().unwrap()
        )),
        Some(Some(parsed)) => Ok(Some(parsed)),
        None => Ok(None),
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchParams {
    /// FTS5 query string, e.g. a construct name or a word from a rule's text.
    query: String,
    /// Restrict results to one domain (e.g. "uaf-1.3"). Omit to search all
    /// loaded domains.
    #[serde(default)]
    domain_id: Option<String>,
    /// Restrict results to one authority layer ("Standard" / "Tool
    /// Implementation" / "Conventions" / "Process"). Omit to search all
    /// layers. An unrecognized value is reported as an error, not silently
    /// ignored.
    #[serde(default)]
    layer: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ConstructLookupParams {
    /// Domain to look up the construct in, e.g. "uaf-1.3".
    domain_id: String,
    /// The construct's short name or ID.
    construct_ref: String,
    /// Restrict returned rules to one authority layer ("Standard" / "Tool
    /// Implementation" / "Conventions" / "Process"). Omit for all layers.
    #[serde(default)]
    layer: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RulesLookupParams {
    /// Domain to look up the construct in, e.g. "uaf-1.3".
    domain_id: String,
    /// The construct's short name or ID.
    construct_ref: String,
    /// Restrict to one authority layer ("Standard" / "Tool Implementation" /
    /// "Conventions" / "Process"). Omit for all layers.
    #[serde(default)]
    layer: Option<String>,
    /// Restrict to one rule type ("MUST" / "SHALL" / "SHOULD" / "MAY" /
    /// "MUST_NOT"). Omit for all types.
    #[serde(default)]
    rule_type: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RelationshipsLookupParams {
    /// Domain both constructs belong to, e.g. "uaf-1.3".
    domain_id: String,
    /// The source construct's short name or ID.
    from_construct_ref: String,
    /// Restrict to relationships targeting this construct (short name or
    /// ID). Unlike `knowledge-mcp`, an unresolvable reference here is an
    /// error, not a silently dropped filter.
    #[serde(default)]
    to_construct_ref: Option<String>,
    /// Restrict to one relationship type (e.g. "records"). Omit for all types.
    #[serde(default)]
    relationship_type: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidRelationshipsLookupParams {
    /// Domain to check, e.g. "uaf-1.3".
    domain_id: String,
    /// Source construct type, e.g. "entity".
    from_type: String,
    /// Target construct type.
    to_type: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct DomainSummaryParams {
    /// Domain to summarize, e.g. "uaf-1.3".
    domain_id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidateElementParams {
    /// Domain the construct belongs to, e.g. "uaf-1.3".
    domain_id: String,
    /// The construct type's short name or ID to validate the element against.
    construct_ref: String,
    /// Restrict validation to one authority layer. Omit to check all layers.
    #[serde(default)]
    layer: Option<String>,
    /// The element's properties (name -> value) to validate.
    #[serde(default)]
    properties: std::collections::HashMap<String, String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidateRelationshipParams {
    /// Domain both constructs belong to, e.g. "uaf-1.3".
    domain_id: String,
    /// The source construct's short name or ID.
    from_construct_ref: String,
    /// The target construct's short name or ID.
    to_construct_ref: String,
    /// The relationship type to validate, e.g. "records".
    relationship_type: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ValidateCompletenessParams {
    /// Domain the construct belongs to, e.g. "uaf-1.3".
    domain_id: String,
    /// The container/viewpoint construct's short name or ID.
    construct_ref: String,
    /// Element type IDs actually present in the model being checked.
    #[serde(default)]
    present_element_types: Vec<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchConstructsParams {
    /// Domain to list constructs from, e.g. "uaf-1.3".
    domain_id: String,
    /// Restrict to one construct type, e.g. "entity". Omit for all types.
    #[serde(default)]
    construct_type: Option<String>,
}

#[derive(Clone)]
struct KnowledgeServer {
    conn: Arc<Mutex<Connection>>,
}

#[tool_router(server_handler)]
impl KnowledgeServer {
    #[tool(
        description = "Search knowledge-base rules by full-text query, optionally scoped to a domain and/or authority layer. Every result carries its authority layer (Standard / Tool Implementation / Conventions / Process), a rank, and the response always declares its retrieval mode (lexical-only today) per RM-KNOWLEDGE-MODEL-0002 and RM-KNOWLEDGE-MODEL-0005."
    )]
    fn search_knowledge(
        &self,
        Parameters(SearchParams {
            query,
            domain_id,
            layer,
        }): Parameters<SearchParams>,
    ) -> String {
        let layer_filter = match parse_layer_filter(&layer) {
            Ok(filter) => filter,
            Err(err) => return format!("Search failed: {err}"),
        };

        let conn = self.conn.lock().expect("store mutex poisoned");
        match store::search_scoped(&conn, &query, domain_id.as_deref(), layer_filter) {
            Ok((hits, mode)) if hits.is_empty() => {
                format!(
                    "Retrieval mode: {}\nNo rules matched {query:?}.",
                    mode.as_str()
                )
            }
            Ok((hits, mode)) => {
                let results = hits
                    .iter()
                    .map(|h| {
                        format!(
                            "[rank={:.3}] [{}] {}: {}",
                            h.rank,
                            h.rule.layer.as_str(),
                            h.rule.construct,
                            h.rule.text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("Retrieval mode: {}\n{results}", mode.as_str())
            }
            Err(err) => format!("Search failed: {err}"),
        }
    }

    #[tool(
        description = "Query routing guidance -- which tools to use for which question types. Call this when unsure how to decompose a task."
    )]
    fn meta_routing_guide(&self) -> String {
        routing_guide()
    }

    #[tool(
        description = "Get full definition of a construct -- description and metadata -- plus its rules, optionally filtered by authority layer. Resolves construct_ref by short name first, then falls back to a direct ID match within the domain."
    )]
    fn lookup_construct(
        &self,
        Parameters(ConstructLookupParams {
            domain_id,
            construct_ref,
            layer,
        }): Parameters<ConstructLookupParams>,
    ) -> String {
        let layer_filter = match parse_layer_filter(&layer) {
            Ok(filter) => filter,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let conn = self.conn.lock().expect("store mutex poisoned");
        let construct = match store::resolve_construct(&conn, &domain_id, &construct_ref) {
            Ok(Some(construct)) => construct,
            Ok(None) => {
                return format!("Construct {construct_ref:?} not found in domain {domain_id:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules = match store::rules_for_construct(&conn, &construct.id, layer_filter, None) {
            Ok(rules) => rules,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules_block = if rules.is_empty() {
            "(no rules)".to_string()
        } else {
            rules
                .iter()
                .map(|r| format!("  [{}] {}", r.layer.as_str(), r.text))
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "{} ({}) [{}]\n{}\nRules:\n{rules_block}",
            construct.short_name, construct.id, construct.construct_type, construct.description
        )
    }

    #[tool(
        description = "Get rules for a construct, optionally filtered by authority layer and/or rule type (MUST/SHALL/SHOULD/MAY/MUST_NOT)."
    )]
    fn lookup_rules(
        &self,
        Parameters(RulesLookupParams {
            domain_id,
            construct_ref,
            layer,
            rule_type,
        }): Parameters<RulesLookupParams>,
    ) -> String {
        let layer_filter = match parse_layer_filter(&layer) {
            Ok(filter) => filter,
            Err(err) => return format!("Lookup failed: {err}"),
        };
        let rule_type_filter = match parse_rule_type_filter(&rule_type) {
            Ok(filter) => filter,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let conn = self.conn.lock().expect("store mutex poisoned");
        let construct = match store::resolve_construct(&conn, &domain_id, &construct_ref) {
            Ok(Some(construct)) => construct,
            Ok(None) => {
                return format!("Construct {construct_ref:?} not found in domain {domain_id:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules = match store::rules_for_construct(
            &conn,
            &construct.id,
            layer_filter,
            rule_type_filter,
        ) {
            Ok(rules) => rules,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if rules.is_empty() {
            return format!(
                "No rules found for {} ({}).",
                construct.short_name, construct.id
            );
        }

        let rules_block = rules
            .iter()
            .map(|r| {
                format!(
                    "  [{}, {}] {}",
                    r.layer.as_str(),
                    r.rule_type.as_str(),
                    r.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "{} ({}) -- {} rule(s):\n{rules_block}",
            construct.short_name,
            construct.id,
            rules.len()
        )
    }

    #[tool(
        description = "Get relationships from a construct -- what it connects to, with cardinality and layer provenance. Optionally narrowed to a target construct and/or relationship type."
    )]
    fn lookup_relationships(
        &self,
        Parameters(RelationshipsLookupParams {
            domain_id,
            from_construct_ref,
            to_construct_ref,
            relationship_type,
        }): Parameters<RelationshipsLookupParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let from_construct = match store::resolve_construct(&conn, &domain_id, &from_construct_ref)
        {
            Ok(Some(construct)) => construct,
            Ok(None) => {
                return format!(
                    "Construct {from_construct_ref:?} not found in domain {domain_id:?}."
                );
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let to_id = match to_construct_ref {
            Some(ref to_ref) => match store::resolve_construct(&conn, &domain_id, to_ref) {
                Ok(Some(construct)) => Some(construct.id),
                Ok(None) => {
                    return format!(
                        "to_construct_ref {to_ref:?} not found in domain {domain_id:?}."
                    );
                }
                Err(err) => return format!("Lookup failed: {err}"),
            },
            None => None,
        };

        let rels = match store::relationships_from(
            &conn,
            &from_construct.id,
            to_id.as_deref(),
            relationship_type.as_deref(),
            None,
        ) {
            Ok(rels) => rels,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        if rels.is_empty() {
            return format!(
                "No relationships found from {} ({}).",
                from_construct.short_name, from_construct.id
            );
        }

        let rels_block = rels
            .iter()
            .map(|r| {
                format!(
                    "  [{}] --{}--> {} (cardinality: {})",
                    r.layer.as_str(),
                    r.relationship_type,
                    r.to_construct_id,
                    r.cardinality
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "{} ({}) -- {} relationship(s):\n{rels_block}",
            from_construct.short_name,
            from_construct.id,
            rels.len()
        )
    }

    #[tool(
        description = "Given two construct types, return all valid relationship types between them across all layers, per the domain's declared valid-relationship set (RM-KNOWLEDGE-MODEL-0004)."
    )]
    fn lookup_valid_relationships(
        &self,
        Parameters(ValidRelationshipsLookupParams {
            domain_id,
            from_type,
            to_type,
        }): Parameters<ValidRelationshipsLookupParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let rules =
            match store::valid_relationships_between(&conn, &domain_id, &from_type, &to_type) {
                Ok(rules) => rules,
                Err(err) => return format!("Lookup failed: {err}"),
            };

        if rules.is_empty() {
            return format!(
                "No valid relationship types declared from {from_type:?} to {to_type:?} in domain {domain_id:?}."
            );
        }

        let rules_block = rules
            .iter()
            .map(|r| format!("  {} (cardinality: {})", r.relationship_type, r.cardinality))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "{from_type} -> {to_type}: {} valid relationship type(s):\n{rules_block}",
            rules.len()
        )
    }

    #[tool(
        description = "Get a summary of a domain: name, authority layers present, and construct counts (total and by type)."
    )]
    fn lookup_domain_summary(
        &self,
        Parameters(DomainSummaryParams { domain_id }): Parameters<DomainSummaryParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let domain = match store::domain_by_id(&conn, &domain_id) {
            Ok(Some(domain)) => domain,
            Ok(None) => return format!("Domain {domain_id:?} not found."),
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let constructs = match store::constructs_in_domain(&conn, &domain_id, None) {
            Ok(constructs) => constructs,
            Err(err) => return format!("Lookup failed: {err}"),
        };
        let layers = match store::layers_present_in_domain(&conn, &domain_id) {
            Ok(layers) => layers,
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let mut by_type: std::collections::BTreeMap<&str, usize> =
            std::collections::BTreeMap::new();
        for c in &constructs {
            *by_type.entry(c.construct_type.as_str()).or_insert(0) += 1;
        }
        let by_type_block = if by_type.is_empty() {
            "  (none)".to_string()
        } else {
            by_type
                .iter()
                .map(|(construct_type, count)| format!("  {construct_type}: {count}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let layers_block = if layers.is_empty() {
            "(none)".to_string()
        } else {
            layers
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };

        format!(
            "{} ({})\nLayers present: {layers_block}\nConstructs: {} total\n{by_type_block}",
            domain.name,
            domain.id,
            constructs.len()
        )
    }

    #[tool(
        description = "Validate an element (its properties) against the machine-checkable rules for a construct type. Returns PASS/FAIL/WARNING per checkable rule, plus an overall result. Use layer to validate against a specific authority layer only."
    )]
    fn validate_element(
        &self,
        Parameters(ValidateElementParams {
            domain_id,
            construct_ref,
            layer,
            properties,
        }): Parameters<ValidateElementParams>,
    ) -> String {
        let layer_filter = match parse_layer_filter(&layer) {
            Ok(filter) => filter,
            Err(err) => return format!("Validation failed: {err}"),
        };

        let conn = self.conn.lock().expect("store mutex poisoned");
        let construct = match store::resolve_construct(&conn, &domain_id, &construct_ref) {
            Ok(Some(construct)) => construct,
            Ok(None) => {
                return format!("Construct {construct_ref:?} not found in domain {domain_id:?}.");
            }
            Err(err) => return format!("Validation failed: {err}"),
        };

        let rules = match store::rules_with_checks_for_construct(&conn, &construct.id, layer_filter)
        {
            Ok(rules) => rules,
            Err(err) => return format!("Validation failed: {err}"),
        };

        let findings: Vec<(ValidationOutcome, String)> = rules
            .iter()
            .filter_map(|(rule, machine_rule)| {
                let machine_rule = machine_rule.as_ref()?;
                let (outcome, message) = store::evaluate_machine_rule(machine_rule, &properties);
                Some((outcome, format!("[{}] {message}", rule.layer.as_str())))
            })
            .collect();

        if findings.is_empty() {
            return format!(
                "{} ({}) -- no machine-checkable rules{}; nothing to validate.",
                construct.short_name,
                construct.id,
                if layer_filter.is_some() {
                    " at this layer"
                } else {
                    ""
                }
            );
        }

        let fail_count = findings
            .iter()
            .filter(|(o, _)| *o == ValidationOutcome::Fail)
            .count();
        let warning_count = findings
            .iter()
            .filter(|(o, _)| *o == ValidationOutcome::Warning)
            .count();
        let overall = if fail_count > 0 {
            ValidationOutcome::Fail
        } else if warning_count > 0 {
            ValidationOutcome::Warning
        } else {
            ValidationOutcome::Pass
        };

        let findings_block = findings
            .iter()
            .map(|(outcome, message)| format!("  {} {message}", outcome.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "{} ({}) -- overall: {} ({fail_count} fail, {warning_count} warning, {} pass)\n{findings_block}",
            construct.short_name,
            construct.id,
            overall.as_str(),
            findings.len() - fail_count - warning_count,
        )
    }

    #[tool(
        description = "Validate whether a relationship type between two constructs is permitted, according to the domain's recorded relationships. VALID if at least one matching relationship is recorded; INVALID otherwise."
    )]
    fn validate_relationship(
        &self,
        Parameters(ValidateRelationshipParams {
            domain_id,
            from_construct_ref,
            to_construct_ref,
            relationship_type,
        }): Parameters<ValidateRelationshipParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let from_construct = match store::resolve_construct(&conn, &domain_id, &from_construct_ref)
        {
            Ok(Some(construct)) => construct,
            Ok(None) => {
                return format!(
                    "Source construct {from_construct_ref:?} not found in domain {domain_id:?}."
                );
            }
            Err(err) => return format!("Validation failed: {err}"),
        };
        let to_construct = match store::resolve_construct(&conn, &domain_id, &to_construct_ref) {
            Ok(Some(construct)) => construct,
            Ok(None) => {
                return format!(
                    "Target construct {to_construct_ref:?} not found in domain {domain_id:?}."
                );
            }
            Err(err) => return format!("Validation failed: {err}"),
        };

        let rels = match store::relationships_from(
            &conn,
            &from_construct.id,
            Some(&to_construct.id),
            Some(&relationship_type),
            None,
        ) {
            Ok(rels) => rels,
            Err(err) => return format!("Validation failed: {err}"),
        };

        if rels.is_empty() {
            return format!(
                "INVALID: no recorded rule permits {relationship_type:?} from {} to {}.",
                from_construct.short_name, to_construct.short_name
            );
        }

        let matches_block = rels
            .iter()
            .map(|r| {
                format!(
                    "  [{}] {} (cardinality: {})",
                    r.layer.as_str(),
                    r.relationship_type,
                    r.cardinality
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "VALID: {} matching rule(s) permit {relationship_type:?} from {} to {}:\n{matches_block}",
            rels.len(),
            from_construct.short_name,
            to_construct.short_name
        )
    }

    #[tool(
        description = "Given a container/viewpoint construct and the element types present in a model, evaluate what's required, optional, and missing. Required element types come from MUST-typed relationships originating at the construct."
    )]
    fn validate_completeness(
        &self,
        Parameters(ValidateCompletenessParams {
            domain_id,
            construct_ref,
            present_element_types,
        }): Parameters<ValidateCompletenessParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let construct = match store::resolve_construct(&conn, &domain_id, &construct_ref) {
            Ok(Some(construct)) => construct,
            Ok(None) => {
                return format!("Construct {construct_ref:?} not found in domain {domain_id:?}.");
            }
            Err(err) => return format!("Validation failed: {err}"),
        };

        let report =
            match store::evaluate_completeness(&conn, &construct.id, &present_element_types) {
                Ok(report) => report,
                Err(err) => return format!("Validation failed: {err}"),
            };

        let list_or_none = |items: &[String]| {
            if items.is_empty() {
                "(none)".to_string()
            } else {
                items.join(", ")
            }
        };

        format!(
            "{} ({}) -- {}\nRequired element types: {}\nPresent: {}\nMissing required: {}\nExtra present (not required): {}\nRequired rules: {}\nRecommended rules: {}",
            construct.short_name,
            construct.id,
            if report.is_complete {
                "COMPLETE"
            } else {
                "INCOMPLETE"
            },
            list_or_none(&report.required_element_types),
            list_or_none(&report.present_element_types),
            list_or_none(&report.missing_required),
            list_or_none(&report.extra_present),
            list_or_none(&report.required_rule_texts),
            list_or_none(&report.recommended_rule_texts),
        )
    }

    #[tool(description = "List and filter constructs within a domain by construct type.")]
    fn search_constructs(
        &self,
        Parameters(SearchConstructsParams {
            domain_id,
            construct_type,
        }): Parameters<SearchConstructsParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let constructs =
            match store::constructs_in_domain(&conn, &domain_id, construct_type.as_deref()) {
                Ok(constructs) => constructs,
                Err(err) => return format!("Search failed: {err}"),
            };

        if constructs.is_empty() {
            return format!("No constructs found in domain {domain_id:?}.");
        }

        let constructs_block = constructs
            .iter()
            .map(|c| format!("  {} ({}) [{}]", c.short_name, c.id, c.construct_type))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "{} construct(s) in domain {domain_id:?}:\n{constructs_block}",
            constructs.len()
        )
    }
}

/// Routing guidance, matching `knowledge-mcp`'s `meta.routing_guide` in shape.
/// Deliberately limited to tools that actually exist in this crate today.
/// `knowledge-mcp`'s own routing table also covers validate/crosscut question
/// patterns and a multi-step evaluation workflow; those entries land here as
/// their tools are implemented (rusty_knowledge#5-#16), not advertised ahead
/// of a working tool.
fn routing_guide() -> String {
    "Routing guidance (grows as more tools land -- see rusty_knowledge#13-#16):\n\
     - \"I can't find the right construct\" -> search_knowledge, search_constructs\n\
     - \"What does X mean?\" -> lookup_construct\n\
     - \"What should X be named/styled?\" -> lookup_rules (layer=Conventions)\n\
     - \"Who owns X / when is X due?\" -> lookup_rules (layer=Process)\n\
     - \"Is X valid/conformant?\" -> validate_element, validate_relationship\n\
     - \"Is this model/viewpoint complete?\" -> validate_completeness"
        .to_string()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let conn = store::open_store()?;
    store::seed(&conn)?;

    // RK-001 sanity check, kept from the previous slice's proof: this
    // still only compiles because AuthorityLayer has no "unknown" variant.
    let _: AuthorityLayer = AuthorityLayer::Standard;

    eprintln!(
        "rusty-knowledge MCP server starting on stdio (tools: search_knowledge, \
         meta_routing_guide, lookup_construct, lookup_rules, lookup_relationships, \
         lookup_valid_relationships, lookup_domain_summary, validate_element, \
         validate_relationship, validate_completeness, search_constructs)"
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

    #[test]
    fn routing_guide_only_references_existing_tools() {
        let guide = routing_guide();
        assert!(guide.contains("search_knowledge"));
        assert!(guide.contains("search_constructs"));
        assert!(guide.contains("lookup_construct"));
        assert!(guide.contains("lookup_rules"));
        assert!(guide.contains("validate_element"));
        assert!(guide.contains("validate_relationship"));
        assert!(guide.contains("validate_completeness"));
        // These tools don't exist yet -- the guide must not claim they do.
        for not_yet_implemented in ["crosscut_conflicts", "meta_list_domains"] {
            assert!(!guide.contains(not_yet_implemented));
        }
    }

    fn test_server() -> KnowledgeServer {
        let conn = store::open_store().unwrap();
        store::seed(&conn).unwrap();
        KnowledgeServer {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    #[test]
    fn search_knowledge_declares_retrieval_mode() {
        let server = test_server();
        let response = server.search_knowledge(Parameters(SearchParams {
            query: "AuthorityGrant".into(),
            domain_id: None,
            layer: None,
        }));
        assert!(response.starts_with("Retrieval mode: lexical-only"));
    }

    #[test]
    fn search_knowledge_domain_filter_excludes_other_domains() {
        let server = test_server();
        let response = server.search_knowledge(Parameters(SearchParams {
            query: "DataProduct".into(),
            domain_id: Some("uaf-1.3".into()),
            layer: None,
        }));
        assert!(response.contains("No rules matched"));
    }

    #[test]
    fn search_knowledge_rejects_unknown_layer_without_panicking() {
        let server = test_server();
        let response = server.search_knowledge(Parameters(SearchParams {
            query: "AuthorityGrant".into(),
            domain_id: None,
            layer: Some("Nonexistent".into()),
        }));
        assert!(response.contains("Search failed"));
        assert!(response.contains("not a known authority layer"));
    }

    #[test]
    fn lookup_construct_resolves_by_short_name() {
        let server = test_server();
        let response = server.lookup_construct(Parameters(ConstructLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: None,
        }));
        assert!(response.contains("AuthorityGrant"));
        assert!(response.contains("uaf-1.3:AuthorityGrant"));
        assert!(response.contains("scoped, time-bounded grant"));
        // Both seeded rules for this construct, across two layers.
        assert!(response.contains("Standard"));
        assert!(response.contains("Conventions"));
    }

    #[test]
    fn lookup_construct_resolves_by_id() {
        let server = test_server();
        let response = server.lookup_construct(Parameters(ConstructLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "uaf-1.3:AuthorityGrant".into(),
            layer: None,
        }));
        assert!(response.contains("AuthorityGrant"));
    }

    #[test]
    fn lookup_construct_layer_filter_narrows_rules() {
        let server = test_server();
        let response = server.lookup_construct(Parameters(ConstructLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: Some("Standard".into()),
        }));
        assert!(response.contains("Standard"));
        assert!(!response.contains("Conventions"));
    }

    #[test]
    fn lookup_construct_does_not_leak_across_domains() {
        let server = test_server();
        // DataProduct exists in data-mesh, not uaf-1.3.
        let response = server.lookup_construct(Parameters(ConstructLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "DataProduct".into(),
            layer: None,
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn lookup_construct_unknown_ref_reports_not_found() {
        let server = test_server();
        let response = server.lookup_construct(Parameters(ConstructLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "DoesNotExist".into(),
            layer: None,
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn lookup_rules_returns_all_rules_for_a_construct() {
        let server = test_server();
        let response = server.lookup_rules(Parameters(RulesLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: None,
            rule_type: None,
        }));
        assert!(response.contains("2 rule(s)"));
        assert!(response.contains("MUST"));
        assert!(response.contains("MAY"));
    }

    #[test]
    fn lookup_rules_layer_and_rule_type_filters_combine() {
        let server = test_server();
        let response = server.lookup_rules(Parameters(RulesLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: Some("Standard".into()),
            rule_type: Some("MUST".into()),
        }));
        assert!(response.contains("1 rule(s)"));

        let response = server.lookup_rules(Parameters(RulesLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: Some("Standard".into()),
            rule_type: Some("MAY".into()),
        }));
        assert!(response.contains("No rules found"));
    }

    #[test]
    fn lookup_rules_rejects_unknown_rule_type() {
        let server = test_server();
        let response = server.lookup_rules(Parameters(RulesLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: None,
            rule_type: Some("MIGHT".into()),
        }));
        assert!(response.contains("Lookup failed"));
        assert!(response.contains("not a known rule type"));
    }

    #[test]
    fn lookup_rules_unknown_construct_reports_not_found() {
        let server = test_server();
        let response = server.lookup_rules(Parameters(RulesLookupParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "DoesNotExist".into(),
            layer: None,
            rule_type: None,
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn lookup_relationships_returns_seeded_relationship() {
        let server = test_server();
        let response = server.lookup_relationships(Parameters(RelationshipsLookupParams {
            domain_id: "uaf-1.3".into(),
            from_construct_ref: "AuthorityGrant".into(),
            to_construct_ref: None,
            relationship_type: None,
        }));
        assert!(response.contains("1 relationship(s)"));
        assert!(response.contains("records"));
        assert!(response.contains("ConflictRegistryEntry"));
    }

    #[test]
    fn lookup_relationships_filters_by_to_construct_ref() {
        let server = test_server();
        let response = server.lookup_relationships(Parameters(RelationshipsLookupParams {
            domain_id: "uaf-1.3".into(),
            from_construct_ref: "AuthorityGrant".into(),
            to_construct_ref: Some("ConflictRegistryEntry".into()),
            relationship_type: None,
        }));
        assert!(response.contains("1 relationship(s)"));
    }

    #[test]
    fn lookup_relationships_unresolvable_to_ref_is_an_error_not_silently_dropped() {
        let server = test_server();
        let response = server.lookup_relationships(Parameters(RelationshipsLookupParams {
            domain_id: "uaf-1.3".into(),
            from_construct_ref: "AuthorityGrant".into(),
            to_construct_ref: Some("DoesNotExist".into()),
            relationship_type: None,
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn lookup_relationships_no_relationships_reports_empty() {
        let server = test_server();
        let response = server.lookup_relationships(Parameters(RelationshipsLookupParams {
            domain_id: "data-mesh".into(),
            from_construct_ref: "DataProduct".into(),
            to_construct_ref: None,
            relationship_type: None,
        }));
        assert!(response.contains("No relationships found"));
    }

    #[test]
    fn lookup_relationships_unknown_from_construct_reports_not_found() {
        let server = test_server();
        let response = server.lookup_relationships(Parameters(RelationshipsLookupParams {
            domain_id: "uaf-1.3".into(),
            from_construct_ref: "DoesNotExist".into(),
            to_construct_ref: None,
            relationship_type: None,
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn lookup_valid_relationships_returns_seeded_rule() {
        let server = test_server();
        let response =
            server.lookup_valid_relationships(Parameters(ValidRelationshipsLookupParams {
                domain_id: "uaf-1.3".into(),
                from_type: "entity".into(),
                to_type: "entity".into(),
            }));
        assert!(response.contains("1 valid relationship type(s)"));
        assert!(response.contains("records"));
    }

    #[test]
    fn lookup_valid_relationships_unknown_type_pair_reports_none() {
        let server = test_server();
        let response =
            server.lookup_valid_relationships(Parameters(ValidRelationshipsLookupParams {
                domain_id: "uaf-1.3".into(),
                from_type: "entity".into(),
                to_type: "viewpoint".into(),
            }));
        assert!(response.contains("No valid relationship types declared"));
    }

    #[test]
    fn lookup_domain_summary_reports_layers_and_counts() {
        let server = test_server();
        let response = server.lookup_domain_summary(Parameters(DomainSummaryParams {
            domain_id: "uaf-1.3".into(),
        }));
        assert!(response.contains("UAF 1.3"));
        assert!(response.contains("Constructs: 2 total"));
        assert!(response.contains("entity: 2"));
        // uaf-1.3's seeded rules span Standard and Conventions layers.
        assert!(response.contains("Standard"));
        assert!(response.contains("Conventions"));
    }

    #[test]
    fn lookup_domain_summary_other_domain_does_not_leak_counts() {
        let server = test_server();
        let response = server.lookup_domain_summary(Parameters(DomainSummaryParams {
            domain_id: "data-mesh".into(),
        }));
        assert!(response.contains("Constructs: 1 total"));
    }

    #[test]
    fn lookup_domain_summary_unknown_domain_reports_not_found() {
        let server = test_server();
        let response = server.lookup_domain_summary(Parameters(DomainSummaryParams {
            domain_id: "does-not-exist".into(),
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn validate_element_missing_required_property_fails() {
        let server = test_server();
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: None,
            properties: std::collections::HashMap::new(),
        }));
        assert!(response.contains("overall: FAIL"));
        assert!(response.contains("FAIL"));
        assert!(response.contains("scope"));
    }

    #[test]
    fn validate_element_present_required_property_passes() {
        let server = test_server();
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: None,
            properties: std::collections::HashMap::from([("scope".to_string(), "org".to_string())]),
        }));
        assert!(response.contains("overall: PASS"));
        assert!(response.contains("1 pass"));
    }

    #[test]
    fn validate_element_construct_with_no_machine_checks_has_nothing_to_validate() {
        let server = test_server();
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "ConflictRegistryEntry".into(),
            layer: None,
            properties: std::collections::HashMap::new(),
        }));
        assert!(response.contains("no machine-checkable rules"));
    }

    #[test]
    fn validate_element_unknown_construct_reports_not_found() {
        let server = test_server();
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "DoesNotExist".into(),
            layer: None,
            properties: std::collections::HashMap::new(),
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn validate_element_layer_filter_excludes_checks_from_other_layers() {
        let server = test_server();
        // The machine-checkable rule (scope required) is on the Standard
        // layer; filtering to Conventions should find nothing to validate.
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            layer: Some("Conventions".into()),
            properties: std::collections::HashMap::new(),
        }));
        assert!(response.contains("no machine-checkable rules"));
    }

    #[test]
    fn validate_element_pattern_check_passes_on_valid_team_slug() {
        let server = test_server();
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_id: "data-mesh".into(),
            construct_ref: "DataProduct".into(),
            layer: None,
            properties: std::collections::HashMap::from([(
                "owning_team".to_string(),
                "checkout-platform".to_string(),
            )]),
        }));
        assert!(response.contains("overall: PASS"));
    }

    #[test]
    fn validate_element_pattern_check_warns_on_invalid_team_slug() {
        let server = test_server();
        let response = server.validate_element(Parameters(ValidateElementParams {
            domain_id: "data-mesh".into(),
            construct_ref: "DataProduct".into(),
            layer: None,
            properties: std::collections::HashMap::from([(
                "owning_team".to_string(),
                "Checkout Platform!".to_string(),
            )]),
        }));
        assert!(response.contains("overall: WARNING"));
    }

    #[test]
    fn validate_relationship_recorded_relationship_is_valid() {
        let server = test_server();
        let response = server.validate_relationship(Parameters(ValidateRelationshipParams {
            domain_id: "uaf-1.3".into(),
            from_construct_ref: "AuthorityGrant".into(),
            to_construct_ref: "ConflictRegistryEntry".into(),
            relationship_type: "records".into(),
        }));
        assert!(response.starts_with("VALID"));
        assert!(response.contains("records"));
    }

    #[test]
    fn validate_relationship_unrecorded_type_is_invalid() {
        let server = test_server();
        let response = server.validate_relationship(Parameters(ValidateRelationshipParams {
            domain_id: "uaf-1.3".into(),
            from_construct_ref: "AuthorityGrant".into(),
            to_construct_ref: "ConflictRegistryEntry".into(),
            relationship_type: "supersedes".into(),
        }));
        assert!(response.starts_with("INVALID"));
    }

    #[test]
    fn validate_relationship_wrong_direction_is_invalid() {
        let server = test_server();
        // The seeded relationship is AuthorityGrant -> ConflictRegistryEntry,
        // not the reverse.
        let response = server.validate_relationship(Parameters(ValidateRelationshipParams {
            domain_id: "uaf-1.3".into(),
            from_construct_ref: "ConflictRegistryEntry".into(),
            to_construct_ref: "AuthorityGrant".into(),
            relationship_type: "records".into(),
        }));
        assert!(response.starts_with("INVALID"));
    }

    #[test]
    fn validate_relationship_unknown_construct_reports_not_found() {
        let server = test_server();
        let response = server.validate_relationship(Parameters(ValidateRelationshipParams {
            domain_id: "uaf-1.3".into(),
            from_construct_ref: "DoesNotExist".into(),
            to_construct_ref: "ConflictRegistryEntry".into(),
            relationship_type: "records".into(),
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn validate_completeness_reports_complete_when_required_present() {
        let server = test_server();
        let response = server.validate_completeness(Parameters(ValidateCompletenessParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            present_element_types: vec!["uaf-1.3:ConflictRegistryEntry".into()],
        }));
        assert!(response.contains("COMPLETE"));
        assert!(!response.contains("INCOMPLETE"));
        assert!(response.contains("scope and expiry"));
    }

    #[test]
    fn validate_completeness_reports_missing_required() {
        let server = test_server();
        let response = server.validate_completeness(Parameters(ValidateCompletenessParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            present_element_types: vec![],
        }));
        assert!(response.contains("INCOMPLETE"));
        assert!(response.contains("Missing required: uaf-1.3:ConflictRegistryEntry"));
    }

    #[test]
    fn validate_completeness_reports_extra_present_without_affecting_completeness() {
        let server = test_server();
        let response = server.validate_completeness(Parameters(ValidateCompletenessParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "AuthorityGrant".into(),
            present_element_types: vec![
                "uaf-1.3:ConflictRegistryEntry".into(),
                "something-unexpected".into(),
            ],
        }));
        assert!(response.contains("COMPLETE"));
        assert!(response.contains("Extra present (not required): something-unexpected"));
    }

    #[test]
    fn validate_completeness_unknown_construct_reports_not_found() {
        let server = test_server();
        let response = server.validate_completeness(Parameters(ValidateCompletenessParams {
            domain_id: "uaf-1.3".into(),
            construct_ref: "DoesNotExist".into(),
            present_element_types: vec![],
        }));
        assert!(response.contains("not found"));
    }

    #[test]
    fn search_constructs_lists_all_in_domain() {
        let server = test_server();
        let response = server.search_constructs(Parameters(SearchConstructsParams {
            domain_id: "uaf-1.3".into(),
            construct_type: None,
        }));
        assert!(response.contains("2 construct(s)"));
        assert!(response.contains("AuthorityGrant"));
        assert!(response.contains("ConflictRegistryEntry"));
        assert!(!response.contains("DataProduct"));
    }

    #[test]
    fn search_constructs_filters_by_type() {
        let server = test_server();
        let response = server.search_constructs(Parameters(SearchConstructsParams {
            domain_id: "uaf-1.3".into(),
            construct_type: Some("viewpoint".into()),
        }));
        assert!(response.contains("No constructs found"));
    }

    #[test]
    fn search_constructs_unknown_domain_reports_none_found() {
        let server = test_server();
        let response = server.search_constructs(Parameters(SearchConstructsParams {
            domain_id: "does-not-exist".into(),
            construct_type: None,
        }));
        assert!(response.contains("No constructs found"));
    }
}
