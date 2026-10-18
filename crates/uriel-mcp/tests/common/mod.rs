#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(60);

const FORMAT_TOOLS: &[&str] = &[
    "rag_search",
    "seal_recall",
    "read_neo4j_cypher",
    "view",
    "soul",
    "why_hot",
    "calibrate",
    "replay",
    "tier_stats",
    "pin",
    "unpin",
];

fn json_format(name: &str, mut args: Value) -> Value {
    if FORMAT_TOOLS.contains(&name) {
        if let Some(obj) = args.as_object_mut() {
            obj.entry("format").or_insert_with(|| json!("json"));
        }
    }
    args
}

pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    next_id: u64,
    pub home: TempDir,
}

impl Session {
    pub fn start(args: &[String], envs: &[(&str, &str)]) -> Session {
        let home = tempfile::tempdir().expect("home");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_uriel-mcp"));
        cmd.args(args)
            .env("USERPROFILE", home.path())
            .env("HOME", home.path())
            .env_remove("SEAL_DR_HOUSE_MODEL")
            .env_remove("URIEL_MODE")
            .env_remove("SEAL_MCP_FAKE_PROBE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn uriel-mcp");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
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
        Session {
            child,
            stdin,
            rx,
            next_id: 1,
            home,
        }
    }

    pub fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string();
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(line.as_bytes()).expect("write");
        stdin.write_all(b"\n").expect("write");
        stdin.flush().expect("flush");
        let resp = self
            .rx
            .recv_timeout(TIMEOUT)
            .expect("timed out waiting for a response");
        let v: Value =
            serde_json::from_str(&resp).unwrap_or_else(|e| panic!("not JSON ({e}): {resp}"));
        assert_eq!(v["id"], json!(id), "{v}");
        v
    }

    pub fn tool(&mut self, name: &str, args: Value) -> (String, bool) {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        assert!(res.is_object(), "expected a result for {name}, got {r}");
        (
            res["content"][0]["text"]
                .as_str()
                .expect("text")
                .to_string(),
            res["isError"].as_bool().expect("isError"),
        )
    }

    pub fn text(&mut self, name: &str, args: Value) -> String {
        let (t, e) = self.tool(name, args);
        assert!(!e, "{name} failed: {t}");
        t
    }

    pub fn ok(&mut self, name: &str, args: Value) -> Value {
        let t = self.text(name, json_format(name, args));
        serde_json::from_str(&t).unwrap_or_else(|e| panic!("{name} returned non-JSON ({e}): {t}"))
    }

    pub fn err(&mut self, name: &str, args: Value) -> String {
        let (t, e) = self.tool(name, args);
        assert!(e, "{name} unexpectedly succeeded: {t}");
        t
    }

    pub fn names(&mut self) -> Vec<String> {
        self.rpc("tools/list", json!({}))["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|t| t["name"].as_str().expect("name").to_string())
            .collect()
    }

    pub fn finish(mut self) -> bool {
        drop(self.stdin.take());
        self.child.wait().expect("wait").success()
    }
}

pub struct Proj {
    pub dir: TempDir,
}

fn write(dir: &TempDir, rel: &str, body: &str) {
    let p = dir.path().join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

impl Proj {
    pub fn new() -> Proj {
        let dir = tempfile::tempdir().expect("project");
        write(
            &dir,
            "src/backoff.rs",
            "/// Exponential backoff delay for the payment gateway retry loop.\n\
             pub fn backoff_delay(attempt: u32, ceiling_ms: u64) -> u64 {\n    \
             let raw = 100u64.saturating_mul(1u64 << attempt.min(20));\n    \
             raw.min(ceiling_ms)\n}\n",
        );
        write(
            &dir,
            "src/invoice.rs",
            "/// Renders the PDF invoice with the customer billing address and the tax lines.\n\
             pub fn render_invoice(customer: &str) -> String {\n    \
             format!(\"invoice for {customer}\")\n}\n",
        );
        write(
            &dir,
            "src/settings.rs",
            "/// Parses the TOML configuration file and validates the cache directory.\n\
             pub fn parse_settings(text: &str) -> usize {\n    text.lines().count()\n}\n",
        );
        write(
            &dir,
            "docs/alpha.md",
            "# Scheduler alpha\nThe scheduler picks the next job by priority and age.\n",
        );
        write(
            &dir,
            "docs/beta.md",
            "# Scheduler beta\nThe scheduler chooses the next job by priority and queue age.\n",
        );
        for f in ["a", "b", "c"] {
            write(&dir, &format!("src/{f}.rs"), f);
        }
        Proj { dir }
    }

    pub fn root(&self) -> String {
        self.dir.path().to_string_lossy().into_owned()
    }

    pub fn events(&self) -> PathBuf {
        self.dir.path().join("house-events.jsonl")
    }

    pub fn db(&self) -> String {
        self.dir
            .path()
            .join("graph.seal")
            .to_string_lossy()
            .into_owned()
    }

    pub fn session(&self, extra: &[&str], envs: &[(&str, &str)]) -> Session {
        let mut args = vec!["--root".to_string(), self.root()];
        if !extra.contains(&"--db") {
            args.push("--in-memory".into());
        }
        if !extra.iter().any(|a| a.starts_with("--mode")) {
            args.extend(["--mode".to_string(), "house".to_string()]);
        }
        args.extend(extra.iter().map(|s| s.to_string()));
        Session::start(&args, envs)
    }

    pub fn event_lines(&self) -> Vec<Value> {
        std::fs::read_to_string(self.events())
            .unwrap_or_default()
            .lines()
            .map(|l| {
                serde_json::from_str(l).unwrap_or_else(|e| panic!("event not JSON ({e}): {l}"))
            })
            .collect()
    }
}

pub fn branch(id: &str, parent: &str) -> Value {
    json!({"id": id, "parent": parent, "text": format!("part {id}"), "leaf": false})
}

pub fn test_name(id: &str) -> String {
    format!("t_{}", id.replace('.', "_"))
}

pub fn leaf(id: &str, parent: &str) -> Value {
    json!({
        "id": id, "parent": parent, "text": format!("do {id}"), "leaf": true,
        "tier": "sonnet", "class": "code", "reads": ["src/a.rs"],
        "test": {"name": test_name(id), "cmd": format!("cargo test -p x {}", test_name(id)), "class": "test_unit"},
        "est": {"ram_mb": 800, "secs": 60, "tokens": 10000}
    })
}

pub fn nurse(id: &str, parent: &str) -> Value {
    json!({
        "id": id, "parent": parent, "text": format!("scan {id}"), "leaf": true,
        "tier": "haiku", "class": "grep", "reads": ["src/a.rs"],
        "test": {"name": test_name(id), "cmd": format!("cargo test -p x {}", test_name(id)), "class": "grep"},
        "est": {"ram_mb": 50, "secs": 5, "tokens": 2000}
    })
}

pub fn guard(id: &str, target: &str) -> Value {
    let mut g = leaf(id, "G0");
    g.as_object_mut().expect("object").remove("parent");
    g["guards"] = json!(target);
    g
}

pub fn alternative(id: &str, target: &str, tokens: u64) -> Value {
    let mut g = leaf(id, "G0");
    g.as_object_mut().expect("object").remove("parent");
    g["alt_of"] = json!(target);
    g["est"]["tokens"] = json!(tokens);
    g
}

pub fn chart(goals: Vec<Value>) -> Value {
    json!({"need": "G0", "goals": goals})
}

pub fn tiny() -> Vec<Value> {
    vec![leaf("G1", "G0"), leaf("G2", "G0")]
}

pub fn base() -> Vec<Value> {
    vec![
        branch("G1", "G0"),
        leaf("G1.1", "G1"),
        leaf("G1.2", "G1"),
        branch("G2", "G0"),
        leaf("G2.1", "G2"),
        leaf("G2.2", "G2"),
    ]
}

pub fn need(s: &mut Session) {
    s.ok(
        "house_need",
        json!({"project": "proj", "goal": "build x", "acceptance": ["it works"]}),
    );
}

pub fn chart_and_merge(s: &mut Session, goals: Vec<Value>) {
    s.ok(
        "house_chart",
        json!({"fellow": "Foreman", "chart": chart(goals)}),
    );
    s.ok("house_merge", json!({}));
}

pub fn prove(s: &mut Session, goal: &str) -> Value {
    let t = test_name(goal);
    s.ok(
        "house_test_run",
        json!({"test": t, "status": "red", "secs": 1.0, "peak_ram_mb": 100}),
    );
    s.ok(
        "house_test_run",
        json!({"test": t, "status": "green", "secs": 1.0, "peak_ram_mb": 100}),
    );
    s.ok(
        "house_report",
        json!({"goal": goal, "status": "reached", "evidence": {"test": t, "status": "green"}}),
    )
}

pub fn leap_ready(s: &mut Session) {
    need(s);
    chart_and_merge(s, base());
    s.ok(
        "house_fact",
        json!({"fact": {"id": "F1", "by": "Wilson", "refutes": "G2", "source": "src/a.rs:1", "text": "G2 cannot hold"}}),
    );
    for _ in 0..2 {
        chart_and_merge(s, base());
    }
    let st = s.ok("house_status", json!({}));
    assert_eq!(st["leap_candidates"][0], "G2", "{st}");
}
