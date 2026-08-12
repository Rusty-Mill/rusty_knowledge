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

    /// Trusted-storage parse: panics on a value that isn't one of the four
    /// layers, since a row already in `rules_fts` was only ever written by
    /// `insert_rule` with a valid `as_str()` output.
    fn from_str(text: &str) -> Self {
        match text {
            "Standard" => AuthorityLayer::Standard,
            "Tool Implementation" => AuthorityLayer::ToolImplementation,
            "Conventions" => AuthorityLayer::Conventions,
            "Process" => AuthorityLayer::Process,
            other => panic!("stored layer {other:?} is not one of the four known layers"),
        }
    }

    /// Fallible parse for untrusted input (e.g. an MCP tool caller's layer
    /// filter) -- `None` rather than a panic on anything that isn't one of
    /// the four known layers.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "Standard" => Some(AuthorityLayer::Standard),
            "Tool Implementation" => Some(AuthorityLayer::ToolImplementation),
            "Conventions" => Some(AuthorityLayer::Conventions),
            "Process" => Some(AuthorityLayer::Process),
            _ => None,
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
///
/// A subset of `knowledge-mcp`'s `Construct` model: `name`, `is_abstract`,
/// `is_deprecated`, `parent_id`, `source_section`, and `metadata` aren't
/// modeled yet, and aren't invented here -- they land with whichever
/// parity-gap issue actually needs them, not speculatively.
pub struct Construct {
    pub id: String,
    pub domain_id: String,
    pub short_name: String,
    pub construct_type: String,
    pub description: String,
}

/// A rule's normative strength, matching `knowledge-mcp`'s five rule types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleType {
    Must,
    Shall,
    Should,
    May,
    MustNot,
}

impl RuleType {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleType::Must => "MUST",
            RuleType::Shall => "SHALL",
            RuleType::Should => "SHOULD",
            RuleType::May => "MAY",
            RuleType::MustNot => "MUST_NOT",
        }
    }

    fn from_str(text: &str) -> Self {
        match text {
            "MUST" => RuleType::Must,
            "SHALL" => RuleType::Shall,
            "SHOULD" => RuleType::Should,
            "MAY" => RuleType::May,
            "MUST_NOT" => RuleType::MustNot,
            other => panic!("stored rule_type {other:?} is not one of the five known types"),
        }
    }

    /// Fallible parse for untrusted input (e.g. an MCP tool caller's
    /// rule_type filter) -- see `AuthorityLayer::parse`'s doc comment for
    /// why this is kept separate from the trusted-storage `from_str` above.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "MUST" => Some(RuleType::Must),
            "SHALL" => Some(RuleType::Shall),
            "SHOULD" => Some(RuleType::Should),
            "MAY" => Some(RuleType::May),
            "MUST_NOT" => Some(RuleType::MustNot),
            _ => None,
        }
    }
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
    pub rule_type: RuleType,
}

/// A typed, directional link between two constructs in the same domain.
/// Queried by `lookup_relationships` and `crosscut_traceability`.
///
/// `rule_type` mirrors `Rule`'s: `knowledge-mcp`'s completeness check
/// (`validate.completeness`) treats a `MUST`/`SHALL` relationship as a
/// required child element type, everything else as optional -- the same
/// distinction `Rule::rule_type` already draws for free-text rules.
pub struct Relationship {
    pub id: String,
    pub domain_id: String,
    pub from_construct_id: String,
    pub to_construct_id: String,
    pub relationship_type: String,
    pub cardinality: String,
    pub layer: AuthorityLayer,
    pub rule_type: RuleType,
}

/// A *declared* rule about which relationship types are valid between two
/// construct *types* in a domain -- distinct from `Relationship`, which
/// records an actual link between two specific construct *instances*.
/// `RM-KNOWLEDGE-MODEL-0004` requires validation to check against this
/// declared set rather than inferring validity from whatever relationship
/// instances happen to already exist.
pub struct ValidRelationshipRule {
    pub domain_id: String,
    pub from_type: String,
    pub to_type: String,
    pub relationship_type: String,
    pub cardinality: String,
}

/// A conflict-registry entry: an explicitly documented place where two
/// authority layers disagree, and how that disagreement is resolved.
/// `construct_id: None` marks a domain-level conflict rather than one tied
/// to a specific construct.
pub struct Conflict {
    pub id: String,
    pub domain_id: String,
    pub construct_id: Option<String>,
    pub layer_a: AuthorityLayer,
    pub layer_b: AuthorityLayer,
    pub conflict_type: String,
    pub description: String,
    pub resolution: String,
    pub rationale: Option<String>,
    pub review_date: Option<String>,
}

/// A typed link between constructs in *different* domains -- e.g. a UAF
/// capability tracing to an RMF control family. Distinct from
/// `Relationship`, which only ever connects two constructs within the same
/// domain.
pub struct CrossDomainRelationship {
    pub id: String,
    pub from_domain_id: String,
    pub from_construct_id: String,
    pub to_domain_id: String,
    pub to_construct_id: String,
    pub relationship_type: String,
    pub description: Option<String>,
    pub rationale: Option<String>,
}

/// A structured, machine-checkable rule attached to a `Rule` row, matching
/// (a subset of) `knowledge-mcp`'s `machine_rule` schema. Most rules are
/// free text only (no `MachineRule` attached) -- this exists for the
/// minority whose normative text can be checked programmatically against an
/// element's properties.
///
/// `Pattern` (regex-match) is checked via `rusty_regx` -- a zero-dependency
/// POSIX-ERE engine from this same GitHub account, added as a dependency
/// only after explicit sign-off, per this skill's stop-and-ask rule for new
/// third-party dependencies (a bare `regex` crate dependency was considered
/// and deliberately not used, to avoid pulling in its several transitive
/// dependencies when a zero-dependency in-ecosystem alternative exists).
#[derive(Debug, Clone, PartialEq)]
pub enum MachineRule {
    RequiredProperty {
        property: String,
    },
    EnumValue {
        property: String,
        values: Vec<String>,
    },
    Pattern {
        property: String,
        pattern: String,
    },
    Range {
        property: String,
        min: Option<f64>,
        max: Option<f64>,
    },
}

/// The result of evaluating one `MachineRule` against an element's
/// properties -- PASS/FAIL/WARNING, matching `knowledge-mcp`'s three-way
/// `validate.element` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationOutcome {
    Pass,
    Fail,
    Warning,
}

impl ValidationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            ValidationOutcome::Pass => "PASS",
            ValidationOutcome::Fail => "FAIL",
            ValidationOutcome::Warning => "WARNING",
        }
    }
}

/// Evaluate one machine rule against an element's properties (property name
/// -> value, both as strings -- this crate doesn't model a typed property
/// schema yet, matching `knowledge-mcp`'s own loosely-typed `dict`).
pub fn evaluate_machine_rule(
    check: &MachineRule,
    properties: &std::collections::HashMap<String, String>,
) -> (ValidationOutcome, String) {
    match check {
        MachineRule::RequiredProperty { property } => match properties.get(property) {
            Some(v) if !v.is_empty() => (
                ValidationOutcome::Pass,
                format!("Property '{property}' is present"),
            ),
            _ => (
                ValidationOutcome::Fail,
                format!("Required property '{property}' is absent or empty"),
            ),
        },
        MachineRule::EnumValue { property, values } => match properties.get(property) {
            Some(v) if values.iter().any(|allowed| allowed == v) => (
                ValidationOutcome::Pass,
                format!("'{property}' value '{v}' is valid"),
            ),
            Some(v) => (
                ValidationOutcome::Fail,
                format!("'{property}' = '{v}' is not in {values:?}"),
            ),
            None => (
                ValidationOutcome::Fail,
                format!("Property '{property}' is absent"),
            ),
        },
        MachineRule::Pattern { property, pattern } => {
            let val = properties.get(property).map(String::as_str).unwrap_or("");
            match rusty_regx::Regex::new(pattern) {
                // `find` is an unanchored search; requiring the match to
                // start at 0 replicates Python's `re.match` semantics
                // (anchored at the start, not necessarily the whole string).
                Ok(re) => match re.find(val) {
                    Some(m) if m.start() == 0 => (
                        ValidationOutcome::Pass,
                        format!("'{property}' matches required pattern '{pattern}'"),
                    ),
                    _ => (
                        ValidationOutcome::Warning,
                        format!("'{property}' = '{val}' does not match pattern '{pattern}'"),
                    ),
                },
                Err(err) => (
                    ValidationOutcome::Warning,
                    format!("Pattern '{pattern}' for '{property}' is invalid: {err}"),
                ),
            }
        }
        MachineRule::Range { property, min, max } => {
            let Some(val) = properties.get(property).and_then(|v| v.parse::<f64>().ok()) else {
                return (
                    ValidationOutcome::Fail,
                    format!("Property '{property}' is absent or not numeric"),
                );
            };
            if let Some(min) = min
                && val < *min
            {
                return (
                    ValidationOutcome::Fail,
                    format!("'{property}' = {val} is below minimum {min}"),
                );
            }
            if let Some(max) = max
                && val > *max
            {
                return (
                    ValidationOutcome::Fail,
                    format!("'{property}' = {val} is above maximum {max}"),
                );
            }
            (
                ValidationOutcome::Pass,
                format!("'{property}' = {val} is within range"),
            )
        }
    }
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
             construct_type TEXT NOT NULL,
             description    TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE rules_fts USING fts5(
             domain_id UNINDEXED,
             construct_id UNINDEXED,
             construct,
             text,
             layer UNINDEXED,
             rule_type UNINDEXED
         );
         CREATE TABLE relationships (
             id                TEXT PRIMARY KEY,
             domain_id         TEXT NOT NULL REFERENCES domains(id),
             from_construct_id TEXT NOT NULL REFERENCES constructs(id),
             to_construct_id   TEXT NOT NULL REFERENCES constructs(id),
             relationship_type TEXT NOT NULL,
             cardinality       TEXT NOT NULL,
             layer             TEXT NOT NULL,
             rule_type         TEXT NOT NULL
         );
         CREATE TABLE valid_relationships (
             domain_id         TEXT NOT NULL REFERENCES domains(id),
             from_type         TEXT NOT NULL,
             to_type           TEXT NOT NULL,
             relationship_type TEXT NOT NULL,
             cardinality       TEXT NOT NULL,
             PRIMARY KEY (domain_id, from_type, to_type, relationship_type)
         );
         CREATE TABLE conflicts (
             id            TEXT PRIMARY KEY,
             domain_id     TEXT NOT NULL REFERENCES domains(id),
             construct_id  TEXT REFERENCES constructs(id),
             layer_a       TEXT NOT NULL,
             layer_b       TEXT NOT NULL,
             conflict_type TEXT NOT NULL,
             description   TEXT NOT NULL,
             resolution    TEXT NOT NULL,
             rationale     TEXT,
             review_date   TEXT
         );
         CREATE TABLE cross_domain_relationships (
             id                 TEXT PRIMARY KEY,
             from_domain_id     TEXT NOT NULL REFERENCES domains(id),
             from_construct_id  TEXT NOT NULL REFERENCES constructs(id),
             to_domain_id       TEXT NOT NULL REFERENCES domains(id),
             to_construct_id    TEXT NOT NULL REFERENCES constructs(id),
             relationship_type  TEXT NOT NULL,
             description        TEXT,
             rationale          TEXT
         );
         CREATE TABLE rule_machine_checks (
             -- No FOREIGN KEY to rules_fts(rowid): SQLite doesn't support FK
             -- constraints referencing a virtual (FTS5) table's rowid.
             rule_rowid  INTEGER PRIMARY KEY,
             check_type  TEXT NOT NULL,
             property    TEXT NOT NULL,
             enum_values TEXT,
             pattern     TEXT,
             min_value   REAL,
             max_value   REAL
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

pub fn domain_by_id(conn: &Connection, domain_id: &str) -> rusqlite::Result<Option<Domain>> {
    let mut stmt = conn.prepare("SELECT id, name FROM domains WHERE id = ?1")?;
    let rows = stmt
        .query_map([domain_id], |row| {
            Ok(Domain {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().next())
}

/// Distinct authority layers with at least one rule in a domain --
/// `lookup.domain_summary`'s "layers" field, derived rather than tracked
/// separately, since it's fully determined by what's already in `rules_fts`.
pub fn layers_present_in_domain(
    conn: &Connection,
    domain_id: &str,
) -> rusqlite::Result<Vec<AuthorityLayer>> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT layer FROM rules_fts WHERE domain_id = ?1 ORDER BY layer")?;
    let rows = stmt.query_map([domain_id], |row| {
        let layer_text: String = row.get(0)?;
        Ok(AuthorityLayer::from_str(&layer_text))
    })?;
    rows.collect()
}

pub fn insert_construct(conn: &Connection, construct: &Construct) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO constructs (id, domain_id, short_name, construct_type, description)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            &construct.id,
            &construct.domain_id,
            &construct.short_name,
            &construct.construct_type,
            &construct.description,
        ),
    )?;
    Ok(())
}

/// Resolve a construct reference within a domain: tries an exact `short_name`
/// match first, then falls back to a direct ID lookup verified against the
/// domain (matching `knowledge-mcp`'s `_resolve` fallback order in `server.py`).
pub fn resolve_construct(
    conn: &Connection,
    domain_id: &str,
    construct_ref: &str,
) -> rusqlite::Result<Option<Construct>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain_id, short_name, construct_type, description
         FROM constructs WHERE domain_id = ?1 AND short_name = ?2",
    )?;
    let by_name = stmt
        .query_map((domain_id, construct_ref), construct_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if let Some(construct) = by_name.into_iter().next() {
        return Ok(Some(construct));
    }

    let mut stmt = conn.prepare(
        "SELECT id, domain_id, short_name, construct_type, description
         FROM constructs WHERE id = ?1 AND domain_id = ?2",
    )?;
    let by_id = stmt
        .query_map((construct_ref, domain_id), construct_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(by_id.into_iter().next())
}

fn construct_from_row(row: &rusqlite::Row) -> rusqlite::Result<Construct> {
    Ok(Construct {
        id: row.get(0)?,
        domain_id: row.get(1)?,
        short_name: row.get(2)?,
        construct_type: row.get(3)?,
        description: row.get(4)?,
    })
}

fn rule_from_row(row: &rusqlite::Row) -> rusqlite::Result<Rule> {
    let layer_text: String = row.get(4)?;
    let rule_type_text: String = row.get(5)?;
    Ok(Rule {
        domain_id: row.get(0)?,
        construct_id: row.get(1)?,
        construct: row.get(2)?,
        text: row.get(3)?,
        layer: AuthorityLayer::from_str(&layer_text),
        rule_type: RuleType::from_str(&rule_type_text),
    })
}

/// Rules attached to one construct, optionally filtered by authority layer
/// and/or rule type (MUST/SHALL/SHOULD/MAY/MUST_NOT).
pub fn rules_for_construct(
    conn: &Connection,
    construct_id: &str,
    layer: Option<AuthorityLayer>,
    rule_type: Option<RuleType>,
) -> rusqlite::Result<Vec<Rule>> {
    let layer_str = layer.map(AuthorityLayer::as_str);
    let rule_type_str = rule_type.map(RuleType::as_str);
    let mut stmt = conn.prepare(
        "SELECT domain_id, construct_id, construct, text, layer, rule_type
         FROM rules_fts
         WHERE construct_id = ?1
           AND (?2 IS NULL OR layer = ?2)
           AND (?3 IS NULL OR rule_type = ?3)",
    )?;
    let rows = stmt.query_map((construct_id, layer_str, rule_type_str), rule_from_row)?;
    rows.collect()
}

/// Returns the inserted row's `rowid`, so a caller that also wants to attach
/// a `MachineRule` (via `insert_machine_check`) has something to key it to --
/// `rules_fts` has no other stable per-row identifier.
pub fn insert_rule(conn: &Connection, rule: &Rule) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO rules_fts (domain_id, construct_id, construct, text, layer, rule_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            &rule.domain_id,
            &rule.construct_id,
            &rule.construct,
            &rule.text,
            rule.layer.as_str(),
            rule.rule_type.as_str(),
        ),
    )?;
    Ok(conn.last_insert_rowid())
}

/// Flattened, table-row-shaped view of a `MachineRule` -- avoids an
/// unwieldy tuple type for `insert_machine_check`'s internal decomposition.
struct MachineCheckRow<'a> {
    check_type: &'a str,
    property: &'a str,
    enum_values: Option<String>,
    pattern: Option<&'a str>,
    min_value: Option<f64>,
    max_value: Option<f64>,
}

pub fn insert_machine_check(
    conn: &Connection,
    rule_rowid: i64,
    check: &MachineRule,
) -> rusqlite::Result<()> {
    let row = match check {
        MachineRule::RequiredProperty { property } => MachineCheckRow {
            check_type: "required_property",
            property,
            enum_values: None,
            pattern: None,
            min_value: None,
            max_value: None,
        },
        MachineRule::EnumValue { property, values } => MachineCheckRow {
            check_type: "enum_value",
            property,
            enum_values: Some(values.join(",")),
            pattern: None,
            min_value: None,
            max_value: None,
        },
        MachineRule::Pattern { property, pattern } => MachineCheckRow {
            check_type: "pattern",
            property,
            enum_values: None,
            pattern: Some(pattern.as_str()),
            min_value: None,
            max_value: None,
        },
        MachineRule::Range { property, min, max } => MachineCheckRow {
            check_type: "range",
            property,
            enum_values: None,
            pattern: None,
            min_value: *min,
            max_value: *max,
        },
    };
    conn.execute(
        "INSERT INTO rule_machine_checks
             (rule_rowid, check_type, property, enum_values, pattern, min_value, max_value)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            rule_rowid,
            row.check_type,
            row.property,
            row.enum_values,
            row.pattern,
            row.min_value,
            row.max_value,
        ),
    )?;
    Ok(())
}

fn machine_rule_from_row(
    check_type: Option<String>,
    property: Option<String>,
    enum_values: Option<String>,
    pattern: Option<String>,
    min_value: Option<f64>,
    max_value: Option<f64>,
) -> Option<MachineRule> {
    let property = property?;
    match check_type?.as_str() {
        "required_property" => Some(MachineRule::RequiredProperty { property }),
        "enum_value" => Some(MachineRule::EnumValue {
            property,
            values: enum_values
                .unwrap_or_default()
                .split(',')
                .map(str::to_string)
                .collect(),
        }),
        "pattern" => Some(MachineRule::Pattern {
            property,
            pattern: pattern.unwrap_or_default(),
        }),
        "range" => Some(MachineRule::Range {
            property,
            min: min_value,
            max: max_value,
        }),
        _ => None,
    }
}

/// Rules for a construct alongside any `MachineRule` each carries --
/// `validate_element`'s input. `None` in the second tuple slot means the
/// rule is free text only, same as `knowledge-mcp`'s `if rule.machine_rule:`
/// guard.
pub fn rules_with_checks_for_construct(
    conn: &Connection,
    construct_id: &str,
    layer: Option<AuthorityLayer>,
) -> rusqlite::Result<Vec<(Rule, Option<MachineRule>)>> {
    let layer_str = layer.map(AuthorityLayer::as_str);
    let mut stmt = conn.prepare(
        "SELECT r.domain_id, r.construct_id, r.construct, r.text, r.layer, r.rule_type,
                m.check_type, m.property, m.enum_values, m.pattern, m.min_value, m.max_value
         FROM rules_fts r
         LEFT JOIN rule_machine_checks m ON m.rule_rowid = r.rowid
         WHERE r.construct_id = ?1 AND (?2 IS NULL OR r.layer = ?2)",
    )?;
    let rows = stmt.query_map((construct_id, layer_str), |row| {
        let rule = rule_from_row(row)?;
        let machine_rule = machine_rule_from_row(
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
        );
        Ok((rule, machine_rule))
    })?;
    rows.collect()
}

pub fn insert_relationship(conn: &Connection, rel: &Relationship) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO relationships
             (id, domain_id, from_construct_id, to_construct_id, relationship_type, cardinality, layer, rule_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            &rel.id,
            &rel.domain_id,
            &rel.from_construct_id,
            &rel.to_construct_id,
            &rel.relationship_type,
            &rel.cardinality,
            rel.layer.as_str(),
            rel.rule_type.as_str(),
        ),
    )?;
    Ok(())
}

/// Relationships originating from one construct, optionally narrowed to a
/// specific target construct and/or relationship type. Unlike
/// `knowledge-mcp`'s `lookup.relationships` (which silently drops an
/// unresolvable `to_construct_ref` filter rather than erroring), an
/// unresolvable `to_construct_id` here is the caller's job to resolve first
/// -- this function takes an already-resolved ID, not a ref string.
fn relationship_from_row(row: &rusqlite::Row) -> rusqlite::Result<Relationship> {
    let layer_text: String = row.get(6)?;
    let rule_type_text: String = row.get(7)?;
    Ok(Relationship {
        id: row.get(0)?,
        domain_id: row.get(1)?,
        from_construct_id: row.get(2)?,
        to_construct_id: row.get(3)?,
        relationship_type: row.get(4)?,
        cardinality: row.get(5)?,
        layer: AuthorityLayer::from_str(&layer_text),
        rule_type: RuleType::from_str(&rule_type_text),
    })
}

pub fn relationships_from(
    conn: &Connection,
    from_construct_id: &str,
    to_construct_id: Option<&str>,
    relationship_type: Option<&str>,
    rule_type: Option<RuleType>,
    layer: Option<AuthorityLayer>,
) -> rusqlite::Result<Vec<Relationship>> {
    let rule_type_str = rule_type.map(RuleType::as_str);
    let layer_str = layer.map(AuthorityLayer::as_str);
    let mut stmt = conn.prepare(
        "SELECT id, domain_id, from_construct_id, to_construct_id, relationship_type, cardinality, layer, rule_type
         FROM relationships
         WHERE from_construct_id = ?1
           AND (?2 IS NULL OR to_construct_id = ?2)
           AND (?3 IS NULL OR relationship_type = ?3)
           AND (?4 IS NULL OR rule_type = ?4)
           AND (?5 IS NULL OR layer = ?5)",
    )?;
    let rows = stmt.query_map(
        (
            from_construct_id,
            to_construct_id,
            relationship_type,
            rule_type_str,
            layer_str,
        ),
        relationship_from_row,
    )?;
    rows.collect()
}

/// Mirror of `relationships_from`, keyed by the target construct instead --
/// "what points at this construct" rather than "what this construct points
/// at". Needed for `crosscut.traceability`'s `must_be_traced_from` side,
/// which `knowledge-mcp` answers with a `to_construct_id`-keyed query.
pub fn relationships_to(
    conn: &Connection,
    to_construct_id: &str,
    from_construct_id: Option<&str>,
    relationship_type: Option<&str>,
    rule_type: Option<RuleType>,
    layer: Option<AuthorityLayer>,
) -> rusqlite::Result<Vec<Relationship>> {
    let rule_type_str = rule_type.map(RuleType::as_str);
    let layer_str = layer.map(AuthorityLayer::as_str);
    let mut stmt = conn.prepare(
        "SELECT id, domain_id, from_construct_id, to_construct_id, relationship_type, cardinality, layer, rule_type
         FROM relationships
         WHERE to_construct_id = ?1
           AND (?2 IS NULL OR from_construct_id = ?2)
           AND (?3 IS NULL OR relationship_type = ?3)
           AND (?4 IS NULL OR rule_type = ?4)
           AND (?5 IS NULL OR layer = ?5)",
    )?;
    let rows = stmt.query_map(
        (
            to_construct_id,
            from_construct_id,
            relationship_type,
            rule_type_str,
            layer_str,
        ),
        relationship_from_row,
    )?;
    rows.collect()
}

/// `validate.completeness`'s result: given a construct (e.g. a viewpoint)
/// and the element types actually present, what's required, what's missing,
/// and what's present but not required.
pub struct CompletenessReport {
    pub required_element_types: Vec<String>,
    pub present_element_types: Vec<String>,
    pub missing_required: Vec<String>,
    pub extra_present: Vec<String>,
    pub required_rule_texts: Vec<String>,
    pub recommended_rule_texts: Vec<String>,
    pub is_complete: bool,
}

/// "Required" element types come from `MUST`-typed relationships originating
/// at `construct_id` -- matching `knowledge-mcp`'s `evaluate_completeness`,
/// which reuses its relationship store the same way `validate.relationship`
/// does, filtered to `rule_type="MUST"`.
pub fn evaluate_completeness(
    conn: &Connection,
    construct_id: &str,
    present_element_types: &[String],
) -> rusqlite::Result<CompletenessReport> {
    let rules = rules_for_construct(conn, construct_id, None, None)?;
    let required_rule_texts: Vec<String> = rules
        .iter()
        .filter(|r| matches!(r.rule_type, RuleType::Must | RuleType::Shall))
        .map(|r| r.text.clone())
        .collect();
    let recommended_rule_texts: Vec<String> = rules
        .iter()
        .filter(|r| r.rule_type == RuleType::Should)
        .map(|r| r.text.clone())
        .collect();

    let rels = relationships_from(conn, construct_id, None, None, Some(RuleType::Must), None)?;
    let required_types: std::collections::BTreeSet<String> =
        rels.iter().map(|r| r.to_construct_id.clone()).collect();
    let present_set: std::collections::BTreeSet<String> =
        present_element_types.iter().cloned().collect();

    let missing_required: Vec<String> = required_types.difference(&present_set).cloned().collect();
    let extra_present: Vec<String> = present_set.difference(&required_types).cloned().collect();
    let is_complete = missing_required.is_empty();

    Ok(CompletenessReport {
        required_element_types: required_types.into_iter().collect(),
        present_element_types: present_element_types.to_vec(),
        missing_required,
        extra_present,
        required_rule_texts,
        recommended_rule_texts,
        is_complete,
    })
}

pub fn insert_valid_relationship(
    conn: &Connection,
    rule: &ValidRelationshipRule,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO valid_relationships (domain_id, from_type, to_type, relationship_type, cardinality)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (
            &rule.domain_id,
            &rule.from_type,
            &rule.to_type,
            &rule.relationship_type,
            &rule.cardinality,
        ),
    )?;
    Ok(())
}

/// All declared valid relationship types between two construct types in a
/// domain -- `RM-KNOWLEDGE-MODEL-0004`'s "declared valid-relationship set".
pub fn valid_relationships_between(
    conn: &Connection,
    domain_id: &str,
    from_type: &str,
    to_type: &str,
) -> rusqlite::Result<Vec<ValidRelationshipRule>> {
    let mut stmt = conn.prepare(
        "SELECT domain_id, from_type, to_type, relationship_type, cardinality
         FROM valid_relationships
         WHERE domain_id = ?1 AND from_type = ?2 AND to_type = ?3",
    )?;
    let rows = stmt.query_map((domain_id, from_type, to_type), |row| {
        Ok(ValidRelationshipRule {
            domain_id: row.get(0)?,
            from_type: row.get(1)?,
            to_type: row.get(2)?,
            relationship_type: row.get(3)?,
            cardinality: row.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn insert_conflict(conn: &Connection, conflict: &Conflict) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO conflicts
             (id, domain_id, construct_id, layer_a, layer_b, conflict_type, description, resolution, rationale, review_date)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        (
            &conflict.id,
            &conflict.domain_id,
            &conflict.construct_id,
            conflict.layer_a.as_str(),
            conflict.layer_b.as_str(),
            &conflict.conflict_type,
            &conflict.description,
            &conflict.resolution,
            &conflict.rationale,
            &conflict.review_date,
        ),
    )?;
    Ok(())
}

/// Conflict-registry entries for a domain, optionally narrowed to one
/// construct. Matching `knowledge-mcp`'s `get_conflicts`: when a
/// `construct_id` is given, this returns both that construct's own
/// conflicts *and* the domain's construct-independent (`construct_id IS
/// NULL`) ones -- a domain-level conflict is relevant no matter which
/// construct you asked about.
pub fn conflicts_for(
    conn: &Connection,
    domain_id: &str,
    construct_id: Option<&str>,
) -> rusqlite::Result<Vec<Conflict>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain_id, construct_id, layer_a, layer_b, conflict_type, description, resolution, rationale, review_date
         FROM conflicts
         WHERE domain_id = ?1 AND (?2 IS NULL OR construct_id = ?2 OR construct_id IS NULL)",
    )?;
    let rows = stmt.query_map((domain_id, construct_id), |row| {
        let layer_a_text: String = row.get(3)?;
        let layer_b_text: String = row.get(4)?;
        Ok(Conflict {
            id: row.get(0)?,
            domain_id: row.get(1)?,
            construct_id: row.get(2)?,
            layer_a: AuthorityLayer::from_str(&layer_a_text),
            layer_b: AuthorityLayer::from_str(&layer_b_text),
            conflict_type: row.get(5)?,
            description: row.get(6)?,
            resolution: row.get(7)?,
            rationale: row.get(8)?,
            review_date: row.get(9)?,
        })
    })?;
    rows.collect()
}

pub fn insert_cross_domain_relationship(
    conn: &Connection,
    rel: &CrossDomainRelationship,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO cross_domain_relationships
             (id, from_domain_id, from_construct_id, to_domain_id, to_construct_id, relationship_type, description, rationale)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            &rel.id,
            &rel.from_domain_id,
            &rel.from_construct_id,
            &rel.to_domain_id,
            &rel.to_construct_id,
            &rel.relationship_type,
            &rel.description,
            &rel.rationale,
        ),
    )?;
    Ok(())
}

/// Cross-domain relationships from a specific construct, optionally
/// narrowed to one target domain. Unlike same-domain `Relationship`s, the
/// target construct is never resolved against a live `constructs` row here
/// -- `knowledge-mcp`'s own `crosscut.cross_domain` doesn't do so either,
/// since the target domain (e.g. an external framework like RMF) may not be
/// loaded at all.
pub fn cross_domain_relationships_from(
    conn: &Connection,
    from_domain_id: &str,
    from_construct_id: &str,
    to_domain_id: Option<&str>,
) -> rusqlite::Result<Vec<CrossDomainRelationship>> {
    let mut stmt = conn.prepare(
        "SELECT id, from_domain_id, from_construct_id, to_domain_id, to_construct_id, relationship_type, description, rationale
         FROM cross_domain_relationships
         WHERE from_domain_id = ?1 AND from_construct_id = ?2
           AND (?3 IS NULL OR to_domain_id = ?3)",
    )?;
    let rows = stmt.query_map((from_domain_id, from_construct_id, to_domain_id), |row| {
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
    })?;
    rows.collect()
}

/// How a search response was produced. `RM-KNOWLEDGE-MODEL-0005` requires
/// this be declared on every search response, not silently omitted or
/// substituted -- there is deliberately no `Default` impl, so a caller can't
/// forget to pick one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalMode {
    /// FTS5 keyword match only. The only mode this crate can produce until
    /// rusty_knowledge#18 wires the existing (but unused) `vec0` table into
    /// search.
    LexicalOnly,
}

impl RetrievalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RetrievalMode::LexicalOnly => "lexical-only",
        }
    }
}

/// One ranked search hit: a rule plus its FTS5 rank (more negative = better
/// match, per SQLite's bm25-derived `rank` auxiliary column -- ascending
/// order is already best-first).
pub struct SearchHit {
    pub rule: Rule,
    pub rank: f64,
}

/// Domain/layer-scoped, ranked search — the `RM-KNOWLEDGE-MODEL-0005`-conforming
/// upgrade to plain `search`. `domain_id`/`layer` are optional filters; `None`
/// means unfiltered on that axis, matching the previous unscoped behavior when
/// both are `None`. Always returns `RetrievalMode::LexicalOnly` today; a caller
/// combining this with vector similarity (once rusty_knowledge#18 lands) would
/// report a different mode rather than this function silently claiming hybrid.
pub fn search_scoped(
    conn: &Connection,
    query: &str,
    domain_id: Option<&str>,
    layer: Option<AuthorityLayer>,
) -> rusqlite::Result<(Vec<SearchHit>, RetrievalMode)> {
    let layer_str = layer.map(AuthorityLayer::as_str);
    let mut stmt = conn.prepare(
        "SELECT domain_id, construct_id, construct, text, layer, rule_type, rank
         FROM rules_fts
         WHERE rules_fts MATCH ?1
           AND (?2 IS NULL OR domain_id = ?2)
           AND (?3 IS NULL OR layer = ?3)
         ORDER BY rank",
    )?;
    let rows = stmt.query_map((query, domain_id, layer_str), |row| {
        Ok(SearchHit {
            rule: rule_from_row(row)?,
            rank: row.get(6)?,
        })
    })?;
    Ok((
        rows.collect::<rusqlite::Result<Vec<_>>>()?,
        RetrievalMode::LexicalOnly,
    ))
}

/// Constructs belonging to exactly one domain — proves `RM-KNOWLEDGE-MODEL-0001`
/// (no cross-domain leakage) at the store level, ahead of the `meta.list_domains`
/// and `search.constructs` tools that will eventually wrap this query.
// Exercised by tests today; by `meta.list_domains`/`search.constructs`
// (rusty_knowledge#12/#16) next.
#[allow(dead_code)]
/// Constructs in a domain, optionally narrowed to one `construct_type`.
/// `knowledge-mcp`'s `search.constructs` also filters by `layer_num`, but
/// this crate's `Construct` doesn't carry an authority layer (only `Rule`
/// does) -- not modeled here, since a construct itself isn't layered, only
/// the rules attached to it are.
pub fn constructs_in_domain(
    conn: &Connection,
    domain_id: &str,
    construct_type: Option<&str>,
) -> rusqlite::Result<Vec<Construct>> {
    let mut stmt = conn.prepare(
        "SELECT id, domain_id, short_name, construct_type, description
         FROM constructs
         WHERE domain_id = ?1 AND (?2 IS NULL OR construct_type = ?2)",
    )?;
    let rows = stmt.query_map((domain_id, construct_type), construct_from_row)?;
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
            description: "A scoped, time-bounded grant of authority to act within a domain.".into(),
        },
    )?;
    insert_construct(
        conn,
        &Construct {
            id: "uaf-1.3:ConflictRegistryEntry".into(),
            domain_id: "uaf-1.3".into(),
            short_name: "ConflictRegistryEntry".into(),
            construct_type: "entity".into(),
            description: "A recorded contradiction between two rules across authority layers."
                .into(),
        },
    )?;
    insert_construct(
        conn,
        &Construct {
            id: "data-mesh:DataProduct".into(),
            domain_id: "data-mesh".into(),
            short_name: "DataProduct".into(),
            construct_type: "entity".into(),
            description: "A discoverable, owned unit of data published by a domain team.".into(),
        },
    )?;

    let authority_grant_scope_rule_id = insert_rule(
        conn,
        &Rule {
            domain_id: "uaf-1.3".into(),
            construct_id: "uaf-1.3:AuthorityGrant".into(),
            construct: "AuthorityGrant".into(),
            text: "An AuthorityGrant MUST declare an explicit scope and expiry.".into(),
            layer: AuthorityLayer::Standard,
            rule_type: RuleType::Must,
        },
    )?;
    insert_machine_check(
        conn,
        authority_grant_scope_rule_id,
        &MachineRule::RequiredProperty {
            property: "scope".into(),
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
            rule_type: RuleType::May,
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
            rule_type: RuleType::Must,
        },
    )?;
    let data_product_owning_team_rule_id = insert_rule(
        conn,
        &Rule {
            domain_id: "data-mesh".into(),
            construct_id: "data-mesh:DataProduct".into(),
            construct: "DataProduct".into(),
            text: "A DataProduct MUST declare an owning domain team.".into(),
            layer: AuthorityLayer::Standard,
            rule_type: RuleType::Must,
        },
    )?;
    insert_machine_check(
        conn,
        data_product_owning_team_rule_id,
        &MachineRule::Pattern {
            property: "owning_team".into(),
            pattern: "[a-z][a-z0-9-]*".into(),
        },
    )?;

    insert_relationship(
        conn,
        &Relationship {
            id: "uaf-1.3:AuthorityGrant-records-ConflictRegistryEntry".into(),
            domain_id: "uaf-1.3".into(),
            from_construct_id: "uaf-1.3:AuthorityGrant".into(),
            to_construct_id: "uaf-1.3:ConflictRegistryEntry".into(),
            relationship_type: "records".into(),
            cardinality: "0..*".into(),
            layer: AuthorityLayer::Standard,
            rule_type: RuleType::Must,
        },
    )?;

    insert_valid_relationship(
        conn,
        &ValidRelationshipRule {
            domain_id: "uaf-1.3".into(),
            from_type: "entity".into(),
            to_type: "entity".into(),
            relationship_type: "records".into(),
            cardinality: "0..*".into(),
        },
    )?;

    // Documents the exact contradiction the two AuthorityGrant rules above
    // already imply: Standard requires expiry, Conventions tolerates
    // omitting it. This is what a ConflictRegistryEntry (the construct these
    // rules reference) exists to record.
    insert_conflict(
        conn,
        &Conflict {
            id: "uaf-1.3:AuthorityGrant-standard-vs-conventions-expiry".into(),
            domain_id: "uaf-1.3".into(),
            construct_id: Some("uaf-1.3:AuthorityGrant".into()),
            layer_a: AuthorityLayer::Standard,
            layer_b: AuthorityLayer::Conventions,
            conflict_type: "contradiction".into(),
            description: "Standard requires every AuthorityGrant to declare an explicit expiry; \
                          convention in practice omits it for internal-only grants."
                .into(),
            resolution: "Standard wins: expiry is required regardless of convention.".into(),
            rationale: Some("Ungoverned indefinite grants are a security risk.".into()),
            review_date: Some("2027-01-01".into()),
        },
    )?;

    insert_cross_domain_relationship(
        conn,
        &CrossDomainRelationship {
            id: "uaf-1.3:AuthorityGrant-governs-data-mesh:DataProduct".into(),
            from_domain_id: "uaf-1.3".into(),
            from_construct_id: "uaf-1.3:AuthorityGrant".into(),
            to_domain_id: "data-mesh".into(),
            to_construct_id: "data-mesh:DataProduct".into(),
            relationship_type: "governs".into(),
            description: Some(
                "The scoped authority an AuthorityGrant confers is what permits a team to \
                 publish a DataProduct in the first place."
                    .into(),
            ),
            rationale: Some("Cross-domain compliance link between UAF and Data Mesh.".into()),
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

        let uaf = constructs_in_domain(&conn, "uaf-1.3", None).unwrap();
        assert_eq!(uaf.len(), 2);
        assert!(uaf.iter().all(|c| c.domain_id == "uaf-1.3"));
        assert!(uaf.iter().any(|c| c.short_name == "AuthorityGrant"));
        assert!(uaf.iter().any(|c| c.short_name == "ConflictRegistryEntry"));
        assert!(!uaf.iter().any(|c| c.short_name == "DataProduct"));

        let data_mesh = constructs_in_domain(&conn, "data-mesh", None).unwrap();
        assert_eq!(data_mesh.len(), 1);
        assert_eq!(data_mesh[0].short_name, "DataProduct");
    }

    #[test]
    fn constructs_in_domain_unknown_domain_returns_empty() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let none = constructs_in_domain(&conn, "does-not-exist", None).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn constructs_in_domain_filters_by_construct_type() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let entities = constructs_in_domain(&conn, "uaf-1.3", Some("entity")).unwrap();
        assert_eq!(entities.len(), 2);

        let none = constructs_in_domain(&conn, "uaf-1.3", Some("viewpoint")).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn relationships_from_returns_seeded_relationship() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels =
            relationships_from(&conn, "uaf-1.3:AuthorityGrant", None, None, None, None).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].to_construct_id, "uaf-1.3:ConflictRegistryEntry");
        assert_eq!(rels[0].relationship_type, "records");
    }

    #[test]
    fn relationships_from_filters_by_to_construct_and_type() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels = relationships_from(
            &conn,
            "uaf-1.3:AuthorityGrant",
            Some("uaf-1.3:ConflictRegistryEntry"),
            Some("records"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(rels.len(), 1);

        let none = relationships_from(
            &conn,
            "uaf-1.3:AuthorityGrant",
            None,
            Some("does-not-exist"),
            None,
            None,
        )
        .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn relationships_from_filters_by_rule_type() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let must_only = relationships_from(
            &conn,
            "uaf-1.3:AuthorityGrant",
            None,
            None,
            Some(RuleType::Must),
            None,
        )
        .unwrap();
        assert_eq!(must_only.len(), 1);

        let should_only = relationships_from(
            &conn,
            "uaf-1.3:AuthorityGrant",
            None,
            None,
            Some(RuleType::Should),
            None,
        )
        .unwrap();
        assert!(should_only.is_empty());
    }

    #[test]
    fn relationships_from_filters_by_layer() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let standard_only = relationships_from(
            &conn,
            "uaf-1.3:AuthorityGrant",
            None,
            None,
            None,
            Some(AuthorityLayer::Standard),
        )
        .unwrap();
        assert_eq!(standard_only.len(), 1);

        let process_only = relationships_from(
            &conn,
            "uaf-1.3:AuthorityGrant",
            None,
            None,
            None,
            Some(AuthorityLayer::Process),
        )
        .unwrap();
        assert!(process_only.is_empty());
    }

    #[test]
    fn relationships_from_construct_with_no_relationships_is_empty() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels =
            relationships_from(&conn, "data-mesh:DataProduct", None, None, None, None).unwrap();
        assert!(rels.is_empty());
    }

    #[test]
    fn relationships_to_returns_seeded_relationship() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels = relationships_to(
            &conn,
            "uaf-1.3:ConflictRegistryEntry",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].from_construct_id, "uaf-1.3:AuthorityGrant");
    }

    #[test]
    fn relationships_to_construct_with_no_incoming_relationships_is_empty() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels =
            relationships_to(&conn, "uaf-1.3:AuthorityGrant", None, None, None, None).unwrap();
        assert!(rels.is_empty());
    }

    #[test]
    fn valid_relationships_between_returns_seeded_rule() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rules = valid_relationships_between(&conn, "uaf-1.3", "entity", "entity").unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].relationship_type, "records");
    }

    #[test]
    fn valid_relationships_between_unknown_type_pair_is_empty() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rules = valid_relationships_between(&conn, "uaf-1.3", "entity", "viewpoint").unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn evaluate_completeness_missing_vs_present() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let complete = evaluate_completeness(
            &conn,
            "uaf-1.3:AuthorityGrant",
            &["uaf-1.3:ConflictRegistryEntry".to_string()],
        )
        .unwrap();
        assert!(complete.is_complete);
        assert!(complete.missing_required.is_empty());
        assert!(
            complete
                .required_rule_texts
                .iter()
                .any(|t| t.contains("scope and expiry"))
        );

        let incomplete = evaluate_completeness(&conn, "uaf-1.3:AuthorityGrant", &[]).unwrap();
        assert!(!incomplete.is_complete);
        assert_eq!(
            incomplete.missing_required,
            vec!["uaf-1.3:ConflictRegistryEntry".to_string()]
        );
    }

    #[test]
    fn evaluate_completeness_extra_present_does_not_block_completeness() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let report = evaluate_completeness(
            &conn,
            "uaf-1.3:AuthorityGrant",
            &[
                "uaf-1.3:ConflictRegistryEntry".to_string(),
                "something-unexpected".to_string(),
            ],
        )
        .unwrap();
        assert!(report.is_complete);
        assert_eq!(
            report.extra_present,
            vec!["something-unexpected".to_string()]
        );
    }

    #[test]
    fn evaluate_completeness_construct_with_no_required_relationships() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        // ConflictRegistryEntry has no outgoing MUST relationships seeded --
        // trivially complete regardless of what's present.
        let report = evaluate_completeness(&conn, "uaf-1.3:ConflictRegistryEntry", &[]).unwrap();
        assert!(report.is_complete);
        assert!(report.required_element_types.is_empty());
    }

    #[test]
    fn domain_by_id_returns_seeded_domain() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let domain = domain_by_id(&conn, "uaf-1.3").unwrap().unwrap();
        assert_eq!(domain.name, "UAF 1.3");

        assert!(domain_by_id(&conn, "does-not-exist").unwrap().is_none());
    }

    #[test]
    fn layers_present_in_domain_matches_seeded_rules() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let layers = layers_present_in_domain(&conn, "uaf-1.3").unwrap();
        assert!(layers.contains(&AuthorityLayer::Standard));
        assert!(layers.contains(&AuthorityLayer::Conventions));
        assert!(!layers.contains(&AuthorityLayer::Process));
    }

    #[test]
    fn rules_with_checks_for_construct_returns_seeded_machine_check() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rules = rules_with_checks_for_construct(&conn, "uaf-1.3:AuthorityGrant", None).unwrap();
        assert_eq!(rules.len(), 2);
        let with_check = rules.iter().filter(|(_, m)| m.is_some()).count();
        assert_eq!(with_check, 1);

        let (_, machine_rule) = rules.iter().find(|(_, m)| m.is_some()).unwrap();
        assert_eq!(
            machine_rule,
            &Some(MachineRule::RequiredProperty {
                property: "scope".into()
            })
        );
    }

    #[test]
    fn evaluate_machine_rule_required_property() {
        let check = MachineRule::RequiredProperty {
            property: "scope".into(),
        };
        let empty = std::collections::HashMap::new();
        let (outcome, _) = evaluate_machine_rule(&check, &empty);
        assert_eq!(outcome, ValidationOutcome::Fail);

        let present = std::collections::HashMap::from([("scope".to_string(), "org".to_string())]);
        let (outcome, _) = evaluate_machine_rule(&check, &present);
        assert_eq!(outcome, ValidationOutcome::Pass);
    }

    #[test]
    fn evaluate_machine_rule_enum_value() {
        let check = MachineRule::EnumValue {
            property: "status".into(),
            values: vec!["active".into(), "revoked".into()],
        };
        let valid = std::collections::HashMap::from([("status".to_string(), "active".to_string())]);
        assert_eq!(
            evaluate_machine_rule(&check, &valid).0,
            ValidationOutcome::Pass
        );

        let invalid =
            std::collections::HashMap::from([("status".to_string(), "pending".to_string())]);
        assert_eq!(
            evaluate_machine_rule(&check, &invalid).0,
            ValidationOutcome::Fail
        );
    }

    #[test]
    fn evaluate_machine_rule_range() {
        let check = MachineRule::Range {
            property: "priority".into(),
            min: Some(1.0),
            max: Some(5.0),
        };
        let in_range = std::collections::HashMap::from([("priority".to_string(), "3".to_string())]);
        assert_eq!(
            evaluate_machine_rule(&check, &in_range).0,
            ValidationOutcome::Pass
        );

        let out_of_range =
            std::collections::HashMap::from([("priority".to_string(), "9".to_string())]);
        assert_eq!(
            evaluate_machine_rule(&check, &out_of_range).0,
            ValidationOutcome::Fail
        );

        let non_numeric =
            std::collections::HashMap::from([("priority".to_string(), "high".to_string())]);
        assert_eq!(
            evaluate_machine_rule(&check, &non_numeric).0,
            ValidationOutcome::Fail
        );
    }

    #[test]
    fn evaluate_machine_rule_pattern_matches() {
        let check = MachineRule::Pattern {
            property: "id".into(),
            pattern: "[A-Z]+".into(),
        };
        let matching = std::collections::HashMap::from([("id".to_string(), "ABC".to_string())]);
        assert_eq!(
            evaluate_machine_rule(&check, &matching).0,
            ValidationOutcome::Pass
        );
    }

    #[test]
    fn evaluate_machine_rule_pattern_mismatch_is_warning_not_fail() {
        let check = MachineRule::Pattern {
            property: "id".into(),
            pattern: "[A-Z]+".into(),
        };
        // Doesn't match at position 0 -- rusty_regx's unanchored `find` would
        // otherwise find "ABC" mid-string; requiring start()==0 replicates
        // Python's re.match (anchored-at-start) semantics.
        let mismatch = std::collections::HashMap::from([("id".to_string(), "1ABC".to_string())]);
        let (outcome, message) = evaluate_machine_rule(&check, &mismatch);
        assert_eq!(outcome, ValidationOutcome::Warning);
        assert!(message.contains("does not match"));

        let absent = std::collections::HashMap::new();
        assert_eq!(
            evaluate_machine_rule(&check, &absent).0,
            ValidationOutcome::Warning
        );
    }

    #[test]
    fn evaluate_machine_rule_invalid_pattern_is_warning_not_a_panic() {
        let check = MachineRule::Pattern {
            property: "id".into(),
            pattern: "[unclosed".into(),
        };
        let (outcome, message) = evaluate_machine_rule(&check, &std::collections::HashMap::new());
        assert_eq!(outcome, ValidationOutcome::Warning);
        assert!(message.contains("invalid"));
    }

    #[test]
    fn search_scoped_unfiltered_matches_plain_search() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let (hits, mode) = search_scoped(&conn, "AuthorityGrant", None, None).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(mode, RetrievalMode::LexicalOnly);
    }

    #[test]
    fn search_scoped_filters_by_domain() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let (hits, _) = search_scoped(&conn, "DataProduct", Some("uaf-1.3"), None).unwrap();
        assert!(
            hits.is_empty(),
            "data-mesh's DataProduct must not leak into a uaf-1.3-scoped search"
        );

        let (hits, _) = search_scoped(&conn, "DataProduct", Some("data-mesh"), None).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn search_scoped_filters_by_layer() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let (hits, _) = search_scoped(
            &conn,
            "AuthorityGrant",
            None,
            Some(AuthorityLayer::Conventions),
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule.layer, AuthorityLayer::Conventions);
    }

    #[test]
    fn search_scoped_always_declares_lexical_only_today() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        // RM-KNOWLEDGE-MODEL-0005: the mode must be declared, never silently
        // substituted -- until rusty_knowledge#18 wires vector retrieval in,
        // every response is lexical-only, and this test pins that down.
        let (_, mode) = search_scoped(&conn, "AuthorityGrant", None, None).unwrap();
        assert_eq!(mode, RetrievalMode::LexicalOnly);
    }

    #[test]
    fn conflicts_for_returns_seeded_conflict() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let conflicts = conflicts_for(&conn, "uaf-1.3", None).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].conflict_type, "contradiction");
        assert_eq!(
            conflicts[0].construct_id.as_deref(),
            Some("uaf-1.3:AuthorityGrant")
        );
    }

    #[test]
    fn conflicts_for_returns_construct_level_and_domain_level() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        // A second, domain-level conflict on top of the seeded
        // construct-level one.
        insert_conflict(
            &conn,
            &Conflict {
                id: "conflict-domain-wide".into(),
                domain_id: "uaf-1.3".into(),
                construct_id: None,
                layer_a: AuthorityLayer::Standard,
                layer_b: AuthorityLayer::Process,
                conflict_type: "gap".into(),
                description: "Domain-wide gap between spec and process.".into(),
                resolution: "Process to be updated.".into(),
                rationale: None,
                review_date: None,
            },
        )
        .unwrap();

        // Unscoped: both conflicts for the domain.
        let all = conflicts_for(&conn, "uaf-1.3", None).unwrap();
        assert_eq!(all.len(), 2);

        // Scoped to the construct with its own conflict: both apply.
        let scoped = conflicts_for(&conn, "uaf-1.3", Some("uaf-1.3:AuthorityGrant")).unwrap();
        assert_eq!(scoped.len(), 2);

        // Scoped to a construct with no conflicts of its own: only the
        // domain-level one still applies.
        let other = conflicts_for(&conn, "uaf-1.3", Some("uaf-1.3:ConflictRegistryEntry")).unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].id, "conflict-domain-wide");
    }

    #[test]
    fn conflicts_for_domain_with_no_conflicts_is_empty() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let none = conflicts_for(&conn, "data-mesh", None).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn cross_domain_relationships_from_returns_seeded_relationship() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels =
            cross_domain_relationships_from(&conn, "uaf-1.3", "uaf-1.3:AuthorityGrant", None)
                .unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].to_domain_id, "data-mesh");
        assert_eq!(rels[0].to_construct_id, "data-mesh:DataProduct");
        assert_eq!(rels[0].relationship_type, "governs");
    }

    #[test]
    fn cross_domain_relationships_from_filters_by_to_domain() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let matching = cross_domain_relationships_from(
            &conn,
            "uaf-1.3",
            "uaf-1.3:AuthorityGrant",
            Some("data-mesh"),
        )
        .unwrap();
        assert_eq!(matching.len(), 1);

        let none = cross_domain_relationships_from(
            &conn,
            "uaf-1.3",
            "uaf-1.3:AuthorityGrant",
            Some("does-not-exist"),
        )
        .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn cross_domain_relationships_from_construct_with_none_is_empty() {
        let conn = open_store().unwrap();
        seed(&conn).unwrap();

        let rels =
            cross_domain_relationships_from(&conn, "data-mesh", "data-mesh:DataProduct", None)
                .unwrap();
        assert!(rels.is_empty());
    }
}
