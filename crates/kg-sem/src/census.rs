//! `decidedInstances` (`Template.hs:86-107`): every template instance the
//! program can instantiate this round, read off the IR and the engine's
//! types.
//!
//! ```haskell
//! decided e@(TemplateInstance a b@(Seq args)) x
//!     | all Map.null x && templateResolved symbols template b =
//!         Map.singleton templateQualifiedName directInstance
//!     | templateArgsResolved template args =
//!         Map.unionsWith Map.union $ Map.unionsWith Map.union x : indirectInstances template
//! decided _ (Note _ StaticIfF{..})  = staticIfCond
//! decided _ (Note _ StaticForF{..}) = forLimit
//! decided _ (Note _ TemplateF{..})  = Map.unionsWith Map.union templateParams
//! decided _ x                       = Map.unionsWith Map.union x
//! ```
//!
//! The result is keyed as the reference's is: the template's qualified name,
//! then the argument `Seq` under `NotedExp`'s structural equality — here the
//! rendering `kg_hir::structure::render_exp` gives it, which ignores notes as
//! the reference's `Eq` does (`Parser/Syntax.hs:597-599`). `Map.union` is
//! left-biased, so an instance's note is its first occurrence in program
//! order; every occurrence is kept besides, since `resolve` rewrites each
//! one (`Template.hs:158-164`).
//!
//! # A parameter's kind, substituted
//!
//! `templateResolved` asks whether each parameter's kind is resolved once
//! the template's parameters are replaced by the arguments
//! (`resolveTemplateParamType`, `TypeCheck.hs:349-362`), and
//! `indirectInstances` censuses those substituted kinds (`:98-100`) — the
//! `index_t<10>` of `template <auto N, index_t<N> M>` under `N = 10`. Both
//! are answered here without building the substituted tree: [`Cx::kind_type`]
//! and [`Cx::kind_value`] evaluate the kind with each parameter bound to its
//! argument ([`Bind`]) — a node of the using module, or a value or type the
//! evaluation already produced. An instance inside a kind is decided with
//! those evaluated arguments ([`Args::Minted`], made into nodes where the
//! template lives), and resolves — `resolveTemplateInstance` — only once
//! its copy exists, a round later, as in the reference.

use kg_base::{FxHashMap, Interner, Symbol};
use kg_const::{ConstValue, LiteralType};
use kg_hir::{Exp, ExpArena, ExpId, ModuleIr};
use kg_ty::{QualifiedName, Ty};

use crate::symbols::{Decl, Symbols};
use crate::types::TypeMap;

/// An argument the census evaluated rather than read off a written node.
#[derive(Clone, Debug, PartialEq)]
pub enum Minted {
    Value(ConstValue),
    Type(Ty),
}

/// Where an instance's arguments come from.
#[derive(Clone, Debug)]
pub enum Args {
    /// The first written occurrence: its module and its `TemplateInstance`.
    Written(usize, ExpId),
    /// Evaluated from a parameter's substituted kind (`indirectInstances`),
    /// with the instance node in the kind it was read from.
    Minted(Vec<Minted>, usize, ExpId),
}

/// One decided instance: the template, the arguments, where it was first
/// written, and every occurrence the census met.
#[derive(Clone, Debug)]
pub struct Decided {
    /// `getQualifiedName` of the template's declaration — what `instantiate`
    /// keys the template by.
    pub template: Vec<Symbol>,
    /// Where the template is declared.
    pub home: Decl,
    /// `render_exp` of the argument `Seq` — the structural key.
    pub key: String,
    /// The arguments the copy is made from.
    pub first: Args,
    /// Every written occurrence the census walked, in program order.
    pub sites: Vec<(usize, ExpId)>,
    /// `validateArg`'s literal re-stamp per argument (`TypeCheck.hs:281-283`).
    pub stamps: Vec<Option<LiteralType>>,
}

/// The census of one round.
#[derive(Debug, Default)]
pub struct Census {
    /// In the order the instances were first met.
    pub decided: Vec<Decided>,
    /// Instances decided out of a parameter's substituted kind.
    pub indirect: u32,
    /// Instances held back because a parameter's substituted kind is not
    /// resolved (yet).
    pub unresolved_kinds: u32,
    /// Instances whose arguments `validateArg` refuses — each an error the
    /// reference raises.
    pub invalid: u32,
    /// Each refusal: the argument `argError` renotes, and its message.
    pub refused: Vec<Refused>,
}

/// One argument `validateArg` refused (`TypeCheck.hs:228-293`).
#[derive(Clone, Debug)]
pub struct Refused {
    pub module: usize,
    pub arg: ExpId,
    pub message: String,
}

impl Census {
    pub fn is_empty(&self) -> bool {
        self.decided.is_empty()
    }
}

/// The modules as the census reads them.
#[derive(Debug)]
pub struct View<'a> {
    pub modules: &'a [(&'a ModuleIr, &'a TypeMap)],
    pub symbols: &'a Symbols,
    pub interner: &'a Interner,
}

/// `Map.unionsWith Map.union $ … $ parallel (para decided) program'`.
pub fn census(view: &View<'_>) -> Census {
    let mut out = Census::default();
    let mut index: FxHashMap<(Vec<Symbol>, String), usize> = FxHashMap::default();
    for (m, (ir, types)) in view.modules.iter().enumerate() {
        let _visit = crate::profile::visit("census", m, ir.arena.len());
        let cx = Cx { view, module: m, arena: &ir.arena, types };
        let mut found = Vec::new();
        cx.decided(ir.root, &mut found, &mut out);
        for f in found {
            match index.get(&(f.template.clone(), f.key.clone())) {
                Some(&i) => out.decided[i].sites.extend(f.sites),
                None => {
                    index.insert((f.template.clone(), f.key.clone()), out.decided.len());
                    out.decided.push(f);
                }
            }
        }
    }
    out
}

/// What a template's parameters are bound to while its kinds are evaluated.
#[derive(Clone, Copy)]
enum Bind<'b> {
    /// Argument nodes of the module being censused.
    Nodes(&'b [ExpId]),
    /// Arguments already evaluated.
    Minted(&'b [Minted]),
}

impl Bind<'_> {
    fn len(&self) -> usize {
        match self {
            Bind::Nodes(a) => a.len(),
            Bind::Minted(a) => a.len(),
        }
    }
}

struct Cx<'a> {
    view: &'a View<'a>,
    module: usize,
    arena: &'a ExpArena,
    types: &'a TypeMap,
}

impl Cx<'_> {
    fn exp(&self, id: ExpId) -> &Exp {
        &self.arena[id].exp
    }

    fn items(&self, id: ExpId) -> Vec<ExpId> {
        items_in(self.arena, id)
    }

    fn render(&self, path: &[Symbol]) -> Vec<String> {
        path.iter().map(|s| self.view.interner.resolve(*s).to_string()).collect()
    }

    /// `para decided`: the instances decided in `id`'s subtree, appended to
    /// `out` in program order. Returns whether anything was decided — the
    /// `all Map.null x` of the parent.
    fn decided(&self, id: ExpId, out: &mut Vec<Decided>, census: &mut Census) -> bool {
        // Only an instance decides anything, or counts or refuses one: a
        // subtree known to hold none is passed over (`kg_hir::facts`).
        if self.arena.known_to_lack(id, kg_hir::facts::TEMPLATE_INSTANCE) {
            return false;
        }
        let start = out.len();
        let arena = self.arena;
        match &arena[id].exp {
            &Exp::TemplateInstance { base, args } => {
                let below = self.decided(base, out, census) | self.decided(args, out, census);
                let Some((home, params)) = self.template_in(self.arena, base) else { return below };
                let items = self.items(args);
                let bind = Bind::Nodes(&items);
                if crate::env_flag!("KG_CENSUS_TRACE") {
                    let args: Vec<String> = items
                        .iter()
                        .map(|&a| {
                            let r = kg_hir::structure::render_exp(self.arena, self.view.interner, a);
                            format!("{} :: {:?}", r.lines().next().unwrap_or_default(), self.types.get(a))
                        })
                        .collect();
                    eprintln!(
                        "census {:?} below={below} args_resolved={} args={args:?}",
                        self.template_name(home).map(|p| self.render(&p)),
                        self.template_args_resolved(home, &params, bind)
                    );
                }
                if !below && self.template_resolved(home, &params, bind, census) {
                    // An argument `validateArg` refuses is an error node by
                    // the census (`Template.hs:71`, the pass list's last),
                    // so the instance is neither direct nor indirect — and
                    // each argument is validated on its own (`zipWith
                    // validateArg`, `TypeCheck.hs:226`).
                    let refused = self.refusals(home, &params, &items);
                    if refused.is_empty() {
                        self.direct(id, home, &params, args, &items, out);
                    } else {
                        census.invalid += 1;
                        for (arg, message) in refused {
                            census.refused.push(Refused { module: self.module, arg, message });
                        }
                    }
                } else if self.template_args_resolved(home, &params, bind) {
                    self.indirect(home, &params, bind, out, census);
                }
            }
            &Exp::StaticIf { cond, .. } => {
                self.decided(cond, out, census);
            }
            &Exp::StaticFor { limit, .. } => {
                self.decided(limit, out, census);
            }
            Exp::Template { params, .. } => {
                for &p in params {
                    self.decided(p, out, census);
                }
            }
            exp => {
                let mut kids = Vec::new();
                exp.children(&mut kids);
                for k in kids {
                    self.decided(k, out, census);
                }
            }
        }
        out.len() > start
    }

    /// `directInstance`.
    fn direct(&self, id: ExpId, home: Decl, params: &[ExpId], args: ExpId, items: &[ExpId], out: &mut Vec<Decided>) {
        let Some(template) = self.template_name(home) else { return };
        let key = kg_hir::structure::render_exp(self.arena, self.view.interner, args);
        if crate::env_flag!("KG_CENSUS_DECIDED") {
            eprintln!("decided {:?} at {:?} key {}", self.render(&template), self.arena[id].src.head, key.replace('\n', " "));
        }
        let stamps = params.iter().zip(items).map(|(&p, &a)| self.stamp(home, p, Some(a), Bind::Nodes(items))).collect();
        out.push(Decided {
            template,
            home,
            key,
            first: Args::Written(self.module, id),
            sites: vec![(self.module, id)],
            stamps,
        });
    }

    /// `indirectInstances template = map (para decided . resolveTemplateParam
    /// symbols t args) params` (`Template.hs:98-100`): every instance inside
    /// a parameter's kind, its arguments evaluated under the substitution,
    /// decided where its own template is resolved over them.
    fn indirect(&self, home: Decl, params: &[ExpId], bind: Bind<'_>, out: &mut Vec<Decided>, census: &mut Census) {
        self.indirect_at(home, params, bind, out, census, 0);
    }

    fn indirect_at(
        &self,
        home: Decl,
        params: &[ExpId],
        bind: Bind<'_>,
        out: &mut Vec<Decided>,
        census: &mut Census,
        depth: u32,
    ) {
        let (arena, _) = self.home(home);
        let Some(scope) = self.template_scope(home) else { return };
        for &p in params {
            let mut stack = vec![p];
            let mut kids = Vec::new();
            while let Some(k) = stack.pop() {
                if let Exp::TemplateInstance { base, args } = &arena[k].exp
                    && let Some(minted) = self.mint_args(home, &scope, *args, bind)
                    && let Some((inner, inner_params)) = self.template_of(home, &scope, *base, bind)
                    && !self.template_resolved(inner, &inner_params, Bind::Minted(&minted), census)
                    && self.template_args_resolved(inner, &inner_params, Bind::Minted(&minted))
                    && depth < 8
                {
                    // `para decided` over the substituted kind meets this
                    // instance as it meets any other: not resolved itself,
                    // its own kinds' instances come first.
                    self.indirect_at(inner, &inner_params, Bind::Minted(&minted), out, census, depth + 1);
                } else if let Exp::TemplateInstance { base, args } = &arena[k].exp
                    && let Some(minted) = self.mint_args(home, &scope, *args, bind)
                    && let Some((inner, inner_params)) = self.template_of(home, &scope, *base, bind)
                    && self.template_resolved(inner, &inner_params, Bind::Minted(&minted), census)
                    && let Some(template) = self.template_name(inner)
                {
                    let stamps: Vec<Option<LiteralType>> = inner_params
                        .iter()
                        .zip(&minted)
                        .map(|(&q, m)| match m {
                            Minted::Value(ConstValue::Int { .. }) => self.stamp(inner, q, None, Bind::Minted(&minted)),
                            _ => None,
                        })
                        .collect();
                    if let Some(key) = self.minted_key(&minted) {
                        census.indirect += 1;
                        if crate::env_flag!("KG_CENSUS_DECIDED") {
                            eprintln!("decided (indirect) {:?} key {}", self.render(&template), key.replace('\n', " "));
                        }
                        out.push(Decided {
                            template,
                            home: inner,
                            key,
                            first: Args::Minted(minted, home.module, k),
                            sites: Vec::new(),
                            stamps,
                        });
                    }
                }
                kids.clear();
                arena[k].exp.children(&mut kids);
                stack.extend(kids.iter().copied());
            }
        }
    }

    /// The arguments of an instance inside a kind, evaluated under `bind`:
    /// each a value where it folds, else a type.
    fn mint_args(&self, home: Decl, scope: &QualifiedName, args: ExpId, bind: Bind<'_>) -> Option<Vec<Minted>> {
        let (arena, _) = self.home(home);
        items_in(arena, args)
            .into_iter()
            .map(|a| match self.kind_value(home, scope, a, bind) {
                Some(v) => Some(Minted::Value(v)),
                None => self.kind_type(home, scope, a, bind).filter(Ty::resolved).map(Minted::Type),
            })
            .collect()
    }

    /// Minted arguments as the nodes a written list would be, in a scratch
    /// arena: what [`Self::minted_key`] renders and the mangle reads.
    fn scratch(&self, minted: &[Minted]) -> Option<(ExpArena, ExpId)> {
        let mut arena = ExpArena::new();
        let src = kg_base::Src::own(kg_base::FileRange::new(kg_base::FileId(0), kg_base::TextRange::new(0.into(), 0.into())));
        let donor = arena.alloc(Exp::Seq(Vec::new()), src, kg_syntax::SyntaxKind::ERROR);
        let mut types = TypeMap::new();
        let mut items = Vec::with_capacity(minted.len());
        {
            let mut rw = kg_hir::rewrite::Rewriter::new(&mut arena, self.view.interner);
            let mut m = crate::from_type::Mint { rw: &mut rw, types: &mut types };
            for a in minted {
                let exp = match a {
                    Minted::Value(v) => minted_literal(v)?,
                    Minted::Type(t) => m.exp_from_type(donor, t)?,
                };
                items.push(m.new_note(donor, exp));
            }
        }
        let seq = arena.alloc(Exp::Seq(items), src, kg_syntax::SyntaxKind::ERROR);
        Some((arena, seq))
    }

    fn minted_key(&self, minted: &[Minted]) -> Option<String> {
        let (arena, seq) = self.scratch(minted)?;
        Some(kg_hir::structure::render_exp(&arena, self.view.interner, seq))
    }

    /// `lookupSymbol a symbols`, when it names a template: the template and
    /// its parameters — `a` a node of `arena`.
    fn template_in(&self, arena: &ExpArena, base: ExpId) -> Option<(Decl, Vec<ExpId>)> {
        let (scope, name) = match &arena[base].exp {
            Exp::ScopedName { scope, .. } => (scope.clone(), kg_hir::sort::spine_qualname(arena, base)),
            Exp::QualifiedIdentifier(a) | Exp::TypeIdentifier(_, a) => match &arena[*a].exp {
                Exp::ScopedName { scope, .. } => (scope.clone(), kg_hir::sort::spine_qualname(arena, *a)),
                _ => return None,
            },
            _ => return None,
        };
        if name.is_empty() {
            return None;
        }
        let d = self.view.symbols.in_scope(scope.segments(), &name)?;
        let (home, _) = self.home(d);
        match &home[d.id].exp {
            Exp::Template { params, .. } => Some((d, params.clone())),
            _ => None,
        }
    }

    /// The template an instance in a kind names: written, or a template
    /// parameter of this template bound to the template its argument names
    /// — `() -> TT<T> fn` under `TT = S1` is `() -> S1<uint32>`
    /// (`template-template-parameter.k::34`'s `FnFn<uint32, TT, Do>`, which
    /// the reference makes; held back, it was left standing).
    fn template_of(&self, home: Decl, scope: &QualifiedName, base: ExpId, bind: Bind<'_>) -> Option<(Decl, Vec<ExpId>)> {
        let (arena, _) = self.home(home);
        if let Some(found) = self.template_in(arena, base) {
            return Some(found);
        }
        let mut at = base;
        while let Exp::QualifiedIdentifier(a) | Exp::TypeIdentifier(_, a) = &arena[at].exp {
            at = *a;
        }
        let Exp::TypeParam { scope: s, index, .. } = &arena[at].exp else { return None };
        if s != scope {
            return None;
        }
        let Bind::Nodes(args) = bind else { return None };
        let arg = *args.get(*index as usize)?;
        self.template_in(self.arena, arg)
    }

    fn home(&self, d: Decl) -> (&ExpArena, &TypeMap) {
        let (ir, types) = self.view.modules[d.module];
        (&ir.arena, types)
    }

    /// `getQualifiedName` of the template's declaration.
    fn template_name(&self, home: Decl) -> Option<Vec<Symbol>> {
        let (arena, _) = self.home(home);
        let (scope, name) = crate::symbols::declared_name(arena, home.id)?;
        let mut path = scope.segments().to_vec();
        path.push(name);
        Some(path)
    }

    /// The parameter's kind node and its type, off the template's module.
    fn param_kind(&self, home: Decl, param: ExpId) -> Option<(ExpId, Ty)> {
        let (arena, types) = self.home(home);
        match &arena[param].exp {
            Exp::TemplateParam { kind, .. } => Some((*kind, types.get(*kind))),
            _ => None,
        }
    }

    /// `templateResolved` (`TypeCheck.hs:299-310`).
    fn template_resolved(&self, home: Decl, params: &[ExpId], bind: Bind<'_>, census: &mut Census) -> bool {
        if !self.template_args_resolved(home, params, bind) {
            return false;
        }
        let (arena, _) = self.home(home);
        let Some(scope) = self.template_scope(home) else { return false };
        for &p in params {
            let Some((kind, t)) = self.param_kind(home, p) else { return false };
            match &arena[kind].exp {
                Exp::Typename | Exp::Template { .. } | Exp::Auto => continue,
                _ if t.resolved() => continue,
                _ if self.kind_type(home, &scope, kind, bind).is_some_and(|t| t.resolved()) => continue,
                _ => {
                    census.unresolved_kinds += 1;
                    return false;
                }
            }
        }
        true
    }

    /// The type of a parameter's kind with this template's parameters
    /// substituted, as inference and `evaluateConstexpr` would leave it
    /// (`resolveTemplateParamType`, `TypeCheck.hs:349-362`), computed without
    /// building the tree: a parameter is its argument's type; `uint<e>` /
    /// `int<e>` take `e`'s folded value — `clog2(N)`, `bitsizeof T` and
    /// arithmetic fold ([`Self::kind_value`]); a function type, an array and
    /// `const` compose; an `auto` inside a function type is `TAuto`; an
    /// instance is the copy it names, once that copy exists
    /// (`resolveTemplateInstance`).
    fn kind_type(&self, home: Decl, scope: &QualifiedName, kind: ExpId, bind: Bind<'_>) -> Option<Ty> {
        let (arena, types) = self.home(home);
        Some(match &arena[kind].exp {
            // `resolveTypeParam` substitutes the argument's node itself
            // (`TypeCheck.hs:334-339`), so the kind is typed as the argument
            // is, and `theType` of a resolved value is `TError "Expected a
            // type"` (`Type.hs:458-464`): `Foo<10, 10>` over `<typename T, T
            // X>` is resolved, and `X`'s argument is refused against that
            // type (`type-check.k::19`, `kg-oracle --errors`).
            Exp::TypeParam { scope: s, index, .. } if s == scope => match bind {
                Bind::Nodes(args) => match self.types.get(*args.get(*index as usize)?) {
                    Ty::Type(t) => (*t).clone(),
                    other if other.resolved() => other.the_type(),
                    _ => return None,
                },
                Bind::Minted(args) => match args.get(*index as usize)? {
                    Minted::Type(t) => t.clone(),
                    Minted::Value(_) => return None,
                },
            },
            Exp::Const(k) => Ty::Const(kg_ty::TyRef::new(self.kind_type(home, scope, *k, bind)?)),
            // `infer (ParamIntF x) = try (TType . TSigned) $
            // fromIntegralConstexpr x` (`Type.hs:1246-1247`): a zero width
            // is a type, `int0` — only the fold asks for a natural
            // (`Constexpr.hs:300-303`) — so `NFoo<0, 10>`'s `int<N> X` wants
            // `int0` (`type-check.k::40`).
            Exp::ParamUint(x) | Exp::ParamInt(x) => {
                let w = self.kind_value(home, scope, *x, bind)?.as_int()?.to_u32()?;
                if matches!(arena[kind].exp, Exp::ParamInt(_)) { Ty::Signed(w) } else { Ty::Unsigned(w) }
            }
            Exp::Unsigned(n) => Ty::Unsigned(*n),
            Exp::Integer(n) => Ty::Signed(*n),
            Exp::Boolean => Ty::Boolean,
            Exp::Float => Ty::Float,
            Exp::Str => Ty::Str,
            Exp::Void => Ty::Void,
            Exp::Auto => Ty::Auto,
            Exp::Array { elem, dims, .. } => {
                let elem = self.kind_type(home, scope, *elem, bind)?;
                let mut ds = Vec::new();
                for &d in &items_in(arena, *dims) {
                    ds.push(self.kind_value(home, scope, d, bind)?.as_natural()?.clone());
                }
                Ty::Array { attrs: kg_ty::TAttrSet::new(), elem: kg_ty::TyRef::new(elem), dims: ds }
            }
            Exp::FunctionType { attr, params, ret } => {
                let mut ps = Vec::new();
                for &p in &items_in(arena, *params) {
                    let Exp::FunctionTypeParam { attr, ty, .. } = &arena[p].exp else { return None };
                    let t = self.kind_type(home, scope, *ty, bind)?;
                    // `typeOf p == TVoid = Nothing` (`Type.hs:1352-1353`).
                    if t == Ty::Void {
                        continue;
                    }
                    let attrs = self.kind_attrs(home, scope, *attr, bind, true);
                    ps.push(kg_ty::TFuncParam { attrs, ty: kg_ty::TyRef::new(t), name: None });
                }
                let ret = self.kind_type(home, scope, *ret, bind)?;
                Ty::Function {
                    kind: kg_ty::FunctionKind::FreeFunction,
                    attrs: self.kind_attrs(home, scope, *attr, bind, false),
                    ret: kg_ty::TyRef::new(ret),
                    params: ps,
                }
            }
            Exp::TypeIdentifier(_, a) if matches!(arena[*a].exp, Exp::TemplateInstance { .. }) => {
                return self.kind_type(home, scope, *a, bind);
            }
            Exp::TypeIdentifier(_, a) if param_rooted(arena, *a, scope) => {
                return self.member_type(home, scope, *a, bind);
            }
            Exp::TemplateInstance { base, args } => return self.instance_type(home, scope, *base, *args, bind),
            _ => {
                let t = types.get(kind);
                if !t.resolved() {
                    return None;
                }
                t.the_type()
            }
        })
    }

    /// A function type's attributes in a kind — `flags` for a parameter's
    /// (`Type.hs:1335-1340`), `attributes` for the function's
    /// (`:1328-1333`) — as far as they are evaluated here: a flag, and an
    /// integer attribute whose value folds. What is left out only widens
    /// the type `validateArg` compares against (`<:` asks the parameter's
    /// attributes to be a subset of the argument's, `Type.hs:333-334`).
    fn kind_attrs(&self, home: Decl, scope: &QualifiedName, attr: ExpId, bind: Bind<'_>, flags_only: bool) -> kg_ty::TAttrSet {
        let (arena, _) = self.home(home);
        let mut out = kg_ty::TAttrSet::new();
        for a in items_in(arena, attr) {
            let mut e = a;
            loop {
                match &arena[e].exp {
                    Exp::FlagAttr(f) => {
                        out.insert(kg_ty::TAttr::Flag(*f));
                    }
                    Exp::IntAttr(x, y) if !flags_only => {
                        if let Some(v) = self.kind_value(home, scope, *y, bind).and_then(|v| v.as_int().cloned()) {
                            out.insert(kg_ty::TAttr::Int(*x, v));
                        }
                    }
                    Exp::Warning { node, .. } if flags_only => {
                        e = *node;
                        continue;
                    }
                    _ => {}
                }
                break;
            }
        }
        out
    }

    /// A type named through a parameter — `T::Foo::X` — under the class the
    /// parameter's argument names: `Bar::Foo::X` under `T = Bar`, the
    /// declaration's own type (`template.k::42`'s `T::Foo::X N`, which
    /// `resolveTemplateParamType` substitutes and inference then types).
    fn member_type(&self, home: Decl, scope: &QualifiedName, name: ExpId, bind: Bind<'_>) -> Option<Ty> {
        let (arena, _) = self.home(home);
        let mut names = Vec::new();
        let mut at = name;
        while let Exp::ScopedName { prefix, ident, .. } = &arena[at].exp {
            names.push(kg_hir::rewrite::get_name(arena, *ident)?);
            at = (*prefix)?;
        }
        let Exp::TypeParam { scope: s, index, .. } = &arena[at].exp else { return None };
        if s != scope {
            return None;
        }
        let class = match bind {
            Bind::Nodes(args) => match self.types.get(*args.get(*index as usize)?) {
                Ty::Type(t) => (*t).clone(),
                _ => return None,
            },
            Bind::Minted(args) => match args.get(*index as usize)? {
                Minted::Type(t) => t.clone(),
                Minted::Value(_) => return None,
            },
        };
        let mut path = class.concrete().qualified_name()?.segments().to_vec();
        path.extend(names.into_iter().rev());
        let d = self.view.symbols.by_name(&kg_base::SymbolPath::new(path))?;
        let (_, types) = self.home(d);
        let t = match types.get(d.id) {
            Ty::Type(t) => (*t).clone(),
            other => other,
        };
        t.resolved().then_some(t)
    }

    /// `resolveTemplateInstance` (`TypeCheck.hs:370-376`) inside a kind: the
    /// instance names its copy once the copy exists — the template's path
    /// with its leaf mangled over the evaluated arguments — and is typed as
    /// the copy is.
    fn instance_type(&self, home: Decl, scope: &QualifiedName, base: ExpId, args: ExpId, bind: Bind<'_>) -> Option<Ty> {
        let minted = self.mint_args(home, scope, args, bind)?;
        let (inner, inner_params) = self.template_of(home, scope, base, bind)?;
        let path = self.template_name(inner)?;
        let stamps: Vec<Option<LiteralType>> = inner_params
            .iter()
            .zip(&minted)
            .map(|(&q, m)| match m {
                Minted::Value(ConstValue::Int { .. }) => self.stamp(inner, q, None, Bind::Minted(&minted)),
                _ => None,
            })
            .collect();
        let (scratch, seq) = self.scratch(&minted)?;
        let (&leaf, qualifier) = path.split_last()?;
        let dotted: Vec<String> = path.iter().map(|s| kg_base::unmangle_namespace(&self.view.interner.resolve(*s))).collect();
        let is_closure = dotted.join(".") == "data.closure.core.Closure";
        let body = kg_hir::rewrite::mangle_args(&scratch, self.view.interner, seq, is_closure, &stamps)?;
        let mangled = self.view.interner.intern(&format!("{}{}", self.view.interner.resolve(leaf), kg_hir::rewrite::resolve::instance_suffix(&body)));
        let mut copy = qualifier.to_vec();
        copy.push(mangled);
        let d = self.view.symbols.by_name(&kg_base::SymbolPath::new(copy))?;
        let (_, types) = self.home(d);
        let t = types.get(d.id);
        let t = match &t {
            Ty::Type(inner) => (**inner).clone(),
            other => other.clone(),
        };
        let t = t.concrete().clone();
        t.resolved().then_some(t)
    }

    /// The value of a constant expression in a parameter's kind, the
    /// parameters substituted — `evaluateConstexpr` over the substituted
    /// kind.
    fn kind_value(&self, home: Decl, scope: &QualifiedName, id: ExpId, bind: Bind<'_>) -> Option<ConstValue> {
        let (arena, _) = self.home(home);
        match &arena[id].exp {
            Exp::IntLiteral { ty, value } => Some(ConstValue::Int { ty: *ty, value: value.clone() }),
            Exp::BoolLiteral(b) => Some(ConstValue::Bool(*b)),
            Exp::TypeParam { scope: s, index, .. } if s == scope => match bind {
                Bind::Nodes(args) => {
                    let arg = *args.get(*index as usize)?;
                    kg_hir::constexpr::value(self.arena, arg, &kg_hir::constexpr::Closed)
                }
                Bind::Minted(args) => match args.get(*index as usize)? {
                    Minted::Value(v) => Some(v.clone()),
                    Minted::Type(_) => None,
                },
            },
            Exp::NamedValue(x) | Exp::QualifiedIdentifier(x) => self.kind_value(home, scope, *x, bind),
            Exp::Unary(op @ (kg_const::UnOp::BitSizeof | kg_const::UnOp::ByteSizeof), x) => {
                let t = self.kind_type(home, scope, *x, bind)?;
                kg_hir::constexpr::size(*op, kg_ty::subtype::bit_sizeof(&t)?)
            }
            Exp::Unary(op, x) => kg_hir::constexpr::unary(*op, &self.kind_value(home, scope, *x, bind)?),
            Exp::Binary(op, a, b) => kg_hir::constexpr::binary(
                *op,
                &self.kind_value(home, scope, *a, bind)?,
                &self.kind_value(home, scope, *b, bind)?,
            ),
            Exp::Cast { ty, value } => {
                let t = self.kind_type(home, scope, *ty, bind)?;
                kg_hir::constexpr::cast(&t, &self.kind_value(home, scope, *value, bind)?)
            }
            _ => None,
        }
    }

    /// `getQualifier template`: the declaration's scope, which its
    /// parameters are scoped with.
    fn template_scope(&self, home: Decl) -> Option<QualifiedName> {
        let (arena, _) = self.home(home);
        let Exp::Template { decl, .. } = &arena[home.id].exp else { return None };
        kg_hir::rewrite::decl_scope_of(arena, *decl).cloned()
    }

    /// `templateArgsResolved` (`TypeCheck.hs:326-332`).
    fn template_args_resolved(&self, home: Decl, params: &[ExpId], bind: Bind<'_>) -> bool {
        if params.len() != bind.len() {
            return false;
        }
        let (arena, _) = self.home(home);
        params.iter().enumerate().all(|(i, &p)| {
            let template_kind = matches!(&arena[p].exp, Exp::TemplateParam { kind, .. }
                if matches!(arena[*kind].exp, Exp::Template { .. }));
            match bind {
                Bind::Nodes(args) => {
                    let a = args[i];
                    if template_kind { self.types.get(a).is_template() } else { self.arg_resolved(a) }
                }
                // An evaluated argument is a value or a resolved type; a
                // template is never one.
                Bind::Minted(_) => !template_kind,
            }
        })
    }

    /// `templateArgResolved` (`TypeCheck.hs:318-324`).
    fn arg_resolved(&self, a: ExpId) -> bool {
        let t = self.types.get(a);
        t.is_function() || t.is_type() || t.is_template() || (t.resolved() && self.constexpr_resolved(a))
    }

    /// `constexprResolved` (`Constexpr.hs:385-390`).
    fn constexpr_resolved(&self, id: ExpId) -> bool {
        match self.exp(id) {
            Exp::InitializerList(items) => self.items(*items).iter().all(|&i| self.constexpr_resolved(i)),
            Exp::Designator { value, .. } | Exp::Positional { value, .. } => self.constexpr_resolved(*value),
            Exp::IntLiteral { .. }
            | Exp::BoolLiteral(_)
            | Exp::FloatLiteral(_)
            | Exp::StringLiteral(_)
            | Exp::EnumValue { .. } => true,
            // A string expression with no hole is the reference's
            // `StringLiteralF` by now (`interpolatedStringPre`,
            // `Frontend.hs:233-234`); this IR keeps it as written.
            Exp::InterpolatedString(tok) => kg_hir::plain_string_value(tok).is_some(),
            _ => false,
        }
    }

    /// `templateTypeCheck`'s `validateArg` (`TypeCheck.hs:228-293`) over a
    /// resolved instance's written arguments: every argument it answers
    /// `argError` for — each on its own, `zipWith validateArg` (`:226`) —
    /// with `argError`'s message (`:232`). Asked: a name where a type is
    /// expected and the reverse (`:233-238`, `:262-263`); a template whose
    /// parameters do not match a template-template parameter's
    /// (`validateTemplate`, `:240-259`); a method or a function not of the
    /// parameter's substituted type (`:265-275`); an initializer list for an
    /// `auto` parameter (`:277`); and a written literal, a folded enum value
    /// or an initializer list not of the parameter's type (`:279-293`). A
    /// constant not yet folded passes: a copy the reference would refuse
    /// rather than an error it would not raise.
    fn refusals(&self, home: Decl, params: &[ExpId], items: &[ExpId]) -> Vec<(ExpId, String)> {
        let mut out = Vec::new();
        let (arena, _) = self.home(home);
        let Some(scope) = self.template_scope(home) else { return out };
        let note = |d: crate::symbols::Decl| {
            let (_, types) = self.view.modules.get(d.module)?;
            types.is_set(d.id).then(|| types.get(d.id))
        };
        let render = |t: &Ty| {
            kg_ty::subtype::show_unmangled_with(t, Some(self.view.interner), &|name| self.view.symbols.noted(name, note))
        };
        for (&p, &a) in params.iter().zip(items) {
            let Exp::TemplateParam { kind, name, .. } = &arena[p].exp else { continue };
            let (kind, name) = (*kind, *name);
            let arg = self.types.get(a);
            let why = match &arena[kind].exp {
                // `validate Typename NamedValueF{} | tIsType …` (`:233-234`):
                // a name written where a type is expected, without
                // `typename`, is read as a value even when it names a type
                // (`template.k::41`'s `C<T::Foo>`).
                Exp::Typename if arg.is_type() && matches!(self.exp(a), Exp::NamedValue(_)) => {
                    Some("Expected a type; did you forget 'typename`".to_owned())
                }
                // `| otherwise = argError "Expected a type"` (`:236-238`),
                // whatever else `templateArgResolved` let through — a
                // template's name (`tIsTemplate`, which is no type) and a
                // function among them (`Type.hs:599-602,620-621`).
                Exp::Typename if !arg.is_type() => Some("Expected a type".to_owned()),
                Exp::Typename => None,
                // `validate (Template params1 _) arg | tIsTemplate …`
                // (`:240-259`). An argument that is no template never gets
                // here: `templateArgsResolved` asks `tIsTemplate` of it
                // (`:329-330`), and the instance is not resolved.
                Exp::Template { params: wanted, .. } if arg.is_template() => self.template_mismatch(home, &scope, items, wanted, a),
                Exp::Template { .. } => None,
                _ if arg.is_type() => {
                    Some("The type can't be used as an argument for non-type template parameter".to_owned())
                }
                Exp::FunctionType { .. } if !arg.is_template() => {
                    let got = arg.un_const();
                    if got.is_method() {
                        Some("Expected a non-member function".to_owned())
                    } else {
                        match self.kind_type(home, &scope, kind, Bind::Nodes(items)) {
                            Some(want) if want.resolved() && got.resolved() && !kg_ty::subtype::is_subtype(&got, &want) => {
                                Some(format!(
                                    "Expected a function of type:\n\t{}\nbut the argument type is:\n\t{}",
                                    render(&want),
                                    render(&got)
                                ))
                            }
                            _ => None,
                        }
                    }
                }
                Exp::FunctionType { .. } => None,
                // `validate Auto InitializerListF{}` (`:277`).
                Exp::Auto if matches!(self.exp(a), Exp::InitializerList(_)) => {
                    Some("Initializer list can't be used as an argument for `auto` parameter".to_owned())
                }
                Exp::Auto => None,
                _ if matches!(
                    self.exp(a),
                    Exp::IntLiteral { .. }
                        | Exp::BoolLiteral(_)
                        | Exp::FloatLiteral(_)
                        | Exp::StringLiteral(_)
                        | Exp::EnumValue { .. }
                        | Exp::InitializerList(_)
                ) =>
                {
                    let want = self.param_type(home, &scope, p, kind, Bind::Nodes(items)).map(|t| t.un_const());
                    let got = arg.un_const();
                    match want {
                        Some(want) if want.resolved() && got.resolved() && !matches!(want, Ty::Auto) => {
                            // `:281-283`: an integer literal under an integer
                            // kind (`tIsSigned`/`tIsUnsigned`, through `const`
                            // and an instance, never an enum) is re-stamped.
                            // A zero width is no stamp (`width > 0`, `:281-282`).
                            let stamped = matches!(self.exp(a), Exp::IntLiteral { .. })
                                && want.is_int()
                                && kg_ty::width::t_width(&want).is_some_and(|w| w > 0);
                            (!stamped && !kg_ty::subtype::is_subtype(&got, &want)).then(|| {
                                format!(
                                    "Expected a constant expression of type:\n\t{}\nbut the argument type is:\n\t{}",
                                    render(&want),
                                    render(&got)
                                )
                            })
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(why) = why {
                let name = self.view.interner.resolve(name);
                out.push((a, format!("Invalid argument for template parameter `{name}`.\n{why}")));
            }
        }
        out
    }

    /// `validateTemplate` (`TypeCheck.hs:243-257`): the template `a` names
    /// matches the parameter list `wanted` kind by kind — `matchParamKind t1
    /// (TemplateParam _ t2 _ _ _)`, equal as written (notes ignored) or as
    /// `theType` — and any parameter of its own past `wanted`'s has a
    /// default. The message lists both as `showPretty` spells them
    /// (`<typename, auto>` against `<typename T>`,
    /// `template-template-parameter.k::3`); `None` where they match, where
    /// `a` names no template here, or where a list holds a shape
    /// [`pretty_param`] cannot spell — refused nothing, as before.
    fn template_mismatch(&self, home: Decl, scope: &QualifiedName, items: &[ExpId], wanted: &[ExpId], a: ExpId) -> Option<String> {
        let (found, given) = self.template_in(self.arena, a)?;
        let (warena, _) = self.home(home);
        let (garena, gtypes) = self.home(found);
        let interner = self.view.interner;
        let bind = Bind::Nodes(items);
        let kind_of = |arena: &ExpArena, p: ExpId| match &arena[p].exp {
            Exp::TemplateParam { kind, .. } => *kind,
            _ => p,
        };
        // The wanted kind is `resolveTemplateParamType`'s, the template's own
        // parameters substituted (`TypeCheck.hs:229`): `template <T>
        // typename TT` under `T = uint32` wants `<uint32>`
        // (`template-template-parameter.k::16`).
        //
        // `theType` of the kind itself, not `kind_type`'s reading of one
        // inside a function type: `infer AutoF = TUnresolved` (`Type.hs:
        // 1174`), which `tType` keeps (`:458-464`), so an `auto` slot is the
        // `TUnresolved` a template-template parameter's `TTemplate` is too
        // (`tResolved TTemplate = False`, `:1002`) — `Z<W>` over `template
        // <template <typename, auto> typename TT>` and `W`'s `template
        // <typename> typename Y` matches.
        let wanted_type = |x: ExpId| match &warena[kind_of(warena, x)].exp {
            Exp::Auto => Some(Ty::Unresolved),
            _ => self.kind_type(home, scope, kind_of(warena, x), bind),
        };
        let given_type = |y: ExpId| {
            let k = kind_of(garena, y);
            gtypes.is_set(k).then(|| gtypes.get(k).the_type())
        };
        let same = |x: ExpId, y: ExpId| {
            let written = kg_hir::structure::render_exp(warena, interner, kind_of(warena, x))
                == kg_hir::structure::render_exp(garena, interner, kind_of(garena, y));
            written || matches!((wanted_type(x), given_type(y)), (Some(s), Some(t)) if s == t)
        };
        let defaulted = |y: ExpId| matches!(&garena[y].exp, Exp::TemplateParam { default: Some(_), .. });
        let matched = wanted.len() <= given.len()
            && wanted.iter().zip(&given).all(|(&x, &y)| same(x, y))
            && given[wanted.len()..].iter().all(|&y| defaulted(y));
        if matched {
            return None;
        }
        let note = |d: crate::symbols::Decl| {
            let (_, types) = self.view.modules.get(d.module)?;
            types.is_set(d.id).then(|| types.get(d.id))
        };
        let render = |t: &Ty| {
            kg_ty::subtype::show_unmangled_with(t, Some(interner), &|name| self.view.symbols.noted(name, note))
        };
        let wanted_spelled: Option<Vec<String>> = wanted
            .iter()
            .map(|&x| pretty_param(warena, interner, x).or_else(|| wanted_type(x).filter(Ty::resolved).map(|t| render(&t))))
            .collect();
        let given_spelled: Option<Vec<String>> = given.iter().map(|&y| pretty_param(garena, interner, y)).collect();
        Some(format!(
            "Expected a template with parameters:\n\t<{}>\nbut the argument has parameters:\n\t<{}>",
            wanted_spelled?.join(", "),
            given_spelled?.join(", ")
        ))
    }

    /// `resolveTemplateParamType`'s type of one parameter
    /// (`TypeCheck.hs:349-362`): the kind's own type where it is resolved
    /// unsubstituted, else the kind evaluated under the arguments.
    fn param_type(&self, home: Decl, scope: &QualifiedName, param: ExpId, kind: ExpId, bind: Bind<'_>) -> Option<Ty> {
        let (_, t) = self.param_kind(home, param)?;
        if t.resolved() { Some(t.the_type()) } else { self.kind_type(home, scope, kind, bind) }
    }

    /// `validateArg`'s literal arm (`TypeCheck.hs:281-285`): an integer
    /// literal under a sized integer parameter is re-stamped to its width —
    /// the width of the parameter's kind substituted (`TypeCheck.hs:229`).
    /// `arg` is the written literal, `None` for an evaluated one.
    fn stamp(&self, home: Decl, param: ExpId, arg: Option<ExpId>, bind: Bind<'_>) -> Option<LiteralType> {
        if let Some(a) = arg
            && !matches!(self.exp(a), Exp::IntLiteral { .. })
        {
            return None;
        }
        let (kind, t) = self.param_kind(home, param)?;
        let t = if t.resolved() {
            t.the_type()
        } else {
            let scope = self.template_scope(home)?;
            self.kind_type(home, &scope, kind, bind)?
        };
        // `tIsSigned (theType typ)` and `sizeOf typ` (`TypeCheck.hs:281-285`,
        // `Type.hs:653-657,695-723`) both see through `TConst` and
        // `TInstance`: a parameter typed by an alias of an instance —
        // `using fifo_index_t = index_t<ArraySize>` — is stamped to the
        // width it names (`data/fifo/array.k`'s `dequeue_one<0>`, `uint2`).
        let w = u32::try_from(kg_ty::width::t_width(&t)?).ok().filter(|&w| w > 0)?;
        if t.is_signed() {
            Some(LiteralType::signed(w))
        } else if t.is_unsigned() {
            Some(LiteralType::unsigned(w))
        } else {
            None
        }
    }
}

/// `pretty` (`PrettyPrint.hs:84-177`) of a template parameter, or of a
/// parameter's kind, as `validateTemplate`'s message spells a parameter list:
/// `typename`, `auto`, a built-in type, a template-template kind, a
/// parameter `kind name` with a literal's or a built-in type's default after
/// ` = `. `None` for any other shape — a named type, another default — which
/// is spelled nowhere yet.
fn pretty_param(arena: &ExpArena, interner: &Interner, id: ExpId) -> Option<String> {
    Some(match &arena[id].exp {
        // `"template" <+> angles (commaSepList a) <> line <> pretty b`
        // (`PrettyPrint.hs:159`): the kinds on one line where `group` fits
        // them in the default page's 80 columns, then a hard break —
        // `<typename X, template <typename>\ntypename Y>`
        // (`kg-oracle --errors`, 2026-10-10). Past 80 the list breaks
        // too, which is not spelled.
        Exp::Template { params, decl } => {
            let kinds: Option<Vec<String>> = params.iter().map(|&k| pretty_param(arena, interner, k)).collect();
            let head = format!("template <{}>", kinds?.join(", "));
            if head.chars().count() > 80 {
                return None;
            }
            format!("{head}\n{}", pretty_param(arena, interner, *decl)?)
        }
        Exp::Typename => "typename".to_owned(),
        Exp::Auto => "auto".to_owned(),
        Exp::Boolean => "bool".to_owned(),
        Exp::Float => "float32".to_owned(),
        Exp::Str => "string".to_owned(),
        Exp::Void => "void".to_owned(),
        Exp::Unsigned(n) => format!("uint{n}"),
        Exp::Integer(n) => format!("int{n}"),
        Exp::Const(k) => format!("const {}", pretty_param(arena, interner, *k)?),
        Exp::TemplateParam { kind, name, default, .. } => {
            let param = format!("{} {}", pretty_param(arena, interner, *kind)?, interner.resolve(*name));
            match default {
                None => param,
                Some(d) => format!("{param} = {}", pretty_param(arena, interner, *d)?),
            }
        }
        Exp::IntLiteral { ty: None, value } => value.to_string(),
        Exp::BoolLiteral(b) => b.to_string(),
        _ => return None,
    })
}

/// Whether a scoped name starts at one of the template's own parameters —
/// `T::Foo::X` — rather than at a module or a class.
fn param_rooted(arena: &ExpArena, name: ExpId, scope: &QualifiedName) -> bool {
    let mut at = name;
    let mut depth = 0;
    while let Exp::ScopedName { prefix: Some(p), .. } = &arena[at].exp {
        at = *p;
        depth += 1;
    }
    depth > 0 && matches!(&arena[at].exp, Exp::TypeParam { scope: s, .. } if s == scope)
}

fn items_in(arena: &ExpArena, id: ExpId) -> Vec<ExpId> {
    match &arena[id].exp {
        Exp::Seq(items) => items.clone(),
        _ => Vec::new(),
    }
}

/// An evaluated value as the literal a written argument would be.
pub fn minted_literal(v: &ConstValue) -> Option<Exp> {
    Some(match v {
        ConstValue::Int { value, .. } => Exp::IntLiteral { ty: None, value: value.clone() },
        ConstValue::Bool(b) => Exp::BoolLiteral(*b),
        ConstValue::Float(f) => Exp::FloatLiteral(*f),
        _ => return None,
    })
}
