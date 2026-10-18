use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};

const EPS: f64 = 1e-12;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::InvalidParameter(message.into()))
}

pub fn orbit_error_bound(n: usize, m: usize) -> Result<usize> {
    if n < 1 {
        return invalid("need at least one entity");
    }
    if m < 1 || m > n {
        return invalid(format!("n_distinct must lie in [1, {n}]; got {m}"));
    }
    Ok(n - m)
}

pub fn admissible_error_bound(n: usize, m_star: usize) -> Result<usize> {
    if n < 1 {
        return invalid("need at least one entity");
    }
    if m_star > n {
        return invalid(format!(
            "n_admissible_distinct must lie in [0, {n}] for an injective relation; got {m_star}"
        ));
    }
    Ok(n - m_star)
}

pub fn certified_precision_bound(n: usize, m: usize, flagged: usize) -> Result<f64> {
    let errors = orbit_error_bound(n, m)?;
    if flagged == 0 {
        return invalid("precision of an empty certified set is undefined");
    }
    if flagged > n {
        return invalid(format!("n_flagged must be at most {n}; got {flagged}"));
    }
    if flagged < errors {
        return invalid(format!(
            "n_flagged={flagged} cannot hold the {errors} certified errors of this partition"
        ));
    }
    Ok(errors as f64 / flagged as f64)
}

pub fn admissible_precision_bound(flagged: usize, b_adm: usize) -> Result<f64> {
    if flagged == 0 {
        return invalid("precision of an empty certified set is undefined");
    }
    if b_adm > flagged / 2 {
        return invalid(format!(
            "b_admissible={b_adm} needs at least {} flagged entities, but |S|={flagged}",
            b_adm.saturating_mul(2)
        ));
    }
    Ok((flagged - b_adm) as f64 / flagged as f64)
}

pub fn pooling_recovery_bound(block_size: usize) -> Result<f64> {
    if block_size < 1 {
        return invalid("block size must be positive");
    }
    Ok(1.0 / block_size as f64)
}

pub fn recall_floor(n: usize, m_star: usize) -> Result<f64> {
    Ok(admissible_error_bound(n, m_star)? as f64 / n as f64)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub n: usize,
    pub orbits: Vec<Vec<usize>>,
}

impl Partition {
    pub fn m(&self) -> usize {
        self.orbits.len()
    }

    pub fn flagged(&self) -> usize {
        self.orbits
            .iter()
            .filter(|o| o.len() > 1)
            .map(Vec::len)
            .sum()
    }
}

pub fn partition(answers: &[&str]) -> Partition {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    let mut orbits: Vec<Vec<usize>> = Vec::new();
    for (i, answer) in answers.iter().enumerate() {
        match index.get(answer) {
            Some(&o) => orbits[o].push(i),
            None => {
                index.insert(answer, orbits.len());
                orbits.push(vec![i]);
            }
        }
    }
    Partition {
        n: answers.len(),
        orbits,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Claims {
    pub errors: Option<usize>,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub bound: &'static str,
    pub floor: f64,
    pub claimed: Option<f64>,
    pub violated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Audit {
    pub n: usize,
    pub m: usize,
    pub flagged: usize,
    pub m_star: Option<usize>,
    pub checks: Vec<Check>,
    pub inapplicable: Vec<(&'static str, String)>,
    pub collisions: Vec<usize>,
    pub inadmissible: Vec<usize>,
}

const PRIORITY: [&str; 5] = ["T1*", "T1", "T6*", "T6", "T8"];

impl Audit {
    pub fn violated(&self) -> bool {
        self.checks.iter().any(|c| c.violated)
    }

    pub fn bound(&self) -> &'static str {
        PRIORITY
            .iter()
            .copied()
            .find(|b| self.checks.iter().any(|c| c.bound == *b && c.violated))
            .unwrap_or(if self.m_star.is_some() { "T1*" } else { "T1" })
    }
}

fn unit_interval(name: &str, v: Option<f64>) -> Result<()> {
    match v {
        Some(x) if !(x.is_finite() && (0.0..=1.0).contains(&x)) => {
            invalid(format!("{name} must be a number in [0, 1]; got {x}"))
        }
        _ => Ok(()),
    }
}

fn rate_check(bound: &'static str, floor: f64, claimed: Option<f64>) -> Check {
    Check {
        bound,
        floor,
        claimed,
        violated: claimed.is_some_and(|c| c + EPS < floor),
    }
}

pub fn audit_orbits(
    answers: &[&str],
    admissible: Option<&[&str]>,
    claims: &Claims,
) -> Result<Audit> {
    unit_interval("precision", claims.precision)?;
    unit_interval("recall", claims.recall)?;
    let part = partition(answers);
    let n = part.n;
    let m = part.m();
    let flagged = part.flagged();
    let collisions: Vec<usize> = (0..m).filter(|&o| part.orbits[o].len() > 1).collect();

    let t1 = orbit_error_bound(n, m)?;
    let mut checks = vec![Check {
        bound: "T1",
        floor: t1 as f64,
        claimed: claims.errors.map(|e| e as f64),
        violated: claims.errors.is_some_and(|e| e < t1),
    }];
    let mut inapplicable: Vec<(&'static str, String)> = Vec::new();

    let gold: Option<BTreeSet<&str>> = admissible.map(|g| g.iter().copied().collect());
    let mut m_star = None;
    let mut inadmissible = Vec::new();
    let mut b_adm = None;
    match &gold {
        Some(g) if g.len() < n => {
            let why = format!(
                "the admissible set holds {} distinct values for {n} entities, so the relation is not injective at this encoding",
                g.len()
            );
            for bound in ["T1*", "T6*", "T8"] {
                inapplicable.push((bound, why.clone()));
            }
        }
        Some(g) => {
            let ms = (0..m)
                .filter(|&o| g.contains(answers[part.orbits[o][0]]))
                .count();
            m_star = Some(ms);
            b_adm = Some(
                collisions
                    .iter()
                    .filter(|&&o| g.contains(answers[part.orbits[o][0]]))
                    .count(),
            );
            inadmissible = (0..n).filter(|&i| !g.contains(answers[i])).collect();
            let t1s = admissible_error_bound(n, ms)?;
            checks.push(Check {
                bound: "T1*",
                floor: t1s as f64,
                claimed: claims.errors.map(|e| e as f64),
                violated: claims.errors.is_some_and(|e| e < t1s),
            });
        }
        None => {}
    }

    if flagged > 0 {
        checks.push(rate_check(
            "T6",
            certified_precision_bound(n, m, flagged)?,
            claims.precision,
        ));
        if let Some(b) = b_adm {
            checks.push(rate_check(
                "T6*",
                admissible_precision_bound(flagged, b)?,
                claims.precision,
            ));
        }
    } else if claims.precision.is_some() {
        let why = "no two entities share an answer, so the flagged set is empty and its precision is undefined";
        inapplicable.push(("T6", why.to_string()));
        if b_adm.is_some() {
            inapplicable.push(("T6*", why.to_string()));
        }
    }

    match m_star {
        Some(ms) => checks.push(rate_check("T8", recall_floor(n, ms)?, claims.recall)),
        None if claims.recall.is_some() && gold.is_none() => inapplicable.push((
            "T8",
            "needs the admissible set, which was not supplied".to_string(),
        )),
        None => {}
    }

    Ok(Audit {
        n,
        m,
        flagged,
        m_star,
        checks,
        inapplicable,
        collisions,
        inadmissible,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pooling {
    pub entity: usize,
    pub block: usize,
    pub k: usize,
    pub ceiling: f64,
    pub claimed: f64,
    pub violated: bool,
    pub members: Vec<usize>,
}

pub fn audit_pooling(answers: &[&str], claims: &[(usize, f64)]) -> Result<Vec<Pooling>> {
    let part = partition(answers);
    if part.n == 0 {
        return invalid("need at least one entity");
    }
    let mut owner = vec![0usize; part.n];
    for (o, members) in part.orbits.iter().enumerate() {
        for &i in members {
            owner[i] = o;
        }
    }
    claims
        .iter()
        .map(|&(entity, claimed)| {
            if entity >= part.n {
                return invalid(format!("entity index {entity} is out of range"));
            }
            unit_interval("recovery", Some(claimed))?;
            let block = owner[entity];
            let members = part.orbits[block].clone();
            let ceiling = pooling_recovery_bound(members.len())?;
            Ok(Pooling {
                entity,
                block,
                k: members.len(),
                ceiling,
                claimed,
                violated: claimed > ceiling + EPS,
                members,
            })
        })
        .collect()
}
