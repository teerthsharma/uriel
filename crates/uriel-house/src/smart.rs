//! Smart testing: result-cache keys, flake detection and test selection.
//!
//! `key = sha256(test command, content hash of every covered file, Cargo.lock hash)`.
//! A cached GREEN with the current key is reused; a RED never is. A test whose result
//! flips while its key is unchanged is quarantined. The flake guard needs a non-empty
//! COVERS set: with no covered files an unchanged key proves nothing.

use std::collections::BTreeSet;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::config::HouseConfig;
use crate::state::*;
use crate::types::*;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// SHA-256 hex of a file's content, or `absent` if it cannot be read.
pub(crate) fn file_hash(root: &Path, rel: &str) -> String {
    match std::fs::read(root.join(rel)) {
        Ok(bytes) => hex(&Sha256::digest(&bytes)),
        Err(_) => "absent".to_string(),
    }
}

/// The result-cache key of a test in the current tree.
pub(crate) fn cache_key(cmd: &str, covers: &BTreeSet<String>, root: &Path) -> String {
    let mut h = Sha256::new();
    h.update(cmd.as_bytes());
    h.update([0]);
    for f in covers {
        h.update(f.as_bytes());
        h.update([0]);
        h.update(file_hash(root, f).as_bytes());
        h.update([0]);
    }
    match std::fs::read(root.join("Cargo.lock")) {
        Ok(bytes) => h.update(Sha256::digest(&bytes)),
        Err(_) => h.update(b"no-lock"),
    }
    hex(&h.finalize())
}

/// `true` if a run with this key and the opposite status exists (flip with no change).
pub(crate) fn flips(runs: &[RunRec], key: &str, status: TestStatus) -> bool {
    runs.iter().any(|r| r.key == key && r.status != status)
}

pub(crate) fn select(state: &State, touched: &[String], cfg: &HouseConfig) -> Vec<TestPick> {
    let touched: BTreeSet<&str> = touched.iter().map(String::as_str).collect();
    let mut chosen: BTreeSet<String> = BTreeSet::new();
    for (name, t) in &state.tests {
        if t.covers.iter().any(|c| touched.contains(c.as_str())) {
            chosen.insert(name.clone());
        }
    }
    for rec in state.goals.values() {
        if rec.status == GoalStatus::Retired {
            continue;
        }
        let g = &rec.goal;
        let hit = g
            .reads
            .iter()
            .chain(g.writes.iter())
            .chain(g.creates.iter())
            .any(|f| touched.contains(f.as_str()));
        if hit {
            if let Some(t) = &g.test {
                chosen.insert(t.name.clone());
            }
        }
    }
    // L1: integration tests of ancestors of the goals whose tests were chosen
    let mut l1: BTreeSet<String> = BTreeSet::new();
    for name in &chosen {
        let Some(t) = state.tests.get(name) else {
            continue;
        };
        for a in state.ancestors(&t.goal) {
            if let Some(test) = state.goals.get(&a).and_then(|r| r.goal.test.as_ref()) {
                if !chosen.contains(&test.name) {
                    l1.insert(test.name.clone());
                }
            }
        }
    }
    let mut picks: Vec<(Level, bool, u64, TestPick)> = Vec::new();
    for (name, level) in chosen
        .iter()
        .map(|n| (n, None))
        .chain(l1.iter().map(|n| (n, Some(Level::L1))))
    {
        let Some(t) = state.tests.get(name) else {
            continue;
        };
        let is_parent = state.goals.get(&t.goal).is_some_and(|r| !r.goal.leaf);
        let level = level.unwrap_or(if is_parent { Level::L1 } else { Level::L0 });
        let last = t.runs.last();
        let last_failed = last.is_some_and(|r| r.status == TestStatus::Red);
        let last_fail_seq = t
            .runs
            .iter()
            .rev()
            .find(|r| r.status == TestStatus::Red)
            .map_or(0, |r| r.seq);
        let key = cache_key(&t.spec.cmd, &t.covers, &cfg.project_root);
        let action = if t.quarantined {
            PickAction::Quarantined
        } else if t
            .runs
            .iter()
            .any(|r| r.key == key && r.status == TestStatus::Green)
        {
            PickAction::ReuseGreen
        } else {
            PickAction::Run
        };
        let est_secs = if t.runs.is_empty() {
            state
                .goals
                .get(&t.goal)
                .and_then(|r| r.goal.est)
                .map_or(0.0, |e| e.secs)
        } else {
            t.runs.iter().map(|r| r.secs).sum::<f64>() / t.runs.len() as f64
        };
        picks.push((
            level,
            last_failed,
            last_fail_seq,
            TestPick {
                test: name.clone(),
                cmd: t.spec.cmd.clone(),
                level,
                action,
                last_failed,
                est_secs,
            },
        ));
    }
    let rank = |l: Level| match l {
        Level::L0 => 0,
        Level::L1 => 1,
        Level::L2 => 2,
    };
    picks.sort_by(|a, b| {
        rank(a.0)
            .cmp(&rank(b.0))
            .then(b.1.cmp(&a.1)) // failed first
            .then(b.2.cmp(&a.2)) // most recently failed first
            .then(
                a.3.est_secs
                    .partial_cmp(&b.3.est_secs)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.3.test.cmp(&b.3.test))
    });
    let mut out: Vec<TestPick> = picks.into_iter().map(|p| p.3).collect();
    if cfg.allow_full_suite {
        out.push(TestPick {
            test: "<full-suite>".to_string(),
            cmd: cfg.full_suite_cmd.clone(),
            level: Level::L2,
            action: PickAction::Run,
            last_failed: false,
            est_secs: 0.0,
        });
    }
    out
}
