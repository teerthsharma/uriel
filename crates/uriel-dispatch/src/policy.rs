use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcceptMode {
    Off,
    Observe,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cell {
    Explicit,
    Inferred,
    Ignore,
}

impl Cell {
    fn strength(self) -> Option<Strength> {
        match self {
            Cell::Explicit => Some(Strength::Explicit),
            Cell::Inferred => Some(Strength::Inferred),
            Cell::Ignore => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admission {
    pub listed_unaddressed: Cell,
    pub unlisted_addressed: Cell,
    pub unlisted_unaddressed: Cell,
}

impl Default for Admission {
    fn default() -> Admission {
        Admission {
            listed_unaddressed: Cell::Inferred,
            unlisted_addressed: Cell::Ignore,
            unlisted_unaddressed: Cell::Ignore,
        }
    }
}

impl Admission {
    fn cell(&self, at: CellRef) -> Cell {
        match at {
            CellRef::ListedUnaddressed => self.listed_unaddressed,
            CellRef::UnlistedAddressed => self.unlisted_addressed,
            CellRef::UnlistedUnaddressed => self.unlisted_unaddressed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScopePolicy {
    pub receive_all: bool,
    pub admission: Admission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePolicy {
    pub listening: bool,
    pub allowed: BTreeSet<String>,
    pub bots: BTreeSet<String>,
    pub scopes: BTreeMap<String, ScopePolicy>,
}

impl Default for SourcePolicy {
    fn default() -> SourcePolicy {
        SourcePolicy {
            listening: true,
            allowed: BTreeSet::new(),
            bots: BTreeSet::new(),
            scopes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub enabled: bool,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub mode: AcceptMode,
    pub rules: BTreeMap<String, Rule>,
    pub sources: BTreeMap<String, SourcePolicy>,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            mode: AcceptMode::Observe,
            rules: BTreeMap::new(),
            sources: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admitted {
    pub strength: Strength,
    pub basis: Basis,
}

impl Policy {
    pub fn rule_enabled(&self, name: &str) -> bool {
        self.rules.get(name).is_some_and(|r| r.enabled)
    }
}

pub fn gate(policy: &Policy, t: &Trigger, has_ticket: bool) -> Result<Admitted, Refusal> {
    let src = policy
        .sources
        .get(&t.source)
        .ok_or(Refusal::UnknownSource)?;
    if !src.listening {
        return Err(Refusal::SourceOff);
    }
    let scope = src.scopes.get(&t.scope).ok_or(Refusal::UnknownScope)?;
    let table = |at: CellRef| -> Result<Strength, Refusal> {
        scope.admission.cell(at).strength().ok_or(Refusal::Ignored)
    };
    let unaddressed_table = |at: CellRef| -> Result<Strength, Refusal> {
        if scope.receive_all {
            table(at)
        } else {
            Err(Refusal::NotReceivedAll)
        }
    };
    let admitted = match &t.actor {
        None => Admitted {
            strength: unaddressed_table(CellRef::UnlistedUnaddressed)?,
            basis: Basis::Table(CellRef::UnlistedUnaddressed),
        },
        Some(a) if a.bot => {
            if !src.bots.contains(&a.id) {
                return Err(Refusal::BotNotListed);
            }
            if !t.addressed && !scope.receive_all {
                return Err(Refusal::BotNotAddressed);
            }
            Admitted {
                strength: Strength::Inferred,
                basis: Basis::Bot,
            }
        }
        Some(a) if src.allowed.contains(&a.id) => {
            if t.addressed {
                Admitted {
                    strength: Strength::Explicit,
                    basis: Basis::Listed,
                }
            } else if has_ticket {
                Admitted {
                    strength: Strength::Explicit,
                    basis: Basis::Followup,
                }
            } else {
                Admitted {
                    strength: unaddressed_table(CellRef::ListedUnaddressed)?,
                    basis: Basis::Table(CellRef::ListedUnaddressed),
                }
            }
        }
        Some(_) => {
            let at = if t.addressed {
                CellRef::UnlistedAddressed
            } else {
                CellRef::UnlistedUnaddressed
            };
            let by_table = if t.addressed {
                table(at)
            } else {
                unaddressed_table(at)
            };
            match by_table {
                Ok(Strength::Explicit) => Admitted {
                    strength: Strength::Explicit,
                    basis: Basis::Table(at),
                },
                _ if has_ticket => Admitted {
                    strength: Strength::Inferred,
                    basis: Basis::Inferred,
                },
                Ok(s) => Admitted {
                    strength: s,
                    basis: Basis::Table(at),
                },
                Err(e) => return Err(e),
            }
        }
    };
    if admitted.strength == Strength::Inferred && policy.mode == AcceptMode::Off {
        return Err(Refusal::ModeOff);
    }
    Ok(admitted)
}

pub fn revalidate(policy: &Policy, m: &Mandate) -> Result<(), Revoke> {
    let src = policy.sources.get(&m.source).ok_or(Revoke::SourceGone)?;
    if !src.listening || !src.scopes.contains_key(&m.receiving_scope) {
        return Err(Revoke::SourceGone);
    }
    let listed = m.sender.as_ref().is_some_and(|s| src.allowed.contains(s));
    let inferred_ok = || -> Result<(), Revoke> {
        if policy.mode != AcceptMode::Auto {
            return Err(Revoke::ProactiveOff);
        }
        if m.rules.is_empty() || m.rules.iter().any(|r| !policy.rule_enabled(r)) {
            return Err(Revoke::RuleDisabled);
        }
        Ok(())
    };
    match m.basis {
        Basis::Followup => Ok(()),
        Basis::Listed => {
            if listed {
                Ok(())
            } else {
                Err(Revoke::SenderRemoved)
            }
        }
        Basis::Bot => {
            let bot = m.sender.as_ref().is_some_and(|s| src.bots.contains(s));
            if !bot {
                return Err(Revoke::BotRemoved);
            }
            inferred_ok()
        }
        Basis::Table(at) => {
            if at == CellRef::ListedUnaddressed && !listed {
                return Err(Revoke::SenderRemoved);
            }
            let scope = &src.scopes[&m.receiving_scope];
            let now = scope
                .admission
                .cell(at)
                .strength()
                .ok_or(Revoke::CellWeakened)?;
            if now < m.strength {
                return Err(Revoke::CellWeakened);
            }
            if m.strength == Strength::Inferred {
                inferred_ok()?;
            }
            Ok(())
        }
        Basis::Inferred => inferred_ok(),
    }
}

pub fn changes_for(old: &Policy, new: &Policy, m: &Mandate) -> Vec<Change> {
    let mut out = Vec::new();
    for name in &m.rules {
        if old.rules.get(name) != new.rules.get(name) {
            out.push(Change::Rule(name.clone()));
        }
    }
    if let Err(r) = revalidate(new, m) {
        out.push(Change::Revoked(r));
    }
    out
}
