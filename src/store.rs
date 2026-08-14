//! The knowledge-model-v2 store: `Source`, `SourceAuthority`, `Subject`,
//! `Rule`, `RuleRelation`, and `SelectionGroup` (a cardinality constraint
//! over a set of relationship-shaped Rules, e.g. "must have both X and Y"
//! -- backs `validate_completeness`), replacing the earlier
//! `AuthorityLayer`/`Construct`/fixed-4-layer model. The fuller
//! seven-table design this was built from also specifies `RuleDerivation`
//! (firewalled, non-authoritative rollup views), deliberately not
//! implemented yet since nothing in the current tool surface needs it.
//! Add it when a real case does, not speculatively.
//!
//! This redesign came out of stress-testing the original design against
//! UDRA (a nested-organization authority chain that doesn't fit a fixed
//! Standard/Tool/Convention/Process taxonomy), UAF (which needs exact,
//! drift-proof construct identity -- `Subject`), a multi-parent-authority
//! case (an org answering to two independent authorities at once --
//! `SourceAuthority` is a DAG, not a tree), supersession over time (`Rule`
//! content changes cascade hard into `RuleRelation.status`; `Source`/
//! `Subject` identity changes cascade soft, as a review flag only), and
//! NIST RMF (rules that need to be machine-checked against a real system's
//! state, not just read by a human -- `Rule.machine_check`).
//!
//! This started as a vertical slice proving the model against real UDRA
//! data end-to-end: schema, insert-time invariants (DAG cycle rejection,
//! supersession cascade), the two-tier conflict-candidate query, and two
//! MCP tools (`lookup_subject`, `crosscut_conflicts` -- see `main.rs`).
//! It's since grown to the full 16-tool surface tracked in
//! rusty_knowledge#55, including a lexical (FTS5) `search_knowledge` --
//! deliberately no vector/hybrid component, since the previous model's
//! `Embedder`/`sqlite-vec` infrastructure was removed entirely along with
//! the schema this replaces and isn't reintroduced here. File-backed
//! persistence is likewise not carried forward yet.

use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};

/// A rule or relationship's normative strength. `Delegated` means the
/// issuing Source explicitly hands the decision to whichever Source(s)
/// answer to it -- a descendant Rule at `Must` under a `Delegated` parent
/// is expected, not a conflict (see `conflict_candidates_for_subject`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingStrength {
    Must,
    MustNot,
    Should,
    ShouldNot,
    May,
    Delegated,
}

impl BindingStrength {
    pub fn as_str(self) -> &'static str {
        match self {
            BindingStrength::Must => "MUST",
            BindingStrength::MustNot => "MUST_NOT",
            BindingStrength::Should => "SHOULD",
            BindingStrength::ShouldNot => "SHOULD_NOT",
            BindingStrength::May => "MAY",
            BindingStrength::Delegated => "DELEGATED",
        }
    }

    /// Trusted-storage parser: rows we wrote ourselves. Panics on
    /// corruption rather than silently misreporting a rule's strength.
    fn from_str(text: &str) -> Self {
        match text {
            "MUST" => BindingStrength::Must,
            "MUST_NOT" => BindingStrength::MustNot,
            "SHOULD" => BindingStrength::Should,
            "SHOULD_NOT" => BindingStrength::ShouldNot,
            "MAY" => BindingStrength::May,
            "DELEGATED" => BindingStrength::Delegated,
            other => panic!("stored binding_strength {other:?} is not one of the six known values"),
        }
    }

    /// Untrusted-input parser: used by `knowledge_mcp_import_v2` reading
    /// an external SQLite file. Never panics.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "MUST" => Some(BindingStrength::Must),
            "MUST_NOT" => Some(BindingStrength::MustNot),
            "SHOULD" => Some(BindingStrength::Should),
            "SHOULD_NOT" => Some(BindingStrength::ShouldNot),
            "MAY" => Some(BindingStrength::May),
            "DELEGATED" => Some(BindingStrength::Delegated),
            _ => None,
        }
    }
}

/// The fixed vocabulary for rule-to-rule relations (`RuleRelation`).
/// Distinct from `Rule.relationship_type`, which is a free/extensible tag
/// for *subject*-to-subject claims (e.g. "contains", "precedes").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationType {
    Extends,
    Restricts,
    Implements,
    Satisfies,
    ConflictsWith,
    Requires,
    Excludes,
    NoRelation,
}

impl RelationType {
    pub fn as_str(self) -> &'static str {
        match self {
            RelationType::Extends => "extends",
            RelationType::Restricts => "restricts",
            RelationType::Implements => "implements",
            RelationType::Satisfies => "satisfies",
            RelationType::ConflictsWith => "conflicts_with",
            RelationType::Requires => "requires",
            RelationType::Excludes => "excludes",
            RelationType::NoRelation => "no_relation",
        }
    }

    fn from_str(text: &str) -> Self {
        match text {
            "extends" => RelationType::Extends,
            "restricts" => RelationType::Restricts,
            "implements" => RelationType::Implements,
            "satisfies" => RelationType::Satisfies,
            "conflicts_with" => RelationType::ConflictsWith,
            "requires" => RelationType::Requires,
            "excludes" => RelationType::Excludes,
            "no_relation" => RelationType::NoRelation,
            other => panic!("stored relation_type {other:?} is not one of the eight known values"),
        }
    }

    // No untrusted-input `parse` yet, same reasoning as `BindingStrength`.
}

/// Whether a `RuleRelation` still reflects the current text of both rules
/// it links. Flips to `Stale` automatically when either side is
/// superseded (see `insert_rule`) -- never deleted or silently rewritten,
/// so the row stays as an audit record of what was confirmed and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationStatus {
    Active,
    Stale,
}

impl RelationStatus {
    fn as_str(self) -> &'static str {
        match self {
            RelationStatus::Active => "active",
            RelationStatus::Stale => "stale",
        }
    }

    fn from_str(text: &str) -> Self {
        match text {
            "active" => RelationStatus::Active,
            "stale" => RelationStatus::Stale,
            other => panic!("stored relation status {other:?} is not \"active\" or \"stale\""),
        }
    }
}

/// The authority node -- anything that can issue a `Rule`. Nesting is
/// expressed separately via `SourceAuthority` (a DAG: a Source may answer
/// to more than one independent parent), not a column on this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Populated only on true roots (no parent edges in `SourceAuthority`).
    pub domain_tags: Vec<String>,
    pub steward: Option<String>,
    pub citation: Option<String>,
    pub supersedes_source_id: Option<String>,
}

/// The thing a `Rule` is about -- canonical identity, independent of who's
/// making claims about it. `parent_subject_id` is a *concept* hierarchy
/// (e.g. a stereotype's supertype), deliberately independent of any
/// Source's authority position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub id: String,
    pub domain_tag: String,
    pub subject_type: String,
    pub name: String,
    pub short_name: String,
    pub description: Option<String>,
    pub is_deprecated: bool,
    pub parent_subject_id: Option<String>,
    pub supersedes_subject_id: Option<String>,
    /// Section reference in the Subject's originating source document,
    /// e.g. "6.2" for a UAF 1.3 spec section. Optional -- most domains
    /// won't have this, but real UAF/UDRA/Data Mesh content does.
    pub source_section: Option<String>,
}

/// The ground-truth requirement/statement. A relationship claim between
/// two subjects (e.g. "X must contain at least one Y") is expressed here
/// too, via `related_subject_id`/`relationship_type`/`cardinality`,
/// rather than in a separate table -- it has the same shape as any other
/// rule (comes from a Source, carries a binding strength, can be
/// superseded, participates in the conflict gate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub source_id: String,
    pub subject_id: String,
    pub related_subject_id: Option<String>,
    pub relationship_type: Option<String>,
    pub cardinality: Option<String>,
    pub statement: String,
    /// Structured, machine-parseable comparison logic (JSON), present
    /// only when this rule is actually checkable against a real system's
    /// state. Carries the same authority as `statement` -- it's the same
    /// requirement, just also machine-parseable.
    pub machine_check: Option<String>,
    pub binding_strength: BindingStrength,
    pub supersedes_rule_id: Option<String>,
}

/// The structured shape a `Rule.machine_check` JSON blob can take.
/// Internally tagged on `"check"`, e.g. `{"check":"pattern","property":
/// "owner_email","pattern":"^[^@]+@[^@]+\\.[^@]+$"}`.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "check", rename_all = "snake_case")]
enum MachineCheck {
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
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
    },
    /// Not evaluated -- always a `Warning`. `knowledge-mcp`'s own schema
    /// allowed a `"custom"` check with no defined evaluation semantics;
    /// this crate doesn't invent one, it just says so honestly rather
    /// than silently treating it as a pass.
    Custom,
}

/// The outcome of evaluating one `machine_check` against a real value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    Pass,
    Fail(String),
    Warning(String),
}

/// Evaluates a `Rule.machine_check` JSON blob against `properties` (a
/// flat `name -> value` map of what's actually true for some real
/// element). A missing property is always a `Fail` -- the check couldn't
/// even run. A present-but-wrong value is a `Fail` for
/// `required_property`/`enum_value`/`range` (hard structural violations)
/// but only a `Warning` for `pattern` (style/format guidance is treated
/// as advisory, not a hard block) -- same distinction `knowledge-mcp`'s
/// own evaluator drew. An unparseable `machine_check` or an invalid regex
/// pattern is a `Warning`, never a panic -- a malformed rule shouldn't
/// take the whole validation call down.
pub fn evaluate_machine_check(
    check_json: &str,
    properties: &HashMap<String, String>,
) -> CheckResult {
    let check: MachineCheck = match serde_json::from_str(check_json) {
        Ok(check) => check,
        Err(err) => return CheckResult::Warning(format!("machine_check is not valid JSON: {err}")),
    };

    match check {
        MachineCheck::RequiredProperty { property } => {
            if properties.contains_key(&property) {
                CheckResult::Pass
            } else {
                CheckResult::Fail(format!("required property {property:?} is missing"))
            }
        }
        MachineCheck::EnumValue { property, values } => match properties.get(&property) {
            None => CheckResult::Fail(format!("required property {property:?} is missing")),
            Some(value) if values.iter().any(|v| v == value) => CheckResult::Pass,
            Some(value) => {
                CheckResult::Fail(format!("{property:?} = {value:?} is not one of {values:?}"))
            }
        },
        MachineCheck::Pattern { property, pattern } => match properties.get(&property) {
            None => CheckResult::Fail(format!("required property {property:?} is missing")),
            Some(value) => match rusty_regx::Regex::new(&pattern) {
                Err(err) => CheckResult::Warning(format!("invalid pattern {pattern:?}: {err}")),
                Ok(regex) if regex.is_match(value) => CheckResult::Pass,
                Ok(_) => CheckResult::Warning(format!(
                    "{property:?} = {value:?} does not match pattern {pattern:?}"
                )),
            },
        },
        MachineCheck::Range { property, min, max } => match properties.get(&property) {
            None => CheckResult::Fail(format!("required property {property:?} is missing")),
            Some(value) => match value.parse::<f64>() {
                Err(_) => CheckResult::Fail(format!("{property:?} = {value:?} is not a number")),
                Ok(number) => {
                    if let Some(min) = min
                        && number < min
                    {
                        return CheckResult::Fail(format!(
                            "{property:?} = {number} is below minimum {min}"
                        ));
                    }
                    if let Some(max) = max
                        && number > max
                    {
                        return CheckResult::Fail(format!(
                            "{property:?} = {number} is above maximum {max}"
                        ));
                    }
                    CheckResult::Pass
                }
            },
        },
        MachineCheck::Custom => CheckResult::Warning("custom checks are not evaluated".to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRelation {
    pub rule_a_id: String,
    pub rule_b_id: String,
    pub relation_type: RelationType,
    pub status: RelationStatus,
    pub confirmed_by: String,
}

/// How many of a `SelectionGroup`'s member Rules must be satisfied for the
/// group as a whole to be satisfied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionConstraint {
    /// Every member rule must be satisfied.
    All,
    /// At least `n` of the member rules must be satisfied.
    AtLeast(u32),
}

impl SelectionConstraint {
    fn as_str(self) -> &'static str {
        match self {
            SelectionConstraint::All => "all",
            SelectionConstraint::AtLeast(_) => "at_least",
        }
    }

    fn threshold(self) -> Option<i64> {
        match self {
            SelectionConstraint::All => None,
            SelectionConstraint::AtLeast(n) => Some(n as i64),
        }
    }

    fn from_row(constraint_type: &str, threshold: Option<i64>) -> Self {
        match constraint_type {
            "all" => SelectionConstraint::All,
            "at_least" => {
                SelectionConstraint::AtLeast(threshold.unwrap_or_else(|| {
                    panic!("stored \"at_least\" selection_group has no threshold")
                }) as u32)
            }
            other => panic!(
                "stored selection_group constraint_type {other:?} is not \"all\" or \"at_least\""
            ),
        }
    }
}

/// A cardinality constraint over a set of relationship-shaped Rules on one
/// Subject -- e.g. "a complete DataProduct must satisfy every rule in this
/// group" (`All`) or "at least 2 of these 3" (`AtLeast(2)`). Backs
/// `validate_completeness`. Distinct from a single Rule's own
/// `cardinality` field, which constrains how many *instances* of one
/// relationship must exist -- a `SelectionGroup` instead picks out which
/// subset of several *different* rules must hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionGroup {
    pub id: String,
    pub subject_id: String,
    pub description: String,
    pub constraint: SelectionConstraint,
    pub member_rule_ids: Vec<String>,
}

/// A synthesized rollup over a set of Rules about one Subject -- e.g. "the
/// combined effective guidance," written by a human/process reading
/// several individual Rules and summarizing them into one text.
/// **Firewalled from authority, deliberately**: a `RuleDerivation` is
/// never itself a `Rule` (it has no `binding_strength`, no
/// `machine_check`, no `id` a `RuleRelation` can reference), is never
/// returned by `rules_for_subject`/`statement_rules_for_subject`/etc, and
/// never participates in the conflict gate. It fully discloses which
/// Rules it was synthesized from (`source_rule_ids`) so a reader can go
/// verify against the ground truth rather than citing the rollup itself
/// as authoritative -- the same "disclose, don't fabricate" posture as
/// `knowledge_mcp_import_v2`'s `ImportReport`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDerivation {
    pub id: String,
    pub subject_id: String,
    pub label: String,
    pub summary: String,
    pub source_rule_ids: Vec<String>,
}

fn schema_ddl() -> &'static str {
    "
    CREATE TABLE sources (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        domain_tags TEXT NOT NULL DEFAULT '[]',
        steward TEXT,
        citation TEXT,
        supersedes_source_id TEXT REFERENCES sources(id)
    );

    CREATE TABLE source_authority (
        child_source_id TEXT NOT NULL REFERENCES sources(id),
        parent_source_id TEXT NOT NULL REFERENCES sources(id),
        PRIMARY KEY (child_source_id, parent_source_id)
    );

    CREATE TABLE subjects (
        id TEXT PRIMARY KEY,
        domain_tag TEXT NOT NULL,
        subject_type TEXT NOT NULL,
        name TEXT NOT NULL,
        short_name TEXT NOT NULL,
        description TEXT,
        is_deprecated INTEGER NOT NULL DEFAULT 0,
        parent_subject_id TEXT REFERENCES subjects(id),
        supersedes_subject_id TEXT REFERENCES subjects(id),
        source_section TEXT
    );
    CREATE INDEX idx_subjects_short ON subjects(domain_tag, short_name);

    CREATE TABLE rules (
        id TEXT PRIMARY KEY,
        source_id TEXT NOT NULL REFERENCES sources(id),
        subject_id TEXT NOT NULL REFERENCES subjects(id),
        related_subject_id TEXT REFERENCES subjects(id),
        relationship_type TEXT,
        cardinality TEXT,
        statement TEXT NOT NULL,
        machine_check TEXT,
        binding_strength TEXT NOT NULL,
        supersedes_rule_id TEXT REFERENCES rules(id)
    );
    CREATE INDEX idx_rules_subject ON rules(subject_id);
    CREATE INDEX idx_rules_related_subject ON rules(related_subject_id);
    CREATE INDEX idx_rules_source ON rules(source_id);

    CREATE TABLE rule_relations (
        rule_a_id TEXT NOT NULL REFERENCES rules(id),
        rule_b_id TEXT NOT NULL REFERENCES rules(id),
        relation_type TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'active',
        confirmed_by TEXT NOT NULL,
        PRIMARY KEY (rule_a_id, rule_b_id)
    );

    CREATE TABLE selection_groups (
        id TEXT PRIMARY KEY,
        subject_id TEXT NOT NULL REFERENCES subjects(id),
        description TEXT NOT NULL,
        constraint_type TEXT NOT NULL,
        threshold INTEGER
    );
    CREATE INDEX idx_selection_groups_subject ON selection_groups(subject_id);

    CREATE TABLE selection_group_members (
        group_id TEXT NOT NULL REFERENCES selection_groups(id),
        rule_id TEXT NOT NULL REFERENCES rules(id),
        PRIMARY KEY (group_id, rule_id)
    );

    CREATE VIRTUAL TABLE search_index USING fts5(
        ref_id UNINDEXED,
        ref_type UNINDEXED,
        text
    );

    CREATE TABLE rule_derivations (
        id TEXT PRIMARY KEY,
        subject_id TEXT NOT NULL REFERENCES subjects(id),
        label TEXT NOT NULL,
        summary TEXT NOT NULL
    );
    CREATE INDEX idx_rule_derivations_subject ON rule_derivations(subject_id);

    CREATE TABLE rule_derivation_sources (
        derivation_id TEXT NOT NULL REFERENCES rule_derivations(id),
        rule_id TEXT NOT NULL REFERENCES rules(id),
        PRIMARY KEY (derivation_id, rule_id)
    );
    "
}

pub fn open_store() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(schema_ddl())?;
    Ok(conn)
}

const SOURCE_COLUMNS: &str = "id, name, kind, domain_tags, steward, citation, supersedes_source_id";

fn source_from_row(row: &rusqlite::Row) -> rusqlite::Result<Source> {
    let domain_tags_json: String = row.get(3)?;
    Ok(Source {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        domain_tags: serde_json::from_str(&domain_tags_json).unwrap_or_default(),
        steward: row.get(4)?,
        citation: row.get(5)?,
        supersedes_source_id: row.get(6)?,
    })
}

pub fn insert_source(conn: &Connection, source: &Source) -> rusqlite::Result<()> {
    let domain_tags_json = serde_json::to_string(&source.domain_tags).unwrap_or_default();
    conn.execute(
        &format!("INSERT INTO sources ({SOURCE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
        params![
            source.id,
            source.name,
            source.kind,
            domain_tags_json,
            source.steward,
            source.citation,
            source.supersedes_source_id,
        ],
    )?;
    Ok(())
}

/// Every `Source`, ordered by id -- backs `meta_list_domains` and
/// `lookup_domain_summary`'s "which Sources root this domain" question,
/// since `domain_tags` is only ever populated on root Sources.
pub fn all_sources(conn: &Connection) -> rusqlite::Result<Vec<Source>> {
    let mut stmt = conn.prepare(&format!("SELECT {SOURCE_COLUMNS} FROM sources ORDER BY id"))?;
    stmt.query_map([], source_from_row)?.collect()
}

pub fn source_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<Source>> {
    conn.query_row(
        &format!("SELECT {SOURCE_COLUMNS} FROM sources WHERE id = ?1"),
        params![id],
        source_from_row,
    )
    .optional()
}

/// All ancestors of `source_id` reachable through any parent-edge path in
/// `source_authority` -- the full transitive closure over the DAG, not a
/// single lineage. This is what makes multi-parent authority (an org
/// answering to two independent frameworks at once) visible to the
/// conflict gate: walking only one parent would make the other parent's
/// rules invisible to enforcement.
pub fn ancestors_of(conn: &Connection, source_id: &str) -> rusqlite::Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE ancestors(id) AS (
            SELECT parent_source_id FROM source_authority WHERE child_source_id = ?1
            UNION
            SELECT sa.parent_source_id
            FROM source_authority sa
            JOIN ancestors a ON sa.child_source_id = a.id
        )
        SELECT id FROM ancestors",
    )?;
    let rows = stmt.query_map(params![source_id], |row| row.get::<_, String>(0))?;
    rows.collect()
}

fn is_ancestor(conn: &Connection, candidate: &str, of_source: &str) -> rusqlite::Result<bool> {
    Ok(ancestors_of(conn, of_source)?.contains(candidate))
}

/// Adds a `child answers to parent` edge, rejecting anything that would
/// make `source_authority` a cyclic graph (a self-loop, or `parent`
/// already answering -- transitively -- to `child`). SQLite has no native
/// DAG-acyclicity constraint, so this is enforced here, in application
/// logic, before the write.
pub fn insert_source_authority_edge(
    conn: &Connection,
    child_source_id: &str,
    parent_source_id: &str,
) -> Result<(), String> {
    if child_source_id == parent_source_id {
        return Err(format!("{child_source_id:?} cannot answer to itself"));
    }
    let would_cycle =
        is_ancestor(conn, child_source_id, parent_source_id).map_err(|err| err.to_string())?;
    if would_cycle {
        return Err(format!(
            "{parent_source_id:?} already answers (transitively) to {child_source_id:?}; \
             adding this edge would create a cycle"
        ));
    }
    conn.execute(
        "INSERT INTO source_authority (child_source_id, parent_source_id) VALUES (?1, ?2)",
        params![child_source_id, parent_source_id],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

const SUBJECT_COLUMNS: &str = "id, domain_tag, subject_type, name, short_name, description, \
     is_deprecated, parent_subject_id, supersedes_subject_id, source_section";

fn subject_from_row(row: &rusqlite::Row) -> rusqlite::Result<Subject> {
    Ok(Subject {
        id: row.get(0)?,
        domain_tag: row.get(1)?,
        subject_type: row.get(2)?,
        name: row.get(3)?,
        short_name: row.get(4)?,
        description: row.get(5)?,
        is_deprecated: row.get::<_, i64>(6)? != 0,
        parent_subject_id: row.get(7)?,
        supersedes_subject_id: row.get(8)?,
        source_section: row.get(9)?,
    })
}

/// Adds one row to the `search_index` FTS5 table. Called from
/// `insert_rule`/`insert_subject` so the index is kept in sync
/// incrementally, at write time -- never rebuilt per `search_knowledge`
/// call.
fn index_for_search(
    conn: &Connection,
    ref_id: &str,
    ref_type: &str,
    text: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO search_index (ref_id, ref_type, text) VALUES (?1, ?2, ?3)",
        params![ref_id, ref_type, text],
    )?;
    Ok(())
}

pub fn insert_subject(conn: &Connection, subject: &Subject) -> rusqlite::Result<()> {
    conn.execute(
        &format!("INSERT INTO subjects ({SUBJECT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"),
        params![
            subject.id,
            subject.domain_tag,
            subject.subject_type,
            subject.name,
            subject.short_name,
            subject.description,
            subject.is_deprecated as i64,
            subject.parent_subject_id,
            subject.supersedes_subject_id,
            subject.source_section,
        ],
    )?;
    index_for_search(
        conn,
        &subject.id,
        "subject",
        &format!(
            "{} {} {}",
            subject.name,
            subject.short_name,
            subject.description.as_deref().unwrap_or("")
        ),
    )?;
    Ok(())
}

pub fn subject_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<Subject>> {
    conn.query_row(
        &format!("SELECT {SUBJECT_COLUMNS} FROM subjects WHERE id = ?1"),
        params![id],
        subject_from_row,
    )
    .optional()
}

/// Resolves a subject reference by exact short name first (within
/// `domain_tag`), then by direct ID match -- mirroring the previous
/// model's `resolve_construct` convention.
pub fn resolve_subject(
    conn: &Connection,
    domain_tag: &str,
    subject_ref: &str,
) -> rusqlite::Result<Option<Subject>> {
    let by_short_name = conn
        .query_row(
            &format!(
                "SELECT {SUBJECT_COLUMNS} FROM subjects WHERE domain_tag = ?1 AND short_name = ?2"
            ),
            params![domain_tag, subject_ref],
            subject_from_row,
        )
        .optional()?;
    if by_short_name.is_some() {
        return Ok(by_short_name);
    }
    subject_by_id(conn, subject_ref)
}

fn rule_from_row(row: &rusqlite::Row) -> rusqlite::Result<Rule> {
    let binding_strength_text: String = row.get(8)?;
    Ok(Rule {
        id: row.get(0)?,
        source_id: row.get(1)?,
        subject_id: row.get(2)?,
        related_subject_id: row.get(3)?,
        relationship_type: row.get(4)?,
        cardinality: row.get(5)?,
        statement: row.get(6)?,
        machine_check: row.get(7)?,
        binding_strength: BindingStrength::from_str(&binding_strength_text),
        supersedes_rule_id: row.get(9)?,
    })
}

const RULE_COLUMNS: &str = "id, source_id, subject_id, related_subject_id, relationship_type, \
     cardinality, statement, machine_check, binding_strength, supersedes_rule_id";

/// Inserts a Rule. When `supersedes_rule_id` is set, every `RuleRelation`
/// touching the superseded rule is flipped from `active` to `stale`
/// automatically -- the confirmed judgment was about text that no longer
/// exists, so it needs re-review rather than silently continuing to read
/// as current. The stale row itself is kept, not deleted or rewritten.
pub fn insert_rule(conn: &Connection, rule: &Rule) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO rules ({RULE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        ),
        params![
            rule.id,
            rule.source_id,
            rule.subject_id,
            rule.related_subject_id,
            rule.relationship_type,
            rule.cardinality,
            rule.statement,
            rule.machine_check,
            rule.binding_strength.as_str(),
            rule.supersedes_rule_id,
        ],
    )?;

    if let Some(superseded_id) = &rule.supersedes_rule_id {
        conn.execute(
            "UPDATE rule_relations SET status = 'stale'
             WHERE (rule_a_id = ?1 OR rule_b_id = ?1) AND status = 'active'",
            params![superseded_id],
        )?;
    }
    index_for_search(conn, &rule.id, "rule", &rule.statement)?;
    Ok(())
}

pub fn rule_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<Rule>> {
    let cols = qualified_rule_columns();
    conn.query_row(
        &format!("SELECT {cols} FROM rules WHERE rules.id = ?1"),
        params![id],
        rule_from_row,
    )
    .optional()
}

/// Every rule about `subject_id` -- either as its primary subject or as
/// the target of a relationship claim (`related_subject_id`) -- alongside
/// the `Source` that issued it. Deliberately not scoped to any one
/// authority chain: a Subject can be the target of claims from Sources
/// anywhere in the DAG, and a caller needs the full picture (what the
/// standard says, what each layer under it adds) to make sense of it.
pub fn rules_for_subject(
    conn: &Connection,
    subject_id: &str,
) -> rusqlite::Result<Vec<(Rule, Source)>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {cols} FROM rules WHERE subject_id = ?1 OR related_subject_id = ?1 ORDER BY id",
        cols = qualified_rule_columns()
    ))?;
    let rules: Vec<Rule> = stmt
        .query_map(params![subject_id], rule_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    with_sources(conn, rules)
}

fn qualified_rule_columns() -> String {
    RULE_COLUMNS
        .split(", ")
        .map(|c| format!("rules.{c}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Joins each `Rule` with the `Source` that issued it. A Rule's
/// `source_id` is a required FK -- an absent Source is a data-integrity
/// bug, not a normal empty result, so this errors rather than silently
/// dropping the row.
fn with_sources(conn: &Connection, rules: Vec<Rule>) -> rusqlite::Result<Vec<(Rule, Source)>> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        let source =
            source_by_id(conn, &rule.source_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        out.push((rule, source));
    }
    Ok(out)
}

pub fn insert_rule_relation(conn: &Connection, relation: &RuleRelation) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO rule_relations (rule_a_id, rule_b_id, relation_type, status, confirmed_by)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            relation.rule_a_id,
            relation.rule_b_id,
            relation.relation_type.as_str(),
            relation.status.as_str(),
            relation.confirmed_by,
        ],
    )?;
    Ok(())
}

fn active_relation_exists(conn: &Connection, rule_a: &str, rule_b: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM rule_relations
         WHERE status = 'active'
           AND ((rule_a_id = ?1 AND rule_b_id = ?2) OR (rule_a_id = ?2 AND rule_b_id = ?1))",
        params![rule_a, rule_b],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
}

/// Confirmed, active `conflicts_with` relations among rules about
/// `subject_id`.
pub fn confirmed_conflicts_for_subject(
    conn: &Connection,
    subject_id: &str,
) -> rusqlite::Result<Vec<(Rule, Rule, RuleRelation)>> {
    let rules = rules_for_subject(conn, subject_id)?;
    let mut out = Vec::new();
    for i in 0..rules.len() {
        for j in (i + 1)..rules.len() {
            let (rule_a, _) = &rules[i];
            let (rule_b, _) = &rules[j];
            let relation = conn
                .query_row(
                    "SELECT rule_a_id, rule_b_id, relation_type, status, confirmed_by
                     FROM rule_relations
                     WHERE status = 'active' AND relation_type = 'conflicts_with'
                       AND ((rule_a_id = ?1 AND rule_b_id = ?2) OR (rule_a_id = ?2 AND rule_b_id = ?1))",
                    params![rule_a.id, rule_b.id],
                    |row| {
                        Ok(RuleRelation {
                            rule_a_id: row.get(0)?,
                            rule_b_id: row.get(1)?,
                            relation_type: RelationType::from_str(&row.get::<_, String>(2)?),
                            status: RelationStatus::from_str(&row.get::<_, String>(3)?),
                            confirmed_by: row.get(4)?,
                        })
                    },
                )
                .optional()?;
            if let Some(relation) = relation {
                out.push((rule_a.clone(), rule_b.clone(), relation));
            }
        }
    }
    Ok(out)
}

/// Pairs of same-subject rules from different Sources with no confirmed
/// `RuleRelation` yet -- candidates needing human review, per the
/// two-tier conflict-gate design (subject_id is the primary, exact
/// correlation key; this covers both ancestor/descendant pairs *and*
/// siblings under a shared parent, which a pure ancestor-chain walk would
/// miss entirely).
///
/// One exclusion: a pair where one rule is `Delegated` and the other's
/// Source is a descendant of the delegating rule's Source is *not*
/// surfaced -- that's the parent explicitly handing the decision down,
/// working as intended, not an ambiguity needing review. A pair of
/// *siblings* both fulfilling the same delegation differently (neither
/// ancestor of the other) is not covered by this exclusion and still
/// surfaces -- that's a real unresolved conflict between cousins.
pub fn conflict_candidates_for_subject(
    conn: &Connection,
    subject_id: &str,
) -> rusqlite::Result<Vec<(Rule, Rule)>> {
    let rules = rules_for_subject(conn, subject_id)?;
    let mut out = Vec::new();
    for i in 0..rules.len() {
        for j in (i + 1)..rules.len() {
            let (rule_a, source_a) = &rules[i];
            let (rule_b, source_b) = &rules[j];
            if rule_a.source_id == rule_b.source_id {
                continue;
            }
            if active_relation_exists(conn, &rule_a.id, &rule_b.id)? {
                continue;
            }
            let delegation_fulfillment = (rule_a.binding_strength == BindingStrength::Delegated
                && is_ancestor(conn, &source_a.id, &source_b.id)?)
                || (rule_b.binding_strength == BindingStrength::Delegated
                    && is_ancestor(conn, &source_b.id, &source_a.id)?);
            if delegation_fulfillment {
                continue;
            }
            out.push((rule_a.clone(), rule_b.clone()));
        }
    }
    Ok(out)
}

/// A domain tag with its subject count and the Sources that root it
/// (`domain_tags` is only ever populated on root Sources -- see `Source`'s
/// own doc comment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainInfo {
    pub domain_tag: String,
    pub subject_count: i64,
    pub root_sources: Vec<Source>,
}

/// Every distinct `domain_tag` in use, derived from `subjects` (there's no
/// `Domain` table in this model -- "Domain is a tag, not a table").
pub fn list_domains(conn: &Connection) -> rusqlite::Result<Vec<DomainInfo>> {
    let mut stmt = conn.prepare(
        "SELECT domain_tag, COUNT(*) FROM subjects GROUP BY domain_tag ORDER BY domain_tag",
    )?;
    let counts: Vec<(String, i64)> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let sources = all_sources(conn)?;
    Ok(counts
        .into_iter()
        .map(|(domain_tag, subject_count)| {
            let root_sources = sources
                .iter()
                .filter(|s| s.domain_tags.iter().any(|t| t == &domain_tag))
                .cloned()
                .collect();
            DomainInfo {
                domain_tag,
                subject_count,
                root_sources,
            }
        })
        .collect())
}

/// Every `Subject` in `domain_tag`, optionally narrowed to one
/// `subject_type`, ordered by short name.
pub fn subjects_in_domain(
    conn: &Connection,
    domain_tag: &str,
    subject_type: Option<&str>,
) -> rusqlite::Result<Vec<Subject>> {
    match subject_type {
        Some(subject_type) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SUBJECT_COLUMNS} FROM subjects \
                 WHERE domain_tag = ?1 AND subject_type = ?2 ORDER BY short_name"
            ))?;
            stmt.query_map(params![domain_tag, subject_type], subject_from_row)?
                .collect()
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SUBJECT_COLUMNS} FROM subjects WHERE domain_tag = ?1 ORDER BY short_name"
            ))?;
            stmt.query_map(params![domain_tag], subject_from_row)?
                .collect()
        }
    }
}

/// Overview of a domain: how many Subjects it has (broken down by
/// `subject_type`) and which Sources root it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainSummary {
    pub domain_tag: String,
    pub subject_count: i64,
    pub subject_count_by_type: Vec<(String, i64)>,
    pub root_sources: Vec<Source>,
}

/// Always returns a summary, even for a `domain_tag` with zero subjects
/// and zero root Sources -- `domain_tag` isn't a first-class, validated
/// entity in this model (any string is a valid key), so "not found" is a
/// call for the tool layer to make from an empty result, not this
/// function.
pub fn domain_summary(conn: &Connection, domain_tag: &str) -> rusqlite::Result<DomainSummary> {
    let subject_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM subjects WHERE domain_tag = ?1",
        params![domain_tag],
        |row| row.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT subject_type, COUNT(*) FROM subjects WHERE domain_tag = ?1 \
         GROUP BY subject_type ORDER BY subject_type",
    )?;
    let subject_count_by_type = stmt
        .query_map(params![domain_tag], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let root_sources = all_sources(conn)?
        .into_iter()
        .filter(|s| s.domain_tags.iter().any(|t| t == domain_tag))
        .collect();
    Ok(DomainSummary {
        domain_tag: domain_tag.to_string(),
        subject_count,
        subject_count_by_type,
        root_sources,
    })
}

/// Plain statement rules about `subject_id` -- excludes relationship
/// claims (`related_subject_id` set; see `outgoing_relationships`) and
/// rules where this subject is only the *target* of someone else's claim
/// (see `rules_for_subject` for the union of both). Optionally filtered
/// to one `binding_strength`.
pub fn statement_rules_for_subject(
    conn: &Connection,
    subject_id: &str,
    binding_strength: Option<BindingStrength>,
) -> rusqlite::Result<Vec<(Rule, Source)>> {
    let cols = qualified_rule_columns();
    let rules: Vec<Rule> = match binding_strength {
        Some(bs) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {cols} FROM rules \
                 WHERE subject_id = ?1 AND related_subject_id IS NULL AND binding_strength = ?2 \
                 ORDER BY id"
            ))?;
            stmt.query_map(params![subject_id, bs.as_str()], rule_from_row)?
                .collect::<rusqlite::Result<_>>()?
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {cols} FROM rules \
                 WHERE subject_id = ?1 AND related_subject_id IS NULL ORDER BY id"
            ))?;
            stmt.query_map(params![subject_id], rule_from_row)?
                .collect::<rusqlite::Result<_>>()?
        }
    };
    with_sources(conn, rules)
}

/// Outgoing relationship claims from `subject_id` (`related_subject_id`
/// set), optionally filtered to one `relationship_type`.
pub fn outgoing_relationships(
    conn: &Connection,
    subject_id: &str,
    relationship_type: Option<&str>,
) -> rusqlite::Result<Vec<(Rule, Source)>> {
    let cols = qualified_rule_columns();
    let rules: Vec<Rule> = match relationship_type {
        Some(rel_type) => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {cols} FROM rules \
                 WHERE subject_id = ?1 AND related_subject_id IS NOT NULL \
                 AND relationship_type = ?2 ORDER BY id"
            ))?;
            stmt.query_map(params![subject_id, rel_type], rule_from_row)?
                .collect::<rusqlite::Result<_>>()?
        }
        None => {
            let mut stmt = conn.prepare(&format!(
                "SELECT {cols} FROM rules \
                 WHERE subject_id = ?1 AND related_subject_id IS NOT NULL ORDER BY id"
            ))?;
            stmt.query_map(params![subject_id], rule_from_row)?
                .collect::<rusqlite::Result<_>>()?
        }
    };
    with_sources(conn, rules)
}

/// Every relationship-shaped `Rule` declaring a `from_type -> to_type`
/// connection within `domain_tag` -- both ends of the relationship must be
/// in that domain (a relationship crossing domains is
/// `cross_domain_relationships`'s job, not this one's).
pub fn valid_relationship_types(
    conn: &Connection,
    domain_tag: &str,
    from_type: &str,
    to_type: &str,
) -> rusqlite::Result<Vec<(Rule, Source)>> {
    let cols = qualified_rule_columns();
    let mut stmt = conn.prepare(&format!(
        "SELECT {cols} FROM rules \
         JOIN subjects sa ON sa.id = rules.subject_id \
         JOIN subjects sb ON sb.id = rules.related_subject_id \
         WHERE rules.related_subject_id IS NOT NULL \
           AND sa.domain_tag = ?1 AND sa.subject_type = ?2 \
           AND sb.domain_tag = ?1 AND sb.subject_type = ?3 \
         ORDER BY rules.id"
    ))?;
    let rules: Vec<Rule> = stmt
        .query_map(params![domain_tag, from_type, to_type], rule_from_row)?
        .collect::<rusqlite::Result<_>>()?;
    with_sources(conn, rules)
}

/// Traceability for `subject_id`: relationship-shaped Rules tagged
/// `relationship_type = "traces_to"`, split into outgoing (this subject
/// traces to something) and incoming (something traces to this subject).
/// `MUST` traces only, unless `include_optional` also pulls in `SHOULD`.
type TraceabilityResult = (Vec<(Rule, Source)>, Vec<(Rule, Source)>);

pub fn traceability(
    conn: &Connection,
    subject_id: &str,
    include_optional: bool,
) -> rusqlite::Result<TraceabilityResult> {
    let cols = qualified_rule_columns();
    let strength_clause = if include_optional {
        "AND rules.binding_strength IN ('MUST', 'SHOULD')"
    } else {
        "AND rules.binding_strength = 'MUST'"
    };

    let mut outgoing_stmt = conn.prepare(&format!(
        "SELECT {cols} FROM rules \
         WHERE rules.subject_id = ?1 AND rules.relationship_type = 'traces_to' \
         {strength_clause} ORDER BY rules.id"
    ))?;
    let outgoing: Vec<Rule> = outgoing_stmt
        .query_map(params![subject_id], rule_from_row)?
        .collect::<rusqlite::Result<_>>()?;

    let mut incoming_stmt = conn.prepare(&format!(
        "SELECT {cols} FROM rules \
         WHERE rules.related_subject_id = ?1 AND rules.relationship_type = 'traces_to' \
         {strength_clause} ORDER BY rules.id"
    ))?;
    let incoming: Vec<Rule> = incoming_stmt
        .query_map(params![subject_id], rule_from_row)?
        .collect::<rusqlite::Result<_>>()?;

    Ok((with_sources(conn, outgoing)?, with_sources(conn, incoming)?))
}

/// Outgoing relationship-shaped Rules from `subject_id` whose target
/// Subject sits in a *different* `domain_tag`, optionally narrowed to one
/// `to_domain_tag`.
pub fn cross_domain_relationships(
    conn: &Connection,
    subject_id: &str,
    to_domain_tag: Option<&str>,
) -> rusqlite::Result<Vec<(Rule, Subject, Source)>> {
    let subject = subject_by_id(conn, subject_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let candidates = outgoing_relationships(conn, subject_id, None)?;

    let mut out = Vec::new();
    for (rule, source) in candidates {
        let related_id = rule
            .related_subject_id
            .as_ref()
            .expect("outgoing_relationships only returns rows with related_subject_id set");
        let related =
            subject_by_id(conn, related_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        if related.domain_tag == subject.domain_tag {
            continue;
        }
        if let Some(to_domain_tag) = to_domain_tag
            && related.domain_tag != to_domain_tag
        {
            continue;
        }
        out.push((rule, related, source));
    }
    Ok(out)
}

/// A suggested (never auto-committed) declared valid-relationship rule,
/// derived from existing relationship-shaped `Rule` instances grouped by
/// `(from_type, to_type, relationship_type)`. Cardinality disagreements
/// across instances are surfaced via `other_cardinalities_seen`, not
/// silently hidden behind the majority choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidRelationshipCandidate {
    pub from_type: String,
    pub to_type: String,
    pub relationship_type: String,
    pub instance_count: i64,
    pub majority_cardinality: Option<String>,
    pub other_cardinalities_seen: Vec<String>,
}

/// Derives `ValidRelationshipCandidate`s from every relationship-shaped
/// Rule within `domain_tag` (both ends in that domain). A human reviews
/// these and decides whether to declare them as real valid-relationship
/// rules -- this function only ever suggests, never writes.
pub fn candidate_valid_relationships(
    conn: &Connection,
    domain_tag: &str,
) -> rusqlite::Result<Vec<ValidRelationshipCandidate>> {
    let cols = qualified_rule_columns();
    let mut stmt = conn.prepare(&format!(
        "SELECT {cols} FROM rules \
         JOIN subjects sa ON sa.id = rules.subject_id \
         JOIN subjects sb ON sb.id = rules.related_subject_id \
         WHERE rules.related_subject_id IS NOT NULL \
           AND sa.domain_tag = ?1 AND sb.domain_tag = ?1 \
         ORDER BY rules.id"
    ))?;
    let rules: Vec<Rule> = stmt
        .query_map(params![domain_tag], rule_from_row)?
        .collect::<rusqlite::Result<_>>()?;

    let mut groups: std::collections::HashMap<(String, String, String), Vec<Option<String>>> =
        std::collections::HashMap::new();
    for rule in &rules {
        let from_subject =
            subject_by_id(conn, &rule.subject_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let to_subject = subject_by_id(conn, rule.related_subject_id.as_ref().unwrap())?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let relationship_type = rule.relationship_type.clone().unwrap_or_default();
        let key = (
            from_subject.subject_type,
            to_subject.subject_type,
            relationship_type,
        );
        groups
            .entry(key)
            .or_default()
            .push(rule.cardinality.clone());
    }

    let mut out: Vec<ValidRelationshipCandidate> = groups
        .into_iter()
        .map(|((from_type, to_type, relationship_type), cardinalities)| {
            let instance_count = cardinalities.len() as i64;
            let mut counts: std::collections::HashMap<String, i64> =
                std::collections::HashMap::new();
            for cardinality in cardinalities.into_iter().flatten() {
                *counts.entry(cardinality).or_insert(0) += 1;
            }
            // Deterministic tie-break: highest count first, then
            // alphabetical -- HashMap iteration order isn't stable, and a
            // majority pick that changes between runs would be worse than
            // useless for a human reviewing it.
            let mut counts: Vec<(String, i64)> = counts.into_iter().collect();
            counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let majority_cardinality = counts.first().map(|(cardinality, _)| cardinality.clone());
            let other_cardinalities_seen = counts
                .into_iter()
                .skip(1)
                .map(|(cardinality, _)| cardinality)
                .collect();
            ValidRelationshipCandidate {
                from_type,
                to_type,
                relationship_type,
                instance_count,
                majority_cardinality,
                other_cardinalities_seen,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (&a.from_type, &a.to_type, &a.relationship_type).cmp(&(
            &b.from_type,
            &b.to_type,
            &b.relationship_type,
        ))
    });
    Ok(out)
}

/// Rules declaring `from_subject_id --relationship_type--> to_subject_id`
/// exactly -- empty means no such declaration exists (INVALID at the tool
/// layer), not an error.
pub fn validate_relationship(
    conn: &Connection,
    from_subject_id: &str,
    to_subject_id: &str,
    relationship_type: &str,
) -> rusqlite::Result<Vec<(Rule, Source)>> {
    let cols = qualified_rule_columns();
    let mut stmt = conn.prepare(&format!(
        "SELECT {cols} FROM rules \
         WHERE subject_id = ?1 AND related_subject_id = ?2 AND relationship_type = ?3 \
         ORDER BY id"
    ))?;
    let rules: Vec<Rule> = stmt
        .query_map(
            params![from_subject_id, to_subject_id, relationship_type],
            rule_from_row,
        )?
        .collect::<rusqlite::Result<_>>()?;
    with_sources(conn, rules)
}

const SELECTION_GROUP_COLUMNS: &str = "id, subject_id, description, constraint_type, threshold";

fn selection_group_from_row(row: &rusqlite::Row) -> rusqlite::Result<SelectionGroup> {
    let constraint_type: String = row.get(3)?;
    let threshold: Option<i64> = row.get(4)?;
    Ok(SelectionGroup {
        id: row.get(0)?,
        subject_id: row.get(1)?,
        description: row.get(2)?,
        constraint: SelectionConstraint::from_row(&constraint_type, threshold),
        member_rule_ids: Vec::new(),
    })
}

/// Inserts a `SelectionGroup` and its member-rule links in one call --
/// there's no standalone "add a member later" path since every group this
/// model needs so far has its membership fixed at authoring time.
pub fn insert_selection_group(conn: &Connection, group: &SelectionGroup) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO selection_groups ({SELECTION_GROUP_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5)"
        ),
        params![
            group.id,
            group.subject_id,
            group.description,
            group.constraint.as_str(),
            group.constraint.threshold(),
        ],
    )?;
    for rule_id in &group.member_rule_ids {
        conn.execute(
            "INSERT INTO selection_group_members (group_id, rule_id) VALUES (?1, ?2)",
            params![group.id, rule_id],
        )?;
    }
    Ok(())
}

/// Every `SelectionGroup` defined on `subject_id`, with member rule ids
/// populated (ordered by rule id).
pub fn selection_groups_for_subject(
    conn: &Connection,
    subject_id: &str,
) -> rusqlite::Result<Vec<SelectionGroup>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SELECTION_GROUP_COLUMNS} FROM selection_groups WHERE subject_id = ?1 ORDER BY id"
    ))?;
    let mut groups: Vec<SelectionGroup> = stmt
        .query_map(params![subject_id], selection_group_from_row)?
        .collect::<rusqlite::Result<_>>()?;

    let mut members_stmt = conn.prepare(
        "SELECT rule_id FROM selection_group_members WHERE group_id = ?1 ORDER BY rule_id",
    )?;
    for group in &mut groups {
        group.member_rule_ids = members_stmt
            .query_map(params![group.id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok(groups)
}

/// One `SelectionGroup`'s outcome against a supplied set of "present"
/// element references -- both the raw per-member satisfaction (so a
/// caller can report *which* members are missing, not just pass/fail) and
/// the group's overall verdict per its `SelectionConstraint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletenessFinding {
    pub group: SelectionGroup,
    /// Each member rule, the Source that issued it, and whether its
    /// related subject was found in the presence set.
    pub members: Vec<(Rule, Source, bool)>,
    pub satisfied_count: usize,
    pub is_satisfied: bool,
}

/// Evaluates every `SelectionGroup` defined on `subject_id` against
/// `present`, a caller-supplied set of what's actually present in the
/// model being checked. Each member rule's `related_subject_id` is
/// matched against `present` by id first, then by short name -- whichever
/// the caller happens to know. A member rule with no `related_subject_id`
/// can't be checked this way and always counts as satisfied, since
/// there's nothing external for the caller to have supplied.
pub fn evaluate_completeness(
    conn: &Connection,
    subject_id: &str,
    present: &HashSet<String>,
) -> rusqlite::Result<Vec<CompletenessFinding>> {
    let groups = selection_groups_for_subject(conn, subject_id)?;
    let mut findings = Vec::with_capacity(groups.len());
    for group in groups {
        let mut members = Vec::with_capacity(group.member_rule_ids.len());
        for rule_id in &group.member_rule_ids {
            let rule = rule_by_id(conn, rule_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            let source =
                source_by_id(conn, &rule.source_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            let satisfied = match &rule.related_subject_id {
                None => true,
                Some(related_id) if present.contains(related_id) => true,
                Some(related_id) => {
                    let related = subject_by_id(conn, related_id)?
                        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                    present.contains(&related.short_name)
                }
            };
            members.push((rule, source, satisfied));
        }
        let satisfied_count = members.iter().filter(|(_, _, ok)| *ok).count();
        let is_satisfied = match group.constraint {
            SelectionConstraint::All => satisfied_count == members.len(),
            SelectionConstraint::AtLeast(n) => satisfied_count >= n as usize,
        };
        findings.push(CompletenessFinding {
            group,
            members,
            satisfied_count,
            is_satisfied,
        });
    }
    Ok(findings)
}

/// What a `SearchResult` matched -- either a Rule's `statement`, or a
/// Subject's `name`/`short_name`/`description`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchRefType {
    Rule,
    Subject,
}

/// One `search_knowledge` hit. `score` is the raw SQLite FTS5 `bm25`
/// rank -- lower (more negative) means more relevant, not higher.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub ref_type: SearchRefType,
    pub ref_id: String,
    pub domain_tag: String,
    pub text: String,
    pub score: f64,
}

/// Escapes `query` into a safe FTS5 `MATCH` expression: each whitespace-
/// separated token becomes a quoted string literal (an embedded `"`
/// doubled, per FTS5 string-literal syntax), joined with `OR`. This means
/// hyphens, `AND`/`OR`/`NOT`, `^`, and every other FTS5 query-syntax
/// character in the caller's input is always treated as literal text to
/// match, never as an operator -- a query like "data-product" searches
/// for that literal token instead of silently becoming "data NOT
/// product". Returns `None` for an all-whitespace query.
fn fts5_safe_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" OR "))
    }
}

/// Lexical (FTS5) search over every `Rule.statement` and `Subject.name`/
/// `short_name`/`description`, kept in sync incrementally by
/// `insert_rule`/`insert_subject` -- never rebuilt per call. Deliberately
/// lexical-only: the previous model's `Embedder` trait and `sqlite-vec`
/// vector/hybrid-search infrastructure were removed entirely along with
/// the schema this replaces and are not reintroduced here (see
/// `ARCHITECTURE.md`'s non-goals) -- this is a scope decision, not a gap.
///
/// Internally over-fetches (bounded by `OVER_FETCH_CAP`) before applying
/// the optional `domain_tag` filter and truncating to `limit`, since a
/// hit's domain isn't stored redundantly in the FTS5 index. Fine at this
/// dataset's scale; revisit if that ever becomes a real bottleneck.
pub fn search_knowledge(
    conn: &Connection,
    query: &str,
    domain_tag: Option<&str>,
    limit: usize,
) -> rusqlite::Result<Vec<SearchResult>> {
    const OVER_FETCH_CAP: usize = 200;
    let Some(match_expr) = fts5_safe_query(query) else {
        return Ok(Vec::new());
    };
    let fetch_limit = if domain_tag.is_some() {
        OVER_FETCH_CAP
    } else {
        limit
    };

    let mut stmt = conn.prepare(
        "SELECT ref_id, ref_type, text, rank FROM search_index \
         WHERE search_index MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows: Vec<(String, String, String, f64)> = stmt
        .query_map(params![match_expr, fetch_limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut out = Vec::new();
    for (ref_id, ref_type_text, text, score) in rows {
        let ref_type = match ref_type_text.as_str() {
            "rule" => SearchRefType::Rule,
            "subject" => SearchRefType::Subject,
            other => {
                panic!("stored search_index ref_type {other:?} is not \"rule\" or \"subject\"")
            }
        };
        let resolved_domain_tag = match ref_type {
            SearchRefType::Rule => {
                let rule =
                    rule_by_id(conn, &ref_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                subject_by_id(conn, &rule.subject_id)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?
                    .domain_tag
            }
            SearchRefType::Subject => {
                subject_by_id(conn, &ref_id)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?
                    .domain_tag
            }
        };
        if let Some(domain_tag) = domain_tag
            && resolved_domain_tag != domain_tag
        {
            continue;
        }
        out.push(SearchResult {
            ref_type,
            ref_id,
            domain_tag: resolved_domain_tag,
            text,
            score,
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

const RULE_DERIVATION_COLUMNS: &str = "id, subject_id, label, summary";

fn rule_derivation_from_row(row: &rusqlite::Row) -> rusqlite::Result<RuleDerivation> {
    Ok(RuleDerivation {
        id: row.get(0)?,
        subject_id: row.get(1)?,
        label: row.get(2)?,
        summary: row.get(3)?,
        source_rule_ids: Vec::new(),
    })
}

/// Inserts a `RuleDerivation` and its source-rule links in one call. Not
/// inserted into `search_index` -- a derivation is a rollup of existing
/// Rule text, not new ground truth, and `search_knowledge` should surface
/// the authoritative Rules themselves, not a paraphrase of them.
pub fn insert_rule_derivation(
    conn: &Connection,
    derivation: &RuleDerivation,
) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO rule_derivations ({RULE_DERIVATION_COLUMNS}) VALUES (?1, ?2, ?3, ?4)"
        ),
        params![
            derivation.id,
            derivation.subject_id,
            derivation.label,
            derivation.summary,
        ],
    )?;
    for rule_id in &derivation.source_rule_ids {
        conn.execute(
            "INSERT INTO rule_derivation_sources (derivation_id, rule_id) VALUES (?1, ?2)",
            params![derivation.id, rule_id],
        )?;
    }
    Ok(())
}

/// Every `RuleDerivation` recorded for `subject_id`, with `source_rule_ids`
/// populated (ordered by rule id). Deliberately separate from
/// `rules_for_subject` and every other Rule-returning query -- a
/// derivation is never mixed into the same result set as actual Rules,
/// so a caller can't accidentally treat one as the other.
pub fn rule_derivations_for_subject(
    conn: &Connection,
    subject_id: &str,
) -> rusqlite::Result<Vec<RuleDerivation>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RULE_DERIVATION_COLUMNS} FROM rule_derivations WHERE subject_id = ?1 ORDER BY id"
    ))?;
    let mut derivations: Vec<RuleDerivation> = stmt
        .query_map(params![subject_id], rule_derivation_from_row)?
        .collect::<rusqlite::Result<_>>()?;

    let mut sources_stmt = conn.prepare(
        "SELECT rule_id FROM rule_derivation_sources WHERE derivation_id = ?1 ORDER BY rule_id",
    )?;
    for derivation in &mut derivations {
        derivation.source_rule_ids = sources_stmt
            .query_map(params![derivation.id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok(derivations)
}

/// Seeds a real UDRA authority chain: a data mesh standard (root) ->
/// Army UDRA -> the org's UDRA implementation -> two subordinate orgs
/// (siblings) implementing under it. Exercises `DELEGATED` (the schema
/// format decision), a genuine sibling conflict (two subordinate orgs
/// independently choosing incompatible schema formats -- neither is an
/// ancestor of the other, so the two-tier conflict gate is what catches
/// it), and one `machine_check`.
pub fn seed_udra(conn: &Connection) -> Result<(), String> {
    let to_string_err = |e: rusqlite::Error| e.to_string();

    insert_source(
        conn,
        &Source {
            id: "src.data-mesh-principles".into(),
            name: "Data Mesh Principles".into(),
            kind: "external-standard".into(),
            domain_tags: vec!["udra".into()],
            steward: Some("Zhamak Dehghani / community".into()),
            citation: Some("Data Mesh: Delivering Data-Driven Value at Scale".into()),
            supersedes_source_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_source(
        conn,
        &Source {
            id: "src.army-udra".into(),
            name: "Army Unified Data Reference Architecture".into(),
            kind: "army-construct".into(),
            domain_tags: vec![],
            steward: Some("Army CDO".into()),
            citation: None,
            supersedes_source_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_source(
        conn,
        &Source {
            id: "src.org-udra-impl".into(),
            name: "Our Org's UDRA Implementation".into(),
            kind: "org-implementation".into(),
            domain_tags: vec![],
            steward: Some("Org Data Governance Board".into()),
            citation: None,
            supersedes_source_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_source(
        conn,
        &Source {
            id: "src.suborg-a-impl".into(),
            name: "Subordinate Org A Implementation".into(),
            kind: "practitioner-implementation".into(),
            domain_tags: vec![],
            steward: Some("Org A Data Team".into()),
            citation: None,
            supersedes_source_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_source(
        conn,
        &Source {
            id: "src.suborg-b-impl".into(),
            name: "Subordinate Org B Implementation".into(),
            kind: "practitioner-implementation".into(),
            domain_tags: vec![],
            steward: Some("Org B Data Team".into()),
            citation: None,
            supersedes_source_id: None,
        },
    )
    .map_err(to_string_err)?;

    insert_source_authority_edge(conn, "src.army-udra", "src.data-mesh-principles")?;
    insert_source_authority_edge(conn, "src.org-udra-impl", "src.army-udra")?;
    insert_source_authority_edge(conn, "src.suborg-a-impl", "src.org-udra-impl")?;
    insert_source_authority_edge(conn, "src.suborg-b-impl", "src.org-udra-impl")?;

    insert_subject(
        conn,
        &Subject {
            id: "udra.DataProduct".into(),
            domain_tag: "udra".into(),
            subject_type: "concept".into(),
            name: "Data Product".into(),
            short_name: "DataProduct".into(),
            description: Some(
                "A domain-oriented, self-contained unit of data ownership with a clear owner, \
                 discoverable in the enterprise catalog."
                    .into(),
            ),
            is_deprecated: false,
            parent_subject_id: None,
            supersedes_subject_id: None,
            source_section: None,
        },
    )
    .map_err(to_string_err)?;
    insert_subject(
        conn,
        &Subject {
            id: "udra.DataContract".into(),
            domain_tag: "udra".into(),
            subject_type: "concept".into(),
            name: "Data Contract".into(),
            short_name: "DataContract".into(),
            description: Some(
                "The schema and quality agreement a data product exposes to its consumers.".into(),
            ),
            is_deprecated: false,
            parent_subject_id: None,
            supersedes_subject_id: None,
            source_section: None,
        },
    )
    .map_err(to_string_err)?;
    insert_subject(
        conn,
        &Subject {
            id: "data_mesh.DataProduct".into(),
            domain_tag: "data_mesh".into(),
            subject_type: "concept".into(),
            name: "Data Product".into(),
            short_name: "DataProduct".into(),
            description: Some(
                "Dehghani's original data-mesh data product concept, which UDRA specializes for \
                 the Army."
                    .into(),
            ),
            is_deprecated: false,
            parent_subject_id: None,
            supersedes_subject_id: None,
            source_section: None,
        },
    )
    .map_err(to_string_err)?;

    insert_rule(
        conn,
        &Rule {
            id: "rule.dm.001".into(),
            source_id: "src.data-mesh-principles".into(),
            subject_id: "udra.DataProduct".into(),
            related_subject_id: None,
            relationship_type: None,
            cardinality: None,
            statement: "A data product must have a clearly defined, accountable owner.".into(),
            machine_check: None,
            binding_strength: BindingStrength::Must,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_rule(
        conn,
        &Rule {
            id: "rule.dm.002".into(),
            source_id: "src.data-mesh-principles".into(),
            subject_id: "udra.DataProduct".into(),
            related_subject_id: Some("udra.DataContract".into()),
            relationship_type: Some("exposes".into()),
            cardinality: Some("1..*".into()),
            statement: "A data product exposes one or more data contracts.".into(),
            machine_check: None,
            binding_strength: BindingStrength::Must,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_rule(
        conn,
        &Rule {
            id: "rule.dm.003".into(),
            source_id: "src.data-mesh-principles".into(),
            subject_id: "udra.DataProduct".into(),
            related_subject_id: Some("data_mesh.DataProduct".into()),
            relationship_type: Some("realizes".into()),
            cardinality: Some("1".into()),
            statement: "A UDRA data product realizes the Data Mesh data product concept.".into(),
            machine_check: None,
            binding_strength: BindingStrength::Must,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_rule(
        conn,
        &Rule {
            id: "rule.org.003".into(),
            source_id: "src.org-udra-impl".into(),
            subject_id: "udra.DataContract".into(),
            related_subject_id: Some("udra.DataProduct".into()),
            relationship_type: Some("traces_to".into()),
            cardinality: Some("1".into()),
            statement: "A data contract must trace to the data product it belongs to.".into(),
            machine_check: None,
            binding_strength: BindingStrength::Must,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_rule(
        conn,
        &Rule {
            id: "rule.army-udra.001".into(),
            source_id: "src.army-udra".into(),
            subject_id: "udra.DataProduct".into(),
            related_subject_id: None,
            relationship_type: None,
            cardinality: None,
            statement: "Each Army UDRA data product must be registered in the enterprise \
                        data catalog."
                .into(),
            machine_check: None,
            binding_strength: BindingStrength::Must,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_rule(
        conn,
        &Rule {
            id: "rule.org.001".into(),
            source_id: "src.org-udra-impl".into(),
            subject_id: "udra.DataProduct".into(),
            related_subject_id: None,
            relationship_type: None,
            cardinality: None,
            statement: "A data product's owner must be identified by a valid organizational \
                        email address."
                .into(),
            machine_check: Some(
                r#"{"check":"pattern","property":"owner_email","pattern":"^[^@]+@[^@]+\\.[^@]+$"}"#
                    .into(),
            ),
            binding_strength: BindingStrength::Must,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_rule(
        conn,
        &Rule {
            id: "rule.org.002".into(),
            source_id: "src.org-udra-impl".into(),
            subject_id: "udra.DataContract".into(),
            related_subject_id: None,
            relationship_type: None,
            cardinality: None,
            statement: "The specific schema format for data contracts is delegated to \
                        implementing organizations."
                .into(),
            machine_check: None,
            binding_strength: BindingStrength::Delegated,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_rule(
        conn,
        &Rule {
            id: "rule.suborg-a.001".into(),
            source_id: "src.suborg-a-impl".into(),
            subject_id: "udra.DataContract".into(),
            related_subject_id: None,
            relationship_type: None,
            cardinality: None,
            statement: "Subordinate Org A data contracts must use JSON Schema.".into(),
            machine_check: None,
            binding_strength: BindingStrength::Must,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;
    insert_rule(
        conn,
        &Rule {
            id: "rule.suborg-b.001".into(),
            source_id: "src.suborg-b-impl".into(),
            subject_id: "udra.DataContract".into(),
            related_subject_id: None,
            relationship_type: None,
            cardinality: None,
            statement: "Subordinate Org B data contracts must use Avro schema.".into(),
            machine_check: None,
            binding_strength: BindingStrength::Must,
            supersedes_rule_id: None,
        },
    )
    .map_err(to_string_err)?;

    // A human reviewer has already confirmed that Org A's JSON Schema
    // choice fulfills the parent's DELEGATED rule -- realistic seed data,
    // and it's what makes `insert_rule_relation` a real production path
    // rather than something only exercised by tests.
    insert_rule_relation(
        conn,
        &RuleRelation {
            rule_a_id: "rule.org.002".into(),
            rule_b_id: "rule.suborg-a.001".into(),
            relation_type: RelationType::Implements,
            status: RelationStatus::Active,
            confirmed_by: "org-data-governance-board@example.org".into(),
        },
    )
    .map_err(to_string_err)?;

    // A complete Data Product must both expose a Data Contract and realize
    // the Data Mesh concept it specializes -- `All`, not just "one or the
    // other". Reuses the two existing relationship-shaped rules on
    // `udra.DataProduct` rather than inventing test-only fixtures.
    insert_selection_group(
        conn,
        &SelectionGroup {
            id: "selgrp.data-product-complete".into(),
            subject_id: "udra.DataProduct".into(),
            description: "A complete Data Product exposes a Data Contract and realizes the \
                           Data Mesh Data Product concept."
                .into(),
            constraint: SelectionConstraint::All,
            member_rule_ids: vec!["rule.dm.002".into(), "rule.dm.003".into()],
        },
    )
    .map_err(to_string_err)?;

    // A non-authoritative rollup of the three separate ownership/
    // registration rules spread across the authority chain (data mesh
    // principle -> Army UDRA -> org implementation) -- useful as a quick
    // orientation summary, but every claim in it traces back to a real
    // Rule via source_rule_ids, which is what keeps it from being cited
    // as ground truth on its own.
    insert_rule_derivation(
        conn,
        &RuleDerivation {
            id: "ruleder.data-product-ownership-summary".into(),
            subject_id: "udra.DataProduct".into(),
            label: "Effective ownership & registration guidance".into(),
            summary: "A UDRA data product must have a clearly defined, accountable owner \
                      (identified by a valid organizational email address), and must be \
                      registered in the enterprise data catalog. See the cited rules for the \
                      authoritative statements -- this is a synthesized summary, not itself a \
                      rule."
                .into(),
            source_rule_ids: vec![
                "rule.dm.001".into(),
                "rule.army-udra.001".into(),
                "rule.org.001".into(),
            ],
        },
    )
    .map_err(to_string_err)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> Connection {
        let conn = open_store().unwrap();
        seed_udra(&conn).unwrap();
        conn
    }

    #[test]
    fn ancestors_of_walks_full_chain() {
        let conn = seeded();
        let ancestors = ancestors_of(&conn, "src.suborg-a-impl").unwrap();
        assert!(ancestors.contains("src.org-udra-impl"));
        assert!(ancestors.contains("src.army-udra"));
        assert!(ancestors.contains("src.data-mesh-principles"));
        assert_eq!(ancestors.len(), 3);
    }

    #[test]
    fn sibling_sources_are_not_ancestors_of_each_other() {
        let conn = seeded();
        assert!(!is_ancestor(&conn, "src.suborg-a-impl", "src.suborg-b-impl").unwrap());
        assert!(!is_ancestor(&conn, "src.suborg-b-impl", "src.suborg-a-impl").unwrap());
    }

    #[test]
    fn self_loop_edge_is_rejected() {
        let conn = open_store().unwrap();
        insert_source(
            &conn,
            &Source {
                id: "s1".into(),
                name: "S1".into(),
                kind: "external-standard".into(),
                domain_tags: vec![],
                steward: None,
                citation: None,
                supersedes_source_id: None,
            },
        )
        .unwrap();
        assert!(insert_source_authority_edge(&conn, "s1", "s1").is_err());
    }

    #[test]
    fn cyclic_edge_is_rejected() {
        let conn = open_store().unwrap();
        for id in ["s1", "s2"] {
            insert_source(
                &conn,
                &Source {
                    id: id.into(),
                    name: id.into(),
                    kind: "external-standard".into(),
                    domain_tags: vec![],
                    steward: None,
                    citation: None,
                    supersedes_source_id: None,
                },
            )
            .unwrap();
        }
        // s2 answers to s1.
        insert_source_authority_edge(&conn, "s2", "s1").unwrap();
        // s1 answering to s2 would close a cycle -- must be rejected.
        assert!(insert_source_authority_edge(&conn, "s1", "s2").is_err());
    }

    #[test]
    fn multi_parent_source_has_two_independent_ancestors() {
        let conn = open_store().unwrap();
        for id in ["rmf", "overlay", "system"] {
            insert_source(
                &conn,
                &Source {
                    id: id.into(),
                    name: id.into(),
                    kind: "external-standard".into(),
                    domain_tags: vec![],
                    steward: None,
                    citation: None,
                    supersedes_source_id: None,
                },
            )
            .unwrap();
        }
        insert_source_authority_edge(&conn, "system", "rmf").unwrap();
        insert_source_authority_edge(&conn, "system", "overlay").unwrap();
        let ancestors = ancestors_of(&conn, "system").unwrap();
        assert!(ancestors.contains("rmf"));
        assert!(ancestors.contains("overlay"));
    }

    /// Builds a deeper multi-parent DAG than the minimal one-edge case
    /// above: two entirely independent, two-level-deep root lineages
    /// (`root-a` -> `mid-a`, `root-b` -> `mid-b`) that share no common
    /// ancestor at all, converging only at `leaf` (which answers to both
    /// `mid-a` and `mid-b`). Stresses the recursive ancestor walk across
    /// a genuine DAG shape, not just a single extra parent edge.
    fn multi_parent_dag_stress_fixture() -> Connection {
        let conn = open_store().unwrap();
        for id in ["root-a", "mid-a", "root-b", "mid-b", "leaf"] {
            insert_source(
                &conn,
                &Source {
                    id: id.into(),
                    name: id.into(),
                    kind: "external-standard".into(),
                    domain_tags: vec![],
                    steward: None,
                    citation: None,
                    supersedes_source_id: None,
                },
            )
            .unwrap();
        }
        insert_source_authority_edge(&conn, "mid-a", "root-a").unwrap();
        insert_source_authority_edge(&conn, "mid-b", "root-b").unwrap();
        insert_source_authority_edge(&conn, "leaf", "mid-a").unwrap();
        insert_source_authority_edge(&conn, "leaf", "mid-b").unwrap();
        conn
    }

    #[test]
    fn multi_parent_dag_stress_ancestors_span_two_independent_root_lineages() {
        let conn = multi_parent_dag_stress_fixture();
        let ancestors = ancestors_of(&conn, "leaf").unwrap();
        assert_eq!(ancestors.len(), 4);
        for expected in ["mid-a", "root-a", "mid-b", "root-b"] {
            assert!(
                ancestors.contains(expected),
                "expected {expected:?} in ancestors of \"leaf\", got {ancestors:?}"
            );
        }
    }

    #[test]
    fn multi_parent_dag_stress_independent_roots_are_not_ancestors_of_each_other() {
        let conn = multi_parent_dag_stress_fixture();
        assert!(!is_ancestor(&conn, "root-a", "root-b").unwrap());
        assert!(!is_ancestor(&conn, "root-b", "root-a").unwrap());
        assert!(!is_ancestor(&conn, "mid-a", "mid-b").unwrap());
        assert!(!is_ancestor(&conn, "mid-b", "mid-a").unwrap());
    }

    #[test]
    fn multi_parent_dag_stress_rules_from_both_independent_lineages_surface_together() {
        let conn = multi_parent_dag_stress_fixture();
        insert_subject(
            &conn,
            &Subject {
                id: "sys.Boundary".into(),
                domain_tag: "rmf".into(),
                subject_type: "concept".into(),
                name: "System Boundary".into(),
                short_name: "Boundary".into(),
                description: None,
                is_deprecated: false,
                parent_subject_id: None,
                supersedes_subject_id: None,
                source_section: None,
            },
        )
        .unwrap();
        insert_rule(
            &conn,
            &Rule {
                id: "rule.root-a.001".into(),
                source_id: "root-a".into(),
                subject_id: "sys.Boundary".into(),
                related_subject_id: None,
                relationship_type: None,
                cardinality: None,
                statement: "Root A requires an annual boundary review.".into(),
                machine_check: None,
                binding_strength: BindingStrength::Must,
                supersedes_rule_id: None,
            },
        )
        .unwrap();
        insert_rule(
            &conn,
            &Rule {
                id: "rule.root-b.001".into(),
                source_id: "root-b".into(),
                subject_id: "sys.Boundary".into(),
                related_subject_id: None,
                relationship_type: None,
                cardinality: None,
                statement: "Root B requires a quarterly boundary review.".into(),
                machine_check: None,
                binding_strength: BindingStrength::Must,
                supersedes_rule_id: None,
            },
        )
        .unwrap();

        // Both rules come from Sources that share no ancestor with each
        // other at all -- they converge only at "leaf", a shared
        // descendant. `rules_for_subject` must still surface both.
        let rules = rules_for_subject(&conn, "sys.Boundary").unwrap();
        assert_eq!(rules.len(), 2);
        let rule_ids: Vec<&str> = rules.iter().map(|(r, _)| r.id.as_str()).collect();
        assert!(rule_ids.contains(&"rule.root-a.001"));
        assert!(rule_ids.contains(&"rule.root-b.001"));

        // And the conflict gate must surface them as a candidate needing
        // review -- disagreeing frequency requirements from two entirely
        // independent standards is exactly the case the two-tier gate
        // exists for.
        let candidates = conflict_candidates_for_subject(&conn, "sys.Boundary").unwrap();
        assert_eq!(candidates.len(), 1);
        let (rule_a, rule_b) = &candidates[0];
        let candidate_ids = [rule_a.id.as_str(), rule_b.id.as_str()];
        assert!(candidate_ids.contains(&"rule.root-a.001"));
        assert!(candidate_ids.contains(&"rule.root-b.001"));
    }

    #[test]
    fn resolve_subject_finds_by_short_name_then_id() {
        let conn = seeded();
        assert_eq!(
            resolve_subject(&conn, "udra", "DataProduct")
                .unwrap()
                .unwrap()
                .id,
            "udra.DataProduct"
        );
        assert_eq!(
            resolve_subject(&conn, "udra", "udra.DataContract")
                .unwrap()
                .unwrap()
                .id,
            "udra.DataContract"
        );
        assert!(
            resolve_subject(&conn, "udra", "NoSuchThing")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rules_for_subject_spans_the_whole_authority_chain() {
        let conn = seeded();
        let rules = rules_for_subject(&conn, "udra.DataProduct").unwrap();
        let source_ids: HashSet<_> = rules.iter().map(|(_, s)| s.id.clone()).collect();
        assert!(source_ids.contains("src.data-mesh-principles"));
        assert!(source_ids.contains("src.army-udra"));
        assert!(source_ids.contains("src.org-udra-impl"));
    }

    #[test]
    fn delegated_parent_and_fulfilling_child_are_not_surfaced_as_candidates() {
        let conn = seeded();
        let candidates = conflict_candidates_for_subject(&conn, "udra.DataContract").unwrap();
        let has_delegation_pair = candidates.iter().any(|(a, b)| {
            (a.id == "rule.org.002" && b.id == "rule.suborg-a.001")
                || (b.id == "rule.org.002" && a.id == "rule.suborg-a.001")
        });
        assert!(
            !has_delegation_pair,
            "a DELEGATED rule and its fulfilling descendant should not need review"
        );
    }

    #[test]
    fn sibling_orgs_fulfilling_the_same_delegation_differently_is_a_real_candidate() {
        let conn = seeded();
        let candidates = conflict_candidates_for_subject(&conn, "udra.DataContract").unwrap();
        let has_sibling_pair = candidates.iter().any(|(a, b)| {
            (a.id == "rule.suborg-a.001" && b.id == "rule.suborg-b.001")
                || (b.id == "rule.suborg-a.001" && a.id == "rule.suborg-b.001")
        });
        assert!(
            has_sibling_pair,
            "two sibling orgs independently choosing incompatible schema formats is exactly \
             what the two-tier conflict gate exists to catch"
        );
    }

    #[test]
    fn confirmed_conflict_relation_surfaces_via_confirmed_conflicts_and_drops_from_candidates() {
        let conn = seeded();
        insert_rule_relation(
            &conn,
            &RuleRelation {
                rule_a_id: "rule.suborg-a.001".into(),
                rule_b_id: "rule.suborg-b.001".into(),
                relation_type: RelationType::ConflictsWith,
                status: RelationStatus::Active,
                confirmed_by: "reviewer@example.org".into(),
            },
        )
        .unwrap();

        let confirmed = confirmed_conflicts_for_subject(&conn, "udra.DataContract").unwrap();
        assert_eq!(confirmed.len(), 1);

        let candidates = conflict_candidates_for_subject(&conn, "udra.DataContract").unwrap();
        assert!(
            !candidates
                .iter()
                .any(|(a, b)| a.id == "rule.suborg-a.001" && b.id == "rule.suborg-b.001")
        );
    }

    #[test]
    fn superseding_a_rule_stales_its_active_relations() {
        let conn = seeded();
        insert_rule_relation(
            &conn,
            &RuleRelation {
                rule_a_id: "rule.suborg-a.001".into(),
                rule_b_id: "rule.suborg-b.001".into(),
                relation_type: RelationType::ConflictsWith,
                status: RelationStatus::Active,
                confirmed_by: "reviewer@example.org".into(),
            },
        )
        .unwrap();

        insert_rule(
            &conn,
            &Rule {
                id: "rule.suborg-a.002".into(),
                source_id: "src.suborg-a-impl".into(),
                subject_id: "udra.DataContract".into(),
                related_subject_id: None,
                relationship_type: None,
                cardinality: None,
                statement: "Subordinate Org A data contracts must use Protobuf.".into(),
                machine_check: None,
                binding_strength: BindingStrength::Must,
                supersedes_rule_id: Some("rule.suborg-a.001".into()),
            },
        )
        .unwrap();

        let confirmed = confirmed_conflicts_for_subject(&conn, "udra.DataContract").unwrap();
        assert!(
            confirmed.is_empty(),
            "the relation should have gone stale, not stayed active, once its rule was superseded"
        );
    }

    #[test]
    fn list_domains_reports_seeded_udra_domain() {
        let conn = seeded();
        let domains = list_domains(&conn).unwrap();
        let udra = domains
            .iter()
            .find(|d| d.domain_tag == "udra")
            .expect("udra domain should be present");
        assert_eq!(udra.subject_count, 2);
        assert!(
            udra.root_sources
                .iter()
                .any(|s| s.id == "src.data-mesh-principles")
        );
    }

    #[test]
    fn subjects_in_domain_filters_by_subject_type() {
        let conn = seeded();
        let all = subjects_in_domain(&conn, "udra", None).unwrap();
        assert_eq!(all.len(), 2);
        let concepts = subjects_in_domain(&conn, "udra", Some("concept")).unwrap();
        assert_eq!(concepts.len(), 2);
        let none = subjects_in_domain(&conn, "udra", Some("no-such-type")).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn domain_summary_reports_counts_and_root_sources() {
        let conn = seeded();
        let summary = domain_summary(&conn, "udra").unwrap();
        assert_eq!(summary.subject_count, 2);
        assert!(
            summary
                .subject_count_by_type
                .contains(&("concept".to_string(), 2))
        );
        assert!(
            summary
                .root_sources
                .iter()
                .any(|s| s.id == "src.data-mesh-principles")
        );
    }

    #[test]
    fn domain_summary_unknown_domain_is_empty_not_an_error() {
        let conn = seeded();
        let summary = domain_summary(&conn, "no-such-domain").unwrap();
        assert_eq!(summary.subject_count, 0);
        assert!(summary.root_sources.is_empty());
    }

    #[test]
    fn statement_rules_for_subject_excludes_relationship_claims() {
        let conn = seeded();
        let rules = statement_rules_for_subject(&conn, "udra.DataProduct", None).unwrap();
        assert!(rules.iter().all(|(r, _)| r.related_subject_id.is_none()));
        assert!(!rules.iter().any(|(r, _)| r.id == "rule.dm.002"));
    }

    #[test]
    fn statement_rules_for_subject_filters_by_binding_strength() {
        let conn = seeded();
        let must_rules =
            statement_rules_for_subject(&conn, "udra.DataProduct", Some(BindingStrength::Must))
                .unwrap();
        assert!(
            must_rules
                .iter()
                .all(|(r, _)| r.binding_strength == BindingStrength::Must)
        );
        let should_rules =
            statement_rules_for_subject(&conn, "udra.DataProduct", Some(BindingStrength::Should))
                .unwrap();
        assert!(
            should_rules
                .iter()
                .all(|(r, _)| r.binding_strength == BindingStrength::Should)
        );
        assert_ne!(must_rules.len(), should_rules.len());
    }

    #[test]
    fn outgoing_relationships_returns_only_relationship_shaped_rules() {
        let conn = seeded();
        let relationships = outgoing_relationships(&conn, "udra.DataProduct", None).unwrap();
        assert_eq!(relationships.len(), 2);
        assert!(relationships.iter().any(|(r, _)| r.id == "rule.dm.002"
            && r.related_subject_id.as_deref() == Some("udra.DataContract")));
        assert!(relationships.iter().any(|(r, _)| r.id == "rule.dm.003"
            && r.related_subject_id.as_deref() == Some("data_mesh.DataProduct")));
    }

    #[test]
    fn outgoing_relationships_filters_by_relationship_type() {
        let conn = seeded();
        let matching = outgoing_relationships(&conn, "udra.DataProduct", Some("exposes")).unwrap();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].0.id, "rule.dm.002");
        let non_matching =
            outgoing_relationships(&conn, "udra.DataProduct", Some("no-such-type")).unwrap();
        assert!(non_matching.is_empty());
    }

    #[test]
    fn valid_relationship_types_scoped_to_domain_and_subject_types() {
        let conn = seeded();
        let found = valid_relationship_types(&conn, "udra", "concept", "concept").unwrap();
        assert!(found.iter().any(|(r, _)| r.id == "rule.dm.002"));
        assert!(found.iter().any(|(r, _)| r.id == "rule.org.003"));
        // rule.dm.003 crosses into data_mesh -- not within-domain, shouldn't match.
        assert!(!found.iter().any(|(r, _)| r.id == "rule.dm.003"));
    }

    #[test]
    fn traceability_finds_outgoing_and_incoming_traces_to() {
        let conn = seeded();
        let (outgoing, incoming) = traceability(&conn, "udra.DataContract", false).unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].0.id, "rule.org.003");
        assert!(incoming.is_empty());

        let (outgoing2, incoming2) = traceability(&conn, "udra.DataProduct", false).unwrap();
        assert!(outgoing2.is_empty());
        assert_eq!(incoming2.len(), 1);
        assert_eq!(incoming2[0].0.id, "rule.org.003");
    }

    #[test]
    fn cross_domain_relationships_finds_only_different_domain_targets() {
        let conn = seeded();
        let found = cross_domain_relationships(&conn, "udra.DataProduct", None).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.id, "rule.dm.003");
        assert_eq!(found[0].1.domain_tag, "data_mesh");
    }

    #[test]
    fn cross_domain_relationships_filters_by_to_domain() {
        let conn = seeded();
        let matching =
            cross_domain_relationships(&conn, "udra.DataProduct", Some("data_mesh")).unwrap();
        assert_eq!(matching.len(), 1);
        let non_matching =
            cross_domain_relationships(&conn, "udra.DataProduct", Some("no-such-domain")).unwrap();
        assert!(non_matching.is_empty());
    }

    #[test]
    fn candidate_valid_relationships_groups_by_type_triple() {
        let conn = seeded();
        let candidates = candidate_valid_relationships(&conn, "udra").unwrap();
        let exposes = candidates
            .iter()
            .find(|c| c.relationship_type == "exposes")
            .expect("exposes candidate should be present");
        assert_eq!(exposes.from_type, "concept");
        assert_eq!(exposes.to_type, "concept");
        assert_eq!(exposes.instance_count, 1);
        assert_eq!(exposes.majority_cardinality.as_deref(), Some("1..*"));
        assert!(exposes.other_cardinalities_seen.is_empty());
    }

    #[test]
    fn validate_relationship_finds_matching_rule() {
        let conn = seeded();
        let matches =
            validate_relationship(&conn, "udra.DataProduct", "udra.DataContract", "exposes")
                .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0.id, "rule.dm.002");
    }

    #[test]
    fn validate_relationship_no_match_is_empty_not_an_error() {
        let conn = seeded();
        let matches = validate_relationship(
            &conn,
            "udra.DataProduct",
            "udra.DataContract",
            "no-such-type",
        )
        .unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn evaluate_machine_check_required_property() {
        let mut props = HashMap::new();
        assert_eq!(
            evaluate_machine_check(
                r#"{"check":"required_property","property":"owner"}"#,
                &props
            ),
            CheckResult::Fail("required property \"owner\" is missing".to_string())
        );
        props.insert("owner".to_string(), "alice".to_string());
        assert_eq!(
            evaluate_machine_check(
                r#"{"check":"required_property","property":"owner"}"#,
                &props
            ),
            CheckResult::Pass
        );
    }

    #[test]
    fn evaluate_machine_check_enum_value() {
        let mut props = HashMap::new();
        props.insert("status".to_string(), "active".to_string());
        assert_eq!(
            evaluate_machine_check(
                r#"{"check":"enum_value","property":"status","values":["active","deprecated"]}"#,
                &props
            ),
            CheckResult::Pass
        );
        props.insert("status".to_string(), "unknown".to_string());
        assert!(matches!(
            evaluate_machine_check(
                r#"{"check":"enum_value","property":"status","values":["active","deprecated"]}"#,
                &props
            ),
            CheckResult::Fail(_)
        ));
    }

    #[test]
    fn evaluate_machine_check_pattern_match_and_mismatch_is_warning_not_fail() {
        let mut props = HashMap::new();
        props.insert("email".to_string(), "a@b.com".to_string());
        assert_eq!(
            evaluate_machine_check(
                r#"{"check":"pattern","property":"email","pattern":"^[^@]+@[^@]+\\.[^@]+$"}"#,
                &props
            ),
            CheckResult::Pass
        );
        props.insert("email".to_string(), "not-an-email".to_string());
        assert!(matches!(
            evaluate_machine_check(
                r#"{"check":"pattern","property":"email","pattern":"^[^@]+@[^@]+\\.[^@]+$"}"#,
                &props
            ),
            CheckResult::Warning(_)
        ));
    }

    #[test]
    fn evaluate_machine_check_invalid_pattern_is_warning_not_a_panic() {
        let mut props = HashMap::new();
        props.insert("email".to_string(), "a@b.com".to_string());
        assert!(matches!(
            evaluate_machine_check(
                r#"{"check":"pattern","property":"email","pattern":"(unclosed"}"#,
                &props
            ),
            CheckResult::Warning(_)
        ));
    }

    #[test]
    fn evaluate_machine_check_range() {
        let mut props = HashMap::new();
        props.insert("priority".to_string(), "3".to_string());
        assert_eq!(
            evaluate_machine_check(
                r#"{"check":"range","property":"priority","min":1,"max":5}"#,
                &props
            ),
            CheckResult::Pass
        );
        props.insert("priority".to_string(), "10".to_string());
        assert!(matches!(
            evaluate_machine_check(
                r#"{"check":"range","property":"priority","min":1,"max":5}"#,
                &props
            ),
            CheckResult::Fail(_)
        ));
        props.insert("priority".to_string(), "not-a-number".to_string());
        assert!(matches!(
            evaluate_machine_check(
                r#"{"check":"range","property":"priority","min":1,"max":5}"#,
                &props
            ),
            CheckResult::Fail(_)
        ));
    }

    #[test]
    fn evaluate_machine_check_custom_is_warning() {
        let props = HashMap::new();
        assert!(matches!(
            evaluate_machine_check(r#"{"check":"custom"}"#, &props),
            CheckResult::Warning(_)
        ));
    }

    #[test]
    fn evaluate_machine_check_invalid_json_is_warning_not_a_panic() {
        let props = HashMap::new();
        assert!(matches!(
            evaluate_machine_check("not json", &props),
            CheckResult::Warning(_)
        ));
    }

    #[test]
    fn selection_groups_for_subject_returns_seeded_group_with_members() {
        let conn = seeded();
        let groups = selection_groups_for_subject(&conn, "udra.DataProduct").unwrap();
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert_eq!(group.id, "selgrp.data-product-complete");
        assert_eq!(group.constraint, SelectionConstraint::All);
        assert_eq!(
            group.member_rule_ids,
            vec!["rule.dm.002".to_string(), "rule.dm.003".to_string()]
        );
    }

    #[test]
    fn selection_groups_for_subject_empty_when_none_defined() {
        let conn = seeded();
        let groups = selection_groups_for_subject(&conn, "udra.DataContract").unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn evaluate_completeness_all_satisfied_when_present_set_covers_every_member() {
        let conn = seeded();
        let present: HashSet<String> = ["udra.DataContract".to_string(), "DataProduct".to_string()]
            .into_iter()
            .collect();
        let findings = evaluate_completeness(&conn, "udra.DataProduct", &present).unwrap();
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_satisfied);
        assert_eq!(findings[0].satisfied_count, 2);
    }

    #[test]
    fn evaluate_completeness_matches_present_set_by_short_name_too() {
        let conn = seeded();
        // "DataContract" (short name) instead of "udra.DataContract" (id),
        // and the data_mesh DataProduct's short name for the other member.
        let present: HashSet<String> = ["DataContract".to_string(), "DataProduct".to_string()]
            .into_iter()
            .collect();
        let findings = evaluate_completeness(&conn, "udra.DataProduct", &present).unwrap();
        assert!(findings[0].is_satisfied);
    }

    #[test]
    fn evaluate_completeness_reports_missing_member_and_group_not_satisfied() {
        let conn = seeded();
        let present: HashSet<String> = ["udra.DataContract".to_string()].into_iter().collect();
        let findings = evaluate_completeness(&conn, "udra.DataProduct", &present).unwrap();
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].is_satisfied);
        assert_eq!(findings[0].satisfied_count, 1);
        let unsatisfied: Vec<_> = findings[0]
            .members
            .iter()
            .filter(|(_, _, ok)| !ok)
            .collect();
        assert_eq!(unsatisfied.len(), 1);
        assert_eq!(unsatisfied[0].0.id, "rule.dm.003");
    }

    #[test]
    fn evaluate_completeness_no_groups_defined_is_empty_not_an_error() {
        let conn = seeded();
        let findings = evaluate_completeness(&conn, "udra.DataContract", &HashSet::new()).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn evaluate_completeness_at_least_constraint_satisfied_by_a_subset() {
        let conn = seeded();
        insert_selection_group(
            &conn,
            &SelectionGroup {
                id: "selgrp.test-at-least".into(),
                subject_id: "udra.DataProduct".into(),
                description: "test-only: at least one of the two relationship rules".into(),
                constraint: SelectionConstraint::AtLeast(1),
                member_rule_ids: vec!["rule.dm.002".into(), "rule.dm.003".into()],
            },
        )
        .unwrap();
        let present: HashSet<String> = ["udra.DataContract".to_string()].into_iter().collect();
        let findings = evaluate_completeness(&conn, "udra.DataProduct", &present).unwrap();
        let at_least_finding = findings
            .iter()
            .find(|f| f.group.id == "selgrp.test-at-least")
            .unwrap();
        assert!(at_least_finding.is_satisfied);
        assert_eq!(at_least_finding.satisfied_count, 1);
    }

    #[test]
    fn fts5_safe_query_quotes_each_token_and_joins_with_or() {
        assert_eq!(
            fts5_safe_query("data product").unwrap(),
            "\"data\" OR \"product\""
        );
        assert_eq!(fts5_safe_query("data-product").unwrap(), "\"data-product\"");
        assert!(fts5_safe_query("   ").is_none());
    }

    #[test]
    fn fts5_safe_query_escapes_embedded_quotes() {
        let escaped = fts5_safe_query("say\"hi").unwrap();
        assert!(escaped.contains("\"\""));
    }

    #[test]
    fn search_knowledge_finds_seeded_rule_by_statement_keyword() {
        let conn = seeded();
        let results = search_knowledge(&conn, "accountable", None, 10).unwrap();
        assert!(
            results
                .iter()
                .any(|r| r.ref_id == "rule.dm.001" && r.ref_type == SearchRefType::Rule)
        );
    }

    #[test]
    fn search_knowledge_finds_seeded_subject_by_name() {
        let conn = seeded();
        let results = search_knowledge(&conn, "Contract", None, 10).unwrap();
        assert!(
            results
                .iter()
                .any(|r| r.ref_id == "udra.DataContract" && r.ref_type == SearchRefType::Subject)
        );
    }

    #[test]
    fn search_knowledge_filters_by_domain_tag() {
        let conn = seeded();
        let results = search_knowledge(&conn, "Product", Some("data_mesh"), 10).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().all(|r| r.domain_tag == "data_mesh"));
    }

    #[test]
    fn search_knowledge_respects_limit() {
        let conn = seeded();
        let results = search_knowledge(&conn, "data", None, 2).unwrap();
        assert!(results.len() <= 2);
    }

    #[test]
    fn search_knowledge_empty_query_is_empty_not_an_error() {
        let conn = seeded();
        let results = search_knowledge(&conn, "   ", None, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_knowledge_no_match_is_empty_not_an_error() {
        let conn = seeded();
        let results = search_knowledge(&conn, "zzzznosuchword", None, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn rule_derivations_for_subject_returns_seeded_derivation_with_sources() {
        let conn = seeded();
        let derivations = rule_derivations_for_subject(&conn, "udra.DataProduct").unwrap();
        assert_eq!(derivations.len(), 1);
        let derivation = &derivations[0];
        assert_eq!(derivation.id, "ruleder.data-product-ownership-summary");
        assert_eq!(
            derivation.source_rule_ids,
            vec![
                "rule.army-udra.001".to_string(),
                "rule.dm.001".to_string(),
                "rule.org.001".to_string(),
            ]
        );
    }

    #[test]
    fn rule_derivations_for_subject_empty_when_none_recorded() {
        let conn = seeded();
        let derivations = rule_derivations_for_subject(&conn, "udra.DataContract").unwrap();
        assert!(derivations.is_empty());
    }

    #[test]
    fn rule_derivations_are_not_returned_by_rules_for_subject() {
        let conn = seeded();
        let rules = rules_for_subject(&conn, "udra.DataProduct").unwrap();
        assert!(
            rules
                .iter()
                .all(|(rule, _)| rule.id != "ruleder.data-product-ownership-summary")
        );
    }

    #[test]
    fn rule_derivations_are_not_indexed_for_search() {
        let conn = seeded();
        let results = search_knowledge(&conn, "synthesized", None, 10).unwrap();
        assert!(
            results.is_empty(),
            "a derivation's own summary text should not be searchable -- only its source \
             Rules are, and none of them contain \"synthesized\""
        );
    }
}
