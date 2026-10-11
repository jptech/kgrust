//! The typed AST layer, one type per rule of `grammar/kanagawa.ungram`.

mod generated;

pub use generated::*;

use crate::{SyntaxKind, SyntaxNode};

/// `node` with its own parentheses removed: the expression the reference's
/// tree holds where this one holds `( … )`. Its parser keeps no parenthesis
/// node (`parens expression` is the inner expression, `Parser.hs`), so any
/// question the reference asks of "the expression here" is asked of this.
pub fn strip_parens(mut node: SyntaxNode) -> SyntaxNode {
    while node.kind() == SyntaxKind::PAREN_EXPR {
        match node.children().next() {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

impl Expr {
    /// This expression through every pair of parentheses around it. The
    /// reference's `parens expression` (`Parser.hs:845,910-912`) returns the
    /// inner expression, so `(e)` and `e` are the same node there, and a
    /// question the reference asks of one is asked here of the stripped one.
    pub fn unparen(&self) -> Expr {
        let mut e = self.clone();
        while let Expr::ParenExpr(p) = &e {
            match p.expr() {
                Some(inner) => e = inner,
                None => break,
            }
        }
        e
    }
}

/// The parent the reference's tree gives `node`: its parent with every
/// enclosing parenthesis stepped over — the `return` of `return (f());`.
pub fn parent_through_parens(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut parent = node.parent()?;
    while parent.kind() == SyntaxKind::PAREN_EXPR {
        parent = parent.parent()?;
    }
    Some(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AstNode, SyntaxKind, parse_file};

    /// The failure mode this guards against is silent: when `kanagawa.ungram`
    /// describes a shape the parser does not build, `cargo xtask codegen`
    /// cheerfully emits an accessor for it, and that accessor returns `None` —
    /// or an empty iterator — on every input forever. Nothing fails to compile
    /// and no test goes red; callers just quietly see nothing.
    ///
    /// That is not hypothetical. `EnumDecl` claimed a `BaseList` and direct
    /// `EnumVariant` children; the parser emits `':' Type` and nests the
    /// variants in a `DeclList`. Both accessors were dead from the day they
    /// were generated, and two separate consumers worked around the symptom
    /// before anyone found the cause. `Attr::name()` was dead the same way.
    ///
    /// So: every accessor exercised below must return real data.
    fn source_file(text: &str) -> SourceFile {
        let parse = parse_file(text);
        assert!(parse.errors().is_empty(), "fixture must parse: {:?}", parse.errors());
        SourceFile::cast(parse.syntax_node()).expect("root is a SourceFile")
    }

    fn first<T: AstNode>(text: &str) -> T {
        source_file(text)
            .syntax()
            .descendants()
            .find_map(T::cast)
            .unwrap_or_else(|| panic!("no {} in {text:?}", std::any::type_name::<T>()))
    }

    #[test]
    fn an_enums_accessors_reach_its_base_type_and_its_variants() {
        let decl: EnumDecl = first("enum Colour : uint8 { Red, Green = 7, Blue }\n");

        assert_eq!(decl.name().unwrap().syntax().text(), "Colour");
        assert!(decl.colon_token().is_some());
        assert!(matches!(decl.ty(), Some(Type::IntType(_))), "base type: {:?}", decl.ty());

        let body = decl.decl_list().expect("an enum body is a DeclList");
        let variants: Vec<String> =
            body.enum_variants().map(|v| v.name().unwrap().syntax().text().to_string()).collect();
        assert_eq!(variants, ["Red", "Green", "Blue"]);

        // `Green = 7` carries a discriminant; the other two do not.
        let discriminants: Vec<bool> = body.enum_variants().map(|v| v.expr().is_some()).collect();
        assert_eq!(discriminants, [false, true, false]);
    }

    #[test]
    fn an_attributes_name_is_reachable() {
        let attr: Attr = first("[[pipelined]] inline void f() {}\n");
        assert_eq!(attr.ident_token().unwrap().text(), "pipelined");
        assert!(attr.attr_args().is_none());

        let attr: Attr = first("[[latency(3)]] inline void f() {}\n");
        assert_eq!(attr.ident_token().unwrap().text(), "latency");
        assert!(attr.attr_args().and_then(|a| a.expr()).is_some(), "latency takes an argument");
    }

    #[test]
    fn a_class_bodys_items_are_reachable() {
        let decl: ClassDecl = first("class C { uint8 x; inline void go() {} }\n");
        assert_eq!(decl.name().unwrap().syntax().text(), "C");
        let items: Vec<SyntaxKind> = decl
            .decl_list()
            .expect("a class body is a DeclList")
            .items()
            .map(|i| i.syntax().kind())
            .collect();
        assert_eq!(items, [SyntaxKind::VAR_DECL, SyntaxKind::FN_DECL]);
    }
}
