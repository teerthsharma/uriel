use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

#[derive(Debug, Clone)]
struct Delivered {
    fingerprint: String,
    detail: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    New,
    Same,
    Changed,
}

#[derive(Debug, Default)]
pub struct Session {
    delivered: Mutex<HashMap<u64, Delivered>>,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    fn map(&self) -> MutexGuard<'_, HashMap<u64, Delivered>> {
        self.delivered.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn check(&self, id: u64, fingerprint: &str, detail: u8) -> Seen {
        match self.map().get(&id) {
            None => Seen::New,
            Some(d) if d.fingerprint != fingerprint => Seen::Changed,
            Some(d) if d.detail >= detail => Seen::Same,
            Some(_) => Seen::New,
        }
    }

    pub fn record(&self, id: u64, fingerprint: &str, detail: u8) {
        let mut map = self.map();
        match map.get_mut(&id) {
            Some(d) if d.fingerprint == fingerprint => d.detail = d.detail.max(detail),
            _ => {
                map.insert(
                    id,
                    Delivered {
                        fingerprint: fingerprint.to_string(),
                        detail,
                    },
                );
            }
        }
    }

    pub fn reset(&self) -> usize {
        let mut map = self.map();
        let n = map.len();
        map.clear();
        n
    }

    pub fn len(&self) -> usize {
        self.map().len()
    }

    pub fn is_empty(&self) -> bool {
        self.map().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_same_changed_and_upgrade() {
        let s = Session::new();
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
