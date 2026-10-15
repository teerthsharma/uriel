//! The write transaction.
//!
//! All mutation funnels through [`WriteTx::apply_node_change`] (nodes) and the
//! relationship helpers, which keep the label index, property indexes, counters and the
//! touched set consistent by diffing the old and new state. Every public operation checks
//! its preconditions *before* the first write, so a rejected operation leaves the
//! transaction exactly as it was and the caller may carry on or drop it.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::sync::Arc;

use redb::ReadableTable;

use crate::catalog::ConstraintKind;
use crate::checksum::crc32c;
use crate::codec::{encode_node, encode_rel};
use crate::error::{Error, Result};
use crate::graph::{CommitInfo, Inner, Touched, WriterGate};
use crate::ids::{Edge, Node, NodeId, Props, Rel, RelId};
use crate::keys::{
    adj_key, adj_prefix, as_slices, check_name, constraint_key, index_key, label_count_key,
    name_id_key, name_prefix, parse_constraint_key, parse_index_key, prefix_bounds, prop_key,
    prop_prefix, tail_u64, type_count_key,
};
use crate::read::{collect_edges, collect_label_ids, load_node_sum, load_rel_sum, meta_value};
use crate::tables::{
    meta, CONSTRAINTS, COUNTS, INDEXES, IN_ADJ, LABEL_IDX, META, NODES, NODE_SUMS, OUT_ADJ,
    PROP_IDX, RELS, REL_SUMS, TYPE_IDX,
};
use crate::tier::CacheKey;
use crate::value::Value;

/// Deepest list/map nesting accepted in a stored property value.
pub const MAX_VALUE_DEPTH: usize = 64;

/// Nodes processed per step when scanning or backfilling a label.
const CHUNK: usize = 4096;

type BoundsPair = (Bound<Vec<u8>>, Bound<Vec<u8>>);

#[derive(Default)]
struct State {
    next_node: u64,
    next_rel: u64,
    node_count: u64,
    rel_count: u64,
    epoch: u64,
    /// label -> key -> backs a unique constraint
    indexes: BTreeMap<String, BTreeMap<String, bool>>,
    unique: BTreeMap<String, BTreeSet<String>>,
    exists: BTreeMap<String, BTreeSet<String>>,
    /// pending changes to per-label / per-type counters, flushed at commit
    deltas: BTreeMap<Vec<u8>, i64>,
}

fn violation(kind: ConstraintKind, label: &str, key: &str, detail: String) -> Error {
    Error::ConstraintViolation {
        kind,
        label: label.to_string(),
        key: key.to_string(),
        detail,
    }
}

fn normalize_labels(labels: &[&str]) -> Result<BTreeSet<String>> {
    labels
        .iter()
        .map(|l| check_name(l).map(|()| (*l).to_string()))
        .collect()
}

fn check_prop(key: &str, value: &Value) -> Result<()> {
    check_name(key)?;
    if value.depth() > MAX_VALUE_DEPTH {
        return Err(Error::InvalidValue(format!(
            "property `{key}` is nested more than {MAX_VALUE_DEPTH} levels deep"
        )));
    }
    Ok(())
}

/// Validates `update` and merges it into `base`; `Null` removes a key.
fn merge_props(base: &mut Props, update: Props) -> Result<()> {
    for (k, v) in &update {
        check_prop(k, v)?;
    }
    for (k, v) in update {
        if v.is_null() {
            base.remove(&k);
        } else {
            base.insert(k, v);
        }
    }
    Ok(())
}

/// An open write transaction; see [`Graph::begin_write`](crate::Graph::begin_write).
///
/// Nothing is visible to readers until [`WriteTx::commit`]. Dropping the transaction (or
/// calling [`WriteTx::rollback`]) discards every change, including allocated ids.
pub struct WriteTx {
    // field order matters: the redb transaction must end before the writer gate opens
    txn: redb::WriteTransaction,
    gate: WriterGate,
    inner: Arc<Inner>,
    st: State,
    touched: Touched,
    touched_keys: BTreeSet<CacheKey>,
    touched_edges: Vec<(u64, u64)>,
    dirty: bool,
    poisoned: bool,
}

impl WriteTx {
    pub(crate) fn begin(inner: Arc<Inner>, gate: WriterGate) -> Result<WriteTx> {
        let txn = inner.database()?.begin_write()?;
        let mut st = State::default();
        {
            let m = txn.open_table(META)?;
            st.next_node = meta_value(&m, meta::NEXT_NODE)?;
            st.next_rel = meta_value(&m, meta::NEXT_REL)?;
            st.node_count = meta_value(&m, meta::NODE_COUNT)?;
            st.rel_count = meta_value(&m, meta::REL_COUNT)?;
            st.epoch = meta_value(&m, meta::EPOCH)?;
        }
        {
            let t = txn.open_table(INDEXES)?;
            for e in t.iter()? {
                let (k, v) = e?;
                let (label, key) = parse_index_key(k.value())?;
                st.indexes
                    .entry(label.to_string())
                    .or_default()
                    .insert(key.to_string(), v.value() == 1);
            }
        }
        {
            let t = txn.open_table(CONSTRAINTS)?;
            for e in t.iter()? {
                let (k, _) = e?;
                let (kind, label, key) = parse_constraint_key(k.value())?;
                let map = match kind {
                    ConstraintKind::Unique => &mut st.unique,
                    ConstraintKind::Exists => &mut st.exists,
                };
                map.entry(label.to_string())
                    .or_default()
                    .insert(key.to_string());
            }
        }
        Ok(WriteTx {
            txn,
            gate,
            inner,
            st,
            touched: Touched::default(),
            touched_keys: BTreeSet::new(),
            touched_edges: Vec::new(),
            dirty: false,
            poisoned: false,
        })
    }

    /// Makes every change durable and visible, advances the epoch by one, and notifies
    /// commit subscribers. Returns the new epoch.
    ///
    /// A transaction that changed nothing commits as a no-op: the epoch is unchanged,
    /// subscribers are not called, and the current epoch is returned.
    pub fn commit(self) -> Result<u64> {
        let WriteTx {
            txn,
            gate,
            inner,
            st,
            touched,
            touched_keys,
            touched_edges,
            dirty,
            poisoned,
        } = self;
        if poisoned {
            return Err(Error::TransactionPoisoned);
        }
        if !dirty {
            txn.abort()?;
            return Ok(st.epoch);
        }
        if inner.mode() != crate::graph::Mode::Full {
            return Err(Error::ReadOnly);
        }
        let epoch = st.epoch + 1;
        {
            let mut m = txn.open_table(META)?;
            m.insert(meta::NEXT_NODE, st.next_node)?;
            m.insert(meta::NEXT_REL, st.next_rel)?;
            m.insert(meta::NODE_COUNT, st.node_count)?;
            m.insert(meta::REL_COUNT, st.rel_count)?;
            m.insert(meta::EPOCH, epoch)?;
        }
        {
            let mut counts = txn.open_table(COUNTS)?;
            for (key, delta) in &st.deltas {
                if *delta == 0 {
                    continue;
                }
                let current = counts.get(key.as_slice())?.map(|g| g.value()).unwrap_or(0);
                let next = i64::try_from(current).unwrap_or(i64::MAX) + delta;
                if next <= 0 {
                    counts.remove(key.as_slice())?;
                } else {
                    counts.insert(key.as_slice(), next as u64)?;
                }
            }
        }
        inner.tier.begin_commit(&touched_keys);
        if let Err(e) = txn.commit() {
            inner.tier.abort_commit();
            let e = Error::from(e);
            if e.is_io() {
                inner.write_failed();
            }
            return Err(e);
        }
        inner.tier.finish_commit(&touched_keys, &touched_edges, epoch);
        let info = CommitInfo {
            epoch,
            labels: touched.labels,
            rel_types: touched.rel_types,
        };
        // still holding the writer gate: subscribers see commits in epoch order
        inner.publish(&info);
        drop(gate);
        Ok(epoch)
    }

    /// Discards every change. Equivalent to dropping the transaction.
    pub fn rollback(self) {
        drop(self);
    }

    /// Labels and relationship types modified so far in this transaction.
    pub fn touched(&self) -> &Touched {
        &self.touched
    }

    fn guarded<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.poisoned {
            return Err(Error::TransactionPoisoned);
        }
        let result = f(self);
        // a validation error is raised before any write and leaves the tx intact; a
        // storage failure may have stopped half way, so no commit is allowed afterwards
        if matches!(
            result,
            Err(Error::Storage(_) | Error::Corrupt(_) | Error::CorruptRecord { .. })
        ) {
            self.poisoned = true;
        }
        if let Err(e) = &result {
            if e.is_io() {
                self.inner.write_failed();
            }
        }
        result
    }

    // ---------------------------------------------------------------- reads (own writes)

    /// Reads a node, including this transaction's uncommitted changes.
    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>> {
        load_node_sum(&self.txn.open_table(NODES)?, &self.txn.open_table(NODE_SUMS)?, id)
    }

    /// Reads a relationship, including this transaction's uncommitted changes.
    pub fn get_rel(&self, id: RelId) -> Result<Option<Rel>> {
        load_rel_sum(&self.txn.open_table(RELS)?, &self.txn.open_table(REL_SUMS)?, id)
    }

    /// Outgoing adjacency of `node`, including uncommitted changes.
    pub fn out_edges(&self, node: NodeId, rel_type: Option<&str>) -> Result<Vec<Edge>> {
        collect_edges(&self.txn.open_table(OUT_ADJ)?, node, rel_type)
    }

    /// Incoming adjacency of `node`, including uncommitted changes.
    pub fn in_edges(&self, node: NodeId, rel_type: Option<&str>) -> Result<Vec<Edge>> {
        collect_edges(&self.txn.open_table(IN_ADJ)?, node, rel_type)
    }

    /// Ids of nodes carrying `label`, including uncommitted changes.
    pub fn node_ids_by_label(&self, label: &str) -> Result<Vec<NodeId>> {
        collect_label_ids(&self.txn.open_table(LABEL_IDX)?, label)
    }

    fn must_node(&self, id: NodeId) -> Result<Node> {
        self.get_node(id)?.ok_or(Error::NodeNotFound(id))
    }

    fn must_rel(&self, id: RelId) -> Result<Rel> {
        self.get_rel(id)?.ok_or(Error::RelNotFound(id))
    }

    fn bump(&mut self, key: Vec<u8>, by: i64) {
        *self.st.deltas.entry(key).or_insert(0) += by;
    }

    // ------------------------------------------------------------------ constraint checks

    /// Newest node other than `exclude` holding `value` under `(label, key)`, via the index.
    fn find_in_index(
        &self,
        label: &str,
        key: &str,
        value: &Value,
        exclude: Option<NodeId>,
    ) -> Result<Option<NodeId>> {
        let t = self.txn.open_table(PROP_IDX)?;
        let mut prefix = prop_prefix(label, key);
        value.write_key(&mut prefix);
        let bounds = prefix_bounds(&prefix);
        for e in t.range::<&[u8]>(as_slices(&bounds))? {
            let (k, _) = e?;
            let id = NodeId(tail_u64(k.value())?);
            if Some(id) != exclude {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Checks `new` against the existence and uniqueness constraints of its labels.
    /// `old` is the node's previous state, used to skip values that did not change.
    fn check_constraints(&self, new: &Node, old: Option<&Node>) -> Result<()> {
        for label in &new.labels {
            if let Some(keys) = self.st.exists.get(label) {
                for key in keys {
                    if !new.props.contains_key(key) {
                        let who = old.map_or_else(
                            || "the new node".to_string(),
                            |o| format!("node {}", o.id),
                        );
                        return Err(violation(
                            ConstraintKind::Exists,
                            label,
                            key,
                            format!("{who} has no `{key}` property"),
                        ));
                    }
                }
            }
            let Some(keys) = self.st.unique.get(label) else {
                continue;
            };
            for key in keys {
                let Some(value) = new.props.get(key) else {
                    continue;
                };
                let unchanged = old
                    .is_some_and(|o| o.labels.contains(label) && o.props.get(key) == Some(value));
                if unchanged {
                    continue;
                }
                if let Some(other) = self.find_in_index(label, key, value, old.map(|o| o.id))? {
                    return Err(violation(
                        ConstraintKind::Unique,
                        label,
                        key,
                        format!("value {value} is already held by node {other}"),
                    ));
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ node mutation core

    fn index_entries(&self, node: Option<&Node>) -> BTreeSet<Vec<u8>> {
        let mut out = BTreeSet::new();
        if let Some(n) = node {
            for label in &n.labels {
                for key in self
                    .st
                    .indexes
                    .get(label)
                    .into_iter()
                    .flat_map(BTreeMap::keys)
                {
                    if let Some(v) = n.props.get(key) {
                        out.insert(prop_key(label, key, v, n.id));
                    }
                }
            }
        }
        out
    }

    /// Moves a node from state `old` to state `new` (`None` = does not exist), updating the
    /// node record, label index, property indexes, counters and touched set.
    fn apply_node_change(&mut self, old: Option<&Node>, new: Option<&Node>) -> Result<()> {
        let empty = BTreeSet::new();
        let old_labels = old.map_or(&empty, |n| &n.labels);
        let new_labels = new.map_or(&empty, |n| &n.labels);
        let id = old
            .or(new)
            .expect("a node change has an old or new state")
            .id;

        {
            let mut t = self.txn.open_table(LABEL_IDX)?;
            for l in new_labels.difference(old_labels) {
                t.insert(name_id_key(l, id.0).as_slice(), ())?;
                *self.st.deltas.entry(label_count_key(l)).or_insert(0) += 1;
            }
            for l in old_labels.difference(new_labels) {
                t.remove(name_id_key(l, id.0).as_slice())?;
                *self.st.deltas.entry(label_count_key(l)).or_insert(0) -= 1;
            }
        }
        let old_keys = self.index_entries(old);
        let new_keys = self.index_entries(new);
        if !old_keys.is_empty() || !new_keys.is_empty() {
            let mut t = self.txn.open_table(PROP_IDX)?;
            for k in old_keys.difference(&new_keys) {
                t.remove(k.as_slice())?;
            }
            for k in new_keys.difference(&old_keys) {
                t.insert(k.as_slice(), ())?;
            }
        }
        {
            let mut t = self.txn.open_table(NODES)?;
            let mut sums = self.txn.open_table(NODE_SUMS)?;
            match new {
                Some(n) => {
                    let rec = self.inner.tier.store(&encode_node(&n.labels, &n.props));
                    t.insert(id.0, rec.as_slice())?;
                    sums.insert(id.0, crc32c(&rec))?;
                }
                None => {
                    t.remove(id.0)?;
                    sums.remove(id.0)?;
                }
            }
        }
        match (old.is_some(), new.is_some()) {
            (false, true) => self.st.node_count += 1,
            (true, false) => self.st.node_count -= 1,
            _ => {}
        }
        self.touched
            .labels
            .extend(old_labels.iter().chain(new_labels).cloned());
        self.touched_keys.insert(CacheKey::Node(id.0));
        self.dirty = true;
        Ok(())
    }

    /// Validates and applies `old -> new`, skipping byte-identical states.
    fn replace_node(&mut self, old: Node, new: Node) -> Result<()> {
        if encode_node(&old.labels, &old.props) == encode_node(&new.labels, &new.props) {
            return Ok(());
        }
        self.check_constraints(&new, Some(&old))?;
        self.apply_node_change(Some(&old), Some(&new))
    }

    // ------------------------------------------------------------------------ public nodes

    /// Creates a node with the given labels and properties and returns its id.
    ///
    /// `Null` property values are dropped. Fails with [`Error::InvalidName`] for empty or
    /// NUL-containing labels/keys, [`Error::InvalidValue`] for over-deep values, and
    /// [`Error::ConstraintViolation`] if a constraint on any of the labels would break. A
    /// failed call allocates no id.
    pub fn create_node(&mut self, labels: &[&str], props: Props) -> Result<NodeId> {
        self.guarded(|tx| {
            let labels = normalize_labels(labels)?;
            let mut clean = Props::new();
            merge_props(&mut clean, props)?;
            let mut node = Node {
                id: NodeId(tx.st.next_node),
                labels,
                props: clean,
            };
            tx.check_constraints(&node, None)?;
            tx.st.next_node += 1;
            node.id = NodeId(tx.st.next_node - 1);
            tx.apply_node_change(None, Some(&node))?;
            Ok(node.id)
        })
    }

    /// Merges `props` into the node's properties: listed keys are set, `Null` removes a key,
    /// other keys are kept.
    pub fn set_node_props(&mut self, id: NodeId, props: Props) -> Result<()> {
        self.guarded(|tx| {
            let old = tx.must_node(id)?;
            let mut new = old.clone();
            merge_props(&mut new.props, props)?;
            tx.replace_node(old, new)
        })
    }

    /// Adds a label (no-op if present). Constraints on the label apply to the node from now on.
    pub fn add_label(&mut self, id: NodeId, label: &str) -> Result<()> {
        self.guarded(|tx| {
            check_name(label)?;
            let old = tx.must_node(id)?;
            let mut new = old.clone();
            new.labels.insert(label.to_string());
            tx.replace_node(old, new)
        })
    }

    /// Removes a label (no-op if absent).
    pub fn remove_label(&mut self, id: NodeId, label: &str) -> Result<()> {
        self.guarded(|tx| {
            check_name(label)?;
            let old = tx.must_node(id)?;
            let mut new = old.clone();
            new.labels.remove(label);
            tx.replace_node(old, new)
        })
    }

    fn has_adjacent(&self, node: NodeId) -> Result<bool> {
        for table in [OUT_ADJ, IN_ADJ] {
            let t = self.txn.open_table(table)?;
            let bounds = prefix_bounds(&adj_prefix(node, None));
            if t.range::<&[u8]>(as_slices(&bounds))?.next().is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Deletes a node. Fails with [`Error::NodeHasRelationships`] while it still has any
    /// incoming or outgoing relationship.
    pub fn delete_node(&mut self, id: NodeId) -> Result<()> {
        self.guarded(|tx| {
            let node = tx.must_node(id)?;
            if tx.has_adjacent(id)? {
                return Err(Error::NodeHasRelationships(id));
            }
            tx.apply_node_change(Some(&node), None)
        })
    }

    /// Deletes a node together with all its incoming and outgoing relationships.
    pub fn detach_delete_node(&mut self, id: NodeId) -> Result<()> {
        self.guarded(|tx| {
            let node = tx.must_node(id)?;
            let mut rels: BTreeSet<RelId> = BTreeSet::new();
            rels.extend(tx.out_edges(id, None)?.into_iter().map(|e| e.rel));
            rels.extend(tx.in_edges(id, None)?.into_iter().map(|e| e.rel)); // self-loops dedup
            for rel in rels {
                tx.delete_rel_inner(rel)?;
            }
            tx.apply_node_change(Some(&node), None)
        })
    }

    // ------------------------------------------------------------------ public relationships

    /// Creates a relationship `src -[rel_type]-> dst` and returns its id.
    ///
    /// Both endpoints must exist ([`Error::NodeNotFound`] otherwise). A failed call
    /// allocates no id.
    pub fn create_rel(
        &mut self,
        src: NodeId,
        dst: NodeId,
        rel_type: &str,
        props: Props,
    ) -> Result<RelId> {
        self.guarded(|tx| {
            check_name(rel_type)?;
            let mut clean = Props::new();
            merge_props(&mut clean, props)?;
            tx.must_node(src)?;
            tx.must_node(dst)?;
            let id = RelId(tx.st.next_rel);
            tx.st.next_rel += 1;
            let stored = tx
                .inner
                .tier
                .store(&encode_rel(src, dst, rel_type, &clean));
            tx.txn.open_table(RELS)?.insert(id.0, stored.as_slice())?;
            tx.txn
                .open_table(REL_SUMS)?
                .insert(id.0, crc32c(&stored))?;
            tx.txn
                .open_table(OUT_ADJ)?
                .insert(adj_key(src, rel_type, id).as_slice(), dst.0)?;
            tx.txn
                .open_table(IN_ADJ)?
                .insert(adj_key(dst, rel_type, id).as_slice(), src.0)?;
            tx.txn
                .open_table(TYPE_IDX)?
                .insert(name_id_key(rel_type, id.0).as_slice(), ())?;
            tx.bump(type_count_key(rel_type), 1);
            tx.st.rel_count += 1;
            tx.touched.rel_types.insert(rel_type.to_string());
            tx.touched_keys.insert(CacheKey::Out(src.0));
            tx.touched_keys.insert(CacheKey::In(dst.0));
            tx.touched_edges.push((src.0, dst.0));
            tx.dirty = true;
            Ok(id)
        })
    }

    /// Merges `props` into the relationship's properties (`Null` removes a key).
    pub fn set_rel_props(&mut self, id: RelId, props: Props) -> Result<()> {
        self.guarded(|tx| {
            let rel = tx.must_rel(id)?;
            let mut merged = rel.props.clone();
            merge_props(&mut merged, props)?;
            let old = encode_rel(rel.src, rel.dst, &rel.rel_type, &rel.props);
            let new = encode_rel(rel.src, rel.dst, &rel.rel_type, &merged);
            if old != new {
                let stored = tx.inner.tier.store(&new);
                tx.txn.open_table(RELS)?.insert(id.0, stored.as_slice())?;
                tx.txn
                    .open_table(REL_SUMS)?
                    .insert(id.0, crc32c(&stored))?;
                tx.touched.rel_types.insert(rel.rel_type);
                tx.dirty = true;
            }
            Ok(())
        })
    }

    fn delete_rel_inner(&mut self, id: RelId) -> Result<()> {
        let rel = self.must_rel(id)?;
        self.txn.open_table(RELS)?.remove(id.0)?;
        self.txn.open_table(REL_SUMS)?.remove(id.0)?;
        self.txn
            .open_table(OUT_ADJ)?
            .remove(adj_key(rel.src, &rel.rel_type, id).as_slice())?;
        self.txn
            .open_table(IN_ADJ)?
            .remove(adj_key(rel.dst, &rel.rel_type, id).as_slice())?;
        self.txn
            .open_table(TYPE_IDX)?
            .remove(name_id_key(&rel.rel_type, id.0).as_slice())?;
        self.bump(type_count_key(&rel.rel_type), -1);
        self.st.rel_count -= 1;
        self.touched_keys.insert(CacheKey::Out(rel.src.0));
        self.touched_keys.insert(CacheKey::In(rel.dst.0));
        self.touched.rel_types.insert(rel.rel_type);
        self.dirty = true;
        Ok(())
    }

    /// Deletes a relationship.
    pub fn delete_rel(&mut self, id: RelId) -> Result<()> {
        self.guarded(|tx| tx.delete_rel_inner(id))
    }

    // ---------------------------------------------------------------------------------- DDL

    fn label_chunk(&self, label: &str, after: Option<&Vec<u8>>) -> Result<Vec<(Vec<u8>, NodeId)>> {
        let t = self.txn.open_table(LABEL_IDX)?;
        let (lo, hi) = prefix_bounds(&name_prefix(label));
        let lo = after.map_or(lo, |a| Bound::Excluded(a.clone()));
        let bounds: BoundsPair = (lo, hi);
        let mut out = Vec::new();
        for e in t.range::<&[u8]>(as_slices(&bounds))?.take(CHUNK) {
            let (k, _) = e?;
            out.push((k.value().to_vec(), NodeId(tail_u64(k.value())?)));
        }
        Ok(out)
    }

    fn create_index_inner(&mut self, label: &str, key: &str, backs: bool) -> Result<()> {
        self.txn
            .open_table(INDEXES)?
            .insert(index_key(label, key).as_slice(), u8::from(backs))?;
        self.st
            .indexes
            .entry(label.to_string())
            .or_default()
            .insert(key.to_string(), backs);
        let mut after: Option<Vec<u8>> = None;
        loop {
            let chunk = self.label_chunk(label, after.as_ref())?;
            let Some((last, _)) = chunk.last() else {
                break;
            };
            let last = last.clone();
            {
                let nodes = self.txn.open_table(NODES)?;
                let sums = self.txn.open_table(NODE_SUMS)?;
                let mut idx = self.txn.open_table(PROP_IDX)?;
                for (_, id) in &chunk {
                    let node = load_node_sum(&nodes, &sums, *id)?.ok_or_else(|| {
                        Error::Corrupt(format!("label index references missing node {id}"))
                    })?;
                    if let Some(v) = node.props.get(key) {
                        idx.insert(prop_key(label, key, v, *id).as_slice(), ())?;
                    }
                }
            }
            after = Some(last);
        }
        self.touched.labels.insert(label.to_string());
        self.dirty = true;
        Ok(())
    }

    fn drop_index_inner(&mut self, label: &str, key: &str) -> Result<()> {
        self.txn
            .open_table(INDEXES)?
            .remove(index_key(label, key).as_slice())?;
        if let Some(keys) = self.st.indexes.get_mut(label) {
            keys.remove(key);
            if keys.is_empty() {
                self.st.indexes.remove(label);
            }
        }
        let bounds = prefix_bounds(&prop_prefix(label, key));
        self.txn
            .open_table(PROP_IDX)?
            .retain_in::<&[u8], _>(as_slices(&bounds), |_, _| false)?;
        self.touched.labels.insert(label.to_string());
        self.dirty = true;
        Ok(())
    }

    fn has_index(&self, label: &str, key: &str) -> bool {
        self.st
            .indexes
            .get(label)
            .is_some_and(|k| k.contains_key(key))
    }

    /// Creates a B-tree property index on `(label, key)` and backfills it from existing
    /// nodes. The index is maintained by every later write.
    ///
    /// Fails with [`Error::IndexExists`] if one is already defined.
    pub fn create_index(&mut self, label: &str, key: &str) -> Result<()> {
        self.guarded(|tx| {
            check_name(label)?;
            check_name(key)?;
            if tx.has_index(label, key) {
                return Err(Error::IndexExists {
                    label: label.to_string(),
                    key: key.to_string(),
                });
            }
            tx.create_index_inner(label, key, false)
        })
    }

    /// Drops an index and all its entries. Fails with [`Error::IndexNotFound`] if absent and
    /// with [`Error::IndexBacksConstraint`] while a unique constraint depends on it.
    pub fn drop_index(&mut self, label: &str, key: &str) -> Result<()> {
        self.guarded(|tx| {
            check_name(label)?;
            check_name(key)?;
            if !tx.has_index(label, key) {
                return Err(Error::IndexNotFound {
                    label: label.to_string(),
                    key: key.to_string(),
                });
            }
            if tx.st.unique.get(label).is_some_and(|k| k.contains(key)) {
                return Err(Error::IndexBacksConstraint {
                    label: label.to_string(),
                    key: key.to_string(),
                });
            }
            tx.drop_index_inner(label, key)
        })
    }

    /// First two nodes holding an equal value under `(label, key)`, scanning the index.
    fn first_duplicate(&self, label: &str, key: &str) -> Result<Option<(NodeId, NodeId)>> {
        let t = self.txn.open_table(PROP_IDX)?;
        let prefix = prop_prefix(label, key);
        let bounds = prefix_bounds(&prefix);
        let mut prev: Option<(Vec<u8>, NodeId)> = None;
        for e in t.range::<&[u8]>(as_slices(&bounds))? {
            let (k, _) = e?;
            let raw = k.value();
            let value = raw[prefix.len()..raw.len() - 8].to_vec();
            let id = NodeId(tail_u64(raw)?);
            if let Some((pv, pid)) = &prev {
                if *pv == value {
                    return Ok(Some((*pid, id)));
                }
            }
            prev = Some((value, id));
        }
        Ok(None)
    }

    /// Creates a uniqueness constraint: no two nodes with `label` may hold an equal value
    /// for `key` (numbers compare by value). Nodes without the key never conflict.
    ///
    /// Requires an index on `(label, key)`; one is created (and owned by the constraint,
    /// dropped with it) if none exists. Fails with [`Error::ConstraintViolation`] if
    /// existing data already has duplicates, leaving the transaction unchanged.
    pub fn create_unique_constraint(&mut self, label: &str, key: &str) -> Result<()> {
        self.guarded(|tx| {
            check_name(label)?;
            check_name(key)?;
            if tx.st.unique.get(label).is_some_and(|k| k.contains(key)) {
                return Err(Error::ConstraintExists {
                    kind: ConstraintKind::Unique,
                    label: label.to_string(),
                    key: key.to_string(),
                });
            }
            let (dirty, touched) = (tx.dirty, tx.touched.clone());
            let had_index = tx.has_index(label, key);
            if !had_index {
                tx.create_index_inner(label, key, true)?;
            }
            if let Some((a, b)) = tx.first_duplicate(label, key)? {
                if !had_index {
                    tx.drop_index_inner(label, key)?;
                }
                tx.dirty = dirty;
                tx.touched = touched;
                return Err(violation(
                    ConstraintKind::Unique,
                    label,
                    key,
                    format!("existing nodes {a} and {b} hold the same value"),
                ));
            }
            tx.txn.open_table(CONSTRAINTS)?.insert(
                constraint_key(ConstraintKind::Unique, label, key).as_slice(),
                (),
            )?;
            tx.st
                .unique
                .entry(label.to_string())
                .or_default()
                .insert(key.to_string());
            tx.touched.labels.insert(label.to_string());
            tx.dirty = true;
            Ok(())
        })
    }

    /// Creates an existence constraint: every node with `label` must have a non-null `key`.
    /// Fails with [`Error::ConstraintViolation`] if an existing node lacks it.
    pub fn create_exists_constraint(&mut self, label: &str, key: &str) -> Result<()> {
        self.guarded(|tx| {
            check_name(label)?;
            check_name(key)?;
            if tx.st.exists.get(label).is_some_and(|k| k.contains(key)) {
                return Err(Error::ConstraintExists {
                    kind: ConstraintKind::Exists,
                    label: label.to_string(),
                    key: key.to_string(),
                });
            }
            let mut after: Option<Vec<u8>> = None;
            loop {
                let chunk = tx.label_chunk(label, after.as_ref())?;
                let Some((last, _)) = chunk.last() else {
                    break;
                };
                let last = last.clone();
                let nodes = tx.txn.open_table(NODES)?;
                let sums = tx.txn.open_table(NODE_SUMS)?;
                for (_, id) in &chunk {
                    let node = load_node_sum(&nodes, &sums, *id)?.ok_or_else(|| {
                        Error::Corrupt(format!("label index references missing node {id}"))
                    })?;
                    if !node.props.contains_key(key) {
                        return Err(violation(
                            ConstraintKind::Exists,
                            label,
                            key,
                            format!("existing node {id} has no `{key}` property"),
                        ));
                    }
                }
                after = Some(last);
            }
            tx.txn.open_table(CONSTRAINTS)?.insert(
                constraint_key(ConstraintKind::Exists, label, key).as_slice(),
                (),
            )?;
            tx.st
                .exists
                .entry(label.to_string())
                .or_default()
                .insert(key.to_string());
            tx.touched.labels.insert(label.to_string());
            tx.dirty = true;
            Ok(())
        })
    }

    /// Drops a constraint. For a unique constraint that created its own backing index, the
    /// index is dropped too; an index that existed beforehand is kept.
    pub fn drop_constraint(&mut self, kind: ConstraintKind, label: &str, key: &str) -> Result<()> {
        self.guarded(|tx| {
            check_name(label)?;
            check_name(key)?;
            let map = match kind {
                ConstraintKind::Unique => &mut tx.st.unique,
                ConstraintKind::Exists => &mut tx.st.exists,
            };
            let present = map.get_mut(label).is_some_and(|k| k.remove(key));
            if !present {
                return Err(Error::ConstraintNotFound {
                    kind,
                    label: label.to_string(),
                    key: key.to_string(),
                });
            }
            if map.get(label).is_some_and(BTreeSet::is_empty) {
                map.remove(label);
            }
            tx.txn
                .open_table(CONSTRAINTS)?
                .remove(constraint_key(kind, label, key).as_slice())?;
            let owned_index = kind == ConstraintKind::Unique
                && tx.st.indexes.get(label).and_then(|k| k.get(key)) == Some(&true);
            if owned_index {
                tx.drop_index_inner(label, key)?;
            }
            tx.touched.labels.insert(label.to_string());
            tx.dirty = true;
            Ok(())
        })
    }
}
