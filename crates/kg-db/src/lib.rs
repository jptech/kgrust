//! The query layer: memoized analyses over a VFS snapshot.
//!
//! Charter: one core serving the batch CLI, the language server, the formatter and
//! the doc generator. Salsa-shaped — inputs are set, derived queries are memoized
//! and invalidated by input revision — but hand-rolled for now so the dependency
//! is a choice rather than a constraint.
//!
//! Dependencies point *inward*: `kg-resolve`, `kg-const` and `kg-ty` are pure
//! libraries over syntax, and this crate orchestrates them. They must never depend
//! on `kg-db`, or the query graph becomes a cycle.
//!
//! Where incrementality stops (see the plan): IDE queries stop at the type-checked
//! HIR. Monomorphization and everything downstream are whole-program and batch-only.
//!
//! # One door
//!
//! [`Db`] — a [`kg_vfs::Vfs`] and a [`kg_config::Settings`], so imports
//! resolve, the base library loads and the whole-program symbol map exists —
//! is how every tool reaches the frontend: `kg build`, `kg check`, the
//! language server, the conformance harness. [`check_source`] is a `Db` over
//! one in-memory file with no base library and no import directories, for
//! callers that hold a text and nothing else (`xtask bench`'s `check` stage,
//! `xtask fuzz`). Until sprint W3.7 it was a second door with passes of its
//! own over a one-file environment; every pass it ran is the loop's now, so
//! it has none.
//!
//! # Module map (owners, one per file)
//!
//! | module | owns |
//! |---|---|
//! | [`db`] | [`Db`], the revision model, every memo table, the loader |
//! | [`program`] | [`Program`] — one resolved compilation: file order, module graph, item trees, loading diagnostics |
//! | [`symbols`] | [`SymbolMap`] — the `getSymbols` query result and its lookup rules |
//! | `imports` | `updateNeeded` and the `redundant-import` warning — the one import diagnostic the loader cannot decide on its own |
//! | `facts` | syntactic facts shared by two or more passes; pure functions of one tree |
//! | [`parsed`] | what the reference's parser built of a module, as its own checks ask it — read off the loop's parse |
//! | [`looped`] | the program-wide loop's result and the editor's reading of it — every type a tool shows |
//! | `sink` | the per-file diagnostic accumulator |
//! | `passes` | one module per diagnostic-producing frontend pass, plus the ordered driver |
//! | [`instances`] | the template instances the program-wide loop made, as `kg dump instances` lists them |
//! | [`navigate`] | [`Definition`] — position → declaration, the query every semantic editor feature is built on |
//! | [`passdiff`] | a CST pass against the loop's version of it on a module's own text — `cargo xtask passdiff`, sprint W3 step 1's instrument |
//!
//! # Why navigation is here and not in the language server
//!
//! [`Db::definition_at`] resolves a name through the *same four sources in the
//! same order* as the CST pass 9's `is_known` did, with the coarse first source
//! replaced by a real scope walk. Written in `kg-lsp` it would be a second name
//! resolver, free to drift from the one that decides the `"name not in scope"`
//! diagnostic. Pass 9 is the loop's since sprint W3 step 1 (`lookupInScope`
//! over the loop's program, `kg_sem::check`), and the CST pass with its
//! `is_known` is deleted; that the two still cannot disagree is held by
//! `navigate::tests::a_name_pass_9_rejects_never_resolves`, at every offset
//! pass 9 reports.
//!
//! # Sound, not complete — the rule every pass obeys
//!
//! A **false negative** — failing to report an error the reference reports —
//! fails one conformance assertion (`Undecidable` until `FRONTEND_COMPLETE`
//! was set, 2026-10-10), and says what is missing. A **false positive** — reporting an error the
//! reference does not — breaks an `expected:0` assertion *and* every one of the
//! 241 `pass-through` assertions the reference defers to the C++ backend.
//!
//! They are not symmetric. **When you are unsure whether the reference rejects
//! something, stay silent.** That asymmetry is why the `unwired` table below is
//! as long as it is, and why the passes that need whole-program symbol
//! resolution stayed there rather than being approximated. Build the docs with
//! `--document-private-items` to read it rendered.

pub mod complete;
pub mod db;
mod docs;
mod budget;
mod facts;
mod imports;
pub mod instances;
mod layered;
pub mod looped;
pub mod navigate;
pub mod parsed;
pub mod passdiff;
mod passes;
pub mod program;
mod sem;
mod snapshot;
pub mod sorted;
pub mod stats;
mod sink;
pub mod symbols;
pub mod text_map;

pub use db::{Db, Revision};
pub use budget::PassBudget;
pub use kg_resolve::ModuleSource;
pub use complete::Completion;
pub use navigate::Definition;
pub use passes::interpolated_string_post::interpolations;
pub use program::Program;
pub use symbols::SymbolMap;

use kg_diag::Diagnostic;

/// Check one source text as a whole program — a `Db` holding it as its one
/// file, with no base library and no import directories — and return every
/// diagnostic the frontend gives it, in program order.
///
/// **One door.** This was the text door, with passes of its own run over a
/// one-file constant environment and the CST inference engine's table for
/// one text — both deleted with that engine (sprint W3.7). Sprint W3 moved every pass that reads a type onto the
/// program-wide loop, which only a `Db` runs, so the text door had come to
/// run the parser's checks and pass 1 and nothing else, while every caller
/// read it as "the frontend". It is the rooted door now, and the program is
/// the text alone: an `import` names a module there is no file for (the
/// loader's error), and a name only the base library declares is undefined
/// (pass 9's) — what the reference says of the same text compiled with
/// `--no-implicit-base` and no import directory.
///
/// # Do not diagnose earlier than the reference
///
/// 241 conformance assertions expect a specific C++ backend error code. A
/// frontend that catches those at name-resolution time will *fail* them. The
/// division of labour is reproduced, not improved — see
/// `docs/known-divergences.md`.
pub fn check_source(text: &str) -> Vec<Diagnostic> {
    let mut settings = kg_config::Settings::fallback("/kg-check-source");
    settings.implicit_base = false;
    let mut db = Db::new(settings);
    let file = db.set_file_text(kg_base::Utf8Path::new("/kg-check-source/main.k"), text.to_owned());
    db.check(&[file])
}

/// Which pass runs where, and its state, is `docs/status.md`'s "Frontend
/// passes" table — kept there, once, so this crate's docs cannot drift from
/// it (a copy here described the twin's era until 2026-09-27; it is in
/// `docs/history/status-2026-09-26.md`). Execution-order numbering of
/// `docs/spec/frontend-passes.md` §3.
///
/// Four diagnostics that are **not** frontend passes and are wired at [`Db`]
/// rather than here, because they belong to loading: a failed import and a
/// circular one (`Parser.hs:201-206`, `Parser/IO.hs:90`), a module whose
/// declared namespace disagrees with the path it was found at
/// (`Parser.hs:203-204`), `checkExposed`'s module half (`Parser.hs:224-231`),
/// and the `redundant-import` warning (`Parser.hs:234-241`, [`crate::imports`]).
/// See [`Program::diagnostics`]. `checkExposed`'s remaining third,
/// `checkSymbols` (`Parser.hs:216-223`), needs
/// [`kg_resolve::find_symbol`] and belongs with pass 9.
///
/// The rewrite-only passes are listed rather than omitted because their *order*
/// still matters to whoever wires the next diagnostic-producing one.
mod unwired {}

#[cfg(test)]
mod tests;
