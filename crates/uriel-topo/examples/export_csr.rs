use std::fs;
use std::path::PathBuf;

use uriel_topo::export::KernelCsr;
use uriel_topo::persistence::Metric;
use uriel_topo::rng::SplitMix64;
use uriel_topo::schedule::{schedule, Blocks, ScheduleParams};

const BLOCK: usize = 64;
const DIM: usize = 16;
const TOPICS: usize = 8;

struct Case {
    name: &'static str,
    seq: usize,
    local: usize,
    salient: Option<usize>,
    relevant: usize,
}

const CASES: [Case; 6] = [
    Case {
        name: "s1024_l2_k0_r2",
        seq: 1024,
        local: 2,
        salient: Some(0),
        relevant: 2,
    },
    Case {
        name: "s1024_l2_k2_r2",
        seq: 1024,
        local: 2,
        salient: Some(2),
        relevant: 2,
    },
    Case {
        name: "s2048_l3_k0_r2",
        seq: 2048,
        local: 3,
        salient: Some(0),
        relevant: 2,
    },
    Case {
        name: "s2048_l3_k2_r2",
        seq: 2048,
        local: 3,
        salient: Some(2),
        relevant: 2,
    },
    Case {
        name: "all_s1024",
        seq: 1024,
        local: 1,
        salient: None,
        relevant: 0,
    },
    Case {
        name: "all_s2048",
        seq: 2048,
        local: 1,
        salient: None,
        relevant: 0,
    },
];

fn topic_blocks(n: usize, seed: u64) -> (Blocks, Vec<f64>) {
    let mut rng = SplitMix64::new(seed);
    let centers: Vec<f64> = (0..TOPICS * DIM).map(|_| 3.0 * rng.normal()).collect();
    let mut centroids = Vec::with_capacity(n * DIM);
    for _ in 0..n {
        let t = rng.below(TOPICS);
        for d in 0..DIM {
            centroids.push(centers[t * DIM + d] + 0.5 * rng.normal());
        }
    }
    let query: Vec<f64> = (0..DIM).map(|d| centers[d] + 0.5 * rng.normal()).collect();
    let blocks = Blocks::new((0..n as u64).collect(), centroids, DIM, vec![BLOCK; n])
        .expect("generated blocks are valid");
    (blocks, query)
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: export_csr <output dir>"),
    );
    fs::create_dir_all(&dir).expect("create output dir");
    for (i, c) in CASES.iter().enumerate() {
        let n = c.seq / BLOCK;
        let (blocks, query) = topic_blocks(n, 0x5EED + i as u64);
        let params = ScheduleParams {
            sink_blocks: 1,
            local_window: c.local,
            salient_k: c.salient.unwrap_or(n),
            relevant_m: c.relevant,
            token_budget: None,
            metric: Metric::Euclidean,
        };
        let q = (c.relevant > 0).then_some(query.as_slice());
        let sched = schedule(&blocks, &params, q).expect("schedule");
        let csr = KernelCsr::from_schedule(&sched, BLOCK as u32).expect("export");
        let bytes = csr.encode();
        let path = dir.join(format!("{}.ucsr", c.name));
        fs::write(&path, &bytes).expect("write csr");
        println!(
            "{} blocks={} nnz={} dense_nnz={} bytes={}",
            c.name,
            csr.blocks(),
            csr.indices().len(),
            n * (n + 1) / 2,
            bytes.len()
        );
    }
}
