//! Edition 1.1's named arguments (`docs/editions/1.1.md`): a call's
//! labelled arguments put in its parameters' order, so that what the loop's
//! later passes and the backend walk see is the call written positionally —
//! the reference program the edition's call stands for.
//!
//! No construct of the reference's; nothing here runs on a module without
//! an [`Exp::Labelled`], which only a file of a later edition lowers to
//! (`kg_hir::facts::LABELLED`).
//!
//! # Where the parameters come from
//!
//! The callee's type, as inference left it in the module's own type map: a
//! function type carries its parameters' names (`TFuncParam::name`), a
//! method's and a closure's included. So the pass reads nothing across a
//! module boundary that inference did not already read ([`crate::reads`]).
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
use kg_ty::{QualifiedName, Ty};

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

/// Every call whose callee is typed, its labelled arguments placed, over
/// every module that holds one.
pub fn named_arguments(program: &mut Program<'_>) {
    let interner = program.interner();
    let cancel = program.cancel_token();
    for module in &mut program.modules {
        cancel.check();
        let root = module.ir.root;
        if module.ir.arena.holds(root) & kg_hir::facts::LABELLED == 0 {
            continue;
        }
        let locals = Locals::of(&module.ir.arena);
        let mut rw = Rewriter::new(&mut module.ir.arena, interner);
        let types = &mut module.types;
        let next = desugar_within(&mut rw, root, kg_hir::facts::LABELLED, &mut |rw, _id, exp| {
            sync(rw, types);
            let cx = Cx { interner, locals: &locals };
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
/// no parameters. Outside a template's own body only, which the reference
/// checks nowhere it is not instantiated; never after a truncated loop,
/// whose next round might have placed them.
pub fn unplaced(modules: &mut [crate::infer::Module], interner: &Interner) {
    for module in modules {
        let root = module.ir.root;
        if module.ir.arena.holds(root) & kg_hir::facts::LABELLED == 0 {
            continue;
        }
        let mut calls = Vec::new();
        let mut stack = vec![root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let exp = &module.ir.arena[id].exp;
            match exp {
                Exp::Template { params, .. } => {
                    stack.extend(params.iter().copied());
                    continue;
                }
                Exp::FunctionCall { args, .. } if has_labels(&module.ir.arena, *args) => calls.push(id),
                _ => {}
            }
            kids.clear();
            exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
        for call in calls {
            let Exp::FunctionCall { function, .. } = module.ir.arena[call].exp.clone() else { continue };
            let t = module.types.get(function);
            // An undefined or mistyped callee is reported for what it is
            // (pass 9, inference); a second error here would say less.
            if t.is_undefined() || t.is_error() {
                continue;
            }
            let name = callee_name(&module.ir.arena, function, interner);
            let message = match t.function_params() {
                Some([]) => format!("{name} takes no parameters to name"),
                Some(_) => format!("cannot tell which of {name}'s parameters these arguments name"),
                None => format!("{name} is not a function whose parameters have names"),
            };
            module.ir.arena.node_mut(call).exp = Exp::Error(ErrorNote::new(message));
        }
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

enum Placed {
    /// The call, its arguments in their parameters' order — or an error.
    Call(Exp),
    /// The call as it was: its callee not yet typed.
    Waiting(Exp),
}

struct Cx<'a> {
    interner: &'a Interner,
    locals: &'a Locals,
}

/// One argument as written: where it was, the parameter it fills, and its
/// value with any label taken off.
struct Arg {
    item: ExpId,
    value: ExpId,
    written: usize,
}

impl Cx<'_> {
    fn place(&self, rw: &mut Rewriter<'_>, types: &mut TypeMap, e: Exp) -> Placed {
        let Exp::FunctionCall { attr, function, args } = e else { return Placed::Call(e) };
        let unchanged = Exp::FunctionCall { attr, function, args };
        let Exp::Seq(items) = rw.arena[args].exp.clone() else { return Placed::Call(unchanged) };
        if !items.iter().any(|&i| matches!(rw.arena[i].exp, Exp::Labelled { .. })) {
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
                Some(s @ None) => *s = Some(Arg { item, value, written }),
            }
        }
        if !faults.is_empty() {
            return Placed::Call(self.faulted(rw, types, attr, function, args, &items, &faults));
        }
        let missing: Vec<String> = filled
            .iter()
            .enumerate()
            .filter(|(_, a)| a.is_none())
            .map(|(slot, _)| if pipelined && slot == 0 { "the thread count".to_owned() } else { format!("`{}`", self.param(&params, slot)) })
            .collect();
        if !missing.is_empty() {
            return Placed::Call(Exp::Error(ErrorNote::new(format!("{name} is missing an argument for {}", missing.join(", ")))));
        }
        let filled: Vec<Arg> = filled.into_iter().map(|a| a.expect("every slot is filled")).collect();
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
    fn reordered_effects(&self, arena: &ExpArena, filled: &[Arg]) -> Option<(usize, usize)> {
        let effects: Vec<Effect> = filled.iter().map(|a| self.effect(arena, a.value)).collect();
        for i in 0..filled.len() {
            for j in i + 1..filled.len() {
                if filled[i].written > filled[j].written && !effects[i].commutes(effects[j]) {
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
