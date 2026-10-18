#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use uriel_core::MemoryBudget;
use uriel_host::{Client, Host, HostConfig, PipeTransport, SearchOptions, ServeConfig, DEFAULT_MAX_FRAME};
use uriel_sys::process_memory;

const MIB: f64 = 1024.0 * 1024.0;
const PEAK_LIMIT_MIB: f64 = 60.0;
const QUERIES: [&str; 20] = [
    "memory budget cache capacity",
    "graph node label property",
    "transaction commit write batch",
    "tier promotion eviction policy",
    "hash embedding vector dimension",
    "path walk directory skip",
    "error type conversion display",
    "iterator over adjacency list",
    "serialize value to bytes",
    "lock poisoned mutex guard",
    "index lookup by label",
    "epoch counter increments",
    "arena slab allocation",
    "decode frame length prefix",
    "relationship type direction",
    "property map btree",
    "read transaction snapshot",
    "node id allocation",
    "string interner symbol",
    "cache admission frequency sketch",
];

fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB
}

fn peak_working_set() -> u64 {
    let script = format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id());
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .expect("run powershell");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("PeakWorkingSet64 is a number")
}

fn count_files(root: &Path) -> usize {
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "target" && n != ".git") {
                    stack.push(path);
                }
            } else {
                total += 1;
            }
        }
    }
    total
}

fn root_from_env() -> PathBuf {
    match std::env::var("URIEL_MEMORY_ROOT") {
        Ok(path) => PathBuf::from(path),
        Err(_) => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("uriel-core"),
    }
}

fn budget_from_env() -> usize {
    std::env::var("URIEL_MEMORY_MIB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64)
}

#[test]
fn host_rss_idle_after_indexing_and_after_one_hundred_searches_stays_under_the_peak_limit() {
    let root = root_from_env().canonicalize().expect("root exists");
    let budget_mib = budget_from_env();
    let files_on_disk = count_files(&root);
    let dir = tempfile::tempdir().expect("tempdir");
    let control = process_memory().expect("control");

    let host = Host::open(HostConfig {
        db_path: dir.path().join("memory.redb"),
        root: root.clone(),
        hot_cap_bytes: 1 << 20,
        memory: MemoryBudget::mib(budget_mib),
        sock_name: format!("uriel-memory-{}", std::process::id()),
    })
    .expect("open");
    host.serve(ServeConfig::default()).expect("serve");
    let pipe = host.listen_pipe().expect("listen");
    let idle_open = process_memory().expect("idle after open");

    let transport = PipeTransport::connect(&pipe, DEFAULT_MAX_FRAME).expect("connect");
    let mut client = Client::connect(transport).expect("client");
    let idle = process_memory().expect("idle");

    let report = client.index(&root.to_string_lossy()).expect("index");
    let indexed = process_memory().expect("after index");

    let opts = SearchOptions::default();
    let mut answered = 0usize;
    for round in 0..100 {
        let pack = client
            .search(QUERIES[round % QUERIES.len()], &opts)
            .expect("search");
        answered += usize::from(!pack.hits.is_empty());
    }
    let searched = process_memory().expect("after searches");
    let stats = client.stats().expect("stats");
    let peak = peak_working_set();

    println!(
        "MEMORY root={} files_on_disk={files_on_disk} files_indexed={} chunks_added={} chunks={} budget_mib={budget_mib} answered={answered}/100",
        root.display(),
        report.files_indexed,
        report.chunks_added,
        stats.chunks,
    );
    println!(
        "MEMORY control_rss={:.1} idle_after_open={:.1} idle_with_client={:.1} after_index={:.1} after_100_searches={:.1} peak={:.1} private_after_searches={:.1} (MiB)",
        mib(control.working_set),
        mib(idle_open.working_set),
        mib(idle.working_set),
        mib(indexed.working_set),
        mib(searched.working_set),
        mib(peak),
        mib(searched.private_bytes),
    );
    assert_eq!(report.files_indexed + report.files_skipped + report.files_unchanged, report.files_scanned);
    assert!(answered >= 90, "only {answered} of 100 searches returned hits");
    assert!(
        mib(peak) < PEAK_LIMIT_MIB,
        "peak working set {:.1} MiB is not under {PEAK_LIMIT_MIB} MiB",
        mib(peak)
    );
}
