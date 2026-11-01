//! A value the loop wrote into a call or an instance, spelled as reference
//! text that the frontend reads back as the same value.
//!
//! Every spelling is checked against the frontend's own readers — the
//! literal parsers the lowering calls (`kg_const::ConstValue::parse_*`) and
//! the folder of a unary minus (`kg_hir::constexpr::unary`) — so that what
//! the text lowers and folds to is the node the loop made; a value no text
//! reads back exactly is refused, never approximated.

use kg_base::Interner;
use kg_const::{ConstInt, ConstValue, F32Bits, LiteralSign, LiteralType};
use kg_hir::{Exp, ExpArena, ExpId};
use kg_ty::{TArg, Ty};

/// A value written as text: `text` is what the expansion writes, `key` what
/// two copies' values compare by — an enum's constant by its whole name,
/// module included, which `text` leaves out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Spelled {
    pub text: String,
    pub key: String,
}

impl Spelled {
    fn plain(text: String) -> Spelled {
        Spelled { key: text.clone(), text }
    }
}

/// The default `named.rs` wrote into a call (`default_argument`): a literal,
/// or an enum's constant by its absolute name — as the loop left it, folded.
pub(crate) fn default(arena: &ExpArena, interner: &Interner, id: ExpId) -> Result<Spelled, String> {
    match &arena[id].exp {
        Exp::IntLiteral { ty, value } => int(*ty, value).map(Spelled::plain),
        Exp::BoolLiteral(b) => Ok(Spelled::plain(b.to_string())),
        Exp::FloatLiteral(bits) => float(*bits).map(Spelled::plain),
        Exp::EnumValue { ty, .. } => {
            let path: Vec<String> =
                kg_hir::sort::spine_qualname(arena, *ty).iter().map(|s| interner.resolve(*s).to_string()).collect();
            enum_constant(&path)
        }
        other => Err(format!("a default the loop wrote as {}, which kg expand does not write", kind(other))),
    }
}

/// An enum's constant: its absolute name (`@elib::Color::Green`, the
/// module's segment marked by its `@`) less the module, which no reference
/// text names — `Color::Green`, as a call that imports the enum writes it.
/// That the name finds the same constant where the call stands is the
/// proof's to show ([`crate::verify`]).
fn enum_constant(path: &[String]) -> Result<Spelled, String> {
    let own: Vec<&str> = path.iter().map(String::as_str).skip_while(|s| s.starts_with('@')).collect();
    if own.len() < 2 || own.iter().any(|s| s.is_empty() || s.contains(['@', '$', '<', '>'])) {
        return Err(format!("an enum's constant whose name `{}` kg expand does not write", path.join("::")));
    }
    Ok(Spelled { text: own.join("::"), key: path.join("::") })
}

fn kind(exp: &Exp) -> &'static str {
    match exp {
        Exp::StringLiteral(_) | Exp::InterpolatedString(_) => "a string",
        Exp::InitializerList(_) => "an initializer list",
        Exp::NamedValue(_) => "a name",
        _ => "an expression",
    }
}

fn suffix(ty: LiteralType) -> String {
    let sign = match ty.sign {
        LiteralSign::Signed => 'i',
        LiteralSign::Unsigned => 'u',
    };
    format!("{sign}{}", ty.width)
}

/// An integer literal, its type kept: `3u9` where the loop folded
/// `Step + 1` to a `uint9` — a bare `3` is a `uint2` and widens nothing the
/// same way. A negative one is a unary minus over the literal whose
/// negation folds to it.
pub(crate) fn int(ty: Option<LiteralType>, value: &ConstInt) -> Result<String, String> {
    let target = ConstValue::Int { ty, value: value.clone() };
    if !value.is_negative() {
        let text = format!("{value}{}", ty.map(suffix).unwrap_or_default());
        return (ConstValue::parse_int_literal(&text).as_ref() == Some(&target))
            .then_some(text)
            .ok_or_else(|| format!("the integer {value} of no literal kg expand can write"));
    }
    let magnitude = value.abs();
    let operands: Vec<Option<LiteralType>> = match ty {
        None => vec![None],
        Some(t) => (1..=t.width + 1)
            .flat_map(|w| [Some(LiteralType::unsigned(w)), Some(LiteralType::signed(w))])
            .collect(),
    };
    for operand in operands {
        let text = format!("{magnitude}{}", operand.map(suffix).unwrap_or_default());
        let Some(read) = ConstValue::parse_int_literal(&text) else { continue };
        if kg_hir::constexpr::unary(kg_const::UnOp::Neg, &read).as_ref() == Some(&target) {
            return Ok(format!("-{text}"));
        }
    }
    let shown = ty.map(|t| format!(" typed {}{}", if t.sign == LiteralSign::Signed { "int" } else { "uint" }, t.width)).unwrap_or_default();
    Err(format!("the integer {value}{shown}, which no literal kg expand can write folds to"))
}

/// A `float32`, in the shortest text the lowering's reader takes back to the
/// same bits — one float token, as the lexer reads it.
pub(crate) fn float(bits: F32Bits) -> Result<String, String> {
    let f = bits.to_f32();
    if !f.is_finite() {
        return Err(format!("the float {f}, which no literal writes"));
    }
    let magnitude = f.abs();
    let text = format!("{magnitude:?}");
    let lexed = kg_parser::LexedStr::new(&text);
    let one_float = lexed.len() == 1 && lexed.kind(0) == kg_syntax::SyntaxKind::FLOAT_NUMBER;
    let read = ConstValue::parse_float_literal(&text);
    let target = ConstValue::Float(bits);
    let exact = if f.is_sign_negative() {
        read.as_ref().and_then(|r| kg_hir::constexpr::unary(kg_const::UnOp::Neg, r)) == Some(target)
    } else {
        read == Some(target)
    };
    if !one_float || !exact {
        return Err(format!("the float {f:?}, which no literal kg expand can write reads back exactly"));
    }
    Ok(if f.is_sign_negative() { format!("-{text}") } else { text })
}

/// A template argument the loop gave a parameter no argument names — its
/// default, or what a function template's call deduced — as the instance's
/// type records it (`tArg`, `Type.hs:433-439`): an integer is the instance's
/// by its value alone (`kg_hir::rewrite::instantiate`'s `t_arg`), a `bool`
/// is its spelling, a type one of the language's own, whose name no
/// declaration can hide.
pub(crate) fn template_argument(arg: &TArg) -> Result<Spelled, String> {
    match arg {
        TArg::Int(v) => int(None, v).map(Spelled::plain),
        TArg::Expr(s) if s == "true" || s == "false" => Ok(Spelled::plain(s.to_string())),
        TArg::Type(t) => builtin(t).map(Spelled::plain).ok_or_else(|| "a type other than the language's own".to_owned()),
        TArg::Expr(_) | TArg::Str(_) => Err("a value other than an integer or a `bool`".to_owned()),
        TArg::Name(..) => Err("a name".to_owned()),
    }
}

fn builtin(t: &Ty) -> Option<String> {
    match t {
        Ty::Unsigned(w) => Some(format!("uint{w}")),
        Ty::Signed(w) => Some(format!("int{w}")),
        Ty::Boolean => Some("bool".to_owned()),
        Ty::Float => Some("float32".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folded integer keeps its type in its spelling, and a negative one
    /// is the minus whose fold gives it back: `-3` untyped, and for a typed
    /// one the operand type the folder widens to it. Red with the suffix
    /// dropped, or with a negative typed value spelled as its untyped text.
    #[test]
    fn an_integer_is_spelled_as_its_type_reads_it() {
        assert_eq!(int(None, &ConstInt::from(3i128)).unwrap(), "3");
        assert_eq!(int(Some(LiteralType::unsigned(9)), &ConstInt::from(3i128)).unwrap(), "3u9");
        assert_eq!(int(None, &ConstInt::from(-3i128)).unwrap(), "-3");
        let typed = int(Some(LiteralType::signed(4)), &ConstInt::from(-3i128)).unwrap();
        let operand = ConstValue::parse_int_literal(typed.strip_prefix('-').unwrap()).unwrap();
        assert_eq!(
            kg_hir::constexpr::unary(kg_const::UnOp::Neg, &operand),
            Some(ConstValue::Int { ty: Some(LiteralType::signed(4)), value: ConstInt::from(-3i128) })
        );
    }

    /// A float is the shortest text that reads back to its bits, and one
    /// with no literal — infinity — is refused. Red with a spelling that
    /// rounds (`{:.3}`), or with the infinity let through.
    #[test]
    fn a_float_reads_back_to_its_bits() {
        for f in [0.1f32, 1.0, -2.5, 3.4028235e38, 1e-10, 16777217.0] {
            let text = float(F32Bits::from_f32(f)).unwrap();
            let read: f32 = text.parse().unwrap();
            assert_eq!(read.to_bits(), f.to_bits(), "{text}");
        }
        assert!(float(F32Bits::from_f32(f32::INFINITY)).is_err());
    }

    /// A template argument the expansion must write is spelled by its kind: an
    /// integer by its value, a `bool` by its name, the language's own types by
    /// theirs; what has no name no declaration can hide is refused. Red with
    /// one of the language's types left out, or with a class let through.
    #[test]
    fn a_template_argument_is_spelled_by_its_kind() {
        let text = |arg: TArg| template_argument(&arg).map(|s| s.text);
        assert_eq!(text(TArg::Int(ConstInt::from(3i128))).unwrap(), "3");
        assert_eq!(text(TArg::Int(ConstInt::from(-3i128))).unwrap(), "-3");
        assert_eq!(text(TArg::Expr("true".into())).unwrap(), "true");
        assert_eq!(text(TArg::Type(Ty::Unsigned(8).into())).unwrap(), "uint8");
        assert_eq!(text(TArg::Type(Ty::Signed(4).into())).unwrap(), "int4");
        assert_eq!(text(TArg::Type(Ty::Boolean.into())).unwrap(), "bool");
        assert_eq!(text(TArg::Type(Ty::Float.into())).unwrap(), "float32");
        assert!(text(TArg::Type(Ty::Void.into())).is_err());
        assert!(text(TArg::Str("s".into())).is_err());
        assert!(text(TArg::Expr("1 + 1".into())).is_err());
    }

    /// An enum's constant drops its module, which reference text cannot
    /// name, and keeps it in the key two copies compare by. Red with the
    /// module kept in the text.
    #[test]
    fn an_enum_constant_is_named_without_its_module() {
        let path: Vec<String> = ["@elib", "Color", "Green"].map(str::to_owned).to_vec();
        let s = enum_constant(&path).unwrap();
        assert_eq!(s.text, "Color::Green");
        assert_eq!(s.key, "@elib::Color::Green");
    }
}
