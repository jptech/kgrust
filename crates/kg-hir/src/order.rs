//! The reference's `Ord` on expressions: `deriving Ord` on `ExpF`
//! (`Parser/Syntax.hs:584`), through `Fix` (`Recursion.hs:48`) and a
//! `Note` that ignores its annotation (`Parser/Syntax.hs:597-599`).
//!
//! It decides one observable order: `Map.toList` over a template's decided
//! instances, keyed by the argument `Seq` (`Template.hs:88,120`), which is
//! the order the copies are emitted in beside their template — `@2` before
//! `@10`, `BooleanF` before `UnsignedF`. [`crate::Exp`] declares its
//! variants in `ExpF`'s order, and every leaf enum (`BinOp`, `Attribute`,
//! `DeclareFlag`, …) derives `Ord` in the reference's declaration order,
//! so a derived comparison is a port; the three things that are not derived
//! are spelled out: a `Name` compares as its `String` (`Hashed`'s `Ord`
//! ignores the hash, hashable-1.5.1.0 `Data/Hashable/Class.hs:883-884`), a
//! string literal compares as its parsed content, and an alias the route
//! minted compares as the name it stands for.

use std::cmp::Ordering;

use kg_base::{Interner, Symbol};
use kg_ty::QualifiedName;

use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;

/// The constructor's index in `data ExpF`.
fn ctor_index(e: &Exp) -> u32 {
    match e {
        Exp::Seq(..) => 0,
        Exp::FlagAttr(..) => 1,
        Exp::IntAttr(..) => 2,
        Exp::Attr(..) => 3,
        Exp::FunctionModifier(..) => 4,
        Exp::Auto => 5,
        Exp::Void => 6,
        Exp::Boolean => 7,
        Exp::Float => 8,
        Exp::Str => 9,
        Exp::Integer(..) => 10,
        Exp::ParamInt(..) => 11,
        Exp::Unsigned(..) => 12,
        Exp::ParamUint(..) => 13,
        Exp::Const(..) => 14,
        Exp::Reference(..) => 15,
        Exp::Array { .. } => 16,
        Exp::TemplateInstance { .. } => 17,
        Exp::TemplateParam { .. } => 18,
        Exp::TypeParam { .. } => 19,
        Exp::Typename => 20,
        Exp::FunctionTypeParam { .. } => 21,
        Exp::FunctionType { .. } => 22,
        Exp::This { .. } => 23,
        Exp::Designator { .. } => 24,
        Exp::Positional { .. } => 25,
        Exp::InitializerList(..) => 26,
        Exp::BitOffset { .. } => 27,
        Exp::ByteOffset { .. } => 28,
        Exp::IntLiteral { .. } => 29,
        Exp::FloatLiteral(..) => 30,
        Exp::BoolLiteral(..) => 31,
        Exp::StringLiteral(..) => 32,
        Exp::InterpolatedString(..) | Exp::Interpolated(..) => 33,
        Exp::Identifier(..) => 34,
        Exp::ScopedName { .. } => 35,
        Exp::QualifiedIdentifier(..) => 36,
        Exp::TypeIdentifier(..) => 37,
        Exp::FunctionSpecifier { .. } => 38,
        Exp::EnumValue { .. } => 39,
        Exp::NamedValue(..) => 40,
        Exp::Binary(..) => 41,
        Exp::Unary(..) => 42,
        Exp::FunctionCall { .. } => 43,
        Exp::ArrayAccess { .. } => 44,
        Exp::ArrayAccessLValue { .. } => 45,
        Exp::MemberAccess { .. } => 46,
        Exp::Cast { .. } => 47,
        Exp::Mux { .. } => 48,
        Exp::Concat(..) => 49,
        Exp::FanOut { .. } => 50,
        Exp::Static(..) => 51,
        Exp::Annotated { .. } => 52,
        Exp::Barrier => 53,
        Exp::Reorder(..) => 54,
        Exp::Return(..) => 55,
        Exp::DoWhile { .. } => 56,
        Exp::UnrolledFor { .. } => 57,
        Exp::StaticFor { .. } => 58,
        Exp::RangeFor { .. } => 59,
        Exp::If { .. } => 60,
        Exp::StaticIf { .. } => 61,
        Exp::Switch { .. } => 62,
        Exp::SwitchCase { .. } => 63,
        Exp::SwitchDefault(..) => 64,
        Exp::Assign { .. } => 65,
        Exp::NestedScope { .. } => 66,
        Exp::ModuleIdentifier(..) => 67,
        Exp::ModuleDiff(..) => 68,
        Exp::ModuleDecl { .. } => 69,
        Exp::EnumConstant { .. } => 70,
        Exp::Enum { .. } => 71,
        Exp::Struct { .. } => 72,
        Exp::Union { .. } => 73,
        Exp::Class { .. } => 74,
        Exp::DefaultInitialization(..) => 75,
        Exp::Private => 76,
        Exp::Public => 77,
        Exp::Template { .. } => 78,
        Exp::Decltype { .. } => 79,
        Exp::Alias { .. } => 80,
        Exp::Capture { .. } => 81,
        Exp::Variable { .. } => 82,
        Exp::FuncParam { .. } => 83,
        Exp::Function { .. } => 84,
        Exp::FunctionDecl { .. } => 85,
        Exp::Lambda { .. } => 86,
        Exp::ExportType { .. } => 87,
        Exp::Extern { .. } => 88,
        Exp::StaticAssert(..) => 89,
        Exp::StaticAssertLegacy(..) => 90,
        Exp::Warning { .. } => 91,
        Exp::Error(..) => 92,
        Exp::Path(..) => 93,
        Exp::Unsupported => 94,
    }
}

/// Compares two subtrees as the reference's derived `Ord` would.
#[derive(Debug)]
pub struct ExpOrd<'a> {
    pub arena: &'a ExpArena,
    pub interner: &'a Interner,
}

impl ExpOrd<'_> {
    /// The name a symbol spells.
    fn name(&self, s: Symbol) -> kg_base::SmolStr {
        self.interner.resolve(s)
    }

    fn names(&self, a: Symbol, b: Symbol) -> Ordering {
        self.name(a).cmp(&self.name(b))
    }

    fn qualified(&self, a: &QualifiedName, b: &QualifiedName) -> Ordering {
        let (a, b) = (a.segments(), b.segments());
        for (x, y) in a.iter().zip(b) {
            let o = self.names(*x, *y);
            if o != Ordering::Equal {
                return o;
            }
        }
        a.len().cmp(&b.len())
    }

    fn opt(&self, a: Option<ExpId>, b: Option<ExpId>) -> Ordering {
        match (a, b) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(a), Some(b)) => self.cmp(a, b),
        }
    }

    fn list(&self, a: &[ExpId], b: &[ExpId]) -> Ordering {
        for (x, y) in a.iter().zip(b) {
            let o = self.cmp(*x, *y);
            if o != Ordering::Equal {
                return o;
            }
        }
        a.len().cmp(&b.len())
    }

    fn string(tok: &str) -> std::borrow::Cow<'_, str> {
        match crate::exp::plain_string_value(tok) {
            Some(s) => std::borrow::Cow::Owned(s),
            None => std::borrow::Cow::Borrowed(tok),
        }
    }

    pub fn cmp(&self, a: ExpId, b: ExpId) -> Ordering {
        if a == b {
            return Ordering::Equal;
        }
        let (x, y) = (&self.arena[a].exp, &self.arena[b].exp);
        let by_ctor = ctor_index(x).cmp(&ctor_index(y));
        if by_ctor != Ordering::Equal {
            return by_ctor;
        }
        let then = |o: Ordering, rest: &dyn Fn() -> Ordering| if o == Ordering::Equal { rest() } else { o };
        match (x, y) {
            (Exp::IntLiteral { ty: t1, value: v1 }, Exp::IntLiteral { ty: t2, value: v2 }) => {
                // `Maybe LiteralType`, then `Integer`.
                let key = |t: &Option<kg_const::LiteralType>| t.map(|t| (t.sign, t.width));
                then(key(t1).cmp(&key(t2)), &|| v1.cmp(v2))
            }
            (Exp::FloatLiteral(f1), Exp::FloatLiteral(f2)) => {
                f1.to_f32().partial_cmp(&f2.to_f32()).unwrap_or(Ordering::Equal)
            }
            (Exp::BoolLiteral(b1), Exp::BoolLiteral(b2)) => b1.cmp(b2),
            (Exp::StringLiteral(s1), Exp::StringLiteral(s2)) | (Exp::InterpolatedString(s1), Exp::InterpolatedString(s2)) => {
                Self::string(s1).cmp(&Self::string(s2))
            }
            // One constructor in the reference, over its segment list: the
            // text, then the hole (`Maybe`'s `Nothing` first). A hole-free
            // string is one segment with no hole.
            (Exp::Interpolated(g1), Exp::Interpolated(g2)) => {
                for (a, b) in g1.iter().zip(g2.iter()) {
                    let o = then(Self::string(&a.text).cmp(&Self::string(&b.text)), &|| {
                        match (&a.hole, &b.hole) {
                            (None, None) => Ordering::Equal,
                            (None, Some(_)) => Ordering::Less,
                            (Some(_), None) => Ordering::Greater,
                            (Some(h1), Some(h2)) => then(self.cmp(h1.expr, h2.expr), &|| {
                                then(h1.echo.cmp(&h2.echo), &|| {
                                    then(self.opt(h1.align, h2.align), &|| h1.format.cmp(&h2.format))
                                })
                            }),
                        }
                    });
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                g1.len().cmp(&g2.len())
            }
            (Exp::InterpolatedString(s1), Exp::Interpolated(g2)) => {
                then(Self::string(s1).cmp(&Self::string(&g2[0].text)), &|| Ordering::Less)
            }
            (Exp::Interpolated(g1), Exp::InterpolatedString(s2)) => {
                then(Self::string(&g1[0].text).cmp(&Self::string(s2)), &|| Ordering::Greater)
            }
            (Exp::Identifier(s1), Exp::Identifier(s2)) => self.names(*s1, *s2),
            (Exp::Integer(n1), Exp::Integer(n2)) | (Exp::Unsigned(n1), Exp::Unsigned(n2)) => n1.cmp(n2),
            (Exp::FlagAttr(f1), Exp::FlagAttr(f2)) => f1.cmp(f2),
            (Exp::IntAttr(a1, e1), Exp::IntAttr(a2, e2)) | (Exp::Attr(a1, e1), Exp::Attr(a2, e2)) => {
                then(a1.cmp(a2), &|| self.cmp(*e1, *e2))
            }
            (Exp::Binary(o1, l1, r1), Exp::Binary(o2, l2, r2)) => {
                then(o1.cmp(o2), &|| then(self.cmp(*l1, *l2), &|| self.cmp(*r1, *r2)))
            }
            (Exp::Unary(o1, e1), Exp::Unary(o2, e2)) => then(o1.cmp(o2), &|| self.cmp(*e1, *e2)),
            (Exp::Positional { index: i1, value: v1 }, Exp::Positional { index: i2, value: v2 }) => {
                then(i1.cmp(i2), &|| self.cmp(*v1, *v2))
            }
            (Exp::ScopedName { scope: s1, prefix: p1, ident: i1 }, Exp::ScopedName { scope: s2, prefix: p2, ident: i2 }) => {
                then(self.qualified(s1, s2), &|| then(self.opt(*p1, *p2), &|| self.cmp(*i1, *i2)))
            }
            (Exp::TypeIdentifier(s1, e1), Exp::TypeIdentifier(s2, e2)) => {
                then(self.qualified(s1, s2), &|| self.cmp(*e1, *e2))
            }
            (Exp::This { scope: s1 }, Exp::This { scope: s2 }) => self.qualified(s1, s2),
            (
                Exp::TemplateParam { scope: s1, kind: k1, name: n1, index: i1, default: d1 },
                Exp::TemplateParam { scope: s2, kind: k2, name: n2, index: i2, default: d2 },
            ) => then(self.qualified(s1, s2), &|| {
                then(self.cmp(*k1, *k2), &|| then(self.names(*n1, *n2), &|| then(i1.cmp(i2), &|| self.opt(*d1, *d2))))
            }),
            (
                Exp::TypeParam { scope: s1, kind: k1, name: n1, index: i1 },
                Exp::TypeParam { scope: s2, kind: k2, name: n2, index: i2 },
            ) => then(self.qualified(s1, s2), &|| then(self.cmp(*k1, *k2), &|| then(self.names(*n1, *n2), &|| i1.cmp(i2)))),
            (Exp::Function { scope: s1, kind: k1, .. }, Exp::Function { scope: s2, kind: k2, .. }) => {
                then(self.qualified(s1, s2), &|| then(k1.cmp(k2), &|| self.children(x, y)))
            }
            (Exp::Variable { scope: s1, flags: f1, .. }, Exp::Variable { scope: s2, flags: f2, .. }) => {
                then(self.qualified(s1, s2), &|| then(self.children(x, y), &|| f1.cmp(f2)))
            }
            _ => {
                // The remaining constructors order by their declaration-scope
                // field first where they have one, then their children in
                // `ExpF` field order — which is [`Exp::children`]'s order.
                let scope = |e: &Exp| -> Option<QualifiedName> {
                    match e {
                        Exp::NestedScope { scope, .. }
                        | Exp::EnumConstant { scope, .. }
                        | Exp::Enum { scope, .. }
                        | Exp::Struct { scope, .. }
                        | Exp::Union { scope, .. }
                        | Exp::Class { scope, .. }
                        | Exp::Decltype { scope, .. }
                        | Exp::Alias { scope, .. }
                        | Exp::Capture { scope, .. }
                        | Exp::FuncParam { scope, .. }
                        | Exp::FunctionDecl { scope, .. }
                        | Exp::Lambda { scope, .. } => Some(scope.clone()),
                        _ => None,
                    }
                };
                let by_scope = match (scope(x), scope(y)) {
                    (Some(s1), Some(s2)) => self.qualified(&s1, &s2),
                    _ => Ordering::Equal,
                };
                then(by_scope, &|| self.children(x, y))
            }
        }
    }

    fn children(&self, x: &Exp, y: &Exp) -> Ordering {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        x.children(&mut a);
        y.children(&mut b);
        self.list(&a, &b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_base::{FileId, FileRange, Src, TextRange};

    fn leaf(arena: &mut ExpArena, exp: Exp) -> ExpId {
        let src = Src::own(FileRange::new(FileId(0), TextRange::default()));
        arena.alloc(exp, src, kg_syntax::SyntaxKind::ERROR)
    }

    /// `Map.toList`'s order over argument lists (`Template.hs:120`):
    /// `Integer` compares numerically, so `AndLiteral<|@2@32|>` precedes
    /// `AndLiteral<|@10@32|>` (`literal-operand.k`, the recorded
    /// reference); `BooleanF` precedes `UnsignedF` by constructor; an
    /// untyped literal precedes a typed one (`Nothing < Just`).
    #[test]
    fn argument_lists_order_as_the_reference_orders_its_map_keys() {
        let interner = Interner::new();
        let mut arena = ExpArena::default();
        let int = |arena: &mut ExpArena, v: u64| leaf(arena, Exp::IntLiteral { ty: None, value: kg_const::ConstInt::from(v) });
        let (two, ten, n32a, n32b) = (int(&mut arena, 2), int(&mut arena, 10), int(&mut arena, 32), int(&mut arena, 32));
        let a = leaf(&mut arena, Exp::Seq(vec![two, n32a]));
        let b = leaf(&mut arena, Exp::Seq(vec![ten, n32b]));
        let ord = ExpOrd { arena: &arena, interner: &interner };
        assert_eq!(ord.cmp(a, b), Ordering::Less);
        assert_eq!(ord.cmp(b, a), Ordering::Greater);
        let mut arena = ExpArena::default();
        let bool_ty = leaf(&mut arena, Exp::Boolean);
        let uint = leaf(&mut arena, Exp::Unsigned(1));
        let typed = leaf(&mut arena, Exp::IntLiteral { ty: Some(kg_const::LiteralType::unsigned(8)), value: kg_const::ConstInt::from(1u64) });
        let untyped = leaf(&mut arena, Exp::IntLiteral { ty: None, value: kg_const::ConstInt::from(200u64) });
        let ord = ExpOrd { arena: &arena, interner: &interner };
        assert_eq!(ord.cmp(bool_ty, uint), Ordering::Less);
        assert_eq!(ord.cmp(untyped, typed), Ordering::Less);
    }
}
