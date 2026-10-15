
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp {
    pub wall: u64,
    pub logical: u32,
    pub node: u32,
}

impl Timestamp {
    pub const MIN: Timestamp = Timestamp { wall: 0, logical: 0, node: 0 };
    pub const MAX: Timestamp = Timestamp { wall: u64::MAX, logical: u32::MAX, node: u32::MAX };

    pub fn new(wall: u64, logical: u32, node: u32) -> Self {
        Timestamp { wall, logical, node }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HlcError {
    #[error("remote timestamp {remote_wall} ms exceeds local {physical} ms by more than {max_skew} ms")]
    SkewExceeded {
        remote_wall: u64,
        physical: u64,
        max_skew: u64,
    },
}

#[derive(Debug, Clone)]
pub struct Hlc {
    node: u32,
    max_skew_ms: u64,
    wall: u64,
    logical: u32,
}

impl Hlc {
    pub fn new(node: u32, max_skew_ms: u64) -> Self {
        Hlc { node, max_skew_ms, wall: 0, logical: 0 }
    }

    pub fn last(&self) -> Timestamp {
        Timestamp::new(self.wall, self.logical, self.node)
    }

    pub fn now(&mut self, physical_ms: u64) -> Timestamp {
        if physical_ms > self.wall {
            self.wall = physical_ms;
            self.logical = 0;
        } else {
            self.tick_logical(self.logical);
        }
        self.last()
    }

    fn tick_logical(&mut self, base: u32) {
        match base.checked_add(1) {
            Some(l) => self.logical = l,
            None => {
                self.wall = self.wall.saturating_add(1);
                self.logical = 0;
            }
        }
    }

    pub fn update(&mut self, remote: Timestamp, physical_ms: u64) -> Result<Timestamp, HlcError> {
        if remote.wall > physical_ms.saturating_add(self.max_skew_ms) {
            return Err(HlcError::SkewExceeded {
                remote_wall: remote.wall,
                physical: physical_ms,
                max_skew: self.max_skew_ms,
            });
        }
        let new_wall = self.wall.max(remote.wall).max(physical_ms);
        let base = if new_wall == self.wall && new_wall == remote.wall {
            Some(self.logical.max(remote.logical))
        } else if new_wall == self.wall {
            Some(self.logical)
        } else if new_wall == remote.wall {
            Some(remote.logical)
        } else {
            None
        };
        self.wall = new_wall;
        match base {
            Some(b) => self.tick_logical(b),
            None => self.logical = 0,
        }
        Ok(self.last())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    #[test]
    fn ordering_is_total_wall_logical_node() {
        let a = Timestamp::new(1, 5, 9);
        let b = Timestamp::new(2, 0, 0);
        let c = Timestamp::new(2, 0, 1);
        let d = Timestamp::new(2, 1, 0);
        assert!(a < b && b < c && c < d);
    }

    #[test]
    fn now_advances_with_physical() {
        let mut h = Hlc::new(1, 100);
        assert_eq!(h.now(10), Timestamp::new(10, 0, 1));
        assert_eq!(h.now(20), Timestamp::new(20, 0, 1));
    }

    #[test]
    fn now_is_monotonic_under_stall() {
        let mut h = Hlc::new(1, 100);
        let a = h.now(10);
        let b = h.now(10);
        let c = h.now(10);
        assert!(a < b && b < c);
        assert_eq!(c.wall, 10);
        assert_eq!(c.logical, 2);
    }

    #[test]
    fn now_is_monotonic_under_backward_jump() {
        let mut h = Hlc::new(1, 100);
        let a = h.now(50);
        let b = h.now(10);
        assert!(b > a);
        assert_eq!(b.wall, 50);
    }

    #[test]
    fn logical_overflow_rolls_wall() {
        let mut h = Hlc::new(1, 100);
        h.wall = 5;
        h.logical = u32::MAX;
        let t = h.now(1);
        assert!(t > Timestamp::new(5, u32::MAX, 1));
    }

    #[test]
    fn update_exceeds_remote_and_local() {
        let mut h = Hlc::new(1, 100);
        let local = h.now(10);
        let remote = Timestamp::new(30, 4, 2);
        let t = h.update(remote, 12).unwrap();
        assert!(t > remote && t > local);
        assert_eq!(t, Timestamp::new(30, 5, 1));
    }

    #[test]
    fn update_with_older_remote_keeps_local_order() {
        let mut h = Hlc::new(1, 100);
        let local = h.now(50);
        let t = h.update(Timestamp::new(5, 0, 2), 50).unwrap();
        assert!(t > local);
    }

    #[test]
    fn update_rejects_far_future_remote() {
        let mut h = Hlc::new(1, 100);
        let before = h.last();
        let err = h.update(Timestamp::new(1000, 0, 2), 10).unwrap_err();
        assert!(matches!(err, HlcError::SkewExceeded { .. }));
        assert_eq!(h.last(), before, "rejected update must not move the clock");
        assert!(h.update(Timestamp::new(110, 0, 2), 10).is_ok());
    }

    #[test]
    fn property_loop_monotonic_and_causal() {
        let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            s >> 33
        };
        let mut a = Hlc::new(1, 1_000_000);
        let mut b = Hlc::new(2, 1_000_000);
        let mut phys_a = 1000u64;
        let mut phys_b = 1000u64;
        let (mut last_a, mut last_b) = (Timestamp::MIN, Timestamp::MIN);
        for _ in 0..20_000 {
            phys_a = phys_a.wrapping_add(next() % 7).wrapping_sub(next() % 5).max(1);
            phys_b = phys_b.wrapping_add(next() % 7).wrapping_sub(next() % 5).max(1);
            match next() % 4 {
                0 => {
                    let t = a.now(phys_a);
                    assert!(t > last_a);
                    last_a = t;
                }
                1 => {
                    let t = b.now(phys_b);
                    assert!(t > last_b);
                    last_b = t;
                }
                2 => {
                    let sent = a.now(phys_a);
                    assert!(sent > last_a);
                    last_a = sent;
                    let t = b.update(sent, phys_b).unwrap();
                    assert!(t > sent && t > last_b);
                    last_b = t;
                }
                _ => {
                    let sent = b.now(phys_b);
                    assert!(sent > last_b);
                    last_b = sent;
                    let t = a.update(sent, phys_a).unwrap();
                    assert!(t > sent && t > last_a);
                    last_a = t;
                }
            }
        }
    }

    #[test]
    fn monotonic_under_random_backward_clock_jumps_over_50_seeds() {
        for seed in 0..50u64 {
            let mut r = Rng::new(seed);
            let mut h = Hlc::new(7, 10_000_000);
            let mut peer = Hlc::new(8, 10_000_000);
            let mut phys = 1_000_000u64;
            let mut last = Timestamp::MIN;
            for _ in 0..2_000 {
                match r.below(4) {
                    0 => phys = phys.saturating_sub(r.below(500_000)),
                    1 => phys += r.below(50),
                    _ => {}
                }
                let t = if r.chance(1, 3) {
                    let remote = peer.now(phys);
                    h.update(remote, phys).unwrap()
                } else {
                    h.now(phys)
                };
                assert!(t > last, "seed {seed}: {t:?} not above {last:?}");
                last = t;
            }
        }
    }

    #[test]
    fn remote_stamp_accepted_at_exactly_max_skew_and_rejected_one_past() {
        for seed in 0..200u64 {
            let mut r = Rng::new(seed);
            let skew = r.below(1_000);
            let phys = r.below(1_000_000);
            let mut h = Hlc::new(1, skew);
            h.now(phys);
            let before = h.last();
            let over = Timestamp::new(phys + skew + 1, 0, 2);
            assert!(matches!(h.update(over, phys), Err(HlcError::SkewExceeded { .. })));
            assert_eq!(h.last(), before);
            let edge = Timestamp::new(phys + skew, 0, 2);
            let t = h.update(edge, phys).unwrap();
            assert!(t > edge && t > before);
        }
    }

    #[test]
    fn skew_check_does_not_overflow_near_u64_max() {
        let mut h = Hlc::new(1, 100);
        let t = h.update(Timestamp::new(u64::MAX, 0, 2), u64::MAX - 5);
        assert!(t.is_ok());
        let again = h.now(1);
        assert!(again > t.unwrap());
    }

    #[test]
    fn rejected_remote_never_moves_the_clock_over_random_attempts() {
        let mut r = Rng::new(77);
        let mut h = Hlc::new(1, 50);
        let mut phys = 10_000u64;
        for _ in 0..5_000 {
            phys += r.below(20);
            let remote = Timestamp::new(phys + r.below(200), r.below(5) as u32, 2);
            let before = h.last();
            match h.update(remote, phys) {
                Ok(t) => assert!(t > before && t > remote),
                Err(_) => {
                    assert_eq!(h.last(), before);
                    assert!(remote.wall > phys + 50);
                }
            }
        }
    }
}