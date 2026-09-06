//! Exact non-negative rationals over `u128`, for the one place the simulator claims exactness.
//!
//! Why not floats: `docs/agent-architecture.md` section 2.2 records a true-division bug that broke
//! the exact proof of the epoch closed form while every float check still passed, because a float
//! answer is correct to 1e-16 and only exact arithmetic notices. The closed form and the naive
//! per-step sum are therefore compared for *equality* on this type, and the type never rounds.
//!
//! Why `u128` rather than a bignum: no dependencies, and `epoch.rs` documents the bound under which
//! nothing here can overflow at the fleet scale of `docs/ARCHITECTURE.md` section 1.1. Every
//! operation is checked, so exceeding that bound panics with a message instead of wrapping into a
//! plausible-looking wrong number, which is the failure mode this crate exists to make impossible.
//!
//! Why comparison never multiplies across: two rationals whose denominators are each near 2^80
//! cannot be cross-multiplied in `u128`, so ordering uses Euclid's continued-fraction descent, which
//! only ever divides. That lets an epoch duration (denominator ~2^75) be compared against an offset
//! in nanoseconds (denominator 10^9) without either being rescaled.

use std::cmp::Ordering;

const OVERFLOW: &str =
    "sim-physics: exact arithmetic overflowed u128; the bound in epoch.rs was exceeded";

#[inline]
pub(crate) fn mul(a: u128, b: u128) -> u128 {
    a.checked_mul(b).expect(OVERFLOW)
}

#[inline]
pub(crate) fn add(a: u128, b: u128) -> u128 {
    a.checked_add(b).expect(OVERFLOW)
}

pub fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

pub fn lcm(a: u128, b: u128) -> u128 {
    if a == 0 || b == 0 {
        return 0;
    }
    mul(a / gcd(a, b), b)
}

/// `num / den`, always non-negative, `den` never zero. Not necessarily in lowest terms: the hot
/// path in `epoch.rs` builds durations over the epoch's shared denominator without a gcd, and
/// equality is by value rather than by representation, so it does not need to.
#[derive(Clone, Copy, Debug)]
pub struct Rational {
    num: u128,
    den: u128,
}

impl Rational {
    pub const ZERO: Rational = Rational { num: 0, den: 1 };

    /// Reduced to lowest terms. Use this for inputs, so denominators start as small as they can.
    pub fn new(num: u128, den: u128) -> Self {
        Rational::raw(num, den).reduced()
    }

    /// Unreduced. `const` so hardware constants can be written as fractions in a `const` table.
    pub const fn raw(num: u128, den: u128) -> Self {
        assert!(den > 0, "sim-physics: zero denominator");
        Rational { num, den }
    }

    pub const fn int(n: u128) -> Self {
        Rational { num: n, den: 1 }
    }

    /// Seconds from integer nanoseconds, the engine's clock unit.
    pub fn from_nanos(ns: u64) -> Self {
        Rational::new(ns as u128, 1_000_000_000)
    }

    pub fn num(&self) -> u128 {
        self.num
    }

    pub fn den(&self) -> u128 {
        self.den
    }

    pub fn is_zero(&self) -> bool {
        self.num == 0
    }

    pub fn reduced(self) -> Self {
        let g = gcd(self.num, self.den);
        Rational { num: self.num / g, den: self.den / g }
    }

    pub fn add(self, o: Rational) -> Self {
        let g = gcd(self.den, o.den);
        let num = add(mul(self.num, o.den / g), mul(o.num, self.den / g));
        Rational { num, den: mul(self.den / g, o.den) }.reduced()
    }

    /// Panics if the result would be negative; this type has no sign.
    pub fn sub(self, o: Rational) -> Self {
        let g = gcd(self.den, o.den);
        let a = mul(self.num, o.den / g);
        let b = mul(o.num, self.den / g);
        let num = a.checked_sub(b).expect("sim-physics: negative rational");
        Rational { num, den: mul(self.den / g, o.den) }.reduced()
    }

    pub fn mul(self, o: Rational) -> Self {
        // Cross-reduce first so intermediate products stay as small as the result.
        let g1 = gcd(self.num, o.den);
        let g2 = gcd(o.num, self.den);
        Rational {
            num: mul(self.num / g1, o.num / g2),
            den: mul(self.den / g2, o.den / g1),
        }
    }

    pub fn div(self, o: Rational) -> Self {
        assert!(o.num > 0, "sim-physics: division by zero rational");
        self.mul(Rational { num: o.den, den: o.num })
    }

    pub fn floor(&self) -> u128 {
        self.num / self.den
    }

    /// Seconds to whole nanoseconds, rounded down. Split into whole seconds plus a remainder so the
    /// remainder times 10^9 fits even when the denominator is near 2^90.
    pub fn to_nanos_floor(&self) -> u64 {
        let whole = self.num / self.den;
        let rem = self.num % self.den;
        let ns = add(mul(whole, 1_000_000_000), mul(rem, 1_000_000_000) / self.den);
        u64::try_from(ns).expect(OVERFLOW)
    }

    /// Seconds to whole nanoseconds, rounded up: the first clock tick at or after the instant.
    pub fn to_nanos_ceil(&self) -> u64 {
        let floor = self.to_nanos_floor();
        let exact = mul(self.num, 1_000_000_000) % self.den == 0;
        if exact {
            floor
        } else {
            floor + 1
        }
    }

    /// Approximate, for calibration checks with a tolerance. Never on the exact path.
    pub fn to_f64(&self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

impl PartialEq for Rational {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}

impl Eq for Rational {}

impl PartialOrd for Rational {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Rational {
    /// Continued-fraction comparison: compare integer parts, and if they tie, compare the
    /// reciprocals of the fractional parts with the order flipped. Only divisions, so it cannot
    /// overflow whatever the denominators are.
    fn cmp(&self, o: &Self) -> Ordering {
        let (mut a, mut b, mut c, mut d) = (self.num, self.den, o.num, o.den);
        loop {
            let (qa, ra) = (a / b, a % b);
            let (qc, rc) = (c / d, c % d);
            if qa != qc {
                return qa.cmp(&qc);
            }
            match (ra == 0, rc == 0) {
                (true, true) => return Ordering::Equal,
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                (false, false) => {
                    // ra/b vs rc/d  is  d/rc vs b/ra.
                    let (nb, nd) = (rc, ra);
                    a = d;
                    c = b;
                    b = nb;
                    d = nd;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_does_not_cross_multiply() {
        let big = 1u128 << 100;
        let a = Rational::raw(big + 1, big);
        let b = Rational::raw(big + 2, big + 1);
        // (2^100+1)/2^100 > (2^100+2)/(2^100+1): cross-multiplying would need 2^200.
        assert!(a > b);
        assert_eq!(Rational::raw(6, 4), Rational::raw(3, 2));
        assert!(Rational::raw(1, 3) < Rational::raw(1, 2));
        assert!(Rational::raw(7, 3) > Rational::raw(9, 4));
        assert_eq!(Rational::ZERO, Rational::raw(0, 12345));
    }

    #[test]
    fn nanos_round_trip() {
        let t = Rational::new(10_250_000_001, 1_000_000_000);
        assert_eq!(t.to_nanos_floor(), 10_250_000_001);
        assert_eq!(t.to_nanos_ceil(), 10_250_000_001);
        let third = Rational::new(1, 3);
        assert_eq!(third.to_nanos_floor(), 333_333_333);
        assert_eq!(third.to_nanos_ceil(), 333_333_334);
    }
}
