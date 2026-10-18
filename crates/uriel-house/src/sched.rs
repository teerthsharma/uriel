//! The scheduler: which ready goals run now.
//!
//! **Ready set.** A goal is a candidate when it is `Pending` and either a live leaf, or a
//! non-leaf with a bound integration test whose children are all done. It is ready when every
//! `needs` entry is `Reached` (needs on an ALT loser were rewritten to the winner at merge).
//!
//! **Order.** Candidates are visited in natural goal-id order (`G1.2 < G1.10`, parents before
//! children). The order is total and does not depend on hash or insertion order, so the same
//! state and the same free RAM always give the same batch.
//!
//! **Gates**, applied to each ready candidate in that order, first failure wins:
//! bench exclusivity, agent cap, file lock (at most one writer per file across the batch
//! *and* every Active goal), heavy cap, RAM (class slots, then the headroom
//! `free - reserve` shared by everything heavy that is in flight or in this batch).
//! Light classes are charged against free RAM, not the reserve; `llm` costs nothing.
//! Declared costs of in-flight goals are charged on top of what the probe reports: a job
//! that was just dispatched has not allocated yet, so the probe alone would lag.

use std::collections::BTreeMap;

use crate::config::HouseConfig;
use crate::governor::{default_cost_mb, reserve_mb, slots};
use crate::probe::SystemSnapshot;
use crate::state::*;
use crate::types::*;

pub(crate) struct Planned {
    pub goal: GoalId,
    pub class: JobClass,
    pub cost_mb: u64,
    pub integration: bool,
}

pub(crate) struct Plan {
    pub planned: Vec<Planned>,
    pub report: GovernorReport,
}

fn files_of(g: &Goal) -> Vec<&str> {
    g.written_files().map(String::as_str).collect()
}

#[cfg(test)]
pub(crate) fn plan(state: &State, snap: SystemSnapshot, cfg: &HouseConfig) -> Plan {
    plan_for(state, snap, cfg, None)
}

pub(crate) fn plan_for(
    state: &State,
    snap: SystemSnapshot,
    cfg: &HouseConfig,
    need: Option<usize>,
) -> Plan {
    let free = snap.free_ram_mb;
    let total = snap.total_ram_mb;
    let reserve = reserve_mb(total, cfg);
    let headroom = free.saturating_sub(reserve);

    // what is already in flight
    let mut locked: BTreeMap<&str, &GoalId> = BTreeMap::new();
    let mut heavy_active = 0u32;
    let mut heavy_committed = 0u64;
    let mut light_committed = 0u64;
    let mut agents = 0u32;
    let mut bench_active = false;
    let mut in_class: BTreeMap<JobClass, u32> = BTreeMap::new();
    for (id, rec) in &state.goals {
        if rec.status != GoalStatus::Active {
            continue;
        }
        let class = rec.class();
        agents += 1;
        *in_class.entry(class).or_default() += 1;
        if class.is_heavy() {
            heavy_active += 1;
            heavy_committed += rec.cost_mb;
        }
        if class == JobClass::Light {
            light_committed += rec.cost_mb;
        }
        if class == JobClass::Bench {
            bench_active = true;
        }
        for f in files_of(&rec.goal) {
            locked.insert(f, id);
        }
    }
    let active_total = agents;

    let tu_cost = state
        .cost
        .average(JobClass::TestUnit)
        .unwrap_or_else(|| default_cost_mb(JobClass::TestUnit));
    let heavy_slots = cfg
        .heavy_cap
        .saturating_sub(heavy_active)
        .min(slots(JobClass::TestUnit, free, total, tu_cost, cfg).saturating_sub(heavy_active));

    let mut planned: Vec<Planned> = Vec::new();
    let mut deferrals: Vec<Deferral> = Vec::new();
    let mut batch_heavy = 0u32;
    let mut batch_heavy_cost = 0u64;
    let mut batch_light_cost = 0u64;
    let mut batch_has_bench = false;
    let mut draining = false;
    let mut defer = |goal: &GoalId, reason: DeferReason, detail: String| {
        deferrals.push(Deferral {
            goal: goal.clone(),
            reason,
            detail,
        });
    };

    for (id, rec) in &state.goals {
        if rec.status != GoalStatus::Pending || need.is_some_and(|n| id.need_ordinal() != n) {
            continue;
        }
        let integration = !rec.goal.leaf;
        if integration {
            let kids = state.children(id);
            if rec.goal.test.is_none() || kids.is_empty() || !kids.iter().all(|k| State::is_done(k))
            {
                continue;
            }
        }
        let unmet: Vec<String> = rec
            .goal
            .needs
            .iter()
            .filter(|n| {
                state
                    .goals
                    .get(*n)
                    .is_none_or(|r| r.status != GoalStatus::Reached)
            })
            .map(|n| match state.goals.get(n) {
                Some(r) => format!("{n} ({})", status_name(r.status)),
                None => format!("{n} (unknown)"),
            })
            .collect();
        if !unmet.is_empty() {
            defer(
                id,
                DeferReason::Needs,
                format!("waiting for {}", unmet.join(", ")),
            );
            continue;
        }
        let class = rec.class();
        let cost = state.cost_of(rec, class);

        // bench exclusivity
        if bench_active || batch_has_bench {
            defer(
                id,
                DeferReason::BenchExclusive,
                "a bench is running".to_string(),
            );
            continue;
        }
        if class == JobClass::Bench {
            if active_total > 0 || !planned.is_empty() {
                draining = true;
                defer(
                    id,
                    DeferReason::BenchExclusive,
                    format!(
                        "bench waits for {} running goal(s) to finish",
                        active_total + planned.len() as u32
                    ),
                );
                continue;
            }
        } else if draining {
            defer(
                id,
                DeferReason::BenchExclusive,
                "draining for a waiting bench".to_string(),
            );
            continue;
        }

        // agent cap
        if agents + planned.len() as u32 >= cfg.max_agents {
            defer(
                id,
                DeferReason::AgentCap,
                format!("{} agents already running", cfg.max_agents),
            );
            continue;
        }

        // one writer per file
        if let Some((f, holder)) = files_of(&rec.goal)
            .into_iter()
            .find_map(|f| locked.get(f).map(|h| (f, *h)))
        {
            defer(
                id,
                DeferReason::FileLock,
                format!("{f} is held by {holder}"),
            );
            continue;
        }

        // governor
        let class_count = in_class.get(&class).copied().unwrap_or(0)
            + planned.iter().filter(|p| p.class == class).count() as u32;
        match class {
            JobClass::TestUnit | JobClass::Build => {
                if heavy_active + batch_heavy >= cfg.heavy_cap {
                    defer(
                        id,
                        DeferReason::HeavyCap,
                        format!(
                            "{} heavy jobs already in flight (cap {})",
                            heavy_active + batch_heavy,
                            cfg.heavy_cap
                        ),
                    );
                    continue;
                }
                let class_slots = slots(class, free, total, cost, cfg);
                let over = heavy_committed + batch_heavy_cost + cost > headroom;
                if class_count >= class_slots || over {
                    defer(
                        id,
                        DeferReason::Ram,
                        format!(
                            "needs {cost} MB; free {free} MB, reserve {reserve} MB, {} MB already committed",
                            heavy_committed + batch_heavy_cost
                        ),
                    );
                    continue;
                }
            }
            JobClass::Light => {
                let class_slots = slots(class, free, total, cost, cfg);
                if class_count >= class_slots || light_committed + batch_light_cost + cost > free {
                    defer(
                        id,
                        DeferReason::Ram,
                        format!("needs {cost} MB; free {free} MB"),
                    );
                    continue;
                }
            }
            JobClass::Bench => {
                if slots(class, free, total, cost, cfg) == 0 {
                    defer(
                        id,
                        DeferReason::Ram,
                        format!(
                            "bench needs free RAM above the {reserve} MB reserve; free {free} MB"
                        ),
                    );
                    continue;
                }
            }
            JobClass::Llm => {}
        }

        // dispatch
        for f in files_of(&rec.goal) {
            locked.insert(f, id);
        }
        match class {
            JobClass::TestUnit | JobClass::Build => {
                batch_heavy += 1;
                batch_heavy_cost += cost;
            }
            JobClass::Light => batch_light_cost += cost,
            JobClass::Bench => batch_has_bench = true,
            JobClass::Llm => {}
        }
        planned.push(Planned {
            goal: id.clone(),
            class,
            cost_mb: cost,
            integration,
        });
    }

    Plan {
        planned,
        report: GovernorReport {
            free_mb: free,
            total_mb: total,
            reserve_mb: reserve,
            heavy_active,
            heavy_slots,
            deferrals,
        },
    }
}

pub(crate) fn status_name(s: GoalStatus) -> &'static str {
    match s {
        GoalStatus::Pending => "pending",
        GoalStatus::Active => "active",
        GoalStatus::Reached => "reached",
        GoalStatus::Blocked => "blocked",
        GoalStatus::Retired => "retired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> SystemSnapshot {
        SystemSnapshot {
            free_ram_mb: 60_000,
            total_ram_mb: 64_000,
            logical_cores: 8,
            load_pct: 0.0,
        }
    }

    fn rec(id: &str, writes: &[&str], status: GoalStatus) -> (GoalId, GoalRec) {
        let goal = Goal {
            id: GoalId::new(id),
            parent: Some(GoalId::new("G0")),
            leaf: true,
            tier: Some(Tier::Sonnet),
            writes: writes.iter().map(|s| s.to_string()).collect(),
            test: Some(TestSpec {
                name: format!("t_{id}"),
                cmd: "cargo test -p x".into(),
                class: "llm".into(),
            }),
            est: Some(Est::default()),
            ..Goal::default()
        };
        (
            GoalId::new(id),
            GoalRec {
                goal,
                owner: Fellow::Foreman,
                status,
                retired: None,
                blocked_reason: None,
                cost_mb: 0,
            },
        )
    }

    fn state(recs: Vec<(GoalId, GoalRec)>) -> State {
        let mut s = State::default();
        s.goals.extend(recs);
        s
    }

    #[test]
    fn one_writer_per_file_within_a_batch() {
        let s = state(vec![
            rec("G1", &["src/a.rs"], GoalStatus::Pending),
            rec("G2", &["src/a.rs"], GoalStatus::Pending),
            rec("G3", &["src/b.rs"], GoalStatus::Pending),
        ]);
        let p = plan(&s, snap(), &HouseConfig::default());
        let got: Vec<&str> = p.planned.iter().map(|x| x.goal.as_str()).collect();
        assert_eq!(got, vec!["G1", "G3"]);
        let d = p
            .report
            .deferrals
            .iter()
            .find(|d| d.goal.as_str() == "G2")
            .expect("deferral");
        assert_eq!(d.reason, DeferReason::FileLock);
        assert!(d.detail.contains("src/a.rs") && d.detail.contains("G1"));
    }

    #[test]
    fn an_active_goal_keeps_its_file_locked() {
        let s = state(vec![
            rec("G1", &["src/a.rs"], GoalStatus::Active),
            rec("G2", &["src/a.rs"], GoalStatus::Pending),
        ]);
        let p = plan(&s, snap(), &HouseConfig::default());
        assert!(p.planned.is_empty());
        assert_eq!(p.report.deferrals[0].reason, DeferReason::FileLock);
    }

    #[test]
    fn creates_are_locked_like_writes() {
        let (id, mut r) = rec("G2", &[], GoalStatus::Pending);
        r.goal.creates = vec!["src/new.rs".into()];
        let mut s = state(vec![rec("G1", &["src/new.rs"], GoalStatus::Active)]);
        s.goals.insert(id, r);
        let p = plan(&s, snap(), &HouseConfig::default());
        assert!(p.planned.is_empty());
    }

    #[test]
    fn agent_cap_limits_the_batch() {
        let recs = (1..=5)
            .map(|i| rec(&format!("G{i}"), &[], GoalStatus::Pending))
            .collect();
        let cfg = HouseConfig {
            max_agents: 3,
            ..HouseConfig::default()
        };
        let p = plan(&state(recs), snap(), &cfg);
        assert_eq!(p.planned.len(), 3);
        assert_eq!(
            p.report
                .deferrals
                .iter()
                .filter(|d| d.reason == DeferReason::AgentCap)
                .count(),
            2
        );
    }
}
