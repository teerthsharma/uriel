#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Continue,
    Converged,
    CapReached,
}

#[derive(Debug, Clone)]
pub struct Stable<T: Eq> {
    previous: Option<T>,
    streak: usize,
    seen: usize,
    need: usize,
    cap: usize,
}

impl<T: Eq> Stable<T> {
    pub fn new(need: usize, cap: usize) -> Self {
        let _ = (need, cap);
        unimplemented!()
    }

    pub fn observe(&mut self, value: T) -> Stop {
        let _ = value;
        unimplemented!()
    }
}
