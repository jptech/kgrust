//! Owner-topic: **the bit-width algebra** — `tWidth`, `tOffsetOf`, and the whole
//! per-operator width table that gives every arithmetic expression its type.
//!
//! Spec: `docs/spec/types.md` §1.6 (`tWidth`), §1.7 (`clog2`), §2 (the algebra:
//! §2.1 dispatch order, §2.2 the generic table, §2.3 the literal-specialised
//! rules, §2.4 unary, §2.7 the C++ cross-check).
//! Reference: `Type.hs:692-730` (widths), `Type.hs:740-926` (the algebra).
//!
//! # Why this file and not a separate `algebra` module
//!
//! `docs/spec/types.md` §14.4 suggests splitting `kg_ty::width` from
//! `kg_ty::algebra`. They are merged here because they are the same subject —
//! every entry in the operator table is a statement about widths — and because a
//! split invites two owners to disagree about where `tWidenInt` lives.
//!
//! # Traps that live in this file
//!
//! * `a - b` on two **unsigned** operands is **signed** (`Type.hs:861`) — trap 1.
//! * `a << b` widens by `2^width(b) - 1`, and `width(b) >= 20` is a hard error
//!   (`Type.hs:894-897`) — trap 2.
//! * `uint & uint` narrows to the **min** of the widths in the front-end but the
//!   **max** in the C++ middle-end (`Type.hs:882-883` vs
//!   `cpp/parse_tree.cpp:3256-3260`) — trap 3. The front-end type wins.
//! * `tWidth TString = Just 0` and `tWidth TReference = Just 1`
//!   (`Type.hs:705`, `Type.hs:710`) — trap 9.
//! * `tWidth (TUnion _ [])` is `maximum []` and **throws** (`Type.hs:717`) — trap
//!   10. Reproduced here as `None`; see [`t_width`].
//! * Comparison operators are `TBoolean` with **no** operand check in the
//!   front-end (`Type.hs:790-795`) — trap 30. Diagnosing here would fail the 241
//!   backend-code conformance assertions.

use kg_base::{Interner, SmolStr};
use kg_const::{BinOp, ConstInt, ConstValue, LiteralType, UnOp};

use crate::subtype::t_super_pretty;
use crate::ty::{Name, Ty, TyRef};

/// `c_maxRegisterBitWidth` (`compiler/cpp/ir.h:39`). Enforced by the C++
/// middle-end in `BinaryOpNode::TypeCheck` (`cpp/parse_tree.cpp:3218-3225`), not
/// by the Haskell front-end — so **do not** check it here.
pub const MAX_REGISTER_BIT_WIDTH: u64 = 1024 * 1024;

/// `tWidth` (`Type.hs:695-723`).
///
/// Widths are `u64` rather than the reference's `Int` because
/// `TArray _ x dims` multiplies `product dims` by the element width and array
/// dimensions are arbitrary precision (`[Integer]`, `Type.hs:261`). An overflow
/// of `u64` yields `None`; the reference would wrap a 64-bit `Int`, but any such
/// value is many orders of magnitude past [`MAX_REGISTER_BIT_WIDTH`] and is
/// rejected downstream either way.
///
/// **Empty union**: the reference calls `maximum []` and throws
/// (`Type.hs:717`, trap 10). This returns `None`. Whether an empty union is even
/// syntactically reachable is `docs/spec/types.md` open question 2.
pub fn t_width(ty: &Ty) -> Option<u64> {
    match ty {
        Ty::Auto | Ty::Template | Ty::Initializer(_) | Ty::Void => None,
        Ty::Dependent(_) | Ty::Function { .. } | Ty::Class(..) => None,
        Ty::Error(_) | Ty::Undefined | Ty::Unresolved | Ty::Untyped => None,
        Ty::Boolean => Some(1),
        Ty::Float => Some(32),
        // Trap 9: `string` is zero-width and `this&` is one bit.
        Ty::Str => Some(0),
        Ty::Reference(_) => Some(1),
        Ty::Signed(n) | Ty::Unsigned(n) => Some(u64::from(*n)),
        Ty::Type(x) | Ty::Designator(_, x) | Ty::Positional(_, x) => t_width(x),
        Ty::Const(x) | Ty::Instance { concrete: x, .. } => t_width(x),
        Ty::Closure { capture, .. } => t_width(capture),
        Ty::Enum(_, base) => t_width(base),
        Ty::Array { elem, dims, .. } => {
            if dims.is_empty() {
                return None;
            }
            let elem = t_width(elem)?;
            let mut total: u64 = elem;
            for d in dims {
                let d = d.to_u64()?;
                total = total.checked_mul(d)?;
            }
            Some(total)
        }
        Ty::Struct(_, fs) => t_sum_widths(fs),
        // `maximum $ fmap (tWidth . snd) x` (`Type.hs:717`) is `maximum` over a
        // list of `Maybe Int`, and Haskell's `Ord (Maybe a)` puts `Nothing`
        // **below** every `Just`. So a width-less field does *not* poison the
        // union: the widest `Just` wins, and the result is `Nothing` only when
        // every field is width-less. (`maximum []` throws — trap 10.)
        Ty::Union(_, fs) => fs.iter().filter_map(|(_, t)| t_width(t)).max(),
    }
}

/// `tSumWidths` (`Type.hs:692-693`): `Nothing` if any field has no width.
pub fn t_sum_widths(fields: &[(Name, TyRef)]) -> Option<u64> {
    let mut total: u64 = 0;
    for (_, t) in fields {
        total = total.checked_add(t_width(t)?)?;
    }
    Some(total)
}

/// `tOffsetOf` (`Type.hs:726-730`): the bit offset of a struct field, i.e. the
/// summed widths of every preceding field. Only structs have offsets.
pub fn t_offset_of(ty: &Ty, field: Name) -> Option<u64> {
    match ty {
        Ty::Instance { concrete, .. } => t_offset_of(concrete, field),
        Ty::Struct(_, fs) => {
            let n = fs.iter().position(|(name, _)| *name == field)?;
            t_sum_widths(&fs[..n])
        }
        _ => None,
    }
}

/// `typeOfOperand` (`Type.hs:732-738`): strips `TEnum`, `TInstance` and `TConst`
/// **recursively**, so enums arithmetic as their base type and `const`ness is
/// erased. Every entry in the operator table below is stated over stripped types.
pub fn type_of_operand(ty: &Ty) -> &Ty {
    match ty {
        Ty::Enum(_, x) | Ty::Instance { concrete: x, .. } | Ty::Const(x) => type_of_operand(x),
        x => x,
    }
}

/// `tWidenInt` (`Type.hs:801-803`).
///
/// `n` is **signed**, because the reference's is: `tWidenInt :: Type -> Int ->
/// Type`, and `tShiftLLiteral` feeds it `fromIntegralConstexpr` of the shift
/// count, which the parser and folder happily produce as a negative number
/// (`test/syntax/basics.k:47-60` compiles `3 << -1`). A negative `n` therefore
/// *narrows*.
///
/// Deviation: Haskell can represent `TUnsigned (-1)`; [`Ty::Unsigned`] holds a
/// `u32`, so the width is clamped to `0` instead. Reachable only for
/// `x << n` with `n < -width(x)`, which is nonsense on both sides.
pub fn t_widen_int(ty: &Ty, n: i64, names: Option<&Interner>) -> Ty {
    match ty {
        Ty::Unsigned(x) => Ty::Unsigned(offset_width(*x, n)),
        Ty::Signed(x) => Ty::Signed(offset_width(*x, n)),
        _ => expected_integer_error(ty, names),
    }
}

/// `tNarrowInt` (`Type.hs:805-807`). Note the asymmetric floors: unsigned clamps
/// at **1** bit, signed at **2**. `n` is signed for the same reason as
/// [`t_widen_int`]'s.
pub fn t_narrow_int(ty: &Ty, n: i64, names: Option<&Interner>) -> Ty {
    match ty {
        Ty::Unsigned(x) => Ty::Unsigned(offset_width(*x, n.saturating_neg()).max(1)),
        Ty::Signed(x) => Ty::Signed(offset_width(*x, n.saturating_neg()).max(2)),
        _ => expected_integer_error(ty, names),
    }
}

/// `x + n` over widths, clamped into `u32`. `i64::from(u32)` cannot overflow the
/// addition, so the only clamping is at the two ends.
fn offset_width(x: u32, n: i64) -> u32 {
    (i64::from(x) + n).clamp(0, i64::from(u32::MAX)) as u32
}

/// `fromIntegralConstexpr y :: Maybe Int` for a shift count (`Type.hs:775`,
/// `Type.hs:843-850`).
///
/// Haskell's `fromIntegral` wraps modulo 2^64 for an out-of-range `Integer`;
/// this saturates instead. Any literal outside `i64` is astronomically past
/// [`MAX_REGISTER_BIT_WIDTH`] and is rejected downstream either way, and
/// saturating keeps the *sign* — which is the only thing the rule branches on.
fn shift_count(n: &ConstInt) -> i64 {
    match n.to_i128() {
        Some(v) => v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
        None if n.is_negative() => i64::MIN,
        None => i64::MAX,
    }
}

/// ``TError "Invalid operand, expected an integer: `x`"`` (`Type.hs:546-547`).
///
/// The reference embeds `showPretty` of the operand, and so does this — the
/// message is what a user reads, and `cargo xtask errors` grades it against
/// the reference's. `names` is the interner a nominal type renders through;
/// without one such a name prints as `?` (`show_pretty`), which is why every
/// caller that has an interner passes it.
pub fn expected_integer_error(ty: &Ty, names: Option<&Interner>) -> Ty {
    Ty::Error(SmolStr::new(format!(
        "Invalid operand, expected an integer: `{}`",
        crate::subtype::show_pretty(ty, names)
    )))
}

/// ``TError "Invalid operands to binary expression: `x` and `y`"``
/// (`Type.hs:543-544`).
pub fn invalid_operands_error(lhs: &Ty, rhs: &Ty, names: Option<&Interner>) -> Ty {
    Ty::Error(SmolStr::new(format!(
        "Invalid operands to binary expression: `{}` and `{}`",
        crate::subtype::show_pretty(lhs, names),
        crate::subtype::show_pretty(rhs, names)
    )))
}

/// `inferIntLiteralType` (`Type.hs:485-491`) — the type of an integer literal.
///
/// ```text
/// Just (LiteralType SignedLiteral w)   -> TSigned w
/// Just (LiteralType UnsignedLiteral w) -> TUnsigned w
/// Nothing, x == 0 -> TUnsigned 1
/// Nothing, x <  0 -> TSigned  (1 + clog2 (abs x))
/// Nothing, x >  0 -> TUnsigned (clog2 (x + 1))
/// ```
///
/// Worked values validated against `test/syntax/literal-operand.k:69-79` and
/// `test/syntax/decltype.k:21-30`: `0 → uint1`, `2 → uint2`, `4 → uint3`,
/// `10 → uint4`, `-16 → int5`, `-17 → int6`, `-200 → int9`.
///
/// The rule is the value domain's, [`ConstValue::literal_type`], which the
/// folder needs without a `Ty` (`kg-ty` depends on `kg-const`, never the
/// reverse); this is its image in `Ty` (`single-source-of-truth.md` D1.6).
pub fn infer_int_literal_type(value: &ConstValue) -> Ty {
    match value.literal_type() {
        Some(lit) => literal_ty(lit),
        None => Ty::Error(SmolStr::new_static("Expected an integer literal")),
    }
}

/// The `Nothing` branch of [`infer_int_literal_type`], usable directly on a
/// value — [`LiteralType::minimal`]'s. `TypeCheck.hs:154` checks conversions
/// against *this* type rather than the suffixed one — trap 19, and the reason
/// constant folding silences narrowing warnings.
pub fn minimal_int_type(value: &ConstInt) -> Ty {
    literal_ty(LiteralType::minimal(value))
}

/// `TSigned w` / `TUnsigned w` of a literal's stamp.
fn literal_ty(lit: LiteralType) -> Ty {
    if lit.is_signed() { Ty::Signed(lit.width) } else { Ty::Unsigned(lit.width) }
}

// --- unary rules --------------------------------------------------------------

/// `tInvert` (`Type.hs:561-563`): `~` keeps the operand type, or errors.
pub fn t_invert(ty: &Ty, names: Option<&Interner>) -> Ty {
    if ty.is_int() { ty.clone() } else { expected_integer_error(ty, names) }
}

/// `tNeg` (`Type.hs:565-568`): unary `-` is **always signed and one bit wider**,
/// for signed operands as well as unsigned ones.
pub fn t_neg(ty: &Ty) -> Ty {
    match ty {
        Ty::Float => Ty::Float,
        Ty::Signed(x) | Ty::Unsigned(x) => Ty::Signed(x.saturating_add(1)),
        _ => Ty::Error(SmolStr::new_static("Expected an integer or a float operand")),
    }
}

/// `tNot` (`Type.hs:570-571`).
pub fn t_not(ty: &Ty) -> Ty {
    match ty {
        Ty::Boolean => Ty::Boolean,
        _ => Ty::Error(SmolStr::new_static("Expected a boolean operand")),
    }
}

// --- the generic binary table -------------------------------------------------

/// `tAdd` (`Type.hs:854-859`): `max(Wa,Wb) + 1`, mixed signedness promoted
/// through `tSigned` first.
fn t_add(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    match (x, y) {
        (Ty::Unsigned(a), Ty::Unsigned(b)) => Ty::Unsigned(a.max(b).saturating_add(1)),
        (Ty::Signed(a), Ty::Signed(b)) => Ty::Signed(a.max(b).saturating_add(1)),
        (Ty::Unsigned(_), Ty::Signed(_)) => t_add(&x.to_signed(), y, names),
        (Ty::Signed(_), Ty::Unsigned(_)) => t_add(x, &y.to_signed(), names),
        (Ty::Float, Ty::Float) => Ty::Float,
        _ => invalid_operands_error(x, y, names),
    }
}

/// `tSub` (`Type.hs:861-862`) — **trap 1**. `uint - uint` is `int(1 + max)`, not
/// `uint`: the one clause that differs from [`t_add`] makes every subtraction
/// signed. `test/syntax/basics.k:16-26` asserts `bitsizeof(uint8 - uint8) == 9`.
fn t_sub(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    match (x, y) {
        (Ty::Unsigned(a), Ty::Unsigned(b)) => Ty::Signed(a.max(b).saturating_add(1)),
        _ => t_add(x, y, names),
    }
}

/// `tMul` (`Type.hs:864-869`): widths **add**, and the result is signed if either
/// operand is — with no promotion widening (`int25 * uint37` is `int62`, not
/// `int63`; `test/syntax/basics.k:188-195`).
fn t_mul(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    match (x, y) {
        (Ty::Unsigned(a), Ty::Unsigned(b)) => Ty::Unsigned(a.saturating_add(*b)),
        (Ty::Signed(a), Ty::Unsigned(b))
        | (Ty::Unsigned(a), Ty::Signed(b))
        | (Ty::Signed(a), Ty::Signed(b)) => Ty::Signed(a.saturating_add(*b)),
        (Ty::Float, Ty::Float) => Ty::Float,
        _ => invalid_operands_error(x, y, names),
    }
}

/// `tDiv` (`Type.hs:871-872`): the **entire left type**, width and signedness
/// unchanged. Note the guard is `tIsInt` on both sides, so `float / float` is an
/// error here even though `float * float` is not.
fn t_div(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    if x.is_int() && y.is_int() { x.clone() } else { invalid_operands_error(x, y, names) }
}

/// `tMod` (`Type.hs:874-876`): the **right** operand's width with the **left**
/// operand's signedness.
fn t_mod(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    let width = || t_width(y).unwrap_or(0).try_into().unwrap_or(u32::MAX);
    match x {
        Ty::Unsigned(_) if y.is_int() => Ty::Unsigned(width()),
        Ty::Signed(_) if y.is_int() => Ty::Signed(width()),
        _ => invalid_operands_error(x, y, names),
    }
}

/// `tBitwiseOr` (`Type.hs:878-880`). `BitwiseXor` routes here too
/// (`Type.hs:786-787`) — `^` and `|` have the same result type.
fn t_bitwise_or(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    if x.is_int() && y.is_int() { t_super_pretty(x, y, names) } else { invalid_operands_error(x, y, names) }
}

/// `tBitwiseAnd` (`Type.hs:882-886`) — **trap 3**. Two unsigned operands narrow
/// to the **min** of the widths; every other integer pair takes `tSuper`, which
/// is the **max**. (The C++ middle-end uses the max in both cases,
/// `cpp/parse_tree.cpp:3256-3260`; the front-end type is the one that counts.)
fn t_bitwise_and(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    match (x, y) {
        (Ty::Unsigned(a), Ty::Unsigned(b)) => Ty::Unsigned(*a.min(b)),
        _ if x.is_int() && y.is_int() => t_super_pretty(x, y, names),
        _ => invalid_operands_error(x, y, names),
    }
}

/// `tLogical` (`Type.hs:888-889`) — `&&`, `||`, `^^`.
fn t_logical(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    match (x, y) {
        (Ty::Boolean, Ty::Boolean) => Ty::Boolean,
        _ => invalid_operands_error(x, y, names),
    }
}

/// `tShiftR` (`Type.hs:891-892`): the left type, unchanged.
fn t_shift_r(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    if x.is_int() && y.is_int() { x.clone() } else { invalid_operands_error(x, y, names) }
}

/// `tShiftL` (`Type.hs:894-897`) — **trap 2**. The result must hold *every*
/// value the shift count can name, so the left type widens by
/// `2^width(rhs) - 1`, and a shift count 20 bits wide or wider is a hard error.
///
/// `test/syntax/basics.k:4-15`: `uint32 << int5` is `uint63` — `32 + 2^5 - 1`.
/// The count's *signedness is ignored*; that test's comment says the amount is
/// "reinterpreted as unsigned".
fn t_shift_l(x: &Ty, y: &Ty, names: Option<&Interner>) -> Ty {
    if !(x.is_int() && y.is_int()) {
        return invalid_operands_error(x, y, names);
    }
    let wy = t_width(y).unwrap_or(0);
    if wy >= 20 {
        return Ty::Error(SmolStr::new_static(
            "Invalid shift left. Result type exceeds maximum bit width.",
        ));
    }
    t_widen_int(x, (1i64 << wy) - 1, names)
}

/// The result type of a binary operator — `tBinaryOp` (`Type.hs:775-798`) plus
/// the individual `tAdd`/`tSub`/… at `Type.hs:854-897`.
///
/// Both operands are run through [`type_of_operand`] first, exactly as
/// `Type.hs:777` does (`result op (typeOfOperand x) (typeOfOperand y)`). That
/// strip is idempotent, so it is harmless if the caller already applied it — and
/// it is what makes the whole table below a statement about bare
/// [`Ty::Signed`]/[`Ty::Unsigned`]/[`Ty::Float`]/[`Ty::Boolean`].
///
/// The complete table is `docs/spec/types.md` §2.2. Callers reach this only after
/// the two dispatch guards of `infer (BinaryF …)` (`Type.hs:1182-1188`, §2.1):
/// both-literal operands fold instead, and an unresolved operand yields
/// [`Ty::Unresolved`]; and after [`binary_literal_guard`] has failed to select a
/// literal-specialised rule (`Type.hs:757-773`).
pub fn t_binary_op(op: BinOp, lhs: &Ty, rhs: &Ty, names: Option<&Interner>) -> Ty {
    let (lhs, rhs) = (type_of_operand(lhs), type_of_operand(rhs));
    match op {
        BinOp::Add => t_add(lhs, rhs, names),
        BinOp::Sub => t_sub(lhs, rhs, names),
        // `result LutMul = tMul` (`Type.hs:781-782`).
        BinOp::Mul | BinOp::LutMul => t_mul(lhs, rhs, names),
        BinOp::Div => t_div(lhs, rhs, names),
        BinOp::Mod => t_mod(lhs, rhs, names),
        BinOp::BitwiseAnd => t_bitwise_and(lhs, rhs, names),
        // `result BitwiseXor = tBitwiseOr` (`Type.hs:786-787`).
        BinOp::BitwiseOr | BinOp::BitwiseXor => t_bitwise_or(lhs, rhs, names),
        BinOp::ShiftRight => t_shift_r(lhs, rhs, names),
        BinOp::ShiftLeft => t_shift_l(lhs, rhs, names),
        // Trap 30: the six comparisons are `const $ const TBoolean`
        // (`Type.hs:790-795`) — the front-end performs **no** operand check.
        // Diagnosing here would reject programs the reference accepts.
        BinOp::Equal
        | BinOp::NotEqual
        | BinOp::Greater
        | BinOp::GreaterEqual
        | BinOp::Less
        | BinOp::LessEqual => Ty::Boolean,
        BinOp::And | BinOp::Or | BinOp::Xor => t_logical(lhs, rhs, names),
    }
}

// --- the literal-specialised binary table -------------------------------------

/// Which operand `inferBinaryOp` will accept as *the* literal, and how strict it
/// is about the suffix (`Type.hs:757-773`; `docs/spec/types.md` §2.3).
///
/// The caller answers the syntactic question — `isIntLiteral` /
/// `isUntypedIntLiteral` are predicates on the **node** (`Type.hs:1087-1095`),
/// not on a type — and then calls [`t_binary_op_literal`] with the *other*
/// operand's stripped type.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum LiteralGuard {
    /// `&` and `*` (`Type.hs:759-764`): the literal may be **either** operand —
    /// the right is tried first — and it must be `isUntypedIntLiteral`.
    EitherUntyped,
    /// `/`, `%`, `<<`, `>>` (`Type.hs:765-772`): only the **right** operand, and
    /// `isIntLiteral` is enough, so a suffixed literal specialises too.
    RightAny,
}

/// [`LiteralGuard`] for an operator, or `None` when the operator has no
/// literal-specialised rule at all (`Type.hs:757-773`).
pub fn binary_literal_guard(op: BinOp) -> Option<LiteralGuard> {
    Some(match op {
        BinOp::BitwiseAnd | BinOp::Mul => LiteralGuard::EitherUntyped,
        BinOp::Div | BinOp::Mod | BinOp::ShiftLeft | BinOp::ShiftRight => LiteralGuard::RightAny,
        _ => return None,
    })
}

/// `tAndLiteral` (`Type.hs:821-829`): `x & n` where `n` is a literal.
///
/// Both signs of `n` matter and the unsigned and signed rows differ in shape —
/// the unsigned row **clamps** at `X` while the signed positive row does not and
/// can therefore *widen*.
fn t_and_literal(x: &Ty, n: &ConstInt, names: Option<&Interner>) -> Ty {
    match x {
        Ty::Unsigned(w) => {
            if n.is_zero() {
                Ty::Unsigned(1)
            } else if n.is_negative() {
                Ty::Unsigned(*w)
            } else {
                // min x (clog2 (n + 1))
                Ty::Unsigned((*w).min(n.add(&ConstInt::ONE).clog2()))
            }
        }
        Ty::Signed(w) => {
            if n.is_zero() {
                Ty::Signed(2)
            } else if n.is_negative() {
                // max x (1 + clog2 (1 - n))
                Ty::Signed((*w).max(ConstInt::ONE.sub(n).clog2().saturating_add(1)))
            } else {
                // 1 + clog2 (n + 1) — deliberately *not* clamped by `x`.
                Ty::Signed(n.add(&ConstInt::ONE).clog2().saturating_add(1))
            }
        }
        _ => expected_integer_error(x, names),
    }
}

/// `tMulLiteral` (`Type.hs:831-835`): `x * n`.
///
/// `x * 1` keeps the width exactly, because `clog2 1 == 0`
/// (`test/syntax/literal-operand.k:70-80` asserts `decltype(a * 1) == uint32`).
fn t_mul_literal(x: &Ty, n: &ConstInt, names: Option<&Interner>) -> Ty {
    if !x.is_int() {
        expected_integer_error(x, names)
    } else if n.is_zero() {
        if x.is_signed() { Ty::Signed(2) } else { Ty::Unsigned(1) }
    } else if n.is_negative() {
        // `tMulLiteral (tNeg x) (abs n)` — one recursion, and `tNeg` of an
        // integer is always signed, so the next call takes the positive branch.
        t_mul_literal(&t_neg(x), &n.abs(), names)
    } else {
        t_widen_int(x, i64::from(n.clog2()), names)
    }
}

/// `tDivLiteral` (`Type.hs:837-841`): `x / n`. Unlike [`t_mod_literal`], the
/// integer check comes **first**, so `float / 0` is the expected-integer error
/// rather than the division-by-zero one.
fn t_div_literal(x: &Ty, n: &ConstInt, names: Option<&Interner>) -> Ty {
    if !x.is_int() {
        expected_integer_error(x, names)
    } else if n.is_zero() {
        Ty::Error(SmolStr::new_static("Division by zero is undefined"))
    } else if n.is_negative() {
        Ty::Error(SmolStr::new_static("Division with negative denominator is undefined"))
    } else {
        t_narrow_int(x, i64::from(n.clog2()), names)
    }
}

/// `tModLiteral` (`Type.hs:810-819`): `x % n`.
///
/// Clause order is load-bearing and **opposite** to [`t_div_literal`]'s: the
/// `n == 0` / `n < 0` guards are written on a wildcard type (`Type.hs:811-813`)
/// and so fire before the integer check.
fn t_mod_literal(x: &Ty, n: &ConstInt, names: Option<&Interner>) -> Ty {
    if n.is_zero() {
        return Ty::Error(SmolStr::new_static("Remainder by zero is undefined"));
    }
    if n.is_negative() {
        return Ty::Error(SmolStr::new_static(
            "Remainder with negative denominator is undefined",
        ));
    }
    let one = *n == ConstInt::ONE;
    match x {
        // `n == 1` is special-cased on both rows: `x % 1` is always 0, so one
        // bit (two, signed — the `tNarrowInt` floor) is enough.
        Ty::Unsigned(_) if one => Ty::Unsigned(1),
        Ty::Signed(_) if one => Ty::Signed(2),
        Ty::Unsigned(w) => Ty::Unsigned((*w).min(n.clog2())),
        Ty::Signed(w) => Ty::Signed((*w).min(n.clog2().saturating_add(1))),
        _ => expected_integer_error(x, names),
    }
}

/// `inferBinaryOp op a b` (`Type.hs:756-773`) over two operands the fold has
/// already reduced to literals — the stamp `typedIntLiteral $ inferBinaryOp
/// op a b` a fold with a suffixed operand carries (`Constexpr.hs:200-202`).
/// Each operand is typed as `inferIntLiteralType` types it, and the literal
/// guards pick the specialised rule exactly as they do on nodes: `0u16 <<
/// 16` is `tShiftLLiteral`, `uint32`. `None` for a non-integer operand.
pub fn infer_binary_op_values(op: BinOp, x: &ConstValue, y: &ConstValue, names: Option<&Interner>) -> Option<Ty> {
    let (xv, yv) = (x.as_int()?, y.as_int()?);
    let (tx, ty) = (infer_int_literal_type(x), infer_int_literal_type(y));
    Some(match binary_literal_guard(op) {
        Some(LiteralGuard::EitherUntyped) if is_untyped_int_literal(y) => t_binary_op_literal(op, &tx, yv, names)?,
        Some(LiteralGuard::EitherUntyped) if is_untyped_int_literal(x) => t_binary_op_literal(op, &ty, xv, names)?,
        Some(LiteralGuard::RightAny) => t_binary_op_literal(op, &tx, yv, names)?,
        _ => t_binary_op(op, &tx, &ty, names),
    })
}

/// The literal-specialised binary rules — `tAndLiteral`, `tMulLiteral`,
/// `tDivLiteral`, `tModLiteral`, `tShiftLLiteral`, `tShiftRLiteral`
/// (`Type.hs:810-850`), selected by the guards at `Type.hs:757-773`.
///
/// `operand` is the **non-literal** operand's type — stripped here through
/// [`type_of_operand`], as `tBinaryOpLiteral` does (`Type.hs:775`) — and
/// `literal` is the literal's value. `None` means "no literal rule applies for
/// this operator; fall through to [`t_binary_op`]"; see [`binary_literal_guard`]
/// for which operand the caller must have checked.
pub fn t_binary_op_literal(op: BinOp, operand: &Ty, literal: &ConstInt, names: Option<&Interner>) -> Option<Ty> {
    let operand = type_of_operand(operand);
    Some(match op {
        BinOp::BitwiseAnd => t_and_literal(operand, literal, names),
        BinOp::Mul => t_mul_literal(operand, literal, names),
        BinOp::Div => t_div_literal(operand, literal, names),
        BinOp::Mod => t_mod_literal(operand, literal, names),
        // `tShiftLLiteral` widens by exactly `n` — **not** by `2^Wy - 1` the way
        // the generic `tShiftL` does (`Type.hs:843-850` vs `Type.hs:894-897`).
        BinOp::ShiftLeft => {
            if operand.is_int() {
                t_widen_int(operand, shift_count(literal), names)
            } else {
                expected_integer_error(operand, names)
            }
        }
        BinOp::ShiftRight => {
            if operand.is_int() {
                t_narrow_int(operand, shift_count(literal), names)
            } else {
                expected_integer_error(operand, names)
            }
        }
        _ => return None,
    })
}

// --- unary dispatch -----------------------------------------------------------

/// `isUntypedIntLiteral` (`Type.hs:1092-1095`) lifted onto a folded value.
///
/// `Type.hs:1093` recurses through `EnumValue` into the literal it wraps.
/// [`ConstValue::Enum`] does not carry that literal's suffix, so this answers
/// `true` for every enum constant. That is right for every enum the corpus
/// contains: `test/syntax/literal-operand.k:123-128` asserts
/// `decltype((0 + Num::Four) + ((Num::Seven * 1) & Num::Three)) == decltype(7)`,
/// and the folded result is only *untyped* — hence `uint3` rather than a
/// suffixed width — if `Num`'s constants are untyped literals
/// (`Constexpr.hs:200-202`).
pub fn is_untyped_int_literal(value: &ConstValue) -> bool {
    matches!(value, ConstValue::Int { ty: None, .. } | ConstValue::Enum { .. })
}

/// `inferUnaryOp` (`Type.hs:740-754`), table in `docs/spec/types.md` §2.4.
///
/// `operand` is stripped here through [`type_of_operand`], as the reference's
/// `typeOfOperand x` does (`Type.hs:743`); the strip is idempotent.
///
/// `literal` is `Some` when the operand node is an integer literal
/// (`isIntLiteral`, `Type.hs:1087-1090` — which sees through `EnumValue`). That
/// changes the rule for `~` (an *untyped* literal inverts through `tSigned`,
/// widening `uintW` to `int(W+1)`) and for `-` (the result is `tSuper` of the
/// operand type and the **negated** value's minimal type).
///
/// `bitsizeof`, `bytesizeof` and `clog2` are all [`Ty::Unresolved`] here: they
/// are answered by constant folding, not by the type algebra
/// (`Constexpr.hs:271-275`).
pub fn infer_unary_op(op: UnOp, operand: &Ty, literal: Option<&ConstValue>, names: Option<&Interner>) -> Ty {
    let operand = type_of_operand(operand);
    match op {
        UnOp::Invert => {
            if literal.is_some_and(is_untyped_int_literal) {
                t_invert(&operand.to_signed(), names)
            } else {
                t_invert(operand, names)
            }
        }
        UnOp::Neg => match literal.and_then(ConstValue::as_int) {
            Some(v) => t_super_pretty(operand, &minimal_int_type(&v.neg()), names),
            None => t_neg(operand),
        },
        UnOp::Not => t_not(operand),
        UnOp::BitSizeof | UnOp::ByteSizeof | UnOp::Clog2 => Ty::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::{TAttrSet, TyRef};
    use kg_base::{Interner, SymbolPath};
    use kg_const::LiteralType;

    fn r(t: Ty) -> TyRef {
        TyRef::new(t)
    }

    /// `inferBinaryOp` over folded literals picks the literal rule the node
    /// guards pick (`Type.hs:757-773`): the right operand of `<<` is the
    /// literal (`tShiftLLiteral`, widening by the count, not `2^w - 1`); an
    /// untyped operand of `&` is (`tAndLiteral`, `uint5 & 0` is `uint1`).
    #[test]
    fn a_binary_over_folded_literals_is_typed_by_the_literal_rules() {
        let u = |w, v: i64| ConstValue::Int { ty: Some(LiteralType::unsigned(w)), value: ConstInt::from_i128(i128::from(v)) };
        let bare = |v: i64| ConstValue::Int { ty: None, value: ConstInt::from_i128(i128::from(v)) };
        assert_eq!(infer_binary_op_values(BinOp::ShiftLeft, &u(16, 0), &bare(16), None), Some(Ty::Unsigned(32)));
        assert_eq!(infer_binary_op_values(BinOp::BitwiseAnd, &u(5, 0), &bare(0), None), Some(Ty::Unsigned(1)));
        assert_eq!(infer_binary_op_values(BinOp::Add, &u(8, 1), &bare(1), None), Some(Ty::Unsigned(9)));
        assert_eq!(infer_binary_op_values(BinOp::Add, &ConstValue::Bool(true), &bare(1), None), None);
    }

    #[test]
    fn string_is_zero_wide_and_reference_is_one_bit() {

        // Trap 9 — Type.hs:705, Type.hs:710.
        assert_eq!(t_width(&Ty::Str), Some(0));
        assert_eq!(t_width(&Ty::Reference(SymbolPath::empty())), Some(1));
        assert_eq!(t_width(&Ty::Boolean), Some(1));
        assert_eq!(t_width(&Ty::Float), Some(32));
        assert_eq!(t_width(&Ty::Void), None);
    }

    #[test]
    fn struct_sums_and_union_maxes() {
        let i = Interner::new();
        let fields =
            vec![(i.intern("a"), r(Ty::Unsigned(8))), (i.intern("b"), r(Ty::Unsigned(24)))];
        assert_eq!(t_width(&Ty::Struct(SymbolPath::empty(), fields.clone())), Some(32));
        assert_eq!(t_width(&Ty::Union(SymbolPath::empty(), fields)), Some(24));
        // Trap 10: `maximum []` throws in the reference; `None` here.
        assert_eq!(t_width(&Ty::Union(SymbolPath::empty(), vec![])), None);
    }

    #[test]
    fn a_width_less_union_field_does_not_poison_the_union() {
        // `maximum :: [Maybe Int] -> Maybe Int` and `Nothing < Just _`, so
        // Type.hs:717 keeps the widest `Just`. A struct, by contrast, uses
        // `liftM2 (+)` (Type.hs:692-693) and *does* propagate `Nothing`.
        let i = Interner::new();
        let fields =
            vec![(i.intern("a"), r(Ty::Unsigned(8))), (i.intern("b"), r(Ty::Void))];
        assert_eq!(t_width(&Ty::Union(SymbolPath::empty(), fields.clone())), Some(8));
        assert_eq!(t_width(&Ty::Struct(SymbolPath::empty(), fields)), None);
    }

    #[test]
    fn array_width_is_product_times_element() {
        let t = Ty::Array {
            attrs: TAttrSet::new(),
            elem: r(Ty::Unsigned(8)),
            dims: vec![ConstInt::from_i128(4), ConstInt::from_i128(3)],
        };
        assert_eq!(t_width(&t), Some(96));
        let no_dims =
            Ty::Array { attrs: TAttrSet::new(), elem: r(Ty::Unsigned(8)), dims: vec![] };
        assert_eq!(t_width(&no_dims), None);
    }

    #[test]
    fn closure_width_is_the_capture_width() {
        // Type.hs:713 — not the function type's width.
        let t = Ty::Closure {
            func: SymbolPath::empty(),
            func_ty: r(Ty::Void),
            capture: r(Ty::Unsigned(12)),
            alias: SymbolPath::empty(),
        };
        assert_eq!(t_width(&t), Some(12));
    }

    #[test]
    fn offset_of_sums_preceding_fields() {
        let i = Interner::new();
        let s = Ty::Struct(
            SymbolPath::empty(),
            vec![
                (i.intern("a"), r(Ty::Unsigned(3))),
                (i.intern("b"), r(Ty::Unsigned(5))),
                (i.intern("c"), r(Ty::Boolean)),
            ],
        );
        assert_eq!(t_offset_of(&s, i.intern("a")), Some(0));
        assert_eq!(t_offset_of(&s, i.intern("b")), Some(3));
        assert_eq!(t_offset_of(&s, i.intern("c")), Some(8));
        assert_eq!(t_offset_of(&s, i.intern("z")), None);
    }

    #[test]
    fn operand_stripping_is_recursive() {
        // Type.hs:732-738.
        let t = Ty::Const(r(Ty::Enum(SymbolPath::empty(), r(Ty::Unsigned(4)))));
        assert_eq!(type_of_operand(&t), &Ty::Unsigned(4));
    }

    #[test]
    fn minimal_literal_types_match_the_corpus() {
        // docs/spec/types.md §3.1, validated against test/syntax/decltype.k:21-30.
        let c = ConstInt::from_i128;
        assert_eq!(minimal_int_type(&c(0)), Ty::Unsigned(1));
        assert_eq!(minimal_int_type(&c(1)), Ty::Unsigned(1));
        assert_eq!(minimal_int_type(&c(2)), Ty::Unsigned(2));
        assert_eq!(minimal_int_type(&c(3)), Ty::Unsigned(2));
        assert_eq!(minimal_int_type(&c(4)), Ty::Unsigned(3));
        assert_eq!(minimal_int_type(&c(10)), Ty::Unsigned(4));
        assert_eq!(minimal_int_type(&c(-16)), Ty::Signed(5));
        assert_eq!(minimal_int_type(&c(-17)), Ty::Signed(6));
        assert_eq!(minimal_int_type(&c(-20)), Ty::Signed(6));
        assert_eq!(minimal_int_type(&c(-200)), Ty::Signed(9));
    }

    #[test]
    fn narrow_int_floors_are_asymmetric() {
        // Type.hs:805-807.
        assert_eq!(t_narrow_int(&Ty::Unsigned(4), 10, None), Ty::Unsigned(1));
        assert_eq!(t_narrow_int(&Ty::Signed(4), 10, None), Ty::Signed(2));
        assert_eq!(t_widen_int(&Ty::Unsigned(4), 3, None), Ty::Unsigned(7));
        // Type.hs:801-803 with a negative count: `tWidenInt` narrows. Reachable
        // from `tShiftLLiteral` because `x << -1` parses and folds
        // (test/syntax/basics.k:47-60).
        assert_eq!(t_widen_int(&Ty::Unsigned(8), -1, None), Ty::Unsigned(7));
        assert_eq!(t_narrow_int(&Ty::Unsigned(8), -1, None), Ty::Unsigned(9));
        // Non-integer operands take the expected-integer error (Type.hs:803).
        assert!(t_widen_int(&Ty::Float, 1, None).is_error());
        assert!(t_narrow_int(&Ty::Boolean, 1, None).is_error());
    }

    // === §2.2 the generic binary table ======================================
    //
    // Every case below is transcribed from `Type.hs:854-897`, and the ones with
    // a `test/…` citation are pinned by a `static assert` in the corpus.

    fn bin(op: BinOp, a: Ty, b: Ty) -> Ty {
        t_binary_op(op, &a, &b, None)
    }

    #[test]
    fn add_is_max_plus_one() {
        // Type.hs:854-855.
        assert_eq!(bin(BinOp::Add, Ty::Unsigned(8), Ty::Unsigned(3)), Ty::Unsigned(9));
        assert_eq!(bin(BinOp::Add, Ty::Signed(8), Ty::Signed(3)), Ty::Signed(9));
        // test/syntax/basics.k:74-82 — `uint8 x; x + 1` asserts bitsizeof == 9.
        // `+` has no literal-specialised rule, so `1` types as uint1 and the
        // generic row applies.
        assert_eq!(bin(BinOp::Add, Ty::Unsigned(8), Ty::Unsigned(1)), Ty::Unsigned(9));
        // test/syntax/conversion-warning.k:118-126 — `uint32 + uint32` is
        // uint33, which is why assigning it to a `uint8` warns.
        assert_eq!(bin(BinOp::Add, Ty::Unsigned(32), Ty::Unsigned(32)), Ty::Unsigned(33));
    }

    #[test]
    fn add_promotes_the_unsigned_side_through_t_signed() {
        // Type.hs:856-857 via Type.hs:551: `uintW` becomes `int(W+1)` *before*
        // the max, so `uint8 + int3` is int9, not int9-by-accident — and
        // `uint8 + int9` is int10.
        assert_eq!(bin(BinOp::Add, Ty::Unsigned(8), Ty::Signed(3)), Ty::Signed(10));
        assert_eq!(bin(BinOp::Add, Ty::Signed(3), Ty::Unsigned(8)), Ty::Signed(10));
        assert_eq!(bin(BinOp::Add, Ty::Signed(12), Ty::Unsigned(8)), Ty::Signed(13));
    }

    #[test]
    fn sub_of_two_unsigned_is_signed() {
        // TRAP 1 — Type.hs:861. test/syntax/basics.k:16-26 asserts
        // `uint8 a, b; bitsizeof(a - b) == 9`, and the type is *signed*.
        assert_eq!(bin(BinOp::Sub, Ty::Unsigned(8), Ty::Unsigned(8)), Ty::Signed(9));
        assert_eq!(bin(BinOp::Sub, Ty::Unsigned(8), Ty::Unsigned(3)), Ty::Signed(9));
        // Every other case delegates to `tAdd` (Type.hs:862).
        // test/syntax/basics.k:63-71: `int8 x; x - 1` asserts bitsizeof == 9 —
        // `1` is uint1, promoted to int2, so `int8 - int2` is int9.
        assert_eq!(bin(BinOp::Sub, Ty::Signed(8), Ty::Unsigned(1)), Ty::Signed(9));
        assert_eq!(bin(BinOp::Sub, Ty::Signed(8), Ty::Signed(3)), Ty::Signed(9));
        assert_eq!(bin(BinOp::Sub, Ty::Float, Ty::Float), Ty::Float);
    }

    #[test]
    fn mul_adds_the_widths() {
        // Type.hs:864-867. test/syntax/basics.k:177-206 asserts all three of
        // `uint25 * uint37`, `int25 * uint37` and `int25 * int37` are 62 bits —
        // note there is *no* sign-promotion widening in the mixed row.
        assert_eq!(bin(BinOp::Mul, Ty::Unsigned(25), Ty::Unsigned(37)), Ty::Unsigned(62));
        assert_eq!(bin(BinOp::Mul, Ty::Signed(25), Ty::Unsigned(37)), Ty::Signed(62));
        assert_eq!(bin(BinOp::Mul, Ty::Unsigned(25), Ty::Signed(37)), Ty::Signed(62));
        assert_eq!(bin(BinOp::Mul, Ty::Signed(25), Ty::Signed(37)), Ty::Signed(62));
        assert_eq!(bin(BinOp::Mul, Ty::Float, Ty::Float), Ty::Float);
    }

    #[test]
    fn lutmul_has_the_same_table_as_mul() {
        // Type.hs:781-782. test/syntax/basics.k:486-508 asserts
        // `lutmul(int8, int4)` and `lutmul(uint8, uint4)` are both 12 bits.
        assert_eq!(bin(BinOp::LutMul, Ty::Signed(8), Ty::Signed(4)), Ty::Signed(12));
        assert_eq!(bin(BinOp::LutMul, Ty::Unsigned(8), Ty::Unsigned(4)), Ty::Unsigned(12));
        // test/syntax/basics.k:510-519 — `lutmul(uint8, int8)` is expected:0 in
        // the front-end; only the C++ back-end rejects lutmul of floats
        // (basics.k:459-468, expected:16), so the type must still be TFloat.
        assert_eq!(bin(BinOp::LutMul, Ty::Unsigned(8), Ty::Signed(8)), Ty::Signed(16));
        assert_eq!(bin(BinOp::LutMul, Ty::Float, Ty::Float), Ty::Float);
    }

    #[test]
    fn div_is_the_left_type_unchanged() {
        // Type.hs:871-872. Both width *and* signedness survive, and — unlike
        // `+`/`*` — floats are rejected, because the guard is `tIsInt` on both
        // sides with no TFloat clause.
        assert_eq!(bin(BinOp::Div, Ty::Unsigned(8), Ty::Unsigned(32)), Ty::Unsigned(8));
        assert_eq!(bin(BinOp::Div, Ty::Signed(8), Ty::Unsigned(3)), Ty::Signed(8));
        assert!(bin(BinOp::Div, Ty::Float, Ty::Float).is_error());
    }

    #[test]
    fn mod_takes_the_right_width_and_the_left_signedness() {
        // Type.hs:874-876.
        assert_eq!(bin(BinOp::Mod, Ty::Unsigned(32), Ty::Unsigned(5)), Ty::Unsigned(5));
        assert_eq!(bin(BinOp::Mod, Ty::Signed(32), Ty::Unsigned(5)), Ty::Signed(5));
        assert_eq!(bin(BinOp::Mod, Ty::Unsigned(3), Ty::Signed(9)), Ty::Unsigned(9));
        assert!(bin(BinOp::Mod, Ty::Boolean, Ty::Unsigned(5)).is_error());
    }

    #[test]
    fn bitwise_and_of_two_unsigned_narrows_to_the_min() {
        // TRAP 3 — Type.hs:882-883. The C++ middle-end uses the *max* here
        // (cpp/parse_tree.cpp:3256-3260); the front-end type is the one that
        // reaches the user.
        assert_eq!(bin(BinOp::BitwiseAnd, Ty::Unsigned(8), Ty::Unsigned(3)), Ty::Unsigned(3));
        // Every other integer pair takes `tSuper`, i.e. the *max*
        // (Type.hs:884-885, Type.hs:899-906).
        assert_eq!(bin(BinOp::BitwiseAnd, Ty::Signed(8), Ty::Signed(3)), Ty::Signed(8));
        assert_eq!(bin(BinOp::BitwiseAnd, Ty::Signed(8), Ty::Unsigned(9)), Ty::Signed(10));
        // test/syntax/basics.k:784-793 — `float32 & float32` is expected:1.
        assert!(bin(BinOp::BitwiseAnd, Ty::Float, Ty::Float).is_error());
    }

    #[test]
    fn bitwise_or_and_xor_are_t_super() {
        // Type.hs:878-879, with `BitwiseXor` routed to `tBitwiseOr`
        // (Type.hs:786-787) — `^` and `|` have identical result types.
        for op in [BinOp::BitwiseOr, BinOp::BitwiseXor] {
            assert_eq!(bin(op, Ty::Unsigned(8), Ty::Unsigned(3)), Ty::Unsigned(8));
            assert_eq!(bin(op, Ty::Signed(8), Ty::Signed(3)), Ty::Signed(8));
            // tSuper clause 7: `uint8` promotes to `int9`, so the lub is int9.
            assert_eq!(bin(op, Ty::Unsigned(8), Ty::Signed(3)), Ty::Signed(9));
            assert!(bin(op, Ty::Boolean, Ty::Boolean).is_error());
        }
    }

    #[test]
    fn shift_left_widens_by_two_to_the_count_width_minus_one() {
        // TRAP 2 — Type.hs:894-897. test/syntax/basics.k:4-15:
        // `int5 amt; uint32 x; bitsizeof(x << amt) == 63` — 32 + 2^5 - 1 = 63.
        // The count's *signedness is ignored*; only its width matters.
        assert_eq!(bin(BinOp::ShiftLeft, Ty::Unsigned(32), Ty::Signed(5)), Ty::Unsigned(63));
        assert_eq!(bin(BinOp::ShiftLeft, Ty::Unsigned(32), Ty::Unsigned(5)), Ty::Unsigned(63));
        // 2^1 - 1 == 1.
        assert_eq!(bin(BinOp::ShiftLeft, Ty::Signed(8), Ty::Unsigned(1)), Ty::Signed(9));
        // 2^19 - 1 is the widest accepted count (Type.hs:896).
        assert_eq!(
            bin(BinOp::ShiftLeft, Ty::Unsigned(1), Ty::Unsigned(19)),
            Ty::Unsigned(1 + (1 << 19) - 1)
        );
        assert!(bin(BinOp::ShiftLeft, Ty::Unsigned(1), Ty::Unsigned(20)).is_error());
        assert!(bin(BinOp::ShiftLeft, Ty::Float, Ty::Unsigned(2)).is_error());
    }

    #[test]
    fn shift_right_is_the_left_type_unchanged() {
        // Type.hs:891-892 — `>>` does *not* narrow in the generic rule; only
        // `tShiftRLiteral` does.
        assert_eq!(bin(BinOp::ShiftRight, Ty::Unsigned(32), Ty::Unsigned(5)), Ty::Unsigned(32));
        assert_eq!(bin(BinOp::ShiftRight, Ty::Signed(4), Ty::Signed(19)), Ty::Signed(4));
        assert!(bin(BinOp::ShiftRight, Ty::Unsigned(32), Ty::Boolean).is_error());
    }

    #[test]
    fn comparisons_are_boolean_with_no_operand_check() {
        // TRAP 30 — Type.hs:790-795 is `const $ const TBoolean`. The front-end
        // never diagnoses comparison operands; the C++ middle-end does. A Rust
        // front-end that reports an error here fails the backend-code
        // conformance assertions.
        for op in [
            BinOp::Equal,
            BinOp::NotEqual,
            BinOp::Greater,
            BinOp::GreaterEqual,
            BinOp::Less,
            BinOp::LessEqual,
        ] {
            assert_eq!(bin(op, Ty::Unsigned(8), Ty::Unsigned(8)), Ty::Boolean);
            assert_eq!(bin(op, Ty::Str, Ty::Void), Ty::Boolean);
            assert_eq!(bin(op, Ty::Float, Ty::Boolean), Ty::Boolean);
        }
    }

    #[test]
    fn logical_operators_require_two_booleans() {
        // Type.hs:796-798 route And/Or/Xor to `tLogical` (Type.hs:888-889).
        for op in [BinOp::And, BinOp::Or, BinOp::Xor] {
            assert_eq!(bin(op, Ty::Boolean, Ty::Boolean), Ty::Boolean);
            assert!(bin(op, Ty::Boolean, Ty::Unsigned(1)).is_error());
        }
    }

    // === §2.3 the literal-specialised binary table ==========================

    fn lit(op: BinOp, operand: Ty, n: i128) -> Ty {
        t_binary_op_literal(op, &operand, &ConstInt::from_i128(n), None)
            .expect("operator has a literal-specialised rule")
    }

    #[test]
    fn the_literal_guard_table_is_asymmetric() {
        // Type.hs:757-773. `&` and `*` are commutative and demand an *untyped*
        // literal; `/`, `%`, `<<`, `>>` look only at the right operand and
        // accept a suffixed one.
        assert_eq!(binary_literal_guard(BinOp::BitwiseAnd), Some(LiteralGuard::EitherUntyped));
        assert_eq!(binary_literal_guard(BinOp::Mul), Some(LiteralGuard::EitherUntyped));
        assert_eq!(binary_literal_guard(BinOp::Div), Some(LiteralGuard::RightAny));
        assert_eq!(binary_literal_guard(BinOp::Mod), Some(LiteralGuard::RightAny));
        assert_eq!(binary_literal_guard(BinOp::ShiftLeft), Some(LiteralGuard::RightAny));
        assert_eq!(binary_literal_guard(BinOp::ShiftRight), Some(LiteralGuard::RightAny));
        // Everything else falls through to the generic table — in particular
        // `+`, `-` and `|`, which is why `uint8 + 1` is uint9 and not uint8.
        for op in [BinOp::Add, BinOp::Sub, BinOp::LutMul, BinOp::BitwiseOr, BinOp::Equal] {
            assert_eq!(binary_literal_guard(op), None);
            assert_eq!(t_binary_op_literal(op, &Ty::Unsigned(8), &ConstInt::ONE, None), None);
        }
    }

    #[test]
    fn and_literal_table() {
        // Type.hs:821-829, row for row.
        assert_eq!(lit(BinOp::BitwiseAnd, Ty::Unsigned(8), 0), Ty::Unsigned(1));
        assert_eq!(lit(BinOp::BitwiseAnd, Ty::Unsigned(8), -1), Ty::Unsigned(8));
        // min 8 (clog2 4) == 2
        assert_eq!(lit(BinOp::BitwiseAnd, Ty::Unsigned(8), 3), Ty::Unsigned(2));
        // min 2 (clog2 256) == 2 — the unsigned row clamps at the operand width.
        assert_eq!(lit(BinOp::BitwiseAnd, Ty::Unsigned(2), 255), Ty::Unsigned(2));
        assert_eq!(lit(BinOp::BitwiseAnd, Ty::Signed(8), 0), Ty::Signed(2));
        // max 8 (1 + clog2 (1 - (-3)) = 1 + 2) == 8
        assert_eq!(lit(BinOp::BitwiseAnd, Ty::Signed(8), -3), Ty::Signed(8));
        // max 2 (1 + clog2 17) == 6
        assert_eq!(lit(BinOp::BitwiseAnd, Ty::Signed(2), -16), Ty::Signed(6));
        // 1 + clog2 4 == 3 — the signed positive row is NOT clamped by the
        // operand width, so `int2 & 3` *widens* to int3.
        assert_eq!(lit(BinOp::BitwiseAnd, Ty::Signed(2), 3), Ty::Signed(3));
        assert!(lit(BinOp::BitwiseAnd, Ty::Float, 3).is_error());
    }

    #[test]
    fn mul_literal_table() {
        // Type.hs:831-835.
        // `x * 1` keeps the width, because clog2 1 == 0. Pinned by
        // test/syntax/literal-operand.k:70-80:
        //   const auto x = 10;  static assert(decltype(x * 1) == uint4);
        //   const uint32 a;     static assert(decltype(a * 1) == uint32);
        //   const int16 b;      static assert(decltype(b * 1) == int16);
        assert_eq!(lit(BinOp::Mul, Ty::Unsigned(4), 1), Ty::Unsigned(4));
        assert_eq!(lit(BinOp::Mul, Ty::Unsigned(32), 1), Ty::Unsigned(32));
        assert_eq!(lit(BinOp::Mul, Ty::Signed(16), 1), Ty::Signed(16));
        // test/syntax/literal-operand.k:105-107: `const int16 b; b * 10` is
        // int20 — 16 + clog2 10 = 16 + 4.
        assert_eq!(lit(BinOp::Mul, Ty::Signed(16), 10), Ty::Signed(20));
        // n == 0 collapses to the narrowest type of the operand's signedness.
        assert_eq!(lit(BinOp::Mul, Ty::Unsigned(32), 0), Ty::Unsigned(1));
        assert_eq!(lit(BinOp::Mul, Ty::Signed(32), 0), Ty::Signed(2));
        // n < 0 goes through `tNeg` (Type.hs:565-567), which is +1 bit and
        // always signed: uint8 -> int9, then widen by clog2 4 = 2.
        assert_eq!(lit(BinOp::Mul, Ty::Unsigned(8), -3), Ty::Signed(11));
        assert_eq!(lit(BinOp::Mul, Ty::Signed(8), -3), Ty::Signed(11));
        assert!(lit(BinOp::Mul, Ty::Boolean, 3).is_error());
    }

    #[test]
    fn div_literal_table() {
        // Type.hs:837-841 — `tNarrowInt x (clog2 n)`, so the floors of
        // Type.hs:805-807 apply.
        assert_eq!(lit(BinOp::Div, Ty::Unsigned(8), 1), Ty::Unsigned(8));
        assert_eq!(lit(BinOp::Div, Ty::Unsigned(8), 4), Ty::Unsigned(6));
        assert_eq!(lit(BinOp::Div, Ty::Signed(8), 5), Ty::Signed(5));
        assert_eq!(lit(BinOp::Div, Ty::Unsigned(8), 1024), Ty::Unsigned(1));
        assert_eq!(lit(BinOp::Div, Ty::Signed(8), 1024), Ty::Signed(2));
        assert_eq!(
            lit(BinOp::Div, Ty::Unsigned(8), 0),
            Ty::Error(SmolStr::new("Division by zero is undefined"))
        );
        assert_eq!(
            lit(BinOp::Div, Ty::Unsigned(8), -2),
            Ty::Error(SmolStr::new("Division with negative denominator is undefined"))
        );
        // The integer check comes FIRST here (Type.hs:838), unlike tModLiteral.
        assert_eq!(lit(BinOp::Div, Ty::Float, 0), expected_integer_error(&Ty::Float, None));
    }

    #[test]
    fn mod_literal_table_and_its_clause_order() {
        // Type.hs:810-819.
        assert_eq!(lit(BinOp::Mod, Ty::Unsigned(8), 1), Ty::Unsigned(1));
        assert_eq!(lit(BinOp::Mod, Ty::Signed(8), 1), Ty::Signed(2));
        // min 8 (clog2 5) == 3
        assert_eq!(lit(BinOp::Mod, Ty::Unsigned(8), 5), Ty::Unsigned(3));
        // min 8 (1 + clog2 5) == 4
        assert_eq!(lit(BinOp::Mod, Ty::Signed(8), 5), Ty::Signed(4));
        // min clamps: `uint2 % 255` stays uint2.
        assert_eq!(lit(BinOp::Mod, Ty::Unsigned(2), 255), Ty::Unsigned(2));
        assert_eq!(
            lit(BinOp::Mod, Ty::Unsigned(8), 0),
            Ty::Error(SmolStr::new("Remainder by zero is undefined"))
        );
        assert_eq!(
            lit(BinOp::Mod, Ty::Unsigned(8), -2),
            Ty::Error(SmolStr::new("Remainder with negative denominator is undefined"))
        );
        // Clause order trap: the zero/negative guards at Type.hs:811-813 are
        // written on a wildcard type and therefore fire BEFORE the integer
        // check at Type.hs:819 — the opposite of tDivLiteral's order.
        assert_eq!(
            lit(BinOp::Mod, Ty::Float, 0),
            Ty::Error(SmolStr::new("Remainder by zero is undefined"))
        );
        assert_eq!(lit(BinOp::Mod, Ty::Float, 4), expected_integer_error(&Ty::Float, None));
    }

    #[test]
    fn shift_literal_rules_use_the_count_itself_not_its_width() {
        // Type.hs:843-850. Contrast with the generic tShiftL (Type.hs:894-897),
        // which widens by 2^width(count) - 1: `uint32 << 5` is uint37 when 5 is
        // a literal, but uint63 when it is a `uint3` variable.
        assert_eq!(lit(BinOp::ShiftLeft, Ty::Unsigned(32), 5), Ty::Unsigned(37));
        assert_eq!(lit(BinOp::ShiftLeft, Ty::Signed(8), 0), Ty::Signed(8));
        assert_eq!(lit(BinOp::ShiftRight, Ty::Unsigned(32), 5), Ty::Unsigned(27));
        assert_eq!(lit(BinOp::ShiftRight, Ty::Signed(8), 100), Ty::Signed(2));
        // No 20-bit cap on this path — the cap is only in the generic rule.
        assert_eq!(lit(BinOp::ShiftLeft, Ty::Unsigned(1), 4096), Ty::Unsigned(4097));
        // A negative literal count reverses the direction; nothing in the
        // front-end rejects it (test/syntax/basics.k:47-60 compiles `3 << -1`).
        assert_eq!(lit(BinOp::ShiftLeft, Ty::Unsigned(8), -1), Ty::Unsigned(7));
        assert_eq!(lit(BinOp::ShiftRight, Ty::Unsigned(8), -1), Ty::Unsigned(9));
        assert!(lit(BinOp::ShiftLeft, Ty::Float, 1).is_error());
        assert!(lit(BinOp::ShiftRight, Ty::Boolean, 1).is_error());
    }

    // === §2.4 unary ==========================================================

    fn untyped(n: i128) -> ConstValue {
        ConstValue::untyped_int(ConstInt::from_i128(n))
    }

    fn typed(n: i128, lt: LiteralType) -> ConstValue {
        ConstValue::typed_int(lt, ConstInt::from_i128(n))
    }

    #[test]
    fn invert_of_an_untyped_literal_goes_through_t_signed() {
        // Type.hs:742-743 + Type.hs:550-551 + Type.hs:561-563.
        // `~10`: 10 is uint4 untyped, tSigned uint4 = int5, tInvert int5 = int5.
        assert_eq!(infer_unary_op(UnOp::Invert, &Ty::Unsigned(4), Some(&untyped(10)), None), Ty::Signed(5));
        // An already-signed literal keeps its width (tSigned is idempotent).
        assert_eq!(infer_unary_op(UnOp::Invert, &Ty::Signed(6), Some(&untyped(-20)), None), Ty::Signed(6));
        // A *suffixed* literal takes the `otherwise` branch (Type.hs:744) and
        // keeps its own type.
        assert_eq!(
            infer_unary_op(UnOp::Invert, &Ty::Unsigned(4), Some(&typed(10, LiteralType::unsigned(4))), None),
            Ty::Unsigned(4)
        );
        // A non-literal operand likewise.
        assert_eq!(infer_unary_op(UnOp::Invert, &Ty::Unsigned(8), None, None), Ty::Unsigned(8));
        assert!(infer_unary_op(UnOp::Invert, &Ty::Float, None, None).is_error());
    }

    #[test]
    fn neg_of_a_literal_is_t_super_with_the_negated_minimal_type() {
        // Type.hs:745-746. `-10`: tSuper uint4 (inferIntLiteralType Nothing -10)
        //                       = tSuper uint4 int5 = int5 (tSuper clause 7:
        //                         tSigned uint4 = int5, max 5 5).
        assert_eq!(infer_unary_op(UnOp::Neg, &Ty::Unsigned(4), Some(&untyped(10)), None), Ty::Signed(5));
        // `-(-20)`: operand is int6, negated value 20 is uint5 -> tSigned = int6.
        assert_eq!(infer_unary_op(UnOp::Neg, &Ty::Signed(6), Some(&untyped(-20)), None), Ty::Signed(6));
        // A typed literal still takes this branch: `isIntLiteral`, not
        // `isUntypedIntLiteral` (Type.hs:745).
        assert_eq!(
            infer_unary_op(UnOp::Neg, &Ty::Unsigned(32), Some(&typed(10, LiteralType::unsigned(32))), None),
            Ty::Signed(33)
        );
        // Non-literals take tNeg (Type.hs:565-568): always signed, one bit wider.
        assert_eq!(infer_unary_op(UnOp::Neg, &Ty::Unsigned(8), None, None), Ty::Signed(9));
        assert_eq!(infer_unary_op(UnOp::Neg, &Ty::Signed(8), None, None), Ty::Signed(9));
        assert_eq!(infer_unary_op(UnOp::Neg, &Ty::Float, None, None), Ty::Float);
        assert_eq!(
            infer_unary_op(UnOp::Neg, &Ty::Boolean, None, None),
            Ty::Error(SmolStr::new("Expected an integer or a float operand"))
        );
    }

    #[test]
    fn not_requires_a_boolean_and_sizeof_is_unresolved() {
        // Type.hs:748 + Type.hs:570-571.
        assert_eq!(infer_unary_op(UnOp::Not, &Ty::Boolean, None, None), Ty::Boolean);
        assert_eq!(
            infer_unary_op(UnOp::Not, &Ty::Unsigned(1), None, None),
            Ty::Error(SmolStr::new("Expected a boolean operand"))
        );
        // Type.hs:749-751 — answered by constant folding, not by the algebra.
        for op in [UnOp::BitSizeof, UnOp::ByteSizeof, UnOp::Clog2] {
            assert_eq!(infer_unary_op(op, &Ty::Struct(SymbolPath::empty(), vec![]), None, None), Ty::Unresolved);
        }
    }

    // === corpus chains =======================================================

    #[test]
    fn literal_operand_k_133_139_derives_uint5() {
        // test/syntax/literal-operand.k:133-139, warning:0:
        //
        //     uint1 one = 1;
        //     const auto x = Num::Four + ((Num::Seven * one) & Num::Three);
        //     static assert(decltype(x) == uint5);
        //
        // with `enum Num : uint4` (helper/literal_operand.k:13-25). Every step
        // is a different row of the table, so this pins the interaction of
        // `typeOfOperand`, the commutative literal guard and the generic `tAdd`.
        let four = Ty::Enum(SymbolPath::empty(), TyRef::new(Ty::Unsigned(4)));

        // `Num::Seven * one`: the literal is the *left* operand, so the
        // commutative guard (Type.hs:763-764) applies tMulLiteral to `one`.
        let a = lit(BinOp::Mul, Ty::Unsigned(1), 7);
        assert_eq!(a, Ty::Unsigned(4), "uint1 widened by clog2 7 = 3");

        // `… & Num::Three`: right operand is an untyped literal.
        let b = lit(BinOp::BitwiseAnd, a, 3);
        assert_eq!(b, Ty::Unsigned(2), "min 4 (clog2 4) = 2");

        // `Num::Four + …`: `+` has no literal rule, so the generic row runs on
        // the *stripped* operand type — uint4, not the enum.
        assert_eq!(type_of_operand(&four), &Ty::Unsigned(4));
        assert_eq!(t_binary_op(BinOp::Add, type_of_operand(&four), &b, None), Ty::Unsigned(5));
    }

    #[test]
    fn literal_operand_k_105_107_derives_int48_and_int20() {
        // test/syntax/literal-operand.k:85-108, warning:0:
        //
        //     const uint32 a = 10;  const int16 b = -20;  const auto x = 10;
        //     static assert(decltype(a * b) == int48);
        //     static assert(decltype(b * x) == int20);
        //
        // `a` and `b` are const ints, so Constexpr.hs:286-293 inlines them as
        // *suffixed* literals; a suffixed literal fails the `isUntypedIntLiteral`
        // guard, so `a * b` takes the generic tMul row.
        assert_eq!(t_binary_op(BinOp::Mul, &Ty::Unsigned(32), &Ty::Signed(16), None), Ty::Signed(48));
        // `b * x`: `x` is untyped, so the commutative guard fires and
        // tMulLiteral widens int16 by clog2 10 = 4.
        assert_eq!(lit(BinOp::Mul, Ty::Signed(16), 10), Ty::Signed(20));
    }

    #[test]
    fn the_operator_table_strips_enum_const_and_instance_itself() {
        // Type.hs:777 and Type.hs:775 apply `typeOfOperand` *inside* tBinaryOp
        // and tBinaryOpLiteral, and Type.hs:743 does the same in inferUnaryOp.
        // Passing an unstripped type must therefore give the same answer as
        // passing a stripped one — this is what makes enums arithmetic as their
        // base type and erases `const`.
        let enum4 = Ty::Enum(SymbolPath::empty(), r(Ty::Unsigned(4)));
        let const_enum4 = Ty::Const(r(enum4.clone()));
        let inst8 = Ty::Instance {
            concrete: r(Ty::Const(r(Ty::Unsigned(8)))),
            template: SymbolPath::empty(),
            args: vec![],
        };
        assert_eq!(t_binary_op(BinOp::Add, &const_enum4, &inst8, None), Ty::Unsigned(9));
        assert_eq!(t_binary_op(BinOp::Sub, &enum4, &Ty::Unsigned(4), None), Ty::Signed(5));
        assert_eq!(
            t_binary_op_literal(BinOp::Mul, &const_enum4, &ConstInt::from_i128(10), None),
            Some(Ty::Unsigned(8))
        );
        assert_eq!(infer_unary_op(UnOp::Neg, &const_enum4, None, None), Ty::Signed(5));
    }

    #[test]
    fn decltype_k_20_28_pins_the_literal_widths() {
        // test/syntax/decltype.k:20-28, expected:0:
        //     auto x = 10;  static assert(bitsizeof x == 4);
        //     decltype(bitsizeof(decltype(x))) y;  static assert(bitsizeof y == 3);
        // i.e. 10 is uint4 and the literal 4 is uint3.
        assert_eq!(infer_int_literal_type(&untyped(10)), Ty::Unsigned(4));
        assert_eq!(t_width(&infer_int_literal_type(&untyped(10))), Some(4));
        assert_eq!(t_width(&infer_int_literal_type(&untyped(4))), Some(3));
        // A suffix overrides the minimal width entirely (Type.hs:486-487).
        assert_eq!(infer_int_literal_type(&typed(10, LiteralType::unsigned(32))), Ty::Unsigned(32));
        assert_eq!(infer_int_literal_type(&typed(10, LiteralType::signed(16))), Ty::Signed(16));
    }
}
