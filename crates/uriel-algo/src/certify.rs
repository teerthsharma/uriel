use std::ops::{Add, Mul, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    F16,
    F32,
    F64,
}

pub fn unit_roundoff(p: Precision) -> f64 {
    match p {
        Precision::F16 => 1.0 / 2048.0,
        Precision::F32 => f32::EPSILON as f64 / 2.0,
        Precision::F64 => f64::EPSILON / 2.0,
    }
}

fn smallest_subnormal(p: Precision) -> f64 {
    match p {
        Precision::F16 => 1.0 / 16_777_216.0,
        Precision::F32 => f32::from_bits(1) as f64,
        Precision::F64 => f64::from_bits(1),
    }
}

fn largest_finite(p: Precision) -> f64 {
    match p {
        Precision::F16 => 65504.0,
        Precision::F32 => f32::MAX as f64,
        Precision::F64 => f64::MAX,
    }
}

fn up(x: f64) -> f64 {
    x.next_up()
}

fn down(x: f64) -> f64 {
    x.next_down()
}

pub fn gamma(n: usize, p: Precision) -> Result<f64, Refusal> {
    if n == 0 {
        return Err(Refusal::Usage("gamma needs a positive reduction length"));
    }
    let nu = n as f64 * unit_roundoff(p);
    if nu > 0.5 {
        return Err(Refusal::BoundVacuous { n });
    }
    Ok(up(nu / (1.0 - nu)))
}

pub fn eta(d: usize, p: Precision) -> f64 {
    4.0 * (d as f64 + 2.0) * smallest_subnormal(p)
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

pub trait Working:
    sealed::Sealed + Copy + Into<f64> + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self>
{
    const PRECISION: Precision;
    const ZERO: Self;
}

impl Working for f32 {
    const PRECISION: Precision = Precision::F32;
    const ZERO: Self = 0.0;
}

impl Working for f64 {
    const PRECISION: Precision = Precision::F64;
    const ZERO: Self = 0.0;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kernel {
    GramCheap,
    GramTight,
    Direct,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Enclosure {
    pub scores: Vec<f64>,
    pub radii: Vec<f64>,
}

pub fn enclose_scores<T: Working, const D: usize>(
    corpus: &[[T; D]],
    query: &[T; D],
    kernel: Kernel,
) -> Result<Enclosure, Refusal> {
    let finite = |v: &T| (*v).into().is_finite();
    if let Some(row) = corpus.iter().position(|x| !x.iter().all(finite)) {
        return Err(Refusal::NonFiniteInput {
            operand: "corpus",
            index: row,
        });
    }
    if let Some(l) = query.iter().position(|v| !finite(v)) {
        return Err(Refusal::NonFiniteInput {
            operand: "query",
            index: l,
        });
    }

    let p = T::PRECISION;
    let g = gamma(D + 2, p)?;
    if kernel == Kernel::Direct && g >= 1.0 {
        return Err(Refusal::BoundVacuous { n: D + 2 });
    }

    let norm2 = |v: &[T; D]| {
        v.iter().fold(0.0f64, |s, &a| {
            let a: f64 = a.into();
            s + a * a
        })
    };
    let qn2 = norm2(query);
    let qn = qn2.sqrt();
    let xn2: Vec<f64> = corpus.iter().map(norm2).collect();
    let max_xn = xn2.iter().fold(0.0f64, |m, &v| m.max(v)).sqrt();
    let headroom = (qn + max_xn) * (qn + max_xn);
    let limit = largest_finite(p);
    if headroom > limit {
        return Err(Refusal::RangeUnsafe { headroom, limit });
    }

    let push = gamma(D + 2, Precision::F64)? + 8.0 * unit_roundoff(Precision::F64);
    let floor = eta(D, p);
    let qn2_t = query.iter().fold(T::ZERO, |s, &a| s + a * a);

    let mut scores = Vec::with_capacity(corpus.len());
    let mut radii = Vec::with_capacity(corpus.len());
    for (x, &x2) in corpus.iter().zip(&xn2) {
        let (score, radius) = match kernel {
            Kernel::GramCheap | Kernel::GramTight => {
                let xn2_t = x.iter().fold(T::ZERO, |s, &a| s + a * a);
                let dot = x.iter().zip(query).fold(T::ZERO, |s, (&a, &b)| s + a * b);
                let score: f64 = ((qn2_t + xn2_t) - (dot + dot)).into();
                let bound = if kernel == Kernel::GramCheap {
                    let sum = qn + x2.sqrt();
                    sum * sum
                } else {
                    let absdot = x.iter().zip(query).fold(0.0f64, |s, (&a, &b)| {
                        let (a, b): (f64, f64) = (a.into(), b.into());
                        s + a.abs() * b.abs()
                    });
                    qn2 + x2 + 2.0 * absdot
                };
                (score, g * bound)
            }
            Kernel::Direct => {
                let score: f64 = x
                    .iter()
                    .zip(query)
                    .fold(T::ZERO, |s, (&a, &b)| {
                        let diff = b - a;
                        s + diff * diff
                    })
                    .into();
                (score, score.max(0.0) * (g / (1.0 - g)))
            }
        };
        scores.push(score);
        radii.push(up(radius * (1.0 + push) + floor));
    }
    Ok(Enclosure { scores, radii })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frontier {
    pub inside: usize,
    pub outside: usize,
    pub inside_lo: f64,
    pub inside_hi: f64,
    pub outside_lo: f64,
    pub outside_hi: f64,
    pub gap: f64,
    pub width: f64,
}

impl Frontier {
    pub fn deficit(&self) -> f64 {
        self.gap - self.width
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    BoundaryUndetermined {
        frontier: Frontier,
        straddling: Vec<usize>,
    },
    NonFiniteInput {
        operand: &'static str,
        index: usize,
    },
    BoundVacuous {
        n: usize,
    },
    RangeUnsafe {
        headroom: f64,
        limit: f64,
    },
    Usage(&'static str),
}

fn check_enclosure(scores: &[f64], radii: &[f64]) -> Result<(), Refusal> {
    if radii.len() != 1 && radii.len() != scores.len() {
        return Err(Refusal::Usage(
            "radii must hold one radius, or one per score",
        ));
    }
    if let Some(i) = scores.iter().position(|v| !v.is_finite()) {
        return Err(Refusal::NonFiniteInput {
            operand: "scores",
            index: i,
        });
    }
    if let Some(i) = radii.iter().position(|v| !v.is_finite()) {
        return Err(Refusal::NonFiniteInput {
            operand: "radii",
            index: i,
        });
    }
    if radii.iter().any(|&r| r < 0.0) {
        return Err(Refusal::Usage("a radius cannot be negative"));
    }
    Ok(())
}

fn radius(radii: &[f64], i: usize) -> f64 {
    radii[if radii.len() == 1 { 0 } else { i }]
}

pub fn topk_set(scores: &[f64], k: usize, largest: bool) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|&a, &b| {
        let o = scores[a].total_cmp(&scores[b]);
        if largest { o.reverse() } else { o }.then(a.cmp(&b))
    });
    idx.truncate(k);
    idx
}

pub fn certified_topk(
    scores: &[f64],
    radii: &[f64],
    k: usize,
    largest: bool,
    ordered: bool,
) -> Result<Vec<usize>, Refusal> {
    check_enclosure(scores, radii)?;
    let n = scores.len();
    if k == 0 || k >= n {
        return Err(Refusal::Usage("k must satisfy 0 < k < n"));
    }

    let sgn = if largest { -1.0 } else { 1.0 };
    let hi = |i: usize| up(sgn * scores[i] + radius(radii, i));
    let lo = |i: usize| down(sgn * scores[i] - radius(radii, i));
    let frontier = |inside: usize, outside: usize| {
        let (ri, ro) = (radius(radii, inside), radius(radii, outside));
        Frontier {
            inside,
            outside,
            inside_lo: down(scores[inside] - ri),
            inside_hi: up(scores[inside] + ri),
            outside_lo: down(scores[outside] - ro),
            outside_hi: up(scores[outside] + ro),
            gap: sgn * (scores[outside] - scores[inside]),
            width: ri + ro,
        }
    };

    let mut set = topk_set(scores, k, largest);
    let mut member = vec![false; n];
    for &i in &set {
        member[i] = true;
    }
    let inside = set
        .iter()
        .copied()
        .max_by(|&a, &b| hi(a).total_cmp(&hi(b)))
        .expect("k > 0");
    let outside = (0..n)
        .filter(|&j| !member[j])
        .min_by(|&a, &b| lo(a).total_cmp(&lo(b)))
        .expect("k < n");
    let (max_in, min_out) = (hi(inside), lo(outside));
    if max_in >= min_out {
        let straddling = (0..n)
            .filter(|&i| {
                if member[i] {
                    hi(i) >= min_out
                } else {
                    lo(i) <= max_in
                }
            })
            .collect();
        return Err(Refusal::BoundaryUndetermined {
            frontier: frontier(inside, outside),
            straddling,
        });
    }

    if ordered {
        if let Some(w) = set.windows(2).find(|w| hi(w[0]) >= lo(w[1])) {
            return Err(Refusal::BoundaryUndetermined {
                frontier: frontier(w[0], w[1]),
                straddling: vec![w[0].min(w[1]), w[0].max(w[1])],
            });
        }
    } else {
        set.sort_unstable();
    }
    Ok(set)
}

pub fn certified_argmin(scores: &[f64], radii: &[f64]) -> Result<usize, Refusal> {
    certified_topk(scores, radii, 1, false, false).map(|set| set[0])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum Trit {
    Below = -1,
    Undetermined = 0,
    Above = 1,
}

pub fn certified_threshold(
    scores: &[f64],
    radii: &[f64],
    threshold: f64,
) -> Result<Vec<Trit>, Refusal> {
    check_enclosure(scores, radii)?;
    if !threshold.is_finite() {
        return Err(Refusal::NonFiniteInput {
            operand: "threshold",
            index: 0,
        });
    }
    Ok(scores
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            let r = radius(radii, i);
            if down(d - r) > threshold {
                Trit::Above
            } else if up(d + r) < threshold {
                Trit::Below
            } else {
                Trit::Undetermined
            }
        })
        .collect())
}
