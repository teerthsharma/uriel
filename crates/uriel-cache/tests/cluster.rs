//! Multi-node integration tests on 127.0.0.1 with ephemeral ports.
//!
//! No fixed sleeps: every wait is a poll against a deadline.

use std::time::{Duration, Instant};

use uriel_cache::{CacheError, CacheMeta, CacheNode, GossipConfig, MemberState, NodeConfig, NodeId};

const CONVERGE: Duration = Duration::from_secs(15);

fn cfg(seeds: Vec<std::net::SocketAddr>) -> NodeConfig {
    let mut gossip = GossipConfig::fast();
    // Roomy enough that a loaded CI box does not declare healthy nodes dead.
    gossip.suspect_timeout = Duration::from_millis(800);
    NodeConfig {
        seeds,
        max_bytes: 4 * 1024 * 1024,
        replicas: 2,
        request_timeout: Duration::from_millis(500),
        gossip,
        ..NodeConfig::default()
    }
}

async fn eventually(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) -> Duration {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < timeout,
            "timed out after {timeout:?} waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    start.elapsed()
}

fn alive(node: &CacheNode) -> usize {
    node.members()
        .iter()
        .filter(|m| m.state == MemberState::Alive)
        .count()
}

fn state_of(node: &CacheNode, id: &NodeId) -> Option<MemberState> {
    node.members()
        .into_iter()
        .find(|m| &m.id == id)
        .map(|m| m.state)
}

async fn cluster(
    n: usize,
    mut make: impl FnMut(Vec<std::net::SocketAddr>) -> NodeConfig,
) -> Vec<CacheNode> {
    let first = CacheNode::spawn(make(vec![])).await.unwrap();
    let seed = first.local_addr();
    let mut nodes = vec![first];
    for _ in 1..n {
        nodes.push(CacheNode::spawn(make(vec![seed])).await.unwrap());
    }
    eventually("full membership convergence", CONVERGE, || {
        nodes.iter().all(|nd| alive(nd) == n)
    })
    .await;
    nodes
}

async fn stop_all(nodes: Vec<CacheNode>) {
    for n in nodes {
        n.abort().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_nodes_converge_on_the_same_membership() {
    let nodes = cluster(3, cfg).await;
    let mut expect: Vec<(NodeId, std::net::SocketAddr)> = nodes
        .iter()
        .map(|n| (n.id().clone(), n.local_addr()))
        .collect();
    expect.sort();
    for n in &nodes {
        let mut got: Vec<(NodeId, std::net::SocketAddr)> = n
            .members()
            .into_iter()
            .map(|m| {
                assert_eq!(m.state, MemberState::Alive);
                (m.id, m.addr)
            })
            .collect();
        got.sort();
        assert_eq!(got, expect, "node {} disagrees", n.id());
    }
    // Same membership => same ring => same owners everywhere.
    for i in 0..200 {
        let k = format!("key-{i}");
        let o0 = nodes[0].cache().owners(&k);
        assert_eq!(o0.len(), 2);
        for n in &nodes[1..] {
            assert_eq!(n.cache().owners(&k), o0);
        }
    }
    stop_all(nodes).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_on_a_is_gettable_from_c() {
    let nodes = cluster(3, cfg).await;
    let (a, c) = (&nodes[0], &nodes[2]);
    // 40 keys exercise every owner layout (A owns, C owns, neither, both).
    for i in 0..40 {
        let key = format!("query:{i}");
        let value = format!("result-{i}").into_bytes();
        a.cache()
            .put(&key, value.clone(), CacheMeta::new(1).with_tags(["Person"]))
            .await;
        let got = c
            .cache()
            .get(&key)
            .await
            .unwrap_or_else(|| panic!("{key} missing on C"));
        assert_eq!(&*got, &value[..]);
        assert!(
            c.cache().local().get(&key).is_some(),
            "remote hit must populate C locally"
        );
    }
    stop_all(nodes).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalidation_broadcast_clears_the_entry_on_all_nodes() {
    let nodes = cluster(3, cfg).await;
    let a = &nodes[0];
    let person = vec!["Person".to_string()];
    a.cache()
        .put("old", vec![1], CacheMeta::new(1).with_tags(["Person"]))
        .await;
    a.cache()
        .put("new", vec![2], CacheMeta::new(9).with_tags(["Person"]))
        .await;
    a.cache()
        .put("city", vec![3], CacheMeta::new(1).with_tags(["City"]))
        .await;
    // Spread copies to every node so there is something to clear everywhere.
    for n in &nodes {
        for k in ["old", "new", "city"] {
            assert!(
                n.cache().get(k).await.is_some(),
                "{k} should be reachable from {}",
                n.id()
            );
        }
    }
    for n in &nodes {
        assert!(
            n.cache().local().get("old").is_some(),
            "precondition: {} holds 'old'",
            n.id()
        );
    }

    let acks = a.cache().invalidate(&person, 5).await;
    assert_eq!(acks, 2, "both peers must ack the broadcast");

    for n in &nodes {
        assert!(
            n.cache().local().get("old").is_none(),
            "old (epoch 1 < 5) survives on {}",
            n.id()
        );
        assert!(
            n.cache().get("old").await.is_none(),
            "old reachable remotely from {}",
            n.id()
        );
        assert!(
            n.cache().local().get("new").is_some(),
            "new (epoch 9) was wrongly dropped on {}",
            n.id()
        );
        assert!(
            n.cache().local().get("city").is_some(),
            "other tag was wrongly dropped on {}",
            n.id()
        );
    }
    stop_all(nodes).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_node_is_marked_dead_and_its_keys_rehome() {
    let nodes = cluster(3, cfg).await;
    let c_id = nodes[2].id().clone();
    let keys: Vec<String> = (0..300)
        .map(|i| format!("k{i}"))
        .filter(|k| nodes[0].cache().owners(k).first() == Some(&c_id))
        .take(20)
        .collect();
    assert!(keys.len() >= 10, "C should be primary for a share of keys");
    let before: Vec<Vec<NodeId>> = keys.iter().map(|k| nodes[0].cache().owners(k)).collect();

    let mut it = nodes.into_iter();
    let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
    let killed_at = Instant::now();
    c.abort().await;

    let took = eventually("A and B mark C dead", CONVERGE, || {
        state_of(&a, &c_id) == Some(MemberState::Dead)
            && state_of(&b, &c_id) == Some(MemberState::Dead)
    })
    .await;
    eprintln!("kill -> dead on both survivors: {:?}", killed_at.elapsed());
    assert!(took < CONVERGE);
    assert_eq!(alive(&a), 2);
    assert_eq!(alive(&b), 2);

    for (k, old) in keys.iter().zip(&before) {
        let now = a.cache().owners(k);
        assert_eq!(now, b.cache().owners(k), "survivors agree on {k}");
        assert!(!now.contains(&c_id), "{k} still maps to the dead node");
        assert_eq!(now.len(), 2, "re-homed onto both survivors");
        assert_ne!(&now, old);
        // The re-homed key is usable end to end.
        a.cache()
            .put(k, k.as_bytes().to_vec(), CacheMeta::new(1))
            .await;
        assert_eq!(&*b.cache().get(k).await.unwrap(), k.as_bytes());
    }
    a.abort().await;
    b.abort().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreachable_owner_is_a_miss_within_the_timeout_never_an_error() {
    let nodes = cluster(3, |s| NodeConfig {
        replicas: 1,
        ..cfg(s)
    })
    .await;
    let c_id = nodes[2].id().clone();
    let keys: Vec<String> = (0..300)
        .map(|i| format!("k{i}"))
        .filter(|k| nodes[0].cache().owners(k) == vec![c_id.clone()])
        .take(20)
        .collect();
    assert!(!keys.is_empty());
    let mut it = nodes.into_iter();
    let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
    c.abort().await; // owner is gone but still in A's and B's ring for a while
    let bound = Duration::from_millis(500) + Duration::from_millis(750);
    for k in &keys {
        for n in [&a, &b] {
            let start = Instant::now();
            assert!(n.cache().get(k).await.is_none());
            assert!(start.elapsed() < bound, "get took {:?}", start.elapsed());
        }
        let start = Instant::now();
        a.cache().put(k, vec![1], CacheMeta::new(1)).await;
        assert!(start.elapsed() < bound, "put took {:?}", start.elapsed());
    }
    a.abort().await;
    b.abort().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_shutdown_is_announced_without_waiting_for_failure_detection() {
    let nodes = cluster(3, |s| {
        let mut c = cfg(s);
        c.gossip.suspect_timeout = Duration::from_secs(120);
        c
    })
    .await;
    let c_id = nodes[2].id().clone();
    let mut it = nodes.into_iter();
    let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
    c.shutdown().await;
    eventually("leave announced", Duration::from_secs(5), || {
        state_of(&a, &c_id) == Some(MemberState::Dead)
            && state_of(&b, &c_id) == Some(MemberState::Dead)
    })
    .await;
    a.abort().await;
    b.abort().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_joiner_learns_the_cluster_through_one_seed_and_serves_existing_data() {
    let nodes = cluster(2, cfg).await;
    nodes[0]
        .cache()
        .put("pre", vec![4, 2], CacheMeta::new(1))
        .await;
    let seed = nodes[1].local_addr();
    let joiner = CacheNode::spawn(cfg(vec![seed])).await.unwrap();
    eventually("joiner converges", CONVERGE, || {
        alive(&joiner) == 3 && nodes.iter().all(|n| alive(n) == 3)
    })
    .await;
    // Adding a node re-homes some keys (a cache does not migrate data), so the writer
    // repeats the put on the new ring; the joiner must then be able to read it.
    nodes[0]
        .cache()
        .put("pre", vec![4, 2], CacheMeta::new(1))
        .await;
    assert_eq!(&*joiner.cache().get("pre").await.unwrap(), &[4, 2]);
    joiner.abort().await;
    stop_all(nodes).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_configuration_and_port_conflicts_are_reported() {
    let err = CacheNode::spawn(NodeConfig {
        replicas: 0,
        ..NodeConfig::default()
    })
    .await
    .err()
    .expect("replicas = 0 must be rejected");
    assert!(matches!(err, CacheError::InvalidConfig(_)), "{err:?}");

    let err = CacheNode::spawn(NodeConfig {
        bind: "0.0.0.0:0".parse().unwrap(),
        ..NodeConfig::default()
    })
    .await
    .err()
    .expect("unspecified bind without advertise must be rejected");
    assert!(matches!(err, CacheError::InvalidConfig(_)), "{err:?}");

    let first = CacheNode::spawn(NodeConfig::default()).await.unwrap();
    let err = CacheNode::spawn(NodeConfig {
        bind: first.local_addr(),
        ..NodeConfig::default()
    })
    .await
    .err()
    .expect("second bind on the same port must fail");
    assert!(matches!(err, CacheError::Bind { .. }), "{err:?}");
    first.abort().await;
}
