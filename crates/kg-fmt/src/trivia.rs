//! Where the comments are, and what the author's line breaks said.
//!
//! The formatter walks the lossless CST, so every comment is a real token
//! sitting between two other tokens. This module turns that raw trivia into the
//! two questions the lowering actually asks:
//!
//! * *Where does a comment go?* — a comment that began its own source line
//!   stays on its own line; one that trailed code stays on that code's line.
//!   `//|` documents what follows and `//<` what precedes
//!   (`Parser/Lexer.hs:63-65`), and those two conventions agree with the
//!   line-position rule everywhere in the corpus, so position is what we use:
//!   it also classifies the 10 108 plain `//` comments, which carry no marker.
//! * *Did the author break here?* — the corpus is genuinely split on several
//!   constructs (`docs/spec/style.md` §10), and for those the author's own line
//!   break is the input to a "preserve" rule.
//!
//! Both questions are answered from the *global* token stream rather than from
//! a node's own children, because the parser deliberately pulls a leading
//! comment run inside the declaration it documents
//! (`kg-parser/src/output.rs`). A node's first child can therefore be a comment
//! whose preceding whitespace lives in the parent, and only a global lookup
//! sees it.

use kg_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

use SyntaxKind::*;

/// A comment token, with the layout facts the formatter needs about it.
#[derive(Debug, Clone)]
pub(crate) struct Comment {
    pub(crate) kind: SyntaxKind,
    pub(crate) text: String,
    /// Byte offset in the input, the key [`crate::reattach`] puts it back by.
    pub(crate) offset: u32,
    /// The comment began its own source line, so it must keep one.
    pub(crate) own_line: bool,
    /// The comment ended its own source line.
    pub(crate) ends_line: bool,
    /// A blank line separated it from whatever came before.
    pub(crate) blank_before: bool,
}

impl Comment {
    /// Whether the comment runs to end of line, so anything emitted after it on
    /// the same line would be swallowed.
    pub(crate) fn is_line(&self) -> bool {
        matches!(self.kind, LINE_COMMENT | DOC_LINE_PRE | DOC_LINE_POST)
    }

    /// Whether the comment trails code and finishes the line — the 525 `//<`
    /// docs and the 35 long trailing `//` comments of `docs/spec/style.md` §5.3.
    ///
    /// These are lifted out of the document and put back after rendering, so
    /// that a 179-column doc comment cannot push the code in front of it over
    /// the line width and break a list that the author wrote on one line
    /// (§5.4: "`line_width` is a break-if-you-must threshold, and long `//<`
    /// doc comments are not lists").
    pub(crate) fn is_trailing(&self) -> bool {
        !self.own_line && self.ends_line && !self.text.contains('\n')
    }
}

/// The trivia between two significant children of one node.
#[derive(Debug, Default, Clone)]
pub(crate) struct Gap {
    pub(crate) comments: Vec<Comment>,
    /// The author put a newline somewhere in this gap.
    pub(crate) newline: bool,
    /// The author left a blank line immediately before the following element.
    pub(crate) blank: bool,
    /// The gap contains any whitespace at all — used to preserve the interior
    /// padding of an initializer list (`{ a }` vs `{a}`, `docs/spec/style.md` §10.7).
    pub(crate) space: bool,
    /// Width of the horizontal whitespace, when the gap holds no newline. Hand
    /// built tables — the `ABI` register table, the generated float32
    /// constants — pad with it, and `docs/spec/style.md` §10.3 says a run of
    /// lines sharing a column is an atom to re-emit rather than reflow.
    pub(crate) width: usize,
}

impl Gap {
    pub(crate) fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }
}

/// A node's children, split into significant elements and the trivia between.
///
/// `gaps[i]` is the trivia immediately before `elems[i]`; `gaps[elems.len()]`
/// is whatever trailed the last element. Every lowering routine walks both
/// vectors, which is how comment preservation stops being a special case: a
/// gap that is never asked about is a comment that was dropped, and the
/// verification stage in `crate::verify` catches it.
#[derive(Debug)]
pub(crate) struct Children {
    pub(crate) elems: Vec<SyntaxElement>,
    pub(crate) gaps: Vec<Gap>,
}

impl Children {
    pub(crate) fn len(&self) -> usize {
        self.elems.len()
    }

    pub(crate) fn kind(&self, i: usize) -> SyntaxKind {
        elem_kind(&self.elems[i])
    }

    /// Index of the first element of `kind`, if any.
    pub(crate) fn find(&self, kind: SyntaxKind) -> Option<usize> {
        self.elems.iter().position(|e| elem_kind(e) == kind)
    }
}

pub(crate) fn elem_kind(e: &SyntaxElement) -> SyntaxKind {
    match e {
        SyntaxElement::Node(n) => n.kind(),
        SyntaxElement::Token(t) => t.kind(),
    }
}

/// Splits `node`'s children into elements and gaps.
pub(crate) fn children(node: &SyntaxNode) -> Children {
    let mut elems = Vec::new();
    let mut gaps = Vec::new();
    let mut gap = Gap::default();

    for child in node.children_with_tokens() {
        match &child {
            SyntaxElement::Token(t) if t.kind().is_trivia() => {
                if t.kind() == WHITESPACE {
                    gap.space = true;
                    if t.text().contains('\n') {
                        gap.newline = true;
                    } else {
                        gap.width = t.text().chars().count();
                    }
                } else {
                    gap.comments.push(Comment {
                        kind: t.kind(),
                        text: t.text().to_owned(),
                        offset: u32::from(t.text_range().start()),
                        own_line: starts_line(t),
                        ends_line: ends_line(t),
                        blank_before: blank_before(t),
                    });
                }
            }
            _ => {
                gap.blank = element_blank_before(&child);
                gaps.push(std::mem::take(&mut gap));
                elems.push(child);
            }
        }
    }
    gaps.push(gap);
    debug_assert_eq!(gaps.len(), elems.len() + 1);
    Children { elems, gaps }
}

/// Whether `tok` is the first thing on its source line.
fn starts_line(tok: &SyntaxToken) -> bool {
    match tok.prev_token() {
        None => true,
        Some(prev) => prev.kind() == WHITESPACE && prev.text().contains('\n'),
    }
}

/// Whether `tok` is the last thing on its source line.
fn ends_line(tok: &SyntaxToken) -> bool {
    match tok.next_token() {
        None => true,
        Some(next) => next.kind() == WHITESPACE && next.text().contains('\n'),
    }
}

/// Whether a blank line immediately precedes `tok`.
fn blank_before(tok: &SyntaxToken) -> bool {
    match tok.prev_token() {
        None => false,
        Some(prev) => prev.kind() == WHITESPACE && newlines(prev.text()) >= 2,
    }
}

fn element_blank_before(e: &SyntaxElement) -> bool {
    first_token(e).as_ref().is_some_and(blank_before)
}

fn first_token(e: &SyntaxElement) -> Option<SyntaxToken> {
    match e {
        SyntaxElement::Token(t) => Some(t.clone()),
        SyntaxElement::Node(n) => n.first_token(),
    }
}

fn newlines(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count()
}

/// Whether `node`'s source text is free of newlines *and* of comments, i.e. the
/// author wrote it on one line and nothing forces it apart.
///
/// This is the test behind every "preserve" rule that asks whether a body was
/// collapsed (`docs/spec/style.md` §10.2): 171 bodies in the corpus are written
/// `{}` or `{ stmt; }` and normalizing them in either direction would be a
/// large diff with no majority behind it.
pub(crate) fn is_collapsed(node: &SyntaxNode) -> bool {
    !node.text().to_string().contains('\n')
        && !node.descendants_with_tokens().any(|e| match e {
            SyntaxElement::Token(t) => t.kind().is_comment(),
            SyntaxElement::Node(_) => false,
        })
}

/// Whether the subtree contains a braced block, i.e. a lambda body or a nested
/// scope that will impose its own multi-line layout.
///
/// A call whose last argument is such a thing must not have its argument list
/// broken into the leading-comma form — the corpus writes
/// `pipelined_for(16, [tag](uint8 itr)` with the lambda's own brace on the next
/// line (`docs/spec/style.md` §12, example 5).
pub(crate) fn contains_block(e: &SyntaxElement) -> bool {
    match e {
        SyntaxElement::Token(_) => false,
        SyntaxElement::Node(n) => {
            matches!(n.kind(), BLOCK_STMT | LAMBDA_EXPR)
                || n.descendants().any(|d| matches!(d.kind(), BLOCK_STMT | LAMBDA_EXPR))
        }
    }
}
