//! `cargo xtask corpus` — the L0 gate.
//!
//! Walks every `.k` file in the reference checkout and checks the two
//! properties that must hold before any semantic work is believable:
//!
//! * **losslessness** — `text(parse(f)) == f`, and
//! * **acceptance** — every whole-program file parses with zero errors.
//!
//! `test/syntax/` is excluded from the acceptance check: those files are not
//! translation units but snippet accumulators, full of deliberately invalid
//! fragments. They are covered instead by `cargo xtask conformance`, which
//! splits them at their `expected:` markers.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Per-file result.
struct FileResult {
    path: PathBuf,
    errors: usize,
    first_error: Option<(usize, usize, String)>,
}

pub fn run(kanagawa_root: &Path, verbose: bool) -> Result<()> {
    let mut files = Vec::new();
    for dir in ["library", "test", "doc", "tools"] {
        collect(&kanagawa_root.join(dir), &mut files);
    }
    files.sort();
    if files.is_empty() {
        anyhow::bail!("no .k files under {}", kanagawa_root.display());
    }

    let mut whole_programs = 0usize;
    let mut clean = 0usize;
    let mut lossy = Vec::new();
    let mut failing: Vec<FileResult> = Vec::new();
    let mut skipped = 0usize;

    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            // The corpus contains one deliberately non-UTF-8 fixture,
            // `test/syntax/imports/binary.k`, used to test importing a binary.
            skipped += 1;
            continue;
        };
        let parse = kg_syntax::parse_file(&text);
        let lossless = parse.text() == text;
        if !lossless {
            lossy.push(path.clone());
        }

        // Snippet accumulators are not translation units.
        if is_snippet_file(path) {
            continue;
        }
        whole_programs += 1;

        let errors = parse.errors().len();
        if errors == 0 {
            clean += 1;
        } else {
            let first = parse.errors().first().map(|e| {
                let offset = usize::from(e.range.start());
                let line = text[..offset].matches('\n').count() + 1;
                let col = offset - text[..offset].rfind('\n').map_or(0, |i| i + 1) + 1;
                (line, col, e.message.clone())
            });
            failing.push(FileResult {
                path: path.clone(),
                errors,
                first_error: first,
            });
        }
    }

    println!("{} .k files ({skipped} unreadable/non-UTF-8)", files.len());
    println!(
        "  losslessness: {}/{} round-trip exactly",
        files.len() - skipped - lossy.len(),
        files.len() - skipped
    );
    println!("  acceptance:   {clean}/{whole_programs} whole programs parse with zero errors");

    for path in lossy.iter().take(20) {
        println!("  LOSSY {}", rel(path, kanagawa_root));
    }

    // Rank by error count: the file with the most errors usually exercises the
    // construct that is missing, so it is the best place to start.
    failing.sort_by_key(|f| std::cmp::Reverse(f.errors));
    let show = if verbose { failing.len() } else { 25.min(failing.len()) };
    for f in &failing[..show] {
        match &f.first_error {
            Some((line, col, msg)) => println!(
                "  FAIL {}:{line}:{col}  {} error(s), first: {msg}",
                rel(&f.path, kanagawa_root),
                f.errors
            ),
            None => println!("  FAIL {}  {} error(s)", rel(&f.path, kanagawa_root), f.errors),
        }
    }
    if show < failing.len() {
        println!("  ... and {} more (pass --verbose)", failing.len() - show);
    }

    if !lossy.is_empty() {
        anyhow::bail!("{} files did not round-trip", lossy.len());
    }
    if !failing.is_empty() {
        anyhow::bail!("{} whole programs failed to parse", failing.len());
    }
    Ok(())
}

/// True for files that are not translation units and so cannot be expected to
/// parse on their own.
///
/// Two kinds:
///
/// * `test/syntax/*.k` accumulate snippets separated by `expected:` markers,
///   most of them deliberately invalid. Helper modules under
///   `test/syntax/helper/` *are* real programs and stay in.
/// * `tools/chkdoc/contexts/*.k` are templates with a bare `***` line where
///   `chkdoc` splices a documentation example before compiling
///   (`tools/chkdoc/chkdoc.cpp:79-81`). `***` is not Kanagawa syntax.
pub(crate) fn is_snippet_file(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.contains("tools/chkdoc/contexts/") {
        return true;
    }
    s.contains("test/syntax/") && !s.contains("test/syntax/helper/")
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

pub(crate) fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
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
