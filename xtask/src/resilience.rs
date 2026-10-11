//! `cargo xtask resilience`: the editor's type-reading answers on broken
//! buffers — hover and completion asked at sampled nodes of every
//! error-recovered corpus file and of fuzzed broken buffers, answered as the
//! language server answers them, from the program-wide loop (sprint W3.7:
//! the one engine).
//!
//! # The answers
//!
//! A **hover** is `Db::definition_at`, then `Db::ir_answer_of_definition`
//! — what `kg-lsp`'s hover reads: the declaration's node by the position
//! index (`kg_hir::position`) and the type the loop gave it, or, for a
//! declaration inside a template's body, which the written tree never
//! types, each instance's copy's (`IrAnswer::Copies`). A **completion** is
//! the list `kg-lsp` serves (`Db::completions_with` over the loop): the
//! scopes' names, or after a `.` the members of the loop's type of the
//! base, an instance's copy included; each candidate's detail is
//! `Db::ir_answer_of_completion`. Both are rendered as the editor renders
//! them — the same printer, the same filter (`hover.rs`'s `shown`).
//!
//! Every answer is recorded, `key = answer`, in the `answered` set: a change
//! in what the editor says is a change the ratchet names — an answer lost
//! is a regression, a new one an improvement to read before blessing.
//! Where the editor has no type to show, why is counted (`silent`, a
//! histogram, by declaration kind):
//!
//! * `redefined:` — the declaration, or one it is written inside, is a
//!   redefinition of a name its scope already declares: the reference's
//!   parser rejects the file there (`parseSymbol`, `SymbolTable.hs:
//!   200-201`), so no program holds it, and the loop keeps one declaration
//!   per name;
//! * `edited:` — text an edit made since the loop the editor reads ran,
//!   which no loop has seen; the next loop over a buffer that parses
//!   answers it;
//! * `no type for` / `no node for` — the loop holds the declaration and
//!   has no type to show there (a template, a namespace, an `auto` it
//!   never deduced), or holds no node for it.
//!
//! **Which loop: the editor's.** No buffer here parses, and the editor runs
//! no loop for one per keystroke (sprint W3.4): it reads the last loop
//! computed for the program, its ranges mapped onto the buffer's text
//! (`kg_db::looped::EditorLoop`), and where there is none at all, a hover
//! runs the buffer's own, once (`Db::editor_loop_or_own`). The gate puts
//! each buffer in the state an editor would hold it in and asks through
//! those same queries:
//!
//! * a **fuzzed** buffer is an edit of a clean file, so the clean file is
//!   loaded first and its loop run, as its diagnostics would run it, and
//!   the mutant laid over it after — every question is then answered from
//!   the clean file's loop, mapped, and text the mutation made is in no
//!   loop (`editor-reads-loop` counts these);
//! * an **error-recovered** file never parsed, so nothing is there to map:
//!   its first hover runs its own loop (`editor-runs-own-loop`), which
//!   every later question reads. The hovers are asked first, as a user
//!   would hover before a completion; the completions read what the
//!   hovers left (`Db::editor_loop_ready`, which never runs one).
//!
//! # The sets
//!
//! | set | polarity | holds |
//! |---|---|---|
//! | `answered` | pass | every answer the editor gives, `key = answer` |
//! | `failed` | fail | a buffer whose questions panicked |
//!
//! # Until sprint W3.7: two engines
//!
//! The gate was the measure the CST inference engine's deletion waited on
//! (sprint W3.0; `docs/plans/ir-elaboration.md` §2.6's balance point): each
//! question answered twice, by the loop and by the CST engine the editor
//! fell back on, graded `agree` / `disagree` / `ir-silent` / `ir-only` /
//! `copies` against the CST engine's answers as the reference. Its last
//! two-engine reading, and what the editor gave up with that engine, are
//! in `docs/plans/sprint-parity.md`, W3.7.
//!
//! # What it cannot see
//!
//! * **Whether an answer is right.** There is no second engine to compare
//!   with: `answered` holds what the editor says, and the ratchet holds it
//!   still. Rightness is the loop's, graded on whole programs by every
//!   other gate (`conformance`, `errors`, `output`).
//! * **Name resolution.** `definition_at` and the scope half of a
//!   completion list are recorded with the answer, not graded apart.
//! * **A hover's `Range`**, and signature help and inlay hints, which read
//!   the same types by other routes.
//! * **Whether a mapped answer is stale.** A fuzzed buffer's questions are
//!   answered by the clean file's loop wherever the mutation left a node's
//!   first and last characters alone, and a node that encloses the
//!   mutation is answered as the clean file had it (`kg_db::text_map`).
//! * **Everything not sampled.** Per buffer, at most `--hover` identifiers
//!   and `--completion` carets, spread evenly over the buffer's candidates
//!   in text order (see `sample`), so a run takes seconds, not the minutes
//!   every identifier would.
//!
//! The settings are the keystroke bench's (`bench::rooted_settings`:
//! `mini-base.k`, the `library` and `test/syntax` import dirs, `mock`) and
//! the budget the editor's (`PassBudget::EDITOR`): this grades what an
//! editor would show, not what the compiler would decide. A mutant's base
//! has run its diagnostics to their last answer, the fixpoint's, before
//! the mutation lands — an editor's state once the user paused on the
//! clean text (sprint W7.6).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::Result;
use kg_base::{FileId, TextSize};
use kg_syntax::SyntaxKind;
use kg_ty::Ty;

pub struct Options {
    pub seed: u64,
    pub mutants: usize,
    pub hover: usize,
    pub completion: usize,
    pub filter: Option<String>,
    pub verbose: bool,
    /// Threads, each with its own `Db`; `1` is the one-`Db` walk in
    /// buffer order, the reading to fall back to (`CLAUDE.md`).
    pub jobs: usize,
}

/// One buffer to ask questions of: an error-recovered corpus file as it is
/// on disk, or a mutant laid over its base as an unsaved editor buffer.
struct Buffer {
    label: String,
    path: PathBuf,
    overlay: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Feature {
    Hover,
    Completion,
}

/// One question and the editor's answer.
struct Row {
    feature: Feature,
    key: String,
    /// What the editor shows, as the ledger spells it; `None` where it
    /// shows no type.
    answer: Option<String>,
    /// Why a hover shows no type, as the `silent` histogram counts it.
    why: Option<String>,
    /// Whether the answer is the copies' — a template body's.
    copies: bool,
}

/// What one buffer yielded.
#[derive(Default)]
struct Asked {
    rows: Vec<Row>,
    hover_declined: usize,
    completion_empty: usize,
    completion_member: usize,
    /// Member lists the loop has no type of the base for.
    completion_member_untyped: usize,
    /// Candidates of the graded lists, and those with a detail.
    completion_candidates: usize,
    completion_candidates_typed: usize,
    /// Why a candidate with a declaration shows no detail, as the `silent`
    /// histogram counts it — one entry per candidate.
    candidate_silences: Vec<String>,
    /// Whether the editor reads a loop here before any hover runs one — the
    /// last loop of the clean file the buffer was edited from
    /// (`Db::editor_loop`).
    editor_reads_loop: bool,
    /// Whether a hover ran the buffer's own loop, there being no other
    /// (`Db::editor_loop_or_own`).
    editor_runs_own_loop: bool,
}

pub fn run(kanagawa_root: &Path, opts: &Options) -> Result<()> {
    let root = kanagawa_root.canonicalize()?;
    let started = std::time::Instant::now();
    let mut files = Vec::new();
    for dir in ["library", "test", "doc", "tools"] {
        crate::corpus::collect(&root.join(dir), &mut files);
    }
    files.sort();
    if files.is_empty() {
        anyhow::bail!("no .k files under {} — this gate grades the corpus", root.display());
    }
    let rel = |p: &Path| p.strip_prefix(&root).unwrap_or(p).display().to_string();

    // Error-recovered: the whole-file parse reports an error. Snippet
    // accumulators (`corpus::is_snippet_file`) are most of them, and are
    // exactly what an editor holds mid-edit: a file of fragments.
    let mut recovered: Vec<Buffer> = Vec::new();
    let mut clean: Vec<(PathBuf, String)> = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        if kg_syntax::parse_file(&text).errors().is_empty() {
            clean.push((path.clone(), text));
        } else {
            recovered.push(Buffer { label: rel(path), path: path.clone(), overlay: None });
        }
    }
    let recovered_files = recovered.len();

    // Fuzzed: `xtask fuzz`'s broken-buffer mutators (deleted, duplicated
    // and swapped ranges and tokens, truncation, stray delimiters) over the
    // clean files, mutant `i` a function of `(seed, i)` alone
    // (`difffuzz::mix`). A mutant that still parses is drawn again from the
    // same generator, so every one is broken.
    let pool: Vec<&(PathBuf, String)> = clean.iter().collect();
    let mut mutants = Vec::new();
    for i in 0..opts.mutants {
        let mut rng = crate::fuzz::Rng(crate::difffuzz::mix(opts.seed, i as u64));
        for _attempt in 0..8 {
            let Some((path, text)) = pool.get(rng.below(pool.len())) else { break };
            let broken = crate::fuzz::mutate(text, &mut rng);
            if !kg_syntax::parse_file(&broken).errors().is_empty() {
                mutants.push(Buffer { label: format!("fuzz/{i:03}:{}", rel(path)), path: path.clone(), overlay: Some(broken) });
                break;
            }
        }
    }
    let fuzzed = mutants.len();
    let mut buffers: Vec<Buffer> = recovered.into_iter().chain(mutants).collect();
    if let Some(f) = &opts.filter {
        buffers.retain(|b| b.label.contains(f.as_str()));
    }

    let asked = fan_out(&buffers, opts.jobs.max(1), |state: &mut Option<kg_db::Db>, buffer| {
        let db = state.get_or_insert_with(|| {
            let mut db = kg_db::Db::new(crate::bench::rooted_settings(&root));
            db.set_pass_budget(kg_db::PassBudget::EDITOR);
            db
        });
        let path = camino::Utf8PathBuf::from_path_buf(buffer.path.clone()).ok()?;
        let file = db.file_id(&path);
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(text) = &buffer.overlay {
                // The edit is made to the clean file as an editor holds it:
                // its diagnostics have run its loop to their last answer,
                // which is what the mutant's questions then read.
                db.diagnostics(&[file], file);
                db.set_overlay(file, text.clone());
            }
            ask(db, file, &buffer.label, opts)
        }));
        if buffer.overlay.is_some() {
            db.remove_overlay(file);
        }
        // A panic may leave a memo half-written: the next buffer gets a
        // fresh database rather than inheriting it.
        if out.is_err() {
            *state = None;
        }
        out.ok()
    });

    let mut ledger = crate::ratchet::Ledger::new("resilience");
    for set in ["answered", "failed"] {
        ledger.set(set);
    }
    let mut counts = std::collections::BTreeMap::<(Feature, &str), usize>::new();
    let (mut declined, mut empty, mut member, mut untyped) = (0usize, 0usize, 0usize, 0usize);
    let (mut candidates, mut typed) = (0usize, 0usize);
    let (mut editor_reads_loop, mut editor_runs_own_loop) = (0usize, 0usize);
    let mut report = Vec::new();
    for (buffer, asked) in buffers.iter().zip(&asked) {
        let Some(asked) = asked else {
            ledger.add("failed", &buffer.label);
            continue;
        };
        declined += asked.hover_declined;
        editor_reads_loop += usize::from(asked.editor_reads_loop);
        editor_runs_own_loop += usize::from(asked.editor_runs_own_loop);
        empty += asked.completion_empty;
        member += asked.completion_member;
        untyped += asked.completion_member_untyped;
        candidates += asked.completion_candidates;
        typed += asked.completion_candidates_typed;
        for why in &asked.candidate_silences {
            ledger.histogram("silent", &format!("completion: {why}"));
        }
        for row in &asked.rows {
            *counts.entry((row.feature, "graded")).or_default() += 1;
            match &row.answer {
                Some(answer) => {
                    *counts.entry((row.feature, if row.copies { "copies" } else { "answered" })).or_default() += 1;
                    ledger.add("answered", &format!("{} = {answer}", row.key));
                    if opts.verbose && row.copies {
                        report.push(format!("  copies  {} = {answer}", row.key));
                    }
                }
                None => {
                    *counts.entry((row.feature, "silent")).or_default() += 1;
                    let why = row.why.as_deref().unwrap_or("?");
                    ledger.histogram("silent", &format!("hover: {why}"));
                    if opts.verbose {
                        report.push(format!("  silent  {} — {why}", row.key));
                    }
                }
            }
        }
    }
    let count = |f: Feature, c: &str| counts.get(&(f, c)).copied().unwrap_or(0);
    let graded = count(Feature::Hover, "graded") + count(Feature::Completion, "graded");
    ledger.total("graded", graded);
    ledger.total("buffers", buffers.len());
    ledger.total("recovered-files", recovered_files);
    ledger.total("fuzzed", fuzzed);
    ledger.total("hover-declined", declined);
    for c in ["answered", "copies", "silent"] {
        ledger.total(&format!("hover-{c}"), count(Feature::Hover, c));
    }
    ledger.total("completion-graded", count(Feature::Completion, "graded"));
    ledger.total("completion-copies", count(Feature::Completion, "copies"));
    ledger.total("completion-empty", empty);
    ledger.total("completion-member", member);
    ledger.total("completion-member-untyped", untyped);
    ledger.total("completion-candidates", candidates);
    ledger.total("completion-candidates-typed", typed);
    ledger.total("editor-reads-loop", editor_reads_loop);
    ledger.total("editor-runs-own-loop", editor_runs_own_loop);

    for l in &report {
        println!("{l}");
    }
    println!(
        "resilience: {} buffers ({recovered_files} error-recovered corpus files, {fuzzed} fuzzed, seed {:#x}), {graded} answers graded in {:.1}s; {} failed",
        buffers.len(),
        opts.seed,
        started.elapsed().as_secs_f64(),
        ledger.section.sets["failed"].len()
    );
    println!(
        "  hover      {:>6} graded: answered {}, copies {}, silent {} — {declined} declined (no declaration)",
        count(Feature::Hover, "graded"),
        count(Feature::Hover, "answered"),
        count(Feature::Hover, "copies"),
        count(Feature::Hover, "silent"),
    );
    println!(
        "  completion {:>6} graded ({} with a candidate only the copies type): {typed}/{candidates} candidates with a detail; \
         {empty} empty, {member} member lists graded, {untyped} member lists the loop has no type for",
        count(Feature::Completion, "graded"),
        count(Feature::Completion, "copies"),
    );
    if opts.filter.is_none() {
        crate::ratchet::enforce(ledger)?;
    }
    Ok(())
}

/// Every question sampled from one buffer, answered as the editor answers.
fn ask(db: &kg_db::Db, file: FileId, label: &str, opts: &Options) -> Asked {
    let roots = [file];
    let root = db.parse(file).syntax_node();
    let names = db.interner();
    let editor_reads_loop = db.editor_loop(&roots, file).is_some();
    if !editor_reads_loop {
        // The buffer's own loop answers a hover with the newest answer
        // ready, the first or the last (`Db::looped_first`), as its run
        // and the question race; graded at the last, so the ledger does
        // not race too.
        db.looped(&roots);
    }
    // What a hover reads, which runs the buffer's own loop where there is
    // no other — so asked first; the completions read what it leaves.
    let looped = db.editor_loop_or_own(&roots, file);
    let mut out = Asked { editor_reads_loop, editor_runs_own_loop: !editor_reads_loop, ..Asked::default() };
    let spell = |answer: kg_db::looped::IrAnswer| -> (String, bool) {
        match answer {
            kg_db::looped::IrAnswer::Written(ty) => (kg_ty::subtype::show_pretty(&ty, Some(names)), false),
            // As the editor's detail spells it, every type the instances
            // give (`kg-lsp`'s `instances::label`, unelided), each with
            // the instances that give it.
            kg_db::looped::IrAnswer::Copies { types, .. } => (
                types
                    .iter()
                    .map(|(t, instances)| format!("{} in {}", kg_ty::subtype::show_pretty(t, Some(names)), instances.join(", ")))
                    .collect::<Vec<_>>()
                    .join(" | "),
                true,
            ),
        }
    };

    let idents: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .filter(|t| t.parent().is_some_and(|p| matches!(p.kind(), SyntaxKind::NAME | SyntaxKind::NAME_REF)))
        .collect();
    for token in sample(&idents, opts.hover) {
        let offset = token.text_range().start();
        let key = format!("{label}@{} {:?} {}", u32::from(offset), token.parent().map_or(SyntaxKind::ERROR, |p| p.kind()), token.text());
        let Some(def) = db.definition_at(&roots, file, offset) else {
            out.hover_declined += 1;
            continue;
        };
        let qualified: Vec<String> = def.qualified_name.segments().iter().map(|s| names.resolve(*s).to_string()).collect();
        let row = match db.ir_answer_of_definition(&looped, &def, &printable) {
            Some(answer) => {
                let (ty, copies) = spell(answer);
                let answer = format!("hover {:?} {} : {ty}", def.kind, qualified.join("::"));
                Row { feature: Feature::Hover, key, answer: Some(answer), why: None, copies }
            }
            None => {
                let node = db.ir_type_of_definition(&looped, &def).is_some();
                let why = format!("{} a {:?}", silence(db, &looped, &def, node), def.kind);
                Row { feature: Feature::Hover, key, answer: None, why: Some(why), copies: false }
            }
        };
        out.rows.push(row);
    }

    // What completion reads: the newest loop computed, never one of its own
    // — here the one the hovers read.
    let looped = db.editor_loop_ready(&roots).unwrap_or(looped);

    // Carets a completion is asked at: just past a `.` (a member list) and
    // just past a name being written (the scopes' list).
    let carets: Vec<(TextSize, &'static str)> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter_map(|t| match t.kind() {
            SyntaxKind::DOT => Some((t.text_range().end(), "after-dot")),
            SyntaxKind::IDENT if t.parent().is_some_and(|p| p.kind() == SyntaxKind::NAME_REF) => {
                Some((t.text_range().end(), "after-name"))
            }
            _ => None,
        })
        .collect();
    for &(offset, what) in sample(&carets, opts.completion) {
        let key = format!("{label}@{} completion {what}", u32::from(offset));
        let list = db.completions_with(&roots, file, offset, Some(&looped));
        let member = db.is_member_completion(file, offset);
        if list.is_empty() {
            out.completion_empty += 1;
            if member && db.loop_members_at(file, offset, &looped).is_none() {
                out.completion_member_untyped += 1;
            }
            continue;
        }
        if member {
            out.completion_member += 1;
        }
        let (mut spelled, mut copies) = (Vec::with_capacity(list.len()), false);
        for c in &list {
            out.completion_candidates += 1;
            let detail = match db.ir_answer_of_completion(&roots, &looped, c, &printable) {
                Some(answer) => {
                    let (ty, from_copies) = spell(answer);
                    copies |= from_copies;
                    out.completion_candidates_typed += 1;
                    ty
                }
                None => {
                    // No declaration: nothing to show, and nothing to say
                    // why. Otherwise the reason, by the candidate's kind.
                    if let Some(def) = db.completion_definition(&roots, c) {
                        let node = c.ty.is_some() || db.ir_type_of_definition(&looped, &def).is_some();
                        out.candidate_silences.push(format!("{} a {:?}", silence(db, &looped, &def, node), def.kind));
                    }
                    String::new()
                }
            };
            spelled.push(format!("{}:{:?}:{detail}", c.name, c.kind));
        }
        let answer = format!("completion {}", digest(&spelled));
        out.rows.push(Row { feature: Feature::Completion, key, answer: Some(answer), why: None, copies });
    }
    out
}

/// Why the loop has no answer at the declaration `def` names, as the
/// `silent` histogram counts it:
///
/// * `redefined:` — the declaration, or one it is written inside, is a
///   redefinition of a name its scope already declares: the reference's
///   parser rejects the file there (`parseSymbol`, `SymbolTable.hs:
///   200-201`), so no program holds it, and the loop keeps one declaration
///   per name (`deduplicateInstances`' `nubBy`, `Template.hs:144-156`). A
///   snippet accumulator redeclares `main` once a snippet;
/// * `edited:` — text an edit made since the loop the editor reads ran,
///   which no loop has seen ([`kg_db::looped::EditorLoop::to_loop`]); the
///   next loop over a buffer that parses answers it;
/// * `no type for` / `no node for` — anything else: the loop holds the
///   declaration and has no type to show, or holds no node for it.
fn silence(db: &kg_db::Db, looped: &kg_db::looped::EditorLoop, def: &kg_db::Definition, node: bool) -> &'static str {
    let module = def.def.module;
    let root = db.parse_module(module).syntax_node();
    let declaration = root
        .token_at_offset(def.range.range.start())
        .find(|t| t.text_range() == def.range.range)
        .and_then(|t| t.parent())
        .and_then(|n| n.parent());
    silence_at(db, looped, module, declaration.as_ref(), node)
}

/// [`silence`] of the declaration node itself.
fn silence_at(
    db: &kg_db::Db,
    looped: &kg_db::looped::EditorLoop,
    module: kg_db::ModuleSource,
    declaration: Option<&kg_syntax::SyntaxNode>,
    node: bool,
) -> &'static str {
    if let Some(declaration) = declaration {
        let redefined: Vec<kg_base::TextSize> = db
            .item_tree(module)
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("symbol redefinition"))
            .map(|d| d.range.range.start())
            .collect();
        if declaration.ancestors().any(|a| redefined.contains(&kg_syntax::trimmed_range(&a).start())) {
            return "redefined:";
        }
        if looped.to_loop(module, declaration.text_range()).is_none() {
            return "edited:";
        }
        // An edit that reshaped the declaration itself — its initializer
        // deleted, a function's parentheses, a token inside it duplicated —
        // leaves its first and last characters to map, onto a range the
        // loop's text holds another declaration at: one starting where this
        // does, ending elsewhere or of another kind (a function the edit
        // made a variable).
        let declares = |k: SyntaxKind| {
            matches!(
                k,
                SyntaxKind::VAR_DECL
                    | SyntaxKind::FN_DECL
                    | SyntaxKind::CLASS_DECL
                    | SyntaxKind::STRUCT_DECL
                    | SyntaxKind::UNION_DECL
                    | SyntaxKind::ENUM_DECL
                    | SyntaxKind::ALIAS_DECL
                    | SyntaxKind::PARAM
            )
        };
        // Or an edit inside it: the text the range maps onto is not the
        // declaration's text now.
        if let Some(mapped) = looped.to_loop(module, declaration.text_range())
            && let Some(then) = looped.looped().parse(module)
            && !std::sync::Arc::ptr_eq(then, &db.parse_module(module))
            && (then.syntax_node().descendants().any(|n| {
                declares(n.kind())
                    && n.text_range().start() == mapped.start()
                    && (n.text_range() != mapped || n.kind() != declaration.kind())
            }) || then.text().get(usize::from(mapped.start())..usize::from(mapped.end())) != Some(declaration.text().to_string().as_str()))
        {
            return "edited:";
        }
    }
    if node { "no type for" } else { "no node for" }
}

/// A list's length and content hash — what the ledger records of a
/// completion, whose lists run to hundreds of names.
fn digest(items: &[String]) -> String {
    let mut h = crate::oracle_cache::Fnv::new();
    for i in items {
        h.write_str(i);
        h.write(&[0]);
    }
    format!("{} #{}", items.len(), h.finish())
}

/// Whether a tooltip prints `ty`, or printing it would overstate what the
/// frontend knows — `kg-lsp`'s `hover::shown` and the completion detail's
/// filter, which are the same test: `tResolved` (`Type.hs:1001-1015`) less
/// `TError`, `Untyped` and an undeduced `Auto`.
fn printable(ty: &Ty) -> bool {
    !matches!(ty, Ty::Untyped | Ty::Auto) && ty.resolved() && !ty.is_error()
}

/// At most `n` of `xs`, spread evenly in their order: the `k`th of `n` is
/// `xs[k * len / n]`. A function of the buffer's text alone, so a node keeps
/// its key from run to run and an edit elsewhere in the compiler moves no
/// sample.
fn sample<T>(xs: &[T], n: usize) -> Vec<&T> {
    if xs.len() <= n {
        return xs.iter().collect();
    }
    (0..n).map(|k| &xs[k * xs.len() / n]).collect()
}

/// `f` over every buffer on up to `workers` threads, each with its own
/// state (a `Db`, `CLAUDE.md`), results in buffer order.
fn fan_out<S, T: Send>(
    work: &[Buffer],
    workers: usize,
    f: impl Fn(&mut Option<S>, &Buffer) -> Option<T> + Sync,
) -> Vec<Option<T>> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(workers);
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<T>>> = work.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut state: Option<S> = None;
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(b) = work.get(i) else { break };
                    *slots[i].lock().expect("no panics hold this lock") = f(&mut state, b);
                }
            });
        }
    });
    slots.into_iter().map(|s| s.into_inner().expect("threads joined")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_is_spread_evenly_and_keeps_order() {
        let xs: Vec<u32> = (0..10).collect();
        assert_eq!(sample(&xs, 4), [&0, &2, &5, &7]);
        assert_eq!(sample(&xs, 20).len(), 10);
        assert!(sample(&xs, 0).is_empty());
    }
}
