mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{fixture, wait_until};
use uriel_host::{AccessMode, Client, Host, PipeTransport, ServeConfig, Value, DEFAULT_MAX_FRAME};

fn unique(tag: &str) -> String {
    format!("uriel-ipc-{}-{tag}", std::process::id())
}

fn serving(tag: &str) -> (tempfile::TempDir, Host, String) {
    let (dir, config) = fixture(&unique(tag));
    let host = Host::open(config).expect("open");
    host.serve(ServeConfig {
        queue_depth: 16,
        workers: 2,
        max_frame: DEFAULT_MAX_FRAME,
    })
    .expect("serve");
    let path = host.listen_pipe().expect("listen on the local ipc endpoint");
    (dir, host, path)
}

#[test]
fn the_local_endpoint_round_trips_a_query_on_every_os() {
    let (_dir, host, path) = serving("round-trip");
    assert_eq!(path, host.pipe_path());
    let transport = PipeTransport::connect(&path, DEFAULT_MAX_FRAME).expect("connect");
    let mut client = Client::connect_with(transport, DEFAULT_MAX_FRAME, Duration::from_secs(10)).expect("handshake");
    client.ping().expect("ping");
    let none: BTreeMap<String, Value> = BTreeMap::new();
    let one = client.query("RETURN 1 AS x", &none, AccessMode::Read).expect("query");
    assert_eq!(one.rows, vec![vec![Value::Int(1)]]);
    assert_eq!(client.stats().expect("stats").connections, 1);
}

#[test]
fn two_clients_share_one_endpoint_and_a_dropped_client_does_not_hurt_the_other() {
    let (_dir, host, path) = serving("two-clients");
    let mut a = Client::connect(PipeTransport::connect(&path, DEFAULT_MAX_FRAME).expect("a")).expect("a handshake");
    let b = Client::connect(PipeTransport::connect(&path, DEFAULT_MAX_FRAME).expect("b")).expect("b handshake");
    drop(b);
    a.ping().expect("ping after b left");
    wait_until("the dropped connection is reaped", || host.stats().is_ok_and(|s| s.connections == 1));
    a.ping().expect("still alive");
}

#[cfg(unix)]
#[test]
fn the_socket_is_owner_only_and_removed_on_shutdown() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, host, path) = serving("owner-only");
    let mode = std::fs::metadata(&path).expect("socket file").permissions().mode();
    assert_eq!(mode & 0o077, 0, "{mode:o}");
    host.shutdown();
    wait_until("socket file removed", || !std::path::Path::new(&path).exists());
}

#[cfg(unix)]
#[test]
fn a_stale_socket_file_from_a_dead_host_does_not_block_a_new_one() {
    let (_dir, config) = fixture(&unique("stale"));
    let stale = uriel_host::pipe_path(&config.sock_name);
    drop(std::os::unix::net::UnixListener::bind(&stale).expect("make a stale socket file"));
    assert!(std::path::Path::new(&stale).exists());
    let host = Host::open(config).expect("open");
    host.serve(ServeConfig { queue_depth: 4, workers: 1, max_frame: DEFAULT_MAX_FRAME }).expect("serve");
    let path = host.listen_pipe().expect("listen over the stale file");
    let mut client = Client::connect(PipeTransport::connect(&path, DEFAULT_MAX_FRAME).expect("connect")).expect("handshake");
    client.ping().expect("ping");
}
