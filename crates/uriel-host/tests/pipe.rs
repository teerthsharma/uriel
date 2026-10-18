#![cfg(windows)]

mod common;

use std::collections::BTreeMap;

use common::fixture;
use uriel_host::{
    AccessMode, Client, Host, PipeTransport, SearchOptions, Value, DEFAULT_MAX_FRAME,
};

fn unique(tag: &str) -> String {
    format!("uriel-pipe-{}-{tag}", std::process::id())
}

#[test]
fn stub_host_round_trip_over_a_named_pipe() {
    let (_dir, config) = fixture(&unique("round-trip"));
    let host = Host::open(config).expect("open");
    let path = host.listen_pipe().expect("listen pipe");
    assert!(path.starts_with(r"\\.\pipe\"));

    let transport = PipeTransport::connect(&path, DEFAULT_MAX_FRAME).expect("connect pipe");
    let mut client = Client::connect(transport).expect("handshake ping");
    client.ping().expect("ping");

    let none: BTreeMap<String, Value> = BTreeMap::new();
    let one = client
        .query("RETURN 1 AS x", &none, AccessMode::Read)
        .expect("query");
    assert_eq!(one.rows, vec![vec![Value::Int(1)]]);

    let pack = client
        .search("rolling sum over a byte buffer", &SearchOptions::default())
        .expect("search");
    assert_eq!(pack.hits[0].file, "a.rs");

    let stats = client.stats().expect("stats");
    assert!(stats.files >= 2);
    assert_eq!(stats.connections, 1);
}
