use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

pub const MAX_DEPARTMENTS: usize = 12;
pub const MAX_SUBGOALS: usize = 24;
pub const MAX_LEAVES: usize = 48;
pub const MAX_ITEMS: usize = 32;
pub const MAX_TEXT: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeafSpec {
    pub goal: String,
    pub checklist: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubGoalSpec {
    pub goal: String,
    pub leaves: Vec<LeafSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepartmentSpec {
    pub name: String,
    pub goal: String,
    pub subgoals: Vec<SubGoalSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSpec {
    pub departments: Vec<DepartmentSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leaf {
    pub id: String,
    pub goal: String,
    pub checklist: Vec<Item>,
    pub ticket: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubGoal {
    pub id: String,
    pub goal: String,
    pub leaves: Vec<Leaf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Department {
    pub slug: String,
    pub name: String,
    pub goal: String,
    pub subgoals: Vec<SubGoal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Plan {
    pub departments: Vec<Department>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
}

impl Progress {
    pub fn add(self, o: Progress) -> Progress {
        Progress {
            done: self.done + o.done,
            total: self.total + o.total,
        }
    }

    pub fn complete(self) -> bool {
        self.total > 0 && self.done == self.total
    }

    pub fn percent(self) -> u32 {
        if self.total == 0 {
            0
        } else {
            u32::try_from(u64::from(self.done) * 100 / u64::from(self.total)).unwrap_or(100)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    NoDepartments,
    TooMany(&'static str),
    Empty(String),
    Long(String),
    Duplicate(String),
    NoLeaves(String),
    NoChecklist(String),
    UnknownLeaf(String),
    BadItem(String, usize),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::NoDepartments => f.write_str("a plan needs at least one department"),
            PlanError::TooMany(what) => write!(f, "too many {what}"),
            PlanError::Empty(w) => write!(f, "{w} is empty"),
            PlanError::Long(w) => write!(f, "{w} is longer than {MAX_TEXT} characters"),
            PlanError::Duplicate(n) => write!(f, "department name '{n}' collides with another after slugging"),
            PlanError::NoLeaves(w) => write!(f, "{w} has no leaves: every sub-goal needs at least one"),
            PlanError::NoChecklist(w) => write!(f, "{w} has an empty checklist: every leaf needs at least one item"),
            PlanError::UnknownLeaf(l) => write!(f, "no leaf '{l}'"),
            PlanError::BadItem(l, i) => write!(f, "leaf '{l}' has no checklist item {i}"),
        }
    }
}

impl std::error::Error for PlanError {}

pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn text(what: &str, s: &str) -> Result<String, PlanError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(PlanError::Empty(what.to_string()));
    }
    if t.chars().count() > MAX_TEXT {
        return Err(PlanError::Long(what.to_string()));
    }
    Ok(t.to_string())
}

impl Plan {
    pub fn build(spec: &PlanSpec) -> Result<Plan, PlanError> {
        if spec.departments.is_empty() {
            return Err(PlanError::NoDepartments);
        }
        if spec.departments.len() > MAX_DEPARTMENTS {
            return Err(PlanError::TooMany("departments"));
        }
        let mut seen = BTreeSet::new();
        let mut departments = Vec::new();
        for d in &spec.departments {
            let name = text("a department name", &d.name)?;
            let slug = slug(&name);
            if slug.is_empty() {
                return Err(PlanError::Empty("a department name".into()));
            }
            if !seen.insert(slug.clone()) {
                return Err(PlanError::Duplicate(name));
            }
            let goal = text(&format!("the goal of {name}"), &d.goal)?;
            if d.subgoals.is_empty() {
                return Err(PlanError::NoLeaves(format!("department '{name}'")));
            }
            if d.subgoals.len() > MAX_SUBGOALS {
                return Err(PlanError::TooMany("sub-goals"));
            }
            let mut subgoals = Vec::new();
            for (si, s) in d.subgoals.iter().enumerate() {
                let sid = format!("{slug}.{}", si + 1);
                let sgoal = text(&format!("sub-goal {sid}"), &s.goal)?;
                if s.leaves.is_empty() {
                    return Err(PlanError::NoLeaves(format!("sub-goal {sid}")));
                }
                if s.leaves.len() > MAX_LEAVES {
                    return Err(PlanError::TooMany("leaves"));
                }
                let mut leaves = Vec::new();
                for (li, l) in s.leaves.iter().enumerate() {
                    let lid = format!("{sid}.{}", li + 1);
                    let lgoal = text(&format!("leaf {lid}"), &l.goal)?;
                    if l.checklist.is_empty() {
                        return Err(PlanError::NoChecklist(format!("leaf {lid}")));
                    }
                    if l.checklist.len() > MAX_ITEMS {
                        return Err(PlanError::TooMany("checklist items"));
                    }
                    let mut checklist = Vec::new();
                    for (ii, it) in l.checklist.iter().enumerate() {
                        checklist.push(Item {
                            text: text(&format!("item {} of leaf {lid}", ii + 1), it)?,
                            done: false,
                        });
                    }
                    leaves.push(Leaf {
                        id: lid,
                        goal: lgoal,
                        checklist,
                        ticket: None,
                    });
                }
                subgoals.push(SubGoal {
                    id: sid,
                    goal: sgoal,
                    leaves,
                });
            }
            departments.push(Department {
                slug,
                name,
                goal,
                subgoals,
            });
        }
        Ok(Plan { departments })
    }

    pub fn leaf(&self, id: &str) -> Option<(&Department, &Leaf)> {
        self.departments.iter().find_map(|d| {
            d.subgoals
                .iter()
                .flat_map(|s| s.leaves.iter())
                .find(|l| l.id == id)
                .map(|l| (d, l))
        })
    }

    pub fn leaf_mut(&mut self, id: &str) -> Option<&mut Leaf> {
        self.departments
            .iter_mut()
            .flat_map(|d| d.subgoals.iter_mut())
            .flat_map(|s| s.leaves.iter_mut())
            .find(|l| l.id == id)
    }

    pub fn check(&mut self, leaf: &str, index: usize, done: bool) -> Result<(), PlanError> {
        let l = self
            .leaf_mut(leaf)
            .ok_or_else(|| PlanError::UnknownLeaf(leaf.to_string()))?;
        let item = l
            .checklist
            .get_mut(index)
            .ok_or_else(|| PlanError::BadItem(leaf.to_string(), index))?;
        item.done = done;
        Ok(())
    }

    pub fn progress(&self) -> Progress {
        self.departments
            .iter()
            .fold(Progress::default(), |a, d| a.add(d.progress()))
    }

    pub fn dept_of_leaf(&self, id: &str) -> Option<&str> {
        self.leaf(id).map(|(d, _)| d.slug.as_str())
    }
}

impl Leaf {
    pub fn progress(&self) -> Progress {
        Progress {
            done: u32::try_from(self.checklist.iter().filter(|i| i.done).count()).unwrap_or(u32::MAX),
            total: u32::try_from(self.checklist.len()).unwrap_or(u32::MAX),
        }
    }
}

impl SubGoal {
    pub fn progress(&self) -> Progress {
        self.leaves.iter().fold(Progress::default(), |a, l| a.add(l.progress()))
    }
}

impl Department {
    pub fn progress(&self) -> Progress {
        self.subgoals.iter().fold(Progress::default(), |a, s| a.add(s.progress()))
    }
}

pub fn distribute(capacity: u32, loads: &BTreeMap<String, u32>) -> BTreeMap<String, u32> {
    distribute_ranked(capacity, loads, &BTreeMap::new())
}

pub fn distribute_ranked(
    capacity: u32,
    loads: &BTreeMap<String, u32>,
    served: &BTreeMap<String, u64>,
) -> BTreeMap<String, u32> {
    let active: Vec<(&String, u32)> = loads.iter().filter(|(_, l)| **l > 0).map(|(k, l)| (k, *l)).collect();
    let mut out: BTreeMap<String, u32> = loads.keys().map(|k| (k.clone(), 0)).collect();
    if active.is_empty() || capacity == 0 {
        return out;
    }
    let n = u32::try_from(active.len()).unwrap_or(u32::MAX);
    if capacity <= n {
        let mut by_load = active.clone();
        let last = |k: &String| served.get(k).copied().unwrap_or(0);
        by_load.sort_by(|a, b| {
            last(a.0)
                .cmp(&last(b.0))
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.0.cmp(b.0))
        });
        for (k, _) in by_load.into_iter().take(capacity as usize) {
            out.insert(k.clone(), 1);
        }
        return out;
    }
    let total: u64 = active.iter().map(|(_, l)| u64::from(*l)).sum();
    let cap = u64::from(capacity);
    let spend = cap.min(total);
    let mut given: BTreeMap<&String, u64> = active.iter().map(|(k, _)| (*k, 1)).collect();
    let mut left = spend - u64::from(n);
    let extra: Vec<(&String, u64, u64)> = active
        .iter()
        .map(|(k, l)| {
            let want = u64::from(*l).saturating_sub(1);
            (*k, want, u64::from(*l))
        })
        .collect();
    let want_total: u64 = extra.iter().map(|(_, w, _)| *w).sum();
    if want_total > 0 && left > 0 {
        let mut rem: Vec<(&String, u64, u64)> = Vec::new();
        for (k, want, l) in &extra {
            let exact = left * l;
            let base = (exact / total).min(*want);
            *given.entry(k).or_insert(1) += base;
            rem.push((*k, exact % total, *want - base));
        }
        let used: u64 = given.values().sum::<u64>() - u64::from(n);
        let mut left2 = left.saturating_sub(used);
        rem.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        while left2 > 0 {
            let mut moved = false;
            for (k, _, room) in &mut rem {
                if left2 == 0 {
                    break;
                }
                if *room > 0 {
                    *given.entry(k).or_insert(1) += 1;
                    *room -= 1;
                    left2 -= 1;
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        left = left2;
    }
    let _ = left;
    for (k, v) in given {
        out.insert(k.clone(), u32::try_from(v).unwrap_or(u32::MAX));
    }
    out
}
