use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};
use sysinfo::{Pid, ProcessesToUpdate, System};
use uriel_core::{Graph, MemoryBudget, NodeId, Props, Value};
use uriel_rag::{Embedder, HashEmbedder};

const BUDGET_MIB: u64 = 50;
const PEAK_LIMIT_MIB: f64 = 60.0;
const PEAK_TARGET_MIB: f64 = 56.0;
const SEARCHES: usize = 100;
const VIEWS: usize = 20;
const SOULS: usize = 20;
const QUERIES: [&str; 10] = [
    "memory budget cache capacity",
    "transaction commit write batch",
    "tier promotion eviction policy",
    "hash embedding vector dimension",
    "path walk directory skip",
    "error type conversion display",
    "iterator over adjacency list",
    "lock poisoned mutex guard",
    "epoch counter increments",
    "cache admission frequency sketch",
];
const CHUNKS_PER_FILE: usize = 50;
const COMMIT_EVERY_FILES: usize = 40;
const SETTLE: Duration = Duration::from_millis(600);
const TIMEOUT: Duration = Duration::from_secs(600);
const SAMPLE_EVERY: Duration = Duration::from_millis(5);

struct Mix(u64);

impl Mix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn skewed(&mut self, n: u64) -> u64 {
        let a = self.below(n);
        let b = self.below(n);
        a.min(b)
    }
}

const PREFIX: [&str; 40] = [
    "read", "write", "parse", "build", "scan", "merge", "split", "flush", "route", "plan", "sweep",
    "probe", "index", "chunk", "embed", "score", "rank", "fuse", "pack", "trim", "hash", "seal",
    "tier", "place", "evict", "admit", "queue", "drain", "fetch", "store", "check", "stamp", "walk",
    "visit", "bound", "clamp", "mixed", "ready", "close", "reset",
];

const SUFFIX: [&str; 40] = [
    "page", "node", "edge", "rows", "cell", "slot", "span", "line", "token", "term", "list", "map",
    "set", "heap", "ring", "tree", "frame", "block", "batch", "stage", "epoch", "cache", "state",
    "guard", "table", "cursor", "buffer", "window", "region", "bucket", "ledger", "vector", "digest",
    "policy", "budget", "metric", "sketch", "filter", "column", "record",
];

fn word(m: &mut Mix) -> String {
    let p = PREFIX[m.skewed(40) as usize];
    let s = SUFFIX[m.skewed(40) as usize];
    format!("{p}_{s}")
}

fn chunk_text(m: &mut Mix) -> String {
    let mut out = String::new();
    for _ in 0..9 {
        let name = word(m);
        let arg = word(m);
        let a = word(m);
        let b = word(m);
        let c = word(m);
        out.push_str(&format!(
            "pub fn {name}(state: &mut State, {arg}: u64) -> usize {{\n    let {a} = {b}(state, {arg});\n    {c}(state, {a}) + {arg} as usize\n}}\n"
        ));
    }
    out
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn build_db(path: &Path, chunks: usize) {
    let graph = open_graph(path);
    let embedder = HashEmbedder::new(256);
    let mut m = Mix(0xC0FFEE ^ chunks as u64);
    let files = chunks.div_ceil(CHUNKS_PER_FILE);
    let mut tx = graph.begin_write().expect("write");
    let mut made = 0usize;
    for f in 0..files {
        let rel = format!("src/gen/mod_{f}.rs");
        let mut fp = Props::new();
        fp.insert("path".into(), Value::from(rel.as_str()));
        fp.insert("hash".into(), Value::from(format!("{f:064x}").as_str()));
        fp.insert("kind".into(), Value::from("dir"));
        let file = tx.create_node(&["File"], fp).expect("file");
        let mut prev: Option<NodeId> = None;
        for c in 0..CHUNKS_PER_FILE {
            if made == chunks {
                break;
            }
            let text = chunk_text(&mut m);
            let mem = cut(&text, 96);
            let mut p = Props::new();
            p.insert("file".into(), Value::from(rel.as_str()));
            p.insert("start".into(), Value::Int((c * 40 + 1) as i64));
            p.insert("end".into(), Value::Int((c * 40 + 40) as i64));
            p.insert("mem".into(), Value::from(mem.as_str()));
            p.insert("g3".into(), Value::from(cut(&text, 72).as_str()));
            p.insert("l1".into(), Value::from(cut(&text, 180).as_str()));
            p.insert("text".into(), Value::from(text.as_str()));
            p.insert("hash".into(), Value::from(format!("{made:064x}").as_str()));
            p.insert("utility".into(), Value::Float(0.0));
            p.insert("vec".into(), Value::from(embedder.embed(&mem)));
            let id = tx.create_node(&["Chunk"], p).expect("chunk");
            tx.create_rel(id, file, "IN_FILE", Props::new()).expect("in_file");
            if let Some(prev) = prev {
                tx.create_rel(prev, id, "NEXT", Props::new()).expect("next");
            }
            prev = Some(id);
            made += 1;
        }
        if (f + 1) % COMMIT_EVERY_FILES == 0 {
            tx.commit().expect("commit");
            tx = graph.begin_write().expect("write");
        }
    }
    tx.commit().expect("commit");
}

fn open_graph(path: &Path) -> Graph {
    Graph::open(path, MemoryBudget::mib(80)).expect("open graph")
}

struct Probe {
    pid: Pid,
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
}

fn working_set(sys: &mut System, pid: Pid) -> Option<u64> {
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(|p| p.memory())
}

impl Probe {
    fn start(pid: u32) -> Probe {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let (s, p) = (stop.clone(), peak.clone());
        let handle = thread::spawn(move || {
            let mut sys = System::new();
            let pid = Pid::from_u32(pid);
            while !s.load(Ordering::Relaxed) {
                if let Some(ws) = working_set(&mut sys, pid) {
                    p.fetch_max(ws, Ordering::Relaxed);
                }
                thread::sleep(SAMPLE_EVERY);
            }
        });
        Probe {
            pid: Pid::from_u32(pid),
            stop,
            peak,
            handle: Some(handle),
        }
    }

    fn peak(&self) -> u64 {
        let mut sys = System::new();
        if let Some(ws) = working_set(&mut sys, self.pid) {
            self.peak.fetch_max(ws, Ordering::Relaxed);
        }
        self.peak.load(Ordering::Relaxed)
    }

    fn reset(&self) {
        self.peak.store(0, Ordering::Relaxed);
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    next_id: u64,
}

impl Server {
    fn spawn(bin: &Path, db: &Path, root: &Path, extra: &[&str]) -> Server {
        let mut child = Command::new(bin)
            .arg("mcp")
            .arg("--db")
            .arg(db)
            .arg("--root")
            .arg(root)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn uriel mcp");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Server {
            child,
            stdin,
            rx,
            next_id: 1,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, v: &Json) {
        let stdin = self.stdin.as_mut().expect("stdin");
        stdin.write_all(v.to_string().as_bytes()).expect("write");
        stdin.write_all(b"\n").expect("write");
        stdin.flush().expect("flush");
    }

    fn rpc(&mut self, method: &str, params: Json) -> Json {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let line = self.rx.recv_timeout(TIMEOUT).expect("response");
        let resp: Json = serde_json::from_str(&line).expect("json");
        assert_eq!(resp["id"], json!(id), "{resp}");
        resp
    }

    fn init(&mut self) {
        self.rpc(
            "initialize",
            json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rss","version":"0"}}),
        );
        self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        self.rpc("tools/list", json!({}));
    }

    fn call(&mut self, name: &str, arguments: Json) -> Json {
        let resp = self.rpc("tools/call", json!({"name":name,"arguments":arguments}));
        assert!(resp["result"]["isError"] != json!(true), "{name} failed: {resp}");
        resp
    }

    fn search(&mut self) -> Json {
        self.rpc(
            "tools/call",
            json!({"name":"rag_search","arguments":{"query":"read_page scan_node","k":8,"budget_tokens":1500}}),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Clone, Copy)]
struct Reading {
    idle: u64,
    after_search_peak: u64,
    after_search_settled: u64,
    search_ms: u128,
}

fn mib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

fn measure(bin: &Path, db: &Path, root: &Path, extra: &[&str]) -> Reading {
    let mut server = Server::spawn(bin, db, root, extra);
    let probe = Probe::start(server.pid());
    server.init();
    thread::sleep(SETTLE);
    let idle = probe.peak();
    probe.reset();
    let started = Instant::now();
    let resp = server.search();
    let search_ms = started.elapsed().as_millis();
    assert!(resp["result"]["isError"] != json!(true), "rag_search failed: {resp}");
    let after_search_peak = probe.peak();
    probe.reset();
    thread::sleep(SETTLE);
    let after_search_settled = probe.peak();
    Reading {
        idle,
        after_search_peak,
        after_search_settled,
        search_ms,
    }
}

fn db_dir() -> PathBuf {
    match std::env::var_os("URIEL_RSS_DB_DIR") {
        Some(d) => {
            let p = PathBuf::from(d);
            std::fs::create_dir_all(&p).expect("db dir");
            p
        }
        None => {
            let p = std::env::temp_dir().join(format!("uriel-rss-{}", std::process::id()));
            std::fs::create_dir_all(&p).expect("db dir");
            p
        }
    }
}

fn prepared(n: usize) -> PathBuf {
    let path = db_dir().join(format!("synthetic-{n}.uriel"));
    if !path.exists() {
        let tmp = path.with_extension("building");
        let _ = std::fs::remove_file(&tmp);
        build_db(&tmp, n);
        std::fs::rename(&tmp, &path).expect("publish db");
    }
    path
}

fn scratch_copy(db: &Path) -> PathBuf {
    let dir = db_dir().join(format!("run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("run dir");
    let copy = dir.join(db.file_name().expect("name"));
    std::fs::copy(db, &copy).expect("copy db");
    copy
}

fn report(label: &str, n: usize, r: Reading) {
    println!(
        "RSS {label} chunks={n} idle_mib={:.1} after_search_peak_mib={:.1} after_search_settled_mib={:.1} search_ms={}",
        mib(r.idle),
        mib(r.after_search_peak),
        mib(r.after_search_settled),
        r.search_ms
    );
}

struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn gate(n: usize) {
    let owned = std::env::var_os("URIEL_RSS_DB_DIR").is_none();
    let mut cleanup = Cleanup(Vec::new());
    if owned {
        cleanup.0.push(db_dir().join(format!("synthetic-{n}.uriel")));
        cleanup.0.push(db_dir().join(format!("run-{}", std::process::id())).join(format!("synthetic-{n}.uriel")));
    }
    let root = db_dir().join("root");
    std::fs::create_dir_all(&root).expect("root");
    let db = scratch_copy(&prepared(n));
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_uriel"));
    let memory = BUDGET_MIB.to_string();
    let r = measure(&bin, &db, &root, &["--memory-mib", &memory]);
    report("candidate", n, r);
    if let Some(control) = std::env::var_os("URIEL_CONTROL_BIN") {
        let db = scratch_copy(&prepared(n));
        let c = measure(Path::new(&control), &db, &root, &[]);
        report("control", n, c);
    }
    let budget = BUDGET_MIB * 1024 * 1024;
    assert!(r.idle <= budget, "idle {:.1} MiB over {BUDGET_MIB} MiB", mib(r.idle));
    assert!(
        r.after_search_peak <= budget,
        "peak after rag_search {:.1} MiB over {BUDGET_MIB} MiB",
        mib(r.after_search_peak)
    );
    assert!(
        r.after_search_settled <= budget,
        "settled after rag_search {:.1} MiB over {BUDGET_MIB} MiB",
        mib(r.after_search_settled)
    );
}

#[test]
fn rss_at_idle_and_after_one_rag_search_on_an_empty_db_is_within_budget() {
    gate(0);
}

#[test]
fn rss_at_idle_and_after_one_rag_search_on_10k_chunks_is_within_budget() {
    gate(10_000);
}

#[test]
fn rss_at_idle_and_after_one_rag_search_on_100k_chunks_is_within_budget() {
    gate(100_000);
}

#[cfg(windows)]
fn os_peak_working_set(pid: u32) -> u64 {
    let script = format!("(Get-Process -Id {pid}).PeakWorkingSet64");
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .expect("run powershell");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("PeakWorkingSet64 is a number")
}

#[cfg(windows)]
fn full_server_peaks(bin: &Path, tree: &Path) -> Vec<(&'static str, f64)> {
    let dir = db_dir().join(format!("full-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("full dir");
    let db = dir.join("full.uriel");
    let memory = std::env::var("URIEL_RSS_MEMORY_MIB").ok();
    let extra: Vec<&str> = match &memory {
        Some(m) => vec!["--memory-mib", m],
        None => Vec::new(),
    };
    let mut server = Server::spawn(bin, &db, tree, &extra);
    let pid = server.pid();
    let mut peaks = Vec::new();
    server.init();
    thread::sleep(SETTLE);
    peaks.push(("idle", mib(os_peak_working_set(pid))));
    let path = tree.to_string_lossy().to_string();
    server.call("rag_index", json!({ "path": path }));
    peaks.push(("rag_index", mib(os_peak_working_set(pid))));
    for i in 0..SEARCHES {
        server.call(
            "rag_search",
            json!({"query": QUERIES[i % QUERIES.len()], "k": 8, "budget_tokens": 1500, "fresh": true}),
        );
    }
    peaks.push(("rag_search_x100", mib(os_peak_working_set(pid))));
    for i in 0..VIEWS {
        server.call(
            "view",
            json!({"question": QUERIES[i % QUERIES.len()], "budget_tokens": 3000}),
        );
    }
    peaks.push(("view_x20", mib(os_peak_working_set(pid))));
    for i in 0..SOULS {
        server.call("soul", json!({"budget_tokens": 300 + 100 * i}));
    }
    peaks.push(("soul_x20", mib(os_peak_working_set(pid))));
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
    peaks
}

#[cfg(windows)]
fn print_peaks(label: &str, peaks: &[(&str, f64)]) {
    let cells: Vec<String> = peaks.iter().map(|(n, p)| format!("{n}={p:.1}")).collect();
    println!("FULLRSS {label} profile={} peak_mib {}", if cfg!(debug_assertions) { "debug" } else { "release" }, cells.join(" "));
}

#[cfg(windows)]
#[test]
fn full_server_peak_working_set_over_the_workspace_tree_stays_under_60_mib_in_release() {
    let tree = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize().expect("crates dir");
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_uriel"));
    let peaks = full_server_peaks(&bin, &tree);
    print_peaks("candidate", &peaks);
    if let Some(control) = std::env::var_os("URIEL_CONTROL_BIN") {
        let c = full_server_peaks(Path::new(&control), &tree);
        print_peaks("control", &c);
    }
    if !cfg!(debug_assertions) {
        for (stage, peak) in &peaks {
            assert!(*peak <= PEAK_LIMIT_MIB, "peak after {stage} {peak:.1} MiB over {PEAK_LIMIT_MIB} MiB");
            assert!(*peak <= PEAK_TARGET_MIB, "peak after {stage} {peak:.1} MiB eats the 4 MiB margin under {PEAK_LIMIT_MIB} MiB");
        }
    }
}
