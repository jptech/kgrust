//! `instantiateTemplates` (`Template.hs:50-62`), program-wide, on the IR —
//! `docs/history/ir-elaboration.md` §2.4: no twin, no rename, no text.
//!
//! ```haskell
//! instantiateTemplates templatePasses templateIterations (program, hasErrors)
//!     | templateIterations == 0 || Map.null decidedInstances = (parallel (desugar deduplicateInstances) program', False)
//!     | otherwise = instantiateTemplates … (inferType $ parallel (desugar (instantiate . resolve)) program', False)
//!   where
//!     program' = foldr ($) program passes'
//! ```
//!
//! One round is the pass list (`Template.hs:65-81`, run bottom-up), the
//! census over its output ([`crate::census`]), and — when the census decided
//! anything — `resolve` and `instantiate` over every module (`kg_hir`'s
//! passes, handed the census's decisions), then `inferType`. Each module
//! keeps its own arena; a template's copies are made in the module that
//! declares it, and an argument written in another module is carried over
//! with its types, as the reference's one program tree shares the node.
//!
//! # The pass list, as far as it is ported here
//!
//! | sub-pass | here |
//! |---|---|
//! | `addLambdaWrappers` | [`crate::closures::add_lambda_wrappers`] |
//! | `desugarLambdas`, `abbreviatedFunctionTemplate`, `moveFunctions` | `kg_hir::rewrite::{pipeline_tail, round_prefix}` |
//! | `inferType` | [`Program::infer`] |
//! | `closureCalls` | [`crate::closures::closure_calls`] |
//! | `deduceTemplateArgs` | [`crate::deduce`] |
//! | `templateTypeCheck` | its `validateArg` literal stamps only ([`crate::census::Decided::stamps`]) |
//!
//! The rewrites keep a node's note in the reference (`desugar`,
//! `Desugar.hs:92-94`); an append-only arena mints a replacement instead,
//! and [`carry_types`] gives each replacement its original's type through
//! the arena's lineage.

use std::sync::Arc;

use kg_base::{ExpansionInterner, FileRange, FxHashMap, Interner, InstanceId};
use kg_hir::rewrite::{DecidedInstances, DecidedSite, InstanceSpec, Site, TemplateInstances};
use kg_hir::{Exp, ExpArena, ExpId, ModuleIr};

use crate::census::{census, Census, View};
use crate::infer::{Module, Program};
use crate::reads::{Door, Pass};
use crate::types::TypeMap;

/// The copy budget: the reference loops until nothing is decided
/// (`template_iterations = maxBound`, `Options/CmdArgs.hs:105`); this bounds
/// a port bug to a stop, not a hang. The reference's own count reaches 51
/// on the corpus (`data/memory/bank/schedule.k`; 33–36 on four more), which
/// the guard of 32 this replaced cut short (`cargo xtask loop-left`,
/// 2026-09-27) — so it sits well above any count the reference needs, and
/// `loop-left` reports a root within two rounds of it.
pub const ROUND_GUARD: u32 = 128;

/// What the loop did, per round and in all.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub rounds: u32,
    /// Instances decided per round.
    pub decided: Vec<usize>,
    pub made: u32,
    pub declined: u32,
    pub skipped_indirect: u32,
    pub unresolved_kinds: u32,
    /// Instances `validateArg` refuses (`TypeCheck.hs:228-293`), summed
    /// over every round's census.
    pub invalid: u32,
    /// Decisions whose template no module's tree declares where the census
    /// found it.
    pub homeless: u32,
    /// Whether the loop stopped at its round bound with instances still
    /// decided — the budget's doing, or at [`ROUND_GUARD`] a loop that does
    /// not converge.
    pub budget_exhausted: bool,
    /// `TemplateInstance`s the final tree still holds outside every
    /// template body — each a decision the reference made and this loop did
    /// not, or a program the reference rejects. `KG_LOOP_LEFT=1` names them.
    pub left: u32,
    /// Every `inferType` fixpoint's unresolved count, iteration by
    /// iteration ([`Program::trace`]) — what the incremental walk must
    /// reproduce exactly, not only where it ends.
    pub infer_trace: Vec<Vec<u64>>,
}

/// The elaborated program: every module's final tree and its types.
#[derive(Debug)]
pub struct Elaborated {
    pub modules: Vec<Module>,
    pub expansions: Arc<ExpansionInterner>,
    pub report: Report,
    /// What the census refused (`validateArg`, `TypeCheck.hs:228-293`), once
    /// per argument and message: the refused argument's `Src` and
    /// `argError`'s message. `kg_sem::check` reports them.
    pub refused: Vec<(kg_base::Src, String)>,
    /// Where each function pass 2 refused is written: a non-inline one
    /// taking a function (`Function.hs:313-325`), which `kg_sem::check`
    /// reports at the module's text. Pass 2 runs before any other pass
    /// deletes a branch or instantiates a template, so every one written
    /// anywhere is here — a dead `static if`'s, a template's body's.
    pub higher_order: Vec<kg_base::FileRange>,
    /// Every copy `instantiate` made, by the expansion its nodes carry, and
    /// the instance it is — the `TInstance` `setTypeOfInstance` notes on
    /// the copy's declaration (`Template.hs:125`). An editor names a
    /// `while compiling` frame from this (`kg_db::looped::Looped`), where
    /// reading it back off the final tree took the first node of the
    /// expansion a walk met for its declaration, and a node of a copy is
    /// reachable from outside it (`test/library/data/fifo/transpose.k`'s
    /// `TestTranspose<…>`, named nothing).
    pub copies: FxHashMap<kg_base::Expansion, kg_ty::Ty>,
    /// The run's state per module and round, where it was recorded
    /// ([`elaborate_recorded`]; sprint W7.5d, `crate::snapshot`): the
    /// history of `modules`, which are its final state.
    pub snapshot: Option<Box<crate::snapshot::Rounds>>,
}

/// Frontend passes 2 through 8 over `irs` — each module's sorted IR, in
/// program order.
pub fn elaborate(irs: Vec<ModuleIr>, interner: &Interner) -> Elaborated {
    elaborate_with(irs, interner, ROUND_GUARD)
}

/// [`elaborate`], stopping after `max_rounds` instantiation rounds —
/// `--template-iterations` (`kg_db::PassBudget::AtMost`); stopping short
/// with instances still decided is reported ([`Report::budget_exhausted`]).
pub fn elaborate_with(irs: Vec<ModuleIr>, interner: &Interner, max_rounds: u32) -> Elaborated {
    elaborate_cancellable(irs, interner, max_rounds, &kg_base::cancel::Cancel::never())
}

/// [`elaborate_with`], stopping at the next checkpoint once `cancel` is
/// raised ([`elaborate_opts`]); inference incremental.
pub fn elaborate_cancellable(irs: Vec<ModuleIr>, interner: &Interner, max_rounds: u32, cancel: &kg_base::cancel::Cancel) -> Elaborated {
    elaborate_opts(irs, interner, max_rounds, false, cancel)
}

/// [`elaborate_with`], inference's fixpoint walked whole every iteration
/// when `full_infer` ([`Program::full`]) — the identity check's reference —
/// and stopping at the next checkpoint once `cancel` is raised: between
/// rounds, between a round's passes, and inside inference
/// (`kg_base::cancel`; sprint W7.2). An uncancelled run is the same run.
pub fn elaborate_opts(
    irs: Vec<ModuleIr>,
    interner: &Interner,
    max_rounds: u32,
    full_infer: bool,
    cancel: &kg_base::cancel::Cancel,
) -> Elaborated {
    run(irs, interner, max_rounds, full_infer, cancel, None, None)
}

/// [`elaborate_opts`], its state per module and round recorded as it runs
/// into [`Elaborated::snapshot`] (`crate::snapshot`) and each round's end
/// shown to `record`'s observer. The loop's output is the same: the
/// recorder only reads, and logs what the round writes over.
pub fn elaborate_recorded(
    irs: Vec<ModuleIr>,
    interner: &Interner,
    max_rounds: u32,
    full_infer: bool,
    cancel: &kg_base::cancel::Cancel,
    record: crate::snapshot::Recorder<'_>,
) -> Elaborated {
    run(irs, interner, max_rounds, full_infer, cancel, None, Some(record))
}

/// [`elaborate_opts`], and on the way the answer a bound of `at` rounds
/// would have given, handed to `first` as the run passes that bound — the
/// language server's layered diagnostics (sprint W7.6, `kg_db::PassBudget::
/// Layered`): its first answer at four rounds, the same run then carried on
/// to `max_rounds`, its last.
///
/// The first answer is `elaborate_opts(…, at, …)`'s exactly: up to the
/// bound the two runs are one computation — `max_rounds` is read nowhere
/// before the check that stops the loop — and [`Stopped::answer`] ends a
/// fork of the state there as the bounded run ends the state itself. So
/// the rounds before the bound are run once, and a fork of the program is
/// the whole price of the first answer. `first` is not called where the
/// loop stops at or before the bound: nothing then lies between the two
/// answers, and the last is the only one.
pub fn elaborate_layered(
    irs: Vec<ModuleIr>,
    interner: &Interner,
    max_rounds: u32,
    at: u32,
    full_infer: bool,
    cancel: &kg_base::cancel::Cancel,
    first: &mut dyn FnMut(Stopped<'_>),
) -> Elaborated {
    run(irs, interner, max_rounds, full_infer, cancel, Some((at, first)), None)
}

/// [`elaborate_layered`], recorded as [`elaborate_recorded`] records — the
/// editor's loop (`kg_db::PassBudget::Layered`), which keeps its snapshot.
#[allow(clippy::too_many_arguments)]
pub fn elaborate_layered_recorded(
    irs: Vec<ModuleIr>,
    interner: &Interner,
    max_rounds: u32,
    at: u32,
    full_infer: bool,
    cancel: &kg_base::cancel::Cancel,
    first: &mut dyn FnMut(Stopped<'_>),
    record: crate::snapshot::Recorder<'_>,
) -> Elaborated {
    run(irs, interner, max_rounds, full_infer, cancel, Some((at, first)), Some(record))
}

/// The loop where a round bound would have stopped it, at its census with
/// instances still decided ([`elaborate_layered`]).
pub struct Stopped<'a> {
    program: &'a Program<'a>,
    round: u32,
    report: &'a Report,
    refused: &'a [(kg_base::Src, String)],
    higher_order: &'a [kg_base::FileRange],
    copies: &'a FxHashMap<kg_base::Expansion, kg_ty::Ty>,
    expansions: &'a Arc<ExpansionInterner>,
}

impl std::fmt::Debug for Stopped<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stopped").field("round", &self.round).finish_non_exhaustive()
    }
}

impl Stopped<'_> {
    /// The round the loop stands at.
    pub fn round(&self) -> u32 {
        self.round
    }

    /// The elaboration a loop bounded here returns, made from a fork of
    /// every module — arena and types — so the run goes on from the state
    /// as it stands: `parallel (desugar deduplicateInstances) program'`
    /// over the fork (`Template.hs:57`), the bound reported, and the rest
    /// as [`elaborate_opts`] fills it in when it stops. The expansions are
    /// shared: frames are only ever appended, and the fork's nodes name
    /// none the run adds later.
    pub fn answer(&self) -> Elaborated {
        let interner = self.program.interner();
        let mut modules: Vec<Module> =
            self.program.modules.iter().map(|m| Module { ir: m.ir.fork(), types: m.types.clone() }).collect();
        for m in modules.iter_mut() {
            kg_hir::rewrite::deduplicate_instances_in_place(&mut m.ir, interner);
            carry_types(m);
        }
        let mut report = self.report.clone();
        report.budget_exhausted = true;
        report.rounds = self.round;
        report.infer_trace.clone_from(&self.program.trace);
        report.left = modules.iter().map(|m| left_instances(&m.ir, interner)).sum();
        Elaborated {
            modules,
            expansions: Arc::clone(self.expansions),
            report,
            refused: self.refused.to_vec(),
            higher_order: self.higher_order.to_vec(),
            copies: self.copies.clone(),
            snapshot: None,
        }
    }
}

/// Where [`elaborate_layered`] takes its first answer, and who it is
/// handed to.
type FirstAnswer<'f> = Option<(u32, &'f mut dyn FnMut(Stopped<'_>))>;

/// [`elaborate_opts`] and [`elaborate_layered`]: the one loop.
fn run(
    irs: Vec<ModuleIr>,
    interner: &Interner,
    max_rounds: u32,
    full_infer: bool,
    cancel: &kg_base::cancel::Cancel,
    mut first: FirstAnswer<'_>,
    mut record: Option<crate::snapshot::Recorder<'_>>,
) -> Elaborated {
    // Zero rounds is the reference's `--template-iterations=0`: the pass
    // list and the census once, nothing instantiated (`Template.hs:57`).
    let max_rounds = max_rounds.min(ROUND_GUARD);
    let started = std::time::Instant::now();
    let timing = std::env::var_os("KG_LOOP_TIME").is_some();
    let lap = |what: &str| {
        if timing {
            eprintln!("loop {:>8.2}s {what}", started.elapsed().as_secs_f64());
        }
    };
    let expansions = Arc::new(ExpansionInterner::new());
    let mut report = Report::default();
    let mut copies = FxHashMap::default();
    crate::profile::begin();
    // A recorded run's read door (`crate::reads`): every pass that reads
    // across a module boundary shows it what, per module and round.
    let door = record.as_mut().map(|r| Door::new(r.names_for(irs.len()), Arc::clone(&expansions)));
    let door = door.as_ref();
    let pass = |p: Pass| {
        if let Some(d) = door {
            d.pass(p);
        }
    };
    // Pass 1's `redundantNameScope`.
    let mut irs = irs;
    pass(Pass::NameScope);
    crate::pass1::redundant_name_scope(&mut irs, interner, door);
    crate::pass1::interpolated_string_echo(&mut irs, interner);
    // Pass 3's `reifyEnums` (its `captureThis` half is `pipeline_head`'s).
    crate::pass1::reify_enums(&mut irs, interner);
    // Passes 2 and 3 (`higherOrderFunctions`, `captureThis`).
    pass(Pass::Heads);
    let (heads, higher_order) = crate::pass1::heads(&irs, interner, door);
    let heads: Vec<ModuleIr> = heads
        .into_iter()
        .map(|mut ir| {
            ir.arena.track_lineage();
            // The passes pass over what holds nothing they rewrite
            // (`kg_hir::facts`) — but not on the whole walk, which stays
            // the reference `infer-identity` grades this against.
            if !full_infer {
                ir.arena.track_facts();
            }
            ir
        })
        .collect();
    let mut refused: Vec<(kg_base::Src, String)> = Vec::new();
    let modules = {
        let mut program =
            Program::new(heads, interner).with_expansions(&expansions).with_cancel(cancel.clone()).with_door(door);
        program.full = full_infer;
        // Pass 4, `deduceTemplateArgs` — before any inference, only an
        // instance written short of its parameters has anything to deduce.
        pass(Pass::Deduce);
        crate::deduce::deduce_template_args(&mut program);
        carry_all(&mut program.modules);
        // Passes 5 and 6: `intrinsics`, `inferType` (with `staticControl`).
        pass(Pass::Infer);
        program.infer();
        carry_all(&mut program.modules);
        // A later edition's named arguments, placed wherever pass 6 typed
        // the callee — before any pass reads a call's arguments by position
        // (`crate::named`). Nothing in a reference program.
        crate::named::named_arguments(&mut program);
        carry_all(&mut program.modules);
        lap("pass 6");
        // Pass 7: `trimExternClasses`.
        pass(Pass::Trim);
        crate::pass7::trim_extern_classes(&mut program);
        carry_all(&mut program.modules);
        let mut round = 0u32;
        loop {
            cancel.check();
            crate::profile::set_round(round);
            // Named arguments in the copies the last round made and typed.
            crate::named::named_arguments(&mut program);
            carry_all(&mut program.modules);
            // `addLambdaWrappers`, first to run (`Template.hs:79-80`).
            pass(Pass::Wrappers);
            crate::closures::add_lambda_wrappers(&mut program);
            carry_all(&mut program.modules);
            lap("wrappers");
            // The pass list's syntactic sub-passes.
            // Appended into each module's own arena: the rewrites never
            // change a node, so a copy per pass bought nothing. A module
            // whose every input to the pass is a kept run's takes the kept
            // run's output instead (`crate::reuse`, D1 step 3).
            if let (Some(r), Some(door)) = (record.as_mut(), door) {
                r.prefix_begins(round, &program.modules, door);
            }
            for (i, m) in program.modules.iter_mut().enumerate() {
                cancel.check();
                let _visit = crate::profile::visit("prefix", i, m.ir.arena.len());
                if record.as_mut().is_some_and(|r| r.reuse_prefix(i, m, round, interner, &expansions)) {
                    continue;
                }
                if round == 0 {
                    kg_hir::rewrite::pipeline_tail_in_place(&mut m.ir, interner);
                } else {
                    kg_hir::rewrite::round_prefix_in_place(&mut m.ir, interner);
                }
                let lineage = m.ir.arena.take_lineage();
                let carried = carry_lineage(m, &lineage);
                if let Some(r) = record.as_mut() {
                    r.prefix_carried(i, &carried);
                }
            }
            if let Some(r) = record.as_mut() {
                r.prefix_ends(round, &mut program.modules);
            }
            pass(Pass::Infer);
            program.infer();
            carry_all(&mut program.modules);
            // `closureCalls` (`Template.hs:74`).
            lap("prefix + infer");
            cancel.check();
            // … and where the prefix's rewrites and inference typed a callee
            // since, before `closureCalls` and deduction read arguments by
            // position.
            crate::named::named_arguments(&mut program);
            carry_all(&mut program.modules);
            crate::closures::closure_calls(&mut program);
            carry_all(&mut program.modules);
            // `deduceTemplateArgs`, then `inferType` (`Template.hs:73-74`).
            pass(Pass::Deduce);
            crate::deduce::deduce_template_args(&mut program);
            carry_all(&mut program.modules);
            pass(Pass::Infer);
            program.infer();
            carry_all(&mut program.modules);
            let decided = {
                let started = std::time::Instant::now();
                // `symbols = getSymbols program'` (`Template.hs:84`).
                let symbols = program.symbols(crate::symbols::Config::AllSymbols);
                crate::profile::whole("census's symbols", started);
                // What each subtree holds, derived where not yet known, for
                // the census to pass over what holds no instance.
                for m in program.modules.iter_mut() {
                    let root = m.ir.root;
                    m.ir.arena.holds(root);
                }
                let view_modules: Vec<(&ModuleIr, &TypeMap)> =
                    program.modules.iter().map(|m| (&m.ir, &m.types)).collect();
                census(&View { modules: &view_modules, symbols: &symbols, interner })
            };
            crate::profile::census(round, &program.modules, &decided, interner, &expansions);
            if let Some(r) = record.as_mut() {
                r.census(&program.modules, &decided, interner, &expansions);
            }
            lap(&format!("deduce + infer + census: {} decided", decided.decided.len()));
            cancel.check();
            report.skipped_indirect += decided.indirect;
            report.unresolved_kinds += decided.unresolved_kinds;
            report.invalid += decided.invalid;
            for r in &decided.refused {
                let src = program.modules[r.module].ir.arena[r.arg].src;
                if !refused.iter().any(|(s, m): &(kg_base::Src, String)| s.head == src.head && s.via == src.via && *m == r.message) {
                    refused.push((src, r.message.clone()));
                }
            }
            if !decided.is_empty() && round >= max_rounds {
                report.budget_exhausted = true;
            }
            if decided.is_empty() || round >= max_rounds {
                // Named arguments no run could place are errors — at the
                // fixpoint only: a loop cut short might yet have placed them.
                if decided.is_empty() {
                    crate::named::unplaced(&mut program.modules, interner);
                }
                // `parallel (desugar deduplicateInstances) program'`.
                for m in program.modules.iter_mut() {
                    kg_hir::rewrite::deduplicate_instances_in_place(&mut m.ir, interner);
                    carry_types(m);
                }
                if let (Some(r), Some(door)) = (record.as_mut(), door) {
                    r.end(round, true, &mut program.modules, &expansions, door);
                }
                break;
            }
            // Where a bound of `at` would have stopped, and the run goes on.
            if let Some((at, first)) = first.as_mut()
                && round == *at
            {
                first(Stopped {
                    program: &program,
                    round,
                    report: &report,
                    refused: &refused,
                    higher_order: &higher_order,
                    copies: &copies,
                    expansions: &expansions,
                });
                lap("the first answer");
            }
            round += 1;
            report.decided.push(decided.decided.len());
            cancel.check();
            pass(Pass::Resolve);
            instantiate_round(&mut program, &decided, round, &expansions, &mut report, &mut copies);
            lap("instantiate");
            pass(Pass::Infer);
            program.infer();
            carry_all(&mut program.modules);
            // The census's round ends here: its copies made and typed.
            if let (Some(r), Some(door)) = (record.as_mut(), door) {
                r.end(round - 1, false, &mut program.modules, &expansions, door);
            }
        }
        report.rounds = round;
        report.infer_trace = std::mem::take(&mut program.trace);
        for m in &program.modules {
            report.left += left_instances(&m.ir, interner);
        }
        crate::profile::end(program.modules.len());
        std::mem::take(&mut program.modules)
    };
    let snapshot = record.map(|r| Box::new(r.finish()));
    Elaborated { modules, expansions, report, refused, copies, higher_order, snapshot }
}

/// Everything the loop produced, as text: each module's tree with every
/// node's source and type in pre-order, the report and the refusals. Two
/// runs agree exactly when their fingerprints do — node ids aside, which a
/// run that skips a no-op rewrite may number differently, and symbols
/// spelled out, which an interner shared across programs numbers by
/// history.
pub fn fingerprint(e: &Elaborated, interner: &Interner) -> String {
    spell_symbols(&numbered_fingerprint(e, interner), interner)
}

/// Every `Symbol(N)` a `Debug` rendering holds, as the text it stands for.
fn spell_symbols(text: &str, interner: &Interner) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("Symbol(") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "Symbol(".len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        match (after[..digits].parse::<usize>().ok(), after[digits..].starts_with(')')) {
            (Some(n), true) => {
                out.push_str(&format!("{:?}", interner.resolve_index(n).as_deref().unwrap_or("?")));
                rest = &after[digits + 1..];
            }
            _ => {
                out.push_str("Symbol(");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn numbered_fingerprint(e: &Elaborated, interner: &Interner) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (i, m) in e.modules.iter().enumerate() {
        let _ = writeln!(out, "== module {i}");
        out.push_str(&kg_hir::structure::render(&m.ir, interner, kg_hir::structure::Detail::Annotated));
        let mut stack = vec![m.ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let _ = writeln!(out, "{}", if m.types.is_set(id) { format!("{:?}", m.types.get(id)) } else { "-".to_owned() });
            kids.clear();
            m.ir.arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().rev().copied());
        }
    }
    let _ = writeln!(out, "== report {:?}\n== refused {:?}", e.report, e.refused);
    out
}

/// Where an instance is noted — which the `while compiling` note of every
/// diagnostic inside the copy points at: the instance, or, where its text
/// begins with a qualifier, its last name, as the parser notes a qualified
/// name (`a::fail(…)`'s deduced `fail<uint32>` at `fail`). A qualifier the
/// sorter supplied (`Hold<S>` is `@h::Hold<S>`) has no text and moves
/// nothing.
pub(crate) fn instance_site(arena: &ExpArena, instance: ExpId) -> FileRange {
    let head = arena[instance].src.head;
    let Exp::TemplateInstance { base, .. } = &arena[instance].exp else { return head };
    let mut base = *base;
    while let Exp::QualifiedIdentifier(a) | Exp::TypeIdentifier(_, a) = &arena[base].exp {
        base = *a;
    }
    match &arena[base].exp {
        Exp::ScopedName { ident, .. } if arena[*ident].src.head.range.start() > head.range.start() => arena[*ident].src.head,
        _ => head,
    }
}

/// `parallel (desugar (instantiate . resolve))` (`Template.hs:58`): every
/// module's decided mentions resolved, and every template's decided
/// instances made where it is declared.
fn instantiate_round(
    program: &mut Program<'_>,
    decided: &Census,
    round: u32,
    expansions: &ExpansionInterner,
    report: &mut Report,
    copies: &mut FxHashMap<kg_base::Expansion, kg_ty::Ty>,
) {
    let interner = program.interner();
    let dotted = |path: &[kg_base::Symbol]| -> String {
        path.iter().map(|s| kg_base::unmangle_namespace(&interner.resolve(*s))).collect::<Vec<_>>().join(".")
    };
    // `resolve`'s decided map, per module: every occurrence the census met.
    let mut sites: Vec<DecidedInstances> = vec![DecidedInstances::default(); program.modules.len()];
    // And the map's keys, whole-program: `resolve` renames *every*
    // `TemplateInstance` whose arguments are a decided key
    // (`Template.hs:158-159`), in any module and inside any body — a copy's
    // the census cannot read yet included.
    let mut seeds: kg_base::FxHashMap<kg_hir::rewrite::Fingerprint, DecidedSite> = kg_base::FxHashMap::default();
    for d in &decided.decided {
        let decision = DecidedSite { template: dotted(&d.template), stamps: d.stamps.clone() };
        for &(m, id) in &d.sites {
            let node = &program.modules[m].ir.arena[id];
            sites[m].insert(Site { head: node.src.head, via: node.src.via }, decision.clone());
            if let Some(key) = kg_hir::rewrite::fingerprint_of(&program.modules[m].ir.arena, interner, id, &d.stamps) {
                seeds.entry(key).or_insert_with(|| decision.clone());
            }
        }
    }
    // `instantiate`'s specs, per home module. The first occurrence's
    // arguments stand for the instance (`Map.union` is left-biased); from
    // another module they are carried into the home arena, types and all.
    let mut specs: Vec<TemplateInstances> = vec![TemplateInstances::default(); program.modules.len()];
    for d in &decided.decided {
        let home = d.home.module;
        let (args, site) = match &d.first {
            crate::census::Args::Written(m, first) => {
                let (m, first) = (*m, *first);
                let Exp::TemplateInstance { args, .. } = program.modules[m].ir.arena[first].exp else {
                    report.homeless += 1;
                    continue;
                };
                let site = instance_site(&program.modules[m].ir.arena, first);
                let args = if m == home {
                    args
                } else {
                    let (from, to) = pair_mut(&mut program.modules, m, home);
                    import(&from.ir.arena, &from.types, args, &mut to.ir.arena, &mut to.types)
                };
                (args, site)
            }
            // An instance out of a parameter's substituted kind: its
            // arguments made where the template lives, noted as its
            // declaration, the kind's instance node the site.
            crate::census::Args::Minted(minted, m, at) => {
                let site: FileRange = program.modules[*m].ir.arena[*at].src.head;
                let module = &mut program.modules[home];
                let donor = d.home.id;
                let mut rw = kg_hir::rewrite::Rewriter::new(&mut module.ir.arena, interner);
                let mut mint = crate::from_type::Mint { rw: &mut rw, types: &mut module.types };
                let mut items = Vec::with_capacity(minted.len());
                let mut ok = true;
                for a in minted {
                    let exp = match a {
                        crate::census::Minted::Value(v) => crate::census::minted_literal(v),
                        crate::census::Minted::Type(t) => mint.exp_from_type(donor, t),
                    };
                    match exp {
                        Some(e) => items.push(mint.new_note(donor, e)),
                        None => ok = false,
                    }
                }
                if !ok {
                    report.homeless += 1;
                    continue;
                }
                (mint.new_note(donor, Exp::Seq(items)), site)
            }
        };
        specs[home].entry(d.template.clone()).or_default().push(InstanceSpec {
            id: InstanceId::of(&dotted(&d.template), &d.key),
            site,
            args,
            stamps: d.stamps.clone(),
            round,
        });
    }
    let cancel = program.cancel_token();
    for (m, module) in program.modules.iter_mut().enumerate() {
        cancel.check();
        let _visit = crate::profile::visit("instantiate", m, module.ir.arena.len());
        let mut asked = Vec::new();
        kg_hir::rewrite::resolve_instances_seeded_logged_in_place(
            &mut module.ir,
            interner,
            &sites[m],
            &seeds,
            program.door.is_some().then_some(&mut asked),
        );
        if let Some(door) = program.door {
            asked.sort_unstable();
            asked.dedup();
            for a in asked {
                door.fact(m, "seed", a);
            }
        }
        carry_types(module);
        // Every module, specs or none: `instantiate`'s catch-all is
        // `deduplicateInstances $ removeNestedSeq x` at every node
        // (`Template.hs:142`) — two lambdas `moveFunctions` hoisted under
        // one name leave one declaration.
        let r = kg_hir::rewrite::instantiate_templates_in_place(
            &mut module.ir,
            interner,
            expansions,
            &specs[m],
            Some(&module.types),
        );
        report.made += r.made;
        report.declined += r.declined;
        // The notes first, then the lineage: a root the scope walk rebuilt
        // after the note was taken carries it over.
        // The declaration's note first (`desugarWithNote`, `Template.hs:127`):
        // a copy of a copy is typed inside the first copy's instance.
        for &(root, decl) in &r.copied_from {
            if let t @ kg_ty::Ty::Instance { .. } = module.types.get(decl)
                && !module.types.is_set(root)
            {
                module.types.reset_type_of(root, t);
            }
        }
        for (root, ty) in r.typed_roots {
            module.types.set_type_of(root, ty);
        }
        for &(root, _) in &r.copied_from {
            if !module.types.is_set(root) {
                continue;
            }
            let ty = match module.types.get(root) {
                kg_ty::Ty::Type(inner) => (*inner).clone(),
                other => other,
            };
            if matches!(ty, kg_ty::Ty::Instance { .. }) {
                copies.insert(module.ir.arena[root].src.via, ty);
            }
        }
        carry_types(module);
    }
}

/// The `TemplateInstance`s reachable outside every template body.
fn left_instances(ir: &ModuleIr, interner: &Interner) -> u32 {
    let mut n = 0;
    for_each_left(ir, |id| {
        n += 1;
        if std::env::var_os("KG_LOOP_LEFT").is_some()
            && let Exp::TemplateInstance { base, .. } = &ir.arena[id].exp
        {
            let name = kg_hir::rewrite::get_name(&ir.arena, *base).map(|s| interner.resolve(s).to_string());
            let chain: Vec<String> =
                kg_hir::sort::spine_qualname(&ir.arena, *base).iter().map(|s| interner.resolve(*s).to_string()).collect();
            let scope = match &ir.arena[*base].exp {
                Exp::ScopedName { scope, .. } => scope.segments().iter().map(|s| interner.resolve(*s).to_string()).collect(),
                _ => Vec::new(),
            };
            eprintln!("left {name:?} at {:?} via {:?} chain {chain:?} scope {scope:?}", ir.arena[id].src.head, ir.arena[id].src.via);
        }
    });
    n
}

/// Every `TemplateInstance` outside every template body, as `f` sees them.
fn for_each_left(ir: &ModuleIr, mut f: impl FnMut(ExpId)) {
    let mut stack = vec![ir.root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        match &ir.arena[id].exp {
            Exp::Template { .. } => continue,
            Exp::TemplateInstance { .. } => f(id),
            _ => {}
        }
        kids.clear();
        ir.arena[id].exp.children(&mut kids);
        stack.extend(kids.iter().copied());
    }
}

/// [`Report::left`]'s instances, each as `SCOPE|QUALIFIER|NAME` — its base's
/// `getScope`, `getQualifier` and `getName` (`Parser/Syntax.hs:727-775`),
/// segments joined by `::` — sorted. The reference's own leftovers are
/// spelled the same way by `kg-oracle --loop`, so `cargo xtask loop-left`
/// compares the two as multisets.
pub fn left_labels(e: &Elaborated, interner: &Interner) -> Vec<String> {
    let mut out = Vec::new();
    for m in &e.modules {
        let arena = &m.ir.arena;
        for_each_left(&m.ir, |id| {
            let join = |syms: &[kg_base::Symbol]| syms.iter().map(|s| interner.resolve(*s).to_string()).collect::<Vec<_>>().join("::");
            let scope = get_scope(arena, id);
            let qualifier = kg_hir::rewrite::get_qualifier(arena, id).unwrap_or_default();
            let name = kg_hir::rewrite::get_name(arena, id).map(|s| interner.resolve(s).to_string()).unwrap_or_default();
            out.push(format!("{}|{}|{name}", join(&scope), join(&qualifier)));
        });
    }
    out.sort();
    out
}

/// `getScope` (`Parser/Syntax.hs:762-775`), over the name-node arms.
fn get_scope(arena: &ExpArena, id: ExpId) -> Vec<kg_base::Symbol> {
    match &arena[id].exp {
        Exp::ScopedName { scope, .. } => scope.segments().to_vec(),
        Exp::TypeIdentifier(_, a)
        | Exp::QualifiedIdentifier(a)
        | Exp::NamedValue(a)
        | Exp::TemplateInstance { base: a, .. }
        | Exp::FunctionSpecifier { function: a, .. } => get_scope(arena, *a),
        _ => Vec::new(),
    }
}

/// Two distinct modules, one to read and one to write.
fn pair_mut(modules: &mut [Module], from: usize, to: usize) -> (&Module, &mut Module) {
    assert_ne!(from, to);
    if from < to {
        let (a, b) = modules.split_at_mut(to);
        (&a[from], &mut b[0])
    } else {
        let (a, b) = modules.split_at_mut(from);
        (&b[0], &mut a[to])
    }
}

/// A copy of `from`'s subtree at `id` into `to`, sources kept — the node the
/// reference's one tree would share — with its types.
fn import(from: &ExpArena, from_types: &TypeMap, id: ExpId, to: &mut ExpArena, to_types: &mut TypeMap) -> ExpId {
    let node = from.node(id).clone();
    let mut exp = node.exp.clone();
    for child in exp.children_mut() {
        *child = import(from, from_types, *child, to, to_types);
    }
    let new = to.alloc(exp, node.src, node.kind);
    if from_types.is_set(id) {
        to_types.reset_type_of(new, from_types.get(id));
    }
    new
}

fn carry_all(modules: &mut [Module]) {
    for (i, m) in modules.iter_mut().enumerate() {
        let _visit = crate::profile::visit("carry types", i, m.ir.arena.len());
        carry_types(m);
    }
}

/// Each node a rewrite minted to stand in for another takes the other's
/// type, as the reference's rewrites keep the note (module comment).
pub(crate) fn carry_types(m: &mut Module) {
    let lineage = m.ir.arena.take_lineage();
    carry_lineage(m, &lineage);
}

/// [`carry_types`] over `lineage`, the arena's `(new, original)` pairs:
/// the pairs that gave a node its type, in order — all a replay of it on
/// the same module needs (`crate::reuse`).
pub(crate) fn carry_lineage(m: &mut Module, lineage: &[(ExpId, ExpId)]) -> Vec<(ExpId, ExpId)> {
    let mut carried: FxHashMap<usize, kg_ty::Ty> = FxHashMap::default();
    let mut wrote = Vec::new();
    for &(new, original) in lineage {
        if m.types.is_set(new) {
            continue;
        }
        let t = carried.get(&original.index()).cloned().or_else(|| m.types.is_set(original).then(|| m.types.get(original)));
        if let Some(t) = t {
            m.types.reset_type_of(new, t.clone());
            carried.insert(new.index(), t);
            wrote.push((new, original));
        }
    }
    wrote
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    use super::*;

    fn run(text: &str, interner: &Interner) -> Elaborated {
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), interner, &kg_hir::NoImports);
        elaborate(vec![ir], interner)
    }

    /// A copy of a copy — the instance a later round makes of the template
    /// an abbreviated copy became (`Template.hs:77`) — is noted inside the
    /// first copy's instance: its nodes start with the first copy's notes
    /// (`desugarWithNote`, `Template.hs:127`) and `setTypeOf` nests the new
    /// `TInstance` under the old (`Type.hs:1050-1057`). The walk names the
    /// copy by that outer note, `Fn_T_auto<bool>`, as the reference's walk
    /// does (`return-type-polymorphism.k::3`). Red without the declaration's
    /// note laid on the root first.
    #[test]
    fn a_copy_of_a_copy_is_noted_as_the_first_copy() {
        let interner = Interner::new();
        let e = run(
            "template <typename T>\ninline T Fn_T_auto(auto x)\n{\n    return cast<T>(x);\n}\n\n\
             inline void main()\n{\n    bool b = Fn_T_auto<bool>(true);\n}\n",
            &interner,
        );
        let m = &e.modules[0];
        let second = m
            .ir
            .top_level_items()
            .into_iter()
            .find(|&id| match &m.ir.arena[id].exp {
                Exp::Function { name, .. } => kg_hir::rewrite::get_name(&m.ir.arena, *name)
                    .is_some_and(|s| interner.resolve(s).matches("<|").count() == 2),
                _ => false,
            })
            .expect("a second-round copy");
        let kg_ty::Ty::Instance { template, args, .. } = m.types.get(second) else {
            panic!("an instance note, not {:?}", m.types.get(second));
        };
        let name: Vec<String> = template.segments().iter().map(|s| interner.resolve(*s).to_string()).collect();
        assert_eq!(name, ["Fn_T_auto"], "the outer note is the first copy's");
        assert_eq!(args.len(), 1);
        assert_eq!(interner.resolve(args[0].0), "T");
    }

    /// An enumerator as an argument is noted as the reference shows it:
    /// `TArg (showPretty e)` (`Type.hs:437`), and `showPretty` of the
    /// folded `EnumValue a _` is `pretty a` (`PrettyPrint.hs:98`), its
    /// written name — `pick<Mode::Off>`, as the walk names the copy
    /// (`getSpecialCases<23, 8, Denorm::Off>`). Red with the arm removed
    /// (the note is declined and the copy keeps no instance face).
    #[test]
    fn an_enumerator_argument_is_noted_by_its_written_name() {
        let interner = Interner::new();
        let e = run(
            "enum Mode : uint1\n{\n    Off,\n    On\n}\n\n\
             template <Mode M>\ninline uint8 pick()\n{\n    return 0;\n}\n\n\
             inline void main()\n{\n    uint8 x = pick<Mode::Off>();\n}\n",
            &interner,
        );
        let m = &e.modules[0];
        let copy = m
            .ir
            .top_level_items()
            .into_iter()
            .find(|&id| match &m.ir.arena[id].exp {
                Exp::Function { name, .. } => kg_hir::rewrite::get_name(&m.ir.arena, *name)
                    .is_some_and(|s| interner.resolve(s).contains("<|")),
                _ => false,
            })
            .expect("the copy");
        let kg_ty::Ty::Instance { args, .. } = m.types.get(copy) else {
            panic!("an instance note, not {:?}", m.types.get(copy));
        };
        assert!(matches!(&args[0].1, kg_ty::TArg::Expr(s) if s == "Mode::Off"), "{:?}", args[0].1);
    }

    /// A float argument is mangled `"@" <> show x` (`Mangle.hs:41`) and
    /// noted `TArg (showPretty e)` (`Type.hs:437`, `PrettyPrint.hs:105`),
    /// both GHC's `show :: Float`: `Bar<|$float@-1.1|>`, displayed
    /// `Bar<float, -1.1>` (`type-check.k::35,36`, the recorded walks). Red
    /// with the mangle arm declining (no copy, the mention left) or the
    /// note's arm removed (the copy has no instance note).
    #[test]
    fn a_float_argument_is_mangled_and_noted_as_haskell_shows_it() {
        let interner = Interner::new();
        let e = run(
            "template <typename T, T X>\nclass Bar\n{\n    const T x = X;\n}\n\n\
             inline void main()\n{\n    static Bar<float32, -1.10> b;\n    static Bar<float32, 1.0> c;\n}\n",
            &interner,
        );
        assert_eq!(instances_left(&e), 0);
        let names = declared(&e, &interner);
        for name in ["Bar<|$float@-1.1|>", "Bar<|$float@1.0|>"] {
            assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
        }
        let m = &e.modules[0];
        let copy = m
            .ir
            .top_level_items()
            .into_iter()
            .find(|&id| crate::symbols::declared_name(&m.ir.arena, id).is_some_and(|(_, n)| interner.resolve(n).contains("<|")))
            .expect("the copy");
        let kg_ty::Ty::Type(t) = m.types.get(copy) else { panic!("a class's type, not {:?}", m.types.get(copy)) };
        let kg_ty::Ty::Instance { args, .. } = &*t else { panic!("an instance note, not {t:?}") };
        assert!(matches!(&args[1].1, kg_ty::TArg::Expr(s) if s == "-1.1"), "{:?}", args[1].1);
    }

    /// `cast<float32>` of an integer reinterprets its low 32 bits
    /// (`castIntegralConstexpr`, `castWord32ToFloat . fromIntegral`,
    /// `Constexpr.hs:332`), so the argument folds and the instance is
    /// decided: `0x40200000` is `2.5`, not `1075838976.0`
    /// (`constexpr.k::87`'s `T4<cast<float32>(n), 32>`, the recorded walk).
    /// Red with the float target out of `constexpr::cast` (the mention is
    /// left undecided).
    #[test]
    fn a_cast_to_float32_folds_to_the_bits_it_names() {
        let interner = Interner::new();
        let e = run(
            "template <auto N, uint32 M>\nclass T4\n{\n}\n\nconst auto n = 0x40200000;\n\n\
             inline void main()\n{\n    static T4<cast<float32>(n), 32> foo;\n    static T4<1.0, cast<uint32>(2.5)> bar;\n}\n",
            &interner,
        );
        assert_eq!(instances_left(&e), 0);
        let names = declared(&e, &interner);
        // A float into an integer type is its bit pattern
        // (`castFloatConstexpr`, `castFloatToWord32`, `:337-343`).
        for name in ["T4<|@2.5@32|>", "T4<|@1.0@1075838976|>"] {
            assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
        }
    }

    /// `~` over an enum value folds in the enum's base type
    /// (`Constexpr.hs:262`, `invert (tEnumBase $ typeOf x)`): `~E5::Count`
    /// over a `uint5` enum whose `Count` is `1` is `30`, so
    /// `T3<~E5::Count == ~cast<uint5>(1)>` is `T3<true>`
    /// (`constexpr.k::118`, the recorded walk). Red with the enum arm
    /// declining (the mention is left) or inverting in the value's own
    /// width (`-2`, and `T3<false>`).
    #[test]
    fn an_inverted_enum_value_folds_in_its_base_type() {
        let interner = Interner::new();
        let e = run(
            "enum E5 : uint5\n{\n    X,\n    Count\n}\n\n\
             template <bool B>\nclass T3\n{\n}\n\n\
             template <auto N>\nclass C\n{\n}\n\n\
             inline void main()\n{\n    static T3<~E5::Count == ~cast<uint5>(1)> x;\n    static C<~E5::Count> y;\n}\n",
            &interner,
        );
        assert_eq!(instances_left(&e), 0);
        let names = declared(&e, &interner);
        for name in ["T3<|@True|>", "C<|@30|>"] {
            assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
        }
    }

    /// An enum constant whose value is itself an enum value folds to an
    /// `EnumValue` of that value, kept whole: `isIntLiteral` sees through
    /// `EnumValue` (`Type.hs:1087-1090`) and the fold keeps `unNote $ unfix
    /// v` (`Constexpr.hs:292`) — `G = F` over `F = Bar::B1` walks as two
    /// nested enum values (`enum.k::8`, the recorded walk). Red with the
    /// arm asking for a bare literal (`G`'s value left a name).
    #[test]
    fn an_enum_constant_naming_an_enum_value_nests_it() {
        let interner = Interner::new();
        let e = run(
            "enum Bar : int32\n{\n    B1,\n    B2\n}\n\nenum Foo : int32\n{\n    F = Bar::B1,\n    G = F\n}\n",
            &interner,
        );
        let ir = &e.modules[0].ir;
        let (mut stack, mut kids, mut nested) = (vec![ir.root], Vec::new(), false);
        while let Some(id) = stack.pop() {
            if let Exp::EnumValue { value, .. } = &ir.arena[id].exp {
                nested |= matches!(ir.arena[*value].exp, Exp::EnumValue { .. });
            }
            kids.clear();
            ir.arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
        assert!(nested, "no EnumValue of an EnumValue");
    }

    /// A member function template with no body — an extern class's
    /// prototype, `template <typename T> T Add(T a, T b);` — is a
    /// `Template` over a `FunctionDecl` (`Parser.hs:417-430`), and a call
    /// deduces and instantiates it like any other: `FunctionDeclF` carries
    /// `declScopeName` and `declName` (`Parser/Syntax.hs:556-561`), which
    /// `getQualifier` and `getName` read (`Parser/Syntax.hs:744,764`).
    /// `interface/external_class.k`'s `_adder.Add(a, b)`. Red with the
    /// `FunctionDecl` arm out of `decl_scope` (the call is left undecided).
    #[test]
    fn a_member_template_without_a_body_is_instantiated() {
        let interner = Interner::new();
        let e = run(
            "class Adder\n{\npublic:\n    template <typename T>\n    [[latency(3)]] T Add(T a, T b);\n}\n\n\
             extern Adder;\n\n\
             class Helper\n{\n    Adder _adder;\npublic:\n    uint32 f(uint32 a, uint32 b)\n    {\n        return _adder.Add(a, b);\n    }\n}\n",
            &interner,
        );
        assert_eq!(instances_left(&e), 0);
    }

    /// Incremental inference — the iteration's view read through undo
    /// records, the symbol map kept while no rewrite changes what it files —
    /// is the whole walk: the same iterations to the same trees and types,
    /// on a program whose fixpoint needs both. `B` reads `A` and `E` reads
    /// `B` as the iteration found them, one iteration apart; the `static
    /// if`'s condition folds late, and the branch it takes is the second,
    /// whose `C` the map built before it had filed under the first's.
    /// Red with the view read live (fewer iterations), the map never
    /// rebuilt (`D` typed from the discarded branch's `C`), a unit not
    /// walked again when a declaration it read changed, or a unit's
    /// unresolved count kept past a change (`sig` misread).
    #[test]
    fn incremental_inference_is_the_whole_walk() {
        let text = "const auto A = 4;\nconst auto B = A + 1;\nconst auto E = B + A;\n\nclass K\n{\npublic:\n    uint32 get()\n    {\n        static if (E == 8)\n        {\n            const auto C = E * 200;\n        }\n        else\n        {\n            const auto C = 1;\n        }\n        const auto D = C + B;\n        return D;\n    }\n}\n";
        let interner = Interner::new();
        let [full, incremental] = [true, false].map(|full| {
            let parse = kg_syntax::parse_file(text);
            assert!(parse.errors().is_empty(), "{:?}", parse.errors());
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
            let mut program = Program::new([ir], &interner);
            program.full = full;
            let outcome = program.infer();
            let e = Elaborated {
                modules: std::mem::take(&mut program.modules),
                expansions: Arc::default(),
                report: Report::default(),
                refused: Vec::new(),
                copies: FxHashMap::default(),
                higher_order: Vec::new(),
                snapshot: None,
            };
            (outcome.rounds, outcome.unresolved, fingerprint(&e, &interner))
        });
        assert!(full.0 > 2, "a fixpoint of several iterations: {}", full.0);
        assert_eq!(full, incremental);
    }

    /// The loop as `kg check` runs it — symbol maps kept between builds,
    /// rewrites and the census passing over what holds nothing they act
    /// on, snapshots refreshed from what changed — ends where the whole
    /// walk, which does none of that, ends: instances in a body with no
    /// call, nested in another's arguments, and called. Red when the census
    /// passes over a subtree by the wrong shape.
    #[test]
    fn the_loop_that_passes_over_is_the_loop_that_walks_everything() {
        let text = "template <typename T>\nstruct Box\n{\n    T x;\n}\n\ntemplate <typename T>\ninline T id(T x)\n{\n    return x;\n}\n\ninline void f()\n{\n    Box<uint8> b;\n    Box<Box<uint16>> c;\n}\n\ninline uint32 g()\n{\n    return id<uint32>(3);\n}\n\ninline uint32 h(uint32 y)\n{\n    return id(y);\n}\n";
        let interner = Interner::new();
        let [whole, passing] = [true, false].map(|full| {
            let parse = kg_syntax::parse_file(text);
            assert!(parse.errors().is_empty(), "{:?}", parse.errors());
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
            let e = elaborate_opts(vec![ir], &interner, ROUND_GUARD, full, &kg_base::cancel::Cancel::never());
            (e.report.decided.clone(), fingerprint(&e, &interner))
        });
        assert_eq!(whole.0.iter().sum::<usize>(), 4, "Box<uint8>, Box<uint16>, Box<Box<uint16>>, id<uint32>");
        assert_eq!(whole, passing);
    }

    /// The names every top-level declaration of the module's final tree
    /// carries, in order.
    fn declared(e: &Elaborated, interner: &Interner) -> Vec<String> {
        let ir = &e.modules[0].ir;
        ir.top_level_items()
            .into_iter()
            .filter_map(|id| crate::symbols::declared_name(&ir.arena, id))
            .map(|(_, n)| interner.resolve(n).to_string())
            .collect()
    }

    fn instances_left(e: &Elaborated) -> usize {
        let ir = &e.modules[0].ir;
        let mut stack = vec![ir.root];
        let (mut n, mut kids) = (0, Vec::new());
        while let Some(id) = stack.pop() {
            match &ir.arena[id].exp {
                // A template's own body is not instantiated.
                Exp::Template { .. } => continue,
                Exp::TemplateInstance { .. } => n += 1,
                _ => {}
            }
            kids.clear();
            ir.arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
        n
    }

    /// `instantiate . resolve` (`Template.hs:58`): the decided `Box<uint8>`
    /// is made beside its template, first (`SeqF $ map makeInstance …
    /// ++ [template]`, `:118-121`), and its mention names the copy. Red with
    /// the round's rewrite skipped: no copy, and the mention stays an
    /// instance.
    #[test]
    fn a_decided_instance_is_made_beside_its_template_and_its_mention_named() {
        let interner = Interner::new();
        let e = run("template <typename T>\nstruct Box\n{\n    T x;\n}\n\ninline void f()\n{\n    Box<uint8> b;\n}\n", &interner);
        assert_eq!(declared(&e, &interner), ["Box<|$uint@8|>", "Box", "f"]);
        assert_eq!(instances_left(&e), 0);
        assert_eq!(e.report.rounds, 1);
    }

    /// A function named as a template argument is its bare name once the
    /// arguments are evaluated (`Constexpr.hs:142-143`), and so mangles
    /// `@DoIt`. Red without that arm: the argument stays a named value, the
    /// mangle declines it every round, and the census decides it again —
    /// the loop runs to its guard (`test/logic/lambda_test_cases.k`).
    #[test]
    fn a_function_named_as_a_template_argument_is_decided_once() {
        let interner = Interner::new();
        let e = run(
            "inline uint32 DoIt(uint32 x)\n{\n    return x;\n}\n\ntemplate <(uint32) -> uint32 Fn>\nclass H\n{\npublic:\n    uint32 Do(uint32 x)\n    {\n        return Fn(x);\n    }\n}\n\ninline void g()\n{\n    static H<DoIt> h;\n}\n",
            &interner,
        );
        assert_eq!(e.report.rounds, 1, "{:?}", e.report);
        assert!(declared(&e, &interner).contains(&"H<|@DoIt|>".to_owned()), "{:?}", declared(&e, &interner));
    }

    /// `deduceFromFunctionSignature` (`Type.hs:1451-1478`): a call of a
    /// function template with no written arguments is given the ones its
    /// argument types deduce, and so decides its instance. Red with the
    /// deduction pass skipped: nothing is decided.
    #[test]
    fn a_function_template_call_deduces_its_instance() {
        let interner = Interner::new();
        let e = run(
            "template <typename T>\ninline T id(T x)\n{\n    return x;\n}\n\ninline uint8 f(uint8 y)\n{\n    return id(y);\n}\n",
            &interner,
        );
        assert_eq!(declared(&e, &interner), ["id<|$uint@8|>", "id", "f"]);
    }

    /// `substituteDefault` (`Type.hs:1421-1427`): an instance written short
    /// of its template's parameters is completed with the defaults, each
    /// substituted with the arguments before it. Red with the pass skipped:
    /// the instance is never resolved (`templateArgsResolved` wants every
    /// argument).
    #[test]
    fn an_instance_short_of_its_parameters_takes_the_defaults() {
        let interner = Interner::new();
        let e = run(
            "template <typename T, typename U = T>\nstruct P\n{\n    T a;\n    U b;\n}\n\ninline void f()\n{\n    P<uint8> p;\n}\n",
            &interner,
        );
        assert_eq!(declared(&e, &interner), ["P<|$uint@8$uint@8|>", "P", "f"]);
    }

    /// Every name the final tree declares, anywhere.
    fn all_declared(e: &Elaborated, interner: &Interner) -> Vec<String> {
        let ir = &e.modules[0].ir;
        let mut stack = vec![ir.root];
        let (mut out, mut kids) = (Vec::new(), Vec::new());
        while let Some(id) = stack.pop() {
            if let Some((_, n)) = crate::symbols::declared_name(&ir.arena, id) {
                out.push(interner.resolve(n).to_string());
            }
            kids.clear();
            ir.arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
        out
    }

    /// `addLambdaWrappers` (`Function.hs:366-423`): a function named as a
    /// call's argument is wrapped in a lambda that calls it, named after
    /// its qualified name — and `redundantNameScope` (pass 1) has taken the
    /// module's own scope off that name, so it is `$wrapper$inc`. Red with
    /// the wrappers skipped: no such declaration.
    #[test]
    fn a_function_named_as_an_argument_is_wrapped_in_a_lambda() {
        let interner = Interner::new();
        let e = run(
            "module m { g }\n\ninline uint32 inc(uint32 x)\n{\n    return x + 1;\n}\n\ninline uint32 apply((uint32) -> uint32 f, uint32 x)\n{\n    return f(x);\n}\n\ninline uint32 g()\n{\n    return apply(inc, 1);\n}\n",
            &interner,
        );
        let names = all_declared(&e, &interner);
        assert!(names.iter().any(|n| n.ends_with("$wrapper$inc")), "{names:?}");
    }

    /// `templateResolved`'s dependent kind (`TypeCheck.hs:307-308`): `F fn`
    /// under `F = (uint8) -> uint8` is resolved, so `H<(uint8) -> uint8, g>`
    /// is decided — and the function type's `auto`-free parameters are the
    /// argument's. Red with a dependent kind never resolved: no copy.
    #[test]
    fn a_parameter_of_a_dependent_kind_is_resolved_by_its_argument() {
        let interner = Interner::new();
        let e = run(
            "inline uint8 g(uint8 x)\n{\n    return x;\n}\n\ntemplate <typename F, F fn>\nclass H\n{\n}\n\ninline void f()\n{\n    static H<(uint8) -> uint8, g> h;\n}\n",
            &interner,
        );
        assert_eq!(e.report.made, 1, "{:?}", e.report);
    }

    /// An `auto` inside a dependent kind is `TAuto` there, resolved — the
    /// function type's rule, not `infer AutoF = TUnresolved` — so
    /// `(C, auto) -> auto fn` under `C = uint8` decides (`compose2`'s
    /// higher-order parameters). Red with the `auto` asked for its own
    /// type: never decided.
    #[test]
    fn an_auto_inside_a_dependent_kind_is_resolved() {
        let interner = Interner::new();
        let e = run(
            "inline uint8 g(uint8 c, uint8 x)\n{\n    return x;\n}\n\ntemplate <typename C, (C, auto) -> auto fn>\nclass H\n{\n}\n\ninline void f()\n{\n    static H<uint8, g> h;\n}\n",
            &interner,
        );
        assert_eq!(e.report.made, 1, "{:?}", e.report);
    }

    /// `deduceTemplateArgs` reads `getSymbols` — `AllSymbols`, template
    /// bodies entered (`Template.hs:65,71`) — so a local in a template body
    /// shadows a template of its name, and its use is no call of that
    /// template. Red with inference's map (bodies not entered): the use
    /// becomes `test<>`, which no round decides, left in the copy.
    #[test]
    fn a_local_in_a_template_body_shadows_a_template_of_its_name() {
        let interner = Interner::new();
        let e = run(
            "template <auto N>\ninline void test()\n{\n}\n\ntemplate <typename F>\ninline void fixture(F t)\n{\n    static F test;\n    test = t;\n}\n\ninline void g()\n{\n    fixture<uint8>(1);\n}\n",
            &interner,
        );
        assert_eq!(e.report.left, 0, "{:?}", e.report);
    }

    /// A copy over a template-template argument carries its
    /// `setTypeOfInstance` note: `tArg` of a template is `TArg (showPretty
    /// e)` (`Type.hs:436-437`), held here as the template's path — what the
    /// backend walk names the instance by (`ParseQualifiedName ["T1"]`,
    /// `template-template-parameter.k::0`). Red with the arm gone: the
    /// write is declined and the copy has no note.
    #[test]
    fn a_copy_over_a_template_argument_is_noted_as_an_instance() {
        let interner = Interner::new();
        let e = run(
            "template <typename T>\nstruct S2\n{\n    T x;\n}\n\ntemplate <template <typename> typename TT>\nclass T1\n{\n}\n\ninline void g()\n{\n    static T1<S2> t;\n}\n",
            &interner,
        );
        let m = &e.modules[0];
        let noted: Vec<bool> = m
            .ir
            .top_level_items()
            .into_iter()
            .filter(|&id| matches!(m.ir.arena[id].exp, Exp::Class { .. }))
            .map(|id| match m.types.get(id) {
                kg_ty::Ty::Type(t) => t.is_instance(),
                t => t.is_instance(),
            })
            .collect();
        assert_eq!(noted, [true], "{:?}", declared(&e, &interner));
    }

    /// `validateArg` stamps a literal to the width of its parameter's
    /// *substituted* kind (`TypeCheck.hs:229`): `T a` under `T = uint4`
    /// truncates 78 to 14 before the mangle (`constexpr.k::80`). Red with
    /// only a resolved kind read: `C<|$uint@4@78|>`.
    #[test]
    fn a_literal_argument_is_stamped_by_its_substituted_kind() {
        let interner = Interner::new();
        let e = run(
            "template <typename T, T a>\nclass C\n{\n}\n\ninline void g()\n{\n    static C<uint4, 78> c;\n}\n",
            &interner,
        );
        assert!(declared(&e, &interner).contains(&"C<|$uint@4@14|>".to_owned()), "{:?}", declared(&e, &interner));
    }

    /// …and a kind that is an alias of an instance is seen through, as
    /// `tIsUnsigned` and `sizeOf` see through `TInstance` (`Type.hs:
    /// 653-657,719`): `T a` under `using T = idx<16>` stamps 78 to `uint4`
    /// and truncates it to 14, `kg-oracle --passes 9`'s `C<|@14|>` on this
    /// program (`data/fifo/array.k`'s `fifo_index_t`; `test_cases_24.k`'s
    /// `Test702<UInt<16>, …>`). Red with only a bare integer type stamped:
    /// `C<|@78|>`.
    #[test]
    fn a_literal_argument_is_stamped_through_an_instance_alias() {
        let interner = Interner::new();
        let e = run(
            "template <auto N>\nusing idx = uint<clog2(N)>;\n\nusing T = idx<16>;\n\ntemplate <T a>\nclass C\n{\n}\n\ninline void g()\n{\n    static C<78> c;\n}\n",
            &interner,
        );
        assert!(declared(&e, &interner).contains(&"C<|@14|>".to_owned()), "{:?}", declared(&e, &interner));
    }

    /// An alias spliced into a template argument is a copy of its body
    /// (`resolve e'@TypeIdentifierF{}`, `Constexpr.hs:149-151`, over the
    /// reference's persistent tree): the next round resolves `word_t`
    /// inside the argument's `word_t[4]` and leaves `line_t`'s own body
    /// naming `word_t`, which the walk spells as written
    /// (`data/cache/write_only.k`'s `cache_line_t`, the recorded walk's
    /// `ParseIdentifier "word_t"`). Red with the body's children shared:
    /// the declaration's `word_t` is rewritten to `uint8` in place.
    #[test]
    fn an_alias_spliced_into_an_argument_leaves_its_declaration_as_written() {
        let interner = Interner::new();
        let e = run(
            "using word_t = uint8;\n\nusing line_t = word_t[4];\n\ntemplate <typename T>\nclass H\n{\n    T v;\n}\n\ninline void g()\n{\n    static H<line_t> h;\n}\n",
            &interner,
        );
        let m = &e.modules[0];
        let line_t = interner.intern("line_t");
        let elems: Vec<String> = m
            .ir
            .top_level_items()
            .into_iter()
            .filter_map(|id| match &m.ir.arena[id].exp {
                Exp::Alias { body, name, .. } if m.ir.arena[*name].exp == Exp::Identifier(line_t) => {
                    let Exp::Array { elem, .. } = &m.ir.arena[*body].exp else { return Some("no array".to_owned()) };
                    Some(format!("{:?}", m.ir.arena[*elem].exp).chars().take(14).collect())
                }
                _ => None,
            })
            .collect();
        assert!(declared(&e, &interner).contains(&"H<|$array$uint@8@4|>".to_owned()), "{:?}", declared(&e, &interner));
        assert_eq!(elems, ["TypeIdentifier"]);
    }

    /// A `static for` whose variable reaches no constant position is the
    /// reference's `UnrolledForF` (`Constexpr.hs:54`), whose body `decided`
    /// walks (`Template.hs:106` stops only at a `StaticForF`): the
    /// `id()` deduced there from its assignment's target is instantiated
    /// (`return-type-polymorphism.k::10`). Red with the loop kept a
    /// `StaticFor`: the instance is left undecided.
    #[test]
    fn a_call_in_an_unrolled_loop_body_is_decided() {
        let interner = Interner::new();
        let e = run(
            "template <typename T>\ninline T id()\n{\n    return 0;\n}\n\ntemplate <auto N>\ninline void f()\n{\n    uint8[N] r;\n    static for (const auto i : N)\n    {\n        r[i] = id();\n    }\n}\n\ninline void g()\n{\n    f<4>();\n}\n",
            &interner,
        );
        assert!(declared(&e, &interner).contains(&"id<|$uint@8|>".to_owned()), "{:?}", declared(&e, &interner));
        assert_eq!(e.report.left, 0);
    }

    /// `interpolatedStringPre`'s echo (`Frontend.hs:236-237`) is written
    /// before inference folds the named constant away: `{x=}` is the text
    /// `x = ` and a plain hole. Red with the pass skipped: the hole stays an
    /// echo over a folded literal, which no walk can spell.
    #[test]
    fn an_echo_hole_is_spelled_before_its_name_folds() {
        let interner = Interner::new();
        let e = run("const uint8 x = 3;\n\ninline void g()\n{\n    const auto s = \"{x=}\";\n}\n", &interner);
        let ir = &e.modules[0].ir;
        let holes: Vec<(String, bool)> = crate::elaborate_tests_reachable(ir)
            .into_iter()
            .filter_map(|id| match &ir.arena[id].exp {
                Exp::Interpolated(segs) => segs.first().and_then(|s| s.hole.as_ref().map(|h| (s.text.to_string(), h.echo))),
                _ => None,
            })
            .collect();
        assert_eq!(holes, [("\"x = ".to_owned(), false)]);
    }

    /// A dependent kind is evaluated with its constants folded
    /// (`resolveTemplateParamType`): `uint<clog2(N)> M` under `N = 10` is
    /// `uint4`, resolved, so `Foo<10, 0>` is decided (`template.k::8`). Red
    /// with the kind's expression not folded: never decided.
    #[test]
    fn a_dependent_kind_is_evaluated_with_its_constants_folded() {
        let interner = Interner::new();
        let e = run(
            "template <auto N, uint<clog2(N)> M>\nclass Foo\n{\n}\n\ninline void g()\n{\n    static Foo<10, 0> foo;\n}\n",
            &interner,
        );
        assert_eq!(e.report.made, 1, "{:?}", e.report);
    }

    /// An unrolled iteration's variable is the loop's, renoted
    /// (`renote inductionVar a`, `Constexpr.hs:88-98`): written `const auto`
    /// as the reference keeps it, and typed `const uint5` — the loop
    /// variable's `constIndexType` over 32 — not its literal's `uint1`
    /// (`literal-operand.k::1`). Red with the type spelled into the tree
    /// for a twin, and red with the note not carried through the unrolling.
    #[test]
    fn an_iterations_variable_keeps_the_loops_note() {
        let interner = Interner::new();
        let e = run(
            "inline void g()\n{\n    static for (const auto x : 32)\n    {\n        uint32[x + 1] a;\n    }\n}\n",
            &interner,
        );
        let m = &e.modules[0];
        let x = interner.intern("x");
        let vars: Vec<(String, String)> = crate::elaborate_tests_reachable(&m.ir)
            .into_iter()
            .filter_map(|id| match &m.ir.arena[id].exp {
                Exp::Variable { name, ty, init: Some(_), .. } if m.ir.arena[*name].exp == Exp::Identifier(x) => Some((
                    format!("{:?}", m.ir.arena[*ty].exp).chars().take(5).collect(),
                    kg_ty::subtype::show_pretty(&m.types.get(id), Some(&interner)),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(vars.len(), 32, "{vars:?}");
        assert!(vars.iter().all(|(t, ty)| t == "Const" && ty == "const uint5"), "{vars:?}");
        let written: Vec<bool> = crate::elaborate_tests_reachable(&m.ir)
            .into_iter()
            .filter_map(|id| match &m.ir.arena[id].exp {
                Exp::Variable { name, ty, init: Some(_), .. } if m.ir.arena[*name].exp == Exp::Identifier(x) => {
                    let Exp::Const(inner) = m.ir.arena[*ty].exp else { return Some(false) };
                    Some(m.ir.arena[inner].exp == Exp::Auto)
                }
                _ => None,
            })
            .collect();
        assert!(written.iter().all(|&a| a), "written `const auto`");
    }

    /// A limit that folds in two steps unrolls a round after it folds:
    /// `2*5-1`'s `2*5` is `10` in round one and the subtraction waits for
    /// `10` to be typed (`typeResolved a`, `Constexpr.hs:172`), so the
    /// loop's `const auto` variable is typed `constIndexType 9` before
    /// `staticControl` copies it (`Type.hs:1139-1141`, `Constexpr.hs:52`,
    /// in that order per node, `Inference.hs:50`) —
    /// `const uint4` in every iteration (`data/array.k`'s `intersperse`,
    /// the recorded walk). Red with the limit's value read through the
    /// arithmetic: unrolled a round early, each iteration's variable is
    /// typed by its own literal, `uint1` to `uint4`.
    #[test]
    fn a_limit_folded_in_two_steps_types_every_iterations_variable() {
        let interner = Interner::new();
        let e = run(
            "inline void g()\n{\n    uint8[16] r;\n    static for (const auto x : 2*5-1)\n        if (static(x % 2 == 0))\n            r[x] = 1;\n}\n",
            &interner,
        );
        let m = &e.modules[0];
        let x = interner.intern("x");
        let vars: Vec<String> = crate::elaborate_tests_reachable(&m.ir)
            .into_iter()
            .filter_map(|id| match &m.ir.arena[id].exp {
                Exp::Variable { name, init: Some(_), .. } if m.ir.arena[*name].exp == Exp::Identifier(x) => {
                    Some(kg_ty::subtype::show_pretty(&m.types.get(id), Some(&interner)))
                }
                _ => None,
            })
            .collect();
        assert!(vars.iter().all(|ty| ty == "const uint4"), "{vars:?}");
    }

    /// `reifyEnums` (`Frontend.hs:195-205`): a constant with no value gets
    /// the previous one's plus one, each value its own copy of the previous
    /// expression, so the last of a chain folds a round after the one
    /// before it — as the reference's persistent tree does. Red with the
    /// pass skipped: `C` has no value and folds to nothing; red with the
    /// previous value shared: the chain's nodes overlap.
    #[test]
    fn an_enum_constant_without_a_value_is_the_previous_plus_one() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file("enum E : uint2\n{\n    A,\n    B,\n    C\n}\n\nconst auto c = E::C;\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let mut irs = vec![ir];
        crate::pass1::reify_enums(&mut irs, &interner);
        let ir = &irs[0];
        let values: Vec<kg_hir::ExpId> = crate::elaborate_tests_reachable(ir)
            .into_iter()
            .filter_map(|id| match ir.arena[id].exp {
                Exp::EnumConstant { value: Some(v), .. } => Some(v),
                _ => None,
            })
            .collect();
        assert_eq!(values.len(), 3);
        let mut seen = std::collections::HashSet::new();
        for v in &values {
            let mut stack = vec![*v];
            let mut kids = Vec::new();
            while let Some(id) = stack.pop() {
                assert!(seen.insert(id.index()), "a value shares a node with another");
                kids.clear();
                ir.arena[id].exp.children(&mut kids);
                stack.extend(kids.iter().copied());
            }
        }
        let e = run("enum E : uint2\n{\n    A,\n    B,\n    C\n}\n\nconst auto c = E::C;\n", &interner);
        let m = &e.modules[0];
        let folded: Vec<String> = crate::elaborate_tests_reachable(&m.ir)
            .into_iter()
            .filter_map(|id| match m.ir.arena[id].exp {
                Exp::EnumValue { value, .. } => Some(format!("{:?}", m.ir.arena[value].exp)),
                _ => None,
            })
            .collect();
        assert_eq!(folded, ["IntLiteral { ty: None, value: 2 }"]);
    }

    /// `indirectInstances` (`Template.hs:98-100`): the instance a
    /// parameter's kind holds, substituted — `index_t<10>` in `index_t<N> M`
    /// under `N = 10` — is decided out of the kind, and the outer instance
    /// once the copy exists (`resolveTemplateInstance`), a round later
    /// (`template.k::7`). Red with the kind's instances not censused: neither
    /// is made.
    #[test]
    fn an_instance_in_a_parameters_kind_is_decided_out_of_it() {
        let interner = Interner::new();
        let e = run(
            "template <auto N>\nusing index_t = uint<N>;\n\ntemplate <auto N, index_t<N> M>\nclass Foo\n{\n}\n\ninline void g()\n{\n    static Foo<10, 0> foo;\n}\n",
            &interner,
        );
        let names = declared(&e, &interner);
        assert!(names.contains(&"index_t<|@10|>".to_owned()), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("Foo<|")), "{names:?}");
        assert_eq!(e.report.rounds, 2, "{:?}", e.report);
    }

    /// Pass 7, `trimExternClasses` (`Frontend.hs:315-335`): an `extern`
    /// class's functions keep their signatures and return `{}` (or nothing,
    /// for `void`). Red with the pass skipped: `Do` keeps its body.
    #[test]
    fn an_extern_classs_functions_are_trimmed() {
        let interner = Interner::new();
        let e = run(
            "class Foo\n{\npublic:\n    uint32 Do(uint32 x)\n    {\n        return x + 1;\n    }\n\n    void Run()\n    {\n        Do(1);\n    }\n}\n\nextern Foo;\n",
            &interner,
        );
        let ir = &e.modules[0].ir;
        let bodies: Vec<String> = crate::elaborate_tests_reachable(ir)
            .into_iter()
            .filter_map(|id| match &ir.arena[id].exp {
                Exp::Function { body, .. } => {
                    let Exp::Seq(items) = &ir.arena[*body].exp else { return None };
                    Some(items.iter().map(|&i| match &ir.arena[i].exp {
                        Exp::Return(Some(v)) => format!("return {:?}", std::mem::discriminant(&ir.arena[*v].exp) == std::mem::discriminant(&Exp::InitializerList(*v))),
                        Exp::Return(None) => "return".to_owned(),
                        _ => "other".to_owned(),
                    }).collect::<Vec<_>>().join(";"))
                }
                _ => None,
            })
            .collect();
        let mut bodies = bodies;
        bodies.sort();
        assert_eq!(bodies, ["return", "return true"]);
    }

    /// `eval (ScopedNameF a (Just b) c) | tIsClass (theType b)`
    /// (`Constexpr.hs:159-160`): a name reached through a class — here an
    /// alias of a copy — is qualified by the class's own name, which the
    /// census then finds its member template under
    /// (`template-instance-identifiers.k::35`). Red without the arm: the
    /// member instance is left undecided.
    #[test]
    fn a_member_template_reached_through_an_alias_of_a_copy_is_decided() {
        let interner = Interner::new();
        let e = run(
            "template <typename T>\nclass B\n{\npublic:\n    template <typename S>\n    class A\n    {\n    public:\n        using A1_t = S;\n    }\n}\n\nusing Bb = B<bool>;\nconst Bb::A<uint32>::A1_t x = 3;\n",
            &interner,
        );
        assert_eq!(e.report.left, 0, "{:?}", e.report);
    }

    /// A constant declared with an alias copy's type (`count_t<N>`) folds to
    /// its literal: `tIsInt`, `castIntegralConstexpr` see through the
    /// instance to `uint4` (`local-functions.k::29`). Red with the declared
    /// type read as written: the argument never resolves.
    #[test]
    fn a_constant_of_an_alias_copys_type_folds() {
        let interner = Interner::new();
        let e = run(
            "template <auto N>\nusing cnt = uint<N>;\n\nconst cnt<4> i = 3;\n\ntemplate <auto M>\nclass Z\n{\n}\n\ninline void g()\n{\n    static Z<i> z;\n}\n",
            &interner,
        );
        assert!(declared(&e, &interner).iter().any(|n| n.starts_with("Z<|")), "{:?}", declared(&e, &interner));
    }

    /// A named string constant folds to its literal (`isLiteral v`,
    /// `Constexpr.hs:290`), which resolves the argument
    /// (`constexprResolved`) — `hal::fadd32<config::hal_device_family, 3>`.
    /// Red with only numeric and boolean initializers folded: the instance
    /// is never decided.
    #[test]
    fn a_named_string_constant_argument_decides_its_instance() {
        let interner = Interner::new();
        let e = run(
            "const string family = \"mock\";\n\ntemplate <string F, auto L>\nclass C\n{\n}\n\ninline void g()\n{\n    static C<family, 3> c;\n}\n",
            &interner,
        );
        assert_eq!(e.report.made, 1, "{:?} {:?}", e.report, declared(&e, &interner));
        assert_eq!(instances_left(&e), 0);
    }

    /// A cast to an alias copy's type folds through the copy's instance to
    /// the integer under it — `cast<count_t<5>>(5)` (`template.k::35`) — and
    /// so resolves the argument it stands in. Red with the cast's type read
    /// as it stands: `castIntegralConstexpr` meets a `TInstance`, nothing
    /// folds, and `Z` is never decided.
    #[test]
    fn a_cast_to_an_alias_copys_type_folds() {
        let interner = Interner::new();
        let e = run(
            "template <auto N>\nusing cnt = uint<N>;\n\ntemplate <auto M>\nclass Z\n{\n}\n\ninline void g()\n{\n    static Z<cast<cnt<3>>(5)> z;\n}\n",
            &interner,
        );
        let names = declared(&e, &interner);
        assert!(names.iter().any(|n| n.starts_with("Z<|")), "{names:?} {:?}", e.report);
    }

    /// An instance in a parameter's kind whose own kinds hold an instance —
    /// `Foo<T, 100> N` where `Foo`'s kind is `index_t<bitsizeof(T)>` — is
    /// met by `para decided` over the substituted kind as any other: not
    /// resolved itself, the instance in its kind comes first
    /// (`Template.hs:98-100`, `template.k::33`). Red with the census not
    /// descending into the inner template's kinds: `index_t<6>` is never
    /// made and `Bar` is left.
    #[test]
    fn an_instance_in_the_kind_of_a_kinds_instance_is_decided() {
        let interner = Interner::new();
        let e = run(
            "template <auto N>\nusing index_t = uint<N>;\n\ntemplate <typename T, index_t<bitsizeof(T)> N>\nusing Foo = index_t<N>;\n\ntemplate <typename T, Foo<T, 100> N>\nclass Bar\n{\n}\n\ninline void g()\n{\n    static Bar<uint7, 101> bar;\n}\n",
            &interner,
        );
        let names = declared(&e, &interner);
        assert!(names.contains(&"index_t<|@7|>".to_owned()), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("Bar<|")), "{names:?} {:?}", e.report);
        assert_eq!(e.report.left, 0, "{:?}", e.report);
    }

    /// `validateArg` (`TypeCheck.hs:228-293`) turns an argument it refuses
    /// into an error, so the instance is never decided: a function not of
    /// the parameter's substituted type — `F1 : (uint14) -> uint13` for
    /// `(T) -> U` under `U = uint12` (`template-param-default.k::13`) — and a
    /// value for a `typename` parameter. Red with the check skipped: both
    /// copies are made.
    #[test]
    fn an_argument_validate_arg_refuses_makes_no_copy() {
        let interner = Interner::new();
        let e = run(
            "inline uint13 F1(uint14 x)\n{\n    return x;\n}\n\ntemplate <typename T, typename U, (T) -> U Fn>\ninline U Bar(T x)\n{\n    return Fn(x);\n}\n\ntemplate <typename T>\nstruct Box\n{\n    T x;\n}\n\ninline void g()\n{\n    const auto x = Bar<uint14, uint12, F1>(10);\n    Box<3> b;\n}\n",
            &interner,
        );
        let names = declared(&e, &interner);
        assert!(!names.iter().any(|n| n.contains("<|")), "{names:?}");
        assert_eq!(e.report.invalid, 2, "{:?}", e.report);
    }

    /// `validateArg`'s constant arm (`TypeCheck.hs:287-293`) for a written
    /// literal — a `5` given to a `bool` parameter, whose type `uint3` is no
    /// subtype (`nested-templates.k::12`) — and the function arm with the
    /// parameter's flags kept: `Do : (bool) -> void` is no
    /// `([[last]] bool) -> void` (`Type.hs:333`,
    /// `higher-order-functions.k::189`). Their well-typed twins, `true` and
    /// a `[[last]]` function, are made. Red with the literal arm unasked, or
    /// the kind's flags dropped.
    #[test]
    fn a_literal_or_a_flag_validate_arg_refuses_makes_no_copy() {
        let interner = Interner::new();
        let e = run(
            "template <bool B>\nstruct Flag\n{\n}\n\ninline void Do(bool b)\n{\n}\n\ninline void Last([[last]] bool b)\n{\n}\n\ntemplate <([[last]] bool) -> void F>\nclass H\n{\n}\n\ninline void g()\n{\n    Flag<5> a;\n    Flag<true> b;\n    static H<Do> c;\n    static H<Last> d;\n}\n",
            &interner,
        );
        let mut made: Vec<String> = declared(&e, &interner).into_iter().filter(|n| n.contains("<|")).collect();
        made.sort();
        assert_eq!(made, ["Flag<|@True|>", "H<|@Last|>"], "{:?}", e.report);
        // The two refused, left as written; `invalid` counts per census.
        assert_eq!(e.report.left, 2, "{:?}", e.report);
    }

    /// The layered loop's first answer is the bounded loop's, and its last
    /// the fixpoint's, from one run (sprint W7.6): `Down<6>` reaches
    /// `Down<0>` at round 7, so a bound of 4 leaves instances standing
    /// and the fork must be ended as the bounded run ends itself —
    /// deduplicated, the bound reported, what was left counted, the
    /// trajectory so far. Both inference walks, since the fork carries the
    /// incremental one's caches only as their emptied shells. Red with any
    /// of those left out of `Stopped::answer`, or the hook called a round
    /// early.
    #[test]
    fn the_first_answer_is_the_bounded_loop_s_and_the_last_the_fixpoint_s() {
        let text = "template <auto N>\ninline void Down()\n{\n    bool[N] x;\n    static if (N > 0)\n    {\n        Down<N - 1>();\n    }\n}\n\ninline void main()\n{\n    Down<6>();\n}\n";
        let interner = Interner::new();
        let ir = || {
            let parse = kg_syntax::parse_file(text);
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports)
        };
        let never = kg_base::cancel::Cancel::never();
        for full in [false, true] {
            let mut firsts = Vec::new();
            let last = elaborate_layered(vec![ir()], &interner, ROUND_GUARD, 4, full, &never, &mut |stopped| {
                firsts.push((stopped.round(), fingerprint(&stopped.answer(), &interner)));
            });
            let bounded = elaborate_opts(vec![ir()], &interner, 4, full, &never);
            assert!(bounded.report.budget_exhausted && bounded.report.left > 0, "the bound must stop this loop: {:?}", bounded.report);
            assert_eq!(firsts.len(), 1, "one first answer");
            assert_eq!(firsts[0].0, 4);
            assert!(firsts[0].1 == fingerprint(&bounded, &interner), "full {full}: the first answer is not the bounded loop's");
            let fixpoint = elaborate_opts(vec![ir()], &interner, ROUND_GUARD, full, &never);
            assert_eq!(fixpoint.report.rounds, 7, "{:?}", fixpoint.report);
            assert!(fingerprint(&last, &interner) == fingerprint(&fixpoint, &interner), "full {full}: the last answer is not the fixpoint's");
        }
        // A loop that ends by the bound has one answer, its last.
        let mut called = false;
        let last = elaborate_layered(vec![ir()], &interner, ROUND_GUARD, 7, false, &never, &mut |_| called = true);
        assert!(!called && !last.report.budget_exhausted, "{:?}", last.report);
    }

    /// The editor's bound stops the loop short and says so; the fixpoint
    /// does not report one. `Foo<10, 0>` needs two rounds (the kind's
    /// `index_t<10>` first). Red with the flag unset at the bound.
    #[test]
    fn a_round_bound_stops_the_loop_and_reports_it() {
        let text = "template <auto N>\nusing index_t = uint<N>;\n\ntemplate <auto N, index_t<N> M>\nclass Foo\n{\n}\n\ninline void g()\n{\n    static Foo<10, 0> foo;\n}\n";
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let bounded = elaborate_with(vec![ir], &interner, 1);
        assert!(bounded.report.budget_exhausted, "{:?}", bounded.report);
        assert_eq!(bounded.report.rounds, 1, "{:?}", bounded.report);
        let whole = run(text, &interner);
        assert!(!whole.report.budget_exhausted, "{:?}", whole.report);
        assert_eq!(whole.report.rounds, 2, "{:?}", whole.report);
    }
}
