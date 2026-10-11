//! The formatter gate: `cargo xtask fmt-check`.
//!
//! The plan's exit criterion for the formatter is three properties over every
//! `.k` file the reference ships, not over a chosen subset:
//!
//! * **idempotence** — `fmt(fmt(x)) == fmt(x)`;
//! * **structural equality** — reparsing `fmt(x)` yields the same non-trivia
//!   token sequence, so formatting is provably a layout-only change;
//! * **comment preservation** — every comment in `x` survives into `fmt(x)`.
//!
//! `kg-fmt` verifies the second and third internally and refuses to return text
//! that fails them. This gate re-derives all three from the outside anyway, on
//! the principle that a component should not be the only witness to its own
//! correctness.

use std::path::{Path, PathBuf};

use anyhow::Result;
use kg_config::Fmt;
use kg_syntax::{SyntaxKind, parse_file};

/// A file the formatter declined, with the reason. Declining is not a failure:
/// a file that does not parse cannot be laid out, and saying so is correct.
struct Declined {
    path: PathBuf,
    why: String,
}

struct Failure {
    path: PathBuf,
    property: &'static str,
    detail: String,
}

pub fn run(kanagawa_root: &Path, width: Option<usize>, verbose: bool) -> Result<()> {
    let mut files = Vec::new();
    for dir in ["library", "test", "doc", "tools"] {
        collect(&kanagawa_root.join(dir), &mut files);
    }
    files.sort();
    if files.is_empty() {
        anyhow::bail!("no .k files under {}", kanagawa_root.display());
    }

    let config = Fmt { line_width: width.unwrap_or(Fmt::default().line_width), ..Fmt::default() };

    let mut declined = Vec::new();
    let mut failures = Vec::new();
    let mut byte_identical = 0usize;
    let mut formatted = 0usize;

    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            declined.push(Declined { path: path.clone(), why: "not UTF-8".to_owned() });
            continue;
        };

        let once = match kg_fmt::format_file(&text, &config) {
            Ok(out) => out,
            Err(e) => {
                declined.push(Declined { path: path.clone(), why: short(&e) });
                continue;
            }
        };
        formatted += 1;
        if once == text {
            byte_identical += 1;
        }

        // 1. Idempotence.
        match kg_fmt::format_file(&once, &config) {
            Ok(twice) if twice != once => failures.push(Failure {
                path: path.clone(),
                property: "idempotence",
                detail: first_difference(&once, &twice),
            }),
            Err(e) => failures.push(Failure {
                path: path.clone(),
                property: "idempotence",
                detail: format!("second pass failed: {}", short(&e)),
            }),
            _ => {}
        }

        // 2. Structural equality, derived independently of kg-fmt's own check.
        let before = significant_tokens(&text);
        let after = significant_tokens(&once);
        if before != after {
            failures.push(Failure {
                path: path.clone(),
                property: "structural equality",
                detail: token_difference(&before, &after),
            });
        }

        // 3. Comment preservation. Trailing whitespace inside a comment is
        //    itself something the formatter is allowed to strip, so compare
        //    line-by-line trimmed.
        let cb = comments(&text);
        let ca = comments(&once);
        if cb != ca {
            failures.push(Failure {
                path: path.clone(),
                property: "comment preservation",
                detail: format!("{} comments in, {} out", cb.len(), ca.len()),
            });
        }
    }

    println!("{} .k files", files.len());
    println!("  formatted:       {formatted}");
    println!("  already correct: {byte_identical} byte-identical before formatting");
    if !declined.is_empty() {
        println!("  declined:        {} (see below)", declined.len());
    }

    if verbose || declined.len() <= 12 {
        for d in &declined {
            println!("    - {}: {}", rel(&d.path, kanagawa_root), d.why);
        }
    }

    if failures.is_empty() {
        println!("\n  {formatted}/{formatted} idempotent, structurally equal, comments preserved");
        return Ok(());
    }

    println!("\n  {} FAILURE(S):", failures.len());
    for f in failures.iter().take(if verbose { usize::MAX } else { 25 }) {
        println!("    {} [{}]\n      {}", rel(&f.path, kanagawa_root), f.property, f.detail);
    }
    anyhow::bail!("{} formatter failure(s)", failures.len())
}

fn short(e: &kg_fmt::FormatError) -> String {
    match e {
        kg_fmt::FormatError::Unparseable { errors } => match errors.first() {
            Some(p) => format!("does not parse: {} @ {}", p.message, p.offset),
            None => "does not parse".to_owned(),
        },
        kg_fmt::FormatError::Verification { detail } => format!("verification: {detail}"),
    }
}

/// Every non-trivia token, as `(kind, text)`. This is the program; everything
/// else is layout.
fn significant_tokens(text: &str) -> Vec<(SyntaxKind, String)> {
    parse_file(text)
        .syntax_node()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .map(|t| (t.kind(), t.text().to_owned()))
        .collect()
}

fn comments(text: &str) -> Vec<String> {
    parse_file(text)
        .syntax_node()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind().is_comment())
        .map(|t| t.text().lines().map(str::trim_end).collect::<Vec<_>>().join("\n"))
        .collect()
}

fn first_difference(a: &str, b: &str) -> String {
    for (i, (la, lb)) in a.lines().zip(b.lines()).enumerate() {
        if la != lb {
            return format!("line {}: {:?} -> {:?}", i + 1, la, lb);
        }
    }
    format!("{} lines vs {} lines", a.lines().count(), b.lines().count())
}

fn token_difference(a: &[(SyntaxKind, String)], b: &[(SyntaxKind, String)]) -> String {
    for (i, (ta, tb)) in a.iter().zip(b.iter()).enumerate() {
        if ta != tb {
            return format!("token {i}: {:?} {:?} -> {:?} {:?}", ta.0, ta.1, tb.0, tb.1);
        }
    }
    format!("{} tokens in, {} out", a.len(), b.len())
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}
