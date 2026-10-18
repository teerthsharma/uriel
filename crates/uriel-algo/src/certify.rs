use std::ops::{Add, Mul, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    F16,
    F32,
    F64,
}

pub fn unit_roundoff(_p: Precision) -> f64 {
    unimplemented!()
}

pub fn gamma(_n: usize, _p: Precision) -> Result<f64, Refusal> {
    unimplemented!()
}

pub fn eta(_d: usize, _p: Precision) -> f64 {
    unimplemented!()
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
    _corpus: &[[T; D]],
    _query: &[T; D],
    _kernel: Kernel,
) -> Result<Enclosure, Refusal> {
    unimplemented!()
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

pub fn topk_set(_scores: &[f64], _k: usize, _largest: bool) -> Vec<usize> {
    unimplemented!()
}

pub fn certified_topk(
    _scores: &[f64],
    _radii: &[f64],
    _k: usize,
    _largest: bool,
    _ordered: bool,
) -> Result<Vec<usize>, Refusal> {
    unimplemented!()
}

pub fn certified_argmin(_scores: &[f64], _radii: &[f64]) -> Result<usize, Refusal> {
    unimplemented!()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum Trit {
    Below = -1,
    Undetermined = 0,
    Above = 1,
}

pub fn certified_threshold(
    _scores: &[f64],
    _radii: &[f64],
    _threshold: f64,
) -> Result<Vec<Trit>, Refusal> {
    unimplemented!()
}
