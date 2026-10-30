//! The two sub-passes of `instantiateTemplates`' list that read types to
//! rewrite closures (`Template.hs:73,79-80`), on the IR:
//!
//! * [`add_lambda_wrappers`] — `addLambdaWrappers` (`Function.hs:366-423`):
//!   a function named as a call's argument becomes a lambda that calls it,
//!   so a higher-order parameter always receives a closure;
//! * [`closure_calls`] — `closureCalls` (`Closure.hs:19-26`): a call through
//!   a closure-typed value calls the closure's function with the closure,
//!   cast to its capture, as the first argument.
//!
//! Both are `desugar` passes over every module, reading a declaration from
//! the round's snapshot and minting as the reference notes
//! ([`crate::from_type::Mint`]).

use kg_base::{Interner, Symbol};
use kg_hir::rewrite::{desugar_within, get_name, get_qualifier, modify_decl_scope_name, Rewriter};
use kg_hir::{Exp, ExpArena, ExpId};
use kg_ty::{FunctionKind, QualifiedName, Ty};

use crate::from_type::Mint;
use crate::infer::{Program, Snapshot};
use crate::symbols::{Config, Decl, Symbols};
use crate::types::TypeMap;

/// A rewrite's replacement takes its original's note at once.
fn sync(rw: &mut Rewriter<'_>, types: &mut TypeMap) {
    for (new, original) in rw.arena.take_lineage() {
        if !types.is_set(new) && types.is_set(original) {
            let t = types.get(original);
            types.reset_type_of(new, t);
        }
    }
}

fn items(arena: &ExpArena, id: ExpId) -> Vec<ExpId> {
    match &arena[id].exp {
        Exp::Seq(items) => items.clone(),
        _ => Vec::new(),
    }
}

/// `getScope`, the name-node arms.
fn scope_of(arena: &ExpArena, id: ExpId) -> Option<QualifiedName> {
    match &arena[id].exp {
        Exp::Identifier(_) => Some(QualifiedName::new(std::iter::empty())),
        Exp::ScopedName { scope, .. } | Exp::TypeParam { scope, .. } | Exp::This { scope } => Some(scope.clone()),
        Exp::TypeIdentifier(_, a)
        | Exp::QualifiedIdentifier(a)
        | Exp::TemplateInstance { base: a, .. }
        | Exp::FunctionSpecifier { function: a, .. }
        | Exp::NamedValue(a) => scope_of(arena, *a),
        _ => None,
    }
}

fn qualified_name(arena: &ExpArena, id: ExpId) -> Option<Vec<Symbol>> {
    let mut q = get_qualifier(arena, id)?;
    q.push(get_name(arena, id)?);
    Some(q)
}

/// The closure a type is, through `TConst` and the instance
/// (`tClosureFunction`, `tClosureCapture`, `Type.hs:493-503`).
fn closure_of(t: &Ty) -> Option<(&QualifiedName, &Ty)> {
    match t {
        Ty::Closure { func, capture, .. } => Some((func, capture)),
        Ty::Instance { concrete, .. } => closure_of(concrete),
        Ty::Const(inner) => closure_of(inner),
        _ => None,
    }
}

/// `parallel $ desugar closureCalls` over every module.
pub fn closure_calls(program: &mut Program<'_>) {
    let interner = program.interner();
    let closure = Ty::closure_template_path(interner);
    let cancel = program.cancel_token();
    for (i, module) in program.modules.iter_mut().enumerate() {
        cancel.check();
        let _visit = crate::profile::visit("closureCalls", i, module.ir.arena.len());
        let root = module.ir.root;
        let mut rw = Rewriter::new(&mut module.ir.arena, interner);
        let types = &mut module.types;
        // Only a call is rewritten. A subtree passed over leaves its
        // replacements' notes to the next `sync`, which carries every one in
        // the order they were made, before any rewrite reads them.
        let next = desugar_within(&mut rw, root, kg_hir::facts::FUNCTION_CALL, &mut |rw, _id, exp| {
            sync(rw, types);
            let mut m = Mint { rw, types };
            closure_call(&mut m, &closure, exp)
        });
        sync(&mut rw, types);
        module.ir.root = next;
    }
}

/// `closureCalls` (`Closure.hs:19-26`).
fn closure_call(m: &mut Mint<'_, '_>, closure: &QualifiedName, e: Exp) -> Exp {
    let Exp::FunctionCall { attr, function, args } = e else { return e };
    let Exp::FunctionSpecifier { function: b, .. } = m.rw.arena[function].exp else {
        return Exp::FunctionCall { attr, function, args };
    };
    let t = m.types.get(b);
    if !t.is_closure(closure) {
        return Exp::FunctionCall { attr, function, args };
    }
    let (Some((func, capture)), Some(scope)) = (closure_of(&t).map(|(f, c)| (f.clone(), c.clone())), scope_of(m.rw.arena, b))
    else {
        return Exp::FunctionCall { attr, function, args };
    };
    let Some(cap_exp) = m.exp_from_type(b, &capture) else { return Exp::FunctionCall { attr, function, args } };
    let Some(q) = m.qualified_identifier(&scope, b, func.segments()) else {
        return Exp::FunctionCall { attr, function, args };
    };
    // `freeFunctionSpecifier`: `copyNote` of the identifier it wraps.
    let callee = m.copy_note(q, Exp::FunctionSpecifier { receiver: None, function: q });
    let ty = m.new_note(b, cap_exp);
    let value = m.new_note(b, Exp::NamedValue(b));
    let capture = m.new_note(b, Exp::Cast { ty, value });
    // `capture `cons` c`: under `c`'s note.
    let mut list = items(m.rw.arena, args);
    list.insert(0, capture);
    let args = m.copy_note(args, Exp::Seq(list));
    Exp::FunctionCall { attr, function: callee, args }
}

/// `withSymbols' NoTemplate $ parallel . desugar . addLambdaWrappers` over
/// every module.
pub fn add_lambda_wrappers(program: &mut Program<'_>) {
    let interner = program.interner();
    let started = std::time::Instant::now();
    let snapshot = program.snapshot();
    crate::profile::whole("wrappers' snapshot", started);
    let started = std::time::Instant::now();
    let symbols = program.symbols(Config::NoTemplate);
    crate::profile::whole("wrappers' symbols", started);
    let cancel = program.cancel_token();
    let door = program.door;
    for (i, module) in program.modules.iter_mut().enumerate() {
        cancel.check();
        let _visit = crate::profile::visit("addLambdaWrappers", i, module.ir.arena.len());
        let cx = Wrappers { snapshot: &snapshot, symbols: &symbols, interner, module: i, door, read: Default::default() };
        let root = module.ir.root;
        let mut rw = Rewriter::new(&mut module.ir.arena, interner);
        let types = &mut module.types;
        // Only a call is rewritten (`Function.hs:367`), as in `closure_calls`.
        let next = desugar_within(&mut rw, root, kg_hir::facts::FUNCTION_CALL, &mut |rw, _id, exp| {
            sync(rw, types);
            let mut m = Mint { rw, types };
            cx.rewrite(&mut m, exp)
        });
        sync(&mut rw, types);
        module.ir.root = next;
    }
    program.done_with(snapshot);
}

struct Wrappers<'a> {
    snapshot: &'a [Snapshot],
    symbols: &'a Symbols,
    interner: &'a Interner,
    /// The module rewritten, and the door its reads of another module are
    /// shown ([`crate::reads`]).
    module: usize,
    door: Option<&'a crate::reads::Door>,
    /// What the wrapping under way read, shown to the door only where it
    /// changed the module ([`Wrappers::wrap_function_arg`]).
    read: std::cell::RefCell<Vec<WrapRead>>,
}

/// One lookup a wrapping made: the name looked up, what it found, and the
/// variable's type read there.
type WrapRead = (ExpId, Option<Decl>, Option<ExpId>);

/// What a wrapper is built from: its kind, its parameters and its return
/// type, each either a declaration's node (from a snapshot) or minted.
enum Part {
    Decl(usize, ExpId),
    Here(ExpId),
}

impl Wrappers<'_> {
    /// `addLambdaWrappers symbols (FunctionCallF a b c)` (`Function.hs:367`).
    fn rewrite(&self, m: &mut Mint<'_, '_>, e: Exp) -> Exp {
        let Exp::FunctionCall { attr, function, args } = e else { return e };
        let Some(scope) = scope_of(m.rw.arena, function) else { return Exp::FunctionCall { attr, function, args } };
        let old = items(m.rw.arena, args);
        let new: Vec<ExpId> = old.iter().map(|&a| self.wrap_argument(m, &scope, a)).collect();
        if new == old {
            return Exp::FunctionCall { attr, function, args };
        }
        let args = self.renote(m, args, Exp::Seq(new));
        Exp::FunctionCall { attr, function, args }
    }

    /// `wrapFunctionArg` of an argument — of its value, where a later
    /// edition names it (`crate::named`): a function named by a labelled
    /// argument is wrapped as one written positionally is.
    fn wrap_argument(&self, m: &mut Mint<'_, '_>, scope: &QualifiedName, a: ExpId) -> ExpId {
        let Exp::Labelled { label, value } = m.rw.arena[a].exp else { return self.wrap_function_arg(m, scope, a) };
        let wrapped = self.wrap_function_arg(m, scope, value);
        if wrapped == value {
            return a;
        }
        self.renote(m, a, Exp::Labelled { label, value: wrapped })
    }

    fn renote(&self, m: &mut Mint<'_, '_>, id: ExpId, exp: Exp) -> ExpId {
        let new = m.rw.replace(id, exp);
        sync(m.rw, m.types);
        new
    }

    /// `wrapFunctionArg declScopeName e` (`Function.hs:369-373`). What it
    /// reads across the module boundary is the door's only where it
    /// changed the module ([`crate::reads::Door::tentative`]): every round
    /// asks again of an argument that names no function.
    fn wrap_function_arg(&self, m: &mut Mint<'_, '_>, scope: &QualifiedName, e: ExpId) -> ExpId {
        let Some(door) = self.door else { return self.wrap_function_arg_reading(m, scope, e) };
        let before = (m.rw.arena.len(), m.rw.arena.writes(), m.types.writes());
        door.tentative();
        let out = self.wrap_function_arg_reading(m, scope, e);
        let changed = out != e || before != (m.rw.arena.len(), m.rw.arena.writes(), m.types.writes());
        let read = std::mem::take(&mut *self.read.borrow_mut());
        if changed {
            for (q, found, ty) in read {
                // The name as `wrap` read it: the rewriter only appends.
                let (Some(qscope), Some(qname)) = (scope_of(m.rw.arena, q), qualified_name(m.rw.arena, q)) else { continue };
                let view = |d: Decl| (&self.snapshot[d.module].arena, &self.snapshot[d.module].arena[d.id].exp);
                door.lookup(self.module, crate::reads::Asked::InScope(qscope.segments(), &qname), found, view);
                let Some(d) = found else { continue };
                let snap = &self.snapshot[d.module];
                door.node(self.module, d.module, &snap.arena, d.id, &snap.arena[d.id].exp);
                if let Some(ty) = ty {
                    door.ty(self.module, d.module, &snap.arena, ty, snap.types.entry(ty.index()));
                }
            }
        }
        door.settle(self.module, changed);
        out
    }

    fn wrap_function_arg_reading(&self, m: &mut Mint<'_, '_>, scope: &QualifiedName, e: ExpId) -> ExpId {
        match m.rw.arena[e].exp.clone() {
            Exp::NamedValue(q) => self.wrap(m, scope, e, q),
            Exp::MemberAccess { base: o, member: f } => {
                let Ty::Reference(class) = m.types.get(o) else { return e };
                let Some(name) = get_name(m.rw.arena, f) else { return e };
                let mut path = class.segments().to_vec();
                path.push(name);
                let none = QualifiedName::new(std::iter::empty());
                match m.qualified_identifier(&none, o, &path) {
                    Some(q) => self.wrap(m, scope, e, q),
                    None => e,
                }
            }
            _ => e,
        }
    }

    /// `wrap q` (`Function.hs:388-396`).
    fn wrap(&self, m: &mut Mint<'_, '_>, scope: &QualifiedName, e: ExpId, q: ExpId) -> ExpId {
        let (Some(qscope), Some(qname)) = (scope_of(m.rw.arena, q), qualified_name(m.rw.arena, q)) else { return e };
        let found = self.symbols.in_scope(qscope.segments(), &qname);
        if self.door.is_some() {
            self.read.borrow_mut().push((q, found, None));
        }
        if crate::env_flag!("KG_WRAP_TRACE") {
            eprintln!(
                "wrap {:?} in {:?} -> {:?}",
                qname.iter().map(|s| self.interner.resolve(*s).to_string()).collect::<Vec<_>>(),
                qscope.segments().iter().map(|s| self.interner.resolve(*s).to_string()).collect::<Vec<_>>(),
                found.map(|d| format!("{:?}", self.snapshot[d.module].arena[d.id].exp).chars().take(60).collect::<String>())
            );
        }
        let Some(d) = found else { return e };
        let snap = &self.snapshot[d.module];
        match snap.arena[d.id].exp.clone() {
            Exp::Function { kind, params, ret, .. } => {
                self.wrapper(m, scope, e, q, &qname, kind, Part::Decl(d.module, params), Part::Decl(d.module, ret))
            }
            Exp::FunctionDecl { params, ret, .. } => self.wrapper(
                m,
                scope,
                e,
                q,
                &qname,
                FunctionKind::FreeFunction,
                Part::Decl(d.module, params),
                Part::Decl(d.module, ret),
            ),
            Exp::Variable { ty, flags, .. } => {
                if let Some(r) = self.read.borrow_mut().last_mut() {
                    r.2 = Some(ty);
                }
                let t = snap.types.get(ty).the_type();
                if !t.is_function() {
                    return e;
                }
                let kind = if flags == kg_resolve::DeclareFlag::DeclareClass {
                    FunctionKind::MemberFunction
                } else {
                    FunctionKind::FreeFunction
                };
                let params = self.params_from_function_type(m, scope, e, &t);
                let Some(ret_exp) = m.exp_from_type(e, &t.return_ty()) else { return e };
                let ret = m.new_note(e, ret_exp);
                self.wrapper(m, scope, e, q, &qname, kind, Part::Here(params), Part::Here(ret))
            }
            _ => e,
        }
    }

    /// `paramsFromFunctionType t` (`Function.hs:379-386`).
    fn params_from_function_type(&self, m: &mut Mint<'_, '_>, scope: &QualifiedName, e: ExpId, t: &Ty) -> ExpId {
        let params = t.function_params().map(<[_]>::to_vec).unwrap_or_default();
        let mut list = Vec::with_capacity(params.len());
        for (i, p) in params.iter().enumerate() {
            let mut attrs = Vec::new();
            for a in &p.attrs {
                let exp = match a {
                    kg_ty::TAttr::Named(a, b, _) => {
                        let none = QualifiedName::new(std::iter::empty());
                        match m.qualified_identifier(&none, e, b.segments()) {
                            Some(q) => Exp::Attr(*a, q),
                            None => continue,
                        }
                    }
                    kg_ty::TAttr::Int(a, b) => {
                        let lit = m.typed(e, Exp::IntLiteral { ty: None, value: b.clone() }, kg_ty::width::minimal_int_type(b));
                        Exp::IntAttr(*a, lit)
                    }
                    kg_ty::TAttr::Flag(f) => Exp::FlagAttr(*f),
                };
                attrs.push(m.new_note(e, exp));
            }
            let attr = m.new_note(e, Exp::Seq(attrs));
            let ty = match m.exp_from_type(e, &p.ty) {
                Some(x) => m.new_note(e, x),
                None => m.new_note(e, Exp::Auto),
            };
            let name = match p.name {
                Some(n) => Exp::Identifier(n),
                None => Exp::Identifier(self.interner.intern(&format!("$wrapper_param{i}"))),
            };
            let name = m.new_note(e, name);
            list.push(m.new_note(e, Exp::FuncParam { scope: scope.clone(), attr, ty, name }));
        }
        m.new_note(e, Exp::Seq(list))
    }

    /// A declaration's node, copied into this module with its note.
    fn import(&self, m: &mut Mint<'_, '_>, module: usize, id: ExpId) -> ExpId {
        let snap = &self.snapshot[module];
        let node = snap.arena[id].clone();
        let mut exp = node.exp.clone();
        for child in exp.children_mut() {
            *child = self.import(m, module, *child);
        }
        let new = m.rw.arena.alloc(exp, node.src, node.kind);
        if snap.types.is_set(id) {
            m.types.reset_type_of(new, snap.types.get(id));
        }
        new
    }

    fn part(&self, m: &mut Mint<'_, '_>, p: Part) -> ExpId {
        match p {
            Part::Decl(module, id) => {
                if let Some(door) = self.door {
                    let snap = &self.snapshot[module];
                    door.subtree(self.module, module, &snap.arena, Some(&snap.types), id);
                }
                self.import(m, module, id)
            }
            Part::Here(id) => id,
        }
    }

    /// `wrapper kind params ret` (`Function.hs:398-421`).
    #[allow(clippy::too_many_arguments)]
    fn wrapper(
        &self,
        m: &mut Mint<'_, '_>,
        scope: &QualifiedName,
        e: ExpId,
        q: ExpId,
        qname: &[Symbol],
        kind: FunctionKind,
        params: Part,
        ret: Part,
    ) -> ExpId {
        let mut name = String::from("$wrapper");
        for s in qname {
            name.push('$');
            name.push_str(&self.interner.resolve(*s));
        }
        let lambda_name = self.interner.intern(&name);
        let ident = m.new_note(e, Exp::Identifier(lambda_name));
        let mut sig = scope.segments().to_vec();
        sig.push(lambda_name);
        let sig = QualifiedName::new(sig);
        let mut body_scope = sig.segments().to_vec();
        body_scope.push(self.interner.intern("$"));
        let body_scope = QualifiedName::new(body_scope);

        // `lambdaParams = renote (fmap $ renote $ modifyDeclScopeName (const
        // lambdaSignatureDeclScopeName)) params`.
        let params = self.part(m, params);
        let mut lambda_params = Vec::new();
        for p in items(m.rw.arena, params) {
            let exp = modify_decl_scope_name(m.rw.arena[p].exp.clone(), &mut |_| Some(sig.clone()));
            lambda_params.push(self.renote(m, p, exp));
        }
        let params = self.renote(m, params, Exp::Seq(lambda_params.clone()));
        let ret = self.part(m, ret);

        let captures = if kind == FunctionKind::MemberFunction {
            let this = m.new_note(e, Exp::This { scope: scope.clone() });
            m.new_note(e, Exp::Seq(vec![this]))
        } else {
            m.new_note(e, Exp::Seq(Vec::new()))
        };

        // `desugar updateScope q`: every scope in the callee is the body's.
        let callee = self.update_scope(m, q, &body_scope);
        let object = (kind == FunctionKind::MemberFunction).then(|| {
            let this_name = self.interner.intern("$this");
            let this_ident = m.new_note(e, Exp::Identifier(this_name));
            let sn = m.new_note(this_ident, Exp::ScopedName { scope: body_scope.clone(), prefix: None, ident: this_ident });
            let qi = m.new_note(this_ident, Exp::QualifiedIdentifier(sn));
            m.new_note(e, Exp::NamedValue(qi))
        });
        let spec = m.copy_note(callee, Exp::FunctionSpecifier { receiver: object, function: callee });
        // `functionArg x = NamedValueF (localIdentifier lambdaBodyDeclScopeName
        // (declName x))`, each under its parameter's note.
        let mut call_args = Vec::with_capacity(lambda_params.len());
        for &p in &lambda_params {
            let Exp::FuncParam { name, .. } = m.rw.arena[p].exp else { continue };
            let sn = m.new_note(name, Exp::ScopedName { scope: body_scope.clone(), prefix: None, ident: name });
            let qi = m.new_note(name, Exp::QualifiedIdentifier(sn));
            call_args.push(m.copy_note(p, Exp::NamedValue(qi)));
        }
        let call_args = m.copy_note(params, Exp::Seq(call_args));
        let empty = m.new_note(e, Exp::Seq(Vec::new()));
        let call = m.new_note(e, Exp::FunctionCall { attr: empty, function: spec, args: call_args });
        let ret_stmt = m.new_note(e, Exp::Return(Some(call)));
        let body = m.new_note(e, Exp::Seq(vec![ret_stmt]));
        m.new_note(e, Exp::Lambda { scope: scope.clone(), name: ident, captures, params, ret, body })
    }

    /// `desugar updateScope q`: every `declScopeName` in `q`'s subtree but
    /// a `TypeParam`'s set to `scope`, each node keeping its note.
    fn update_scope(&self, m: &mut Mint<'_, '_>, id: ExpId, scope: &QualifiedName) -> ExpId {
        let mut exp = m.rw.arena[id].exp.clone();
        for child in exp.children_mut() {
            *child = self.update_scope(m, *child, scope);
        }
        let exp = if matches!(exp, Exp::TypeParam { .. }) {
            exp
        } else {
            modify_decl_scope_name(exp, &mut |_| Some(scope.clone()))
        };
        if exp == m.rw.arena[id].exp {
            return id;
        }
        self.renote(m, id, exp)
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner, SymbolPath};
    use kg_syntax::{AstNode, ast};
    use kg_ty::{Ty, TyRef};

    use super::*;

    /// `closureCalls` (`Closure.hs:19-26`): a call through a closure-typed
    /// callee calls the closure's function, the closure cast to its capture
    /// first. Red with the pass skipped: the call is as written.
    #[test]
    fn a_call_through_a_closure_calls_its_function_with_the_capture_first() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file("inline void f()\n{\n    g(1);\n}\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let mut program = Program::new([ir], &interner);
        // Type the callee as a closure, as `inferType` would once the
        // `Closure` copy exists.
        let callee = program.modules[0]
            .ir
            .arena
            .iter()
            .find_map(|(id, n)| matches!(n.exp, Exp::FunctionSpecifier { .. }).then_some(id))
            .expect("the call's callee");
        let Exp::FunctionSpecifier { function: b, .. } = program.modules[0].ir.arena[callee].exp else { unreachable!() };
        let capture = Ty::Struct(SymbolPath::new([interner.intern("C")]), Vec::new());
        let closure = Ty::Instance {
            concrete: TyRef::new(Ty::Closure {
                func: SymbolPath::new([interner.intern("h")]),
                func_ty: TyRef::new(Ty::Void),
                capture: TyRef::new(capture),
                alias: SymbolPath::new([interner.intern("Closure<|x|>")]),
            }),
            template: Ty::closure_template_path(&interner),
            args: Vec::new(),
        };
        program.modules[0].types.reset_type_of(b, closure);
        closure_calls(&mut program);
        let m = &program.modules[0];
        let call = crate::elaborate_tests_reachable(&m.ir)
            .into_iter()
            .find_map(|id| match &m.ir.arena[id].exp {
                Exp::FunctionCall { function, args, .. } => Some((*function, *args)),
                _ => None,
            })
            .expect("the call");
        let first = items(&m.ir.arena, call.1)[0];
        assert!(matches!(m.ir.arena[first].exp, Exp::Cast { .. }), "{:?}", m.ir.arena[first].exp);
        assert_eq!(get_name(&m.ir.arena, call.0).map(|s| interner.resolve(s).to_string()).as_deref(), Some("h"));
    }
}
