//! Which `name <` the context-free parse read as a template argument list
//! although `name` is provably a value.
//!
//! The reference decides `<` after a name with its symbol table, while
//! parsing: a template argument list only when `findSymbol` returns a
//! template (`prefixedScopedNameAndSymbol`, `Parser.hs:652-690`); a value, or
//! nothing at all, and `<` is the comparison. `kg_parser` has no symbol table
//! and decides by an angle scan instead (`grammar/lookahead.rs`), which reads
//! a region that balances as a list. That is right wherever a template could
//! stand, and wrong in exactly one shape: two comparisons side by side in an
//! argument list whose `>` is followed by something a binary operator can
//! take — `g(x < y, z > -1)`, `g(x < y, z > (y))` — which the scan reads as
//! `g(x<y, z> - 1)`, one argument (`docs/history/audit-2026-09-22.md`, D1).
//!
//! The parse's own tree already holds the evidence the reference would have
//! consulted for the commonest heads: a parameter, a local declared earlier
//! in an enclosing block, a value template parameter, a variable declared
//! earlier at the top of the file. None of those can be a template, so
//! `findSymbol` on such a head returns a non-template and the reference reads
//! a comparison. [`value_headed_angles`] lists those `<`s; the caller marks
//! them and parses again.
//!
//! Only proven values are listed. A head this cannot see — an import, a class
//! member, a name declared later — keeps the scan's reading, so the re-read
//! can only move a parse toward the reference's, never away from it.

use kg_base::TextSize;

use crate::{SyntaxKind, SyntaxNode};
use SyntaxKind::*;

/// Start offsets of every `<` the tree reads as opening a template argument
/// list after a single-segment expression path whose name is bound, where
/// the path stands, as a value.
pub(crate) fn value_headed_angles(root: &SyntaxNode) -> Vec<TextSize> {
    let mut out = Vec::new();
    for list in root.descendants().filter(|n| n.kind() == TEMPLATE_ARG_LIST) {
        let Some(segment) = list.parent().filter(|s| s.kind() == PATH_SEGMENT) else { continue };
        let Some(path) = segment.parent().filter(|p| p.kind() == PATH) else { continue };
        // A type position (`PATH_TYPE`) is where a list belongs; inside a
        // list, `<` is no operator at all (`opsTemplate`, `Parser.hs:913`),
        // so a comparison there would be a parse error the reference does
        // not make either.
        if path.parent().is_none_or(|p| matches!(p.kind(), PATH_TYPE | TEMPLATE_ARG_LIST)) {
            continue;
        }
        // One segment, spelled without `template`: `a::b<…>` and
        // `template b<…>` name something this tree cannot bind.
        if path.children().filter(|c| c.kind() == PATH_SEGMENT).count() != 1
            || segment.children_with_tokens().any(|e| e.kind() == TEMPLATE_KW)
        {
            continue;
        }
        let Some(name) = segment
            .children()
            .find(|c| c.kind() == NAME_REF)
            .and_then(|n| n.children_with_tokens().find(|e| e.kind() == IDENT))
            .and_then(|e| e.into_token())
        else {
            continue;
        };
        let Some(angle) = list.children_with_tokens().find(|e| e.kind() == L_ANGLE) else { continue };
        if bound_as_value(&path, name.text()) {
            out.push(angle.text_range().start());
        }
    }
    out
}

/// Whether `name`, read at `at`, is bound by a declaration that makes it a
/// value: the innermost binding of `name` the tree shows is a parameter, a
/// value template parameter or a variable declared before `at`.
///
/// Innermost first, and the first binding found decides — a local `x`
/// shadows a file-level one, as `findSymbol`'s scope walk does
/// (`SymbolTable.hs:229-250`). A declaration that binds `name` as anything
/// else (a function, a class, a template) stops the walk with `false`.
fn bound_as_value(at: &SyntaxNode, name: &str) -> bool {
    let start = at.text_range().start();
    for scope in at.ancestors().skip(1) {
        match scope.kind() {
            // Parameters bind in the body only; `at` is below this node, so
            // it is in the body, the parameter list or a default — all
            // after the parameter it could name.
            FN_DECL | LAMBDA_EXPR => {
                let params = scope.children().filter(|c| c.kind() == PARAM_LIST);
                if params.flat_map(|l| l.children()).any(|p| p.kind() == PARAM && declares(&p, name)) {
                    return true;
                }
            }
            TEMPLATE_DECL => {
                for param in scope
                    .children()
                    .filter(|c| c.kind() == TEMPLATE_PARAM_LIST)
                    .flat_map(|l| l.children())
                {
                    if declares(&param, name) {
                        return param.kind() == VALUE_PARAM;
                    }
                }
            }
            BLOCK_STMT | FOR_STMT | SOURCE_FILE => {
                if let Some(value) = declared_before(&scope, name, start) {
                    return value;
                }
            }
            _ => {}
        }
    }
    false
}

/// The verdict of the last declaration of `name` among `scope`'s children
/// that ends before `start`: `Some(true)` for a variable, `Some(false)` for
/// anything else that declares the name, `None` for no declaration.
fn declared_before(scope: &SyntaxNode, name: &str, start: TextSize) -> Option<bool> {
    let mut verdict = None;
    for child in scope.children() {
        if child.text_range().end() > start {
            break;
        }
        match child.kind() {
            VAR_DECL if declares(&child, name) => verdict = Some(true),
            FN_DECL | CLASS_DECL | STRUCT_DECL | UNION_DECL | ENUM_DECL | TEMPLATE_DECL | ALIAS_DECL
                if declares_anywhere(&child, name) =>
            {
                verdict = Some(false);
            }
            _ => {}
        }
    }
    verdict
}

/// Whether `decl`'s own `NAME` child is `name`.
fn declares(decl: &SyntaxNode, name: &str) -> bool {
    decl.children()
        .filter(|c| c.kind() == NAME)
        .any(|n| n.text() == name)
}

/// [`declares`] for a declaration that may wrap the named one — a template
/// over a class, say.
fn declares_anywhere(decl: &SyntaxNode, name: &str) -> bool {
    declares(decl, name) || decl.children().any(|c| declares(&c, name))
}

#[cfg(test)]
mod tests {
    use crate::{SyntaxKind, parse_file};

    /// The number of arguments of the one call to `g` in `text`, and whether
    /// the tree still holds any template argument list.
    fn g_args(text: &str) -> (usize, bool) {
        let parse = parse_file(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let root = parse.syntax_node();
        let call = root
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::CALL_EXPR)
            .find(|c| c.first_child().is_some_and(|p| p.text() == "g"))
            .expect("a call to g");
        let args = call.children().find(|c| c.kind() == SyntaxKind::ARG_LIST).expect("arguments");
        let lists = root.descendants().any(|n| n.kind() == SyntaxKind::TEMPLATE_ARG_LIST);
        (args.children().count(), lists)
    }

    /// `docs/history/audit-2026-09-22.md` D1, both shapes: `kg-oracle --errors` accepts each, reading
    /// two comparisons because `x` is a parameter (`Parser.hs:652-690`).
    #[test]
    fn a_parameter_before_an_angle_is_a_comparison() {
        for tail in ["-1", "(y)"] {
            let text = format!("inline bool h(uint8 x, uint8 y, uint8 z) {{ return g(x < y, z > {tail}); }}");
            assert_eq!(g_args(&text), (2, false), "{text}");
        }
    }

    #[test]
    fn a_local_declared_earlier_is_a_value_too() {
        let text = "void h(uint8 y, uint8 z) { uint8 x = 1; g(x < y, z > -1); }";
        assert_eq!(g_args(text), (2, false));
        let text = "const uint8 x = 1;\nvoid h(uint8 y, uint8 z) { g(x < y, z > -1); }";
        assert_eq!(g_args(text), (2, false));
        let text = "template <auto x> void h(uint8 y, uint8 z) { g(x < y, z > -1); }";
        assert_eq!(g_args(text), (2, false));
    }

    /// A head the tree cannot prove a value keeps the scan's reading: a
    /// template may stand there, and only the symbol table would know.
    #[test]
    fn an_unbound_or_template_head_keeps_the_list() {
        let text = "void h(uint8 y, uint8 z) { g(x < y, z > -1); }";
        assert_eq!(g_args(text), (1, true));
        let text = "template <typename A, typename B> class x {}\nvoid h() { uint8 x; g(x < uint8, uint8 > -1); }";
        // The local `x` is declared earlier and shadows the class.
        assert_eq!(g_args(text), (2, false));
        let text = "void h(uint8 y, uint8 z) { g(x < y, z > -1); uint8 x; }";
        assert_eq!(g_args(text), (1, true), "a local declared later binds nothing yet");
        let text = "template <typename x> void h(uint8 y, uint8 z) { g(x < y, z > -1); }";
        assert_eq!(g_args(text), (1, true), "a type parameter is no value");
    }
}
