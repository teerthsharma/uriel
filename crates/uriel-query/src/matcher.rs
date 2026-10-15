//! Pattern matching: node scans, index seeks, relationship expansion, variable-length and
//! shortest paths.

use std::collections::{HashMap, VecDeque};

use uriel_core::{Edge, NodeId, RelId};
use uriel_cypher::{
    BinaryOp, Direction, Expr, NodePattern, PathKind, PatternPart, PatternStep, RelPattern,
};

use crate::analyze::conjuncts;
use crate::ctx::Ctx;
use crate::error::{rt, QResult, QueryError};
use crate::val::{equals, PathVal, Row, Val};

/// One matched row plus the relationships already used (Cypher relationship uniqueness
/// holds across all parts of one `MATCH`).
pub type State = (Row, Vec<RelId>);

struct Partial {
    row: Row,
    used: Vec<RelId>,
    nodes: Vec<NodeId>,
    rels: Vec<RelId>,
    cur: NodeId,
}

/// `Property(Variable(var), key)` -> `key`.
pub fn prop_of(e: &Expr, var: &str) -> Option<String> {
    match e {
        Expr::Property { base, key } => match &**base {
            Expr::Variable(v) if v == var => Some(key.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Equality predicates `(key, other side)` that constrain `np` in the pattern map or the
/// `WHERE` conjuncts; the raw material of index seeks.
pub fn equality_keys<'e>(np: &'e NodePattern, conj: &[&'e Expr]) -> Vec<(String, &'e Expr)> {
    let mut keys: Vec<(String, &Expr)> = Vec::new();
    if let Some(Expr::Map(items)) = &np.properties {
        for (k, e) in items {
            keys.push((k.clone(), e));
        }
    }
    if let Some(var) = &np.variable {
        for c in conj {
            if let Expr::Binary {
                op: BinaryOp::Eq,
                lhs,
                rhs,
            } = c
            {
                if let Some(k) = prop_of(lhs, var) {
                    keys.push((k, rhs));
                } else if let Some(k) = prop_of(rhs, var) {
                    keys.push((k, lhs));
                }
            }
        }
    }
    keys
}

impl Ctx<'_> {
    pub fn match_parts(
        &mut self,
        parts: &[PatternPart],
        row: &Row,
        where_: Option<&Expr>,
    ) -> QResult<Vec<State>> {
        let mut conj: Vec<&Expr> = Vec::new();
        if let Some(w) = where_ {
            conjuncts(w, &mut conj);
        }
        let mut states: Vec<State> = vec![(row.clone(), Vec::new())];
        for part in parts {
            let mut next = Vec::new();
            for (r, u) in states {
                next.extend(self.match_part(part, &r, &u, &conj)?);
            }
            states = next;
        }
        Ok(states)
    }

    fn match_part(
        &mut self,
        part: &PatternPart,
        row: &Row,
        used: &[RelId],
        conj: &[&Expr],
    ) -> QResult<Vec<State>> {
        if part.kind != PathKind::Plain {
            return self.match_shortest(part, row, used, conj);
        }
        let el = &part.element;
        let starts = self.start_candidates(&el.start, row, conj)?;
        let mut partials = Vec::new();
        for s in starts {
            if let Some(r) = self.bind_node(&el.start, s, row)? {
                partials.push(Partial {
                    row: r,
                    used: used.to_vec(),
                    nodes: vec![s],
                    rels: Vec::new(),
                    cur: s,
                });
            }
        }
        for step in &el.steps {
            let mut next = Vec::new();
            for p in &partials {
                self.expand(p, step, &mut next)?;
            }
            partials = next;
        }
        let mut out = Vec::with_capacity(partials.len());
        for p in partials {
            let mut row = p.row;
            if let Some(v) = &part.variable {
                row.insert(
                    v.clone(),
                    Val::Path(PathVal {
                        nodes: p.nodes,
                        rels: p.rels,
                    }),
                );
            }
            out.push((row, p.used));
        }
        Ok(out)
    }

    fn touch_types(&mut self, types: &[String]) {
        if types.is_empty() {
            self.star = true;
        } else {
            for t in types {
                self.tags.insert(t.clone());
            }
        }
    }

    fn start_candidates(
        &mut self,
        np: &NodePattern,
        row: &Row,
        conj: &[&Expr],
    ) -> QResult<Vec<NodeId>> {
        if let Some(v) = &np.variable {
            match row.get(v) {
                Some(Val::Node(id)) => {
                    self.note_node_pattern(np);
                    return Ok(vec![*id]);
                }
                Some(Val::Null) => return Ok(Vec::new()),
                Some(other) => {
                    return rt(format!("`{v}` is a {}, expected a Node", other.type_name()))
                }
                None => {}
            }
        }
        self.note_node_pattern(np);
        if let Some(ids) = self.seek_start(np, row, conj)? {
            return Ok(ids);
        }
        if np.labels.is_empty() {
            return self.all_node_ids();
        }
        // scan the rarest label
        let mut best = &np.labels[0];
        let mut best_n = u64::MAX;
        for l in &np.labels {
            let n = self.read.count_nodes_with_label(l).unwrap_or(u64::MAX);
            if n < best_n {
                best_n = n;
                best = l;
            }
        }
        let best = best.clone();
        self.label_ids(&best)
    }

    fn note_node_pattern(&mut self, np: &NodePattern) {
        if np.labels.is_empty() {
            self.star = true;
        } else {
            for l in &np.labels {
                self.tags.insert(l.clone());
            }
        }
    }

    fn seek_start(
        &mut self,
        np: &NodePattern,
        row: &Row,
        conj: &[&Expr],
    ) -> QResult<Option<Vec<NodeId>>> {
        if self.is_write() || np.labels.is_empty() {
            return Ok(None);
        }
        let keys = equality_keys(np, conj);
        for label in &np.labels {
            for (k, e) in &keys {
                if !self.has_index(label, k) {
                    continue;
                }
                let Ok(v) = self.eval(e, row) else { continue };
                if v.is_null() {
                    continue;
                }
                let Ok(value) = v.to_value() else { continue };
                if let Some(ids) = self.seek(label, k, &value)? {
                    return Ok(Some(ids));
                }
            }
        }
        Ok(None)
    }

    pub fn node_matches(&mut self, np: &NodePattern, id: NodeId, row: &Row) -> QResult<bool> {
        let Some(node) = self.node(id)? else {
            return Ok(false);
        };
        if !np.labels.iter().all(|l| node.labels.contains(l)) {
            return Ok(false);
        }
        if let Some(pe) = &np.properties {
            let pv = self.eval(pe, row)?;
            match pv {
                Val::Map(m) => {
                    for (k, want) in &m {
                        match node.props.get(k) {
                            Some(have) => {
                                if equals(&Val::from(have), want) != Some(true) {
                                    return Ok(false);
                                }
                            }
                            None => return Ok(false),
                        }
                    }
                }
                Val::Null => return Ok(false),
                other => {
                    return rt(format!(
                        "expected a property map, got {}",
                        other.type_name()
                    ))
                }
            }
        }
        Ok(true)
    }

    fn bind_node(&mut self, np: &NodePattern, id: NodeId, row: &Row) -> QResult<Option<Row>> {
        if let Some(v) = &np.variable {
            if let Some(existing) = row.get(v) {
                match existing {
                    Val::Node(e) if *e == id => {}
                    _ => return Ok(None),
                }
            }
        }
        if !self.node_matches(np, id, row)? {
            return Ok(None);
        }
        let mut r = row.clone();
        if let Some(v) = &np.variable {
            r.insert(v.clone(), Val::Node(id));
        }
        Ok(Some(r))
    }

    fn edges_from(&mut self, n: NodeId, dir: Direction, types: &[String]) -> QResult<Vec<Edge>> {
        let tys: Vec<Option<&str>> = if types.is_empty() {
            vec![None]
        } else {
            types.iter().map(|t| Some(t.as_str())).collect()
        };
        let mut out = Vec::new();
        for t in tys {
            match dir {
                Direction::Right => out.extend(self.out_edges(n, t)?),
                Direction::Left => out.extend(self.in_edges(n, t)?),
                Direction::Both => {
                    out.extend(self.out_edges(n, t)?);
                    out.extend(self.in_edges(n, t)?.into_iter().filter(|e| e.other != n));
                }
            }
        }
        Ok(out)
    }

    fn rel_filter(&mut self, rp: &RelPattern, row: &Row) -> QResult<Option<Vec<(String, Val)>>> {
        match &rp.properties {
            None => Ok(None),
            Some(e) => match self.eval(e, row)? {
                Val::Map(m) => Ok(Some(m.into_iter().collect())),
                Val::Null => Ok(Some(vec![("".into(), Val::Null)])),
                o => rt(format!("expected a property map, got {}", o.type_name())),
            },
        }
    }

    fn rel_ok(&mut self, id: RelId, filter: &Option<Vec<(String, Val)>>) -> QResult<bool> {
        let Some(f) = filter else { return Ok(true) };
        if f.is_empty() {
            return Ok(true);
        }
        let Some(rel) = self.rel(id)? else {
            return Ok(false);
        };
        for (k, want) in f {
            match rel.props.get(k) {
                Some(have) if equals(&Val::from(have), want) == Some(true) => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    fn expand(&mut self, p: &Partial, step: &PatternStep, out: &mut Vec<Partial>) -> QResult<()> {
        let rp = &step.rel;
        self.touch_types(&rp.types);
        self.note_node_pattern(&step.node);
        let filter = self.rel_filter(rp, &p.row)?;
        let edges = self.edges_from(p.cur, rp.direction, &rp.types)?;
        let Some(len) = rp.length else {
            for e in edges {
                if p.used.contains(&e.rel) {
                    continue;
                }
                if let Some(rv) = &rp.variable {
                    if let Some(existing) = p.row.get(rv) {
                        if !matches!(existing, Val::Rel(r) if *r == e.rel) {
                            continue;
                        }
                    }
                }
                if !self.rel_ok(e.rel, &filter)? {
                    continue;
                }
                let Some(mut row2) = self.bind_node(&step.node, e.other, &p.row)? else {
                    continue;
                };
                if let Some(rv) = &rp.variable {
                    row2.insert(rv.clone(), Val::Rel(e.rel));
                }
                let mut used = p.used.clone();
                used.push(e.rel);
                let mut nodes = p.nodes.clone();
                nodes.push(e.other);
                let mut rels = p.rels.clone();
                rels.push(e.rel);
                out.push(Partial {
                    row: row2,
                    used,
                    nodes,
                    rels,
                    cur: e.other,
                });
            }
            return Ok(());
        };
        let lo = len.min.unwrap_or(1);
        let hi = len.max;
        // depth-first over simple (relationship-unique) paths
        struct Frame {
            nodes: Vec<NodeId>,
            rels: Vec<RelId>,
            edges: Option<Vec<Edge>>,
        }
        let mut stack = vec![Frame {
            nodes: vec![p.cur],
            rels: Vec::new(),
            edges: Some(edges),
        }];
        while let Some(f) = stack.pop() {
            let depth = f.rels.len() as u64;
            let node = *f.nodes.last().unwrap_or(&p.cur);
            if depth >= lo {
                if let Some(mut row2) = self.bind_node(&step.node, node, &p.row)? {
                    if let Some(rv) = &rp.variable {
                        row2.insert(
                            rv.clone(),
                            Val::List(f.rels.iter().map(|r| Val::Rel(*r)).collect()),
                        );
                    }
                    let mut used = p.used.clone();
                    used.extend(f.rels.iter().copied());
                    let mut nodes = p.nodes.clone();
                    nodes.extend(f.nodes.iter().skip(1).copied());
                    let mut rels = p.rels.clone();
                    rels.extend(f.rels.iter().copied());
                    out.push(Partial {
                        row: row2,
                        used,
                        nodes,
                        rels,
                        cur: node,
                    });
                }
            }
            if hi.is_none_or(|h| depth < h) {
                let edges = match f.edges {
                    Some(e) => e,
                    None => self.edges_from(node, rp.direction, &rp.types)?,
                };
                let mut children = Vec::new();
                for e in edges {
                    if p.used.contains(&e.rel) || f.rels.contains(&e.rel) {
                        continue;
                    }
                    if !self.rel_ok(e.rel, &filter)? {
                        continue;
                    }
                    let mut nodes = f.nodes.clone();
                    nodes.push(e.other);
                    let mut rels = f.rels.clone();
                    rels.push(e.rel);
                    children.push(Frame {
                        nodes,
                        rels,
                        edges: None,
                    });
                }
                while let Some(c) = children.pop() {
                    stack.push(c);
                }
            }
        }
        Ok(())
    }

    fn match_shortest(
        &mut self,
        part: &PatternPart,
        row: &Row,
        used: &[RelId],
        conj: &[&Expr],
    ) -> QResult<Vec<State>> {
        let el = &part.element;
        if el.steps.len() != 1 {
            return Err(QueryError::Unsupported(
                "shortestPath requires a single relationship pattern".into(),
            ));
        }
        let step = &el.steps[0];
        let rp = &step.rel;
        let (lo, hi) = match rp.length {
            Some(l) => (l.min.unwrap_or(1), l.max),
            None => (1, Some(1)),
        };
        self.touch_types(&rp.types);
        let filter = self.rel_filter(rp, row)?;
        let starts = self.start_candidates(&el.start, row, conj)?;
        let ends = self.start_candidates(&step.node, row, conj)?;
        let all = part.kind == PathKind::AllShortest;
        let mut out: Vec<State> = Vec::new();
        for s in starts {
            let Some(row_s) = self.bind_node(&el.start, s, row)? else {
                continue;
            };
            // breadth-first search with predecessor lists
            let mut dist: HashMap<NodeId, u64> = HashMap::new();
            let mut preds: HashMap<NodeId, Vec<(NodeId, RelId)>> = HashMap::new();
            dist.insert(s, 0);
            let mut queue = VecDeque::from([s]);
            while let Some(n) = queue.pop_front() {
                let d = dist[&n];
                if hi.is_some_and(|h| d >= h) {
                    continue;
                }
                for e in self.edges_from(n, rp.direction, &rp.types)? {
                    if used.contains(&e.rel) || !self.rel_ok(e.rel, &filter)? {
                        continue;
                    }
                    match dist.get(&e.other) {
                        None => {
                            dist.insert(e.other, d + 1);
                            preds.insert(e.other, vec![(n, e.rel)]);
                            queue.push_back(e.other);
                        }
                        Some(&od) if od == d + 1 => {
                            if let Some(p) = preds.get_mut(&e.other) {
                                p.push((n, e.rel));
                            }
                        }
                        _ => {}
                    }
                }
            }
            for &t in &ends {
                let Some(row_t) = self.bind_node(&step.node, t, &row_s)? else {
                    continue;
                };
                let Some(&d) = dist.get(&t) else { continue };
                if d < lo || (s == t && lo > 0) {
                    continue;
                }
                let mut paths: Vec<Vec<(NodeId, RelId)>> = Vec::new();
                if d > 0 {
                    let mut cur = Vec::new();
                    collect_paths(&preds, t, s, &mut cur, &mut paths, all);
                } else {
                    paths.push(Vec::new());
                }
                for rev in paths {
                    // `rev` lists (node, rel) from the target back towards the source
                    let mut nodes = vec![s];
                    let rels: Vec<RelId> = rev.iter().rev().map(|(_, r)| *r).collect();
                    // nodes along the path: reconstruct from relationship endpoints
                    let mut at = s;
                    for r in &rels {
                        let rel = self.rel(*r)?;
                        let Some(rel) = rel else { continue };
                        at = if rel.src == at { rel.dst } else { rel.src };
                        nodes.push(at);
                    }
                    let mut r3 = row_t.clone();
                    if let Some(rv) = &rp.variable {
                        r3.insert(
                            rv.clone(),
                            if rp.length.is_none() {
                                rels.first().map_or(Val::Null, |r| Val::Rel(*r))
                            } else {
                                Val::List(rels.iter().map(|r| Val::Rel(*r)).collect())
                            },
                        );
                    }
                    if let Some(pv) = &part.variable {
                        r3.insert(
                            pv.clone(),
                            Val::Path(PathVal {
                                nodes,
                                rels: rels.clone(),
                            }),
                        );
                    }
                    let mut u = used.to_vec();
                    u.extend(rels);
                    out.push((r3, u));
                }
            }
        }
        Ok(out)
    }
}

/// Enumerates predecessor chains from `node` back to `src`; each chain is target-first.
fn collect_paths(
    preds: &HashMap<NodeId, Vec<(NodeId, RelId)>>,
    node: NodeId,
    src: NodeId,
    cur: &mut Vec<(NodeId, RelId)>,
    out: &mut Vec<Vec<(NodeId, RelId)>>,
    all: bool,
) {
    if node == src {
        out.push(cur.clone());
        return;
    }
    let Some(ps) = preds.get(&node) else { return };
    for &(prev, rel) in ps {
        cur.push((prev, rel));
        collect_paths(preds, prev, src, cur, out, all);
        cur.pop();
        if !all && !out.is_empty() {
            return;
        }
    }
}
