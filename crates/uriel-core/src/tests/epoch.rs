//! Epoch counter, touched sets and commit callbacks.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::thread;

use super::{mem, p, tmp_graph_path};
use crate::{CommitInfo, Graph};

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn recorder(g: &Graph) -> Arc<Mutex<Vec<CommitInfo>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    g.on_commit(Box::new(move |info| {
        sink.lock().unwrap().push(info.clone())
    }));
    log
}

#[test]
fn epoch_starts_at_zero_and_counts_write_commits() {
    let g = mem();
    assert_eq!(g.epoch(), 0);
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    assert_eq!(g.epoch(), 0, "not visible before commit");
    assert_eq!(tx.commit().unwrap(), 1, "commit returns the new epoch");
    assert_eq!(g.epoch(), 1);
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    tx.commit().unwrap();
    assert_eq!(g.epoch(), 2);
    assert_eq!(g.begin_read().unwrap().epoch(), 2);
}

#[test]
fn rollbacks_and_empty_commits_do_not_advance_the_epoch() {
    let g = mem();
    let log = recorder(&g);
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    drop(tx);
    let tx = g.begin_write().unwrap();
    assert_eq!(
        tx.commit().unwrap(),
        0,
        "a commit that wrote nothing returns the unchanged epoch"
    );
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["A"], p(&[])).unwrap();
    tx.rollback();
    assert_eq!(g.epoch(), 0);
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn a_reader_keeps_the_epoch_of_its_snapshot() {
    let g = mem();
    let old = g.begin_read().unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&[], p(&[])).unwrap();
    tx.commit().unwrap();
    assert_eq!(old.epoch(), 0);
    assert_eq!(g.begin_read().unwrap().epoch(), 1);
}

#[test]
fn epoch_survives_reopen() {
    let (_dir, path) = tmp_graph_path();
    {
        let g = Graph::open(&path).unwrap();
        for _ in 0..3 {
            let mut tx = g.begin_write().unwrap();
            tx.create_node(&[], p(&[])).unwrap();
            tx.commit().unwrap();
        }
    }
    let g = Graph::open(&path).unwrap();
    assert_eq!(g.epoch(), 3);
    let log = recorder(&g);
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&[], p(&[])).unwrap();
    tx.commit().unwrap();
    assert_eq!(log.lock().unwrap()[0].epoch, 4);
}

#[test]
fn touched_tracks_labels_and_rel_types_for_every_kind_of_write() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    assert!(tx.touched().labels.is_empty() && tx.touched().rel_types.is_empty());
    let a = tx.create_node(&["A", "B"], p(&[])).unwrap();
    assert_eq!(tx.touched().labels, set(&["A", "B"]));
    assert!(tx.touched().rel_types.is_empty());
    let n = tx.create_node(&[], p(&[])).unwrap();
    tx.create_rel(a, n, "T", p(&[])).unwrap();
    assert_eq!(tx.touched().rel_types, set(&["T"]));
    tx.commit().unwrap();

    // each probe starts from a fresh tx so the sets are isolated
    let probe = |f: &dyn Fn(&mut crate::WriteTx)| -> (BTreeSet<String>, BTreeSet<String>) {
        let mut tx = g.begin_write().unwrap();
        f(&mut tx);
        let t = tx.touched().clone();
        drop(tx);
        (t.labels, t.rel_types)
    };
    assert_eq!(
        probe(&|tx| tx.set_node_props(a, p(&[("x", 1.into())])).unwrap()),
        (set(&["A", "B"]), set(&[]))
    );
    assert_eq!(
        probe(&|tx| tx.add_label(a, "C").unwrap()),
        (set(&["A", "B", "C"]), set(&[]))
    );
    assert_eq!(
        probe(&|tx| tx.remove_label(a, "B").unwrap()),
        (set(&["A", "B"]), set(&[]))
    );
    assert_eq!(
        probe(&|tx| tx.delete_rel(crate::RelId(0)).unwrap()),
        (set(&[]), set(&["T"]))
    );
    assert_eq!(
        probe(&|tx| tx
            .set_rel_props(crate::RelId(0), p(&[("w", 1.into())]))
            .unwrap()),
        (set(&[]), set(&["T"]))
    );
    assert_eq!(
        probe(&|tx| tx.detach_delete_node(a).unwrap()),
        (set(&["A", "B"]), set(&["T"]))
    );
    assert_eq!(
        probe(&|tx| tx.create_index("L", "k").unwrap()),
        (set(&["L"]), set(&[]))
    );
    assert_eq!(
        probe(&|tx| tx.create_exists_constraint("M", "k").unwrap()),
        (set(&["M"]), set(&[]))
    );
    // a failed op touches nothing
    assert_eq!(
        probe(&|tx| {
            let _ = tx.create_node(&["Bad\0"], p(&[]));
        }),
        (set(&[]), set(&[]))
    );
}

#[test]
fn callbacks_receive_epoch_and_touched_sets_after_the_commit_is_durable_and_visible() {
    let g = mem();
    let log = recorder(&g);
    let seen_nodes = Arc::new(Mutex::new(Vec::new()));
    {
        let g2 = g.clone();
        let sink = seen_nodes.clone();
        g.on_commit(Box::new(move |info| {
            // the committed data and the epoch are already visible from inside the callback
            let r = g2.begin_read().unwrap();
            sink.lock()
                .unwrap()
                .push((info.epoch, g2.epoch(), r.epoch(), r.node_count().unwrap()));
        }));
    }
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["Person"], p(&[])).unwrap();
    let b = tx.create_node(&["City"], p(&[])).unwrap();
    tx.create_rel(a, b, "LIVES_IN", p(&[])).unwrap();
    tx.commit().unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.add_label(a, "Admin").unwrap();
    tx.commit().unwrap();

    let log = log.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(
        log[0],
        CommitInfo {
            epoch: 1,
            labels: set(&["City", "Person"]),
            rel_types: set(&["LIVES_IN"])
        }
    );
    assert_eq!(
        log[1],
        CommitInfo {
            epoch: 2,
            labels: set(&["Admin", "Person"]),
            rel_types: set(&[])
        }
    );
    assert_eq!(
        *seen_nodes.lock().unwrap(),
        vec![(1, 1, 1, 2), (2, 2, 2, 2)]
    );
}

#[test]
fn subscribers_are_called_in_registration_order_and_can_unsubscribe() {
    let g = mem();
    let order = Arc::new(Mutex::new(Vec::new()));
    let o1 = order.clone();
    let id1 = g.on_commit(Box::new(move |_| o1.lock().unwrap().push("first")));
    let o2 = order.clone();
    let _id2 = g.subscribe_commits(Box::new(move |_| o2.lock().unwrap().push("second")));
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&[], p(&[])).unwrap();
    tx.commit().unwrap();
    assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    assert!(g.unsubscribe(id1));
    assert!(!g.unsubscribe(id1));
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&[], p(&[])).unwrap();
    tx.commit().unwrap();
    assert_eq!(*order.lock().unwrap(), vec!["first", "second", "second"]);
}

#[test]
fn a_callback_may_unsubscribe_itself_without_deadlock() {
    let g = mem();
    let id = Arc::new(Mutex::new(None));
    let (g2, id2) = (g.clone(), id.clone());
    let calls = Arc::new(Mutex::new(0));
    let c2 = calls.clone();
    let sub = g.on_commit(Box::new(move |_| {
        *c2.lock().unwrap() += 1;
        if let Some(s) = *id2.lock().unwrap() {
            g2.unsubscribe(s);
        }
    }));
    *id.lock().unwrap() = Some(sub);
    for _ in 0..3 {
        let mut tx = g.begin_write().unwrap();
        tx.create_node(&[], p(&[])).unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(*calls.lock().unwrap(), 1);
}

#[test]
fn callbacks_fire_in_epoch_order_under_concurrent_writers() {
    let g = mem();
    let epochs = Arc::new(Mutex::new(Vec::new()));
    let sink = epochs.clone();
    g.on_commit(Box::new(move |info| sink.lock().unwrap().push(info.epoch)));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let g = g.clone();
            thread::spawn(move || {
                for _ in 0..20 {
                    let mut tx = g.begin_write().unwrap();
                    tx.create_node(&["N"], p(&[])).unwrap();
                    tx.commit().unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(*epochs.lock().unwrap(), (1..=80).collect::<Vec<u64>>());
    assert_eq!(g.epoch(), 80);
}

#[test]
fn ddl_commits_bump_the_epoch_and_notify_with_the_affected_label() {
    let g = mem();
    let log = recorder(&g);
    g.create_index("Person", "name").unwrap();
    g.create_unique_constraint("Person", "email").unwrap();
    g.drop_index("Person", "name").unwrap();
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 3);
    assert!(log
        .iter()
        .all(|c| c.labels == set(&["Person"]) && c.rel_types.is_empty()));
    assert_eq!(
        log.iter().map(|c| c.epoch).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}
