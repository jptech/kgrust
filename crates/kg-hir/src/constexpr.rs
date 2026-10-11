//! `evaluateConstexpr` over the IR (`Constexpr.hs:117-356`): the one reading
//! of a constant expression every IR consumer asks.
//!
//! The reference folds by *rewriting*: each node, once its children are
//! folded and typed, becomes the literal its rule computes, under its own
//! note. [`value`] computes the same literal bottom-up without rewriting —
//! what the census, the mangler, a copy's `static if` and the backend walk
//! need — and every rule is a function of *values*, so the rewriting
//! inference of `docs/plans/ir-elaboration.md` (§2.2) calls the same rules
//! node by node.
//!
//! What a subtree cannot say by itself — a template parameter's binding, a
//! named constant, the type `bitsizeof x` measures — comes from the caller's
//! [`Leaves`]. [`Closed`] knows nothing beyond written built-in types, which
//! is exactly a copy whose parameters are substituted.
//!
//! This replaced four folders that had drifted apart
//! (`docs/plans/single-source-of-truth.md`, D1.3): `rewrite::exp_value`,
//! `kg_db::instances`' `Dependent::value`, `width::value` (checked `i128`,
//! no comparisons) and the backend walk's `literal_arithmetic`. Where they
//! disagreed with the reference this follows the reference: a typed
//! operand's result is typed (`Constexpr.hs:200-202`, `:276-278`), and
//! `bytesizeof` of a width that is not whole bytes does not fold
//! (`bytesFromBits`, `:350-352`).

use kg_const::{BinOp, CastTarget, ConstValue, LiteralSign, LiteralType, UnOp};
use kg_ty::Ty;

use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;

/// What a subtree cannot fold by itself.
pub trait Leaves {
    /// The value of a node no rule folds — a parameter's binding, a named
    /// constant. Asked only of nodes [`value`] has no structural rule for.
    fn leaf(&self, _arena: &ExpArena, _id: ExpId) -> Option<ConstValue> {
        None
    }

    /// The type `bitsizeof`/`bytesizeof` measures (`sizeOf`,
    /// `Type.hs:462`): by default, a written type the arena spells.
    fn measured(&self, arena: &ExpArena, id: ExpId) -> Option<Ty> {
        written_type(arena, id, self)
    }
}

/// A subtree with nothing outside it: literals, written built-in types.
#[derive(Debug, Clone, Copy)]
pub struct Closed;

impl Leaves for Closed {}

/// The value `e` folds to, or `None` where it does not fold — a name the
/// leaves do not know, a rule the reference has no clause for, or one of the
/// fold's `desugarError`s (division by zero, a bad mux selector), which are
/// the checking passes' to report and never a value.
pub fn value(arena: &ExpArena, e: ExpId, env: &(impl Leaves + ?Sized)) -> Option<ConstValue> {
    match &arena[e].exp {
        Exp::IntLiteral { ty, value } => Some(ConstValue::Int { ty: *ty, value: value.clone() }),
        Exp::BoolLiteral(b) => Some(ConstValue::Bool(*b)),
        Exp::FloatLiteral(bits) => Some(ConstValue::Float(*bits)),
        Exp::InterpolatedString(tok) | Exp::StringLiteral(tok) => {
            crate::exp::plain_string_value(tok).map(|s| ConstValue::Str(kg_base::SmolStr::new(s)))
        }
        // `evalBinaryType` (`Constexpr.hs:244-249`): two types compare by
        // `<:` both ways — `static if (T == bool)` in a copy of `Fn_T`.
        Exp::Binary(op, a, b)
            if written_type(arena, *a, env).is_some() && written_type(arena, *b, env).is_some() =>
        {
            let (s, t) = (written_type(arena, *a, env)?, written_type(arena, *b, env)?);
            binary_types(*op, &s, &t).map(ConstValue::Bool)
        }
        Exp::Binary(op, a, b) => binary(*op, &value(arena, *a, env)?, &value(arena, *b, env)?),
        Exp::Unary(op @ (UnOp::BitSizeof | UnOp::ByteSizeof), operand) => {
            let bits = kg_ty::width::t_width(&env.measured(arena, *operand)?)?;
            size(*op, bits)
        }
        Exp::Unary(op, operand) => unary(*op, &value(arena, *operand, env)?),
        // `eval (MuxF (BoolLiteral x) y)` and an integer selector
        // (`Constexpr.hs:311-315`); `c ? t : f` lowers to `mux(c, f, t)`.
        Exp::Mux { selector, cases } => {
            let index = match value(arena, *selector, env)? {
                ConstValue::Bool(x) => usize::from(x),
                v => v.as_int()?.to_usize()?,
            };
            let Exp::Seq(items) = &arena[*cases].exp else { return None };
            value(arena, *items.get(index)?, env)
        }
        // `eval (StaticF x) | isLiteral x = x` (`Constexpr.hs:317-318`).
        Exp::Static(x) => value(arena, *x, env),
        // `eval (CastF t x)` (`Constexpr.hs:305-307`) to a written type.
        Exp::Cast { ty, value: x } => cast(&written_type(arena, *ty, env)?, &value(arena, *x, env)?),
        Exp::NamedValue(inner) | Exp::QualifiedIdentifier(inner) => {
            env.leaf(arena, e).or_else(|| value(arena, *inner, env))
        }
        _ => env.leaf(arena, e),
    }
}

/// `evalBinary` over two folded operands (`Constexpr.hs:171-215`), the
/// result stamped as the fold stamps it: untyped when both integer operands
/// are, else `typedIntLiteral $ inferBinaryOp op a b` (`:200-202`).
pub fn binary(op: BinOp, x: &ConstValue, y: &ConstValue) -> Option<ConstValue> {
    let folded = kg_const::fold_binary_values(op, x, y).ok().flatten()?;
    let typed = |v: &ConstValue| matches!(v, ConstValue::Int { ty: Some(_), .. });
    match folded {
        ConstValue::Int { value, .. } if typed(x) || typed(y) => {
            let ty = literal_type(&kg_ty::width::infer_binary_op_values(op, x, y, None)?)?;
            Some(ConstValue::Int { ty: Some(ty), value })
        }
        other => Some(other),
    }
}

/// `evalBinaryType` (`Constexpr.hs:244-249`).
pub fn binary_types(op: BinOp, s: &Ty, t: &Ty) -> Option<bool> {
    let (st, ts) = (kg_ty::subtype::is_subtype(s, t), kg_ty::subtype::is_subtype(t, s));
    Some(match op {
        BinOp::Equal => st && ts,
        BinOp::NotEqual => !st || !ts,
        BinOp::LessEqual => st,
        BinOp::Less => st && !ts,
        BinOp::GreaterEqual => ts,
        BinOp::Greater => ts && !st,
        _ => return None,
    })
}

/// `evalUnary` over a folded operand (`Constexpr.hs:251-278`), for the
/// operators whose rule needs no type beyond the literal's own: `-`, `~`,
/// `!` and `clog2`. An untyped operand's result is untyped, a typed one's is
/// `typedIntLiteral $ inferUnaryOp op a` (`:276-278`).
pub fn unary(op: UnOp, x: &ConstValue) -> Option<ConstValue> {
    let stamp = |value: kg_const::ConstInt| -> Option<ConstValue> {
        match x {
            ConstValue::Int { ty: Some(_), .. } => {
                let operand = kg_ty::width::infer_int_literal_type(x);
                let ty = literal_type(&kg_ty::width::infer_unary_op(op, &operand, Some(x), None))?;
                Some(ConstValue::Int { ty: Some(ty), value })
            }
            _ => Some(ConstValue::untyped_int(value)),
        }
    };
    match (op, x) {
        (UnOp::Neg, ConstValue::Float(bits)) => Some(ConstValue::Float(kg_const::F32Bits::from_f32(-bits.to_f32()))),
        (UnOp::Neg, _) => stamp(x.as_int()?.neg()),
        (UnOp::Not, ConstValue::Bool(b)) => Some(ConstValue::Bool(!b)),
        // `invert t` truncates the complement to the operand's width, an
        // untyped literal through `tSigned` (`:258-264`).
        (UnOp::Invert, ConstValue::Int { .. }) => {
            let t = kg_ty::width::infer_int_literal_type(x);
            let t = if matches!(x, ConstValue::Int { ty: None, .. }) { t.to_signed() } else { t };
            let int = x.as_int()?.not();
            let wrapped = match t {
                Ty::Unsigned(w) => int.truncate_uint(w),
                Ty::Signed(w) => int.truncate_int(w),
                _ => return None,
            };
            stamp(wrapped)
        }
        // `clog2` folds regardless of types (`Constexpr.hs:168`).
        (UnOp::Clog2, _) => {
            let n = x.as_natural()?.checked_clog2()?;
            Some(ConstValue::untyped_int(i128::from(n)))
        }
        _ => None,
    }
}

/// `bitsizeof`/`bytesizeof` of a `bits`-wide type (`Constexpr.hs:266-270`):
/// `bytesizeof` of a width that is not whole bytes is `bytesFromBits`'
/// error, which does not fold.
pub fn size(op: UnOp, bits: u64) -> Option<ConstValue> {
    match op {
        UnOp::BitSizeof => Some(ConstValue::untyped_int(i128::from(bits))),
        UnOp::ByteSizeof if bits.is_multiple_of(8) => Some(ConstValue::untyped_int(i128::from(bits / 8))),
        _ => None,
    }
}

/// `castIntegralConstexpr` and `castFloatConstexpr` (`Constexpr.hs:329-343`):
/// an integer into an integer type wraps to it, typed; into `float32` it is
/// the low 32 bits reinterpreted (`castWord32ToFloat . fromIntegral`, a
/// mod-2^32 wrap) — `cast<float32>(0)` is `0.0` (`constexpr.k::87`); a
/// float into an integer type is its bit pattern, wrapped; into `float32`
/// itself. Other targets do not fold.
pub fn cast(target: &Ty, x: &ConstValue) -> Option<ConstValue> {
    let target = match target.un_const() {
        Ty::Unsigned(w) => CastTarget::Unsigned(w),
        Ty::Signed(w) => CastTarget::Signed(w),
        Ty::Float => CastTarget::Float,
        _ => return None,
    };
    let int = match x {
        ConstValue::Float(f) if target == CastTarget::Float => return Some(ConstValue::Float(*f)),
        ConstValue::Float(f) => kg_const::ConstInt::from_u32(f.0),
        _ => x.as_int()?.clone(),
    };
    if target == CastTarget::Float {
        let bits = int.truncate_uint(32).to_u32()?;
        return Some(ConstValue::Float(kg_const::F32Bits(bits)));
    }
    let value = target.truncate(&int)?;
    Some(ConstValue::Int { ty: target.literal_type(), value })
}

/// A written type the arena spells without a table: the built-in types,
/// `int<e>`/`uint<e>` whose width folds, `const`, and a type identifier
/// around one of those.
pub fn written_type(arena: &ExpArena, id: ExpId, env: &(impl Leaves + ?Sized)) -> Option<Ty> {
    Some(match &arena[id].exp {
        Exp::Boolean => Ty::Boolean,
        Exp::Float => Ty::Float,
        Exp::Str => Ty::Str,
        Exp::Void => Ty::Void,
        Exp::Integer(w) => Ty::Signed(*w),
        Exp::Unsigned(w) => Ty::Unsigned(*w),
        Exp::ParamInt(w) => Ty::Signed(width(arena, *w, env)?),
        Exp::ParamUint(w) => Ty::Unsigned(width(arena, *w, env)?),
        Exp::Const(inner) => Ty::Const(kg_ty::TyRef::new(written_type(arena, *inner, env)?)),
        Exp::TypeIdentifier(_, inner) => return written_type(arena, *inner, env),
        _ => return None,
    })
}

/// The width `int<e>`/`uint<e>`'s expression folds to.
pub fn width(arena: &ExpArena, e: ExpId, env: &(impl Leaves + ?Sized)) -> Option<u32> {
    value(arena, e, env)?.as_int()?.to_u32()
}

/// A literal's stamp for an integer type.
pub fn literal_type(t: &Ty) -> Option<LiteralType> {
    match t {
        Ty::Unsigned(w) => Some(LiteralType { sign: LiteralSign::Unsigned, width: *w }),
        Ty::Signed(w) => Some(LiteralType { sign: LiteralSign::Signed, width: *w }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    use super::*;
    use crate::sort::NoImports;

    /// The initializer of every `const auto` in `text`, folded closed.
    fn folds(text: &str) -> Vec<Option<ConstValue>> {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = crate::lower_sorted(&file, FileId(0), &interner, &NoImports);
        ir.arena
            .iter()
            .filter_map(|(_, n)| match &n.exp {
                Exp::Variable { init: Some(i), .. } => Some(value(&ir.arena, *i, &Closed)),
                _ => None,
            })
            .collect()
    }

    fn int(ty: Option<LiteralType>, v: i128) -> Option<ConstValue> {
        Some(ConstValue::Int { ty, value: kg_const::ConstInt::from_i128(v) })
    }

    /// Arithmetic, comparison and logic over literals; the stamp follows the
    /// operands (`Constexpr.hs:200-202`): `1 + 2` is untyped, `1u4 + 2` is
    /// `inferBinaryOp`'s `uint5`. Red with the stamp dropped.
    #[test]
    fn a_binary_folds_and_a_typed_operand_types_the_result() {
        let v = folds("const auto a = 1 + 2;\nconst auto b = 1u4 + 2;\nconst auto c = 3 > 2;\nconst auto d = true && false;\nconst auto e = 7 / 0;\n");
        assert_eq!(v[0], int(None, 3));
        assert_eq!(v[1], int(Some(LiteralType::unsigned(5)), 3));
        assert_eq!(v[2], Some(ConstValue::Bool(true)));
        assert_eq!(v[3], Some(ConstValue::Bool(false)));
        assert_eq!(v[4], None, "division by zero is the pass's error, never a value");
    }

    /// `-`, `~`, `!`, `clog2`: an untyped operand stays untyped, a typed one
    /// is stamped by `inferUnaryOp` (`Constexpr.hs:276-278`); `~` truncates
    /// to the operand's width, an untyped literal's through `tSigned`.
    #[test]
    fn a_unary_folds_as_the_reference_stamps_it() {
        let v = folds("const auto a = -5;\nconst auto b = -5u8;\nconst auto c = ~0u4;\nconst auto d = !true;\nconst auto e = clog2(16);\n");
        assert_eq!(v[0], int(None, -5));
        assert!(matches!(&v[1], Some(ConstValue::Int { ty: Some(t), value }) if t.is_signed() && value.to_i128() == Some(-5)), "{:?}", v[1]);
        assert_eq!(v[2], int(Some(LiteralType::unsigned(4)), 15));
        assert_eq!(v[3], Some(ConstValue::Bool(false)));
        assert_eq!(v[4], int(None, 4));
    }

    /// `bitsizeof`/`bytesizeof` of a written type, `int<e>` included; a
    /// width that is not whole bytes has no `bytesizeof` (`bytesFromBits`).
    /// Two written types compare by `<:` (`evalBinaryType`). A mux picks its
    /// case; a cast wraps and types.
    #[test]
    fn sizes_type_comparisons_muxes_and_casts_fold() {
        let v = folds("const auto a = bitsizeof uint<3 + 4>;\nconst auto b = bytesizeof uint16;\nconst auto c = bytesizeof uint12;\nconst auto d = uint8 == uint8;\nconst auto e = uint4 < uint8;\nconst auto f = true ? 10 : 20;\nconst auto g = cast<uint4>(19);\n");
        assert_eq!(v[0], int(None, 7));
        assert_eq!(v[1], int(None, 2));
        assert_eq!(v[2], None);
        assert_eq!(v[3], Some(ConstValue::Bool(true)));
        assert_eq!(v[4], Some(ConstValue::Bool(true)));
        assert_eq!(v[5], int(None, 10));
        assert_eq!(v[6], int(Some(LiteralType::unsigned(4)), 3));
    }

    /// A name folds only through the caller's leaves.
    #[test]
    fn a_name_folds_only_through_the_leaves() {
        struct Five;
        impl Leaves for Five {
            fn leaf(&self, arena: &ExpArena, id: ExpId) -> Option<ConstValue> {
                matches!(arena[id].exp, Exp::ScopedName { .. }).then(|| ConstValue::untyped_int(5i128))
            }
        }
        let interner = Interner::new();
        let parse = kg_syntax::parse_file("const auto N = 1;\nconst auto a = N + 1;\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = crate::lower_sorted(&file, FileId(0), &interner, &NoImports);
        let init = ir
            .arena
            .iter()
            .filter_map(|(_, n)| match &n.exp {
                Exp::Variable { init: Some(i), .. } => Some(*i),
                _ => None,
            })
            .last()
            .expect("a");
        assert_eq!(value(&ir.arena, init, &Closed), None);
        assert_eq!(value(&ir.arena, init, &Five), int(None, 6));
    }
}
