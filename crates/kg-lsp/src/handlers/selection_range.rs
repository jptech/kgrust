//! `textDocument/selectionRange` — expand-selection.
//!
//! The CST ancestor chain *is* the answer: token, then every enclosing node up
//! to the file. Because the tree is lossless, every intermediate step is a real
//! span of source, and because `ERROR` nodes are nodes like any other, the
//! feature works on a file that does not parse — expanding through the error
//! region instead of jumping straight to the whole file.

use kg_base::{LineIndex, TextRange, TextSize};
use kg_syntax::{SyntaxNode, SyntaxToken};
use lsp_types::{Position, SelectionRange};

use crate::convert::{self, PositionEncoding};

pub fn selection_ranges(
    root: &SyntaxNode,
    index: &LineIndex,
    enc: PositionEncoding,
    positions: &[Position],
) -> Vec<SelectionRange> {
    positions
        .iter()
        .map(|pos| {
            let offset = convert::offset(index, enc, *pos);
            selection_range(root, index, enc, offset)
        })
        .collect()
}

fn selection_range(
    root: &SyntaxNode,
    index: &LineIndex,
    enc: PositionEncoding,
    offset: TextSize,
) -> SelectionRange {
    let mut chain: Vec<TextRange> = Vec::new();

    if let Some(token) = token_at(root, offset) {
        chain.push(token.text_range());
        chain.extend(token.parent_ancestors().map(|n| n.text_range()));
    } else {
        chain.push(root.text_range());
    }

    // Adjacent equal ranges (a node with exactly one child) would make the
    // editor's "expand" key appear to do nothing.
    chain.dedup();

    // Build outermost-first so each range can own its parent.
    let mut current: Option<SelectionRange> = None;
    for range in chain.into_iter().rev() {
        current = Some(SelectionRange {
            range: convert::range(index, enc, range),
            parent: current.map(Box::new),
        });
    }
    current.unwrap_or_else(|| SelectionRange {
        range: convert::range(index, enc, TextRange::empty(offset)),
        parent: None,
    })
}

/// The token at `offset`, preferring the one to the left when the cursor sits
/// between two — which is what an editor's caret means at the end of a word.
fn token_at(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    // `token_at_offset` panics outside the tree, and a client is entitled to
    // send a position from a document state we have already moved past.
    let offset = offset.min(root.text_range().end());
    let at = root.token_at_offset(offset);
    at.clone()
        .find(|t| !t.kind().is_trivia())
        .or_else(|| at.left_biased())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_syntax::parse_file;

    fn chain(text: &str, offset: u32) -> Vec<(u32, u32)> {
        let parse = parse_file(text);
        let index = LineIndex::new(text);
        let mut out = Vec::new();
        let mut node = Some(selection_range(
            &parse.syntax_node(),
            &index,
            PositionEncoding::Utf16,
            TextSize::new(offset),
        ));
        while let Some(n) = node {
            out.push((n.range.start.character, n.range.end.character));
            node = n.parent.map(|b| *b);
        }
        out
    }

    #[test]
    fn the_chain_starts_at_the_token_and_grows() {
        let src = "class Adder {}";
        let out = chain(src, 8); // inside "Adder"
        assert!(out.len() >= 2, "{out:?}");
        assert_eq!(out[0], (6, 11), "the innermost range is the identifier");

        // Strictly widening, ending at the whole file.
        for pair in out.windows(2) {
            assert!(pair[1].0 <= pair[0].0 && pair[1].1 >= pair[0].1, "{out:?}");
        }
        assert_eq!(*out.last().unwrap(), (0, src.len() as u32));
    }

    #[test]
    fn no_two_consecutive_ranges_are_equal() {
        let out = chain("inline void f(uint32 x) { return; }", 21);
        for pair in out.windows(2) {
            assert_ne!(pair[0], pair[1], "{out:?}");
        }
    }

    #[test]
    fn an_empty_file_still_answers() {
        let out = chain("", 0);
        assert_eq!(out, vec![(0, 0)]);
    }

    #[test]
    fn a_position_past_the_end_does_not_panic() {
        let out = chain("class A {}", 9999);
        assert!(!out.is_empty());
    }

    #[test]
    fn multiple_positions_are_answered_in_order() {
        let src = "class A {}\nclass B {}\n";
        let parse = parse_file(src);
        let out = selection_ranges(
            &parse.syntax_node(),
            &LineIndex::new(src),
            PositionEncoding::Utf16,
            &[Position::new(0, 7), Position::new(1, 7)],
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].range.start.line, 0);
        assert_eq!(out[1].range.start.line, 1);
    }
}
