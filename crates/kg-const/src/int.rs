//! Owner-topic: **arbitrary-precision integers**. The Rust image of Haskell
//! `Integer` as the Kanagawa front-end uses it, and of `boost::multiprecision::cpp_int`
//! as the C++ middle-end uses it (`compiler/cpp/pch.h:92`).
//!
//! Spec: `docs/spec/types.md` §14.1 (`ConstInt`), §1.7 (`clog2`), §1.8 (truncation).
//!
//! Widths legally reach `c_maxRegisterBitWidth = 1024 * 1024` bits
//! (`compiler/cpp/ir.h:39`), so `i128` is not sufficient and every operation here
//! must be exact. The `i128` fast path is an allocation optimisation only: it is
//! **invisible** through `Eq`, `Ord` and `Hash`, which is why [`ConstInt`] is a
//! struct over a private repr rather than the public two-variant enum sketched in
//! `types.md` §14.1 (see the module note in `src/lib.rs`).
//!
//! Semantics that must match Haskell exactly:
//!
//! * `quot` / `rem` truncate toward zero (`Constexpr.hs:181-186` uses `quot`/`rem`,
//!   not `div`/`mod`);
//! * bitwise operators are two's-complement over an infinite sign extension
//!   (`Data.Bits Integer`), which is also what `num_bigint::BigInt` implements;
//! * `shiftR` is arithmetic (sign-propagating).

use std::cmp::Ordering;
use std::sync::Arc;

use num_bigint::BigInt;

#[derive(Clone)]
enum Repr {
    /// Canonical for every value representable in `i128`.
    Small(i128),
    /// Only ever holds a value outside `i128`'s range. Enforced by
    /// [`ConstInt::from_bigint`], which is the sole constructor of this variant.
    Big(Arc<BigInt>),
}

/// An exact, unbounded signed integer.
///
/// Construction always normalizes: a value that fits in `i128` is *always*
/// [`Repr::Small`]. Every impl below relies on that invariant, so `Big` must
/// never be built except through [`ConstInt::from_bigint`].
#[derive(Clone)]
pub struct ConstInt(Repr);

impl ConstInt {
    pub const ZERO: ConstInt = ConstInt(Repr::Small(0));
    pub const ONE: ConstInt = ConstInt(Repr::Small(1));
    pub const MINUS_ONE: ConstInt = ConstInt(Repr::Small(-1));

    #[inline]
    pub const fn from_i128(v: i128) -> ConstInt {
        ConstInt(Repr::Small(v))
    }

    #[inline]
    pub const fn from_i64(v: i64) -> ConstInt {
        ConstInt(Repr::Small(v as i128))
    }

    #[inline]
    pub const fn from_u64(v: u64) -> ConstInt {
        ConstInt(Repr::Small(v as i128))
    }

    #[inline]
    pub const fn from_u32(v: u32) -> ConstInt {
        ConstInt(Repr::Small(v as i128))
    }

    pub fn from_usize(v: usize) -> ConstInt {
        ConstInt(Repr::Small(v as i128))
    }

    /// The only way to build the big representation. Normalizes down to `i128`
    /// whenever possible, which is what keeps `Eq`/`Ord`/`Hash` well defined.
    pub fn from_bigint(v: BigInt) -> ConstInt {
        match i128::try_from(&v) {
            Ok(small) => ConstInt(Repr::Small(small)),
            Err(_) => ConstInt(Repr::Big(Arc::new(v))),
        }
    }

    /// Parses `text` in `radix`, matching the lexer's decimal / `0b` / `0o` / `0x`
    /// forms after the prefix and `_` separators have been stripped
    /// (`docs/spec/lexer.md`, `Parser/Lexer.hs:133-155`).
    pub fn parse(text: &str, radix: u32) -> Option<ConstInt> {
        BigInt::parse_bytes(text.as_bytes(), radix).map(ConstInt::from_bigint)
    }

    #[inline]
    pub fn to_i128(&self) -> Option<i128> {
        match &self.0 {
            Repr::Small(v) => Some(*v),
            Repr::Big(_) => None,
        }
    }

    pub fn to_u64(&self) -> Option<u64> {
        u64::try_from(self.to_i128()?).ok()
    }

    pub fn to_u32(&self) -> Option<u32> {
        u32::try_from(self.to_i128()?).ok()
    }

    pub fn to_usize(&self) -> Option<usize> {
        usize::try_from(self.to_i128()?).ok()
    }

    pub fn to_bigint(&self) -> BigInt {
        match &self.0 {
            Repr::Small(v) => BigInt::from(*v),
            Repr::Big(v) => (**v).clone(),
        }
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        matches!(self.0, Repr::Small(0))
    }

    pub fn is_negative(&self) -> bool {
        match &self.0 {
            Repr::Small(v) => *v < 0,
            Repr::Big(v) => v.sign() == num_bigint::Sign::Minus,
        }
    }

    pub fn is_positive(&self) -> bool {
        !self.is_negative() && !self.is_zero()
    }

    pub fn abs(&self) -> ConstInt {
        if self.is_negative() { self.neg() } else { self.clone() }
    }

    /// Number of bits in the *magnitude*, i.e. Haskell
    /// `integerLog2 x + 1` for `x > 0`. `bits(0) == 0`.
    pub fn bits(&self) -> u64 {
        match &self.0 {
            Repr::Small(v) => {
                let m = v.unsigned_abs();
                (u128::BITS - m.leading_zeros()) as u64
            }
            Repr::Big(v) => v.bits(),
        }
    }

    /// True for `x > 0` that are exact powers of two. Haskell's
    /// `isPow2 x = x .&. (x - 1) == 0` (`Internal.hs:56`) also answers `True` for
    /// `0` and for some negatives; this predicate is the mathematical one, and
    /// [`ConstInt::clog2`] is the place that reproduces the Haskell guard.
    pub fn is_power_of_two(&self) -> bool {
        self.is_positive() && self.bitand(&self.sub(&ConstInt::ONE)).is_zero()
    }

    /// `clog2` — `Internal.hs:53-57`.
    ///
    /// ```text
    /// clog2 n | isPow2 n  = integerLog2 n
    ///         | otherwise = integerLog2 n + 1
    /// ```
    ///
    /// so `clog2 1 = 0`, `clog2 2 = 1`, `clog2 3 = 2`, `clog2 4 = 2`, `clog2 5 = 3`.
    ///
    /// Haskell's is **partial**: `integerLog2` is undefined for `n <= 0`. Every
    /// reference call site guards first — `fromNaturalConstexpr` requires `> 0`
    /// (`Type.hs:1102-1103`) — so this returns `None` rather than panicking, and
    /// callers reproduce the guard by treating `None` as "did not fold".
    pub fn checked_clog2(&self) -> Option<u32> {
        if !self.is_positive() {
            return None;
        }
        let bits = self.bits() as u32;
        Some(if self.is_power_of_two() { bits - 1 } else { bits })
    }

    /// [`ConstInt::checked_clog2`] with the reference's partiality made explicit.
    ///
    /// # Panics
    /// If `self <= 0`, matching `Internal.hs:53`. Prefer `checked_clog2`.
    pub fn clog2(&self) -> u32 {
        self.checked_clog2().expect("clog2 is undefined for n <= 0 (Internal.hs:53)")
    }

    /// `truncateUInt n x = x .&. ((1 << n) - 1)` (`Internal.hs:34-35`).
    pub fn truncate_uint(&self, n: u32) -> ConstInt {
        if n == 0 {
            return ConstInt::ZERO;
        }
        let mask = ConstInt::ONE.shl(n as u64).sub(&ConstInt::ONE);
        self.bitand(&mask)
    }

    /// Sign-extending two's-complement truncation into `n` bits.
    ///
    /// ```text
    /// truncateInt n x = let w = n - 1 in
    ///     if 0 == (x .&. (1 << w)) then truncateUInt w x
    ///                              else -(1 << w) + truncateUInt w x
    /// ```
    /// `Internal.hs:37-41`.
    ///
    /// `n == 0` makes `w == -1`, on which Haskell's `shiftL` is an error; a
    /// zero-width signed literal is separately rejected as
    /// ``"Invalid literal type specifier"`` (`Frontend.hs:210-217`,
    /// `docs/spec/types.md` §3.1), so this returns zero rather than diverging.
    pub fn truncate_int(&self, n: u32) -> ConstInt {
        if n == 0 {
            return ConstInt::ZERO;
        }
        let w = n - 1;
        let sign_bit = ConstInt::ONE.shl(w as u64);
        let low = self.truncate_uint(w);
        if self.bitand(&sign_bit).is_zero() { low } else { low.sub(&sign_bit) }
    }

    // --- arithmetic -------------------------------------------------------
    //
    // Inherent methods rather than `std::ops` impls: every call site in the
    // evaluator works on borrowed values, and the four `T`/`&T` permutations of
    // twelve operators would be noise.

    pub fn neg(&self) -> ConstInt {
        match &self.0 {
            Repr::Small(v) => match v.checked_neg() {
                Some(r) => ConstInt(Repr::Small(r)),
                None => ConstInt::from_bigint(-self.to_bigint()),
            },
            Repr::Big(_) => ConstInt::from_bigint(-self.to_bigint()),
        }
    }

    /// `complement` / `~x` — two's complement, i.e. `-x - 1`.
    pub fn not(&self) -> ConstInt {
        self.neg().sub(&ConstInt::ONE)
    }

    pub fn add(&self, other: &ConstInt) -> ConstInt {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => match a.checked_add(*b) {
                Some(r) => ConstInt(Repr::Small(r)),
                None => ConstInt::from_bigint(self.to_bigint() + other.to_bigint()),
            },
            _ => ConstInt::from_bigint(self.to_bigint() + other.to_bigint()),
        }
    }

    pub fn sub(&self, other: &ConstInt) -> ConstInt {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => match a.checked_sub(*b) {
                Some(r) => ConstInt(Repr::Small(r)),
                None => ConstInt::from_bigint(self.to_bigint() - other.to_bigint()),
            },
            _ => ConstInt::from_bigint(self.to_bigint() - other.to_bigint()),
        }
    }

    pub fn mul(&self, other: &ConstInt) -> ConstInt {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => match a.checked_mul(*b) {
                Some(r) => ConstInt(Repr::Small(r)),
                None => ConstInt::from_bigint(self.to_bigint() * other.to_bigint()),
            },
            _ => ConstInt::from_bigint(self.to_bigint() * other.to_bigint()),
        }
    }

    /// Haskell `quot`: truncates toward zero. `None` for a zero divisor, which the
    /// evaluator turns into `"Division by zero is undefined"` (`Constexpr.hs:181-183`).
    pub fn quot(&self, other: &ConstInt) -> Option<ConstInt> {
        if other.is_zero() {
            return None;
        }
        Some(match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => match a.checked_div(*b) {
                Some(r) => ConstInt(Repr::Small(r)),
                None => ConstInt::from_bigint(self.to_bigint() / other.to_bigint()),
            },
            _ => ConstInt::from_bigint(self.to_bigint() / other.to_bigint()),
        })
    }

    /// Haskell `rem`: sign follows the dividend. `None` for a zero divisor —
    /// `"Remainder by zero is undefined"` (`Constexpr.hs:184-186`).
    pub fn rem(&self, other: &ConstInt) -> Option<ConstInt> {
        if other.is_zero() {
            return None;
        }
        Some(match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => match a.checked_rem(*b) {
                Some(r) => ConstInt(Repr::Small(r)),
                None => ConstInt::from_bigint(self.to_bigint() % other.to_bigint()),
            },
            _ => ConstInt::from_bigint(self.to_bigint() % other.to_bigint()),
        })
    }

    pub fn bitand(&self, other: &ConstInt) -> ConstInt {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => ConstInt(Repr::Small(a & b)),
            _ => ConstInt::from_bigint(self.to_bigint() & other.to_bigint()),
        }
    }

    pub fn bitor(&self, other: &ConstInt) -> ConstInt {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => ConstInt(Repr::Small(a | b)),
            _ => ConstInt::from_bigint(self.to_bigint() | other.to_bigint()),
        }
    }

    pub fn bitxor(&self, other: &ConstInt) -> ConstInt {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => ConstInt(Repr::Small(a ^ b)),
            _ => ConstInt::from_bigint(self.to_bigint() ^ other.to_bigint()),
        }
    }

    /// Logical left shift. Unbounded, as `Integer`'s is.
    pub fn shl(&self, n: u64) -> ConstInt {
        if self.is_zero() {
            return ConstInt::ZERO;
        }
        if let Repr::Small(v) = &self.0
            && n < 127
            && let Some(r) = v.checked_shl(n as u32)
            && (r >> n) == *v
        {
            return ConstInt(Repr::Small(r));
        }
        ConstInt::from_bigint(self.to_bigint() << n)
    }

    /// Arithmetic (sign-propagating) right shift, as `Data.Bits Integer` is.
    pub fn shr(&self, n: u64) -> ConstInt {
        match &self.0 {
            Repr::Small(v) if n >= 127 => ConstInt(Repr::Small(if *v < 0 { -1 } else { 0 })),
            Repr::Small(v) => ConstInt(Repr::Small(v >> n)),
            Repr::Big(_) => ConstInt::from_bigint(self.to_bigint() >> n),
        }
    }

    /// True when bit `n` (counting from 0) of the infinite two's-complement
    /// expansion is set — Haskell `testBit` (`Constexpr.hs` imports it).
    pub fn test_bit(&self, n: u64) -> bool {
        !self.shr(n).bitand(&ConstInt::ONE).is_zero()
    }

    pub fn to_str_radix(&self, radix: u32) -> String {
        self.to_bigint().to_str_radix(radix)
    }
}

// --- Eq / Ord / Hash --------------------------------------------------------
//
// Hand-written because deriving over `Repr` would make `Small(5) != Big(5)` and
// would sort every `Small` below every `Big`. The normalization invariant makes
// these cheap: two values are equal iff their reprs are.

impl PartialEq for ConstInt {
    fn eq(&self, other: &ConstInt) -> bool {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => a == b,
            (Repr::Big(a), Repr::Big(b)) => Arc::ptr_eq(a, b) || a == b,
            // Unreachable under the invariant: a `Big` never fits in `i128`.
            _ => false,
        }
    }
}

impl Eq for ConstInt {}

impl PartialOrd for ConstInt {
    fn partial_cmp(&self, other: &ConstInt) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ConstInt {
    fn cmp(&self, other: &ConstInt) -> Ordering {
        match (&self.0, &other.0) {
            (Repr::Small(a), Repr::Small(b)) => a.cmp(b),
            (Repr::Big(a), Repr::Big(b)) => a.cmp(b),
            // A `Big` is out of `i128` range, so its sign decides.
            (Repr::Small(_), Repr::Big(b)) => {
                if b.sign() == num_bigint::Sign::Minus {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            (Repr::Big(a), Repr::Small(_)) => {
                if a.sign() == num_bigint::Sign::Minus {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
        }
    }
}

impl std::hash::Hash for ConstInt {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match &self.0 {
            Repr::Small(v) => v.hash(state),
            Repr::Big(v) => v.hash(state),
        }
    }
}

impl std::fmt::Display for ConstInt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Repr::Small(v) => write!(f, "{v}"),
            Repr::Big(v) => write!(f, "{v}"),
        }
    }
}

impl std::fmt::Debug for ConstInt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl From<i128> for ConstInt {
    fn from(v: i128) -> ConstInt {
        ConstInt::from_i128(v)
    }
}

impl From<u64> for ConstInt {
    fn from(v: u64) -> ConstInt {
        ConstInt::from_u64(v)
    }
}

impl From<BigInt> for ConstInt {
    fn from(v: BigInt) -> ConstInt {
        ConstInt::from_bigint(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(v: i128) -> ConstInt {
        ConstInt::from_i128(v)
    }

    #[test]
    fn clog2_matches_internal_hs() {
        // Internal.hs:53-57, worked values from docs/spec/types.md §1.7.
        assert_eq!(c(1).checked_clog2(), Some(0));
        assert_eq!(c(2).checked_clog2(), Some(1));
        assert_eq!(c(3).checked_clog2(), Some(2));
        assert_eq!(c(4).checked_clog2(), Some(2));
        assert_eq!(c(5).checked_clog2(), Some(3));
        assert_eq!(c(16).checked_clog2(), Some(4));
        assert_eq!(c(17).checked_clog2(), Some(5));
        assert_eq!(c(200).checked_clog2(), Some(8));
        // Partial in the reference (integerLog2 is undefined for n <= 0).
        assert_eq!(c(0).checked_clog2(), None);
        assert_eq!(c(-1).checked_clog2(), None);
    }

    #[test]
    fn truncate_uint_masks() {
        assert_eq!(c(0xff).truncate_uint(4), c(0xf));
        assert_eq!(c(-1).truncate_uint(8), c(255));
        assert_eq!(c(5).truncate_uint(0), c(0));
    }

    #[test]
    fn truncate_int_sign_extends() {
        // Internal.hs:37-41.
        assert_eq!(c(0xff).truncate_int(8), c(-1));
        assert_eq!(c(0x7f).truncate_int(8), c(127));
        assert_eq!(c(0x80).truncate_int(8), c(-128));
        assert_eq!(c(1).truncate_int(1), c(-1));
    }

    #[test]
    fn quot_and_rem_truncate_toward_zero() {
        assert_eq!(c(-7).quot(&c(2)), Some(c(-3)));
        assert_eq!(c(-7).rem(&c(2)), Some(c(-1)));
        assert_eq!(c(1).quot(&ConstInt::ZERO), None);
        assert_eq!(c(1).rem(&ConstInt::ZERO), None);
    }

    #[test]
    fn survives_beyond_i128() {
        // Widths reach 1048576 bits (cpp/ir.h:39): the i128 path must not be load-bearing.
        let big = ConstInt::ONE.shl(1_000);
        assert!(big.to_i128().is_none());
        assert_eq!(big.bits(), 1_001);
        assert_eq!(big.checked_clog2(), Some(1_000));
        assert_eq!(big.shr(1_000), ConstInt::ONE);
        assert_eq!(big.sub(&ConstInt::ONE).add(&ConstInt::ONE), big);
    }

    #[test]
    fn representation_is_invisible() {
        let round_trip = ConstInt::from_bigint(ConstInt::ONE.shl(200).shr(200).to_bigint());
        assert_eq!(round_trip, ConstInt::ONE);
        assert!(ConstInt::ONE.shl(200).neg() < ConstInt::from_i128(i128::MIN));
        assert!(ConstInt::ONE.shl(200) > ConstInt::from_i128(i128::MAX));
    }

    #[test]
    fn not_is_two_s_complement() {
        assert_eq!(c(0).not(), c(-1));
        assert_eq!(c(5).not(), c(-6));
    }

    /// `Constexpr.hs:182,185` uses `quot`/`rem`, **not** `div`/`mod`.
    ///
    /// Haskell's `quot` truncates toward zero and `rem`'s sign follows the
    /// *dividend*: `(-7) quot 2 == -3` and `(-7) rem 2 == -1`, where `div`/`mod`
    /// would give `-4` and `1`. Rust's `/` and `%` agree with `quot`/`rem`;
    /// Python's `//` and `%` agree with `div`/`mod`, which is the trap this test
    /// exists to pin.
    #[test]
    fn quot_and_rem_are_quot_rem_not_div_mod() {
        let table = [
            (7, 2, 3, 1),
            (-7, 2, -3, -1),
            (7, -2, -3, 1),
            (-7, -2, 3, -1),
            (1, 2, 0, 1),
            (-1, 2, 0, -1),
        ];
        for (x, y, q, r) in table {
            assert_eq!(c(x).quot(&c(y)), Some(c(q)), "{x} quot {y}");
            assert_eq!(c(x).rem(&c(y)), Some(c(r)), "{x} rem {y}");
            // `quot`/`rem` satisfy `(x quot y) * y + (x rem y) == x`.
            assert_eq!(c(q).mul(&c(y)).add(&c(r)), c(x));
        }
    }

    /// `Data.Bits Integer` operates on an infinite two's-complement sign
    /// extension, which is also what `num_bigint::BigInt` implements. This is the
    /// model `Constexpr.hs:187-189` and `evalBinaryInt`'s `testBit`
    /// (`Constexpr.hs:234`) assume.
    #[test]
    fn bit_ops_are_two_s_complement_over_an_infinite_extension() {
        assert_eq!(c(-1).bitand(&c(0xff)), c(255));
        assert_eq!(c(-2).bitor(&c(1)), c(-1));
        assert_eq!(c(-1).bitxor(&c(-1)), c(0));
        assert_eq!(c(-1).bitxor(&c(0)), c(-1));
        // Every bit of -1 is set, arbitrarily far up.
        assert!(c(-1).test_bit(0));
        assert!(c(-1).test_bit(1_000));
        // 0x2fffffff has bits 0..27 set and bit 28 clear — the exact boundary
        // `test/syntax/constexpr.k:1200-1222` turns on.
        let x = c(0x2fff_ffff);
        assert!((0..28).all(|i| x.test_bit(i)));
        assert!(!x.test_bit(28));
    }

    /// `shiftR` on `Integer` is arithmetic: it propagates the sign bit forever.
    #[test]
    fn shift_right_is_arithmetic() {
        assert_eq!(c(-8).shr(2), c(-2));
        assert_eq!(c(-1).shr(1), c(-1));
        assert_eq!(c(-1).shr(1_000), c(-1));
        assert_eq!(c(8).shr(1_000), c(0));
        assert_eq!(c(-3).shr(1), c(-2), "arithmetic shift floors, it does not truncate");
    }

    /// The truncation helpers have to be exact past 128 bits, because
    /// `c_maxRegisterBitWidth` is `1024 * 1024` (`cpp/ir.h:39`) and
    /// `castIntegralConstexpr` truncates into the *declared* width
    /// (`Constexpr.hs:329-331`).
    #[test]
    fn truncation_is_exact_past_i128() {
        let sign_bit = ConstInt::ONE.shl(1023);
        // The value with only bit 1023 set is exactly `int1024`'s minimum.
        assert_eq!(sign_bit.truncate_int(1024), sign_bit.neg());
        assert_eq!(sign_bit.truncate_uint(1024), sign_bit);
        // -1 truncated into N unsigned bits is 2^N - 1, for every N.
        for n in [1u32, 8, 32, 64, 128, 1024] {
            assert_eq!(c(-1).truncate_uint(n), ConstInt::ONE.shl(u64::from(n)).sub(&ConstInt::ONE));
            assert_eq!(c(-1).truncate_int(n), c(-1));
        }
    }

    /// The `cast<uint8>` / `cast<int8>` rows of `test/syntax/constexpr.k:819-859`,
    /// which is the only place the corpus states truncation results outright.
    #[test]
    fn truncation_matches_the_cast_corpus() {
        assert_eq!(c(255).truncate_uint(8), c(255));
        assert_eq!(c(-1).truncate_uint(8), c(255));
        assert_eq!(c(255).truncate_int(8), c(-1));
        assert_eq!(c(127).truncate_int(8), c(127));
        assert_eq!(c(128).truncate_int(8), c(-128));
        assert_eq!(c(129).truncate_int(8), c(-127));
    }

    /// `test/syntax/constexpr.k:1423-1447`.
    #[test]
    fn clog2_matches_the_corpus_assertions() {
        assert_eq!(c(1024).checked_clog2(), Some(10));
        assert_eq!(c(513).checked_clog2(), Some(10));
        assert_eq!(c(127).checked_clog2(), Some(7));
        // `clog2 1 == 0` — test/syntax/constexpr.k:427-432.
        assert_eq!(c(1).checked_clog2(), Some(0));
        // A width at the register cap still answers exactly.
        assert_eq!(ConstInt::ONE.shl(1_048_576).checked_clog2(), Some(1_048_576));
    }

    #[test]
    fn shl_is_unbounded_and_shr_inverts_it() {
        let x = c(0x1234_5678);
        for n in [0u64, 1, 63, 64, 127, 128, 4096] {
            assert_eq!(x.shl(n).shr(n), x, "shl/shr round trip at {n}");
        }
        assert_eq!(ConstInt::ZERO.shl(1_000_000), ConstInt::ZERO);
    }
}
