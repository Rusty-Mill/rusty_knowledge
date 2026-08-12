//! Rusty Knowledge — vertical-slice proof of concept.
//!
//! Authorized by `rusty_foundation_akb`'s ADR-0166 (RFC-0005 fast-lane
//! entry): `knowledge` doesn't author unsafe/FFI, a native platform
//! backend, or authority/crypto primitives, so implementation proceeds
//! without TRIAL-0003's full entry-gate process.
//!
//! This slice adds a real, minimal MCP server (via `rmcp`, stdio
//! transport) exposing one tool — `search_knowledge` — over the
//! FTS5/sqlite-vec store from the previous slice. It is deliberately
//! not the full 15-tool surface `knowledge-mcp` exposes; the point of
//! a vertical slice is proving the transport, the store, and the typed
//! authority layer all compose end-to-end before building the rest.
//!
//! What this does *not* yet do: Streamable HTTP transport (stdio only,
//! since that's rmcp's simplest documented starting point), the
//! layered-authority conflict registry (RK-002), multi-domain hosting
//! (RK-003), or hybrid vector retrieval in the tool surface itself
//! (RK-004's vec0 table exists in the store but isn't queried by this
//! tool yet). Each is a candidate for the next slice, not silently
//! dropped.

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
}

#[derive(Clone)]
struct KnowledgeServer {
    conn: Arc<Mutex<Connection>>,
}

#[tool_router(server_handler)]
impl KnowledgeServer {
    #[tool(
        description = "Search knowledge-base rules by full-text query; every result carries its authority layer (Standard / Tool Implementation / Conventions / Process) — RM-KNOWLEDGE-MODEL-0002."
    )]
    fn search_knowledge(
        &self,
        Parameters(SearchParams { query }): Parameters<SearchParams>,
    ) -> String {
        let conn = self.conn.lock().expect("store mutex poisoned");
        match store::search(&conn, &query) {
            Ok(rules) if rules.is_empty() => format!("No rules matched {query:?}."),
            Ok(rules) => rules
                .iter()
                .map(|r| format!("[{}] {}: {}", r.layer.as_str(), r.construct, r.text))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(err) => format!("Search failed: {err}"),
        }
    }

    #[tool(
        description = "Query routing guidance -- which tools to use for which question types. Call this when unsure how to decompose a task."
    )]
    fn meta_routing_guide(&self) -> String {
        routing_guide()
    }
}

/// Routing guidance, matching `knowledge-mcp`'s `meta.routing_guide` in shape.
/// Deliberately limited to tools that actually exist in this crate today --
/// `search_knowledge` only. `knowledge-mcp`'s own routing table also covers
/// lookup/validate/crosscut question patterns and a multi-step evaluation
/// workflow; those entries land here as their tools are implemented
/// (rusty_knowledge#4-#16), not advertised ahead of a working tool.
fn routing_guide() -> String {
    "Routing guidance (grows as more tools land -- see rusty_knowledge#4-#16):\n\
     - \"I can't find the right construct\" -> search_knowledge"
        .to_string()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let conn = store::open_store()?;
    store::seed(&conn)?;

    // RK-001 sanity check, kept from the previous slice's proof: this
    // still only compiles because AuthorityLayer has no "unknown" variant.
    let _: AuthorityLayer = AuthorityLayer::Standard;

    eprintln!("rusty-knowledge MCP server starting on stdio (tool: search_knowledge)");

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
        // None of these tools exist yet -- the guide must not claim they do.
        for not_yet_implemented in ["lookup_construct", "validate_element", "crosscut_conflicts"] {
            assert!(!guide.contains(not_yet_implemented));
        }
    }
}
