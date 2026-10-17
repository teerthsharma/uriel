use super::fault::{export, run, script, FaultBackend};
use crate::{Error, Graph, Mode, ModeCause, Node, Options, Props};

fn options() -> Options {
    Options {
        cache_bytes: 0,
        hot_entries: 4,
        hot_bytes: 1 << 20,
        warm_bytes: 1 << 20,
        admission_gate: false,
        ..Options::new(crate::MemoryBudget::mib(80))
    }
}

fn seeded(b: &FaultBackend) -> Graph {
    let g = b.open(&options()).unwrap();
    assert!(!run(&g, &script(3, 200), |_, _| {}).failed);
    g
}

fn add_node(g: &Graph) -> Result<u64, Error> {
    let mut tx = g.begin_write()?;
    tx.create_node(&["Doc"], Props::new())?;
    tx.commit()
}

fn transitions(g: &Graph) -> Vec<(Mode, Mode, ModeCause)> {
    g.mode_log()
        .iter()
        .map(|e| (e.from, e.to, e.cause))
        .collect()
}

fn warm_up(g: &Graph, nodes: &[Node]) {
    let r = g.begin_read().unwrap();
    for n in nodes {
        assert_eq!(r.get_node(n.id).unwrap().as_ref(), Some(n));
    }
}

fn assert_serves(g: &Graph, nodes: &[Node]) {
    let r = g.begin_read().unwrap();
    for n in nodes {
        assert_eq!(r.get_node(n.id).unwrap().as_ref(), Some(n));
    }
}

fn walk(b: &FaultBackend) -> Vec<(u64, Mode, Mode, ModeCause)> {
    let g = seeded(b);
    let (nodes, _) = export(&g).unwrap();
    warm_up(&g, &nodes[..8]);
    b.set_fail_writes(true);
    assert!(add_node(&g).is_err());
    b.set_fail_reads(true);
    let _ = g.begin_read().and_then(|r| r.get_node(nodes[nodes.len() - 1].id));
    b.revive();
    assert_eq!(g.try_recover().unwrap(), Mode::Full);
    g.mode_log()
        .iter()
        .map(|e| (e.seq, e.from, e.to, e.cause))
        .collect()
}

#[test]
fn a_write_io_error_moves_full_to_read_only_and_no_later_write_is_acknowledged() {
    let b = FaultBackend::new();
    let g = seeded(&b);
    let before = export(&g).unwrap();
    let epoch = g.epoch();
    assert_eq!(g.mode(), Mode::Full);
    assert!(g.mode_log().is_empty());

    b.set_fail_writes(true);
    assert!(add_node(&g).is_err());
    assert_eq!(g.mode(), Mode::ReadOnly);
    let log = g.mode_log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].seq, 0);
    assert_eq!(log[0].epoch, epoch);
    assert_eq!(
        (log[0].from, log[0].to, log[0].cause),
        (Mode::Full, Mode::ReadOnly, ModeCause::WriteFailed)
    );

    for _ in 0..25 {
        assert!(matches!(add_node(&g), Err(Error::ReadOnly)));
    }
    assert!(matches!(g.begin_write(), Err(Error::ReadOnly)));
    assert!(matches!(g.create_index("Doc", "n"), Err(Error::ReadOnly)));
    assert_eq!(g.epoch(), epoch);
    assert_eq!(export(&g).unwrap(), before);

    b.set_fail_writes(false);
    assert!(matches!(add_node(&g), Err(Error::ReadOnly)));
    assert_eq!(g.mode(), Mode::ReadOnly);
    assert_eq!(g.mode_log().len(), 1);

    drop(g);
    let reopened = b.open(&options()).unwrap();
    assert_eq!(reopened.epoch(), epoch);
    assert_eq!(export(&reopened).unwrap(), before);
}

#[test]
fn a_cleared_disk_full_fault_climbs_back_to_full_through_try_recover() {
    let b = FaultBackend::new();
    let g = seeded(&b);
    let before = export(&g).unwrap();
    let epoch = g.epoch();

    b.set_fail_writes(true);
    assert!(add_node(&g).is_err());
    assert_eq!(g.mode(), Mode::ReadOnly);
    b.set_fail_writes(false);

    assert_eq!(g.try_recover().unwrap(), Mode::Full);
    assert_eq!(g.mode(), Mode::Full);
    assert_eq!(g.epoch(), epoch);
    assert_eq!(export(&g).unwrap(), before);
    assert_eq!(
        transitions(&g),
        vec![
            (Mode::Full, Mode::ReadOnly, ModeCause::WriteFailed),
            (Mode::ReadOnly, Mode::Full, ModeCause::Recovered),
        ]
    );
    let seqs: Vec<u64> = g.mode_log().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![0, 1]);

    assert_eq!(add_node(&g).unwrap(), epoch + 1);
    let after = export(&g).unwrap();
    assert_eq!(after.0.len(), before.0.len() + 1);
    assert_eq!(after.1, before.1);
    assert_eq!(&after.0[..before.0.len()], &before.0[..]);
}

#[test]
fn try_recover_does_not_climb_while_the_fault_persists() {
    let b = FaultBackend::new();
    let g = seeded(&b);
    let before = export(&g).unwrap();
    let epoch = g.epoch();

    b.set_fail_writes(true);
    assert!(add_node(&g).is_err());
    for _ in 0..4 {
        assert_eq!(g.try_recover().unwrap(), Mode::ReadOnly);
        assert_eq!(g.mode(), Mode::ReadOnly);
        assert_eq!(export(&g).unwrap(), before);
        assert!(matches!(add_node(&g), Err(Error::ReadOnly)));
        assert_eq!(g.epoch(), epoch);
    }
    assert_eq!(g.mode_log().len(), 1);

    b.set_fail_writes(false);
    assert_eq!(g.try_recover().unwrap(), Mode::Full);
    assert_eq!(add_node(&g).unwrap(), epoch + 1);
}

#[test]
fn failing_cold_reads_move_read_only_to_cache_only_while_hot_and_warm_keep_serving() {
    let b = FaultBackend::new();
    let g = seeded(&b);
    let (nodes, rels) = export(&g).unwrap();
    let before = (nodes.clone(), rels.clone());
    assert!(nodes.len() >= 20);
    let epoch = g.epoch();
    let held = &nodes[..8];
    let cold = &nodes[nodes.len() - 5..];

    warm_up(&g, held);
    let s = g.tier_stats().unwrap();
    assert_eq!(s.hot.entries + s.warm.entries, 8);
    assert!(s.warm.entries > 0);

    b.set_fail_writes(true);
    assert!(add_node(&g).is_err());
    assert_eq!(g.mode(), Mode::ReadOnly);
    assert_eq!(export(&g).unwrap(), before);
    assert_serves(&g, held);

    b.set_fail_reads(true);
    let tripped = g.begin_read().and_then(|r| r.get_node(cold[0].id));
    assert!(tripped.is_err());
    assert_eq!(g.mode(), Mode::CacheOnly);

    let r = g.begin_read().unwrap();
    for n in held {
        assert_eq!(r.get_node(n.id).unwrap().as_ref(), Some(n));
    }
    for n in cold {
        assert!(matches!(r.get_node(n.id), Err(Error::Unavailable)));
    }
    assert!(matches!(r.get_rel(rels[0].id), Err(Error::Unavailable)));
    assert!(matches!(r.all_nodes(), Err(Error::Unavailable)));
    assert!(matches!(r.all_rels(), Err(Error::Unavailable)));
    assert!(matches!(r.node_count(), Err(Error::Unavailable)));
    drop(r);
    let s = g.tier_stats().unwrap();
    assert_eq!(s.hot.entries + s.warm.entries, 8);

    for _ in 0..5 {
        assert!(matches!(add_node(&g), Err(Error::ReadOnly)));
    }
    assert!(matches!(g.begin_write(), Err(Error::ReadOnly)));
    assert_eq!(g.epoch(), epoch);

    assert_eq!(
        transitions(&g),
        vec![
            (Mode::Full, Mode::ReadOnly, ModeCause::WriteFailed),
            (Mode::ReadOnly, Mode::CacheOnly, ModeCause::ReadFailed),
        ]
    );
    assert!(g.mode_log().iter().all(|e| e.epoch == epoch));

    assert_eq!(g.try_recover().unwrap(), Mode::CacheOnly);
    assert_eq!(g.mode(), Mode::CacheOnly);

    b.revive();
    assert_eq!(g.try_recover().unwrap(), Mode::Full);
    assert_eq!(g.epoch(), epoch);
    assert_eq!(export(&g).unwrap(), before);
    assert_serves(&g, held);
    assert_serves(&g, cold);
    assert_eq!(
        transitions(&g),
        vec![
            (Mode::Full, Mode::ReadOnly, ModeCause::WriteFailed),
            (Mode::ReadOnly, Mode::CacheOnly, ModeCause::ReadFailed),
            (Mode::CacheOnly, Mode::ReadOnly, ModeCause::Recovered),
            (Mode::ReadOnly, Mode::Full, ModeCause::Recovered),
        ]
    );
    let seqs: Vec<u64> = g.mode_log().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3]);
    assert_eq!(add_node(&g).unwrap(), epoch + 1);
}

#[test]
fn a_power_cut_during_a_commit_degrades_and_recovers_without_losing_or_inventing_data() {
    let b = FaultBackend::new();
    let g = seeded(&b);
    let before = export(&g).unwrap();
    let epoch = g.epoch();

    b.cut_after(b.ops(), false);
    assert!(add_node(&g).is_err());
    assert_eq!(g.mode(), Mode::ReadOnly);
    assert_eq!(g.epoch(), epoch);
    assert!(export(&g).is_err());
    assert_eq!(g.mode(), Mode::CacheOnly);
    assert!(matches!(add_node(&g), Err(Error::ReadOnly)));

    b.revive();
    assert_eq!(g.try_recover().unwrap(), Mode::Full);
    assert_eq!(g.epoch(), epoch);
    assert_eq!(export(&g).unwrap(), before);
    assert_eq!(add_node(&g).unwrap(), epoch + 1);
    let causes: Vec<ModeCause> = g.mode_log().iter().map(|e| e.cause).collect();
    assert_eq!(
        causes,
        vec![
            ModeCause::WriteFailed,
            ModeCause::ReadFailed,
            ModeCause::Recovered,
            ModeCause::Recovered,
        ]
    );
}

#[test]
fn mode_transitions_are_deterministic_across_identical_runs() {
    let first = walk(&FaultBackend::new());
    let second = walk(&FaultBackend::new());
    assert_eq!(first, second);
    assert_eq!(first.len(), 4);
    assert_eq!(
        first.iter().map(|e| e.0).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
}
