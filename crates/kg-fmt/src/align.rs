//! Putting hand-built columns back where they were.
//!
//! `docs/spec/style.md` §10.3 is emphatic: 144 of the 164 multi-line
//! trailing-comment runs in the corpus are column-aligned, and a formatter that
//! collapses them to one space destroys 144 hand-built tables. Example 9 —
//! `library/codec/crc.k:39-45` — is required to come out byte-identical even
//! though one of its lines is 179 columns wide, purely because the overflow is
//! a `//<` doc comment aligned with three others.
//!
//! Column alignment cannot be expressed in the document algebra, which knows
//! nothing about the column a group will land in. It is instead a pass over the
//! rendered text, and it is only possible because of the property
//! [`crate::verify`] enforces: the input and the output hold the same tokens in
//! the same order, so the *i*-th `=` of the output is the *i*-th `=` of the
//! input and its original column can simply be looked up.
//!
//! Two shapes are restored, both idempotent by construction — a second run
//! reads back the columns the first one wrote and computes the same target:
//!
//! * a run of consecutive lines whose anchors shared a column in the input is
//!   re-aligned to one column in the output;
//! * a continuation line of a trailing doc comment — the bare `//` lines under
//!   a `//<`, 2695 of 2696 of which sit at the marker's own column
//!   (`docs/spec/style.md` §8.4) — is re-indented to that column rather than to
//!   the code indent.

use kg_syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

/// Where one anchor token sits.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    line: usize,
    /// Column in characters, which is what alignment is about.
    col: usize,
    /// Byte offset of the token within its line.
    byte: usize,
}

/// Restores the column alignment `input` had, in `output`.
pub(crate) fn realign(input: &SyntaxNode, output: String) -> String {
    let input_text = input.text().to_string();
    // Code anchors first — `=` in a value table, `as` in an import table —
    // because padding them shifts any comment that trails them, and the comment
    // pass has to see the final code layout.
    //
    // Both passes need the *output* parsed, and re-parsing was 40% of `realign`
    // and a tenth of the whole formatter. The second pass only needs a fresh
    // tree if the first one actually moved something, which for most files it
    // does not — so the tree is threaded through and re-parsed on demand.
    // Both passes read the *input* tree, which neither of them changes, so its
    // anchors are gathered in one walk instead of one per pass.
    let (code, comments) = input_anchors(input, &input_text);

    let mut tree = None;
    let out = pass(&code, output, &mut tree, |k| {
        matches!(k, SyntaxKind::EQ | SyntaxKind::AS_KW)
    });
    pass(&comments, out, &mut tree, SyntaxKind::is_comment)
}

/// The output text parsed, reused across both passes of [`realign`].
///
/// `None` means "not parsed yet, or invalidated by an edit". A pass that changes
/// the string must clear it; a pass that declines must leave it, because the
/// string it declined to change is the one already in the cache.
type OutTree = Option<SyntaxNode>;

fn pass(
    before: &[Anchor],
    output: String,
    tree: &mut OutTree,
    pick: impl Fn(SyntaxKind) -> bool + Copy,
) -> String {
    if before.len() < 2 {
        return output;
    }
    let out_root = match tree {
        Some(t) => t.clone(),
        None => {
            let t = crate::reparse(&output).syntax_node();
            *tree = Some(t.clone());
            t
        }
    };
    let after = anchors(out_root, &output, pick);
    // A mismatch means something upstream is wrong; alignment is cosmetic, so
    // it declines rather than guesses. `verify` will report the real problem.
    if before.len() != after.len() {
        return output;
    }

    let mut lines: Vec<String> = output.split('\n').map(str::to_owned).collect();
    // (line, byte offset in line, target column, whether the line starts with
    // the anchor) — collected first so the string is mutated once per line.
    let mut fixes: Vec<(usize, usize, usize, bool)> = Vec::new();

    let mut i = 0;
    while i < before.len() {
        let mut j = i + 1;
        while j < before.len()
            && before[j].line == before[j - 1].line + 1
            && before[j].col == before[j - 1].col
            && after[j].line == after[j - 1].line + 1
        {
            j += 1;
        }
        if j - i >= 2 {
            let run = i..j;
            // The run must begin with an anchor that trails code; a run of
            // own-line comments is an ordinary comment block whose indent the
            // layout already chose.
            let leads = has_code_before(&lines[after[i].line], after[i].byte);
            if leads {
                // The column the author chose, unless the code has since grown
                // past it. Anchoring on the input is what makes
                // `library/codec/crc.k:39-45` come back byte-identical
                // (`docs/spec/style.md` §12, example 9).
                let target =
                    run.clone().map(|k| after[k].col).chain([before[i].col]).max().unwrap_or(0);
                for k in run {
                    let a = after[k];
                    let own_line = !has_code_before(&lines[a.line], a.byte);
                    if a.col != target || own_line {
                        fixes.push((a.line, a.byte, target, own_line));
                    }
                }
            }
        }
        i = j.max(i + 1);
    }

    // Any fix rewrites the string, so the cached tree no longer describes it.
    // Clearing here rather than at each mutation keeps the invariant in one
    // place: the cache holds a parse of exactly the string this function was
    // handed, and stops holding one the moment that stops being true.
    if !fixes.is_empty() {
        *tree = None;
    }

    for (line, byte, target, own_line) in fixes.into_iter().rev() {
        let l = &mut lines[line];
        if own_line {
            let rest = l[byte..].to_owned();
            *l = " ".repeat(target) + &rest;
        } else {
            let col = l[..byte].chars().count();
            if target > col {
                l.insert_str(byte, &" ".repeat(target - col));
            }
        }
    }
    lines.join("\n")
}

fn has_code_before(line: &str, byte: usize) -> bool {
    line.get(..byte).is_some_and(|p| !p.trim().is_empty())
}

/// The `=`/`as` anchors and the comment anchors of the input, in one traversal.
///
/// Splitting them costs nothing — a token is one kind or the other — and the
/// walk is the expensive half: it was a quarter of `realign` when each pass did
/// its own.
fn input_anchors(root: &SyntaxNode, text: &str) -> (Vec<Anchor>, Vec<Anchor>) {
    let starts = line_starts(text);
    let mut code = Vec::new();
    let mut comments = Vec::new();
    for e in root.descendants_with_tokens() {
        let SyntaxElement::Token(t) = e else { continue };
        let is_code = matches!(t.kind(), SyntaxKind::EQ | SyntaxKind::AS_KW);
        if !is_code && !SyntaxKind::is_comment(t.kind()) {
            continue;
        }
        let off = usize::from(t.text_range().start());
        let line = starts.partition_point(|&s| s <= off) - 1;
        let a = Anchor { line, col: text[starts[line]..off].chars().count(), byte: off - starts[line] };
        if is_code { code.push(a) } else { comments.push(a) }
    }
    (code, comments)
}

fn anchors(root: SyntaxNode, text: &str, pick: impl Fn(SyntaxKind) -> bool) -> Vec<Anchor> {
    let starts = line_starts(text);
    root.descendants_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if pick(t.kind()) => {
                let off = usize::from(t.text_range().start());
                let line = starts.partition_point(|&s| s <= off) - 1;
                let byte = off - starts[line];
                let col = text[starts[line]..off].chars().count();
                Some(Anchor { line, col, byte })
            }
            _ => None,
        })
        .collect()
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    starts
}
