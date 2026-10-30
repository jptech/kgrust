//! `deduceTemplateArgs` (`Type.hs:1384-1567`), on the IR: a call of a
//! function template gets its template arguments deduced from its argument
//! types and written into the tree as an instance; an instance written short
//! of its template's parameters gets the defaults; a member access typed
//! undefined on a class becomes a method callee.
//!
//! ```haskell
//! deduceTemplateArgs symbols = desugar' rewrite
//!   where
//!     rewrite n (MemberAccessF a b) | tIsUndefined (typeOf n) && tIsClass (typeOf a) = …
//!     rewrite _ (FunctionSpecifierF (Just a) b) | getQualifier b == [unresolvedName] && tIsClass (typeOf a) = …
//!     rewrite _ (DesignatorF a b@(FunctionSpecifier x y)) = …
//!     rewrite _ (FunctionCallF a b@(FunctionSpecifier x y) c) = …
//!     rewrite _ (NamedValueF a@QualifiedIdentifier{}) | not $ typeResolved a = …
//!     rewrite _ (TypeIdentifierF s t@TemplateInstance{}) = …
//!     rewrite _ (ScopedNameF s (Just t@TemplateInstance{}) a) = …
//! ```
//!
//! Declarations are read from the round's snapshot — the reference's
//! `symbols` is taken before the pass — and what the pass mints goes into
//! the module being rewritten, noted as the reference notes it
//! ([`crate::from_type::Mint`]). A template's default is copied over from the
//! template's module with its parameters substituted.
//!
//! A conflicting deduction is `reconcile`'s `TError` (`Type.hs:1502-1506`),
//! written into the tree as an `Exp::Error` at the call, which
//! `crate::check` reports as the round's error.

use std::collections::BTreeMap;

use kg_base::{Interner, Symbol};
use kg_diag::DiagnosticKind;
use kg_hir::rewrite::{desugar_within, get_name, get_qualifier, remove_nested_type_identifier, Rewriter};
use kg_hir::{Exp, ExpArena, ExpId};
use kg_ty::{QualifiedName, TArg, Ty, TyRef};

use crate::from_type::Mint;
use crate::infer::{Program, Snapshot};
use crate::symbols::{Decl, Symbols};
use crate::types::TypeMap;

/// `DeduceType` (`Type.hs:1380-1381`), the reader of the deduction.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum How {
    Supertype,
    Subtype,
}

/// One parameter's deduction: how it reads, what it bound, and — for
/// `reconcile`'s `TError` — that the binding is the conflict
/// ([`DiagnosticKind::ConflictingDeduction`]), which travels with it as its
/// message does, into the `Exp::Error` [`Cx::arg`] writes.
type Bound = (How, TArg, Option<DiagnosticKind>);

type Deduced = BTreeMap<usize, Option<Bound>>;

/// `parallel . deduceTemplateArgs` over every module of `program`.
pub fn deduce_template_args(program: &mut Program<'_>) {
    let interner = program.interner();
    let started = std::time::Instant::now();
    let snapshot = program.snapshot();
    crate::profile::whole("deduce's snapshot", started);
    let started = std::time::Instant::now();
    // `withSymbols` is `getSymbols`, `AllSymbols` (`Template.hs:65,71`):
    // a template's body is entered, so a local there shadows a template
    // of its name.
    let symbols = program.symbols(crate::symbols::Config::AllSymbols);
    crate::profile::whole("deduce's symbols", started);
    let templates = template_names(&symbols, &snapshot);
    let cancel = program.cancel_token();
    let door = program.door;
    // Where each module stood when the snapshot was taken: a template's
    // signature is spelled again only after a write to its module.
    let versions: Vec<(u64, u64)> = program.modules.iter().map(|m| (m.ir.arena.writes(), m.types.writes())).collect();
    for (i, module) in program.modules.iter_mut().enumerate() {
        cancel.check();
        let _visit = crate::profile::visit("deduceTemplateArgs", i, module.ir.arena.len());
        let root = module.ir.root;
        let cx = Cx {
            snapshot: &snapshot,
            symbols: &symbols,
            templates: &templates,
            interner,
            module: i,
            door,
            versions: &versions,
            read: std::cell::RefCell::default(),
        };
        let mut rw = Rewriter::new(&mut module.ir.arena, interner);
        let types = &mut module.types;
        // Only the shapes `rewrite` matches change; what holds none is passed
        // over (`kg_hir::facts`), its notes left to the next `sync`.
        let next = desugar_within(&mut rw, root, kg_hir::facts::DEDUCES, &mut |rw, id, exp| {
            sync(rw, types);
            let mut mint = Mint { rw, types };
            cx.rewrite(&mut mint, id, exp)
        });
        sync(&mut rw, types);
        module.ir.root = next;
    }
    program.done_with(snapshot);
}

/// The last segment of every key `symbols` files a `Template` under, as
/// [`Cx::template`] reads the declaration — off the snapshot. Whatever
/// `lookupSymbol` finds for a name is filed under a key ending in that
/// name, so a call whose callee's name is none of these deduces nothing,
/// and is passed before the lookup: deduction asked the map of every call
/// every round, most of the pass on `risc-v.k` (2026-10-10).
fn template_names(symbols: &Symbols, snapshot: &[Snapshot]) -> kg_base::FxHashSet<Symbol> {
    symbols
        .iter()
        .filter(|(_, d)| matches!(snapshot[d.module].arena[d.id].exp, Exp::Template { .. }))
        .filter_map(|(key, _)| key.last())
        .collect()
}

/// A rewrite's replacement takes its original's note at once — the next
/// arm up reads it (`desugar'` keeps the note, `Desugar.hs:71-74`).
fn sync(rw: &mut Rewriter<'_>, types: &mut TypeMap) {
    for (new, original) in rw.arena.take_lineage() {
        if !types.is_set(new) && types.is_set(original) {
            let t = types.get(original);
            types.reset_type_of(new, t);
        }
    }
}

pub(crate) struct Cx<'a> {
    snapshot: &'a [Snapshot],
    symbols: &'a Symbols,
    /// [`template_names`].
    templates: &'a kg_base::FxHashSet<Symbol>,
    interner: &'a Interner,
    /// The module rewritten, the door its reads of another module are
    /// shown ([`crate::reads`]), and where each module stood when the
    /// snapshot was taken. A template's signature is shown at every
    /// deduction that reads it: a deduction that changes nothing drops
    /// what it read ([`Cx::deduce_instance`]), so no reading of it stands
    /// for another's.
    module: usize,
    door: Option<&'a crate::reads::Door>,
    versions: &'a [(u64, u64)],
    /// The templates of another module the deductions under way read, in
    /// order ([`Cx::deduce_instance`]).
    read: std::cell::RefCell<Vec<Decl>>,
}

/// A template, read off its module's snapshot.
struct Template<'a> {
    decl: Decl,
    arena: &'a ExpArena,
    types: &'a TypeMap,
    params: Vec<ExpId>,
    /// `getQualifier template`: the declaration's scope — what a
    /// `TypeParam` of this template is scoped with.
    scope: QualifiedName,
}

impl<'a> Cx<'a> {
    fn items(arena: &ExpArena, id: ExpId) -> Vec<ExpId> {
        match &arena[id].exp {
            Exp::Seq(items) => items.clone(),
            _ => Vec::new(),
        }
    }

    /// `getScope` (`Parser/Syntax.hs:766-780`), the name-node arms.
    pub(crate) fn scope_of(arena: &ExpArena, id: ExpId) -> Option<QualifiedName> {
        match &arena[id].exp {
            Exp::Identifier(_) => Some(QualifiedName::new(std::iter::empty())),
            Exp::ScopedName { scope, .. } | Exp::TypeParam { scope, .. } | Exp::This { scope } => Some(scope.clone()),
            Exp::TypeIdentifier(_, a)
            | Exp::QualifiedIdentifier(a)
            | Exp::TemplateInstance { base: a, .. }
            | Exp::FunctionSpecifier { function: a, .. }
            | Exp::NamedValue(a) => Self::scope_of(arena, *a),
            _ => None,
        }
    }

    /// `getQualifiedName`.
    pub(crate) fn qualified_name(arena: &ExpArena, id: ExpId) -> Option<Vec<Symbol>> {
        let mut q = get_qualifier(arena, id)?;
        q.push(get_name(arena, id)?);
        Some(q)
    }

    /// `lookupSymbol e symbols`, for a node of `arena`.
    fn lookup(&self, arena: &ExpArena, id: ExpId) -> Option<Decl> {
        let scope = Self::scope_of(arena, id)?;
        let name = Self::qualified_name(arena, id)?;
        let found = self.symbols.in_scope(scope.segments(), &name);
        if let Some(door) = self.door {
            door.lookup(self.module, crate::reads::Asked::InScope(scope.segments(), &name), found, |d| {
                (&self.snapshot[d.module].arena, &self.snapshot[d.module].arena[d.id].exp)
            });
        }
        found
    }

    /// The template a mention in this module names, if it names one — the
    /// lookup shown to the door only where it finds a template: where it
    /// finds none, or the name is no template's anywhere (`templates`, a
    /// filter the lookup's answer settles alike), nothing is deduced, which
    /// is what the reads of another run that found none say too.
    fn target(&self, arena: &ExpArena, id: ExpId) -> Option<Decl> {
        let scope = Self::scope_of(arena, id)?;
        let name = Self::qualified_name(arena, id)?;
        let d = self.symbols.in_scope(scope.segments(), &name)?;
        if !matches!(self.snapshot[d.module].arena[d.id].exp, Exp::Template { .. }) {
            return None;
        }
        if let Some(door) = self.door {
            door.lookup(self.module, crate::reads::Asked::InScope(scope.segments(), &name), Some(d), |d| {
                (&self.snapshot[d.module].arena, &self.snapshot[d.module].arena[d.id].exp)
            });
        }
        Some(d)
    }

    fn template(&self, d: Decl) -> Option<Template<'a>> {
        let snap = &self.snapshot[d.module];
        if self.door.is_some() && d.module != self.module {
            self.read.borrow_mut().push(d);
        }
        let Exp::Template { params, decl } = &snap.arena[d.id].exp else { return None };
        let scope = kg_hir::rewrite::decl_scope_of(&snap.arena, *decl)?.clone();
        Some(Template { decl: d, arena: &snap.arena, types: &snap.types, params: params.clone(), scope })
    }

    /// What a deduction reads of a template declared in another module,
    /// shown to the door whole: the template node, its parameters (their
    /// kinds and defaults, which a deduction substitutes into the reader),
    /// the declaration node, and a function's parameters and return type —
    /// each node with its type. Not a function's body, which no deduction
    /// reads.
    fn show_signature(door: &crate::reads::Door, reader: usize, module: usize, snap: &Snapshot, id: ExpId) {
        let (arena, types) = (&snap.arena, Some(&snap.types));
        door.node(reader, module, arena, id, &arena[id].exp);
        door.ty(reader, module, arena, id, snap.types.entry(id.index()));
        let Exp::Template { params, decl } = &arena[id].exp else { return };
        for &p in params {
            door.subtree(reader, module, arena, types, p);
        }
        door.node(reader, module, arena, *decl, &arena[*decl].exp);
        door.ty(reader, module, arena, *decl, snap.types.entry(decl.index()));
        if let Exp::Function { params, ret, .. } | Exp::FunctionDecl { params, ret, .. } = &arena[*decl].exp {
            door.subtree(reader, module, arena, types, *params);
            door.subtree(reader, module, arena, types, *ret);
        }
    }

    fn rewrite(&self, m: &mut Mint<'_, '_>, n: ExpId, e: Exp) -> Exp {
        match e {
            Exp::MemberAccess { base: a, member: b } => {
                let ta = m.types.get(a);
                if !(m.types.get(n).is_undefined() && ta.is_class()) {
                    return Exp::MemberAccess { base: a, member: b };
                }
                let (Some(class), Some(name)) = (ta.qualified_name().cloned(), get_name(m.rw.arena, b)) else {
                    return Exp::MemberAccess { base: a, member: b };
                };
                let mut path = class.segments().to_vec();
                path.push(name);
                let none = QualifiedName::new(std::iter::empty());
                match m.qualified_identifier(&none, b, &path) {
                    Some(q) => Exp::FunctionSpecifier { receiver: Some(a), function: q },
                    None => Exp::MemberAccess { base: a, member: b },
                }
            }
            Exp::FunctionSpecifier { receiver: Some(a), function: b } => {
                let unresolved = self.interner.intern(kg_hir::sort::UNRESOLVED_NAME);
                let ta = m.types.get(a);
                if get_qualifier(m.rw.arena, b).as_deref() == Some(&[unresolved][..])
                    && ta.is_class()
                    && let Some(class) = ta.qualified_name().cloned()
                {
                    let b = self.set_qualifier(m, class.segments(), b);
                    return Exp::FunctionSpecifier { receiver: Some(a), function: b };
                }
                Exp::FunctionSpecifier { receiver: Some(a), function: b }
            }
            Exp::Designator { name: a, value: b } => {
                let Exp::FunctionSpecifier { receiver: x, function: y } = m.rw.arena[b].exp.clone() else {
                    return Exp::Designator { name: a, value: b };
                };
                let y2 = self.deduce_instance(m, &[], &[], y);
                if y2 == y {
                    return Exp::Designator { name: a, value: b };
                }
                let b = m.copy_note(b, Exp::FunctionSpecifier { receiver: x, function: y2 });
                Exp::Designator { name: a, value: b }
            }
            Exp::FunctionCall { attr, function: b, args: c } => {
                let Exp::FunctionSpecifier { receiver: x, function: y } = m.rw.arena[b].exp.clone() else {
                    return Exp::FunctionCall { attr, function: b, args: c };
                };
                let items = Self::items(m.rw.arena, c);
                let arg_types: Vec<Ty> = items.iter().map(|&i| m.types.get(i)).collect();
                // A later edition's labels, which name the template's own
                // parameters until `crate::named` can read its instance's.
                let labels: Vec<Option<Symbol>> = items
                    .iter()
                    .map(|&i| match m.rw.arena[i].exp {
                        Exp::Labelled { label, .. } => Some(label),
                        _ => None,
                    })
                    .collect();
                let labels = if labels.iter().any(Option::is_some) { labels } else { Vec::new() };
                let y2 = self.deduce_instance(m, &arg_types, &labels, y);
                if y2 == y {
                    return Exp::FunctionCall { attr, function: b, args: c };
                }
                let b = m.copy_note(b, Exp::FunctionSpecifier { receiver: x, function: y2 });
                Exp::FunctionCall { attr, function: b, args: c }
            }
            Exp::NamedValue(a) if matches!(m.rw.arena[a].exp, Exp::QualifiedIdentifier(_)) && !m.types.resolved(a) => {
                Exp::NamedValue(self.deduce_instance(m, &[], &[], a))
            }
            Exp::TypeIdentifier(s, t) if matches!(m.rw.arena[t].exp, Exp::TemplateInstance { .. }) => {
                Exp::TypeIdentifier(s, self.deduce_instance(m, &[], &[], t))
            }
            Exp::ScopedName { scope, prefix: Some(t), ident } if matches!(m.rw.arena[t].exp, Exp::TemplateInstance { .. }) => {
                Exp::ScopedName { scope, prefix: Some(self.deduce_instance(m, &[], &[], t)), ident }
            }
            e => e,
        }
    }

    /// `setQualifier` (`Desugar.hs:193-201`), each rewritten node `renote`d.
    fn set_qualifier(&self, m: &mut Mint<'_, '_>, q: &[Symbol], id: ExpId) -> ExpId {
        let Some((&last, init)) = q.split_last() else { return id };
        let out = match m.rw.arena[id].exp.clone() {
            Exp::ScopedName { scope, prefix: Some(a), ident } if get_name(m.rw.arena, a) == Some(last) => {
                let a = self.set_qualifier(m, init, a);
                Exp::ScopedName { scope, prefix: Some(a), ident }
            }
            Exp::ScopedName { scope, ident, .. } => {
                let prefix = m.scoped_qualifier(&scope, ident, q);
                Exp::ScopedName { scope, prefix, ident }
            }
            Exp::TemplateInstance { base, args } => Exp::TemplateInstance { base: self.set_qualifier(m, q, base), args },
            Exp::QualifiedIdentifier(a) => Exp::QualifiedIdentifier(self.set_qualifier(m, q, a)),
            _ => return id,
        };
        self.renote(m, id, out)
    }

    /// `renote f n`: `f`'s result under `n`'s note.
    fn renote(&self, m: &mut Mint<'_, '_>, id: ExpId, exp: Exp) -> ExpId {
        if m.rw.arena[id].exp == exp {
            return id;
        }
        let new = m.rw.replace(id, exp);
        sync(m.rw, m.types);
        new
    }

    /// `deduceInstance funcArgTypes templateInstance` (`Type.hs:1433-1449`).
    /// What it reads across the module boundary is the door's only where
    /// it changed the module ([`crate::reads::Door::tentative`]): a
    /// deduction a later round makes again of a call it already rewrote
    /// reads the template's signature and changes nothing.
    fn deduce_instance(&self, m: &mut Mint<'_, '_>, arg_types: &[Ty], labels: &[Option<Symbol>], x: ExpId) -> ExpId {
        let Some(door) = self.door else { return self.deduce_instance_reading(m, arg_types, labels, x) };
        let before = (m.rw.arena.len(), m.rw.arena.writes(), m.types.writes());
        let read = self.read.borrow().len();
        door.tentative();
        let out = self.deduce_instance_reading(m, arg_types, labels, x);
        let changed = out != x || before != (m.rw.arena.len(), m.rw.arena.writes(), m.types.writes());
        // The templates this deduction read, shown only where it changed
        // the module: a signature is spelled for what it moved, never for a
        // deduction made again of a call already rewritten.
        let templates: Vec<Decl> = self.read.borrow_mut().drain(read..).collect();
        if changed {
            for d in templates {
                let snap = &self.snapshot[d.module];
                door.declaration(self.module, d.module, d.id, self.versions[d.module], || {
                    Self::show_signature(door, usize::MAX, d.module, snap, d.id);
                });
            }
        }
        door.settle(self.module, changed);
        out
    }

    fn deduce_instance_reading(&self, m: &mut Mint<'_, '_>, arg_types: &[Ty], labels: &[Option<Symbol>], x: ExpId) -> ExpId {
        if get_name(m.rw.arena, x).is_none_or(|name| !self.templates.contains(&name)) {
            return x;
        }
        let Some(t) = self.target(m.rw.arena, x).and_then(|d| self.template(d)) else { return x };
        let decl = match &t.arena[t.decl.id].exp {
            Exp::Template { decl, .. } => *decl,
            _ => return x,
        };
        match t.arena[decl].exp.clone() {
            Exp::Function { scope, ret, params, .. } | Exp::FunctionDecl { scope, ret, params, .. } => {
                self.deduce_from_signature(m, &t, &scope, ret, params, arg_types, labels, x)
            }
            _ => {
                let Exp::TemplateInstance { base: typ, args: a } = m.rw.arena[x].exp.clone() else { return x };
                let args = Self::items(m.rw.arena, a);
                if args.len() >= t.params.len() {
                    return x;
                }
                let unresolved: Deduced = (args.len()..t.params.len()).map(|i| (i, None)).collect();
                let Some(resolved) = self.resolve_template_args(m, x, &t, &args, &unresolved) else { return x };
                let seq = m.copy_note(a, Exp::Seq(resolved));
                m.copy_note(x, Exp::TemplateInstance { base: typ, args: seq })
            }
        }
    }

    /// `resolveTemplateArgs` (`Type.hs:1405-1428`): the explicit arguments,
    /// the deduced ones spelled, the defaults of the rest substituted —
    /// `None` where any stays unresolved (`fromMaybe args`).
    fn resolve_template_args(
        &self,
        m: &mut Mint<'_, '_>,
        donor: ExpId,
        t: &Template<'_>,
        args: &[ExpId],
        deduced: &Deduced,
    ) -> Option<Vec<ExpId>> {
        let mut resolved: BTreeMap<usize, Option<ExpId>> = args.iter().enumerate().map(|(i, &a)| (i, Some(a))).collect();
        for (&i, d) in deduced {
            if resolved.contains_key(&i) {
                continue;
            }
            let a = self.arg(m, donor, d.as_ref());
            resolved.insert(i, a);
        }
        for &p in &t.params {
            let Exp::TemplateParam { index, default, .. } = &t.arena[p].exp else { continue };
            let n = *index as usize;
            if deduced.get(&n) == Some(&None) {
                let d = default.map(|x| self.substitute(m, t, x, &resolved));
                resolved.insert(n, d);
            }
        }
        resolved.into_values().collect()
    }

    /// `arg` (`Type.hs:1413-1419`): a deduction spelled as an argument.
    fn arg(&self, m: &mut Mint<'_, '_>, donor: ExpId, d: Option<&Bound>) -> Option<ExpId> {
        let (_, a, kind) = d?;
        match a {
            TArg::Type(x) if x.is_error() => {
                let Ty::Error(msg) = &**x else { return None };
                Some(m.new_note(donor, Exp::Error(kg_hir::ErrorNote { kind: *kind, message: msg.clone() })))
            }
            TArg::Type(x) => {
                let e = m.exp_from_type(donor, x)?;
                Some(m.new_note(donor, e))
            }
            TArg::Int(x) => Some(m.new_note(donor, Exp::IntLiteral { ty: None, value: x.clone() })),
            TArg::Name(_, x) => {
                let none = QualifiedName::new(std::iter::empty());
                m.qualified_identifier(&none, donor, x.segments())
            }
            _ => None,
        }
    }

    /// `desugar resolve <$> x` (`Type.hs:1421-1427`): the template's default
    /// `x` copied into this module, each of the template's parameters
    /// replaced by its resolved argument, every node keeping its note.
    fn substitute(
        &self,
        m: &mut Mint<'_, '_>,
        t: &Template<'_>,
        x: ExpId,
        resolved: &BTreeMap<usize, Option<ExpId>>,
    ) -> ExpId {
        let node = t.arena[x].clone();
        let mut exp = node.exp.clone();
        for child in exp.children_mut() {
            *child = self.substitute(m, t, *child, resolved);
        }
        let exp = match &exp {
            Exp::TypeParam { scope, index, .. } if *scope == t.scope => {
                match resolved.get(&(*index as usize)).copied().flatten() {
                    Some(arg) => m.rw.arena[arg].exp.clone(),
                    None => exp,
                }
            }
            _ => remove_nested_type_identifier(m.rw.arena, exp),
        };
        let new = m.rw.arena.alloc(exp, node.src, node.kind);
        if t.types.is_set(x) {
            m.types.reset_type_of(new, t.types.get(x));
        }
        new
    }

    /// `deduceFromFunctionSignature` (`Type.hs:1451-1478`) and its deduction.
    #[allow(clippy::too_many_arguments)]
    fn deduce_from_signature(
        &self,
        m: &mut Mint<'_, '_>,
        t: &Template<'_>,
        scope: &QualifiedName,
        ret: ExpId,
        func_params: ExpId,
        arg_types: &[Ty],
        labels: &[Option<Symbol>],
        q: ExpId,
    ) -> ExpId {
        let Exp::QualifiedIdentifier(ident) = m.rw.arena[q].exp.clone() else { return q };
        let (func, args_seq, args) = match m.rw.arena[ident].exp.clone() {
            Exp::TemplateInstance { base, args } => (base, Some(args), Self::items(m.rw.arena, args)),
            _ => (ident, None, Vec::new()),
        };
        if args.len() > t.params.len() {
            return q;
        }
        let mut d = Deduction {
            cx: self,
            t,
            scope,
            state: (args.len()..t.params.len()).map(|i| (i, None)).collect(),
            how: How::Supertype,
        };
        let params = Self::items(t.arena, func_params);
        let Some(by_param) = Self::by_parameter(t.arena, &params, arg_types, labels) else { return q };
        for (&p, ty) in params.iter().zip(&by_param) {
            if let (Exp::FuncParam { ty: pt, .. }, Some(ty)) = (&t.arena[p].exp, ty) {
                d.deduce_type(Pat::of(*pt), ty);
            }
        }
        for (&p, &a) in t.params.iter().zip(&args) {
            if let Exp::TemplateParam { kind, .. } = &t.arena[p].exp
                && matches!(t.arena[*kind].exp, Exp::FunctionType { .. })
            {
                let ta = m.types.get(a);
                d.deduce_type(Pat::of(*kind), &ta);
            }
        }
        let ti = m.types.get(ident);
        if ti.is_function() {
            d.deduce_type(Pat::of(ret), &ti.return_ty());
        }
        let state = d.state;
        if crate::env_flag!("KG_DEDUCE_TRACE") {
            eprintln!(
                "deduce {:?} args {:?} -> {:?}",
                Cx::qualified_name(m.rw.arena, q).map(|p| p.iter().map(|s| self.interner.resolve(*s).to_string()).collect::<Vec<_>>()),
                arg_types.iter().map(|t| kg_ty::subtype::show_pretty(t, Some(self.interner))).collect::<Vec<_>>(),
                state.iter().map(|(i, v)| (i, v.as_ref().map(|(_, a, _)| format!("{a:?}").chars().take(80).collect::<String>()))).collect::<Vec<_>>()
            );
        }
        let resolved_prime = self.resolve_template_args(m, q, t, &args, &state).unwrap_or_else(|| args.clone());
        // `zipWith deduceInstanceFromTemplateParam params resolvedArgs'`.
        let mut items = Vec::with_capacity(resolved_prime.len().min(t.params.len()));
        for (&p, &e) in t.params.iter().zip(&resolved_prime) {
            let Exp::TemplateParam { kind, .. } = &t.arena[p].exp else {
                items.push(e);
                continue;
            };
            let item = match &t.arena[*kind].exp {
                Exp::TypeParam { kind: k, index, .. } if matches!(t.arena[*k].exp, Exp::Typename) => {
                    match resolved_prime.get(*index as usize) {
                        Some(&pat) => self.deduce_instance_from_type(m, Side::Here, pat, e),
                        None => e,
                    }
                }
                _ => self.deduce_instance_from_type(m, Side::Template(t), *kind, e),
            };
            items.push(item);
        }
        if let Some(seq) = args_seq
            && Self::items(m.rw.arena, seq) == items
        {
            return q;
        }
        let donor = args_seq.unwrap_or(ident);
        let seq = m.copy_note(donor, Exp::Seq(items));
        let instance = m.copy_note(q, Exp::TemplateInstance { base: func, args: seq });
        m.copy_note(q, Exp::QualifiedIdentifier(instance))
    }

    /// Each parameter's argument type: positionally, as the reference zips
    /// them (`zipWithM_ deduceFromFuncParam`, `Type.hs:1477`), or — a later
    /// edition's labelled arguments — by the parameter each names. `None`
    /// where a label names no parameter, or two arguments one: nothing is
    /// deduced, and the call is left for `crate::named` to report.
    fn by_parameter<'t>(arena: &ExpArena, params: &[ExpId], arg_types: &'t [Ty], labels: &[Option<Symbol>]) -> Option<Vec<Option<&'t Ty>>> {
        if labels.is_empty() {
            return Some(arg_types.iter().map(Some).collect());
        }
        let mut out: Vec<Option<&Ty>> = vec![None; params.len().max(arg_types.len())];
        for (i, (ty, label)) in arg_types.iter().zip(labels).enumerate() {
            let slot = match label {
                None => i,
                Some(l) => params.iter().position(|&p| match &arena[p].exp {
                    Exp::FuncParam { name, .. } => kg_hir::rewrite::ident_sym(arena, *name) == Some(*l),
                    _ => false,
                })?,
            };
            if out[slot].replace(ty).is_some() {
                return None;
            }
        }
        Some(out)
    }

    /// `deduceInstanceFromType` (`Type.hs:1468-1471`): a function named as
    /// the argument of a function-typed parameter is deduced against that
    /// type's parameters.
    fn deduce_instance_from_type(&self, m: &mut Mint<'_, '_>, side: Side<'_, '_>, pat: ExpId, e: ExpId) -> ExpId {
        let params = match side {
            Side::Here => match &m.rw.arena[pat].exp {
                Exp::FunctionType { params, .. } => {
                    Self::items(m.rw.arena, *params).iter().map(|&p| m.types.get(p)).collect::<Vec<_>>()
                }
                _ => return e,
            },
            Side::Template(t) => match &t.arena[pat].exp {
                Exp::FunctionType { params, .. } => Self::items(t.arena, *params).iter().map(|&p| t.types.get(p)).collect(),
                _ => return e,
            },
        };
        let Exp::NamedValue(b) = m.rw.arena[e].exp.clone() else { return e };
        if m.types.resolved(b) {
            return e;
        }
        let b2 = self.deduce_instance(m, &params, &[], b);
        if b2 == b {
            return e;
        }
        self.renote(m, e, Exp::NamedValue(b2))
    }
}

/// Which arena a pattern of `deduceInstanceFromType` lives in.
#[derive(Clone, Copy)]
enum Side<'t, 'a> {
    Here,
    Template(&'t Template<'a>),
}

/// A node of the template, or the `element` of an array node with `skip`
/// dimensions dropped (`Type.hs:1560-1564`, which mints a node for it).
#[derive(Copy, Clone)]
struct Pat {
    id: ExpId,
    skip: usize,
}

impl Pat {
    fn of(id: ExpId) -> Pat {
        Pat { id, skip: 0 }
    }
}

/// The deduction's state and reader (`execState (runReaderT deduction
/// DeduceSupertype)`).
struct Deduction<'c, 't, 'a> {
    cx: &'c Cx<'c>,
    t: &'t Template<'a>,
    /// The function's `declScopeName`: a parameter of this template is
    /// scoped with it (`setArg`'s `scope' == scope`).
    scope: &'t QualifiedName,
    state: Deduced,
    how: How,
}

impl Deduction<'_, '_, '_> {
    fn exp(&self, id: ExpId) -> &Exp {
        &self.t.arena[id].exp
    }

    fn items(&self, id: ExpId) -> Vec<ExpId> {
        Cx::items(self.t.arena, id)
    }

    /// `element` (`Type.hs:1560-1564`).
    fn element(&self, p: Pat) -> Option<Pat> {
        let Exp::Array { elem, dims, .. } = self.exp(p.id) else { return None };
        let n = self.items(*dims).len();
        if p.skip + 1 >= n { Some(Pat::of(*elem)) } else { Some(Pat { id: p.id, skip: p.skip + 1 }) }
    }

    fn array_dims(&self, p: Pat) -> Option<Vec<ExpId>> {
        match self.exp(p.id) {
            Exp::Array { dims, .. } => Some(self.items(*dims).into_iter().skip(p.skip).collect()),
            _ => None,
        }
    }

    /// `deduceType` (`Type.hs:1498-1553`), arm for arm in the reference's
    /// order.
    fn deduce_type(&mut self, p: Pat, t: &Ty) {
        if matches!(t, Ty::Auto) {
            return;
        }
        if p.skip == 0 {
            match (self.exp(p.id).clone(), t) {
                (Exp::FunctionType { params: a, ret: r, .. }, Ty::Function { ret: r2, params: a2, .. }) => {
                    self.deduce_type(Pat::of(r), r2);
                    let saved = self.how;
                    self.how = How::Subtype;
                    for (i, &fp) in self.items(a).iter().enumerate() {
                        let Exp::FunctionTypeParam { ty, .. } = self.exp(fp).clone() else { continue };
                        let ti = a2.get(i).map_or(Ty::Void, |p| (*p.ty).clone());
                        self.deduce_type(Pat::of(ty), &ti);
                    }
                    self.how = saved;
                    return;
                }
                (Exp::TypeIdentifier(_, ti), _) if matches!(self.exp(ti), Exp::TemplateInstance { .. }) => {
                    let Exp::TemplateInstance { base: a, args: b } = self.exp(ti).clone() else { unreachable!() };
                    match t {
                        Ty::Instance { template, args, .. }
                            if Cx::qualified_name(self.t.arena, a).as_deref() == Some(template.segments()) =>
                        {
                            for (&x, (_, y)) in self.items(b).iter().zip(args) {
                                self.deduce_arg(x, y);
                            }
                            return;
                        }
                        Ty::Initializer(xs) => {
                            if let Some(d) = self.cx.lookup(self.t.arena, ti) {
                                self.deduce_from_struct(d, b, xs);
                            }
                            return;
                        }
                        t if !t.resolved() => {
                            for x in self.items(b) {
                                self.deduce_arg(x, &TArg::Type(TyRef::new(Ty::Unresolved)));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        if let Ty::Instance { concrete, .. } = t {
            return self.deduce_type(p, concrete);
        }
        if p.skip == 0
            && let Exp::Const(a) = self.exp(p.id)
        {
            let a = *a;
            return self.deduce_type(Pat::of(a), t);
        }
        if let Ty::Const(inner) = t {
            return self.deduce_type(p, inner);
        }
        if let Some(dims) = self.array_dims(p) {
            match t {
                Ty::Array { dims: tdims, .. } if tdims.is_empty() => {
                    if let Some(e) = self.element(p) {
                        self.deduce_type(e, &t.element(None));
                    }
                    return;
                }
                Ty::Array { dims: tdims, .. } => {
                    if let (Some(&a1), Some(t1)) = (dims.first(), tdims.first())
                        && self.deduce_int(a1, t1)
                        && let Some(e) = self.element(p)
                    {
                        self.deduce_type(e, &t.element(None));
                    }
                    return;
                }
                Ty::Initializer(ts) => {
                    let Some(&a1) = dims.first() else { return };
                    if !ts.is_empty() {
                        self.deduce_int(a1, &kg_const::ConstInt::from(ts.len() as u64));
                    }
                    if let Some(e) = self.element(p) {
                        for x in ts {
                            self.deduce_type(e, &positional_type(x));
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        if p.skip != 0 {
            return;
        }
        match (self.exp(p.id).clone(), t) {
            (Exp::ParamInt(x), Ty::Signed(w)) if matches!(self.exp(x), Exp::TypeParam { .. }) => {
                self.set_arg(x, TArg::Int(kg_const::ConstInt::from(u64::from(*w))));
            }
            (Exp::ParamInt(x), Ty::Unsigned(w)) if matches!(self.exp(x), Exp::TypeParam { .. }) => {
                self.set_arg(x, TArg::Int(kg_const::ConstInt::from(u64::from(*w) + 1)));
            }
            (Exp::ParamUint(x), Ty::Unsigned(w)) if matches!(self.exp(x), Exp::TypeParam { .. }) => {
                self.set_arg(x, TArg::Int(kg_const::ConstInt::from(u64::from(*w))));
            }
            (Exp::TypeParam { kind, .. }, t) if matches!(self.exp(kind), Exp::Typename) => {
                self.set_arg(p.id, TArg::Type(TyRef::new(t.clone())));
            }
            _ => {}
        }
    }

    /// The `TInitializer` arm's struct fields (`Type.hs:1512-1519`).
    fn deduce_from_struct(&mut self, d: Decl, b: ExpId, xs: &[TyRef]) {
        let snap = &self.cx.snapshot[d.module];
        // Another module's struct template, read to its fields' types.
        let show = |id: ExpId, whole: bool| {
            if let Some(door) = self.cx.door {
                if whole {
                    door.subtree(self.cx.module, d.module, &snap.arena, None, id);
                } else {
                    door.node(self.cx.module, d.module, &snap.arena, id, &snap.arena[id].exp);
                }
            }
        };
        show(d.id, false);
        let Exp::Template { decl, .. } = &snap.arena[d.id].exp else { return };
        show(*decl, false);
        let Exp::Struct { body, .. } = &snap.arena[*decl].exp else { return };
        show(*body, true);
        let args = self.items(b);
        let fields = Cx::items(&snap.arena, *body);
        for (&f, x) in fields.iter().zip(xs) {
            if let Exp::Variable { ty, .. } = &snap.arena[f].exp
                && let Exp::TypeParam { index, .. } = &snap.arena[*ty].exp
                && let Some(&arg) = args.get(*index as usize)
            {
                self.deduce_type(Pat::of(arg), &positional_type(x));
            }
        }
    }

    /// `deduceArg` (`Type.hs:1491-1496`).
    fn deduce_arg(&mut self, x: ExpId, a: &TArg) {
        match a {
            TArg::Type(y) => self.deduce_type(Pat::of(x), y),
            TArg::Int(y) => {
                self.deduce_int(x, y);
            }
            TArg::Name(t, y) => self.deduce_name(x, t, y),
            _ => {}
        }
    }

    /// `deduceName` (`Type.hs:1555-1560`).
    fn deduce_name(&mut self, p: ExpId, t: &TyRef, s: &QualifiedName) {
        match self.exp(p).clone() {
            Exp::TypeParam { kind, .. } => {
                self.set_arg(p, TArg::Name(t.clone(), s.clone()));
                self.deduce_type(Pat::of(kind), t);
            }
            Exp::NamedValue(a) => self.deduce_name(a, t, s),
            _ => {}
        }
    }

    /// `deduceInt` (`Type.hs:1562-1565`).
    fn deduce_int(&mut self, p: ExpId, y: &kg_const::ConstInt) -> bool {
        match self.exp(p) {
            Exp::IntLiteral { value, .. } => value == y,
            Exp::TypeParam { .. } => {
                self.set_arg(p, TArg::Int(y.clone()));
                true
            }
            _ => false,
        }
    }

    /// `setArg` (`Type.hs:1480-1486`): recorded only for a parameter of this
    /// template still to be deduced (`Map.adjust`).
    fn set_arg(&mut self, p: ExpId, arg: TArg) {
        let Exp::TypeParam { scope, index, name, .. } = self.exp(p).clone() else { return };
        if &scope != self.scope {
            return;
        }
        let i = index as usize;
        let Some(existing) = self.state.get(&i).cloned() else { return };
        let merged = reconcile(Some((self.how, arg, None)), existing, name, self.cx.interner);
        self.state.insert(i, merged);
    }
}

/// `tPositionalType` (`Type.hs:284-286`).
fn positional_type(t: &Ty) -> Ty {
    match t {
        Ty::Positional(_, x) => (**x).clone(),
        _ => Ty::Unresolved,
    }
}

/// `reconcile` (`Type.hs:1486-1489` … ), arm for arm: `x` the new
/// deduction, `y` the one recorded.
fn reconcile(x: Option<Bound>, y: Option<Bound>, param: Symbol, interner: &Interner) -> Option<Bound> {
    use kg_ty::subtype::is_subtype;
    let ty_of = |a: &Option<Bound>| match a {
        Some((h, TArg::Type(t), _)) => Some((*h, (**t).clone())),
        _ => None,
    };
    if let Some((_, Ty::Initializer(_))) = ty_of(&x) {
        return y;
    }
    if y.is_none() {
        return x;
    }
    let (tx, ty) = (ty_of(&x), ty_of(&y));
    if let (Some((How::Supertype, a)), Some((How::Supertype, b))) = (&tx, &ty) {
        if (a.is_int() || a.is_enum()) && (b.is_int() || b.is_enum()) {
            return Some((How::Supertype, TArg::Type(TyRef::new(kg_ty::subtype::t_super(a, b))), None));
        }
        if is_subtype(a, b) {
            return y;
        }
        if is_subtype(b, a) {
            return x;
        }
    }
    if let (Some((How::Subtype, a)), Some((_, b))) = (&tx, &ty)
        && is_subtype(b, a)
    {
        return y;
    }
    if let (Some((_, a)), Some((How::Subtype, b))) = (&tx, &ty)
        && is_subtype(a, b)
    {
        return x;
    }
    if let (Some((_, a)), Some((_, b))) = (&tx, &ty) {
        if !a.resolved() {
            return x;
        }
        if !b.resolved() {
            return y;
        }
        if b.is_error() {
            return y;
        }
    }
    let (xa, ya) = (x.as_ref().map(|(_, a, _)| a), y.as_ref().map(|(_, a, _)| a));
    if xa == ya {
        return x;
    }
    let show = |a: Option<&TArg>| a.map_or_else(String::new, |a| kg_ty::subtype::show_pretty_arg(a, Some(interner)));
    Some((
        How::Supertype,
        TArg::Type(TyRef::new(Ty::Error(kg_base::SmolStr::new(format!(
            "Conflicting template argument deduction results for parameter `{}`. Could be `{}` or `{}`.",
            interner.resolve(param),
            show(xa),
            show(ya)
        ))))),
        Some(DiagnosticKind::ConflictingDeduction),
    ))
}
