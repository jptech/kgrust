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
//! # Order of evaluation
//!
//! The backend evaluates a call's arguments in list order
//! (`CallNode::GenerateIR`, `compiler/cpp/ir.cpp:10912-10919`), and the
//! edition evaluates them in the order *written*. Putting them in their
//! parameters' order is exact only where no two arguments it swaps could
//! tell — where neither calls a function, or one calls and the other reads
//! nothing a call could write (a literal, a local or a parameter of the
//! enclosing function). Where two could tell, the call is an error that says
//! so, rather than a program whose side effects run in another order than
//! written ([`Effect`]).

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
    for module in &mut program.modules {
        cancel.check();
        let root = module.ir.root;
        if module.ir.arena.holds(root) & needs == 0 {
            continue;
        }
        let locals = Locals::of(&module.ir.arena);
        let mut rw = Rewriter::new(&mut module.ir.arena, interner);
        let types = &mut module.types;
        let next = desugar_within(&mut rw, root, needs, &mut |rw, _id, exp| {
            sync(rw, types);
            let cx = Cx { interner, locals: &locals, defaults };
            match cx.place(rw, types, exp) {
                Placed::Call(e) | Placed::Waiting(e) => e,
            }
        });
        sync(&mut rw, types);
        module.ir.root = next;
    }
}

/// After the loop's fixpoint: a call whose labels no run of the pass could
/// place is an error — its callee was never typed as a function, or names
/// no parameters — and so is a default that never folded to a constant a
/// call could be given, at the default. Outside a template's own body only,
/// which the reference checks nowhere it is not instantiated; never after a
/// truncated loop, whose next round might have placed them.
pub fn unplaced(modules: &mut [crate::infer::Module], interner: &Interner) {
    let mut found = Vec::new();
    for (m, module) in modules.iter_mut().enumerate() {
        let root = module.ir.root;
        if module.ir.arena.holds(root) & (kg_hir::facts::LABELLED | kg_hir::facts::DEFAULTED) == 0 {
            continue;
        }
        let arena = &module.ir.arena;
        let mut stack = vec![root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let exp = &arena[id].exp;
            match exp {
                Exp::Template { params, .. } => {
                    stack.extend(params.iter().copied());
                    continue;
                }
                Exp::FunctionCall { args, .. } if has_labels(arena, *args) => found.push((m, id, None)),
                Exp::FuncParam { name, default: Some(d), .. } if !folded(arena, *d) => found.push((m, *d, Some(*name))),
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
    let unfolded = found.iter().any(|(_, _, name)| name.is_some());
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
        let name = callee_name(&module.ir.arena, function, interner);
        let message = match t.function_params() {
            Some([]) => format!("{name} takes no parameters to name"),
            Some(_) => format!("cannot tell which of {name}'s parameters these arguments name"),
            None => format!("{name} is not a function whose parameters have names"),
        };
        module.ir.arena.node_mut(id).exp = Exp::Error(ErrorNote::new(message));
    }
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

fn has_labels(arena: &ExpArena, args: ExpId) -> bool {
    matches!(&arena[args].exp, Exp::Seq(items) if items.iter().any(|&i| matches!(arena[i].exp, Exp::Labelled { .. })))
}

/// `` `f` `` for a callee with a name, `this function` for one without.
fn callee_name(arena: &ExpArena, function: ExpId, interner: &Interner) -> String {
    let inner = match &arena[function].exp {
        Exp::FunctionSpecifier { function, .. } => *function,
        _ => function,
    };
    match get_name(arena, inner) {
        Some(s) => format!("`{}`", interner.resolve(s)),
        None => "this function".to_owned(),
    }
}

/// A default as the argument a call written whole would pass: its literal,
/// or an enum's constant by its name — the enum's own, from the parameter's
/// type, so that the name finds the constant from wherever the call stands
/// — which inference folds as it folds a written one. Made at the call's
/// arguments, so an error in it is reported at the call.
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
    locals: &'a Locals,
    /// Whether any module declares a default, so that a call with no label
    /// may still be short of one.
    defaults: bool,
}

/// One argument: where it was written — `None` for a default the call is
/// given — and its value with any label taken off.
struct Arg {
    item: ExpId,
    value: ExpId,
    written: Option<usize>,
}

impl Cx<'_> {
    fn place(&self, rw: &mut Rewriter<'_>, types: &mut TypeMap, e: Exp) -> Placed {
        let Exp::FunctionCall { attr, function, args } = e else { return Placed::Call(e) };
        let unchanged = Exp::FunctionCall { attr, function, args };
        let Exp::Seq(items) = rw.arena[args].exp.clone() else { return Placed::Call(unchanged) };
        let labelled = items.iter().any(|&i| matches!(rw.arena[i].exp, Exp::Labelled { .. }));
        if !labelled && !self.defaults {
            return Placed::Call(unchanged);
        }
        if !types.is_set(function) {
            return Placed::Waiting(unchanged);
        }
        let t = types.get(function);
        let Some(params) = t.function_params().map(<[_]>::to_vec) else {
            // Not yet a function type — or never one, which the sweep after
            // the fixpoint reports ([`unplaced`]).
            return Placed::Waiting(unchanged);
        };
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
        let name = callee_name(rw.arena, function, self.interner);
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
        if let Some((early, late)) = self.reordered_effects(rw.arena, &filled) {
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
    fn reordered_effects(&self, arena: &ExpArena, filled: &[Arg]) -> Option<(usize, usize)> {
        let effects: Vec<Effect> = filled.iter().map(|a| self.effect(arena, a.value)).collect();
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
    fn effect(&self, arena: &ExpArena, id: ExpId) -> Effect {
        match &arena[id].exp {
            Exp::FunctionCall { .. } => Effect::Calls,
            // A closure is made, not run: what it captures is read.
            Exp::Lambda { .. } => Effect::Reads,
            Exp::NamedValue(q) => {
                if self.locals.is_local(arena, *q) {
                    Effect::Pure
                } else {
                    Effect::Reads
                }
            }
            exp => {
                let mut kids = Vec::new();
                exp.children(&mut kids);
                kids.into_iter().map(|k| self.effect(arena, k)).max().unwrap_or(Effect::Pure)
            }
        }
    }
}

/// What evaluating an argument could do that evaluating another could see,
/// least first.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Effect {
    /// Reads nothing a call could write: literals, and the enclosing
    /// function's locals and parameters — a callee cannot reach them. A
    /// global constant is read as [`Effect::Reads`]: the name is not placed
    /// as a constant here, and erring toward an error is the safe side.
    Pure,
    /// Reads state a call could write: a member, a memory, a global.
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
/// with whether it is a function's own: a local, a parameter, or neither.
struct Locals(FxHashMap<(QualifiedName, Symbol), bool>);

impl Locals {
    fn of(arena: &ExpArena) -> Locals {
        let mut map = FxHashMap::default();
        for i in 0..arena.len() {
            let id = arena.id_at(i);
            match &arena[id].exp {
                Exp::Variable { scope, name, flags, .. } => {
                    if let Some(n) = get_name(arena, *name) {
                        map.insert((scope.clone(), n), *flags == kg_resolve::DeclareFlag::DeclareLocal);
                    }
                }
                Exp::FuncParam { scope, name, .. } => {
                    if let Some(n) = get_name(arena, *name) {
                        map.insert((scope.clone(), n), true);
                    }
                }
                _ => {}
            }
        }
        Locals(map)
    }

    /// Whether the name `q` reads resolves, innermost scope first, to one of
    /// the enclosing function's locals or parameters. A name it cannot
    /// place — qualified, imported, a template's parameter — is not.
    fn is_local(&self, arena: &ExpArena, q: ExpId) -> bool {
        let q = match &arena[q].exp {
            Exp::QualifiedIdentifier(inner) => *inner,
            _ => q,
        };
        let Exp::ScopedName { scope, prefix: None, ident } = &arena[q].exp else { return false };
        let Some(name) = kg_hir::rewrite::ident_sym(arena, *ident) else { return false };
        let segments = scope.segments();
        for k in (0..=segments.len()).rev() {
            let at = QualifiedName::new(segments[..k].iter().copied());
            if let Some(&local) = self.0.get(&(at, name)) {
                return local;
            }
        }
        false
    }
}
