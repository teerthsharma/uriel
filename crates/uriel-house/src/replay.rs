#![allow(missing_docs)]

use std::sync::{Mutex, PoisonError};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::probe::{SystemProbe, SystemSnapshot};
use crate::sink::EventSink;
use crate::types::*;

const DIGEST_DOMAIN: &[u8] = b"uriel-house/decision-log/v1\0";

const EMPTY_SNAPSHOT: SystemSnapshot = SystemSnapshot {
    free_ram_mb: 0,
    total_ram_mb: 0,
    logical_cores: 0,
    load_pct: 0.0,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRecord {
    pub goal: GoalId,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRecord {
    pub call_index: u64,
    pub probe: SystemSnapshot,
    pub dispatched: Vec<DispatchRecord>,
    pub deferrals: Vec<Deferral>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HouseCall {
    Need {
        project: String,
        goal: String,
        acceptance: Vec<String>,
    },
    SubmitChart {
        fellow: Fellow,
        chart: Chart,
    },
    Merge,
    AddFact(Fact),
    Next,
    Report {
        goal: GoalId,
        status: GoalStatus,
        evidence: TestEvidence,
    },
    RecordTestRun {
        test: String,
        status: TestStatus,
        secs: f64,
        peak_ram_mb: u64,
        covered_files: Vec<String>,
        key: Option<String>,
    },
}

fn reason_tag(r: DeferReason) -> u8 {
    match r {
        DeferReason::Ram => 1,
        DeferReason::HeavyCap => 2,
        DeferReason::FileLock => 3,
        DeferReason::Needs => 4,
        DeferReason::BenchExclusive => 5,
        DeferReason::AgentCap => 6,
    }
}

fn put_u64(h: &mut Sha256, v: u64) {
    h.update(v.to_le_bytes());
}

fn put_str(h: &mut Sha256, s: &str) {
    put_u64(h, s.len() as u64);
    h.update(s.as_bytes());
}

pub fn decision_digest(log: &[DecisionRecord]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(DIGEST_DOMAIN);
    put_u64(&mut h, log.len() as u64);
    for rec in log {
        put_u64(&mut h, rec.call_index);
        put_u64(&mut h, rec.probe.free_ram_mb);
        put_u64(&mut h, rec.probe.total_ram_mb);
        h.update(rec.probe.logical_cores.to_le_bytes());
        h.update(rec.probe.load_pct.to_bits().to_le_bytes());
        put_u64(&mut h, rec.dispatched.len() as u64);
        for d in &rec.dispatched {
            put_str(&mut h, d.goal.as_str());
            put_str(&mut h, &d.model);
        }
        put_u64(&mut h, rec.deferrals.len() as u64);
        for d in &rec.deferrals {
            put_str(&mut h, d.goal.as_str());
            h.update([reason_tag(d.reason)]);
            put_str(&mut h, &d.detail);
        }
    }
    let out = h.finalize();
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&out);
    digest
}

pub(crate) struct ScriptedProbe {
    script: Vec<SystemSnapshot>,
    cursor: Mutex<usize>,
}

impl ScriptedProbe {
    pub(crate) fn new(script: Vec<SystemSnapshot>) -> ScriptedProbe {
        ScriptedProbe {
            script,
            cursor: Mutex::new(0),
        }
    }
}

impl SystemProbe for ScriptedProbe {
    fn snapshot(&self) -> SystemSnapshot {
        let mut cursor = self.cursor.lock().unwrap_or_else(PoisonError::into_inner);
        let snap = self
            .script
            .get(*cursor)
            .or_else(|| self.script.last())
            .copied()
            .unwrap_or(EMPTY_SNAPSHOT);
        *cursor = cursor.saturating_add(1);
        snap
    }
}

pub(crate) struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: Value) {}
}
