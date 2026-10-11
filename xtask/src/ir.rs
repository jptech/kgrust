//! `cargo xtask ir` — the three gates over `kg-hir`'s lowering.
//!
//! * **Totality** — every file the parser accepts lowers with zero
//!   `Exp::Unsupported`; that half is the hard gate. Files the parser
//!   error-recovers are *reported* instead: their `Unsupported` nodes are the
//!   recovered forms of deliberately invalid snippet fragments (`reorder bool
//!   x;`, `[[name(10)]]`, a nested `union`) — text the reference rejects at
//!   parse time and has no `ExpF` for, so "zero" is not a meaningful demand
//!   there and `Unsupported` is doing exactly its counting job. The count is
//!   reported per offending `SyntaxKind`, so a miss is a work item, not a
//!   mystery.
//! * **Round trip** — `lower(parse(pretty(lower(parse(f)))))` is structurally
//!   equal to `lower(parse(f))`. Not byte-for-byte: the IR carries no trivia,
//!   and desugared forms print in their lowered spelling. It runs only over
//!   files whose parse has **zero errors**, for the same reason as above.
//!   What it pins is printer↔lowering agreement — a field the printer loses,
//!   an ordering the two sides read differently. A bug that drops the same
//!   field *deterministically in the lowering itself* appears on both sides
//!   of the comparison and is invisible here; the oracle differential below
//!   is the gate that catches that class.
//! * **Sorted trip** — the same equality over **`lower_sorted`**, printed by
//!   `pretty_with_source`. The sorter's name shapes carry prefix chains that
//!   interleave written segments with materialised ones, and no per-node
//!   field separates the two (`kg-hir/src/pretty.rs`, the module comment), so
//!   the printer slices each sorted name's own file text instead of
//!   reconstructing it; this line is what grades that. It is the ground the
//!   materialisation loop stands on — an instantiated body is printed,
//!   reparsed and inferred with the existing engine, so the printed form must
//!   relower to the very shape it was printed from. Plain `pretty` reads
//!   188/394 here; the source-sliced printing is what closed it.
//! * **Provenance** — every `Src::head` resolves to a live CST node of the
//!   original tree; every `written == Own` node has a node at exactly that
//!   range whose kind matches; and no two structurally *different* `Own`
//!   nodes claim the same `(range, kind)` — that pair is the future `TypeKey`
//!   (`kg-ty/src/table.rs`), and a shared key over different shapes
//!   is precisely the collision divergence #21 warns about. Equal-shape
//!   claimants are allowed: `x += y` reads the written lvalue twice
//!   (`Parser.hs:631-638`), and both readings are the user's own text. That
//!   exception is **unexercised** since the re-read started donating
//!   (divergence #24) — 0 keys corpus-wide have two claimants at all — and is
//!   kept because what it guards is the shape comparison, not the count.
//!
//!   It and the four rewrite gates run over **`lower_sorted`**, not `lower`:
//!   the sorted tree is what the oracle differential grades and what every
//!   rewrite will run on, so it is the one whose provenance has to hold. That
//!   was blocked until divergence #24 closed — a method callee's leaf
//!   `Identifier` and the `QualifiedIdentifier` round it claimed one
//!   `(range, kind)`, and 30 library modules failed clause three on it alone.
//!
//! Slice 2 added three more over a second census of `decidedInstances`
//! (`kg_db::instances` as it was, `Template.hs:85-108`) — a differential of
//! two readings of `templateResolved`, its zero-diagnostics gate and its
//! statistics — and slices 6 and 7 graded `resolve`, the mangler and
//! `instantiate` against the reference with that census as their input.
//! Sprint W3.7 deleted the second census, the independent reading
//! `docs/plans/single-source-of-truth.md`'s register kept until M6, and the
//! five lines with it: the loop's census is the one census, and its
//! `resolve` and `instantiate` are graded end to end by `ptdiff` (every
//! mangled name the backend is told), `backend`, `output` and `loop-left`.
//!
//! Slice 3 adds the **oracle differential**: every `library/` file, lowered
//! sorted (`kg_db::sorted::sorted_program`), compared node-for-node against
//! the tree the *reference's own parser* holds (`kg-oracle --tree`). Its two
//! sides share no code at all, so it sees the deterministic-lowering-bug
//! class every other gate is blind to. Without the oracle binary
//! (`oracle/build.sh`) it fails: an instrument absent is red, never skipped.
//!
//! Slice 4 adds the **lambda ledger**, `desugarLambdas`' equivalent of the
//! `capture diff` line — but a signature rather than a count. The pass mints
//! two declarations per lambda (`Function.hs:237-262`), and the reference
//! emits them adjacently, so the whole of what it did to one lambda reads off
//! the tree as `(name, capture arity, parameter count, generic?, uncapture
//! field indices)`. A count could not be used here: `addLambdaWrappers` is
//! sub-pass 1 and is *not* ported, so it synthesizes 27 extra `library/`
//! lambdas this port will never hold, and every whole-file count of what
//! sub-pass 2 added is inflated by them. The ledger drops them by name
//! (`$wrapper$…`, `Function.hs:400`) and compares only the lambdas both sides
//! can see.
//!
//! Both reference-graded lines run over **`library/` and `test/`** — 351 roots
//! the oracle parses, against `oracle diff`'s 124 — because `library/` holds
//! only 119 of the corpus's 1306 lambdas, so a gate confined to it grades
//! `desugarLambdas` on a twentieth of its own blast radius. Widening took
//! `lambda diff` from 114 rewritten lambdas to **890** and immediately found a
//! defect in the gate rather than in the pass: `survivors` was counting
//! `addLambdaWrappers`' own `$wrapper$` lambdas, so on
//! `test/library/processor/risc-v.k` the reference claimed a survivor
//! (`$wrapper$CustomDecode`, from `decode_instr(LUI, CustomDecode)` passing a
//! bare function name) that this port can never hold. The tree comparison
//! stays on `library/`: it is specified against `oracle/differential.py`'s
//! root selection, and `test/`'s multi-fixture files — separated by
//! `expected:N`, which is not a program — are what it was never defined over.
//!
//! **What the two reference-graded lines do *not* give you is independent
//! detection.** Measured, on the one plant available: `captureThis` narrowed to
//! fire only on lambdas that already capture something moves both to exactly
//! **298/351**. `lambda diff` sees it because the implicit `this` is capture 0
//! and therefore part of every in-class lambda's `make_captureN` arity. So
//! `capture diff`'s distinct value is not extra arity coverage — it is the
//! `--passes 3` boundary, which is independent of pass 8 entirely, and the
//! lambdas `desugarLambdas` never rewrites and so never enters in the ledger.
//! Both are kept for that reason and not for the count.
//!
//! Slice 5 adds the **move ledger**, `moveFunctions`' grade, at
//! `--template-passes 3` vs `4`. Same instrument family as the lambda ledger
//! and for the same reason — `addLambdaWrappers` is still unported and 850 of
//! the 1536 declarations the reference mangles are its `$wrapper$` output — but
//! the ledger is a **partition**, not a list: every `Function`/`Class` the pass
//! could move is either a *moved* entry (mangled name, new landing scope) or a
//! *stuck* one (still nested, under one of the five deferral arms), and both
//! multisets are compared. That partition is what holds the `$wrapper$`
//! exclusion down: the exclusion drops entries **by name** from the reference's
//! half, and `stuck` is the complement it cannot touch, so a move this port
//! skips reappears as a `stuck` the reference does not have.
//!
//! **What it grades and what it does not.** The *hoist* is gradeable
//! whole-corpus by this ledger; the *re-qualification* is not. Three of the
//! four re-qualification behaviours ride along as entry fields — the
//! `$this.<member>` rewrite (`Function.hs:52-56`) as a multiset of member
//! names, the `thisMethod` re-point (`:58-62`) as a count, and the mangled
//! identifiers `qualifyName`/`fixScopedName` leave behind (`:69-81`) as a
//! count — each attributed to the innermost enclosing declaration so that a
//! nested declaration's rewrites belong to *it*. Four things are invisible and
//! stay invisible:
//!
//! * `memberAccess`'s const-inlining (`:55`), which splices an initializer out
//!   of a class declaration that survives and leaves nothing behind saying a
//!   splice happened rather than the user writing the same expression;
//! * the *content* of a qualifier `qualifyName` builds. The obvious field —
//!   how many `ScopedName`s carry an explicit qualifier at all — was built,
//!   run whole-corpus and thrown away: it disagrees on 75 of 351 roots with
//!   ours systematically higher (2378 against 1338 over those roots) because
//!   `kg_db::sorted` qualifies names the reference leaves bare. It graded the
//!   sorter. [`MoveEntry::mangled_refs`] is the part that survives;
//! * the **order** of the moved declarations inside their landing `Seq`, which
//!   this ledger sorts away on purpose — pinned by the donor unit tests
//!   instead. Measured: reversing the emission order leaves the line and its
//!   whole per-behaviour histogram byte-identical;
//! * every `Src` donor, per the house rule — measured here too, by giving the
//!   spliced member-name `Identifier` the call site's `Src` instead of the
//!   class declaration's own name node (survey donor E3): byte-identical.
//!
//! So are the five arms the corpus never reaches (`Function.hs:92`, `:99`,
//! `:104-117`), which only unit tests can hold.
//!
//! **Booked, not failed — and what stops that being a suppressed failure.**
//! 12 of the 351 roots disagree for a reason that is not this pass: 6 because
//! `localMemberFunctions` (pass 1) has already promoted a `funcKind` the
//! reference reads back out, 6 because `higherOrderFunctions` (pass 7) has
//! already wrapped a declaration in a `Template` this port has no way to hold.
//! `oracle diff` is the precedent — it prints 116/124 and does not bail,
//! because its 8 are booked with named causes. The rule those bookings follow
//! here is that **the predicate is the missing pass's own guard**, evaluated
//! against our pre-move tree ([`higher_order_sites`]), never the shape of the
//! disagreement; and that the difference it is allowed to explain is bounded by
//! the same partition the whole ledger rests on — an absent `Template` can only
//! move declarations out of the reference's `stuck` and into our `entries`,
//! never the reverse and never out of thin air. Three plants that produce
//! *booked-looking* differences for reasons that are not those passes — hoist
//! every class member, promote every moved function to `MemberFunction`, hoist
//! nothing at all — are each still red. The printed line names the pass and the
//! count against it, so `localMemberFunctions` or `higherOrderFunctions`
//! landing is expected to move a known number of roots to `ok` rather than to
//! quietly change what the gate means.
//!
//! Measured, by planting three ports into `moveFunctions` and re-running the
//! whole corpus. Hoisting every class member out of its class
//! (`Select::FunctionsAndClasses` in the `Class` arm): **201/351**,
//! `hoisted-too-many 145`, bookings *down* to 5. Promoting every moved function
//! to `MemberFunction` in `move_one`: **274/351**, `kind 17` (and
//! `hoisted-too-many 56`, since `funcKind` feeds the next arm's `selects`),
//! bookings down to 4. Hoisting nothing at all: **272/351**,
//! `hoisted-too-few 76`, bookings down to 3. In each case the count of booked
//! roots *falls* rather than absorbing the plant, which is the property that
//! matters: the predicates are conditions on the missing pass, so a port that
//! merely resembles one cannot buy itself a booking. A fourth plant, written
//! first, was **inert** — it defined the promotion function without calling
//! it, and came back green at 339/351 with the histogram byte-identical to the
//! unplanted run. That is the reminder that a plant proves nothing until it is
//! confirmed to have fired.
//!
//! **Read the histogram, not the count.** Against a partial port the root count
//! saturates: a stub that hoists correctly but does no `qualifyName` is red on
//! 76 of the 77 roots that hold a move, and a plant that then removes the
//! *hoist entirely* leaves the count at exactly 272/351 while flipping every
//! entry in `first disagreement, by behaviour` from `mangled-refs 59,
//! this-members 7, this-calls 6` to `hoisted-too-few 76`. That is why the line
//! prints the histogram: the count is only informative once the pass is whole.
//!
//! **This line is independent of the other two, and that cuts both ways.** The
//! plant that moves `capture diff` and `lambda diff` to the *same* 298/351 —
//! `captureThis` narrowed to fire only on lambdas that already capture
//! something — leaves `move diff` at **339/351 with the same 12 bookings and 0
//! DIFF**: not a smaller change, no change at all. No field here reads a
//! capture arity, and `this_members`/`this_calls` come from `memberAccess`
//! (`Function.hs:52-62`) reading the symbol map, which that plant does not
//! touch. So this is a third reading rather than a fourth copy of the same one
//! — and it is stated the other way round in the same breath: this line is
//! **blind** to a capture-arity bug. `capture:` (the self-report, 62 files red
//! under that plant) is what covers it.
//!
//! The oracle fan-outs cost 23.6s, 42.3s, 44.5s and 44.4s. They shared one
//! timer until the widening, at which point `oracle diff` appeared to have
//! tripled; it had not — it was being charged for the other two.
//!
//! Deliberately a separate file from `corpus.rs`: that file is the scoreboard
//! and is not edited; the ~15 lines of file collection are repeated here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use kg_base::{FileId, Interner, Written};

use kg_syntax::{AstNode, SyntaxKind, ast};

pub fn run(kanagawa_root: &Path, verbose: bool) -> Result<()> {
    let mut files = Vec::new();
    for dir in ["library", "test", "doc", "tools"] {
        collect(&kanagawa_root.join(dir), &mut files);
    }
    files.sort();
    if files.is_empty() {
        anyhow::bail!("no .k files under {}", kanagawa_root.display());
    }

    let interner = Interner::new();
    let lowering_started = std::time::Instant::now();

    let mut total_files = 0usize;
    let mut clean_total = 0usize;
    let mut recovered_total = 0usize;
    let mut unsupported_total = 0usize;
    let mut unsupported_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut unsupported_files: Vec<(PathBuf, usize)> = Vec::new();
    let mut clean_unsupported: Vec<(PathBuf, usize)> = Vec::new();

    let mut clean = 0usize;
    let mut round_trip_ok = 0usize;
    let mut round_trip_failures: Vec<(PathBuf, String)> = Vec::new();
    let mut sorted_trip_ok = 0usize;
    let mut sorted_trip_failures: Vec<(PathBuf, String)> = Vec::new();

    let mut provenance_ok = 0usize;
    let mut provenance_failures: Vec<(PathBuf, String)> = Vec::new();

    let mut rewrite_ok = 0usize;
    let mut rewrite_fired = 0u32;
    let mut rewrite_failures: Vec<(PathBuf, String)> = Vec::new();
    let mut capture_ok = 0usize;
    let mut capture_fired = 0u32;
    let mut capture_failures: Vec<(PathBuf, String)> = Vec::new();
    let mut lambdas_ok = 0usize;
    let mut lambdas_fired = 0u32;
    let mut lambdas_failures: Vec<(PathBuf, String)> = Vec::new();
    let mut moves_ok = 0usize;
    let mut moves_fired = 0u32;
    let mut moves_failures: Vec<(PathBuf, String)> = Vec::new();

    for path in &files {
        // The corpus contains one deliberately non-UTF-8 fixture.
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        total_files += 1;

        let parse = kg_syntax::parse_file(&text);
        let source =
            ast::SourceFile::cast(parse.syntax_node()).expect("the root is always SOURCE_FILE");
        let ir = kg_hir::lower(&source, FileId(0), &interner);

        // --- totality ---
        let unsupported = ir.unsupported();
        if parse.errors().is_empty() {
            if unsupported.is_empty() {
                clean_total += 1;
            } else {
                clean_unsupported.push((path.clone(), unsupported.len()));
            }
        } else if unsupported.is_empty() {
            recovered_total += 1;
        }
        if !unsupported.is_empty() {
            unsupported_total += unsupported.len();
            unsupported_files.push((path.clone(), unsupported.len()));
            for (kind, _) in &unsupported {
                *unsupported_by_kind.entry(format!("{kind:?}")).or_default() += 1;
            }
        }

        // --- provenance and the rewrites, over the **sorted** lowering ---
        //
        // `lower_sorted` is the lowering the oracle differential grades and the
        // one every rewrite will eventually run on, so it is the one whose
        // provenance has to hold. It could not be graded here until divergence
        // #24 was closed: the sorted lowering used to give a method callee's
        // leaf `Identifier` and the `QualifiedIdentifier` round it the same
        // `(range, kind)`, both `Own`, and 30 library modules failed clause
        // three for that reason alone. The leaf now donates
        // (`kg-hir/src/lower.rs`, `donate_inner_claims`); the shapes are
        // untouched, because the reference builds them
        // (`Parser.hs:644,655`).
        //
        // `NoImports`, not the loaded program: what a symbol context changes is
        // *which* sorted arm a name takes and which qualifier it is given, and
        // that half is graded node-for-node against the reference by
        // `oracle diff` below, over `kg_db::sorted`. What this loop needs is
        // the sorted *construction* — the wrappers, the materialised prefixes
        // and the claims they stamp — which every file gets either way, over
        // 479 files rather than 124 and with no module graph to load. Round
        // trip and totality stay on `lower`; the sorted lowering's own round
        // trip is `sorted trip:` below, over `pretty_with_source` — plain
        // `pretty` prints a sorted name leaf-only and reads 188/394 here.
        let sorted =
            kg_hir::lower_sorted(&source, FileId(0), &interner, &kg_hir::NoImports);
        let root = source.syntax().clone();
        let open = |f: FileId| (f == ir.file).then(|| root.clone());
        match check_provenance(&sorted, &mut CstIndex::new(&open), &interner) {
            Ok(()) => provenance_ok += 1,
            Err(msg) => provenance_failures.push((path.clone(), msg)),
        }

        // --- the rewrite substrate, over the same lowering ---
        //
        // `abbreviatedFunctionTemplate` is wired into no pass; this is the only
        // thing that runs it over the corpus, and running its output through
        // the same provenance invariant is what makes the four clauses above
        // evidence rather than decoration.
        //
        // **This line is a self-report, not a graded score.** It says the pass
        // ran to completion, kept the tree's provenance sound and touched
        // nothing outside an abbreviated declaration. It does *not* compare the
        // 65 against the reference — and the reference does not agree with it:
        // it fires on 12 declarations we decline (desugared generic lambdas,
        // which need `desugarLambdas`) and declines 4 we fire on
        // (`library/data/function.k`'s `apply`/`bind`/`bind1st`/`bind2nd`,
        // which `higherOrderFunctions` has already wrapped in a `Template` by
        // the time this sub-pass runs there, so its clause 2 reverts). Both
        // gaps are unported *upstream* sub-passes, not clause bugs — the
        // clauses are graded against the reference by the unit tests. Grading
        // the count wants the oracle differential run at
        // `--template-passes 2` vs `3`; see `docs/history/roadmap-history.md`. Over
        // `lower_sorted`, with the provenance gate above and for the same
        // reason: a rewrite that will run on sorted input is not graded by
        // running it on written input. Measured across the move, every count
        // this loop prints is byte-identical — sorting rewrites *names*, and
        // all four of these passes fire on declarations. A count that does
        // move on some later change is therefore the sorting becoming visible
        // to a pass, which is worth knowing.
        let rewritten = kg_hir::rewrite::abbreviated_function_template(&sorted, &interner);
        rewrite_fired += fired(&sorted, &rewritten);
        match check_provenance(&rewritten, &mut CstIndex::new(&open), &interner) {
            Ok(()) => rewrite_ok += 1,
            Err(msg) => rewrite_failures.push((path.clone(), msg)),
        }
        // The rewrite must not reach a node it was not asked to: everything
        // outside an abbreviated declaration keeps its id, and no `Template`
        // node is rebuilt at all. The second check is not redundant — a broken
        // revert is caught by the `TypeKey` clauses only where the rewritten
        // declaration's parent is *not* a `Seq`, which is 12 of the 14 corpus
        // files it damages; the two it misses are the class-template members,
        // the sharpest case.
        if let Err(msg) = unchanged_outside(&sorted, &rewritten, &interner) {
            rewrite_failures.push((path.clone(), msg));
        }
        if let Err(msg) = kg_hir::rewrite::abbreviated::templates_are_untouched(&sorted, &rewritten) {
            rewrite_failures.push((path.clone(), msg));
        }

        // `captureThis` is frontend pass 3, five passes ahead of the one above,
        // so it is run over its own fork and graded on its own line rather than
        // folded into `rewrite:`. Two things would be lost by merging them: the
        // `unchanged_outside` check above is false the moment a lambda is
        // rewritten, and a movement in either count could no longer be
        // attributed. `rewrite::pipeline` is where the two do compose, and it
        // is what `kg dump ir --rewrite` runs.
        let captured = kg_hir::rewrite::capture_this(&sorted, &interner);
        capture_fired += this_captures(&captured);
        match check_provenance(&captured, &mut CstIndex::new(&open), &interner) {
            Ok(()) => capture_ok += 1,
            Err(msg) => capture_failures.push((path.clone(), msg)),
        }
        if let Err(msg) = kg_hir::rewrite::capture_this::this_captures_match_class_enclosure(
            &captured,
        ) {
            capture_failures.push((path.clone(), msg));
        }

        // `desugarLambdas`, over the **post-`captureThis`** IR and nothing
        // later — the same window the `lambda diff` line grades against the
        // reference, so a disagreement between the two lines is attributable.
        // Composing `abbreviatedFunctionTemplate` in front would double every
        // generic lambda's function, which is that pass working correctly.
        let desugared = kg_hir::rewrite::desugar_lambdas(&captured, &interner);
        lambdas_fired += lambda_functions(&desugared);
        match check_provenance(&desugared, &mut CstIndex::new(&open), &interner) {
            Ok(()) => lambdas_ok += 1,
            Err(msg) => lambdas_failures.push((path.clone(), msg)),
        }
        if let Err(msg) = kg_hir::rewrite::lambdas::lambda_functions_match_lambda_sites(
            &captured,
            &desugared,
            &interner,
        ) {
            lambdas_failures.push((path.clone(), msg));
        }

        // `moveFunctions` over the composed prefix, which is the only input it
        // ever sees: its abbreviated-`Function` defer (`Function.hs:89-90`)
        // exists for the leftover half of the sub-pass immediately before it,
        // so grading it over the raw IR would leave that arm unreachable and
        // the line would agree with a port that never wrote it. That prefix is
        // `pipeline` minus this step, and `abbreviated:` above is the same pass
        // over the *raw* IR — deliberately a different window, so the two lines
        // stay attributable.
        let abbreviated = kg_hir::rewrite::abbreviated_function_template(&desugared, &interner);
        let moved = kg_hir::rewrite::move_functions(&abbreviated, &interner);
        moves_fired += mangled_declarations(&moved, &interner);
        match check_provenance(&moved, &mut CstIndex::new(&open), &interner) {
            Ok(()) => moves_ok += 1,
            Err(msg) => moves_failures.push((path.clone(), msg)),
        }
        if let Err(msg) = kg_hir::rewrite::move_functions::moved_declarations_reach_fixpoint(
            &abbreviated,
            &moved,
            &interner,
        ) {
            moves_failures.push((path.clone(), msg));
        }

        // --- round trip, clean parses only ---
        if !parse.errors().is_empty() {
            continue;
        }
        clean += 1;

        // The sorted round trip — the module comment's `Sorted trip` bullet.
        // Same interner, same `NoImports` context on both sides, so the only
        // variable is the printing itself.
        let printed_sorted = kg_hir::pretty_with_source(&sorted, &interner, &text);
        let sorted_reparse = kg_syntax::parse_file(&printed_sorted);
        if sorted_reparse.errors().is_empty() {
            let reparsed = ast::SourceFile::cast(sorted_reparse.syntax_node())
                .expect("the root is always SOURCE_FILE");
            let sorted2 =
                kg_hir::lower_sorted(&reparsed, FileId(0), &interner, &kg_hir::NoImports);
            let a = sorted.structure_string(&interner);
            let b = sorted2.structure_string(&interner);
            if a == b {
                sorted_trip_ok += 1;
            } else {
                sorted_trip_failures.push((path.clone(), first_diff(&a, &b)));
            }
        } else {
            let first =
                sorted_reparse.errors().first().map(|e| e.message.clone()).unwrap_or_default();
            sorted_trip_failures
                .push((path.clone(), format!("sorted print does not parse: {first}")));
        }

        let printed = kg_hir::pretty(&ir, &interner);
        let reparse = kg_syntax::parse_file(&printed);
        if !reparse.errors().is_empty() {
            let first = reparse.errors().first().map(|e| e.message.clone()).unwrap_or_default();
            round_trip_failures.push((path.clone(), format!("pretty output does not parse: {first}")));
            continue;
        }
        let reparsed =
            ast::SourceFile::cast(reparse.syntax_node()).expect("the root is always SOURCE_FILE");
        let ir2 = kg_hir::lower(&reparsed, FileId(0), &interner);
        let a = ir.structure_string(&interner);
        let b = ir2.structure_string(&interner);
        if a == b {
            round_trip_ok += 1;
        } else {
            round_trip_failures.push((path.clone(), first_diff(&a, &b)));
        }
    }

    let recovered_files = total_files - clean;
    println!(
        "{total_files} .k files ({clean} parse clean, {recovered_files} error-recovered)  [{:.1}s]",
        lowering_started.elapsed().as_secs_f64()
    );
    println!(
        "  totality:   {clean_total}/{clean} clean parses lower with zero Exp::Unsupported"
    );
    println!(
        "              {recovered_total}/{recovered_files} error-recovered files do too \
         ({unsupported_total} unsupported node(s) in the rest, all invalid-fragment recoveries)"
    );
    let mut by_kind: Vec<(String, usize)> = unsupported_by_kind.into_iter().collect();
    by_kind.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (kind, n) in by_kind.iter().take(10) {
        println!("    {n:>5}  {kind}");
    }
    println!("  round trip: {round_trip_ok}/{clean} clean parses reparse to a structurally equal IR");
    println!(
        "  sorted trip: {sorted_trip_ok}/{clean} sorted lowerings print source-sliced, \
         reparse and relower to an equal IR"
    );
    println!(
        "  provenance: {provenance_ok}/{total_files} sorted lowerings resolve every \
         Src::head to a live CST node"
    );
    println!(
        "  rewrite:    {rewrite_ok}/{total_files} survive abbreviatedFunctionTemplate \
         ({rewrite_fired} declaration(s) rewritten)"
    );
    for (path, msg) in rewrite_failures.iter().take(if verbose { usize::MAX } else { 5 }) {
        println!("    REWRITE {}  {msg}", rel(path, kanagawa_root));
    }
    println!(
        "  capture:    {capture_ok}/{total_files} survive captureThis \
         ({capture_fired} lambda(s) given a `This`)"
    );
    for (path, msg) in capture_failures.iter().take(if verbose { usize::MAX } else { 5 }) {
        println!("    CAPTURE {}  {msg}", rel(path, kanagawa_root));
    }
    println!(
        "  lambdas:    {lambdas_ok}/{total_files} survive desugarLambdas \
         ({lambdas_fired} lambda(s) rewritten)"
    );
    for (path, msg) in lambdas_failures.iter().take(if verbose { usize::MAX } else { 5 }) {
        println!("    LAMBDAS {}  {msg}", rel(path, kanagawa_root));
    }
    println!(
        "  moves:      {moves_ok}/{total_files} survive moveFunctions \
         ({moves_fired} declaration(s) hoisted and mangled)"
    );
    for (path, msg) in moves_failures.iter().take(if verbose { usize::MAX } else { 5 }) {
        println!("    MOVES {}  {msg}", rel(path, kanagawa_root));
    }
    // --- the oracle differential (slice 3) ---
    let oracle_gate = oracle_differential(kanagawa_root, verbose);

    let show = |failures: &[(PathBuf, String)], label: &str| {
        let cap = if verbose { failures.len() } else { 10.min(failures.len()) };
        for (path, msg) in &failures[..cap] {
            println!("  {label} {}  {msg}", rel(path, kanagawa_root));
        }
        if cap < failures.len() {
            println!("  ... and {} more (pass --verbose)", failures.len() - cap);
        }
    };
    unsupported_files.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let cap = if verbose { unsupported_files.len() } else { 10.min(unsupported_files.len()) };
    for (path, n) in &unsupported_files[..cap] {
        println!("  UNSUPPORTED {}  {n} node(s)", rel(path, kanagawa_root));
    }
    if cap < unsupported_files.len() {
        println!("  ... and {} more (pass --verbose)", unsupported_files.len() - cap);
    }
    show(&round_trip_failures, "ROUNDTRIP");
    show(&sorted_trip_failures, "SORTED-TRIP");
    show(&provenance_failures, "PROVENANCE");
    // The binding halves: a clean parse may not contain a hole, and the two
    // structural invariants hold everywhere. Error-recovered totality is a
    // report, not a gate — see the module comment.
    if !clean_unsupported.is_empty() {
        for (path, n) in &clean_unsupported {
            println!("  CLEAN-UNSUPPORTED {}  {n} node(s)", rel(path, kanagawa_root));
        }
        anyhow::bail!("{} cleanly-parsed file(s) contain unsupported nodes", clean_unsupported.len());
    }
    if round_trip_ok < clean {
        anyhow::bail!("{} file(s) fail the round trip", clean - round_trip_ok);
    }
    if sorted_trip_ok < clean {
        anyhow::bail!("{} file(s) fail the sorted round trip", clean - sorted_trip_ok);
    }
    if provenance_ok < total_files {
        anyhow::bail!("{} file(s) fail the provenance invariant", total_files - provenance_ok);
    }
    if !capture_failures.is_empty() {
        anyhow::bail!("{} file(s) fail the captureThis invariant", capture_failures.len());
    }
    if !lambdas_failures.is_empty() {
        anyhow::bail!("{} file(s) fail the desugarLambdas invariant", lambdas_failures.len());
    }
    if !moves_failures.is_empty() {
        anyhow::bail!("{} file(s) fail the moveFunctions invariant", moves_failures.len());
    }
    if !rewrite_failures.is_empty() {
        anyhow::bail!("{} file(s) fail the rewrite invariant", rewrite_failures.len());
    }
    oracle_gate?;
    Ok(())
}

/// What the count gates and the twins accumulate over the roots they grade,
/// per thread; `merge` folds the threads together and `sort` puts every
/// failure list back in corpus order, so the report reads exactly as the
/// one-`Db` loop wrote it. Every field is a sum, a union or a list keyed by
/// its root — nothing here depends on which root was graded before it.
#[derive(Default)]
struct CountGrades {
    capture_ok: usize,
    capture_seen: usize,
    capture_failures: Vec<(PathBuf, String)>,
    lambda_ok: usize,
    lambda_seen: usize,
    lambda_roots: usize,
    lambda_entries: usize,
    lambda_wrappers: usize,
    lambda_survivors: usize,
    lambda_arity: BTreeMap<usize, usize>,
    lambda_unrolled: usize,
    lambda_failures: Vec<(PathBuf, String)>,
    move_ok: usize,
    move_seen: usize,
    move_roots: usize,
    move_entries: usize,
    move_stuck: usize,
    move_wrappers: usize,
    move_depth: BTreeMap<usize, usize>,
    move_this: usize,
    move_why: BTreeMap<&'static str, usize>,
    move_booked: BTreeMap<Booked, usize>,
    move_booked_delta: usize,
    move_failures: Vec<(PathBuf, String)>,
    conservation_failures: Vec<(PathBuf, String)>,
}

impl CountGrades {
    fn merge(&mut self, o: CountGrades) {
        self.capture_ok += o.capture_ok;
        self.capture_seen += o.capture_seen;
        self.capture_failures.extend(o.capture_failures);
        self.lambda_ok += o.lambda_ok;
        self.lambda_seen += o.lambda_seen;
        self.lambda_roots += o.lambda_roots;
        self.lambda_entries += o.lambda_entries;
        self.lambda_wrappers += o.lambda_wrappers;
        self.lambda_survivors += o.lambda_survivors;
        for (k, n) in o.lambda_arity {
            *self.lambda_arity.entry(k).or_default() += n;
        }
        self.lambda_unrolled += o.lambda_unrolled;
        self.lambda_failures.extend(o.lambda_failures);
        self.move_ok += o.move_ok;
        self.move_seen += o.move_seen;
        self.move_roots += o.move_roots;
        self.move_entries += o.move_entries;
        self.move_stuck += o.move_stuck;
        self.move_wrappers += o.move_wrappers;
        for (k, n) in o.move_depth {
            *self.move_depth.entry(k).or_default() += n;
        }
        self.move_this += o.move_this;
        for (k, n) in o.move_why {
            *self.move_why.entry(k).or_default() += n;
        }
        for (k, n) in o.move_booked {
            *self.move_booked.entry(k).or_default() += n;
        }
        self.move_booked_delta += o.move_booked_delta;
        self.move_failures.extend(o.move_failures);
        self.conservation_failures.extend(o.conservation_failures);
    }

    /// Corpus order: the roots were sorted before they were handed out, and
    /// a stable sort by root keeps each root's own entries in the order the
    /// grading pushed them.
    fn sort(&mut self) {
        self.capture_failures.sort_by(|a, b| a.0.cmp(&b.0));
        self.lambda_failures.sort_by(|a, b| a.0.cmp(&b.0));
        self.move_failures.sort_by(|a, b| a.0.cmp(&b.0));
        self.conservation_failures.sort_by(|a, b| a.0.cmp(&b.0));
    }
}

/// Grades one root for the count gates and the twins: the body of what was
/// one loop over `count_files`, now run per root on whichever thread took it,
/// into that thread's own `CountGrades`. `db` is the thread's; a root set
/// reads nothing another root set decided.
#[allow(clippy::too_many_arguments)]
fn grade_root(
    db: &mut kg_db::Db,
    c: &mut CountGrades,
    path: &Path,
    text: &str,
    their_this: &Option<(u32, u32)>,
    their_ledger: &Option<LambdaLedger>,
    their_move: &Option<MoveLedger>,
) {
    if their_this.is_none() && their_ledger.is_none() && their_move.is_none() {
        return;
    }
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Ok(utf8) = camino::Utf8PathBuf::from_path_buf(canon) else { return };
    let file = db.set_file_text(&utf8, text.to_owned());
    let sorted = kg_db::sorted::sorted_program(db, &[file]);
    let Some(ir) = sorted.ir(kg_db::ModuleSource::File(file)) else { return };

    // The reference's grade for `captureThis`.
    if let Some(&(their_n, their_lambdas)) = their_this.as_ref() {
        c.capture_seen += 1;
        let mine = kg_hir::rewrite::capture_this(ir, db.interner());
        let (ours_n, ours_lambdas) = (this_captures(&mine), lambdas(&mine));
        if ours_n == their_n && ours_lambdas == their_lambdas {
            c.capture_ok += 1;
        } else {
            c.capture_failures.push((
                path.to_path_buf(),
                format!(
                    "This {ours_n} vs {their_n}, Lambda {ours_lambdas} vs {their_lambdas}"
                ),
            ));
        }
    }

    // The reference's grade for `desugarLambdas`.
    if let Some(theirs) = their_ledger.as_ref() {
        c.lambda_seen += 1;
        c.lambda_entries += theirs.entries.len();
        c.lambda_wrappers += theirs.wrappers;
        c.lambda_survivors += theirs.survivors;
        c.lambda_roots += usize::from(!theirs.entries.is_empty());
        c.lambda_unrolled += usize::from(theirs.unrolled);
        for e in &theirs.entries {
            *c.lambda_arity.entry(e.arity).or_default() += 1;
        }
        // `captureThis` first: it is frontend pass 3 and prepends the
        // implicit `this` to `lambdaCaptures` (`Frontend.hs:186-191`), so a
        // ledger built without it has every in-class lambda's arity one
        // short.
        let captured = kg_hir::rewrite::capture_this(ir, db.interner());
        if let Some(ours) = our_desugar_lambdas(&captured, db.interner()) {
            let ours = our_lambda_ledger(&ours, db.interner());
            if theirs.agrees(&ours) {
                c.lambda_ok += 1;
            } else {
                c.lambda_failures.push((path.to_path_buf(), theirs.diff(&ours)));
            }
        }
    }

    // The reference's grade for `moveFunctions`.
    if let Some(theirs) = their_move.as_ref() {
        c.move_seen += 1;
        c.move_entries += theirs.entries.len();
        c.move_stuck += theirs.stuck.len();
        c.move_wrappers += theirs.wrappers;
        c.move_roots += usize::from(!theirs.entries.is_empty());
        for e in &theirs.entries {
            *c.move_depth.entry(e.name.matches('@').count()).or_default() += 1;
            c.move_this += e.this_members.len();
        }
        if MOVE_PORTED
            && let Some((ours, movable, higher_order)) =
                our_move_functions(ir, db.interner())
        {
            let ours = our_move_ledger(&ours, db.interner());
            // **Conservation**, on our own tree and checked on every root:
            // a `Function`/`Class` the pass could move either becomes a
            // ledger entry or stays nested, never both and never neither.
            // It is intra-side deliberately — the reference's half is
            // `$wrapper$`-filtered and its own pre-pass population is not
            // observable at `--template-passes 4` — and it is what stops
            // the exclusion from being indistinguishable from deleting the
            // check: a declaration this port moves *and* leaves behind, or
            // loses entirely, fails here whatever the reference reports.
            if ours.entries.len() + ours.stuck.len() != movable {
                c.conservation_failures.push((
                    path.to_path_buf(),
                    format!(
                        "{movable} movable declaration(s) before the pass, \
                         {} moved + {} stuck after",
                        ours.entries.len(),
                        ours.stuck.len()
                    ),
                ));
            }
            // **Booked, not failed**: the two sides were handed different
            // trees. `higherOrderFunctions` (`Function.hs:313-325`) is
            // frontend pass 7 and unported, and it turns every `inline`
            // function with a function-type parameter into a `Template` —
            // which is `moveFunctions`' *first* deferral arm
            // (`Function.hs:88`). So on those roots the reference declines
            // to move declarations this port has no `Template` over and
            // moves; `library/data/function.k` alone accounts for 22.
            // The condition is a strict equality on a quantity the entry
            // comparison does not use, and a port cannot arrange it in its
            // own favour: losing or duplicating a declaration fails the
            // conservation law above first, and moving the wrong subset of
            // the *same* population leaves the total untouched and is
            // graded. What it cannot distinguish is "an earlier pass is
            // unported" from "an earlier pass is ported and wrong" — that
            // is what `capture diff` and `lambda diff` are for.
            //
            // It books 3 roots on the count alone. The other 3 —
            // `library/control/async.k`, `test/library/data/bits.k` and
            // `test/logic/lambda_test_cases.k` — are roots where the same
            // unported pass changes *which* declarations move without
            // changing how many are movable, so the count is no help and
            // [`MoveLedger::booked`] has to earn them one entry at a time.
            if !theirs.same_population(movable) {
                *c.move_booked.entry(Booked::HigherOrderFunctions).or_default() += 1;
                c.move_booked_delta += movable.abs_diff(
                    theirs.entries.len() + theirs.stuck.len(),
                );
            } else if let Some((field, why)) = theirs.disagreement(&ours) {
                // Booked or failed — never both, and the predicate is the
                // upstream pass's own guard, not the shape of the
                // disagreement. See [`MoveLedger::booked`].
                if let Some(book) = theirs.booked(&ours, &higher_order) {
                    *c.move_booked.entry(book).or_default() += 1;
                } else {
                    *c.move_why.entry(field).or_default() += 1;
                    c.move_failures.push((path.to_path_buf(), format!("[{field}] {why}")));
                }
            } else {
                c.move_ok += 1;
            }
        }
    }

}

/// The oracle differential — see the module comment.
///
/// Comparison and root-tree selection are ports of `oracle/differential.py`,
/// the by-hand instrument for a single file; this is the same comparison run
/// as a gate. One divergence is *booked*, not failed (a second — a file
/// whose first difference sat at an interpolated string — closed on
/// 2026-09-22, when kg-hir began carrying a string's holes as children):
///
/// * a file the oracle produces no tree for. Under `-I library` and the
///   default `mock` target device, a device override that *differs* from the
///   mock copy `base` re-exposes makes its own locals ambiguous
///   (`findInScopes` concatenates import hits, `SymbolTable.hs:257`), and the
///   reference *rejects the file*: `registerSymbolError` rides megaparsec's
///   deferred-error channel, which fails the parse at end of input.
fn oracle_differential(kanagawa_root: &Path, verbose: bool) -> Result<()> {
    // Absent is red (sprint §2): "skipped" here once passed the gate.
    let Some(oracle) = find_oracle() else {
        anyhow::bail!("oracle diff: kg-oracle is not built — run oracle/build.sh; the differential graded nothing");
    };

    let lib = kanagawa_root.join("library");
    let base = lib.join("base.k");
    let mut paths = Vec::new();
    collect(&lib, &mut paths);
    paths.sort();
    let files: Vec<(PathBuf, String)> = paths
        .into_iter()
        .filter_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
        .collect();

    // The two *count* gates below run over a wider list than the tree
    // comparison does, and the reason is coverage rather than tidiness.
    // `library/` holds 119 of the corpus's 1306 lambdas — 9% — so a gate
    // confined to it grades `desugarLambdas` on a twentieth of its own blast
    // radius. `test/` adds 229 roots the oracle parses under the same
    // `-I library` (measured: the extra `-I test` changes nothing), and takes
    // the ledger from 114 rewritten lambdas to ~890. The tree comparison stays
    // on `library/` alone: it is specified against `oracle/differential.py`'s
    // root selection, and `test/`'s multi-fixture files — separated by
    // `expected:N`, which is not a program — are exactly what that instrument
    // was never defined over.
    let mut count_paths = Vec::new();
    collect(&lib, &mut count_paths);
    collect(&kanagawa_root.join("test"), &mut count_paths);
    count_paths.sort();
    let count_files: Vec<(PathBuf, String)> = count_paths
        .into_iter()
        .filter_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
        .collect();

    // One timer per fan-out. They were one timer until the count gates grew
    // from 124 roots to 351 and `oracle diff` appeared to have tripled, which
    // it had not — it was being charged for the other two.
    let started = std::time::Instant::now();
    let theirs = oracle_trees(&oracle, &lib, &base, &files);
    let tree_secs = started.elapsed().as_secs_f64();
    let capture_started = std::time::Instant::now();
    let their_this = oracle_this_counts(&oracle, &lib, &base, &count_files);
    let capture_oracle_secs = capture_started.elapsed().as_secs_f64();
    let lambda_started = std::time::Instant::now();
    let their_lambdas = oracle_lambda_ledgers(&oracle, &lib, &base, &count_files);
    let lambda_oracle_secs = lambda_started.elapsed().as_secs_f64();
    let move_started = std::time::Instant::now();
    let their_moves = oracle_move_ledgers(&oracle, &lib, &base, &count_files);
    let move_oracle_secs = move_started.elapsed().as_secs_f64();

    let root_utf8 = camino::Utf8PathBuf::from_path_buf(
        kanagawa_root.canonicalize().unwrap_or_else(|_| kanagawa_root.to_path_buf()),
    )
    .expect("the reference path is UTF-8");
    let mut settings = kg_config::Settings::fallback(root_utf8.clone());
    settings.import_dirs = vec![root_utf8.join("library")];
    settings.base_library = Some(root_utf8.join("library/base.k"));
    settings.target_device = "mock".to_owned();
    let mut db = kg_db::Db::new(settings.clone());

    let mut ok = 0usize;
    let mut skipped = 0usize;
    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    let mut unsorted_total = 0usize;
    let mut unsorted_reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut donor_bad = 0usize;
    let mut donor_total = 0usize;
    let mut donor_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut donor_failures: Vec<(PathBuf, String)> = Vec::new();

    // The two count gates, over the wider list. Their own loop: they are
    // defined per file and the tree comparison is not, and keeping them here
    // is what lets the lists differ.
    let count_started = std::time::Instant::now();
    // Every root is graded on its own `Db`, one per thread, and the grades
    // are folded together afterwards: this loop was 90 of the gate's 125
    // seconds when it ran the roots one at a time, and nothing a root set
    // decides is read by another. Measured byte-identical to the sequential
    // loop it replaced, `sort` included.
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let grades: std::sync::Mutex<Vec<CountGrades>> = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut db = kg_db::Db::new(settings.clone());
                let mut c = CountGrades::default();
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some((path, text)) = count_files.get(i) else { break };
                    grade_root(
                        &mut db,
                        &mut c,
                        path,
                        text,
                        &their_this[i],
                        &their_lambdas[i],
                        &their_moves[i],
                    );
                }
                grades.lock().expect("no panics hold this lock").push(c);
            });
        }
    });
    let mut c = CountGrades::default();
    for g in grades.into_inner().expect("threads joined") {
        c.merge(g);
    }
    c.sort();
    let CountGrades {
        capture_ok,
        capture_seen,
        capture_failures,
        lambda_ok,
        lambda_seen,
        lambda_roots,
        lambda_entries,
        lambda_wrappers,
        lambda_survivors,
        lambda_arity,
        lambda_unrolled,
        lambda_failures,
        move_ok,
        move_seen,
        move_roots,
        move_entries,
        move_stuck,
        move_wrappers,
        move_depth,
        move_this,
        move_why,
        move_booked,
        move_booked_delta,
        move_failures,
        conservation_failures,
    } = c;

    let tree_started = std::time::Instant::now();
    for ((path, text), their_names) in files.iter().zip(&theirs) {
        // Canonical, like the reference (`canonicalizePath`, `Parser/IO.hs:107`)
        // and the CLI's manifest resolution: a root `base.k` also imports
        // (`control.loop`, …) must get the *same* `FileId` from both doors, or
        // the program holds the module twice and every seed-backed lookup in
        // it goes ambiguous against its own twin.
        let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
        let Ok(utf8) = camino::Utf8PathBuf::from_path_buf(canon) else { continue };
        let file = db.set_file_text(&utf8, text.clone());
        let sorted = kg_db::sorted::sorted_program(&db, &[file]);
        let Some(ir) = sorted.ir(kg_db::ModuleSource::File(file)) else {
            failures.push((path.clone(), "root module not in sorted program".to_owned()));
            continue;
        };
        unsorted_total += ir.sort.unsorted.len();
        for (_, reason) in &ir.sort.unsorted {
            *unsorted_reasons.entry(format!("{reason:?}")).or_default() += 1;
        }
        let Some(their_noted) = their_names else {
            skipped += 1;
            continue;
        };
        let their_names = their_noted;
        let rendered = kg_hir::structure::render(
            ir,
            db.interner(),
            kg_hir::structure::Detail::Annotated,
        );
        let ours = our_noted(&rendered);
        let our_names: Vec<&str> = ours.iter().map(|(n, _)| n.as_str()).collect();
        let their_names: Vec<&str> = their_names.iter().map(|(n, _)| n.as_str()).collect();
        if our_names != their_names {
            let i = our_names
                .iter()
                .zip(&their_names)
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| our_names.len().min(their_names.len()));
            let ctx = |v: &[&str]| -> String {
                v[i.saturating_sub(3)..(i + 4).min(v.len())].join(",")
            };
            failures.push((
                path.clone(),
                format!("first at #{i}: ours=[{}] theirs=[{}]", ctx(&our_names), ctx(&their_names)),
            ));
            continue;
        }

        // Shapes agree. Now the donors — **reported, never gated**, and the
        // measurement is why. The reference's ranges include the trailing
        // trivia a megaparsec lexeme consumes (`auto ` where ours is `auto`),
        // so each is first resolved to the CST node it names rather than
        // compared byte-for-byte. After that 93.4% of nodes agree exactly and
        // the remaining 6.6% do not fall into a few clean categories: 6,315
        // `Seq`, 2,641 `ScopedName`, 1,706 `Binary`, and a long tail. Those are
        // two different objects — ours is the CST node's range, the
        // reference's is a parser-combinator span that sometimes covers a
        // leading keyword and sometimes is `SrcUnknown` — and reconciling them
        // is its own project (divergence #25). The number is printed so that a
        // change moves it.
        //
        // What this does *not* protect is the thing that motivated it: a
        // rewrite giving a synthesized node the wrong donor. That is caught by
        // per-pass unit tests read off `kg-oracle --notes`, which is now a
        // house rule (`docs/gates.md`), because a corpus-wide equality gate cannot
        // exist until the 6.6% is understood.
        let cst = cst_ranges(&db, file);
        let mut bad = 0usize;
        let mut first: Option<String> = None;
        for (k, ((name, ours_r), (_, theirs_r))) in
            ours.iter().zip(their_noted.iter()).enumerate()
        {
            let (Some(a), Some(b)) = (*ours_r, *theirs_r) else { continue };
            let b = canonical(b, &cst, text);
            if a != b {
                bad += 1;
                let how = if a.0 == b.0 { "end" } else if a.1 == b.1 { "start" } else { "both" };
                *donor_by_kind.entry(format!("{name} ({how})")).or_default() += 1;
                if first.is_none() {
                    first = Some(format!(
                        "#{k} {name}: ours @{}..{} theirs @{}..{}",
                        a.0, a.1, b.0, b.1
                    ));
                }
            }
        }
        donor_bad += bad;
        donor_total += ours.len();
        if bad > 0 {
            donor_failures.push((path.clone(), first.unwrap_or_default()));
        }
        ok += 1;
    }

    println!(
        "  oracle diff: {ok}/{} library roots match the reference's own tree node-for-node \
         [{tree_secs:.1}s oracle, {:.1}s ours]",
        files.len(),
        tree_started.elapsed().as_secs_f64(),
    );
    println!(
        "              {skipped} rejected by the oracle (device override ambiguous with mock), \
         {} DIFF",
        failures.len(),
    );
    let reasons: Vec<String> =
        unsorted_reasons.iter().map(|(r, n)| format!("{r}: {n}")).collect();
    println!(
        "              {} of {donor_total} nodes ({:.1}%) also wear the reference's exact \
         Src span — reported, not gated; see divergence #25",
        donor_total - donor_bad,
        100.0 * (donor_total - donor_bad) as f64 / donor_total.max(1) as f64,
    );
    let mut by: Vec<(&String, &usize)> = donor_by_kind.iter().collect();
    by.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    if verbose {
        for (k, n) in by.iter().take(20) {
            println!("                {n:7} {k}");
        }
    }
    println!(
        "  capture diff: {capture_ok}/{capture_seen} roots agree with the reference on \
         `ThisF` and `LambdaF` counts at --passes 3 [{capture_oracle_secs:.1}s oracle, \
         {:.1}s ours for the count gates and the twins]",
        count_started.elapsed().as_secs_f64(),
    );
    for (path, msg) in capture_failures.iter().take(if verbose { usize::MAX } else { 5 }) {
        println!("    CAPTURE {}  {msg}", rel(path, kanagawa_root));
    }
    let arities: Vec<String> =
        lambda_arity.iter().map(|(n, k)| format!("{n}:{k}")).collect();
    if LAMBDAS_PORTED {
        println!(
            "  lambda diff: {lambda_ok}/{lambda_seen} roots agree with the reference \
             on every lambda `desugarLambdas` rewrites, at --passes 8 --template-passes 2 \
             [{lambda_oracle_secs:.1}s]"
        );
    } else {
        println!(
            "  lambda diff: skipped — `desugarLambdas` is not ported; the reference half \
             below is live [{lambda_oracle_secs:.1}s]"
        );
    }
    println!(
        "              reference: {lambda_entries} lambda(s) rewritten across {lambda_roots} \
         root(s), {lambda_survivors} left standing under `static for`, {lambda_wrappers} \
         `$wrapper$` excluded and {lambda_unrolled} root(s) booked for `staticControl` \
         (neither pass is ported)"
    );
    println!("              make_captureN arities: {}", arities.join(" "));
    for (path, msg) in lambda_failures.iter().take(if verbose { usize::MAX } else { 5 }) {
        println!("    LAMBDA {}  {msg}", rel(path, kanagawa_root));
    }
    if MOVE_PORTED {
        println!(
            "  move diff: {move_ok}/{move_seen} roots agree with the reference on every \
             declaration `moveFunctions` hoists and every one it leaves, at --passes 8 \
             --template-passes 4 ({} booked, {} DIFF) [{move_oracle_secs:.1}s]",
            move_booked.values().sum::<usize>(),
            move_failures.len(),
        );
    } else {
        println!(
            "  move diff: skipped — `moveFunctions` is not ported; the reference half \
             below is live [{move_oracle_secs:.1}s]"
        );
    }
    println!(
        "             reference: {move_entries} declaration(s) hoisted across {move_roots} \
         root(s), {move_stuck} left nested under a deferral arm, {move_wrappers} \
         `$wrapper$` excluded (`addLambdaWrappers` is not ported); \
         {move_this} `$this.<member>` rewrite(s) inside moved bodies"
    );
    let depths: Vec<String> = move_depth.iter().map(|(n, k)| format!("{n}:{k}")).collect();
    println!("             mangle depth: {}", depths.join(" "));
    let booked_total: usize = move_booked.values().sum();
    if booked_total > 0 {
        let cells: Vec<String> =
            move_booked.iter().map(|(b, n)| format!("{n} to {}", b.label())).collect();
        println!(
            "             booked, not failed: {booked_total} root(s) — {} — neither pass \
             is ported, and {move_booked_delta} movable declaration(s) differ before this \
             pass runs. Expect {booked_total} root(s) to move to `ok` when they land.",
            cells.join("; "),
        );
    }
    if !move_why.is_empty() {
        let mut by: Vec<(&&str, &usize)> = move_why.iter().collect();
        by.sort_by_key(|(f, n)| (std::cmp::Reverse(**n), **f));
        let cells: Vec<String> = by.iter().map(|(f, n)| format!("{f} {n}")).collect();
        println!("             first disagreement, by behaviour: {}", cells.join(", "));
    }
    for (path, msg) in move_failures.iter().take(if verbose { usize::MAX } else { 5 }) {
        println!("    MOVE {}  {msg}", rel(path, kanagawa_root));
    }
    for (path, msg) in conservation_failures.iter().take(if verbose { usize::MAX } else { 5 }) {
        println!("    MOVE-CONSERVATION {}  {msg}", rel(path, kanagawa_root));
    }
    println!("  {}", crate::oracle_cache::report());
    for (path, msg) in donor_failures.iter().take(if verbose { 5 } else { 0 }) {
        println!("  DONOR {}  {msg}", rel(path, kanagawa_root));
    }
    println!(
        "              {unsorted_total} unsorted path(s) across the roots ({})",
        if reasons.is_empty() { "none".to_owned() } else { reasons.join(", ") },
    );
    let cap = if verbose { failures.len() } else { 10.min(failures.len()) };
    for (path, msg) in &failures[..cap] {
        println!("  ORACLE-DIFF {}  {msg}", rel(path, kanagawa_root));
    }
    if !capture_failures.is_empty() {
        anyhow::bail!(
            "{} file(s) disagree with the reference on captureThis",
            capture_failures.len()
        );
    }
    if !lambda_failures.is_empty() {
        anyhow::bail!(
            "{} file(s) disagree with the reference on desugarLambdas",
            lambda_failures.len()
        );
    }
    if !conservation_failures.is_empty() {
        anyhow::bail!(
            "{} file(s) fail moveFunctions' conservation law — a movable declaration \
             is neither hoisted nor left standing",
            conservation_failures.len()
        );
    }
    // Only the **unbooked** disagreements bail. `oracle diff` is the precedent:
    // it is 116/124 and green, because its 8 are booked with named causes. A
    // booking that names no pass would be a suppressed failure, so
    // [`MoveLedger::booked`] is all-or-nothing per root and keys on the
    // upstream pass's own guard; anything it cannot account for lands here.
    if !move_failures.is_empty() {
        anyhow::bail!(
            "{} file(s) disagree with the reference on moveFunctions",
            move_failures.len()
        );
    }
    if !failures.is_empty() {
        anyhow::bail!(
            "{} library file(s) disagree with the reference's tree",
            failures.len()
        );
    }
    Ok(())
}

/// Every CST node range of one file, grouped by start offset.
fn cst_ranges(db: &kg_db::Db, file: FileId) -> BTreeMap<u32, Vec<u32>> {
    let mut out: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let parse = db.parse(file);
    for node in parse.syntax_node().descendants() {
        let r = node.text_range();
        out.entry(r.start().into()).or_default().push(r.end().into());
    }
    for ends in out.values_mut() {
        ends.sort_unstable();
        ends.dedup();
    }
    out
}

/// The reference's range, resolved to the CST node it names.
///
/// A megaparsec lexeme consumes its trailing whitespace and comments, so the
/// reference's `auto` is `14..19` where the CST's `AUTO_TYPE` is `14..18`. The
/// node named is the **largest** one starting at the same offset that still
/// fits inside the reference's span; a range naming no node at all falls back
/// to trimming ASCII whitespace, which keeps the comparison honest rather than
/// silently agreeing.
fn canonical(r: (u32, u32), cst: &BTreeMap<u32, Vec<u32>>, text: &str) -> (u32, u32) {
    if let Some(ends) = cst.get(&r.0)
        && let Some(&e) = ends.iter().rev().find(|&&e| e <= r.1)
    {
        return (r.0, e);
    }
    let mut end = r.1 as usize;
    let bytes = text.as_bytes();
    while end > r.0 as usize && end <= bytes.len() && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    (r.0, end as u32)
}

/// The built `kg-oracle`, wherever cabal put it under `oracle/dist-newstyle`.
pub(crate) fn find_oracle() -> Option<PathBuf> {
    let kgrust = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.to_path_buf();
    let dist = kgrust.join("oracle/dist-newstyle");
    let mut found: Option<PathBuf> = None;
    fn walk(dir: &Path, found: &mut Option<PathBuf>) {
        if found.is_some() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.file_name().is_some_and(|n| n == "kg-oracle") {
                *found = Some(path);
                return;
            }
        }
    }
    walk(&dist, &mut found);
    found
}

/// `kg-oracle --tree --passes 3` on every file: how many `ThisF` nodes the
/// reference holds once `captureThis` has run, and how many `LambdaF` it holds
/// at all.
///
/// This is the whole of the reference's grade for `captureThis`. Comparing the
/// *trees* at `--passes 3` is not available — passes 1 and 2 have run by then
/// and neither is ported, so the shapes differ for reasons that have nothing to
/// do with this pass. Two counts do isolate it: `captureThis` is the only thing
/// in the first three passes that mints a `ThisF` (the other producer,
/// `addLambdaWrappers`, is inside pass 8, `Function.hs:407-409`), and no pass
/// before it adds or removes a `LambdaF`, so the second count is the guard that
/// stops the first from agreeing by accident on a tree we read wrong.
fn oracle_this_counts(
    oracle: &Path,
    lib: &Path,
    base: &Path,
    files: &[(PathBuf, String)],
) -> Vec<Option<(u32, u32)>> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<Option<(u32, u32)>>> =
        files.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= files.len() {
                    break;
                }
                let (path, _) = &files[i];
                let line = oracle_line(oracle, lib, base, path, &["--tree", "--passes", "3"]);
                // The brace is not decoration. `--tree` renders a record as
                // `LambdaF {declScopeName = …`, and `LambdaF` is a **prefix of
                // `LambdaFunction`**, the `funcKind` `desugarLambdas` mints —
                // so a bare substring count would read 2 lambdas on a file with
                // none the moment pass 8's sub-pass 2 lands. It cannot fire at
                // `--passes 3`, where no `LambdaFunction` exists yet; it is
                // spelled this way so that it still cannot when it does.
                *out[i].lock().expect("no panics hold this lock") = line.map(|l| {
                    (u32::try_from(l.matches("ThisF {").count()).unwrap_or(u32::MAX),
                     u32::try_from(l.matches("LambdaF {").count()).unwrap_or(u32::MAX))
                });
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined")).collect()
}

// ---------------------------------------------------------------------------
// The lambda ledger — the reference's grade for `desugarLambdas`.
// ---------------------------------------------------------------------------

/// Flip with the one-line replacement in [`our_desugar_lambdas`]. Until then
/// the `lambda diff` line reports the reference's half and enforces nothing,
/// because an unported pass would fail every root and say nothing true.
const LAMBDAS_PORTED: bool = true;

/// One lambda as `desugarLambdas` left it (`Function.hs:237-262`).
///
/// Not a byte comparison of the emitted subtree, and not a node count either.
/// A count cannot work at this boundary: `addLambdaWrappers` is sub-pass 1
/// (`Template.hs:79-80`) and is not ported, so the reference's tree at
/// `--template-passes 2` carries 27 `library/` lambdas the port will never
/// hold, and their alias/function pairs land inside *other* lambdas' bodies —
/// inflating any per-file `AliasF`/`CastF`/`FunctionF` total by an amount that
/// is not attributable to sub-pass 2. Each field here is a direct function of
/// the source lambda, so a wrapper is simply a ledger entry that gets dropped.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
struct LambdaEntry {
    /// `declName`, read off the alias as `<name>$capture` (`Function.hs:222`).
    /// The parser mints it, never this pass (`Parser.hs:434`), so it is the
    /// one field a correct port cannot compute — only carry.
    name: String,
    /// `N` in `make_captureN` — `length lambdaCaptures` (`Function.hs:154`),
    /// *after* `captureThis` has prepended any implicit `this`. The field that
    /// catches an off-by-one no `AliasF` count can see, and the one that picks
    /// `@data@closure@core` over `@data@closure` at zero (`Function.hs:153`).
    arity: usize,
    /// Emitted `FuncParamF` count less the prepended capture param
    /// (`Function.hs:248`).
    params: usize,
    /// Some `paramType` is `auto`, so the `Closure`'s first argument is a
    /// `FunctionTypeF` rather than a `DecltypeF` (`Function.hs:186-187`, `:207`).
    generic: bool,
    /// The uncapture prologue's field indices in body order — `x0`, `x1`, …
    /// (`Function.hs:256-261`). `zip [0..]` over the captures, so this is
    /// `0..arity` for a correct port and the cheapest possible falsifier for a
    /// prologue built in the wrong order or with a repeated index.
    xs: Vec<usize>,
}

/// Every lambda one root's tree accounts for, at `--template-passes 2`.
///
/// Deliberately not `PartialEq`: [`wrappers`](Self::wrappers) is a
/// reference-only field, and a derived `==` compares it. That is not a
/// theoretical worry — it made `data/fifo.k` and `data/string.k`
/// unsatisfiable, because every lambda they hold at this boundary is one
/// `addLambdaWrappers` synthesized, so the reference reports 5 and 3 against a
/// port that will always report 0. [`agrees`](Self::agrees) is the comparison.
#[derive(Default, Clone, Debug)]
struct LambdaLedger {
    /// Sorted, so the comparison is a multiset — the pass's *output order*
    /// within a `Seq` is pinned by the donor unit tests, not here.
    entries: Vec<LambdaEntry>,
    /// `$wrapper$…` entries, dropped from `entries` and only counted
    /// (`Function.hs:400`).
    wrappers: usize,
    /// `LambdaF` still standing. Not zero: `lambdaDefs` and `lambdaRef` both
    /// stop dead at a `StaticFor` (`Function.hs:219`, `:234`), so a lambda
    /// written inside `static for` is untouched — 5 of `library/`'s 119, in
    /// `data/hash_table.k` and `data/vector.k`. This is the guard on the
    /// entries above: it is the one number that moves if the port desugars a
    /// lambda the reference leaves alone, or leaves one the reference takes.
    survivors: usize,
    /// The reference's tree holds an `UnrolledForF`. `staticControl`
    /// (`Constexpr.hs:41-54`) rewrites `StaticForF a b c` to `UnrolledForF a b
    /// c`, or to a `NestedScope` of literal iterations when the induction
    /// variable is a `DependentConstexpr` over a literal bound — and it is an
    /// algebra of `inferExpTree`, i.e. **pass 6**, so it has run long before
    /// pass 8 begins. Only `evaluateConstexpr`'s third of that is ported here,
    /// so our tree still holds an `Exp::StaticFor` and `desugarLambdas`
    /// correctly reverts on it where the reference, holding no `StaticFor` at
    /// all, goes ahead. See [`LambdaLedger::agrees`] for what that costs.
    unrolled: bool,
}

impl LambdaLedger {
    /// `self` is the reference's. Everything the port can be held to: the
    /// lambdas it rewrote, and the lambdas it correctly left alone.
    fn agrees(&self, ours: &LambdaLedger) -> bool {
        // **Conservation**, checked on every file: a written lambda either
        // becomes a ledger entry or survives, never both and never neither.
        // Wrappers are excluded from both halves, so the two sides count the
        // same population and this is an equality, not a bound. It is what
        // keeps the relaxation below from admitting a port that does nothing.
        let conserved = ours.entries.len() + ours.survivors
            == self.entries.len() + self.survivors;
        if !self.unrolled {
            return conserved && self.entries == ours.entries
                && self.survivors == ours.survivors;
        }
        // `staticControl` (`Constexpr.hs:41-54`, an algebra of pass 6's
        // `inferExpTree`) has unrolled a constant-bound `static for` long
        // before pass 8 began. It is not ported, so our tree still holds an
        // `Exp::StaticFor` there and `desugarLambdas` correctly reverts on it
        // where the reference — holding no `StaticFor` at all — goes ahead. The
        // effect is one-directional and exactly two-fold: entries of theirs go
        // missing from ours, and the same lambdas turn up in our survivor
        // count. So on these 27 of 351 roots the claim becomes "every lambda we
        // rewrote, we rewrote exactly as they did", with conservation still
        // pinning the total. A port that rewrote nothing fails conservation; a
        // port that rewrote one wrongly fails the subset.
        conserved && Self::is_submultiset(&ours.entries, &self.entries)
    }

    /// Both slices are sorted, so one merge walk answers it.
fn is_submultiset(small: &[LambdaEntry], big: &[LambdaEntry]) -> bool {
    let mut j = 0;
    for a in small {
        while j < big.len() && &big[j] < a {
            j += 1;
        }
        if j == big.len() || &big[j] != a {
            return false;
        }
        j += 1;
    }
    true
}

/// The first disagreement, as a line a reader can act on.
    fn diff(&self, ours: &LambdaLedger) -> String {
        if self.survivors != ours.survivors
            && !(self.unrolled && ours.survivors >= self.survivors)
        {
            return format!(
                "survivors: ours {} vs theirs {}", ours.survivors, self.survivors
            );
        }
        if self.unrolled {
            if let Some(a) = ours.entries.iter().find(|a| !self.entries.contains(a)) {
                return format!("booked for staticControl, but ours has an extra {a:?}");
            }
            return format!(
                "conservation: ours {}+{} vs theirs {}+{}",
                ours.entries.len(), ours.survivors, self.entries.len(), self.survivors
            );
        }
        for (a, b) in ours.entries.iter().zip(&self.entries) {
            if a != b {
                return format!("ours {a:?} vs theirs {b:?}");
            }
        }
        format!("{} entries vs {}", ours.entries.len(), self.entries.len())
    }
}

/// `kg-oracle --tree --passes 8 --template-passes 2 --template-iterations 0`
/// on every file, spread over threads.
///
/// `--passes 8` is mandatory: `removeTemplate` is frontend execution order 15
/// and deletes the output. `--template-iterations 0` is what makes
/// `instantiateTemplates` honour `templatePasses` at all (`Template.hs:62-64`).
/// The `1`-to-`2` step is `desugarLambdas` alone, and it is the *only* window
/// that isolates it: at `3` `abbreviatedFunctionTemplate` has run, and it
/// `deep_copy`s declarations whose bodies may hold lambdas, so every count and
/// every ledger entry inside a copied declaration is doubled. Nothing here
/// reads the `1` boundary — the ledger is self-describing at `2`.
fn oracle_lambda_ledgers(
    oracle: &Path,
    lib: &Path,
    base: &Path,
    files: &[(PathBuf, String)],
) -> Vec<Option<LambdaLedger>> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<Option<LambdaLedger>>> =
        files.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= files.len() {
                    break;
                }
                let (path, _) = &files[i];
                let line = oracle_line(oracle, lib, base, path, &[
                    "--tree", "--passes", "8",
                    "--template-passes", "2", "--template-iterations", "0",
                ]);
                *out[i].lock().expect("no panics hold this lock") =
                    line.as_deref().map(reference_ledger);
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined")).collect()
}

/// The ledger, read off one `--tree` line.
///
/// Pairing is by **adjacency**, never by name. `foldr cons e [a1, f1, a2, f2]`
/// (`Function.hs:204`) puts a lambda's two declarations next to each other in
/// the enclosing `Seq`, so the `FunctionF` that belongs to an `AliasF` is the
/// record that starts immediately after it. Name keying is wrong because
/// `?lambdaN` recurs: the parser's counter resets at every named scope
/// (`Parser/SymbolTable.hs:183-192`), so a lambda nested in `?lambda0`'s body
/// is itself `?lambda0` — and its declarations sit *inside* the outer
/// function's subtree, where a name-keyed reader merges the two.
fn reference_ledger(line: &str) -> LambdaLedger {
    let mut entries = Vec::new();
    let mut wrappers = 0usize;
    let mut at = 0usize;
    while let Some(open) = find_ctor(line, "AliasF {", at) {
        at = open + 1;
        let alias_end = match_span(line, open);
        let alias = &line[open..alias_end];
        let Some(name) = trailing_decl_name(alias).and_then(|n| n.strip_suffix("$capture"))
        else {
            continue;
        };
        // A user `using` alias has no `make_capture` call under it, and a
        // *nested* one would have more than the pass's single call.
        let Some(arity) = sole_make_capture(alias) else { continue };
        let name = name.to_owned();

        let rest = &line[alias_end..];
        let entry = (|| {
            // `strip_prefix` returns a suffix of `line`, so its length gives
            // the offset of the `{` that opens the paired record.
            let fn_open = line.len() - rest.strip_prefix(",FunctionF ")?.len();
            let func = &line[fn_open..match_span(line, fn_open)];
            if !func.contains("funcKind = LambdaFunction") {
                return None;
            }
            let head = format!("declName = IdentifierF {:?}, funcParams = SeqF [", name);
            func.find(&head)?;
            let p = func.find("funcParams = SeqF [")? + "funcParams = SeqF ".len();
            let params = &func[p..match_span(func, p)];
            let pitems = split_top(params);
            let generic = pitems.iter().any(|p| p.contains("paramType = AutoF,"));
            let b = func.find(", body = SeqF [")? + ", body = SeqF ".len();
            let body = &func[b..match_span(func, b)];
            // The prologue is a *leading* run of `Variable`s reading the
            // capture param, so the scan stops at the first item that is not
            // one — user code further down may legitimately touch an `x0`.
            let needle = format!("IdentifierF {:?} IdentifierF \"x", name.clone() + "$capture_param");
            let mut xs = Vec::new();
            for item in split_top(body) {
                if !item.starts_with("VariableF {") {
                    break;
                }
                let Some(k) = item.find(&needle) else { break };
                let digits = &item[k + needle.len()..];
                let n: String = digits.chars().take_while(char::is_ascii_digit).collect();
                let Ok(n) = n.parse() else { break };
                xs.push(n);
            }
            Some(LambdaEntry { name: name.clone(), arity, params: pitems.len() - 1, generic, xs })
        })();
        let entry = entry.unwrap_or(LambdaEntry {
            name: name.clone(),
            arity,
            params: usize::MAX,
            generic: false,
            xs: Vec::new(),
        });
        if entry.name.starts_with("$wrapper$") {
            wrappers += 1;
        } else {
            entries.push(entry);
        }
    }
    entries.sort();
    LambdaLedger {
        entries,
        wrappers,
        survivors: reference_survivors(line),
        unrolled: line.contains("UnrolledForF"),
    }
}

/// `LambdaF` nodes still standing, **excluding `addLambdaWrappers`' own**.
///
/// The exclusion is not symmetry with `entries` for its own sake; without it
/// the guard measures an unported pass. `addLambdaWrappers` is sub-pass 1 —
/// inside this window — and the wrappers it synthesizes are ordinary `LambdaF`
/// nodes that `desugarLambdas` does not always reach. One does not, in
/// `test/library/processor/risc-v.k`: `$wrapper$CustomDecode`, at
/// `Tests::DecoderTests::$::$quasi0::$quasi1`, from `decode_instr(LUI,
/// CustomDecode)` passing a bare function name. Counting it made the reference
/// claim a survivor our tree can never hold, on a file where every real lambda
/// agreed.
fn reference_survivors(line: &str) -> usize {
    let mut n = 0;
    let mut at = 0;
    while let Some(i) = find_ctor(line, "LambdaF {", at) {
        at = i + 1;
        let rec = &line[i..match_span(line, i)];
        let head = "declName = IdentifierF \"";
        let name = rec.find(head).map(|k| &rec[k + head.len()..]).unwrap_or("");
        if !name.starts_with("$wrapper$") {
            n += 1;
        }
    }
    n
}

/// The same ledger, read off our own rewritten tree.
///
/// Deliberately the same shape as [`reference_ledger`] rather than something
/// the IR makes easier: the two readers must be able to disagree only about
/// what the *passes* produced, and a reader that took a shortcut our arena
/// happens to allow would hide exactly the bug this gate exists to find.
fn our_lambda_ledger(ir: &kg_hir::ModuleIr, interner: &Interner) -> LambdaLedger {
    use kg_hir::Exp;
    let text = |id| match &ir.arena[id].exp {
        Exp::Identifier(s) => interner.resolve(*s).to_string(),
        _ => String::new(),
    };
    let mut entries = Vec::new();
    let mut wrappers = 0usize;
    let mut survivors = 0usize;
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children = Vec::new();
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        if matches!(ir.arena[id].exp, Exp::Lambda { .. }) {
            survivors += 1;
        }
        if let Exp::Seq(items) = &ir.arena[id].exp {
            for pair in items.windows(2) {
                let (Exp::Alias { body, name, .. }, Exp::Function { kind, params, body: fb, .. }) =
                    (&ir.arena[pair[0]].exp, &ir.arena[pair[1]].exp)
                else {
                    continue;
                };
                if *kind != kg_ty::FunctionKind::LambdaFunction {
                    continue;
                }
                let Some(name) = text(*name).strip_suffix("$capture").map(str::to_owned) else {
                    continue;
                };
                let Some(arity) = our_sole_make_capture(ir, interner, *body) else { continue };
                let Exp::Seq(ps) = &ir.arena[*params].exp else { continue };
                let generic = ps.iter().any(|&p| {
                    matches!(&ir.arena[p].exp, Exp::FuncParam { ty, .. }
                             if matches!(ir.arena[*ty].exp, Exp::Auto))
                });
                let cap_param = format!("{name}$capture_param");
                let mut xs = Vec::new();
                if let Exp::Seq(bs) = &ir.arena[*fb].exp {
                    for &item in bs {
                        let Exp::Variable { init: Some(init), .. } = &ir.arena[item].exp else {
                            break;
                        };
                        let Exp::MemberAccess { base, member } = &ir.arena[*init].exp else {
                            break;
                        };
                        if !our_mentions(ir, interner, *base, &cap_param) {
                            break;
                        }
                        let Some(n) =
                            text(*member).strip_prefix('x').and_then(|d| d.parse::<usize>().ok())
                        else {
                            break;
                        };
                        xs.push(n);
                    }
                }
                let entry =
                    LambdaEntry { name, arity, params: ps.len() - 1, generic, xs };
                if entry.name.starts_with("$wrapper$") {
                    wrappers += 1;
                } else {
                    entries.push(entry);
                }
            }
        }
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
    entries.sort();
    LambdaLedger { entries, wrappers, survivors, unrolled: false }
}

/// `parallel $ desugarLambdas` (`Template.hs:78`) over our tree.
///
/// Over a fork of the **post-`captureThis`** IR and nothing later: the ledger's
/// window is the reference's `--template-passes 1`→`2` step, so composing
/// `abbreviatedFunctionTemplate` in front of the comparison would double every
/// entry inside a declaration it `deep_copy`s. `rewrite::pipeline` is where the
/// three do compose; this is deliberately not it.
///
/// `None` was the stub while the pass did not exist, and the option is kept
/// because it is what let the gate be written, falsified and measured against
/// the reference *before* there was anything on our side to compare — the
/// reverse of the order this project usually manages.
fn our_desugar_lambdas(
    ir: &kg_hir::ModuleIr,
    interner: &Interner,
) -> Option<kg_hir::ModuleIr> {
    Some(kg_hir::rewrite::desugar_lambdas(ir, interner))
}

/// `N` in the single `make_captureN` a `desugarLambdas` alias calls, or `None`
/// when the record holds none or several.
fn sole_make_capture(alias: &str) -> Option<usize> {
    let needle = "IdentifierF \"make_capture";
    let mut hits = alias.match_indices(needle);
    let (i, _) = hits.next()?;
    if hits.next().is_some() {
        return None;
    }
    let digits: String =
        alias[i + needle.len()..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// [`sole_make_capture`] over our arena.
fn our_sole_make_capture(
    ir: &kg_hir::ModuleIr,
    interner: &Interner,
    root: kg_hir::ExpId,
) -> Option<usize> {
    let mut found = None;
    let mut stack = vec![root];
    let mut children = Vec::new();
    while let Some(id) = stack.pop() {
        if let kg_hir::Exp::Identifier(s) = &ir.arena[id].exp
            && let Some(d) = interner.resolve(*s).strip_prefix("make_capture")
            && let Ok(n) = d.parse::<usize>()
        {
            if found.is_some() {
                return None;
            }
            found = Some(n);
        }
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
    found
}

/// Whether `root`'s subtree names `ident` — how the uncapture initializer is
/// told apart from a member access the user wrote (`Function.hs:262`).
fn our_mentions(
    ir: &kg_hir::ModuleIr,
    interner: &Interner,
    root: kg_hir::ExpId,
    ident: &str,
) -> bool {
    let mut stack = vec![root];
    let mut children = Vec::new();
    while let Some(id) = stack.pop() {
        if let kg_hir::Exp::Identifier(s) = &ir.arena[id].exp
            && interner.resolve(*s) == ident
        {
            return true;
        }
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
    false
}

/// The record's own `declName`, which `show` puts last (`Parser/Syntax.hs:518-522`
/// for `AliasF`) — so it is the trailing field, not any of the nested ones a
/// plain search would find first.
fn trailing_decl_name(rec: &str) -> Option<&str> {
    let rec = rec.strip_suffix('}')?;
    let head = "declName = IdentifierF \"";
    let at = rec.rfind(head)?;
    rec[at + head.len()..].strip_suffix('"')
}

/// The next `NameF` token at or after `from` that is a whole word.
///
/// `LambdaF` is a prefix of `LambdaFunction`, the very `funcKind` this pass
/// mints (`Function.hs:248`), so a substring search for the constructor counts
/// the pass's own output as un-rewritten input — it reads "nothing happened"
/// on a tree where everything did.
/// The trailing check applies only when the needle *ends* in a word byte.
/// `"AliasF {"` ends in a brace and is always followed by `declScopeName`, so
/// demanding a non-word byte there rejects every hit in the corpus — which is
/// what the first version of this did, silently, reporting nothing rewritten
/// on a tree holding 141 rewrites.
fn find_ctor(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let bytes = hay.as_bytes();
    let closed = needle.as_bytes().last().is_some_and(|&c| is_word(c));
    hay[from..].match_indices(needle).map(|(i, _)| i + from).find(|&i| {
        let before = i == 0 || !is_word(bytes[i - 1]);
        let j = i + needle.len();
        before && (!closed || j >= bytes.len() || !is_word(bytes[j]))
    })
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The index just past the bracket matching the one at `open`, skipping
/// Haskell string literals — an identifier's text can hold a `}`.
fn match_span(s: &str, open: usize) -> usize {
    let b = s.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'[' | b'{' | b'(' => depth += 1,
            // `open` may point at the constructor name rather than its brace,
            // so a stray closer before the first opener means "no span here"
            // rather than an underflow.
            b']' | b'}' | b')' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    s.len()
}

/// The depth-1 comma-separated items of a bracketed blob.
fn split_top(blob: &str) -> Vec<&str> {
    let b = blob.as_bytes();
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'[' | b'{' | b'(' => {
                depth += 1;
                if depth == 1 {
                    start = i + 1;
                }
            }
            b']' | b'}' | b')' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    items.push(&blob[start..i]);
                }
            }
            b',' if depth == 1 => {
                items.push(&blob[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    items.retain(|s| !s.trim().is_empty());
    items
}

// ---------------------------------------------------------------------------
// The move ledger — the reference's grade for `moveFunctions`.
// ---------------------------------------------------------------------------

/// Flip with the one-line replacement in [`our_move_functions`]. Until then the
/// `move diff` line reports the reference's half and enforces nothing.
const MOVE_PORTED: bool = true;

/// One declaration `moveFunctions` hoisted (`Function.hs:28-82`).
///
/// Every field is a direct function of **one** source node — the moved
/// declaration's own record — so a declaration this port can never hold (an
/// `addLambdaWrappers` `$wrapper$`) is one entry dropped by name rather than a
/// count that is wrong by an unattributable amount. Nothing here is an offset:
/// our spans and the reference's are different objects and agree for 93.4% of
/// nodes (divergence #25).
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
struct MoveEntry {
    /// The **structural** landing site: the constructor of the nearest
    /// enclosing declaration, `<root>` for the top-level `Seq`. Not the same
    /// question as [`scope`](Self::scope) and that is the point — `newScope`
    /// (`Function.hs:45-47`) rewrites `declScopeName` from the *parent's*
    /// qualifier whatever `Seq` the node is spliced into, so a port that
    /// mangles and re-scopes correctly but drops the declaration at module
    /// scope where the reference keeps it in the class body agrees on `scope`
    /// and differs only here.
    landing: String,
    /// `declScopeName` after `newScope`, `::`-joined.
    scope: String,
    /// `declName` after `mangleName` (`Function.hs:36`) — `getName parent ++
    /// "@" ++ getName e`, composed **left** as the move walks outward, so a
    /// double move reads `Run@Go@Do` and a port that composes the other way
    /// reads `Do@Go@Run`. The one field a wrong composition order cannot hide
    /// from.
    name: String,
    /// `Function` or `Class` — `functionsAndClasses` admits both
    /// (`Function.hs:119-122`).
    ctor: String,
    /// `funcKind`, empty for a `Class`. `moveToParentScope` does not touch it,
    /// so a lambda hoisted out of a class body is still `LambdaFunction` and a
    /// member function hoisted out of a member function is still
    /// `MemberFunction` — the reference's own nested-double-move transcript.
    ///
    /// Which is why it is worth carrying: it is the one field that reports on
    /// the tree the pass was *handed*. It found one disagreement —
    /// `test/logic/test_cases_49.k`'s `Test787_helper@Do`, `FreeFunction` here
    /// and `MemberFunction` there. Not a `moveFunctions` bug: the reference
    /// parses a nested function as `FreeFunction` too (`Parser.hs:478`), and
    /// **frontend pass 1** promotes it — `localMemberFunctions`
    /// (`Frontend.hs:219-225`), which is not ported. Bisected on `--passes`:
    /// `MemberFunction` already at `--passes 1`, `FreeFunction` at `0`.
    kind: String,
    /// `funcParams` length, `usize::MAX` for a `Class`. A cheap shape guard:
    /// it is what tells a moved declaration from a differently-arity one that
    /// happens to share its mangled name, and the corpus forces that —
    /// 35 duplicated `(scope, name)` pairs across 11 files.
    params: usize,
    /// The moved item was `Template _ Function{}`, which `mangleDeclName`
    /// pierces (`Function.hs:41`).
    templated: bool,
    /// `memberAccess`'s `$this.<member>` rewrite (`Function.hs:52-56`), as the
    /// sorted multiset of member names — 0 of these exist at `tp=3` and 571 at
    /// `tp=4`, so the field measures this sub-pass and nothing else.
    this_members: Vec<String>,
    /// `thisMethod` re-points (`Function.hs:58-62`). A **count**, not a
    /// multiset: unlike the member reads these already exist at `tp=3` (108 of
    /// them, from `addLambdaWrappers`), so the whole-file delta is not this
    /// pass's — only the per-declaration figure is.
    this_calls: usize,
    /// `fixScopedName` (`Function.hs:79-80`): the mangled names this pass wrote
    /// into references **inside** the declaration, as a sorted multiset.
    ///
    /// A mangled name exists only because `moveFunctions` made one, so unlike a
    /// raw qualifier count this cannot be forged by our sorted lowering — but
    /// only in the **leaf** position, and that restriction is the field's whole
    /// content. `fixScopedName` rewrites the *identifier* of a scoped name
    /// whose qualified name it moved; `qualifyName` (`:72-77`) builds the
    /// *prefix* chain in front of it, and fires only on `ScopedNameF a Nothing
    /// b` — a name with no written qualifier at all. `kg_db::sorted` attaches a
    /// qualifier to names the reference leaves bare, so on this side
    /// `qualifyName` correctly declines at sites where the reference fires, and
    /// the prefix that results names the *pre-move* path rather than the
    /// mangled one. Measured: counting prefix segments too leaves 6 of 351
    /// roots disagreeing on this field alone and on nothing else — always the
    /// declaration's own name, always ours the shorter multiset
    /// (`test/library/data/buffer/gearbox.k`, 2 against 5;
    /// `test/library/processor/risc-v.k`, 15 against 22). That is divergence
    /// #22 arriving in a third place, not a `moveFunctions` defect, and the
    /// only honest reading of the field is the leaf.
    ///
    /// So `qualifyName` is **not graded here** — neither its placement nor its
    /// content. Unit tests own it.
    ///
    /// A sorted **multiset of the names**, not a count: when this disagrees the
    /// question a reader has is *which* reference the reference re-rooted and we
    /// did not, and a number cannot answer it.
    ///
    /// Two exclusions, and the second was a defect this field shipped with.
    /// The declaration's own `declName` is excluded on both sides — it is the
    /// [`name`](Self::name) field and would otherwise be counted twice. And a
    /// whole **name chain** is excluded when any segment of it holds
    /// `$wrapper$`: `addLambdaWrappers` is unported, so a body that references
    /// a hoisted wrapper carries mangled identifiers this port can never hold.
    /// Excluding wrapper *entries* by name and then counting wrapper
    /// *references* applied the exclusion to one half of the gate and not to a
    /// field derived from the same population — measured on
    /// `test/library/data/buffer/burst_write.k`, where the reference's
    /// `BurstWriteBufferTests@BasicTest@?lambda0` reads 6 and this port can
    /// only ever reach 3: two are the wrapper's own name, and the third is the
    /// declaration's self-qualifier on the path
    /// `…@?lambda0::$::$wrapper$WriteCallback$capture`, which is not itself a
    /// `$wrapper$` token. Suppressing per identifier catches two of the three;
    /// suppressing per *chain* is what makes the exclusion the same exclusion.
    ///
    /// **This is not a grade of `qualifyName`.** The obvious field — how many
    /// `ScopedName`s carry an explicit qualifier at all — was built, run over
    /// the corpus and thrown away: it disagrees on 75 of 351 roots with ours
    /// systematically *higher* (2378 against the reference's 1338 over the 75,
    /// 112 against 58 on `test/library/control/async/rendezvous/lenient.k`),
    /// because `kg_db::sorted` attaches an explicit qualifier to names the
    /// reference leaves bare. That field graded the sorter. This one is the
    /// part of the same behaviour that survives: it counts only the names the
    /// pass itself minted.
    mangled_refs: Vec<String>,
}

/// A `Function`/`Class` the pass could have moved and did not — the other half
/// of the partition. Deliberately only identity: what a stuck declaration
/// *contains* is not this pass's doing.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
struct StuckEntry {
    scope: String,
    name: String,
    ctor: String,
}

/// Every declaration one root's tree accounts for, at `--template-passes 4`.
///
/// Deliberately not `PartialEq`: [`wrappers`](Self::wrappers) is a
/// reference-only field and a derived `==` would compare it against a port
/// that can only ever report 0.
#[derive(Default, Clone, Debug)]
struct MoveLedger {
    /// Sorted, so the comparison is a **multiset**. It has to be: 35
    /// `(landing scope, mangled name)` pairs recur across 11 corpus files, 14
    /// of them non-`$wrapper$` in 5 files, and a set silently swallows them.
    entries: Vec<MoveEntry>,
    /// Sorted. The complement of `entries` over the movable population — see
    /// the module comment for why this is the quantity that holds the
    /// `$wrapper$` exclusion down.
    stuck: Vec<StuckEntry>,
    /// Mangled declarations whose name holds `$wrapper$` in any segment,
    /// dropped from `entries` and only counted. 829 of the reference's 1515.
    /// The test is "holds", not "starts with": a wrapper moved out of a real
    /// function is named `test_main@$wrapper$TestAdd`, and prefix-matching
    /// catches only the 21 that never moved at all.
    wrappers: usize,
}

impl MoveLedger {
    /// Whether the two sides were handed the same movable population, which is
    /// what makes the comparison a question about *this* pass.
    fn same_population(&self, movable: usize) -> bool {
        self.entries.len() + self.stuck.len() == movable
    }

    /// The unported pass that accounts for **every** disagreement on this root,
    /// or `None` — in which case the root fails.
    ///
    /// Deliberately all-or-nothing per root. A root whose entries differ in two
    /// ways, one explained and one not, is a failure: booking it would hide the
    /// second behind the first, which is how a booking stops being honest.
    fn booked(&self, ours: &MoveLedger, higher_order: &std::collections::BTreeSet<String>)
        -> Option<Booked>
    {
        let their_names: Vec<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        let our_names: Vec<&str> = ours.entries.iter().map(|e| e.name.as_str()).collect();
        if our_names == their_names {
            // Same declarations moved, so nothing changed hands: `stuck` must
            // match exactly, and the only bookable difference is `kind`.
            if self.stuck != ours.stuck {
                return None;
            }
            let mut any = false;
            for (a, b) in ours.entries.iter().zip(&self.entries) {
                if a == b {
                    continue;
                }
                any = true;
                let only_kind = MoveEntry { kind: b.kind.clone(), ..a.clone() } == *b;
                if !only_kind || a.kind != "FreeFunction" || b.kind != "MemberFunction" {
                    return None;
                }
            }
            return any.then_some(Booked::LocalMemberFunctions);
        }
        // Different declarations moved. `higherOrderFunctions` produces one
        // shape and only one: the reference's `Template` deferral leaves a
        // declaration **nested**, so every extra entry of ours must show up on
        // the reference's side as a `stuck` entry we do not have. Requiring
        // both halves is what stops this branch from booking a port that
        // hoists something for its own reasons — that port's extra entry has
        // no stuck counterpart, because the reference moved the declaration
        // too and it is in neither `stuck` list.
        // Pair as a **multiset**, by whole entry. Pairing by first name match
        // is wrong here and was: 35 `(landing scope, mangled name)` pairs recur
        // across the corpus, and `abbreviatedFunctionTemplate` mints a *second*
        // declaration under the same mangled name for every generic lambda —
        // the `Template` and its leftover — so `find(|t| t.name == e.name)`
        // compares both of ours against the first of theirs and reports a
        // `templated` difference between two entries that each have an exact
        // partner. That is what it did on `test/logic/lambda_test_cases.k`.
        let mut their_left: Vec<&MoveEntry> = self.entries.iter().collect();
        let mut our_extra: Vec<&MoveEntry> = Vec::new();
        for e in &ours.entries {
            match their_left.iter().position(|t| *t == e) {
                Some(i) => {
                    their_left.swap_remove(i);
                }
                None => our_extra.push(e),
            }
        }
        // Anything left on the reference's side is a move this port did not
        // make, which is the opposite of what an unported `Template` produces —
        // with one exception, and it is the same pass's second shape.
        //
        // `higherOrderFunctions` wraps an *inline* function that takes a
        // function-typed parameter in a `Template` (`Function.hs:313-325`), and
        // `moveFunctions` then hoists the `Template` along with it, so the
        // reference's entry for that declaration is ours in every field but
        // `templated`. Two consequences follow from that one `Template` and
        // from nothing else, so both are required rather than tolerated: the
        // declaration must itself be a higher-order site in our own pre-move
        // tree, and — because clause 2 reverts on a `Template`
        // (`Function.hs:86`), so the reference never descends into that body —
        // the only `mangled_refs` ours may hold beyond the reference's are the
        // names of declarations we hoisted out of that very body.
        let extra_names: Vec<&str> = our_extra.iter().map(|e| e.name.as_str()).collect();
        let mut any = false;
        let mut theirs_unmatched: Vec<&MoveEntry> = Vec::new();
        for t in their_left {
            let hit = our_extra.iter().position(|e| {
                if e.templated
                    || !t.templated
                    || !higher_order.contains(&site_key(&e.scope, &e.name))
                {
                    return false;
                }
                let prefix = format!("{}@", e.name);
                let refs: Vec<String> = e
                    .mangled_refs
                    .iter()
                    .filter(|r| !(r.starts_with(&prefix) && extra_names.contains(&r.as_str())))
                    .cloned()
                    .collect();
                MoveEntry { templated: true, mangled_refs: refs, ..(*e).clone() } == *t
            });
            match hit {
                Some(i) => {
                    our_extra.swap_remove(i);
                    any = true;
                }
                None => theirs_unmatched.push(t),
            }
        }
        if !theirs_unmatched.is_empty() {
            return None;
        }
        // Multiset difference over the stuck names, in both directions.
        let mut unexplained: Vec<&str> = Vec::new();
        let mut theirs_left: Vec<&StuckEntry> = self.stuck.iter().collect();
        for e in &ours.stuck {
            match theirs_left.iter().position(|t| *t == e) {
                Some(i) => {
                    theirs_left.swap_remove(i);
                }
                // We left something nested that the reference did not. That is
                // the opposite of what this pass's absence produces.
                None => return None,
            }
        }
        unexplained.extend(theirs_left.iter().map(|t| t.name.as_str()));

        any |= !our_extra.is_empty();
        for e in our_extra {
            // Two shapes, and the pass produces both.
            //
            // The declaration the reference had a `Template` over either never
            // moved — then it is the **outermost** mangled segment, and our own
            // pre-move tree is where its higher-order guard is checked — or it
            // moved *as* a `Template` and the reference simply did not descend
            // into it, in which case the reference's own entry for it says
            // `templated`. `test/logic/lambda_test_cases.k` is the second:
            // `higherOrderFunctions` templates the **inner** `Go((auto x) ->
            // auto Do, uint32 a)`, not the outer function, so the reference
            // holds `…@Go` templated and this port descends into
            // `abbreviatedFunctionTemplate`'s leftover copy — which on the
            // reference does not exist, because clause 2 reverts on the
            // `Template` that pass had already made.
            let (outer, _) = e.name.split_once('@')?;
            let path = site_key(&e.scope, outer);
            let under_template = self.entries.iter().any(|t| {
                t.templated && e.name.starts_with(&format!("{}@", t.name))
            });
            if !higher_order.contains(&path) && !under_template {
                return None;
            }
            let written = e.name.rsplit('@').next().unwrap_or(&e.name);
            match unexplained.iter().position(|n| *n == written) {
                Some(i) => {
                    unexplained.swap_remove(i);
                }
                None => return None,
            }
        }
        if !unexplained.is_empty() {
            return None;
        }
        any.then_some(Booked::HigherOrderFunctions)
    }

    /// The first disagreement: which behaviour of the pass it belongs to, and
    /// a line a reader can act on.
    ///
    /// The **name of the field** is what a falsification run reads, not the
    /// root count. Against a partial port most roots are red for one reason —
    /// with a stub that does no `qualifyName`, 76 of 77 fail on
    /// `mangled_refs` alone — and a single number cannot then show that a
    /// plant in the mangling moved anything. A per-field histogram can, and it
    /// is also what a real failure wants to say.
    ///
    /// Names are compared before fields, deliberately: a *missing* move shifts
    /// every later entry in the sorted vector, so a positional walk would
    /// report the wrong field for the wrong declaration.
    fn disagreement(&self, ours: &MoveLedger) -> Option<(&'static str, String)> {
        let their_names: Vec<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        let our_names: Vec<&str> = ours.entries.iter().map(|e| e.name.as_str()).collect();
        if our_names != their_names {
            if let Some(e) = ours.entries.iter().find(|e| !self.entries.iter().any(|t| t.name == e.name)) {
                return Some(("hoisted-too-many", format!("ours moved {:?}, theirs did not", e.name)));
            }
            if let Some(t) = self.entries.iter().find(|t| !ours.entries.iter().any(|e| e.name == t.name)) {
                return Some(("hoisted-too-few", format!("theirs moved {:?}, ours did not", t.name)));
            }
            return Some((
                "hoisted-multiplicity",
                format!("ours {our_names:?} vs theirs {their_names:?}"),
            ));
        }
        for (a, b) in ours.entries.iter().zip(&self.entries) {
            let field = if a.landing != b.landing {
                "landing"
            } else if a.scope != b.scope {
                "scope"
            } else if a.ctor != b.ctor {
                "ctor"
            } else if a.kind != b.kind {
                "kind"
            } else if a.params != b.params {
                "params"
            } else if a.templated != b.templated {
                "templated"
            } else if a.this_members != b.this_members {
                "this-members"
            } else if a.this_calls != b.this_calls {
                "this-calls"
            } else if a.mangled_refs != b.mangled_refs {
                "mangled-refs"
            } else {
                continue;
            };
            return Some((field, format!("ours {a:?} vs theirs {b:?}")));
        }
        if ours.stuck != self.stuck {
            if let Some(e) = ours.stuck.iter().find(|e| !self.stuck.contains(e)) {
                return Some(("stuck-too-many", format!("ours left {e:?} nested, theirs did not")));
            }
            if let Some(t) = self.stuck.iter().find(|t| !ours.stuck.contains(t)) {
                return Some(("stuck-too-few", format!("theirs left {t:?} nested, ours did not")));
            }
            return Some(("stuck-multiplicity", format!("{} vs {}", ours.stuck.len(), self.stuck.len())));
        }
        None
    }
}

/// `kg-oracle --tree --passes 8 --template-passes 4 --template-iterations 0`
/// on every file, spread over threads.
///
/// `--passes 8` is mandatory: without it `removeTemplate` (`Frontend.hs:300-302`,
/// frontend execution order 15) deletes the output and every ledger is empty on
/// a tree where everything happened. `--template-iterations 0` is what makes
/// `instantiateTemplates` honour `templatePasses` at all (`Template.hs:62-64`).
/// The `3`-to-`4` step is `moveFunctions` alone — `passes` is written in
/// reverse execution order and `drop (length passes - templatePasses)`
/// (`Template.hs:63`) keeps the last N, so `4` adds exactly `Template.hs:76`.
fn oracle_move_ledgers(
    oracle: &Path,
    lib: &Path,
    base: &Path,
    files: &[(PathBuf, String)],
) -> Vec<Option<MoveLedger>> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<Option<MoveLedger>>> =
        files.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= files.len() {
                    break;
                }
                let (path, _) = &files[i];
                let line = oracle_line(oracle, lib, base, path, &[
                    "--tree", "--passes", "8",
                    "--template-passes", "4", "--template-iterations", "0",
                ]);
                *out[i].lock().expect("no panics hold this lock") =
                    line.as_deref().map(reference_move_ledger);
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined")).collect()
}

/// A declaration name `moveFunctions` composed — `parent@child`.
///
/// The `@` is also the module-namespace separator (`moduleNamespace =
/// concatMap ('@':)`, `Parser/Syntax.hs:273-274`), so `@data@closure` is not a
/// move; the leading-`@` test excludes it. Measured over the corpus at
/// `tp=3`: **56** declarations already answer yes to "holds an `@`", and all
/// 56 are `addLambdaWrappers`' `$wrapper$@data@order$less_than` shape, which
/// [`MoveLedger::wrappers`] excludes on its own account. So among non-wrapper
/// declarations this predicate is exactly "moveFunctions renamed it": 0 at
/// `tp=3`, 686 at `tp=4`.
fn is_mangled(name: &str) -> bool {
    name.contains('@') && !name.starts_with('@')
}

/// The declaration constructors a nested declaration is nested *in* — the
/// bodies `moveToParentScope` empties, plus the ones it does not, so that
/// "still nested" is a question about the tree rather than about a name.
///
/// `Template` is deliberately **not** here: it wraps a declaration without
/// being a scope, `functionsAndClasses` matches through it
/// (`Function.hs:120`), and `mangleDeclName` pierces it (`:41`). It is
/// recorded as [`MoveEntry::templated`] instead.
fn is_container(ctor: &str) -> bool {
    matches!(
        ctor,
        "Function" | "Class" | "Struct" | "Union" | "NestedScope" | "Lambda" | "ModuleDecl"
    )
}

/// A body `moveToParentScope`'s `functionsAndClasses` arm empties
/// (`Function.hs:96-99`).
fn is_movable_body(ctor: &str) -> bool {
    matches!(ctor, "Function" | "NestedScope" | "Lambda")
}

/// One record of the reference's `show` output, while the scan is inside it.
#[derive(Default)]
struct Frame {
    /// Bracket depth *inside* the record, so a field of this record is exactly
    /// a token read at this depth and a nested record's fields never are.
    depth: usize,
    ctor: String,
    scope: String,
    name: String,
    kind: String,
    params: usize,
    this_members: Vec<String>,
    this_calls: usize,
    mangled_refs: Vec<String>,
}

/// The ledger, read off one `--tree --template-passes 4` line.
///
/// **One linear pass.** The obvious reader — find each `FunctionF {`, take its
/// balanced span, `split_top` the fields — is quadratic in the nesting, and
/// `test/library/processor/risc-v.k` renders to 13 MB. So the scan carries a
/// frame stack instead: a record's own fields are the tokens read at its own
/// bracket depth, a `FuncParamF` frame increments its *parent's* arity, and
/// every `$this` hit is attributed to the innermost enclosing declaration at
/// the moment it is seen.
fn reference_move_ledger(line: &str) -> MoveLedger {
    let b = line.as_bytes();
    let mut ledger = MoveLedger::default();
    let mut frames: Vec<Frame> = Vec::new();
    let mut pending: Option<String> = None;
    let mut depth = 0usize;
    let mut i = 0usize;
    // A `ScopedNameF` chain renders positionally, outermost keyword first and
    // its prefix nested inside its own `(Just …)`, so the first one the scan
    // meets is the chain root and everything up to `chain_until` is the rest of
    // that chain. `suppress_until` is the same span when the chain names
    // something `$wrapper$`.
    let mut chain = Chain::default();
    while i < b.len() {
        match b[i] {
            b'"' => {
                i = skip_string(b, i);
            }
            b'{' => {
                depth += 1;
                if let Some(ctor) = pending.take() {
                    frames.push(Frame { depth, ctor, ..Frame::default() });
                }
                i += 1;
            }
            b'[' | b'(' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                if frames.last().is_some_and(|f| f.depth == depth) {
                    let frame = frames.pop().expect("just checked");
                    finish_frame(&mut ledger, &mut frames, frame);
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b']' | b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            c if is_word(c) && (i == 0 || !is_word(b[i - 1])) => {
                let start = i;
                while i < b.len() && is_word(b[i]) {
                    i += 1;
                }
                let word = &line[start..i];
                // A record constructor is the only word followed by ` {`.
                if word.len() > 1 && word.ends_with('F') && line[i..].starts_with(" {") {
                    pending = Some(word[..word.len() - 1].to_owned());
                    continue;
                }
                if word == "ScopedNameF" && start >= chain.until {
                    chain.open(line, b, start, i);
                }
                read_token(line, word, start, i, depth, &mut frames, &chain);
            }
            _ => i += 1,
        }
    }
    ledger.entries.sort();
    ledger.stuck.sort();
    ledger
}

/// The index just past the string literal opening at `i`.
fn skip_string(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() && b[i] != b'"' {
        i += if b[i] == b'\\' { 2 } else { 1 };
    }
    i + 1
}

/// The index just past the `[…]` at `j`, string-aware — a `Name` can hold a
/// `]`.
fn skip_list(b: &[u8], mut j: usize) -> Option<usize> {
    if b.get(j) != Some(&b'[') {
        return None;
    }
    j += 1;
    let mut depth = 1usize;
    while j < b.len() {
        match b[j] {
            b'"' => {
                j = skip_string(b, j);
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// The quoted string starting at `j`.
fn quoted(line: &str, j: usize) -> Option<String> {
    let b = line.as_bytes();
    if b.get(j) != Some(&b'"') {
        return None;
    }
    let end = skip_string(b, j);
    Some(line[j + 1..end - 1].to_owned())
}

/// One non-constructor word, and everything the scan learns from it.
///
/// Field markers (`declScopeName`, `declName`, `funcKind`) count only at the
/// innermost frame's own depth. The three re-qualification anchors are
/// positional shapes with no braces of their own, so they are matched forward
/// from the constructor name and attributed to the innermost `Function`/`Class`
/// frame — which is what makes a nested declaration's rewrites belong to *it*
/// and not to the declaration it sits in.
fn read_token(
    line: &str,
    word: &str,
    start: usize,
    after: usize,
    depth: usize,
    frames: &mut [Frame],
    chain: &Chain,
) {
    let b = line.as_bytes();
    match word {
        "declScopeName" | "declName" | "funcKind" => {
            let Some(frame) = frames.last_mut() else { return };
            if frame.depth != depth {
                return;
            }
            let Some(rest) = line[after..].strip_prefix(" = ") else { return };
            let at = line.len() - rest.len();
            match word {
                "declScopeName" => {
                    if frame.scope.is_empty()
                        && let Some(end) = skip_list(b, at)
                    {
                        frame.scope = join_name_list(&line[at..end]);
                    }
                }
                "declName" => {
                    if frame.name.is_empty()
                        && let Some(q) = rest.strip_prefix("IdentifierF ")
                    {
                        frame.name =
                            quoted(line, line.len() - q.len()).unwrap_or_default();
                    }
                }
                _ => {
                    if frame.kind.is_empty() {
                        frame.kind =
                            rest.chars().take_while(char::is_ascii_alphabetic).collect();
                    }
                }
            }
        }
        // `memberAccess`' `MemberAccessF (thisValue a) name` (`Function.hs:53`,
        // `:56`), spelled by `thisNamedValue` (`Type.hs:991-995`).
        "MemberAccessF" => {
            let Some(rest) =
                line[after..].strip_prefix(" NamedValueF QualifiedIdentifierF ScopedNameF ")
            else {
                return;
            };
            let Some(end) = skip_list(b, line.len() - rest.len()) else { return };
            let Some(tail) = line[end..].strip_prefix(" Nothing IdentifierF \"$this\" IdentifierF ")
            else {
                return;
            };
            let Some(member) = quoted(line, line.len() - tail.len()) else { return };
            if let Some(f) = innermost_decl(frames) {
                f.this_members.push(member);
            }
        }
        // `thisMethod` (`Function.hs:67`) — the same value, as a receiver.
        "FunctionSpecifierF" => {
            let Some(rest) = line[after..]
                .strip_prefix(" (Just NamedValueF QualifiedIdentifierF ScopedNameF ")
            else {
                return;
            };
            let Some(end) = skip_list(b, line.len() - rest.len()) else { return };
            if line[end..].starts_with(" Nothing IdentifierF \"$this\")")
                && let Some(f) = innermost_decl(frames)
            {
                f.this_calls += 1;
            }
        }
        // `fixScopedName` mangles the *identifier* of a reference to a moved
        // declaration (`Function.hs:79-80`), and `qualifyName` spells the
        // declaration's own new qualifier with the same name (`:72-77`). Both
        // read off as a mangled `IdentifierF` inside the record; the record's
        // own `declName` is not one of them.
        "IdentifierF" => {
            // Inside a chain, only the leaf counts — see
            // [`MoveEntry::mangled_refs`]. Outside one, an `IdentifierF` is a
            // bare name and counts as written.
            if start < chain.suppress
                || (start < chain.until && start != chain.leaf)
                || line[..start].ends_with("declName = ")
            {
                return;
            }
            let Some(rest) = line[after..].strip_prefix(' ') else { return };
            let Some(name) = quoted(line, line.len() - rest.len()) else { return };
            if is_mangled(&name)
                && let Some(f) = innermost_decl(frames)
            {
                f.mangled_refs.push(name);
            }
        }
        _ => {}
    }
}

/// The extent of the `ScopedNameF` chain the scan is inside, and whether that
/// chain names anything `$wrapper$`.
///
/// `ScopedNameF <scope> Nothing IdentifierF "leaf"` or
/// `ScopedNameF <scope> (Just <inner chain>) IdentifierF "leaf"` — positional,
/// no braces of its own, and the qualifier nests *inside* the outer one's
/// parentheses. So the first `ScopedNameF` the scan meets is the root, and its
/// span covers every segment: computing the span once and skipping until its
/// end is both the de-duplication and the place the `$wrapper$` exclusion
/// belongs.
#[derive(Default)]
struct Chain {
    until: usize,
    suppress: usize,
    /// Byte offset of the chain's leaf `IdentifierF`, the one `fixScopedName`
    /// rewrites. Only meaningful while the scan is inside the chain, which
    /// `until` is what says.
    leaf: usize,
}

impl Chain {
    /// `word_end` is the index just past the `ScopedNameF` keyword at `start`.
    fn open(&mut self, line: &str, b: &[u8], start: usize, word_end: usize) {
        let Some(rest) = line[word_end..].strip_prefix(' ') else { return };
        let Some(mut j) = skip_list(b, line.len() - rest.len()) else { return };
        if let Some(r) = line[j..].strip_prefix(" Nothing ") {
            j = line.len() - r.len();
        } else if line[j..].starts_with(" (") {
            let Some(e) = skip_parens(b, j + 1) else { return };
            let Some(r) = line[e..].strip_prefix(' ') else { return };
            j = line.len() - r.len();
        } else {
            return;
        }
        let Some(r) = line[j..].strip_prefix("IdentifierF ") else { return };
        let leaf = line.len() - r.len();
        if b.get(leaf) != Some(&b'"') {
            return;
        }
        let end = skip_string(b, leaf);
        self.until = end;
        self.leaf = j;
        if line[start..end].contains("$wrapper$") {
            self.suppress = end;
        }
    }
}

/// The index just past the `(…)` at `j`, string-aware.
fn skip_parens(b: &[u8], mut j: usize) -> Option<usize> {
    if b.get(j) != Some(&b'(') {
        return None;
    }
    j += 1;
    let mut depth = 1usize;
    while j < b.len() {
        match b[j] {
            b'"' => {
                j = skip_string(b, j);
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// Why a disagreeing root is **booked** rather than failed: the named upstream
/// pass this port does not have, which handed `moveFunctions` a different tree.
///
/// `oracle diff` is the precedent — 116/124, and it does not bail, because its
/// 8 are booked with named causes. A booking that names no pass is a suppressed
/// failure, so each variant here carries a citation and a predicate that is a
/// property of *that pass's own guard*, never "this root disagrees".
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Booked {
    /// `localMemberFunctions` (`Frontend.hs:219-225`), **frontend pass 1**:
    /// `funcKind == MemberFunction => body = desugar makeMember body`, which
    /// promotes every `Function` written inside a member function's body.
    /// The reference parses one as `FreeFunction` too (`Parser.hs:478`);
    /// bisected on `--passes`, it is already `MemberFunction` at `1`.
    ///
    /// Predicate: **every** disagreeing entry differs in `kind` and in nothing
    /// else, and in the one direction that pass produces — ours
    /// `FreeFunction`, theirs `MemberFunction`. Ours `MemberFunction` where
    /// theirs is `FreeFunction`, or any `LambdaFunction` involved, is not this
    /// pass and is not booked.
    LocalMemberFunctions,
    /// `higherOrderFunctions` (`Function.hs:313-325`), **frontend pass 7**: an
    /// `inline` function with a function-type parameter becomes a `Template`,
    /// which is `moveFunctions`' first deferral arm (`Function.hs:88`), so the
    /// reference declines to descend where this port has no `Template` and
    /// moves.
    ///
    /// The absence of that pass shows up in **two** shapes, and both are
    /// spelled out in [`MoveLedger::booked`] because each clause is
    /// load-bearing: the `Template` may sit over a declaration that then never
    /// moves — the reference holds it `stuck` and we hoist what is under it —
    /// or over one that moves *as* a `Template`, in which case the reference's
    /// own entry for it says `templated` and ours does not.
    ///
    /// The guard itself is read off **our** pre-move tree
    /// ([`higher_order_sites`]), never inferred from the disagreement: the
    /// question the predicate asks is "would that pass have fired here", not
    /// "does this difference look like that pass".
    HigherOrderFunctions,
}

impl Booked {
    fn label(self) -> &'static str {
        match self {
            Booked::LocalMemberFunctions => "localMemberFunctions (Frontend.hs:219-225, pass 1)",
            Booked::HigherOrderFunctions => "higherOrderFunctions (Function.hs:313-325, pass 7)",
        }
    }
}

/// The declarations `higherOrderFunctions` would have wrapped in a `Template`,
/// as `scope::name` — read off **our** tree, which is the half that lacks the
/// pass.
///
/// `not $ null functionTypeParams` over `resolvedFuncParams`
/// (`Function.hs:315,321`). The `resolved` half is an alias walk this does not
/// do, so the set is a *subset* of the reference's: a parameter whose written
/// type is an alias for a function type is missed, and a root that needed it
/// stays red rather than being booked. Under-booking is the safe direction.
fn higher_order_sites(ir: &kg_hir::ModuleIr, interner: &Interner) -> std::collections::BTreeSet<String> {
    use kg_hir::Exp;
    let mut out = std::collections::BTreeSet::new();
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        if let Exp::Function { scope, modifier, name, params, .. } = &ir.arena[id].exp
            && modifier.is_some_and(|m| {
                matches!(ir.arena[m].exp, Exp::FunctionModifier(kg_hir::Modifier::Inline))
            })
            && let Exp::Seq(items) = &ir.arena[*params].exp
            && items.iter().any(|&p| {
                matches!(&ir.arena[p].exp, Exp::FuncParam { ty, .. }
                         if matches!(ir.arena[*ty].exp, Exp::FunctionType { .. }))
            })
            && let Exp::Identifier(n) = &ir.arena[*name].exp
        {
            let scope = scope
                .segments()
                .iter()
                .map(|x| interner.resolve(*x).to_string())
                .collect::<Vec<_>>()
                .join("::");
            out.insert(site_key(&scope, &interner.resolve(*n)));
        }
        ir.arena[id].exp.children(&mut kids);
        stack.append(&mut kids);
    }
    out
}

/// Where a declaration was written, as one string, spelled the same way from
/// either half of the ledger.
///
/// The two halves arrive in different alphabets. Our pre-move tree carries a
/// `QualifiedName` whose segments include `$` — the reference's name for a
/// function *body* scope (`HofCallsHelper::$::hof_calls_non_inline_fn`) — while
/// a moved entry carries its landing scope plus the `@`-joined chain
/// `mangleDeclName` built (`Function.hs:35-45`), which holds no `$` because
/// only *named* declarations are mangled. Dropping `$` is what makes a nested
/// function and a class member key alike; without it the higher-order check
/// never matches anything below module scope, which is what it was doing.
fn site_key(scope: &str, name: &str) -> String {
    scope
        .split("::")
        .flat_map(|s| s.split('@'))
        .chain(name.split("::").flat_map(|s| s.split('@')))
        .filter(|s| !s.is_empty() && *s != "$")
        .collect::<Vec<_>>()
        .join("::")
}

/// The innermost `Function`/`Class` frame — the declaration a rewrite inside it
/// belongs to.
fn innermost_decl(frames: &mut [Frame]) -> Option<&mut Frame> {
    frames.iter_mut().rev().find(|f| f.ctor == "Function" || f.ctor == "Class")
}

/// `["a","b"]` as `a::b`, which is how our side spells a `QualifiedName`.
fn join_name_list(list: &str) -> String {
    let mut out = String::new();
    let b = list.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'"' {
            let end = skip_string(b, i);
            if !out.is_empty() {
                out.push_str("::");
            }
            out.push_str(&list[i + 1..end - 1]);
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// A closed record: what it contributes to the ledger, and to its parent.
fn finish_frame(ledger: &mut MoveLedger, frames: &mut [Frame], frame: Frame) {
    if frame.ctor == "FuncParam" {
        if let Some(parent) = frames.last_mut() {
            parent.params += 1;
        }
        return;
    }
    if frame.ctor != "Function" && frame.ctor != "Class" {
        return;
    }
    // The stack no longer holds this frame, so the search is over its
    // ancestors. `Template` is skipped by `is_container` and read separately.
    let templated = frames.last().is_some_and(|f| f.ctor == "Template");
    let landing = frames
        .iter()
        .rev()
        .find(|f| is_container(&f.ctor))
        .map_or("<root>", |f| f.ctor.as_str())
        .to_owned();

    if is_mangled(&frame.name) {
        if frame.name.contains("$wrapper$") {
            ledger.wrappers += 1;
            return;
        }
        let mut this_members = frame.this_members;
        this_members.sort();
        let mut mangled_refs = frame.mangled_refs;
        mangled_refs.sort();
        ledger.entries.push(MoveEntry {
            landing,
            scope: frame.scope,
            name: frame.name,
            params: if frame.ctor == "Class" { usize::MAX } else { frame.params },
            ctor: frame.ctor,
            kind: frame.kind,
            templated,
            this_members,
            this_calls: frame.this_calls,
            mangled_refs,
        });
        return;
    }
    if frame.name.contains("$wrapper$") {
        return;
    }
    // Still nested: the pass could have taken it and did not.
    let movable = is_movable_body(&landing)
        || (landing == "Class" && frame.kind == "LambdaFunction");
    if movable {
        ledger.stuck.push(StuckEntry {
            scope: frame.scope,
            name: frame.name,
            ctor: frame.ctor,
        });
    }
}

/// The same ledger, read off our own rewritten tree.
///
/// Deliberately the same shape as [`reference_move_ledger`] rather than
/// something the arena makes easier: the two readers must be able to disagree
/// only about what the *pass* produced.
fn our_move_ledger(ir: &kg_hir::ModuleIr, interner: &Interner) -> MoveLedger {
    let mut ledger = MoveLedger::default();
    walk_decls(ir, interner, ir.root, &mut Vec::new(), &mut |ctx, id, stack| {
        let (ctor, scope, name, kind, params) = decl_facts(ctx.ir, ctx.interner, id);
        let templated = stack.last().is_some_and(|c| *c == "Template");
        let landing = stack
            .iter()
            .rev()
            .find(|c| is_container(c))
            .map_or("<root>", |c| *c)
            .to_owned();
        if is_mangled(&name) {
            if name.contains("$wrapper$") {
                ledger.wrappers += 1;
                return;
            }
            let (this_members, this_calls, mangled_refs) =
                requalification(ctx.ir, ctx.interner, id);
            ledger.entries.push(MoveEntry {
                landing,
                scope,
                name,
                params: if ctor == "Class" { usize::MAX } else { params },
                ctor: ctor.to_owned(),
                kind,
                templated,
                this_members,
                this_calls,
                mangled_refs,
            });
            return;
        }
        if name.contains("$wrapper$") {
            return;
        }
        if is_movable_body(&landing) || (landing == "Class" && kind == "LambdaFunction") {
            ledger.stuck.push(StuckEntry { scope, name, ctor: ctor.to_owned() });
        }
    });
    ledger.entries.sort();
    ledger.stuck.sort();
    ledger
}

/// Every `Function`/`Class` the pass **could** move, counted on the tree it is
/// handed — the left-hand side of the conservation law.
fn our_movable(ir: &kg_hir::ModuleIr, interner: &Interner) -> usize {
    let mut n = 0usize;
    walk_decls(ir, interner, ir.root, &mut Vec::new(), &mut |ctx, id, stack| {
        let (_, _, _, kind, _) = decl_facts(ctx.ir, ctx.interner, id);
        let landing = stack.iter().rev().find(|c| is_container(c)).map_or("<root>", |c| *c);
        if is_movable_body(landing) || (landing == "Class" && kind == "LambdaFunction") {
            n += 1;
        }
    });
    n
}

struct WalkCtx<'a> {
    ir: &'a kg_hir::ModuleIr,
    interner: &'a Interner,
}

/// Pre-order over the reachable tree, calling `f` at every `Function`/`Class`
/// with the stack of enclosing constructor names — the walk both readers above
/// share, and the exact analogue of the reference reader's frame stack.
fn walk_decls(
    ir: &kg_hir::ModuleIr,
    interner: &Interner,
    root: kg_hir::ExpId,
    stack: &mut Vec<&'static str>,
    f: &mut impl FnMut(&WalkCtx<'_>, kg_hir::ExpId, &[&'static str]),
) {
    use kg_hir::Exp;
    let exp = &ir.arena[root].exp;
    let ctor = ctor_name(exp);
    if matches!(exp, Exp::Function { .. } | Exp::Class { .. }) {
        f(&WalkCtx { ir, interner }, root, stack);
    }
    let mut kids = Vec::new();
    exp.children(&mut kids);
    if let Some(ctor) = ctor {
        stack.push(ctor);
    }
    for k in kids {
        walk_decls(ir, interner, k, stack, f);
    }
    if ctor.is_some() {
        stack.pop();
    }
}

/// The constructors the frame stack records — every one the reference's `show`
/// renders as a record and this walk needs to see.
fn ctor_name(exp: &kg_hir::Exp) -> Option<&'static str> {
    use kg_hir::Exp;
    Some(match exp {
        Exp::Function { .. } => "Function",
        Exp::Class { .. } => "Class",
        Exp::Struct { .. } => "Struct",
        Exp::Union { .. } => "Union",
        Exp::NestedScope { .. } => "NestedScope",
        Exp::Lambda { .. } => "Lambda",
        Exp::ModuleDecl { .. } => "ModuleDecl",
        Exp::Template { .. } => "Template",
        _ => return None,
    })
}

/// `(ctor, scope, name, funcKind, arity)` of one declaration.
fn decl_facts(
    ir: &kg_hir::ModuleIr,
    interner: &Interner,
    id: kg_hir::ExpId,
) -> (&'static str, String, String, String, usize) {
    use kg_hir::Exp;
    let text = |n: kg_hir::ExpId| match &ir.arena[n].exp {
        Exp::Identifier(s) => interner.resolve(*s).to_string(),
        _ => String::new(),
    };
    let path = |q: &kg_ty::QualifiedName| {
        q.segments().iter().map(|s| interner.resolve(*s)).collect::<Vec<_>>().join("::")
    };
    match &ir.arena[id].exp {
        Exp::Function { scope, kind, name, params, .. } => {
            let arity = match &ir.arena[*params].exp {
                Exp::Seq(items) => items.len(),
                _ => 0,
            };
            ("Function", path(scope), text(*name), format!("{kind:?}"), arity)
        }
        Exp::Class { scope, name, .. } => {
            ("Class", path(scope), text(*name), String::new(), usize::MAX)
        }
        _ => ("", String::new(), String::new(), String::new(), 0),
    }
}

/// The three re-qualification measurements over one declaration's subtree,
/// stopping at a nested `Function`/`Class` so the attribution matches the
/// reference reader's innermost rule.
///
/// The `ScopedName` half mirrors [`Chain`]: a chain is read whole at its root
/// and dropped entire when any segment holds `$wrapper$`, so the exclusion that
/// removes `addLambdaWrappers`' declarations from the ledger removes its
/// references from this field too.
fn requalification(
    ir: &kg_hir::ModuleIr,
    interner: &Interner,
    root: kg_hir::ExpId,
) -> (Vec<String>, usize, Vec<String>) {
    use kg_hir::Exp;
    let mut members = Vec::new();
    let mut refs: Vec<String> = Vec::new();
    let mut calls = 0usize;
    // Every declaration's own name node — the reference reader skips each
    // `declName = IdentifierF` unconditionally, so this does too, and not just
    // for the root.
    let mut is_decl_name = vec![false; ir.arena.len()];
    let mut consumed = vec![false; ir.arena.len()];
    let mut stack = vec![(root, true)];
    let mut kids = Vec::new();
    while let Some((id, is_root)) = stack.pop() {
        let exp = &ir.arena[id].exp;
        if !is_root && matches!(exp, Exp::Function { .. } | Exp::Class { .. }) {
            continue;
        }
        if let Some(n) = decl_name_of(exp) {
            is_decl_name[n.index()] = true;
        }
        match exp {
            Exp::MemberAccess { base, member } if is_this_value(ir, interner, *base) => {
                if let Exp::Identifier(s) = &ir.arena[*member].exp {
                    members.push(interner.resolve(*s).to_string());
                }
            }
            Exp::FunctionSpecifier { receiver: Some(r), .. }
                if is_this_value(ir, interner, *r) =>
            {
                calls += 1;
            }
            Exp::ScopedName { .. } if !consumed[id.index()] => {
                scoped_name_chain(ir, interner, id, &mut consumed, &mut refs);
            }
            Exp::Identifier(s)
                if !consumed[id.index()]
                    && !is_decl_name[id.index()]
                    && is_mangled(&interner.resolve(*s)) =>
            {
                refs.push(interner.resolve(*s).to_string());
            }
            _ => {}
        }
        exp.children(&mut kids);
        stack.extend(kids.drain(..).map(|k| (k, false)));
    }
    members.sort();
    refs.sort();
    (members, calls, refs)
}

/// One `ScopedName` chain, read whole: mark every node of it consumed, and add
/// its mangled segments unless any segment holds `$wrapper$`.
fn scoped_name_chain(
    ir: &kg_hir::ModuleIr,
    interner: &Interner,
    root: kg_hir::ExpId,
    consumed: &mut [bool],
    refs: &mut Vec<String>,
) {
    use kg_hir::Exp;
    let mut segments: Vec<String> = Vec::new();
    let mut node = Some(root);
    // `segments` is filled leaf-first; see the tail of this function.
    while let Some(id) = node {
        let Exp::ScopedName { prefix, ident, .. } = &ir.arena[id].exp else { break };
        consumed[id.index()] = true;
        consumed[ident.index()] = true;
        if let Exp::Identifier(s) = &ir.arena[*ident].exp {
            segments.push(interner.resolve(*s).to_string());
        }
        node = *prefix;
    }
    if segments.iter().any(|s| s.contains("$wrapper$")) {
        return;
    }
    // `segments[0]` is the chain root's own `ident` — the leaf, the one
    // `fixScopedName` rewrites. Everything after it is `qualifyName`'s prefix,
    // which this field deliberately does not read.
    if segments.first().is_some_and(|s| is_mangled(s)) {
        refs.push(segments.swap_remove(0));
    }
}

/// The `declName` slot of any declaration the reference's `show` prints one for.
fn decl_name_of(exp: &kg_hir::Exp) -> Option<kg_hir::ExpId> {
    use kg_hir::Exp;
    Some(match exp {
        Exp::Function { name, .. }
        | Exp::Class { name, .. }
        | Exp::Struct { name, .. }
        | Exp::Union { name, .. }
        | Exp::Enum { name, .. }
        | Exp::EnumConstant { name, .. }
        | Exp::NestedScope { name, .. }
        | Exp::Variable { name, .. }
        | Exp::FuncParam { name, .. }
        | Exp::Alias { name, .. }
        | Exp::Capture { name, .. }
        | Exp::Lambda { name, .. } => *name,
        _ => return None,
    })
}

/// `thisNamedValue` (`Type.hs:991-995`) — `NamedValue (QualifiedIdentifier
/// (ScopedName _ Nothing (Identifier "$this")))`, which is **not** an
/// `Exp::This`: `moveFunctions` spells the receiver as an ordinary name, and
/// `captureThis` is the pass that mints `ThisF`.
fn is_this_value(ir: &kg_hir::ModuleIr, interner: &Interner, id: kg_hir::ExpId) -> bool {
    use kg_hir::Exp;
    let Exp::NamedValue(q) = &ir.arena[id].exp else { return false };
    let Exp::QualifiedIdentifier(sn) = &ir.arena[*q].exp else { return false };
    let Exp::ScopedName { prefix: None, ident, .. } = &ir.arena[*sn].exp else { return false };
    matches!(&ir.arena[*ident].exp, Exp::Identifier(s) if interner.resolve(*s) == "$this")
}

/// The pass under test, over the **post-`abbreviatedFunctionTemplate`** IR and
/// nothing later — the same window the reference's `--template-passes 3`→`4`
/// step isolates.
///
/// One fork for the three sub-passes that set up the input, then
/// [`kg_hir::rewrite::move_functions`] — the pass's own documented door, which
/// forks again. Deliberately *not* `rewrite::pipeline`: that composition is
/// where the passes meet for `kg dump ir --rewrite`, and this line must keep
/// grading the `--template-passes 3`→`4` window unchanged when `pipeline`
/// grows a fifth pass in front of it.
///
/// `None` is the stub while the pass does not exist — what lets the gate be
/// written, falsified and measured against the reference before there is
/// anything on our side to compare.
fn our_move_functions(
    ir: &kg_hir::ModuleIr,
    interner: &Interner,
) -> Option<(kg_hir::ModuleIr, usize, std::collections::BTreeSet<String>)> {
    use kg_hir::rewrite::{desugar_shallow, AbbreviatedFunctionTemplate, Rewriter};
    let mut handed = ir.fork();
    let mut rw = Rewriter::new(&mut handed.arena, interner);
    let root = kg_hir::rewrite::capture_this::apply(&mut rw, ir.root);
    let root = kg_hir::rewrite::lambdas::apply(&mut rw, root);
    handed.root = desugar_shallow(&mut rw, root, &mut AbbreviatedFunctionTemplate::default());
    // The conservation law's left-hand side, read off the tree the pass is
    // *handed* rather than the one it produces.
    let movable = our_movable(&handed, interner);
    let sites = higher_order_sites(&handed, interner);
    Some((kg_hir::rewrite::move_functions(&handed, interner), movable, sites))
}

// ---------------------------------------------------------------------------
// The resolve ledger — the reference's grade for the mangler and `resolve`.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The instance ledger — the reference's grade for `instantiate`.
// ---------------------------------------------------------------------------

/// `kg-oracle --tree` on every file, spread over threads — each run parses
/// the whole import closure, and there are ~124 of them.
fn oracle_trees(
    oracle: &Path,
    lib: &Path,
    base: &Path,
    files: &[(PathBuf, String)],
) -> Vec<Option<Vec<Noted>>> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<Option<Vec<Noted>>>> =
        files.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= files.len() {
                    break;
                }
                let (path, text) = &files[i];
                *out[i].lock().expect("no panics hold this lock") =
                    oracle_names(oracle, lib, base, path, text);
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined")).collect()
}

/// One oracle run: the root module's tree as constructor names, or `None`
/// when the oracle rejects the program.
///
/// `--roots-only` makes the driver print the command-line file and nothing
/// else. Before it existed this function had to *find* the root among one line
/// per parsed file, by matching its `module` header and taking the last hit —
/// because the root is only usually last (a root that `base.k` itself imports
/// completes mid-list) and a device file can share its module name with the
/// copy base's own import pulled in. The driver filters on the canonical path
/// `parseProgram` recorded (`Parser/IO.hs:95`), which is the same identity the
/// reference uses, so the heuristic is gone rather than merely improved.
fn oracle_names(
    oracle: &Path,
    lib: &Path,
    base: &Path,
    path: &Path,
    text: &str,
) -> Option<Vec<Noted>> {
    // `--notes` rather than `--tree`: the note is the *donor*, and nothing in
    // this project grades donors. `check_provenance`'s four clauses all sit
    // inside `if written == Own`, so a `Donated` node's whole obligation is
    // that its range resolves to some CST node — which `Src::donated` makes
    // true by construction. Swapping two donors leaves `xtask ir`
    // byte-identical; this is the comparison that does not.
    let line = oracle_line(oracle, lib, base, path, &["--notes"])?;
    Some(drop_warning_seed_noted(f_constructors_noted(&line, &line_starts(text))))
}

/// The single line `--roots-only` prints for `path`, or `None` when the oracle
/// rejects the program.
fn oracle_line(
    oracle: &Path,
    lib: &Path,
    base: &Path,
    path: &Path,
    mode: &[&str],
) -> Option<String> {
    let mut args = vec![
        "-I".to_owned(),
        lib.to_string_lossy().into_owned(),
        "--base".to_owned(),
        base.to_string_lossy().into_owned(),
    ];
    args.extend(mode.iter().map(|m| (*m).to_owned()));
    args.push("--roots-only".to_owned());
    args.push(path.to_string_lossy().into_owned());
    let text = std::fs::read_to_string(path).ok()?;
    let root = lib.parent().unwrap_or(lib);
    let output = crate::oracle_cache::run(oracle, root, &args, None, &text).ok()?;
    output.stdout.lines().find(|l| !l.trim().is_empty()).map(std::borrow::ToOwned::to_owned)
}

/// One node as the oracle differential compares it: its constructor name and
/// the byte range its `Src` note names, `None` for `SrcUnknown`.
type Noted = (String, Option<(u32, u32)>);

/// Pre-order `(constructor, note range)` from a `--notes` line.
///
/// `emit Notes` renders each node as `show e ++ " @" ++ show n`
/// (`oracle/driver/Main.hs`), so a node's note follows its whole subtree. That
/// makes the note order the tree's **post**-order while the constructor order is
/// its pre-order — a plain zip is wrong. A stack fixes it exactly: children
/// complete before their parent, so the node a note belongs to is always the
/// most recently seen constructor that has not been noted yet.
///
/// The note text is skipped wholesale rather than scanned, because it contains
/// its own capitalised words and the constructor rule here is purely
/// lexical.
fn f_constructors_noted(line: &str, line_starts: &[u32]) -> Vec<Noted> {
    let bytes = line.as_bytes();
    let mut out: Vec<Noted> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && i > 0 && bytes[i - 1] == b' ' {
            let (note, next) = read_note(line, i + 1, line_starts);
            if let Some(idx) = open.pop() {
                out[idx].1 = note;
            }
            i = next;
            continue;
        }
        let c = bytes[i] as char;
        let boundary = i == 0 || {
            let p = bytes[i - 1] as char;
            !(p.is_ascii_alphanumeric() || p == '_')
        };
        if boundary && c.is_ascii_uppercase() {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_ascii_alphabetic() {
                i += 1;
            }
            let word = &line[start..i];
            let after_ok = i >= bytes.len() || {
                let n = bytes[i] as char;
                !(n.is_ascii_alphanumeric() || n == '_')
            };
            if after_ok && word.len() > 1 && word.ends_with('F') {
                open.push(out.len());
                out.push((word[..word.len() - 1].to_owned(), None));
            }
            continue;
        }
        i += 1;
    }
    out
}

/// One `@…` note: its byte range in the source file, and where it ends.
///
/// Three shapes reach here — `Typed {typeof = …, untype = Src {…}}`,
/// `Typed {… untype = SrcUnknown}` and a bare `Src {…}` (the driver is
/// polymorphic in the note, and an unprocessed parse is not `Typed`). `Src`
/// carries `offset` for the start but only a `SourcePos` for the end, so the
/// end is resolved through the file's line table.
fn read_note(line: &str, at: usize, line_starts: &[u32]) -> (Option<(u32, u32)>, usize) {
    let bytes = line.as_bytes();
    let mut i = at;
    while i < bytes.len() && (bytes[i] as char).is_ascii_alphabetic() {
        i += 1;
    }
    let head = &line[at..i];
    if head == "SrcUnknown" {
        return (None, i);
    }
    // Consume the balanced `{…}` that follows, respecting string literals —
    // `sourceName` is a path.
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    let body_start = i;
    let mut depth = 0usize;
    let mut in_str = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    i += 1;
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let body = &line[body_start..i.min(line.len())];
    if body.contains("SrcUnknown") {
        return (None, i);
    }
    (src_range(body, line_starts), i)
}

/// `offset = N, … end = SourcePos {… sourceLine = Pos L, sourceColumn = Pos C}`.
fn src_range(body: &str, line_starts: &[u32]) -> Option<(u32, u32)> {
    let start: u32 = after(body, "offset = ")?.parse().ok()?;
    let end_at = body.find("end = SourcePos")?;
    let tail = &body[end_at..];
    let l: u32 = after(tail, "sourceLine = Pos ")?.parse().ok()?;
    let c: u32 = after(tail, "sourceColumn = Pos ")?.parse().ok()?;
    let base = *line_starts.get(l.checked_sub(1)? as usize)?;
    Some((start, base + c.checked_sub(1)?))
}

fn after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let i = text.find(key)? + key.len();
    let rest = &text[i..];
    let n = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    Some(&rest[..n])
}

/// Byte offset of the start of each line.
fn line_starts(text: &str) -> Vec<u32> {
    let mut out = vec![0u32];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            out.push(i as u32 + 1);
        }
    }
    out
}

/// The reference leads every program with a seed `Seq` carrying any
/// redundant-import warnings (`Parser.hs:234-241`); kg-hir omits it — it
/// diagnoses nothing. The seed is `Warning*` wrapping one `Seq`.
fn drop_warning_seed_noted(names: Vec<Noted>) -> Vec<Noted> {
    if names.len() < 2 || names[0].0 != "Seq" {
        return names;
    }
    let mut i = 1;
    while i < names.len() && names[i].0 == "Warning" {
        i += 1;
    }
    if i < names.len() && names[i].0 == "Seq" {
        let mut out = names;
        out.remove(i);
        return out;
    }
    names
}

/// Pre-order `(constructor, range)` from the **annotated** rendering — the
/// `Detail::Annotated` line format is `… @start..end KIND own|donated`.
fn our_noted(rendered: &str) -> Vec<Noted> {
    let mut out = Vec::new();
    for line in rendered.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("seg:") {
            continue;
        }
        let t = match t.split_once(':') {
            Some((label, rest))
                if !label.is_empty()
                    && label.chars().all(|c| c.is_ascii_lowercase() || c == '_') =>
            {
                rest.trim_start()
            }
            _ => t,
        };
        if t.starts_with(|c: char| c.is_ascii_uppercase()) {
            let name: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
            out.push((name, annotation(t)));
        }
    }
    out
}

/// The `@start..end` of one annotated line.
fn annotation(t: &str) -> Option<(u32, u32)> {
    let at = t.rfind(" @")?;
    let rest = &t[at + 2..];
    let (range, _) = rest.split_once(' ')?;
    let (a, b) = range.split_once("..")?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// The provenance invariant, all three clauses — see the module comment.
fn check_provenance(
    ir: &kg_hir::ModuleIr,
    cst: &mut CstIndex<'_>,
    interner: &Interner,
) -> Result<(), String> {
    // Keyed on `FileId` as well as offsets. Today every head is in the file
    // the IR belongs to — measured: 0 cross-file heads in 473,003 nodes,
    // enforced by `FnInit::Foreign` declining rather than splicing
    // (`kg-hir/src/sort.rs:141-147`) — so this clause is *structurally* right
    // and *numerically* vacuous. `addLambdaWrappers` is the pass that makes it
    // bite: 40 of its 904 wrappers splice a parameter list out of another
    // module's IR, and two modules' offset spaces overlap completely.
    let mut own_claims: BTreeMap<(FileId, u32, u32, SyntaxKind), Vec<kg_hir::ExpId>> =
        BTreeMap::new();

    // Every node the *lowering* built, by key — including the ones a rewrite
    // has since superseded, which is why this half still walks the arena.
    let lowered = ir.lowered_len as usize;
    let mut lowered_by_key: BTreeMap<(FileId, u32, u32, SyntaxKind), Vec<kg_hir::ExpId>> =
        BTreeMap::new();
    for (id, data) in ir.arena.iter() {
        if id.index() >= lowered {
            continue;
        }
        let r = data.src.head.range;
        lowered_by_key
            .entry((data.src.head.file, r.start().into(), r.end().into(), data.kind))
            .or_default()
            .push(id);
    }

    // Walk from `root`, not `arena.iter()`. Under the append-only rewrite
    // substrate (`kg_hir::rewrite`) a replaced node stays in the arena, so an
    // arena walk reads one rewrite as two claimants of one `(range, kind)`.
    // The visited set costs one `Vec<bool>` per file and is not measurable;
    // it is also what stops the walk if the IR ever becomes a DAG.
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children: Vec<kg_hir::ExpId> = Vec::new();
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        let data = &ir.arena[id];
        let head = data.src.head;
        let r = head.range;
        let key = (u32::from(r.start()), u32::from(r.end()));
        let Some(kinds) = cst.kinds(head.file, key) else {
            return Err(format!(
                "no CST node at {:?} {}..{} (IR node lowered as {:?})",
                head.file, key.0, key.1, data.kind
            ));
        };
        let kinds = kinds.to_vec();
        if data.src.written == Written::Own {
            if !kinds.contains(&data.kind) {
                return Err(format!(
                    "own node at {:?} {}..{} claims kind {:?}, CST has {kinds:?}",
                    head.file, key.0, key.1, data.kind
                ));
            }
            // Clause four. "Forbid an `Own` node the lowering did not create"
            // is too strong: in an append-only arena a *replacement* — a node
            // whose children were rewritten — is necessarily a fresh index and
            // may keep `Own`, because what it replaced goes unreachable in the
            // same step. What must be forbidden is a *copy* wearing `Own`:
            // `moveFunctions`' spliced member-name `Identifier` is shape-equal
            // to its donor and sails through clause three below. So an
            // appended `Own` node may not be structurally equal to a lowered
            // node at its own key.
            if id.index() >= lowered {
                let full = (head.file, key.0, key.1, data.kind);
                if let Some(originals) = lowered_by_key.get(&full) {
                    let mine = kg_hir::structure::render_subtree(ir, interner, id);
                    for &other in originals {
                        if kg_hir::structure::render_subtree(ir, interner, other) == mine {
                            return Err(format!(
                                "appended own node #{} {:?} at {}..{} is structurally equal \
                                 to the lowered node #{} at the same key — a rewrite may \
                                 inherit an `Own` claim, never copy one",
                                id.index(),
                                data.kind,
                                key.0,
                                key.1,
                                other.index()
                            ));
                        }
                    }
                }
            }
            own_claims.entry((head.file, key.0, key.1, data.kind)).or_default().push(id);
        }
        data.exp.children(&mut children);
        stack.append(&mut children);
    }

    // Clause three: claimants of one `TypeKey` must be one shape. The lvalue
    // re-read of a compound assignment is the legitimate duplicate
    // (`Parser.hs:631-638`), and its one shape difference is the
    // lvalue/rvalue split the reference itself erases with
    // `rvalue (ArrayAccessLValueF a b) = ArrayAccessF a b` (`Parser.hs:637`),
    // so the comparison erases it the same way.
    for ((_, start, end, kind), claimants) in own_claims {
        if claimants.len() < 2 {
            continue;
        }
        let shapes: Vec<String> = claimants
            .iter()
            .map(|id| {
                kg_hir::structure::render_subtree(ir, interner, *id)
                    .replace("ArrayAccessLValue", "ArrayAccess")
            })
            .collect();
        if shapes.windows(2).any(|w| w[0] != w[1]) {
            return Err(format!(
                "{} structurally different own nodes claim the TypeKey \
                 {start}..{end} {kind:?}",
                claimants.len()
            ));
        }
    }
    Ok(())
}

/// The `(start, end) -> kinds` table of one CST, per [`FileId`], built on first
/// use.
///
/// The gate resolves every `Src::head` against `head.file`, not against the one
/// file it was handed: a rewrite that splices a subtree out of another module's
/// IR carries *that* module's ranges, and checking them against this file's
/// text either fails with a misleading "no CST node at X..Y" or passes by
/// coincidence.
struct CstIndex<'a> {
    open: &'a dyn Fn(FileId) -> Option<kg_syntax::SyntaxNode>,
    tables: std::collections::HashMap<FileId, Option<KindsAt>>,
}

type KindsAt = std::collections::HashMap<(u32, u32), Vec<SyntaxKind>>;

impl<'a> CstIndex<'a> {
    fn new(open: &'a dyn Fn(FileId) -> Option<kg_syntax::SyntaxNode>) -> CstIndex<'a> {
        CstIndex { open, tables: std::collections::HashMap::new() }
    }

    fn kinds(&mut self, file: FileId, range: (u32, u32)) -> Option<&[SyntaxKind]> {
        // The destructure is load-bearing: `self.tables.entry(..)` with
        // `self.open` inside the closure borrows `self` twice.
        let CstIndex { open, tables } = self;
        let table = tables.entry(file).or_insert_with(|| {
            let root = open(file)?;
            let mut kinds_at: KindsAt = std::collections::HashMap::new();
            for node in root.descendants() {
                let r = node.text_range();
                kinds_at.entry((r.start().into(), r.end().into())).or_default().push(node.kind());
            }
            Some(kinds_at)
        });
        table.as_ref()?.get(&range).map(Vec::as_slice)
    }
}

/// The first differing line of the two structural renderings — the diff a
/// failing round trip wants to show.
fn first_diff(a: &str, b: &str) -> String {
    for (i, (la, lb)) in a.lines().zip(b.lines()).enumerate() {
        if la != lb {
            return format!("line {}: `{la}` != `{lb}`", i + 1);
        }
    }
    let (n_a, n_b) = (a.lines().count(), b.lines().count());
    format!("one rendering is a prefix of the other ({n_a} vs {n_b} lines)")
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

/// How many declarations clause 1 rewrote, counted off the output rather than
/// reported by the pass — the number is a measurement, not a claim.
fn fired(before: &kg_hir::ModuleIr, after: &kg_hir::ModuleIr) -> u32 {
    templates(after).saturating_sub(templates(before))
}

/// How many `This` captures `captureThis` put in. Counted on the rewritten IR
/// rather than as a delta, because the lowering never constructs one.
fn this_captures(ir: &kg_hir::ModuleIr) -> u32 {
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children = Vec::new();
    let mut n = 0;
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        if matches!(ir.arena[id].exp, kg_hir::Exp::This { .. }) {
            n += 1;
        }
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
    n
}

/// How many `LambdaFunction`s the tree holds — one per lambda `desugarLambdas`
/// rewrote, since the pass is the only thing in this port that mints the kind.
/// How many declarations wear a mangled `parent@child` name — the count
/// `moveFunctions` alone can produce, since `@` is not a legal identifier
/// character in written source (`Parser.hs`'s `identifier`) and no other
/// ported pass mints one. A self-report, not a comparison: it says the pass
/// fired, not that it fired where the reference does.
fn mangled_declarations(ir: &kg_hir::ModuleIr, interner: &kg_base::Interner) -> u32 {
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children = Vec::new();
    let mut n = 0;
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        let name = match &ir.arena[id].exp {
            kg_hir::Exp::Function { name, .. } | kg_hir::Exp::Class { name, .. } => Some(*name),
            _ => None,
        };
        if let Some(kg_hir::Exp::Identifier(sym)) = name.map(|n| ir.arena[n].exp.clone())
            && interner.resolve(sym).contains('@')
        {
            n += 1;
        }
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
    n
}

fn lambda_functions(ir: &kg_hir::ModuleIr) -> u32 {
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children = Vec::new();
    let mut n = 0;
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        if matches!(ir.arena[id].exp,
                    kg_hir::Exp::Function { kind: kg_ty::FunctionKind::LambdaFunction, .. }) {
            n += 1;
        }
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
    n
}

/// How many `Lambda` nodes the tree holds — the guard on the count above.
fn lambdas(ir: &kg_hir::ModuleIr) -> u32 {
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children = Vec::new();
    let mut n = 0;
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        if matches!(ir.arena[id].exp, kg_hir::Exp::Lambda { .. }) {
            n += 1;
        }
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
    n
}

fn templates(ir: &kg_hir::ModuleIr) -> u32 {
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children = Vec::new();
    let mut n = 0;
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        if matches!(ir.arena[id].exp, kg_hir::Exp::Template { .. }) {
            n += 1;
        }
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
    n
}

/// A rewrite that fires nowhere must change nothing — including ids.
///
/// The append substrate makes this checkable: if the pass allocated at all on a
/// file with no abbreviated declaration, either the guard is too wide or the
/// driver rebuilt a node whose `Exp` it should have found equal.
fn unchanged_outside(
    before: &kg_hir::ModuleIr,
    after: &kg_hir::ModuleIr,
    interner: &Interner,
) -> Result<(), String> {
    if fired(before, after) > 0 {
        // Where it did fire, the structural rendering must still differ only
        // by the declarations it added — checked by the unit tests against the
        // reference's own transcripts, not here.
        return Ok(());
    }
    if before.root != after.root {
        return Err("the rewrite fired nowhere but rebuilt the root".to_owned());
    }
    if after.arena.len() != before.arena.len() {
        return Err(format!(
            "the rewrite fired nowhere but appended {} node(s)",
            after.arena.len() - before.arena.len()
        ));
    }
    let (a, b) = (before.structure_string(interner), after.structure_string(interner));
    if a != b {
        return Err(format!("the rewrite fired nowhere but changed the tree: {}", first_diff(&a, &b)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        LambdaEntry, MoveEntry, StuckEntry, is_mangled, reference_ledger, reference_move_ledger,
    };

    /// A verbatim `kg-oracle --tree --passes 8 --template-passes 2
    /// --template-iterations 0` line for one two-capture lambda:
    ///
    /// ```text
    /// inline uint8 f(uint8 p, uint8 q) {
    ///     auto g = [p, q](uint8 a) -> uint8 { return a + p + q; };
    ///     return g(1);
    /// }
    /// ```
    ///
    /// Verbatim on purpose. Every earlier attempt at this reader was
    /// checked only by a whole-corpus run, and the one that shipped
    /// found zero rewrites on a tree holding 141 because `AliasF {`
    /// is followed by a word byte and the boundary test rejected it.
    /// A hand-written approximation of the reference's `show` output
    /// would have agreed with the broken reader.
    const CAP2: &str = r#"SeqF [FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 8, declName = IdentifierF "f", funcParams = SeqF [FuncParamF {declScopeName = ["f"], paramAttr = SeqF [], paramType = UnsignedF 8, declName = IdentifierF "p"},FuncParamF {declScopeName = ["f"], paramAttr = SeqF [], paramType = UnsignedF 8, declName = IdentifierF "q"}], body = SeqF [AliasF {declScopeName = ["f","$"], body = DecltypeF {declScopeName = ["f","$"], declExp = FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] Nothing IdentifierF "@data@closure") IdentifierF "make_capture2", callArgs = SeqF [NamedValueF QualifiedIdentifierF ScopedNameF ["f","$"] Nothing IdentifierF "p",NamedValueF QualifiedIdentifierF ScopedNameF ["f","$"] Nothing IdentifierF "q"]}}, declName = IdentifierF "?lambda0$capture"},FunctionF {declScopeName = ["f","$"], funcKind = LambdaFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 8, declName = IdentifierF "?lambda0", funcParams = SeqF [FuncParamF {declScopeName = ["f","$","?lambda0"], paramAttr = SeqF [], paramType = TypeIdentifierF ["f","$","?lambda0"] ScopedNameF ["f","$","?lambda0"] (Just ScopedNameF ["f","$","?lambda0"] (Just ScopedNameF ["f","$","?lambda0"] Nothing IdentifierF "f") IdentifierF "$") IdentifierF "?lambda0$capture", declName = IdentifierF "?lambda0$capture_param"},FuncParamF {declScopeName = ["f","$","?lambda0"], paramAttr = SeqF [], paramType = UnsignedF 8, declName = IdentifierF "a"}], body = SeqF [VariableF {declScopeName = ["f","$","?lambda0","$"], varAttr = SeqF [], varType = AutoF, declName = IdentifierF "p", varInit = Just MemberAccessF NamedValueF QualifiedIdentifierF ScopedNameF ["f","$","?lambda0","$"] Nothing IdentifierF "?lambda0$capture_param" IdentifierF "x0", declFlags = DeclareLocal},VariableF {declScopeName = ["f","$","?lambda0","$"], varAttr = SeqF [], varType = AutoF, declName = IdentifierF "q", varInit = Just MemberAccessF NamedValueF QualifiedIdentifierF ScopedNameF ["f","$","?lambda0","$"] Nothing IdentifierF "?lambda0$capture_param" IdentifierF "x1", declFlags = DeclareLocal},ReturnF (Just BinaryF Add BinaryF Add NamedValueF QualifiedIdentifierF ScopedNameF ["f","$","?lambda0","$"] Nothing IdentifierF "a" NamedValueF QualifiedIdentifierF ScopedNameF ["f","$","?lambda0","$"] Nothing IdentifierF "p" NamedValueF QualifiedIdentifierF ScopedNameF ["f","$","?lambda0","$"] Nothing IdentifierF "q")]},VariableF {declScopeName = ["f","$"], varAttr = SeqF [], varType = AutoF, declName = IdentifierF "g", varInit = Just CastF TypeIdentifierF ["f","$"] TemplateInstanceF ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] Nothing IdentifierF "@data@closure@core") IdentifierF "Closure" SeqF [DecltypeF {declScopeName = ["f","$"], declExp = QualifiedIdentifierF ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] Nothing IdentifierF "f") IdentifierF "$") IdentifierF "?lambda0"},QualifiedIdentifierF ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] Nothing IdentifierF "f") IdentifierF "$") IdentifierF "?lambda0",TypeIdentifierF ["f","$"] ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] Nothing IdentifierF "f") IdentifierF "$") IdentifierF "?lambda0$capture"] FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["f","$"] (Just ScopedNameF ["f","$"] Nothing IdentifierF "@data@closure") IdentifierF "make_capture2", callArgs = SeqF [NamedValueF QualifiedIdentifierF ScopedNameF ["f","$"] Nothing IdentifierF "p",NamedValueF QualifiedIdentifierF ScopedNameF ["f","$"] Nothing IdentifierF "q"]}, declFlags = DeclareLocal},ReturnF (Just FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["f","$"] Nothing IdentifierF "g", callArgs = SeqF [IntLiteralF {literalType = Nothing, literalValue = 1}]})]}]"#;

    #[test]
    fn the_ledger_reads_one_two_capture_lambda_off_the_reference() {
        let l = reference_ledger(CAP2);
        assert_eq!(l.wrappers, 0);
        // the `Lambda` is gone — replaced by the `Cast` (Function.hs:161)
        assert_eq!(l.survivors, 0);
        assert_eq!(l.entries, vec![LambdaEntry {
            name: "?lambda0".to_owned(),
            arity: 2,
            params: 1,
            generic: false,
            xs: vec![0, 1],
        }]);
    }

    /// The `make_captureN` call appears twice — once under the alias's
    /// `decltype` and once as the `Cast`'s value (`Function.hs:244`,
    /// `:161`) — so a reader keyed on the call rather than on the alias
    /// double-counts every lambda in the corpus.
    #[test]
    fn the_second_make_capture_call_does_not_mint_a_second_entry() {
        assert_eq!(CAP2.matches("make_capture2").count(), 2);
        assert_eq!(reference_ledger(CAP2).entries.len(), 1);
    }

    /// A verbatim `kg-oracle --tree --roots-only --passes 8 --template-passes 4
    /// --template-iterations 0` line for the reference's own nested double
    /// move:
    ///
    /// ```text
    /// class C {
    /// public:
    ///     inline uint32 Run(uint32 a) {
    ///         inline uint32 Go(uint32 b) {
    ///             inline uint32 Do(uint32 c) { return c + m; }
    ///             return Do(b) + m;
    ///         }
    ///         return Go(a);
    ///     }
    ///     uint32 m;
    /// }
    /// ```
    ///
    /// Verbatim on purpose, for the reason `CAP2` is: every earlier reader in
    /// this file was checked only by a whole-corpus run, and the one that
    /// shipped found zero rewrites on a tree holding 141 because a boundary
    /// test rejected every hit. A hand-written approximation of `show` would
    /// have agreed with the broken reader.
    const NESTED_TP4: &str = r#"SeqF [ClassF {declScopeName = [], declName = IdentifierF "C", body = SeqF [PublicF,FunctionF {declScopeName = ["C"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 32, declName = IdentifierF "Run@Go@Do", funcParams = SeqF [FuncParamF {declScopeName = ["C","Run@Go@Do"], paramAttr = SeqF [], paramType = UnsignedF 32, declName = IdentifierF "c"}], body = SeqF [ReturnF (Just BinaryF Add NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run@Go@Do","$"] (Just ScopedNameF ["C","Run@Go@Do","$"] (Just ScopedNameF ["C","Run@Go@Do","$"] Nothing IdentifierF "C") IdentifierF "Run@Go@Do") IdentifierF "c" NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run@Go@Do","$"] Nothing IdentifierF "m")]},FunctionF {declScopeName = ["C"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 32, declName = IdentifierF "Run@Go", funcParams = SeqF [FuncParamF {declScopeName = ["C","Run@Go"], paramAttr = SeqF [], paramType = UnsignedF 32, declName = IdentifierF "b"}], body = SeqF [ReturnF (Just BinaryF Add FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run@Go","$"] (Just ScopedNameF ["C","Run@Go","$"] Nothing IdentifierF "C") IdentifierF "Run@Go@Do", callArgs = SeqF [NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run@Go","$"] (Just ScopedNameF ["C","Run@Go","$"] (Just ScopedNameF ["C","Run@Go","$"] Nothing IdentifierF "C") IdentifierF "Run@Go") IdentifierF "b"]} NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run@Go","$"] Nothing IdentifierF "m")]},FunctionF {declScopeName = ["C"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 32, declName = IdentifierF "Run", funcParams = SeqF [FuncParamF {declScopeName = ["C","Run"], paramAttr = SeqF [], paramType = UnsignedF 32, declName = IdentifierF "a"}], body = SeqF [ReturnF (Just FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run","$"] (Just ScopedNameF ["C","Run","$"] Nothing IdentifierF "C") IdentifierF "Run@Go", callArgs = SeqF [NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run","$"] Nothing IdentifierF "a"]})]},VariableF {declScopeName = ["C"], varAttr = SeqF [], varType = UnsignedF 32, declName = IdentifierF "m", varInit = Nothing, declFlags = DeclareClass}]},ExportTypeF SeqF [] TypeIdentifierF [] ScopedNameF [] Nothing IdentifierF "C"]"#;

    /// The same file one sub-pass earlier — the tree `moveFunctions` is
    /// *handed*.
    const NESTED_TP3: &str = r#"SeqF [ClassF {declScopeName = [], declName = IdentifierF "C", body = SeqF [PublicF,FunctionF {declScopeName = ["C"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 32, declName = IdentifierF "Run", funcParams = SeqF [FuncParamF {declScopeName = ["C","Run"], paramAttr = SeqF [], paramType = UnsignedF 32, declName = IdentifierF "a"}], body = SeqF [FunctionF {declScopeName = ["C","Run","$"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 32, declName = IdentifierF "Go", funcParams = SeqF [FuncParamF {declScopeName = ["C","Run","$","Go"], paramAttr = SeqF [], paramType = UnsignedF 32, declName = IdentifierF "b"}], body = SeqF [FunctionF {declScopeName = ["C","Run","$","Go","$"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = UnsignedF 32, declName = IdentifierF "Do", funcParams = SeqF [FuncParamF {declScopeName = ["C","Run","$","Go","$","Do"], paramAttr = SeqF [], paramType = UnsignedF 32, declName = IdentifierF "c"}], body = SeqF [ReturnF (Just BinaryF Add NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run","$","Go","$","Do","$"] Nothing IdentifierF "c" NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run","$","Go","$","Do","$"] Nothing IdentifierF "m")]},ReturnF (Just BinaryF Add FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run","$","Go","$"] Nothing IdentifierF "Do", callArgs = SeqF [NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run","$","Go","$"] Nothing IdentifierF "b"]} NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run","$","Go","$"] Nothing IdentifierF "m")]},ReturnF (Just FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run","$"] Nothing IdentifierF "Go", callArgs = SeqF [NamedValueF QualifiedIdentifierF ScopedNameF ["C","Run","$"] Nothing IdentifierF "a"]})]},VariableF {declScopeName = ["C"], varAttr = SeqF [], varType = UnsignedF 32, declName = IdentifierF "m", varInit = Nothing, declFlags = DeclareClass}]},ExportTypeF SeqF [] TypeIdentifierF [] ScopedNameF [] Nothing IdentifierF "C"]"#;

    /// A lambda written in a member function, at `tp=4`: the `ClassF` arm has
    /// hoisted it to module scope and `memberAccess` has rewritten the two
    /// bare reads of the class member `x` to `$this.x`.
    ///
    /// ```text
    /// inline void Go(() -> void Do) {}
    /// class C {
    /// public:
    ///     inline void Run() { Go([](){ x += 2; }); }
    ///     uint32 x;
    /// }
    /// ```
    const CLASS_LAMBDA_TP4: &str = r#"SeqF [TemplateF {templateParams = [TemplateParamF [] TypenameF "Do$capture" 0 Nothing,TemplateParamF [] FunctionTypeF SeqF [] SeqF [FunctionTypeParamF SeqF [] TypeParamF [] TypenameF "Do$capture" 0 Nothing] VoidF "Do" 1 Nothing], templateDecl = FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "Go", funcParams = SeqF [FuncParamF {declScopeName = ["Go"], paramAttr = SeqF [], paramType = TypeIdentifierF ["Go"] TemplateInstanceF ScopedNameF ["Go"] (Just ScopedNameF ["Go"] Nothing IdentifierF "@data@closure@core") IdentifierF "Closure" SeqF [FunctionTypeF SeqF [] SeqF [FunctionTypeParamF SeqF [] TypeParamF [] TypenameF "Do$capture" 0 Nothing] VoidF,TypeParamF [] FunctionTypeF SeqF [] SeqF [FunctionTypeParamF SeqF [] TypeParamF [] TypenameF "Do$capture" 0 Nothing] VoidF "Do" 1,TypeParamF [] TypenameF "Do$capture" 0], declName = IdentifierF "Do"}], body = SeqF []}},FunctionF {declScopeName = [], funcKind = LambdaFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = AutoF, declName = IdentifierF "C@Run@?lambda0", funcParams = SeqF [FuncParamF {declScopeName = ["C@Run@?lambda0"], paramAttr = SeqF [], paramType = TypeIdentifierF ["C@Run@?lambda0"] ScopedNameF ["C@Run@?lambda0"] (Just ScopedNameF ["C@Run@?lambda0"] (Just ScopedNameF ["C@Run@?lambda0"] (Just ScopedNameF ["C@Run@?lambda0"] Nothing IdentifierF "C") IdentifierF "Run") IdentifierF "$") IdentifierF "?lambda0$capture", declName = IdentifierF "?lambda0$capture_param"}], body = SeqF [VariableF {declScopeName = ["C@Run@?lambda0","$"], varAttr = SeqF [], varType = AutoF, declName = IdentifierF "$this", varInit = Just MemberAccessF NamedValueF QualifiedIdentifierF ScopedNameF ["C@Run@?lambda0","$"] (Just ScopedNameF ["C@Run@?lambda0","$"] Nothing IdentifierF "C@Run@?lambda0") IdentifierF "?lambda0$capture_param" IdentifierF "x0", declFlags = DeclareLocal},AssignF MemberAccessF NamedValueF QualifiedIdentifierF ScopedNameF ["C@Run@?lambda0","$"] Nothing IdentifierF "$this" IdentifierF "x" BinaryF Add MemberAccessF NamedValueF QualifiedIdentifierF ScopedNameF ["C@Run@?lambda0","$"] Nothing IdentifierF "$this" IdentifierF "x" IntLiteralF {literalType = Nothing, literalValue = 2}]},ClassF {declScopeName = [], declName = IdentifierF "C", body = SeqF [PublicF,FunctionF {declScopeName = ["C"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "Run", funcParams = SeqF [], body = SeqF [AliasF {declScopeName = ["C","Run","$"], body = DecltypeF {declScopeName = ["C","Run","$"], declExp = FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run","$"] (Just ScopedNameF ["C","Run","$"] Nothing IdentifierF "@data@closure") IdentifierF "make_capture1", callArgs = SeqF [ThisF {declScopeName = ["C","Run","$"]}]}}, declName = IdentifierF "?lambda0$capture"},FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF TemplateInstanceF ScopedNameF ["C","Run","$"] Nothing IdentifierF "Go" SeqF [], callArgs = SeqF [CastF TypeIdentifierF ["C","Run","$"] TemplateInstanceF ScopedNameF ["C","Run","$"] (Just ScopedNameF ["C","Run","$"] Nothing IdentifierF "@data@closure@core") IdentifierF "Closure" SeqF [DecltypeF {declScopeName = ["C","Run","$"], declExp = QualifiedIdentifierF ScopedNameF ["C","Run","$"] Nothing IdentifierF "C@Run@?lambda0"},QualifiedIdentifierF ScopedNameF ["C","Run","$"] Nothing IdentifierF "C@Run@?lambda0",TypeIdentifierF ["C","Run","$"] ScopedNameF ["C","Run","$"] (Just ScopedNameF ["C","Run","$"] (Just ScopedNameF ["C","Run","$"] (Just ScopedNameF ["C","Run","$"] Nothing IdentifierF "C") IdentifierF "Run") IdentifierF "$") IdentifierF "?lambda0$capture"] FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run","$"] (Just ScopedNameF ["C","Run","$"] Nothing IdentifierF "@data@closure") IdentifierF "make_capture1", callArgs = SeqF [ThisF {declScopeName = ["C","Run","$"]}]}]}]},VariableF {declScopeName = ["C"], varAttr = SeqF [], varType = UnsignedF 32, declName = IdentifierF "x", varInit = Nothing, declFlags = DeclareClass}]},ExportTypeF SeqF [] TypeIdentifierF [] ScopedNameF [] Nothing IdentifierF "C"]"#;

    #[test]
    fn the_move_ledger_reads_the_reference_nested_double_move() {
        let l = reference_move_ledger(NESTED_TP4);
        assert_eq!(l.wrappers, 0);
        assert_eq!(l.stuck, vec![]);
        // Mangling composes **left** — `Run@Go@Do`, not `Do@Go@Run` — and both
        // land in the class body, because the `ClassF` arm selects only
        // `lambdaFunctions` (`Function.hs:101`) and neither of these is one.
        // `funcKind` is untouched by the move.
        assert_eq!(l.entries, vec![
            MoveEntry {
                landing: "Class".to_owned(),
                scope: "C".to_owned(),
                name: "Run@Go".to_owned(),
                ctor: "Function".to_owned(),
                kind: "MemberFunction".to_owned(),
                params: 1,
                templated: false,
                this_members: vec![],
                this_calls: 0,
                // `fixScopedName` re-pointed the call to `Do` — the leaf of
                // `C::Run@Go@Do`. **Corrected**, not loosened: this expectation
                // read `["Run@Go", "Run@Go@Do"]` while the field also counted
                // prefix segments, and the prefix `Run@Go` on the parameter `b`
                // is `qualifyName`'s (`Function.hs:72-77`), which fires only on
                // an *unqualified* name and therefore never fires where
                // `kg_db::sorted` has already put a qualifier. See
                // [`MoveEntry::mangled_refs`] for the corpus measurement that
                // forced the narrowing.
                mangled_refs: vec!["Run@Go@Do".to_owned()],
            },
            MoveEntry {
                landing: "Class".to_owned(),
                scope: "C".to_owned(),
                name: "Run@Go@Do".to_owned(),
                ctor: "Function".to_owned(),
                kind: "MemberFunction".to_owned(),
                params: 1,
                templated: false,
                this_members: vec![],
                this_calls: 0,
                // Nothing: its one mangled identifier was its *own* qualifier
                // on the parameter `c`, which is `qualifyName`'s half and no
                // longer read. Corrected from `["Run@Go@Do"]` for the reason
                // above. The innermost declaration of a chain of moves
                // references nothing that moved, so an empty multiset here is
                // the right answer and not a hole.
                mangled_refs: vec![],
            },
        ]);
    }

    /// The reader's own falsifier, and the reason no second whole-corpus
    /// fan-out at `--template-passes 3` is worth 50 seconds: on the tree the
    /// pass is *handed*, every field this ledger reports must be empty. `Go`
    /// and `Do` are the movable population, and `qualifyName` has not run, so
    /// nothing carries a qualifier yet.
    #[test]
    fn the_move_ledger_finds_nothing_one_sub_pass_earlier() {
        let l = reference_move_ledger(NESTED_TP3);
        assert_eq!(l.entries, vec![]);
        assert_eq!(l.wrappers, 0);
        assert_eq!(l.stuck, vec![
            StuckEntry {
                scope: "C::Run::$".to_owned(),
                name: "Go".to_owned(),
                ctor: "Function".to_owned(),
            },
            StuckEntry {
                scope: "C::Run::$::Go::$".to_owned(),
                name: "Do".to_owned(),
                ctor: "Function".to_owned(),
            },
        ]);
    }

    /// The `$this.<member>` half. Two bare reads of `x` in one `x += 2`
    /// (`Parser.hs:631-638` reads the written lvalue twice), so the multiset
    /// carries `x` twice and a set would not.
    #[test]
    fn a_hoisted_class_lambda_carries_its_this_member_rewrites() {
        let l = reference_move_ledger(CLASS_LAMBDA_TP4);
        assert_eq!(l.entries.len(), 1);
        let e = &l.entries[0];
        assert_eq!(e.name, "C@Run@?lambda0");
        // Out of the class entirely — `lambdaFunctions` (`Function.hs:101`),
        // whose whole purpose is that a lambda written in a member function
        // does not become a member.
        assert_eq!(e.landing, "<root>");
        assert_eq!(e.scope, "");
        assert_eq!(e.kind, "LambdaFunction");
        assert_eq!(e.this_members, vec!["x".to_owned(), "x".to_owned()]);
        assert_eq!(e.this_calls, 0);
        // The uncapture prologue reads
        // `C@Run@?lambda0::?lambda0$capture_param` — the mangled name is the
        // *qualifier*, which is `qualifyName`'s half. Corrected from
        // `["C@Run@?lambda0"]`; this lambda references nothing that moved, so
        // the leaf reading is empty. `this_members` above is what carries this
        // declaration's re-qualification evidence.
        assert_eq!(e.mangled_refs, Vec::<String>::new());
        assert_eq!(l.stuck, vec![]);
    }

    /// The substring trap, in the two places it can bite this reader.
    ///
    /// `LambdaF` is a prefix of `LambdaFunction` — the `funcKind` a hoisted
    /// lambda still wears — and `FunctionF` is a prefix of `FunctionCallF`,
    /// `FunctionDeclF`, `FunctionSpecifierF`, `FunctionTypeF` and
    /// `FunctionModifierF`, all of which the tree above holds. The scan reads
    /// whole words and demands a following ` {`, so neither can be mistaken
    /// for a record; this test is what says so on the corpus's own text.
    #[test]
    fn a_constructor_that_is_a_prefix_of_another_is_not_a_record() {
        assert!(CLASS_LAMBDA_TP4.contains("funcKind = LambdaFunction"));
        assert!(CLASS_LAMBDA_TP4.contains("FunctionCallF {"));
        assert!(CLASS_LAMBDA_TP4.contains("FunctionSpecifierF Nothing"));
        assert!(CLASS_LAMBDA_TP4.contains("FunctionModifierF inline"));
        // One `Function` entry and no `Lambda`-kinded ghost: a reader that
        // took `LambdaF` as a substring would find a `Lambda` frame inside
        // every `funcKind = LambdaFunction` and mis-attribute every `$this`
        // below it.
        let l = reference_move_ledger(CLASS_LAMBDA_TP4);
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.entries[0].params, 1);
    }

    /// A verbatim `tp=4` line for the smallest program that reaches the shape
    /// `mangled_refs` shipped wrong: a nested function that passes a bare
    /// function name, so `addLambdaWrappers` synthesizes a wrapper *inside* the
    /// declaration `moveFunctions` then hoists.
    ///
    /// ```text
    /// inline void Go(() -> void Do) { Do(); }
    /// inline void Helper() {}
    /// class C {
    /// public:
    ///     inline void Run() { inline void Inner() { Go(Helper); } Inner(); }
    /// }
    /// export C;
    /// ```
    const WRAPPER_IN_MOVED_BODY_TP4: &str = r#"SeqF [TemplateF {templateParams = [TemplateParamF [] TypenameF "Do$capture" 0 Nothing,TemplateParamF [] FunctionTypeF SeqF [] SeqF [FunctionTypeParamF SeqF [] TypeParamF [] TypenameF "Do$capture" 0 Nothing] VoidF "Do" 1 Nothing], templateDecl = FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "Go", funcParams = SeqF [FuncParamF {declScopeName = ["Go"], paramAttr = SeqF [], paramType = TypeIdentifierF ["Go"] TemplateInstanceF ScopedNameF ["Go"] (Just ScopedNameF ["Go"] Nothing IdentifierF "@data@closure@core") IdentifierF "Closure" SeqF [FunctionTypeF SeqF [] SeqF [FunctionTypeParamF SeqF [] TypeParamF [] TypenameF "Do$capture" 0 Nothing] VoidF,TypeParamF [] FunctionTypeF SeqF [] SeqF [FunctionTypeParamF SeqF [] TypeParamF [] TypenameF "Do$capture" 0 Nothing] VoidF "Do" 1,TypeParamF [] TypenameF "Do$capture" 0], declName = IdentifierF "Do"}], body = SeqF [FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["Go","$"] Nothing IdentifierF "Do", callArgs = SeqF []}]}},FunctionF {declScopeName = [], funcKind = FreeFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "Helper", funcParams = SeqF [], body = SeqF []},FunctionF {declScopeName = [], funcKind = LambdaFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "C@Run@Inner@$wrapper$Helper", funcParams = SeqF [FuncParamF {declScopeName = ["C@Run@Inner@$wrapper$Helper"], paramAttr = SeqF [], paramType = TypeIdentifierF ["C@Run@Inner@$wrapper$Helper"] ScopedNameF ["C@Run@Inner@$wrapper$Helper"] (Just ScopedNameF ["C@Run@Inner@$wrapper$Helper"] (Just ScopedNameF ["C@Run@Inner@$wrapper$Helper"] (Just ScopedNameF ["C@Run@Inner@$wrapper$Helper"] Nothing IdentifierF "C") IdentifierF "Run@Inner") IdentifierF "$") IdentifierF "$wrapper$Helper$capture", declName = IdentifierF "$wrapper$Helper$capture_param"}], body = SeqF [ReturnF (Just FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C@Run@Inner@$wrapper$Helper","$"] Nothing IdentifierF "Helper", callArgs = SeqF []})]},ClassF {declScopeName = [], declName = IdentifierF "C", body = SeqF [PublicF,FunctionF {declScopeName = ["C"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "Run@Inner", funcParams = SeqF [], body = SeqF [AliasF {declScopeName = ["C","Run@Inner","$"], body = DecltypeF {declScopeName = ["C","Run@Inner","$"], declExp = FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run@Inner","$"] (Just ScopedNameF ["C","Run@Inner","$"] Nothing IdentifierF "@data@closure@core") IdentifierF "make_capture0", callArgs = SeqF []}}, declName = IdentifierF "$wrapper$Helper$capture"},FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF TemplateInstanceF ScopedNameF ["C","Run@Inner","$"] Nothing IdentifierF "Go" SeqF [], callArgs = SeqF [CastF TypeIdentifierF ["C","Run@Inner","$"] TemplateInstanceF ScopedNameF ["C","Run@Inner","$"] (Just ScopedNameF ["C","Run@Inner","$"] Nothing IdentifierF "@data@closure@core") IdentifierF "Closure" SeqF [DecltypeF {declScopeName = ["C","Run@Inner","$"], declExp = QualifiedIdentifierF ScopedNameF ["C","Run@Inner","$"] Nothing IdentifierF "C@Run@Inner@$wrapper$Helper"},QualifiedIdentifierF ScopedNameF ["C","Run@Inner","$"] Nothing IdentifierF "C@Run@Inner@$wrapper$Helper",TypeIdentifierF ["C","Run@Inner","$"] ScopedNameF ["C","Run@Inner","$"] (Just ScopedNameF ["C","Run@Inner","$"] (Just ScopedNameF ["C","Run@Inner","$"] (Just ScopedNameF ["C","Run@Inner","$"] Nothing IdentifierF "C") IdentifierF "Run@Inner") IdentifierF "$") IdentifierF "$wrapper$Helper$capture"] FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run@Inner","$"] (Just ScopedNameF ["C","Run@Inner","$"] Nothing IdentifierF "@data@closure@core") IdentifierF "make_capture0", callArgs = SeqF []}]}]},FunctionF {declScopeName = ["C"], funcKind = MemberFunction, funcAttr = SeqF [], funcModifier = Just FunctionModifierF inline, funcReturnType = VoidF, declName = IdentifierF "Run", funcParams = SeqF [], body = SeqF [FunctionCallF {callAttr = SeqF [], callFunction = FunctionSpecifierF Nothing QualifiedIdentifierF ScopedNameF ["C","Run","$"] (Just ScopedNameF ["C","Run","$"] Nothing IdentifierF "C") IdentifierF "Run@Inner", callArgs = SeqF []}]}]},ExportTypeF SeqF [] TypeIdentifierF [] ScopedNameF [] Nothing IdentifierF "C"]"#;

    /// The `$wrapper$` exclusion has to reach the *references*, not only the
    /// entries — and it has to reach them a **chain** at a time.
    ///
    /// `C@Run@Inner@$wrapper$Helper` is `addLambdaWrappers`' declaration, which
    /// this port can never hold, and `Run@Inner`'s body names it three ways:
    /// twice as a bare leaf, and once as the tail of
    /// `C::Run@Inner::$::$wrapper$Helper$capture` — where the segment that is
    /// mangled is `Run@Inner` itself and holds no `$wrapper$` token at all.
    /// Excluding per identifier catches the first two and leaves the third, and
    /// the third is the one that made the corpus read 6 where this port can
    /// only reach 3.
    #[test]
    fn a_reference_to_a_wrapper_is_excluded_the_way_the_wrapper_is() {
        let l = reference_move_ledger(WRAPPER_IN_MOVED_BODY_TP4);
        assert_eq!(l.wrappers, 1, "the wrapper declaration itself");
        assert_eq!(l.entries.len(), 1);
        let e = &l.entries[0];
        assert_eq!(e.name, "Run@Inner");
        assert_eq!(e.landing, "Class");
        assert_eq!(e.scope, "C");
        // Every mangled identifier in this body is on a path to the wrapper.
        assert_eq!(e.mangled_refs, Vec::<String>::new());
        // ... and the text really does hold them, all three:
        assert_eq!(
            WRAPPER_IN_MOVED_BODY_TP4.matches(r#"IdentifierF "C@Run@Inner@$wrapper$Helper""#).count(),
            3,
        );
        assert!(WRAPPER_IN_MOVED_BODY_TP4
            .contains(r#"IdentifierF "Run@Inner") IdentifierF "$") IdentifierF "$wrapper$Helper$capture""#));
    }

    /// The second half of the same needle: `@data@closure@core` and
    /// `@test@internal@assert` hold `@` and are `moduleNamespace` names
    /// (`Parser/Syntax.hs:273-274`), not mangles. The fixture holds four of the
    /// first, in both leaf and prefix position.
    #[test]
    fn a_module_namespace_inside_a_moved_body_is_not_a_reference() {
        assert_eq!(
            WRAPPER_IN_MOVED_BODY_TP4.matches(r#"IdentifierF "@data@closure@core""#).count(),
            4,
        );
        let l = reference_move_ledger(WRAPPER_IN_MOVED_BODY_TP4);
        assert!(l.entries.iter().all(|e| e.mangled_refs.is_empty()));
    }

    /// `moveToParentScope` renames nothing else, so the discriminator has to be
    /// the name — and the name is only a discriminator because the module
    /// separator is a *leading* `@` (`Parser/Syntax.hs:273-274`).
    #[test]
    fn a_module_namespace_is_not_a_mangled_name() {
        assert!(!is_mangled("@data@closure"));
        assert!(!is_mangled("@data@closure@core"));
        assert!(is_mangled("Run@Go"));
        assert!(is_mangled("test_main@$wrapper$TestAdd"));
        assert!(!is_mangled("?lambda0$capture_param"));
    }
}
