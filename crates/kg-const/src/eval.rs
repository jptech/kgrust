//! The fold's tables: `evalBinary` over two literals (`Constexpr.hs:171-215`)
//! and what a cast or a stamp truncates into (`Constexpr.hs:307-339`) — what
//! the IR's fold reads (`kg_hir::constexpr`, `kg_sem`'s inference).
//!
//! Spec: `docs/spec/types.md` §3.3 (constant folding and literal re-typing).
//!
//! Until sprint W3.7 this module was the CST's constexpr evaluator —
//! `evaluateConstexpr` and `staticControl` over the lossless tree, asking a
//! `ConstEnv` what the reference's rewritten tree already knew — which the
//! CST inference engine and its one-file constant environment folded with.
//! It went with that engine; the program-wide loop folds its own tree, as
//! the reference does, and only the tables both shared are left here.

use crate::{BinOp, ConstInt, ConstValue, LiteralType};

/// What `cast<T>(literal)` should do to a folded integer (`Constexpr.hs:307-339`).
///
/// Doubles as the fold's minimal image of a *scalar type*: it is precisely
/// the information `tIsSigned`/`tIsUnsigned`/`tIsFloat`/`tIsBool` plus `tWidth`
/// carry (`kg_hir::constexpr`'s casts and stamps).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CastTarget {
    /// `intN` — `truncateInt N` (`Internal.hs:37-41`).
    Signed(u32),
    /// `uintN` — `truncateUInt N` (`Internal.hs:34-35`).
    Unsigned(u32),
    /// `float32` — **reinterpret** the low 32 bits, do not convert
    /// (`castWord32ToFloat`, `Constexpr.hs:332`).
    Float,
    Bool,
}

impl CastTarget {
    /// The `LiteralType` suffix `typedIntLiteral` would stamp
    /// (`Constexpr.hs:324-327`). `None` for the non-integer targets, where
    /// `fromJust $ tWidth t` has nothing to take.
    pub fn literal_type(self) -> Option<LiteralType> {
        match self {
            CastTarget::Signed(w) => Some(LiteralType::signed(w)),
            CastTarget::Unsigned(w) => Some(LiteralType::unsigned(w)),
            CastTarget::Float | CastTarget::Bool => None,
        }
    }

    /// `tWidth` (`Type.hs:695-723`) of the type this stands for.
    pub fn width(self) -> u32 {
        match self {
            CastTarget::Signed(w) | CastTarget::Unsigned(w) => w,
            CastTarget::Float => 32,
            CastTarget::Bool => 1,
        }
    }

    /// `tIsInt` (`Type.hs:672`).
    pub fn is_int(self) -> bool {
        matches!(self, CastTarget::Signed(_) | CastTarget::Unsigned(_))
    }

    /// Two's-complement truncation into this type — the `truncateInt` /
    /// `truncateUInt` split of `castIntegralConstexpr` (`Constexpr.hs:329-331`)
    /// and of `evalUnary Invert` (`Constexpr.hs:266-269`). `None` for a
    /// non-integer target, mirroring `otherwise = const Nothing`
    /// (`Constexpr.hs:333`).
    pub fn truncate(self, value: &ConstInt) -> Option<ConstInt> {
        match self {
            CastTarget::Signed(w) => Some(value.truncate_int(w)),
            CastTarget::Unsigned(w) => Some(value.truncate_uint(w)),
            CastTarget::Float | CastTarget::Bool => None,
        }
    }
}

/// Beyond this a left shift cannot produce anything the later passes accept:
/// `c_maxRegisterBitWidth` is `1024 * 1024` (`cpp/ir.h:39`). The cap exists so
/// that a hostile `1 << 10000000000` allocates nothing.
const MAX_FOLDED_SHIFT: u64 = 1 << 21;

/// `fromIntegral y :: Int` for a shift count, with the two cases Haskell cannot
/// survive turned into `None`: a negative count (`shiftL` raises
/// `overflowError`) and one that does not fit a machine word.
fn shift_count(y: &ConstInt) -> Option<u64> {
    y.to_i128().filter(|n| *n >= 0).and_then(|n| u64::try_from(n).ok())
}

/// `evalBinary`'s three homogeneous clauses over two folded values
/// (`Constexpr.hs:171-215`) — int/int, bool/bool, string/string — with no
/// environment: what the IR's fold asks of two literals (`kg_hir::constexpr`,
/// `kg_sem`'s inference). `Ok(None)` where the table has no rule
/// (`_ -> e`), `Err` for the two `desugarError`s. The result is untyped; a
/// caller with typed operands stamps it (`Constexpr.hs:200-202`).
pub fn fold_binary_values(op: BinOp, lhs: &ConstValue, rhs: &ConstValue) -> Result<Option<ConstValue>, &'static str> {
    // `evalBinary (EnumValue _ x) y = evalBinary x y` (`Constexpr.hs:174-175`).
    if let (Some(x), Some(y)) = (lhs.as_int(), rhs.as_int()) {
        return fold_binary_int(op, x, y);
    }
    if let (Some(x), Some(y)) = (lhs.as_bool(), rhs.as_bool()) {
        // `Constexpr.hs:204-210`.
        return Ok(match op {
            BinOp::Equal => Some(ConstValue::Bool(x == y)),
            BinOp::NotEqual => Some(ConstValue::Bool(x != y)),
            BinOp::Or => Some(ConstValue::Bool(x || y)),
            BinOp::And => Some(ConstValue::Bool(x && y)),
            // `(x || y) && not (x && y)` — exclusive or, written out.
            BinOp::Xor => Some(ConstValue::Bool((x || y) && !(x && y))),
            _ => None,
        });
    }
    if let (Some(x), Some(y)) = (lhs.as_str(), rhs.as_str()) {
        // `Constexpr.hs:212-215`.
        return Ok(match op {
            BinOp::Equal => Some(ConstValue::Bool(x == y)),
            BinOp::NotEqual => Some(ConstValue::Bool(x != y)),
            _ => None,
        });
    }
    Ok(None)
}

/// The int/int table of `Constexpr.hs:176-198`, in source order.
fn fold_binary_int(op: BinOp, x: &ConstInt, y: &ConstInt) -> Result<Option<ConstValue>, &'static str> {
    let int = |v: ConstInt| Ok(Some(ConstValue::Int { ty: None, value: v }));
    let boolean = |b: bool| Ok(Some(ConstValue::Bool(b)));
    match op {
        BinOp::Add => int(x.add(y)),
        BinOp::Sub => int(x.sub(y)),
        BinOp::Mul | BinOp::LutMul => int(x.mul(y)),
        BinOp::Div => match x.quot(y) {
            Some(v) => int(v),
            None => Err("Division by zero is undefined"),
        },
        BinOp::Mod => match x.rem(y) {
            Some(v) => int(v),
            None => Err("Remainder by zero is undefined"),
        },
        BinOp::BitwiseAnd => int(x.bitand(y)),
        BinOp::BitwiseOr => int(x.bitor(y)),
        BinOp::BitwiseXor => int(x.bitxor(y)),
        BinOp::ShiftLeft => match shift_count(y) {
            Some(n) if n <= MAX_FOLDED_SHIFT => int(x.shl(n)),
            // Haskell's `shiftL` on `Integer` raises `overflowError` for a
            // negative count — the reference *crashes*: `static assert((3 <<
            // -1) == 1)` exits 1 with no `Error` line (`test/syntax/basics.k
            // ::4`, the backend gate's recorded run; its `expected:0` holds
            // only because the harness counts `Error` lines). A huge count
            // would allocate without bound, and no width past
            // `c_maxRegisterBitWidth = 1048576` (`cpp/ir.h:39`) survives
            // the later checks anyway. Both are left unfolded.
            _ => Ok(None),
        },
        BinOp::ShiftRight => match shift_count(y) {
            Some(n) => int(x.shr(n)),
            None => Ok(None),
        },
        BinOp::Equal => boolean(x == y),
        BinOp::NotEqual => boolean(x != y),
        BinOp::Greater => boolean(x > y),
        BinOp::GreaterEqual => boolean(x >= y),
        BinOp::Less => boolean(x < y),
        BinOp::LessEqual => boolean(x <= y),
        // `_ -> e`: `&&`, `||`, `^^` on two integers do not fold
        // (`test/syntax/constexpr.k:934-939`, `uint<1 && 1 && 1>` is an
        // error).
        BinOp::And | BinOp::Or | BinOp::Xor => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i128) -> ConstValue {
        ConstValue::Int { ty: None, value: ConstInt::from_i128(v) }
    }

    fn folds(op: BinOp, x: &ConstValue, y: &ConstValue) -> ConstValue {
        fold_binary_values(op, x, y).expect("no error").unwrap_or_else(|| panic!("{x:?} {op:?} {y:?} folds"))
    }

    fn int_of(op: BinOp, x: i128, y: i128) -> ConstInt {
        match folds(op, &int(x), &int(y)) {
            ConstValue::Int { ty: None, value } => value,
            other => panic!("{x} {op:?} {y} is {other:?}, not an untyped integer"),
        }
    }

    fn unfolded(op: BinOp, x: &ConstValue, y: &ConstValue) -> bool {
        matches!(fold_binary_values(op, x, y), Ok(None))
    }

    // These were the CST evaluator's, read through `static assert`
    // expressions; the table they pin is this one (sprint W3.7).

    /// The int/int table (`Constexpr.hs:176-198`): `quot` and `rem`, not
    /// `div` and `mod` — `-7 / 2` is `-3`.
    #[test]
    fn arithmetic_folds_with_haskell_quot_rem() {
        assert_eq!(int_of(BinOp::Add, 2, 2), ConstInt::from_i128(4));
        assert_eq!(int_of(BinOp::Sub, 2, 1), ConstInt::from_i128(1));
        assert_eq!(int_of(BinOp::Mul, 7, 6), ConstInt::from_i128(42));
        assert_eq!(int_of(BinOp::LutMul, 2, 3), ConstInt::from_i128(6));
        assert_eq!(int_of(BinOp::Div, 7, 2), ConstInt::from_i128(3));
        assert_eq!(int_of(BinOp::Div, -7, 2), ConstInt::from_i128(-3));
        assert_eq!(int_of(BinOp::Mod, -7, 2), ConstInt::from_i128(-1));
        assert_eq!(int_of(BinOp::Mod, 7, -2), ConstInt::from_i128(1));
        // `test/syntax/constexpr.k:1063-1077` — the only corpus rows that
        // state a product outright, over `const int8` operands.
        assert_eq!(int_of(BinOp::Mul, -1, -2), ConstInt::from_i128(2));
        assert_eq!(int_of(BinOp::Mul, -4, 5), ConstInt::from_i128(-20));
    }

    #[test]
    fn division_and_remainder_by_zero_are_errors() {
        assert_eq!(fold_binary_values(BinOp::Div, &int(1), &int(0)), Err("Division by zero is undefined"));
        assert_eq!(fold_binary_values(BinOp::Mod, &int(1), &int(0)), Err("Remainder by zero is undefined"));
    }

    #[test]
    fn bitwise_and_shift_fold() {
        assert_eq!(int_of(BinOp::BitwiseAnd, 12, 10), ConstInt::from_i128(8));
        assert_eq!(int_of(BinOp::BitwiseOr, 12, 10), ConstInt::from_i128(14));
        assert_eq!(int_of(BinOp::BitwiseXor, 12, 10), ConstInt::from_i128(6));
        assert_eq!(int_of(BinOp::ShiftLeft, 1, 10), ConstInt::from_i128(1024));
        assert_eq!(int_of(BinOp::ShiftRight, -8, 2), ConstInt::from_i128(-2));
        // `test/syntax/constexpr.k:230-235`: `S3_size / 8 == S3_size >> 3`.
        assert_eq!(int_of(BinOp::Div, 40, 8), int_of(BinOp::ShiftRight, 40, 3));
        // A negative shift count is `overflowError` in Haskell — the
        // reference crashes on `(3 << -1) == 1` (`basics.k::4,5`: exit 1, no
        // `Error` line) — so folding stops rather than crashing; so does a
        // count past any width the later checks accept.
        assert!(unfolded(BinOp::ShiftLeft, &int(1), &int(-1)));
        assert!(unfolded(BinOp::ShiftRight, &int(3), &int(-1)));
        assert!(unfolded(BinOp::ShiftLeft, &int(1), &int(i128::from(MAX_FOLDED_SHIFT) + 1)));
    }

    #[test]
    fn relational_operators_fold_to_booleans() {
        for (op, x, y, expected) in [
            (BinOp::Equal, 1, 1, true),
            (BinOp::NotEqual, 1, 1, false),
            (BinOp::Greater, 2, 1, true),
            (BinOp::GreaterEqual, 1, 1, true),
            (BinOp::Less, 1, 2, true),
            (BinOp::LessEqual, 2, 1, false),
        ] {
            assert_eq!(folds(op, &int(x), &int(y)), ConstValue::Bool(expected), "{x} {op:?} {y}");
        }
    }

    /// `evalBinary (IntLiteral _) (IntLiteral _)` has no case for `&&`/`||`/`^^`
    /// and returns `e` — it does **not** fall through to the mixed clauses.
    /// `test/syntax/constexpr.k:934-939` (`uint<1 && 1 && 1>`) is `expected:1`
    /// precisely because of this.
    #[test]
    fn logical_operators_do_not_fold_on_integers() {
        for op in [BinOp::And, BinOp::Or, BinOp::Xor] {
            assert!(unfolded(op, &int(1), &int(1)), "{op:?}");
        }
    }

    /// `Constexpr.hs:204-210` — and `Xor` is spelled `(x || y) && not (x && y)`.
    #[test]
    fn boolean_operators_fold() {
        let (t, f) = (ConstValue::Bool(true), ConstValue::Bool(false));
        assert_eq!(folds(BinOp::And, &t, &f), ConstValue::Bool(false));
        assert_eq!(folds(BinOp::Or, &t, &f), ConstValue::Bool(true));
        assert_eq!(folds(BinOp::Xor, &t, &f), ConstValue::Bool(true));
        assert_eq!(folds(BinOp::Xor, &t, &t), ConstValue::Bool(false));
        assert_eq!(folds(BinOp::Equal, &t, &t), ConstValue::Bool(true));
        assert_eq!(folds(BinOp::NotEqual, &t, &t), ConstValue::Bool(false));
        // No rule for `<` on booleans (`Constexpr.hs:210`).
        assert!(unfolded(BinOp::Less, &t, &f));
    }

    /// `Constexpr.hs:212-215`: only `==` and `!=`.
    #[test]
    fn string_comparison_folds() {
        let (a, b) = (ConstValue::Str("a".into()), ConstValue::Str("b".into()));
        assert_eq!(folds(BinOp::Equal, &a, &a), ConstValue::Bool(true));
        assert_eq!(folds(BinOp::NotEqual, &a, &b), ConstValue::Bool(true));
        assert!(unfolded(BinOp::Less, &a, &b));
    }

    /// `evalBinary (EnumValue _ x) y = evalBinary x y` (`Constexpr.hs:174-175`).
    #[test]
    fn an_enum_value_folds_as_its_integer() {
        let e = ConstValue::Enum { name: kg_base::SymbolPath::empty(), value: ConstInt::from_i128(3) };
        assert_eq!(folds(BinOp::Add, &e, &int(1)), int(4));
    }

    /// Truncation into a cast's target (`Constexpr.hs:329-333`).
    #[test]
    fn a_cast_target_truncates_an_integer_and_not_a_float() {
        assert_eq!(CastTarget::Unsigned(8).truncate(&ConstInt::from_i128(300)), Some(ConstInt::from_i128(44)));
        assert_eq!(CastTarget::Signed(8).truncate(&ConstInt::from_i128(200)), Some(ConstInt::from_i128(-56)));
        assert_eq!(CastTarget::Float.truncate(&ConstInt::from_i128(1)), None);
        assert_eq!(CastTarget::Unsigned(9).literal_type(), Some(LiteralType::unsigned(9)));
        assert_eq!((CastTarget::Float.width(), CastTarget::Bool.width()), (32, 1));
    }
}
