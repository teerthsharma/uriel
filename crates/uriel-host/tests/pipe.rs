#![cfg(windows)]

mod common;

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::{fixture, wait_until, Rng};
use uriel_host::{
    encode_ping, encode_search, AccessMode, Client, ErrorCode, Host, HostError, PipeTransport,
    Recv, Response, SearchOptions, ServeConfig, Transport, Value, DEFAULT_MAX_FRAME,
};

const GATE_MAX_FRAME: usize = 8192;
const REPLY_WAIT: Duration = Duration::from_secs(5);

fn unique(tag: &str) -> String {
    format!("uriel-pipe-{}-{tag}", std::process::id())
}

fn gate_host(tag: &str) -> (tempfile::TempDir, Host, String) {
    let (dir, config) = fixture(&unique(tag));
    let host = Host::open(config).expect("open");
    host.serve(ServeConfig {
        queue_depth: 16,
        workers: 2,
        max_frame: GATE_MAX_FRAME,
    })
    .expect("serve");
    let path = host.listen_pipe().expect("listen pipe");
    (dir, host, path)
}

fn within<T: Send + 'static>(seconds: u64, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(Duration::from_secs(seconds))
        .expect("work finished within its watchdog")
}

fn frame_of(body: &[u8]) -> Vec<u8> {
    let mut frame = u32::try_from(body.len())
        .expect("body fits")
        .to_le_bytes()
        .to_vec();
    frame.extend_from_slice(body);
    frame
}

fn ping_frame(nonce: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_ping(&mut buf, nonce, DEFAULT_MAX_FRAME).expect("encode ping");
    buf
}

enum Reply {
    Frame(Response),
    Closed,
    Silent,
}

struct Probe {
    transport: PipeTransport,
}

impl Probe {
    fn connect(path: &str) -> Probe {
        Probe {
            transport: PipeTransport::connect(path, DEFAULT_MAX_FRAME).expect("connect probe"),
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.transport.send(bytes).expect("send raw bytes");
    }

    fn reply_within(&mut self, wait: Duration) -> Reply {
        let mut buf = Vec::new();
        match self.transport.recv(&mut buf, wait) {
            Ok(Recv::Frame) => Reply::Frame(
                Response::decode(&buf, DEFAULT_MAX_FRAME).expect("server frames always decode"),
            ),
            Ok(Recv::Timeout) => Reply::Silent,
            Err(HostError::Closed) => Reply::Closed,
            Err(e) => panic!("unexpected transport error: {e}"),
        }
    }

    fn reply(&mut self) -> Reply {
        self.reply_within(REPLY_WAIT)
    }

    fn expect_error(&mut self, code: ErrorCode) {
        match self.reply() {
            Reply::Frame(Response::Error { code: got, .. }) => assert_eq!(got, code),
            Reply::Frame(other) => panic!("expected error {code:?}, got {other:?}"),
            Reply::Closed => panic!("expected error {code:?}, connection closed"),
            Reply::Silent => panic!("expected error {code:?}, server stayed silent"),
        }
    }

    fn expect_closed(&mut self) {
        assert!(matches!(self.reply(), Reply::Closed), "expected the server to close");
    }

    fn expect_pong(&mut self, nonce: u64) {
        match self.reply() {
            Reply::Frame(Response::Pong { nonce: got }) => assert_eq!(got, nonce),
            Reply::Frame(other) => panic!("expected pong, got {other:?}"),
            Reply::Closed => panic!("expected pong, connection closed"),
            Reply::Silent => panic!("expected pong, server stayed silent"),
        }
    }
}

fn server_is_healthy(path: &str) {
    let transport = PipeTransport::connect(path, DEFAULT_MAX_FRAME).expect("fresh connection");
    let mut client = Client::connect(transport).expect("handshake");
    client.ping().expect("ping");
    let none: BTreeMap<String, Value> = BTreeMap::new();
    let one = client
        .query("RETURN 1 AS x", &none, AccessMode::Read)
        .expect("query");
    assert_eq!(one.rows, vec![vec![Value::Int(1)]]);
}

#[test]
fn stub_host_round_trip_over_a_named_pipe() {
    let (_dir, host, path) = gate_host("round-trip");
    assert_eq!(path, host.pipe_path());
    assert!(path.starts_with(r"\\.\pipe\"));

    let transport = PipeTransport::connect(&path, GATE_MAX_FRAME).expect("connect pipe");
    let mut client = Client::connect_with(transport, GATE_MAX_FRAME, Duration::from_secs(10))
        .expect("handshake ping");
    client.ping().expect("ping");

    let none: BTreeMap<String, Value> = BTreeMap::new();
    let one = client
        .query("RETURN 1 AS x", &none, AccessMode::Read)
        .expect("query");
    assert_eq!(one.rows, vec![vec![Value::Int(1)]]);

    let opts = SearchOptions {
        k: 2,
        budget_tokens: 200,
        ..SearchOptions::default()
    };
    let pack = client
        .search("rolling sum over a byte buffer", &opts)
        .expect("search");
    assert_eq!(pack.hits[0].file, "a.rs");
    let local = host
        .search("rolling sum over a byte buffer", &opts)
        .expect("local search");
    assert_eq!(pack.hits, local.hits);

    let stats = client.stats().expect("stats");
    assert_eq!(stats.files, 2);
    assert_eq!(stats.connections, 1);
    assert!(stats.requests_served >= 2);
}

#[test]
fn a_plain_file_handle_speaks_the_protocol() {
    let (_dir, _host, path) = gate_host("plain-file");
    let echoed = within(20, move || {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open pipe as a file");
        file.write_all(&ping_frame(0xFEED_FACE)).expect("write ping");
        let mut reply = [0u8; 4 + 9];
        file.read_exact(&mut reply).expect("read pong");
        Response::decode(&reply, DEFAULT_MAX_FRAME).expect("decode pong")
    });
    assert!(matches!(echoed, Response::Pong { nonce: 0xFEED_FACE }));
}

#[test]
fn zero_bytes_change_nothing_and_a_zero_length_frame_is_refused() {
    let (_dir, _host, path) = gate_host("zero");
    let mut probe = Probe::connect(&path);
    probe.send(&[]);
    assert!(matches!(probe.reply_within(Duration::from_millis(200)), Reply::Silent));
    probe.send(&ping_frame(1));
    probe.expect_pong(1);

    probe.send(&frame_of(&[]));
    probe.expect_error(ErrorCode::Malformed);
    probe.expect_closed();
    server_is_healthy(&path);
}

fn search_frame_with_body(len: usize) -> Vec<u8> {
    let opts = SearchOptions {
        k: 1,
        budget_tokens: 50,
        ..SearchOptions::default()
    };
    let mut buf = Vec::new();
    encode_search(&mut buf, "", &opts, usize::MAX).expect("encode base");
    let base = buf.len() - 4;
    assert!(len >= base);
    encode_search(&mut buf, &"a".repeat(len - base), &opts, usize::MAX).expect("encode padded");
    assert_eq!(buf.len() - 4, len);
    buf
}

#[test]
fn a_frame_of_exactly_the_cap_is_served_and_one_byte_more_is_refused() {
    let (_dir, _host, path) = gate_host("cap");
    let mut probe = Probe::connect(&path);
    probe.send(&search_frame_with_body(GATE_MAX_FRAME));
    match probe.reply() {
        Reply::Frame(Response::Search(_)) => {}
        Reply::Frame(Response::Error { code, message }) => {
            panic!("a frame at the cap was refused: {code:?} {message}")
        }
        _ => panic!("no search reply for a frame at the cap"),
    }
    probe.send(&ping_frame(7));
    probe.expect_pong(7);

    let mut over = Probe::connect(&path);
    let mut head = ((GATE_MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
    head.extend_from_slice(&[0x01]);
    over.send(&head);
    over.expect_error(ErrorCode::TooLarge);
    over.expect_closed();
    server_is_healthy(&path);
}

#[test]
fn an_absurd_length_prefix_is_refused_before_any_body_arrives() {
    let (_dir, host, path) = gate_host("absurd");
    let mut probe = Probe::connect(&path);
    let started = Instant::now();
    probe.send(&u32::MAX.to_le_bytes());
    probe.expect_error(ErrorCode::TooLarge);
    probe.expect_closed();
    assert!(started.elapsed() < Duration::from_secs(5));
    wait_until("connection to close", || {
        host.stats().map(|s| s.connections == 0).unwrap_or(false)
    });
    for gauge in host.connection_buffers() {
        assert!(gauge.inbound_capacity <= 4 * GATE_MAX_FRAME);
    }
    server_is_healthy(&path);
}

#[test]
fn a_truncated_payload_is_malformed_and_a_cut_wire_does_not_wedge_the_server() {
    let (_dir, host, path) = gate_host("truncated");
    let mut probe = Probe::connect(&path);
    let full = search_frame_with_body(40);
    probe.send(&frame_of(&full[4..full.len() - 7]));
    probe.expect_error(ErrorCode::Malformed);
    probe.send(&ping_frame(3));
    probe.expect_pong(3);
    drop(probe);

    let mut cut = Probe::connect(&path);
    let mut half = 100u32.to_le_bytes().to_vec();
    half.extend_from_slice(&[0x01; 10]);
    cut.send(&half);
    assert!(matches!(cut.reply_within(Duration::from_millis(200)), Reply::Silent));
    drop(cut);

    let mut prefix_only = Probe::connect(&path);
    prefix_only.send(&[0x05, 0x00]);
    drop(prefix_only);

    wait_until("cut connections to be reaped", || {
        host.stats().map(|s| s.connections <= 1).unwrap_or(false)
    });
    server_is_healthy(&path);
    wait_until("all connections to close", || {
        host.stats().map(|s| s.connections == 0).unwrap_or(false)
    });
}

#[test]
fn a_length_prefix_that_disagrees_with_the_body_is_an_error() {
    let (_dir, _host, path) = gate_host("bad-length");
    let ping = ping_frame(9);
    let mut short = Probe::connect(&path);
    let mut lying = ping.clone();
    lying[..4].copy_from_slice(&4u32.to_le_bytes());
    lying.truncate(4 + 4);
    short.send(&lying);
    short.expect_error(ErrorCode::Malformed);
    short.send(&ping_frame(10));
    short.expect_pong(10);

    let mut padded = Probe::connect(&path);
    let mut body = ping[4..].to_vec();
    body.extend_from_slice(&[0xAB; 3]);
    padded.send(&frame_of(&body));
    padded.expect_error(ErrorCode::Malformed);
    padded.send(&ping_frame(11));
    padded.expect_pong(11);
    server_is_healthy(&path);
}

#[test]
fn twenty_thousand_random_byte_strings_never_panic_or_wedge_the_server() {
    let (_dir, host, path) = gate_host("fuzz");
    let mut rng = Rng::new(0x5EED_0007);
    let mut probe = Probe::connect(&path);
    let mut reconnects = 0usize;
    let mut errors = 0usize;
    for round in 0..19_000usize {
        let len = 1 + rng.index(200);
        let body = rng.bytes(len);
        probe.send(&frame_of(&body));
        match probe.reply() {
            Reply::Frame(Response::Error { code, .. }) => {
                assert!(matches!(code, ErrorCode::Malformed | ErrorCode::TooLarge));
                errors += 1;
            }
            Reply::Frame(_) => {}
            Reply::Closed => {
                reconnects += 1;
                probe = Probe::connect(&path);
            }
            Reply::Silent => panic!("round {round}: server went silent"),
        }
    }
    assert!(errors > 18_000, "only {errors} of 19000 random frames were refused");
    assert_eq!(reconnects, 0);

    let mut refused = 0usize;
    for _ in 0..1_000usize {
        let mut raw = Probe::connect(&path);
        let len = 4 + rng.index(60);
        raw.send(&rng.bytes(len));
        match raw.reply() {
            Reply::Frame(Response::Error { .. }) | Reply::Closed => refused += 1,
            Reply::Frame(_) => {}
            Reply::Silent => panic!("a raw random string drew no reply"),
        }
    }
    assert!(refused > 990, "only {refused} of 1000 raw strings were refused");

    for gauge in host.connection_buffers() {
        assert!(gauge.inbound_capacity <= 4 * GATE_MAX_FRAME);
        assert!(gauge.outbound_capacity <= 4 * GATE_MAX_FRAME);
    }
    probe.send(&ping_frame(77));
    probe.expect_pong(77);
    server_is_healthy(&path);
}

#[test]
fn two_clients_are_served_at_once() {
    let (_dir, host, path) = gate_host("two");
    let mut first = Probe::connect(&path);
    let mut second = Probe::connect(&path);
    first.send(&ping_frame(1));
    second.send(&ping_frame(2));
    second.expect_pong(2);
    first.expect_pong(1);
    assert_eq!(host.stats().expect("stats").connections, 2);
    drop(first);
    second.send(&ping_frame(3));
    second.expect_pong(3);
    server_is_healthy(&path);
}

#[test]
fn a_second_host_cannot_take_over_a_live_pipe_name() {
    let (_dir, _host, path) = gate_host("squat");
    let (_other_dir, other_config) = fixture(&unique("squat"));
    let other = Host::open(other_config).expect("open second host");
    assert_eq!(other.pipe_path(), path);
    assert!(other.listen_pipe().is_err());
    server_is_healthy(&path);
}

#[test]
fn dropping_the_host_with_clients_attached_returns() {
    let (dir, host, path) = gate_host("drop");
    let mut probe = Probe::connect(&path);
    probe.send(&ping_frame(5));
    probe.expect_pong(5);
    within(20, move || drop(host));
    probe.expect_closed();
    drop(dir);
}

fn python_on_path() -> bool {
    std::process::Command::new("python")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[test]
fn the_python_reference_client_round_trips_against_a_live_pipe() {
    if !python_on_path() {
        eprintln!("SKIPPED: `python` is not on PATH, the reference client was not exercised");
        return;
    }
    let (_dir, _host, path) = gate_host("python");
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("clients")
        .join("uriel_client.py");
    let output = within(60, move || {
        std::process::Command::new("python")
            .arg("-I")
            .arg(script)
            .arg(path)
            .arg("rolling sum over a byte buffer")
            .output()
            .expect("run python client")
    });
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "python client failed: {stderr}");
    assert!(stdout.contains("\"pong\": true"), "{stdout}");
    assert!(stdout.contains("\"top_file\": \"a.rs\""), "{stdout}");
    assert!(stdout.contains("\"files\": 2"), "{stdout}");
}

#[test]
fn index_and_index_file_round_trip_over_a_named_pipe() {
    let (dir, host, path) = gate_host("index-pipe");
    let transport = PipeTransport::connect(&path, GATE_MAX_FRAME).expect("connect pipe");
    let mut client = Client::connect_with(transport, GATE_MAX_FRAME, Duration::from_secs(10))
        .expect("handshake");
    let root = dir.path().join("src").to_string_lossy().into_owned();
    let report = client.index(&root).expect("index");
    assert_eq!((report.files_scanned, report.files_indexed), (2, 2));
    assert_eq!(host.index_passes(), 1);
    let report = client
        .index_file("scratch/ghost.rs", "pub fn quokka_pelican_shuffle() {}\n")
        .expect("index_file");
    assert_eq!(report.files_indexed, 1);
    let pack = client
        .search("quokka_pelican_shuffle", &SearchOptions::default())
        .expect("search");
    assert_eq!(pack.hits[0].file, "scratch/ghost.rs");
    match client.index_file("x", &"y".repeat(GATE_MAX_FRAME)) {
        Err(HostError::Codec(_)) => {}
        other => panic!("client let an over-cap frame out: {other:?}"),
    }
    client.ping().expect("still usable");
}

#[test]
fn two_thousand_random_index_frames_never_wedge_the_pipe_server() {
    let (_dir, host, path) = gate_host("index-fuzz");
    let mut rng = Rng::new(0x1DE7_0001);
    let mut probe = Probe::connect(&path);
    for round in 0..2_000usize {
        let mut body = vec![[0x05u8, 0x06][rng.index(2)]];
        let len = rng.index(80);
        body.extend_from_slice(&rng.bytes(len));
        probe.send(&frame_of(&body));
        match probe.reply() {
            Reply::Frame(Response::Error { code, .. }) => {
                assert!(matches!(
                    code,
                    ErrorCode::Malformed | ErrorCode::TooLarge | ErrorCode::Index
                ));
            }
            Reply::Frame(Response::Indexed(_)) => {}
            Reply::Frame(other) => panic!("round {round}: unexpected {other:?}"),
            Reply::Closed => panic!("round {round}: server closed"),
            Reply::Silent => panic!("round {round}: server went silent"),
        }
    }
    for gauge in host.connection_buffers() {
        assert!(gauge.inbound_capacity <= 4 * GATE_MAX_FRAME);
        assert!(gauge.outbound_capacity <= 4 * GATE_MAX_FRAME);
    }
    probe.send(&ping_frame(5));
    probe.expect_pong(5);
    server_is_healthy(&path);
}
