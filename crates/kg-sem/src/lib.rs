//! Type inference over the lowered IR (`docs/history/ir-elaboration.md`, step
//! M1): the reference's `inferType` (`Type/Inference.hs`) transcribed onto
//! the tree it was written for, rather than translated onto a concrete
//! syntax tree.
//!
//! | module | owns |
//! |---|---|
//! | [`types`] | [`TypeMap`]: a node's type, keyed by the node |
//! | [`symbols`] | `getSymbols` / `lookupSymbol` over the program's IR |
//! | [`infer`] | the fixpoint: `inferExpType`, `evaluateConstexpr`, `staticControl` |
//! | [`census`] | `decidedInstances`: what a round can instantiate |
//! | [`elaborate`] | `instantiateTemplates`, program-wide: the loop, no twin |
//! | [`snapshot`] | the loop's state per module and round, kept, and its shadow |
//! | [`reads`] | the read door: what each module's round read across its boundary |
//!
//! Debugging switches, each printing to stderr: `KG_CENSUS_TRACE` (every
//! instance the census weighs, and why not), `KG_CENSUS_DECIDED` (every
//! decision), `KG_DEDUCE_TRACE` (each function-template deduction's state),
//! `KG_WRAP_TRACE` (each `addLambdaWrappers` lookup).
//!
//! The loop in [`elaborate`] is the program's elaboration: `kg build`'s
//! walk reads its trees, the editor and the parser's checks read its types
//! (`kg_db::looped`; the CST passes' type table derived from them went with
//! the CST inference engine, sprint W3.7), and [`check`] runs the passes
//! after instantiation over its copies.

/// An environment switch, read once per process: the tracing switches sit
/// on paths taken per node or per instance, where reading the environment
/// each time was 6% of a whole-library check.
macro_rules! env_flag {
    ($name:literal) => {{
        static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *FLAG.get_or_init(|| std::env::var_os($name).is_some())
    }};
}
pub(crate) use env_flag;

pub mod census;
pub mod check;
pub mod closures;
pub mod deduce;
pub mod elaborate;
pub mod profile;
pub mod reads;
pub mod reuse;
pub mod snapshot;
pub mod from_type;
pub mod infer;
pub mod instance;
pub mod named;
pub mod pass1;
pub mod pass7;
pub mod symbols;
pub mod types;

pub use infer::{Module, Outcome, Program};
pub use types::TypeMap;

/// Every node reachable from `ir`'s root, for the unit tests.
#[cfg(test)]
pub(crate) fn elaborate_tests_reachable(ir: &kg_hir::ModuleIr) -> Vec<kg_hir::ExpId> {
    let mut out = Vec::new();
    let mut stack = vec![ir.root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        out.push(id);
        kids.clear();
        ir.arena[id].exp.children(&mut kids);
        stack.extend(kids.iter().copied());
    }
    out
}
