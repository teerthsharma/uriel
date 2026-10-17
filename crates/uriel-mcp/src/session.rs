use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use uriel_core::MemoryBudget;

pub const ENTRY_BYTES: usize = 160;

#[derive(Debug, Clone)]
struct Delivered {
    fingerprint: String,
    detail: u8,
    tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    New,
    Same,
    Changed,
}

#[derive(Debug, Default)]
struct Delivery {
    map: HashMap<u64, Delivered>,
    clock: u64,
}

#[derive(Debug)]
pub struct Session {
    delivered: Mutex<Delivery>,
    cap: usize,
}

impl Session {
    pub fn with_cap(cap: usize) -> Session {
        Session {
            delivered: Mutex::new(Delivery::default()),
            cap: cap.max(1),
        }
    }

    pub fn for_budget(budget: MemoryBudget) -> Session {
        Session::with_cap(budget.session() / ENTRY_BYTES)
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    fn state(&self) -> MutexGuard<'_, Delivery> {
        self.delivered.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn check(&self, id: u64, fingerprint: &str, detail: u8) -> Seen {
        match self.state().map.get(&id) {
            None => Seen::New,
            Some(d) if d.fingerprint != fingerprint => Seen::Changed,
            Some(d) if d.detail >= detail => Seen::Same,
            Some(_) => Seen::New,
        }
    }

    pub fn record(&self, id: u64, fingerprint: &str, detail: u8) {
        let mut st = self.state();
        st.clock += 1;
        let tick = st.clock;
        match st.map.get_mut(&id) {
            Some(d) if d.fingerprint == fingerprint => {
                d.detail = d.detail.max(detail);
                d.tick = tick;
            }
            _ => {
                st.map.insert(
                    id,
                    Delivered {
                        fingerprint: fingerprint.to_string(),
                        detail,
                        tick,
                    },
                );
            }
        }
        while st.map.len() > self.cap {
            let Some(oldest) = st.map.iter().min_by_key(|(_, d)| d.tick).map(|(k, _)| *k) else {
                break;
            };
            st.map.remove(&oldest);
        }
    }

    pub fn reset(&self) -> usize {
        let mut st = self.state();
        let n = st.map.len();
        st.map.clear();
        n
    }

    pub fn len(&self) -> usize {
        self.state().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.state().map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_past_the_cap_evicts_the_least_recently_recorded_id() {
        let s = Session::with_cap(3);
        for id in 1..=3 {
            s.record(id, "h", 1);
        }
        s.record(4, "h", 1);
        assert_eq!(s.len(), 3);
        assert_eq!(s.check(1, "h", 1), Seen::New, "id 1 was the oldest and was evicted");
        for id in 2..=4 {
            assert_eq!(s.check(id, "h", 1), Seen::Same, "id {id} survives");
        }
    }

    #[test]
    fn recording_an_id_again_makes_it_the_newest() {
        let s = Session::with_cap(3);
        for id in 1..=3 {
            s.record(id, "h", 1);
        }
        s.record(1, "h", 2);
        s.record(4, "h", 1);
        assert_eq!(s.check(1, "h", 2), Seen::Same, "refreshed id 1 survives");
        assert_eq!(s.check(2, "h", 1), Seen::New, "id 2 became the oldest and was evicted");
    }

    #[test]
    fn the_map_never_exceeds_its_cap_over_ten_thousand_records() {
        let s = Session::with_cap(64);
        for id in 0..10_000u64 {
            s.record(id, "h", 1);
            assert!(s.len() <= 64);
        }
        assert_eq!(s.len(), 64);
        assert_eq!(s.check(9_999, "h", 1), Seen::Same);
        assert_eq!(s.check(9_935, "h", 1), Seen::New);
        assert_eq!(s.check(9_936, "h", 1), Seen::Same);
    }

    #[test]
    fn a_cap_of_zero_still_holds_one_entry() {
        let s = Session::with_cap(0);
        s.record(1, "h", 1);
        s.record(2, "h", 1);
        assert_eq!(s.len(), 1);
        assert_eq!(s.check(2, "h", 1), Seen::Same);
    }

    #[test]
    fn the_cap_comes_from_the_budget_share() {
        let small = Session::for_budget(uriel_core::MemoryBudget::mib(16));
        let large = Session::for_budget(uriel_core::MemoryBudget::mib(256));
        assert!(small.cap() >= 1 && small.cap() < large.cap());
        assert_eq!(
            Session::for_budget(uriel_core::MemoryBudget::mib(50)).cap(),
            uriel_core::MemoryBudget::mib(50).session() / ENTRY_BYTES
        );
    }

    #[test]
    fn new_same_changed_and_upgrade() {
        let s = Session::with_cap(16);
        assert_eq!(s.check(1, "h", 1), Seen::New);
        s.record(1, "h", 1);
        assert_eq!(s.check(1, "h", 1), Seen::Same);
        assert_eq!(s.check(1, "h", 0), Seen::Same, "a gist is covered by L2");
        assert_eq!(s.check(1, "h", 2), Seen::New, "a zoom is new information");
        assert_eq!(s.check(1, "other", 1), Seen::Changed);
        s.record(1, "h", 3);
        assert_eq!(s.check(1, "h", 2), Seen::Same);
        s.record(1, "other", 0);
        assert_eq!(s.check(1, "other", 0), Seen::Same);
        assert_eq!(s.check(1, "h", 0), Seen::Changed);
        assert_eq!(s.len(), 1);
        assert_eq!(s.reset(), 1);
        assert!(s.is_empty());
        assert_eq!(s.check(1, "h", 1), Seen::New);
    }
}
