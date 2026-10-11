//! `cargo xtask fuzz --differential` — mutate the corpus's snippets the way a
//! programmer edits code, and grade each mutant against the reference.
//!
//! `fuzz` proves the frontend total over broken text and `errors` grades its
//! diagnostics over the snippets the corpus already has. Neither can see a
//! correct program the corpus does not contain: docs/history/audit-2026-09-22.md D1 — `g(x < y, z >
//! -1)` over parameters rejected with "Invalid number of arguments" — sat in
//! no corpus file, and every gate was green over it. This walks off the
//! corpus one small, *grammatical* edit at a time, so most mutants are still
//! programs the reference has an opinion about, and compares the two
//! compilers' opinions with the [`differential`](crate::differential) probe.
//!
//! # The mutators
//!
//! Each targets a place where a port's reading can part from the
//! reference's without any corpus file noticing:
//!
//! | mutator | edit | reaches |
//! |---|---|---|
//! | `swap-op` | one binary operator for another | precedence, `<`/`>` against template brackets, operand typing |
//! | `neg` | a unary `-`, `!` or `~` before an operand | `> -1`: what may follow a closing `>` |
//! | `paren` | parentheses around an operand | `> (y)`: the same, and paren-sensitive folds |
//! | `angle-pair` | `a < b` in one argument and `c > d` in the next, the right side optionally negated or parenthesized | the D1 shape itself: a balanced `<…>` scan across a comma |
//! | `swap-items` | two top-level declarations exchanged | declaration order, forward references |
//! | `swap-stmts` | two adjacent statements exchanged | use before declaration in a block |
//! | `rename` | every occurrence of one declared name spelled as another name in the file | collisions, shadowing, overloads |
//! | `dup-stmt` / `del-stmt` | a statement or member duplicated or deleted | redefinition, missing returns, unused names |
//! | `literal` | an integer literal replaced by a boundary value | widths, folds, array bounds |
//!
//! A mutant is one or two of these, drawn from a generator seeded by
//! `(seed, index)` alone, so mutant `i` of seed `s` is the same program
//! however many threads ran and however long the run was allowed.
//!
//! # What is reported
//!
//! Every disagreement is bucketed by its [`Signature`] with names and numbers
//! blanked, and the bucket's smallest mutant is written under
//! `target/difffuzz/<NN>-<kind>/` with the snippet it came from, the seed and
//! index, the edits, and a command line that reduces it. A mutant whose
//! unmutated snippet already disagrees the same way is **inherited** — the
//! `errors` gate owns it — and counted, not listed. `--reduce` runs the
//! [`reduce`](crate::reduce) reducer over each bucket's smallest mutant.
//!
//! # What it cannot see
//!
//! * Only syntax-suite snippets, under their own ctest flags; whole corpus
//!   programs (`test/library/…`) are not mutated, and snippets whose flags
//!   the oracle driver cannot take (`--define`, `--using`, `--target-device`,
//!   `--Werror`) are never bases.
//! * Only the first error per side, as in `errors`.
//! * The oracle is the reference's frontend, not its backend: a check the
//!   reference makes only in code generation reads as a false alarm unless
//!   the unmutated snippet already shows it (then it is inherited).
//! * Mutations are token- and tree-local. Nothing here writes a new
//!   declaration, a new type or a new import.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use kg_syntax::SyntaxKind::{self, *};
use kg_syntax::SyntaxNode;

use crate::conformance::{Assertion, load_all};
use crate::differential::{Context, Kind, Probe, Signature, Worker, jobs, probe_all, workspace_target};
use crate::errors::oracle_args;
use crate::fuzz::Rng;

pub struct Options {
    pub iterations: usize,
    pub seed: u64,
    pub jobs: Option<usize>,
    pub seconds: Option<u64>,
    /// Reduce buckets, worst first, until this many seconds have gone.
    pub reduce: Option<u64>,
    pub max_tests: usize,
    pub verbose: bool,
}

/// One mutant: the snippet it came from and the edits that made it.
struct Mutant {
    base: usize,
    text: String,
    ops: Vec<String>,
}

pub fn run(kanagawa_root: &Path, opts: &Options) -> Result<()> {
    let cx = Context::new(kanagawa_root, "xtask-difffuzz")?;
    let mut pool: Vec<Assertion> = load_all(&cx.root)?;
    pool.retain(|a| oracle_args(&cx.root, &a.options).is_ok());
    if pool.is_empty() {
        anyhow::bail!("no snippet the oracle can run");
    }

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = fuzz(&cx, &pool, opts);
    std::panic::set_hook(hook);
    let _ = std::fs::remove_dir_all(&cx.scratch);
    outcome
}

fn fuzz(cx: &Context, pool: &[Assertion], opts: &Options) -> Result<()> {
    let start = Instant::now();
    let mutants: Vec<Mutant> = (0..opts.iterations)
        .map(|i| {
            let mut rng = Rng(mix(opts.seed, i as u64));
            let base = rng.below(pool.len());
            let (text, ops) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mutate(&pool[base].source, &mut rng)))
                .unwrap_or_else(|_| (pool[base].source.clone(), Vec::new()));
            Mutant { base, text, ops }
        })
        .collect();

    // Grouped by flags so each thread's `Db` is re-configured at a group
    // boundary, not per mutant; the results land in index order regardless.
    let mut order: Vec<usize> = (0..mutants.len()).filter(|&i| !mutants[i].ops.is_empty()).collect();
    order.sort_by(|&a, &b| pool[mutants[a].base].options.cmp(&pool[mutants[b].base].options).then(a.cmp(&b)));
    let items: Vec<(&Assertion, &str)> = mutants.iter().map(|m| (&pool[m.base], m.text.as_str())).collect();
    let mut workers: Vec<Worker> = (0..jobs(opts.jobs)).map(|_| Worker::new()).collect();
    let deadline = opts.seconds.map(|s| start + Duration::from_secs(s));
    let probes = probe_all(cx, &mut workers, &items, &order, deadline);
    let probed = probes.iter().filter(|p| p.is_some()).count();

    // Every base a finding came from, graded unmutated.
    let mut bases: Vec<usize> = (0..mutants.len())
        .filter(|&i| probes[i].as_ref().is_some_and(|p| p.kind().is_finding()))
        .map(|i| mutants[i].base)
        .collect();
    bases.sort_unstable();
    bases.dedup();
    let base_items: Vec<(&Assertion, &str)> = bases.iter().map(|&b| (&pool[b], pool[b].source.as_str())).collect();
    let base_order: Vec<usize> = (0..base_items.len()).collect();
    let base_probes = probe_all(cx, &mut workers, &base_items, &base_order, None);
    let base_signature = |b: usize| -> Option<Signature> {
        let k = bases.binary_search(&b).ok()?;
        base_probes[k].as_ref().map(|p| bucket(&p.signature()))
    };

    let mut counts: BTreeMap<Kind, usize> = BTreeMap::new();
    let mut failed: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut inherited = 0usize;
    let mut buckets: BTreeMap<Signature, Vec<usize>> = BTreeMap::new();
    for (i, probe) in probes.iter().enumerate() {
        let Some(probe) = probe else { continue };
        let kind = probe.kind();
        *counts.entry(kind).or_default() += 1;
        if kind == Kind::OracleFailed {
            failed.entry(probe.render_theirs()).or_insert((0, i)).0 += 1;
        }
        if !kind.is_finding() {
            continue;
        }
        let sig = bucket(&probe.signature());
        if base_signature(mutants[i].base).as_ref() == Some(&sig) {
            inherited += 1;
            continue;
        }
        buckets.entry(sig).or_default().push(i);
    }
    let mut buckets: Vec<(Signature, Vec<usize>)> = buckets.into_iter().collect();
    for (_, members) in &mut buckets {
        members.sort_by_key(|&i| (mutants[i].text.len(), i));
    }
    buckets.sort_by(|a, b| a.0.kind.cmp(&b.0.kind).then(b.1.len().cmp(&a.1.len())).then(a.0.cmp(&b.0)));

    let out_dir = workspace_target().join("difffuzz");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir)?;

    let mut report = String::new();
    let _ = writeln!(
        report,
        "differential fuzz: {} mutants (seed {:#x}) of {} snippets, {probed} probed in {:.1} s over {} threads",
        mutants.len(),
        opts.seed,
        pool.len(),
        start.elapsed().as_secs_f64(),
        workers.len()
    );
    if probed < order.len() {
        let _ = writeln!(report, "  {} not probed: the --seconds budget ran out", order.len() - probed);
    }
    let unchanged = mutants.len() - order.len();
    if unchanged > 0 {
        let _ = writeln!(report, "  {unchanged} mutants left their snippet unchanged and were not probed");
    }
    for (kind, n) in &counts {
        let _ = writeln!(report, "  {:<16}{n:>6}", kind.label());
    }
    for (why, (n, first)) in &failed {
        let _ = writeln!(report, "      {n:>5}  {why} (first: mutant #{first})");
    }
    let found: usize = buckets.iter().map(|(_, m)| m.len()).sum();
    let _ = writeln!(
        report,
        "  {found} disagreements in {} buckets; {inherited} more inherited from a snippet that already disagrees the same way",
        buckets.len()
    );
    let _ = writeln!(report, "  {}", crate::oracle_cache::report());

    let reduce_start = Instant::now();
    for (n, (sig, members)) in buckets.iter().enumerate() {
        let i = members[0];
        let m = &mutants[i];
        let like = &pool[m.base];
        let probe = probes[i].as_ref().expect("a bucket member was probed");
        let dir = out_dir.join(format!("{:02}-{}", n + 1, sig.kind.label()));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("mutant.k"), &m.text)?;
        std::fs::write(dir.join("base.k"), &like.source)?;
        let rel = |p: &Path| p.strip_prefix(workspace_target().parent().unwrap()).unwrap_or(p).display().to_string();
        let mut entry = String::new();
        let _ = writeln!(entry, "\n[{:02}] {} ×{}", n + 1, sig.kind.label(), members.len());
        let _ = writeln!(entry, "  ours:   {}", probe.render_ours());
        let _ = writeln!(entry, "  theirs: {}", probe.render_theirs());
        if ours_precedes_theirs(probe) {
            let _ = writeln!(entry, "  ours comes before every error the reference reports");
        }
        let _ = writeln!(entry, "  base {} ({}:{}), seed {:#x} mutant #{i}: {}", like.name, like.file.display(), like.marker_line, opts.seed, m.ops.join("; "));
        let _ = writeln!(entry, "  smallest: {} ({} bytes)", rel(&dir.join("mutant.k")), m.text.len());
        let _ = writeln!(entry, "  reduce:   cargo xtask reduce {} --like {}", rel(&dir.join("mutant.k")), like.name);
        if let Some(budget) = opts.reduce
            && reduce_start.elapsed() < Duration::from_secs(budget)
        {
            let want = probe.signature();
            let deadline = reduce_start + Duration::from_secs(budget);
            let keep = crate::reduce::Keep::Errors;
            let reduced = crate::reduce::reduce(cx, &mut workers, like, &m.text, keep, &want, opts.max_tests, Some(deadline));
            std::fs::write(dir.join("reduced.k"), &reduced.text)?;
            let _ = writeln!(entry, "  reduced:  {} ({} bytes, {} probes)", rel(&dir.join("reduced.k")), reduced.text.len(), reduced.tests);
            let _ = writeln!(entry, "    ours:   {}", reduced.probe.render_ours());
            let _ = writeln!(entry, "    theirs: {}", reduced.probe.render_theirs());
            if reduced.text.len() <= 600 {
                for line in reduced.text.trim_end().lines() {
                    let _ = writeln!(entry, "    | {line}");
                }
            }
        }
        if opts.verbose {
            for &j in &members[1..] {
                let p = probes[j].as_ref().expect("probed");
                let _ = writeln!(entry, "    also #{j} ({}): {} | {}", pool[mutants[j].base].name, p.render_ours(), p.render_theirs());
            }
        }
        std::fs::write(dir.join("info.txt"), entry.trim_start())?;
        report.push_str(&entry);
    }
    if opts.reduce.is_some() {
        let _ = writeln!(report, "\nreductions took {:.1} s", reduce_start.elapsed().as_secs_f64());
    }
    std::fs::write(out_dir.join("report.txt"), &report)?;
    print!("{report}");
    println!("\nwritten to {}", out_dir.display());

    let alarms: usize = buckets
        .iter()
        .filter(|(s, _)| matches!(s.kind, Kind::FalseAlarm | Kind::OursCrashed | Kind::OursHung))
        .map(|(_, m)| m.len())
        .sum();
    if alarms > 0 {
        anyhow::bail!("{alarms} mutants where we reject what the reference accepts, crash, or hang");
    }
    Ok(())
}

/// A signature with the program's own names and numbers blanked, for
/// bucketing: `Symbol name not in scope: \`x\`` and the same for `y` are
/// one bug.
fn bucket(sig: &Signature) -> Signature {
    Signature { kind: sig.kind, ours: template(&sig.ours), theirs: template(&sig.theirs) }
}

/// Backticked and quoted spans become `_`, digit runs `N`.
fn template(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' | '\'' | '"' => {
                let rest: String = chars.clone().collect();
                if let Some(end) = rest.find(c) {
                    out.push(c);
                    out.push('_');
                    out.push(c);
                    for _ in 0..=rest[..end].chars().count() {
                        chars.next();
                    }
                } else {
                    out.push(c);
                }
            }
            '0'..='9' => {
                out.push('N');
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    chars.next();
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Both reject, and our first error comes before every one the reference
/// reports in the same file: we diagnose earlier than the reference does,
/// which is the both-reject form of a false alarm.
fn ours_precedes_theirs(probe: &Probe) -> bool {
    let (Some(m), crate::errors::OracleSide::Parse(theirs) | crate::errors::OracleSide::Frontend(theirs)) =
        (probe.ours_error(), &probe.theirs)
    else {
        return false;
    };
    !theirs.is_empty() && theirs.iter().filter(|r| r.path == m.path).all(|r| (m.line, m.col) < (r.line, r.col))
}

/// SplitMix64 over the seed and the index, so each mutant's generator is
/// independent of every other's and of the order they are made in.
pub(crate) fn mix(seed: u64, i: u64) -> u64 {
    let mut z = seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One text replacement; a mutation is a set of non-overlapping ones.
struct Edit {
    at: Range<usize>,
    with: String,
}

fn apply(text: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse((e.at.start, e.at.end)));
    let mut out = text.to_owned();
    for e in edits {
        out.replace_range(e.at, &e.with);
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Mutator {
    SwapOp,
    Neg,
    Paren,
    AnglePair,
    SwapItems,
    SwapStmts,
    Rename,
    DupStmt,
    DelStmt,
    Literal,
}

/// Weighted toward the expression edits: they are the cheapest way to a
/// program the reference still accepts, and the class D1 lived in.
const TABLE: [(Mutator, usize); 10] = [
    (Mutator::SwapOp, 3),
    (Mutator::Neg, 2),
    (Mutator::Paren, 2),
    (Mutator::AnglePair, 2),
    (Mutator::SwapItems, 1),
    (Mutator::SwapStmts, 1),
    (Mutator::Rename, 2),
    (Mutator::DupStmt, 1),
    (Mutator::DelStmt, 1),
    (Mutator::Literal, 1),
];

/// One or two edits of `base`, and what they were; no edits when nothing in
/// the snippet offered a site.
fn mutate(base: &str, rng: &mut Rng) -> (String, Vec<String>) {
    let mut text = base.to_owned();
    let mut ops = Vec::new();
    for _ in 0..1 + rng.below(2) {
        for _attempt in 0..8 {
            let total: usize = TABLE.iter().map(|(_, w)| w).sum();
            let mut pick = rng.below(total);
            let mutator = TABLE.iter().find(|(_, w)| if pick < *w { true } else { pick -= w; false }).expect("in range").0;
            if let Some((edits, what)) = mutate_once(&text, mutator, rng) {
                let at = edits.iter().map(|e| e.at.start).min().unwrap_or(0);
                ops.push(format!("{what} @{}", line_col(&text, at)));
                text = apply(&text, edits);
                break;
            }
        }
    }
    (text, ops)
}

fn line_col(text: &str, at: usize) -> String {
    let before = &text[..at];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit_once('\n').map_or(before, |(_, l)| l).chars().count() + 1;
    format!("{line}:{col}")
}

const BINOPS: [&str; 18] = ["+", "-", "*", "/", "%", "&", "|", "^", "&&", "||", "<", ">", "<=", ">=", "==", "!=", "<<", ">>"];

fn is_expr(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        LITERAL
            | INTERPOLATED_STRING
            | PATH
            | BIN_EXPR
            | PREFIX_EXPR
            | TERNARY_EXPR
            | PAREN_EXPR
            | CALL_EXPR
            | INDEX_EXPR
            | FIELD_EXPR
            | CAST_EXPR
            | CONCAT_EXPR
            | MUX_EXPR
            | STATIC_EXPR
            | INIT_LIST_EXPR
    )
}

fn is_stmt(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        EXPR_STMT
            | ASSIGN_STMT
            | DECL_STMT
            | IF_STMT
            | STATIC_IF_STMT
            | FOR_STMT
            | UNROLLED_FOR_STMT
            | STATIC_FOR_STMT
            | WHILE_STMT
            | DO_WHILE_STMT
            | SWITCH_STMT
            | RETURN_STMT
            | BARRIER_STMT
            | ATOMIC_STMT
            | ANNOTATED_STMT
            | BLOCK_STMT
    )
}

fn span(n: &SyntaxNode) -> Range<usize> {
    let r = kg_syntax::trimmed_range(n);
    usize::from(r.start())..usize::from(r.end())
}

/// An expression in operand position: under a binary operator, an argument
/// list, a ternary, parentheses, an index, or a `return`.
fn operands(root: &SyntaxNode) -> Vec<SyntaxNode> {
    root.descendants()
        .filter(|n| {
            is_expr(n.kind())
                && n.parent().is_some_and(|p| {
                    matches!(p.kind(), BIN_EXPR | ARG_LIST | TERNARY_EXPR | PAREN_EXPR | INDEX_EXPR | RETURN_STMT)
                })
        })
        .collect()
}

/// A binary expression's operator: its own non-trivia tokens, first to last,
/// so a `>>` the lexer split in two is still one operator.
fn operator(bin: &SyntaxNode) -> Option<Range<usize>> {
    let toks: Vec<_> = bin
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    let (first, last) = (toks.first()?, toks.last()?);
    Some(usize::from(first.text_range().start())..usize::from(last.text_range().end()))
}

fn name_refs(root: &SyntaxNode) -> Vec<String> {
    let mut out: Vec<String> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == IDENT && t.parent().is_some_and(|p| matches!(p.kind(), NAME_REF | NAME)))
        .map(|t| t.text().to_owned())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn pick<'a, T>(rng: &mut Rng, xs: &'a [T]) -> Option<&'a T> {
    (!xs.is_empty()).then(|| &xs[rng.below(xs.len())])
}

fn mutate_once(text: &str, mutator: Mutator, rng: &mut Rng) -> Option<(Vec<Edit>, String)> {
    let root = kg_syntax::parse_file(text).syntax_node();
    match mutator {
        Mutator::SwapOp => {
            let bins: Vec<SyntaxNode> = root.descendants().filter(|n| n.kind() == BIN_EXPR).collect();
            let op = operator(pick(rng, &bins)?)?;
            let old = &text[op.clone()];
            let new = BINOPS[rng.below(BINOPS.len())];
            (new != old).then(|| (vec![Edit { at: op, with: new.to_owned() }], format!("swap-op `{old}`→`{new}`")))
        }
        Mutator::Neg => {
            let at = span(pick(rng, &operands(&root))?).start;
            let sign = ["-", "-", "-", "!", "~"][rng.below(5)];
            Some((vec![Edit { at: at..at, with: sign.to_owned() }], format!("neg `{sign}`")))
        }
        Mutator::Paren => {
            let r = span(pick(rng, &operands(&root))?);
            Some((vec![Edit { at: r.start..r.start, with: "(".into() }, Edit { at: r.end..r.end, with: ")".into() }], "paren".into()))
        }
        Mutator::AnglePair => {
            // `f(a < b, c > d)`: the two halves of a template bracket in two
            // arguments, which is what a `<…>` scan balances across a comma.
            let lists: Vec<Vec<SyntaxNode>> = root
                .descendants()
                .filter(|n| n.kind() == ARG_LIST)
                .map(|n| n.children().filter(|c| is_expr(c.kind())).collect::<Vec<_>>())
                .filter(|args| args.len() >= 2)
                .collect();
            let args = pick(rng, &lists)?;
            let i = rng.below(args.len() - 1);
            let names = name_refs(&root);
            let mut other = || pick(rng, &names).cloned().unwrap_or_else(|| "1".to_owned());
            let (left, right) = (other(), other());
            let tail = rng.below(3);
            let mut edits = Vec::new();
            let halve = |arg: &SyntaxNode, op: &str, name: String, tail: usize, edits: &mut Vec<Edit>| {
                let wrap = |s: &str| match tail {
                    1 => format!("-{s}"),
                    2 => format!("({s})"),
                    _ => s.to_owned(),
                };
                match (arg.kind(), operator(arg)) {
                    (BIN_EXPR, Some(r)) => {
                        edits.push(Edit { at: r, with: op.to_owned() });
                        if tail != 0
                            && let Some(rhs) = arg.children().filter(|c| is_expr(c.kind())).last()
                        {
                            let s = span(&rhs);
                            edits.push(Edit { at: s.clone(), with: wrap(&text[s]) });
                        }
                    }
                    _ => {
                        let end = span(arg).end;
                        edits.push(Edit { at: end..end, with: format!(" {op} {}", wrap(&name)) });
                    }
                }
            };
            halve(&args[i], "<", left, 0, &mut edits);
            halve(&args[i + 1], ">", right, tail, &mut edits);
            Some((edits, format!("angle-pair{}", ["", " -rhs", " (rhs)"][tail])))
        }
        Mutator::SwapItems => {
            let items: Vec<SyntaxNode> = root.children().collect();
            if items.len() < 2 {
                return None;
            }
            let i = rng.below(items.len() - 1);
            let j = if rng.below(2) == 0 { i + 1 } else { i + 1 + rng.below(items.len() - i - 1) };
            let (a, b) = (span(&items[i]), span(&items[j]));
            Some((
                vec![Edit { at: a.clone(), with: text[b.clone()].to_owned() }, Edit { at: b, with: text[a].to_owned() }],
                format!("swap-items {i}↔{j}"),
            ))
        }
        Mutator::SwapStmts => {
            let blocks: Vec<Vec<SyntaxNode>> = root
                .descendants()
                .filter(|n| n.kind() == BLOCK_STMT)
                .map(|b| b.children().filter(|c| is_stmt(c.kind())).collect::<Vec<_>>())
                .filter(|s| s.len() >= 2)
                .collect();
            let stmts = pick(rng, &blocks)?;
            let i = rng.below(stmts.len() - 1);
            let (a, b) = (span(&stmts[i]), span(&stmts[i + 1]));
            Some((
                vec![Edit { at: a.clone(), with: text[b.clone()].to_owned() }, Edit { at: b, with: text[a].to_owned() }],
                "swap-stmts".into(),
            ))
        }
        Mutator::Rename => {
            let declared: Vec<String> = {
                let mut d: Vec<String> = root
                    .descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| t.kind() == IDENT && t.parent().is_some_and(|p| p.kind() == NAME))
                    .map(|t| t.text().to_owned())
                    .collect();
                d.sort();
                d.dedup();
                d
            };
            let from = pick(rng, &declared)?.clone();
            let names: Vec<String> = name_refs(&root).into_iter().filter(|n| *n != from).collect();
            let to = pick(rng, &names)?.clone();
            let edits: Vec<Edit> = root
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| t.kind() == IDENT && t.text() == from)
                .map(|t| Edit { at: usize::from(t.text_range().start())..usize::from(t.text_range().end()), with: to.clone() })
                .collect();
            Some((edits, format!("rename `{from}`→`{to}`")))
        }
        Mutator::DupStmt | Mutator::DelStmt => {
            let stmts: Vec<SyntaxNode> = root
                .descendants()
                .filter(|n| {
                    let parent = n.parent().map(|p| p.kind());
                    (parent == Some(BLOCK_STMT) && is_stmt(n.kind()))
                        || (parent == Some(DECL_LIST) && n.kind() != VISIBILITY)
                })
                .collect();
            let r = crate::reduce::member_span(pick(rng, &stmts)?);
            if matches!(mutator, Mutator::DupStmt) {
                Some((vec![Edit { at: r.end..r.end, with: format!(" {}", &text[r.clone()]) }], "dup-stmt".into()))
            } else {
                Some((vec![Edit { at: r, with: String::new() }], "del-stmt".into()))
            }
        }
        Mutator::Literal => {
            let lits: Vec<_> =
                root.descendants_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == INT_NUMBER).collect();
            let t = pick(rng, &lits)?;
            let new = ["0", "1", "2", "3", "7", "8", "15", "16", "255", "256", "65535", "65536", "4294967296"][rng.below(13)];
            let at = usize::from(t.text_range().start())..usize::from(t.text_range().end());
            (t.text() != new).then(|| (vec![Edit { at, with: new.to_owned() }], format!("literal `{}`→`{new}`", t.text())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D1_BASE: &str = "inline void g(bool a, bool b) {}\ninline void f(uint8 x, uint8 y, uint8 z) { g(x + y, z + y); }\n";

    /// The mutator family can write docs/history/audit-2026-09-22.md D1's two shapes from a program
    /// that holds neither: `g(x < y, z > -y)` and `g(x < y, z > (y))`.
    #[test]
    fn the_mutators_reach_both_d1_shapes() {
        let (mut neg, mut paren) = (false, false);
        for i in 0..4000 {
            let mut rng = Rng(mix(7, i));
            let (text, _) = mutate(D1_BASE, &mut rng);
            neg |= text.contains("g(x < y, z > -y)");
            paren |= text.contains("g(x < y, z > (y))");
        }
        assert!(neg && paren, "neg {neg}, paren {paren}");
    }

    #[test]
    fn a_mutant_is_a_function_of_seed_and_index_alone() {
        let one = |i| mutate(D1_BASE, &mut Rng(mix(0x5EED, i)));
        assert_eq!(one(17).0, one(17).0);
        assert!((0..50).map(one).any(|(t, _)| t != one(17).0));
    }

    #[test]
    fn every_mutator_fires_on_a_program_with_a_site_for_it() {
        let base = "struct S { uint8 a; uint8 b; }\ninline uint8 f(uint8 x, uint8 y) { uint8 t = x + 1; t = y; return g(t * 2, y - x); }\n";
        for (m, _) in TABLE {
            let fired = (0..200).any(|i| mutate_once(base, m, &mut Rng(mix(1, i))).is_some_and(|(e, _)| apply(base, e) != base));
            assert!(fired, "{m:?} never changed the program");
        }
    }

    #[test]
    fn templates_blank_names_numbers_and_quotes() {
        assert_eq!(template("Symbol name not in scope: `x1`"), "Symbol name not in scope: `_`");
        assert_eq!(template("Invalid number of arguments, expected 2"), "Invalid number of arguments, expected N");
        assert_eq!(template("unexpected '<'"), "unexpected '_'");
        assert_eq!(template("an unmatched ` stays"), "an unmatched ` stays");
    }

    #[test]
    fn edits_apply_right_to_left() {
        let e = vec![Edit { at: 0..1, with: "xx".into() }, Edit { at: 2..2, with: "(".into() }, Edit { at: 3..3, with: ")".into() }];
        assert_eq!(apply("a b", e), "xx (b)");
    }
}
