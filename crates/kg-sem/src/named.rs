//! Edition 1.1's named and default arguments (`docs/editions/1.1.md`): a
//! call's labelled arguments put in its parameters' order, and the default
//! of each parameter it gives no argument written in, so that what the
//! loop's later passes and the backend walk see is the call written
//! positionally and whole — the reference program the edition's call
//! stands for.
//!
//! No construct of the reference's; nothing here runs on a module without
//! an [`Exp::Labelled`], which only a file of a later edition lowers to
//! (`kg_hir::facts::LABELLED`), unless some module of the program declares
//! a default (`kg_hir::facts::DEFAULTED`): a call of any edition's file may
//! lean on one, the callee's file being the one that opted in.
//!
//! # Where the parameters come from
//!
//! The callee's type, as inference left it in the module's own type map: a
//! function type carries its parameters' names (`TFuncParam::name`), a
//! method's and a closure's included. So the pass reads nothing across a
//! module boundary that inference did not already read ([`crate::reads`]).
//! A default travels the same way, folded (`kg_ty::TDefault`).
//! A callee not yet typed — a method of an instance the loop has still to
//! make, a function template whose arguments are still to be deduced
//! (`crate::deduce` matches labels to the template's own parameters) — is
//! left for a later run of the pass; the loop runs it before every pass that
//! reads a call's arguments by position (`crate::elaborate`).
//!
//! But not a value's type: a callee that is a value of function type — a
//! parameter, a callback, a template's function parameter — is typed as
//! what it holds. Pass 2 types a function-typed parameter as the closure
//! of the lambda passed in (`higherOrderFunctions`, `Function.hs:313-359`),
//! whose names are the caller's, and a template's parameter is replaced in
//! its instance by the function given for it (`resolveTypeParam`,
//! `TypeCheck.hs:334-336`), whose names and defaults are that function's.
//! Its labels are the declared type's ([`Declared`]); and no call through a
//! value is given a default, which belongs to a declaration, not a type: a
//! call short of an argument through one is the reference's own count error.
//!
//! # Order of evaluation
//!
//! The backend evaluates a call's arguments in list order
//! (`CallNode::GenerateIR`, `compiler/cpp/ir.cpp:10912-10919`), and the
//! edition evaluates them in the order *written*. Putting them in their
//! parameters' order is exact only where no two arguments it swaps could
//! tell — where neither calls a function, or one calls and the other reads
//! nothing a call could write (a literal, a constant, a local or a parameter
//! of the enclosing function). Where two could tell, the call is an error
//! that says so, rather than a program whose side effects run in another
//! order than written ([`Effect`]).

use kg_base::{FxHashMap, Interner, Symbol};
use kg_hir::rewrite::{desugar_within, get_name, Rewriter};
use kg_hir::{ErrorNote, Exp, ExpArena, ExpId};
use kg_const::ConstValue;
use kg_ty::{QualifiedName, TDefault, Ty};

use crate::infer::Program;
use crate::types::TypeMap;

/// A rewrite's replacement takes its original's type at once.
fn sync(rw: &mut Rewriter<'_>, types: &mut TypeMap) {
    for (new, original) in rw.arena.take_lineage() {
        if !types.is_set(new) && types.is_set(original) {
            let t = types.get(original);
            types.reset_type_of(new, t);
        }
    }
}

/// Every call whose callee is typed, its labelled arguments placed and its
/// defaults given, over every module that holds one: a labelled argument,
/// or — where any module declares a default — a call.
pub fn named_arguments(program: &mut Program<'_>) {
    let interner = program.interner();
    let cancel = program.cancel_token();
    let defaults = program.modules.iter_mut().any(|m| m.ir.arena.holds(m.ir.root) & kg_hir::facts::DEFAULTED != 0);
    let needs = kg_hir::facts::LABELLED | if defaults { kg_hir::facts::FUNCTION_CALL } else { 0 };
    let mut quiet = std::mem::take(&mut program.named_quiet);
    quiet.resize(program.modules.len(), None);
    for (m, module) in program.modules.iter_mut().enumerate() {
        cancel.check();
        let root = module.ir.root;
        if module.ir.arena.holds(root) & needs == 0 {
            continue;
        }
        // A run over what a run that changed nothing read changes nothing.
        let read = Quiet::of(module, needs);
        if quiet[m] == Some(read) {
            continue;
        }
        let _visit = crate::profile::visit("namedArguments", m, module.ir.arena.len());
        place_all(module, interner, defaults, needs);
        quiet[m] = (Quiet::of(module, needs) == read).then_some(read);
    }
    program.named_quiet = quiet;
}

/// What a run of [`named_arguments`] over a module reads that can change
/// between runs: the module's arena — appended to, or written in place —
/// and root, its types, and what the run looks for. The run reads nothing
/// else ([module docs](self)), and reads it the same way each time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Quiet {
    arena: u32,
    len: usize,
    writes: u64,
    root: ExpId,
    types: u32,
    typed: u64,
    needs: kg_hir::facts::Flags,
}

impl Quiet {
    fn of(module: &crate::infer::Module, needs: kg_hir::facts::Flags) -> Quiet {
        let arena = &module.ir.arena;
        Quiet {
            arena: arena.stamp(),
            len: arena.len(),
            writes: arena.writes(),
            root: module.ir.root,
            types: module.types.id(),
            typed: module.types.writes(),
            needs,
        }
    }
}

/// [`named_arguments`] over one module.
fn place_all(module: &mut crate::infer::Module, interner: &Interner, defaults: bool, needs: kg_hir::facts::Flags) {
    let root = module.ir.root;
    let closure = Ty::closure_template_path(interner);
    let locals = std::cell::OnceCell::new();
    let cx = Cx { interner, locals: &locals, before: module.ir.arena.len(), defaults, closure: &closure };
    let mut rw = Rewriter::new(&mut module.ir.arena, interner);
    let types = &mut module.types;
    // A module none of whose calls this run could change is not walked:
    // the walk would copy every node on the way to each call, and answer
    // every call with itself ([`Cx::placeable`]). What the walk's first
    // step does before it reads a call, it does all the same.
    sync(&mut rw, types);
    if !cx.any_placeable(rw.arena, types, root, needs) {
        return;
    }
    let next = desugar_within(&mut rw, root, needs, &mut |rw, _id, exp| {
        sync(rw, types);
        match cx.place(rw, types, exp) {
            Placed::Call(e) | Placed::Waiting(e) => e,
        }
    });
    sync(&mut rw, types);
    module.ir.root = next;
}

/// After the loop's fixpoint: a call whose labels no run of the pass could
/// place is an error — its callee was never typed as a function, or names
/// no parameters — and so is a default that never folded to a constant a
/// call could be given, at the default. Outside a template's own body only,
/// which the reference checks nowhere it is not instantiated; never after a
/// truncated loop, whose next round might have placed them.
pub fn unplaced(modules: &mut [crate::infer::Module], interner: &Interner) {
    let mut found = Vec::new();
    let mut unfolded = false;
    for (m, module) in modules.iter_mut().enumerate() {
        let root = module.ir.root;
        if module.ir.arena.holds(root) & (kg_hir::facts::LABELLED | kg_hir::facts::DEFAULTED) == 0 {
            continue;
        }
        let arena = &module.ir.arena;
        let types = &module.types;
        let mut stack = vec![root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let exp = &arena[id].exp;
            match exp {
                Exp::Template { params, .. } => {
                    stack.extend(params.iter().copied());
                    continue;
                }
                // `abbreviatedFunctionTemplate` keeps the function it made a
                // template of beside it (`Function.hs:288-292`), judged in its
                // copies and never as written (`crate::check`'s `abbreviated`):
                // its `auto` parameter is never typed.
                Exp::Function { params, .. } if kg_hir::rewrite::is_abbreviated_template(arena, *params) => continue,
                Exp::FunctionCall { args, .. } if has_labels(arena, *args) => found.push((m, id, None)),
                // A name in it that names nothing is pass 9's ordinary
                // error, at the name (`crate::check`'s `Undefined`), which a
                // second here would replace.
                Exp::FuncParam { name, default: Some(d), .. } if !folded(arena, *d) => {
                    unfolded = true;
                    if !names_nothing(arena, types, *d) {
                        found.push((m, *d, Some(*name)));
                    }
                }
                _ => {}
            }
            kids.clear();
            exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
    }
    // A function whose default never folded keeps a type that never
    // resolves (`Ty::resolved`), and so does every name of it: a call of one
    // is not reported again for what the default's error already says.
    for (m, id, name) in found {
        let module = &mut modules[m];
        if let Some(name) = name {
            let name = get_name(&module.ir.arena, name).map_or_else(String::new, |n| format!(" of `{}`", interner.resolve(n)));
            let message = format!(
                "the default{name} is not a constant a call can be given: a default is a constant integer, `bool`, `float` or enum value"
            );
            module.ir.arena.node_mut(id).exp = Exp::Error(ErrorNote::new(message));
            continue;
        }
        let Exp::FunctionCall { function, .. } = module.ir.arena[id].exp.clone() else { continue };
        let t = module.types.get(function);
        // An undefined or mistyped callee is reported for what it is (pass
        // 9, inference); a second error here would say less.
        if t.is_undefined() || t.is_error() || (unfolded && !t.resolved()) {
            continue;
        }
        let name = callee_name(&module.ir.arena, function, &t, interner);
        let message = match t.function_params() {
            Some([]) => format!("{name} takes no parameters to name"),
            Some(_) => format!("cannot tell which of {name}'s parameters these arguments name"),
            None => format!("{name} is not a function whose parameters have names"),
        };
        module.ir.arena.node_mut(id).exp = Exp::Error(ErrorNote::new(message));
    }
}

/// Whether a name under `id` is one inference found nothing for
/// (`Ty::Undefined`).
fn names_nothing(arena: &ExpArena, types: &TypeMap, id: ExpId) -> bool {
    let mut stack = vec![id];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        if types.is_set(id) && matches!(types.get(id), Ty::Undefined) {
            return true;
        }
        kids.clear();
        arena[id].exp.children(&mut kids);
        stack.extend(kids.iter().copied());
    }
    false
}

/// Whether a default has folded to what [`crate::infer`] carries in the
/// type (`kg_ty::TDefault::Value`).
fn folded(arena: &ExpArena, id: ExpId) -> bool {
    match &arena[id].exp {
        Exp::IntLiteral { .. } | Exp::BoolLiteral(_) | Exp::FloatLiteral(_) => true,
        // An enum's constant naming another's (`G = F`) nests.
        Exp::EnumValue { value, .. } => {
            let mut inner = *value;
            while let Exp::EnumValue { value, .. } = &arena[inner].exp {
                inner = *value;
            }
            matches!(arena[inner].exp, Exp::IntLiteral { .. })
        }
        _ => false,
    }
}

/// Whether `t` is a lambda's own function's type.
fn is_lambda(t: &Ty) -> bool {
    match t {
        Ty::Function { kind, .. } => *kind == kg_ty::FunctionKind::LambdaFunction,
        Ty::Instance { concrete: x, .. } | Ty::Const(x) => is_lambda(x),
        _ => false,
    }
}

/// Whether the call's labels are still to be placed: it has one, and no
/// pass has reported a fault at one ([`faulted_label`]).
fn has_labels(arena: &ExpArena, args: ExpId) -> bool {
    let Exp::Seq(items) = &arena[args].exp else { return false };
    items.iter().any(|&i| matches!(arena[i].exp, Exp::Labelled { .. })) && !items.iter().any(|&i| faulted_label(arena, i))
}

/// Whether `item` is an argument whose label `crate::deduce` found no
/// parameter of a function template for: its value is the error, at the
/// label, and the call is left as it is.
pub(crate) fn faulted_label(arena: &ExpArena, item: ExpId) -> bool {
    matches!(arena[item].exp, Exp::Labelled { value, .. } if matches!(arena[value].exp, Exp::Error(_)))
}

/// `` `f` `` for a callee with a name, `this function` for one without —
/// typed `t`. A function template's instance's name is the template's and
/// its arguments, mangled: the call is of the template the user named,
/// which its type names. A value's closure type names the closure's
/// template, and the value is named by its own name.
fn callee_name(arena: &ExpArena, function: ExpId, t: &Ty, interner: &Interner) -> String {
    let inner = match &arena[function].exp {
        Exp::FunctionSpecifier { function, .. } => *function,
        _ => function,
    };
    let template = match t {
        Ty::Instance { template, concrete, .. } if matches!(**concrete, Ty::Function { .. }) => template.segments().last().copied(),
        _ => None,
    };
    match template.or_else(|| get_name(arena, inner)) {
        Some(s) => format!("`{}`", interner.resolve(s)),
        None => "this function".to_owned(),
    }
}

/// A default as the argument a call written whole would pass: its literal,
/// or an enum's constant by its name — the enum's own, from the parameter's
/// type, so that the name finds the constant from wherever the call stands
/// — which inference folds as it folds a written one. Made at the call's
/// arguments, so an error in it is reported at the call — but for its
/// conversion to the parameter's type, which `crate::check` judges at the
/// default's declaration and knows the call's given default from a written
/// argument by.
fn default_argument(rw: &mut Rewriter<'_>, args: ExpId, function: ExpId, v: &ConstValue, ty: &Ty) -> Option<ExpId> {
    let exp = match v {
        ConstValue::Int { ty, value } => Exp::IntLiteral { ty: *ty, value: value.clone() },
        ConstValue::Bool(b) => Exp::BoolLiteral(*b),
        ConstValue::Float(f) => Exp::FloatLiteral(*f),
        ConstValue::Enum { name, .. } => {
            let Ty::Enum(enum_name, _) = &ty.un_const() else { return None };
            let constant = *name.segments().last()?;
            let scope = crate::deduce::Cx::scope_of(rw.arena, function)?;
            let mut chain = None;
            for &segment in enum_name.segments().iter().chain([&constant]) {
                let ident = rw.donate(args, Exp::Identifier(segment));
                chain = Some(rw.donate(args, Exp::ScopedName { scope: scope.clone(), prefix: chain, ident }));
            }
            let q = rw.donate(args, Exp::QualifiedIdentifier(chain?));
            Exp::NamedValue(q)
        }
        ConstValue::Str(_) | ConstValue::Aggregate(_) => return None,
    };
    Some(rw.donate(args, exp))
}

enum Placed {
    /// The call, its arguments in their parameters' order — or an error.
    Call(Exp),
    /// The call as it was: its callee not yet typed.
    Waiting(Exp),
}

struct Cx<'a> {
    interner: &'a Interner,
    /// The module's [`Locals`], made the first time a call asks — most
    /// runs place no call, and a call by position never asks — of the
    /// nodes the module held when the run began ([`Cx::before`]), which
    /// the run writes none of in place: as if made then.
    locals: &'a std::cell::OnceCell<Locals>,
    before: usize,
    /// Whether any module declares a default, so that a call with no label
    /// may still be short of one.
    defaults: bool,
    /// `data.closure.core::Closure`, the template of a closure's type.
    closure: &'a QualifiedName,
}

/// The names a callee that is a value of function type was declared with,
/// where they are not its type's ([module docs](self)).
struct Declared {
    /// A function type's, as written: a parameter's or a variable's type —
    /// for a parameter pass 2 made a closure's, its closure's first argument
    /// (`closureFunctionType`, `Function.hs:333-338`), the capture first —
    /// or the kind of the template parameter an instance replaced. `None`
    /// for a kind that spells no function type here, which the reference
    /// does not call (`` `F`: illegal use of this template parameter as a
    /// function ``): no label is placed.
    names: Option<Vec<Option<Symbol>>>,
    /// The template parameter's name, which a message names rather than the
    /// function its instance put in its place.
    param: Option<Symbol>,
}

/// One argument: where it was written — `None` for a default the call is
/// given — and its value with any label taken off.
struct Arg {
    item: ExpId,
    value: ExpId,
    written: Option<usize>,
}

impl Cx<'_> {
    fn locals(&self, arena: &ExpArena) -> &Locals {
        self.locals.get_or_init(|| Locals::of(arena, self.before, self.closure))
    }

    /// Whether [`Cx::place`] could answer the call of `function` with `args`
    /// with anything but the call: one with a label whose callee is typed
    /// as a function, or — where a module declares a default — one written
    /// by position short of arguments its callee has a default for each
    /// of. Read off the call in place, as [`Cx::place`] reads it first.
    fn placeable(&self, arena: &ExpArena, types: &TypeMap, function: ExpId, args: ExpId) -> bool {
        let Exp::Seq(items) = &arena[args].exp else { return false };
        let labelled = items.iter().any(|&i| matches!(arena[i].exp, Exp::Labelled { .. }));
        if !labelled && !self.defaults {
            return false;
        }
        // Not yet a function type — or never one, which the sweep after the
        // fixpoint reports ([`unplaced`]).
        let Some(params) = types.entry(function.index()).and_then(|t| t.function_params()) else { return false };
        // A call written by position is ours only where it is short of
        // arguments and every one it lacks has a default ([`Cx::place`]).
        labelled || {
            let lacking = params.get(items.len()..).unwrap_or_default();
            !lacking.is_empty() && lacking.iter().all(|p| p.default.is_some())
        }
    }

    /// Whether a call under `root` is [`Cx::placeable`], over the subtrees
    /// [`desugar_within`] would walk for `needs` — and, where facts are not
    /// known, more.
    fn any_placeable(&self, arena: &ExpArena, types: &TypeMap, root: ExpId, needs: kg_hir::facts::Flags) -> bool {
        let mut stack = vec![root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            if arena.known_to_lack(id, needs) {
                continue;
            }
            if let Exp::FunctionCall { function, args, .. } = arena[id].exp
                && self.placeable(arena, types, function, args)
            {
                return true;
            }
            kids.clear();
            arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
        false
    }

    fn place(&self, rw: &mut Rewriter<'_>, types: &mut TypeMap, e: Exp) -> Placed {
        let Exp::FunctionCall { attr, function, args } = e else { return Placed::Call(e) };
        let unchanged = Exp::FunctionCall { attr, function, args };
        if !self.placeable(rw.arena, types, function, args) {
            return Placed::Waiting(unchanged);
        }
        let Exp::Seq(items) = rw.arena[args].exp.clone() else { return Placed::Call(unchanged) };
        let labelled = items.iter().any(|&i| matches!(rw.arena[i].exp, Exp::Labelled { .. }));
        let t = types.get(function);
        let Some(mut params) = t.function_params().map(<[_]>::to_vec) else { return Placed::Waiting(unchanged) };
        // A value's labels are its declared type's, and neither a value nor
        // a lambda's own function, which only a call through its closure
        // reaches (`closureCalls`, `Closure.hs:19-26`), is given a default
        // ([module docs](self)).
        let declared = self.declared(rw.arena, function);
        let shown = |arena: &ExpArena| match declared.as_ref().and_then(|d| d.param) {
            Some(p) => format!("`{}`", self.interner.resolve(p)),
            None => callee_name(arena, function, &t, self.interner),
        };
        match declared.as_ref().map(|d| &d.names) {
            Some(Some(names)) => {
                if labelled && !names.is_empty() && names.iter().all(Option::is_none) {
                    let name = shown(rw.arena);
                    return Placed::Call(Exp::Error(ErrorNote::new(format!(
                        "{name}'s type gives its parameters no names, so a call through it passes them by position"
                    ))));
                }
                let unknown = kg_ty::TFuncParam {
                    attrs: kg_ty::TAttrSet::new(),
                    ty: kg_ty::TyRef::new(Ty::Unresolved),
                    name: None,
                    default: None,
                };
                params = names
                    .iter()
                    .enumerate()
                    .map(|(i, &name)| kg_ty::TFuncParam { name, ..params.get(i).cloned().unwrap_or_else(|| unknown.clone()) })
                    .collect();
            }
            // Left for the sweep after the fixpoint, which says it cannot
            // tell ([`unplaced`]).
            Some(None) if labelled => return Placed::Waiting(unchanged),
            _ => {}
        }
        if declared.is_some() || t.is_closure(self.closure) || is_lambda(&t) {
            for p in &mut params {
                p.default = None;
            }
        }
        // A template call not yet deduced is typed with no parameters
        // (`setTypeOfTemplateCall`, `Type.hs:1120-1129`); its instance's
        // type will have them.
        if params.is_empty() {
            return Placed::Waiting(unchanged);
        }
        // A call written by position is ours only where it is short of
        // arguments and every one it lacks has a default: one that lacks a
        // parameter with none is the reference's error, its count reported
        // as the reference reports it (`TypeCheck.hs`'s argument count).
        if !labelled {
            let lacking = params.get(items.len()..).unwrap_or_default();
            if lacking.is_empty() || lacking.iter().any(|p| p.default.is_none()) {
                return Placed::Call(unchanged);
            }
        }
        let name = shown(rw.arena);
        // A `[[pipelined]]` function's type keeps every parameter it
        // declares, its return type made dependent (`funcType`,
        // `Type.hs:1364-1373`), and a call passes the thread count where the
        // first parameter — the thread's index — stands (`argumentRegisters[0]`,
        // `compiler/cpp/ir.cpp:10932`). So that parameter is no argument's
        // to name: the count is written first, by position.
        let pipelined = matches!(t.return_ty(), Ty::Dependent(_));
        let slots = params.len();
        let mut filled: Vec<Option<Arg>> = (0..slots).map(|_| None).collect();
        let mut faults: Vec<(ExpId, String)> = Vec::new();
        for (written, &item) in items.iter().enumerate() {
            let (slot, value) = match rw.arena[item].exp {
                Exp::Labelled { label, value } => {
                    let Some(p) = params.iter().position(|p| p.name == Some(label)) else {
                        faults.push((item, format!("{name} has no parameter named `{}`", self.interner.resolve(label))));
                        continue;
                    };
                    if pipelined && p == 0 {
                        faults.push((
                            item,
                            format!(
                                "`{}` is the thread's index in pipelined {name}, which no call passes: \
                                 the first argument, written by position, is the thread count",
                                self.interner.resolve(label)
                            ),
                        ));
                        continue;
                    }
                    (p, value)
                }
                _ => (written, item),
            };
            match filled.get_mut(slot) {
                None => faults.push((item, format!("{name} takes {} argument{}", slots, if slots == 1 { "" } else { "s" }))),
                Some(Some(_)) => faults.push((item, format!("the parameter `{}` is given an argument twice", self.param(&params, slot)))),
                Some(s @ None) => *s = Some(Arg { item, value, written: Some(written) }),
            }
        }
        if !faults.is_empty() {
            return Placed::Call(self.faulted(rw, types, attr, function, args, &items, &faults));
        }
        let missing: Vec<String> = filled
            .iter()
            .enumerate()
            .filter(|&(slot, a)| a.is_none() && params[slot].default.is_none())
            .map(|(slot, _)| if pipelined && slot == 0 { "the thread count".to_owned() } else { format!("`{}`", self.param(&params, slot)) })
            .collect();
        if !missing.is_empty() {
            return Placed::Call(Exp::Error(ErrorNote::new(format!("{name} is missing an argument for {}", missing.join(", ")))));
        }
        // A default not yet folded may fold in a later round; one that never
        // does is an error at its declaration ([`unplaced`]).
        if filled.iter().zip(&params).any(|(a, p)| a.is_none() && p.default == Some(TDefault::Unfolded)) {
            return Placed::Waiting(unchanged);
        }
        let mut given = Vec::with_capacity(slots);
        for (slot, a) in filled.into_iter().enumerate() {
            given.push(match a {
                Some(a) => a,
                None => {
                    let Some(TDefault::Value(v)) = &params[slot].default else { unreachable!("a slot left empty has a default") };
                    let Some(value) = default_argument(rw, args, function, v, &params[slot].ty) else {
                        return Placed::Waiting(unchanged);
                    };
                    Arg { item: value, value, written: None }
                }
            });
        }
        let filled = given;
        if let Some((early, late)) = self.reordered_effects(rw.arena, types, &filled) {
            // `early` is the parameter first in order and the argument
            // written after `late`'s: placed, it would be evaluated first.
            let message = format!(
                "in {name}'s parameter order `{}` would be evaluated before `{}`, which is written first, \
                 and one of them calls a function: write them in the parameters' order, or bind one to a local first",
                self.label(rw.arena, filled[early].item),
                self.label(rw.arena, filled[late].item),
            );
            let item = filled[early].item;
            return Placed::Call(self.faulted(rw, types, attr, function, args, &items, &[(item, message)]));
        }
        let ordered: Vec<ExpId> = filled.iter().map(|a| a.value).collect();
        let args = rw.replace(args, Exp::Seq(ordered));
        sync(rw, types);
        Placed::Call(Exp::FunctionCall { attr, function, args })
    }

    /// The names `function` was declared with, where it is a value of
    /// function type and not a function's or a method's name.
    fn declared(&self, arena: &ExpArena, function: ExpId) -> Option<Declared> {
        let Exp::FunctionSpecifier { receiver: None, function: f } = arena[function].exp else { return None };
        // A template's function parameter: in its own body, or the function
        // an instance put in its place, which wears the parameter's head
        // (`resolveTypeParam`; `kg_hir::rewrite::instantiate`'s copy).
        let param = match &arena[f].exp {
            Exp::TypeParam { name, kind, .. } => Some((*name, *kind)),
            _ if arena[f].src.written == kg_base::Written::Donated => self.locals(arena).kinds.get(&arena[f].src.head).copied(),
            _ => None,
        };
        if let Some((name, kind)) = param {
            return Some(Declared { names: declared_names(arena, kind, None), param: Some(name) });
        }
        let locals = self.locals(arena);
        let binding = locals.binding(arena, f)?;
        let names = locals.declared.get(&arena[binding.decl].src.head)?;
        Some(Declared { names: Some(names.clone()), param: None })
    }

    fn param(&self, params: &[kg_ty::TFuncParam], i: usize) -> String {
        match params.get(i).and_then(|p| p.name) {
            Some(n) => self.interner.resolve(n).to_string(),
            None => format!("#{}", i + 1),
        }
    }

    fn label(&self, arena: &ExpArena, item: ExpId) -> String {
        match arena[item].exp {
            Exp::Labelled { label, .. } => self.interner.resolve(label).to_string(),
            _ => "a positional argument".to_owned(),
        }
    }

    /// The call with each faulted argument an error at its label — reported
    /// there (`crate::check`'s `fold_errors`) — and every other argument's
    /// label taken off, so that no later run of the pass meets the call
    /// again and reports what this one did twice. The arguments are then in
    /// no parameter's order, which nothing sees: the error stops the
    /// program short of every pass that would read them, and of the walk.
    #[allow(clippy::too_many_arguments)]
    fn faulted(
        &self,
        rw: &mut Rewriter<'_>,
        types: &mut TypeMap,
        attr: ExpId,
        function: ExpId,
        args: ExpId,
        items: &[ExpId],
        faults: &[(ExpId, String)],
    ) -> Exp {
        let mut out = Vec::with_capacity(items.len());
        for &item in items {
            out.push(match faults.iter().find(|(i, _)| *i == item) {
                Some((_, message)) => rw.replace(item, Exp::Error(ErrorNote::new(message.clone()))),
                None => match rw.arena[item].exp {
                    Exp::Labelled { value, .. } => value,
                    _ => item,
                },
            });
        }
        let args = rw.replace(args, Exp::Seq(out));
        sync(rw, types);
        Exp::FunctionCall { attr, function, args }
    }

    /// The first pair of arguments the reordering swaps that could tell it
    /// did — `(i, j)`, parameter positions, `i` written after `j` — if any.
    /// A default is a constant, and is in no written order to keep.
    fn reordered_effects(&self, arena: &ExpArena, types: &TypeMap, filled: &[Arg]) -> Option<(usize, usize)> {
        let effects: Vec<Effect> = filled.iter().map(|a| self.effect(arena, types, a.value)).collect();
        for i in 0..filled.len() {
            for j in i + 1..filled.len() {
                let (Some(wi), Some(wj)) = (filled[i].written, filled[j].written) else { continue };
                if wi > wj && !effects[i].commutes(effects[j]) {
                    return Some((i, j));
                }
            }
        }
        None
    }

    /// What evaluating `id` could do that another argument could see.
    fn effect(&self, arena: &ExpArena, types: &TypeMap, id: ExpId) -> Effect {
        match &arena[id].exp {
            Exp::FunctionCall { .. } => Effect::Calls,
            // A closure is made, not run: what it captures is read.
            Exp::Lambda { .. } => Effect::Reads,
            // A constant is no state, whether or not it has yet folded to
            // the literal it names (`Constexpr.hs:280-290`) — an aggregate
            // never does, and a scalar only once its initializer has, which
            // nothing orders before a call's placement. The module's own
            // constant says so by its declaration, typed or not, a `const`
            // memory's included; an imported one or an enum's, by the type
            // inference gave the name as it typed the call around it
            // (`tIsConst`, `Constexpr.hs:280`).
            Exp::NamedValue(q) => match self.locals(arena).binding(arena, *q) {
                Some(b) if b.local || b.constant => Effect::Pure,
                _ if types.is_set(*q) && types.get(*q).is_const() => Effect::Pure,
                _ => Effect::Reads,
            },
            exp => {
                let mut kids = Vec::new();
                exp.children(&mut kids);
                kids.into_iter().map(|k| self.effect(arena, types, k)).max().unwrap_or(Effect::Pure)
            }
        }
    }
}

/// What evaluating an argument could do that evaluating another could see,
/// least first.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Effect {
    /// Reads nothing a call could write: literals, constants — a global's, a
    /// class's, an enum's, folded or not — and the enclosing function's
    /// locals and parameters, which a callee cannot reach. A name this pass
    /// cannot tell is a constant — one inference has not typed, another
    /// object's member — is read as [`Effect::Reads`]: erring toward an
    /// error is the safe side.
    Pure,
    /// Reads state a call could write: a member, a memory.
    Reads,
    /// Calls a function, which may write anything it can reach.
    Calls,
}

impl Effect {
    /// Whether two arguments evaluate to the same values in either order.
    fn commutes(self, other: Effect) -> bool {
        match (self, other) {
            (Effect::Calls, Effect::Pure) | (Effect::Pure, Effect::Calls) => true,
            (Effect::Calls, _) | (_, Effect::Calls) => false,
            _ => true,
        }
    }
}

/// Every variable and parameter the module declares, by scope and name,
/// with whether it is a function's own — a local, a parameter, or neither;
/// the names of each one's declared function type, by its head; and every
/// template parameter's mention, by head, with its kind.
struct Locals {
    bindings: FxHashMap<(QualifiedName, Symbol), Binding>,
    /// By the declaration's head, which each copy of it keeps: an
    /// instance's copy of a function-typed parameter is typed by the
    /// closure instance the loop decided, whose name spells no names
    /// (`Closure<|…|>`), while the template's own still says them.
    declared: FxHashMap<kg_base::FileRange, Vec<Option<Symbol>>>,
    kinds: FxHashMap<kg_base::FileRange, (Symbol, ExpId)>,
}

#[derive(Clone, Copy)]
struct Binding {
    local: bool,
    /// Declared `const`, so that nothing writes it once it is made.
    constant: bool,
    /// The variable or parameter.
    decl: ExpId,
}

impl Locals {
    /// Of the arena's first `len` nodes.
    fn of(arena: &ExpArena, len: usize, closure: &QualifiedName) -> Locals {
        let mut bindings = FxHashMap::default();
        let mut declared = FxHashMap::default();
        let mut kinds = FxHashMap::default();
        for i in 0..len {
            let id = arena.id_at(i);
            let (scope, name, ty, local) = match &arena[id].exp {
                Exp::Variable { scope, name, flags, ty, .. } => (scope, *name, *ty, *flags == kg_resolve::DeclareFlag::DeclareLocal),
                Exp::FuncParam { scope, name, ty, .. } => (scope, *name, *ty, true),
                Exp::TypeParam { name, kind, .. } => {
                    kinds.insert(arena[id].src.head, (*name, *kind));
                    continue;
                }
                _ => continue,
            };
            if let Some(n) = get_name(arena, name) {
                let constant = matches!(arena[ty].exp, Exp::Const(_));
                bindings.insert((scope.clone(), n), Binding { local, constant, decl: id });
            }
            if let Some(names) = declared_names(arena, ty, Some(closure)) {
                declared.insert(arena[id].src.head, names);
            }
        }
        Locals { bindings, declared, kinds }
    }

    /// The variable or parameter the name `q` reads, innermost scope first;
    /// `None` for a name it cannot place — qualified, imported, a template's
    /// parameter.
    fn binding(&self, arena: &ExpArena, q: ExpId) -> Option<Binding> {
        let q = match &arena[q].exp {
            Exp::QualifiedIdentifier(inner) => *inner,
            _ => q,
        };
        let Exp::ScopedName { scope, prefix: None, ident } = &arena[q].exp else { return None };
        let name = kg_hir::rewrite::ident_sym(arena, *ident)?;
        let segments = scope.segments();
        for k in (0..=segments.len()).rev() {
            let at = QualifiedName::new(segments[..k].iter().copied());
            if let Some(&b) = self.bindings.get(&(at, name)) {
                return Some(b);
            }
        }
        None
    }
}

/// The parameters' names the function type `ty` spells, as inference reads
/// them (`params`, `Type.hs:1350-1356`: a `void` parameter is none) — or,
/// given the closure template, those of the function type a closure's type
/// is made from, its capture dropped (`tFunctionParams`, `Type.hs:528`).
fn declared_names(arena: &ExpArena, ty: ExpId, closure: Option<&QualifiedName>) -> Option<Vec<Option<Symbol>>> {
    let (params, capture) = match &arena[ty].exp {
        Exp::FunctionType { params, .. } => (*params, 0),
        Exp::TypeIdentifier(_, instance) => {
            let (Exp::TemplateInstance { base, args }, Some(closure)) = (&arena[*instance].exp, closure) else { return None };
            let mut path = kg_hir::rewrite::get_qualifier(arena, *base)?;
            path.push(get_name(arena, *base)?);
            if path != closure.segments() {
                return None;
            }
            let Exp::Seq(args) = &arena[*args].exp else { return None };
            let Exp::FunctionType { params, .. } = &arena[*args.first()?].exp else { return None };
            (*params, 1)
        }
        _ => return None,
    };
    let Exp::Seq(params) = &arena[params].exp else { return None };
    Some(
        params
            .iter()
            .skip(capture)
            .filter_map(|&p| match &arena[p].exp {
                Exp::FunctionTypeParam { ty, .. } if matches!(arena[*ty].exp, Exp::Void) => None,
                Exp::FunctionTypeParam { name, .. } => Some(name.and_then(|n| kg_hir::rewrite::ident_sym(arena, n))),
                _ => None,
            })
            .collect(),
    )
}
