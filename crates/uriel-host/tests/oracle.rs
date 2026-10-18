#![cfg(windows)]

mod common;

use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use common::{fixture, wait_until};
use uriel_host::{Client, Host, PipeTransport, Request, ServeConfig, DEFAULT_MAX_FRAME};

const UTIL_PY: &str = "import json


def parse_config(path):
    with open(path) as handle:
        return json.load(handle)
";

const EXPECTED_TESTS: usize = 23;

fn python_on_path() -> bool {
    Command::new("python")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn within<T: Send + 'static>(seconds: u64, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(Duration::from_secs(seconds))
        .expect("work finished within its watchdog")
}

fn oracle_host(tag: &str) -> (tempfile::TempDir, Host, String) {
    let (dir, config) = fixture(&format!("uriel-oracle-{}-{tag}", std::process::id()));
    std::fs::write(config.root.join("util.py"), UTIL_PY).expect("write util.py");
    let host = Host::open(config).expect("open");
    host.serve(ServeConfig {
        queue_depth: 32,
        workers: 4,
        max_frame: DEFAULT_MAX_FRAME,
    })
    .expect("serve");
    host.set_handler_hook(|request| {
        if let Request::Search { query, .. } = request {
            if query.contains("slow-probe") {
                thread::sleep(Duration::from_millis(400));
            }
        }
    });
    let path = host.listen_pipe().expect("listen pipe");
    (dir, host, path)
}

fn clients_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("clients")
}

fn driver() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("oracle_driver.py")
}

#[test]
fn both_python_adapters_pass_every_shape_and_concurrency_check_against_a_live_host() {
    if !python_on_path() {
        eprintln!("SKIPPED: `python` is not on PATH, the Clara adapters were not exercised");
        return;
    }
    let (dir, host, pipe) = oracle_host("adapters");
    let root = dir.path().join("src");
    let outside = dir.path().to_path_buf();
    let output = within(180, move || {
        Command::new("python")
            .arg("-I")
            .arg(driver())
            .env("URIEL_CLIENTS", clients_dir())
            .env("URIEL_PIPE", pipe)
            .env("URIEL_ROOT", root)
            .env("URIEL_OUTSIDE", outside)
            .output()
            .expect("run python driver")
    });
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "python adapters failed:\n{stderr}");
    assert!(
        stderr.contains(&format!("Ran {EXPECTED_TESTS} tests")),
        "the python driver did not run {EXPECTED_TESTS} tests:\n{stderr}"
    );
    assert!(!stderr.contains("skipped"), "{stderr}");
    wait_until("python clients to disconnect", || {
        host.stats().map(|s| s.connections == 0).unwrap_or(false)
    });
    let stats = host.stats().expect("stats");
    assert!(stats.files >= 4);
    let path = host.pipe_path();
    let transport = PipeTransport::connect(&path, DEFAULT_MAX_FRAME).expect("fresh connection");
    let mut client = Client::connect(transport).expect("handshake after the python run");
    client.ping().expect("ping");
}
