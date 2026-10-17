
use std::collections::HashMap;

use redb::{ReadableDatabase, ReadableTable};

use super::{p, tmp_graph_path};
use crate::codec::encode_node;
use crate::tables::{META, NODES, RELS};
use crate::{Graph, NodeId, Options, Placement, Props, Value};

macro_rules! both {
    ($($n:ident),* $(,)?) => {
        $(
            mod $n {
                #[test]
                fn local() {
                    super::$n(super::Placement::Local)
                }
                #[test]
                fn planned() {
                    super::$n(super::Placement::Planned)
                }
            }
        )*
    };
}

both!(
    reads_equal_writes_through_every_tier,
    values_on_disk_are_tagged_and_smaller_than_logical,
    compress_cold_false_stores_tagged_raw,
    commit_invalidates_exactly_its_touched_ids,
    a_warm_hit_is_repromoted_to_hot_and_the_displaced_record_goes_warm,
    rollback_invalidates_nothing,
    older_snapshot_never_sees_newer_cached_data,
    adjacency_lists_are_cached_and_invalidated_per_endpoint,
    check_integrity_drops_the_caches,
    check_integrity_sees_damage_that_the_caches_would_hide,
    typed_adjacency_requests_agree_whether_or_not_the_list_is_cached,
    records_below_the_threshold_are_stored_raw,
    hot_plus_warm_bytes_never_exceed_the_budget,
    hot_plus_warm_bytes_hold_under_100k_random_operations,
    a_database_written_by_the_old_code_opens_and_reads,
    randomized_differential_against_a_hashmap_model,
    identical_operation_sequences_give_identical_tier_counters,
    concurrent_readers_never_go_backwards_while_a_writer_commits,
);

fn opts(mode: Placement, hot_entries: usize, hot_bytes: usize, warm_bytes: usize) -> Options {
    opts_every(mode, 1, hot_entries, hot_bytes, warm_bytes)
}

fn opts_every(
    mode: Placement,
    plan_every: usize,
    hot_entries: usize,
    hot_bytes: usize,
    warm_bytes: usize,
) -> Options {
    Options {
        placement: mode,
        region_ids: 1,
        plan_every,
        hot_entries,
        hot_bytes,
        warm_bytes,
        cold_threshold_bytes: 64,
        ..Options::new(crate::MemoryBudget::mib(80))
    }
}

fn text(i: usize) -> String {
    format!("node {i}: the quick brown fox jumps over the lazy dog; ").repeat(12)
}

fn make(g: &Graph, n: usize) -> Vec<NodeId> {
    let mut tx = g.begin_write().unwrap();
    let ids = (0..n)
        .map(|i| {
            tx.create_node(
                &["Doc"],
                p(&[("i", Value::Int(i as i64)), ("body", Value::from(text(i)))]),
            )
            .unwrap()
        })
        .collect();
    tx.commit().unwrap();
    ids
}

fn body(g: &Graph, id: NodeId) -> Option<String> {
    let n = g.begin_read().unwrap().get_node(id).unwrap()?;
    match n.props.get("body") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn reads_equal_writes_through_every_tier(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 8, 64 * 1024, 64 * 1024)).unwrap();
    let ids = make(&g, 40);
    for _ in 0..3 {
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(body(&g, *id), Some(text(i)), "node {i}");
        }
    }
    let s = g.tier_stats().unwrap();
    assert!(s.hot.hits + s.warm.hits > 0, "{s:?}");
    assert!(s.warm.hits > 0, "warm tier never served a read: {s:?}");
    assert!(s.hot.promotions > 0 && s.warm.promotions > 0, "{s:?}");
}

fn values_on_disk_are_tagged_and_smaller_than_logical(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 8, 1 << 16, 1 << 16)).unwrap();
    make(&g, 50);
    let s = g.tier_stats().unwrap();
    assert!(s.logical_bytes > 50 * 500, "{s:?}");
    assert!(
        s.stored_bytes * 3 < s.logical_bytes,
        "text should compress at least 3x: stored {} logical {}",
        s.stored_bytes,
        s.logical_bytes
    );
    let r = g.db().begin_read().unwrap();
    let t = r.open_table(NODES).unwrap();
    let first = t
        .iter()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .1
        .value()
        .to_vec();
    assert!(uriel_tier::is_tagged(&first));
}

fn compress_cold_false_stores_tagged_raw(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let o = Options {
        compress_cold: false,
        ..opts(mode, 8, 1 << 16, 1 << 16)
    };
    let g = Graph::open_with(&path, &o).unwrap();
    make(&g, 10);
    let s = g.tier_stats().unwrap();
    assert_eq!(s.stored_bytes, s.logical_bytes + 10);
}

fn commit_invalidates_exactly_its_touched_ids(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 64, 1 << 20, 1 << 20)).unwrap();
    let ids = make(&g, 10);
    for id in &ids {
        body(&g, *id);
    }
    let before = g.tier_stats().unwrap();
    assert_eq!(before.hot.entries, 10);

    let mut tx = g.begin_write().unwrap();
    tx.set_node_props(ids[3], p(&[("body", Value::from("changed"))]))
        .unwrap();
    tx.commit().unwrap();

    let after = g.tier_stats().unwrap();
    assert_eq!(after.invalidated - before.invalidated, 1);
    assert_eq!(after.hot.entries, 9);
    assert_eq!(body(&g, ids[3]).as_deref(), Some("changed"));
    let mid = g.tier_stats().unwrap();
    for (i, id) in ids.iter().enumerate() {
        if i != 3 {
            assert_eq!(body(&g, *id), Some(text(i)));
        }
    }
    let end = g.tier_stats().unwrap();
    assert_eq!(end.hot.hits - mid.hot.hits, 9);
    assert_eq!(end.disk_reads, mid.disk_reads);
}

fn a_warm_hit_is_repromoted_to_hot_and_the_displaced_record_goes_warm(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let o = Options {
        admission_gate: false,
        ..opts(mode, 1, 1 << 20, 1 << 20)
    };
    let g = Graph::open_with(&path, &o).unwrap();
    let ids = make(&g, 2);
    body(&g, ids[0]);
    body(&g, ids[1]);
    let s = g.tier_stats().unwrap();
    assert_eq!((s.hot.entries, s.warm.entries), (1, 1));
    let disk = s.disk_reads;
    assert_eq!(body(&g, ids[0]), Some(text(0)));
    let s = g.tier_stats().unwrap();
    assert_eq!(s.warm.hits, 1);
    assert_eq!(s.disk_reads, disk);
    assert_eq!((s.hot.entries, s.warm.entries), (1, 1));
    assert_eq!(body(&g, ids[0]), Some(text(0)));
    let s2 = g.tier_stats().unwrap();
    assert_eq!(s2.hot.hits, s.hot.hits + 1);
    assert_eq!(s2.warm.hits, 1);
}
fn rollback_invalidates_nothing(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 64, 1 << 20, 1 << 20)).unwrap();
    let ids = make(&g, 6);
    for id in &ids {
        body(&g, *id);
    }
    let before = g.tier_stats().unwrap();
    {
        let mut tx = g.begin_write().unwrap();
        tx.set_node_props(ids[1], p(&[("body", Value::from("never"))]))
            .unwrap();
        tx.delete_node(ids[2]).unwrap();
        tx.rollback();
    }
    let after = g.tier_stats().unwrap();
    assert_eq!(after.invalidated, before.invalidated);
    assert_eq!(after.hot.entries, before.hot.entries);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(body(&g, *id), Some(text(i)));
    }
    assert_eq!(g.tier_stats().unwrap().disk_reads, before.disk_reads);
}

fn older_snapshot_never_sees_newer_cached_data(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 64, 1 << 20, 1 << 20)).unwrap();
    let ids = make(&g, 3);
    let old = g.begin_read().unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.set_node_props(ids[0], p(&[("body", Value::from("v2"))]))
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(body(&g, ids[0]).as_deref(), Some("v2"));
    assert_eq!(body(&g, ids[0]).as_deref(), Some("v2"));
    let n = old.get_node(ids[0]).unwrap().unwrap();
    assert_eq!(n.props.get("body"), Some(&Value::from(text(0))));
}

fn adjacency_lists_are_cached_and_invalidated_per_endpoint(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 64, 1 << 20, 1 << 20)).unwrap();
    let ids = make(&g, 4);
    let mut tx = g.begin_write().unwrap();
    tx.create_rel(ids[0], ids[1], "K", Props::new()).unwrap();
    tx.create_rel(ids[2], ids[3], "K", Props::new()).unwrap();
    tx.commit().unwrap();
    let outs = |g: &Graph, n: NodeId| {
        g.begin_read()
            .unwrap()
            .out_edges(n, None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(outs(&g, ids[0]).len(), 1);
    assert_eq!(outs(&g, ids[2]).len(), 1);
    let before = g.tier_stats().unwrap();
    assert_eq!(outs(&g, ids[0]).len(), 1);
    assert_eq!(g.tier_stats().unwrap().hot.hits, before.hot.hits + 1);

    let mut tx = g.begin_write().unwrap();
    tx.create_rel(ids[0], ids[3], "K", Props::new()).unwrap();
    tx.commit().unwrap();
    let after = g.tier_stats().unwrap();
    assert_eq!(
        after.invalidated - before.invalidated,
        1,
        "only Out(0) was cached; In(3) was not"
    );
    assert_eq!(outs(&g, ids[0]).len(), 2);
    let again = g.tier_stats().unwrap();
    assert_eq!(outs(&g, ids[2]).len(), 1);
    assert_eq!(g.tier_stats().unwrap().hot.hits, again.hot.hits + 1);
}

fn check_integrity_drops_the_caches(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 64, 1 << 20, 1 << 20)).unwrap();
    let ids = make(&g, 5);
    for id in &ids {
        body(&g, *id);
    }
    assert_eq!(g.tier_stats().unwrap().hot.entries, 5);
    assert!(g
        .begin_read()
        .unwrap()
        .check_integrity()
        .unwrap()
        .is_empty());
    let s = g.tier_stats().unwrap();
    assert_eq!(s.hot.entries, 0);
    assert_eq!(s.warm.entries, 0);
}

fn check_integrity_sees_damage_that_the_caches_would_hide(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 64, 1 << 20, 1 << 20)).unwrap();
    let ids = make(&g, 5);
    for id in &ids {
        body(&g, *id);
    }
    let w = g.db().begin_write().unwrap();
    w.open_table(NODES).unwrap().remove(ids[2].0).unwrap();
    w.commit().unwrap();
    let problems = g
        .begin_read()
        .unwrap()
        .check_integrity()
        .unwrap();
    assert!(!problems.is_empty(), "audit was fooled by the cache");
    assert_eq!(body(&g, ids[2]), None, "the stale entry survived the audit");
}

fn typed_adjacency_requests_agree_whether_or_not_the_list_is_cached(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 64, 1 << 20, 1 << 20)).unwrap();
    let ids = make(&g, 6);
    let mut tx = g.begin_write().unwrap();
    for (i, ty) in ["B", "A", "B", "C", "A"].iter().enumerate() {
        tx.create_rel(ids[0], ids[i + 1], ty, Props::new()).unwrap();
    }
    tx.commit().unwrap();
    let typed = |ty: Option<&str>| -> Vec<(u64, String, u64)> {
        g.begin_read()
            .unwrap()
            .out_edges(ids[0], ty)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.rel.0, e.rel_type, e.other.0)
            })
            .collect()
    };
    let cold: Vec<_> = ["A", "B", "C", "Z"]
        .iter()
        .map(|t| typed(Some(t)))
        .collect();
    let all_cold = typed(None);
    assert_eq!(g.tier_stats().unwrap().hot.entries, 1);
    let warm: Vec<_> = ["A", "B", "C", "Z"]
        .iter()
        .map(|t| typed(Some(t)))
        .collect();
    assert_eq!(cold, warm, "typed answer differs once the list is cached");
    assert_eq!(all_cold.len(), 5);
    assert!(warm[3].is_empty());
}

fn records_below_the_threshold_are_stored_raw(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let o = Options {
        cold_threshold_bytes: 4096,
        ..opts(mode, 8, 1 << 16, 1 << 16)
    };
    let g = Graph::open_with(&path, &o).unwrap();
    make(&g, 10);
    let s = g.tier_stats().unwrap();
    assert_eq!(s.stored_bytes, s.logical_bytes + 10, "one tag byte each");
    let r = g.db().begin_read().unwrap();
    let t = r.open_table(NODES).unwrap();
    for e in t.iter().unwrap() {
        assert_eq!(e.unwrap().1.value()[0], uriel_tier::TAG_RAW);
    }
}

fn hot_plus_warm_bytes_never_exceed_the_budget(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 16, 6 * 1024, 4 * 1024)).unwrap();
    let ids = make(&g, 120);
    let mut x = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..3000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let i = (x % 120) as usize;
        assert_eq!(body(&g, ids[i]), Some(text(i)));
        let s = g.tier_stats().unwrap();
        assert!(
            s.hot.bytes + s.warm.bytes <= s.budget_bytes,
            "{} + {} > {}",
            s.hot.bytes,
            s.warm.bytes,
            s.budget_bytes
        );
    }
}

fn hot_plus_warm_bytes_hold_under_100k_random_operations(mode: Placement) {
    let o = opts_every(mode, 64, 16, 6 * 1024, 4 * 1024);
    let g = Graph::open_in_memory_with(&o).unwrap();
    let mut live = make(&g, 150);
    let mut model: HashMap<NodeId, String> = live
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, text(i)))
        .collect();
    let mut x = 0x5851_f42d_4c95_7f2du64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (mut max_used, mut ops) = (0usize, 0u64);
    for step in 0..100_000usize {
        ops += 1;
        let pick = (next() % live.len() as u64) as usize;
        match next() % 20 {
            0 => {
                let t = text(step + 1_000);
                let mut tx = g.begin_write().unwrap();
                let id = tx
                    .create_node(&["Doc"], p(&[("body", Value::from(t.clone()))]))
                    .unwrap();
                tx.commit().unwrap();
                model.insert(id, t);
                live.push(id);
            }
            1 | 2 => {
                let t = format!("v{step} ").repeat(1 + (next() % 60) as usize);
                let mut tx = g.begin_write().unwrap();
                tx.set_node_props(live[pick], p(&[("body", Value::from(t.clone()))]))
                    .unwrap();
                tx.commit().unwrap();
                model.insert(live[pick], t);
            }
            3 if live.len() > 100 => {
                let id = live.swap_remove(pick);
                let mut tx = g.begin_write().unwrap();
                tx.detach_delete_node(id).unwrap();
                tx.commit().unwrap();
                model.remove(&id);
            }
            _ => assert_eq!(body(&g, live[pick]).as_ref(), model.get(&live[pick])),
        }
        let s = g.inner.tier.counters(0, 0);
        let used = s.hot.bytes + s.warm.bytes;
        max_used = max_used.max(used);
        assert!(
            used <= s.budget_bytes,
            "step {step}: {used} > {}",
            s.budget_bytes
        );
    }
    assert_eq!(ops, 100_000);
    assert!(max_used > o.hot_bytes, "T1 never filled: peak {max_used}");
    let s = g.inner.tier.counters(0, 0);
    assert!(
        s.warm.hits > 0 && s.hot.hits > 0 && s.hot.evictions > 0,
        "{s:?}"
    );
}
fn downgrade_to_legacy(g: &Graph) {
    let w = g.db().begin_write().unwrap();
    for def in [NODES, RELS] {
        let mut t = w.open_table(def).unwrap();
        let rows: Vec<(u64, Vec<u8>)> = t
            .iter()
            .unwrap()
            .map(|e| {
                let (k, v) = e.unwrap();
                (k.value(), uriel_tier::decode_value(v.value()).unwrap())
            })
            .collect();
        for (k, v) in rows {
            t.insert(k, v.as_slice()).unwrap();
        }
    }
    w.open_table(META).unwrap().insert("format", 1u64).unwrap();
    w.commit().unwrap();
}

fn a_database_written_by_the_old_code_opens_and_reads(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let ids = {
        let g = Graph::open_with(&path, &opts(mode, 8, 1 << 16, 1 << 16)).unwrap();
        let ids = make(&g, 20);
        let mut tx = g.begin_write().unwrap();
        tx.create_rel(ids[0], ids[1], "K", p(&[("w", Value::Int(3))]))
            .unwrap();
        tx.commit().unwrap();
        downgrade_to_legacy(&g);
        let r = g.db().begin_read().unwrap();
        let t = r.open_table(NODES).unwrap();
        for e in t.iter().unwrap() {
            let (k, v) = e.unwrap();
            let n = crate::codec::decode_node(NodeId(k.value()), v.value()).unwrap();
            assert_eq!(v.value(), encode_node(&n.labels, &n.props).as_slice());
        }
        ids
    };
    let g = Graph::open_with(&path, &opts(mode, 8, 1 << 16, 1 << 16)).unwrap();
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(body(&g, *id), Some(text(i)));
    }
    let r = g.begin_read().unwrap();
    assert_eq!(r.out_rels(ids[0], None).unwrap().count(), 1);
    assert!(r.check_integrity().unwrap().is_empty());
    let mut tx = g.begin_write().unwrap();
    let fresh = tx
        .create_node(&["Doc"], p(&[("body", Value::from(text(99)))]))
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(body(&g, fresh), Some(text(99)));
    assert_eq!(body(&g, ids[5]), Some(text(5)));
    let rd = g.db().begin_read().unwrap();
    let t = rd.open_table(NODES).unwrap();
    let raw = t.get(fresh.0).unwrap().unwrap().value().to_vec();
    assert!(uriel_tier::is_tagged(&raw));
    let old = t.get(ids[5].0).unwrap().unwrap().value().to_vec();
    assert!(!uriel_tier::is_tagged(&old));
}

fn randomized_differential_against_a_hashmap_model(mode: Placement) {
    for seed in [
        0x9e37_79b9_7f4a_7c15u64,
        0x2545_f491_4f6c_dd1d,
        0xdead_beef_1234_5678,
        0x0123_4567_89ab_cdef,
        0xfeed_face_cafe_f00d,
    ] {
        differential(mode, seed);
    }
}

fn differential(mode: Placement, seed: u64) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts_every(mode, 16, 3, 2048, 1024)).unwrap();
    let mut model: HashMap<NodeId, String> = HashMap::new();
    let mut edges: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut live: Vec<NodeId> = Vec::new();
    let mut x = seed;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for step in 0..2000 {
        match next() % 8 {
            0 | 1 => {
                let t = text(step);
                let mut tx = g.begin_write().unwrap();
                let id = tx
                    .create_node(&["Doc"], p(&[("body", Value::from(t.clone()))]))
                    .unwrap();
                tx.commit().unwrap();
                model.insert(id, t);
                live.push(id);
            }
            2 | 3 if !live.is_empty() => {
                let id = live[(next() % live.len() as u64) as usize];
                let t = format!("updated at {step} ").repeat(1 + (next() % 20) as usize);
                let mut tx = g.begin_write().unwrap();
                tx.set_node_props(id, p(&[("body", Value::from(t.clone()))]))
                    .unwrap();
                if next() % 5 == 0 {
                    tx.rollback();
                } else {
                    tx.commit().unwrap();
                    model.insert(id, t);
                }
            }
            4 if live.len() > 1 => {
                let i = (next() % live.len() as u64) as usize;
                let id = live.swap_remove(i);
                let mut tx = g.begin_write().unwrap();
                tx.detach_delete_node(id).unwrap();
                tx.commit().unwrap();
                model.remove(&id);
                edges.remove(&id);
                for v in edges.values_mut() {
                    v.retain(|o| *o != id);
                }
            }
            5 if live.len() > 1 => {
                let a = live[(next() % live.len() as u64) as usize];
                let b = live[(next() % live.len() as u64) as usize];
                let mut tx = g.begin_write().unwrap();
                tx.create_rel(a, b, "K", Props::new()).unwrap();
                tx.commit().unwrap();
                edges.entry(a).or_default().push(b);
            }
            _ => {
                if live.is_empty() {
                    continue;
                }
                let id = live[(next() % live.len() as u64) as usize];
                assert_eq!(body(&g, id).as_ref(), model.get(&id), "step {step}");
                let mut got: Vec<u64> = g
                    .begin_read()
                    .unwrap()
                    .out_edges(id, None)
                    .unwrap()
                    .map(|e| e.unwrap().other.0)
                    .collect();
                let mut want: Vec<u64> = edges
                    .get(&id)
                    .map_or(vec![], |v| v.iter().map(|n| n.0).collect());
                got.sort_unstable();
                want.sort_unstable();
                assert_eq!(got, want, "edges of {id} at step {step}");
            }
        }
    }
    for id in &live {
        assert_eq!(body(&g, *id).as_ref(), model.get(id));
    }
    assert!(g
        .begin_read()
        .unwrap()
        .check_integrity()
        .unwrap()
        .is_empty());
    let s = g.tier_stats().unwrap();
    assert!(
        s.hot.evictions > 0 && s.warm.hits + s.hot.hits > 0,
        "no tier churn at seed {seed:#x}: {s:?}"
    );
    assert!(
        s.warm.hits > 0,
        "T1 never served a read at seed {seed:#x}: {s:?}"
    );
}

fn tier_trace(mode: Placement, seed: u64) -> crate::TierStats {
    let g = Graph::open_in_memory_with(&opts(mode, 5, 4096, 2048)).unwrap();
    let ids = make(&g, 40);
    let mut x = seed;
    for _ in 0..3000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let id = ids[(x % 40) as usize];
        if x.is_multiple_of(11) {
            let mut tx = g.begin_write().unwrap();
            tx.set_node_props(id, p(&[("body", Value::from(format!("w{x}")))]))
                .unwrap();
            tx.commit().unwrap();
        } else {
            body(&g, id);
        }
    }
    g.inner.tier.counters(0, 0)
}

fn identical_operation_sequences_give_identical_tier_counters(mode: Placement) {
    for seed in [3u64, 0x9e37_79b9_7f4a_7c15] {
        let (a, b) = (tier_trace(mode, seed), tier_trace(mode, seed));
        assert_eq!(a, b, "seed {seed:#x}");
        assert!(a.hot.hits > 0 && a.warm.hits > 0 && a.invalidated > 0, "{a:?}");
    }
    assert_ne!(tier_trace(mode, 3), tier_trace(mode, 4));
}
fn concurrent_readers_never_go_backwards_while_a_writer_commits(mode: Placement) {
    let (_d, path) = tmp_graph_path();
    let g = Graph::open_with(&path, &opts(mode, 8, 1 << 16, 1 << 16)).unwrap();
    let id = {
        let mut tx = g.begin_write().unwrap();
        let id = tx.create_node(&["C"], p(&[("n", Value::Int(0))])).unwrap();
        tx.commit().unwrap();
        id
    };
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let (g, done) = (g.clone(), done.clone());
            std::thread::spawn(move || {
                let mut last = 0i64;
                while !done.load(std::sync::atomic::Ordering::SeqCst) {
                    let n = g.begin_read().unwrap().get_node(id).unwrap().unwrap();
                    let Some(Value::Int(v)) = n.props.get("n") else {
                        panic!("missing counter")
                    };
                    assert!(*v >= last, "read went backwards: {v} after {last}");
                    last = *v;
                }
                last
            })
        })
        .collect();
    for i in 1..=60 {
        let mut tx = g.begin_write().unwrap();
        tx.set_node_props(id, p(&[("n", Value::Int(i))])).unwrap();
        tx.commit().unwrap();
    }
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    for r in readers {
        assert!(r.join().unwrap() <= 60);
    }
    let n = g.begin_read().unwrap().get_node(id).unwrap().unwrap();
    assert_eq!(n.props.get("n"), Some(&Value::Int(60)));
}

pub(super) fn corpus() -> String {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "target" && n != ".git") {
                    walk(&p, out);
                }
            } else if matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("rs" | "md" | "toml")
            ) {
                out.push(p);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    walk(&root.join("docs"), &mut files);
    files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn windows(text: &str, n: usize, win: usize) -> Vec<&str> {
    let stride = (text.len().saturating_sub(win) / n).max(1);
    (0..n)
        .map(|i| {
            let mut a = (i * stride).min(text.len());
            while !text.is_char_boundary(a) {
                a += 1;
            }
            let mut b = (a + win).min(text.len());
            while !text.is_char_boundary(b) {
                b -= 1;
            }
            &text[a..b]
        })
        .collect()
}

#[test]
#[ignore = "measurement"]
fn measure_disk_bytes_and_codec_ratios() {
    let corpus = corpus();
    let docs = windows(&corpus, 20_000, 1024);
    println!(
        "corpus {} bytes; 20000 nodes, 1024-byte text windows",
        corpus.len()
    );
    let build = |compress: bool| {
        let (d, path) = tmp_graph_path();
        let o = Options {
            compress_cold: compress,
            ..Options::new(crate::MemoryBudget::mib(80))
        };
        let t0 = std::time::Instant::now();
        let g = Graph::open_with(&path, &o).unwrap();
        for chunk in docs.chunks(2000).enumerate() {
            let mut tx = g.begin_write().unwrap();
            for (j, t) in chunk.1.iter().enumerate() {
                let i = chunk.0 * 2000 + j;
                tx.create_node(
                    &["Chunk"],
                    p(&[
                        ("i", Value::Int(i as i64)),
                        ("path", Value::from("crates/uriel-core/src/example.rs")),
                        ("body", Value::from(*t)),
                    ]),
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        (d, path, g, t0.elapsed())
    };
    let (_d1, p1, g_raw, t_raw) = build(false);
    let (_d2, p2, g_cold, t_cold) = build(true);
    let s_raw = g_raw.tier_stats().unwrap();
    let s_cold = g_cold.tier_stats().unwrap();
    let (mut lz4_total, mut zstd_check) = (0u64, 0u64);
    {
        let r = g_raw.db().begin_read().unwrap();
        let t = r.open_table(NODES).unwrap();
        for e in t.iter().unwrap() {
            let (_, v) = e.unwrap();
            let rec = uriel_tier::decode_value(v.value()).unwrap();
            lz4_total += uriel_tier::encode_value(&rec, uriel_tier::Tier::Warm, 256).len() as u64;
            zstd_check += uriel_tier::encode_value(&rec, uriel_tier::Tier::Cold, 256).len() as u64;
        }
    }
    let f1 = std::fs::metadata(&p1).unwrap().len();
    let f2 = std::fs::metadata(&p2).unwrap().len();
    println!("logical record bytes        {}", s_cold.logical_bytes);
    println!(
        "stored, raw tagged          {}  ratio {:.2}",
        s_raw.stored_bytes,
        s_raw.compression_ratio()
    );
    println!(
        "stored, lz4 per record      {}  ratio {:.2}",
        lz4_total,
        s_cold.logical_bytes as f64 / lz4_total as f64
    );
    println!(
        "stored, zstd-19 per record  {}  ratio {:.2} (recheck {})",
        s_cold.stored_bytes,
        s_cold.compression_ratio(),
        zstd_check
    );
    println!("redb file, compress off     {f1}");
    println!(
        "redb file, compress on      {f2}  ({:.2}x smaller)",
        f1 as f64 / f2 as f64
    );
    println!("build time off {t_raw:?}, on {t_cold:?}");
    let r = g_cold.begin_read().unwrap();
    for i in [0usize, 777, 19_999] {
        let n = r.get_node(NodeId(i as u64)).unwrap().unwrap();
        assert_eq!(n.props.get("body"), Some(&Value::from(docs[i])));
    }
}

pub(super) struct Zipf {
    pub(super) cdf: Vec<f64>,
    state: u64,
}

impl Zipf {
    pub(super) fn new(n: usize, s: f64, seed: u64) -> Zipf {
        let mut acc = 0.0;
        let mut cdf: Vec<f64> = (1..=n)
            .map(|r| {
                acc += 1.0 / (r as f64).powf(s);
                acc
            })
            .collect();
        for c in &mut cdf {
            *c /= acc;
        }
        Zipf { cdf, state: seed }
    }

    pub(super) fn sample(&mut self) -> usize {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let u = (self.state >> 11) as f64 / (1u64 << 53) as f64;
        self.cdf.partition_point(|c| *c < u).min(self.cdf.len() - 1)
    }
}

#[test]
#[ignore = "measurement"]
fn measure_hot_set_hit_rate_with_and_without_the_gate() {
    const N: usize = 20_000;
    let corpus = corpus();
    let docs = windows(&corpus, N, 1024);
    let (_d, path) = tmp_graph_path();
    {
        let g = Graph::open_with(&path, &Options::new(crate::MemoryBudget::mib(80))).unwrap();
        for chunk in docs.chunks(2000).enumerate() {
            let mut tx = g.begin_write().unwrap();
            for (j, t) in chunk.1.iter().enumerate() {
                tx.create_node(
                    &["C"],
                    p(&[
                        ("i", Value::Int((chunk.0 * 2000 + j) as i64)),
                        ("body", Value::from(*t)),
                    ]),
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
    }
    let id_of = |rank: usize| NodeId(((rank * 7919) % N) as u64);
    println!("N={N}, hot_entries=1000 (5% of keys), warm tier off, Zipf s=1.0, 200000 Zipf reads");
    println!(
        "ideal static top-1000 (oracle that knows the ranks): {:.1}%",
        Zipf::new(N, 1.0, 1).cdf[999] * 100.0
    );
    println!("{:<34} {:>8} {:>8}", "workload", "gate on", "gate off");
    for (name, scan_every, burst) in [
        ("zipf only", 0usize, false),
        ("zipf + 1 scan read per zipf read", 1, false),
        ("zipf + full 20k scan burst/20k reads", 0, true),
    ] {
        let mut rates = Vec::new();
        for gate in [true, false] {
            let g = Graph::open_with(
                &path,
                &Options {
                    hot_entries: 1000,
                    hot_bytes: 1 << 30,
                    warm_bytes: 0,
                    admission_gate: gate,
                    ..Options::new(crate::MemoryBudget::mib(80))
                },
            )
            .unwrap();
            let mut z = Zipf::new(N, 1.0, 0x1234_5678_9abc_def1);
            let (mut hits, mut reads, mut scan_pos) = (0u64, 0u64, 0usize);
            let r = g.begin_read().unwrap();
            for k in 0..200_000usize {
                let before = g.inner.tier.counters(0, 0).hot.hits;
                r.get_node(id_of(z.sample())).unwrap();
                hits += g.inner.tier.counters(0, 0).hot.hits - before;
                reads += 1;
                let scans = if burst && k % 20_000 == 19_999 {
                    N
                } else {
                    scan_every
                };
                for _ in 0..scans {
                    r.get_node(NodeId((scan_pos % N) as u64)).unwrap();
                    scan_pos += 1;
                }
            }
            rates.push(hits as f64 / reads as f64);
        }
        println!(
            "{:<34} {:>7.1}% {:>7.1}%",
            name,
            rates[0] * 100.0,
            rates[1] * 100.0
        );
    }
}

#[test]
#[ignore = "measurement"]
fn measure_read_latency_per_tier() {
    const N: usize = 20_000;
    const READS: usize = 1_000_000;
    const SET: usize = 1000;
    let corpus = corpus();
    let docs = windows(&corpus, N, 1024);
    let (_d, path) = tmp_graph_path();
    {
        let g = Graph::open_with(&path, &Options::new(crate::MemoryBudget::mib(80))).unwrap();
        for chunk in docs.chunks(2000).enumerate() {
            let mut tx = g.begin_write().unwrap();
            for (j, t) in chunk.1.iter().enumerate() {
                tx.create_node(
                    &["C"],
                    p(&[
                        ("i", Value::Int((chunk.0 * 2000 + j) as i64)),
                        ("body", Value::from(*t)),
                    ]),
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
    }
    let median = |g: &Graph, ids: &[NodeId], label: &str| {
        let r = g.begin_read().unwrap();
        for _ in 0..3 {
            for id in ids {
                r.get_node(*id).unwrap();
            }
        }
        let (batches, per) = (1_000usize, READS / 1_000);
        let mut ns = Vec::with_capacity(batches);
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..batches {
            let mut picked = Vec::with_capacity(per);
            for _ in 0..per {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                picked.push(ids[(state % ids.len() as u64) as usize]);
            }
            let t = std::time::Instant::now();
            for id in &picked {
                std::hint::black_box(r.get_node(*id).unwrap());
            }
            ns.push(t.elapsed().as_nanos() as u64 / per as u64);
        }
        ns.sort_unstable();
        let s = g.inner.tier.counters(0, 0);
        println!(
            "{label:<46} median {:>6} ns  p10 {:>6}  p90 {:>6}  (T0 hits {}, T1 hits {}, disk reads {})",
            ns[batches / 2],
            ns[batches / 10],
            ns[batches * 9 / 10],
            s.hot.hits,
            s.warm.hits,
            g.inner.tier.disk_reads_for_test()
        );
    };
    let ids: Vec<NodeId> = (0..SET).map(|i| NodeId((i * 19) as u64)).collect();
    let base = Options {
        hot_bytes: 1 << 30,
        warm_bytes: 1 << 30,
        ..Options::new(crate::MemoryBudget::mib(80))
    };
    println!("{SET} distinct 1 KiB-text nodes, {READS} random reads per row (1,000 batches of 1,000), release build");
    {
        let g = Graph::open_with(
            &path,
            &Options {
                hot_entries: 4096,
                ..base.clone()
            },
        )
        .unwrap();
        median(&g, &ids, "T0 hot (decoded, cloned out)");
    }
    {
        let g = Graph::open_with(
            &path,
            &Options {
                hot_entries: 0,
                ..base.clone()
            },
        )
        .unwrap();
        median(&g, &ids, "T1 warm (lz4 block + record decode)");
    }
    {
        let g = Graph::open_with(
            &path,
            &Options {
                hot_entries: 0,
                warm_bytes: 0,
                ..base.clone()
            },
        )
        .unwrap();
        median(&g, &ids, "T2 redb, 1000 ids, page cache, zstd-19 record");
        let wide: Vec<NodeId> = (0..N).map(|i| NodeId(i as u64)).collect();
        median(&g, &wide, "T2 redb, 20000 ids, page cache, zstd-19 record");
    }
}
