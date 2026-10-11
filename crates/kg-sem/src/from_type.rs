//! `expFromType` (`Type.hs:1612-1660`) and the note helpers a rewrite mints
//! with: a type spelled back as the tree the parser would have built for
//! it, every node noted as the reference notes it.
//!
//! The reference's three ways to note a minted node, over a donor `e`:
//!
//! | reference | here | type |
//! |---|---|---|
//! | `copyNote x e` (`Desugar.hs:103-104`) | [`Mint::copy_note`] | `e`'s |
//! | `newNote x e` (`Type.hs:983-984`) | [`Mint::new_note`] | `TUnresolved` |
//! | `resetTypeOfExp t . flip newNote e` | [`Mint::typed`] | `t` |
//!
//! Every minted node wears the donor's head, `Donated` — its text is not its
//! range's.

use kg_base::Symbol;
use kg_hir::rewrite::Rewriter;
use kg_hir::{Exp, ExpId};
use kg_ty::{QualifiedName, TAttr, TAttrSet, Ty};

use crate::types::TypeMap;

/// A rewriter and the notes beside it.
#[allow(missing_debug_implementations)]
pub struct Mint<'r, 'a> {
    pub rw: &'r mut Rewriter<'a>,
    pub types: &'r mut TypeMap,
}

impl Mint<'_, '_> {
    /// `copyNote x e`.
    pub fn copy_note(&mut self, donor: ExpId, exp: Exp) -> ExpId {
        let id = self.rw.donate(donor, exp);
        if self.types.is_set(donor) {
            let t = self.types.get(donor);
            self.types.reset_type_of(id, t);
        }
        id
    }

    /// `newNote x e`: the donor's position, no type.
    pub fn new_note(&mut self, donor: ExpId, exp: Exp) -> ExpId {
        self.rw.donate(donor, exp)
    }

    /// `resetTypeOfExp t (newNote x e)`.
    pub fn typed(&mut self, donor: ExpId, exp: Exp, t: Ty) -> ExpId {
        let id = self.rw.donate(donor, exp);
        self.types.reset_type_of(id, t);
        id
    }

    /// `scopedQualifier n e` (`Desugar.hs:182-185`): the chain of
    /// `ScopedName`s spelling `name`, each `copyNote`d from `e`.
    pub fn scoped_qualifier(&mut self, scope: &QualifiedName, donor: ExpId, name: &[Symbol]) -> Option<ExpId> {
        let mut chain = None;
        for &segment in name {
            let ident = self.copy_note(donor, Exp::Identifier(segment));
            chain = Some(self.copy_note(donor, Exp::ScopedName { scope: scope.clone(), prefix: chain, ident }));
        }
        chain
    }

    /// `typeIdentifier declScopeName name e` (`Desugar.hs:206-207`), as its
    /// `Exp` — the caller notes it.
    pub fn type_identifier_exp(&mut self, scope: &QualifiedName, donor: ExpId, name: &[Symbol]) -> Option<Exp> {
        let chain = self.scoped_qualifier(scope, donor, name)?;
        Some(Exp::TypeIdentifier(scope.clone(), chain))
    }

    /// `qualifiedIdentifier declScopeName name e` (`Type.hs:997-998`).
    pub fn qualified_identifier(&mut self, scope: &QualifiedName, donor: ExpId, name: &[Symbol]) -> Option<ExpId> {
        let chain = self.scoped_qualifier(scope, donor, name)?;
        Some(self.new_note(donor, Exp::QualifiedIdentifier(chain)))
    }

    /// `expFromType e t`, as its `Exp` (`typ`): `None` where the reference's
    /// is `Nothing`.
    pub fn exp_from_type(&mut self, donor: ExpId, t: &Ty) -> Option<Exp> {
        let none = QualifiedName::new(std::iter::empty());
        Some(match t {
            Ty::Auto => Exp::Auto,
            Ty::Type(x) | Ty::Dependent(x) => return self.exp_from_type(donor, x),
            Ty::Instance { concrete, .. } => return self.exp_from_type(donor, concrete),
            Ty::Void => Exp::Void,
            Ty::Boolean => Exp::Boolean,
            Ty::Float => Exp::Float,
            Ty::Str => Exp::Str,
            Ty::Signed(n) => Exp::Integer(*n),
            Ty::Unsigned(n) => Exp::Unsigned(*n),
            Ty::Const(x) => Exp::Const(self.typ(donor, x)?),
            Ty::Reference(s) => {
                let e = self.type_identifier_exp(&none, donor, s.segments())?;
                Exp::Reference(self.copy_note(donor, e))
            }
            Ty::Array { dims, .. } if dims.is_empty() => return None,
            Ty::Array { attrs, elem, dims } => {
                let attr = self.attributes(donor, attrs);
                let elem = self.typ(donor, elem)?;
                let items: Vec<ExpId> = dims
                    .iter()
                    .map(|d| {
                        let t = kg_ty::width::minimal_int_type(d);
                        self.typed(donor, Exp::IntLiteral { ty: None, value: d.clone() }, t)
                    })
                    .collect();
                let dims = self.typed(donor, Exp::Seq(items), Ty::Untyped);
                Exp::Array { attr, elem, dims }
            }
            Ty::Function { attrs, ret, params, .. } => {
                let attr = self.attributes(donor, attrs);
                let mut items = Vec::with_capacity(params.len());
                for p in params {
                    let attr = self.attributes(donor, &p.attrs);
                    let ty = self.typ(donor, &p.ty)?;
                    let name = p.name.map(|n| self.typed(donor, Exp::Identifier(n), Ty::Untyped));
                    items.push(self.typed(donor, Exp::FunctionTypeParam { attr, ty, name }, (*p.ty).clone()));
                }
                let params = self.typed(donor, Exp::Seq(items), Ty::Untyped);
                let ret = self.typ(donor, ret)?;
                Exp::FunctionType { attr, params, ret }
            }
            Ty::Closure { alias: s, .. }
            | Ty::Enum(s, _)
            | Ty::Struct(s, _)
            | Ty::Union(s, _)
            | Ty::Class(s, _) => return self.type_identifier_exp(&none, donor, s.segments()),
            Ty::Template
            | Ty::Initializer(_)
            | Ty::Designator(..)
            | Ty::Positional(..)
            | Ty::Error(_)
            | Ty::Undefined
            | Ty::Unresolved
            | Ty::Untyped => return None,
        })
    }

    /// `typ' x = noteWithType (TType x) <$> typ x`.
    fn typ(&mut self, donor: ExpId, x: &Ty) -> Option<ExpId> {
        let e = self.exp_from_type(donor, x)?;
        Some(self.typed(donor, e, Ty::Type(kg_ty::TyRef::new(x.clone()))))
    }

    /// `attributes = mapSeq (const TUntyped) Just . expFromTAttr e`.
    fn attributes(&mut self, donor: ExpId, attrs: &TAttrSet) -> ExpId {
        let none = QualifiedName::new(std::iter::empty());
        let mut items = Vec::with_capacity(attrs.len());
        for a in attrs {
            let e = match a {
                TAttr::Named(a, b, _) => match self.qualified_identifier(&none, donor, b.segments()) {
                    Some(q) => Exp::Attr(*a, q),
                    None => continue,
                },
                TAttr::Int(a, b) => {
                    let t = kg_ty::width::minimal_int_type(b);
                    Exp::IntAttr(*a, self.typed(donor, Exp::IntLiteral { ty: None, value: b.clone() }, t))
                }
                TAttr::Flag(f) => Exp::FlagAttr(*f),
            };
            items.push(self.typed(donor, e, Ty::Untyped));
        }
        self.typed(donor, Exp::Seq(items), Ty::Untyped)
    }
}
