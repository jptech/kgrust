//! The Kanagawa syntax tree.
//!
//! A thin typed layer over `rowan`: [`SyntaxNode`] is an untyped, lossless,
//! immutable tree node, and [`ast`] gives every grammar production a Rust type
//! that wraps one. The wrappers are generated from `grammar/kanagawa.ungram` by
//! `cargo xtask codegen`; nothing in this crate is hand-written except the
//! language binding and the two traits below.

pub mod ast;
mod doc_comments;
mod parse;
mod reread;

pub use doc_comments::{Doc, FileDocs, Headings, file_docs, to_commonmark};
pub use parse::{Parse, ParseOptions, SyntaxError, parse_and_check_lossless, parse_file, parse_file_with};

pub use kg_parser::SyntaxKind;
pub use rowan::{NodeOrToken, WalkEvent};

/// The `rowan` language binding: a bijection between [`SyntaxKind`] and
/// `rowan`'s untyped `u16` kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KanagawaLanguage {}

impl rowan::Language for KanagawaLanguage {
    type Kind = SyntaxKind;

    #[inline]
    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        // Out-of-range only happens if a tree was built by something that is
        // not this workspace's parser; degrading to `ERROR` keeps the tree
        // walkable instead of panicking deep inside an accessor.
        SyntaxKind::try_from(raw.0).unwrap_or(SyntaxKind::ERROR)
    }

    #[inline]
    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind.into())
    }
}

/// A node in the lossless syntax tree.
pub type SyntaxNode = rowan::SyntaxNode<KanagawaLanguage>;
/// A token in the lossless syntax tree.
pub type SyntaxToken = rowan::SyntaxToken<KanagawaLanguage>;
/// Either of the two.
pub type SyntaxElement = rowan::SyntaxElement<KanagawaLanguage>;

/// A typed view of a [`SyntaxNode`] of one particular kind.
///
/// Casting is a checked view, not a conversion: it borrows nothing, allocates
/// nothing, and fails only when the kind does not match.
pub trait AstNode {
    /// Whether [`AstNode::cast`] would succeed for a node of this kind.
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;

    /// Views `syntax` as this node type, if its kind allows.
    fn cast(syntax: SyntaxNode) -> Option<Self>
    where
        Self: Sized;

    /// The underlying untyped node.
    fn syntax(&self) -> &SyntaxNode;
}

/// A typed view of a [`SyntaxToken`] of one particular kind.
pub trait AstToken {
    /// Whether [`AstToken::cast`] would succeed for a token of this kind.
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;

    /// Views `syntax` as this token type, if its kind allows.
    fn cast(syntax: SyntaxToken) -> Option<Self>
    where
        Self: Sized;

    /// The underlying untyped token.
    fn syntax(&self) -> &SyntaxToken;

    /// The token's source text.
    fn text(&self) -> &str {
        self.syntax().text()
    }
}

/// The range of `node` without its leading and trailing trivia.
///
/// The tree attaches a comment to the node that follows it, so a
/// declaration's `text_range` can begin at a `//` several lines up. The
/// reference's notes never do — `getOffset` is taken after `whitespace` has
/// run (`Parser/Lexer.hs`) — so a diagnostic "at the declaration" that copied
/// the node range would point at the comment. Every report site that means
/// "where this declaration starts" goes through here.
pub fn trimmed_range(node: &SyntaxNode) -> kg_base::TextRange {
    let mut start = node.text_range().start();
    let mut end = node.text_range().end();
    let mut tok = node.first_token();
    while let Some(t) = tok {
        if !t.kind().is_trivia() {
            start = t.text_range().start();
            break;
        }
        if t.text_range().end() >= end {
            break;
        }
        tok = t.next_token();
    }
    let mut tok = node.last_token();
    while let Some(t) = tok {
        if !t.kind().is_trivia() {
            end = t.text_range().end();
            break;
        }
        if t.text_range().start() <= start {
            break;
        }
        tok = t.prev_token();
    }
    kg_base::TextRange::new(start, end.max(start))
}

/// Whether `node` is a well-formed `default = e;` — `defaultInitialization`
/// (`Parser.hs:314`), `static`-prefixed in a statement list (`:475`).
///
/// The frozen [`SyntaxKind`] vocabulary has no `DEFAULT_INIT`, so the parser
/// wraps one in an `ERROR` node without registering a message
/// (`kg_parser::grammar::items::default_init`). It is no error: every
/// consumer that reads `ERROR` as the reference's `ErrorF` asks this first.
/// A malformed one — no expression, no `;` — stays an error.
pub fn is_default_init(node: &SyntaxNode) -> bool {
    if node.kind() != SyntaxKind::ERROR {
        return false;
    }
    let mut tokens = node.children_with_tokens().filter(|e| !e.kind().is_trivia()).map(|e| e.kind());
    let mut first = tokens.next();
    if first == Some(SyntaxKind::STATIC_KW) {
        first = tokens.next();
    }
    first == Some(SyntaxKind::DEFAULT_KW)
        && tokens.next() == Some(SyntaxKind::EQ)
        && tokens.next().is_some_and(ast::Expr::can_cast)
        && tokens.next() == Some(SyntaxKind::SEMICOLON)
        && tokens.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_error(text: &str) -> Option<SyntaxNode> {
        parse_file(text).syntax_node().descendants().find(|n| n.kind() == SyntaxKind::ERROR)
    }

    #[test]
    fn a_default_initialization_is_recognised_and_a_malformed_one_is_not() {
        for text in [
            "class C\n{\n    default = { .x = 1 };\n}\n",
            "inline void F()\n{\n    static default = { .x = 1 };\n}\n",
        ] {
            assert!(is_default_init(&first_error(text).expect("an ERROR node")), "{text}");
        }
        assert!(!is_default_init(&first_error("class C\n{\n    default = ;\n}\n").expect("an ERROR node")));
    }
}
