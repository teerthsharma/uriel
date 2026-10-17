//! Property indexes: backfill, maintenance, lookups, ranges.

use std::ops::Bound;

use super::{mem, p, raw_len, tmp_graph_path};
use crate::tables::PROP_IDX;
use crate::{Error, Graph, NodeId, Value};

fn lookup(g: &Graph, label: &str, key: &str, v: &Value) -> Vec<NodeId> {
    let r = g.begin_read().unwrap();
    let mut ids: Vec<NodeId> = r
        .nodes_by_prop(label, key, v)
        .unwrap()
        .map(|n| n.unwrap().id)
        .collect();
    ids.sort();
    ids
}

fn range(
    g: &Graph,
    label: &str,
    key: &str,
    lo: Bound<&Value>,
    hi: Bound<&Value>,
) -> Vec<(Value, NodeId)> {
    let r = g.begin_read().unwrap();
    r.nodes_by_prop_range(label, key, lo, hi)
        .unwrap()
        .map(|n| {
            let n = n.unwrap();
            (n.props[key].clone(), n.id)
        })
        .collect()
}

fn clean(g: &Graph) {
    let problems = g.begin_read().unwrap().check_integrity().unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
}

fn seed(g: &Graph) -> Vec<NodeId> {
    let mut tx = g.begin_write().unwrap();
    let ids = vec![
        tx.create_node(&["Person"], p(&[("age", 30.into())]))
            .unwrap(),
        tx.create_node(&["Person"], p(&[("age", 25.into())]))
            .unwrap(),
        tx.create_node(&["Person"], p(&[("age", 30.into())]))
            .unwrap(),
        tx.create_node(&["Person"], p(&[("name", "no-age".into())]))
            .unwrap(),
        tx.create_node(&["Person"], p(&[])).unwrap(),
        tx.create_node(&["Robot"], p(&[("age", 30.into())]))
            .unwrap(),
    ];
    tx.commit().unwrap();
    ids
}

#[test]
fn create_index_backfills_existing_nodes_of_that_label_only() {
    let g = mem();
    let ids = seed(&g);
    assert_eq!(raw_len(&g, PROP_IDX), 0);
    g.create_index("Person", "age").unwrap();
    assert_eq!(
        raw_len(&g, PROP_IDX),
        3,
        "three Person nodes carry age; Robot does not count"
    );
    let r = g.begin_read().unwrap();
    assert!(r.has_index("Person", "age").unwrap());
    assert!(!r.has_index("Robot", "age").unwrap());
    drop(r);
    assert_eq!(
        lookup(&g, "Person", "age", &30.into()),
        vec![ids[0], ids[2]]
    );
    assert_eq!(lookup(&g, "Person", "age", &25.into()), vec![ids[1]]);
    assert!(lookup(&g, "Person", "age", &99.into()).is_empty());
    assert!(lookup(&g, "Person", "age", &Value::Null).is_empty());
    clean(&g);
}

#[test]
fn indexed_and_unindexed_lookups_return_identical_results() {
    let g = mem();
    seed(&g);
    let probes: Vec<Value> = vec![30.into(), 25.into(), 99.into(), 30.0.into(), "x".into()];
    let scan: Vec<_> = probes
        .iter()
        .map(|v| lookup(&g, "Person", "age", v))
        .collect();
    let scan_robot = lookup(&g, "Robot", "age", &30.into());
    g.create_index("Person", "age").unwrap();
    let indexed: Vec<_> = probes
        .iter()
        .map(|v| lookup(&g, "Person", "age", v))
        .collect();
    assert_eq!(scan, indexed);
    assert_eq!(
        scan_robot.len(),
        1,
        "unindexed (Robot, age) still answers via scan"
    );
    assert_eq!(lookup(&g, "Robot", "age", &30.into()), scan_robot);
}

#[test]
fn index_follows_creates_updates_removals_label_changes_and_deletes() {
    let g = mem();
    g.create_index("Person", "age").unwrap();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(&["Person"], p(&[("age", 1.into())]))
        .unwrap();
    let b = tx.create_node(&["Other"], p(&[("age", 1.into())])).unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 1);

    // update value: old entry replaced, not accumulated
    let mut tx = g.begin_write().unwrap();
    tx.set_node_props(a, p(&[("age", 2.into())])).unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 1);
    assert!(lookup(&g, "Person", "age", &1.into()).is_empty());
    assert_eq!(lookup(&g, "Person", "age", &2.into()), vec![a]);

    // gaining the label adds an entry
    let mut tx = g.begin_write().unwrap();
    tx.add_label(b, "Person").unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 2);
    assert_eq!(lookup(&g, "Person", "age", &1.into()), vec![b]);

    // unrelated property change leaves the index alone
    let mut tx = g.begin_write().unwrap();
    tx.set_node_props(b, p(&[("other", 5.into())])).unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 2);

    // losing the label removes it
    let mut tx = g.begin_write().unwrap();
    tx.remove_label(b, "Person").unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 1);

    // removing the property removes it
    let mut tx = g.begin_write().unwrap();
    tx.set_node_props(a, p(&[("age", Value::Null)])).unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 0);

    // delete
    let mut tx = g.begin_write().unwrap();
    tx.set_node_props(a, p(&[("age", 7.into())])).unwrap();
    assert_eq!(tx.node_ids_by_label("Person").unwrap(), vec![a]);
    tx.delete_node(a).unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 0);
    clean(&g);
}

#[test]
fn equal_numbers_of_different_kinds_share_an_index_entry_key() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    let i = tx.create_node(&["N"], p(&[("x", 1.into())])).unwrap();
    let f = tx.create_node(&["N"], p(&[("x", 1.0.into())])).unwrap();
    let h = tx.create_node(&["N"], p(&[("x", 1.5.into())])).unwrap();
    tx.commit().unwrap();
    let scan = lookup(&g, "N", "x", &1.into());
    assert_eq!(scan, vec![i, f]);
    g.create_index("N", "x").unwrap();
    assert_eq!(lookup(&g, "N", "x", &1.into()), vec![i, f]);
    assert_eq!(lookup(&g, "N", "x", &1.0.into()), vec![i, f]);
    assert_eq!(lookup(&g, "N", "x", &1.5.into()), vec![h]);
}

#[test]
fn range_lookup_is_ordered_bounded_and_restricted_to_the_bounds_kind() {
    let g = mem();
    let mut tx = g.begin_write().unwrap();
    // shuffled ages 0..100 plus values of other kinds that must never leak into numeric ranges
    for k in 0..100_i64 {
        let age = (k * 37) % 100;
        tx.create_node(&["P"], p(&[("v", age.into())])).unwrap();
    }
    tx.create_node(&["P"], p(&[("v", "text".into())])).unwrap();
    tx.create_node(&["P"], p(&[("v", true.into())])).unwrap();
    tx.create_node(&["P"], p(&[("v", 15.5.into())])).unwrap();
    tx.commit().unwrap();

    for indexed in [false, true] {
        if indexed {
            g.create_index("P", "v").unwrap();
        }
        let ten = Value::from(10);
        let twenty = Value::from(20);
        let r = range(
            &g,
            "P",
            "v",
            Bound::Included(&ten),
            Bound::Excluded(&twenty),
        );
        let vals: Vec<Value> = r.iter().map(|(v, _)| v.clone()).collect();
        let mut expect: Vec<Value> = (10..20).map(Value::from).collect();
        expect.push(15.5.into());
        expect.sort();
        assert_eq!(vals, expect, "indexed={indexed}");

        // exclusive lower, inclusive upper
        let r = range(
            &g,
            "P",
            "v",
            Bound::Excluded(&ten),
            Bound::Included(&twenty),
        );
        assert_eq!(r.len(), 11, "11..=20 plus 15.5 minus nothing = 10 + 1");
        assert_eq!(r.first().unwrap().0, Value::Int(11));
        assert_eq!(r.last().unwrap().0, Value::Int(20));

        // half-open: stays within numbers, so no "text" / true
        let five = Value::from(5);
        let low = range(&g, "P", "v", Bound::Unbounded, Bound::Included(&five));
        assert_eq!(low.len(), 6, "0..=5");
        assert!(
            low.iter().all(|(v, _)| matches!(v, Value::Int(_))),
            "indexed={indexed}"
        );
        let ninety = Value::from(90);
        let high = range(&g, "P", "v", Bound::Excluded(&ninety), Bound::Unbounded);
        assert_eq!(high.len(), 9, "91..=99 only; strings and bools excluded");

        // strings
        let a = Value::from("a");
        let z = Value::from("z");
        let s = range(&g, "P", "v", Bound::Included(&a), Bound::Included(&z));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, Value::from("text"));

        // ties come back in node id order
        let c15 = Value::from(15);
        let tie = range(&g, "P", "v", Bound::Included(&c15), Bound::Included(&c15));
        assert_eq!(tie.len(), 1);

        // empty / inverted / mixed-kind ranges are empty, not panics
        assert!(range(
            &g,
            "P",
            "v",
            Bound::Included(&twenty),
            Bound::Included(&ten)
        )
        .is_empty());
        assert!(range(&g, "P", "v", Bound::Excluded(&ten), Bound::Excluded(&ten)).is_empty());
        assert!(range(&g, "P", "v", Bound::Included(&ten), Bound::Included(&z)).is_empty());
        let all = range(&g, "P", "v", Bound::Unbounded, Bound::Unbounded);
        assert_eq!(all.len(), 103);
        let vals: Vec<&Value> = all.iter().map(|(v, _)| v).collect();
        assert!(
            vals.windows(2).all(|w| w[0] <= w[1]),
            "fully open range is globally ordered"
        );
        let missing = range(&g, "Nope", "v", Bound::Unbounded, Bound::Unbounded);
        assert!(missing.is_empty());
    }
    clean(&g);
}

#[test]
fn range_results_match_between_index_and_scan_on_random_data() {
    let g = mem();
    let mut state = 0x1234_5678_9abc_def0_u64;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let mut tx = g.begin_write().unwrap();
    for _ in 0..400 {
        let v: Value = match next() % 4 {
            0 => ((next() % 50) as i64 - 25).into(),
            1 => (((next() % 100) as f64) / 4.0 - 12.0).into(),
            2 => format!("s{}", next() % 30).into(),
            _ => i64::MAX.wrapping_sub((next() % 3) as i64).into(),
        };
        tx.create_node(&["R"], p(&[("k", v)])).unwrap();
    }
    tx.commit().unwrap();

    let probes: Vec<Value> = vec![
        (-10).into(),
        0.into(),
        3.5.into(),
        20.into(),
        "s10".into(),
        "s2".into(),
        "s9".into(),
        i64::MAX.into(),
        (i64::MAX - 1).into(),
    ];
    let mut cases = Vec::new();
    for lo in &probes {
        for hi in &probes {
            for (bl, bh) in [(0, 0), (1, 1), (0, 1), (1, 0), (2, 0), (0, 2), (2, 2)] {
                cases.push((lo, hi, bl, bh));
            }
        }
    }
    fn bound(v: &Value, kind: i32) -> Bound<&Value> {
        match kind {
            0 => Bound::Included(v),
            1 => Bound::Excluded(v),
            _ => Bound::Unbounded,
        }
    }
    let run = |g: &Graph| -> Vec<Vec<(Value, NodeId)>> {
        cases
            .iter()
            .map(|(lo, hi, bl, bh)| range(g, "R", "k", bound(lo, *bl), bound(hi, *bh)))
            .collect()
    };
    let by_scan = run(&g);
    g.create_index("R", "k").unwrap();
    let by_index = run(&g);
    assert_eq!(by_scan.len(), by_index.len());
    for (i, (a, b)) in by_scan.iter().zip(&by_index).enumerate() {
        assert_eq!(a, b, "case {i}");
    }
    assert!(by_index.iter().any(|r| !r.is_empty()));
}

#[test]
fn drop_index_removes_all_entries_and_lookups_fall_back_to_scan() {
    let g = mem();
    let ids = seed(&g);
    g.create_index("Person", "age").unwrap();
    g.create_index("Person", "name").unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 4);
    g.drop_index("Person", "age").unwrap();
    assert_eq!(
        raw_len(&g, PROP_IDX),
        1,
        "only the name index entry remains"
    );
    assert!(!g.begin_read().unwrap().has_index("Person", "age").unwrap());
    assert_eq!(
        lookup(&g, "Person", "age", &30.into()),
        vec![ids[0], ids[2]]
    );
    let names: Vec<String> = g
        .list_indexes()
        .unwrap()
        .into_iter()
        .map(|d| d.key)
        .collect();
    assert_eq!(names, vec!["name"]);
    clean(&g);
}

#[test]
fn index_ddl_errors() {
    let g = mem();
    g.create_index("A", "k").unwrap();
    assert!(matches!(
        g.create_index("A", "k"),
        Err(Error::IndexExists { .. })
    ));
    assert!(matches!(
        g.drop_index("A", "nope"),
        Err(Error::IndexNotFound { .. })
    ));
    assert!(matches!(
        g.create_index("", "k"),
        Err(Error::InvalidName(_))
    ));
    assert!(matches!(
        g.create_index("A", "bad\0"),
        Err(Error::InvalidName(_))
    ));
}

#[test]
fn index_created_in_a_rolled_back_tx_leaves_nothing() {
    let g = mem();
    seed(&g);
    let mut tx = g.begin_write().unwrap();
    tx.create_index("Person", "age").unwrap();
    drop(tx);
    assert!(g.list_indexes().unwrap().is_empty());
    assert_eq!(raw_len(&g, PROP_IDX), 0);
}

#[test]
fn list_indexes_is_sorted_and_survives_reopen() {
    let (_dir, path) = tmp_graph_path();
    {
        let g = Graph::open(&path, crate::MemoryBudget::mib(80)).unwrap();
        let mut tx = g.begin_write().unwrap();
        tx.create_node(&["B"], p(&[("z", 1.into())])).unwrap();
        tx.commit().unwrap();
        g.create_index("B", "z").unwrap();
        g.create_index("A", "y").unwrap();
        g.create_index("A", "x").unwrap();
    }
    let g = Graph::open(&path, crate::MemoryBudget::mib(80)).unwrap();
    let defs: Vec<(String, String)> = g
        .list_indexes()
        .unwrap()
        .into_iter()
        .map(|d| (d.label, d.key))
        .collect();
    assert_eq!(
        defs,
        vec![
            ("A".into(), "x".into()),
            ("A".into(), "y".into()),
            ("B".into(), "z".into())
        ]
    );
    assert_eq!(raw_len(&g, PROP_IDX), 1);
    // and it is still maintained after reopen
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["B"], p(&[("z", 2.into())])).unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, PROP_IDX), 2);
    clean(&g);
}

#[test]
fn string_values_with_nul_bytes_and_shared_prefixes_stay_distinct() {
    let g = mem();
    g.create_index("S", "k").unwrap();
    let vals = ["", "a", "a\0", "a\0b", "ab", "b", "\0"];
    let mut tx = g.begin_write().unwrap();
    let ids: Vec<NodeId> = vals
        .iter()
        .map(|v| tx.create_node(&["S"], p(&[("k", (*v).into())])).unwrap())
        .collect();
    tx.commit().unwrap();
    for (v, id) in vals.iter().zip(&ids) {
        assert_eq!(lookup(&g, "S", "k", &(*v).into()), vec![*id], "value {v:?}");
    }
    let lo = Value::from("a");
    let hi = Value::from("ab");
    let r = range(&g, "S", "k", Bound::Included(&lo), Bound::Included(&hi));
    let got: Vec<String> = r
        .into_iter()
        .map(|(v, _)| match v {
            Value::String(s) => s,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(got, vec!["a", "a\0", "a\0b", "ab"]);
}

#[test]
fn collection_and_vector_values_can_be_indexed_and_found() {
    let g = mem();
    g.create_index("C", "k").unwrap();
    let mut tx = g.begin_write().unwrap();
    let a = tx
        .create_node(&["C"], p(&[("k", Value::List(vec![1.into(), "x".into()]))]))
        .unwrap();
    let b = tx
        .create_node(&["C"], p(&[("k", Value::Vector(vec![0.5, 1.5]))]))
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        lookup(&g, "C", "k", &Value::List(vec![1.0.into(), "x".into()])),
        vec![a]
    );
    assert_eq!(
        lookup(&g, "C", "k", &Value::Vector(vec![0.5, 1.5])),
        vec![b]
    );
    assert!(lookup(&g, "C", "k", &Value::Vector(vec![0.5])).is_empty());
}

#[test]
fn many_nodes_index_stays_consistent_through_churn() {
    let g = mem();
    g.create_index("Churn", "n").unwrap();
    let mut tx = g.begin_write().unwrap();
    let ids: Vec<NodeId> = (0..500)
        .map(|i| {
            tx.create_node(&["Churn"], p(&[("n", (i % 50).into())]))
                .unwrap()
        })
        .collect();
    tx.commit().unwrap();
    let mut tx = g.begin_write().unwrap();
    for (i, id) in ids.iter().enumerate() {
        match i % 3 {
            0 => tx
                .set_node_props(*id, p(&[("n", ((i + 1) as i64 % 50).into())]))
                .unwrap(),
            1 => tx.detach_delete_node(*id).unwrap(),
            _ => {}
        }
    }
    tx.commit().unwrap();
    // 500 - 167 deleted = 333 entries
    assert_eq!(raw_len(&g, PROP_IDX), 333);
    clean(&g);
}
