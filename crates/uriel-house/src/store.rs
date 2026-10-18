//! Persistence of a House in a seal graph.
//!
//! Nodes: `Project`, `Goal`, `Test`, `File`, `TestRun`, `CostObs`, `Decision`, `AltGroup`.
//! Edges: `DECOMPOSES_INTO` (parent to child), `NEEDS`, `TOUCHES{mode}` (goal to file),
//! `COVERS` (test to file), `BOUND_BY` (goal to its test), `GUARDS` (guard to guarded goal),
//! `ALT_OF` (member to its group), `RETIRED{reason}` (retired goal to the winner or the
//! refuting fact).
//!
//! Every `Goal` node carries its full record as JSON, so loading never re-derives anything
//! from edges; the edges exist for queries (Cypher through uriel-query) and are rebuilt
//! whenever the merged tree changes.

use std::collections::BTreeMap;
use std::path::Path;

use uriel_core::{ConstraintKind, Graph, NodeId, Props, Value, WriteTx};

use crate::error::HouseError;
use crate::smart;
use crate::state::*;
use crate::types::*;

const STRUCTURAL: [&str; 7] = [
    "DECOMPOSES_INTO",
    "NEEDS",
    "TOUCHES",
    "BOUND_BY",
    "GUARDS",
    "ALT_OF",
    "RETIRED",
];

#[derive(Clone, Default)]
struct Ids {
    goals: BTreeMap<String, NodeId>,
    tests: BTreeMap<String, NodeId>,
    files: BTreeMap<String, NodeId>,
    alts: BTreeMap<String, NodeId>,
    decisions: BTreeMap<String, NodeId>,
    project: Option<NodeId>,
}

pub(crate) struct Store {
    graph: Graph,
    ids: Ids,
}

fn text(props: &Props, key: &str) -> Option<String> {
    match props.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn int(props: &Props, key: &str) -> Option<i64> {
    match props.get(key) {
        Some(Value::Int(i)) => Some(*i),
        _ => None,
    }
}

fn float(props: &Props, key: &str) -> Option<f64> {
    match props.get(key) {
        Some(Value::Float(f)) => Some(*f),
        Some(Value::Int(i)) => Some(*i as f64),
        _ => None,
    }
}

fn corrupt(what: &str) -> HouseError {
    HouseError::Corrupt(what.to_string())
}

fn props<const N: usize>(items: [(&str, Value); N]) -> Props {
    items.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn opt(s: Option<String>) -> Value {
    s.map_or(Value::Null, Value::String)
}

fn upsert(
    tx: &mut WriteTx,
    map: &mut BTreeMap<String, NodeId>,
    label: &str,
    key: &str,
    p: Props,
) -> Result<NodeId, HouseError> {
    if let Some(id) = map.get(key) {
        tx.set_node_props(*id, p)?;
        Ok(*id)
    } else {
        let id = tx.create_node(&[label], p)?;
        map.insert(key.to_string(), id);
        Ok(id)
    }
}

fn class_from_key(k: &str) -> Option<JobClass> {
    match k {
        "llm" => Some(JobClass::Llm),
        "light" => Some(JobClass::Light),
        "test_unit" => Some(JobClass::TestUnit),
        "build" => Some(JobClass::Build),
        "bench" => Some(JobClass::Bench),
        _ => None,
    }
}

impl Store {
    /// Opens the store over `graph`, creating constraints, and loads the stored state.
    pub fn open(graph: Graph, cost_window: usize) -> Result<(Store, State), HouseError> {
        let existing = graph.list_constraints()?;
        for (label, key) in [
            ("Goal", "gid"),
            ("Test", "name"),
            ("File", "path"),
            ("AltGroup", "id"),
            ("Decision", "key"),
        ] {
            let has = existing
                .iter()
                .any(|c| c.kind == ConstraintKind::Unique && c.label == label && c.key == key);
            if !has && !graph.is_read_only() {
                graph.create_unique_constraint(label, key)?;
            }
        }
        let mut store = Store {
            graph,
            ids: Ids::default(),
        };
        let state = store.load(cost_window)?;
        Ok((store, state))
    }

    fn load(&mut self, cost_window: usize) -> Result<State, HouseError> {
        let tx = self.graph.begin_read()?;
        let mut state = State::default();
        let mut ids = Ids::default();

        for n in tx.nodes_by_label("Project")? {
            let n = n?;
            let meta: Meta = serde_json::from_str(
                &text(&n.props, "meta").ok_or_else(|| corrupt("project meta"))?,
            )?;
            state.apply_meta(meta);
            ids.project = Some(n.id);
        }
        for n in tx.nodes_by_label("Goal")? {
            let n = n?;
            let gid = text(&n.props, "gid").ok_or_else(|| corrupt("goal without gid"))?;
            let rec: GoalRec =
                serde_json::from_str(&text(&n.props, "json").ok_or_else(|| corrupt("goal json"))?)?;
            state.goals.insert(GoalId::new(gid.clone()), rec);
            ids.goals.insert(gid, n.id);
        }
        for n in tx.nodes_by_label("File")? {
            let n = n?;
            if let Some(p) = text(&n.props, "path") {
                ids.files.insert(p, n.id);
            }
        }
        for n in tx.nodes_by_label("AltGroup")? {
            let n = n?;
            if let Some(p) = text(&n.props, "id") {
                ids.alts.insert(p, n.id);
            }
        }
        for n in tx.nodes_by_label("Decision")? {
            let n = n?;
            if let Some(p) = text(&n.props, "key") {
                ids.decisions.insert(p, n.id);
            }
        }
        for n in tx.nodes_by_label("Test")? {
            let n = n?;
            let name = text(&n.props, "name").ok_or_else(|| corrupt("test without name"))?;
            let spec = TestSpec {
                name: name.clone(),
                cmd: text(&n.props, "cmd").unwrap_or_default(),
                class: text(&n.props, "class").unwrap_or_default(),
            };
            let goal = GoalId::new(text(&n.props, "goal").unwrap_or_default());
            let quarantined = matches!(n.props.get("quarantined"), Some(Value::Bool(true)));
            let mut covers = std::collections::BTreeSet::new();
            for e in tx.out_edges(n.id, Some("COVERS"))? {
                let e = e?;
                if let Some(f) = tx.get_node(e.other)? {
                    if let Some(p) = text(&f.props, "path") {
                        covers.insert(p);
                    }
                }
            }
            ids.tests.insert(name.clone(), n.id);
            state.tests.insert(
                name,
                TestRec {
                    spec,
                    goal,
                    covers,
                    quarantined,
                    runs: Vec::new(),
                },
            );
        }
        let mut runs: Vec<(String, RunRec)> = Vec::new();
        for n in tx.nodes_by_label("TestRun")? {
            let n = n?;
            let status = match text(&n.props, "status").as_deref() {
                Some("red") => TestStatus::Red,
                Some("green") => TestStatus::Green,
                _ => return Err(corrupt("test run status")),
            };
            runs.push((
                text(&n.props, "test").unwrap_or_default(),
                RunRec {
                    key: text(&n.props, "key").unwrap_or_default(),
                    status,
                    secs: float(&n.props, "secs").unwrap_or(0.0),
                    ram_mb: int(&n.props, "ram_mb").unwrap_or(0).max(0) as u64,
                    seq: int(&n.props, "seq").unwrap_or(0).max(0) as u64,
                },
            ));
        }
        runs.sort_by_key(|(_, r)| r.seq);
        for (test, run) in runs {
            state.seq = state.seq.max(run.seq);
            if let Some(t) = state.tests.get_mut(&test) {
                t.runs.push(run);
            }
        }
        let mut obs: Vec<(u64, JobClass, u64)> = Vec::new();
        for n in tx.nodes_by_label("CostObs")? {
            let n = n?;
            if let Some(c) = text(&n.props, "class").as_deref().and_then(class_from_key) {
                obs.push((
                    int(&n.props, "seq").unwrap_or(0).max(0) as u64,
                    c,
                    int(&n.props, "ram_mb").unwrap_or(0).max(0) as u64,
                ));
            }
        }
        obs.sort_by_key(|o| o.0);
        for (seq, class, ram) in obs {
            state.seq = state.seq.max(seq);
            state.cost.record(class, ram, cost_window);
        }
        drop(tx);
        self.ids = ids;
        Ok(state)
    }

    /// Writes everything marked dirty in one transaction.
    pub fn flush(&mut self, state: &mut State, root: &Path) -> Result<(), HouseError> {
        let d = &state.dirty;
        let nothing = !d.meta
            && d.goals.is_empty()
            && !d.structure
            && d.tests.is_empty()
            && d.runs.is_empty()
            && d.cost.is_empty()
            && d.covers.is_empty()
            && d.facts.is_empty();
        if nothing {
            return Ok(());
        }
        let mut ids = self.ids.clone();
        let mut tx = self.graph.begin_write()?;

        // goals
        let goal_ids: Vec<GoalId> = if state.dirty.structure {
            state.goals.keys().cloned().collect()
        } else {
            state.dirty.goals.iter().cloned().collect()
        };
        for gid in &goal_ids {
            let Some(rec) = state.goals.get(gid) else {
                continue;
            };
            let p = props([
                ("gid", Value::String(gid.to_string())),
                ("text", Value::String(rec.goal.text.clone())),
                ("leaf", Value::Bool(rec.goal.leaf)),
                ("tier", opt(rec.goal.tier.map(|t| t.model().to_string()))),
                (
                    "status",
                    Value::String(crate::sched::status_name(rec.status).to_string()),
                ),
                ("class", opt(rec.goal.class.clone())),
                ("by", Value::String(rec.owner.name().to_string())),
                (
                    "retired_reason",
                    opt(rec.retired.as_ref().map(|r| r.reason.clone())),
                ),
                ("json", Value::String(serde_json::to_string(rec)?)),
            ]);
            upsert(&mut tx, &mut ids.goals, "Goal", gid.as_str(), p)?;
        }

        // tests
        let test_names: Vec<String> = if state.dirty.structure {
            state.tests.keys().cloned().collect()
        } else {
            state.dirty.tests.iter().cloned().collect()
        };
        for name in &test_names {
            let Some(t) = state.tests.get(name) else {
                continue;
            };
            let p = props([
                ("name", Value::String(name.clone())),
                ("cmd", Value::String(t.spec.cmd.clone())),
                ("class", Value::String(t.spec.class.clone())),
                ("goal", Value::String(t.goal.to_string())),
                ("quarantined", Value::Bool(t.quarantined)),
            ]);
            upsert(&mut tx, &mut ids.tests, "Test", name, p)?;
        }

        // facts as decisions
        for fid in &state.dirty.facts {
            let Some(f) = state.facts.iter().find(|f| &f.id == fid) else {
                continue;
            };
            let key = format!("fact:{fid}");
            let p = props([
                ("key", Value::String(key.clone())),
                ("kind", Value::String("fact".into())),
                ("text", Value::String(f.text.clone())),
                ("by", Value::String(f.by.clone())),
                ("source", Value::String(f.source.clone())),
                ("refutes", opt(f.refutes.as_ref().map(ToString::to_string))),
            ]);
            upsert(&mut tx, &mut ids.decisions, "Decision", &key, p)?;
        }

        if state.dirty.structure {
            // ALT groups
            for (target, g) in &state.alt_groups {
                let key = format!("ALT:{target}");
                let p = props([
                    ("id", Value::String(key.clone())),
                    ("winner", opt(g.winner.as_ref().map(ToString::to_string))),
                    ("members", Value::String(serde_json::to_string(&g.members)?)),
                ]);
                upsert(&mut tx, &mut ids.alts, "AltGroup", &key, p)?;
            }
            // clear and rebuild structural edges
            for gid in state.goals.keys() {
                let Some(node) = ids.goals.get(gid.as_str()).copied() else {
                    continue;
                };
                for e in tx.out_edges(node, None)? {
                    if STRUCTURAL.contains(&e.rel_type.as_str()) {
                        tx.delete_rel(e.rel)?;
                    }
                }
            }
            for (gid, rec) in &state.goals {
                let Some(node) = ids.goals.get(gid.as_str()).copied() else {
                    continue;
                };
                if let Some(p) = rec
                    .goal
                    .parent
                    .as_ref()
                    .and_then(|p| ids.goals.get(p.as_str()).copied())
                {
                    tx.create_rel(p, node, "DECOMPOSES_INTO", Props::new())?;
                }
                for n in &rec.goal.needs {
                    if let Some(t) = ids.goals.get(n.as_str()).copied() {
                        tx.create_rel(node, t, "NEEDS", Props::new())?;
                    }
                }
                let touched = rec
                    .goal
                    .reads
                    .iter()
                    .map(|f| (f, "read"))
                    .chain(rec.goal.writes.iter().map(|f| (f, "write")))
                    .chain(rec.goal.creates.iter().map(|f| (f, "create")));
                for (f, mode) in touched {
                    let fnode = file_node(&mut tx, &mut ids.files, root, f)?;
                    tx.create_rel(
                        node,
                        fnode,
                        "TOUCHES",
                        props([("mode", Value::String(mode.into()))]),
                    )?;
                }
                if let Some(t) = rec
                    .goal
                    .test
                    .as_ref()
                    .and_then(|t| ids.tests.get(&t.name).copied())
                {
                    tx.create_rel(node, t, "BOUND_BY", Props::new())?;
                }
                if let Some(t) = rec
                    .goal
                    .guards
                    .as_ref()
                    .and_then(|t| ids.goals.get(t.as_str()).copied())
                {
                    tx.create_rel(node, t, "GUARDS", Props::new())?;
                }
                if let Some(r) = &rec.retired {
                    let target = if r.reason == "refuted" {
                        ids.decisions.get(&format!("fact:{}", r.by)).copied()
                    } else {
                        ids.goals.get(r.by.as_str()).copied()
                    };
                    if let Some(t) = target {
                        tx.create_rel(
                            node,
                            t,
                            "RETIRED",
                            props([("reason", Value::String(r.reason.clone()))]),
                        )?;
                    }
                }
            }
            for (target, g) in &state.alt_groups {
                let Some(group) = ids.alts.get(&format!("ALT:{target}")).copied() else {
                    continue;
                };
                for m in &g.members {
                    if let Some(node) = ids.goals.get(m.as_str()).copied() {
                        tx.create_rel(node, group, "ALT_OF", Props::new())?;
                    }
                }
            }
        }

        // covers
        for (test, file) in &state.dirty.covers {
            let (Some(t), fnode) = (
                ids.tests.get(test).copied(),
                file_node(&mut tx, &mut ids.files, root, file)?,
            ) else {
                continue;
            };
            let have = tx
                .out_edges(t, Some("COVERS"))?
                .iter()
                .any(|e| e.other == fnode);
            if !have {
                tx.create_rel(t, fnode, "COVERS", Props::new())?;
            }
        }
        // runs and cost observations
        for (test, run) in &state.dirty.runs {
            tx.create_node(
                &["TestRun"],
                props([
                    ("test", Value::String(test.clone())),
                    ("key", Value::String(run.key.clone())),
                    ("status", Value::String(run.status.name().into())),
                    ("secs", Value::Float(run.secs)),
                    (
                        "ram_mb",
                        Value::Int(i64::try_from(run.ram_mb).unwrap_or(i64::MAX)),
                    ),
                    (
                        "seq",
                        Value::Int(i64::try_from(run.seq).unwrap_or(i64::MAX)),
                    ),
                ]),
            )?;
        }
        for (class, ram, seq) in &state.dirty.cost {
            tx.create_node(
                &["CostObs"],
                props([
                    ("class", Value::String(class.key().into())),
                    (
                        "ram_mb",
                        Value::Int(i64::try_from(*ram).unwrap_or(i64::MAX)),
                    ),
                    ("seq", Value::Int(i64::try_from(*seq).unwrap_or(i64::MAX))),
                ]),
            )?;
        }

        // project / meta
        if state.dirty.meta || (ids.project.is_none() && !state.needs.is_empty()) {
            if let Some(need) = state.needs.first().map(|n| &n.rec) {
                let p = props([
                    ("name", Value::String(need.project.clone())),
                    ("goal", Value::String(need.goal.clone())),
                    ("meta", Value::String(serde_json::to_string(&state.meta())?)),
                ]);
                match ids.project {
                    Some(id) => tx.set_node_props(id, p)?,
                    None => ids.project = Some(tx.create_node(&["Project"], p)?),
                }
            }
        }
        tx.commit()?;
        self.ids = ids;
        state.dirty = Dirty::default();
        Ok(())
    }
}

fn file_node(
    tx: &mut WriteTx,
    files: &mut BTreeMap<String, NodeId>,
    root: &Path,
    path: &str,
) -> Result<NodeId, HouseError> {
    let hash = smart::file_hash(root, path);
    upsert(
        tx,
        files,
        "File",
        path,
        props([
            ("path", Value::String(path.to_string())),
            ("hash", Value::String(hash)),
        ]),
    )
}
