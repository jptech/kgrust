//! Proving the formatter only moved whitespace.
//!
//! A formatter that reformats is useful; one that *edits* is a disaster, and
//! the difference is invisible in a diff of 40 000 lines. So every result
//! [`crate::format_file`] returns has been checked against three properties,
//! all cheap next to the cost of parsing:
//!
//! * the output parses, with no errors;
//! * its non-trivia token sequence is identical to the input's, token for
//!   token, kind and text — the formal statement of "a comment never moved
//!   across a token", and of "no declaration was reordered, dropped or
//!   invented";
//! * every comment survives, in order, with its text intact.
//!
//! Comment text is compared line-by-line with the ends trimmed, because
//! stripping trailing whitespace is itself a house rule (`docs/spec/style.md`
//! §1.5: 228 lines in 60 files) and a multi-line block comment is re-indented
//! to wherever its code moved. Nothing else about a comment may change.

use kg_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, parse_file};

/// One token, as the comparison sees it.
type Sig = (SyntaxKind, String);

/// The non-trivia tokens of a tree, in source order.
pub(crate) fn tokens(root: &SyntaxNode) -> Vec<Sig> {
    root.descendants_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if !t.kind().is_trivia() => {
                Some((t.kind(), t.text().to_owned()))
            }
            _ => None,
        })
        .collect()
}

/// The comment tokens of a tree, in source order, normalized for comparison.
pub(crate) fn comments(root: &SyntaxNode) -> Vec<Sig> {
    root.descendants_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if t.kind().is_comment() => {
                Some((t.kind(), normalize(t.text())))
            }
            _ => None,
        })
        .collect()
}

fn normalize(text: &str) -> String {
    text.replace('\r', "")
        .split('\n')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Checks `output` against `input_root`. `Ok(())` means the formatter changed
/// only layout.
///
/// `ordered` is false when `sort_imports` is on — the one setting under which
/// the formatter is licensed to move a declaration. Order is then no longer
/// evidence of anything, so the check falls back to a multiset comparison,
/// which still catches a dropped, duplicated or rewritten token or comment.
pub(crate) fn check(
    input_root: &SyntaxNode,
    output: &str,
    ordered: bool,
) -> Result<(), String> {
    let reparsed = parse_file(output);
    if let Some(err) = reparsed.errors().first() {
        return Err(format!(
            "the formatted output does not parse: {} at offset {:?}",
            err.message,
            u32::from(err.range.start())
        ));
    }
    let out_root = reparsed.syntax_node();

    let (mut before, mut after) = (tokens(input_root), tokens(&out_root));
    if !same(&mut before, &mut after, ordered) {
        return Err(describe("token", &before, &after));
    }

    let (mut before, mut after) = (comments(input_root), comments(&out_root));
    if !same(&mut before, &mut after, ordered) {
        return Err(describe("comment", &before, &after));
    }
    Ok(())
}

fn same(before: &mut [Sig], after: &mut [Sig], ordered: bool) -> bool {
    if !ordered {
        before.sort();
        after.sort();
    }
    before == after
}

fn describe(what: &str, before: &[Sig], after: &[Sig]) -> String {
    let at = before.iter().zip(after).position(|(a, b)| a != b);
    match at {
        Some(i) => format!(
            "{what} {i} changed: {:?} became {:?} ({} before, {} after)",
            before[i],
            after[i],
            before.len(),
            after.len()
        ),
        None => format!(
            "{what} count changed: {} before, {} after (first extra: {:?})",
            before.len(),
            after.len(),
            before.get(after.len()).or_else(|| after.get(before.len()))
        ),
    }
}
