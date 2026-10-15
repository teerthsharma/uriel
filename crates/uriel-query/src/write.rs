//! Write clauses: CREATE, MERGE, SET, REMOVE, DELETE.

use std::collections::{BTreeMap, BTreeSet};

use uriel_core::{NodeId, Props, RelId, Value};
use uriel_cypher::{
    DeleteClause, Direction, MergeClause, MergeEvent, NodePattern, Pattern, PatternPart,
    RemoveClause, RemoveItem, SetClause, SetItem,
};

use crate::ctx::Ctx;
use crate::error::{rt, QResult, QueryError};
use crate::val::{PathVal, Row, Val};

fn props_from(m: &BTreeMap<String, Val>) -> QResult<Props> {
    let mut p = Props::new();
    for (k, v) in m {
        if !v.is_null() {
            p.insert(k.clone(), v.to_value()?);
        }
    }
    Ok(p)
}

impl Ctx<'_> {
    fn eval_prop_map(&mut self, e: &Option<uriel_cypher::Expr>, row: &Row) -> QResult<Props> {
        match e {
            None => Ok(Props::new()),
            Some(e) => match self.eval(e, row)? {
                Val::Map(m) => props_from(&m),
                Val::Null => Ok(Props::new()),
                o => rt(format!("expected a property map, got {}", o.type_name())),
            },
        }
    }

    fn create_node_from(&mut self, np: &NodePattern, row: &mut Row) -> QResult<NodeId> {
        if let Some(v) = &np.variable {
            match row.get(v) {
                Some(Val::Node(id)) => return Ok(*id),
                Some(Val::Null) => {
                    return rt(format!("cannot create a pattern with `{v}` bound to null"))
                }
                Some(o) => return rt(format!("`{v}` is a {}, expected a Node", o.type_name())),
                None => {}
            }
        }
        let props = self.eval_prop_map(&np.properties, row)?;
        let nprops = props.len() as u64;
        let labels: Vec<&str> = np.labels.iter().map(String::as_str).collect();
        let id = self.tx_mut()?.create_node(&labels, props)?;
        self.created.push(id);
        self.stats.nodes_created += 1;
        self.stats.labels_added += np.labels.len() as u64;
        self.stats.properties_set += nprops;
        if let Some(v) = &np.variable {
            row.insert(v.clone(), Val::Node(id));
        }
        self.invalidate();
        Ok(id)
    }

    pub fn create_part(&mut self, part: &PatternPart, row: &Row) -> QResult<Row> {
        let mut row = row.clone();
        let el = &part.element;
        let mut cur = self.create_node_from(&el.start, &mut row)?;
        let mut nodes = vec![cur];
        let mut rels: Vec<RelId> = Vec::new();
        for step in &el.steps {
            let next = self.create_node_from(&step.node, &mut row)?;
            let rp = &step.rel;
            if rp.types.len() != 1 {
                return rt("CREATE requires exactly one relationship type");
            }
            if rp.length.is_some() {
                return rt("variable length relationships cannot be used in CREATE");
            }
            let (src, dst) = match rp.direction {
                Direction::Right => (cur, next),
                Direction::Left => (next, cur),
                Direction::Both => {
                    return rt("only directed relationships are supported in CREATE")
                }
            };
            let props = self.eval_prop_map(&rp.properties, &row)?;
            let n = props.len() as u64;
            let id = self.tx_mut()?.create_rel(src, dst, &rp.types[0], props)?;
            self.stats.rels_created += 1;
            self.stats.properties_set += n;
            if let Some(v) = &rp.variable {
                row.insert(v.clone(), Val::Rel(id));
            }
            rels.push(id);
            nodes.push(next);
            cur = next;
        }
        if let Some(v) = &part.variable {
            row.insert(v.clone(), Val::Path(PathVal { nodes, rels }));
        }
        self.invalidate();
        Ok(row)
    }

    pub fn create_pattern(&mut self, pat: &Pattern, row: &Row) -> QResult<Row> {
        let mut row = row.clone();
        for part in &pat.parts {
            row = self.create_part(part, &row)?;
        }
        Ok(row)
    }

    pub fn exec_merge(&mut self, m: &MergeClause, rows: Vec<Row>) -> QResult<Vec<Row>> {
        let mut out = Vec::new();
        let parts = std::slice::from_ref(&m.pattern);
        for row in rows {
            let matches = self.match_parts(parts, &row, None)?;
            if matches.is_empty() {
                // a null property can never be matched, so refuse to create it
                self.check_merge_nulls(&m.pattern, &row)?;
                let r = self.create_part(&m.pattern, &row)?;
                let r = self.apply_actions(m, MergeEvent::Create, r)?;
                out.push(r);
            } else {
                for (r, _) in matches {
                    let r = self.apply_actions(m, MergeEvent::Match, r)?;
                    out.push(r);
                }
            }
        }
        Ok(out)
    }

    fn check_merge_nulls(&mut self, part: &PatternPart, row: &Row) -> QResult<()> {
        let mut exprs = Vec::new();
        if let Some(p) = &part.element.start.properties {
            exprs.push(p);
        }
        for s in &part.element.steps {
            if let Some(p) = &s.node.properties {
                exprs.push(p);
            }
            if let Some(p) = &s.rel.properties {
                exprs.push(p);
            }
        }
        for e in exprs {
            if let Val::Map(m) = self.eval(e, row)? {
                if let Some((k, _)) = m.iter().find(|(_, v)| v.is_null()) {
                    return rt(format!("cannot merge using null property value for `{k}`"));
                }
            }
        }
        Ok(())
    }

    fn apply_actions(&mut self, m: &MergeClause, ev: MergeEvent, row: Row) -> QResult<Row> {
        for a in &m.actions {
            if a.event == ev {
                self.set_items(&a.items, &row)?;
            }
        }
        Ok(row)
    }

    pub fn exec_set(&mut self, c: &SetClause, rows: &[Row]) -> QResult<()> {
        for row in rows {
            self.set_items(&c.items, row)?;
        }
        Ok(())
    }

    fn entity_props(&mut self, v: &Val) -> QResult<BTreeMap<String, Val>> {
        Ok(match v {
            Val::Map(m) => m.clone(),
            Val::Node(id) => self
                .node(*id)?
                .map(|n| {
                    n.props
                        .iter()
                        .map(|(k, v)| (k.clone(), Val::from(v)))
                        .collect()
                })
                .unwrap_or_default(),
            Val::Rel(id) => self
                .rel(*id)?
                .map(|n| {
                    n.props
                        .iter()
                        .map(|(k, v)| (k.clone(), Val::from(v)))
                        .collect()
                })
                .unwrap_or_default(),
            o => {
                return rt(format!(
                    "SET expects a map or entity, got {}",
                    o.type_name()
                ))
            }
        })
    }

    fn write_props(&mut self, target: &Val, props: Props) -> QResult<()> {
        match target {
            Val::Null => {}
            Val::Node(id) => {
                if self.node(*id)?.is_some() {
                    self.tx_mut()?.set_node_props(*id, props)?;
                }
            }
            Val::Rel(id) => {
                if self.rel(*id)?.is_some() {
                    self.tx_mut()?.set_rel_props(*id, props)?;
                }
            }
            o => return rt(format!("cannot set properties on {}", o.type_name())),
        }
        self.invalidate();
        Ok(())
    }

    pub fn set_items(&mut self, items: &[SetItem], row: &Row) -> QResult<()> {
        for item in items {
            match item {
                SetItem::Property { target, value } => {
                    let uriel_cypher::Expr::Property { base, key } = target else {
                        return Err(QueryError::Unsupported(
                            "SET target must be a property access".into(),
                        ));
                    };
                    let b = self.eval(base, row)?;
                    let v = self.eval(value, row)?.to_value()?;
                    let mut p = Props::new();
                    p.insert(key.clone(), v);
                    self.write_props(&b, p)?;
                    self.stats.properties_set += 1;
                }
                SetItem::Replace { variable, value } | SetItem::Merge { variable, value } => {
                    let replace = matches!(item, SetItem::Replace { .. });
                    let Some(target) = row.get(variable).cloned() else {
                        return crate::error::sem(format!("Variable `{variable}` not defined"));
                    };
                    let v = self.eval(value, row)?;
                    let new = self.entity_props(&v)?;
                    let mut props = props_from(&new)?;
                    let n = props.len() as u64;
                    if replace {
                        for k in self.entity_props(&target)?.keys() {
                            if !props.contains_key(k) {
                                props.insert(k.clone(), Value::Null);
                            }
                        }
                    } else {
                        for (k, v) in &new {
                            if v.is_null() {
                                props.insert(k.clone(), Value::Null);
                            }
                        }
                    }
                    self.write_props(&target, props)?;
                    self.stats.properties_set += n;
                }
                SetItem::Labels { variable, labels } => {
                    let Some(target) = row.get(variable).cloned() else {
                        return crate::error::sem(format!("Variable `{variable}` not defined"));
                    };
                    match target {
                        Val::Null => {}
                        Val::Node(id) => {
                            for l in labels {
                                let has = self.node(id)?.is_some_and(|n| n.labels.contains(l));
                                if !has {
                                    self.tx_mut()?.add_label(id, l)?;
                                    self.stats.labels_added += 1;
                                    self.invalidate();
                                }
                            }
                        }
                        o => return rt(format!("cannot set labels on {}", o.type_name())),
                    }
                }
            }
        }
        Ok(())
    }

    pub fn exec_remove(&mut self, c: &RemoveClause, rows: &[Row]) -> QResult<()> {
        for row in rows {
            for item in &c.items {
                match item {
                    RemoveItem::Labels { variable, labels } => {
                        let Some(target) = row.get(variable).cloned() else {
                            return crate::error::sem(format!("Variable `{variable}` not defined"));
                        };
                        if let Val::Node(id) = target {
                            for l in labels {
                                let has = self.node(id)?.is_some_and(|n| n.labels.contains(l));
                                if has {
                                    self.tx_mut()?.remove_label(id, l)?;
                                    self.stats.labels_removed += 1;
                                    self.invalidate();
                                }
                            }
                        }
                    }
                    RemoveItem::Property(e) => {
                        let uriel_cypher::Expr::Property { base, key } = e else {
                            return Err(QueryError::Unsupported(
                                "REMOVE target must be a property access".into(),
                            ));
                        };
                        let b = self.eval(base, row)?;
                        let mut p = Props::new();
                        p.insert(key.clone(), Value::Null);
                        self.write_props(&b, p)?;
                        self.stats.properties_set += 1;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn exec_delete(&mut self, c: &DeleteClause, rows: &[Row]) -> QResult<()> {
        let mut nodes: BTreeSet<NodeId> = BTreeSet::new();
        let mut rels: BTreeSet<RelId> = BTreeSet::new();
        for row in rows {
            for e in &c.exprs {
                let v = self.eval(e, row)?;
                collect_entities(&v, &mut nodes, &mut rels)?;
            }
        }
        for r in rels {
            if self.rel(r)?.is_some() {
                self.tx_mut()?.delete_rel(r)?;
                self.stats.rels_deleted += 1;
                self.invalidate();
            }
        }
        for n in nodes {
            if self.node(n)?.is_none() {
                continue;
            }
            if c.detach {
                let mut ids: BTreeSet<RelId> = BTreeSet::new();
                for e in self.out_edges(n, None)? {
                    ids.insert(e.rel);
                }
                for e in self.in_edges(n, None)? {
                    ids.insert(e.rel);
                }
                self.stats.rels_deleted += ids.len() as u64;
                self.tx_mut()?.detach_delete_node(n)?;
            } else {
                self.tx_mut()?.delete_node(n)?;
            }
            self.stats.nodes_deleted += 1;
            self.invalidate();
        }
        Ok(())
    }
}

fn collect_entities(
    v: &Val,
    nodes: &mut BTreeSet<NodeId>,
    rels: &mut BTreeSet<RelId>,
) -> QResult<()> {
    match v {
        Val::Null => {}
        Val::Node(n) => {
            nodes.insert(*n);
        }
        Val::Rel(r) => {
            rels.insert(*r);
        }
        Val::Path(p) => {
            nodes.extend(p.nodes.iter().copied());
            rels.extend(p.rels.iter().copied());
        }
        Val::List(l) => {
            for x in l {
                collect_entities(x, nodes, rels)?;
            }
        }
        o => return rt(format!("cannot delete a {}", o.type_name())),
    }
    Ok(())
}
