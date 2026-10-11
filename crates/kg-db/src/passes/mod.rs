//! The nineteen frontend passes, one module per pass that produces a diagnostic.
//!
//! # Ownership
//!
//! One file per pass, named for the reference's algebra rather than for its
//! number, because the numbers are an artefact of `foldr ($)` and the names are
//! what the Haskell calls them. A pass owns its file completely: its algebra,
//! its helpers, and its tests. Anything two passes need lives in
//! [`crate::facts`].
//!
//! | module | execution-order pass | reference |
//! |---|---|---|
//! | [`early_normalization`] | 0, the parser's own checks; 1 | `Parser.hs:823-828`, `Frontend.hs:74-79` |
//! | [`templates`] | the index pass 0 and the parse-time checks ask | `Symbols.hs:73-87` |
//! | [`undefined_symbols`] | 0, the parse-time table; the run through 9 | `SymbolTable.hs:229-250` |
//! | [`interpolated_string_post`] | 0, the format's parse | `Parser.hs:895-903` |
//!
//! Positions 3, 7, 11 and 15 report nothing here. Position 3 for its own
//! reason: `reifyEnums`' numbering and `captureThis` are *tree rewrites*,
//! the loop's (`kg_hir::rewrite::capture_this` with pass 8's rewrites). It
//! is not diagnostic-free — see the `unwired` table in the crate docs for
//! the spurious `$this` its absence would manufacture — it simply reports
//! nothing itself.
//!
//! A pass moved onto the program-wide loop (sprint W3 step 1) has no module
//! here: its verdicts at a module's text are `kg_sem::check`'s, admitted in
//! its place ([`FromLoop`]) — pass 2 `higherOrderFunctions`, pass 6
//! `inferType`'s errors, pass 9 `undefinedSymbols` (in
//! [`undefined_symbols::run_through_9`]), 10 `typeErrors`, 12
//! `unresolvedTemplates`, 13 `validateExportableClasses`, 14
//! `validateExtern`, 16 `postDesugar`, 17 `deduceAuto`, 18
//! `interpolatedStringPost`, and 19 in its three pieces — each switched once
//! `cargo xtask passdiff` read its CST version, recorded in its place in
//! this run, settled for the loop on every corpus program, and deleted the
//! commit after — passes 4 and 8 last (`TextPass::TemplateTypeCheck`, the
//! loop's census and deduction). What still judges a module's text on the
//! CST is pass 1 and the parser's own checks, asking the loop's parse what
//! it built (`crate::parsed`), and every run is a rooted one
//! ([`crate::Db`]): the text door that ran these over one text with no loop
//! is a `Db` since sprint W3.7 ([`crate::check_source`]). Pass 5's
//! intrinsics, which wrote into the CST engine's table before its fixpoint,
//! went with that engine (sprint W3.7); the loop's are `kg_sem`'s.

pub(crate) mod early_normalization;
pub(crate) mod interpolated_string_post;
pub(crate) mod templates;
pub(crate) mod undefined_symbols;

use kg_base::{FileId, Interner};
use kg_diag::Diagnostic;
use kg_syntax::ast;

use crate::sink::Sink;

/// What the program-wide loop decided at one module's own text, for the
/// CST run to admit in its passes' places ([`run_from_10`]).
pub(crate) struct FromLoop<'a> {
    /// The verdicts of each pass the loop judges this text for
    /// ([`TextPass`]), admitted in that pass's place.
    pub(crate) text: Vec<kg_sem::check::TextVerdict>,
    /// Whether any module of the program erred through pass 9 — the
    /// reference's one flag as pass 10 finds it (`Frontend.hs:44-52,
    /// 85-86`; `crate::Db::program_erred_through_pass_9` computes it) —
    /// asked only where an undecided verdict waits on it, and once.
    erred: &'a dyn Fn() -> bool,
    asked: std::cell::OnceCell<bool>,
}

impl std::fmt::Debug for FromLoop<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FromLoop").field("text", &self.text).finish()
    }
}

/// [`FromLoop`]'s flag where nothing can ask it — no loop, so no verdict
/// waits on it — and, were something to, silence.
pub(crate) fn erred_always() -> bool {
    true
}

impl<'a> FromLoop<'a> {
    /// What `looped` — the loop's run over the program — decided at
    /// `module`'s text. `erred` is the program-wide flag, asked lazily.
    pub(crate) fn of(
        looped: &crate::looped::Looped,
        module: kg_resolve::ModuleSource,
        erred: &'a dyn Fn() -> bool,
    ) -> FromLoop<'a> {
        FromLoop { text: looped.text_for(module).to_vec(), erred, asked: std::cell::OnceCell::new() }
    }

    /// Whether the undecided verdicts wait on nothing: no module erred
    /// through pass 9 ([`kg_sem::check::TextVerdict::undecided`]).
    fn program_clean(&self) -> bool {
        !*self.asked.get_or_init(|| (self.erred)())
    }

    /// `pass`'s verdicts at this text: each behind the checkpoints this
    /// run already passed, per file, and an undecided one behind the
    /// program's flag as well.
    fn admitted(&self, pass: TextPass) -> Vec<Diagnostic> {
        let mine = self.text.iter().filter(|v| v.pass == pass);
        mine.filter(|v| !v.undecided || self.program_clean()).map(|v| v.diagnostic.clone()).collect()
    }

    /// `pass`'s verdicts at this text, admitted in its place.
    fn admit(&self, pass: TextPass, d: &mut Sink) {
        admit(d, self.admitted(pass));
    }
}

/// Verdicts into the sink. A width pass 16 rejects takes whatever an
/// earlier pass reported inside it: `desugarError` replaces the
/// `ParamIntF`, width and all (`Frontend.hs:404-405`; `kg-oracle --errors`
/// on `int<bitsizeof (foo + 1)>` with `foo` a type prints the width's error
/// alone).
fn admit(d: &mut Sink, verdicts: Vec<Diagnostic>) {
    for x in verdicts {
        if x.is(kg_diag::DiagnosticKind::WidthUndecided) {
            d.out.retain(|y| !(y.range.file == x.range.file && x.range.range.contains_range(y.range.range)));
        }
        d.out.push(x);
    }
}

pub(crate) use kg_sem::check::TextPass;

/// What [`run_through_8`] hands [`run_from_10`]: the sink, and whether a
/// checkpoint has already closed the pipeline.
pub(crate) struct Early {
    pub(crate) d: Sink,
    /// `hasErrors` (`Frontend.hs:44-52`): once set, every remaining pass is
    /// `(p, hasErrors)` — skipped. Set at the reference's three checkpoints
    /// and read by [`run_from_10`] and by pass 9's caller.
    pub(crate) gated: bool,
    /// The loop's verdicts in the copies this run took in, each at its pass
    /// ([`Carried`]) — what of them the run shows.
    pub(crate) carried: Vec<Diagnostic>,
}

/// What the loop raised inside the copies of one module's templates, by
/// the pass each is the reference's (`kg_sem::check::Stage`). Each enters
/// the CST run's sink at that pass and no earlier: an error is in the
/// program's error set from the pass that raises it (`Frontend.hs:38-52`),
/// so a copy's conversion closes none of the module's own passes before
/// 19, and a refusal none before 8's checkpoint. Where the run stops first,
/// it is not shown — the reference never reached it.
#[derive(Default)]
pub(crate) struct Carried {
    pub(crate) at_8: Vec<Diagnostic>,
    pub(crate) at_9: Vec<Diagnostic>,
    pub(crate) after_9: Vec<Diagnostic>,
}

impl Carried {
    pub(crate) fn of(staged: &[(kg_sem::check::Stage, Diagnostic)]) -> Carried {
        use kg_sem::check::Stage;
        let mut out = Carried::default();
        for (stage, d) in staged {
            match stage {
                Stage::Instantiation => out.at_8.push(d.clone()),
                Stage::UndefinedSymbols => out.at_9.push(d.clone()),
                Stage::AfterNine => out.after_9.push(d.clone()),
            }
        }
        out
    }
}

impl Early {
    /// `hasErrorSymbol $ getSymbols p'` (`checkPar`, `Frontend.hs:48-52`) for
    /// this file: an error in the sink.
    fn has_errors(&self) -> bool {
        // Pass 12's LT verdict is minted by `unresolvedTemplates` — *after*
        // the pass-9 checkpoint — so it closes nothing: `inline auto Run() {
        // return Fn_T_N(); }` reports pass 17's return type as well as the
        // LT (`kg-oracle --errors`, 2026-09-08). A copy's carries the kind.
        self.d.out.iter().any(|x| x.is_error() && !x.is(kg_diag::DiagnosticKind::CouldNotDeduceAll))
    }

    /// The checkpoint. The reference's is program-wide — `getSymbols` folds
    /// every file — and this one is per file: a file with no early error
    /// keeps its late diagnostics even when another file's early error would
    /// have stopped the reference's pipeline for both. That is the shape the
    /// language server wants, where one file's diagnostics are one payload,
    /// and for a verdict about the file's own text it shows an error the
    /// reference shows once the other file is fixed. It is not enough for a
    /// verdict that something was left *undecided*: another file's error
    /// can be its whole reason — an instantiation that error stopped — and
    /// the file is then correct (`test/library/data/datetime.k` under
    /// `mini-base` read 19 such). Those wait on the program's flag as well
    /// ([`FromLoop`]'s `erred`), which every module's run through this
    /// checkpoint and pass 9's decides (`crate::Db::program_erred_through_pass_9`).
    pub(crate) fn checkpoint(&mut self) {
        if self.has_errors() {
            self.gated = true;
        }
    }
}

/// Passes 1 through 8, with the reference's first two checkpoints.
///
/// `desugarPasses` is `foldr ($) (program, False)` over wrappers that carry
/// a `hasErrors` flag (`Frontend.hs:38-52`): `pass`/`passPar` do nothing
/// once it is set, and it is set by `checkPar` — after the pass-1 composite
/// — by `undefinedSymbols` (pass 9), and by `instantiateTemplates` (pass 8,
/// `hasErrors'`). So an error from pass 1 stops everything after it, and an
/// error from passes 2–8 stops 9 onward. What that decides is *which* errors
/// a rejected program shows, never whether it is rejected: `type-identifier.k
/// ::12` reports pass 9's undefined name and not pass 19's "Global state"
/// eight lines above it, because pass 19 never ran (`kg-oracle --errors`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_through_8(
    file: FileId,
    root: &ast::SourceFile,
    templates: &templates::Templates,
    parsed: &crate::parsed::Parsed,
    interner: &Interner,
    at_8: Vec<Diagnostic>,
    from_loop: &FromLoop,
    out: Vec<Diagnostic>,
) -> Early {
    let d = Sink { file, out };
    let binders = templates::Binders::of(root);
    let mut early = Early { d, gated: false, carried: Vec::new() };

    // The parser's own template checks, before anything else
    // (`Parser.hs:712-721`); see `early_normalization`.
    early_normalization::pass_0_template_arguments_missing(file, root, templates, &binders, interner, &mut early.d);
    early_normalization::pass_0_template_parameter_kinds(root, &mut early.d);
    interpolated_string_post::pass_0_interpolation_format(root, &mut early.d);
    early_normalization::pass_0_attributes_on_a_variable(root, &mut early.d);
    // Whether a name the text writes where a value goes is a type: the type
    // the parser built of it (`crate::parsed`; the CST engine's type table
    // until sprint W3.7).
    let named_type = |e: &ast::Expr| parsed.is_type(kg_syntax::AstNode::syntax(e).text_range());
    early_normalization::pass_0_types_and_constants_as_values(root, &named_type, templates, interner, &mut early.d);
    early_normalization::pass_0_attribute_arguments(root, &mut early.d);
    early_normalization::pass_0_postfix_on_a_name(root, &mut early.d);
    early_normalization::pass_0_values_as_types(root, &|r| parsed.is_value_as_type(r), &mut early.d);
    // Each of those is a `PARSE-ERROR`: the reference never enters
    // `frontend` past one (`Main.hs:73-75`), so pass 1 says nothing beside
    // it — `[[max_threads(int32)]] void Go()` is the attribute's error
    // alone (`type-check.k::17`), where the non-inline error, noted at the
    // function's first token, would come first.
    if early.has_errors() {
        early.gated = true;
        return early;
    }
    early_normalization::pass_1_early_normalization(root, &mut early.d);
    kg_base::cancel::checkpoint();
    // `checkPar` after the pass-1 composite (`Frontend.hs:76-80`).
    early.checkpoint();
    if early.gated {
        return early;
    }

    // Pass 2 at the text is the loop's (sprint W3 step 1): the functions
    // `higherOrderFunctions` refused for want of `inline`.
    from_loop.admit(TextPass::HigherOrderFunctions, &mut early.d);

    // Pass 6 at the text is the loop's (sprint W3 step 1): what its fold
    // made an error of.
    from_loop.admit(TextPass::InferType, &mut early.d);
    kg_base::cancel::checkpoint();
    // Passes 4 and 8 at the text are the loop's (sprint W3 step 1):
    // `instantiateTemplates`' verdicts there — a refusal (`templateTypeCheck`'s
    // `validateArg`), a conflicting deduction (`reconcile`, pass 4's first)
    // — admitted in pass 8's place, before the same checkpoint.
    from_loop.admit(TextPass::TemplateTypeCheck, &mut early.d);
    // `instantiateTemplates`' `hasErrors'` (`Template.hs:55,84`), over its
    // own errors in the copies as well ([`Carried`]).
    early.d.out.extend(at_8.iter().cloned());
    early.carried.extend(at_8);
    early.checkpoint();
    early
}

/// Passes 10 through 19, after pass 9 has had its turn — or not, at the
/// text-only door, which has no program for it. `from_loop` is what the
/// loop decided at this module's text, admitted only here, past pass 9's
/// checkpoint: each pass the loop judges this text for in that pass's
/// place — pass 12's verdicts before the passes that read them (pass 17's
/// return arm reads an LT verdict).
pub(crate) fn run_from_10(early: Early, from_loop: &FromLoop) -> Vec<Diagnostic> {
    let Early { mut d, gated, .. } = early;
    if !gated {
        from_loop.admit(TextPass::TypeErrors, &mut d);
        from_loop.admit(TextPass::UnresolvedTemplates, &mut d);
        from_loop.admit(TextPass::ExportableClasses, &mut d);
        from_loop.admit(TextPass::ValidateExtern, &mut d);
        from_loop.admit(TextPass::PostDesugar, &mut d);
        from_loop.admit(TextPass::DeduceAuto, &mut d);
        from_loop.admit(TextPass::InterpolatedStringPost, &mut d);
        from_loop.admit(TextPass::ValidateProgram, &mut d);
        from_loop.admit(TextPass::CheckShadowDecl, &mut d);
        from_loop.admit(TextPass::CheckConversion, &mut d);
    }
    d.out.sort_by_key(|x| x.range.range.start());
    // One `desugarError` is one `ErrorF` node in the reference, however
    // many passes walk over it; a verdict the loop gives at a node two
    // parents share, or one seeded beside a copy's, can be recorded twice.
    dedup_reports(&mut d.out);
    d.out
}

/// One report per `(range, message, code)`, folding a sorted list's adjacent
/// repeats — and keeping the framed copy where only one is: a twin's seam
/// verdict (`crate::elaborate`'s carry, unframed by construction) is seeded
/// ahead of the base run's, and the base's may carry the `while compiling`
/// note at a template read off another module's copy.
pub(crate) fn dedup_reports(out: &mut Vec<kg_diag::Diagnostic>) {
    out.dedup_by(|later, kept| {
        let same = later.range == kept.range && later.message == kept.message && later.code == kept.code;
        if same && kept.labels.is_empty() && !later.labels.is_empty() {
            std::mem::swap(later, kept);
        }
        same
    });
}
