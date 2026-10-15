
use crate::sketch::Sketch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyClass {
    Node,
    Adjacency,
    Index,
}

impl KeyClass {
    pub const ALL: [KeyClass; 3] = [KeyClass::Node, KeyClass::Adjacency, KeyClass::Index];

    fn slot(self) -> usize {
        match self {
            KeyClass::Node => 0,
            KeyClass::Adjacency => 1,
            KeyClass::Index => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shed {
    Nothing,
    Warm,
    Hot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    total: usize,
}

impl Budget {
    pub fn new(total: usize) -> Budget {
        Budget { total }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn headroom(&self, hot: usize, warm: usize) -> usize {
        self.total.saturating_sub(hot.saturating_add(warm))
    }

    pub fn shed(&self, hot: usize, warm: usize) -> Shed {
        if hot.saturating_add(warm) <= self.total {
            Shed::Nothing
        } else if warm > 0 {
            Shed::Warm
        } else {
            Shed::Hot
        }
    }
}

#[derive(Debug, Clone)]
pub struct TierController {
    heat: [u64; 3],
    sketch: Sketch,
    warm_threshold: u8,
    age_every: u64,
    ops: u64,
}

impl TierController {
    pub fn new(capacity: usize, warm_threshold: u8, age_every: u64) -> TierController {
        TierController {
            heat: [0; 3],
            sketch: Sketch::new(capacity),
            warm_threshold: warm_threshold.max(1),
            age_every: age_every.max(1),
            ops: 0,
        }
    }

    pub fn on_access(&mut self, class: KeyClass, key_hash: u64) {
        self.sketch.increment(key_hash ^ class.slot() as u64);
        self.heat[class.slot()] = self.heat[class.slot()].saturating_add(1);
        self.ops += 1;
        if self.ops.is_multiple_of(self.age_every) {
            for h in &mut self.heat {
                *h /= 2;
            }
        }
    }

    pub fn should_promote_to_warm(&self, class: KeyClass, key_hash: u64) -> bool {
        self.sketch.estimate(key_hash ^ class.slot() as u64) >= self.warm_threshold
    }

    pub fn heat(&self, class: KeyClass) -> u64 {
        self.heat[class.slot()]
    }

    pub fn hottest(&self) -> Option<KeyClass> {
        KeyClass::ALL
            .into_iter()
            .filter(|c| self.heat(*c) > 0)
            .max_by_key(|c| self.heat(*c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_says_who_sheds() {
        let b = Budget::new(100);
        assert_eq!(b.shed(60, 40), Shed::Nothing);
        assert_eq!(b.shed(60, 41), Shed::Warm);
        assert_eq!(b.shed(101, 0), Shed::Hot);
        assert_eq!(b.headroom(60, 30), 10);
        assert_eq!(b.headroom(90, 30), 0);
    }

    #[test]
    fn heat_counts_per_class_and_ages() {
        let mut c = TierController::new(64, 2, 10);
        for i in 0..9 {
            c.on_access(KeyClass::Node, i);
        }
        assert_eq!(c.heat(KeyClass::Node), 9);
        assert_eq!(c.heat(KeyClass::Index), 0);
        c.on_access(KeyClass::Adjacency, 1);
        assert_eq!(c.heat(KeyClass::Node), 4);
        assert_eq!(c.heat(KeyClass::Adjacency), 0);
        assert_eq!(c.hottest(), Some(KeyClass::Node));
    }

    #[test]
    fn promotion_needs_repeat_access_and_is_per_class() {
        let mut c = TierController::new(64, 2, 1000);
        c.on_access(KeyClass::Node, 5);
        assert!(!c.should_promote_to_warm(KeyClass::Node, 5));
        c.on_access(KeyClass::Node, 5);
        assert!(c.should_promote_to_warm(KeyClass::Node, 5));
        assert!(!c.should_promote_to_warm(KeyClass::Adjacency, 5));
    }
}
