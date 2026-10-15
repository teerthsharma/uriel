//! Expression evaluation with Cypher three-valued logic.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use uriel_cypher::{
    BinaryOp, ExistsBody, Expr, Literal, PathKind, PatternPart, Quantifier, UnaryOp,
};

use crate::ctx::Ctx;
use crate::error::{rt, sem, QResult, QueryError};
use crate::val::{self, compare, equals, Row, Val};

fn tri_and(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

fn tri_or(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

fn tri(v: Option<bool>) -> Val {
    v.map_or(Val::Null, Val::Bool)
}

impl Ctx<'_> {
    /// Interprets a value as a boolean for `AND`/`OR`/`NOT`/`WHERE`; null stays null.
    fn truth(&self, v: &Val) -> QResult<Option<bool>> {
        match v {
            Val::Null => Ok(None),
            Val::Bool(b) => Ok(Some(*b)),
            other => rt(format!("expected a Boolean but got {}", other.type_name())),
        }
    }

    pub fn eval_truth(&mut self, e: &Expr, row: &Row) -> QResult<Option<bool>> {
        let v = self.eval(e, row)?;
        self.truth(&v)
    }

    pub fn eval(&mut self, e: &Expr, row: &Row) -> QResult<Val> {
        match e {
            Expr::Literal(l) => Ok(match l {
                Literal::Integer(i) => Val::Int(*i),
                Literal::Float(f) => Val::Float(*f),
                Literal::String(s) => Val::Str(s.clone()),
                Literal::Bool(b) => Val::Bool(*b),
                Literal::Null => Val::Null,
            }),
            Expr::Variable(v) => match row.get(v) {
                Some(x) => Ok(x.clone()),
                None => sem(format!("Variable `{v}` not defined")),
            },
            Expr::Parameter(p) => match self.params.get(p) {
                Some(v) => Ok(Val::from(v)),
                None => sem(format!("Expected parameter(s): {p}")),
            },
            Expr::Property { base, key } => {
                let b = self.eval(base, row)?;
                self.property(&b, key)
            }
            Expr::Index { base, index } => {
                let b = self.eval(base, row)?;
                let i = self.eval(index, row)?;
                self.index(&b, &i)
            }
            Expr::Slice { base, from, to } => {
                let b = self.eval(base, row)?;
                let f = match from {
                    Some(x) => Some(self.eval(x, row)?),
                    None => None,
                };
                let t = match to {
                    Some(x) => Some(self.eval(x, row)?),
                    None => None,
                };
                slice(&b, f, t)
            }
            Expr::Unary { op, expr } => {
                let v = self.eval(expr, row)?;
                match op {
                    UnaryOp::Not => Ok(tri(self.truth(&v)?.map(|b| !b))),
                    UnaryOp::Neg => val::neg(&v),
                    UnaryOp::Pos => Ok(v),
                }
            }
            Expr::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, row),
            Expr::IsNull { expr, negated } => {
                let v = self.eval(expr, row)?;
                Ok(Val::Bool(v.is_null() != *negated))
            }
            Expr::HasLabels { expr, labels } => {
                let v = self.eval(expr, row)?;
                match v {
                    Val::Null => Ok(Val::Null),
                    Val::Node(id) => {
                        let n = self.node(id)?;
                        Ok(Val::Bool(n.is_some_and(|n| {
                            labels.iter().all(|l| n.labels.contains(l))
                        })))
                    }
                    Val::Rel(id) => {
                        let r = self.rel(id)?;
                        Ok(Val::Bool(
                            r.is_some_and(|r| labels.iter().all(|l| *l == r.rel_type)),
                        ))
                    }
                    other => rt(format!("cannot test labels of {}", other.type_name())),
                }
            }
            Expr::FunctionCall {
                namespace,
                name,
                distinct,
                args,
            } => self.call_function(namespace, name, *distinct, args, row),
            Expr::CountStar => match &self.group {
                Some(g) => Ok(Val::Int(g.len() as i64)),
                None => sem("Invalid use of aggregating function count(*)"),
            },
            Expr::Case {
                scrutinee,
                whens,
                else_expr,
            } => {
                let sv = match scrutinee {
                    Some(s) => Some(self.eval(s, row)?),
                    None => None,
                };
                for (w, t) in whens {
                    let wv = self.eval(w, row)?;
                    let hit = match &sv {
                        Some(s) => equals(s, &wv) == Some(true),
                        None => self.truth(&wv)? == Some(true),
                    };
                    if hit {
                        return self.eval(t, row);
                    }
                }
                match else_expr {
                    Some(x) => self.eval(x, row),
                    None => Ok(Val::Null),
                }
            }
            Expr::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    out.push(self.eval(i, row)?);
                }
                Ok(Val::List(out))
            }
            Expr::Map(items) => {
                let mut out = BTreeMap::new();
                for (k, v) in items {
                    out.insert(k.clone(), self.eval(v, row)?);
                }
                Ok(Val::Map(out))
            }
            Expr::ListComprehension {
                variable,
                list,
                predicate,
                projection,
            } => {
                let l = self.eval(list, row)?;
                let items = match l {
                    Val::Null => return Ok(Val::Null),
                    Val::List(items) => items,
                    other => return rt(format!("expected a List but got {}", other.type_name())),
                };
                let mut out = Vec::new();
                let mut local = row.clone();
                for item in items {
                    local.insert(variable.clone(), item.clone());
                    if let Some(p) = predicate {
                        if self.eval_truth(p, &local)? != Some(true) {
                            continue;
                        }
                    }
                    match projection {
                        Some(pr) => out.push(self.eval(pr, &local)?),
                        None => out.push(item),
                    }
                }
                Ok(Val::List(out))
            }
            Expr::PatternComprehension {
                path_variable,
                pattern,
                predicate,
                projection,
            } => {
                let part = PatternPart {
                    variable: path_variable.clone(),
                    kind: PathKind::Plain,
                    element: (**pattern).clone(),
                };
                let matches = self.match_parts(std::slice::from_ref(&part), row, None)?;
                let mut out = Vec::new();
                for (r, _) in matches {
                    if let Some(p) = predicate {
                        if self.eval_truth(p, &r)? != Some(true) {
                            continue;
                        }
                    }
                    out.push(self.eval(projection, &r)?);
                }
                Ok(Val::List(out))
            }
            Expr::PatternPredicate(el) => {
                let part = PatternPart {
                    variable: None,
                    kind: PathKind::Plain,
                    element: (**el).clone(),
                };
                let m = self.match_parts(std::slice::from_ref(&part), row, None)?;
                Ok(Val::Bool(!m.is_empty()))
            }
            Expr::Exists(body) => match &**body {
                ExistsBody::Pattern {
                    pattern,
                    where_clause,
                } => {
                    let matches = self.match_parts(&pattern.parts, row, where_clause.as_deref())?;
                    for (r, _) in matches {
                        match where_clause {
                            Some(w) => {
                                if self.eval_truth(w, &r)? == Some(true) {
                                    return Ok(Val::Bool(true));
                                }
                            }
                            None => return Ok(Val::Bool(true)),
                        }
                    }
                    Ok(Val::Bool(false))
                }
                ExistsBody::Subquery(q) => {
                    let rows = self.run_regular(q, vec![row.clone()])?.rows;
                    Ok(Val::Bool(!rows.is_empty()))
                }
            },
            Expr::ListPredicate {
                kind,
                variable,
                list,
                predicate,
            } => {
                let l = self.eval(list, row)?;
                let items = match l {
                    Val::Null => return Ok(Val::Null),
                    Val::List(items) => items,
                    other => return rt(format!("expected a List but got {}", other.type_name())),
                };
                let (mut t, mut f, mut n) = (0usize, 0usize, 0usize);
                let mut local = row.clone();
                for item in items {
                    local.insert(variable.clone(), item);
                    let r = match predicate {
                        Some(p) => self.eval_truth(p, &local)?,
                        None => Some(true),
                    };
                    match r {
                        Some(true) => t += 1,
                        Some(false) => f += 1,
                        None => n += 1,
                    }
                }
                Ok(tri(match kind {
                    Quantifier::All => {
                        if f > 0 {
                            Some(false)
                        } else if n > 0 {
                            None
                        } else {
                            Some(true)
                        }
                    }
                    Quantifier::Any => {
                        if t > 0 {
                            Some(true)
                        } else if n > 0 {
                            None
                        } else {
                            Some(false)
                        }
                    }
                    Quantifier::None => {
                        if t > 0 {
                            Some(false)
                        } else if n > 0 {
                            None
                        } else {
                            Some(true)
                        }
                    }
                    Quantifier::Single => {
                        if t > 1 {
                            Some(false)
                        } else if t == 1 {
                            if n > 0 {
                                None
                            } else {
                                Some(true)
                            }
                        } else if n > 0 {
                            None
                        } else {
                            Some(false)
                        }
                    }
                }))
            }
            Expr::Reduce {
                accumulator,
                init,
                variable,
                list,
                expr,
            } => {
                let mut acc = self.eval(init, row)?;
                let l = self.eval(list, row)?;
                let items = match l {
                    Val::Null => return Ok(Val::Null),
                    Val::List(items) => items,
                    other => return rt(format!("expected a List but got {}", other.type_name())),
                };
                let mut local = row.clone();
                for item in items {
                    local.insert(accumulator.clone(), acc);
                    local.insert(variable.clone(), item);
                    acc = self.eval(expr, &local)?;
                }
                Ok(acc)
            }
        }
    }

    pub fn property(&mut self, base: &Val, key: &str) -> QResult<Val> {
        match base {
            Val::Null => Ok(Val::Null),
            Val::Node(id) => Ok(self
                .node(*id)?
                .and_then(|n| n.props.get(key).map(Val::from))
                .unwrap_or(Val::Null)),
            Val::Rel(id) => Ok(self
                .rel(*id)?
                .and_then(|n| n.props.get(key).map(Val::from))
                .unwrap_or(Val::Null)),
            Val::Map(m) => Ok(m.get(key).cloned().unwrap_or(Val::Null)),
            other => rt(format!(
                "cannot access property `{key}` on {}",
                other.type_name()
            )),
        }
    }

    fn index(&mut self, base: &Val, idx: &Val) -> QResult<Val> {
        match (base, idx) {
            (Val::Null, _) | (_, Val::Null) => Ok(Val::Null),
            (Val::List(l), Val::Int(i)) => {
                let n = l.len() as i64;
                let i = if *i < 0 { n + i } else { *i };
                Ok(if i >= 0 && i < n {
                    l[i as usize].clone()
                } else {
                    Val::Null
                })
            }
            (Val::Map(_) | Val::Node(_) | Val::Rel(_), Val::Str(k)) => self.property(base, k),
            (b, i) => rt(format!(
                "cannot index {} with {}",
                b.type_name(),
                i.type_name()
            )),
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, row: &Row) -> QResult<Val> {
        match op {
            BinaryOp::And => {
                let a = self.eval_truth(lhs, row)?;
                if a == Some(false) {
                    return Ok(Val::Bool(false));
                }
                let b = self.eval_truth(rhs, row)?;
                return Ok(tri(tri_and(a, b)));
            }
            BinaryOp::Or => {
                let a = self.eval_truth(lhs, row)?;
                if a == Some(true) {
                    return Ok(Val::Bool(true));
                }
                let b = self.eval_truth(rhs, row)?;
                return Ok(tri(tri_or(a, b)));
            }
            BinaryOp::Xor => {
                let a = self.eval_truth(lhs, row)?;
                let b = self.eval_truth(rhs, row)?;
                return Ok(tri(match (a, b) {
                    (Some(x), Some(y)) => Some(x != y),
                    _ => None,
                }));
            }
            _ => {}
        }
        let a = self.eval(lhs, row)?;
        let b = self.eval(rhs, row)?;
        Ok(match op {
            BinaryOp::Eq => tri(equals(&a, &b)),
            BinaryOp::Ne => tri(equals(&a, &b).map(|x| !x)),
            BinaryOp::Lt => tri(compare(&a, &b).map(|o| o == Ordering::Less)),
            BinaryOp::Le => tri(compare(&a, &b).map(|o| o != Ordering::Greater)),
            BinaryOp::Gt => tri(compare(&a, &b).map(|o| o == Ordering::Greater)),
            BinaryOp::Ge => tri(compare(&a, &b).map(|o| o != Ordering::Less)),
            BinaryOp::In => match &b {
                Val::Null => Val::Null,
                Val::List(items) => {
                    if items.is_empty() {
                        Val::Bool(false)
                    } else if a.is_null() {
                        Val::Null
                    } else {
                        let mut saw_null = false;
                        for it in items {
                            match equals(&a, it) {
                                Some(true) => return Ok(Val::Bool(true)),
                                None => saw_null = true,
                                Some(false) => {}
                            }
                        }
                        if saw_null {
                            Val::Null
                        } else {
                            Val::Bool(false)
                        }
                    }
                }
                other => return rt(format!("IN expects a List but got {}", other.type_name())),
            },
            BinaryOp::StartsWith | BinaryOp::EndsWith | BinaryOp::Contains => match (&a, &b) {
                (Val::Str(x), Val::Str(y)) => Val::Bool(match op {
                    BinaryOp::StartsWith => x.starts_with(y.as_str()),
                    BinaryOp::EndsWith => x.ends_with(y.as_str()),
                    _ => x.contains(y.as_str()),
                }),
                _ => Val::Null,
            },
            BinaryOp::RegexMatch => match (&a, &b) {
                (Val::Str(x), Val::Str(p)) => {
                    if !self.regexes.contains_key(p) {
                        let re = regex::Regex::new(&format!("^(?:{p})$"))
                            .map_err(|e| QueryError::Runtime(format!("invalid regex: {e}")))?;
                        self.regexes.insert(p.clone(), re);
                    }
                    Val::Bool(self.regexes[p].is_match(x))
                }
                _ => Val::Null,
            },
            op => val::arith(op, &a, &b)?,
        })
    }
}

fn slice(b: &Val, from: Option<Val>, to: Option<Val>) -> QResult<Val> {
    if b.is_null()
        || from.as_ref().is_some_and(Val::is_null)
        || to.as_ref().is_some_and(Val::is_null)
    {
        return Ok(Val::Null);
    }
    let items = match b {
        Val::List(l) => l,
        other => return rt(format!("cannot slice {}", other.type_name())),
    };
    let n = items.len() as i64;
    let norm = |v: Option<Val>, default: i64| -> QResult<i64> {
        Ok(match v {
            None => default,
            Some(Val::Int(i)) => {
                if i < 0 {
                    (n + i).max(0)
                } else {
                    i.min(n)
                }
            }
            Some(o) => {
                return rt(format!(
                    "slice bound must be Integer, got {}",
                    o.type_name()
                ))
            }
        })
    };
    let f = norm(from, 0)?;
    let t = norm(to, n)?;
    if f >= t {
        return Ok(Val::List(Vec::new()));
    }
    Ok(Val::List(items[f as usize..t as usize].to_vec()))
}
