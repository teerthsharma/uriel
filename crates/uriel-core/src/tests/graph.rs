//! Core graph behaviour: CRUD, adjacency, transactions, persistence, concurrency.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::{mem, p, raw_keys, raw_len, tmp_graph_path};
use crate::tables::{COUNTS, IN_ADJ, LABEL_IDX, NODES, OUT_ADJ, PROP_IDX, RELS, TYPE_IDX};
use crate::{Error, Graph, Node, NodeId, ReadTx, RelId, Value, WriteTx};

fn node_ids(r: &ReadTx, label: &str) -> Vec<NodeId> {
    let mut v: Vec<NodeId> = r
        .nodes_by_label(label)
        .unwrap()
        .map(|n| n.unwrap().id)
        .collect();
    v.sort();
    v
}

fn all_clean(g: &Graph) {
    let problems = g.begin_read().unwrap().check_integrity().unwrap();
    assert!(problems.is_empty(), "integrity problems: {problems:#?}");
}

#[test]
fn handles_are_thread_safe() {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>() {}
    send_sync::<Graph>();
    send::<ReadTx>();
    send::<WriteTx>();
}

#[test]
fn nodes_get_monotonic_ids_and_round_trip_labels_and_props() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(
            &["Person", "Admin"],
            p(&[("name", "Ada".into()), ("age", 36.into())]),
        )
        .unwrap();
    let b = tx.create_node(&[], p(&[])).unwrap();
    let c = tx.create_node(&["Person", "Person"], p(&[])).unwrap();
    assert_eq!((a, b, c), (NodeId(0), NodeId(1), NodeId(2)));
    tx.commit().unwrap();

    let r = g.begin_read().unwrap();
    let node = r.get_node(a).unwrap().expect("node a");
    assert_eq!(node.id, a);
    assert_eq!(
        node.labels.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["Admin", "Person"]
    );
    assert_eq!(node.props, p(&[("name", "Ada".into()), ("age", 36.into())]));
    assert!(r.get_node(b).unwrap().unwrap().labels.is_empty());
    assert_eq!(
        r.get_node(c).unwrap().unwrap().labels.len(),
        1,
        "duplicate labels collapse"
    );
    assert!(r.get_node(NodeId(99)).unwrap().is_none());
    assert_eq!(r.node_count().unwrap(), 3);
    all_clean(&g);
}

#[test]
fn null_properties_are_never_stored() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(&["T"], p(&[("keep", 1.into()), ("drop", Value::Null)]))
        .unwrap();
    tx.commit().unwrap();
    let r = g.begin_read().unwrap();
    assert_eq!(
        r.get_node(a).unwrap().unwrap().props,
        p(&[("keep", 1.into())])
    );
    drop(r);

    let mut tx = g.begin_write().unwrap();
    tx.set_node_props(a, p(&[("keep", Value::Null), ("new", "x".into())]))
        .unwrap();
    tx.commit().unwrap();
    let r = g.begin_read().unwrap();
    assert_eq!(
        r.get_node(a).unwrap().unwrap().props,
        p(&[("new", "x".into())])
    );
}

#[test]
fn set_node_props_merges_and_missing_node_is_an_error() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(&["T"], p(&[("a", 1.into()), ("b", 2.into())]))
        .unwrap();
    tx.set_node_props(a, p(&[("b", 20.into()), ("c", 30.into())]))
        .unwrap();
    assert!(matches!(
        tx.set_node_props(NodeId(77), p(&[])),
        Err(Error::NodeNotFound(NodeId(77)))
    ));
    assert_eq!(
        tx.get_node(a).unwrap().unwrap().props,
        p(&[("a", 1.into()), ("b", 20.into()), ("c", 30.into())])
    );
    tx.commit().unwrap();
}

#[test]
fn labels_can_be_added_and_removed_and_the_label_index_follows() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["A"], p(&[])).unwrap();
    let b = tx.create_node(&["A", "B"], p(&[])).unwrap();
    tx.add_label(a, "B").unwrap();
    tx.add_label(a, "B").unwrap(); // idempotent
    tx.remove_label(b, "A").unwrap();
    tx.remove_label(b, "Never").unwrap(); // idempotent
    tx.commit().unwrap();

    let r = g.begin_read().unwrap();
    assert_eq!(node_ids(&r, "A"), vec![a]);
    assert_eq!(node_ids(&r, "B"), vec![a, b]);
    assert!(node_ids(&r, "Unknown").is_empty());
    assert_eq!(r.count_nodes_with_label("A").unwrap(), 1);
    assert_eq!(r.count_nodes_with_label("B").unwrap(), 2);
    assert_eq!(r.labels().unwrap(), vec!["A".to_string(), "B".to_string()]);
    assert_eq!(r.node_ids_by_label("B").unwrap().count(), 2);
    drop(r);
    all_clean(&g);

    // removing the last holder of a label retires it from the catalog
    let mut tx = g.begin_write().unwrap();
    tx.remove_label(a, "A").unwrap();
    tx.commit().unwrap();
    let r = g.begin_read().unwrap();
    assert_eq!(r.labels().unwrap(), vec!["B".to_string()]);
    assert_eq!(r.count_nodes_with_label("A").unwrap(), 0);
}

#[test]
fn relationships_are_reachable_by_range_scan_in_both_directions() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["P"], p(&[])).unwrap();
    let b = tx.create_node(&["P"], p(&[])).unwrap();
    let c = tx.create_node(&["P"], p(&[])).unwrap();
    let r1 = tx
        .create_rel(a, b, "KNOWS", p(&[("since", 2020.into())]))
        .unwrap();
    let r2 = tx.create_rel(a, b, "KNOWS", p(&[])).unwrap(); // parallel
    let r3 = tx.create_rel(a, c, "KNOWS2", p(&[])).unwrap(); // type with KNOWS as prefix
    let r4 = tx.create_rel(c, a, "LIKES", p(&[])).unwrap();
    let r5 = tx.create_rel(a, a, "SELF", p(&[])).unwrap();
    assert_eq!(
        (r1, r2, r3, r4, r5),
        (RelId(0), RelId(1), RelId(2), RelId(3), RelId(4))
    );
    tx.commit().unwrap();

    let r = g.begin_read().unwrap();
    let ids = |it: crate::RelIter| -> Vec<RelId> {
        let mut v: Vec<_> = it.map(|x| x.unwrap().id).collect();
        v.sort();
        v
    };
    assert_eq!(ids(r.out_rels(a, None).unwrap()), vec![r1, r2, r3, r5]);
    assert_eq!(
        ids(r.out_rels(a, Some("KNOWS")).unwrap()),
        vec![r1, r2],
        "KNOWS must not match KNOWS2"
    );
    assert_eq!(ids(r.out_rels(a, Some("KNOWS2")).unwrap()), vec![r3]);
    assert_eq!(ids(r.out_rels(a, Some("NOPE")).unwrap()), vec![]);
    assert_eq!(ids(r.in_rels(a, None).unwrap()), vec![r4, r5]);
    assert_eq!(ids(r.in_rels(b, Some("KNOWS")).unwrap()), vec![r1, r2]);
    assert_eq!(ids(r.in_rels(b, Some("LIKES")).unwrap()), vec![]);
    assert_eq!(ids(r.out_rels(b, None).unwrap()), vec![]);
    assert_eq!(ids(r.out_rels(NodeId(1000), None).unwrap()), vec![]);

    let rel = r.get_rel(r1).unwrap().unwrap();
    assert_eq!((rel.src, rel.dst, rel.rel_type.as_str()), (a, b, "KNOWS"));
    assert_eq!(rel.props, p(&[("since", 2020.into())]));
    assert!(r.get_rel(RelId(99)).unwrap().is_none());

    let edges: Vec<_> = r
        .out_edges(a, Some("KNOWS"))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|e| e.other == b && e.rel_type == "KNOWS"));
    let edges: Vec<_> = r.in_edges(a, None).unwrap().map(Result::unwrap).collect();
    assert_eq!(edges.iter().map(|e| e.other).collect::<Vec<_>>().len(), 2);
    assert!(edges.iter().any(|e| e.other == c && e.rel == r4));
    assert!(edges.iter().any(|e| e.other == a && e.rel == r5));

    assert_eq!(r.rel_count().unwrap(), 5);
    assert_eq!(r.count_rels_with_type("KNOWS").unwrap(), 2);
    assert_eq!(
        r.rel_types().unwrap(),
        vec!["KNOWS", "KNOWS2", "LIKES", "SELF"]
    );
    assert_eq!(r.all_rels().unwrap().count(), 5);
    drop(r);
    all_clean(&g);
}

#[test]
fn create_rel_requires_both_endpoints() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&[], p(&[])).unwrap();
    assert!(matches!(
        tx.create_rel(a, NodeId(5), "T", p(&[])),
        Err(Error::NodeNotFound(NodeId(5)))
    ));
    assert!(matches!(
        tx.create_rel(NodeId(6), a, "T", p(&[])),
        Err(Error::NodeNotFound(NodeId(6)))
    ));
    // failed calls consumed nothing: the next id is still 0
    assert_eq!(tx.create_rel(a, a, "T", p(&[])).unwrap(), RelId(0));
    tx.commit().unwrap();
    all_clean(&g);
}

#[test]
fn rel_props_merge_and_delete_rel_removes_every_trace() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&[], p(&[])).unwrap();
    let b = tx.create_node(&[], p(&[])).unwrap();
    let r1 = tx.create_rel(a, b, "T", p(&[("w", 1.into())])).unwrap();
    tx.set_rel_props(r1, p(&[("w", Value::Null), ("x", 2.into())]))
        .unwrap();
    assert!(matches!(
        tx.set_rel_props(RelId(9), p(&[])),
        Err(Error::RelNotFound(RelId(9)))
    ));
    tx.commit().unwrap();
    let r = g.begin_read().unwrap();
    assert_eq!(r.get_rel(r1).unwrap().unwrap().props, p(&[("x", 2.into())]));
    drop(r);

    let mut tx = g.begin_write().unwrap();
    tx.delete_rel(r1).unwrap();
    assert!(matches!(tx.delete_rel(r1), Err(Error::RelNotFound(_))));
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, RELS), 0);
    assert_eq!(raw_len(&g, OUT_ADJ), 0);
    assert_eq!(raw_len(&g, IN_ADJ), 0);
    assert_eq!(raw_len(&g, TYPE_IDX), 0);
    assert_eq!(raw_len(&g, COUNTS), 0);
    let r = g.begin_read().unwrap();
    assert_eq!(r.rel_count().unwrap(), 0);
    assert!(r.rel_types().unwrap().is_empty());
    assert_eq!(r.out_rels(a, None).unwrap().count(), 0);
    drop(r);
    all_clean(&g);
}

#[test]
fn delete_node_refuses_while_relationships_remain_and_the_tx_stays_usable() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["P"], p(&[])).unwrap();
    let b = tx.create_node(&["P"], p(&[])).unwrap();
    let rel = tx.create_rel(a, b, "T", p(&[])).unwrap();
    tx.commit().unwrap();

    let mut tx = g.begin_write().unwrap();
    assert!(matches!(tx.delete_node(a), Err(Error::NodeHasRelationships(n)) if n == a));
    assert!(
        matches!(tx.delete_node(b), Err(Error::NodeHasRelationships(n)) if n == b),
        "incoming only"
    );
    assert!(matches!(
        tx.delete_node(NodeId(50)),
        Err(Error::NodeNotFound(_))
    ));
    tx.delete_rel(rel).unwrap();
    tx.delete_node(a).unwrap();
    tx.commit().unwrap();

    let r = g.begin_read().unwrap();
    assert!(r.get_node(a).unwrap().is_none());
    assert!(r.get_node(b).unwrap().is_some());
    assert_eq!(node_ids(&r, "P"), vec![b]);
    drop(r);
    all_clean(&g);
}

#[test]
fn detach_delete_leaves_no_dangling_adjacency_or_index_entries() {
    let g = mem();
    g.create_index("Person", "name").unwrap();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(&["Person"], p(&[("name", "a".into())]))
        .unwrap();
    let b = tx
        .create_node(&["Person"], p(&[("name", "b".into())]))
        .unwrap();
    let c = tx
        .create_node(&["Person"], p(&[("name", "c".into())]))
        .unwrap();
    tx.create_rel(a, b, "KNOWS", p(&[])).unwrap();
    tx.create_rel(b, a, "KNOWS", p(&[])).unwrap();
    tx.create_rel(a, a, "SELF", p(&[])).unwrap();
    tx.create_rel(c, a, "LIKES", p(&[])).unwrap();
    let keep = tx.create_rel(b, c, "FOLLOWS", p(&[])).unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 3);
    assert_eq!(raw_len(&g, OUT_ADJ), 5);

    let mut tx = g.begin_write().unwrap();
    tx.detach_delete_node(a).unwrap();
    assert!(matches!(
        tx.detach_delete_node(a),
        Err(Error::NodeNotFound(_))
    ));
    let touched = tx.touched().clone();
    assert!(touched.rel_types.contains("SELF") && touched.rel_types.contains("LIKES"));
    tx.commit().unwrap();

    // raw tables, not the API: nothing may be left behind
    assert_eq!(raw_len(&g, NODES), 2);
    assert_eq!(raw_len(&g, RELS), 1);
    assert_eq!(raw_len(&g, OUT_ADJ), 1);
    assert_eq!(raw_len(&g, IN_ADJ), 1);
    assert_eq!(raw_len(&g, TYPE_IDX), 1);
    assert_eq!(raw_len(&g, LABEL_IDX), 2);
    assert_eq!(raw_len(&g, PROP_IDX), 2);
    for key in raw_keys(&g, OUT_ADJ)
        .into_iter()
        .chain(raw_keys(&g, IN_ADJ))
    {
        let node = u64::from_be_bytes(key[..8].try_into().unwrap());
        assert!(
            node == b.0 || node == c.0,
            "adjacency entry for deleted node {node}"
        );
    }

    let r = g.begin_read().unwrap();
    assert_eq!(r.node_count().unwrap(), 2);
    assert_eq!(r.rel_count().unwrap(), 1);
    assert_eq!(r.rel_types().unwrap(), vec!["FOLLOWS"]);
    assert_eq!(r.count_rels_with_type("KNOWS").unwrap(), 0);
    assert_eq!(
        r.out_rels(b, None)
            .unwrap()
            .map(|x| x.unwrap().id)
            .collect::<Vec<_>>(),
        vec![keep]
    );
    assert_eq!(r.in_rels(c, None).unwrap().count(), 1);
    assert_eq!(r.in_rels(b, None).unwrap().count(), 0);
    assert_eq!(
        r.nodes_by_prop("Person", "name", &"a".into())
            .unwrap()
            .count(),
        0
    );
    drop(r);
    all_clean(&g);
}

#[test]
fn all_nodes_all_rels_and_counts() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let ids: Vec<NodeId> = (0..5)
        .map(|i| tx.create_node(&["N"], p(&[("i", i.into())])).unwrap())
        .collect();
    for w in ids.windows(2) {
        tx.create_rel(w[0], w[1], "NEXT", p(&[])).unwrap();
    }
    tx.commit().unwrap();
    let r = g.begin_read().unwrap();
    let nodes: Vec<Node> = r.all_nodes().unwrap().map(Result::unwrap).collect();
    assert_eq!(
        nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
        ids,
        "all_nodes is in id order"
    );
    assert_eq!(r.node_count().unwrap(), 5);
    assert_eq!(r.rel_count().unwrap(), 4);
    assert_eq!(r.all_rels().unwrap().count(), 4);
}

#[test]
fn uncommitted_changes_roll_back_on_drop() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["Temp"], p(&[("x", 1.into())])).unwrap();
    tx.create_rel(a, a, "LOOP", p(&[])).unwrap();
    assert_eq!(
        tx.out_edges(a, None).unwrap().len(),
        1,
        "visible inside the tx"
    );
    drop(tx);

    let r = g.begin_read().unwrap();
    assert_eq!(r.node_count().unwrap(), 0);
    assert_eq!(r.rel_count().unwrap(), 0);
    assert!(r.get_node(a).unwrap().is_none());
    assert!(r.labels().unwrap().is_empty());
    assert_eq!(r.epoch(), 0);
    drop(r);
    assert_eq!(g.epoch(), 0);
    assert_eq!(raw_len(&g, NODES), 0);
    assert_eq!(raw_len(&g, LABEL_IDX), 0);

    // id counters rolled back too
    let mut tx = g.begin_write().unwrap();
    assert_eq!(tx.create_node(&[], p(&[])).unwrap(), NodeId(0));
    tx.rollback();
    assert_eq!(g.begin_read().unwrap().node_count().unwrap(), 0);
}

#[test]
fn rollback_restores_a_populated_graph_exactly() {
    let g = mem();
    g.create_index("P", "k").unwrap();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["P"], p(&[("k", 1.into())])).unwrap();
    let b = tx.create_node(&["P"], p(&[("k", 2.into())])).unwrap();
    let rel = tx.create_rel(a, b, "T", p(&[])).unwrap();
    tx.commit().unwrap();
    let epoch = g.epoch();

    let mut tx = g.begin_write().unwrap();
    tx.delete_rel(rel).unwrap();
    tx.detach_delete_node(b).unwrap();
    tx.set_node_props(a, p(&[("k", 9.into())])).unwrap();
    tx.create_node(&["Q"], p(&[])).unwrap();
    drop(tx);

    assert_eq!(g.epoch(), epoch);
    let r = g.begin_read().unwrap();
    assert_eq!(r.node_count().unwrap(), 2);
    assert_eq!(r.get_node(a).unwrap().unwrap().props, p(&[("k", 1.into())]));
    assert_eq!(r.out_rels(a, None).unwrap().count(), 1);
    assert_eq!(r.nodes_by_prop("P", "k", &2.into()).unwrap().count(), 1);
    drop(r);
    all_clean(&g);
}

#[test]
fn readers_see_a_stable_snapshot_while_a_writer_commits() {
    let g = mem();
    let before = g.begin_read().unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["X"], p(&[])).unwrap();
    // a reader started while a write tx is open must not block and sees nothing of it
    let during = g.begin_read().unwrap();
    assert_eq!(during.node_count().unwrap(), 0);
    tx.commit().unwrap();
    assert_eq!(before.node_count().unwrap(), 0, "old snapshot unchanged");
    assert_eq!(during.node_count().unwrap(), 0);
    assert_eq!(g.begin_read().unwrap().node_count().unwrap(), 1);
}

#[test]
fn only_one_writer_at_a_time() {
    let g = mem();
    let tx = g.begin_write().unwrap();
    let (tx_acquired, rx_acquired) = mpsc::channel();
    let g2 = g.clone();
    let handle = thread::spawn(move || {
        let tx2 = g2.begin_write().unwrap();
        tx_acquired.send(Instant::now()).unwrap();
        tx2.commit().unwrap();
    });
    assert!(
        rx_acquired
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "second writer must wait for the first"
    );
    drop(tx);
    rx_acquired
        .recv_timeout(Duration::from_secs(10))
        .expect("second writer proceeds after release");
    handle.join().unwrap();
}

#[test]
fn concurrent_writers_serialize_without_losing_updates() {
    let g = mem();
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let g = g.clone();
            thread::spawn(move || {
                for i in 0..25 {
                    let mut tx = g.begin_write().unwrap();
                    tx.create_node(&["W"], p(&[("t", t.into()), ("i", i.into())]))
                        .unwrap();
                    tx.commit().unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let r = g.begin_read().unwrap();
    assert_eq!(r.node_count().unwrap(), 100);
    let mut ids: Vec<u64> = r.all_nodes().unwrap().map(|n| n.unwrap().id.0).collect();
    ids.dedup();
    assert_eq!(ids, (0..100).collect::<Vec<_>>());
    assert_eq!(g.epoch(), 100);
}

#[test]
fn readers_on_other_threads_run_while_a_write_is_open() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&[], p(&[])).unwrap();
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let g = g.clone();
            thread::spawn(move || g.begin_read().unwrap().node_count().unwrap())
        })
        .collect();
    for h in readers {
        assert_eq!(h.join().unwrap(), 0);
    }
    tx.commit().unwrap();
}

#[test]
fn data_ids_and_epoch_survive_reopen_and_ids_are_never_reused() {
    let (_dir, path) = tmp_graph_path();
    let (a, c, rel);
    {
        let g = Graph::open(&path).unwrap();
        let mut tx = g.begin_write().unwrap();
        a = tx
            .create_node(&["Person"], p(&[("name", "Ada".into())]))
            .unwrap();
        let b = tx.create_node(&["Person"], p(&[])).unwrap();
        c = tx.create_node(&["Person"], p(&[])).unwrap();
        rel = tx
            .create_rel(a, b, "KNOWS", p(&[("w", 0.5.into())]))
            .unwrap();
        tx.create_rel(b, c, "KNOWS", p(&[])).unwrap();
        tx.commit().unwrap();
        // delete the highest ids so a naive "max + 1" allocator would reuse them
        let mut tx = g.begin_write().unwrap();
        tx.detach_delete_node(c).unwrap();
        tx.commit().unwrap();
        assert_eq!(g.epoch(), 2);
    }
    let g = Graph::open(&path).unwrap();
    assert_eq!(g.epoch(), 2);
    let r = g.begin_read().unwrap();
    assert_eq!(r.node_count().unwrap(), 2);
    assert_eq!(r.rel_count().unwrap(), 1);
    assert_eq!(
        r.get_node(a).unwrap().unwrap().props,
        p(&[("name", "Ada".into())])
    );
    assert_eq!(
        r.get_rel(rel).unwrap().unwrap().props,
        p(&[("w", 0.5.into())])
    );
    assert_eq!(r.out_rels(a, Some("KNOWS")).unwrap().count(), 1);
    drop(r);
    let mut tx = g.begin_write().unwrap();
    let n = tx.create_node(&[], p(&[])).unwrap();
    let r2 = tx.create_rel(a, n, "T", p(&[])).unwrap();
    assert_eq!(n, NodeId(3), "node ids are not reused");
    assert_eq!(r2, RelId(2), "rel ids are not reused");
    tx.commit().unwrap();
    all_clean(&g);
}

#[test]
fn a_database_file_cannot_be_opened_twice_at_once() {
    let (_dir, path) = tmp_graph_path();
    let _g = Graph::open(&path).unwrap();
    assert!(matches!(Graph::open(&path), Err(Error::Storage(_))));
}

#[test]
fn opening_a_path_in_a_missing_directory_fails_cleanly() {
    let (dir, _) = tmp_graph_path();
    let bad = dir.path().join("no-such-dir").join("x.redb");
    assert!(matches!(Graph::open(bad), Err(Error::Storage(_))));
}

#[test]
fn invalid_names_and_values_are_rejected_without_side_effects() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    assert!(matches!(
        tx.create_node(&["bad\0label"], p(&[])),
        Err(Error::InvalidName(_))
    ));
    assert!(matches!(
        tx.create_node(&[""], p(&[])),
        Err(Error::InvalidName(_))
    ));
    assert!(matches!(
        tx.create_node(&["Ok"], p(&[("bad\0key", 1.into())])),
        Err(Error::InvalidName(_))
    ));
    assert!(matches!(
        tx.create_node(&["Ok"], p(&[("", 1.into())])),
        Err(Error::InvalidName(_))
    ));
    let mut deep = Value::Int(1);
    for _ in 0..100 {
        deep = Value::List(vec![deep]);
    }
    assert!(matches!(
        tx.create_node(&["Ok"], p(&[("d", deep)])),
        Err(Error::InvalidValue(_))
    ));
    let a = tx.create_node(&[], p(&[])).unwrap();
    assert_eq!(a, NodeId(0), "failed creates must not consume ids");
    assert!(matches!(
        tx.create_rel(a, a, "bad\0type", p(&[])),
        Err(Error::InvalidName(_))
    ));
    assert!(matches!(
        tx.create_rel(a, a, "", p(&[])),
        Err(Error::InvalidName(_))
    ));
    assert!(matches!(tx.add_label(a, ""), Err(Error::InvalidName(_))));
    tx.commit().unwrap();
    assert_eq!(g.begin_read().unwrap().node_count().unwrap(), 1);
    all_clean(&g);
}

#[test]
fn ten_thousand_nodes_bulk_insert_and_traversal() {
    let g = mem();
    let n: u64 = 10_000;
    let mut tx = g.begin_write().unwrap();
    let ids: Vec<NodeId> = (0..n)
        .map(|i| {
            tx.create_node(&["Item"], p(&[("i", (i as i64).into())]))
                .unwrap()
        })
        .collect();
    for i in 0..(n - 1) as usize {
        tx.create_rel(ids[i], ids[i + 1], "NEXT", p(&[])).unwrap();
    }
    for i in 0..(n - 7) as usize {
        tx.create_rel(ids[i], ids[i + 7], "SKIP", p(&[])).unwrap();
    }
    tx.commit().unwrap();

    let r = g.begin_read().unwrap();
    assert_eq!(r.node_count().unwrap(), n);
    assert_eq!(r.rel_count().unwrap(), (n - 1) + (n - 7));
    assert_eq!(r.count_nodes_with_label("Item").unwrap(), n);
    assert_eq!(r.nodes_by_label("Item").unwrap().count() as u64, n);
    assert_eq!(r.count_rels_with_type("SKIP").unwrap(), n - 7);

    // walk the whole chain using only adjacency range scans
    let mut cur = ids[0];
    let mut steps = 0;
    loop {
        let mut next = r.out_edges(cur, Some("NEXT")).unwrap();
        match next.next() {
            Some(e) => {
                assert!(next.next().is_none());
                cur = e.unwrap().other;
                steps += 1;
            }
            None => break,
        }
    }
    assert_eq!(steps, n - 1);
    assert_eq!(cur, ids[(n - 1) as usize]);

    // each interior node has exactly the expected typed degree
    for i in [0usize, 1, 5_000, 9_992, 9_993, 9_999] {
        let skip = r.out_rels(ids[i], Some("SKIP")).unwrap().count();
        assert_eq!(skip, usize::from(i < (n - 7) as usize), "node {i}");
        let into = r.in_rels(ids[i], Some("NEXT")).unwrap().count();
        assert_eq!(into, usize::from(i > 0), "node {i}");
        let all = r.out_rels(ids[i], None).unwrap().count();
        assert_eq!(
            all,
            usize::from(i < (n - 1) as usize) + usize::from(i < (n - 7) as usize)
        );
    }
    drop(r);
    all_clean(&g);
}

#[test]
fn check_integrity_is_not_vacuous_it_detects_planted_corruption() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["Real"], p(&[])).unwrap();
    tx.commit().unwrap();
    all_clean(&g);

    // dangling label index entry
    let raw = g.db().begin_write().unwrap();
    {
        let mut t = raw.open_table(LABEL_IDX).unwrap();
        let mut key = b"Ghost\0".to_vec();
        key.extend_from_slice(&99u64.to_be_bytes());
        t.insert(key.as_slice(), ()).unwrap();
    }
    raw.commit().unwrap();
    let problems = g.begin_read().unwrap().check_integrity().unwrap();
    assert!(!problems.is_empty());
    assert!(problems.iter().any(|m| m.contains("Ghost")), "{problems:?}");

    // adjacency entry pointing at a rel that does not exist
    let raw = g.db().begin_write().unwrap();
    {
        let mut t = raw.open_table(OUT_ADJ).unwrap();
        let mut key = a.0.to_be_bytes().to_vec();
        key.extend_from_slice(b"T\0");
        key.extend_from_slice(&7u64.to_be_bytes());
        t.insert(key.as_slice(), a.0).unwrap();
    }
    raw.commit().unwrap();
    let problems = g.begin_read().unwrap().check_integrity().unwrap();
    assert!(problems.len() >= 2, "{problems:?}");
}

#[test]
fn write_tx_reads_its_own_writes() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["A"], p(&[("v", 1.into())])).unwrap();
    let b = tx.create_node(&["B"], p(&[])).unwrap();
    let rel = tx.create_rel(a, b, "T", p(&[])).unwrap();
    assert_eq!(
        tx.get_node(a).unwrap().unwrap().props,
        p(&[("v", 1.into())])
    );
    assert_eq!(tx.get_rel(rel).unwrap().unwrap().dst, b);
    assert_eq!(tx.out_edges(a, Some("T")).unwrap()[0].other, b);
    assert_eq!(tx.in_edges(b, None).unwrap()[0].other, a);
    assert_eq!(tx.node_ids_by_label("A").unwrap(), vec![a]);
    assert!(tx.get_node(NodeId(40)).unwrap().is_none());
    tx.commit().unwrap();
}

#[test]
fn iterators_keep_their_snapshot_alive_after_the_read_tx_is_dropped() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    for _ in 0..3 {
        tx.create_node(&["A"], p(&[])).unwrap();
    }
    tx.commit().unwrap();
    let (nodes, ids, rels) = {
        let r = g.begin_read().unwrap();
        (
            r.nodes_by_label("A").unwrap(),
            r.node_ids_by_label("A").unwrap(),
            r.all_rels().unwrap(),
        )
    };
    // later commits do not leak into the iterators taken from the earlier snapshot
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    tx.commit().unwrap();
    assert_eq!(nodes.count(), 3);
    assert_eq!(ids.count(), 3);
    assert_eq!(rels.count(), 0);
    assert_eq!(
        g.begin_read().unwrap().nodes_by_label("A").unwrap().count(),
        4
    );
}

#[test]
fn a_small_cache_option_is_accepted() {
    let (_dir, path) = tmp_graph_path();
    let g = Graph::open_with(
        &path,
        &crate::Options {
            cache_bytes: 1 << 20,
            ..crate::Options::default()
        },
    )
    .unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    tx.commit().unwrap();
    assert_eq!(g.begin_read().unwrap().node_count().unwrap(), 1);
}
