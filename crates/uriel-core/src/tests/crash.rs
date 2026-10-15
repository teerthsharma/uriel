use super::fault::{export, run, script, Export, FaultBackend};
use crate::{Graph, Integrity, Options};

fn options() -> Options {
    Options {
        cache_bytes: 1 << 20,
        hot_entries: 16,
        hot_bytes: 1 << 16,
        warm_bytes: 1 << 16,
        ..Options::default()
    }
}

struct Reference {
    states: Vec<(u64, Export)>,
    total_ops: u64,
}

fn reference(seed: u64, ops: usize) -> Reference {
    let backend = FaultBackend::new();
    let g = backend.open(&options()).unwrap();
    let mut states = vec![(g.epoch(), export(&g).unwrap())];
    let script = script(seed, ops);
    let r = run(&g, &script, |g, epoch| {
        states.push((epoch, export(g).unwrap()));
    });
    assert!(!r.failed);
    assert_eq!(r.acked, script.len());
    drop(g);
    Reference {
        states,
        total_ops: backend.ops(),
    }
}

struct Outcome {
    opened_after_cut: bool,
    prefix: usize,
}

fn cut_run(seed: u64, ops: usize, refs: &Reference, k: u64, torn: bool) -> Outcome {
    let backend = FaultBackend::new();
    backend.cut_after(k, torn);
    let script = script(seed, ops);
    let acked = match backend.open(&options()) {
        Ok(g) => {
            let r = run(&g, &script, |_, _| {});
            drop(g);
            r.acked
        }
        Err(_) => 0,
    };
    backend.revive();
    let g = backend
        .open(&options())
        .unwrap_or_else(|e| panic!("cut {k} torn {torn}: reopen failed: {e}"));
    let report = g.scrub().unwrap();
    assert_eq!(report.integrity, Integrity::Clean, "cut {k} torn {torn}: {report:?}");
    assert!(report.is_clean(), "cut {k} torn {torn}: {report:?}");
    let logical = g.begin_read().unwrap().check_integrity().unwrap();
    assert!(logical.is_empty(), "cut {k} torn {torn}: {logical:?}");
    let epoch = g.epoch();
    let content = export(&g).unwrap();
    let prefix = refs
        .states
        .iter()
        .position(|(e, state)| *e == epoch && *state == content)
        .unwrap_or_else(|| panic!("cut {k} torn {torn}: state is not any committed prefix (epoch {epoch})"));
    let lo = acked;
    let hi = (acked + 1).min(refs.states.len() - 1);
    let in_window = (lo..=hi).any(|p| refs.states[p].0 == epoch && refs.states[p].1 == content);
    assert!(
        in_window,
        "cut {k} torn {torn}: matched prefix {prefix} but {acked} commits were acknowledged"
    );
    Outcome {
        opened_after_cut: true,
        prefix,
    }
}

#[test]
fn every_write_boundary_of_a_500_op_workload_reopens_to_a_committed_prefix() {
    let seed = 0x5EA1_C0DE;
    let ops = 500;
    let refs = reference(seed, ops);
    assert!(refs.total_ops >= 200, "only {} write operations", refs.total_ops);
    let stride = (refs.total_ops / 220).max(1);
    let mut checked = 0u64;
    let mut prefixes = std::collections::BTreeSet::new();
    let mut k = 0;
    while k < refs.total_ops {
        for torn in [false, true] {
            let out = cut_run(seed, ops, &refs, k, torn);
            assert!(out.opened_after_cut);
            prefixes.insert(out.prefix);
            checked += 1;
        }
        k += stride;
    }
    assert!(checked >= 400, "only {checked} cut runs");
    assert!(
        prefixes.len() >= 20,
        "cuts only reached {} distinct prefixes",
        prefixes.len()
    );
    eprintln!(
        "crash gate: {} write ops, {} cut runs, {} distinct prefixes, {} commits",
        refs.total_ops,
        checked,
        prefixes.len(),
        refs.states.len() - 1
    );
}

#[test]
fn a_cut_after_the_last_write_keeps_every_commit() {
    let seed = 7;
    let ops = 120;
    let refs = reference(seed, ops);
    let out = cut_run(seed, ops, &refs, refs.total_ops, false);
    assert_eq!(out.prefix, refs.states.len() - 1);
}

#[test]
fn a_cut_before_the_first_write_leaves_an_empty_database() {
    let seed = 7;
    let ops = 120;
    let refs = reference(seed, ops);
    let out = cut_run(seed, ops, &refs, 0, false);
    assert_eq!(out.prefix, 0);
}

#[test]
fn a_graph_over_the_fault_backend_matches_an_in_memory_graph() {
    let script = script(99, 150);
    let a = FaultBackend::new().open(&options()).unwrap();
    let b = Graph::open_in_memory().unwrap();
    assert!(!run(&a, &script, |_, _| {}).failed);
    assert!(!run(&b, &script, |_, _| {}).failed);
    assert_eq!(export(&a).unwrap(), export(&b).unwrap());
    assert_eq!(a.epoch(), b.epoch());
}
