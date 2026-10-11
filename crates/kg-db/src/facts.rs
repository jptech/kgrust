//! Syntactic facts the passes share.
//!
//! Everything here is a **pure function of one tree** — no environment, no
//! symbol table, no file system. That is the membership rule: a helper earns a
//! place in this module when two owners would otherwise copy it, and only when
//! its answer cannot change once the text is fixed.
//!
//! Anything two passes need in common belongs here rather than in whichever
//! pass happened to need it first. The constant environment that shared
//! most of what was here — `static if` decisions, cast targets, enum
//! variants — went with the CST inference engine (sprint W3.7).

use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, ast};

/// Whether the reference's inference traversal reaches `node` only after an
/// earlier pass has elaborated something away — in which case judging it at
/// the text would diagnose code the reference discards unread.
///
/// Two exclusions, both straight out of `Inference.hs:52-68`, whose descent is
/// deliberately incomplete:
///
/// * **`StaticIfF` — *only the condition* is visited** (`Inference.hs:64`). A
///   branch becomes reachable only once `staticControl` has replaced the
///   `static if` with the taken branch, so the untaken one is deleted before
///   anything folds it. The condition itself is visited from round one, which
///   is what makes the elaboration possible at all — so it is *not* excluded.
/// * **`TemplateF` — only `templateParams` are visited, never `templateDecl`**
///   (`Inference.hs:63`). A template body is folded on its *instances*, which
///   pass 8 creates; the definition is deleted by `removeTemplate` at pass 15
///   (`Frontend.hs:298-299`), so an uninstantiated body is never read. A
///   parameter's default argument is in `templateParams` and is not excluded.
///
/// The walk tracks which child of each ancestor it came through, because both
/// rules are about *which* subtree of the node is visited, not about the node.
pub(crate) fn reached_only_after_elaboration(node: &SyntaxNode) -> bool {
    let mut child = node.clone();
    for ancestor in node.ancestors().skip(1) {
        let visited = match ancestor.kind() {
            SyntaxKind::STATIC_IF_STMT => ast::StaticIfStmt::cast(ancestor.clone())
                .and_then(|s| s.cond())
                .is_some_and(|c| c.syntax() == &child),
            SyntaxKind::TEMPLATE_DECL => child.kind() == SyntaxKind::TEMPLATE_PARAM_LIST,
            _ => true,
        };
        if !visited {
            return true;
        }
        child = ancestor;
    }
    false
}

/// The innermost declaration of `name` around `from` — a `PARAM` or a
/// `VAR_DECL`, found by walking outward one enclosing node at a time and
/// looking at what each declares directly. `None` for a name declared
/// somewhere this walk does not look: a global in another file, an enum
/// constant, a template parameter.
pub(crate) fn innermost_declaration(from: &SyntaxNode, name: &str) -> Option<SyntaxNode> {
    let named = |n: Option<ast::Name>| n.and_then(|n| n.ident_token()).is_some_and(|t| t.text() == name);
    for ancestor in from.ancestors().skip(1) {
        let declares: Vec<SyntaxNode> = ancestor
            .children()
            .flat_map(|c| match c.kind() {
                SyntaxKind::PARAM_LIST => c.children().collect::<Vec<_>>(),
                _ => vec![c],
            })
            .collect();
        for node in declares {
            let hit = match node.kind() {
                SyntaxKind::PARAM => ast::Param::cast(node.clone()).is_some_and(|p| named(p.name())),
                SyntaxKind::VAR_DECL => {
                    ast::VarDecl::cast(node.clone()).is_some_and(|v| named(v.name()))
                }
                _ => false,
            };
            if hit {
                return Some(node);
            }
        }
    }
    None
}
