//! Static analysis helpers over the AST.

use uriel_cypher::{
    Clause, Expr, Pattern, PatternElement, Query, QueryBody, RegularQuery, SchemaCommand,
};

/// Name of the first write clause in the statement, if any.
pub fn write_kind(q: &Query) -> Option<&'static str> {
    match &q.body {
        QueryBody::Schema(s) => match s {
            SchemaCommand::ShowIndexes | SchemaCommand::ShowConstraints => None,
            SchemaCommand::CreateIndex(_) => Some("CREATE INDEX"),
            SchemaCommand::DropIndex { .. } => Some("DROP INDEX"),
            SchemaCommand::CreateConstraint(_) => Some("CREATE CONSTRAINT"),
            SchemaCommand::DropConstraint { .. } => Some("DROP CONSTRAINT"),
        },
        QueryBody::Regular(r) => regular_write_kind(r),
    }
}

pub fn regular_write_kind(r: &RegularQuery) -> Option<&'static str> {
    let singles = std::iter::once(&r.first).chain(r.unions.iter().map(|u| &u.query));
    for s in singles {
        for c in &s.clauses {
            let k = match c {
                Clause::Create(_) => Some("CREATE"),
                Clause::Merge(_) => Some("MERGE"),
                Clause::Set(_) => Some("SET"),
                Clause::Remove(_) => Some("REMOVE"),
                Clause::Delete(d) => Some(if d.detach { "DETACH DELETE" } else { "DELETE" }),
                Clause::Subquery(sq) => regular_write_kind(&sq.query),
                _ => None,
            };
            if k.is_some() {
                return k;
            }
        }
    }
    None
}

const AGGREGATES: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "collect",
    "stdev",
    "stdevp",
    "percentilecont",
    "percentiledisc",
];

pub fn is_aggregate_name(name: &str) -> bool {
    AGGREGATES.contains(&name.to_ascii_lowercase().as_str())
}

/// Does the expression contain an aggregate call (outside nested pattern subqueries)?
pub fn contains_agg(e: &Expr) -> bool {
    match e {
        Expr::CountStar => true,
        Expr::FunctionCall {
            namespace,
            name,
            args,
            ..
        } => (namespace.is_empty() && is_aggregate_name(name)) || args.iter().any(contains_agg),
        Expr::Literal(_) | Expr::Variable(_) | Expr::Parameter(_) => false,
        Expr::Property { base, .. } => contains_agg(base),
        Expr::Index { base, index } => contains_agg(base) || contains_agg(index),
        Expr::Slice { base, from, to } => {
            contains_agg(base)
                || from.as_deref().is_some_and(contains_agg)
                || to.as_deref().is_some_and(contains_agg)
        }
        Expr::Unary { expr, .. } | Expr::IsNull { expr, .. } | Expr::HasLabels { expr, .. } => {
            contains_agg(expr)
        }
        Expr::Binary { lhs, rhs, .. } => contains_agg(lhs) || contains_agg(rhs),
        Expr::Case {
            scrutinee,
            whens,
            else_expr,
        } => {
            scrutinee.as_deref().is_some_and(contains_agg)
                || whens
                    .iter()
                    .any(|(a, b)| contains_agg(a) || contains_agg(b))
                || else_expr.as_deref().is_some_and(contains_agg)
        }
        Expr::List(items) => items.iter().any(contains_agg),
        Expr::Map(items) => items.iter().any(|(_, v)| contains_agg(v)),
        Expr::ListComprehension {
            list,
            predicate,
            projection,
            ..
        } => {
            contains_agg(list)
                || predicate.as_deref().is_some_and(contains_agg)
                || projection.as_deref().is_some_and(contains_agg)
        }
        Expr::ListPredicate {
            list, predicate, ..
        } => contains_agg(list) || predicate.as_deref().is_some_and(contains_agg),
        Expr::Reduce {
            init, list, expr, ..
        } => contains_agg(init) || contains_agg(list) || contains_agg(expr),
        Expr::PatternComprehension { .. } | Expr::PatternPredicate(_) | Expr::Exists(_) => false,
    }
}

/// Variables introduced by a pattern: node, relationship and path variables.
pub fn pattern_vars(p: &Pattern, out: &mut Vec<String>) {
    for part in &p.parts {
        if let Some(v) = &part.variable {
            push_unique(out, v);
        }
        element_vars(&part.element, out);
    }
}

pub fn element_vars(el: &PatternElement, out: &mut Vec<String>) {
    if let Some(v) = &el.start.variable {
        push_unique(out, v);
    }
    for s in &el.steps {
        if let Some(v) = &s.rel.variable {
            push_unique(out, v);
        }
        if let Some(v) = &s.node.variable {
            push_unique(out, v);
        }
    }
}

pub fn push_unique(out: &mut Vec<String>, v: &str) {
    if !out.iter().any(|x| x == v) {
        out.push(v.to_string());
    }
}

/// Splits a predicate on top-level `AND`.
pub fn conjuncts<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    if let Expr::Binary {
        op: uriel_cypher::BinaryOp::And,
        lhs,
        rhs,
    } = e
    {
        conjuncts(lhs, out);
        conjuncts(rhs, out);
    } else {
        out.push(e);
    }
}
