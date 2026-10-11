//! The loop's diagnostics (`docs/plans/ir-elaboration.md` §2.5, step M4):
//! the census's `validateArg` refusals, and pass 19's `checkConversion`
//! (`TypeCheck.hs:87-178`) — the first of the passes after instantiation
//! to run on the IR.
//!
//! The reference runs every pass after `instantiateTemplates` over the
//! program with the copies in it, and reports an error a copy's body raises
//! at the template body's own text, with each instantiation site below it as
//! a `while compiling` note (`expandErrorStack`, `Error.hs:176-195`). Here a
//! copied node's `Src` *is* that note: `head` the body range, `via` the
//! chain of sites ([`kg_base::ExpansionInterner::copy_frames`]). The check
//! runs over every node outside a template's own body, with the sites and
//! silences the CST pass had (its tests are `kg_db`'s `tests::conversion`
//! now): in a copy it is the only voice, and at the module's own text its
//! verdict is
//! [`TextPass::CheckConversion`]'s, admitted in pass 19's place (sprint W3
//! step 1). The walk's other verdicts at the text are still unframed
//! "seam" verdicts among the copies', yielding to the CST passes' at the
//! same node.
//!
//! A pass moved off the CST (sprint W3 step 1, [`TextPass`]) judges the
//! module's own text here too: its verdicts there are [`Checked::text`],
//! which the CST run admits in the pass's place.
//!
//! One silence of the CST pass is not kept: it stands down where either
//! side is a class, struct, union or enum copy's instance, whose spelling
//! the twin could not give — its table typed a copy as its bare class. The
//! engine types it `TInstance`, as `setTypeOfInstance` does
//! (`Type.hs:426-440`), and `showUnmangled` spells that as the reference
//! does (`nested-templates.k::6`'s `Foo<uint4>::Bar<uint3>`, `struct.k::3`,
//! `initializer-list.k::50`'s elements under `Bux<bool, uint32>`).
//!
//! Three sites, as there: a call's arguments (`checkArguments`,
//! `TypeCheck.hs:87-97`), a variable's initializer (`checkVarDecl`,
//! `:123-144`) and a function's `return` (`checkReturn`, `:84`). The walk is
//! post-order: `errorMsg` replaces the node it judges with an `ErrorF`
//! (`Error.hs:147-148`), and `null (errors a)` (`:162`) then keeps an
//! enclosing conversion quiet — an argument's verdict comes before its
//! call's.

use kg_base::{Expansion, ExpansionInterner, FileRange, FxHashMap, Interner, TextRange, TextSize};
use kg_const::ConstValue;
use kg_diag::{Diagnostic, DiagnosticKind};
use kg_hir::{Exp, ExpId, ModuleIr};
use kg_resolve::def::DeclareFlag;
use kg_ty::convert::{Narrowing, cast_fix, invalid_conversion_error, narrowing_warning, normalize_source};
use kg_ty::subtype::{initialized_type_pretty, show_unmangled_with};
use kg_ty::{Flag, QualifiedName, TAttr, Ty, TyRef, is_convertible_to, is_subtype};

use crate::elaborate::Elaborated;
use crate::infer::Module;

/// Every diagnostic the loop's passes raise: the census's refusals
/// (`validateArg`, wherever the instance is written) and the checks here
/// inside the copies — each framed by its instantiation sites, and merged
/// where several copies of one body raise the same one.
///
/// `leftovers` asks the verdicts that judge what the loop left undone in
/// its copies — pass 12's inferential arms and a `static if` left standing
/// in a copy. On by default since sprint W1.4, once asking them changed no
/// verdict the gates see; the setting is `kg_config::Verdicts::leftovers`
/// (`KG_LEFTOVERS=0` clears it, read at the front door).
pub fn diagnostics(e: &Elaborated, interner: &Interner, leftovers: bool) -> Vec<Diagnostic> {
    diagnostics_at(e, interner, leftovers, &|_| None)
}

/// The `[` before a written array dimension, where the text knows it.
pub type Bracket<'a> = &'a dyn Fn(FileRange) -> Option<FileRange>;

/// [`diagnostics`], with the text to hand: `bracket` is the `[` a written
/// array dimension's bracket group opens with, given the dimension's range
/// — the note the reference's `brackets expression` gives it
/// (`TypeCheck.hs:189`), which the IR does not keep. Without it the
/// dimension's own range is reported.
pub fn diagnostics_at(e: &Elaborated, interner: &Interner, leftovers: bool, bracket: Bracket<'_>) -> Vec<Diagnostic> {
    let Checked { diagnostics, text, .. } = checked(e, interner, leftovers, false, bracket);
    let mut out: Vec<Diagnostic> = diagnostics.into_iter().map(|(_, d)| d).collect();
    // The loop's own verdicts at a module's text: the census's refusals and
    // a round's conflicts (pass 8's), and pass 12's.
    let own = |v: &TextVerdict| matches!(v.pass, TextPass::TemplateTypeCheck | TextPass::UnresolvedTemplates);
    out.extend(text.into_iter().filter(own).map(|v| v.diagnostic));
    out
}

/// The pass a verdict the loop raises inside the copies is the
/// reference's: what decides which of the CST run's checkpoints
/// (`Frontend.hs:38-52`) it is behind. Each enters the run's sink at its
/// pass and no earlier, so it closes no checkpoint before it — a copy's
/// conversion stops none of the module's own passes 2 to 18 — and is
/// shown only where the run got that far.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Pass 8, `instantiateTemplates`: a refusal, a round's error,
    /// inference's own in a copy (`Template.hs:55-56,84`).
    Instantiation,
    /// Pass 9, `undefinedSymbols` in a copy (`Frontend.hs:85-86`).
    UndefinedSymbols,
    /// Passes 10 to 19 in a copy, which no checkpoint follows.
    AfterNine,
}

/// What the loop's passes raise, in three parts by who may admit them.
#[derive(Debug, Default)]
pub struct Checked {
    /// Inside the copies, and the rounds' own — each framed by its
    /// instantiation sites, and with the pass it is the reference's
    /// ([`Stage`]). The CST passes never judge these nodes, so each enters
    /// the CST run's sink at its pass.
    pub diagnostics: Vec<(Stage, Diagnostic)>,
    /// The verdicts at a module's own text of each pass the loop now
    /// judges there in the CST pass's place ([`TextPass`]).
    pub text: Vec<TextVerdict>,
    /// Pass 9's verdicts at a module's own text on a name a rewrite made,
    /// which only the text can place ([`CopiedName`]).
    pub copied: Vec<CopiedName>,
    /// Whether the loop's own part of the program raised an error by the
    /// end of pass 9 — a refusal or a round's error, which stop
    /// `instantiateTemplates` with `hasErrors'` set (`Template.hs:55-56,
    /// 84`), or pass 9 in a copy (`Frontend.hs:85-86`). The rest of the
    /// reference's flag is each module's run at its text, which `kg_db`
    /// folds in (`Db::program_erred_through_pass_9`), and [`Checked::copied`]
    /// that no text spells.
    pub erred: bool,
}

/// One verdict at a module's own text ([`Checked::text`]).
#[derive(Clone, Debug)]
pub struct TextVerdict {
    pub pass: TextPass,
    /// The module whose CST run admits it, by its file: the one whose text
    /// it is at — for pass 13, the one whose `export` or `extern` made the
    /// class exportable, which may declare it in another.
    pub home: kg_base::FileId,
    pub diagnostic: Diagnostic,
    /// Whether it says something was left undecided — a type the loop never
    /// settled — which an error in *another* module can cause: the
    /// instantiation that error stopped is what would have settled it
    /// (`test/library/data/datetime.k` under `mini-base`). The reference
    /// never shows one then: its one flag stops every pass after the first
    /// that errs, program-wide (`Frontend.hs:44-52`). So the CST run admits
    /// such a verdict only where no module erred through pass 9, and every
    /// other verdict behind its own per-file checkpoints (`kg_db`'s
    /// `passes::FromLoop`).
    pub undecided: bool,
}

/// A pass whose verdicts at a module's own text — not only in its copies —
/// are the loop's (sprint W3 step 1, one pass at a time): computed whether
/// or not an earlier verdict stopped the program, since the CST run admits
/// them in its pass's place, past the same checkpoints
/// (`Frontend.hs:38-52`) the CST pass they replace ran behind. So a module
/// shows what it showed: `cargo xtask passdiff` held the two identical on
/// every corpus root and conformance snippet before each switch.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TextPass {
    /// Pass 2, `higherOrderFunctions` (`Function.hs:313-325`): a function
    /// taking a function — written so, or through an alias
    /// (`resolvedFuncParams`, `:133-141`) — that is not `inline`, at the
    /// function, wherever it is written ([`Elaborated::higher_order`]).
    HigherOrderFunctions,
    /// Pass 6, `inferType` (`Type/Inference.hs:25-69`): what
    /// `evaluateConstexpr` turned into an error as it folded the module's
    /// text — a division or remainder by zero, `bytesizeof` of a width
    /// that is not whole bytes, an illegal `decltype`, `bitoffsetof` or
    /// `byteoffsetof`, a `mux` selector out of range (`Constexpr.hs:
    /// 181-186,274-275,304-305,345-355`) — wherever inference reaches: a
    /// template's parameters, never its body (`Inference.hs:55-69`).
    InferType,
    /// Pass 8, `instantiateTemplates`' verdicts at the text: `validateArg`'s
    /// refusal of an argument the text wrote (`templateTypeCheck`,
    /// `TypeCheck.hs:218-297`), and `reconcile`'s conflicting deduction at a
    /// call the text wrote (`Type.hs:1502-1506`) — pass 4's first, and
    /// before the same checkpoint (`Frontend.hs:38-52`). The CST reported
    /// both from its passes 4 and 8.
    TemplateTypeCheck,
    /// Pass 9, `undefinedSymbols` (`Frontend.hs:82-157`).
    UndefinedSymbols,
    /// Pass 10, `cata typeErrors` (`Frontend.hs:169-174`).
    TypeErrors,
    /// Pass 12, `unresolvedTemplates` (`Frontend.hs:409-432`): an instance
    /// left standing at the text — too few arguments or too many, an
    /// argument `templateArgResolved` refuses, "Failed to instantiate" —
    /// asked only where the loop's own part of the flag is clear (no
    /// refusal, no error in a copy through pass 9, `Frontend.hs:86-87`).
    UnresolvedTemplates,
    /// Pass 13, `validateExportableClasses` (`Frontend.hs:337-391`).
    ExportableClasses,
    /// Pass 14, `validateExtern` (`Frontend.hs:304-313`).
    ValidateExtern,
    /// Pass 16, `postDesugar` (`Frontend.hs:176-183`).
    PostDesugar,
    /// Pass 17, `deduceAuto` (`Type.hs:1595-1610`).
    DeduceAuto,
    /// Pass 18, `interpolatedStringPost` (`Frontend.hs:241-271`).
    InterpolatedStringPost,
    /// Pass 19's `checkConversion` sites (`TypeCheck.hs:84-178`): a call's
    /// arguments and their count (`checkFunctionArgs`, `:87-97`), a
    /// variable's initializer with the ECC memory clause before it
    /// (`checkVarDecl`, `:123-144`), and a `return` at a function body's
    /// top level (`checkReturn`, `:76,84`) — `validateProgram`'s one `cata`,
    /// split by the CST module each clause was ported to.
    CheckConversion,
    /// Pass 19's `checkShadowDecl` (`TypeCheck.hs:50,110-121`): a local
    /// that shadows a variable or a parameter in scope, a warning.
    CheckShadowDecl,
    /// The rest of pass 19, `validateProgram` (`TypeCheck.hs:38-71,180-215`):
    /// a type as an operand, an array's dimension, an integer attribute's
    /// argument, an enum's base and constants, a variable's global state and
    /// function type, an undeduced `auto`, a constant assigned, a `[[reset]]`
    /// function, a `default` initializer and an initializer's overridden
    /// designator — and the type-as-a-value positions the reference's
    /// parser rejects (`Parser.hs:823-828`), which the CST pass ported here.
    ValidateProgram,
}

impl TextPass {
    pub const ALL: [TextPass; 14] = [
        TextPass::HigherOrderFunctions,
        TextPass::InferType,
        TextPass::TemplateTypeCheck,
        TextPass::UndefinedSymbols,
        TextPass::TypeErrors,
        TextPass::UnresolvedTemplates,
        TextPass::ExportableClasses,
        TextPass::ValidateExtern,
        TextPass::PostDesugar,
        TextPass::DeduceAuto,
        TextPass::InterpolatedStringPost,
        TextPass::CheckConversion,
        TextPass::CheckShadowDecl,
        TextPass::ValidateProgram,
    ];

    /// The reference's name for the pass, as the differential prints it.
    pub fn name(self) -> &'static str {
        match self {
            TextPass::HigherOrderFunctions => "higherOrderFunctions",
            TextPass::InferType => "inferType",
            TextPass::TemplateTypeCheck => "templateTypeCheck",
            TextPass::UndefinedSymbols => "undefinedSymbols",
            TextPass::TypeErrors => "typeErrors",
            TextPass::UnresolvedTemplates => "unresolvedTemplates",
            TextPass::ExportableClasses => "validateExportableClasses",
            TextPass::ValidateExtern => "validateExtern",
            TextPass::PostDesugar => "postDesugar",
            TextPass::DeduceAuto => "deduceAuto",
            TextPass::InterpolatedStringPost => "interpolatedStringPost",
            TextPass::CheckConversion => "checkConversion",
            TextPass::CheckShadowDecl => "checkShadowDecl",
            TextPass::ValidateProgram => "validateProgram",
        }
    }
}

/// `pass`'s verdicts at every module's own text, in program order, whatever
/// an earlier verdict would have stopped — what [`Checked::text`] holds of
/// it, for the differential against the CST pass.
pub fn text_verdicts(e: &Elaborated, interner: &Interner, pass: TextPass) -> Vec<Diagnostic> {
    if pass == TextPass::HigherOrderFunctions {
        return higher_order_functions(e);
    }
    if pass == TextPass::InferType {
        return fold_errors(e);
    }
    if pass == TextPass::TemplateTypeCheck {
        let mut out: Vec<Diagnostic> = e
            .refused
            .iter()
            .filter(|(src, _)| e.expansions.copy_frames(src.via).is_empty())
            .map(|(src, message)| Diagnostic::error(src.head, message.clone()))
            .collect();
        for m in &e.modules {
            for id in errors_in(&m.ir) {
                let node = &m.ir.arena[id];
                if let Exp::Error(note) = &node.exp
                    && note.is(DiagnosticKind::ConflictingDeduction)
                    && e.expansions.copy_frames(node.src.via).is_empty()
                {
                    merge(&mut out, Diagnostic::error(node.src.head, note.message.to_string()).with_kind(note.kind));
                }
            }
        }
        return out;
    }
    let symbols = crate::symbols::Symbols::with(e.modules.iter().map(|m| &m.ir), interner, crate::symbols::Config::AllSymbols);
    // A name a rewrite made is `kg_db`'s to place by the text
    // ([`CopiedName`]); with no text here, every one is given.
    if pass == TextPass::UndefinedSymbols {
        let p9 = undefined_symbols(e, &symbols, interner);
        return p9.text.into_iter().chain(p9.copied.into_iter().map(|c| c.diagnostic)).collect();
    }
    let externs = validate_extern(e, &symbols);
    let exportables = exportable_classes(e, &symbols, interner);
    let earlier = earlier_text(e, &symbols, interner, false, exportables.text, externs.text);
    let mut out: Vec<Diagnostic> = earlier.iter().filter(|v| v.pass == pass).map(|v| v.diagnostic.clone()).collect();
    let replaced = replaced_by(&earlier);
    let walked = walk_modules(e, &symbols, interner, true, false, &|_| None, &replaced);
    if pass == TextPass::UnresolvedTemplates {
        out.extend(walked.late);
        out.extend(walked.late_named.into_iter().map(|c| c.diagnostic));
    }
    if pass == TextPass::TypeErrors {
        out.retain(|d| !inside_any(&walked.replacing, d.range));
    }
    out.extend(walked.text.into_iter().filter(|(_, p, _)| *p == pass).map(|(_, _, d)| d));
    out
}

/// Whether `at` lies inside one of `ranges`.
fn inside_any(ranges: &[FileRange], at: FileRange) -> bool {
    ranges.iter().any(|r| r.file == at.file && r.range.contains_range(at.range))
}

/// Pass 9 over every module ([`Undefined`]): in the copies, and at each
/// module's own text, each verdict once.
fn undefined_symbols(e: &Elaborated, symbols: &crate::symbols::Symbols, interner: &Interner) -> UndefinedVerdicts {
    let mut out = UndefinedVerdicts::default();
    let defaults = std::cell::OnceCell::new();
    for m in &e.modules {
        let mut p9 = Undefined {
            m,
            modules: &e.modules,
            symbols,
            expansions: &e.expansions,
            interner,
            raised: Vec::new(),
            text: Vec::new(),
            copied: Vec::new(),
            callee: None,
            template_default: None,
            defaults: &defaults,
        };
        p9.walk(m.ir.root);
        out.raised.extend(p9.raised);
        for d in p9.text {
            if !out.text.iter().any(|x| x.range == d.range && x.message == d.message) {
                out.text.push(d);
            }
        }
        for c in p9.copied {
            if !out.copied.iter().any(|x| x.diagnostic.range == c.diagnostic.range && x.diagnostic.message == c.diagnostic.message) {
                out.copied.push(c);
            }
        }
    }
    out
}

/// Where every template template parameter's default is written as a name
/// — `typename M = memory`'s `memory` — in any module
/// ([`Undefined::template_default`]).
fn template_defaults(modules: &[Module]) -> Vec<FileRange> {
    let mut out = Vec::new();
    for m in modules {
        let arena = &m.ir.arena;
        for (_, node) in arena.iter() {
            let Exp::TemplateParam { kind, default: Some(d), .. } = &node.exp else { continue };
            let (Exp::Template { .. }, Exp::NamedValue(name)) = (&arena[*kind].exp, &arena[*d].exp) else { continue };
            let mut at = *name;
            loop {
                match &arena[at].exp {
                    Exp::QualifiedIdentifier(a) => at = *a,
                    Exp::ScopedName { ident, .. } => at = *ident,
                    _ => break,
                }
            }
            out.push(arena[at].src.head);
        }
    }
    out
}

#[derive(Default)]
struct UndefinedVerdicts {
    raised: Vec<(Expansion, Diagnostic)>,
    text: Vec<Diagnostic>,
    copied: Vec<CopiedName>,
}

/// Pass 9's verdict at a module's own text on a name whose identifier a
/// rewrite made: a copy of a written one — `higherOrderFunctions` copies a
/// function-typed parameter's type into the template parameter it makes, so
/// `([[last]] is_last) -> void Do` is judged there
/// (`higher-order-functions.k::210`, `kg-oracle --errors`: "Type name not in
/// scope: `is_last`" at the name) — or one it minted, `desugarLambdas`'
/// `make_captureN` with no base library to declare it, the reference's
/// "Symbol name not in scope" at a note no text here spells (`kg-oracle
/// --errors --no-base`, 2026-10-10). Only the text tells them apart: `kg_db`
/// admits the verdict where the text at `name` spells `spelling`, and
/// otherwise counts it toward the flag alone, so nothing it left undecided
/// is reported.
#[derive(Clone, Debug)]
pub struct CopiedName {
    pub diagnostic: Diagnostic,
    /// The identifier's range, which spells the name in a copy.
    pub name: FileRange,
    pub spelling: String,
    /// The pass whose verdict it is: pass 9's, which counts toward the
    /// program's flag where no text spells it; or pass 12's "Undefined
    /// template" at a module's text, which says nothing then
    /// ([`Cx::unresolved_template`]).
    pub pass: TextPass,
}

/// The verdicts at every module's own text of the moved passes before 19
/// — 10, 13, 14, 16, 17 and 18, in that order — which pass 19's walk
/// reads ([`replaced_by`]).
fn earlier_text(
    e: &Elaborated,
    symbols: &crate::symbols::Symbols,
    interner: &Interner,
    truncated: bool,
    exportables: Vec<(kg_base::FileId, Diagnostic)>,
    externs: Vec<Diagnostic>,
) -> Vec<TextVerdict> {
    let at_text =
        |pass: TextPass| move |diagnostic: Diagnostic| TextVerdict { pass, home: diagnostic.range.file, diagnostic, undecided: false };
    let (type_errors, replaced) = type_errors(e);
    let mut text: Vec<TextVerdict> = type_errors.into_iter().map(at_text(TextPass::TypeErrors)).collect();
    let exportable = |(home, diagnostic): (kg_base::FileId, Diagnostic)| TextVerdict {
        pass: TextPass::ExportableClasses,
        home,
        diagnostic,
        undecided: false,
    };
    text.extend(exportables.into_iter().map(exportable));
    text.extend(externs.into_iter().map(at_text(TextPass::ValidateExtern)));
    let post = post_desugar(e, symbols, &replaced, truncated);
    // A `static assert`'s verdict replaced its operand whole, and pass 10's
    // inside it with it (`staticAssert`, `Frontend.hs:396-400`).
    let asserted: Vec<FileRange> = post
        .iter()
        .filter(|(d, _)| d.is(DiagnosticKind::StaticAssertUndecided))
        .map(|(d, _)| d.range)
        .collect();
    text.retain(|v| {
        v.pass != TextPass::TypeErrors
            || !asserted.iter().any(|r| r.file == v.diagnostic.range.file && r.range.contains_range(v.diagnostic.range.range))
    });
    text.extend(post.into_iter().map(|(diagnostic, undecided)| TextVerdict {
        pass: TextPass::PostDesugar,
        home: diagnostic.range.file,
        diagnostic,
        undecided,
    }));
    text.extend(deduce_auto(e, symbols, &replaced, truncated).into_iter().map(|(diagnostic, undecided)| TextVerdict {
        pass: TextPass::DeduceAuto,
        home: diagnostic.range.file,
        diagnostic,
        undecided,
    }));
    text.extend(interpolated_strings(e, interner).into_iter().map(at_text(TextPass::InterpolatedStringPost)));
    text
}

/// What an earlier pass's error at the text replaced: `desugarError` and
/// `errorMsg` put an `ErrorF` where the node they judge was
/// (`Error.hs:147-148`), so pass 19 never sees anything inside one —
/// `1 + (uint32)` is pass 10's verdict alone, `clog2(uint32)` pass 16's
/// (`kg-oracle --errors`, 2026-10-10).
fn replaced_by(earlier: &[TextVerdict]) -> Vec<FileRange> {
    earlier.iter().filter(|v| v.diagnostic.is_error()).map(|v| v.diagnostic.range).collect()
}

/// What [`Cx`]'s walk over every module raises: inside the copies, pass
/// 12's at the text, and the verdicts at each module's own text of the
/// moved passes it judges ([`Cx::piece`]).
struct Walked {
    raised: Vec<(Expansion, Diagnostic)>,
    /// What a verdict at a module's text replaced whole, pass 10's inside
    /// it with it ([`Cx::replacing`]).
    replacing: Vec<FileRange>,
    late: Vec<Diagnostic>,
    /// Pass 12's "Undefined template" at a module's text, placed by the
    /// text that spells the name ([`CopiedName`]).
    late_named: Vec<CopiedName>,
    text: Vec<(Expansion, TextPass, Diagnostic)>,
}

fn walk_modules(
    e: &Elaborated,
    symbols: &crate::symbols::Symbols,
    interner: &Interner,
    leftovers: bool,
    truncated: bool,
    bracket: Bracket<'_>,
    replaced: &[FileRange],
) -> Walked {
    let mut out = Walked { raised: Vec::new(), replacing: Vec::new(), late: Vec::new(), late_named: Vec::new(), text: Vec::new() };
    for m in &e.modules {
        let mut cx = Cx {
            m,
            expansions: &e.expansions,
            modules: &e.modules,
            symbols,
            interner,
            leftovers,
            truncated,
            copies: 0,
            bracket,
            replaced,
            deduced: kg_base::FxHashSet::default(),
            unrolled: kg_base::FxHashSet::default(),
            piece: None,
            raised: Vec::new(),
            replacing: Vec::new(),
            late: Vec::new(),
            late_named: Vec::new(),
            text: Vec::new(),
        };
        cx.walk(m.ir.root, None);
        // Pass 12's verdicts at the text replaced what they judged too —
        // `Foo<auto, false>`'s argument is pass 12's "Unresolved argument"
        // and no `auto` for pass 19 (`type-check.k::38`) — and the walk
        // raised them after the arguments, post-order.
        let late: Vec<FileRange> = cx.late.iter().filter(|d| d.is_error()).map(|d| d.range).collect();
        cx.text.retain(|(_, pass, d)| {
            *pass != TextPass::ValidateProgram
                || !late.iter().any(|r| r.file == d.range.file && r.range.contains_range(d.range.range))
        });
        out.raised.extend(cx.raised);
        out.replacing.extend(cx.replacing);
        out.late.extend(cx.late);
        out.late_named.extend(cx.late_named);
        out.text.extend(cx.text);
    }
    out
}

/// [`diagnostics_at`], its two parts apart.
/// `truncated`: a latency bound — the editor's, never the reference's own
/// `--template-iterations` — stopped the loop short of its fixpoint (see
/// `Cx::truncated`).
pub fn checked(e: &Elaborated, interner: &Interner, leftovers: bool, truncated: bool, bracket: Bracket<'_>) -> Checked {
    let mut late: Vec<Diagnostic> = Vec::new();
    let expansions: &ExpansionInterner = &e.expansions;
    // `getSymbols` over the program as it leaves instantiation — what
    // `withSymbols` hands passes 9 to 19 (`Frontend.hs:40`).
    let symbols = crate::symbols::Symbols::with(e.modules.iter().map(|m| &m.ir), interner, crate::symbols::Config::AllSymbols);
    let externs = validate_extern(e, &symbols);
    let mut exportables = exportable_classes(e, &symbols, interner);
    let exportable_copies = std::mem::take(&mut exportables.copies);
    let mut text = earlier_text(e, &symbols, interner, truncated, exportables.text, externs.text);
    let replaced = replaced_by(&text);
    // A refusal or a conflict at a module's own text is pass 8's verdict
    // there ([`TextPass::TemplateTypeCheck`]); in a copy, the round's.
    let mut instantiation_text: Vec<Diagnostic> = Vec::new();
    let mut raised: Vec<(Stage, Expansion, Diagnostic)> = Vec::new();
    for (src, message) in &e.refused {
        let d = Diagnostic::error(src.head, message.clone());
        if expansions.copy_frames(src.via).is_empty() {
            instantiation_text.push(d);
        } else {
            raised.push((Stage::Instantiation, src.via, d));
        }
    }
    // An error a round's passes wrote into the tree — `reconcile`'s
    // conflicting deduction, an `ErrorF` at the call (`Type.hs:1502-1506`) —
    // is raised by the round like a refusal.
    // Inference's own — `evaluateConstexpr`'s errors — are pass 6's, at a
    // module's own text where the user wrote the node ([`fold_errors`]);
    // here they count in a copy, or on a node a rewrite made
    // (`desugarLambdas`' `decltype(fn)`, `lambdas.k::6`). A conflicting
    // deduction is the round's, which no CST pass raises: anywhere. (Known
    // by its kind, not its text. Today every one is minted `Donated` —
    // `Mint::new_note` — so `written` alone already raises it, and no test
    // can tell the kind test from its absence; it states the rule for an
    // `Own` one, 2026-09-27.)
    // Every error in the final tree is one passes 1–8 made. At a module's
    // own text it is inference's, pass 6's ([`TextPass::InferType`]),
    // admitted in its place by the CST run; and the reference's flag is
    // set by it (`Template.hs:84`, `hasErrorSymbol`).
    let fold = fold_errors(e);
    let higher = higher_order_functions(e);
    let mut in_the_tree = !fold.is_empty() || !higher.is_empty();
    text.extend(higher.into_iter().map(|diagnostic| TextVerdict {
        pass: TextPass::HigherOrderFunctions,
        home: diagnostic.range.file,
        diagnostic,
        undecided: false,
    }));
    text.extend(fold.into_iter().map(|diagnostic| TextVerdict {
        pass: TextPass::InferType,
        home: diagnostic.range.file,
        diagnostic,
        undecided: false,
    }));
    for m in &e.modules {
        for id in errors_in(&m.ir) {
            let node = &m.ir.arena[id];
            let Exp::Error(note) = &node.exp else { continue };
            in_the_tree = true;
            let round = note.is(DiagnosticKind::ConflictingDeduction);
            let written = node.src.written == kg_base::Written::Own && expansions.copy_frames(node.src.via).is_empty();
            if !round && written {
                continue;
            }
            let d = Diagnostic::error(node.src.head, note.message.to_string()).with_kind(note.kind);
            if round && expansions.copy_frames(node.src.via).is_empty() {
                merge(&mut instantiation_text, d);
            } else {
                raised.push((Stage::Instantiation, node.src.via, d));
            }
        }
    }
    // `instantiateTemplates` stops with the flag set once a round raised an
    // error (`Template.hs:55-56`), and no pass after it runs
    // (`Frontend.hs:44-46`): a refusal is the program's last word. Pass 9
    // sets it again for passes 10 onward (`Frontend.hs:86-87`).
    // Pass 9 at each module's text is computed whatever the flag says, as
    // every moved pass's is ([`TextPass`]); in the copies only where no
    // refusal stopped instantiation first.
    let p9 = undefined_symbols(e, &symbols, interner);
    let (undefined_text, mut copied) = (p9.text, p9.copied);
    if raised.is_empty() && instantiation_text.is_empty() {
        raised.extend(p9.raised.into_iter().map(|(via, d)| (Stage::UndefinedSymbols, via, d)));
    }
    let stopped = !raised.is_empty() || !instantiation_text.is_empty();
    let erred = stopped || in_the_tree;
    // The walk's verdicts at a module's own text are computed whatever the
    // flag says — the CST run admits them behind its own per-file
    // checkpoints ([`TextPass`]) — and its copies' only where the flag
    // lets the passes after 9 run at all.
    let walked = walk_modules(e, &symbols, interner, leftovers, truncated, bracket, &replaced);
    if !stopped {
        let after = |(via, d): (Expansion, Diagnostic)| (Stage::AfterNine, via, d);
        // Pass 10 in the copies first. What `errorMsg` replaced
        // (`Error.hs:147-148`) the later checks already pass over: each
        // stands down on an operand whose type is an error
        // ([`Cx::poisoned`]).
        raised.extend(copy_type_errors(e).into_iter().map(after));
        raised.extend(walked.raised.into_iter().map(after));
        late.extend(walked.late);
        copied.extend(walked.late_named);
        raised.extend(exportable_copies.into_iter().map(|d| (Stage::AfterNine, Expansion::ROOT, d)));
        raised.extend(externs.copies.into_iter().map(after));
    }
    let mut merged: Vec<(Stage, Diagnostic)> = Vec::new();
    // Each frame's instance, spelled for `kg check` and the editor
    // (`crate::instance`) — only where a copy's diagnostic has frames.
    let named = if raised.iter().any(|(_, via, _)| *via != Expansion::ROOT) {
        crate::instance::spelled_copies(e, &symbols, interner)
    } else {
        FxHashMap::default()
    };
    for (stage, via, mut d) in raised {
        // Outermost last, the order `expandErrorStack` renders them in; a
        // `static for` iteration's frame is no note (`copy_frames`).
        let mut at = via;
        while let Some(frame) = expansions.frame(at) {
            if !frame.iteration {
                d = d.with_frame(frame.site, Some(crate::instance::chain(expansions, &named, at)));
            }
            at = frame.parent;
        }
        if let Some((_, existing)) = merged.iter_mut().find(|(_, x)| x.range == d.range && x.message == d.message && x.code == d.code) {
            for label in d.labels {
                if !existing.labels.iter().any(|l| l.range == label.range) {
                    existing.labels.push(label);
                }
            }
            continue;
        }
        merged.push((stage, d));
    }
    let mut late_merged: Vec<Diagnostic> = Vec::new();
    for d in late {
        merge(&mut late_merged, d);
    }
    text.extend(undefined_text.into_iter().map(|diagnostic| TextVerdict {
        pass: TextPass::UndefinedSymbols,
        home: diagnostic.range.file,
        diagnostic,
        undecided: false,
    }));
    let replacing = walked.replacing;
    text.retain(|v| v.pass != TextPass::TypeErrors || !inside_any(&replacing, v.diagnostic.range));
    text.extend(walked.text.into_iter().map(|(_, pass, diagnostic)| TextVerdict {
        pass,
        home: diagnostic.range.file,
        diagnostic,
        undecided: false,
    }));
    text.extend(instantiation_text.into_iter().map(|diagnostic| TextVerdict {
        pass: TextPass::TemplateTypeCheck,
        home: diagnostic.range.file,
        diagnostic,
        undecided: false,
    }));
    // Pass 12's verdicts on an instance left standing at a module's own
    // text (`Frontend.hs:409-432`), admitted in its place.
    text.extend(late_merged.into_iter().map(|diagnostic| TextVerdict {
        pass: TextPass::UnresolvedTemplates,
        home: diagnostic.range.file,
        diagnostic,
        undecided: false,
    }));
    Checked { diagnostics: merged, text, copied, erred }
}

fn merge(out: &mut Vec<Diagnostic>, d: Diagnostic) {
    if let Some(existing) = out.iter_mut().find(|e| e.range == d.range && e.message == d.message && e.code == d.code) {
        for label in d.labels {
            if !existing.labels.iter().any(|l| l.range == label.range) {
                existing.labels.push(label);
            }
        }
        return;
    }
    out.push(d);
}

struct Cx<'a> {
    m: &'a Module,
    expansions: &'a ExpansionInterner,
    /// Whether the leftover verdicts speak ([`diagnostics`]).
    leftovers: bool,
    /// Whether a latency bound stopped the loop short of its fixpoint (the
    /// editor's, `PassBudget::INTERACTIVE`): then an instance left standing
    /// may be one the next round makes, and what judges the loop's
    /// leftovers says nothing — the reference has no budget
    /// (`Template.hs:55-64`), so such a verdict is never its own, and a
    /// truncated loop may leave diagnostics out, never put one in.
    truncated: bool,
    /// How many of the walk's enclosing nodes are in a copy: a template
    /// argument substituted into one keeps its own `Src`, unframed, so the
    /// node alone does not say it is in a copy (`auto_in_a_copied_alias`).
    copies: usize,
    /// [`diagnostics_at`].
    bracket: Bracket<'a>,
    /// What an earlier pass's error at the text replaced ([`replaced_by`]):
    /// no `validateProgram` clause judges inside one.
    replaced: &'a [FileRange],
    /// The `auto`s `deduceAuto` rewrote before pass 19 — a variable's own
    /// `auto` or `const auto`, a function's bare `auto` return
    /// (`Type.hs:1600-1608`) — which `check AutoF{}` therefore never sees.
    deduced: kg_base::FxHashSet<ExpId>,
    /// The induction variable of every `static for` `staticControl` kept a
    /// loop for the backend (`UnrolledForF`, `Constexpr.hs:50-84`) met so
    /// far: no fold reads one ([`Cx::never_folds`]).
    unrolled: kg_base::FxHashSet<ExpId>,
    modules: &'a [Module],
    symbols: &'a crate::symbols::Symbols,
    interner: &'a Interner,
    /// The moved pass the site being checked is judged for, where it is at
    /// a module's own text: what [`Cx::raise`] files there goes to
    /// [`Cx::text`], admitted by the CST run in that pass's place, and
    /// seeds nothing.
    piece: Option<TextPass>,
    /// In the order raised, with the frames of the node each is at.
    raised: Vec<(Expansion, Diagnostic)>,
    /// The nodes at a module's text a `validateProgram` verdict replaced
    /// whole — `errorMsg` over the node (`Error.hs:147`), an `ErrorF`
    /// pass 10 left inside it gone with it: an array dimension's
    /// (`checkArraySize`, `TypeCheck.hs:187-196`). Pass 10's verdicts inside
    /// one are dropped ([`checked`]).
    replacing: Vec<FileRange>,
    /// Pass 12's verdicts at a module's own text
    /// ([`TextPass::UnresolvedTemplates`]).
    late: Vec<Diagnostic>,
    /// Pass 12's "Undefined template" at a module's own text, which only
    /// the text can place ([`CopiedName`]).
    late_named: Vec<CopiedName>,
    /// The verdicts at a module's own text of each moved pass this walk
    /// judges ([`Cx::piece`]), in the order raised.
    text: Vec<(Expansion, TextPass, Diagnostic)>,
}

impl Cx<'_> {
    fn ir(&self) -> &ModuleIr {
        &self.m.ir
    }

    fn exp(&self, id: ExpId) -> &Exp {
        &self.m.ir.arena[id].exp
    }

    fn ty(&self, id: ExpId) -> Ty {
        self.m.types.get(id)
    }

    fn items(&self, id: ExpId) -> Vec<ExpId> {
        match self.exp(id) {
            Exp::Seq(items) => items.clone(),
            _ => Vec::new(),
        }
    }

    fn head(&self, id: ExpId) -> FileRange {
        self.ir().arena[id].src.head
    }

    fn in_copy(&self, id: ExpId) -> bool {
        !self.expansions.copy_frames(self.ir().arena[id].src.via).is_empty()
    }

    fn render(&self, t: &Ty) -> String {
        let note = |d: crate::symbols::Decl| {
            let m = self.modules.get(d.module)?;
            m.types.is_set(d.id).then(|| m.types.get(d.id))
        };
        show_unmangled_with(t, Some(self.interner), &|name| self.symbols.noted(name, note))
    }

    fn raise(&mut self, at: ExpId, d: Diagnostic) {
        let via = self.ir().arena[at].src.via;
        if self.piece == Some(TextPass::ValidateProgram)
            && self.replaced.iter().any(|r| r.file == d.range.file && r.range.contains_range(d.range.range))
        {
            return;
        }
        match self.piece {
            Some(pass) => self.text.push((via, pass, d)),
            None => self.raised.push((via, d)),
        }
    }

    /// Whether an earlier pass's error at the text lies inside `id`.
    fn replaced_inside(&self, id: ExpId) -> bool {
        let head = self.head(id);
        self.replaced.iter().any(|r| r.file == head.file && head.range.contains_range(r.range))
    }

    /// [`Cx::raise`] for a verdict whose `errorMsg` replaces its node whole,
    /// an earlier pass's error inside it with it (`Error.hs:147`): raised
    /// whatever was reported inside, and pass 10's there dropped
    /// ([`Cx::replacing`]).
    fn raise_replacing(&mut self, at: ExpId, d: Diagnostic) {
        let via = self.ir().arena[at].src.via;
        self.replacing.push(self.head(at));
        match self.piece {
            Some(pass) => self.text.push((via, pass, d)),
            None => self.raised.push((via, d)),
        }
    }

    /// `check` with what it raises filed as `piece`'s verdicts where the
    /// site is at the module's own text (`text`), and raised as before
    /// elsewhere.
    fn judged_as<R>(&mut self, text: bool, piece: TextPass, check: impl FnOnce(&mut Self) -> R) -> R {
        let outer = std::mem::replace(&mut self.piece, text.then_some(piece));
        let r = check(self);
        self.piece = outer;
        r
    }

    /// A pass-12 verdict: at a module's own text,
    /// [`TextPass::UnresolvedTemplates`]'s; in a copy, raised with its
    /// frames.
    fn pass_12(&mut self, text: bool, at: ExpId, d: Diagnostic) {
        if text {
            self.late.push(d);
        } else {
            self.raise(at, d);
        }
    }

    /// Post-order, with the type of the function a `return` returns from.
    fn walk(&mut self, id: ExpId, function: Option<&Ty>) {
        let mut kids = Vec::new();
        self.exp(id).children(&mut kids);
        // A lambda's `return` is deduced, not converted: no function reaches
        // through one (below).
        let own = match self.exp(id) {
            Exp::Function { .. } => Some(self.ty(id)),
            _ => None,
        };
        // A template's body is checked in its copies, never as written:
        // its types name parameters no argument has bound. A `static if`
        // still standing was not decided, and `staticControl` has not chosen
        // the branch the later passes see (`Constexpr.hs:41-47`).
        // With pass 12's inferential arms, under `leftovers` — once gated
        // because a `static if` the loop left unfolded in a copy of
        // `data.memory.bank.schedule` was one the reference folds (`kg-db`'s
        // rooted-door test, 2026-09-26), on by default since W1.4. Asked
        // regardless where the predicate certainly reads a runtime value,
        // which no round can fold ([`Cx::certainly_runtime`]).
        if let Exp::StaticIf { cond, .. } = *self.exp(id)
            && self.in_copy(id)
            && ((self.leftovers && !self.truncated) || self.certainly_runtime(cond))
        {
            // `unresolved StaticIfF{} = desugarError "static if predicate did
            // not evaluate to a boolean constant"` (`Frontend.hs:406`): a copy
            // is fully substituted, so a `static if` `staticControl` left
            // standing in one has a predicate that is no constant
            // (`static-if.k::26`, over a runtime parameter).
            let d = Diagnostic::error(self.head(id), "static if predicate did not evaluate to a boolean constant")
                .with_kind(DiagnosticKind::StaticIfUndecided);
            self.raise(id, d);
            return;
        }
        if matches!(self.exp(id), Exp::Template { .. } | Exp::StaticIf { .. }) || abbreviated(self.ir(), id) {
            return;
        }
        // `checkReturn` reaches the body's own statements only — `renote
        // (fmap $ renote checkReturn) $ body e` (`TypeCheck.hs:76`) — so the
        // function is passed to its body and through `Seq`s, which
        // `removeNestedSeq` flattens into it (`Desugar.hs:132`; a taken
        // `static if` branch among them, `Constexpr.hs:44-47`), and to
        // nothing else: a `return` one `if`, loop or block down is never
        // converted (`kg-oracle --errors`, 2026-10-10).
        let body = match self.exp(id) {
            Exp::Function { body, .. } => Some(*body),
            _ => None,
        };
        let seq = matches!(self.exp(id), Exp::Seq(_));
        match *self.exp(id) {
            Exp::Variable { ty, .. } => {
                let ty = match *self.exp(ty) {
                    Exp::Const(inner) => inner,
                    _ => ty,
                };
                if matches!(self.exp(ty), Exp::Auto) {
                    self.deduced.insert(ty);
                }
            }
            Exp::Function { ret, .. } if matches!(self.exp(ret), Exp::Auto) => {
                self.deduced.insert(ret);
            }
            Exp::UnrolledFor { var, .. } => {
                self.unrolled.insert(var);
            }
            _ => {}
        }
        let copied = self.in_copy(id);
        self.copies += usize::from(copied);
        for k in kids {
            let reaching = if Some(k) == body { own.as_ref() } else if seq { function } else { None };
            self.walk(k, reaching);
        }
        self.copies -= usize::from(copied);
        // A site at the module's own text: neither it nor anything above it
        // is in a copy. Its moved passes' verdicts are the text's.
        let text = !copied && self.copies == 0;
        match self.exp(id).clone() {
            Exp::Alias { body, .. } if self.copies > 0 || copied => self.auto_in_a_copied_alias(body),
            Exp::Array { dims, .. } if self.copies > 0 || copied => self.check_array_size(dims),
            Exp::ParamInt(width) | Exp::ParamUint(width) if self.copies > 0 || copied => self.check_width(id, width),
            // At the module's text a `static assert` is pass 16's
            // (`TextPass::PostDesugar`), judged there with the rest of the
            // fold's leftovers.
            Exp::StaticAssert(cond) if !text => self.check_static_assert(id, cond),
            Exp::Enum { ty, body, .. } if !text => self.check_enum(ty, body),
            // Inside a copy only: at the module's own text an instance the
            // loop left standing is not yet always one the reference left
            // (`conformance`, 2026-09-26: every arm over the whole tree
            // failed 330 accept assertions).
            Exp::QualifiedIdentifier(i) | Exp::TypeIdentifier(_, i) if matches!(self.exp(i), Exp::TemplateInstance { .. }) => {
                self.unresolved_template(i);
            }
            Exp::FunctionCall { function: callee, args, .. } => {
                self.judged_as(text, TextPass::CheckConversion, |cx| cx.check_call(id, callee, args));
            }
            Exp::Variable { init: Some(init), flags, ty, .. } => self.check_var(text, id, ty, init, flags),
            Exp::Assign { value, .. } if text => {
                self.judged_as(true, TextPass::TypeErrors, |cx| cx.desugared_operation_error(value));
            }
            Exp::Return(Some(value)) => {
                if let Some(t) = function {
                    self.judged_as(text, TextPass::CheckConversion, |cx| cx.check_return(t, value));
                }
            }
            _ => {}
        }
        if text {
            self.judged_as(true, TextPass::ValidateProgram, |cx| cx.validate_program(id));
        } else if let Exp::Variable { init, flags, .. } = self.exp(id).clone() {
            // `checkVarDecl`'s function-type clauses in a copy:
            // `() -> void bar1;` in `Foo<bool>`'s body is "Expected
            // 'const'" at the template's text (difffuzz 2026-10-10 [38]'s
            // shape, `kg-oracle --errors`). The rest of `validateProgram`
            // judges the text only.
            self.var_decl(id, init, flags);
        }
        // `checkShadowDecl $ checkVarDecl x` (`TypeCheck.hs:50`): after the
        // variable's own clauses, and only on what they left a variable.
        if text && let Exp::Variable { scope, name, flags, .. } = self.exp(id).clone() {
            self.judged_as(true, TextPass::CheckShadowDecl, |cx| cx.check_shadow(id, &scope, name, flags));
        }
    }

    /// `validateProgram`'s clauses but the conversions and the shadow check
    /// (`TypeCheck.hs:38-71`), at a node of the module's own text, after
    /// its children: `desugar'` is a `cata` (`Desugar.hs:71-74`). An
    /// earlier pass's error replaced what it judged, and nothing inside it
    /// is judged here ([`Cx::raise`]). `LambdaF`'s clause has no version on
    /// either side; `DecltypeF`'s judges the text only — a copy's is not
    /// asked yet, the silent direction (`decltype(z) z;` in an instantiated
    /// function template is the reference's error and not ours).
    fn validate_program(&mut self, id: ExpId) {
        match self.exp(id).clone() {
            // `check x@(UnaryF BitSizeof _) = x`, and `ByteSizeof`'s.
            Exp::Unary(kg_const::UnOp::BitSizeof | kg_const::UnOp::ByteSizeof, _) => {}
            Exp::Unary(_, x) | Exp::ParamInt(x) | Exp::ParamUint(x) => self.invalid_type(&[x], true),
            Exp::Binary(_, a, b) => self.invalid_type(&[a, b], true),
            Exp::FanOut { count, value } => self.invalid_type(&[count, value], true),
            Exp::Mux { selector, cases } => self.invalid_type(&[selector, cases], true),
            // The parser's: `expression` rejects a result that `isType`
            // (`Parser.hs:823-828`) — a `static assert`'s operand, a
            // statement — ported to this pass by the CST port.
            Exp::StaticAssert(cond) => self.invalid_type(&[cond], false),
            // A declaration among the statements is no expression.
            Exp::Seq(items) if self.ir().arena[id].kind == kg_syntax::SyntaxKind::BLOCK_STMT => {
                let statements: Vec<ExpId> = items.into_iter().filter(|&i| !declares(self.exp(i))).collect();
                self.invalid_type(&statements, false);
            }
            Exp::Array { dims, .. } => self.array_size(dims),
            Exp::IntAttr(_, value) => self.int_attr(value),
            Exp::Enum { ty, body, .. } => {
                self.check_enum(ty, body);
                self.enum_unfolded(ty, body);
            }
            Exp::Variable { init, flags, .. } => self.var_decl(id, init, flags),
            // `check AutoF{} = desugarError "Could not deduce type"`
            // (`TypeCheck.hs:51`): one `deduceAuto` did not rewrite — in an
            // alias, a function type, a `const auto` return, a declaration
            // with no body.
            // Only one the module's text wrote: a rewrite's copy of an
            // abbreviated template's signature is no `auto` the reference
            // has standing (`extern-class.k::15`'s `void Do(auto x);`;
            // `library/numeric/int/operator.k`'s `increment`, passed as a
            // value from a program whose instantiation stopped).
            Exp::Auto if !self.deduced.contains(&id) && self.written(id) => {
                self.raise(id, Diagnostic::error(self.head(id), "Could not deduce type"));
            }
            // `check x@DecltypeF{} = fmap (errorMsg "Couldn't deduce type of
            // the expression") x` (`TypeCheck.hs:53`): a `decltype` the fold
            // never replaced — its operand never typed (`eval _ | not $
            // typeResolved n = e`, `Constexpr.hs:304-305`) — at the operand.
            // `decltype(x) x;` reads the variable it declares (difffuzz
            // 2026-10-10 [36]). Only one the text wrote: `desugarLambdas`'
            // `decltype(make_captureN())` stands where no base library
            // declares `make_captureN`, which is pass 9's to report. Not
            // under a latency bound: a loop stopped short may not have typed
            // an operand the next round types — `decltype(p.first)` over an
            // instance a later round makes (`multi_word.k`, `edit-identity`
            // 2026-10-10) — so the gutter may miss it, never gain it.
            Exp::Decltype { expr, .. } if self.written(id) && !self.truncated => {
                self.raise(expr, Diagnostic::error(self.head(expr), "Couldn't deduce type of the expression"));
            }
            Exp::Assign { lvalue, .. } => self.const_lvalue(lvalue),
            Exp::Function { .. } => self.reset(id),
            Exp::DefaultInitialization(value) => self.designated_default(value),
            Exp::InitializerList(items) => self.overriding_initializers(items),
            _ => {}
        }
    }

    /// `checkInvalidType = fmap errorIfType` (`TypeCheck.hs:180-185`): an
    /// operand that `isType` (`Parser/Syntax.hs:676-699`) is "Invalid use
    /// of a type as an expression" at itself; a `Seq` operand — a mux's
    /// cases — is looked into (`errorIfType a@Seq{} = renote
    /// checkInvalidType a`) where `into` says so.
    fn invalid_type(&mut self, operands: &[ExpId], into: bool) {
        for &a in operands {
            match self.exp(a).clone() {
                Exp::Seq(items) if into => self.invalid_type(&items, true),
                _ if kg_hir::sort::is_type(&self.ir().arena, a) => {
                    self.raise(a, Diagnostic::error(self.head(a), "Invalid use of a type as an expression"));
                }
                _ => {}
            }
        }
    }

    /// `checkArraySize` (`TypeCheck.hs:187-196`) at the module's text: a
    /// dimension folded to anything but a positive integer, at the `[` (or
    /// a memory's expression, [`Bracket`]). One the loop left unfolded is
    /// reported only where no fold can give it — a runtime name
    /// ([`Cx::certainly_runtime`]) — since elsewhere it may be this
    /// engine's gap, as for a copy ([`Cx::check_array_size`]). A type as a
    /// dimension is the parser's error instead (`Parser.hs:823-828`).
    fn array_size(&mut self, dims: ExpId) {
        for d in self.items(dims) {
            let head = self.head(d);
            if kg_hir::sort::is_type(&self.ir().arena, d) {
                self.raise(d, Diagnostic::error(head, "Invalid use of a type as an expression"));
                continue;
            }
            // `errorIfNotPositive a@(EnumValue _ (IntLiteral _ n))`
            // (`TypeCheck.hs:193-195`): an enum constant by its value.
            let value = match self.exp(d) {
                Exp::EnumValue { value, .. } => *value,
                _ => d,
            };
            let positive = match kg_hir::constexpr::value(&self.ir().arena, value, &kg_hir::constexpr::Closed) {
                Some(v) => v.as_int().is_some_and(|n| n.is_positive()),
                None => !self.certainly_runtime(d) && !self.never_folds(d, 4) && !self.literals_only(d),
            };
            if !positive {
                let at = (self.bracket)(head).unwrap_or(head);
                self.raise(d, Diagnostic::error(at, "Array dimension must be a positive integer"));
                self.replacing.push(head);
            }
        }
    }

    /// Whether `e` is operators over literals alone, which the fold has
    /// had all it could have of: one it left standing has no arm —
    /// `evalBinaryInt` none for `||` over integers (`Constexpr.hs:
    /// 217-242`) — and never folds, as `post_desugar`'s `settled` reads it.
    /// `uint32[3][i||(1)][2] a;` in a `static for` over `i` unrolls to `0 ||
    /// (1)` and is the dimension's error (difffuzz 2026-10-10, the re-run's
    /// `static-for.k::1` mutant; `kg-oracle --errors`).
    fn literals_only(&self, e: ExpId) -> bool {
        match self.exp(e) {
            Exp::IntLiteral { .. } | Exp::BoolLiteral(_) | Exp::FloatLiteral(_) => true,
            Exp::Binary(_, a, b) => self.literals_only(*a) && self.literals_only(*b),
            Exp::Unary(kg_const::UnOp::Neg | kg_const::UnOp::Invert | kg_const::UnOp::Not, a) => self.literals_only(*a),
            _ => false,
        }
    }

    /// Whether the node is one the module's own text wrote.
    fn written(&self, id: ExpId) -> bool {
        let src = self.ir().arena[id].src;
        src.written == kg_base::Written::Own && src.head.file == self.ir().file
    }

    /// Whether `e` is computed from the induction variable of a `static for`
    /// kept a loop ([`Cx::unrolled`]), through `const` locals at most `depth`
    /// deep, by literals and operators that cannot fold a name away — no
    /// mux, no absorbing operator (`Constexpr.hs:228-240,311-314`), no call,
    /// no size: no round gives it a value. `const auto a = j + 1; uint32[a]
    /// b;` in such a loop is the dimension's error at its `[`
    /// (`static-for.k::10`, `kg-oracle --errors`).
    fn never_folds(&self, e: ExpId, depth: u32) -> bool {
        fn shape(cx: &Cx<'_>, e: ExpId, depth: u32, unfolded: &mut bool) -> bool {
            match cx.exp(e) {
                Exp::IntLiteral { .. } | Exp::BoolLiteral(_) | Exp::FloatLiteral(_) => true,
                Exp::Binary(op, a, b) => {
                    !may_absorb(&cx.ir().arena, *op, *a, *b) && shape(cx, *a, depth, unfolded) && shape(cx, *b, depth, unfolded)
                }
                Exp::Unary(kg_const::UnOp::Neg | kg_const::UnOp::Invert | kg_const::UnOp::Not, a) => shape(cx, *a, depth, unfolded),
                Exp::NamedValue(inner) => {
                    let arena = &cx.ir().arena;
                    let Exp::QualifiedIdentifier(name) = &arena[*inner].exp else { return false };
                    let Exp::ScopedName { scope, .. } = &arena[*name].exp else { return false };
                    let path = kg_hir::sort::spine_qualname(arena, *name);
                    if path.len() != 1 {
                        return false;
                    }
                    let Some(d) = cx.symbols.in_scope(scope.segments(), &path) else { return true };
                    if !std::ptr::eq(&cx.modules[d.module], cx.m) {
                        return true;
                    }
                    if cx.unrolled.contains(&d.id) {
                        *unfolded = true;
                    } else if let Exp::Variable { ty, init: Some(init), .. } = &arena[d.id].exp
                        && matches!(arena[*ty].exp, Exp::Const(_))
                    {
                        *unfolded |= cx.never_folds(*init, depth - 1);
                    }
                    true
                }
                _ => false,
            }
        }
        if depth == 0 {
            return false;
        }
        let mut unfolded = false;
        shape(self, e, depth, &mut unfolded) && unfolded
    }

    /// `check x@IntAttrF{} = fmap errorIfNotInteger x` (`TypeCheck.hs:48,
    /// 197-198`): an integer attribute's argument must fold to a
    /// non-negative `IntLiteral`. A literal of another kind, a negative one
    /// and a runtime name are refused; one the loop left unfolded otherwise
    /// is passed over. A type there is the parser's error.
    fn int_attr(&mut self, value: ExpId) {
        let head = self.head(value);
        if kg_hir::sort::is_type(&self.ir().arena, value) {
            self.raise(value, Diagnostic::error(head, "Invalid use of a type as an expression"));
            return;
        }
        let refused = match self.exp(value) {
            Exp::IntLiteral { value: n, .. } => n.is_negative(),
            Exp::EnumValue { .. } => false,
            Exp::BoolLiteral(_) | Exp::FloatLiteral(_) | Exp::StringLiteral(_) | Exp::InterpolatedString(_) => true,
            _ => self.certainly_runtime(value),
        };
        if refused {
            self.raise(value, Diagnostic::error(head, "Expected a constant expression that evaluates to non-negative integer"));
        }
    }

    /// `errorIfOutOfRange'`'s last clause (`TypeCheck.hs:214`): a constant
    /// left unfolded "doesn't evaluate to an integer constant" — claimed
    /// only where every one left is written over literals and this enum's
    /// own constants (`G = F + 1, F = G + 1`, `enum.k`): nothing outside the
    /// enum can be what stopped the fold. One naming anything else
    /// silences the set — its failure may be this engine's. A constant the
    /// fold *did* finish, to a literal that is no integer — `E1 = 0 != 2`,
    /// `true`, `1.5`, a `const bool` — is no gap of the engine's and is
    /// claimed alone (difffuzz 2026-10-10 [37], `kg-oracle --errors`, at
    /// the value).
    fn enum_unfolded(&mut self, ty: ExpId, body: ExpId) {
        let base = self.ty(ty).the_type();
        if !base.resolved() || !base.is_int() {
            return;
        }
        let mut names = Vec::new();
        let mut unfolded = Vec::new();
        for c in self.items(body) {
            let Exp::EnumConstant { name, value, .. } = self.exp(c).clone() else { continue };
            names.extend(kg_hir::rewrite::ident_sym(&self.ir().arena, name));
            let Some(mut v) = value else { continue };
            while let Exp::EnumValue { value, .. } = self.exp(v) {
                v = *value;
            }
            match self.exp(v) {
                Exp::IntLiteral { .. } => {}
                Exp::BoolLiteral(_) | Exp::FloatLiteral(_) | Exp::StringLiteral(_) | Exp::InterpolatedString(_) => {
                    self.raise(v, Diagnostic::error(self.head(v), "Enum constant value doesn't evaluate to an integer constant"));
                }
                // An earlier pass's error inside — pass 10's `!F`, `1 + true`
                // — is an `ErrorF` that never folds, and `errorMsg` over the
                // value replaces it with this verdict (`Error.hs:147`;
                // difffuzz 2026-10-10 [19], `kg-oracle --errors`).
                _ if self.replaced_inside(v) => {
                    self.raise_replacing(v, Diagnostic::error(self.head(v), "Enum constant value doesn't evaluate to an integer constant"));
                }
                _ => unfolded.push(v),
            }
        }
        fn closed(cx: &Cx<'_>, e: ExpId, names: &[kg_base::Symbol]) -> bool {
            match cx.exp(e) {
                Exp::IntLiteral { .. } => true,
                Exp::Binary(_, a, b) => closed(cx, *a, names) && closed(cx, *b, names),
                Exp::Unary(kg_const::UnOp::Neg | kg_const::UnOp::Invert | kg_const::UnOp::Not, a) => closed(cx, *a, names),
                Exp::NamedValue(x) | Exp::QualifiedIdentifier(x) => closed(cx, *x, names),
                Exp::ScopedName { prefix: None, ident, .. } => {
                    kg_hir::rewrite::ident_sym(&cx.ir().arena, *ident).is_some_and(|s| names.contains(&s))
                }
                _ => false,
            }
        }
        if unfolded.is_empty() || !unfolded.iter().all(|&v| closed(self, v, &names)) {
            return;
        }
        for v in unfolded {
            self.raise(v, Diagnostic::error(self.head(v), "Enum constant value doesn't evaluate to an integer constant"));
        }
    }

    /// `checkVarDecl`'s clauses but the conversions (`TypeCheck.hs:123-139`):
    /// a resolved, mutable global is "Global state is not allowed", first
    /// of all; and a variable of a function type (`tIsFunction`, no closure)
    /// must be initialized if `const`, and `const` unless a class's —
    /// an initializer that is no subtype being the conversion's error
    /// instead. Each at the variable, from its first token.
    fn var_decl(&mut self, var: ExpId, init: Option<ExpId>, flags: DeclareFlag) {
        let t = self.ty(var);
        if t.resolved() && !t.is_const() && flags == DeclareFlag::DeclareGlobal {
            self.raise(var, Diagnostic::error(self.head(var), "Global state is not allowed"));
            return;
        }
        if !t.resolved() || !matches!(t.un_const(), Ty::Function { .. }) {
            return;
        }
        let mutable = "Expected 'const'. Mutable variables of a function type are not allowed";
        let message = match init {
            None if t.is_const() => "A constant of a function type must be initialized",
            Some(init) if !is_subtype(&self.ty(init).un_const(), &t) => return,
            _ if t.is_const() || flags == DeclareFlag::DeclareClass => return,
            _ => mutable,
        };
        self.raise(var, Diagnostic::error(self.head(var), message));
    }

    /// `checkConstLValue` (`TypeCheck.hs:54,105-108`): an assignment to an
    /// lvalue typed `const` — every `AssignF`, compound and `++` included.
    /// Spelled as the CST pass spelled it: the base name where the base is
    /// declared `const` (the reference's parser rejects that one first,
    /// `namedLValue`, `Parser.hs:794-801`), else the whole lvalue, as
    /// written; a spelling that is not the written text's length is not
    /// the text, and says nothing.
    fn const_lvalue(&mut self, lvalue: ExpId) {
        if !self.ty(lvalue).is_const() {
            return;
        }
        let mut base = lvalue;
        while let Exp::MemberAccess { base: b, .. } | Exp::ArrayAccess { base: b, .. } | Exp::ArrayAccessLValue { base: b, .. } =
            self.exp(base)
        {
            base = *b;
        }
        let spelled_at = if self.ty(base).is_const() { base } else { lvalue };
        let Some(spelled) = self.spell(spelled_at) else { return };
        if spelled.len() != usize::from(self.head(spelled_at).range.len()) {
            return;
        }
        let d = Diagnostic::error(self.head(lvalue), format!("`{spelled}`: illegal use of this constant as an lvalue"));
        self.raise(lvalue, d);
    }

    /// An lvalue as written: names, members, indices of names and literals.
    fn spell(&self, id: ExpId) -> Option<String> {
        Some(match self.exp(id) {
            Exp::Identifier(s) => self.interner.resolve(*s).to_string(),
            Exp::NamedValue(x) | Exp::QualifiedIdentifier(x) => self.spell(*x)?,
            Exp::ScopedName { prefix: None, ident, .. } => self.spell(*ident)?,
            Exp::ScopedName { prefix: Some(p), ident, .. } => format!("{}::{}", self.spell(*p)?, self.spell(*ident)?),
            Exp::MemberAccess { base, member } => format!("{}.{}", self.spell(*base)?, self.spell(*member)?),
            Exp::ArrayAccess { base, index } | Exp::ArrayAccessLValue { base, index } => {
                format!("{}[{}]", self.spell(*base)?, self.spell(*index)?)
            }
            Exp::IntLiteral { value, .. } => value.to_string(),
            _ => return None,
        })
    }

    /// `checkFunctionType`'s `[[reset]]` arm (`TypeCheck.hs:73-82`): such a
    /// function takes no parameters and returns `void` — the deduced type
    /// of an `auto` return included (`tReturn`) — at the function, from its
    /// first token (its attributes).
    fn reset(&mut self, function: ExpId) {
        let t = self.ty(function);
        if !t.function_attrs().is_some_and(|a| a.contains(&TAttr::Flag(Flag::Reset))) {
            return;
        }
        let message = if t.function_params().is_some_and(|p| !p.is_empty()) {
            "Function with [[reset]] attribute can't have parameters"
        } else {
            let r = t.return_ty();
            if !r.resolved() || r.is_void() {
                return;
            }
            "Function with [[reset]] must have void return type"
        };
        self.raise(function, Diagnostic::error(self.head(function), message));
    }

    /// `checkDesignatedInitializer` (`TypeCheck.hs:56,69-71`): a `default`
    /// is an initializer list of designators — `tIsDesignatedInitializer`,
    /// an empty list included (`Type.hs:682-684`) — or "Expected designated
    /// initializer" at it.
    fn designated_default(&mut self, value: ExpId) {
        let designated = match self.exp(value) {
            Exp::InitializerList(items) => self.items(*items).iter().all(|&i| matches!(self.exp(i), Exp::Designator { .. })),
            _ => false,
        };
        if !designated {
            self.raise(value, Diagnostic::error(self.head(value), "Expected designated initializer"));
        }
    }

    /// `overridingInitializerWarning` (`TypeCheck.hs:57-65`): a designator
    /// naming a field an earlier one in the same list named warns at
    /// itself, noted at the nearest earlier one; each list alone.
    fn overriding_initializers(&mut self, items: ExpId) {
        let list = self.items(items);
        let field = |cx: &Self, i: ExpId| match cx.exp(i) {
            Exp::Designator { name, .. } => kg_hir::rewrite::ident_sym(&cx.ir().arena, *name),
            _ => None,
        };
        for (k, &x) in list.iter().enumerate() {
            let Some(name) = field(self, x) else { continue };
            let Some(&y) = list[..k].iter().rev().find(|&&y| field(self, y) == Some(name)) else { continue };
            let message = format!("Initializer overrides prior initialization of `{}`", self.interner.resolve(name));
            let d = Diagnostic::warning(self.head(x), kg_diag::WarningKind::InitializerOverrides, message)
                .with_label(self.head(y), "while compiling");
            self.raise(x, d);
        }
    }

    /// `checkShadowDecl` (`TypeCheck.hs:110-121`): a variable with a scope,
    /// not a member, whose name `lookupInScope (init declScopeName)` finds —
    /// from the scope enclosing its own outward, so never a sibling — as a
    /// variable that has a scope itself or as a parameter, warns at its name
    /// with a note at what it shadows (`noteContext x declName`). The map is
    /// the program's at pass 19, `moveFunctions` and `desugarLambdas` done
    /// (`Template.hs:76,78`): a function or lambda body hoisted out of
    /// another reaches only its own scopes and its parent's qualifier's
    /// (`Function.hs:47-50`) — what the deleted CST pass had to
    /// reconstruct from the text. `checkVarDecl` goes first, and a
    /// variable it replaced with an error is no `VariableF` for this clause.
    fn check_shadow(&mut self, var: ExpId, scope: &kg_base::SymbolPath, name: ExpId, flags: DeclareFlag) {
        if scope.is_empty() || matches!(flags, DeclareFlag::DeclareMember | DeclareFlag::DeclareClass) {
            return;
        }
        let at = self.head(var);
        let replaced = |d: &Diagnostic| d.is_error() && d.range == at;
        if self.raised.iter().any(|(_, d)| replaced(d)) || self.text.iter().any(|(_, _, d)| replaced(d)) {
            return;
        }
        let Some(symbol) = kg_hir::rewrite::ident_sym(&self.ir().arena, name) else { return };
        let segments = scope.segments();
        let Some(found) = self.symbols.in_scope(&segments[..segments.len() - 1], &[symbol]) else { return };
        let node = &self.modules[found.module].ir.arena[found.id];
        let shadowable = match &node.exp {
            Exp::Variable { scope, .. } => !scope.is_empty(),
            Exp::FuncParam { .. } => true,
            _ => false,
        };
        if !shadowable {
            return;
        }
        let d = kg_resolve::shadow_warning(self.head(name), &self.interner.resolve(symbol), node.src.head);
        self.raise(name, d);
    }

    /// `staticAssert` (`Frontend.hs:393-398`, pass 16), over the condition
    /// `evaluateConstexpr` folded in place: `false` fails at the statement,
    /// which `desugarError` replaces; a constant of another kind is the
    /// non-boolean error at the expression (`errorMsg … a`). An unfolded
    /// condition is left alone, as the CST pass leaves it — its verdict
    /// needs what the reference knows and this tree may not.
    /// `check AutoF{} = desugarError "Could not deduce type"` (`TypeCheck.hs:
    /// 51`) over an alias in a copy: a function type written as a template's
    /// argument, `Foo<(uint32) -> auto>`, substituted into the class's
    /// `using callback_t = T;`. `deduceAuto` rewrites only a variable's and a
    /// function's own `auto` (`Type.hs:1600-1608`), so this one survives to
    /// pass 19, and is reported where the argument was written — the copy
    /// keeps the argument's note (`callbacks.k::68,69`; `kg-oracle --errors`
    /// 2026-09-27, one error, at the `auto`). The module's own aliases are
    /// the CST pass's (`kg_db`'s `validate_program`); the reference's other
    /// sites for such an `auto` — a copied field, `T x;` — are not claimed:
    /// there a variable's own verdict can replace the node first
    /// (`Expected 'const'…` alone, probed the same day).
    fn auto_in_a_copied_alias(&mut self, ty: ExpId) {
        fn autos(ir: &ModuleIr, id: ExpId, in_function_type: bool, out: &mut Vec<ExpId>) {
            match &ir.arena[id].exp {
                Exp::Auto if in_function_type => out.push(id),
                Exp::FunctionType { params, ret, .. } => {
                    autos(ir, *params, true, out);
                    autos(ir, *ret, true, out);
                }
                Exp::FunctionTypeParam { ty, .. } => autos(ir, *ty, in_function_type, out),
                Exp::Const(inner) | Exp::Array { elem: inner, .. } => autos(ir, *inner, in_function_type, out),
                Exp::Seq(items) => items.iter().for_each(|&i| autos(ir, i, in_function_type, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        autos(self.ir(), ty, false, &mut found);
        for auto in found {
            self.raise(auto, Diagnostic::error(self.head(auto), "Could not deduce type"));
        }
    }

    /// Whether `e` reads a value no fold can give, and nothing in it could
    /// fold that read away: a function's parameter or a variable not
    /// declared `const` — `evaluateConstexpr` folds a name only through a
    /// `const` variable's initializer or an enum constant (`Constexpr.hs:
    /// 286-292`) — under literals and operators alone, none of them an
    /// absorbing one (`evalBinaryInt`/`evalBinaryBool`, `:228-240`) or a
    /// mux (`:311-314`). The IR's reading of `kg_db`'s
    /// `facts::certainly_runtime`, for a copy the CST pass cannot see.
    fn certainly_runtime(&self, e: ExpId) -> bool {
        certainly_runtime(self.m, self.modules, self.symbols, e)
    }

    /// `checkArraySize` (`TypeCheck.hs:187-196`) in a copy: a dimension the
    /// engine folded to anything but a positive integer — `bool[I - 3]` in
    /// `Do<3>` (`template.k::30`), `T1[N]` with `N` a `false`
    /// (`nested-templates.k::9`) — is "Array dimension must be a positive
    /// integer" at the `[` of the template's text, framed by the
    /// instantiations (`kg-oracle --errors`, 2026-09-27). One the engine
    /// left unfolded is passed over: it may be this engine's gap and not
    /// the reference's (`constexpr.k::121`'s imported enum constant, which
    /// the walk declined on). The module's own text is the CST pass's.
    fn check_array_size(&mut self, dims: ExpId) {
        for d in self.items(dims) {
            let Some(v) = kg_hir::constexpr::value(&self.ir().arena, d, &kg_hir::constexpr::Closed) else { continue };
            if v.as_int().is_some_and(|n| n.is_positive()) {
                continue;
            }
            let head = self.head(d);
            let at = (self.bracket)(head).unwrap_or(head);
            self.raise(d, Diagnostic::error(at, "Array dimension must be a positive integer"));
        }
    }

    /// Pass 16's `unresolved ParamIntF{}` and `ParamUintF{}`
    /// (`Frontend.hs:404-405`) in a copy: a width the engine folded to a
    /// literal that is no positive integer — `uint<N>` in `c1<0>`, an
    /// imported template's copy (difffuzz 2026-10-10 [43]) — stays an
    /// `int<…>`, which `evaluateConstexpr` replaces only for a natural
    /// (`Constexpr.hs:302-303`), and is "uint width did not evaluate to a
    /// positive integer constant" at the type in the template's text, framed
    /// by the instantiations (`kg-oracle --errors`). One the engine left
    /// unfolded is passed over, as [`Cx::check_array_size`] passes a
    /// dimension over; the module's own text is `post_desugar`'s.
    fn check_width(&mut self, ty: ExpId, width: ExpId) {
        let refused = match self.exp(width) {
            Exp::IntLiteral { value, .. } => !value.is_positive(),
            Exp::BoolLiteral(_) | Exp::FloatLiteral(_) | Exp::StringLiteral(_) => true,
            _ => false,
        };
        if !refused {
            return;
        }
        let kind = if matches!(self.exp(ty), Exp::ParamInt(_)) { "int" } else { "uint" };
        let message = format!("{kind} width did not evaluate to a positive integer constant");
        self.raise(ty, Diagnostic::error(self.head(ty), message).with_kind(DiagnosticKind::WidthUndecided));
    }

    fn check_static_assert(&mut self, statement: ExpId, cond: ExpId) {
        match self.exp(cond) {
            Exp::BoolLiteral(false) => {
                self.raise(statement, Diagnostic::error(self.head(statement), "static assertion failed"));
            }
            Exp::BoolLiteral(true) => {}
            Exp::IntLiteral { .. } | Exp::FloatLiteral(_) | Exp::StringLiteral(_) => {
                let d = Diagnostic::error(self.head(cond), "static assertion expression did not evaluate to a boolean constant");
                self.raise(cond, d);
            }
            _ => {}
        }
    }

    /// Pass 12, `unresolvedTemplates` (`Frontend.hs:409-432`): an instance
    /// still standing after instantiation is one the loop could not make.
    /// Its template undefined; too few arguments or too many; each argument
    /// `templateArgResolved` refuses; else "Failed to instantiate template"
    /// — the counts and the last with the template's declaration as its
    /// context (`errorMsgWithContext (note $ unfix t)`, `:419`). An argument
    /// list that already holds an error says nothing more (`diagnostics
    /// args`).
    fn unresolved_template(&mut self, instance: ExpId) {
        let Exp::TemplateInstance { base, args } = self.exp(instance).clone() else { return };
        let arena = &self.m.ir.arena;
        let name = kg_hir::rewrite::get_name(arena, base).map(|s| self.interner.resolve(s).to_string()).unwrap_or_default();
        let at = self.head(instance);
        let found = match &arena[base].exp {
            Exp::ScopedName { scope, .. } => {
                let path = kg_hir::sort::spine_qualname(arena, base);
                self.symbols.in_scope(scope.segments(), &path)
            }
            _ => return,
        };
        let template = found.and_then(|d| match &self.modules[d.module].ir.arena[d.id].exp {
            Exp::Template { params, .. } => Some((d, params.clone())),
            _ => None,
        });
        // A template declared without a body — a member template of an
        // `extern` class, `template <typename T> void Do(T x);` — is one the
        // loop does not yet copy (`extern-class.k::36-40`); its leftovers
        // say nothing but `GT`, anywhere.
        let bodiless = template.as_ref().is_some_and(|(d, _)| {
            let a = &self.modules[d.module].ir.arena;
            matches!(&a[d.id].exp, Exp::Template { decl, .. } if matches!(a[*decl].exp, Exp::FunctionDecl { .. }))
        });
        // At the module's own text every arm speaks, as
        // [`TextPass::UnresolvedTemplates`]: pass 9 sets the reference's
        // flag before pass 12 runs
        // (`Frontend.hs:86-87`) — asked unconditionally, the arms spoke
        // where the reference had already stopped (`type-check.k::19` and
        // 22 more, 2026-09-26), so the CST run admits them only past its
        // own pass-9 checkpoint (sprint W1.4). An over-long list is never
        // decided anywhere (`template-instance-identifiers.k::7`'s
        // `A<bool>::A2_t<2, 3>`, which the CST pass cannot name).
        //
        // Inside a copy, the arms that judge what the loop failed to make —
        // too few arguments, an unresolved one, "Failed to instantiate" —
        // are `leftovers`': once they invented errors in
        // `test/library/data/bitarray/banked.k`'s deep copies (`static_cast`
        // in `data.counter` in `sync.lock`; `kg-db`'s rooted-door test,
        // 2026-09-26), and since 2026-09-27 asking them changes no verdict
        // of the gates (sprint W1.4), so they are on; `KG_LEFTOVERS=0` is
        // the reading to fall back to. One is asked regardless: an
        // instance no round can make ([`Cx::never_instantiated`]).
        let text = self.copies == 0 && !self.in_copy(instance);
        let all = text
            || self.leftovers
            || template.as_ref().is_some_and(|(d, params)| self.never_instantiated(*d, params, args));
        let judged = !bodiless && all;
        let Some((decl, params)) = template else {
            // A loop stopped short of its fixpoint (the editor's first
            // answer) may not have made the instance a name is looked up
            // through yet — `fj.fork<0>(…)` on a `ForkJoin<…>` the next
            // round makes (`test/library/control/async.k`) — so no name is
            // undefined there: the gutter may miss the verdict, never gain
            // it (`edit-identity`'s editor row, 2026-10-10).
            if self.truncated {
                return;
            }
            // `unresolvedTemplateErrors e _ = errorMsg ("Undefined template
            // `" ++ getName e ++ "`") e` (`Frontend.hs:431`): pass 9 judges
            // an instance's arguments only (`:96-97`), so a name no lookup
            // finds is this pass's — `template Go<bool>()` over a `Go` a dead
            // `static if` took (`dependent-template.k::8`), `c.c1s.Foo<…>`
            // on an array (`member-function-template.k::20`) — at the name
            // (`kg-oracle --errors`). At the text, only an instance the text
            // wrote: one `desugarLambdas` made of `Closure` is undefined only
            // without the base library, where `make_captureN` is pass 9's
            // first and the reference never reaches pass 12 — an error no
            // text here places (`CopiedName`), which closes no run. Neither
            // `Own` nor `Donated` tells a written name from a minted one —
            // the lowering donates a method template's name its
            // `NAME_REF` — and the text does: the verdict is admitted where
            // the identifier's range spells the name.
            let ident = match &arena[base].exp {
                Exp::ScopedName { ident, .. } => Some(*ident),
                _ => None,
            };
            match ident {
                Some(i) if text => {
                    let diagnostic = Diagnostic::error(self.head(i), format!("Undefined template `{name}`"));
                    let named = CopiedName { diagnostic, name: self.head(i), spelling: name, pass: TextPass::UnresolvedTemplates };
                    self.late_named.push(named);
                }
                _ => self.pass_12(text, instance, Diagnostic::error(at, format!("Undefined template `{name}`"))),
            }
            return;
        };
        let items_len = self.items(args).len();
        if !judged && items_len <= params.len() {
            return;
        }
        // Too few arguments or an unresolved one may be the next round's
        // to settle; too many never are, nor an instance no round makes.
        if self.truncated && items_len <= params.len() && !self.never_instantiated(decl, &params, args) {
            return;
        }
        let items = self.items(args);
        if items.iter().any(|&a| has_error(&self.m.ir, a)) {
            return;
        }
        // `note $ unfix t`: the template's declaration, not its header —
        // `struct One` (`kg-oracle --errors`, 2026-09-26).
        let home = &self.modules[decl.module].ir.arena;
        let context = match &home[decl.id].exp {
            Exp::Template { decl: inner, .. } => home[*inner].src,
            _ => home[decl.id].src,
        };
        // `note $ unfix t` is the declaration's whole note: in a copy, its
        // instantiation sites follow its text (`SrcStack`, `Error.hs:
        // 185-189`) — `A<bool>::A2_t<2, 3>` at the module's text is noted
        // at `A2_t` and then at `A<bool>` (`template-instance-
        // identifiers.k::7`, `kg-oracle --errors`). In a copy the
        // verdict's own frames follow; how the two stacks interleave there
        // is unmeasured, and the declaration's text alone is given.
        let mut frames = Vec::new();
        let mut via = if text { context.via } else { Expansion::ROOT };
        while let Some(frame) = self.expansions.frame(via) {
            if !frame.iteration {
                frames.push(frame.site);
            }
            via = frame.parent;
        }
        let with_context = |d: Diagnostic| {
            let d = d.with_label(context.head, "while compiling");
            frames.iter().fold(d, |d, &site| d.with_frame(site, None))
        };
        match items.len().cmp(&params.len()) {
            std::cmp::Ordering::Less => {
                let d = with_context(
                    Diagnostic::error(at, format!("Could not deduce all arguments for template `{name}`"))
                        .with_kind(DiagnosticKind::CouldNotDeduceAll),
                );
                self.pass_12(text, instance, d);
            }
            std::cmp::Ordering::Greater => {
                let d = with_context(Diagnostic::error(at, format!("Too many arguments specified for template `{name}`")));
                self.pass_12(text, instance, d);
            }
            std::cmp::Ordering::Equal => {
                let mut any = false;
                for (&p, &a) in params.iter().zip(&items) {
                    if self.arg_resolved(a) {
                        continue;
                    }
                    let param = match &self.modules[decl.module].ir.arena[p].exp {
                        Exp::TemplateParam { name, .. } => self.interner.resolve(*name).to_string(),
                        _ => String::new(),
                    };
                    let message = format!("Unresolved argument for the parameter {param} of template {name}");
                    let d = Diagnostic::error(self.head(a), message);
                    self.pass_12(text, a, d);
                    any = true;
                }
                if !any {
                    let d = with_context(Diagnostic::error(at, "Failed to instantiate template"));
                    self.pass_12(text, instance, d);
                }
            }
        }
    }

    /// Whether the instance `args` of the template declared at `decl` is
    /// one no round makes, and so pass 12's "Failed to instantiate
    /// template": as many arguments as parameters, each one
    /// `templateArgResolved` accepts, and one a template-template
    /// parameter's that is a type. `templateArgsResolved` asks `tIsTemplate`
    /// of that argument (`TypeCheck.hs:323-329`), so `templateResolved` never
    /// holds and the instance is never made (`resolveTemplateInstance`,
    /// `:340-345`) — while `unresolveArgError`'s `templateArgResolved`
    /// passes a type (`Frontend.hs:423-426`), which leaves the last arm
    /// (`template-template-parameter.k::36,37`: `FnFn<uint32, T, Do>` with
    /// `T` a type, in `Go`'s copy). A type argument is final: no fold makes
    /// a type a template.
    fn never_instantiated(&self, decl: crate::symbols::Decl, params: &[ExpId], args: ExpId) -> bool {
        let items = self.items(args);
        if items.len() != params.len() || !items.iter().all(|&a| self.arg_resolved(a)) {
            return false;
        }
        let home = &self.modules[decl.module].ir.arena;
        params.iter().zip(&items).any(|(&p, &a)| {
            matches!(&home[p].exp, Exp::TemplateParam { kind, .. } if matches!(home[*kind].exp, Exp::Template { .. }))
                && matches!(self.ty(a), Ty::Type(_))
        })
    }

    /// `templateArgResolved` (`TypeCheck.hs:314-321`) with
    /// `constexprResolved` (`Constexpr.hs:385-390`).
    fn arg_resolved(&self, a: ExpId) -> bool {
        let t = self.ty(a);
        t.is_function() || t.is_type() || t.is_template() || (t.resolved() && self.constexpr_resolved(a))
    }

    fn constexpr_resolved(&self, id: ExpId) -> bool {
        match self.exp(id) {
            Exp::InitializerList(items) => self.items(*items).iter().all(|&i| self.constexpr_resolved(i)),
            Exp::Designator { value, .. } | Exp::Positional { value, .. } => self.constexpr_resolved(*value),
            Exp::IntLiteral { .. } | Exp::BoolLiteral(_) | Exp::FloatLiteral(_) | Exp::StringLiteral(_) | Exp::EnumValue { .. } => true,
            Exp::InterpolatedString(tok) => kg_hir::plain_string_value(tok).is_some(),
            _ => false,
        }
    }

    /// `checkEnumBaseType` and `checkEnumConsts` (`TypeCheck.hs:199-215`):
    /// the base must be an integer type, and each constant's folded value
    /// in its range — `[-2^(n-1), 2^(n-1))` for `intN`, `[0, 2^n)` for
    /// `uintN`, at the value. A value the engine left unfolded is passed
    /// over: `errorIfOutOfRange'`'s last clause would need what the
    /// reference knows and this tree may not.
    fn check_enum(&mut self, ty: ExpId, body: ExpId) {
        let base = self.ty(ty).the_type();
        if !base.resolved() {
            return;
        }
        if !base.is_int() {
            self.raise(ty, Diagnostic::error(self.head(ty), "Enums must be based on an integral type"));
            return;
        }
        let (lo, hi) = match base.concrete().un_const() {
            Ty::Signed(n) if n > 0 => {
                let half = num_bigint::BigInt::from(1) << (n - 1);
                (-half.clone(), half)
            }
            Ty::Unsigned(n) => (num_bigint::BigInt::from(0), num_bigint::BigInt::from(1) << n),
            _ => return,
        };
        for c in self.items(body) {
            let Exp::EnumConstant { value: Some(mut v), .. } = self.exp(c).clone() else { continue };
            while let Exp::EnumValue { value, .. } = self.exp(v) {
                v = *value;
            }
            let Exp::IntLiteral { value, .. } = self.exp(v) else { continue };
            let n = value.to_bigint();
            if n < lo || n >= hi {
                self.raise(v, Diagnostic::error(self.head(v), "Enum constant value is out of range for the base type"));
            }
        }
    }

    /// `checkFunctionArgs` / `checkArguments` (`TypeCheck.hs:87-97`).
    fn check_call(&mut self, call: ExpId, callee: ExpId, args: ExpId) {
        let t = self.ty(callee);
        let Some(params) = t.function_params() else { return };
        let params: Vec<Ty> = params.iter().map(|p| (*p.ty).clone()).collect();
        let args = self.items(args);
        if params.len() != args.len() {
            // A template's call is deduced and a higher-order function's is
            // rewritten before any call to it is checked.
            if params.iter().any(|p| matches!(p, Ty::Auto) || p.is_function()) {
                return;
            }
            // `closureCalls` puts the closure, cast to its capture, in front
            // of the arguments and keeps the list's note — at the first
            // argument written, or at the `)` (`Closure.hs:19-26`).
            let minted = args.first().is_some_and(|&a| {
                matches!(self.exp(a), Exp::Cast { value, .. } if is_closure(&self.ty(*value)))
            });
            let written = if minted { &args[1..] } else { &args[..] };
            let at = match written.first() {
                Some(&first) => self.head(first),
                None => {
                    let head = self.head(call);
                    FileRange { range: TextRange::at(head.range.end() - TextSize::from(1), TextSize::from(1)), ..head }
                }
            };
            let d = kg_ty::convert::invalid_argument_count_error(at, params.len());
            self.raise(call, d);
            return;
        }
        let pipelined = t.function_attrs().is_some_and(|a| a.contains(&TAttr::Flag(Flag::Pipelined)));
        let first = kg_ty::convert::first_checked_argument(pipelined, args.len());
        for (param, &arg) in params.iter().zip(args.iter()).skip(first) {
            self.check(param, arg, Narrowing::Warn);
        }
    }

    /// `checkVarDecl` (`TypeCheck.hs:123-144`), the clauses the CST pass
    /// speaks for.
    fn check_var(&mut self, text: bool, var: ExpId, ty: ExpId, init: ExpId, flags: DeclareFlag) {
        if flags == DeclareFlag::DeclareGlobal && self.judged_as(text, TextPass::ValidateProgram, |cx| cx.global_closure(var, init)) {
            return;
        }
        self.judged_as(text, TextPass::CheckConversion, |cx| cx.convert_var(var, ty, init, flags));
    }

    /// [`Cx::check_var`] past `reduceClosure`: the ECC memory clause and the
    /// initializer's conversion.
    fn convert_var(&mut self, var: ExpId, ty: ExpId, init: ExpId, flags: DeclareFlag) {
        // A declared `auto` is `deduceAuto`'s, never a conversion site.
        let declared = match self.exp(ty) {
            Exp::Const(inner) => *inner,
            _ => ty,
        };
        if matches!(self.exp(declared), Exp::Auto) {
            return;
        }
        let t = self.ty(var);
        if !t.resolved() {
            return;
        }
        if t.is_ecc_mem() {
            if flags != DeclareFlag::DeclareGlobal || t.is_const() {
                self.raise(var, Diagnostic::error(self.head(var), "ECC memories with initial values are not supported"));
            }
            return;
        }
        // "Global state is not allowed" fires first (`TypeCheck.hs:125-126`).
        if !t.is_const() && flags == DeclareFlag::DeclareGlobal {
            return;
        }
        // A designated initializer takes `errorNarrowingConversions`
        // (`TypeCheck.hs:130`) — under a copy's `TInstance` as under any
        // other type (`callbacks.k::80`'s `Foo<([[last]] bool) -> void>`).
        if self.starts_with_designator(&init) {
            self.check(&t, init, Narrowing::Error);
            return;
        }
        let s = self.ty(init).un_const();
        if !s.resolved() || s.is_error() {
            return;
        }
        self.check(&t, init, Narrowing::Warn);
    }

    /// `typeErrors` (`Frontend.hs:169-174`, pass 10) over the one node of the
    /// module's own text the CST pass has no node for: the `Binary` the
    /// parser builds for `x op= e` (`Parser.hs:631-638`), noted at the
    /// lvalue. `mem[0] += 10` over a `memory_ecc` is "Invalid
    /// operands to binary expression: `data.memory::ecc<|$uint@32|>` and
    /// `uint4`" at `mem[0]` (`memory.k::19`, `kg-oracle --errors`
    /// 2026-09-27). The engine's rounds fold as `inferExpTree` does
    /// (`Inference.hs:47`), so the type is the reference's; only an error
    /// no operand already carries is reported — `errorMsg` would have
    /// replaced that operand first (`Error.hs:147-148`). Written nodes
    /// stay `kg_db`'s `passes::type_errors`, copies are not claimed.
    fn desugared_operation_error(&mut self, value: ExpId) {
        let node = &self.ir().arena[value];
        let Exp::Binary(_, a, b) = node.exp else { return };
        // `x++`'s `Binary` and its `1` carry no note at all (`Parser.hs:
        // 612-614`), and `errorMsg` on it crashes the reference
        // (`Error.hs:199`, `bool b; b++;` under `kg-oracle --errors`): no
        // verdict to match. Its `1` is the lowering's; `op=`'s operand is
        // written.
        if node.src.written == kg_base::Written::Own || self.ir().arena[b].src.written != kg_base::Written::Own {
            return;
        }
        fn message(t: &Ty) -> Option<String> {
            match t {
                Ty::Error(m) => Some(m.to_string()),
                Ty::Const(x) => message(x).map(|m| format!("const {m}")),
                Ty::Instance { concrete, .. } => message(concrete),
                _ => None,
            }
        }
        let Some(text) = message(&self.ty(value)) else { return };
        if self.poisoned(a) || self.poisoned(b) || !self.ty(a).resolved() || !self.ty(b).resolved() {
            return;
        }
        let d = Diagnostic::error(self.head(value), text);
        self.raise(value, d);
    }

    /// `reduceClosure` (`TypeCheck.hs:129,146-148`): a global initialized
    /// with a closure (`tIsClosure s`, `Type.hs:617-618`) keeps it only when
    /// its capture is zero bits wide; otherwise "Closures that capture
    /// value(s) are not allowed in global scope" at the initializer. `true`
    /// when the clause decided the variable, as it ends `checkVar`. After
    /// "Global state is not allowed" (`:125-126`), which a mutable global
    /// hears first. The closure a *call* returns is the loop's to see —
    /// `const auto cipher = bind2nd(xor, 0xc0de);`, `higher-order-functions.
    /// k::195` (`kg-oracle --errors`, at the call); a lambda written as the
    /// initializer is also `kg_db`'s `validate_program::check_global_closure`,
    /// and is reported once (the seam, `Db::frontend_diagnostics_in`). A
    /// capture whose width is unknown is no verdict.
    fn global_closure(&mut self, var: ExpId, init: ExpId) -> bool {
        fn capture(t: &Ty) -> Option<&Ty> {
            match t {
                Ty::Closure { capture, .. } => Some(capture.get()),
                Ty::Const(x) | Ty::Instance { concrete: x, .. } => capture(x),
                _ => None,
            }
        }
        let t = self.ty(var);
        if t.resolved() && !t.is_const() {
            return false;
        }
        let s = self.ty(init);
        if !s.is_closure(&Ty::closure_template_path(self.interner)) {
            return false;
        }
        if capture(&s).and_then(kg_ty::width::t_width).is_some_and(|w| w > 0) {
            let d = Diagnostic::error(self.head(init), "Closures that capture value(s) are not allowed in global scope");
            self.raise(init, d);
        }
        true
    }

    /// `checkReturn` (`TypeCheck.hs:84`): `allowNarrowingConversions`, so
    /// only the unconvertible error.
    fn check_return(&mut self, function: &Ty, value: ExpId) {
        if !function.is_function() || !function.resolved() {
            return;
        }
        if function.function_attrs().is_some_and(|a| a.contains(&TAttr::Flag(Flag::Reset))) {
            return;
        }
        // `returnVoid` (`Frontend.hs:293-296`) has made `return e` of a void
        // `e` into `e; return;` before pass 19.
        if self.ty(value).un_const().is_void() {
            return;
        }
        let target = function.return_ty();
        self.check(&target, value, Narrowing::Allow);
    }

    fn starts_with_designator(&self, init: &ExpId) -> bool {
        let Exp::InitializerList(items) = self.exp(*init) else { return false };
        self.items(*items).first().is_some_and(|&f| matches!(self.exp(f), Exp::Designator { .. }))
    }

    /// `checkConversion` (`TypeCheck.hs:152-159`): the source normalised —
    /// an integer literal checks as its minimal type, an enum value under
    /// an integer target as its literal. The engine has already folded a
    /// constant into the literal it names.
    fn check(&mut self, target: &Ty, expr: ExpId, narrowing: Narrowing) {
        if target.is_error() || !target.resolved() || matches!(target, Ty::Auto) {
            return;
        }
        match self.exp(expr).clone() {
            Exp::Designator { value, .. } => return self.check(target, value, Narrowing::Error),
            Exp::Positional { value, .. } => return self.check(target, value, narrowing),
            _ => {}
        }
        let folded = match self.exp(expr) {
            Exp::IntLiteral { value, .. } => Some(ConstValue::Int { ty: None, value: value.clone() }),
            Exp::EnumValue { value, .. } => match self.exp(*value) {
                Exp::IntLiteral { value: v, .. } => {
                    Some(ConstValue::Enum { name: kg_base::SymbolPath::empty(), value: v.clone() })
                }
                _ => None,
            },
            _ => None,
        };
        let source = normalize_source(&self.ty(expr), target, folded.as_ref());
        self.check_prime(&source, target, expr, narrowing);
    }

    /// `checkConversion'` (`TypeCheck.hs:161-178`).
    fn check_prime(&mut self, source: &Ty, target: &Ty, expr: ExpId, narrowing: Narrowing) {
        // A type written where a value is expected is pass 1's error
        // ("Invalid use of a type as an expression", `validateTypedLiterals`)
        // and the program stops there (`Frontend.hs:44-52`): never a
        // conversion. One a rewrite made — `makeCapture`'s capture of a type
        // alias — is the conversion "from `type uint32`" (`lambdas.k::22`).
        let src = self.ir().arena[expr].src;
        let written_type = source.is_type() && src.written == kg_base::Written::Own && self.expansions.copy_frames(src.via).is_empty();
        if self.poisoned(expr) || source.is_error() || written_type {
            return;
        }
        if self.certainly_unresolved(expr) {
            let d = invalid_conversion_error(self.head(expr), "unresolved", &self.render(target), false);
            self.raise(expr, d);
            return;
        }
        if is_subtype(source, target) {
            return;
        }
        if let Exp::InitializerList(items) = self.exp(expr).clone() {
            self.check_init_list(items, target, narrowing);
            return;
        }
        if !source.resolved() {
            return;
        }
        let range = self.head(expr);
        if is_convertible_to(source, target) {
            let to = self.render(target);
            let d = match narrowing {
                Narrowing::Warn => narrowing_warning(range, &self.render(source), &to).with_fix(cast_fix(range, &to)),
                Narrowing::Error => invalid_conversion_error(range, &self.render(source), &to, true).with_fix(cast_fix(range, &to)),
                Narrowing::Allow => return,
            };
            self.raise(expr, d);
            return;
        }
        let d = invalid_conversion_error(range, &self.render(source), &self.render(target), false);
        self.raise(expr, d);
    }

    /// Whether the reference types `e` `TUnresolved` for good: `bitsizeof`
    /// and `bytesizeof` are `TUnresolved` until `evaluateConstexpr` folds
    /// them (`Type.hs:749-750`), which it does only through `sizeOf = tWidth
    /// . typeOf` (`Constexpr.hs:271-275`, `Type.hs:467-468`) — and a class
    /// has no width (`tWidth TClass{} = Nothing`, `Type.hs:718`); a unary or
    /// binary operation over an unresolved operand is `TUnresolved` in turn
    /// (`Type.hs:1183-1187,1279-1281`). The source is then `unresolved`,
    /// which no type is a supertype of and nothing converts to:
    /// `assert(bitsizeof(_c309) == 40)` over a class variable is "Invalid
    /// conversion from `unresolved` to `bool`" at the argument (`basics.k::
    /// 110`, `kg-oracle --errors`). Only that root is claimed — elsewhere an
    /// unresolved source may be this engine's not knowing — as the CST pass
    /// claimed it (the deleted CST pass's `certainly_unresolved`).
    fn certainly_unresolved(&self, e: ExpId) -> bool {
        match self.exp(e) {
            Exp::Unary(kg_const::UnOp::BitSizeof | kg_const::UnOp::ByteSizeof, x) => {
                let t = self.ty(*x);
                let t = match &t {
                    Ty::Type(inner) => inner.get().clone(),
                    other => other.clone(),
                };
                matches!(t.un_const().concrete(), Ty::Class(..))
            }
            Exp::Unary(_, x) => self.certainly_unresolved(*x),
            Exp::Binary(_, a, b) => self.certainly_unresolved(*a) || self.certainly_unresolved(*b),
            _ => false,
        }
    }

    /// `checkConversion'` step 4 (`TypeCheck.hs:165-175`), element by
    /// element against the slot each initializes.
    fn check_init_list(&mut self, items: ExpId, target: &Ty, narrowing: Narrowing) {
        let mut index: u32 = 0;
        for (n, item) in self.items(items).into_iter().enumerate() {
            if target.is_union() && n == 1 {
                self.raise(item, Diagnostic::error(self.head(item), "Excess elements in union initializer"));
                return;
            }
            let (element, slot_of) = match self.exp(item).clone() {
                Exp::Designator { name, value } => {
                    let Some(field) = kg_hir::rewrite::ident_sym(&self.ir().arena, name) else { continue };
                    (value, Ty::Designator(field, TyRef::new(self.ty(value).un_const())))
                }
                Exp::Positional { value, .. } => {
                    let slot = Ty::Positional(index, TyRef::new(self.ty(value).un_const()));
                    index += 1;
                    (value, slot)
                }
                _ => {
                    let slot = Ty::Positional(index, TyRef::new(self.ty(item).un_const()));
                    index += 1;
                    (item, slot)
                }
            };
            let designated = matches!(slot_of, Ty::Designator(..));
            let slot = initialized_type_pretty(target, &slot_of, Some(self.interner));
            if matches!(slot, Ty::Undefined) && designated && target.resolved() {
                let source = self.ty(element).un_const();
                if source.resolved() && !self.poisoned(element) {
                    let message = format!("Invalid conversion from `{}` to `undefined`", self.render(&source));
                    self.raise(element, Diagnostic::error(self.head(element), message));
                }
                continue;
            }
            if let Ty::Error(message) = &slot {
                let at = if designated { item } else { element };
                self.raise(at, Diagnostic::error(self.head(at), message.to_string()));
                if designated {
                    return;
                }
                continue;
            }
            let narrowing = if designated { Narrowing::Error } else { narrowing };
            self.check(&slot, element, narrowing);
        }
    }

    /// `null (errors a)` (`TypeCheck.hs:162`), widened to "this subtree
    /// already carries a diagnostic raised here, or an error type".
    fn poisoned(&self, expr: ExpId) -> bool {
        let range = self.head(expr);
        // Within the same copy: two copies of one body stand at one text,
        // and the first's verdict is not the second's.
        let via = self.ir().arena[expr].src.via;
        let inside = |v: &Expansion, d: &Diagnostic| {
            *v == via && d.is_error() && d.range.file == range.file && range.range.contains_range(d.range.range)
        };
        if self.raised.iter().any(|(v, d)| inside(v, d)) || self.text.iter().any(|(v, _, d)| inside(v, d)) {
            return true;
        }
        let mut stack = vec![expr];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            if self.ty(id).is_error() {
                return true;
            }
            kids.clear();
            self.exp(id).children(&mut kids);
            stack.extend(kids.iter().copied());
        }
        false
    }
}

/// Pass 9, `undefinedSymbols` (`Frontend.hs:82-157`): `para undefinedSymbol`
/// over every module — inside a copy, framed by its sites, and at the
/// module's own text, [`TextPass::UndefinedSymbols`]'s verdicts since sprint
/// W3 step 1. A name is undefined where the engine types it `TUndefined`,
/// which is `typeOfSymbol`'s failed `lookupInScope` (`Type.hs:1243-1245,
/// 1336`); a node inference never typed is no verdict, as in the reference.
struct Undefined<'a> {
    m: &'a Module,
    modules: &'a [Module],
    symbols: &'a crate::symbols::Symbols,
    expansions: &'a ExpansionInterner,
    interner: &'a Interner,
    raised: Vec<(Expansion, Diagnostic)>,
    /// The verdicts at the module's own text ([`TextPass::UndefinedSymbols`]).
    text: Vec<Diagnostic>,
    /// The same, on a name whose identifier a rewrite made ([`CopiedName`]).
    copied: Vec<CopiedName>,
    /// The callee a free call names, which the reference notes over the
    /// whole path (`freeFunctionSpecifier`, `Parser.hs:772-780`).
    callee: Option<ExpId>,
    /// A template template parameter's default, which the parser reads
    /// with `templateSpecifier` (`Parser.hs:368`) — a type's name.
    template_default: Option<ExpId>,
    /// Where every such default's name is written, in any module, asked
    /// only once a value's name at the text is undefined.
    defaults: &'a std::cell::OnceCell<Vec<FileRange>>,
}

impl Undefined<'_> {
    fn exp(&self, id: ExpId) -> &Exp {
        &self.m.ir.arena[id].exp
    }

    fn ty(&self, id: ExpId) -> Ty {
        self.m.types.get(id)
    }

    /// A verdict at `at`: in a copy, framed by its sites; at the text, the
    /// pass's own there — or, where a rewrite made the name's identifier,
    /// one only the text can place ([`CopiedName`]).
    fn err(&mut self, at: ExpId, message: String) -> bool {
        let src = self.m.ir.arena[at].src;
        let d = Diagnostic::error(src.head, message);
        if !self.expansions.copy_frames(src.via).is_empty() {
            self.raised.push((src.via, d));
            return true;
        }
        let name = self.ident(at).unwrap_or(at);
        let named = self.m.ir.arena[name].src;
        if named.written == kg_base::Written::Donated {
            self.copied.push(CopiedName { diagnostic: d, name: named.head, spelling: self.leaf(name), pass: TextPass::UndefinedSymbols });
        } else {
            self.text.push(d);
        }
        true
    }

    /// `para undefinedSymbol`: whether `id`'s subtree holds an error.
    fn walk(&mut self, id: ExpId) -> bool {
        let exp = self.exp(id).clone();
        match &exp {
            // `undefinedSymbol (Template _ b) …`: the parameters are judged,
            // the declaration restored unjudged.
            Exp::Template { params, .. } => return params.iter().fold(false, |acc, &p| self.walk(p) | acc),
            // `undefinedSymbol (TemplateInstance a _) …`: the arguments only.
            Exp::TemplateInstance { args, .. } => return self.walk(*args),
            // A method named on a receiver whose type is not resolved is
            // left as it is (`Frontend.hs:106-107`).
            Exp::FunctionSpecifier { receiver: Some(r), .. } if !self.ty(*r).resolved() => return false,
            // A method the receiver's class does not have, reported at its
            // name — `obj.Do()` on a `Method<false>` whose `Do` its `static
            // if` dropped is "not in scope" at `Do` (`static-if.k::42`,
            // `basics.k::183`). At the module's own text too: the receiver
            // is resolved, so the engine's "undefined" is the class's
            // answer, not a gap.
            Exp::FunctionSpecifier { receiver: Some(r), function } if matches!(self.ty(*function), Ty::Undefined) => {
                let below = self.walk(*r);
                if below {
                    return true;
                }
                let message = format!("Symbol name not in scope: `{}`{}", self.leaf(*function), self.hints(*function, false));
                let at = self.ident(*function).unwrap_or(*function);
                return self.err(at, message);
            }
            Exp::FunctionSpecifier { receiver: None, function } => self.callee = Some(*function),
            // A template template parameter's default is the parser's
            // `templateSpecifier` (`Parser.hs:368`): a name nothing declares
            // fails the parse there, at the name (`kg-oracle --errors`,
            // 2026-10-10: `template <typename, auto> typename M = memory`
            // under `mini-base`, "unexpected memory … expecting template
            // specifier"), which divergence #7 reports as the type name it
            // is read as. The lowering reads it as a value.
            Exp::TemplateParam { kind, default: Some(d), .. } if matches!(self.exp(*kind), Exp::Template { .. }) => {
                if let Exp::NamedValue(name) = *self.exp(*d) {
                    self.template_default = Some(name);
                }
            }
            // A `static if` still standing was not decided; its branches are
            // not the program's yet.
            Exp::StaticIf { cond, .. } => return self.walk(*cond),
            // `undefinedSymbol e _ | isAbbreviatedTemplate e = (e, False)`
            // (`Frontend.hs:91-92`).
            _ if abbreviated(&self.m.ir, id) => return false,
            _ => {}
        }
        let mut kids = Vec::new();
        match &exp {
            // The name a qualified or type identifier wraps is judged as the
            // identifier, not again on its own: the engine types a value's
            // `ScopedName` undefined with it, and the reference's verdict is
            // the identifier's (`static-if.k::61`, "Symbol name").
            Exp::QualifiedIdentifier(a) | Exp::TypeIdentifier(_, a) if matches!(self.exp(*a), Exp::ScopedName { .. }) => {
                self.exp(*a).children(&mut kids);
            }
            _ => exp.children(&mut kids),
        }
        let marks = (self.raised.len(), self.text.len(), self.copied.len());
        let below = kids.into_iter().fold(false, |acc, k| self.walk(k) | acc);
        if let Exp::Path(path) = &exp {
            return self.refused_path(id, path) | below;
        }
        if !matches!(self.ty(id), Ty::Undefined) {
            return below;
        }
        match exp {
            // At the name's last identifier, the note the parser gives a
            // scoped name (`kg-oracle --errors`: `Outer::Inne` at `Inne`,
            // `type-identifier.k::14`).
            Exp::TypeIdentifier(..) | Exp::ScopedName { .. } if !below => {
                let message = format!("Type name not in scope: `{}`{}", self.leaf(id), self.hints(id, true));
                let at = self.ident(id).unwrap_or(id);
                self.err(at, message)
            }
            // `err n` replaces the node, and what its subtree raised with it
            // (`Frontend.hs:101-102,113`): `Backend::Sv` is `Sv`'s verdict
            // alone, its undefined qualifier none (`options.k::5`). At its
            // note: a value's is its last identifier (`T::B2` in a copy at
            // `B2`, `enum.k::58`), a free call's the whole path.
            Exp::QualifiedIdentifier(_) => {
                self.raised.truncate(marks.0);
                self.text.truncate(marks.1);
                self.copied.truncate(marks.2);
                let as_type = self.template_default == Some(id);
                // The default an instance leaving the parameter out takes
                // is judged there as the value the lowering read; the
                // parameter's own verdict, as the type it is, is the one.
                let name = self.ident(id).unwrap_or(id);
                let head = self.m.ir.arena[name].src.head;
                let at_text = self.expansions.copy_frames(self.m.ir.arena[name].src.via).is_empty();
                if !as_type && at_text && self.defaults.get_or_init(|| template_defaults(self.modules)).contains(&head) {
                    return true;
                }
                let kind = if as_type { "Type" } else { "Symbol" };
                let message = format!("{kind} name not in scope: `{}`{}", self.leaf(id), self.hints(id, as_type));
                let at = if self.callee == Some(id) { id } else { self.ident(id).unwrap_or(id) };
                self.err(at, message)
            }
            Exp::MemberAccess { base, member } if self.ty(base).resolved() => {
                let name = kg_hir::rewrite::get_name(&self.m.ir.arena, member).map(|s| self.interner.resolve(s).to_string()).unwrap_or_default();
                let of = kg_ty::subtype::show_pretty(&self.ty(base), Some(self.interner));
                self.err(member, format!("Undefined member `{name}` of type `{of}`"))
            }
            _ => below,
        }
    }

    /// A path the sorter refused at the module's text, whose head neither
    /// the parse-time table nor `lookupInScope` from its scope finds
    /// (`Symbols.hs:76-87`): divergence #7's verdict where the reference's
    /// parser rejects — `optional<uint32>` under `mini-base`, whose `<` no
    /// parse-time template opens (`kg-oracle --errors`: "unexpected '<'",
    /// `processor/risc_v/trace.k:681`) — and pass 9's own where it reads the
    /// `<` as a comparison: the head's name, not in scope, at the head, as a
    /// type or a value as it is written. A head anything answers is left
    /// alone, and so is a path in a copy (the parser never reads one) and
    /// one with a `typename` before it.
    fn refused_path(&mut self, id: ExpId, path: &kg_hir::UnsortedPath) -> bool {
        let node = &self.m.ir.arena[id];
        let (Some(refused), Some(head)) = (&path.refused, path.segments.first()) else { return false };
        if refused.head_found
            || path.typename_kw
            || head.template_kw
            || !self.expansions.copy_frames(node.src.via).is_empty()
            || node.src.written == kg_base::Written::Donated
            || self.symbols.in_scope(refused.scope.segments(), &[head.name]).is_some()
        {
            return false;
        }
        let name = self.interner.resolve(head.name).to_string();
        let kind = if refused.in_type { "Type" } else { "Symbol" };
        let hint = self.hints_for(&name, &refused.scope, &[], refused.in_type);
        let at = FileRange { range: TextRange::at(node.src.head.range.start(), TextSize::of(name.as_str())), ..node.src.head };
        self.text.push(Diagnostic::error(at, format!("{kind} name not in scope: `{name}`{hint}")));
        true
    }

    /// The identifier node a qualified name ends in.
    fn ident(&self, id: ExpId) -> Option<ExpId> {
        match self.exp(id) {
            Exp::TypeIdentifier(_, a) | Exp::QualifiedIdentifier(a) => self.ident(*a),
            Exp::ScopedName { ident, .. } => Some(*ident),
            _ => None,
        }
    }

    /// `symbolName undefinedName`: the leaf of `getQualifiedName e`.
    fn leaf(&self, id: ExpId) -> String {
        kg_hir::rewrite::get_name(&self.m.ir.arena, id).map(|s| self.interner.resolve(s).to_string()).unwrap_or_default()
    }

    /// The use's `ScopedName`: `getScope e` and `getQualifier e` read it.
    fn scoped(&self, id: ExpId) -> Option<(QualifiedName, Vec<kg_base::Symbol>)> {
        let arena = &self.m.ir.arena;
        let mut at = id;
        loop {
            match &arena[at].exp {
                Exp::TypeIdentifier(_, a) | Exp::QualifiedIdentifier(a) => at = *a,
                Exp::ScopedName { scope, .. } => {
                    let mut path = kg_hir::sort::spine_qualname(arena, at);
                    path.pop();
                    return Some((scope.clone(), path));
                }
                _ => return None,
            }
        }
    }

    /// `hintMsg (hints cond)` (`Frontend.hs:110-155`): up to three names
    /// the edit distance finds similar — a module's own at distance one,
    /// anything in scope within the threshold — sorted by distance (ties by
    /// spelling, where the reference's `HashMap.toList` order is its own).
    fn hints(&self, id: ExpId, want_type: bool) -> String {
        let Some((scope, qualifier)) = self.scoped(id) else { return String::new() };
        self.hints_for(&self.leaf(id), &scope, &qualifier, want_type)
    }

    /// [`Undefined::hints`] for `target` written at `scope` under `qualifier`.
    fn hints_for(&self, target: &str, scope: &QualifiedName, qualifier: &[kg_base::Symbol], want_type: bool) -> String {
        let threshold = 3.min(target.chars().count() as u32 / 2);
        let mut scored: Vec<(u32, String)> = Vec::new();
        for (key, d) in self.symbols.iter() {
            let Some(last) = key.last() else { continue };
            let t = self.modules[d.module].types.get(d.id);
            let fits = if want_type { t.is_type() } else { t.is_template() || (t.resolved() && !t.is_type()) };
            if !fits {
                continue;
            }
            let candidate = self.interner.resolve(last);
            let distance = kg_base::restricted_damerau_levenshtein(target, &candidate);
            let segments = key.segments();
            let module_defined = segments.len() == 2 && self.interner.resolve(segments[0]).starts_with('@');
            let init = &segments[..segments.len() - 1];
            let in_scope = init.ends_with(qualifier) && scope.segments().starts_with(&init[..init.len() - qualifier.len()]);
            if !((distance <= 1 && module_defined) || (distance <= threshold && in_scope)) {
                continue;
            }
            let text = if module_defined && !in_scope {
                let ns = self.interner.resolve(segments[0]);
                format!("`{candidate}` defined in {}", kg_base::unmangle_namespace(&ns))
            } else {
                format!("`{candidate}`")
            };
            scored.push((distance, text));
        }
        scored.sort();
        scored.dedup_by(|a, b| a.1 == b.1);
        let chosen: Vec<String> = scored.into_iter().take(3).map(|(_, t)| t).collect();
        match chosen.len() {
            0 => String::new(),
            1 => format!("\nPerhaps you meant: {}", chosen[0]),
            _ => format!("\nPerhaps you meant one of these:\n\t{}", chosen.join("\n\t")),
        }
    }
}

/// Whether `op` over `a` and `b` is one of the fold's absorbing identities,
/// which give a value with one operand unknown: an integer literal beside
/// `&` or `|` (`evalBinaryInt`, `x & 0`, `x | 0xff`) and a boolean one
/// beside `&&` or `||` (`evalBinaryBool`, `x && false`, `x || true`) —
/// `Constexpr.hs:217-242`, an enum constant by its value (`:174-175`).
/// `i || (1)` is none of them: `evalBinaryInt` has no arm for `Or`, so a
/// dimension over a `static for`'s induction variable written so never
/// folds (difffuzz 2026-10-10, the re-run's `static-for.k::1` mutant;
/// `kg-oracle --errors`). Asked of the operands as written: a name that
/// may fold to such a literal is answered by the leaf rules that follow.
fn may_absorb(arena: &kg_hir::ExpArena, op: kg_const::BinOp, a: ExpId, b: ExpId) -> bool {
    use kg_const::BinOp::{And, BitwiseAnd, BitwiseOr, Or};
    let literal = |mut e: ExpId| {
        while let Exp::EnumValue { value, .. } = &arena[e].exp {
            e = *value;
        }
        arena[e].exp.clone()
    };
    let sides = [literal(a), literal(b)];
    match op {
        BitwiseAnd | BitwiseOr => sides.iter().any(|x| matches!(x, Exp::IntLiteral { .. })),
        And | Or => sides.iter().any(|x| matches!(x, Exp::BoolLiteral(_))),
        _ => false,
    }
}

/// [`Cx::certainly_runtime`], for `e` in `m`.
fn certainly_runtime(m: &Module, modules: &[Module], symbols: &crate::symbols::Symbols, e: ExpId) -> bool {
    fn go(m: &Module, modules: &[Module], symbols: &crate::symbols::Symbols, e: ExpId, runtime: &mut bool) -> bool {
        let arena = &m.ir.arena;
        match &arena[e].exp {
            Exp::IntLiteral { .. } | Exp::BoolLiteral(_) | Exp::FloatLiteral(_) => true,
            Exp::Binary(op, a, b) => {
                !may_absorb(arena, *op, *a, *b) && go(m, modules, symbols, *a, runtime) && go(m, modules, symbols, *b, runtime)
            }
            Exp::Unary(kg_const::UnOp::Neg | kg_const::UnOp::Invert | kg_const::UnOp::Not, a) => go(m, modules, symbols, *a, runtime),
            // `evaluateConstexpr` has no arm for a call (`Constexpr.hs:
            // 125-320`, `eval _ = removeNestedSeq e`): `uint32[three()] a;`
            // is "Array dimension must be a positive integer" at its `[`
            // (difffuzz 2026-10-10 [35], `kg-oracle --errors`).
            Exp::FunctionCall { .. } => {
                *runtime = true;
                true
            }
            Exp::NamedValue(inner) => {
                let Exp::QualifiedIdentifier(name) = &arena[*inner].exp else { return false };
                let Exp::ScopedName { scope, .. } = &arena[*name].exp else { return false };
                let path = kg_hir::sort::spine_qualname(arena, *name);
                let Some(d) = symbols.in_scope(scope.segments(), &path) else { return false };
                let home = &modules[d.module].ir.arena;
                *runtime |= match &home[d.id].exp {
                    Exp::FuncParam { .. } => true,
                    Exp::Variable { ty, .. } => !matches!(home[*ty].exp, Exp::Const(_)),
                    _ => false,
                };
                true
            }
            _ => false,
        }
    }
    let mut runtime = false;
    go(m, modules, symbols, e, &mut runtime) && runtime
}

/// A declaration, which `isType` holds of when it declares a type but which
/// stands among the statements as a declaration, never an expression.
fn declares(exp: &Exp) -> bool {
    matches!(exp, Exp::Enum { .. } | Exp::Struct { .. } | Exp::Class { .. } | Exp::Union { .. } | Exp::Alias { .. } | Exp::Template { .. })
}

/// `isAbbreviatedTemplate`: a function with an `auto` parameter is the
/// template `abbreviatedFunctionTemplate` makes of it, and its body is
/// judged in its copies, never as written — `run(auto iter)` in a class
/// copy, whose `foo([]() {})` is deduced only in `run`'s own copy
/// (`higher-order-functions.k::120`: asked, pass 12's LT arm invented
/// "Could not deduce all arguments for template `foo`"). A `const auto`
/// parameter is no `auto` (`isAuto . paramType`, `Parser/Syntax.hs:706-707,
/// 722`): its function is judged as written.
fn abbreviated(ir: &ModuleIr, id: ExpId) -> bool {
    let Exp::Function { params, .. } = &ir.arena[id].exp else { return false };
    kg_hir::rewrite::is_abbreviated_template(&ir.arena, *params)
}

/// A note as the reference stacks them (`data Src`, `Parser/Syntax.hs:
/// 610-626`): a source range, or a stack of two, which `<>` builds.
#[derive(Clone, Debug)]
enum Note {
    At(FileRange),
    Stack(Box<Note>, Box<Note>),
}

impl Note {
    /// A node's note: its range, stacked on its copy's instantiation
    /// sites, innermost first — the order `expandErrorStack` renders them
    /// in.
    fn of(expansions: &ExpansionInterner, src: kg_base::Src) -> Note {
        let mut sites: Vec<FileRange> = expansions.copy_frames(src.via).into_iter().map(|f| f.site).collect();
        let Some(last) = sites.pop() else { return Note::At(src.head) };
        let tail = sites.into_iter().rev().fold(Note::At(last), |acc, site| Note::Stack(Box::new(Note::At(site)), Box::new(acc)));
        Note::Stack(Box::new(Note::At(src.head)), Box::new(tail))
    }

    /// `self <> other` (`Parser/Syntax.hs:623-626`).
    fn then(self, other: Note) -> Note {
        match (self, other) {
            (a, Note::Stack(b, c)) => Note::Stack(Box::new(a), Box::new(Note::Stack(b, c))),
            (Note::Stack(a, b), c) => Note::Stack(a, Box::new(Note::Stack(Box::new(c), b))),
            (a, b) => Note::Stack(Box::new(a), Box::new(b)),
        }
    }

    /// Where an error so noted is reported: the head's own range
    /// (`fixErrorOffset`, `Error.hs:181-183`).
    fn head(&self) -> FileRange {
        match self {
            Note::At(r) => *r,
            Note::Stack(a, _) => a.head(),
        }
    }

    /// Its `while compiling` frames (`expandErrorStack`, `Error.hs:185-
    /// 189`): each plain range down the stack's right spine, in order; a
    /// stack on the left of one adds none (`addErrorContext _ e _`).
    fn frames(&self) -> Vec<FileRange> {
        let mut out = Vec::new();
        let Note::Stack(_, rest) = self else { return out };
        let mut rest: &Note = rest;
        loop {
            match rest {
                Note::At(r) => {
                    out.push(*r);
                    return out;
                }
                Note::Stack(a, b) => {
                    if let Note::At(r) = **a {
                        out.push(r);
                    }
                    rest = b;
                }
            }
        }
    }

    /// An error at this note.
    fn error(&self, message: impl Into<String>) -> Diagnostic {
        self.frames().into_iter().fold(Diagnostic::error(self.head(), message), |d, f| d.with_label(f, "while compiling"))
    }
}

/// Pass 13's verdicts, by who admits them ([`Checked`]).
struct ExportableVerdicts {
    /// In a copy, each with its whole note: its frames are in its labels.
    copies: Vec<Diagnostic>,
    /// At a module's own text, with the module whose CST run admits it —
    /// the one whose `export` or `extern` made the class exportable, where
    /// the CST pass gave it.
    text: Vec<(kg_base::FileId, Diagnostic)>,
}

/// Pass 13, `validateExportableClasses` (`Frontend.hs:337-391`). An
/// exported type template is refused at its `export` (`:340-341`,
/// `export-class.k::25`'s `export Foo;` in `Bar<bool>`); and a class made
/// exportable — by an `export` or `extern` of it, or as the type of a
/// public object of one (`classAndPublicObjects`, `:352-363`) — has its
/// public members restricted (`checkExportableClass`, `:365-390`), each
/// refusal at the member, stacked on what made the class exportable (`err
/// x e = NotedExp (note (unfix x) <> n)`): the member's own note, then the
/// public object's, then the `export`.
///
/// `exportableClasses` is `mapWith` over each module (`Recursion.hs:87-
/// 90`): a class two declarations make exportable is noted by both, the
/// earlier's first (`old <> new`) — `kg-oracle --errors`, 2026-10-10:
/// `export Foo;` then `extern Foo;` are two `while compiling` notes, in
/// that order; and the modules' maps are `Map.unions`, the first module in
/// program order that makes a class exportable giving it its whole note.
fn exportable_classes(e: &Elaborated, symbols: &crate::symbols::Symbols, interner: &Interner) -> ExportableVerdicts {
    const TEMPLATE: &str = "Type template cannot be exported. Perhaps you meant to export an instance of the template?";
    let expansions: &ExpansionInterner = &e.expansions;
    let in_copy = |src: kg_base::Src| !expansions.copy_frames(src.via).is_empty();
    let mut out = ExportableVerdicts { copies: Vec::new(), text: Vec::new() };
    let mut exportable: FxHashMap<kg_base::SymbolPath, (Note, kg_base::FileId)> = FxHashMap::default();
    for m in &e.modules {
        let ir = &m.ir;
        let mut here: Vec<(kg_base::SymbolPath, Note)> = Vec::new();
        for id in pre_order(ir) {
            let node = &ir.arena[id];
            let (Exp::ExportType { ty, .. } | Exp::Extern { ty, .. }) = &node.exp else { continue };
            let t = m.types.get(*ty);
            let note = Note::of(expansions, node.src);
            if matches!(node.exp, Exp::ExportType { .. }) && t.is_template() {
                if in_copy(node.src) {
                    out.copies.push(note.error(TEMPLATE));
                } else {
                    out.text.push((ir.file, note.error(TEMPLATE)));
                }
            }
            let mut found = Vec::new();
            class_and_public_objects(e, symbols, &t.the_type(), note, &mut found, &mut Vec::new());
            // `Map.insertWith (flip (<>))`: the note already there first.
            for (path, note) in found {
                match here.iter_mut().find(|(p, _)| *p == path) {
                    Some((_, old)) => *old = old.clone().then(note),
                    None => here.push((path, note)),
                }
            }
        }
        for (path, note) in here {
            exportable.entry(path).or_insert((note, ir.file));
        }
    }
    if exportable.is_empty() {
        return out;
    }
    for m in &e.modules {
        let ir = &m.ir;
        for id in pre_order(ir) {
            let node = &ir.arena[id];
            let Exp::Class { body, .. } = &node.exp else { continue };
            let Some((scope, name)) = crate::symbols::declared_name(&ir.arena, id) else { continue };
            let Some((n, home)) = exportable.get(&scope.concat(&kg_base::SymbolPath::single(name))) else { continue };
            for (x, what) in restricted_members(m, *body, interner) {
                let src = ir.arena[x].src;
                let d = Note::of(expansions, src).then(n.clone()).error(format!("{what} is not allowed in an exportable class"));
                if in_copy(src) {
                    out.copies.push(d);
                } else {
                    out.text.push((*home, d));
                }
            }
        }
    }
    out
}

/// Every node `ir`'s root reaches in source order, but inside a template's
/// own body or a `static if` left standing — untyped, so neither half of
/// pass 13 finds anything there.
fn pre_order(ir: &ModuleIr) -> Vec<ExpId> {
    let mut out = Vec::new();
    let mut stack = vec![ir.root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        let exp = &ir.arena[id].exp;
        if matches!(exp, Exp::Template { .. } | Exp::StaticIf { .. }) {
            continue;
        }
        out.push(id);
        kids.clear();
        exp.children(&mut kids);
        stack.extend(kids.iter().rev().copied());
    }
    out
}

/// `classAndPublicObjects n x` (`Frontend.hs:357-363`): the class `t`
/// names, through arrays, with `note`; and each public object of a class
/// type it holds, noted `note (unfix a) <> n`. `seen` guards the walk down
/// one path of members.
fn class_and_public_objects(
    e: &Elaborated,
    symbols: &crate::symbols::Symbols,
    t: &Ty,
    note: Note,
    out: &mut Vec<(kg_base::SymbolPath, Note)>,
    seen: &mut Vec<kg_base::SymbolPath>,
) {
    let t = t.un_const();
    if let Ty::Array { .. } = t.concrete() {
        return class_and_public_objects(e, symbols, &t.element(None), note, out, seen);
    }
    let Ty::Class(path, _) = t.concrete() else { return };
    out.push((path.clone(), note.clone()));
    if seen.contains(path) {
        return;
    }
    let Some(d) = symbols.by_name(path) else { return };
    let m = &e.modules[d.module];
    let Exp::Class { body, .. } = &m.ir.arena[d.id].exp else { return };
    seen.push(path.clone());
    for (public, member) in class_members(&m.ir, *body) {
        if public && matches!(m.ir.arena[member].exp, Exp::Variable { .. }) {
            let at = Note::of(&e.expansions, m.ir.arena[member].src).then(note.clone());
            class_and_public_objects(e, symbols, &m.types.get(member), at, out, seen);
        }
    }
    seen.pop();
}

/// `mapClassMembers` (`Desugar.hs:115-121`): each member with whether it
/// is public — private until a `public:`, and after each marker as it says.
fn class_members(ir: &ModuleIr, body: ExpId) -> Vec<(bool, ExpId)> {
    let Exp::Seq(items) = &ir.arena[body].exp else { return Vec::new() };
    let mut public = false;
    let mut out = Vec::new();
    for &i in items {
        match ir.arena[i].exp {
            Exp::Public => public = true,
            Exp::Private => public = false,
            _ => out.push((public, i)),
        }
    }
    out
}

/// `checkExportableClass (Just n)`'s `check` (`Frontend.hs:365-390`): each
/// public member it refuses, with what it is.
fn restricted_members(m: &Module, body: ExpId, interner: &Interner) -> Vec<(ExpId, String)> {
    let ir = &m.ir;
    let mut out = Vec::new();
    for (public, x) in class_members(ir, body) {
        if !public {
            continue;
        }
        let what = match &ir.arena[x].exp {
            Exp::Variable { .. } if m.types.get(x).is_function() => "Public callback".to_owned(),
            Exp::Variable { .. } => "Public field".to_owned(),
            Exp::Function { modifier: Some(md), .. } if matches!(ir.arena[*md].exp, Exp::FunctionModifier(kg_hir::Modifier::Inline)) => {
                "Public inline method".to_owned()
            }
            Exp::Function { .. } => {
                let t = m.types.get(x);
                let Some(attrs) = t.function_attrs() else { continue };
                let valid = |a: &kg_ty::TAttr| match a {
                    TAttr::Flag(f) => matches!(f, Flag::Async | Flag::NoBackPressure | Flag::Pure | Flag::Reset),
                    TAttr::Int(k, _) => matches!(k, kg_ty::Attribute::Latency | kg_ty::Attribute::MaxThreads | kg_ty::Attribute::ThreadRate),
                    _ => false,
                };
                let Some(bad) = attrs.iter().find(|a| !valid(a)) else { continue };
                format!("Public method with [[{}]] attribute", kg_ty::subtype::show_attr(bad, Some(interner)))
            }
            _ => continue,
        };
        out.push((x, what));
    }
    out
}

/// Pass 10, `cata typeErrors` (`Frontend.hs:169-174`), at each module's
/// own text: a node `inferType` typed as an error (`tIsError`, through
/// `TConst` and `TInstance`, `Type.hs:626-627`) whose subtree holds no
/// error yet — `errorMsg` replaces the node it judges (`Error.hs:147-148`),
/// so one ill-typed subexpression is one report, at the innermost node —
/// reported as `showPretty` spells its type.
///
/// The types are the loop's, folded as `inferExpTree` folds them
/// (`Inference.hs:47`), which is the whole point of the move: the CST pass
/// read the CST engine's, and had to stand down where a constant it could
/// not fold might have changed the rule (its `fold_divergent`).
///
/// Walked as `inferType` types the program: not a template's own body,
/// which it visits for its condition only (`Inference.hs:63`), nor a
/// `static if` it left standing, nor an abbreviated template's leftover.
/// A copy is not judged here yet. Only a node written at this text is
/// reported — one a rewrite minted, the `Binary` of `x op= e` say, has no
/// node in it (`desugared_operation_error` speaks for that one) — but
/// any error below a node keeps it quiet, as it keeps the reference's.
/// The reference's note of a parenthesized expression starts at its `(`
/// (`parens`, `Parser.hs`); the IR has no node for the parentheses, and
/// `kg_db` moves the range there, where the text is.
///
/// With the nodes it replaced, by module index: `errorMsg` leaves an
/// `ErrorF` in the node's place (`Error.hs:147-148`), so no later pass sees
/// it as what it was — `static for (const auto i : -3)`'s induction
/// variable is pass 10's "can't be negative" and not also pass 17's "Could
/// not deduce variable type" (`basics.k::2`, `kg-oracle --errors`
/// 2026-10-10). One a rewrite minted is replaced too, though no text
/// reports it.
fn type_errors(e: &Elaborated) -> (Vec<Diagnostic>, kg_base::FxHashSet<(usize, ExpId)>) {
    fn message(t: &Ty) -> Option<String> {
        match t {
            Ty::Error(m) => Some(m.to_string()),
            Ty::Const(x) => message(x).map(|m| format!("const {m}")),
            Ty::Instance { concrete, .. } => message(concrete),
            _ => None,
        }
    }
    struct Walk<'a> {
        e: &'a Elaborated,
        m: &'a Module,
        index: usize,
        out: Vec<Diagnostic>,
        replaced: kg_base::FxHashSet<(usize, ExpId)>,
    }
    impl Walk<'_> {
        /// Whether `id`'s subtree holds an error after the walk.
        fn walk(&mut self, id: ExpId) -> bool {
            let ir = &self.m.ir;
            let node = &ir.arena[id];
            if !self.e.expansions.copy_frames(node.src.via).is_empty() {
                return false;
            }
            match &node.exp {
                Exp::Template { .. } | Exp::StaticIf { .. } => return false,
                Exp::Error(_) => return true,
                _ if abbreviated(ir, id) => return false,
                _ => {}
            }
            let mut kids = Vec::new();
            node.exp.children(&mut kids);
            let below = kids.into_iter().fold(false, |acc, k| self.walk(k) | acc);
            if below {
                return true;
            }
            let Some(text) = message(&self.m.types.get(id)) else { return false };
            if node.src.written == kg_base::Written::Own && node.src.via == Expansion::ROOT {
                self.out.push(Diagnostic::error(node.src.head, text));
            }
            self.replaced.insert((self.index, id));
            true
        }
    }
    let mut out = Vec::new();
    let mut replaced = kg_base::FxHashSet::default();
    for (index, m) in e.modules.iter().enumerate() {
        let mut w = Walk { e, m, index, out: std::mem::take(&mut out), replaced: std::mem::take(&mut replaced) };
        w.walk(m.ir.root);
        out = w.out;
        replaced = w.replaced;
    }
    (out, replaced)
}

/// Pass 10, `typeErrors` (`Frontend.hs:169-174`), inside the copies — what
/// [`type_errors`] leaves out: `cata typeErrors` runs over the program
/// with its instances in it, so `!x` in `Foo<uint32>`'s copy of a member
/// that nothing calls is "Expected a boolean operand" at the template's
/// text, framed by the instantiation (difffuzz 2026-10-10 [34];
/// `kg-oracle --errors`, as for `f<bool>`'s `bool + 1`). The same rule as
/// at the text: post-order, an error below keeps a node quiet, and only a
/// node the template's text wrote is reported. A node with no frames of
/// its own — a template argument substituted in, which keeps its `Src` —
/// is the text's to judge where it is written.
fn copy_type_errors(e: &Elaborated) -> Vec<(Expansion, Diagnostic)> {
    fn message(t: &Ty) -> Option<String> {
        match t {
            Ty::Error(m) => Some(m.to_string()),
            Ty::Const(x) => message(x).map(|m| format!("const {m}")),
            Ty::Instance { concrete, .. } => message(concrete),
            _ => None,
        }
    }
    struct Walk<'a> {
        e: &'a Elaborated,
        m: &'a Module,
        out: Vec<(Expansion, Diagnostic)>,
    }
    impl Walk<'_> {
        /// Whether `id`'s subtree holds an error after the walk; `copied`,
        /// whether the walk is inside a copy.
        fn walk(&mut self, id: ExpId, copied: bool) -> bool {
            let ir = &self.m.ir;
            let node = &ir.arena[id];
            let framed = !self.e.expansions.copy_frames(node.src.via).is_empty();
            let copied = copied || framed;
            match &node.exp {
                Exp::Template { .. } | Exp::StaticIf { .. } => return false,
                Exp::Error(_) => return true,
                _ if abbreviated(ir, id) => return false,
                _ => {}
            }
            let mut kids = Vec::new();
            node.exp.children(&mut kids);
            let below = kids.into_iter().fold(false, |acc, k| self.walk(k, copied) | acc);
            if below || !copied {
                return below;
            }
            let Some(text) = message(&self.m.types.get(id)) else { return false };
            if framed && node.src.written == kg_base::Written::Own {
                self.out.push((node.src.via, Diagnostic::error(node.src.head, text)));
            }
            true
        }
    }
    let mut out = Vec::new();
    for m in &e.modules {
        let mut w = Walk { e, m, out: std::mem::take(&mut out) };
        w.walk(m.ir.root, false);
        out = w.out;
    }
    out
}

/// Pass 17, `deduceAuto` (`Type.hs:1595-1610`), at each module's own text:
/// a function whose written return type is `auto` and a variable declared
/// `auto` or `const auto` take the type inference gave them, written back
/// as syntax by `expFromType` (`:1612-1650`), and where it has none to
/// write — "Could not deduce function return type" or "Could not deduce
/// variable type", at the `auto`.
///
/// ```haskell
/// typeExp e err t = NotedExp (Typed (TType t) (untype $ note $ unfix e)) (fromMaybe err $ expFromType e t)
/// auto n e@FunctionF{..} | isAuto funcReturnType = e { funcReturnType = typeExp funcReturnType err $ tReturn $ typeof n }
/// auto n e@VariableF{..} | isAuto varType || isConstAuto varType = e { varType = typeExp varType err $ typeof n }
/// ```
///
/// `isAuto` alone for a function: `isAuto (Const Auto)` is false
/// (`Parser/Syntax.hs:706-707`), so `inline const auto F()` is left to pass
/// 19's `check AutoF{}`. The verdict is at the written type's note
/// (`note $ unfix e`): `const auto x` from the `const`, but a `for`'s
/// induction variable from the `auto` — `uninitConst` notes the `Const`
/// after the keyword (`Parser.hs:451`; `auto-placeholder.k::7` at 5:19).
///
/// Walked as pass 17 sees the program: after `removeTemplate` (pass 15,
/// `Frontend.hs:297-302`) — no template's own body, no abbreviated
/// template's — and after pass 16 replaced every `static if` left standing
/// with its error (`Frontend.hs:406`), and pass 10 every node it reported
/// at (`replaced`, [`type_errors`]). A copy is not judged here yet.
///
/// Each verdict says whether the type was only *undecided* (an unresolved
/// or undefined part, and nothing `expFromType` refuses outright): such a
/// verdict may be another module's fallout ([`TextVerdict::undecided`]).
/// Two are settled by the syntax, whatever the rest of the program — and
/// the CST pass's two certainties: a variable with no initializer at all
/// (`try _ Nothing = TUnresolved`, `Type.hs:1108,1285-1286`; `auto x;`),
/// and a `static for`'s induction variable over a bound no fold can give
/// (`constIndexType`, `:1113,1136-1141`; a parameter, a non-`const`
/// local — [`certainly_runtime`]). Under a latency bound (`truncated`)
/// the next round might have decided an undecided one, and none is given
/// — a truncated loop leaves verdicts out, never puts one in
/// (`Cx::truncated`).
fn deduce_auto(
    e: &Elaborated,
    symbols: &crate::symbols::Symbols,
    replaced: &kg_base::FxHashSet<(usize, ExpId)>,
    truncated: bool,
) -> Vec<(Diagnostic, bool)> {
    let mut out = Vec::new();
    for (index, m) in e.modules.iter().enumerate() {
        let ir = &m.ir;
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        let mut runtime_bound: kg_base::FxHashSet<ExpId> = kg_base::FxHashSet::default();
        while let Some(id) = stack.pop() {
            let node = &ir.arena[id];
            if !e.expansions.copy_frames(node.src.via).is_empty() || abbreviated(ir, id) || replaced.contains(&(index, id)) {
                continue;
            }
            let judged = match &node.exp {
                Exp::Template { .. } | Exp::StaticIf { .. } => continue,
                Exp::StaticFor { var, limit, .. } | Exp::UnrolledFor { var, limit, .. }
                    if certainly_runtime(m, &e.modules, symbols, *limit) =>
                {
                    runtime_bound.insert(*var);
                    None
                }
                Exp::Function { ret, .. } if matches!(ir.arena[*ret].exp, Exp::Auto) => {
                    Some((*ret, m.types.get(id).return_ty(), "Could not deduce function return type", false))
                }
                Exp::Variable { ty, flags, init, .. } => {
                    let settled = (init.is_none() && *flags != DeclareFlag::DeclareUninitConst) || runtime_bound.contains(&id);
                    match &ir.arena[*ty].exp {
                        Exp::Auto => Some((*ty, m.types.get(id), "Could not deduce variable type", settled)),
                        Exp::Const(inner) if matches!(ir.arena[*inner].exp, Exp::Auto) => {
                            let at = if *flags == DeclareFlag::DeclareUninitConst { *inner } else { *ty };
                            Some((at, m.types.get(id), "Could not deduce variable type", settled))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some((at, deduced, message, settled)) = judged
                && ir.arena[at].src.written == kg_base::Written::Own
            {
                let undecided = match writable(&deduced) {
                    Writable::Yes => None,
                    Writable::Undecided => Some(!settled),
                    Writable::No => Some(false),
                };
                if let Some(undecided) = undecided.filter(|u| !(*u && truncated)) {
                    out.push((Diagnostic::error(ir.arena[at].src.head, message), undecided));
                }
            }
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().rev().copied());
        }
    }
    out
}

/// Whether `expFromType` (`Type.hs:1612-1650`) writes a type back as
/// syntax.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Writable {
    Yes,
    /// `Nothing` for a part inference has not decided — `TUnresolved`,
    /// `TUndefined` — and for no other reason.
    Undecided,
    /// `Nothing` whatever is decided: a template, an initializer list or
    /// a member of one, an array of no dimensions, an error, `TUntyped`.
    No,
}

fn writable(t: &Ty) -> Writable {
    let all = |parts: &mut dyn Iterator<Item = Writable>| {
        parts.fold(Writable::Yes, |acc, w| match (acc, w) {
            (Writable::No, _) | (_, Writable::No) => Writable::No,
            (Writable::Undecided, _) | (_, Writable::Undecided) => Writable::Undecided,
            _ => Writable::Yes,
        })
    };
    match t {
        Ty::Auto
        | Ty::Void
        | Ty::Boolean
        | Ty::Float
        | Ty::Str
        | Ty::Signed(_)
        | Ty::Unsigned(_)
        | Ty::Reference(_)
        | Ty::Closure { .. }
        | Ty::Enum(..)
        | Ty::Struct(..)
        | Ty::Union(..)
        | Ty::Class(..) => Writable::Yes,
        Ty::Type(x) | Ty::Dependent(x) | Ty::Const(x) => writable(x),
        Ty::Instance { concrete, .. } => writable(concrete),
        Ty::Array { dims, .. } if dims.is_empty() => Writable::No,
        Ty::Array { elem, .. } => writable(elem),
        Ty::Function { ret, params, .. } => all(&mut std::iter::once(&**ret).chain(params.iter().map(|p| &*p.ty)).map(writable)),
        Ty::Unresolved | Ty::Undefined => Writable::Undecided,
        Ty::Template | Ty::Initializer(_) | Ty::Designator(..) | Ty::Positional(..) | Ty::Error(_) | Ty::Untyped => Writable::No,
    }
}

/// Pass 16, `postDesugar` (`Frontend.hs:176-183`), its two diagnostic
/// algebras at each module's own text, over what `evaluateConstexpr` left:
///
/// ```haskell
/// staticAssert (StaticAssertF a) = case a of
///     BoolLiteral True  -> SeqF []
///     BoolLiteral False -> desugarError "static assertion failed"
///     _                 -> StaticAssertF $ errorMsg "static assertion expression did not evaluate to a boolean constant" a
/// unresolved (ByteOffsetF t f) = offsetErr t f
/// unresolved (BitOffsetF t f)  = offsetErr t f
/// unresolved (UnaryF Clog2 _)  = desugarError "clog2 argument did not evaluate to a positive integer constant"
/// unresolved ParamIntF{}       = desugarError "int width did not evaluate to a positive integer constant"
/// unresolved ParamUintF{}      = desugarError "uint width did not evaluate to a positive integer constant"
/// unresolved StaticIfF{}       = desugarError "static if predicate did not evaluate to a boolean constant"
/// ```
/// (`Frontend.hs:393-407,446-449`)
///
/// The loop folds in place as `evaluateConstexpr` does, so a node still
/// standing is one the fold left — a `clog2` of no natural, a width that
/// is no natural, a `static if` it could not decide, an offset of a type
/// it never resolved (`Constexpr.hs:162-169,298-302`). `desugar` is a
/// `cata` (`Desugar.hs:66-69`): a node it replaces takes everything inside
/// with it, this pass's verdicts below included — `int<clog2(0)>` is the
/// width's error alone, `static if (clog2(0) == 1)` the `static if`'s.
///
/// Walked as pass 16 sees the program: after `removeTemplate` (pass 15) —
/// no template's own body, no abbreviated template's — and after pass 10
/// replaced the nodes it reported at (`replaced`). A copy is the loop's
/// own check's (`Cx`). Three places the CST pass stood down and this
/// does too: a `static assert` whose expression holds an earlier error
/// (the reference replaces the expression, that error with it, which the
/// CST run cannot unsay); a `clog2` as the argument of a call to a
/// template's instance left standing, which pass 11 replaced first with
/// "Unresolved argument" where the argument is a constant and which no
/// version judges otherwise yet; and the legacy
/// `static_assert(…)`, a node `staticAssert` does not match.
///
/// Each verdict says whether it is *undecided* — the expression it judges
/// holds something besides literals and runtime names, which an error in
/// another module, or a later round, could have folded
/// ([`TextVerdict::undecided`]); under a latency bound those are not
/// given.
fn post_desugar(
    e: &Elaborated,
    symbols: &crate::symbols::Symbols,
    replaced: &kg_base::FxHashSet<(usize, ExpId)>,
    truncated: bool,
) -> Vec<(Diagnostic, bool)> {
    struct Walk<'a> {
        e: &'a Elaborated,
        m: &'a Module,
        index: usize,
        symbols: &'a crate::symbols::Symbols,
        replaced: &'a kg_base::FxHashSet<(usize, ExpId)>,
    }
    impl Walk<'_> {
        fn exp(&self, id: ExpId) -> &Exp {
            &self.m.ir.arena[id].exp
        }

        fn head(&self, id: ExpId) -> FileRange {
            self.m.ir.arena[id].src.head
        }

        /// Whether `id` was written at this text: a verdict needs a place
        /// in it.
        fn own(&self, id: ExpId) -> bool {
            self.m.ir.arena[id].src.written == kg_base::Written::Own
        }

        /// Whether nothing could fold `id` that has not: every leaf a
        /// literal, a runtime name (a parameter, a variable not `const`,
        /// which `eval (NamedValueF x)` never replaces, `Constexpr.hs:
        /// 286-293`), a call (no `eval` arm folds one), or an error already
        /// reported.
        fn settled(&self, id: ExpId) -> bool {
            match self.exp(id) {
                Exp::IntLiteral { .. } | Exp::BoolLiteral(_) | Exp::FloatLiteral(_) | Exp::StringLiteral(_) => true,
                Exp::Error(_) | Exp::FunctionCall { .. } => true,
                Exp::Binary(_, a, b) => self.settled(*a) && self.settled(*b),
                Exp::Unary(_, a) => self.settled(*a),
                Exp::Mux { selector, cases } => self.settled(*selector) && self.settled(*cases),
                Exp::Seq(items) => items.iter().all(|&i| self.settled(i)),
                Exp::NamedValue(_) => certainly_runtime(self.m, &self.e.modules, self.symbols, id),
                _ => false,
            }
        }

        fn natural(&self, id: ExpId) -> bool {
            matches!(self.exp(id), Exp::IntLiteral { value, .. } if value.is_positive())
        }

        fn literal(&self, id: ExpId) -> bool {
            matches!(self.exp(id), Exp::IntLiteral { .. } | Exp::BoolLiteral(_) | Exp::FloatLiteral(_) | Exp::StringLiteral(_))
        }

        /// Whether `id`'s subtree holds what keeps this pass from judging
        /// it at all: an error passes 1–8 left (`Exp::Error`) — the
        /// reference stops before pass 16 then (`Template.hs:84`), though
        /// the CST run's folder reports its own only after every pass — or
        /// a shift of two literals the fold left, a negative count, on
        /// which the reference *crashes* (`basics.k::4,5`; `kg_const`'s
        /// `shift_count`).
        fn unjudged(&self, id: ExpId) -> bool {
            let ir = &self.m.ir;
            let mut stack = vec![id];
            let mut kids = Vec::new();
            while let Some(at) = stack.pop() {
                match &ir.arena[at].exp {
                    Exp::Error(_) => return true,
                    Exp::Binary(kg_const::BinOp::ShiftLeft | kg_const::BinOp::ShiftRight, a, b) if self.literal(*a) && self.literal(*b) => {
                        return true;
                    }
                    _ => {}
                }
                kids.clear();
                ir.arena[at].exp.children(&mut kids);
                stack.extend(kids.iter().copied());
            }
            false
        }

        /// Whether pass 10 replaced a node in `id`'s subtree.
        fn typed_wrong(&self, id: ExpId) -> bool {
            let ir = &self.m.ir;
            let mut stack = vec![id];
            let mut kids = Vec::new();
            while let Some(at) = stack.pop() {
                if self.replaced.contains(&(self.index, at)) {
                    return true;
                }
                kids.clear();
                ir.arena[at].exp.children(&mut kids);
                stack.extend(kids.iter().copied());
            }
            false
        }

        /// `id`'s own verdict, its subtree's already in `out`: `None` keeps
        /// them, `Some` replaces them — with nothing, or with this one.
        fn judge(&self, id: ExpId, argument: bool) -> Option<Option<(Diagnostic, bool)>> {
            let exp = &self.m.ir.arena[id].exp;
            let error = |at: FileRange, message: &str| Diagnostic::error(at, message.to_owned());
            let judged = match exp {
                Exp::StaticIf { cond: x, .. } | Exp::ParamInt(x) | Exp::ParamUint(x) | Exp::Unary(kg_const::UnOp::Clog2, x) | Exp::StaticAssert(x) => Some(*x),
                Exp::BitOffset { ty, .. } | Exp::ByteOffset { ty, .. } => Some(*ty),
                _ => None,
            };
            if judged.is_some_and(|x| self.unjudged(x)) {
                return Some(None);
            }
            match *exp {
                Exp::StaticIf { cond, .. } if !matches!(self.exp(cond), Exp::BoolLiteral(_)) && self.own(id) => {
                    let decided = self.literal(cond) || self.settled(cond);
                    let d = error(self.head(id), "static if predicate did not evaluate to a boolean constant")
                        .with_kind(DiagnosticKind::StaticIfUndecided);
                    Some(Some((d, !decided)))
                }
                Exp::ParamInt(width) | Exp::ParamUint(width) if self.own(id) => {
                    let kind = if matches!(exp, Exp::ParamInt(_)) { "int" } else { "uint" };
                    // A width pass 10 rejected inside never folds, whatever
                    // else does (`type-check.k::1-3`).
                    let decided = self.literal(width) || self.settled(width) || self.typed_wrong(width);
                    let d = error(self.head(id), &format!("{kind} width did not evaluate to a positive integer constant"))
                        .with_kind(DiagnosticKind::WidthUndecided);
                    Some(Some((d, !decided)))
                }
                Exp::Unary(kg_const::UnOp::Clog2, arg) if self.own(id) => {
                    if argument {
                        // `unresolvedFunctionTemplateArgs` (pass 11,
                        // `Frontend.hs:436-444`) got there first.
                        let unresolved = self.literal(arg) && !self.natural(arg);
                        return Some(unresolved.then(|| (error(self.head(id), "Unresolved argument"), false)));
                    }
                    let decided = self.literal(arg) || self.settled(arg);
                    Some(Some((error(self.head(id), "clog2 argument did not evaluate to a positive integer constant"), !decided)))
                }
                Exp::StaticAssert(cond) if self.own(id) => match self.exp(cond) {
                    Exp::BoolLiteral(true) => Some(None),
                    Exp::BoolLiteral(false) => Some(Some((error(self.head(id), "static assertion failed"), false))),
                    // An operand pass 10 reported inside is judged all the
                    // same: `errorMsg` replaces it whole (`Error.hs:147`),
                    // pass 10's verdict with it (`static assert(!0xff);` at
                    // the `!`, difffuzz 2026-10-10 [17]; `kg-oracle
                    // --errors`) — [`earlier_text`] drops pass 10's.
                    _ if !self.own(cond) => None,
                    // A type where the value goes is the reference's parse
                    // error, "Invalid use of a type as an expression"
                    // (`type-check.k::9`, `kg-oracle --errors`).
                    _ if matches!(self.m.types.get(cond), Ty::Type(_)) => None,
                    _ => {
                        let decided = self.literal(cond) || self.settled(cond);
                        let d = error(self.head(cond), "static assertion expression did not evaluate to a boolean constant")
                            .with_kind(DiagnosticKind::StaticAssertUndecided);
                        Some(Some((d, !decided)))
                    }
                },
                Exp::BitOffset { ty, field } | Exp::ByteOffset { ty, field } if self.own(id) => {
                    // `offsetErr` (`Frontend.hs:446-449`): the fold resolves
                    // every offset of a type it resolved, so what stands is
                    // the type's own undecided.
                    let t = self.m.types.get(ty);
                    let struct_ = matches!(&t, Ty::Type(x) if matches!(**x, Ty::Struct(..)));
                    let (at, message) = if struct_ { (field, "Undefined field") } else { (ty, "Field offset is only defined for structs") };
                    Some(Some((error(self.head(at), message), !t.resolved())))
                }
                _ => None,
            }
        }

        /// Whether the walk stops at `id`: a copy (the loop's own check's),
        /// an abbreviated template's leftover or a template's body (pass
        /// 15), a node pass 10 replaced, an error, the legacy assertion.
        fn skipped(&self, id: ExpId) -> bool {
            let node = &self.m.ir.arena[id];
            !self.e.expansions.copy_frames(node.src.via).is_empty()
                || abbreviated(&self.m.ir, id)
                || self.replaced.contains(&(self.index, id))
                || matches!(node.exp, Exp::Template { .. } | Exp::Error(_) | Exp::StaticAssertLegacy(_))
        }

        /// Whether `callee` is still a template's instance after
        /// instantiation — what `unresolvedTemplateCall` matches,
        /// `FunctionSpecifier _ (QualifiedIdentifier TemplateInstance{})`
        /// (`Frontend.hs:437`): a call the loop could not deduce, `println`'s
        /// over `clog2(-1)` (`basics.k::3`). A plain function's call, or an
        /// instance a round made and `resolve` renamed, is not one, and its
        /// `clog2` argument is pass 16's (`kg-oracle --errors`, 2026-10-10).
        fn calls_an_instance(&self, callee: ExpId) -> bool {
            let Exp::FunctionSpecifier { function, .. } = *self.exp(callee) else { return false };
            matches!(self.exp(function), Exp::QualifiedIdentifier(i) if matches!(self.exp(*i), Exp::TemplateInstance { .. }))
        }

        /// Every verdict in the module, post-order, into `out`: a node's
        /// verdict replaces its subtree's, as the `cata` replaces the node.
        /// The arguments of a call to a template's instance are walked as
        /// such (`argument`), the list that holds them not at all.
        fn walk(&self, out: &mut Vec<(Diagnostic, bool)>) {
            enum Visit {
                Enter(ExpId, bool),
                Exit(ExpId, bool, usize),
            }
            let mut stack = vec![Visit::Enter(self.m.ir.root, false)];
            let mut kids = Vec::new();
            let mut items = Vec::new();
            while let Some(visit) = stack.pop() {
                match visit {
                    Visit::Enter(id, argument) => {
                        if self.skipped(id) {
                            continue;
                        }
                        stack.push(Visit::Exit(id, argument, out.len()));
                        let exp = self.exp(id);
                        let args = match exp {
                            Exp::FunctionCall { function, args, .. } if self.calls_an_instance(*function) => Some(*args),
                            _ => None,
                        };
                        kids.clear();
                        exp.children(&mut kids);
                        for &k in kids.iter().rev() {
                            if Some(k) == args {
                                items.clear();
                                self.exp(k).children(&mut items);
                                stack.extend(items.iter().rev().map(|&i| Visit::Enter(i, true)));
                            } else {
                                stack.push(Visit::Enter(k, false));
                            }
                        }
                    }
                    Visit::Exit(id, argument, start) => {
                        if let Some(verdict) = self.judge(id, argument) {
                            out.truncate(start);
                            out.extend(verdict);
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (index, m) in e.modules.iter().enumerate() {
        Walk { e, m, index, symbols, replaced }.walk(&mut out);
    }
    out.retain(|(_, undecided)| !(*undecided && truncated));
    out
}

/// Pass 18, `interpolatedStringPost` (`Frontend.hs:241-271`), at each
/// module's own text: each hole's expression, by the type the loop gave it,
/// and its alignment, as `evaluateConstexpr` left it.
///
/// ```haskell
/// validateAlignment x | isIntLiteral x = x
///                     | otherwise = errorMsg "Interpolated string alignment did not evaluate to an integer constant" x
/// validateFormat x Nothing | tIsValueType (typeOf x) = x
///                          | otherwise = errorMsg ("The type of interpolation expression `" ++ … ++ "` is not a value type") x
/// validateFormat x (Just Format{..})
///     | tIsInt (typeOf x) || tIsEnum (typeOf x) || tIsFloat (typeOf x) = x
///     | tIsBool (typeOf x) && formatSpecifier == Bin = x
///     | otherwise = errorMsg ("Invalid format specifier `" ++ … ++ "` for the value of type `" ++ … ++ "`") x
/// ```
///
/// A type the loop left unresolved, or an error pass 10 has reported, is
/// passed over: no verdict on a type the reference would have and this
/// tree may not. A format whose grammar fails is pass 0's parse error, and
/// the hole is not also judged under a specifier it invented. Walked as
/// pass 10 walks; a copy is not judged here yet.
fn interpolated_strings(e: &Elaborated, interner: &Interner) -> Vec<Diagnostic> {
    let closure = Ty::closure_template_path(interner);
    let mut out = Vec::new();
    for m in &e.modules {
        let ir = &m.ir;
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let node = &ir.arena[id];
            if !e.expansions.copy_frames(node.src.via).is_empty() || abbreviated(ir, id) {
                continue;
            }
            match &node.exp {
                Exp::Template { .. } | Exp::StaticIf { .. } => continue,
                Exp::Interpolated(segments) => {
                    for hole in segments.iter().filter_map(|s| s.hole.as_ref()) {
                        let at = ir.arena[hole.expr].src;
                        let ty = m.types.get(hole.expr);
                        if at.written == kg_base::Written::Own && ty.resolved() && !ty.is_error() {
                            let pretty = || kg_ty::subtype::show_pretty(&ty, Some(interner));
                            let message = match hole.format.as_deref().map(format_specifier) {
                                None if !ty.is_value_type(&closure) => {
                                    Some(format!("The type of interpolation expression `{}` is not a value type", pretty()))
                                }
                                Some(Some(spec)) if !(ty.is_int() || ty.is_enum() || ty.is_float() || (ty.is_bool() && spec == "b")) => {
                                    Some(format!("Invalid format specifier `{spec}` for the value of type `{}`", pretty()))
                                }
                                _ => None,
                            };
                            if let Some(message) = message {
                                out.push(Diagnostic::error(at.head, message));
                            }
                        }
                        if let Some(align) = hole.align {
                            let a = &ir.arena[align];
                            if a.src.written == kg_base::Written::Own && !matches!(a.exp, Exp::IntLiteral { .. }) {
                                out.push(Diagnostic::error(a.src.head, "Interpolated string alignment did not evaluate to an integer constant"));
                            }
                        }
                    }
                }
                _ => {}
            }
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().rev().copied());
        }
    }
    out
}

/// `FormatSpecifier` as `show` spells it (`Parser/Syntax.hs:241-246`), read
/// off a hole's format text as `format` reads it (`Parser.hs:900-903`):
/// the whitespace `colon` leaves, then one of `b o d x X`. `None` where
/// that grammar fails — pass 0's parse error.
fn format_specifier(format: &str) -> Option<&'static str> {
    match format.trim_start().chars().next()? {
        'b' => Some("b"),
        'o' => Some("o"),
        'd' => Some("d"),
        'x' => Some("x"),
        'X' => Some("X"),
        _ => None,
    }
}

/// Pass 14's verdicts, by who admits them ([`Checked`]).
struct ExternVerdicts {
    /// In a copy, framed by its instantiation sites.
    copies: Vec<(Expansion, Diagnostic)>,
    /// At a module's own text, in program order, once per position.
    text: Vec<Diagnostic>,
}

/// Pass 14, `validateExtern` (`Frontend.hs:304-313`): `checkExtern` keeps an
/// `extern` whose name `lookupSymbol` finds as a `Class` or a `Template _
/// Class`, and refuses any other — a failed lookup included — at the name.
///
/// `desugar` reaches every node, so a template's own body is judged and a
/// `static if` left standing in one in both branches: `removeTemplate`, the
/// pass after, keeps a template's diagnostics (`Frontend.hs:299`).
/// `kg-oracle --errors`, 2026-10-10: `extern S;` of a struct declared in an
/// uninstantiated class template's body is refused there; so is `extern T;`
/// of the template's own parameter, whatever the argument, since no map
/// files a parameter (`Symbols.hs:52-68`) and the failed lookup is the
/// catch-all's; and inside a copy the verdict is the copy's too, framed.
/// At the text outside a template a failed lookup cannot reach here: the
/// name was pass 9's, which stops the program first (`Frontend.hs:86-87`).
///
/// The reference notes a copy's verdict at the parameter's use in the body
/// (`extern T;` under `Foo<S>`), which a substituted type parameter's node
/// wears in the copy. A name whose node is not the copy's own — one that
/// kept an argument's `Src` unframed — is not reported there: its range is
/// the argument's, not the body's, and the body's own verdict stands.
fn validate_extern(e: &Elaborated, symbols: &crate::symbols::Symbols) -> ExternVerdicts {
    const MESSAGE: &str = "Invalid extern declaration. Only classes and class templates can be declared as external.";
    let expansions: &ExpansionInterner = &e.expansions;
    let mut out = ExternVerdicts { copies: Vec::new(), text: Vec::new() };
    for m in &e.modules {
        let ir = &m.ir;
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let node = &ir.arena[id];
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().rev().copied());
            let Exp::Extern { ty, .. } = node.exp else { continue };
            if extern_names_a_class(e, symbols, ir, ty) {
                continue;
            }
            let at = ir.arena[ty].src;
            if expansions.copy_frames(node.src.via).is_empty() {
                let d = Diagnostic::error(at.head, MESSAGE);
                if !out.text.iter().any(|x| x.range == d.range) {
                    out.text.push(d);
                }
            } else if at.via == node.src.via {
                out.copies.push((at.via, Diagnostic::error(at.head, MESSAGE)));
            }
        }
    }
    out
}

/// `checkExtern`'s two accepting cases for the name at `ty`: `lookupSymbol`
/// (`Symbols.hs:79-87`) from the name's own scope finds a `Class`, or a
/// `Template` declaring one. A name of another shape is passed over — the
/// parser gives `extern` a type identifier or a template's
/// (`Parser.hs:280`).
fn extern_names_a_class(e: &Elaborated, symbols: &crate::symbols::Symbols, ir: &ModuleIr, ty: ExpId) -> bool {
    let arena = &ir.arena;
    let mut at = ty;
    let scope = loop {
        match &arena[at].exp {
            Exp::TypeIdentifier(_, inner) | Exp::QualifiedIdentifier(inner) => at = *inner,
            Exp::TemplateInstance { base, .. } => at = *base,
            // A template's parameter, as the parser resolved its use
            // (`getScope (TypeParamF n _ _ _) = n`, `Parser/Syntax.hs:772`):
            // filed nowhere, so found only if an enclosing scope declares
            // its name.
            Exp::ScopedName { scope, .. } | Exp::TypeParam { scope, .. } => break scope,
            _ => return true,
        }
    };
    let name = kg_hir::sort::spine_qualname(arena, at);
    let Some(d) = symbols.in_scope(scope.segments(), &name) else { return false };
    let home = &e.modules[d.module].ir.arena;
    match &home[d.id].exp {
        Exp::Class { .. } => true,
        Exp::Template { decl, .. } => matches!(home[*decl].exp, Exp::Class { .. }),
        _ => false,
    }
}

/// Whether `id`'s subtree holds an `Exp::Error`.
fn has_error(ir: &ModuleIr, id: ExpId) -> bool {
    let mut stack = vec![id];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        if matches!(ir.arena[id].exp, Exp::Error(_)) {
            return true;
        }
        kids.clear();
        ir.arena[id].exp.children(&mut kids);
        stack.extend(kids.iter().copied());
    }
    false
}

/// Pass 2's verdicts ([`TextPass::HigherOrderFunctions`]): `functionTemplate
/// _ = desugarError "Non-inline higher order functions are not supported"`
/// (`Function.hs:322`) at each function the pass refused, in program order —
/// where the error node is still in the final tree, which is where the
/// reference collects errors from: a refused member of a `static if` branch
/// pass 6 deleted is gone with it, and says nothing (`kg-oracle --errors`,
/// 2026-10-10), while a template's body keeps its own (`removeTemplate`,
/// `Frontend.hs:298-299`). The loop leaves a refused function as written,
/// so it stands in the final tree exactly where the error would.
fn higher_order_functions(e: &Elaborated) -> Vec<Diagnostic> {
    if e.higher_order.is_empty() {
        return Vec::new();
    }
    let mut standing = kg_base::FxHashSet::default();
    let mut kids = Vec::new();
    for m in &e.modules {
        let ir = &m.ir;
        let mut stack = vec![ir.root];
        while let Some(id) = stack.pop() {
            let node = &ir.arena[id];
            if matches!(node.exp, Exp::Function { .. }) && e.expansions.copy_frames(node.src.via).is_empty() {
                standing.insert(node.src.head);
            }
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
    }
    e.higher_order
        .iter()
        .filter(|at| standing.contains(*at))
        .map(|&at| Diagnostic::error(at, "Non-inline higher order functions are not supported"))
        .collect()
}

/// Pass 6's verdicts at every module's own text ([`TextPass::InferType`]):
/// each error the fold left on a node the text wrote — outside every copy,
/// and not a round's conflicting deduction, which is raised wherever it
/// stands — where inference reaches, a template's parameters included
/// (`TemplateF (inferExpTree' alg <$> a) b`, `Inference.hs:63`) and its body
/// never. In program order, each at the node the error replaced.
fn fold_errors(e: &Elaborated) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut kids = Vec::new();
    for m in &e.modules {
        let ir = &m.ir;
        let mut stack = vec![ir.root];
        while let Some(id) = stack.pop() {
            let node = &ir.arena[id];
            kids.clear();
            match &node.exp {
                Exp::Template { params, .. } => kids.extend(params.iter().copied()),
                Exp::Error(note) => {
                    let written =
                        node.src.written == kg_base::Written::Own && e.expansions.copy_frames(node.src.via).is_empty();
                    if written && !note.is(DiagnosticKind::ConflictingDeduction) {
                        let d = Diagnostic::error(node.src.head, note.message.to_string()).with_kind(note.kind);
                        // A node two parents share is one verdict.
                        if !out.iter().any(|x: &Diagnostic| x.range == d.range && x.message == d.message) {
                            out.push(d);
                        }
                    }
                }
                exp => exp.children(&mut kids),
            }
            stack.extend(kids.iter().rev().copied());
        }
    }
    out
}

/// Every `Exp::Error` reachable outside a template's own body.
fn errors_in(ir: &ModuleIr) -> Vec<ExpId> {
    let mut out = Vec::new();
    let mut stack = vec![ir.root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        match &ir.arena[id].exp {
            Exp::Template { .. } => continue,
            Exp::Error(_) => out.push(id),
            _ => {}
        }
        kids.clear();
        ir.arena[id].exp.children(&mut kids);
        stack.extend(kids.iter().copied());
    }
    out
}

fn is_closure(t: &Ty) -> bool {
    match t {
        Ty::Closure { .. } => true,
        Ty::Instance { concrete, .. } | Ty::Const(concrete) | Ty::Type(concrete) => is_closure(concrete),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    fn diagnostics(text: &str) -> Vec<(String, String, Vec<String>)> {
        diagnostics_leftovers(text, false)
    }

    fn diagnostics_leftovers(text: &str, leftovers: bool) -> Vec<(String, String, Vec<String>)> {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let e = crate::elaborate::elaborate(vec![ir], &interner);
        let at = |r: kg_base::FileRange| text[r.range].to_owned();
        super::diagnostics(&e, &interner, leftovers)
            .into_iter()
            .filter(|d| d.is_error())
            .map(|d| (d.message.clone(), at(d.range), d.labels.iter().map(|l| format!("{}: {}", l.message, at(l.range))).collect()))
            .collect()
    }

    /// Pass 19's conversions at the module's own text, as the message and
    /// the text each is at.
    fn conversions(text: &str) -> Vec<(String, String)> {
        text_of(text, super::TextPass::CheckConversion).into_iter().map(|(m, at, _)| (m, at)).collect()
    }

    /// `pass`'s verdicts at the module's own text ([`super::text_verdicts`]),
    /// as the message and the text each is at, with its line.
    fn text_of(text: &str, pass: super::TextPass) -> Vec<(String, String, usize)> {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let e = crate::elaborate::elaborate(vec![ir], &interner);
        let line = |r: kg_base::FileRange| text[..usize::from(r.range.start())].matches('\n').count() + 1;
        super::text_verdicts(&e, &interner, pass)
            .into_iter()
            .map(|d| (d.message.clone(), text[d.range.range].to_owned(), line(d.range)))
            .collect()
    }

    /// `cata typeErrors` (`Frontend.hs:169-174`) at the module's text, over
    /// the loop's types: one report per ill-typed subexpression, at the
    /// innermost node (`!a + 1`, not each `!` above it); a shift by a
    /// constant the loop folds takes `tShiftLLiteral` (`Type.hs:770-771`)
    /// and is fine, by a 32-bit variable it is not (`:894-897`); a negative
    /// denominator is the reference's second error after pass 17's
    /// (`auto-placeholder.k::33`, `kg-oracle --errors` 2026-10-10), which
    /// the CST pass missed. A template's own body is not typed, and a
    /// copy's error is not this pass's yet. Red with the subtree's errors
    /// not keeping a node quiet.
    #[test]
    fn a_type_error_is_one_report_at_the_innermost_node() {
        let at = |text: &str| -> Vec<(String, String)> {
            text_of(text, super::TextPass::TypeErrors).into_iter().map(|(m, at, _)| (m, at)).collect()
        };
        let one = at("inline void main(bool a)\n{\n    bool b = !(!(!a + 1));\n}\n");
        assert_eq!(one.len(), 1, "{one:?}");
        assert_eq!(one[0].1, "!a + 1");
        let shift = |by: &str| at(&format!("{by}\ninline void main()\n{{\n    uint32 x;\n    auto z = x << s;\n}}\n"));
        assert_eq!(shift("const uint32 s = bitsizeof(uint8);"), vec![]);
        let variable = shift("uint32 s;");
        assert_eq!(variable.len(), 1, "{variable:?}");
        assert_eq!(variable[0].1, "x << s");
        let negative = at("inline void main()\n{\n    static uint32 x;\n    const auto y = x / -1;\n}\n");
        assert_eq!(negative, vec![("Division with negative denominator is undefined".to_owned(), "x / -1".to_owned())]);
        let body = "template <typename T>\ninline void f(T x)\n{\n    auto y = x + 1;\n    bool b;\n    auto c = b + 1;\n}\n";
        assert_eq!(at(body), vec![]);
        assert_eq!(at(&format!("{body}\ninline void main()\n{{\n    f<bool>(true);\n}}\n")), vec![]);
    }

    /// `deduceAuto` (`Type.hs:1595-1610`) at the module's text, each
    /// verdict with the text it is at, its line, and whether it waits on
    /// the program's flag (`TextVerdict::undecided`) — every position
    /// `kg-oracle --errors`' (2026-10-10, the same file): `auto x;`
    /// (nothing to deduce from, whatever the program), `{1, 2}` for a
    /// variable and for a return (`TInitializer`, never written), a
    /// `static for` over a parameter at the `auto` past the `const`
    /// (`uninitConst`, `Parser.hs:451`; no fold gives it), a negative
    /// denominator at the `const auto` (`TError`), and none for a negative
    /// bound — pass 10 replaced that variable (`basics.k::2`) — nor for
    /// `inline const auto`, pass 19's; `const auto X = X;` waits
    /// (`constexpr.k::0`). Under a latency bound the one that waits is not
    /// given. Red with pass 10's replacements judged, with the `for`
    /// variable at its `const`, with the classes swapped, and with either
    /// syntactic certainty dropped.
    #[test]
    fn deduce_auto_reports_at_the_auto_and_says_which_wait() {
        let text = "inline void f1()\n{\n    auto x;\n}\n\ninline void f2()\n{\n    const auto y = {1, 2};\n}\n\n\
                    inline auto f3()\n{\n    return {1, 2};\n}\n\ninline void f4(uint32 n)\n{\n    static for (const auto i : n)\n    {\n    }\n}\n\n\
                    inline void f5()\n{\n    static for (const auto i : -3)\n    {\n    }\n}\n\n\
                    inline void f6()\n{\n    static uint32 x;\n    const auto z = x / -1;\n}\n\ninline const auto f7()\n{\n    return 1;\n}\n\nconst auto X = X;\n";
        let verdicts = |truncated: bool| -> Vec<(String, String, usize, bool)> {
            let interner = Interner::new();
            let parse = kg_syntax::parse_file(text);
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
            let e = crate::elaborate::elaborate(vec![ir], &interner);
            let line = |r: kg_base::FileRange| text[..usize::from(r.range.start())].matches('\n').count() + 1;
            super::checked(&e, &interner, true, truncated, &|_| None)
                .text
                .into_iter()
                .filter(|v| v.pass == super::TextPass::DeduceAuto)
                .map(|v| (v.diagnostic.message.clone(), text[v.diagnostic.range.range].to_owned(), line(v.diagnostic.range), v.undecided))
                .collect()
        };
        let var = |at: &str, line: usize, undecided: bool| ("Could not deduce variable type".to_owned(), at.to_owned(), line, undecided);
        let ret = ("Could not deduce function return type".to_owned(), "auto".to_owned(), 11, false);
        let decided = [var("auto", 3, false), var("const auto", 8, false), ret, var("auto", 18, false), var("const auto", 33, false)];
        let mut all = decided.to_vec();
        all.push(var("const auto", 41, true));
        assert_eq!(verdicts(false), all);
        assert_eq!(verdicts(true), decided);
    }

    /// `postDesugar` (`Frontend.hs:176-183,393-407`) at the module's text,
    /// each verdict with the text it is at, its line, and whether it waits
    /// on the program's flag — every one `kg-oracle --errors`' (2026-10-10,
    /// each shape alone): a `static assert` over a runtime `bool`, at the
    /// expression; `uint<0>`, at the type; `int<clog2(0)>`, the width's
    /// error alone (the `cata` replaced the `clog2` inside first); a
    /// `clog2(0)` value; a `static if` over a parameter, at the statement;
    /// all settled by their syntax. One over a `const auto X = X;` waits,
    /// and is not given under a latency bound; a negative shift count,
    /// on which the reference crashes, is none. Red with the `cata`'s
    /// replacement keeping what is inside, with the classes swapped, and
    /// with the crashing shift judged.
    #[test]
    fn post_desugar_reports_what_the_fold_left_standing() {
        let text = "const auto X = X;\n\ninline void f(bool b, uint32 x)\n{\n    static assert(b);\n    uint<0> u;\n    int<clog2(0)> i;\n    \
                    uint32 z = clog2(0);\n    static if (x == 1)\n    {\n    }\n    static assert(X == 1);\n    static assert((3 << -1) == 1);\n}\n";
        let verdicts = |truncated: bool| -> Vec<(String, String, usize, bool)> {
            let interner = Interner::new();
            let parse = kg_syntax::parse_file(text);
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
            let e = crate::elaborate::elaborate(vec![ir], &interner);
            let line = |r: kg_base::FileRange| text[..usize::from(r.range.start())].matches('\n').count() + 1;
            super::checked(&e, &interner, true, truncated, &|_| None)
                .text
                .into_iter()
                .filter(|v| v.pass == super::TextPass::PostDesugar)
                .map(|v| {
                    let at = text[v.diagnostic.range.range].trim().lines().next().unwrap_or_default().to_owned();
                    (v.diagnostic.message.clone(), at, line(v.diagnostic.range), v.undecided)
                })
                .collect()
        };
        let v = |message: &str, at: &str, line: usize, undecided: bool| (message.to_owned(), at.to_owned(), line, undecided);
        let not_boolean = "static assertion expression did not evaluate to a boolean constant";
        let decided = vec![
            v(not_boolean, "b", 5, false),
            v("uint width did not evaluate to a positive integer constant", "uint<0>", 6, false),
            v("int width did not evaluate to a positive integer constant", "int<clog2(0)>", 7, false),
            v("clog2 argument did not evaluate to a positive integer constant", "clog2(0)", 8, false),
            v("static if predicate did not evaluate to a boolean constant", "static if (x == 1)", 9, false),
        ];
        let mut all = decided.clone();
        all.push(v(not_boolean, "X == 1", 12, true));
        assert_eq!(verdicts(false), all);
        assert_eq!(verdicts(true), decided);
    }

    /// `validateExportableClasses` (`Frontend.hs:337-391`) at the module's
    /// text, each verdict with its notes' lines — every case `kg-oracle
    /// --errors`' (2026-10-10): a public field and a public inline method,
    /// noted at the `extern`; a class two declarations export, noted at
    /// both in order; a class an exported class holds as a public object,
    /// noted at the object, then at its own `extern`, then at its holder's
    /// (`Src`'s `<>`, `Parser/Syntax.hs:623-626`), and the holder's object
    /// itself; an exported type template, at the `export`. A prototype, a
    /// private member and an unexported class are fine. Red with the second
    /// export's note dropped, with `<>`'s second rule reordered, and with
    /// public objects not followed.
    #[test]
    fn an_exportable_class_is_restricted_with_what_made_it_so() {
        let at = |text: &str| -> Vec<(String, usize, Vec<usize>)> {
            let interner = Interner::new();
            let parse = kg_syntax::parse_file(text);
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
            let e = crate::elaborate::elaborate(vec![ir], &interner);
            let line = |r: kg_base::FileRange| text[..usize::from(r.range.start())].matches('\n').count() + 1;
            super::text_verdicts(&e, &interner, super::TextPass::ExportableClasses)
                .into_iter()
                .map(|d| (d.message.replace(" is not allowed in an exportable class", ""), line(d.range), d.labels.iter().map(|l| line(l.range)).collect()))
                .collect()
        };
        let one = |m: &str, l: usize, notes: &[usize]| (m.to_owned(), l, notes.to_vec());
        let members = "class Foo\n{\npublic:\n    uint32 x;\n    inline void Do()\n    {\n    }\n    void Go();\nprivate:\n    uint32 y;\n}\n\nextern Foo;\n";
        assert_eq!(at(members), [one("Public field", 4, &[13]), one("Public inline method", 5, &[13])]);
        let twice = "class Foo\n{\npublic:\n    uint32 x;\n}\n\nexport Foo;\n\nextern Foo;\n";
        assert_eq!(at(twice), [one("Public field", 4, &[7, 9])]);
        let held = "class Foo\n{\npublic:\n    uint32 x;\n}\n\nclass Bar\n{\npublic:\n    Foo f;\n}\n\nextern Bar;\n\nextern Foo;\n";
        assert_eq!(at(held), [one("Public field", 4, &[10, 15, 13]), one("Public field", 10, &[13])]);
        let template = "template <typename T>\nclass Foo\n{\n}\n\nexport Foo;\n";
        let refused = "Type template cannot be exported. Perhaps you meant to export an instance of the template?";
        assert_eq!(at(template), [one(refused, 6, &[])]);
        assert_eq!(at("class Foo\n{\npublic:\n    uint32 x;\n}\n"), []);
    }

    /// `interpolatedStringPost` (`Frontend.hs:241-271`) at the module's
    /// text, by the loop's types and folds — each verdict `kg-oracle
    /// --errors`' on this program (2026-10-10): `:x` on a `bool`; a class
    /// instance with no format; an alignment that calls a function, and one
    /// that reads a variable, neither of which folds. `:b` on a `bool`, `:x`
    /// on an integer and an alignment a `const` folds are fine; an
    /// initializer list is no value type, spelled as `showPretty` spells it.
    /// Red with the format left unread, with a folded alignment refused,
    /// and with value types unchecked.
    #[test]
    fn an_interpolations_type_format_and_alignment_are_judged() {
        let text = "class C\n{\n}\n\ninline uint8 f()\n{\n    return 3;\n}\n\nconst uint8 c = 4;\n\ninline void main()\n{\n    static C k;\n    bool b = true;\n    uint8 x = 1;\n    const auto s1 = \"{b:x}\";\n    const auto s2 = \"{b:b}{x:x}{x,c}\";\n    const auto s3 = \"{x,f()}{k}{x,x}{ {1, 0} }\";\n}\n";
        let out: Vec<(String, String, usize)> = text_of(text, super::TextPass::InterpolatedStringPost);
        let line = |l: usize, at: &str, m: &str| (m.to_owned(), at.to_owned(), l);
        assert_eq!(
            out,
            [
                line(17, "b", "Invalid format specifier `x` for the value of type `bool`"),
                line(19, "f()", "Interpolated string alignment did not evaluate to an integer constant"),
                line(19, "k", "The type of interpolation expression `C` is not a value type"),
                line(19, "x", "Interpolated string alignment did not evaluate to an integer constant"),
                line(19, "{1, 0}", "The type of interpolation expression `{uint1, uint1}` is not a value type"),
            ]
        );
    }

    const EXTERN: &str = "Invalid extern declaration. Only classes and class templates can be declared as external.";

    /// `validateExtern` (`Frontend.hs:304-313`) at the module's own text,
    /// wherever `desugar` reaches — each case `kg-oracle --errors`'
    /// (2026-10-10): a struct declared in an uninstantiated class
    /// template's body; inside `static if (true)`, and in a branch of one a
    /// template's body leaves standing; a struct a class or a function
    /// declares; an alias, a struct template and its instance; a
    /// template's own parameter, whatever its argument. A dead branch says
    /// nothing, nor a class, a class template or its instance. Red with
    /// the template's body unwalked, with its parameter unlooked-up, and
    /// with `Template _ Class{}` refused.
    #[test]
    fn an_extern_is_judged_wherever_desugar_reaches() {
        let at = |text: &str| -> Vec<(String, usize)> {
            text_of(text, super::TextPass::ValidateExtern)
                .into_iter()
                .inspect(|(m, _, _)| assert_eq!(m, EXTERN))
                .map(|(_, at, line)| (at, line))
                .collect()
        };
        let s = "struct S\n{\n    uint8 x;\n}\n\n";
        let pair = |a: &str, line: usize| vec![(a.to_owned(), line)];
        assert_eq!(at("template <typename T>\nclass Foo\n{\n    struct S\n    {\n        T x;\n    }\n\n    extern S;\n}\n"), pair("S", 9));
        assert_eq!(at(&format!("{s}static if (true)\n{{\n    extern S;\n}}\n")), pair("S", 8));
        assert_eq!(at(&format!("{s}static if (false)\n{{\n    extern S;\n}}\n")), vec![]);
        let standing = "template <auto N>\nclass Foo\n{\n    struct S\n    {\n        uint8 x;\n    }\n\n    static if (N == 1)\n    {\n        extern S;\n    }\n}\n";
        assert_eq!(at(standing), pair("S", 11));
        let nested = "class C\n{\n    struct S\n    {\n        uint8 x;\n    }\n\n    extern S;\n}\n\ninline void main()\n{\n    struct T\n    {\n        uint8 y;\n    }\n\n    extern T;\n}\n";
        assert_eq!(at(nested), vec![("S".to_owned(), 8), ("T".to_owned(), 18)]);
        let kinds = "class C\n{\n}\n\nusing A = C;\n\nextern A;\n\ntemplate <typename T>\nclass K\n{\n}\n\nextern C;\nextern K;\nextern K<uint8>;\n";
        assert_eq!(at(kinds), pair("A", 7));
        let template = "template <typename T>\nstruct K\n{\n    T x;\n}\n\n";
        assert_eq!(at(&format!("{template}extern K;\n")), pair("K", 7));
        assert_eq!(at(&format!("{template}extern K<uint8>;\n")), pair("K<uint8>", 7));
        let parameter = "class C\n{\n}\n\ntemplate <typename T>\nclass Foo\n{\n    extern T;\n}\n\ninline void main()\n{\n    static Foo<C> f;\n}\n";
        assert_eq!(at(parameter), pair("T", 8));
    }

    /// Inside a copy the verdict is the copy's, framed by its
    /// instantiation, beside the body's own at the template's text —
    /// `kg-oracle --errors` gives both (2026-10-10), for a struct the body
    /// declares and for the parameter's argument. Red with copies passed
    /// over.
    #[test]
    fn an_extern_in_a_copy_is_framed_by_its_instantiation() {
        let declared = "template <typename T>\nclass Foo\n{\n    struct S\n    {\n        T x;\n    }\n\n    extern S;\n}\n\ninline void main()\n{\n    static Foo<uint8> f;\n}\n";
        assert_eq!(diagnostics(declared), vec![(EXTERN.to_owned(), "S".to_owned(), vec!["while compiling: Foo<uint8>".to_owned()])]);
        let argument = "struct S\n{\n    uint8 x;\n}\n\ntemplate <typename T>\nclass Foo\n{\n    extern T;\n}\n\ninline void main()\n{\n    static Foo<S> f;\n}\n";
        assert_eq!(diagnostics(argument), vec![(EXTERN.to_owned(), "T".to_owned(), vec!["while compiling: Foo<S>".to_owned()])]);
        assert_eq!(text_of(argument, super::TextPass::ValidateExtern).len(), 1);
    }

    /// `checkConversion` in a copy's body (`TypeCheck.hs:178`), reported at
    /// the template body's text with the instantiation site as a `while
    /// compiling` note (`expandErrorStack`, `Error.hs:176-195`). Red with the
    /// initializer site unchecked.
    #[test]
    fn a_conversion_in_a_copy_is_reported_at_the_body_with_its_site() {
        let text = "template <typename T>\ninline void f(T x)\n{\n    bool b = x;\n}\n\ninline void g()\n{\n    f<uint8>(3);\n}\n";
        assert_eq!(
            diagnostics(text),
            vec![("Invalid conversion from `uint8` to `bool`".to_owned(), "x".to_owned(), vec!["while compiling: f<uint8>".to_owned()])]
        );
    }

    /// `argError` (`TypeCheck.hs:232`) at the refused argument, and nothing
    /// after it: `instantiateTemplates` stops with the flag set
    /// (`Template.hs:55-56`) and pass 19 never runs (`Frontend.hs:44-46`) —
    /// `f<uint8>`'s body conversion goes unsaid. Red with the gate removed.
    #[test]
    fn a_refused_argument_is_the_programs_last_word() {
        let text = "template <bool B>\nstruct Flag\n{\n}\n\ntemplate <typename T>\ninline void f(T x)\n{\n    bool b = x;\n}\n\ninline void g()\n{\n    Flag<5> a;\n    f<uint8>(3);\n}\n";
        assert_eq!(
            diagnostics(text),
            vec![(
                "Invalid argument for template parameter `B`.\nExpected a constant expression of type:\n\tbool\nbut the argument type is:\n\tuint3".to_owned(),
                "5".to_owned(),
                Vec::new()
            )]
        );
    }

    /// An integer literal for an enum parameter is no re-stamp — `tIsUnsigned`
    /// sees through `const` and an instance, never an enum (`Type.hs:
    /// 599-605,656-657`) — so `validateType` asks `uint1 <: E` and refuses
    /// (`enum.k::16`). Red with an enum kind let through as an integer.
    #[test]
    fn an_integer_for_an_enum_parameter_is_refused() {
        let text = "enum E : uint8\n{\n    A,\n    B\n}\n\ntemplate <E V>\nstruct En\n{\n}\n\ninline void g()\n{\n    En<1> a;\n}\n";
        let out = diagnostics(text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].0.starts_with("Invalid argument for template parameter `V`.\nExpected a constant expression of type:\n\tE\n"), "{out:?}");
        assert_eq!(out[0].1, "1");
    }

    /// `staticAssert` (`Frontend.hs:393-398`) over the condition the
    /// engine folded in a copy: "failed" at the statement, framed by the
    /// instantiation. Red with the `StaticAssert` arm unasked.
    #[test]
    fn a_failed_static_assert_in_a_copy_is_reported_at_the_statement() {
        let text = "template <auto N>\ninline void f()\n{\n    static assert(N == 3);\n}\n\ninline void g()\n{\n    f<2>();\n}\n";
        assert_eq!(
            diagnostics(text),
            vec![("static assertion failed".to_owned(), "static assert(N == 3);".to_owned(), vec!["while compiling: f<2>".to_owned()])]
        );
    }

    /// `reconcile`'s conflict (`Type.hs:1502-1506`) is the round's error at
    /// the call, its candidates spelled by `pretty` (`Type.hs:217-222`):
    /// `3` then `2`, new first — `kg-oracle --errors` on this program,
    /// 2026-09-26 — not a debug rendering. Red with the tree's `Exp::Error`
    /// nodes unread.
    #[test]
    fn a_conflicting_deduction_is_the_rounds_error_at_the_call() {
        let text = "template <auto N>\ninline void Foo(uint8[N] x, uint8[N] y)\n{\n}\n\ninline void g()\n{\n    uint8[2] x;\n    uint8[3] y;\n    Foo(x, y);\n}\n";
        let out = diagnostics(text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Conflicting template argument deduction results for parameter `N`. Could be `3` or `2`.");
        assert!(out[0].1.starts_with("Foo"), "{out:?}");
    }

    /// The conflict is known by its kind, not its text
    /// (`DiagnosticKind::ConflictingDeduction`, sprint W8.1): set where
    /// `reconcile` makes it, carried by the deduction into the tree's
    /// `Exp::Error` and from there onto the diagnostic. Red with the kind
    /// left off at `reconcile`, and with `arg` dropping it.
    #[test]
    fn a_conflicting_deduction_carries_its_kind_from_reconcile() {
        let text = "template <auto N>\ninline void Foo(uint8[N] x, uint8[N] y)\n{\n}\n\ninline void g()\n{\n    uint8[2] x;\n    uint8[3] y;\n    Foo(x, y);\n}\n";
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let e = crate::elaborate::elaborate(vec![ir], &interner);
        let kinds: Vec<_> = super::diagnostics(&e, &interner, false).into_iter().map(|d| d.kind).collect();
        assert_eq!(kinds, [Some(kg_diag::DiagnosticKind::ConflictingDeduction)]);
    }

    /// A copy is typed `TInstance` (`setTypeOfInstance`, `Type.hs:426-440`),
    /// so an initializer under one is judged element by element and
    /// spelled as `showUnmangled` spells it (`initializer-list.k::50`) —
    /// both elements, as `kg-oracle --errors` reports them (2026-09-26).
    /// The declaration is at the module's text, so the verdicts are pass
    /// 19's there (`TextPass::CheckConversion`), raised among no copy's.
    /// Red with the CST pass's opaque-instance silence restored.
    #[test]
    fn an_initializer_under_a_struct_copy_is_judged() {
        let text = "template <typename T, typename U>\nstruct Bux\n{\n    T a;\n    U b;\n}\n\ninline void g()\n{\n    Bux<bool, uint32> x = {10, true};\n}\n";
        assert_eq!(
            conversions(text),
            vec![
                ("Invalid conversion from `uint4` to `bool`".to_owned(), "10".to_owned()),
                ("Invalid conversion from `bool` to `uint32`".to_owned(), "true".to_owned()),
            ]
        );
        assert_eq!(diagnostics(text), Vec::new());
    }

    /// `checkEnumConsts` (`TypeCheck.hs:203-215`) over a copy's enum: a
    /// `uint2` base takes `[0, 4)`, so `B = 5` is out of range at the
    /// value; and `checkEnumBaseType` (`:199-201`) refuses a `bool` base.
    /// Red with the `Enum` arm unasked.
    #[test]
    fn a_copys_enum_constants_are_checked_against_its_base() {
        let text = "template <typename T>\nclass C\n{\n    enum E : T\n    {\n        A = 1,\n        B = 5\n    }\n}\n\ninline void g()\n{\n    static C<uint2> c;\n}\n";
        let out = diagnostics(text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!((out[0].0.as_str(), out[0].1.as_str()), ("Enum constant value is out of range for the base type", "5"));
        // The module's own enum is pass 19's at its text
        // (`TextPass::ValidateProgram`), never raised among the copies'.
        let text = "enum E : bool\n{\n    A\n}\n";
        assert_eq!(diagnostics(text), Vec::new());
        let out = text_of(text, super::TextPass::ValidateProgram);
        assert_eq!(out.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["Enums must be based on an integral type"], "{out:?}");
    }

    /// Pass 9 inside a copy (`Frontend.hs:82-157`): an undefined name is
    /// reported at its identifier, with the one name within the threshold
    /// the edit distance finds in scope (`hints`, `:110-155`), framed by the
    /// instantiation; and it sets the flag, so pass 19's conversion of the
    /// same body goes unsaid (`Frontend.hs:86-87`) — `kg-oracle --errors`
    /// on this program, 2026-09-26. Red with pass 9 skipped.
    #[test]
    fn an_undefined_name_in_a_copy_is_reported_with_its_hint() {
        let text = "const uint32 value = 3;\n\ntemplate <typename T>\ninline void f(T x)\n{\n    bool b = x;\n    uint32 y = valeu;\n}\n\ninline void g()\n{\n    f<uint8>(1);\n}\n";
        assert_eq!(
            diagnostics(text),
            vec![(
                "Symbol name not in scope: `valeu`\nPerhaps you meant: `value`".to_owned(),
                "valeu".to_owned(),
                vec!["while compiling: f<uint8>".to_owned()]
            )]
        );
    }

    /// Pass 12 inside a copy (`Frontend.hs:409-432`): an instance written
    /// with more arguments than its template takes is never decided, and is
    /// reported at the instance with the template as its context; and a
    /// template no scope declares is "Undefined template". The message,
    /// position and both notes are `kg-oracle --errors`' on this program
    /// (2026-09-26). Red with pass 12 unasked.
    #[test]
    fn a_copys_leftover_instances_are_judged() {
        let text = "template <typename A>\nstruct One\n{\n    A a;\n}\n\ntemplate <typename T>\ninline void f(T x)\n{\n    One<T, T> y;\n}\n\ninline void g()\n{\n    f<uint8>(1);\n}\n";
        let out = diagnostics(text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Too many arguments specified for template `One`");
        assert!(out[0].1.starts_with("One<T, T>"), "{out:?}");
        // The template's declaration first, then the instantiation — the
        // reference's two notes, in its order.
        assert_eq!(out[0].2.len(), 2, "{out:?}");
        assert!(out[0].2[0].starts_with("while compiling: struct One"), "{out:?}");
        assert!(out[0].2[1].starts_with("while compiling: f<uint8>"), "{out:?}");
    }

    /// Each of these is `kg-oracle --errors` on the same program
    /// (2026-09-26, 2026-09-27). A `static if` left standing in a copy — its
    /// predicate a runtime parameter, or a local not declared `const` — is
    /// `unresolved`'s error at the `static if`, framed by the call
    /// (`Frontend.hs:406`), asked or not: no round can fold it. An absorbing
    /// operator folds the read away, and a `const` local folds: both
    /// compile. Corrected 2026-09-27: this asserted silence unasked, which
    /// was the gate's caution, not the reference's answer. Red with the arm
    /// removed, or with [`super::Cx::certainly_runtime`] cut.
    #[test]
    fn a_static_if_left_standing_in_a_copy_is_no_constant() {
        let foo = |body: &str| {
            format!("template <typename T>\ninline void Foo(const T x)\n{{\n{body}    {{\n    }}\n}}\n\ninline void main()\n{{\n    Foo(10);\n}}\n")
        };
        for body in ["    static if (x != 0)\n", "    uint32 y = 3;\n    static if (y != 0)\n"] {
            let text = foo(body);
            for out in [diagnostics(&text), diagnostics_leftovers(&text, true)] {
                assert_eq!(out.len(), 1, "{out:?}");
                assert_eq!(out[0].0, "static if predicate did not evaluate to a boolean constant");
                assert!(out[0].1.starts_with("static if ("), "{out:?}");
                assert_eq!(out[0].2, vec!["while compiling: Foo".to_owned()]);
            }
        }
        assert_eq!(diagnostics(&foo("    static if ((x != 0) && false)\n")), Vec::new());
        assert_eq!(diagnostics(&foo("    const uint32 y = 3;\n    static if (y != 0)\n")), Vec::new());
    }

    /// A `const auto` parameter makes no abbreviated template
    /// (`isAuto . paramType`, `Parser/Syntax.hs:706-707,722`), so the
    /// function is judged as written and pass 19's `check AutoF{}`
    /// (`TypeCheck.hs:51`) reports its `auto` — the reference's first error
    /// (`kg-oracle --errors`, 2026-10-10: `1:22 Could not deduce type`).
    /// Red with `const auto` read as abbreviated: the function is skipped
    /// and nothing is said.
    #[test]
    fn a_const_auto_parameter_makes_no_abbreviated_template() {
        let text = "inline uint8 F(const auto x)\n{\n    return x;\n}\n\ninline uint8 G()\n{\n    return F(3);\n}\n";
        let found: Vec<(String, String)> =
            text_of(text, super::TextPass::ValidateProgram).into_iter().map(|(m, at, _)| (m, at)).collect();
        assert_eq!(found, vec![("Could not deduce type".to_owned(), "auto".to_owned())]);
    }

    /// A member with an `auto` parameter is a template
    /// (`isAbbreviatedTemplate`), in a class's copy as anywhere: never
    /// called, its body is never judged — `f(x)` in `Foo<1>`'s `run` is
    /// deduced only in `run`'s own copy, and the program is clean
    /// (`kg-oracle --errors`, 2026-09-26; `higher-order-functions.k::120`'s
    /// shape). Red with its body walked: pass 12 calls `f` undeducible.
    #[test]
    fn an_abbreviated_member_in_a_copy_is_not_judged() {
        let text = "template <typename T>\ninline void f(T y)\n{\n}\n\ntemplate <auto N>\nclass Foo\n{\npublic:\n    inline void run(auto x)\n    {\n        f(x);\n    }\n}\n\ninline void main()\n{\n    static Foo<1> test;\n}\n";
        assert_eq!(diagnostics(text), Vec::new());
    }

    /// A method a copy's class does not have — its `static if` dropped `Do`
    /// — is "not in scope" at the method's name, on the module's own text:
    /// pass 9's verdict there ([`super::TextPass::UndefinedSymbols`]), no
    /// longer a seam among the copies'. Red with the method arm removed.
    #[test]
    fn a_method_the_copy_lacks_is_not_in_scope_at_its_name() {
        let text = "template <auto B>\nclass M\n{\npublic:\n    static if (B)\n    {\n        inline void Do()\n        {\n        }\n    }\n}\n\ninline void main()\n{\n    static M<false> obj;\n    obj.Do();\n}\n";
        assert_eq!(diagnostics(text), Vec::new());
        assert_eq!(text_of(text, super::TextPass::UndefinedSymbols), [("Symbol name not in scope: `Do`".to_owned(), "Do".to_owned(), 16)]);
    }

    /// Pass 9 at the module's own text (`Frontend.hs:82-157`): every name
    /// the text writes that `lookupInScope` does not find, a value's
    /// included — the lowering donates a value's `QualifiedIdentifier` its
    /// `NamedValue`'s range, and the name is the text's all the same
    /// (`kg-oracle --errors`: both, at `nosuch` and at `y`). Red with a
    /// donated node read as a rewrite's.
    #[test]
    fn pass_9_judges_every_name_the_text_writes() {
        let text = "inline void f()\n{\n    nosuch();\n    uint32 x = y;\n}\n";
        let got = text_of(text, super::TextPass::UndefinedSymbols);
        let want = [("Symbol name not in scope: `nosuch`", "nosuch", 3), ("Symbol name not in scope: `y`", "y", 4)];
        assert_eq!(got, want.map(|(m, at, l)| (m.to_owned(), at.to_owned(), l)));
    }

    /// Where pass 9 notes a name (`kg-oracle --errors`, 2026-10-10): a
    /// scoped type name at its last identifier — the parser's note for it —
    /// (`Outer::Inne` at `Inne`, `type-identifier.k::14`); a value's
    /// qualified name at its last identifier too, and a free call's over
    /// the whole path (`freeFunctionSpecifier`, `Parser.hs:772-780`:
    /// `Outer::nope()` at `Outer`). And `err n` replaces the node it judges
    /// with everything under it (`Frontend.hs:101-102,113`): `Nope::Sv` is
    /// `Sv`'s verdict alone, its undefined qualifier none (`options.k::5`).
    /// Red with each of the three undone.
    #[test]
    fn pass_9_notes_a_name_where_the_parser_noted_it() {
        let text = "class Outer\n{\npublic:\n    struct Inner\n    {\n    }\n}\n\ninline void f()\n{\n    static Outer::Inne a;\n    Outer::nope();\n    auto b = Nope::Sv;\n}\n";
        let got = text_of(text, super::TextPass::UndefinedSymbols);
        let want = [
            ("Type name not in scope: `Inne`\nPerhaps you meant: `Inner`", "Inne", 11),
            ("Symbol name not in scope: `nope`", "Outer::nope", 12),
            ("Symbol name not in scope: `Sv`", "Sv", 13),
        ];
        assert_eq!(got, want.map(|(m, at, l)| (m.to_owned(), at.to_owned(), l)));
    }

    /// A template template parameter's default is read as a type by the
    /// parser (`templateSpecifier`, `Parser.hs:368`), which rejects a name
    /// no parse-time template answers — "unexpected memory … expecting
    /// template specifier" at the name (`kg-oracle --errors`, `bitarray.k:
    /// 17` under `mini-base`) — and divergence #7 reports it as the type
    /// name it is, once: where an instance leaving it out takes the default,
    /// that copy is judged as the value the lowering read and is dropped.
    /// Red with the wording a value's, and with the copy kept.
    #[test]
    fn a_template_template_default_nothing_declares_is_a_type_name() {
        let text = "template <auto N, template <typename, auto> typename M = memory>\nclass Foo\n{\n}\n\ninline void f()\n{\n    static Foo<1> x;\n}\n";
        let got = text_of(text, super::TextPass::UndefinedSymbols);
        assert_eq!(got, [("Type name not in scope: `memory`".to_owned(), "memory".to_owned(), 1)]);
    }

    /// Pass 12's `GT` arm on the module's own text, with the template's
    /// declaration as its note. Red with that arm kept to copies.
    #[test]
    fn too_many_arguments_is_judged_outside_copies_too() {
        let text = "template <typename A>\nstruct One\n{\n    A a;\n}\n\ninline void main()\n{\n    One<uint8, uint8> y;\n}\n";
        let out = diagnostics(text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Too many arguments specified for template `One`");
        assert!(out[0].1.starts_with("One<uint8, uint8>"), "{out:?}");
        assert_eq!(out[0].2.len(), 1, "{out:?}");
        assert!(out[0].2[0].starts_with("while compiling: struct One"), "{out:?}");
    }

    /// A designated initializer under a copy's type is judged
    /// (`TypeCheck.hs:130`): a flag the field's function type has and the
    /// value's does not is no subtype (`Type.hs:333`). Red with the CST
    /// pass's instance guard restored.
    #[test]
    fn a_designated_initializer_under_a_copy_is_judged() {
        let text = "inline void void_F_bool(bool b)\n{\n}\n\ntemplate <typename Callback>\nclass Foo\n{\nprivate:\n    Callback callback;\n}\n\ninline void main()\n{\n    static Foo<([[last]] bool) -> void> foo = {.callback = void_F_bool};\n}\n";
        let out = conversions(text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Invalid conversion from `(bool) -> void` to `([[last]] bool) -> void`");
        assert_eq!(out[0].1, "void_F_bool");
    }

    /// A kind naming an instance of a template-template parameter —
    /// `() -> TT<T> fn` — is evaluated with the template the argument names:
    /// under `TT = S1` the copy is made and nothing is said; under `TT = S2`
    /// the indirect `S2<uint32>` is decided first, and then `Do` is refused
    /// at `Do`, framed by the call (`kg-oracle --errors`, 2026-09-26). Red
    /// with a kind's template parameter left unfollowed: the first leaves
    /// `FnFn<…>` standing ("Failed to instantiate template"), the second
    /// refuses nothing.
    #[test]
    fn a_template_template_parameter_in_a_kind_is_followed() {
        let ok = "template <typename T>\nstruct S1\n{\n    T x;\n}\n\ntemplate <typename T>\nstruct S2\n{\n    T x;\n}\n\ntemplate <typename T, template <typename> typename TT, () -> TT<T> fn>\ninline void FnFn()\n{\n}\n\ninline S1<uint32> Do()\n{\n    return {};\n}\n\ntemplate <template <typename> typename TT>\ninline void Go()\n{\n    FnFn<uint32, TT, Do>();\n}\n\ninline void main()\n{\n    Go<S1>();\n}\n";
        assert_eq!(diagnostics(ok), Vec::new());
        let bad = "template <typename T>\nstruct S1\n{\n    T x;\n}\n\ntemplate <typename T>\nstruct S2\n{\n    T x;\n}\n\ntemplate <typename T, template <typename> typename TT, () -> TT<T> fn>\ninline void FnFn()\n{\n}\n\ninline S1<uint32> Do()\n{\n    return {};\n}\n\ntemplate <template <typename> typename TT>\ninline void Go()\n{\n    FnFn<uint32, TT, Do>();\n}\n\ninline void main()\n{\n    Go<S2>();\n}\n";
        let out = diagnostics(bad);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(
            out[0].0,
            "Invalid argument for template parameter `fn`.\nExpected a function of type:\n\t() -> S2<uint32>\nbut the argument type is:\n\t() -> S1<uint32>"
        );
        assert_eq!(out[0].1, "Do");
        assert_eq!(out[0].2, vec!["while compiling: Go<S2>".to_owned()]);
    }

    /// A type given for a template-template parameter is never a template,
    /// so the instance is never made and pass 12 says "Failed to
    /// instantiate template" at it, framed by the template's declaration
    /// and the copy — asked with `leftovers` cleared too
    /// (`template-template-parameter.k::36,37`, `kg-oracle --errors`
    /// 2026-09-27). The same call over a template compiles (above).
    #[test]
    fn a_type_for_a_template_template_parameter_never_instantiates() {
        let head = "template <typename T>\nstruct S1\n{\n    T x;\n}\n\ntemplate <typename T, template <typename> typename TT, () -> TT<T> fn>\ninline void FnFn()\n{\n}\n\ninline S1<uint32> Do()\n{\n    return {};\n}\n\ntemplate <typename T>\ninline void Go()\n{\n    FnFn<uint32, T, Do>();\n}\n\n";
        for arg in ["uint32", "S1<uint32>"] {
            let out = diagnostics(&format!("{head}inline void main()\n{{\n    Go<{arg}>();\n}}\n"));
            assert_eq!(out.len(), 1, "{out:?}");
            assert_eq!(out[0].0, "Failed to instantiate template");
            assert!(out[0].1.starts_with("FnFn<uint32, T, Do>"), "{out:?}");
            assert_eq!(out[0].2, vec!["while compiling: inline void FnFn()\n{\n}".to_owned(), format!("while compiling: Go<{arg}>")]);
        }
    }

    /// `validate Typename NamedValueF{} | tIsType …` (`TypeCheck.hs:233-234`):
    /// `C<T::Foo>` without `typename` is a value naming a type, refused at
    /// the argument (`kg-oracle --errors`, 2026-09-26). Red with that arm
    /// removed.
    #[test]
    fn a_type_named_without_typename_is_refused() {
        let text = "class A\n{\npublic:\n    using Foo = uint8;\n}\n\ntemplate <typename U>\nclass C\n{\n}\n\ntemplate <typename T>\ninline void Go()\n{\n    C<T::Foo> c;\n}\n\ninline void main()\n{\n    Go<A>();\n}\n";
        let out = diagnostics(text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Invalid argument for template parameter `U`.\nExpected a type; did you forget 'typename`");
        assert!(out[0].1.starts_with("T::Foo"), "{out:?}");
    }

    /// `check AutoF{}` (`TypeCheck.hs:51`) in a class's copy: the argument
    /// `(uint32) -> auto` substituted into `using callback_t = T;` is "Could
    /// not deduce type" at the written `auto`, unframed — the copy keeps the
    /// argument's note (`callbacks.k::68,69`; `kg-oracle --errors`
    /// 2026-09-27). A function template that never uses its `T` has no
    /// such alias and compiles; the module's own alias is the CST pass's.
    /// Red with the alias arm unasked.
    #[test]
    fn an_auto_argument_in_a_copied_alias_is_undeduced() {
        let class = "template <typename T>\nclass Foo\n{\n    using callback_t = T;\n}\n\n";
        let undeduced = vec![("Could not deduce type".to_owned(), "auto".to_owned(), Vec::<String>::new())];
        assert_eq!(diagnostics(&format!("{class}inline void main()\n{{\n    static Foo<(uint32) -> auto> foo;\n}}\n")), undeduced);
        let member = "template <typename T>\nclass Foo\n{\n    using callback_t = T;\n    callback_t callback;\n}\n\n";
        assert_eq!(diagnostics(&format!("{member}inline void main()\n{{\n    static Foo<(uint32) -> auto> foo;\n}}\n")), undeduced);
        assert_eq!(diagnostics("template <typename T>\ninline void F()\n{\n}\n\ninline void main()\n{\n    F<(uint32) -> auto>();\n}\n"), Vec::new());
        assert_eq!(diagnostics("using fn = (auto) -> void;\n"), Vec::new());
    }

    /// `checkArraySize` (`TypeCheck.hs:187-196`) in a copy, framed by the
    /// instantiations (`kg-oracle --errors`, 2026-09-27): `I - 3` folded to
    /// `0` in `Do<3>` (`template.k::30`), and a `false` bound to `T1 N`
    /// (`nested-templates.k::9`). Without the text the dimension's own
    /// range is reported; `kg_db` hands the `[` (its `looped` tests). A
    /// positive dimension, and one bound to an enum constant, are fine.
    /// Red with the array arm unasked.
    #[test]
    fn a_copys_dimension_must_be_a_positive_integer() {
        let text = "template <typename T>\ninline void Go(T x)\n{\n    template <auto I>\n    inline void Do()\n    {\n        bool[I - 3] x;\n    }\n\n    Do<3>();\n}\n\ninline void main()\n{\n    Go(10);\n}\n";
        let out = diagnostics(text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!((out[0].0.as_str(), out[0].1.as_str()), ("Array dimension must be a positive integer", "I - 3"));
        assert_eq!(out[0].2.len(), 2, "{out:?}");
        let nested = |arg: &str| {
            format!("template <typename T1>\nclass Foo\n{{\npublic:\n    template <T1 N>\n    struct S\n    {{\n        T1[N] x;\n    }}\n}}\n\ninline void main()\n{{\n    static Foo<{arg}>::S<{}> bar;\n}}\n", if arg == "bool" { "false" } else { "2" })
        };
        let out = diagnostics(&nested("bool"));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!((out[0].0.as_str(), out[0].1.as_str()), ("Array dimension must be a positive integer", "N"));
        assert_eq!(diagnostics(&nested("uint8")), Vec::new());
        let enumerated = "enum E : uint5\n{\n    X,\n    Count\n}\n\ntemplate <auto N>\nusing Foo = bool[N];\n\ninline void main()\n{\n    Foo<E::Count> x;\n}\n";
        assert_eq!(diagnostics(enumerated), Vec::new());
    }

    /// Pass 19's conversions at the module's text are [`super::TextPass::
    /// CheckConversion`]'s, and never raised as the copies' are: a
    /// `bitsizeof` of a class has no width to fold to, so the comparison
    /// over it is `unresolved` for good and converts to nothing — "Invalid
    /// conversion from `unresolved` to `bool`" at the argument (`basics.k::
    /// 110`, `kg-oracle --errors` 2026-10-10) — while a struct's folds and
    /// is fine. Red with the class's `bitsizeof` read as any other, and
    /// with the text's verdicts raised among the copies'.
    #[test]
    fn a_class_s_size_is_unresolved_for_good_at_the_text() {
        let program = |agg: &str| {
            format!("{agg} C\n{{\n    uint32 x;\n}}\n\ninline void take(bool b)\n{{\n}}\n\ninline void main()\n{{\n    static C c;\n    take(bitsizeof(c) == 32);\n}}\n")
        };
        let at = |text: &str| -> Vec<(String, String)> {
            text_of(text, super::TextPass::CheckConversion).into_iter().map(|(m, at, _)| (m, at)).collect()
        };
        let unresolved = vec![("Invalid conversion from `unresolved` to `bool`".to_owned(), "bitsizeof(c) == 32".to_owned())];
        assert_eq!(at(&program("class")), unresolved);
        assert_eq!(at(&program("struct")), Vec::new());
        assert_eq!(diagnostics(&program("class")), Vec::new());
    }

    /// `checkShadowDecl` (`TypeCheck.hs:110-121`) at the module's text, over
    /// the tree as pass 19 has it: the warning at the local's name, the note
    /// at the declaration it shadows from its first token (`kg-oracle
    /// --errors --Wshadow --Werror`, 2026-10-10: `bool x;` at its `bool`).
    /// A parameter is shadowable, a module's variable only under a `module`
    /// header (an empty `getScope` is not); a function hoisted out of
    /// another (`moveFunctions`, `Function.hs:47-50`) reaches its own scopes
    /// and its parent's qualifier, never the parent's body
    /// (`shadow-warning.k`'s `warning:0` row) — but its own parameter moves
    /// with it. Red with the lookup starting at the declaration's own scope
    /// (every local shadows itself), and with the note at the name.
    #[test]
    fn a_shadowing_local_warns_at_its_name_noted_at_what_it_shadows() {
        let shadows = |text: &str| -> Vec<(String, String)> {
            let interner = Interner::new();
            let parse = kg_syntax::parse_file(text);
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
            let e = crate::elaborate::elaborate(vec![ir], &interner);
            super::checked(&e, &interner, true, false, &|_| None)
                .text
                .into_iter()
                .filter(|v| v.pass == super::TextPass::CheckShadowDecl)
                .map(|v| {
                    let note = v.diagnostic.labels[0].range.range;
                    (text[v.diagnostic.range.range].to_owned(), text[note].trim().to_owned())
                })
                .collect()
        };
        let one = |at: &str, note: &str| vec![(at.to_owned(), note.to_owned())];
        assert_eq!(shadows("inline void main()\n{\n    bool x;\n    {\n        uint32 x;\n    }\n}\n"), one("x", "bool x;"));
        assert_eq!(shadows("inline void Run(bool x)\n{\n    {\n        uint32 x;\n    }\n}\n"), one("x", "bool x"));
        assert_eq!(shadows("const bool x = true;\n\ninline void main()\n{\n    uint32 x;\n}\n"), vec![]);
        assert_eq!(shadows("module m {}\n\nconst uint32 x = 1;\n\ninline void main()\n{\n    uint32 x;\n}\n"), one("x", "const uint32 x = 1;"));
        let hoisted = "inline void main()\n{\n    uint32 x;\n\n    inline void Bar()\n    {\n        uint32 x;\n\n        x = 0;\n    }\n\n    x = 0;\n}\n";
        assert_eq!(shadows(hoisted), vec![]);
        let parameter = "inline void main()\n{\n    inline void Bar(bool x)\n    {\n        {\n            uint32 x;\n        }\n    }\n}\n";
        assert_eq!(shadows(parameter), one("x", "bool x"));
    }

    /// The rest of `validateProgram` (`TypeCheck.hs:38-71,180-215`) at the
    /// module's text, each shape as `kg-oracle --errors` reports it alone
    /// (2026-10-10): a type as a mux's arm; a global, at its declaration;
    /// an `auto` `deduceAuto` leaves — in an alias, a `const auto` return —
    /// and none it rewrote; a dimension an enum constant folds to `-1`; and
    /// a `static for`'s induction variable kept a loop, through a `const`
    /// local (`static-for.k::10`). Silent: a declaration among the
    /// statements (`data/closure/core.k`'s `struct Capture`), the `auto`
    /// of a bodiless function's parameter in a rewrite's copy, which the text
    /// never wrote (`extern-class.k::15`), an argument
    /// pass 12 already refused (`type-check.k::38`), and anything inside
    /// what pass 10 or pass 16 replaced (`1 + (uint32)`, `clog2(uint32)`).
    /// Red with each of those six rules undone, and with a rewritten
    /// `auto` judged.
    #[test]
    fn the_rest_of_validate_program_judges_the_text_as_pass_19_sees_it() {
        let at = |text: &str| -> Vec<(String, String)> {
            text_of(text, super::TextPass::ValidateProgram).into_iter().map(|(m, at, _)| (m, at)).collect()
        };
        let one = |message: &str, at: &str| vec![(message.to_owned(), at.to_owned())];
        assert_eq!(at("const int32 x = 2 ? bool : 3;\n"), one("Invalid use of a type as an expression", "bool"));
        assert_eq!(at("uint32 x;\n"), one("Global state is not allowed", "uint32 x;"));
        let autos = "using fn = (auto) -> void;\n\ninline const auto Foo()\n{\n    return 10;\n}\n\ninline void main()\n{\n    auto x = 1;\n}\n";
        assert_eq!(at(autos), vec![("Could not deduce type".to_owned(), "auto".to_owned()); 2]);
        let dimension = "enum Foo : int8\n{\n    A = -2,\n    B\n}\n\ninline void main()\n{\n    bool[Foo::B] x;\n}\n";
        assert_eq!(at(dimension), one("Array dimension must be a positive integer", "Foo::B"));
        let unrolled = "inline void main()\n{\n    static for (const auto i : 2)\n    {\n        static for (const auto j : i)\n        {\n            const auto a = j + 1;\n\n            uint32[a] b;\n        }\n    }\n}\n";
        assert!(at(unrolled).iter().any(|(m, at)| m == "Array dimension must be a positive integer" && at == "a"), "{:?}", at(unrolled));
        assert_eq!(at("inline void main()\n{\n    struct S\n    {\n    }\n\n    S s;\n}\n"), vec![]);
        assert_eq!(at("class Foo\n{\npublic:\n    void Do(auto x);\n}\n"), vec![]);
        assert_eq!(at("template <typename T, bool B>\nclass Foo\n{\n}\n\ninline void main()\n{\n    static Foo<auto, false> x;\n}\n"), vec![]);
        assert_eq!(at("inline void main()\n{\n    uint32 x = 1 + (uint32);\n}\n"), vec![]);
        assert_eq!(at("inline void main()\n{\n    uint32 x = clog2(uint32);\n}\n"), vec![]);
    }

    /// `check x@DecltypeF{}` (`TypeCheck.hs:53`): a `decltype` the fold
    /// never replaced, at its operand — `decltype(x) x;` reads the variable
    /// it declares, a local and a global alike (`kg-oracle --errors`
    /// 2026-10-10; difffuzz [36]). One whose operand types is folded
    /// away, a later local's included; a rewrite's is not the text's. Red
    /// with the clause removed, and with the rewrite's judged.
    #[test]
    fn a_decltype_left_standing_could_not_deduce_its_operand() {
        let at = |text: &str| -> Vec<(String, String)> {
            text_of(text, super::TextPass::ValidateProgram).into_iter().map(|(m, at, _)| (m, at)).collect()
        };
        let one = |at: &str| vec![("Couldn't deduce type of the expression".to_owned(), at.to_owned())];
        assert_eq!(at("inline void main()\n{\n    decltype(x) x;\n}\n"), one("x"));
        assert_eq!(at("decltype(q) q;\n"), one("q"));
        assert_eq!(at("inline void main()\n{\n    uint32 x;\n    decltype(x) y;\n}\n"), vec![]);
        assert_eq!(at("inline void main()\n{\n    decltype(y) x;\n    uint32 y;\n}\n"), vec![]);
        // A lambda's synthesized `decltype(make_capture0())`, with no base
        // library to declare `make_capture0`, is not the text's.
        let lambda = "inline void main()\n{\n    uint32 a;\n    auto f = [a](uint32 x) -> uint32 { return x - a; };\n}\n";
        assert!(!at(lambda).iter().any(|(m, _)| m.starts_with("Couldn't deduce")), "{:?}", at(lambda));
        // Under a latency bound none is given: the next round may type the
        // operand (`multi_word.k`'s `decltype(p.first)`).
        let truncated = |text: &str| -> Vec<String> {
            let interner = Interner::new();
            let parse = kg_syntax::parse_file(text);
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
            let e = crate::elaborate::elaborate(vec![ir], &interner);
            super::checked(&e, &interner, true, true, &|_| None).text.into_iter().map(|v| v.diagnostic.message).collect()
        };
        assert!(!truncated("decltype(q) q;\n").iter().any(|m| m.starts_with("Couldn't deduce")));
    }
}
