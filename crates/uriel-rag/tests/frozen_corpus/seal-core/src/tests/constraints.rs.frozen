//! Uniqueness and existence constraints.

use super::{mem, p, raw_len, tmp_graph_path};
use crate::tables::{NODES, PROP_IDX};
use crate::{ConstraintDef, ConstraintKind, Error, Graph, NodeId, Value};

fn unique_violation(e: &Error) -> bool {
    matches!(
        e,
        Error::ConstraintViolation {
            kind: ConstraintKind::Unique,
            ..
        }
    )
}

fn exists_violation(e: &Error) -> bool {
    matches!(
        e,
        Error::ConstraintViolation {
            kind: ConstraintKind::Exists,
            ..
        }
    )
}

fn clean(g: &Graph) {
    let problems = g.begin_read().unwrap().check_integrity().unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn unique_constraint_rejects_duplicates_on_create_and_update() {
    let g = mem();
    g.create_unique_constraint("User", "email").unwrap();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(&["User"], p(&[("email", "a@x".into())]))
        .unwrap();
    let b = tx
        .create_node(&["User"], p(&[("email", "b@x".into())]))
        .unwrap();
    tx.commit().unwrap();

    let mut tx = g.begin_write().unwrap();
    let err = tx
        .create_node(&["User"], p(&[("email", "a@x".into())]))
        .unwrap_err();
    assert!(unique_violation(&err), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("User") && msg.contains("email"), "{msg}");
    let err = tx
        .set_node_props(b, p(&[("email", "a@x".into())]))
        .unwrap_err();
    assert!(unique_violation(&err));
    // writing a node's own current value is not a conflict
    tx.set_node_props(a, p(&[("email", "a@x".into()), ("n", 1.into())]))
        .unwrap();
    // different label, same key: unconstrained
    tx.create_node(&["Other"], p(&[("email", "a@x".into())]))
        .unwrap();
    // nodes without the key never conflict
    tx.create_node(&["User"], p(&[])).unwrap();
    tx.create_node(&["User"], p(&[])).unwrap();
    tx.commit().unwrap();
    clean(&g);
}

#[test]
fn unique_constraint_sees_writes_from_the_same_transaction() {
    let g = mem();
    g.create_unique_constraint("U", "k").unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    let err = tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap_err();
    assert!(unique_violation(&err));
    tx.commit().unwrap();
    assert_eq!(g.begin_read().unwrap().node_count().unwrap(), 1);
}

#[test]
fn unique_constraint_values_become_free_after_delete_or_change() {
    let g = mem();
    g.create_unique_constraint("U", "k").unwrap();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["U"], p(&[("k", "v".into())])).unwrap();
    tx.commit().unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.delete_node(a).unwrap();
    tx.create_node(&["U"], p(&[("k", "v".into())])).unwrap();
    tx.commit().unwrap();
    let mut tx = g.begin_write().unwrap();
    let b = tx.create_node(&["U"], p(&[("k", "w".into())])).unwrap();
    tx.set_node_props(b, p(&[("k", "x".into())])).unwrap();
    tx.create_node(&["U"], p(&[("k", "w".into())])).unwrap();
    tx.commit().unwrap();
    clean(&g);
}

#[test]
fn unique_constraint_compares_numbers_by_value_and_catches_label_additions() {
    let g = mem();
    g.create_unique_constraint("U", "k").unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    let err = tx.create_node(&["U"], p(&[("k", 1.0.into())])).unwrap_err();
    assert!(
        unique_violation(&err),
        "Int(1) and Float(1.0) are the same value"
    );
    let stray = tx.create_node(&["Stray"], p(&[("k", 1.into())])).unwrap();
    let err = tx.add_label(stray, "U").unwrap_err();
    assert!(
        unique_violation(&err),
        "gaining the label brings the node under the constraint"
    );
    assert!(
        !tx.get_node(stray).unwrap().unwrap().labels.contains("U"),
        "failed op left no trace"
    );
    tx.commit().unwrap();
    clean(&g);
}

#[test]
fn unique_constraint_creates_and_owns_a_backing_index() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    tx.create_node(&["U"], p(&[("k", 2.into())])).unwrap();
    tx.commit().unwrap();
    g.create_unique_constraint("U", "k").unwrap();
    let idx = g.list_indexes().unwrap();
    assert_eq!(idx.len(), 1);
    assert!(idx[0].backs_constraint);
    assert_eq!(raw_len(&g, PROP_IDX), 2);
    assert!(matches!(
        g.drop_index("U", "k"),
        Err(Error::IndexBacksConstraint { .. })
    ));
    g.drop_constraint(ConstraintKind::Unique, "U", "k").unwrap();
    assert!(
        g.list_indexes().unwrap().is_empty(),
        "owned index goes with the constraint"
    );
    assert_eq!(raw_len(&g, PROP_IDX), 0);
    clean(&g);
}

#[test]
fn unique_constraint_reuses_an_existing_user_index_and_leaves_it_on_drop() {
    let g = mem();
    g.create_index("U", "k").unwrap();
    g.create_unique_constraint("U", "k").unwrap();
    let idx = g.list_indexes().unwrap();
    assert_eq!(idx.len(), 1);
    assert!(!idx[0].backs_constraint);
    g.drop_constraint(ConstraintKind::Unique, "U", "k").unwrap();
    assert_eq!(g.list_indexes().unwrap().len(), 1);
}

#[test]
fn creating_a_unique_constraint_fails_on_existing_duplicates_and_leaves_no_trace() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["U"], p(&[("k", 5.into())])).unwrap();
    tx.create_node(&["U"], p(&[("k", 6.into())])).unwrap();
    tx.create_node(&["U"], p(&[("k", 5.0.into())])).unwrap();
    tx.commit().unwrap();
    let err = g.create_unique_constraint("U", "k").unwrap_err();
    assert!(unique_violation(&err), "{err:?}");
    assert!(g.list_constraints().unwrap().is_empty());
    assert!(
        g.list_indexes().unwrap().is_empty(),
        "backing index must be rolled back"
    );
    assert_eq!(raw_len(&g, PROP_IDX), 0);
    // not enforced: a duplicate can still be written
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["U"], p(&[("k", 6.into())])).unwrap();
    tx.commit().unwrap();
}

#[test]
fn unique_constraint_on_a_pre_existing_index_with_duplicates_leaves_the_index_intact() {
    let g = mem();
    g.create_index("U", "k").unwrap();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    tx.commit().unwrap();
    assert!(g.create_unique_constraint("U", "k").is_err());
    assert_eq!(
        g.list_indexes().unwrap().len(),
        1,
        "the user's own index survives the failed attempt"
    );
    assert_eq!(raw_len(&g, PROP_IDX), 2);
}

#[test]
fn exists_constraint_requires_a_non_null_value() {
    let g = mem();
    g.create_exists_constraint("P", "name").unwrap();
    let mut tx = g.begin_write().unwrap();
    let err = tx.create_node(&["P"], p(&[])).unwrap_err();
    assert!(exists_violation(&err), "{err:?}");
    let err = tx
        .create_node(&["P"], p(&[("name", Value::Null)]))
        .unwrap_err();
    assert!(exists_violation(&err));
    tx.create_node(&["Q"], p(&[])).unwrap();
    let ok = tx.create_node(&["P"], p(&[("name", "n".into())])).unwrap();
    let err = tx
        .set_node_props(ok, p(&[("name", Value::Null)]))
        .unwrap_err();
    assert!(exists_violation(&err));
    assert_eq!(
        tx.get_node(ok).unwrap().unwrap().props,
        p(&[("name", "n".into())]),
        "unchanged"
    );
    tx.set_node_props(ok, p(&[("name", "m".into())])).unwrap();
    let q = tx.create_node(&["Q"], p(&[])).unwrap();
    let err = tx.add_label(q, "P").unwrap_err();
    assert!(exists_violation(&err));
    tx.set_node_props(q, p(&[("name", "q".into())])).unwrap();
    tx.add_label(q, "P").unwrap();
    tx.remove_label(q, "P").unwrap();
    tx.set_node_props(q, p(&[("name", Value::Null)])).unwrap(); // no longer a P
    tx.commit().unwrap();
    clean(&g);
}

#[test]
fn creating_an_exists_constraint_fails_when_existing_data_violates_it() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["P"], p(&[("name", "a".into())])).unwrap();
    tx.create_node(&["P"], p(&[])).unwrap();
    tx.commit().unwrap();
    let err = g.create_exists_constraint("P", "name").unwrap_err();
    assert!(exists_violation(&err), "{err:?}");
    assert!(g.list_constraints().unwrap().is_empty());
}

#[test]
fn a_violation_mid_transaction_leaves_the_transaction_consistent_and_rollback_complete() {
    let g = mem();
    g.create_unique_constraint("U", "k").unwrap();
    g.create_exists_constraint("U", "must").unwrap();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(&["U"], p(&[("k", 1.into()), ("must", true.into())]))
        .unwrap();
    // fails after the first create succeeded
    let err = tx
        .create_node(&["U"], p(&[("k", 1.into()), ("must", true.into())]))
        .unwrap_err();
    assert!(unique_violation(&err));
    drop(tx);
    assert_eq!(
        raw_len(&g, NODES),
        0,
        "dropping after a violation rolls back everything"
    );
    assert_eq!(raw_len(&g, PROP_IDX), 0);
    let _ = a;

    // ...and when the caller carries on instead, only the failed op is absent
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(&["U"], p(&[("k", 1.into()), ("must", true.into())]))
        .unwrap();
    assert!(tx
        .create_node(&["U"], p(&[("k", 1.into()), ("must", true.into())]))
        .is_err());
    assert!(tx.create_node(&["U"], p(&[("k", 2.into())])).is_err());
    let c = tx
        .create_node(&["U"], p(&[("k", 3.into()), ("must", true.into())]))
        .unwrap();
    assert_eq!(
        (a, c),
        (NodeId(0), NodeId(1)),
        "failed creates consume no ids"
    );
    tx.commit().unwrap();
    let r = g.begin_read().unwrap();
    assert_eq!(r.node_count().unwrap(), 2);
    assert_eq!(r.count_nodes_with_label("U").unwrap(), 2);
    drop(r);
    assert_eq!(raw_len(&g, PROP_IDX), 2);
    clean(&g);
}

#[test]
fn a_failed_update_changes_nothing_in_the_node_or_the_index() {
    let g = mem();
    g.create_unique_constraint("U", "k").unwrap();
    let mut tx = g.begin_write().unwrap();
    let a = tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    let b = tx.create_node(&["U"], p(&[("k", 2.into())])).unwrap();
    tx.commit().unwrap();
    let mut tx = g.begin_write().unwrap();
    assert!(tx
        .set_node_props(b, p(&[("k", 1.into()), ("extra", 9.into())]))
        .is_err());
    tx.commit().unwrap();
    let r = g.begin_read().unwrap();
    assert_eq!(r.get_node(b).unwrap().unwrap().props, p(&[("k", 2.into())]));
    assert_eq!(
        r.nodes_by_prop("U", "k", &1.into())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .id,
        a
    );
    assert_eq!(
        r.nodes_by_prop("U", "k", &2.into())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .id,
        b
    );
    drop(r);
    clean(&g);
}

#[test]
fn constraint_ddl_listing_errors_and_drop() {
    let g = mem();
    g.create_exists_constraint("B", "y").unwrap();
    g.create_unique_constraint("A", "x").unwrap();
    g.create_exists_constraint("A", "x").unwrap();
    let defs = g.list_constraints().unwrap();
    assert_eq!(
        defs,
        vec![
            ConstraintDef {
                kind: ConstraintKind::Unique,
                label: "A".into(),
                key: "x".into()
            },
            ConstraintDef {
                kind: ConstraintKind::Exists,
                label: "A".into(),
                key: "x".into()
            },
            ConstraintDef {
                kind: ConstraintKind::Exists,
                label: "B".into(),
                key: "y".into()
            },
        ]
    );
    assert!(matches!(
        g.create_unique_constraint("A", "x"),
        Err(Error::ConstraintExists { .. })
    ));
    assert!(matches!(
        g.create_exists_constraint("B", "y"),
        Err(Error::ConstraintExists { .. })
    ));
    assert!(matches!(
        g.drop_constraint(ConstraintKind::Unique, "B", "y"),
        Err(Error::ConstraintNotFound { .. })
    ));
    g.drop_constraint(ConstraintKind::Exists, "B", "y").unwrap();
    assert_eq!(g.list_constraints().unwrap().len(), 2);
    assert!(matches!(
        g.create_unique_constraint("", "x"),
        Err(Error::InvalidName(_))
    ));
    // dropped constraints stop being enforced
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["B"], p(&[])).unwrap();
    tx.commit().unwrap();
}

#[test]
fn constraints_persist_across_reopen_and_stay_enforced() {
    let (_dir, path) = tmp_graph_path();
    {
        let g = Graph::open(&path).unwrap();
        g.create_unique_constraint("U", "k").unwrap();
        g.create_exists_constraint("U", "k").unwrap();
        let mut tx = g.begin_write().unwrap();
        tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
        tx.commit().unwrap();
    }
    let g = Graph::open(&path).unwrap();
    assert_eq!(g.list_constraints().unwrap().len(), 2);
    let mut tx = g.begin_write().unwrap();
    assert!(unique_violation(
        &tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap_err()
    ));
    assert!(exists_violation(
        &tx.create_node(&["U"], p(&[])).unwrap_err()
    ));
    tx.create_node(&["U"], p(&[("k", 2.into())])).unwrap();
    tx.commit().unwrap();
    clean(&g);
}

#[test]
fn constraint_created_in_a_rolled_back_tx_is_not_enforced() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    tx.create_unique_constraint("U", "k").unwrap();
    // enforced inside the creating tx
    tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    assert!(tx.create_node(&["U"], p(&[("k", 1.into())])).is_err());
    drop(tx);
    assert!(g.list_constraints().unwrap().is_empty());
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    tx.create_node(&["U"], p(&[("k", 1.into())])).unwrap();
    tx.commit().unwrap();
}
