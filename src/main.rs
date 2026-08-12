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
//! ranked, always declares its retrieval mode per RM-KNOWLEDGE-MODEL-0005)
//! and `meta_routing_guide`.
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
use store::AuthorityLayer;

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
        let layer_filter = match layer.as_deref().map(AuthorityLayer::parse) {
            Some(None) => {
                return format!(
                    "Search failed: {:?} is not a known authority layer (expected one of Standard, \
                     Tool Implementation, Conventions, Process).",
                    layer.unwrap()
                );
            }
            Some(Some(parsed)) => Some(parsed),
            None => None,
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
        let layer_filter = match layer.as_deref().map(AuthorityLayer::parse) {
            Some(None) => {
                return format!(
                    "Lookup failed: {:?} is not a known authority layer (expected one of Standard, \
                     Tool Implementation, Conventions, Process).",
                    layer.unwrap()
                );
            }
            Some(Some(parsed)) => Some(parsed),
            None => None,
        };

        let conn = self.conn.lock().expect("store mutex poisoned");
        let construct = match store::resolve_construct(&conn, &domain_id, &construct_ref) {
            Ok(Some(construct)) => construct,
            Ok(None) => {
                return format!("Construct {construct_ref:?} not found in domain {domain_id:?}.");
            }
            Err(err) => return format!("Lookup failed: {err}"),
        };

        let rules = match store::rules_for_construct(&conn, &construct.id, layer_filter) {
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
}

/// Routing guidance, matching `knowledge-mcp`'s `meta.routing_guide` in shape.
/// Deliberately limited to tools that actually exist in this crate today.
/// `knowledge-mcp`'s own routing table also covers validate/crosscut question
/// patterns and a multi-step evaluation workflow; those entries land here as
/// their tools are implemented (rusty_knowledge#5-#16), not advertised ahead
/// of a working tool.
fn routing_guide() -> String {
    "Routing guidance (grows as more tools land -- see rusty_knowledge#5-#16):\n\
     - \"I can't find the right construct\" -> search_knowledge\n\
     - \"What does X mean?\" -> lookup_construct"
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
        "rusty-knowledge MCP server starting on stdio (tools: search_knowledge, meta_routing_guide)"
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
        assert!(guide.contains("lookup_construct"));
        // These tools don't exist yet -- the guide must not claim they do.
        for not_yet_implemented in ["validate_element", "crosscut_conflicts"] {
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
}
