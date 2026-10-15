//! Typed abstract syntax tree for openCypher (Neo4j 5 dialect).
//!
//! Every node implements `Debug + Clone + PartialEq` and [`std::fmt::Display`].
//! `Display` prints canonical single-line Cypher with keywords in upper case,
//! and the printed text always re-parses to an equal tree
//! (`parse(&q.to_string()) == Ok(q)`).

use std::fmt::{self, Display, Formatter};

use crate::keywords::is_reserved;

// ---------------------------------------------------------------------------
// Top level
// ---------------------------------------------------------------------------

/// A complete statement: optional `EXPLAIN`/`PROFILE` prefix plus a body.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// `EXPLAIN` or `PROFILE` prefix, if present.
    pub mode: Option<QueryMode>,
    /// The statement itself.
    pub body: QueryBody,
}

/// Query execution mode prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryMode {
    /// `EXPLAIN`: show the plan without running the query.
    Explain,
    /// `PROFILE`: run the query and report plan statistics.
    Profile,
}

/// Body of a statement.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryBody {
    /// A data query (`MATCH ... RETURN ...`, possibly with `UNION`).
    Regular(RegularQuery),
    /// An index/constraint administration command.
    Schema(SchemaCommand),
}

/// One or more single queries joined by `UNION` / `UNION ALL`.
#[derive(Debug, Clone, PartialEq)]
pub struct RegularQuery {
    /// The first query.
    pub first: SingleQuery,
    /// The remaining queries, each with its union flavour.
    pub unions: Vec<UnionPart>,
}

/// A `UNION [ALL] <query>` tail element.
#[derive(Debug, Clone, PartialEq)]
pub struct UnionPart {
    /// True for `UNION ALL` (keep duplicates).
    pub all: bool,
    /// The query on the right of the union.
    pub query: SingleQuery,
}

/// A sequence of clauses without `UNION`.
#[derive(Debug, Clone, PartialEq)]
pub struct SingleQuery {
    /// Clauses in source order.
    pub clauses: Vec<Clause>,
}

// ---------------------------------------------------------------------------
// Clauses
// ---------------------------------------------------------------------------

/// A single query clause.
#[derive(Debug, Clone, PartialEq)]
pub enum Clause {
    /// `[OPTIONAL] MATCH ... [WHERE ...]`
    Match(MatchClause),
    /// `UNWIND expr AS var`
    Unwind(UnwindClause),
    /// `CREATE pattern`
    Create(CreateClause),
    /// `MERGE pattern [ON CREATE|MATCH SET ...]`
    Merge(MergeClause),
    /// `SET ...`
    Set(SetClause),
    /// `REMOVE ...`
    Remove(RemoveClause),
    /// `[DETACH] DELETE ...`
    Delete(DeleteClause),
    /// `WITH ...`
    With(WithClause),
    /// `RETURN ...`
    Return(ReturnClause),
    /// `CALL proc(args) [YIELD ...]`
    Call(CallClause),
    /// `CALL { subquery }`
    Subquery(SubqueryClause),
}

/// `[OPTIONAL] MATCH pattern [WHERE predicate]`.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchClause {
    /// True for `OPTIONAL MATCH`.
    pub optional: bool,
    /// Comma-separated pattern parts.
    pub pattern: Pattern,
    /// Optional `WHERE` predicate.
    pub where_clause: Option<Expr>,
}

/// `UNWIND list AS variable`.
#[derive(Debug, Clone, PartialEq)]
pub struct UnwindClause {
    /// The list expression.
    pub expr: Expr,
    /// The variable bound to each element.
    pub variable: String,
}

/// `CREATE pattern`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateClause {
    /// Patterns to create.
    pub pattern: Pattern,
}

/// Which `ON ... SET` branch of a `MERGE` an action belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeEvent {
    /// `ON CREATE SET`
    Create,
    /// `ON MATCH SET`
    Match,
}

/// `ON CREATE SET ...` or `ON MATCH SET ...`.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeAction {
    /// When the action fires.
    pub event: MergeEvent,
    /// The `SET` items.
    pub items: Vec<SetItem>,
}

/// `MERGE pattern-part [ON CREATE SET ...] [ON MATCH SET ...]`.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeClause {
    /// The single pattern part to merge.
    pub pattern: PatternPart,
    /// `ON CREATE` / `ON MATCH` actions in source order.
    pub actions: Vec<MergeAction>,
}

/// `SET item, item, ...`.
#[derive(Debug, Clone, PartialEq)]
pub struct SetClause {
    /// The assignments.
    pub items: Vec<SetItem>,
}

/// One assignment inside `SET`.
#[derive(Debug, Clone, PartialEq)]
pub enum SetItem {
    /// `target.prop = value`
    Property {
        /// The property expression (e.g. `n.age`).
        target: Expr,
        /// New value.
        value: Expr,
    },
    /// `n = {map}`: replace all properties.
    Replace {
        /// The node/relationship variable.
        variable: String,
        /// Map or parameter expression.
        value: Expr,
    },
    /// `n += {map}`: merge properties.
    Merge {
        /// The node/relationship variable.
        variable: String,
        /// Map or parameter expression.
        value: Expr,
    },
    /// `n:Label1:Label2`
    Labels {
        /// The node variable.
        variable: String,
        /// Labels to add.
        labels: Vec<String>,
    },
}

/// `REMOVE item, item, ...`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveClause {
    /// Things to remove.
    pub items: Vec<RemoveItem>,
}

/// One item inside `REMOVE`.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoveItem {
    /// `n:Label1:Label2`
    Labels {
        /// The node variable.
        variable: String,
        /// Labels to remove.
        labels: Vec<String>,
    },
    /// `n.prop`
    Property(Expr),
}

/// `[DETACH] DELETE expr, ...`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteClause {
    /// True for `DETACH DELETE`.
    pub detach: bool,
    /// Entities to delete.
    pub exprs: Vec<Expr>,
}

/// Shared body of `WITH` and `RETURN`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionBody {
    /// `DISTINCT` flag.
    pub distinct: bool,
    /// True if the list starts with `*`.
    pub include_existing: bool,
    /// Explicit items (after the optional `*`).
    pub items: Vec<ProjectionItem>,
    /// `ORDER BY` items.
    pub order_by: Vec<SortItem>,
    /// `SKIP` expression.
    pub skip: Option<Expr>,
    /// `LIMIT` expression.
    pub limit: Option<Expr>,
}

/// `expr [AS alias]`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionItem {
    /// The projected expression.
    pub expr: Expr,
    /// Optional alias.
    pub alias: Option<String>,
}

/// One `ORDER BY` key.
#[derive(Debug, Clone, PartialEq)]
pub struct SortItem {
    /// The sort key.
    pub expr: Expr,
    /// True for `DESC`/`DESCENDING`.
    pub descending: bool,
}

/// `WITH projection [WHERE predicate]`.
#[derive(Debug, Clone, PartialEq)]
pub struct WithClause {
    /// The projection.
    pub body: ProjectionBody,
    /// Optional `WHERE` after the projection.
    pub where_clause: Option<Expr>,
}

/// `RETURN projection`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnClause {
    /// The projection.
    pub body: ProjectionBody,
}

/// `CALL namespace.name(args) [YIELD ...]`.
#[derive(Debug, Clone, PartialEq)]
pub struct CallClause {
    /// Namespace segments (`db` in `db.labels`).
    pub namespace: Vec<String>,
    /// Procedure name.
    pub name: String,
    /// Arguments (empty for `CALL db.labels` and `CALL db.labels()`).
    pub args: Vec<Expr>,
    /// Optional `YIELD` part.
    pub yield_clause: Option<YieldClause>,
}

/// `YIELD item, ... [WHERE predicate]`.
#[derive(Debug, Clone, PartialEq)]
pub struct YieldClause {
    /// True for `YIELD *`.
    pub star: bool,
    /// Named results.
    pub items: Vec<YieldItem>,
    /// Optional filter.
    pub where_clause: Option<Expr>,
}

/// `name [AS alias]` inside `YIELD`.
#[derive(Debug, Clone, PartialEq)]
pub struct YieldItem {
    /// Procedure output column.
    pub name: String,
    /// Optional alias.
    pub alias: Option<String>,
}

/// `CALL [(scope vars)] { subquery }`.
#[derive(Debug, Clone, PartialEq)]
pub struct SubqueryClause {
    /// Imported variables for `CALL (a, b) { ... }`; `None` for plain `CALL { ... }`.
    pub scope: Option<Vec<String>>,
    /// The inner query.
    pub query: RegularQuery,
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

/// Comma-separated list of pattern parts.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    /// The parts.
    pub parts: Vec<PatternPart>,
}

/// How a path pattern is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Ordinary pattern.
    Plain,
    /// `shortestPath(...)`
    Shortest,
    /// `allShortestPaths(...)`
    AllShortest,
}

/// `[var =] [shortestPath(] element [)]`.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternPart {
    /// Named-path variable.
    pub variable: Option<String>,
    /// Plain, shortest or all-shortest.
    pub kind: PathKind,
    /// The path element.
    pub element: PatternElement,
}

/// A node followed by zero or more (relationship, node) steps.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternElement {
    /// First node.
    pub start: NodePattern,
    /// Following steps.
    pub steps: Vec<PatternStep>,
}

/// One `-[rel]->(node)` step.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternStep {
    /// The relationship.
    pub rel: RelPattern,
    /// The node it leads to.
    pub node: NodePattern,
}

/// `(var:Label1:Label2 {props})`.
#[derive(Debug, Clone, PartialEq)]
pub struct NodePattern {
    /// Variable name.
    pub variable: Option<String>,
    /// Labels.
    pub labels: Vec<String>,
    /// Property map (`Expr::Map`) or parameter (`Expr::Parameter`).
    pub properties: Option<Expr>,
}

/// Relationship direction in a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// `-->`
    Right,
    /// `<--`
    Left,
    /// `--`
    Both,
}

/// Variable-length bounds: `*`, `*2`, `*1..3`, `*..5`, `*2..`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LengthRange {
    /// Minimum hops (`None` = unbounded below, i.e. 1).
    pub min: Option<u64>,
    /// Maximum hops (`None` = unbounded).
    pub max: Option<u64>,
}

/// `-[var:TYPE1|TYPE2*1..3 {props}]->`.
#[derive(Debug, Clone, PartialEq)]
pub struct RelPattern {
    /// Variable name.
    pub variable: Option<String>,
    /// Allowed relationship types (empty = any).
    pub types: Vec<String>,
    /// Variable-length bounds.
    pub length: Option<LengthRange>,
    /// Property map (`Expr::Map`) or parameter.
    pub properties: Option<Expr>,
    /// Arrow direction.
    pub direction: Direction,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// Literal values.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// 64-bit signed integer (decimal, hex or octal in source).
    Integer(i64),
    /// 64-bit float.
    Float(f64),
    /// String.
    String(String),
    /// `true` / `false`.
    Bool(bool),
    /// `null`.
    Null,
}

/// Prefix operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `NOT`
    Not,
    /// Unary `-`
    Neg,
    /// Unary `+`
    Pos,
}

/// Infix operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `OR`
    Or,
    /// `XOR`
    Xor,
    /// `AND`
    And,
    /// `=`
    Eq,
    /// `<>`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `IN`
    In,
    /// `STARTS WITH`
    StartsWith,
    /// `ENDS WITH`
    EndsWith,
    /// `CONTAINS`
    Contains,
    /// `=~`
    RegexMatch,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `^`
    Pow,
}

/// List predicate kinds: `all`, `any`, `none`, `single`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantifier {
    /// `all(x IN l WHERE p)`
    All,
    /// `any(...)`
    Any,
    /// `none(...)`
    None,
    /// `single(...)`
    Single,
}

/// Body of `EXISTS { ... }`.
#[derive(Debug, Clone, PartialEq)]
pub enum ExistsBody {
    /// `EXISTS { pattern [WHERE predicate] }`
    Pattern {
        /// The pattern.
        pattern: Pattern,
        /// Optional filter.
        where_clause: Option<Box<Expr>>,
    },
    /// `EXISTS { MATCH ... }`: a full subquery.
    Subquery(RegularQuery),
}

/// An expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Literal value.
    Literal(Literal),
    /// Variable reference.
    Variable(String),
    /// `$name` parameter.
    Parameter(String),
    /// `base.key`
    Property {
        /// The expression being accessed.
        base: Box<Expr>,
        /// Property name.
        key: String,
    },
    /// `base[index]`
    Index {
        /// Indexed expression.
        base: Box<Expr>,
        /// Index expression.
        index: Box<Expr>,
    },
    /// `base[from..to]`
    Slice {
        /// Sliced expression.
        base: Box<Expr>,
        /// Inclusive start.
        from: Option<Box<Expr>>,
        /// Exclusive end.
        to: Option<Box<Expr>>,
    },
    /// Prefix operator application.
    Unary {
        /// The operator.
        op: UnaryOp,
        /// The operand.
        expr: Box<Expr>,
    },
    /// Infix operator application.
    Binary {
        /// The operator.
        op: BinaryOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `expr IS [NOT] NULL`
    IsNull {
        /// Tested expression.
        expr: Box<Expr>,
        /// True for `IS NOT NULL`.
        negated: bool,
    },
    /// `expr:Label1:Label2`
    HasLabels {
        /// Tested expression.
        expr: Box<Expr>,
        /// Required labels (or relationship types).
        labels: Vec<String>,
    },
    /// `ns.name([DISTINCT] args)`
    FunctionCall {
        /// Namespace segments.
        namespace: Vec<String>,
        /// Function name.
        name: String,
        /// `DISTINCT` flag.
        distinct: bool,
        /// Arguments.
        args: Vec<Expr>,
    },
    /// `count(*)`
    CountStar,
    /// `CASE [scrutinee] WHEN .. THEN .. [ELSE ..] END`
    Case {
        /// Present for the simple form.
        scrutinee: Option<Box<Expr>>,
        /// `(WHEN, THEN)` pairs.
        whens: Vec<(Expr, Expr)>,
        /// `ELSE` branch.
        else_expr: Option<Box<Expr>>,
    },
    /// `[a, b, c]`
    List(Vec<Expr>),
    /// `{k: v, ...}`
    Map(Vec<(String, Expr)>),
    /// `[x IN list WHERE pred | projection]`
    ListComprehension {
        /// Iteration variable.
        variable: String,
        /// Source list.
        list: Box<Expr>,
        /// Optional filter.
        predicate: Option<Box<Expr>>,
        /// Optional projection.
        projection: Option<Box<Expr>>,
    },
    /// `[p = (a)-->(b) WHERE pred | projection]`
    PatternComprehension {
        /// Optional path variable.
        path_variable: Option<String>,
        /// The pattern.
        pattern: Box<PatternElement>,
        /// Optional filter.
        predicate: Option<Box<Expr>>,
        /// Projection.
        projection: Box<Expr>,
    },
    /// A bare pattern used as a boolean predicate: `(a)-->(b)`.
    PatternPredicate(Box<PatternElement>),
    /// `EXISTS { ... }`
    Exists(Box<ExistsBody>),
    /// `all|any|none|single(x IN list WHERE pred)`
    ListPredicate {
        /// Which quantifier.
        kind: Quantifier,
        /// Iteration variable.
        variable: String,
        /// Source list.
        list: Box<Expr>,
        /// Predicate.
        predicate: Option<Box<Expr>>,
    },
    /// `reduce(acc = init, x IN list | expr)`
    Reduce {
        /// Accumulator variable.
        accumulator: String,
        /// Initial accumulator value.
        init: Box<Expr>,
        /// Iteration variable.
        variable: String,
        /// Source list.
        list: Box<Expr>,
        /// Step expression.
        expr: Box<Expr>,
    },
}

// ---------------------------------------------------------------------------
// Schema commands
// ---------------------------------------------------------------------------

/// Index and constraint administration.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaCommand {
    /// `CREATE INDEX ...`
    CreateIndex(CreateIndex),
    /// `DROP INDEX name [IF EXISTS]`
    DropIndex {
        /// Index name.
        name: String,
        /// `IF EXISTS` flag.
        if_exists: bool,
    },
    /// `CREATE CONSTRAINT ...`
    CreateConstraint(CreateConstraint),
    /// `DROP CONSTRAINT name [IF EXISTS]`
    DropConstraint {
        /// Constraint name.
        name: String,
        /// `IF EXISTS` flag.
        if_exists: bool,
    },
    /// `SHOW INDEXES`
    ShowIndexes,
    /// `SHOW CONSTRAINTS`
    ShowConstraints,
}

/// What an index or constraint applies to.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaTarget {
    /// `(var:Label)`
    Node {
        /// Pattern variable.
        variable: String,
        /// Node label.
        label: String,
    },
    /// `()-[var:TYPE]-()`
    Relationship {
        /// Pattern variable.
        variable: String,
        /// Relationship type.
        rel_type: String,
    },
}

/// Index flavour keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    /// `RANGE`
    Range,
    /// `TEXT`
    Text,
    /// `POINT`
    Point,
}

/// `CREATE [kind] INDEX [name] [IF NOT EXISTS] FOR target ON (props)`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateIndex {
    /// Optional index name.
    pub name: Option<String>,
    /// Optional index flavour.
    pub kind: Option<IndexKind>,
    /// `IF NOT EXISTS` flag.
    pub if_not_exists: bool,
    /// Indexed label / relationship type.
    pub target: SchemaTarget,
    /// Indexed property names.
    pub properties: Vec<String>,
}

/// Constraint flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    /// `IS UNIQUE`
    Unique,
    /// `IS NOT NULL` (existence)
    NotNull,
    /// `IS NODE KEY`
    NodeKey,
}

/// `CREATE CONSTRAINT [name] [IF NOT EXISTS] FOR target REQUIRE props IS kind`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateConstraint {
    /// Optional constraint name.
    pub name: Option<String>,
    /// `IF NOT EXISTS` flag.
    pub if_not_exists: bool,
    /// Constrained label / relationship type.
    pub target: SchemaTarget,
    /// Constrained property names.
    pub properties: Vec<String>,
    /// Constraint flavour.
    pub kind: ConstraintKind,
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

fn is_simple_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn write_quoted(f: &mut Formatter<'_>, s: &str) -> fmt::Result {
    f.write_str("`")?;
    for c in s.chars() {
        if c == '`' {
            f.write_str("``")?;
        } else {
            write!(f, "{c}")?;
        }
    }
    f.write_str("`")
}

/// Property key, label, relationship type: quote only when not a plain identifier.
fn write_name(f: &mut Formatter<'_>, s: &str) -> fmt::Result {
    if is_simple_ident(s) {
        f.write_str(s)
    } else {
        write_quoted(f, s)
    }
}

/// Variable-like identifier: also quote reserved words.
fn write_var(f: &mut Formatter<'_>, s: &str) -> fmt::Result {
    if is_simple_ident(s) && !is_reserved(s) {
        f.write_str(s)
    } else {
        write_quoted(f, s)
    }
}

/// Schema object name: additionally avoids `FOR`, `ON`, `IF`, which end the name.
fn write_schema_name(f: &mut Formatter<'_>, s: &str) -> fmt::Result {
    let clash = ["FOR", "ON", "IF"]
        .iter()
        .any(|k| k.eq_ignore_ascii_case(s));
    if clash {
        write_quoted(f, s)
    } else {
        write_var(f, s)
    }
}

fn write_string(f: &mut Formatter<'_>, s: &str) -> fmt::Result {
    f.write_str("'")?;
    for c in s.chars() {
        match c {
            '\\' => f.write_str("\\\\")?,
            '\'' => f.write_str("\\'")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            '\u{8}' => f.write_str("\\b")?,
            '\u{c}' => f.write_str("\\f")?,
            c if (c as u32) < 0x20 || c == '\u{7f}' => write!(f, "\\u{:04X}", c as u32)?,
            c => write!(f, "{c}")?,
        }
    }
    f.write_str("'")
}

fn write_joined<T>(
    f: &mut Formatter<'_>,
    items: &[T],
    sep: &str,
    mut each: impl FnMut(&mut Formatter<'_>, &T) -> fmt::Result,
) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(sep)?;
        }
        each(f, item)?;
    }
    Ok(())
}

fn write_labels(f: &mut Formatter<'_>, labels: &[String]) -> fmt::Result {
    for l in labels {
        f.write_str(":")?;
        write_name(f, l)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Display: statements and clauses
// ---------------------------------------------------------------------------

impl Display for Query {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(mode) = self.mode {
            write!(f, "{mode} ")?;
        }
        match &self.body {
            QueryBody::Regular(q) => write!(f, "{q}"),
            QueryBody::Schema(s) => write!(f, "{s}"),
        }
    }
}

impl Display for QueryMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            QueryMode::Explain => "EXPLAIN",
            QueryMode::Profile => "PROFILE",
        })
    }
}

impl Display for RegularQuery {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.first)?;
        for u in &self.unions {
            f.write_str(if u.all { " UNION ALL " } else { " UNION " })?;
            write!(f, "{}", u.query)?;
        }
        Ok(())
    }
}

impl Display for SingleQuery {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write_joined(f, &self.clauses, " ", |f, c| write!(f, "{c}"))
    }
}

impl Display for Clause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Clause::Match(c) => write!(f, "{c}"),
            Clause::Unwind(c) => {
                write!(f, "UNWIND ")?;
                fmt_expr(f, &c.expr, 0)?;
                f.write_str(" AS ")?;
                write_var(f, &c.variable)
            }
            Clause::Create(c) => write!(f, "CREATE {}", c.pattern),
            Clause::Merge(c) => write!(f, "{c}"),
            Clause::Set(c) => {
                f.write_str("SET ")?;
                write_set_items(f, &c.items)
            }
            Clause::Remove(c) => {
                f.write_str("REMOVE ")?;
                write_joined(f, &c.items, ", ", |f, i| match i {
                    RemoveItem::Labels { variable, labels } => {
                        write_var(f, variable)?;
                        write_labels(f, labels)
                    }
                    RemoveItem::Property(e) => fmt_expr(f, e, 0),
                })
            }
            Clause::Delete(c) => {
                f.write_str(if c.detach {
                    "DETACH DELETE "
                } else {
                    "DELETE "
                })?;
                write_joined(f, &c.exprs, ", ", |f, e| fmt_expr(f, e, 0))
            }
            Clause::With(c) => {
                write!(f, "WITH {}", c.body)?;
                if let Some(w) = &c.where_clause {
                    f.write_str(" WHERE ")?;
                    fmt_expr(f, w, 0)?;
                }
                Ok(())
            }
            Clause::Return(c) => write!(f, "RETURN {}", c.body),
            Clause::Call(c) => write!(f, "{c}"),
            Clause::Subquery(c) => {
                f.write_str("CALL ")?;
                if let Some(scope) = &c.scope {
                    f.write_str("(")?;
                    write_joined(f, scope, ", ", |f, v| write_var(f, v))?;
                    f.write_str(") ")?;
                }
                write!(f, "{{ {} }}", c.query)
            }
        }
    }
}

impl Display for MatchClause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.optional {
            f.write_str("OPTIONAL ")?;
        }
        write!(f, "MATCH {}", self.pattern)?;
        if let Some(w) = &self.where_clause {
            f.write_str(" WHERE ")?;
            fmt_expr(f, w, 0)?;
        }
        Ok(())
    }
}

impl Display for MergeClause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "MERGE {}", self.pattern)?;
        for a in &self.actions {
            f.write_str(match a.event {
                MergeEvent::Create => " ON CREATE SET ",
                MergeEvent::Match => " ON MATCH SET ",
            })?;
            write_set_items(f, &a.items)?;
        }
        Ok(())
    }
}

fn write_set_items(f: &mut Formatter<'_>, items: &[SetItem]) -> fmt::Result {
    write_joined(f, items, ", ", |f, i| match i {
        SetItem::Property { target, value } => {
            fmt_expr(f, target, 11)?;
            f.write_str(" = ")?;
            fmt_expr(f, value, 0)
        }
        SetItem::Replace { variable, value } => {
            write_var(f, variable)?;
            f.write_str(" = ")?;
            fmt_expr(f, value, 0)
        }
        SetItem::Merge { variable, value } => {
            write_var(f, variable)?;
            f.write_str(" += ")?;
            fmt_expr(f, value, 0)
        }
        SetItem::Labels { variable, labels } => {
            write_var(f, variable)?;
            write_labels(f, labels)
        }
    })
}

impl Display for ProjectionBody {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.distinct {
            f.write_str("DISTINCT ")?;
        }
        if self.include_existing {
            f.write_str("*")?;
            if !self.items.is_empty() {
                f.write_str(", ")?;
            }
        }
        write_joined(f, &self.items, ", ", |f, i| {
            fmt_expr(f, &i.expr, 0)?;
            if let Some(a) = &i.alias {
                f.write_str(" AS ")?;
                write_var(f, a)?;
            }
            Ok(())
        })?;
        if !self.order_by.is_empty() {
            f.write_str(" ORDER BY ")?;
            write_joined(f, &self.order_by, ", ", |f, s| {
                fmt_expr(f, &s.expr, 0)?;
                if s.descending {
                    f.write_str(" DESC")?;
                }
                Ok(())
            })?;
        }
        if let Some(s) = &self.skip {
            f.write_str(" SKIP ")?;
            fmt_expr(f, s, 0)?;
        }
        if let Some(l) = &self.limit {
            f.write_str(" LIMIT ")?;
            fmt_expr(f, l, 0)?;
        }
        Ok(())
    }
}

impl Display for CallClause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("CALL ")?;
        for ns in &self.namespace {
            write_var(f, ns)?;
            f.write_str(".")?;
        }
        write_var(f, &self.name)?;
        f.write_str("(")?;
        write_joined(f, &self.args, ", ", |f, a| fmt_expr(f, a, 0))?;
        f.write_str(")")?;
        if let Some(y) = &self.yield_clause {
            f.write_str(" YIELD ")?;
            if y.star {
                f.write_str("*")?;
            }
            write_joined(f, &y.items, ", ", |f, i| {
                write_var(f, &i.name)?;
                if let Some(a) = &i.alias {
                    f.write_str(" AS ")?;
                    write_var(f, a)?;
                }
                Ok(())
            })?;
            if let Some(w) = &y.where_clause {
                f.write_str(" WHERE ")?;
                fmt_expr(f, w, 0)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Display: patterns
// ---------------------------------------------------------------------------

impl Display for Pattern {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write_joined(f, &self.parts, ", ", |f, p| write!(f, "{p}"))
    }
}

impl Display for PatternPart {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(v) = &self.variable {
            write_var(f, v)?;
            f.write_str(" = ")?;
        }
        match self.kind {
            PathKind::Plain => write!(f, "{}", self.element),
            PathKind::Shortest => write!(f, "shortestPath({})", self.element),
            PathKind::AllShortest => write!(f, "allShortestPaths({})", self.element),
        }
    }
}

impl Display for PatternElement {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.start)?;
        for s in &self.steps {
            write!(f, "{}{}", s.rel, s.node)?;
        }
        Ok(())
    }
}

impl Display for NodePattern {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        let mut wrote = false;
        if let Some(v) = &self.variable {
            write_var(f, v)?;
            wrote = true;
        }
        if !self.labels.is_empty() {
            write_labels(f, &self.labels)?;
            wrote = true;
        }
        if let Some(p) = &self.properties {
            if wrote {
                f.write_str(" ")?;
            }
            fmt_expr(f, p, 0)?;
        }
        f.write_str(")")
    }
}

impl Display for RelPattern {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.direction == Direction::Left {
            f.write_str("<")?;
        }
        f.write_str("-")?;
        let has_detail = self.variable.is_some()
            || !self.types.is_empty()
            || self.length.is_some()
            || self.properties.is_some();
        if has_detail {
            f.write_str("[")?;
            let mut wrote = false;
            if let Some(v) = &self.variable {
                write_var(f, v)?;
                wrote = true;
            }
            if !self.types.is_empty() {
                f.write_str(":")?;
                write_joined(f, &self.types, "|", |f, t| write_name(f, t))?;
                wrote = true;
            }
            if let Some(r) = &self.length {
                write!(f, "{r}")?;
                wrote = true;
            }
            if let Some(p) = &self.properties {
                if wrote {
                    f.write_str(" ")?;
                }
                fmt_expr(f, p, 0)?;
            }
            f.write_str("]")?;
        }
        f.write_str("-")?;
        if self.direction == Direction::Right {
            f.write_str(">")?;
        }
        Ok(())
    }
}

impl Display for LengthRange {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("*")?;
        match (self.min, self.max) {
            (None, None) => Ok(()),
            (Some(a), Some(b)) if a == b => write!(f, "{a}"),
            (min, max) => {
                if let Some(a) = min {
                    write!(f, "{a}")?;
                }
                f.write_str("..")?;
                if let Some(b) = max {
                    write!(f, "{b}")?;
                }
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Display: expressions
// ---------------------------------------------------------------------------

impl BinaryOp {
    /// The operator's source text.
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Or => "OR",
            BinaryOp::Xor => "XOR",
            BinaryOp::And => "AND",
            BinaryOp::Eq => "=",
            BinaryOp::Ne => "<>",
            BinaryOp::Lt => "<",
            BinaryOp::Le => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Ge => ">=",
            BinaryOp::In => "IN",
            BinaryOp::StartsWith => "STARTS WITH",
            BinaryOp::EndsWith => "ENDS WITH",
            BinaryOp::Contains => "CONTAINS",
            BinaryOp::RegexMatch => "=~",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "^",
        }
    }

    fn precedence(self) -> u8 {
        match self {
            BinaryOp::Or => 1,
            BinaryOp::Xor => 2,
            BinaryOp::And => 3,
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => 5,
            BinaryOp::In
            | BinaryOp::StartsWith
            | BinaryOp::EndsWith
            | BinaryOp::Contains
            | BinaryOp::RegexMatch => 6,
            BinaryOp::Add | BinaryOp::Sub => 7,
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 8,
            BinaryOp::Pow => 10,
        }
    }
}

impl Display for BinaryOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.symbol())
    }
}

impl Display for UnaryOp {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnaryOp::Not => "NOT",
            UnaryOp::Neg => "-",
            UnaryOp::Pos => "+",
        })
    }
}

impl Quantifier {
    fn name(self) -> &'static str {
        match self {
            Quantifier::All => "all",
            Quantifier::Any => "any",
            Quantifier::None => "none",
            Quantifier::Single => "single",
        }
    }
}

fn precedence(e: &Expr) -> u8 {
    match e {
        Expr::Binary { op, .. } => op.precedence(),
        Expr::Unary {
            op: UnaryOp::Not, ..
        } => 4,
        Expr::Unary { .. } => 9,
        Expr::IsNull { .. } => 6,
        Expr::HasLabels { .. } => 10,
        Expr::Literal(Literal::Integer(i)) if *i < 0 => 9,
        Expr::Literal(Literal::Float(x)) if x.is_sign_negative() => 9,
        Expr::Property { .. } | Expr::Index { .. } | Expr::Slice { .. } => 11,
        _ => 12,
    }
}

/// Writes `e`, parenthesising when its precedence is below `min`.
fn fmt_expr(f: &mut Formatter<'_>, e: &Expr, min: u8) -> fmt::Result {
    let paren = precedence(e) < min;
    if paren {
        f.write_str("(")?;
    }
    fmt_expr_inner(f, e)?;
    if paren {
        f.write_str(")")?;
    }
    Ok(())
}

fn fmt_opt(f: &mut Formatter<'_>, prefix: &str, e: &Option<Box<Expr>>) -> fmt::Result {
    if let Some(e) = e {
        f.write_str(prefix)?;
        fmt_expr(f, e, 0)?;
    }
    Ok(())
}

fn fmt_expr_inner(f: &mut Formatter<'_>, e: &Expr) -> fmt::Result {
    match e {
        Expr::Literal(l) => write!(f, "{l}"),
        Expr::Variable(v) => write_var(f, v),
        Expr::Parameter(p) => {
            f.write_str("$")?;
            if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
                f.write_str(p)
            } else {
                write_name(f, p)
            }
        }
        Expr::Property { base, key } => {
            fmt_expr(f, base, 11)?;
            f.write_str(".")?;
            write_name(f, key)
        }
        Expr::Index { base, index } => {
            fmt_expr(f, base, 11)?;
            f.write_str("[")?;
            fmt_expr(f, index, 0)?;
            f.write_str("]")
        }
        Expr::Slice { base, from, to } => {
            fmt_expr(f, base, 11)?;
            f.write_str("[")?;
            if let Some(a) = from {
                fmt_expr(f, a, 0)?;
            }
            f.write_str("..")?;
            if let Some(b) = to {
                fmt_expr(f, b, 0)?;
            }
            f.write_str("]")
        }
        Expr::Unary { op, expr } => match op {
            UnaryOp::Not => {
                f.write_str("NOT ")?;
                fmt_expr(f, expr, 4)
            }
            _ => {
                write!(f, "{op}")?;
                fmt_expr(f, expr, 10)
            }
        },
        Expr::Binary { op, lhs, rhs } => {
            let p = op.precedence();
            fmt_expr(f, lhs, p)?;
            write!(f, " {op} ")?;
            fmt_expr(f, rhs, p + 1)
        }
        Expr::IsNull { expr, negated } => {
            fmt_expr(f, expr, 6)?;
            f.write_str(if *negated { " IS NOT NULL" } else { " IS NULL" })
        }
        Expr::HasLabels { expr, labels } => {
            fmt_expr(f, expr, 11)?;
            write_labels(f, labels)
        }
        Expr::FunctionCall {
            namespace,
            name,
            distinct,
            args,
        } => {
            for ns in namespace {
                write_var(f, ns)?;
                f.write_str(".")?;
            }
            // Names that the parser treats as special forms when unquoted
            // are quoted so the call stays an ordinary function call.
            let var_op = |op: BinaryOp| {
                matches!(
                    args.first(),
                    Some(Expr::Binary { op: o, lhs, .. })
                        if *o == op && matches!(**lhs, Expr::Variable(_))
                )
            };
            let special = namespace.is_empty()
                && ((["all", "any", "none", "single"]
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(name))
                    && var_op(BinaryOp::In))
                    || (name.eq_ignore_ascii_case("reduce") && var_op(BinaryOp::Eq)));
            if special {
                write_quoted(f, name)?;
            } else {
                write_var(f, name)?;
            }
            f.write_str("(")?;
            if *distinct {
                f.write_str("DISTINCT ")?;
            }
            write_joined(f, args, ", ", |f, a| fmt_expr(f, a, 0))?;
            f.write_str(")")
        }
        Expr::CountStar => f.write_str("count(*)"),
        Expr::Case {
            scrutinee,
            whens,
            else_expr,
        } => {
            f.write_str("CASE")?;
            fmt_opt(f, " ", scrutinee)?;
            for (w, t) in whens {
                f.write_str(" WHEN ")?;
                fmt_expr(f, w, 0)?;
                f.write_str(" THEN ")?;
                fmt_expr(f, t, 0)?;
            }
            fmt_opt(f, " ELSE ", else_expr)?;
            f.write_str(" END")
        }
        Expr::List(items) => {
            f.write_str("[")?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                // `[x IN l]` would re-parse as a list comprehension.
                let comp_like = i == 0
                    && matches!(
                        item,
                        Expr::Binary { op: BinaryOp::In, lhs, .. }
                            if matches!(**lhs, Expr::Variable(_))
                    );
                if comp_like {
                    f.write_str("(")?;
                    fmt_expr(f, item, 0)?;
                    f.write_str(")")?;
                } else {
                    fmt_expr(f, item, 0)?;
                }
            }
            f.write_str("]")
        }
        Expr::Map(entries) => {
            f.write_str("{")?;
            write_joined(f, entries, ", ", |f, (k, v)| {
                write_name(f, k)?;
                f.write_str(": ")?;
                fmt_expr(f, v, 0)
            })?;
            f.write_str("}")
        }
        Expr::ListComprehension {
            variable,
            list,
            predicate,
            projection,
        } => {
            f.write_str("[")?;
            write_var(f, variable)?;
            f.write_str(" IN ")?;
            fmt_expr(f, list, 0)?;
            fmt_opt(f, " WHERE ", predicate)?;
            fmt_opt(f, " | ", projection)?;
            f.write_str("]")
        }
        Expr::PatternComprehension {
            path_variable,
            pattern,
            predicate,
            projection,
        } => {
            f.write_str("[")?;
            if let Some(v) = path_variable {
                write_var(f, v)?;
                f.write_str(" = ")?;
            }
            write!(f, "{pattern}")?;
            fmt_opt(f, " WHERE ", predicate)?;
            f.write_str(" | ")?;
            fmt_expr(f, projection, 0)?;
            f.write_str("]")
        }
        Expr::PatternPredicate(p) => write!(f, "{p}"),
        Expr::Exists(body) => match &**body {
            ExistsBody::Pattern {
                pattern,
                where_clause,
            } => {
                write!(f, "EXISTS {{ {pattern}")?;
                fmt_opt(f, " WHERE ", where_clause)?;
                f.write_str(" }")
            }
            ExistsBody::Subquery(q) => write!(f, "EXISTS {{ {q} }}"),
        },
        Expr::ListPredicate {
            kind,
            variable,
            list,
            predicate,
        } => {
            write!(f, "{}(", kind.name())?;
            write_var(f, variable)?;
            f.write_str(" IN ")?;
            fmt_expr(f, list, 0)?;
            fmt_opt(f, " WHERE ", predicate)?;
            f.write_str(")")
        }
        Expr::Reduce {
            accumulator,
            init,
            variable,
            list,
            expr,
        } => {
            f.write_str("reduce(")?;
            write_var(f, accumulator)?;
            f.write_str(" = ")?;
            fmt_expr(f, init, 0)?;
            f.write_str(", ")?;
            write_var(f, variable)?;
            f.write_str(" IN ")?;
            fmt_expr(f, list, 0)?;
            f.write_str(" | ")?;
            fmt_expr(f, expr, 0)?;
            f.write_str(")")
        }
    }
}

impl Display for Expr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fmt_expr(f, self, 0)
    }
}

impl Display for Literal {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Integer(i) => write!(f, "{i}"),
            Literal::Float(x) => write!(f, "{x:?}"),
            Literal::String(s) => write_string(f, s),
            Literal::Bool(b) => f.write_str(if *b { "true" } else { "false" }),
            Literal::Null => f.write_str("null"),
        }
    }
}

// ---------------------------------------------------------------------------
// Display: schema commands
// ---------------------------------------------------------------------------

impl Display for SchemaTarget {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SchemaTarget::Node { variable, label } => {
                f.write_str("(")?;
                write_var(f, variable)?;
                f.write_str(":")?;
                write_name(f, label)?;
                f.write_str(")")
            }
            SchemaTarget::Relationship { variable, rel_type } => {
                f.write_str("()-[")?;
                write_var(f, variable)?;
                f.write_str(":")?;
                write_name(f, rel_type)?;
                f.write_str("]-()")
            }
        }
    }
}

impl SchemaTarget {
    fn variable(&self) -> &str {
        match self {
            SchemaTarget::Node { variable, .. } | SchemaTarget::Relationship { variable, .. } => {
                variable
            }
        }
    }
}

fn write_target_props(
    f: &mut Formatter<'_>,
    target: &SchemaTarget,
    props: &[String],
    parens_if_single: bool,
) -> fmt::Result {
    let paren = parens_if_single || props.len() != 1;
    if paren {
        f.write_str("(")?;
    }
    write_joined(f, props, ", ", |f, p| {
        write_var(f, target.variable())?;
        f.write_str(".")?;
        write_name(f, p)
    })?;
    if paren {
        f.write_str(")")?;
    }
    Ok(())
}

impl Display for SchemaCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SchemaCommand::CreateIndex(c) => {
                f.write_str("CREATE ")?;
                match c.kind {
                    Some(IndexKind::Range) => f.write_str("RANGE ")?,
                    Some(IndexKind::Text) => f.write_str("TEXT ")?,
                    Some(IndexKind::Point) => f.write_str("POINT ")?,
                    None => {}
                }
                f.write_str("INDEX ")?;
                if let Some(n) = &c.name {
                    write_schema_name(f, n)?;
                    f.write_str(" ")?;
                }
                if c.if_not_exists {
                    f.write_str("IF NOT EXISTS ")?;
                }
                write!(f, "FOR {} ON ", c.target)?;
                write_target_props(f, &c.target, &c.properties, true)
            }
            SchemaCommand::DropIndex { name, if_exists } => {
                f.write_str("DROP INDEX ")?;
                write_schema_name(f, name)?;
                if *if_exists {
                    f.write_str(" IF EXISTS")?;
                }
                Ok(())
            }
            SchemaCommand::CreateConstraint(c) => {
                f.write_str("CREATE CONSTRAINT ")?;
                if let Some(n) = &c.name {
                    write_schema_name(f, n)?;
                    f.write_str(" ")?;
                }
                if c.if_not_exists {
                    f.write_str("IF NOT EXISTS ")?;
                }
                write!(f, "FOR {} REQUIRE ", c.target)?;
                write_target_props(f, &c.target, &c.properties, false)?;
                f.write_str(match c.kind {
                    ConstraintKind::Unique => " IS UNIQUE",
                    ConstraintKind::NotNull => " IS NOT NULL",
                    ConstraintKind::NodeKey => " IS NODE KEY",
                })
            }
            SchemaCommand::DropConstraint { name, if_exists } => {
                f.write_str("DROP CONSTRAINT ")?;
                write_schema_name(f, name)?;
                if *if_exists {
                    f.write_str(" IF EXISTS")?;
                }
                Ok(())
            }
            SchemaCommand::ShowIndexes => f.write_str("SHOW INDEXES"),
            SchemaCommand::ShowConstraints => f.write_str("SHOW CONSTRAINTS"),
        }
    }
}
