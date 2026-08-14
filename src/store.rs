//! The knowledge-model-v2 store: five tables (`Source`, `SourceAuthority`,
//! `Subject`, `Rule`, `RuleRelation`) replacing the earlier
//! `AuthorityLayer`/`Construct`/fixed-4-layer model. The fuller
//! seven-table design this was built from also specifies
//! `SelectionGroup` (cardinality constraints over a set of Rules) and
//! `RuleDerivation` (firewalled, non-authoritative rollup views) -- both
//! deliberately not implemented yet, since nothing in this vertical
//! slice's two tools needs them. Add them when a real case does, not
//! speculatively.
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
//! This vertical slice proves the model against real UDRA data end-to-end:
//! schema, insert-time invariants (DAG cycle rejection, supersession
//! cascade), the two-tier conflict-candidate query, and two MCP tools
//! (`lookup_subject`, `crosscut_conflicts` -- see `main.rs`). It does not
//! yet carry forward the previous model's full 15-tool surface, the
//! `knowledge-mcp` importer, file-backed persistence, or search -- those
//! were all built around the schema this replaces and are deferred to
//! follow-up work, not silently dropped.

use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRelation {
    pub rule_a_id: String,
    pub rule_b_id: String,
    pub relation_type: RelationType,
    pub status: RelationStatus,
    pub confirmed_by: String,
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
    Ok(())
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
        assert_eq!(relationships.len(), 1);
        assert_eq!(relationships[0].0.id, "rule.dm.002");
        assert_eq!(
            relationships[0].0.related_subject_id.as_deref(),
            Some("udra.DataContract")
        );
    }

    #[test]
    fn outgoing_relationships_filters_by_relationship_type() {
        let conn = seeded();
        let matching = outgoing_relationships(&conn, "udra.DataProduct", Some("exposes")).unwrap();
        assert_eq!(matching.len(), 1);
        let non_matching =
            outgoing_relationships(&conn, "udra.DataProduct", Some("no-such-type")).unwrap();
        assert!(non_matching.is_empty());
    }
}
