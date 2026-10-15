pub type Fx = i64;

pub const FRAC_BITS: u32 = 32;
pub const ONE: Fx = 1 << FRAC_BITS;

pub fn clamp(x: i128) -> i64 {
    if x > i128::from(i64::MAX) {
        i64::MAX
    } else if x < i128::from(i64::MIN) {
        i64::MIN
    } else {
        x as i64
    }
}

pub fn ratio(n: u64, d: u64) -> Fx {
    if d == 0 {
        return 0;
    }
    clamp((i128::from(n) << FRAC_BITS) / i128::from(d))
}

pub fn mul(a: Fx, b: Fx) -> Fx {
    clamp((i128::from(a) * i128::from(b)) >> FRAC_BITS)
}

pub fn decay(h: Fx, dt: u64, half_life: u64) -> Fx {
    if h <= 0 {
        return 0;
    }
    if dt == 0 {
        return h;
    }
    let hl = half_life.max(1);
    let whole = dt / hl;
    if whole >= 63 {
        return 0;
    }
    let rem = dt % hl;
    let shifted = h >> whole;
    if rem == 0 {
        return shifted;
    }
    let drop = (i128::from(rem) << FRAC_BITS) / (2 * i128::from(hl));
    mul(shifted, clamp(i128::from(ONE) - drop))
}

pub fn log2(n: u64) -> Fx {
    if n == 0 {
        return 0;
    }
    let lz = n.leading_zeros();
    let int = i64::from(63 - lz);
    let mut x: u128 = u128::from(n << lz);
    let mut frac: u64 = 0;
    for i in 0..FRAC_BITS {
        x = (x * x) >> 63;
        if x >= (1u128 << 64) {
            frac |= 1u64 << (FRAC_BITS - 1 - i);
            x >>= 1;
        }
    }
    (int << FRAC_BITS) | frac as i64
}

pub fn entropy(counts: &[u64]) -> Fx {
    let total: u64 = counts.iter().fold(0u64, |a, &c| a.saturating_add(c));
    if total <= 1 {
        return 0;
    }
    let mut acc: i128 = 0;
    for &c in counts {
        if c > 0 {
            acc += i128::from(c) * i128::from(log2(c));
        }
    }
    let h = i128::from(log2(total)) - acc / i128::from(total);
    clamp(h.max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_halves_exactly_per_half_life() {
        assert_eq!(decay(8 * ONE, 10, 10), 4 * ONE);
        assert_eq!(decay(8 * ONE, 30, 10), ONE);
        assert_eq!(decay(8 * ONE, 0, 10), 8 * ONE);
        assert_eq!(decay(8 * ONE, 10 * 63, 10), 0);
        assert_eq!(decay(-5, 3, 10), 0);
    }

    #[test]
    fn decay_is_monotone_in_dt() {
        let mut last = i64::MAX;
        for dt in 0..400 {
            let d = decay(3 * ONE, dt, 37);
            assert!(d <= last, "dt {dt}");
            last = d;
        }
    }

    #[test]
    fn log2_matches_known_values() {
        assert_eq!(log2(1), 0);
        assert_eq!(log2(8), 3 * ONE);
        let l3 = log2(3);
        let want = (1.584_962_500_721_156_f64 * ONE as f64) as i64;
        assert!((l3 - want).abs() < 4, "{l3} vs {want}");
        assert_eq!(log2(0), 0);
    }

    #[test]
    fn entropy_of_uniform_and_point_mass() {
        assert_eq!(entropy(&[5]), 0);
        assert_eq!(entropy(&[]), 0);
        let h = entropy(&[3, 3, 3, 3]);
        assert!((h - 2 * ONE).abs() < 16, "{h}");
        let h = entropy(&[1, 1]);
        assert!((h - ONE).abs() < 16, "{h}");
    }

    #[test]
    fn saturates_instead_of_overflowing() {
        assert_eq!(mul(i64::MAX, i64::MAX), i64::MAX);
        assert_eq!(ratio(u64::MAX, 1), i64::MAX);
        assert_eq!(ratio(1, 0), 0);
    }
}
