mod common;

use std::collections::BTreeMap;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use common::{fixture, wait_until};
use uriel_host::{
    encode_ping, encode_query, AccessMode, ChannelTransport, Client, ErrorCode, Host, HostConfig,
    HostError, Request, Response, SearchOptions, ServeConfig, TcpTransport, Transport, Value,
    DEFAULT_MAX_FRAME,
};
use uriel_sys::SysError;

const WAIT: Duration = Duration::from_secs(10);
const CREATE_TWO: &str = "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Bob'})";
const READ_ONE: &str = "RETURN 1 AS x";

fn none() -> BTreeMap<String, Value> {
    BTreeMap::new()
}

fn open_host(name: &str) -> (tempfile::TempDir, Host) {
    let (dir, config) = fixture(name);
    let host = Host::open(config).expect("open host");
    (dir, host)
}

fn channel_client(host: &Host) -> Client {
    let (client_end, server_end) = ChannelTransport::pair();
    host.attach(server_end).expect("attach");
    Client::connect(client_end).expect("connect")
}

struct Gate {
    released: AtomicBool,
    entered: Mutex<mpsc::Sender<()>>,
}

fn gated_host(host: &Host) -> (Arc<Gate>, mpsc::Receiver<()>) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let gate = Arc::new(Gate {
        released: AtomicBool::new(false),
        entered: Mutex::new(entered_tx),
    });
    let hook_gate = gate.clone();
    host.set_handler_hook(move |request| {
        if matches!(request, Request::Query { .. }) {
            let _ = hook_gate
                .entered
                .lock()
                .expect("entered lock")
                .send(());
            while !hook_gate.released.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(2));
            }
        }
    });
    (gate, entered_rx)
}

#[test]
fn query_and_search_round_trip_through_channel_transport() {
    let (_dir, host) = open_host("round-trip");
    let mut client = channel_client(&host);

    let created = client
        .query(CREATE_TWO, &none(), AccessMode::Write)
        .expect("create");
    assert_eq!(created.stats.nodes_created, 2);

    let read = "MATCH (a)-[:KNOWS]->(b) RETURN a.name AS a, b.name AS b";
    let remote = client.query(read, &none(), AccessMode::Read).expect("read");
    assert_eq!(remote.columns, vec!["a".to_owned(), "b".to_owned()]);
    assert_eq!(
        remote.rows,
        vec![vec![Value::from("Ada"), Value::from("Bob")]]
    );
    let direct = host.query(read, &none(), AccessMode::Read).expect("direct");
    assert_eq!(direct.columns, remote.columns);
    assert_eq!(direct.rows, remote.rows);

    let mut params = BTreeMap::new();
    params.insert("name".to_owned(), Value::from("Bob"));
    let bound = client
        .query(
            "MATCH (p:Person {name: $name}) RETURN p.name AS n",
            &params,
            AccessMode::Read,
        )
        .expect("bound");
    assert_eq!(bound.rows, vec![vec![Value::from("Bob")]]);

    let opts = SearchOptions::default();
    let pack = client
        .search("rolling sum over a byte buffer", &opts)
        .expect("search");
    assert!(!pack.hits.is_empty());
    assert_eq!(pack.hits[0].file, "a.rs");
    let local = host
        .search("rolling sum over a byte buffer", &opts)
        .expect("local search");
    assert_eq!(pack.hits, local.hits);
    assert_eq!(pack.text, local.text);
    assert_eq!(pack.tokens, local.tokens);

    let stats = client.stats().expect("stats");
    assert!(stats.chunks > 0);
    assert!(stats.files >= 2);
    assert_eq!(stats.embed_dim, 256);
    assert_eq!(stats.connections, 1);
    assert!(stats.requests_served >= 4);
    assert_eq!(stats.arena_cap, 1 << 20);
}

#[test]
fn remote_errors_are_typed_and_the_connection_stays_usable() {
    let (_dir, host) = open_host("typed-errors");
    let mut client = channel_client(&host);

    let syntax = client.query("MATCH (", &none(), AccessMode::Read);
    assert!(matches!(
        syntax,
        Err(HostError::Remote {
            code: ErrorCode::Query,
            ..
        })
    ));
    let forbidden = client.query(CREATE_TWO, &none(), AccessMode::Read);
    assert!(matches!(
        forbidden,
        Err(HostError::Remote {
            code: ErrorCode::Query,
            ..
        })
    ));
    client.ping().expect("ping after errors");
    let ok = client
        .query(READ_ONE, &none(), AccessMode::Read)
        .expect("query after errors");
    assert_eq!(ok.rows, vec![vec![Value::Int(1)]]);
    assert_eq!(client.stats().expect("stats").requests_failed, 2);
}

#[test]
fn busy_is_returned_when_the_queue_is_full() {
    let (_dir, host) = open_host("busy");
    host.serve(ServeConfig {
        queue_depth: 1,
        workers: 1,
        max_frame: DEFAULT_MAX_FRAME,
    })
    .expect("serve");
    let (gate, entered) = gated_host(&host);
    let mut first = channel_client(&host);
    let mut second = channel_client(&host);
    let mut third = channel_client(&host);

    let first_call = thread::spawn(move || first.query(READ_ONE, &BTreeMap::new(), AccessMode::Read));
    entered.recv_timeout(WAIT).expect("worker picked up the first request");

    let second_call =
        thread::spawn(move || second.query(READ_ONE, &BTreeMap::new(), AccessMode::Read));
    wait_until("second request to queue", || {
        host.stats().map(|s| s.queue_len == 1).unwrap_or(false)
    });

    let refused = third.query(READ_ONE, &none(), AccessMode::Read);
    assert!(matches!(refused, Err(HostError::Busy)));
    third.ping().expect("ping bypasses the queue");
    let mid = host.stats().expect("stats");
    assert_eq!(mid.queue_len, 1);
    assert_eq!(mid.in_flight, 2);
    assert_eq!(mid.requests_busy, 1);

    gate.released.store(true, Ordering::SeqCst);
    first_call.join().expect("first thread").expect("first result");
    second_call.join().expect("second thread").expect("second result");
    let after = host.stats().expect("stats");
    assert_eq!(after.queue_len, 0);
    assert_eq!(after.in_flight, 0);
    assert_eq!(after.requests_served, 2);
    third
        .query(READ_ONE, &none(), AccessMode::Read)
        .expect("accepted once the queue drained");
}

#[test]
fn arena_cap_is_respected() {
    let (_dir, mut config) = fixture("arena-cap");
    config.hot_cap_bytes = 4_096;
    let host = Host::open(config).expect("open");
    assert_eq!(host.arena_cap(), 4_096);
    assert_eq!(host.arena_used(), 0);

    host.reserve_hot(1_000).expect("first slab");
    assert_eq!(host.arena_used(), 1_024);

    let too_big = host.reserve_hot(4_096);
    assert!(matches!(
        too_big,
        Err(HostError::Arena(SysError::CapExceeded { .. }))
    ));
    assert_eq!(host.arena_used(), 1_024);

    host.reserve_hot(3_072).expect("exact remainder");
    assert_eq!(host.arena_used(), 4_096);
    assert!(matches!(
        host.reserve_hot(1),
        Err(HostError::Arena(SysError::CapExceeded { .. }))
    ));
    assert_eq!(host.stats().expect("stats").arena_used, 4_096);
}

#[test]
fn open_fails_when_the_hot_cap_cannot_be_reserved() {
    for cap in [1usize << 46, usize::MAX] {
        let (_dir, mut config) = fixture("arena-huge");
        config.hot_cap_bytes = cap;
        let db_path = config.db_path.clone();
        let result = Host::open(config);
        assert!(matches!(result, Err(HostError::Arena(_))), "cap {cap}");
        assert!(!db_path.exists(), "database created despite arena failure");
    }
}

#[test]
fn open_rejects_a_zero_hot_cap_a_bad_sock_name_and_a_missing_root() {
    let (_dir, mut config) = fixture("zero-cap");
    config.hot_cap_bytes = 0;
    assert!(matches!(
        Host::open(config),
        Err(HostError::InvalidConfig(_))
    ));

    for name in ["", "a/b", "a\\b", "..", "x:y"] {
        let (_dir, config) = fixture(name);
        assert!(
            matches!(Host::open(config), Err(HostError::InvalidConfig(_))),
            "sock_name {name:?}"
        );
    }

    let (dir, mut config) = fixture("missing-root");
    config.root = dir.path().join("does-not-exist");
    assert!(matches!(
        Host::open(config),
        Err(HostError::InvalidConfig(_))
    ));
}

#[test]
fn serve_config_is_validated_and_cannot_be_applied_twice() {
    let (_dir, host) = open_host("serve-config");
    let base = ServeConfig::default();
    for bad in [
        ServeConfig {
            queue_depth: 0,
            ..base
        },
        ServeConfig { workers: 0, ..base },
        ServeConfig {
            max_frame: 8,
            ..base
        },
    ] {
        assert!(matches!(host.serve(bad), Err(HostError::InvalidConfig(_))));
    }
    host.serve(base).expect("serve");
    assert!(matches!(host.serve(base), Err(HostError::AlreadyServing)));
}

#[test]
fn loopback_tcp_round_trip_on_an_ephemeral_port() {
    let (_dir, host) = open_host("tcp-round-trip");
    let addr = host.listen_tcp().expect("listen");
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0);
    let port_text = std::fs::read_to_string(host.port_file()).expect("port file");
    assert_eq!(port_text.trim(), addr.port().to_string());
    assert!(matches!(host.listen_tcp(), Err(HostError::AlreadyServing)));

    let transport =
        TcpTransport::connect_port_file(&host.port_file(), DEFAULT_MAX_FRAME).expect("connect");
    let mut client = Client::connect(transport).expect("client");
    client
        .query(CREATE_TWO, &none(), AccessMode::Write)
        .expect("create");
    let rows = client
        .query(
            "MATCH (p:Person) RETURN p.name AS name ORDER BY name",
            &none(),
            AccessMode::Read,
        )
        .expect("read");
    assert_eq!(
        rows.rows,
        vec![vec![Value::from("Ada")], vec![Value::from("Bob")]]
    );
    let pack = client
        .search("when to prune roses", &SearchOptions::default())
        .expect("search");
    assert_eq!(pack.hits[0].file, "notes.md");
    client.ping().expect("ping");

    let mut second = Client::connect(TcpTransport::connect(addr, DEFAULT_MAX_FRAME).expect("dial"))
        .expect("second client");
    assert_eq!(second.stats().expect("stats").connections, 2);

    host.shutdown();
    assert!(!host.port_file().exists());
    assert!(client.ping().is_err());
}

#[test]
fn tcp_server_answers_an_oversized_declared_frame_with_too_large_and_closes() {
    let (_dir, host) = open_host("tcp-oversized");
    host.serve(ServeConfig {
        max_frame: 1_024,
        ..ServeConfig::default()
    })
    .expect("serve");
    let addr = host.listen_tcp().expect("listen");

    let stream = TcpStream::connect(addr).expect("dial");
    let mut raw = TcpTransport::new(stream, DEFAULT_MAX_FRAME).expect("raw");
    let mut frame = Vec::new();
    frame.extend_from_slice(&(1u32 << 30).to_le_bytes());
    frame.push(0x01);
    raw.send(&frame).expect("send header");

    let mut reply = Vec::new();
    raw.recv(&mut reply, WAIT).expect("recv");
    let response = Response::decode(&reply, DEFAULT_MAX_FRAME).expect("decode");
    assert!(matches!(
        response,
        Response::Error {
            code: ErrorCode::TooLarge,
            ..
        }
    ));
    assert!(matches!(raw.recv(&mut reply, WAIT), Err(HostError::Closed)));
}

#[test]
fn channel_server_rejects_an_oversized_frame_and_keeps_the_connection() {
    let (_dir, host) = open_host("channel-oversized");
    host.serve(ServeConfig {
        max_frame: 1_024,
        ..ServeConfig::default()
    })
    .expect("serve");
    let (mut raw, server_end) = ChannelTransport::pair();
    host.attach(server_end).expect("attach");

    let mut frame = Vec::new();
    encode_query(
        &mut frame,
        &"x".repeat(2_000),
        &none(),
        AccessMode::Read,
        DEFAULT_MAX_FRAME,
    )
    .expect("encode");
    raw.send(&frame).expect("send");
    let mut reply = Vec::new();
    raw.recv(&mut reply, WAIT).expect("recv");
    assert!(matches!(
        Response::decode(&reply, DEFAULT_MAX_FRAME).expect("decode"),
        Response::Error {
            code: ErrorCode::TooLarge,
            ..
        }
    ));

    encode_ping(&mut frame, 77, 1_024).expect("encode ping");
    raw.send(&frame).expect("send ping");
    raw.recv(&mut reply, WAIT).expect("recv pong");
    assert_eq!(
        Response::decode(&reply, DEFAULT_MAX_FRAME).expect("decode"),
        Response::Pong { nonce: 77 }
    );
}

#[test]
fn malformed_frames_get_a_malformed_error_and_the_connection_stays_usable() {
    let (_dir, host) = open_host("malformed");
    let (mut raw, server_end) = ChannelTransport::pair();
    host.attach(server_end).expect("attach");

    let garbage: [&[u8]; 3] = [
        &[3, 0, 0, 0, 0x7F, 1, 2],
        &[0, 0, 0, 0],
        &[9, 0, 0, 0, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0],
    ];
    let mut reply = Vec::new();
    for bytes in garbage {
        raw.send(bytes).expect("send");
        raw.recv(&mut reply, WAIT).expect("recv");
        assert!(matches!(
            Response::decode(&reply, DEFAULT_MAX_FRAME).expect("decode"),
            Response::Error {
                code: ErrorCode::Malformed,
                ..
            }
        ));
    }
    let mut frame = Vec::new();
    encode_ping(&mut frame, 5, DEFAULT_MAX_FRAME).expect("encode ping");
    raw.send(&frame).expect("send ping");
    raw.recv(&mut reply, WAIT).expect("recv pong");
    assert_eq!(
        Response::decode(&reply, DEFAULT_MAX_FRAME).expect("decode"),
        Response::Pong { nonce: 5 }
    );
}

#[test]
fn response_buffers_do_not_grow_over_one_thousand_identical_requests() {
    let (_dir, host) = open_host("buffer-capacity");
    let mut client = channel_client(&host);
    client
        .query(CREATE_TWO, &none(), AccessMode::Write)
        .expect("create");
    let read = "MATCH (p:Person) RETURN p.name AS name, p.name AS again";
    for _ in 0..20 {
        client.query(read, &none(), AccessMode::Read).expect("warm up");
    }
    let warm_server = host.connection_buffers();
    assert_eq!(warm_server.len(), 1);
    let warm_client = (client.outbound_capacity(), client.inbound_capacity());
    assert!(warm_server[0].outbound_capacity > 0);

    for _ in 0..1_000 {
        let result = client.query(read, &none(), AccessMode::Read).expect("query");
        assert_eq!(result.rows.len(), 2);
    }
    assert_eq!(host.connection_buffers(), warm_server);
    assert_eq!(
        (client.outbound_capacity(), client.inbound_capacity()),
        warm_client
    );
}

#[test]
fn shutdown_drains_in_flight_requests() {
    let (_dir, host) = open_host("drain");
    host.serve(ServeConfig {
        queue_depth: 4,
        workers: 1,
        max_frame: DEFAULT_MAX_FRAME,
    })
    .expect("serve");
    let host = Arc::new(host);
    let (gate, entered) = gated_host(&host);
    let mut first = channel_client(&host);
    let mut second = channel_client(&host);

    let first_call = thread::spawn(move || first.query(READ_ONE, &BTreeMap::new(), AccessMode::Read));
    entered.recv_timeout(WAIT).expect("first request running");
    let second_call =
        thread::spawn(move || second.query(READ_ONE, &BTreeMap::new(), AccessMode::Read));
    wait_until("second request to queue", || {
        host.stats().map(|s| s.queue_len == 1).unwrap_or(false)
    });

    let stopper = {
        let host = host.clone();
        thread::spawn(move || host.shutdown())
    };
    thread::sleep(Duration::from_millis(100));
    assert!(!stopper.is_finished(), "shutdown returned with requests in flight");

    gate.released.store(true, Ordering::SeqCst);
    stopper.join().expect("shutdown thread");
    let first_result = first_call.join().expect("first thread").expect("first result");
    let second_result = second_call.join().expect("second thread").expect("second result");
    assert_eq!(first_result.rows, vec![vec![Value::Int(1)]]);
    assert_eq!(second_result.rows, vec![vec![Value::Int(1)]]);

    let stats = host.stats().expect("stats");
    assert_eq!(stats.requests_served, 2);
    assert_eq!(stats.in_flight, 0);
    assert_eq!(stats.connections, 0);

    let (_late_client, late_server) = ChannelTransport::pair();
    assert!(matches!(
        host.attach(late_server),
        Err(HostError::ShuttingDown)
    ));
}

#[test]
fn shutdown_is_idempotent_and_runs_on_drop() {
    let (_dir, config) = fixture("idempotent");
    let again: HostConfig = config.clone();
    let host = Host::open(config).expect("open");
    let mut client = channel_client(&host);
    client.ping().expect("ping");
    host.shutdown();
    host.shutdown();
    assert!(client.ping().is_err());
    drop(host);
    assert!(Host::open(again).is_ok());
}


#[test]
fn open_leaves_the_root_alone_and_the_first_search_indexes_once() {
    let (_dir, host) = open_host("lazy-search");
    assert_eq!(host.index_passes(), 0);
    let before = host.stats().expect("stats before");
    assert_eq!((before.files, before.chunks), (0, 0));

    let opts = SearchOptions::default();
    let pack = host
        .search("rolling sum over a byte buffer", &opts)
        .expect("first search");
    assert_eq!(pack.hits[0].file, "a.rs");
    assert_eq!(host.index_passes(), 1);
    assert_eq!(host.stats().expect("stats after").files, 2);

    host.search("rolling sum over a byte buffer", &opts)
        .expect("second search");
    assert_eq!(host.index_passes(), 1);
}

#[test]
fn first_remote_search_triggers_the_index_pass() {
    let (_dir, host) = open_host("lazy-remote");
    let mut client = channel_client(&host);
    assert_eq!(client.stats().expect("stats").files, 0);
    assert_eq!(host.index_passes(), 0);
    let pack = client
        .search("rolling sum over a byte buffer", &SearchOptions::default())
        .expect("search");
    assert_eq!(pack.hits[0].file, "a.rs");
    assert_eq!(host.index_passes(), 1);
}

#[test]
fn explicit_index_reports_the_files_read_and_spares_the_first_search() {
    let (_dir, host) = open_host("lazy-explicit");
    let report = host.index().expect("index");
    assert_eq!(report.files_scanned, 2);
    assert_eq!(report.files_indexed, 2);
    assert_eq!(host.index_passes(), 1);
    host.search("rolling sum over a byte buffer", &SearchOptions::default())
        .expect("search");
    assert_eq!(host.index_passes(), 1);
    let again = host.index().expect("second index");
    assert_eq!(again.files_unchanged, 2);
    assert_eq!(host.index_passes(), 2);
}

#[test]
fn a_failed_index_pass_is_retried_by_the_next_search() {
    let (dir, host) = open_host("lazy-retry");
    let root = dir.path().join("src");
    let moved = dir.path().join("moved");
    std::fs::rename(&root, &moved).expect("move root away");
    assert!(host.search("rolling sum", &SearchOptions::default()).is_err());
    assert_eq!(host.index_passes(), 0);
    std::fs::rename(&moved, &root).expect("restore root");
    host.search("rolling sum", &SearchOptions::default())
        .expect("search after restore");
    assert_eq!(host.index_passes(), 1);
}

fn root_text(dir: &tempfile::TempDir) -> String {
    dir.path().join("src").to_string_lossy().into_owned()
}

#[test]
fn index_over_a_channel_returns_the_report_counts_of_host_index() {
    let (dir, host) = open_host("idx-channel");
    let mut client = channel_client(&host);
    let report = client.index(&root_text(&dir)).expect("index");
    assert_eq!(report.files_scanned, 2);
    assert_eq!(report.files_indexed, 2);
    assert_eq!(report.files_unchanged, 0);
    assert_eq!(report.files_skipped, 0);
    assert!(report.chunks_added >= 2);
    assert_eq!(host.index_passes(), 1);
    let again = client.index("").expect("empty root means the host root");
    assert_eq!(again.files_unchanged, 2);
    assert_eq!(again.files_indexed, 0);
    assert_eq!(again.chunks_added, 0);
    assert_eq!(host.index_passes(), 2);
    assert_eq!(client.stats().expect("stats").files, 2);
}

#[test]
fn index_picks_up_a_file_written_after_the_first_pass() {
    let (dir, host) = open_host("idx-new-file");
    let mut client = channel_client(&host);
    client.index("").expect("first");
    std::fs::write(dir.path().join("src").join("b.rs"), "pub fn zebra_crossing() {}\n")
        .expect("write");
    let report = client.index("").expect("second");
    assert_eq!(report.files_indexed, 1);
    assert_eq!(report.files_unchanged, 2);
    let pack = client
        .search("zebra_crossing", &SearchOptions::default())
        .expect("search");
    assert_eq!(pack.hits[0].file, "b.rs");
}

#[test]
fn index_refuses_a_root_that_is_not_the_host_root() {
    let (dir, host) = open_host("idx-foreign");
    let mut client = channel_client(&host);
    let outside = dir.path().to_string_lossy().into_owned();
    for bad in [outside, "nowhere/at/all".to_owned(), "..".to_owned(), "a\0b".to_owned()] {
        match client.index(&bad) {
            Err(HostError::Remote {
                code: ErrorCode::Index,
                ..
            }) => {}
            other => panic!("root {bad:?} was not refused: {other:?}"),
        }
    }
    assert_eq!(host.index_passes(), 0);
    client.ping().expect("connection still usable");
}

#[test]
fn index_file_over_tcp_makes_the_content_searchable() {
    let (_dir, host) = open_host("idxf-tcp");
    let addr = host.listen_tcp().expect("listen");
    let mut client =
        Client::connect(TcpTransport::connect(addr, DEFAULT_MAX_FRAME).expect("dial")).expect("client");
    let report = client
        .index_file("buffer/live.rs", "pub fn quokka_pelican_shuffle() -> u32 { 41 }\n")
        .expect("index_file");
    assert_eq!(report.files_scanned, 1);
    assert_eq!(report.files_indexed, 1);
    assert!(report.chunks_added >= 1);
    assert_eq!(host.index_passes(), 0);
    let pack = client
        .search("quokka_pelican_shuffle", &SearchOptions::default())
        .expect("search");
    assert_eq!(pack.hits[0].file, "buffer/live.rs");
    let same = client
        .index_file("buffer/live.rs", "pub fn quokka_pelican_shuffle() -> u32 { 41 }\n")
        .expect("repeat");
    assert_eq!(same.files_unchanged, 1);
    assert_eq!(same.files_indexed, 0);
    let changed = client
        .index_file("buffer/live.rs", "pub fn quokka_pelican_shuffle() -> u32 { 42 }\n")
        .expect("edit");
    assert_eq!(changed.files_indexed, 1);
}

#[test]
fn index_file_distrusts_its_content_and_its_path() {
    let (_dir, host) = open_host("idxf-distrust");
    host.serve(ServeConfig {
        max_frame: 4 << 20,
        ..ServeConfig::default()
    })
    .expect("serve");
    let (client_end, server_end) = ChannelTransport::pair();
    host.attach(server_end).expect("attach");
    let mut client =
        Client::connect_with(client_end, 4 << 20, Duration::from_secs(10)).expect("connect");
    let binary = client.index_file("blob.bin", "abc\0def").expect("binary");
    assert_eq!(binary.files_skipped, 1);
    assert_eq!(binary.files_indexed, 0);
    let big = "x ".repeat(600_000);
    let huge = client.index_file("huge.txt", &big).expect("huge");
    assert_eq!(huge.files_skipped, 1);
    assert_eq!(huge.files_indexed, 0);
    let empty = client.index_file("empty.txt", "").expect("empty content");
    assert_eq!(
        empty.files_indexed + empty.files_skipped + empty.files_unchanged,
        1
    );
    for bad in ["", "a\0b", "line\nbreak", &"p".repeat(5_000)] {
        match client.index_file(bad, "fn x() {}") {
            Err(HostError::Remote {
                code: ErrorCode::Index,
                ..
            }) => {}
            other => panic!("path {bad:?} was not refused: {other:?}"),
        }
    }
    assert_eq!(client.stats().expect("stats").files, 1);
    client.ping().expect("connection still usable");
    drop(host);
}

#[test]
fn index_file_over_the_serve_frame_cap_is_refused_and_the_connection_survives() {
    let (_dir, host) = open_host("idxf-cap");
    host.serve(ServeConfig {
        queue_depth: 8,
        workers: 2,
        max_frame: 4_096,
    })
    .expect("serve");
    let (client_end, server_end) = ChannelTransport::pair();
    host.attach(server_end).expect("attach");
    let mut client =
        Client::connect_with(client_end, 1 << 20, Duration::from_secs(10)).expect("connect");
    let within_cap = "a ".repeat(1_000);
    assert_eq!(
        client.index_file("ok.txt", &within_cap).expect("fits").files_indexed,
        1
    );
    let over_cap = "a ".repeat(3_000);
    match client.index_file("over.txt", &over_cap) {
        Err(HostError::Remote {
            code: ErrorCode::TooLarge,
            ..
        }) => {}
        other => panic!("an index_file above the serve cap was not refused: {other:?}"),
    }
    assert_eq!(client.stats().expect("stats").files, 1);
}
