//! What the loop did with each list, read off its final trees — the trees
//! the backend walk reads (`kg_ptbridge::program::run`), so that what the
//! expansion writes is what the walk was given, and nothing `kg check` runs
//! records anything for it.
//!
//! A call's arguments keep the range of the list they were written in
//! (`kg_hir::lower`'s `args`), and each placed argument the range of its
//! own text; a default `kg_sem::named` gave the call is donated from the
//! list (`default_argument`), so it wears the list's range alone. A template
//! instance's arguments, defaults and deductions included, are its type's
//! (`TInstance`, `Type.hs:275`), on the node the path was lowered to.
//!
//! Only the copies the walk reads count: a template's own body and the
//! function an abbreviated template was made from are never walked
//! (`removeTemplate`, `Frontend.hs:298-302`; `kg_ptbridge::walk`'s
//! `seq_items`), and are never placed either — their calls wait for an
//! instance. A copy made of a template's body for each instance keeps the
//! body's ranges, so one list may have several copies, one per instance.

use kg_base::{Expansion, FileId, FileRange, FxHashMap, FxHashSet, Interner, Symbol, TextRange, Written};
use kg_hir::{Exp, ExpId};
use kg_ty::{QualifiedName, TArg, Ty};

use crate::spell::{self, Spelled};

/// One argument of a call's copy.
pub(crate) enum Arg {
    /// A value: its node's range, within what was written.
    Value(TextRange),
    /// A default the call was given.
    Default(Result<Spelled, String>),
    /// A label the loop never placed.
    Labelled,
    /// A node a later pass added outside the list (a closure's object
    /// before its arguments, `closureCalls`, `Closure.hs:19-26`), which the
    /// same pass adds to the call as the expansion writes it.
    Added,
}

pub(crate) struct CallCopy {
    pub via: Expansion,
    pub args: Vec<Arg>,
}

pub(crate) struct InstanceCopy {
    pub via: Expansion,
    pub template: QualifiedName,
    pub args: Vec<(Symbol, TArg)>,
}

/// Every walked copy of every call whose list is in one of `files`, by the
/// list's range, and of every instance named at one of `paths`.
#[derive(Default)]
pub(crate) struct Copies {
    pub calls: FxHashMap<FileRange, Vec<CallCopy>>,
    pub instances: FxHashMap<FileRange, Vec<InstanceCopy>>,
}

pub(crate) fn read(
    looped: &kg_db::looped::Looped,
    files: &FxHashSet<FileId>,
    paths: &FxHashSet<FileRange>,
    interner: &Interner,
) -> Copies {
    let mut out = Copies::default();
    for module in &looped.out.modules {
        let arena = &module.ir.arena;
        let types = &module.types;
        let mut stack = vec![module.ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let node = &arena[id];
            match &node.exp {
                Exp::Template { .. } => continue,
                Exp::Function { params, .. } | Exp::FunctionDecl { params, .. }
                    if kg_hir::rewrite::is_abbreviated_template(arena, *params) =>
                {
                    continue;
                }
                Exp::FunctionCall { args, .. } => {
                    let list = arena[*args].src.head;
                    if files.contains(&list.file) {
                        let copy = CallCopy { via: node.src.via, args: call_args(arena, interner, *args, list) };
                        out.calls.entry(list).or_default().push(copy);
                    }
                }
                _ => {}
            }
            if paths.contains(&node.src.head)
                && types.is_set(id)
                && let Some((template, args)) = instance(&types.get(id))
            {
                out.instances.entry(node.src.head).or_default().push(InstanceCopy { via: node.src.via, template, args });
            }
            kids.clear();
            node.exp.children(&mut kids);
            // In program order, for the instances a refusal names first.
            stack.extend(kids.iter().rev().copied());
        }
    }
    out
}

fn call_args(arena: &kg_hir::ExpArena, interner: &Interner, args: ExpId, list: FileRange) -> Vec<Arg> {
    let Exp::Seq(items) = &arena[args].exp else { return Vec::new() };
    items
        .iter()
        .map(|&item| {
            let n = &arena[item];
            if matches!(n.exp, Exp::Labelled { .. }) {
                Arg::Labelled
            } else if n.src.written == Written::Donated && n.src.head == list {
                Arg::Default(spell::default(arena, interner, item))
            } else if n.src.head.file == list.file && list.range.contains_range(n.src.head.range) {
                Arg::Value(n.src.head.range)
            } else {
                Arg::Added
            }
        })
        .collect()
}

/// A template instance's template and arguments, through the type a type's
/// mention has.
fn instance(t: &Ty) -> Option<(QualifiedName, Vec<(Symbol, TArg)>)> {
    match t {
        Ty::Type(inner) | Ty::Const(inner) => instance(inner),
        Ty::Instance { template, args, .. } => Some((template.clone(), args.clone())),
        _ => None,
    }
}
