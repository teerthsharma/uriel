//! Validator rules 1-10 of the spec.
//!
//! [`validate_chart`] checks one fellow's chart on its own (a Foreman chart is a tree under
//! the virtual root `G0`; Chase and Cameron nodes are all leaves). [`validate_merged`]
//! checks the effective merged tree, where every reference must also resolve.
//!
//! Rule numbers follow `docs/HOUSE-SEAL.md`:
//! 1 parent tree and `needs` DAG, 2 depth <= 6 and fan-out <= 7, 3 non-leaf has >= 2
//! children, 4 leaf has tier, test command and estimate, 5 Haiku leaf is read-only,
//! 6 token caps, 7 writers of one file are ordered, 8 distinct scoped tests,
//! 9 paths exist or are created, 10 Chase names `guards` and Cameron names `alt_of`.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::HouseConfig;
use crate::types::{Chart, Fellow, Goal, GoalId, JobClass, Tier, Violation};

/// Deepest allowed goal (G0 is depth 0).
pub const MAX_DEPTH: usize = 6;
/// Most children a node may have.
pub const MAX_FAN_OUT: usize = 7;
/// Goal classes a nurse may carry.
pub const HAIKU_CLASSES: [&str; 5] = ["read", "grep", "run_test", "collect", "format"];

/// One goal of a tree under validation.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub fellow: Fellow,
    pub goal: Goal,
    /// A refuted goal waiting for its re-chart: counts as a child, is not checked as a leaf.
    pub placeholder: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Chart(Fellow),
    Merged,
}

/// Validates one fellow's chart in isolation.
pub fn validate_chart(chart: &Chart, cfg: &HouseConfig) -> Vec<Violation> {
    let entries: Vec<Entry> = chart
        .goals
        .iter()
        .map(|g| Entry {
            fellow: chart.fellow,
            goal: g.clone(),
            placeholder: false,
        })
        .collect();
    let root = GoalId::new(chart.need.clone());
    run(&entries, &root, Mode::Chart(chart.fellow), None, cfg)
}

/// Validates the merged, effective tree (rooted at `root`).
pub(crate) fn validate_merged(
    root: &GoalId,
    goals: &[Entry],
    known: &BTreeSet<GoalId>,
    cfg: &HouseConfig,
) -> Vec<Violation> {
    run(goals, root, Mode::Merged, Some(known), cfg)
}

fn viol(rule: u8, e: &Entry, message: impl Into<String>) -> Violation {
    Violation {
        rule,
        fellow: e.fellow,
        goal: e.goal.id.clone(),
        message: message.into(),
    }
}

/// `true` when a shell command runs a whole-workspace cargo test.
pub fn is_whole_workspace(cmd: &str) -> bool {
    for segment in cmd.split(['&', ';', '|']) {
        let tokens: Vec<&str> = segment.split_whitespace().collect();
        let Some(pos) = tokens
            .iter()
            .position(|t| *t == "cargo" || t.ends_with("cargo.exe") || t.ends_with("/cargo"))
        else {
            continue;
        };
        let mut rest = tokens[pos + 1..].iter().copied().peekable();
        while rest.peek().is_some_and(|t| t.starts_with('+')) {
            rest.next();
        }
        match rest.next() {
            Some("test") | Some("nextest") => {}
            _ => continue,
        }
        let args: Vec<&str> = rest.collect();
        let workspace = args.iter().any(|a| *a == "--workspace" || *a == "--all");
        let scoped = args.iter().any(|a| {
            *a == "-p"
                || *a == "--package"
                || a.starts_with("--package=")
                || (a.starts_with("-p") && a.len() > 2 && !a.starts_with("--"))
                || *a == "--manifest-path"
                || a.starts_with("--manifest-path=")
        });
        if workspace || !scoped {
            return true;
        }
    }
    false
}

fn run(
    entries: &[Entry],
    root: &GoalId,
    mode: Mode,
    known: Option<&BTreeSet<GoalId>>,
    cfg: &HouseConfig,
) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.goal.id.cmp(&b.goal.id));
    let idx: BTreeMap<&GoalId, &Entry> = sorted.iter().map(|e| (&e.goal.id, *e)).collect();
    let tree_mode = !matches!(mode, Mode::Chart(Fellow::Chase | Fellow::Cameron));
    let is_leaf = |e: &Entry| !tree_mode || e.goal.leaf;

    // duplicate ids inside one set
    {
        let mut seen = BTreeSet::new();
        for e in &sorted {
            if !seen.insert(&e.goal.id) {
                out.push(viol(1, e, format!("goal id {} appears twice", e.goal.id)));
            }
        }
    }

    if tree_mode {
        tree_rules(&sorted, &idx, root, &mut out);
    }
    needs_rules(&sorted, &idx, mode, known, &mut out);
    let closure = needs_closure(&sorted, &idx);

    // rules 4, 5, 6 (leaves)
    for e in sorted.iter().filter(|e| is_leaf(e) && !e.placeholder) {
        let g = &e.goal;
        match &g.test {
            None => out.push(viol(4, e, "leaf has no test")),
            Some(t) => {
                if t.name.trim().is_empty() {
                    out.push(viol(4, e, "leaf test has no name"));
                }
                if t.cmd.trim().is_empty() {
                    out.push(viol(4, e, "leaf test has no command"));
                }
                if JobClass::parse(&t.class).is_none() {
                    out.push(viol(4, e, format!("unknown test class `{}`", t.class)));
                }
            }
        }
        if g.tier.is_none() {
            out.push(viol(4, e, "leaf has no tier"));
        }
        if g.est.is_none() {
            out.push(viol(4, e, "leaf has no estimate"));
        }
        if g.tier == Some(Tier::Haiku) {
            if g.written_files().next().is_some() {
                out.push(viol(5, e, "haiku leaf must not write or create files"));
            }
            match g.class.as_deref() {
                Some(c) if HAIKU_CLASSES.contains(&c) => {}
                other => out.push(viol(
                    5,
                    e,
                    format!(
                        "haiku leaf class {} is not one of read|grep|run_test|collect|format",
                        other.map_or("(none)".to_string(), |c| format!("`{c}`"))
                    ),
                )),
            }
        }
        if let (Some(tier), Some(est)) = (g.tier, g.est) {
            if est.tokens > tier.token_cap() {
                out.push(viol(
                    6,
                    e,
                    format!(
                        "{} tokens exceeds the {} cap of {}; decompose further",
                        est.tokens,
                        tier.model(),
                        tier.token_cap()
                    ),
                ));
            }
        }
    }

    // rule 7: writers of one file must be ordered
    let mut writers: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in sorted.iter().filter(|e| is_leaf(e) && !e.placeholder) {
        let files: BTreeSet<&str> = e.goal.written_files().map(String::as_str).collect();
        for f in files {
            writers.entry(f).or_default().push(e);
        }
    }
    for (file, ws) in &writers {
        for (i, a) in ws.iter().enumerate() {
            for b in &ws[i + 1..] {
                let ordered = closure
                    .get(&b.goal.id)
                    .is_some_and(|c| c.contains(&a.goal.id))
                    || closure
                        .get(&a.goal.id)
                        .is_some_and(|c| c.contains(&b.goal.id));
                if !ordered {
                    out.push(viol(
                        7,
                        b,
                        format!(
                            "{} and {} both write {file} with no needs ordering",
                            a.goal.id, b.goal.id
                        ),
                    ));
                }
            }
        }
    }

    // rule 8: distinct, scoped tests
    let mut names: BTreeMap<&str, &GoalId> = BTreeMap::new();
    for e in sorted.iter().filter(|e| is_leaf(e) && !e.placeholder) {
        let Some(t) = &e.goal.test else { continue };
        if !t.name.trim().is_empty() {
            if let Some(first) = names.insert(t.name.as_str(), &e.goal.id) {
                out.push(viol(
                    8,
                    e,
                    format!("test name `{}` already used by {first}", t.name),
                ));
            }
        }
        if is_whole_workspace(&t.cmd) {
            out.push(viol(
                8,
                e,
                format!("`{}` runs the whole workspace; scope it with -p", t.cmd),
            ));
        }
    }

    // rule 9: paths exist or are created (here or by something this leaf needs)
    for e in sorted.iter().filter(|e| is_leaf(e) && !e.placeholder) {
        let g = &e.goal;
        let inherited: BTreeSet<&str> = closure
            .get(&g.id)
            .into_iter()
            .flatten()
            .filter_map(|id| idx.get(id))
            .flat_map(|d| d.goal.creates.iter().map(String::as_str))
            .collect();
        let mut seen = BTreeSet::new();
        for p in g.reads.iter().chain(g.writes.iter()) {
            if !seen.insert(p) {
                continue;
            }
            let ok = g.creates.contains(p)
                || inherited.contains(p.as_str())
                || cfg.project_root.join(p).exists();
            if !ok {
                out.push(viol(
                    9,
                    e,
                    format!("path `{p}` does not exist and no goal declares `creates` for it"),
                ));
            }
        }
    }

    // rule 10
    for e in &sorted {
        let chase = e.fellow == Fellow::Chase;
        let cameron = e.fellow == Fellow::Cameron;
        if chase && e.goal.guards.is_none() {
            out.push(viol(10, e, "Chase node must name the goal it guards"));
        }
        if cameron && e.goal.alt_of.is_none() {
            out.push(viol(
                10,
                e,
                "Cameron node must name the goal it is an alternative to",
            ));
        }
    }
    out
}

fn tree_rules(
    sorted: &[&Entry],
    idx: &BTreeMap<&GoalId, &Entry>,
    root: &GoalId,
    out: &mut Vec<Violation>,
) {
    let mut children: BTreeMap<&GoalId, usize> = BTreeMap::new();
    for e in sorted {
        if let Some(p) = &e.goal.parent {
            *children.entry(p).or_default() += 1;
        }
    }
    for e in sorted {
        // walk to the root, detecting cycles and unknown parents
        let mut visited: BTreeSet<&GoalId> = BTreeSet::new();
        visited.insert(&e.goal.id);
        let mut cur = e.goal.parent.as_ref();
        let mut depth = 0usize;
        loop {
            let Some(p) = cur else {
                out.push(viol(1, e, "goal has no parent"));
                break;
            };
            depth += 1;
            if p == root {
                if depth > MAX_DEPTH {
                    out.push(viol(
                        2,
                        e,
                        format!("depth {depth} exceeds the maximum of {MAX_DEPTH}"),
                    ));
                }
                break;
            }
            if !visited.insert(p) {
                out.push(viol(1, e, format!("parent links form a cycle through {p}")));
                break;
            }
            match idx.get(p) {
                None => {
                    out.push(viol(1, e, format!("parent {p} is not in the chart")));
                    break;
                }
                Some(pe) => cur = pe.goal.parent.as_ref(),
            }
        }
        let n = children.get(&e.goal.id).copied().unwrap_or(0);
        if n > MAX_FAN_OUT {
            out.push(viol(
                2,
                e,
                format!("fan-out {n} exceeds the maximum of {MAX_FAN_OUT}"),
            ));
        }
        if e.goal.leaf && n > 0 {
            out.push(viol(3, e, format!("leaf has {n} children")));
        }
        if !e.goal.leaf && !e.placeholder && n < 2 {
            out.push(viol(
                3,
                e,
                format!("non-leaf has {n} child(ren); a single child is padding"),
            ));
        }
    }
    // the root itself
    let n = children.get(root).copied().unwrap_or(0);
    let root_violation = |rule: u8, message: String| Violation {
        rule,
        fellow: Fellow::Foreman,
        goal: root.clone(),
        message,
    };
    if n > MAX_FAN_OUT {
        out.push(root_violation(
            2,
            format!("fan-out {n} exceeds the maximum of {MAX_FAN_OUT}"),
        ));
    }
    if n < 2 && !sorted.is_empty() {
        out.push(root_violation(
            3,
            format!("root has {n} child(ren); a single child is padding"),
        ));
    }
}

fn needs_rules(
    sorted: &[&Entry],
    idx: &BTreeMap<&GoalId, &Entry>,
    mode: Mode,
    known: Option<&BTreeSet<GoalId>>,
    out: &mut Vec<Violation>,
) {
    if mode == Mode::Merged {
        for e in sorted {
            for n in &e.goal.needs {
                let exists = idx.contains_key(n) || known.is_some_and(|k| k.contains(n));
                if !exists {
                    out.push(viol(1, e, format!("needs {n}, which is not a goal")));
                }
            }
        }
    }
    // iterative three-colour DFS over known nodes
    let mut color: BTreeMap<&GoalId, u8> = BTreeMap::new(); // 1 = on stack, 2 = done
    for start in sorted {
        if color.contains_key(&start.goal.id) {
            continue;
        }
        let mut stack: Vec<(&GoalId, usize)> = vec![(&start.goal.id, 0)];
        color.insert(&start.goal.id, 1);
        while let Some((id, i)) = stack.pop() {
            let Some(e) = idx.get(id) else { continue };
            if i < e.goal.needs.len() {
                stack.push((id, i + 1));
                let next = &e.goal.needs[i];
                if !idx.contains_key(next) {
                    continue;
                }
                match color.get(next) {
                    Some(1) => out.push(viol(1, e, format!("needs forms a cycle through {next}"))),
                    Some(_) => {}
                    None => {
                        color.insert(next, 1);
                        stack.push((next, 0));
                    }
                }
            } else {
                color.insert(id, 2);
            }
        }
    }
}

/// Transitive `needs` of every goal (cycle safe).
fn needs_closure<'a>(
    sorted: &[&'a Entry],
    idx: &BTreeMap<&'a GoalId, &'a Entry>,
) -> BTreeMap<GoalId, BTreeSet<GoalId>> {
    let mut all = BTreeMap::new();
    for e in sorted {
        let mut seen: BTreeSet<GoalId> = BTreeSet::new();
        let mut stack: Vec<&GoalId> = e.goal.needs.iter().collect();
        while let Some(n) = stack.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            if let Some(d) = idx.get(n) {
                stack.extend(d.goal.needs.iter());
            }
        }
        all.insert(e.goal.id.clone(), seen);
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Est, GoalId, TestSpec, Tier};
    use tempfile::TempDir;

    fn project() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/a.rs"), "a").expect("write");
        std::fs::write(dir.path().join("src/b.rs"), "b").expect("write");
        dir
    }

    fn cfg(dir: &TempDir) -> HouseConfig {
        HouseConfig::at(dir.path())
    }

    fn leaf(id: &str, parent: &str) -> Goal {
        Goal {
            id: GoalId::new(id),
            parent: Some(GoalId::new(parent)),
            text: format!("do {id}"),
            leaf: true,
            tier: Some(Tier::Sonnet),
            class: Some("code".into()),
            test: Some(TestSpec {
                name: format!("t_{id}"),
                cmd: format!("cargo test -p x t_{id}"),
                class: "test_unit".into(),
            }),
            est: Some(Est {
                ram_mb: 800,
                secs: 60.0,
                tokens: 10_000,
            }),
            ..Goal::default()
        }
    }

    fn branch(id: &str, parent: &str) -> Goal {
        Goal {
            id: GoalId::new(id),
            parent: Some(GoalId::new(parent)),
            text: format!("part {id}"),
            leaf: false,
            ..Goal::default()
        }
    }

    fn foreman(goals: Vec<Goal>) -> Chart {
        Chart {
            need: "G0".into(),
            fellow: Fellow::Foreman,
            goals,
        }
    }

    fn base() -> Vec<Goal> {
        vec![
            branch("G1", "G0"),
            leaf("G1.1", "G1"),
            leaf("G1.2", "G1"),
            branch("G2", "G0"),
            leaf("G2.1", "G2"),
            leaf("G2.2", "G2"),
        ]
    }

    fn rules_of(chart: &Chart, dir: &TempDir) -> Vec<u8> {
        let mut r: Vec<u8> = validate_chart(chart, &cfg(dir))
            .iter()
            .map(|v| v.rule)
            .collect();
        r.sort_unstable();
        r.dedup();
        r
    }

    fn find<'a>(goals: &'a mut [Goal], id: &str) -> &'a mut Goal {
        goals
            .iter_mut()
            .find(|g| g.id.as_str() == id)
            .expect("goal present")
    }

    #[test]
    fn base_chart_is_valid() {
        let dir = project();
        assert_eq!(rules_of(&foreman(base()), &dir), Vec::<u8>::new());
    }

    #[test]
    fn rule1_cycle_in_parent_tree() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1").parent = Some(GoalId::new("G2"));
        find(&mut g, "G2").parent = Some(GoalId::new("G1"));
        assert!(rules_of(&foreman(g), &dir).contains(&1));
        assert!(!rules_of(&foreman(base()), &dir).contains(&1));
    }

    #[test]
    fn rule1_unknown_parent_and_missing_parent() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").parent = Some(GoalId::new("G9"));
        assert!(rules_of(&foreman(g), &dir).contains(&1));
        let mut g = base();
        find(&mut g, "G1.1").parent = None;
        assert!(rules_of(&foreman(g), &dir).contains(&1));
    }

    #[test]
    fn rule1_cycle_in_needs() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").needs = vec![GoalId::new("G1.2")];
        assert!(!rules_of(&foreman(g.clone()), &dir).contains(&1));
        find(&mut g, "G1.2").needs = vec![GoalId::new("G1.1")];
        assert!(rules_of(&foreman(g), &dir).contains(&1));
    }

    #[test]
    fn rule1_self_need_is_a_cycle() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").needs = vec![GoalId::new("G1.1")];
        assert!(rules_of(&foreman(g), &dir).contains(&1));
    }

    fn deep(levels: usize) -> Vec<Goal> {
        // non-leaf chain of `levels` goals (depth 1..=levels), leaves one level below
        let mut goals = Vec::new();
        let mut parent = "G0".to_string();
        let mut id = String::new();
        for l in 1..=levels {
            id = if l == 1 {
                "G1".to_string()
            } else {
                format!("{id}.1")
            };
            goals.push(branch(&id, &parent));
            if l < levels {
                // sibling leaf so the chain node has two children
                goals.push(leaf(&format!("{parent}.x{l}"), &parent));
            }
            parent = id.clone();
        }
        // G0 needs two children too
        goals.push(leaf("G0x", "G0"));
        goals.push(leaf(&format!("{id}.a"), &id));
        goals.push(leaf(&format!("{id}.b"), &id));
        goals
    }

    #[test]
    fn rule2_depth_seven_rejected_depth_six_accepted() {
        let dir = project();
        assert!(
            rules_of(&foreman(deep(6)), &dir).contains(&2),
            "leaves at depth 7"
        );
        assert!(
            !rules_of(&foreman(deep(5)), &dir).contains(&2),
            "leaves at depth 6"
        );
    }

    #[test]
    fn rule2_fan_out_eight_rejected_seven_accepted() {
        let dir = project();
        let mk = |n: usize| {
            let mut g = vec![branch("G1", "G0")];
            for i in 1..=n {
                g.push(leaf(&format!("G1.{i}"), "G1"));
            }
            g.push(branch("G2", "G0"));
            g.push(leaf("G2.1", "G2"));
            g.push(leaf("G2.2", "G2"));
            foreman(g)
        };
        assert!(rules_of(&mk(8), &dir).contains(&2));
        assert!(!rules_of(&mk(7), &dir).contains(&2));
    }

    #[test]
    fn rule3_single_child_non_leaf() {
        let dir = project();
        let mut g = base();
        g.retain(|x| x.id.as_str() != "G1.2");
        assert!(rules_of(&foreman(g), &dir).contains(&3));
        assert!(!rules_of(&foreman(base()), &dir).contains(&3));
    }

    #[test]
    fn rule3_non_leaf_without_children_and_leaf_with_children() {
        let dir = project();
        let mut g = base();
        g.push(branch("G3", "G0"));
        assert!(rules_of(&foreman(g), &dir).contains(&3));
        let mut g = base();
        g.push(leaf("G1.1.1", "G1.1"));
        assert!(rules_of(&foreman(g), &dir).contains(&3));
    }

    #[test]
    fn rule4_leaf_without_test_tier_or_estimate() {
        let dir = project();
        for which in ["test", "cmd", "tier", "est", "class"] {
            let mut g = base();
            let t = find(&mut g, "G1.1");
            match which {
                "test" => t.test = None,
                "cmd" => t.test.as_mut().expect("test").cmd = "  ".into(),
                "tier" => t.tier = None,
                "est" => t.est = None,
                _ => t.test.as_mut().expect("test").class = "wat".into(),
            }
            assert!(rules_of(&foreman(g), &dir).contains(&4), "{which}");
        }
        assert!(!rules_of(&foreman(base()), &dir).contains(&4));
    }

    #[test]
    fn rule5_haiku_leaf_with_writes_or_wrong_class() {
        let dir = project();
        let haiku = |g: &mut Goal| {
            g.tier = Some(Tier::Haiku);
            g.class = Some("grep".into());
            g.est = Some(Est {
                ram_mb: 50,
                secs: 5.0,
                tokens: 2000,
            });
        };
        let mut g = base();
        haiku(find(&mut g, "G1.1"));
        assert!(!rules_of(&foreman(g.clone()), &dir).contains(&5));
        find(&mut g, "G1.1").writes = vec!["src/a.rs".into()];
        assert!(rules_of(&foreman(g.clone()), &dir).contains(&5));
        find(&mut g, "G1.1").writes.clear();
        find(&mut g, "G1.1").class = Some("code".into());
        assert!(rules_of(&foreman(g.clone()), &dir).contains(&5));
        find(&mut g, "G1.1").class = None;
        assert!(rules_of(&foreman(g), &dir).contains(&5));
    }

    #[test]
    fn rule6_token_caps() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").est.as_mut().expect("est").tokens = 40_000;
        assert!(!rules_of(&foreman(g.clone()), &dir).contains(&6));
        find(&mut g, "G1.1").est.as_mut().expect("est").tokens = 40_001;
        assert!(rules_of(&foreman(g), &dir).contains(&6));

        let mut g = base();
        let t = find(&mut g, "G1.1");
        t.tier = Some(Tier::Haiku);
        t.class = Some("read".into());
        t.est = Some(Est {
            ram_mb: 50,
            secs: 5.0,
            tokens: 8_000,
        });
        assert!(!rules_of(&foreman(g.clone()), &dir).contains(&6));
        find(&mut g, "G1.1").est.as_mut().expect("est").tokens = 8_001;
        assert!(rules_of(&foreman(g), &dir).contains(&6));
    }

    #[test]
    fn rule7_two_writers_need_ordering() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").writes = vec!["src/a.rs".into()];
        find(&mut g, "G2.1").writes = vec!["src/a.rs".into()];
        assert!(rules_of(&foreman(g.clone()), &dir).contains(&7));
        find(&mut g, "G2.1").needs = vec![GoalId::new("G1.1")];
        assert!(!rules_of(&foreman(g.clone()), &dir).contains(&7));
        // transitive ordering also counts
        find(&mut g, "G2.1").needs = vec![GoalId::new("G1.2")];
        find(&mut g, "G1.2").needs = vec![GoalId::new("G1.1")];
        assert!(!rules_of(&foreman(g), &dir).contains(&7));
    }

    #[test]
    fn rule7_creates_counts_as_writing() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").creates = vec!["src/new.rs".into()];
        find(&mut g, "G2.1").creates = vec!["src/new.rs".into()];
        assert!(rules_of(&foreman(g), &dir).contains(&7));
    }

    #[test]
    fn rule8_duplicate_test_names() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G2.1").test.as_mut().expect("test").name = "t_G1.1".into();
        assert!(rules_of(&foreman(g), &dir).contains(&8));
        assert!(!rules_of(&foreman(base()), &dir).contains(&8));
    }

    #[test]
    fn rule8_whole_workspace_test_command() {
        let dir = project();
        for bad in [
            "cargo test",
            "cargo test --workspace",
            "cargo test -p x --workspace",
            "cargo test --all",
            "cd x && cargo test",
        ] {
            let mut g = base();
            find(&mut g, "G1.1").test.as_mut().expect("test").cmd = bad.into();
            assert!(rules_of(&foreman(g), &dir).contains(&8), "{bad}");
        }
        for good in [
            "cargo test -p x foo",
            "cargo test --package x foo",
            "pytest -k foo",
        ] {
            let mut g = base();
            find(&mut g, "G1.1").test.as_mut().expect("test").cmd = good.into();
            assert!(!rules_of(&foreman(g), &dir).contains(&8), "{good}");
        }
    }

    #[test]
    fn rule9_paths_must_exist_or_be_created() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").reads = vec!["src/missing.rs".into()];
        assert!(rules_of(&foreman(g.clone()), &dir).contains(&9));
        find(&mut g, "G1.1").reads = vec!["src/a.rs".into()];
        assert!(!rules_of(&foreman(g.clone()), &dir).contains(&9));
        find(&mut g, "G1.1").writes = vec!["src/fresh.rs".into()];
        assert!(rules_of(&foreman(g.clone()), &dir).contains(&9));
        find(&mut g, "G1.1").creates = vec!["src/fresh.rs".into()];
        assert!(!rules_of(&foreman(g), &dir).contains(&9));
    }

    #[test]
    fn rule9_reading_a_file_created_by_a_dependency_is_fine() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").creates = vec!["src/fresh.rs".into()];
        find(&mut g, "G1.1").writes = vec!["src/fresh.rs".into()];
        find(&mut g, "G1.2").reads = vec!["src/fresh.rs".into()];
        assert!(rules_of(&foreman(g.clone()), &dir).contains(&9));
        find(&mut g, "G1.2").needs = vec![GoalId::new("G1.1")];
        assert!(!rules_of(&foreman(g), &dir).contains(&9));
    }

    #[test]
    fn rule10_chase_needs_guards_and_cameron_needs_alt_of() {
        let dir = project();
        let chase = |guards: Option<&str>| Chart {
            need: "G0".into(),
            fellow: Fellow::Chase,
            goals: vec![Goal {
                guards: guards.map(GoalId::new),
                parent: None,
                ..leaf("C1", "G0")
            }],
        };
        assert!(rules_of(&chase(None), &dir).contains(&10));
        assert!(!rules_of(&chase(Some("G1.1")), &dir).contains(&10));
        let cameron = |alt: Option<&str>| Chart {
            need: "G0".into(),
            fellow: Fellow::Cameron,
            goals: vec![Goal {
                alt_of: alt.map(GoalId::new),
                parent: None,
                ..leaf("A1", "G0")
            }],
        };
        assert!(rules_of(&cameron(None), &dir).contains(&10));
        assert!(!rules_of(&cameron(Some("G1.1")), &dir).contains(&10));
    }

    #[test]
    fn chase_and_cameron_leaves_obey_leaf_rules() {
        let dir = project();
        let mut bad = leaf("C1", "G0");
        bad.parent = None;
        bad.guards = Some(GoalId::new("G1.1"));
        bad.est = None;
        let chart = Chart {
            need: "G0".into(),
            fellow: Fellow::Chase,
            goals: vec![bad],
        };
        assert!(rules_of(&chart, &dir).contains(&4));
    }

    #[test]
    fn violations_name_fellow_and_goal() {
        let dir = project();
        let mut g = base();
        find(&mut g, "G1.1").test = None;
        let v = validate_chart(&foreman(g), &cfg(&dir));
        let first = v.iter().find(|v| v.rule == 4).expect("rule 4");
        assert_eq!(first.fellow, Fellow::Foreman);
        assert_eq!(first.goal.as_str(), "G1.1");
        assert!(!first.message.is_empty());
    }
}
