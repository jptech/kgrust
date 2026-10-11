//! [`TypeKey`] — a node of the program's text by position, kind and
//! expansion: what an IR node's `Src` names (`kg_hir::NodeData::type_key`),
//! and what instantiation's type writes are keyed by.
//!
//! The type *table* this key indexed — one type per node, the CST inference
//! engine's (`kg-cst-infer`), split out in sprint W8.3 — went with that
//! engine in sprint W3.7. The loop keys its types by node
//! (`kg_sem::TypeMap`), whose write rules were the table's, moved over
//! unchanged and tested there.

use kg_base::{Expansion, FileId, TextSize};
use kg_syntax::SyntaxKind;

/// Identifies a node within a file. Deliberately not a `SyntaxNode`: the tree is
/// rebuilt between rounds, and a stable key is what lets a round's results be
/// compared with the previous round's.
///
/// # Why `end` is here
///
/// `(file, offset, kind)` is **not** injective over a `rowan` tree. Left-nested
/// operator chains put two same-kind nodes at the same start offset:
/// `a + b + c` is `BinExpr(BinExpr(a, +, b), +, c)`, `a.b.c` is
/// `FieldExpr(FieldExpr(a, b), c)`, `a[0][1]` is `IndexExpr(IndexExpr(a, 0), 1)`.
/// Adding the end offset separates them, and no two distinct nodes of the same
/// kind share a full range — over clean *and* error-recovered trees, pinned
/// corpus-wide by `kg-parser`'s `no_two_nodes_of_one_kind_share_a_range`,
/// after one recovery path was found nesting two `ERROR` nodes of one extent.
/// # Why `via` is here
///
/// A materialised template instance keeps `written == Own` — the copy is the
/// user's own text compiled again (`kg-base/src/src.rs`, `Src::expanded`) — so
/// once pass 8's `instantiate` runs, N structurally *different* copies of one
/// body node stand at one `(file, range, kind)`. The expansion is the
/// discriminator that keeps the key injective: verified by running the
/// provenance gate itself over planted copies — red 180/180 on the four-field
/// key, green 180/180 on five, at depth 1 and 2 (`docs/journal.md`, "The
/// fifth field works…"). [`Expansion::ROOT`] for every node outside an
/// instantiated body.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct TypeKey {
    pub file: kg_base::FileId,
    /// Byte offset of the node's start.
    pub offset: TextSize,
    /// Byte offset of the node's end. See the type-level note.
    pub end: TextSize,
    pub kind: SyntaxKind,
    /// Which expansion of that text this key names. See the type-level note.
    pub via: Expansion,
}

impl TypeKey {
    /// The key of the text `range` of `file` as `kind`, in expansion `via` —
    /// the one spelling of a key built from an IR node's `Src`
    /// (`kg_hir::NodeData::type_key` for the common case).
    pub fn at(file: FileId, range: kg_base::TextRange, kind: SyntaxKind, via: Expansion) -> TypeKey {
        TypeKey { file, offset: range.start(), end: range.end(), kind, via }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_key_separates_a_left_nested_operator_chain() {
        // `a + b + c` is BinExpr(BinExpr(a,+,b), +, c); both BIN_EXPRs start at
        // `a`. A `(file, offset, kind)` key would collide and the two nodes would
        // share one type.
        let parse = kg_syntax::parse_file("inline void main() { uint8 z = a + b + c; }\n");
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let root = parse.syntax_node();
        let mut bins = root.descendants().filter(|x| x.kind() == SyntaxKind::BIN_EXPR);
        let (outer, inner) = (bins.next().expect("outer"), bins.next().expect("inner"));
        assert_eq!(outer.text_range().start(), inner.text_range().start());
        let key = |n: &kg_syntax::SyntaxNode| TypeKey::at(FileId(0), n.text_range(), n.kind(), Expansion::ROOT);
        assert_ne!(key(&outer), key(&inner), "nested same-kind nodes at the same start offset must not share a key");
    }
}
