//! `evaluateConstexpr` over a *width*: the expression inside `uint<e>` or
//! `int<e>`, folded to the literal pass 6 would have left there
//! (`Constexpr.hs:50`, `Frontend.hs:69`).
//!
//! Pass 6 runs before pass 8, so the reference reaches `instantiateTemplates`
//! with `uint<bitsizeof T>` already `uint32` in a copy — one instance of
//! `reinterpret_cast<uint32>` however the width was written
//! (`library/test/internal/assert.k:59`, reached by every `test/logic`
//! program through `assert_equal`). This port folds inside the type table,
//! not in the IR, so the tree still spells the expression and the mangler,
//! which reads the tree, had no arm for it: the copy was declined and never
//! made.
//!
//! What folds here is what a *closed* width can be: a literal, the arithmetic
//! `kg-const` folds over literals, and `bitsizeof`/`bytesizeof` of a written
//! built-in type — everything a substituted `bitsizeof T` becomes. A width
//! naming anything else (a constant, a struct, a template parameter still
//! unbound) is `None`, and its occurrence declines as it did before: the
//! table is where those are decided, and reaching for it here would key one
//! instance two ways.

use kg_base::Interner;
use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;

/// The width `e` folds to, or `None` where this port cannot fold it without
/// a type table — [`crate::constexpr::width`] over what the subtree spells.
pub fn folded_width(arena: &ExpArena, e: ExpId) -> Option<u32> {
    crate::constexpr::width(arena, e, &crate::constexpr::Closed)
}

/// The mangled spelling of a folded width type, for a caller that has the
/// arena and not the interner (`_` keeps the signature honest).
pub fn mangled_param_int(arena: &ExpArena, id: ExpId, _: &Interner) -> Option<String> {
    let (signed, width) = match &arena[id].exp {
        Exp::ParamInt(w) => (true, *w),
        Exp::ParamUint(w) => (false, *w),
        _ => return None,
    };
    let w = folded_width(arena, width)?;
    Some(if signed { format!("$int@{w}") } else { format!("$uint@{w}") })
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    use super::*;
    use crate::sort::NoImports;

    /// The width of the `k`-th `uint<…>`/`int<…>` written in `text`.
    fn width(text: &str, k: usize) -> Option<u32> {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = crate::lower_sorted(&file, FileId(0), &interner, &NoImports);
        let widths: Vec<ExpId> = ir
            .arena
            .iter()
            .filter_map(|(_, n)| match n.exp {
                Exp::ParamInt(w) | Exp::ParamUint(w) => Some(w),
                _ => None,
            })
            .collect();
        folded_width(&ir.arena, *widths.get(k)?)
    }

    /// What a *closed* width folds to — the shape a copy's `uint<bitsizeof T>`
    /// becomes once `T` is substituted, which pass 6 would have folded before
    /// pass 8 mangled it (`Constexpr.hs:50`). A width naming a parameter or a
    /// constant stays unfolded: the table decides those, and keying one
    /// instance two ways is worse than declining it. Red with the arithmetic
    /// dropped, and red with `bitsizeof` of a built-in unread.
    #[test]
    fn a_closed_width_folds_to_its_literal() {
        let text = "template <typename T>\ninline void F(uint<bitsizeof uint32> a, uint<bitsizeof T> b, uint<bitsizeof uint5 + 2> c, int<32 / 4> d, uint<bitsizeof bool> e)\n{\n}\n";
        assert_eq!(width(text, 0), Some(32), "bitsizeof of a built-in");
        assert_eq!(width(text, 1), None, "a template parameter is the table's");
        assert_eq!(width(text, 2), Some(7), "arithmetic over it");
        assert_eq!(width(text, 3), Some(8), "a constant expression");
        assert_eq!(width(text, 4), Some(1), "`bool` is one bit (`Type.hs:705`)");
    }
}
