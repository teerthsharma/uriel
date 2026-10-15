//! Consistent-hash ring with virtual nodes.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hash::hash64;

/// Cluster-unique node identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(s: impl Into<String>) -> Self {
        NodeId(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        NodeId(s.to_owned())
    }
}

/// Default virtual nodes per physical node.
pub const DEFAULT_VNODES: usize = 128;

/// Consistent-hash ring. Placement depends only on the node set, never on the
/// order nodes were added, so every member that sees the same membership computes
/// the same owners.
#[derive(Debug, Clone)]
pub struct Ring {
    vnodes: usize,
    nodes: BTreeSet<NodeId>,
    /// Sorted by `(hash, node)`.
    points: Vec<(u64, NodeId)>,
}

impl Default for Ring {
    fn default() -> Self {
        Ring::new(DEFAULT_VNODES)
    }
}

impl Ring {
    pub fn new(vnodes: usize) -> Self {
        Ring {
            vnodes: vnodes.max(1),
            nodes: BTreeSet::new(),
            points: Vec::new(),
        }
    }

    /// Returns `false` if the node was already present.
    pub fn add_node(&mut self, node: &NodeId) -> bool {
        if !self.nodes.insert(node.clone()) {
            return false;
        }
        for i in 0..self.vnodes {
            let label = format!("{}#{}", node.0, i);
            self.points
                .push((hash64(label.as_bytes(), 0), node.clone()));
        }
        self.points.sort_unstable();
        true
    }

    /// Returns `false` if the node was not present.
    pub fn remove_node(&mut self, node: &NodeId) -> bool {
        if !self.nodes.remove(node) {
            return false;
        }
        self.points.retain(|(_, n)| n != node);
        true
    }

    pub fn contains(&self, node: &NodeId) -> bool {
        self.nodes.contains(node)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn nodes(&self) -> Vec<NodeId> {
        self.nodes.iter().cloned().collect()
    }

    /// Total virtual points on the ring.
    pub fn points(&self) -> usize {
        self.points.len()
    }

    /// Up to `replicas` distinct nodes, clockwise from the key's position.
    pub fn owners(&self, key: &str, replicas: usize) -> Vec<NodeId> {
        let want = replicas.min(self.nodes.len());
        let mut out: Vec<NodeId> = Vec::with_capacity(want);
        if want == 0 {
            return out;
        }
        let h = hash64(key.as_bytes(), 0);
        let start = self.points.partition_point(|(p, _)| *p < h);
        let n = self.points.len();
        for step in 0..n {
            let (_, node) = &self.points[(start + step) % n];
            if !out.contains(node) {
                out.push(node.clone());
                if out.len() == want {
                    break;
                }
            }
        }
        out
    }

    pub fn owner(&self, key: &str) -> Option<NodeId> {
        self.owners(key, 1).pop()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ring_of(n: usize) -> Ring {
        let mut r = Ring::default();
        for i in 0..n {
            assert!(r.add_node(&NodeId::new(format!("node-{i}"))));
        }
        r
    }

    fn keys(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("query:{i}:MATCH (n:Person)"))
            .collect()
    }

    #[test]
    fn empty_ring_has_no_owners() {
        let r = Ring::default();
        assert!(r.owners("k", 3).is_empty());
        assert!(r.owner("k").is_none());
        assert!(r.is_empty());
    }

    #[test]
    fn membership_bookkeeping() {
        let mut r = ring_of(3);
        assert_eq!(r.len(), 3);
        assert_eq!(r.points(), 3 * DEFAULT_VNODES);
        assert!(!r.add_node(&NodeId::new("node-1")), "duplicate add");
        assert_eq!(r.points(), 3 * DEFAULT_VNODES);
        assert!(r.contains(&NodeId::new("node-2")));
        assert!(r.remove_node(&NodeId::new("node-2")));
        assert!(!r.remove_node(&NodeId::new("node-2")));
        assert_eq!(r.points(), 2 * DEFAULT_VNODES);
        assert_eq!(r.nodes().len(), 2);
    }

    #[test]
    fn owners_are_distinct_capped_and_prefix_consistent() {
        let r = ring_of(4);
        for k in keys(500) {
            let o3 = r.owners(&k, 3);
            assert_eq!(o3.len(), 3);
            let mut d = o3.clone();
            d.sort();
            d.dedup();
            assert_eq!(d.len(), 3, "owners must be distinct");
            assert_eq!(r.owners(&k, 1), o3[..1]);
            assert_eq!(r.owner(&k).as_ref(), Some(&o3[0]));
            assert_eq!(r.owners(&k, 99).len(), 4, "capped at node count");
            assert!(r.owners(&k, 0).is_empty());
        }
    }

    #[test]
    fn placement_is_independent_of_insertion_order() {
        let mut a = Ring::default();
        let mut b = Ring::default();
        let ids: Vec<NodeId> = (0..5).map(|i| NodeId::new(format!("n{i}"))).collect();
        for id in &ids {
            a.add_node(id);
        }
        for id in ids.iter().rev() {
            b.add_node(id);
        }
        for k in keys(2000) {
            assert_eq!(a.owners(&k, 3), b.owners(&k, 3));
        }
    }

    /// Stated bound: with 128 vnodes and 5 nodes, max/min key load <= 1.5.
    #[test]
    fn distribution_is_even() {
        let r = ring_of(5);
        let mut load: HashMap<NodeId, usize> = HashMap::new();
        for k in keys(100_000) {
            *load.entry(r.owner(&k).unwrap()).or_default() += 1;
        }
        assert_eq!(load.len(), 5, "every node owns something");
        let max = *load.values().max().unwrap() as f64;
        let min = *load.values().min().unwrap() as f64;
        let ratio = max / min;
        eprintln!("ring evenness max/min = {ratio:.3}");
        assert!(ratio <= 1.5, "max/min load ratio {ratio:.3} > 1.5");
    }

    #[test]
    fn adding_a_fifth_node_moves_about_one_fifth_of_keys_all_to_the_new_node() {
        let before = ring_of(4);
        let mut after = before.clone();
        let newcomer = NodeId::new("node-4");
        after.add_node(&newcomer);

        let ks = keys(50_000);
        let mut moved = 0usize;
        for k in &ks {
            let a = before.owner(k).unwrap();
            let b = after.owner(k).unwrap();
            if a != b {
                moved += 1;
                assert_eq!(b, newcomer, "keys may only move to the new node");
            }
        }
        let frac = moved as f64 / ks.len() as f64;
        eprintln!("moved fraction on 4->5 = {frac:.4}");
        assert!(
            (0.10..=0.30).contains(&frac),
            "moved {frac:.3}, ideal 0.200"
        );
    }

    #[test]
    fn removing_a_node_only_moves_that_nodes_keys() {
        let before = ring_of(5);
        let mut after = before.clone();
        let gone = NodeId::new("node-3");
        after.remove_node(&gone);
        for k in keys(20_000) {
            let a = before.owner(&k).unwrap();
            let b = after.owner(&k).unwrap();
            if a != gone {
                assert_eq!(a, b, "key not owned by the removed node must stay");
            } else {
                assert_ne!(b, gone);
            }
        }
    }
}
