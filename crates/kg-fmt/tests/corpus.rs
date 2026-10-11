//! The formatter, run over the reference corpus.
//!
//! Three properties are checked on every file that parses, and they are the
//! whole argument that the formatter is safe to run on someone's tree:
//!
//! | property | what it rules out |
//! |---|---|
//! | idempotence — `fmt(fmt(x)) == fmt(x)` | a layout with two fixed points, so that running the formatter twice churns the diff |
//! | structural equality — the non-trivia token sequence of `fmt(x)` equals that of `x` | any edit that is not layout: a dropped `;`, a reordered declaration, a rewritten spelling |
//! | comment survival — every comment appears once, in order, with its text intact | the failure mode of every AST re-printer, `kanagawa pretty` included |
//!
//! The last two are enforced inside [`kg_fmt::format_file`] itself, which
//! returns [`kg_fmt::FormatError::Verification`] rather than handing back text
//! it cannot vouch for; this test additionally checks them from the outside so
//! a bug in the checker cannot hide a bug in the formatter.
//!
//! The corpus is the reference tree, which is not part of this repository: when
//! it is absent the test reports and passes. Files that do not parse are
//! counted and skipped rather than failed — the grammar is still being
//! finished, and `test/syntax/`'s 69 multi-case harness fixtures
//! (`docs/spec/style.md` §0.3) are not translation units and never will parse.

use std::path::{Path, PathBuf};

use kg_config::Fmt;
use kg_fmt::{FormatError, format_file};
use kg_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, parse_file};

/// Under the reference ([`kg_base::reference_root`]).
const ROOTS: [&str; 4] = ["library", "test/logic", "test/library", "test/interface"];

#[derive(Default)]
struct Tally {
    files: usize,
    unparseable: usize,
    parsed: usize,
    idempotent: usize,
    same_tokens: usize,
    same_comments: usize,
    unchanged: usize,
}

#[test]
fn the_corpus_survives_formatting() {
    let files = collect();
    if files.is_empty() {
        kg_base::corpus_absent(&format!("corpus not present at {ROOTS:?}"));
        return;
    }
    let cfg = Fmt::default();
    let mut tally = Tally { files: files.len(), ..Tally::default() };
    let mut failures: Vec<String> = Vec::new();

    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            // `test/syntax/imports/binary.k` is four bytes of non-UTF-8, a
            // fixture for the "imported file is not source" error path.
            continue;
        };
        let formatted = match format_file(&text, &cfg) {
            Ok(out) => {
                tally.parsed += 1;
                out
            }
            Err(FormatError::Unparseable { .. }) => {
                tally.unparseable += 1;
                continue;
            }
            Err(e) => {
                tally.parsed += 1;
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };

        if formatted == text {
            tally.unchanged += 1;
        }
        match format_file(&formatted, &cfg) {
            Ok(twice) if twice == formatted => tally.idempotent += 1,
            Ok(twice) => failures.push(format!(
                "{}: not idempotent, first difference at line {}",
                path.display(),
                first_difference(&formatted, &twice)
            )),
            Err(e) => failures.push(format!("{}: second pass failed: {e}", path.display())),
        }

        let (before, after) = (parse_file(&text), parse_file(&formatted));
        if tokens(&before.syntax_node()) == tokens(&after.syntax_node()) {
            tally.same_tokens += 1;
        } else {
            failures.push(format!("{}: the token sequence changed", path.display()));
        }
        if comments(&before.syntax_node()) == comments(&after.syntax_node()) {
            tally.same_comments += 1;
        } else {
            failures.push(format!("{}: a comment was lost or moved", path.display()));
        }
    }

    eprintln!(
        "corpus: {} files, {} skipped as unparseable, {} formatted\n\
         \x20 idempotent          {}/{}\n\
         \x20 same token sequence {}/{}\n\
         \x20 comments preserved  {}/{}\n\
         \x20 byte-identical      {}/{}",
        tally.files,
        tally.unparseable,
        tally.parsed,
        tally.idempotent,
        tally.parsed,
        tally.same_tokens,
        tally.parsed,
        tally.same_comments,
        tally.parsed,
        tally.unchanged,
        tally.parsed,
    );

    assert!(
        failures.is_empty(),
        "{} of {} formatted files broke a property:\n{}",
        failures.len(),
        tally.parsed,
        failures.join("\n")
    );
}

/// Reformatting a formatted file changes nothing, whatever the width.
///
/// A narrow width is the interesting case: it forces every group open, so the
/// broken layout of every list is exercised at once.
#[test]
fn narrow_and_wide_layouts_are_both_stable() {
    let files = collect();
    if files.is_empty() {
        kg_base::corpus_absent(&format!("corpus not present at {ROOTS:?}"));
        return;
    }
    for width in [40usize, 80, 100, 200] {
        let cfg = Fmt { line_width: width, ..Fmt::default() };
        let mut failures = Vec::new();
        for path in files.iter().take(120) {
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            let Ok(once) = format_file(&text, &cfg) else { continue };
            match format_file(&once, &cfg) {
                Ok(twice) if twice == once => {}
                Ok(twice) => failures.push(format!(
                    "{} at width {width}: differs at line {}",
                    path.display(),
                    first_difference(&once, &twice)
                )),
                Err(e) => failures.push(format!("{} at width {width}: {e}", path.display())),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

fn collect() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let reference = kg_base::reference_root();
    for root in ROOTS {
        walk(&reference.join(root), &mut out);
    }
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

fn tokens(root: &SyntaxNode) -> Vec<(SyntaxKind, String)> {
    root.descendants_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if !t.kind().is_trivia() => {
                Some((t.kind(), t.text().to_owned()))
            }
            _ => None,
        })
        .collect()
}

fn comments(root: &SyntaxNode) -> Vec<(SyntaxKind, String)> {
    root.descendants_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind().is_comment() => Some((
                t.kind(),
                t.text().replace('\r', "").split('\n').map(str::trim).collect::<Vec<_>>().join("\n"),
            )),
            _ => None,
        })
        .collect()
}

fn first_difference(a: &str, b: &str) -> usize {
    a.lines().zip(b.lines()).position(|(x, y)| x != y).map_or(0, |i| i + 1)
}
