//! Names, `::` paths, template argument lists and module paths.
//!
//! The reference has five node kinds where this module has one. `IdentifierF`,
//! `ScopedNameF`, `QualifiedIdentifierF`, `TypeIdentifierF` and `NamedValueF`
//! differ only in what `findSymbol` said the name *was* (`Parser.hs:640-734`),
//! which is a resolution question, not a syntactic one. So a path is always a
//! [`PATH`] of [`PATH_SEGMENT`]s here, and lowering decides whether it names a
//! type, a value, a template or nothing at all (grammar.md §6.4 step 3).
//!
//! The one genuinely syntactic decision — is `<` an argument list or a
//! comparison — is delegated to [`lookahead::at_template_arg_list`].

use super::lookahead::AngleCtx;
use super::{ExprMode, exprs, lookahead};
use crate::{CompletedMarker, Parser, SyntaxKind, TokenSet};
use SyntaxKind::*;

/// Tokens that can begin a path.
pub(crate) const PATH_FIRST: TokenSet = TokenSet::new(&[IDENT, TEMPLATE_KW]);

/// Where a malformed template argument gives up.
const TEMPLATE_ARG_RECOVERY: TokenSet =
    TokenSet::new(&[COMMA, R_ANGLE, R_PAREN, R_BRACK, R_CURLY, SEMICOLON]);

/// A name being introduced by a declaration.
pub(crate) fn name(p: &mut Parser<'_>) {
    let m = p.start();
    if !p.eat(IDENT) {
        p.error("expected a name");
    }
    m.complete(p, NAME);
}

/// A name being used.
pub(crate) fn name_ref(p: &mut Parser<'_>) {
    let m = p.start();
    if !p.eat(IDENT) {
        p.error("expected an identifier");
    }
    m.complete(p, NAME_REF);
}

/// `scopedName = pathSegment ("::" pathSegment)*` (`Parser.hs:648-695`).
///
/// Stored flat rather than as the reference's left-nested `ScopedNameF` chain:
/// the segments are siblings and the `::` tokens sit between them, which is
/// what a formatter and a "go to definition" both want.
pub(crate) fn path(p: &mut Parser<'_>, ctx: AngleCtx) -> CompletedMarker {
    let m = p.start();
    path_segment(p, ctx);
    while p.at(COLON2) {
        p.bump(COLON2);
        path_segment(p, ctx);
    }
    m.complete(p, PATH)
}

fn path_segment(p: &mut Parser<'_>, ctx: AngleCtx) {
    let m = p.start();
    // `obj.template Go<uint32>`: `templateQualifier` is `template` *not*
    // followed by `<`, which is what tells it apart from a `template<…>`
    // header (`Parser.hs:646`).
    if at_template_qualifier(p) {
        p.bump(TEMPLATE_KW);
    }
    if p.at(IDENT) {
        name_ref(p);
    } else {
        p.error("expected an identifier");
    }
    if lookahead::at_template_arg_list(p, ctx) {
        template_arg_list(p);
    }
    m.complete(p, PATH_SEGMENT);
}

pub(crate) fn at_template_qualifier(p: &Parser<'_>) -> bool {
    p.at(TEMPLATE_KW) && !p.nth_at_angle(1, L_ANGLE)
}

/// `templateArgs = "<" (templateArg ("," templateArg)*)? ">"` (`Parser.hs:697`).
///
/// `sepBy`, so `Foo<>` is legal — unlike a template *parameter* list, which is
/// `paramList` and needs at least one (`Parser.hs:361`, grammar.md §9 trap 23).
///
/// Each argument is `try templateSpecifier <|> expressionTemplate`; both
/// collapse to one syntactic form here, because a template specifier is just a
/// path and a path is already an expression term. The mode is
/// [`ExprMode::TemplateArg`], which is what removes `<`, `>`, `<<` and `>>`
/// from the operator table and so lets the list terminate (`Parser.hs:913`).
pub(crate) fn template_arg_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_ANGLE);
    // Edition 1.1's named template arguments: `IDENT ':'` begins no
    // reference template argument (`::` is one token, and a ternary's `:`
    // follows its `?`), so a label is a parse error in 1.0.
    let named = p.has(kg_base::language::Feature::NamedTemplateArguments);
    let mut labels = Vec::new();
    if !p.at_angle(R_ANGLE) {
        loop {
            let pos = p.token_pos();
            if named {
                exprs::arg_label(p, &mut labels, R_ANGLE);
            }
            exprs::expr(p, ExprMode::TemplateArg);
            if p.token_pos() == pos {
                p.err_recover("expected a template argument", TEMPLATE_ARG_RECOVERY);
                if p.token_pos() == pos {
                    break;
                }
            }
            if !p.eat(COMMA) || p.at_trailing(R_ANGLE) {
                break;
            }
        }
    }
    if p.at_angle(R_ANGLE) {
        p.bump(R_ANGLE);
    } else {
        p.error("expected `>`");
    }
    m.complete(p, TEMPLATE_ARG_LIST);
}

// -------------------------------------------------------------- module paths

/// `moduleName = ".cmdargs" | ".options" | dotSepName` (`Parser.hs:179-183`).
///
/// `dotSepName` (`Parser/Lexer.hs:230-233`) is a *single lexeme* whose
/// components may contain `-`, so `intel.arria-10.config` is one name that this
/// lexer necessarily splits into `IDENT . IDENT - INT_NUMBER . IDENT`. It is
/// reassembled here structurally rather than by adjacency, because a component
/// may also be a reserved word — `numeric.float32.operator` (`library/base.k:44`)
/// contains the `float32` keyword — while the `as` that may follow an import
/// must not be swallowed.
///
/// And it is a lexeme of *characters*: `try (ident' … \`sepBy1\` char '.')`,
/// no space anywhere inside. So a component, its `-` parts and the dots
/// between components must be glued; `import a1 .a1` imports `a1` and leaves
/// `.a1` behind, and `import a1. a1` fails the whole name at its start — the
/// `try` backtracks there — "expecting module name" (`imports.k::117-119`,
/// `kg-oracle --errors`).
pub(crate) fn module_path(p: &mut Parser<'_>) {
    let m = p.start();
    // The two synthetic modules are spelled with a leading dot
    // (`Parser.hs:41-45`).
    let dotted = p.at(DOT);
    if dotted && !(p.nth_glued(0) && at_nth_component_start(p, 1)) {
        p.error("expected a module name");
        p.bump(DOT);
        m.complete(p, MODULE_PATH);
        return;
    }
    p.eat(DOT);
    if !at_component_start(p) {
        p.error("expected a module name");
        m.complete(p, MODULE_PATH);
        return;
    }
    if !dotted && name_breaks_after_a_dot(p) {
        p.error("expected a module name");
    }
    component(p);
    // A dot joins the next component only glued on both sides.
    while p.at(DOT) && glued_before(p) && p.nth_glued(0) && at_nth_component_start(p, 1) {
        p.bump(DOT);
        component(p);
    }
    m.complete(p, MODULE_PATH);
}

/// Whether the current token is adjacent to the one before it.
fn glued_before(p: &Parser<'_>) -> bool {
    p.token_pos() > 0 && p.nth_glued_back()
}

/// Whether the dotted name starting here has a dot glued to its component
/// with no component glued after it — the name `dotSepName`'s `try` rejects
/// whole. Read ahead without consuming, since the error belongs at the start.
fn name_breaks_after_a_dot(p: &Parser<'_>) -> bool {
    let mut n = 0;
    loop {
        // One component: its first token, then glued `-` parts.
        n += 1;
        while p.nth(n) == MINUS && p.nth_glued(n - 1) && p.nth_glued(n) && at_nth_component_part(p, n + 1) {
            n += 2;
            while at_nth_component_part(p, n) && p.nth_glued(n - 1) {
                n += 1;
            }
        }
        if p.nth(n) != DOT || !p.nth_glued(n - 1) {
            return false;
        }
        if !(p.nth_glued(n) && at_nth_component_start(p, n + 1)) {
            return true;
        }
        n += 1;
    }
}

fn component(p: &mut Parser<'_>) {
    p.bump_any();
    // `-` is a component character, so anything glued after one is still part
    // of this component.
    while p.at(MINUS) && glued_before(p) && p.nth_glued(0) && at_nth_component_part(p, 1) {
        p.bump(MINUS);
        p.bump_any();
        // `041b` lexes as a number and a name; glued, they are one part.
        while at_component_part(p) && glued_before(p) {
            p.bump_any();
        }
    }
}

/// A component must start the way an identifier does: a letter or `_`.
fn at_component_start(p: &Parser<'_>) -> bool {
    at_nth_component_start(p, 0)
}

fn at_nth_component_start(p: &Parser<'_>, n: usize) -> bool {
    let kind = p.nth(n);
    kind == IDENT || kind == INT_TYPE_TOKEN || kind == UINT_TYPE_TOKEN || kind.is_keyword()
}

fn at_component_part(p: &Parser<'_>) -> bool {
    at_nth_component_part(p, 0)
}

/// What may follow a `-` inside a component. Reserved words are excluded so
/// that `import a.b-c as d` cannot swallow the `as`.
fn at_nth_component_part(p: &Parser<'_>, n: usize) -> bool {
    matches!(p.nth(n), IDENT | INT_NUMBER | INT_TYPE_TOKEN | UINT_TYPE_TOKEN)
}
