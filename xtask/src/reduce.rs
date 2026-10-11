//! `cargo xtask reduce <file.k>` — delta debugging over Kanagawa source,
//! keeping whatever makes this frontend and `kg-oracle --errors` disagree.
//!
//! The journal's most useful entries are hand reductions ("nine library-free
//! lines"); this is the machine doing that. The interestingness test is the
//! differential probe's [`Signature`]: the kind of disagreement plus the first
//! line of each side's first error, positions left out. `--keep errors` (the
//! default) holds the whole signature, so the reduced program fails the same
//! way in the same words; `--keep accept` holds only the kind, and applies
//! only where one side accepts — the bug is then "we disagree about whether
//! this compiles", and the words may change on the way down.
//!
//! # The algorithm
//!
//! Hierarchical ddmin, complements only: at each granularity the program is
//! a list of units, and chunks of them are deleted while the signature holds,
//! halving the chunk size when no chunk can go. The granularities run coarse
//! to fine — top-level declarations, then statements, members and block
//! bodies, then lines, then tokens — and the round repeats until one changes
//! nothing, because deleting a token can free a statement.
//!
//! A deletion that glues two words together (`a b` losing what separated
//! them) gets a space, so a token-level cut never invents an identifier.
//!
//! The candidates of one ddmin step are probed over threads that each own a
//! `Db` (`docs/gates.md`, "A gate is a driver over threads"); the step takes the
//! lowest-numbered interesting chunk of the first batch holding one, so the
//! result does not depend on which thread finished first.
//!
//! # What it cannot see
//!
//! * One file only. A disagreement that needs an import to keep a helper
//!   module in the program is kept only while the `import` line survives,
//!   and the helper itself is never reduced.
//! * The signature is the first line of each message. A reduction can move
//!   between two bugs that share a first line (a hint list below it, say).

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::conformance::{Assertion, load_all};
use crate::differential::{Context, Kind, Probe, Signature, Worker, jobs, probe_all, workspace_target};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Keep {
    /// The whole signature: kind, and the first line of both messages.
    Errors,
    /// The kind alone — "one side accepts and the other does not".
    Accept,
}

impl Keep {
    fn holds(self, want: &Signature, got: &Signature) -> bool {
        match self {
            Keep::Errors => got == want,
            Keep::Accept => got.kind == want.kind,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    Items,
    Statements,
    Lines,
    Tokens,
}

const LEVELS: [Level; 4] = [Level::Items, Level::Statements, Level::Lines, Level::Tokens];

/// The result of one reduction.
pub(crate) struct Reduced {
    pub(crate) text: String,
    pub(crate) probe: Probe,
    pub(crate) tests: usize,
}

/// Cuts `text` down while `keep` says its signature still matches `want`.
/// `budget` bounds the probes and `deadline` the time — a hang costs the
/// probe server's whole limit per interesting candidate; a reduction that
/// runs out of either returns the smallest interesting program found so far.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reduce(
    cx: &Context,
    workers: &mut [Worker],
    like: &Assertion,
    text: &str,
    keep: Keep,
    want: &Signature,
    budget: usize,
    deadline: Option<std::time::Instant>,
) -> Reduced {
    let mut state = State { cx, like, keep, want, budget, deadline, tests: 0, seen: HashMap::new(), best_probe: None };
    let mut current = text.to_owned();
    for _round in 0..6 {
        let before = current.len();
        for level in LEVELS {
            current = state.ddmin(workers, &current, &units(&current, level));
        }
        if current.len() == before || state.exhausted() {
            break;
        }
    }
    let probe = match state.best_probe.take() {
        Some((t, p)) if t == current => p,
        _ => workers[0].probe(cx, like, &current),
    };
    Reduced { text: current, probe, tests: state.tests }
}

struct State<'a> {
    cx: &'a Context,
    like: &'a Assertion,
    keep: Keep,
    want: &'a Signature,
    budget: usize,
    deadline: Option<std::time::Instant>,
    tests: usize,
    /// Every candidate already probed, by text: the levels overlap, and a
    /// line that is one token is proposed at both.
    seen: HashMap<String, bool>,
    /// The last interesting candidate and its probe, so the final report
    /// does not probe the result a second time.
    best_probe: Option<(String, Probe)>,
}

impl State<'_> {
    fn exhausted(&self) -> bool {
        self.tests >= self.budget || self.deadline.is_some_and(|d| std::time::Instant::now() >= d)
    }

    fn ddmin(&mut self, workers: &mut [Worker], text: &str, units: &[Range<usize>]) -> String {
        let mut removed = vec![false; units.len()];
        let mut current = text.to_owned();
        let mut n = 2usize;
        loop {
            let alive: Vec<usize> = (0..units.len()).filter(|&i| !removed[i]).collect();
            if alive.is_empty() || self.exhausted() {
                return current;
            }
            n = n.min(alive.len());
            let size = alive.len().div_ceil(n);
            let mut candidates: Vec<(Vec<bool>, String)> = Vec::new();
            for chunk in alive.chunks(size) {
                let mut r = removed.clone();
                for &i in chunk {
                    r[i] = true;
                }
                let t = build(text, units, &r);
                if t != current {
                    candidates.push((r, t));
                }
            }
            match self.first_interesting(workers, &candidates) {
                Some(k) => {
                    let (r, t) = candidates.swap_remove(k);
                    removed = r;
                    current = t;
                    n = n.saturating_sub(1).max(2);
                }
                None if n >= alive.len() => return current,
                None => n = (n * 2).min(alive.len()),
            }
        }
    }

    /// The lowest-numbered interesting candidate, probing in batches of one
    /// per worker and stopping at the first batch that holds one.
    fn first_interesting(&mut self, workers: &mut [Worker], candidates: &[(Vec<bool>, String)]) -> Option<usize> {
        for batch_start in (0..candidates.len()).step_by(workers.len()) {
            let batch: Vec<usize> = (batch_start..candidates.len().min(batch_start + workers.len()))
                .filter(|&k| !self.seen.contains_key(&candidates[k].1))
                .collect();
            let room = self.budget.saturating_sub(self.tests);
            let batch = &batch[..batch.len().min(room)];
            let items: Vec<(&Assertion, &str)> = batch.iter().map(|&k| (self.like, candidates[k].1.as_str())).collect();
            let order: Vec<usize> = (0..items.len()).collect();
            let probes = probe_all(self.cx, workers, &items, &order, None);
            self.tests += items.len();
            for (&k, probe) in batch.iter().zip(probes) {
                let probe = probe.expect("no deadline, every slot probed");
                let ok = self.keep.holds(self.want, &probe.signature());
                self.seen.insert(candidates[k].1.clone(), ok);
                if ok && self.best_probe.as_ref().is_none_or(|(t, _)| candidates[k].1.len() < t.len()) {
                    self.best_probe = Some((candidates[k].1.clone(), probe));
                }
            }
            let end = candidates.len().min(batch_start + workers.len());
            if let Some(k) = (batch_start..end).find(|&k| self.seen.get(&candidates[k].1) == Some(&true)) {
                return Some(k);
            }
            if self.exhausted() {
                return None;
            }
        }
        None
    }
}

/// `text` without the units `removed` marks, a space kept where a cut would
/// glue two words into one.
fn build(text: &str, units: &[Range<usize>], removed: &[bool]) -> String {
    let mut cut = vec![false; text.len()];
    for (u, &r) in units.iter().zip(removed) {
        if r {
            cut[u.clone()].iter_mut().for_each(|c| *c = true);
        }
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for (i, c) in text.char_indices() {
        if cut[i] {
            gap = true;
            continue;
        }
        if gap && word(c) && out.chars().next_back().is_some_and(word) {
            out.push(' ');
        }
        gap = false;
        out.push(c);
    }
    out
}

/// The deletable units of `text` at one granularity, in source order.
fn units(text: &str, level: Level) -> Vec<Range<usize>> {
    use kg_syntax::SyntaxKind::*;
    let range = |n: &kg_syntax::SyntaxNode| {
        let r = kg_syntax::trimmed_range(n);
        usize::from(r.start())..usize::from(r.end())
    };
    match level {
        Level::Items => kg_syntax::parse_file(text).syntax_node().children().map(|n| range(&n)).collect(),
        Level::Statements => {
            let root = kg_syntax::parse_file(text).syntax_node();
            let mut out = Vec::new();
            for n in root.descendants() {
                let parent = n.parent().map(|p| p.kind());
                let member = matches!(parent, Some(BLOCK_STMT | DECL_LIST))
                    && !matches!(n.kind(), VISIBILITY);
                // A block's whole body, braces kept: `{ … }` to `{}`.
                if matches!(n.kind(), BLOCK_STMT | DECL_LIST) {
                    let r = range(&n);
                    if r.len() > 2 {
                        out.push(r.start + 1..r.end - 1);
                    }
                }
                if member {
                    out.push(member_span(&n));
                } else if matches!(n.kind(), ENUM_VARIANT | PARAM | ELSE_BRANCH | ATTR_LIST) {
                    out.push(range(&n));
                }
            }
            out.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
            out
        }
        Level::Lines => {
            let mut out = Vec::new();
            let mut start = 0;
            for (i, c) in text.char_indices() {
                if c == '\n' {
                    out.push(start..i + 1);
                    start = i + 1;
                }
            }
            if start < text.len() {
                out.push(start..text.len());
            }
            out
        }
        Level::Tokens => {
            // A token owns the trivia after it, so deleting one leaves its
            // neighbours separated as they were.
            let lexed = kg_parser::LexedStr::new(text);
            let mut out: Vec<Range<usize>> = Vec::new();
            for i in 0..lexed.len() {
                let r = lexed.range(i);
                let r = usize::from(r.start())..usize::from(r.end());
                match out.last_mut() {
                    Some(last) if lexed.kind(i).is_trivia() => last.end = r.end,
                    _ => out.push(r),
                }
            }
            out
        }
    }
}

/// A statement or member with its terminator: a member's `;` is the
/// `DECL_LIST`'s token, not the declaration's, and a copy or a cut that
/// leaves it behind makes a syntax error of an edit meant to be grammatical.
pub(crate) fn member_span(n: &kg_syntax::SyntaxNode) -> Range<usize> {
    let t = kg_syntax::trimmed_range(n);
    let mut r = usize::from(t.start())..usize::from(t.end());
    let mut next = n.next_sibling_or_token();
    while let Some(e) = next {
        match e.as_token() {
            Some(t) if t.kind().is_trivia() => next = t.next_sibling_or_token(),
            Some(t) if t.kind() == kg_syntax::SyntaxKind::SEMICOLON => {
                r.end = usize::from(t.text_range().end());
                break;
            }
            _ => break,
        }
    }
    r
}

/// The flags a lone file is compiled under: the ones the most snippets share,
/// which is the syntax suite's `set(OPTIONS …)` block plus the helper
/// directory — how `xtask errors` sets up the import dirs for all but a few.
fn default_like(assertions: &[Assertion]) -> Option<&Assertion> {
    let mut counts: HashMap<&[String], usize> = HashMap::new();
    for a in assertions {
        *counts.entry(&a.options).or_default() += 1;
    }
    let (options, _) = counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))?;
    assertions.iter().find(|a| &*a.options == options)
}

/// Where a reduction may be written: under this workspace, never under the
/// reference tree.
fn checked_out(root: &Path, out: &Path) -> Result<PathBuf> {
    let out = if out.is_absolute() { out.to_path_buf() } else { std::env::current_dir()?.join(out) };
    let parent = out.parent().context("the output path has no directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let parent = parent.canonicalize()?;
    if parent.starts_with(root) {
        anyhow::bail!("refusing to write under the reference tree ({})", root.display());
    }
    Ok(parent.join(out.file_name().context("the output path names no file")?))
}

pub fn run(
    kanagawa_root: &Path,
    file: &Path,
    keep: &str,
    like: Option<&str>,
    out: Option<&Path>,
    jobs_requested: Option<usize>,
    max_tests: usize,
) -> Result<()> {
    let keep = match keep {
        "errors" => Keep::Errors,
        "accept" => Keep::Accept,
        other => anyhow::bail!("unknown --keep {other:?}; use errors or accept"),
    };
    let cx = Context::new(kanagawa_root, "xtask-reduce")?;
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let assertions = load_all(&cx.root)?;
    let like = match like {
        Some(name) => assertions
            .iter()
            .find(|a| a.name == name)
            .with_context(|| format!("no assertion named {name}"))?,
        None => default_like(&assertions).context("the corpus has no assertions")?,
    };
    let stem = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let out = checked_out(&cx.root, out.map_or_else(|| workspace_target().join("reduce").join(format!("{stem}.reduced.k")), Path::to_path_buf).as_path())?;

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut workers: Vec<Worker> = (0..jobs(jobs_requested)).map(|_| Worker::new()).collect();
    let start = std::time::Instant::now();
    let first = workers[0].probe(&cx, like, &text);
    let want = first.signature();
    let outcome = (|| {
        if !want.kind.is_finding() {
            anyhow::bail!("nothing to keep: {} (ours: {}; theirs: {})", want.kind.label(), first.render_ours(), first.render_theirs());
        }
        if keep == Keep::Accept && want.kind.both_reject() {
            anyhow::bail!("--keep accept needs one side to accept; both reject here ({}) — use --keep errors", want.kind.label());
        }
        Ok(reduce(&cx, &mut workers, like, &text, keep, &want, max_tests, None))
    })();
    std::panic::set_hook(hook);
    let _ = std::fs::remove_dir_all(&cx.scratch);
    let reduced = outcome?;

    std::fs::write(&out, &reduced.text).with_context(|| format!("writing {}", out.display()))?;
    println!("{} under the flags of {}", want.kind.label(), like.name);
    println!("  ours:   {}", first.render_ours());
    println!("  theirs: {}", first.render_theirs());
    println!(
        "reduced {} → {} bytes in {} probes, {:.1} s; written to {}",
        text.len(),
        reduced.text.len(),
        reduced.tests,
        start.elapsed().as_secs_f64(),
        out.display()
    );
    println!("  ours:   {}", reduced.probe.render_ours());
    println!("  theirs: {}", reduced.probe.render_theirs());
    println!("  {}", crate::oracle_cache::report());
    if reduced.text.len() <= 2000 {
        println!("---\n{}\n---", reduced.text.trim_end());
    }
    if reduced.probe.kind() == Kind::Agree {
        anyhow::bail!("the reduced program no longer disagrees — the probe is not deterministic");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_never_glues_two_words() {
        let text = "a b c";
        let u = units(text, Level::Tokens);
        assert_eq!(u.len(), 3);
        assert_eq!(build(text, &u, &[false, true, false]), "a c");
        let text = "a+b";
        let u = units(text, Level::Tokens);
        assert_eq!(build(text, &u, &[false, true, false]), "a b");
    }

    #[test]
    fn statement_units_include_block_bodies_and_members() {
        let text = "inline void f() { uint8 x = 1; x = 2; }\nstruct S { uint8 a; uint8 b; }\n";
        let u = units(text, Level::Statements);
        let spans: Vec<&str> = u.iter().map(|r| &text[r.clone()]).collect();
        assert!(spans.contains(&"uint8 x = 1;"), "{spans:?}");
        assert!(spans.contains(&"x = 2;"), "{spans:?}");
        // A member's `;` belongs to the list, and goes with the member.
        assert!(spans.contains(&"uint8 a;"), "{spans:?}");
        assert!(spans.contains(&" uint8 x = 1; x = 2; "), "{spans:?}");
    }

    #[test]
    fn the_output_never_lands_under_the_reference() {
        let dir = workspace_target().join("reduce-test-root");
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        let root = dir.canonicalize().unwrap();
        assert!(checked_out(&root, &dir.join("inner/x.k")).is_err());
        assert!(checked_out(&root, &workspace_target().join("reduce-test-out/x.k")).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(workspace_target().join("reduce-test-out"));
    }
}
