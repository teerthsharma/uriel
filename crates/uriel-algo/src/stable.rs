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
        Self {
            previous: None,
            streak: 0,
            seen: 0,
            need: need.max(1),
            cap: cap.max(1),
        }
    }

    pub fn observe(&mut self, value: T) -> Stop {
        self.streak = if self.previous.as_ref() == Some(&value) {
            self.streak.saturating_add(1)
        } else {
            1
        };
        self.seen = self.seen.saturating_add(1);
        self.previous = Some(value);
        if self.streak >= self.need {
            Stop::Converged
        } else if self.seen >= self.cap {
            Stop::CapReached
        } else {
            Stop::Continue
        }
    }
}
