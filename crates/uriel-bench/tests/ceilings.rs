use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use uriel_bench::hot::{alloc_many, arena, edges};
use uriel_bench::placement::{events, placer, warmed_placer};
use uriel_bench::query::{engine, run, POINT_LOOKUP, TWO_HOP};
use uriel_bench::retrieval::{corpus, search_options, QUERY};
use uriel_bench::rng::Rng;
use uriel_bench::store::{open_store, tier_options, warm, Tier};
use uriel_bench::table::table;
use uriel_bench::topo::{cloud, DIM};
use uriel_bench::zipf;
use uriel_mcp::compact::encode_table;
use uriel_sys::HotCsr;
use uriel_tier::{stable_hash, Gate, Sketch};
use uriel_topo::persistence::{persistence_f64, PersistenceConfig};
use serde_json::{json, Value};

const RUNS: usize = 5;
const FACTOR: f64 = 5.0;
const HASHES: usize = 1 << 16;
const EVENTS: usize = 1 << 16;

fn ceilings_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("ceilings.json")
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

fn median_ns(iters: usize, mut op: impl FnMut()) -> f64 {
    for _ in 0..iters / 10 + 1 {
        op();
    }
    let mut samples: Vec<f64> = (0..RUNS)
        .map(|_| {
            let started = Instant::now();
            for _ in 0..iters {
                op();
            }
            started.elapsed().as_nanos() as f64 / iters as f64
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    samples[RUNS / 2]
}

fn hashes() -> Vec<u64> {
    zipf::sequence(20_000, 1.0, HASHES, 3)
        .into_iter()
        .map(|k| stable_hash(&(k as u64)))
        .collect()
}

fn measure_tiers(out: &mut Vec<(String, f64)>) {
    for (name, tier, iters) in [
        ("tier_read_t0", Tier::T0, 20_000),
        ("tier_read_t1", Tier::T1, 5_000),
        ("tier_read_t2", Tier::T2, 2_000),
    ] {
        let store = open_store(&tier_options(tier), 64);
        warm(&store, 2);
        let tx = store.reader();
        let mut k = 0usize;
        let ns = median_ns(iters, || {
            k = (k + 1) % 64;
            black_box(tx.get_node(store.ids[k]).expect("read"));
        });
        out.push((name.to_string(), ns));
    }
}

fn measure_policy(out: &mut Vec<(String, f64)>) {
    let hashes = hashes();
    let mut sketch = Sketch::new(4096);
    let mut k = 0usize;
    let ns = median_ns(500_000, || {
        k = (k + 1) & (HASHES - 1);
        sketch.increment(black_box(hashes[k]));
    });
    out.push(("sketch_increment".to_string(), ns));

    let mut gate = Gate::new(4096);
    for h in &hashes {
        gate.record_hash(*h);
    }
    let mut k = 0usize;
    let ns = median_ns(500_000, || {
        k = (k + 1) & (HASHES - 1);
        black_box(gate.admit_hash(hashes[k], hashes[(k + 1) & (HASHES - 1)]));
    });
    out.push(("gate_admit".to_string(), ns));
}

fn measure_place(out: &mut Vec<(String, f64)>) {
    let evs = events(1_000, EVENTS, 5);
    let mut p = placer(1_000);
    let mut k = 0usize;
    let ns = median_ns(200_000, || {
        k = (k + 1) & (EVENTS - 1);
        p.apply(black_box(evs[k]));
    });
    out.push(("place_apply_1000".to_string(), ns));

    let mut p = warmed_placer(1_000);
    let ns = median_ns(20, || {
        black_box(p.plan());
    });
    out.push(("place_plan_1000".to_string(), ns));
}

fn measure_topo(out: &mut Vec<(String, f64)>) {
    let data = cloud(500, DIM, 1);
    let cfg = PersistenceConfig::default();
    let ns = median_ns(5, || {
        black_box(persistence_f64(&data, DIM, None, &cfg).expect("persistence"));
    });
    out.push(("topo_persistence_500".to_string(), ns));
}

fn measure_retrieval(out: &mut Vec<(String, f64)>) {
    let corpus = corpus(20);
    let opts = search_options(8, 4000);
    let ns = median_ns(100, || {
        black_box(corpus.rag.search(QUERY, &opts).expect("search"));
    });
    out.push(("rag_search_20_files".to_string(), ns));
}

fn measure_query(out: &mut Vec<(String, f64)>) {
    let engine = engine(2_000);
    for (name, text) in [
        ("query_point_lookup", POINT_LOOKUP),
        ("query_two_hop_traversal", TWO_HOP),
    ] {
        let mut k = 0usize;
        let ns = median_ns(300, || {
            k = (k + 37) % 2_000;
            black_box(run(&engine, text, k));
        });
        out.push((name.to_string(), ns));
    }
}

fn measure_compact(out: &mut Vec<(String, f64)>) {
    let table = table(200);
    let ns = median_ns(30, || {
        black_box(encode_table(&table));
    });
    out.push(("compact_encode_table_200_rows".to_string(), ns));
}

fn measure_sys(out: &mut Vec<(String, f64)>) {
    let nodes = 100_000;
    let mut arena_a = arena();
    let csr = HotCsr::build(&mut arena_a, nodes, &edges(nodes, 5, 9)).expect("build csr");
    let mut rng = Rng::new(21);
    let ns = median_ns(1_000_000, || {
        black_box(csr.neighbours(rng.below(nodes) as u32).len());
    });
    out.push(("hot_csr_neighbours_100k".to_string(), ns));

    let mut arena_b = arena();
    let ns = median_ns(200, || {
        black_box(alloc_many(&mut arena_b, 1024, 64));
    });
    out.push(("arena_alloc_1024_slabs_and_reset".to_string(), ns));
}

fn measure_all() -> Vec<(String, f64)> {
    let mut out = Vec::new();
    measure_tiers(&mut out);
    measure_policy(&mut out);
    measure_place(&mut out);
    measure_topo(&mut out);
    measure_retrieval(&mut out);
    measure_query(&mut out);
    measure_compact(&mut out);
    measure_sys(&mut out);
    out
}

fn load_calibrated() -> Option<Value> {
    let text = fs::read_to_string(ceilings_path()).ok()?;
    let doc: Value = serde_json::from_str(&text).ok()?;
    (doc.get("calibrated").and_then(Value::as_bool) == Some(true)).then_some(doc)
}

#[test]
fn operations_stay_under_five_times_the_calibrated_median() {
    if std::env::var("SEAL_BENCH_SKIP").as_deref() == Ok("1") {
        eprintln!("SEAL_BENCH_SKIP=1: latency ceilings skipped");
        return;
    }
    let measured = measure_all();
    let existing = load_calibrated();
    let mut dirty = existing.is_none();
    let mut doc = existing.unwrap_or_else(|| json!({ "calibrated": true, "factor": FACTOR }));
    let mut section = doc
        .get(profile())
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut failures = Vec::new();
    for (name, ns) in &measured {
        match section.get(name).and_then(Value::as_f64) {
            Some(ceiling) => {
                if *ns > ceiling {
                    failures.push(format!(
                        "{name}: median {ns:.1} ns exceeds ceiling {ceiling:.1} ns"
                    ));
                }
            }
            None => {
                let ceiling = (ns * FACTOR * 1000.0).round() / 1000.0;
                section.insert(name.clone(), json!(ceiling));
                dirty = true;
            }
        }
    }
    if dirty {
        doc[profile()] = Value::Object(section);
        let text = serde_json::to_string_pretty(&doc).expect("serialise ceilings");
        fs::write(ceilings_path(), format!("{text}\n")).expect("write ceilings.json");
        eprintln!(
            "ceilings calibrated for the {} profile at {FACTOR}x the measured median",
            profile()
        );
    }
    assert!(
        failures.is_empty(),
        "latency regressions:\n{}",
        failures.join("\n")
    );
}
