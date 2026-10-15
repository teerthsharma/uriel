use std::collections::HashMap;
use std::sync::Arc;

use uriel_place::replay::SplitMix;
use uriel_place::{AccessKind, Event, Tier};

use super::p;
use crate::tier::CacheKey;
use crate::{Graph, NodeId, Options, Placement, Props, TierStats, Value};

fn planned(
    hot_entries: usize,
    hot_bytes: usize,
    warm_bytes: usize,
    region_ids: usize,
    plan_every: usize,
) -> Options {
    Options {
        placement: Placement::Planned,
        region_ids,
        plan_every,
        hot_entries,
        hot_bytes,
        warm_bytes,
        cold_threshold_bytes: 64,
        record_events: true,
        ..Options::default()
    }
}

fn text(i: usize) -> String {
    format!("node {i}: the quick brown fox jumps over the lazy dog; ").repeat(6)
}

fn make(g: &Graph, n: usize) -> Vec<NodeId> {
    let mut tx = g.begin_write().unwrap();
    let ids = (0..n)
        .map(|i| {
            tx.create_node(&["Doc"], p(&[("body", Value::from(text(i)))]))
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

fn write_regions(log: &[Event]) -> Vec<u32> {
    let mut v: Vec<u32> = log
        .iter()
        .filter_map(|e| match e {
            Event::Access {
                region,
                kind: AccessKind::Write,
            } => Some(region.0),
            _ => None,
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[test]
fn a_region_is_a_fixed_run_of_ids_per_record_class() {
    let g = Graph::open_in_memory_with(&planned(64, 1 << 20, 1 << 20, 4, 1)).unwrap();
    let ids = make(&g, 12);
    let log = g.placement_log();
    let writes = log
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::Access {
                    kind: AccessKind::Write,
                    ..
                }
            )
        })
        .count();
    assert_eq!(writes, 12);
    assert_eq!(write_regions(&log), vec![0, 3, 6]);
    let mut tx = g.begin_write().unwrap();
    tx.create_rel(ids[0], ids[5], "K", Props::new()).unwrap();
    tx.commit().unwrap();
    assert_eq!(write_regions(&g.placement_log()), vec![0, 1, 3, 5, 6]);
}

#[test]
fn resident_records_always_belong_to_a_hot_or_warm_region_and_cold_ones_leave() {
    let g = Graph::open_in_memory_with(&planned(2, 1 << 20, 1 << 20, 1, 1)).unwrap();
    let ids = make(&g, 20);
    for round in 0..3 {
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(body(&g, *id), Some(text(i)), "round {round} node {i}");
            for k in g.inner.tier.hot_keys() {
                assert_eq!(g.inner.tier.tier_of(k), Some(Tier::Hot), "{k:?} in T0");
            }
            for k in g.inner.tier.warm_keys() {
                assert!(
                    matches!(g.inner.tier.tier_of(k), Some(Tier::Hot | Tier::Warm)),
                    "{k:?} in T1 sits in a cold region"
                );
            }
        }
    }
    let hot = g.inner.tier.hot_keys();
    let warm = g.inner.tier.warm_keys();
    assert!(!g.placement_hot_regions().is_empty());
    assert!(g.placement_hot_regions().len() <= 2);
    assert_eq!(hot.len(), 2, "{hot:?}");
    assert!(!warm.is_empty() && warm.len() <= 8, "{warm:?}");
    assert!(
        hot.len() + warm.len() < 20,
        "cold regions were never evicted"
    );
}

#[test]
fn a_prefetch_region_is_loaded_into_t1_before_any_read_of_it() {
    let g = Graph::open_in_memory_with(&planned(4, 1 << 20, 1 << 20, 4, 1)).unwrap();
    let ids = make(&g, 16);
    let mut tx = g.begin_write().unwrap();
    tx.create_rel(ids[0], ids[8], "K", Props::new()).unwrap();
    tx.commit().unwrap();
    for _ in 0..3 {
        body(&g, ids[0]);
    }
    let warm = g.inner.tier.warm_keys();
    for i in 8..12u64 {
        assert!(
            warm.contains(&CacheKey::Node(i)),
            "node {i} not prefetched: {warm:?}"
        );
    }
    let before = g.tier_stats().unwrap();
    assert_eq!(body(&g, ids[9]), Some(text(9)));
    let after = g.tier_stats().unwrap();
    assert_eq!(after.warm.hits, before.warm.hits + 1);
    assert_eq!(after.disk_reads, before.disk_reads);
}

fn trace(o: &Options) -> (TierStats, Vec<Event>) {
    let g = Graph::open_in_memory_with(o).unwrap();
    let ids = make(&g, 40);
    let mut rng = SplitMix(5);
    for _ in 0..600 {
        let i = (rng.below(40) * rng.below(40) / 40) as usize;
        assert_eq!(body(&g, ids[i]), Some(text(i)));
    }
    (g.tier_stats().unwrap(), g.placement_log())
}

#[test]
fn the_default_community_is_zero() {
    let base = planned(6, 1 << 16, 1 << 16, 4, 8);
    let explicit = Options {
        communities: Some(Arc::new(vec![0; 64])),
        ..base.clone()
    };
    let (a, la) = trace(&base);
    let (b, lb) = trace(&explicit);
    assert_eq!(a, b);
    assert_eq!(la, lb);
    assert!(!la.is_empty());
    let split = Options {
        communities: Some(Arc::new((0..64).collect())),
        ..base
    };
    let (c, _) = trace(&split);
    assert!(c.disk_reads > 0);
}

fn differential(seed: u64, region_ids: usize, plan_every: usize) {
    let g = Graph::open_in_memory_with(&planned(3, 2048, 1024, region_ids, plan_every)).unwrap();
    let mut model: HashMap<NodeId, String> = HashMap::new();
    let mut edges: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut live: Vec<NodeId> = Vec::new();
    let mut rng = SplitMix(seed);
    for step in 0..2000usize {
        match rng.below(8) {
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
                let id = live[rng.below(live.len() as u64) as usize];
                let t = format!("updated at {step} ").repeat(1 + rng.below(20) as usize);
                let mut tx = g.begin_write().unwrap();
                tx.set_node_props(id, p(&[("body", Value::from(t.clone()))]))
                    .unwrap();
                if rng.below(5) == 0 {
                    tx.rollback();
                } else {
                    tx.commit().unwrap();
                    model.insert(id, t);
                }
            }
            4 if live.len() > 1 => {
                let i = rng.below(live.len() as u64) as usize;
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
                let a = live[rng.below(live.len() as u64) as usize];
                let b = live[rng.below(live.len() as u64) as usize];
                let mut tx = g.begin_write().unwrap();
                tx.create_rel(a, b, "K", Props::new()).unwrap();
                tx.commit().unwrap();
                edges.entry(a).or_default().push(b);
            }
            _ => {
                if live.is_empty() {
                    continue;
                }
                let id = live[rng.below(live.len() as u64) as usize];
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
    assert!(!g.placement_log().is_empty());
    assert!(g.tier_stats().unwrap().hot.promotions > 0, "seed {seed:#x}");
}

#[test]
fn planned_differential_against_a_hashmap_model_over_region_and_interval_shapes() {
    for (r, n) in [(1usize, 1usize), (4, 7), (16, 64), (64, 3)] {
        for seed in [
            0x9e37_79b9_7f4a_7c15u64,
            0x2545_f491_4f6c_dd1d,
            0xdead_beef_1234_5678,
            0x0123_4567_89ab_cdef,
            0xfeed_face_cafe_f00d,
        ] {
            differential(seed, r, n);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Read(u64),
    Out(u64),
    In(u64),
    Scan(u64),
    Write(u64, u32),
}

const NODES: u64 = 400;
const INTERVAL: usize = 50;

fn built(o: &Options) -> Graph {
    let g = Graph::open_in_memory_with(o).unwrap();
    let mut rng = SplitMix(0xabc);
    let mut tx = g.begin_write().unwrap();
    let ids: Vec<NodeId> = (0..NODES as usize)
        .map(|i| {
            tx.create_node(&["Doc"], p(&[("body", Value::from(text(i)))]))
                .unwrap()
        })
        .collect();
    for i in 0..NODES {
        for _ in 0..2 {
            let j = (i + 1 + rng.below(24)) % NODES;
            tx.create_rel(ids[i as usize], ids[j as usize], "K", Props::new())
                .unwrap();
        }
    }
    tx.commit().unwrap();
    g
}

fn workload(n: usize, seed: u64) -> Vec<Op> {
    let mut rng = SplitMix(seed);
    (0..n)
        .map(|_| {
            let skew = rng.below(NODES) * rng.below(NODES) / NODES;
            match rng.below(20) {
                0..=10 => Op::Read(skew),
                11..=13 => Op::Out(skew),
                14 => Op::In(skew),
                15..=17 => Op::Scan(rng.below(NODES)),
                _ => Op::Write(skew, rng.below(1000) as u32),
            }
        })
        .collect()
}

fn exec(g: &Graph, op: Op) {
    let r = g.begin_read().unwrap();
    match op {
        Op::Read(i) => {
            r.get_node(NodeId(i)).unwrap().unwrap();
        }
        Op::Scan(i) => {
            r.scan_node(NodeId(i)).unwrap().unwrap();
        }
        Op::Out(i) => {
            r.out_edges(NodeId(i), None).unwrap().count();
        }
        Op::In(i) => {
            r.in_edges(NodeId(i), None).unwrap().count();
        }
        Op::Write(..) => {}
    }
}

type Snapshot = (
    TierStats,
    Vec<CacheKey>,
    Vec<CacheKey>,
    Vec<u32>,
    Vec<Event>,
);

fn replay(ops: &[Op], threads: usize) -> Snapshot {
    let g = built(&planned(48, 1 << 20, 1 << 20, 8, INTERVAL));
    let mut i = 0;
    while i < ops.len() {
        let w = INTERVAL - g.inner.tier.pending_ops();
        let end = (i + w).min(ops.len());
        let win = &ops[i..end];
        for op in win {
            if let Op::Write(id, tag) = *op {
                let mut tx = g.begin_write().unwrap();
                tx.set_node_props(NodeId(id), p(&[("body", Value::from(format!("w{tag}")))]))
                    .unwrap();
                tx.commit().unwrap();
            }
        }
        let reads: Vec<Op> = win
            .iter()
            .copied()
            .filter(|o| !matches!(o, Op::Write(..)))
            .collect();
        std::thread::scope(|s| {
            for t in 0..threads {
                let (g, reads) = (&g, &reads);
                s.spawn(move || {
                    for (j, op) in reads.iter().enumerate() {
                        if j % threads == t {
                            exec(g, *op);
                        }
                    }
                });
            }
        });
        i = end;
    }
    (
        g.tier_stats().unwrap(),
        g.inner.tier.hot_keys(),
        g.inner.tier.warm_keys(),
        g.placement_hot_regions(),
        g.placement_log(),
    )
}

#[test]
fn replaying_one_event_log_gives_identical_tiers_for_one_and_eight_reader_threads() {
    let ops = workload(5000, 0x5ea1);
    let recorded = replay(&ops, 1);
    let (stats, hot, warm, regions, log) = &recorded;
    assert!(stats.hot.hits > 0 && stats.warm.hits > 0, "{stats:?}");
    assert!(
        stats.hot.promotions > 0 && stats.warm.promotions > 0,
        "{stats:?}"
    );
    assert!(stats.hot.misses > 0 && stats.warm.misses > 0, "{stats:?}");
    assert!(!hot.is_empty() && !warm.is_empty() && !regions.is_empty());
    assert!(log.len() >= 5000, "{}", log.len());
    for threads in [8usize, 1, 8] {
        let again = replay(&ops, threads);
        assert_eq!(
            again.0, recorded.0,
            "tier_stats differ at {threads} threads"
        );
        assert_eq!(
            again.1, recorded.1,
            "T0 membership differs at {threads} threads"
        );
        assert_eq!(
            again.2, recorded.2,
            "T1 membership differs at {threads} threads"
        );
        assert_eq!(
            again.3, recorded.3,
            "hot regions differ at {threads} threads"
        );
        assert!(
            again.4 == recorded.4,
            "event log differs at {threads} threads"
        );
    }
    let other = replay(&workload(5000, 0x5ea2), 1);
    assert_ne!(other.0, recorded.0);
}

fn hot_set_through_scan(gate: bool) -> (usize, u64, u64) {
    let resident_hot = |g: &Graph| {
        g.inner
            .tier
            .hot_keys()
            .iter()
            .filter(|k| matches!(k, CacheKey::Node(i) if *i < 100))
            .count()
    };
    const HOT: u64 = 100;
    const COLD: u64 = 100_000;
    let o = Options {
        admission_gate: gate,
        ..planned(128, 1 << 30, 0, 4, 512)
    };
    let g = Graph::open_in_memory_with(&o).unwrap();
    for chunk in 0..((HOT + COLD) / 10_000 + 1) {
        let mut tx = g.begin_write().unwrap();
        for i in (chunk * 10_000)..((chunk + 1) * 10_000).min(HOT + COLD) {
            tx.create_node(&["N"], p(&[("i", Value::Int(i as i64))]))
                .unwrap();
        }
        tx.commit().unwrap();
    }
    let r = g.begin_read().unwrap();
    for _ in 0..10 {
        for k in 0..HOT {
            r.get_node(NodeId(k)).unwrap();
        }
    }
    let (mut cursor, mut hits, mut reads, mut lowest) = (0u64, 0u64, 0u64, usize::MAX);
    for s in 0..COLD {
        if s % 50 == 0 {
            lowest = lowest.min(resident_hot(&g));
        }
        r.scan_node(NodeId(HOT + s)).unwrap();
        if s % 2 == 0 {
            let before = g.inner.tier.counters(0, 0).hot.hits;
            r.get_node(NodeId(cursor % HOT)).unwrap();
            hits += g.inner.tier.counters(0, 0).hot.hits - before;
            reads += 1;
            cursor += 1;
        }
    }
    (lowest, hits, reads)
}

#[test]
fn a_100k_node_scan_keeps_a_100_node_hot_set_with_the_gate_and_flushes_it_without() {
    let (kept, g_hits, reads) = hot_set_through_scan(true);
    let (flushed, p_hits, _) = hot_set_through_scan(false);
    eprintln!("SCAN PAIR (core, Planned): lowest resident hot nodes {kept}/100 gate on, {flushed}/100 always-admit; hot-read hits {g_hits}/{reads} vs {p_hits}/{reads}");
    assert!(kept >= 90, "gate on dropped to {kept}/100 resident");
    assert!(g_hits * 100 >= reads * 95, "gate on hit {g_hits}/{reads}");
    assert!(
        flushed <= 5,
        "always-admit never flushed the hot set (lowest {flushed}/100), the pair proves nothing"
    );
    assert!(
        p_hits * 100 <= reads * 60,
        "always-admit hit {p_hits}/{reads}"
    );
}
#[test]
#[ignore = "measurement"]
fn measure_planned_against_local() {
    use super::tier::{corpus, windows, Zipf};
    const N: usize = 20_000;
    const READS: usize = 200_000;
    let corpus = corpus();
    let docs = windows(&corpus, N, 1024);
    let (_d, path) = super::tmp_graph_path();
    {
        let g = Graph::open_with(&path, &Options::default()).unwrap();
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
    println!(
        "N={N}, hot_entries=1000, warm off, Zipf s=1.0, {READS} Zipf reads; oracle ceiling {:.1}%",
        Zipf::new(N, 1.0, 1).cdf[999] * 100.0
    );
    let configs: Vec<(String, Options)> = {
        let base = |placement, gate, region_ids, plan_every| Options {
            hot_entries: 1000,
            hot_bytes: 1 << 30,
            warm_bytes: 0,
            admission_gate: gate,
            placement,
            region_ids,
            plan_every,
            ..Options::default()
        };
        vec![
            ("Local gate on".into(), base(Placement::Local, true, 1, 1)),
            ("Local gate off".into(), base(Placement::Local, false, 1, 1)),
            (
                "Planned R=1 N=128 gate on".into(),
                base(Placement::Planned, true, 1, 128),
            ),
            (
                "Planned R=1 N=128 gate off".into(),
                base(Placement::Planned, false, 1, 128),
            ),
            (
                "Planned R=16 N=128 gate on".into(),
                base(Placement::Planned, true, 16, 128),
            ),
            (
                "Planned R=16 N=128 gate off".into(),
                base(Placement::Planned, false, 16, 128),
            ),
        ]
    };
    println!(
        "{:<28} {:>10} {:>14} {:>14}",
        "config", "zipf only", "zipf+1 scan", "zipf+20k burst"
    );
    for (name, o) in &configs {
        let mut cols = Vec::new();
        for (scan_every, burst) in [(0usize, false), (1, false), (0, true)] {
            let g = Graph::open_with(&path, o).unwrap();
            let mut z = Zipf::new(N, 1.0, 0x1234_5678_9abc_def1);
            let (mut hits, mut reads, mut scan_pos) = (0u64, 0u64, 0usize);
            let r = g.begin_read().unwrap();
            for k in 0..READS {
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
                    r.scan_node(NodeId((scan_pos % N) as u64)).unwrap();
                    scan_pos += 1;
                }
            }
            cols.push(hits as f64 / reads as f64 * 100.0);
        }
        println!(
            "{:<28} {:>9.1}% {:>13.1}% {:>13.1}%",
            name, cols[0], cols[1], cols[2]
        );
    }
    const SET: usize = 1000;
    const BATCHES: usize = 1000;
    const PER: usize = 1000;
    println!("read latency, {SET} distinct ids, {BATCHES} batches of {PER} random reads, median ns per read");
    let ids: Vec<NodeId> = (0..SET).map(|i| NodeId((i * 19) as u64)).collect();
    let median = |o: &Options| {
        let g = Graph::open_with(&path, o).unwrap();
        let r = g.begin_read().unwrap();
        for _ in 0..3 {
            for id in &ids {
                r.get_node(*id).unwrap();
            }
        }
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut ns = Vec::with_capacity(BATCHES);
        for _ in 0..BATCHES {
            let mut picked = Vec::with_capacity(PER);
            for _ in 0..PER {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                picked.push(ids[(state % SET as u64) as usize]);
            }
            let t = std::time::Instant::now();
            for id in &picked {
                std::hint::black_box(r.get_node(*id).unwrap());
            }
            ns.push(t.elapsed().as_nanos() as u64 / PER as u64);
        }
        ns.sort_unstable();
        ns[BATCHES / 2]
    };
    let local = median(&Options::default());
    println!("Local                      {local} ns");
    for (r, n) in [(1usize, 128usize), (16, 128), (16, 1024)] {
        let o = Options {
            placement: Placement::Planned,
            region_ids: r,
            plan_every: n,
            ..Options::default()
        };
        let m = median(&o);
        println!(
            "Planned R={r} N={n:<5}        {m} ns  (overhead {} ns/op)",
            m as i64 - local as i64
        );
    }
}
