use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use redb::{ReadableDatabase, ReadableTable, TableDefinition};

use super::fault::{export, run, script, FaultBackend, Rng};
use super::{raw_len, tmp_graph_path};
use crate::checksum::crc32c;
use crate::graph::{FORMAT_VERSION, SUMLESS_FORMAT_VERSION};
use crate::tables::{meta, META, NODES, NODE_SUMS, RELS, REL_SUMS};
use crate::{Error, Graph, Integrity, Mode, ModeCause, Options, Props, RecordId};

type Stored = BTreeMap<RecordId, Vec<u8>>;

fn options() -> Options {
    Options {
        cache_bytes: 1 << 20,
        hot_entries: 16,
        hot_bytes: 1 << 16,
        warm_bytes: 1 << 16,
        compress_cold: false,
        ..Options::new(crate::MemoryBudget::mib(80))
    }
}

fn stored(g: &Graph) -> Result<Stored, String> {
    let db = g.inner.database().map_err(|e| e.to_string())?;
    let read = db.begin_read().map_err(|e| e.to_string())?;
    let mut out = Stored::new();
    for (name, def) in [("nodes", NODES), ("rels", RELS)] {
        let t = read.open_table(def).map_err(|e| e.to_string())?;
        for entry in t.iter().map_err(|e| e.to_string())? {
            let (k, v) = entry.map_err(|e| e.to_string())?;
            out.insert(
                RecordId {
                    table: name,
                    id: k.value(),
                },
                v.value().to_vec(),
            );
        }
    }
    Ok(out)
}

fn tamper(g: &Graph, def: TableDefinition<u64, &'static [u8]>, id: u64) {
    let db = g.inner.database().unwrap();
    let w = db.begin_write().unwrap();
    {
        let mut t = w.open_table(def).unwrap();
        let mut bytes = t.get(id).unwrap().unwrap().value().to_vec();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        t.insert(id, bytes.as_slice()).unwrap();
    }
    w.commit().unwrap();
}

fn populate(g: &Graph, seed: u64, ops: usize) {
    let r = run(g, &script(seed, ops), |_, _| {});
    assert!(!r.failed);
}

struct Probe {
    opened: bool,
    changed: usize,
}

fn probe(path: &Path, original: &Stored) -> Probe {
    let Ok(g) = Graph::open_with(path, &options()) else {
        return Probe {
            opened: false,
            changed: 0,
        };
    };
    let Ok(report) = g.scrub() else {
        return Probe {
            opened: true,
            changed: 0,
        };
    };
    assert!(
        !report.errors.iter().any(|e| e.contains("panic")),
        "scrub hit a panic: {report:?}"
    );
    let Ok(now) = stored(&g) else {
        assert!(!report.is_clean(), "unreadable tables but clean report: {report:?}");
        return Probe {
            opened: true,
            changed: 0,
        };
    };
    let changed: Vec<RecordId> = original
        .iter()
        .filter(|(id, bytes)| now.get(*id).is_some_and(|n| n != *bytes))
        .map(|(id, _)| *id)
        .collect();
    if report.errors.is_empty() {
        for id in &changed {
            assert!(
                report.corrupt.contains(id),
                "record {id:?} changed on disk but scrub did not report it: {report:?}"
            );
        }
    } else {
        assert!(!report.is_clean(), "{report:?}");
    }
    Probe {
        opened: true,
        changed: changed.len(),
    }
}

#[test]
fn crc32c_matches_the_standard_check_values() {
    assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    assert_eq!(crc32c(&[]), 0);
    assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
    assert_ne!(crc32c(b"abc"), crc32c(b"abd"));
}

#[test]
fn every_write_keeps_one_checksum_per_record() {
    let b = FaultBackend::new();
    let g = b.open(&options()).unwrap();
    populate(&g, 21, 300);
    assert!(!b.bytes().is_empty());
    assert_eq!(raw_len(&g, NODE_SUMS), raw_len(&g, NODES));
    assert_eq!(raw_len(&g, REL_SUMS), raw_len(&g, RELS));
    assert!(raw_len(&g, NODES) > 0);
    for (id, bytes) in stored(&g).unwrap() {
        let db = g.inner.database().unwrap();
        let r = db.begin_read().unwrap();
        let sums = if id.table == "nodes" { NODE_SUMS } else { REL_SUMS };
        let t = r.open_table(sums).unwrap();
        assert_eq!(t.get(id.id).unwrap().unwrap().value(), crc32c(&bytes));
    }
}

#[test]
fn a_healthy_database_scrubs_clean_and_stays_full() {
    let b = FaultBackend::new();
    let g = b.open(&options()).unwrap();
    populate(&g, 22, 300);
    let total = stored(&g).unwrap().len();
    let report = g.scrub().unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert!(report.corrupt.is_empty());
    assert_eq!(report.records_checked, total);
    assert_eq!(report.unchecked, 0);
    assert_eq!(report.repaired, 0);
    assert!(matches!(
        report.integrity,
        Integrity::Clean | Integrity::Skipped
    ));
    assert_eq!(g.mode(), Mode::Full);
    assert!(g.mode_log().is_empty());
}

#[test]
fn scrub_reports_every_changed_record_or_the_open_fails_cleanly_over_20_seeds() {
    let mut opened = 0;
    let mut changed = 0;
    for seed in 1..=20u64 {
        let (_dir, path) = tmp_graph_path();
        let original = {
            let g = Graph::open_with(&path, &options()).unwrap();
            populate(&g, seed, 150);
            let s = stored(&g).unwrap();
            drop(g);
            s
        };
        let mut bytes = std::fs::read(&path).unwrap();
        let live: Vec<usize> = bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b != 0)
            .map(|(i, _)| i)
            .collect();
        assert!(!live.is_empty());
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for _ in 0..50 {
            let at = live[rng.below(live.len())];
            bytes[at] ^= (1 + rng.below(255)) as u8;
        }
        std::fs::write(&path, &bytes).unwrap();
        let outcome = catch_unwind(AssertUnwindSafe(|| probe(&path, &original)));
        let p = outcome.unwrap_or_else(|_| panic!("seed {seed}: open or scrub panicked"));
        opened += usize::from(p.opened);
        changed += p.changed;
    }
    assert!(opened > 0, "no seed produced an openable file");
    assert!(changed > 0, "no seed changed a stored record");
}

#[test]
fn a_checksum_mismatch_surfaces_as_a_corrupt_record_on_every_read_path() {
    let b = FaultBackend::new();
    let g = b.open(&options()).unwrap();
    populate(&g, 11, 150);
    let (nodes, rels) = export(&g).unwrap();
    assert!(nodes.len() >= 5 && !rels.is_empty());
    let bad_node = nodes[0].id;
    let bad_rel = rels[0].clone();
    tamper(&g, NODES, bad_node.0);
    tamper(&g, RELS, bad_rel.id.0);
    drop(g);
    let g = b.open(&options()).unwrap();
    let r = g.begin_read().unwrap();
    match r.get_node(bad_node) {
        Err(Error::CorruptRecord { table, id }) => {
            assert_eq!(table, "nodes");
            assert_eq!(id, bad_node.0);
        }
        other => panic!("unexpected {other:?}"),
    }
    match r.get_rel(bad_rel.id) {
        Err(Error::CorruptRecord { table, id }) => {
            assert_eq!(table, "rels");
            assert_eq!(id, bad_rel.id.0);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(r.all_nodes().unwrap().filter(|e| e.is_err()).count(), 1);
    assert_eq!(r.all_rels().unwrap().filter(|e| e.is_err()).count(), 1);
    let from_adj = r
        .out_rels(bad_rel.src, None)
        .unwrap()
        .filter(|e| matches!(e, Err(Error::CorruptRecord { .. })))
        .count();
    assert_eq!(from_adj, 1);
    let healthy = nodes[1].id;
    assert_eq!(r.get_node(healthy).unwrap().unwrap(), nodes[1]);
}

#[test]
fn scrub_finding_corruption_goes_read_only_and_repair_from_a_replica_restores_a_full_export() {
    let b = FaultBackend::new();
    let g = b.open(&options()).unwrap();
    populate(&g, 11, 150);
    let (nodes, rels) = export(&g).unwrap();
    assert!(nodes.len() >= 5 && !rels.is_empty());
    let bad_nodes = [nodes[0].id, nodes[nodes.len() / 2].id];
    let bad_rel = rels[0].id;
    for n in bad_nodes {
        tamper(&g, NODES, n.0);
    }
    tamper(&g, RELS, bad_rel.0);
    drop(g);

    let replica = Graph::open_in_memory_with(&options()).unwrap();
    populate(&replica, 11, 150);
    assert_eq!(export(&replica).unwrap(), (nodes.clone(), rels.clone()));

    let g = b.open(&options()).unwrap();
    assert!(export(&g).is_err());
    let report = g.scrub().unwrap();
    let mut want = vec![
        RecordId {
            table: "nodes",
            id: bad_nodes[0].0,
        },
        RecordId {
            table: "nodes",
            id: bad_nodes[1].0,
        },
        RecordId {
            table: "rels",
            id: bad_rel.0,
        },
    ];
    want.sort();
    assert_eq!(report.corrupt, want);
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(report.records_checked, nodes.len() + rels.len());
    assert_eq!(report.unchecked, 0);
    assert!(matches!(
        report.integrity,
        Integrity::Clean | Integrity::Skipped
    ));
    assert_eq!(g.mode(), Mode::ReadOnly);
    assert!(matches!(g.begin_write(), Err(Error::ReadOnly)));
    let log = g.mode_log();
    assert_eq!(log.len(), 1);
    assert_eq!(
        (log[0].from, log[0].to, log[0].cause),
        (Mode::Full, Mode::ReadOnly, ModeCause::ScrubCorrupt)
    );

    let after = g.repair_from(&replica).unwrap();
    assert!(after.corrupt.is_empty(), "{after:?}");
    assert!(after.errors.is_empty(), "{after:?}");
    assert_eq!(after.repaired, 3);
    assert!(after.is_clean(), "{after:?}");
    assert_eq!(export(&g).unwrap(), export(&replica).unwrap());
    assert_eq!(g.epoch(), replica.epoch());
    assert!(g
        .begin_read()
        .unwrap()
        .check_integrity()
        .unwrap()
        .is_empty());
    assert_eq!(g.mode(), Mode::Full);
    let causes: Vec<_> = g.mode_log().iter().map(|e| (e.from, e.to, e.cause)).collect();
    assert_eq!(
        causes,
        vec![
            (Mode::Full, Mode::ReadOnly, ModeCause::ScrubCorrupt),
            (Mode::ReadOnly, Mode::Full, ModeCause::Repaired),
        ]
    );
    let epoch = g.epoch();
    let mut tx = g.begin_write().unwrap();
    tx.create_node(&["Doc"], Props::new()).unwrap();
    assert_eq!(tx.commit().unwrap(), epoch + 1);
}

#[test]
fn repair_from_a_replica_with_the_same_damage_repairs_nothing_and_stays_read_only() {
    let b = FaultBackend::new();
    let g = b.open(&options()).unwrap();
    populate(&g, 13, 120);
    let (nodes, _) = export(&g).unwrap();
    let victim = nodes[0].id;
    tamper(&g, NODES, victim.0);
    drop(g);

    let damaged = FaultBackend::new();
    let replica = damaged.open(&options()).unwrap();
    populate(&replica, 13, 120);
    tamper(&replica, NODES, victim.0);
    drop(replica);
    let replica = damaged.open(&options()).unwrap();

    let g = b.open(&options()).unwrap();
    let after = g.repair_from(&replica).unwrap();
    assert_eq!(after.repaired, 0);
    assert_eq!(after.corrupt.len(), 1);
    assert!(!after.is_clean());
    assert_eq!(g.mode(), Mode::ReadOnly);
    assert!(matches!(g.begin_write(), Err(Error::ReadOnly)));
}

#[test]
fn flipped_payload_bytes_in_a_file_never_leave_a_full_graph_that_differs_from_the_replica() {
    let replica = Graph::open_in_memory_with(&options()).unwrap();
    populate(&replica, 5, 150);
    let (_dir, path) = tmp_graph_path();
    {
        let g = Graph::open_with(&path, &options()).unwrap();
        populate(&g, 5, 150);
    }
    let mut bytes = std::fs::read(&path).unwrap();
    let marker = b"record ";
    let mut hits = 0;
    let mut at = 0;
    while at + 16 < bytes.len() {
        if bytes[at..at + marker.len()] == marker[..] {
            bytes[at + 9] ^= 1;
            hits += 1;
            at += 16;
        } else {
            at += 1;
        }
    }
    assert!(hits > 0);
    std::fs::write(&path, &bytes).unwrap();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let Ok(g) = Graph::open_with(&path, &options()) else {
            return;
        };
        let Ok(before) = g.scrub() else {
            return;
        };
        assert!(!before.is_clean(), "{before:?}");
        if g.repair_from(&replica).is_err() {
            return;
        }
        if g.mode() == Mode::Full {
            assert_eq!(export(&g).unwrap(), export(&replica).unwrap());
        }
    }));
    assert!(outcome.is_ok(), "open, scrub or repair panicked");
}

#[test]
fn a_format_2_database_without_checksums_opens_reads_and_upgrades() {
    let b = FaultBackend::new();
    let g = b.open(&options()).unwrap();
    populate(&g, 31, 200);
    let want = export(&g).unwrap();
    let total = stored(&g).unwrap().len();
    assert!(total > 0);
    {
        let db = g.inner.database().unwrap();
        let w = db.begin_write().unwrap();
        w.open_table(NODE_SUMS).unwrap().retain(|_, _| false).unwrap();
        w.open_table(REL_SUMS).unwrap().retain(|_, _| false).unwrap();
        {
            let mut m = w.open_table(META).unwrap();
            m.insert(meta::FORMAT, SUMLESS_FORMAT_VERSION).unwrap();
        }
        w.commit().unwrap();
    }
    drop(g);

    let g = b.open(&options()).unwrap();
    assert_eq!(export(&g).unwrap(), want);
    {
        let db = g.inner.database().unwrap();
        let r = db.begin_read().unwrap();
        let m = r.open_table(META).unwrap();
        assert_eq!(m.get(meta::FORMAT).unwrap().unwrap().value(), FORMAT_VERSION);
    }
    assert_eq!(raw_len(&g, NODE_SUMS), 0);
    assert_eq!(raw_len(&g, REL_SUMS), 0);
    let report = g.scrub().unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.records_checked, total);
    assert_eq!(report.unchecked, total);
    assert_eq!(g.mode(), Mode::Full);

    let id = want.0[0].id;
    let mut tx = g.begin_write().unwrap();
    let mut p = Props::new();
    p.insert("extra".into(), crate::Value::Int(1));
    tx.set_node_props(id, p).unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_len(&g, NODE_SUMS), 1);
    let report = g.scrub().unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.unchecked, total - 1);
}
