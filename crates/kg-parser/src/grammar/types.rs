//! Types.
//!
//! `typ = typ' True` (`Parser.hs:515-557`), a four-level cascade whose shape is
//! entirely about where `const`, attributes and array dimensions may appear:
//!
//! ```text
//! typ    ::= "void" | "const" type1 | type1
//! type1  ::= "auto" | memoryAttr type3 "[" e "]" | functionType | type2
//! type2  ::= type3 ( "[" e "]" )*
//! type3  ::= bool | float32 | intN | uintN | int<e> | uint<e>
//!          | path | "typename" path | decltype "(" e ")" | string
//! ```
//!
//! Three consequences worth stating outright, because they are easy to get
//! wrong: `const` binds at the outermost level only (`Parser.hs:519`); an
//! attributed array takes exactly **one** dimension while a plain one takes any
//! number (`Parser.hs:529` vs `:533-538`, grammar.md Q1); and a function type
//! may not appear as a function type's parameter or return type, because both
//! route through `type2` (`Parser.hs:554, 557`).
//!
//! There is no `typeSpecifier` here. The reference decides whether a path names
//! a type by asking `findSymbol` (`Parser.hs:712-726`); this parser emits a
//! [`PATH_TYPE`] whenever the *position* wants a type and a plain [`PATH`]
//! whenever it wants a value, and lowering reconciles the two.

use super::lookahead::{self, AngleCtx};
use super::{ExprMode, attrs, exprs, paths};
use crate::{CompletedMarker, Parser, SyntaxKind, TokenSet};
use SyntaxKind::*;

/// Tokens that can begin a type without any lookahead.
///
/// `L_BRACK` (a memory-attributed array) and `L_PAREN` (a function type) are
/// deliberately absent: both need a scan to tell them from an attributed
/// statement and a parenthesized expression respectively.
pub(crate) const TYPE_FIRST: TokenSet = TokenSet::new(&[
    VOID_KW,
    CONST_KW,
    AUTO_KW,
    BOOL_KW,
    FLOAT32_KW,
    STRING_KW,
    INT_KW,
    UINT_KW,
    INT_TYPE_TOKEN,
    UINT_TYPE_TOKEN,
    DECLTYPE_KW,
    TYPENAME_KW,
    IDENT,
]);

const TYPE_RECOVERY: TokenSet = TokenSet::new(&[
    SEMICOLON,
    COMMA,
    R_PAREN,
    R_BRACK,
    R_CURLY,
    R_ANGLE,
    L_CURLY,
    EQ,
    COLON,
    THIN_ARROW,
]);

/// Tokens that may appear at the top level of a function type's parameter
/// list — that is, outside any `<…>`, `[…]` or `decltype(…)`.
///
/// `ftParam = attributeSeq? ftParamType identifier?` and
/// `ftParamType = "const" type2 | "auto" | "string" | type2` (`Parser.hs:553-554`),
/// so a parameter is a type and an optional name and nothing else. Anything
/// outside this set — an operator, a literal, a `(` that is not `decltype`'s —
/// settles the question at that token instead of at the matching `)`, which is
/// what keeps nested parentheses linear.
const IN_FN_TYPE_PARAMS: TokenSet = TokenSet::new(&[
    COMMA,
    COLON2,
    IDENT,
    TEMPLATE_KW,
    TYPENAME_KW,
    CONST_KW,
    AUTO_KW,
    VOID_KW,
    BOOL_KW,
    FLOAT32_KW,
    STRING_KW,
    INT_KW,
    UINT_KW,
    INT_TYPE_TOKEN,
    UINT_TYPE_TOKEN,
]);

/// `typ` (`Parser.hs:517-521`).
pub(crate) fn type_(p: &mut Parser<'_>) -> CompletedMarker {
    // Types and expressions are mutually recursive; see [`super::depth`].
    let Some(_guard) = super::depth::enter() else {
        let m = p.start();
        p.error(super::depth::TOO_DEEP);
        if !p.at_ts(TYPE_RECOVERY) && !p.at_end() {
            p.bump_any();
        }
        return m.complete(p, ERROR);
    };
    match p.current() {
        VOID_KW => {
            let m = p.start();
            p.bump(VOID_KW);
            m.complete(p, VOID_TYPE)
        }
        CONST_KW => {
            let m = p.start();
            p.bump(CONST_KW);
            type1(p);
            m.complete(p, CONST_TYPE)
        }
        _ => type1(p),
    }
}

/// `type'` (`Parser.hs:528-531`).
fn type1(p: &mut Parser<'_>) -> CompletedMarker {
    match p.current() {
        AUTO_KW => {
            let m = p.start();
            p.bump(AUTO_KW);
            return m.complete(p, AUTO_TYPE);
        }
        // A leading `[[…]]` is either the memory attributes of an array
        // (`Parser.hs:529`) or the attributes of a function type (`:551`). The
        // two permitted lists are disjoint, so one scan each decides it.
        L_BRACK if p.nth_at(1, L_BRACK) => {
            if attrs::at_attrs(p, attrs::MEMORY_ATTRS) {
                let m = p.start();
                attrs::opt_attrs(p, attrs::MEMORY_ATTRS);
                type3(p);
                // `Seq . pure <$> brackets expression` — singular, unlike the
                // unattributed form. grammar.md Q1.
                if p.at(L_BRACK) {
                    array_dim(p);
                } else {
                    p.error("expected `[` — an attributed array needs a dimension");
                }
                return m.complete(p, ARRAY_TYPE);
            }
            if attrs::at_attrs(p, attrs::FN_TYPE_ATTRS) {
                let m = p.start();
                attrs::opt_attrs(p, attrs::FN_TYPE_ATTRS);
                return fn_type_rest(p, m);
            }
            // Neither list matches. `optionalAttributes` yields nothing and the
            // type parse fails right here, at the `[` (grammar.md §9 trap 4).
        }
        L_PAREN if at_fn_type(p) => {
            let m = p.start();
            return fn_type_rest(p, m);
        }
        _ => {}
    }
    type2(p)
}

/// `type''` (`Parser.hs:533-538`): a base type and any number of dimensions,
/// collected into one [`ARRAY_TYPE`] the way the reference collects them into
/// one `Seq`.
fn type2(p: &mut Parser<'_>) -> CompletedMarker {
    let base = type3(p);
    if !at_array_dim(p) {
        return base;
    }
    let m = base.precede(p);
    while at_array_dim(p) {
        array_dim(p);
    }
    m.complete(p, ARRAY_TYPE)
}

/// `[[` opens an attribute group, never a dimension.
fn at_array_dim(p: &Parser<'_>) -> bool {
    p.at(L_BRACK) && !p.nth_at(1, L_BRACK)
}

fn array_dim(p: &mut Parser<'_>) {
    p.bump(L_BRACK);
    exprs::expr(p, ExprMode::Full);
    p.expect(R_BRACK);
}

/// `type'''` (`Parser.hs:540-549`).
fn type3(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    match p.current() {
        BOOL_KW => {
            p.bump(BOOL_KW);
            m.complete(p, BOOL_TYPE)
        }
        FLOAT32_KW => {
            p.bump(FLOAT32_KW);
            m.complete(p, FLOAT_TYPE)
        }
        STRING_KW => {
            p.bump(STRING_KW);
            m.complete(p, STRING_TYPE)
        }
        // `int32` / `uint8`: one lexeme, because `intSuffix` follows `string
        // "int"` with no whitespace allowed (`Parser.hs:542-543`).
        INT_TYPE_TOKEN | UINT_TYPE_TOKEN => {
            p.bump_any();
            m.complete(p, INT_TYPE)
        }
        // `int<e>` / `uint<e>` (`Parser.hs:544-545`). Bare `int` is not a type.
        INT_KW | UINT_KW => {
            p.bump_any();
            if p.at_angle(L_ANGLE) {
                p.bump(L_ANGLE);
                exprs::expr(p, ExprMode::TemplateArg);
                if p.at_angle(R_ANGLE) {
                    p.bump(R_ANGLE);
                } else {
                    p.error("expected `>`");
                }
            } else {
                p.error("expected `<` — `int` and `uint` need a width");
            }
            m.complete(p, PARAM_INT_TYPE)
        }
        // The C++ disambiguator: `typename X` is a type even when `X` cannot be
        // resolved (`Parser.hs:547`).
        TYPENAME_KW => {
            p.bump(TYPENAME_KW);
            if p.at_ts(paths::PATH_FIRST) {
                path_type(p);
            } else {
                p.error("expected a type name after `typename`");
            }
            m.complete(p, TYPENAME_TYPE)
        }
        DECLTYPE_KW => {
            p.bump(DECLTYPE_KW);
            p.expect(L_PAREN);
            exprs::expr(p, ExprMode::Full);
            p.expect(R_PAREN);
            m.complete(p, DECLTYPE_TYPE)
        }
        IDENT | TEMPLATE_KW => {
            paths::path(p, AngleCtx::Type);
            m.complete(p, PATH_TYPE)
        }
        _ => {
            // Not `err_recover`: that wraps the token it consumes in its own
            // `ERROR`, and completing `m` around it nested two `ERROR` nodes of
            // one extent — the only shape in the corpus on which
            // `kg_ty::TypeKey`'s "no two nodes of one kind share a
            // range" was false. The node here is the *type slot*, kept even
            // when nothing is consumed, so the token goes straight into it.
            p.error("expected a type");
            if !p.at_ts(TYPE_RECOVERY) && !p.at_end() {
                p.bump_any();
            }
            m.complete(p, ERROR)
        }
    }
}

fn path_type(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    paths::path(p, AngleCtx::Type);
    m.complete(p, PATH_TYPE)
}

// ------------------------------------------------------------ function types

/// True when the `(` at the cursor opens a function type's parameter list.
///
/// `functionType` (`Parser.hs:551`) is the only type that starts with `(`, and
/// the only thing that distinguishes it from a parenthesized expression is the
/// `->` after the closing paren — so that is what is looked for. The scan is
/// why `static assert (decltype(F::mul) == (float32, float32) -> float32);`
/// (`library/base.k:50`) parses: the right-hand side is a *type* in expression
/// position.
///
/// It does **not** simply hunt for the matching `)`. Walking the whole region
/// at every nesting level makes `(((…)))` quadratic, and this predicate is
/// consulted at every `(` in primary position. Instead it steps over the
/// parameter list by its own grammar — a type, an optional name, `,` — and
/// gives up at the first token a parameter list cannot contain, which for a
/// parenthesized expression is almost always the token right after the `(`.
pub(crate) fn at_fn_type(p: &Parser<'_>) -> bool {
    if !p.at(L_PAREN) {
        return false;
    }
    let mut i = 1usize;
    loop {
        i = match p.nth(i) {
            R_PAREN => return p.nth_at(i + 1, THIN_ARROW),
            // `decltype(e)` is the one parameter type with a `(` of its own.
            DECLTYPE_KW => match skip_balanced(p, i + 1, L_PAREN, R_PAREN) {
                Some(n) => n,
                None => return false,
            },
            // `[[last]]` attributes and `[N]` array dimensions alike.
            L_BRACK => match skip_balanced(p, i, L_BRACK, R_BRACK) {
                Some(n) => n,
                None => return false,
            },
            // A template instantiation or an `int<N>` width.
            L_ANGLE => match lookahead::skip_angles(p, i) {
                Some(n) => n,
                None => return false,
            },
            k if IN_FN_TYPE_PARAMS.contains(k) => i + 1,
            _ => return false,
        };
    }
}

/// Skips a balanced `open`..`close` region starting at `i`.
fn skip_balanced(p: &Parser<'_>, i: usize, open: SyntaxKind, close: SyntaxKind) -> Option<usize> {
    if !p.nth_at(i, open) {
        return None;
    }
    let mut depth = 0usize;
    let mut n = i;
    loop {
        let kind = p.nth(n);
        if !kind.is_token() {
            return None;
        }
        if kind == open {
            depth += 1;
        } else if kind == close {
            depth -= 1;
            if depth == 0 {
                return Some(n + 1);
            }
        }
        n += 1;
    }
}

/// The `( params ) -> ret` tail, with `m` already covering any attributes.
fn fn_type_rest(p: &mut Parser<'_>, m: crate::Marker) -> CompletedMarker {
    let params = p.start();
    p.expect(L_PAREN);
    if !p.at(R_PAREN) {
        loop {
            let pos = p.token_pos();
            fn_type_param(p);
            if p.token_pos() == pos {
                p.err_recover("expected a parameter", TYPE_RECOVERY);
                if p.token_pos() == pos {
                    break;
                }
            }
            if !p.eat_list_comma(R_PAREN) {
                break;
            }
        }
    }
    p.expect(R_PAREN);
    params.complete(p, FN_TYPE_PARAM_LIST);
    p.expect(THIN_ARROW);
    fn_type_return(p);
    m.complete(p, FN_TYPE)
}

/// `ftParam = attributeSeq<[last]>? ftParamType identifier?` (`Parser.hs:553`).
/// The name is optional, which is what makes `(uint32) -> void` legal.
fn fn_type_param(p: &mut Parser<'_>) {
    let m = p.start();
    attrs::opt_attrs(p, attrs::PARAM_ATTRS);
    fn_type_param_type(p);
    if p.at(IDENT) {
        paths::name(p);
    }
    m.complete(p, FN_TYPE_PARAM);
}

/// `ftParamType = "const" type2 | "auto" | "string" | type2` (`Parser.hs:554`).
///
/// Routing through `type2` rather than `typ` is what makes
/// `((uint32) -> void) -> void` a syntax error: a function type cannot be a
/// function type's parameter (`test/syntax/function-type.k:4-16`, `expected:1`).
fn fn_type_param_type(p: &mut Parser<'_>) -> CompletedMarker {
    if p.at(CONST_KW) {
        let m = p.start();
        p.bump(CONST_KW);
        type2(p);
        return m.complete(p, CONST_TYPE);
    }
    if p.at(AUTO_KW) {
        let m = p.start();
        p.bump(AUTO_KW);
        return m.complete(p, AUTO_TYPE);
    }
    type2(p)
}

/// `ftReturn = "void" | "auto" | type2` (`Parser.hs:557`). Same exclusion.
fn fn_type_return(p: &mut Parser<'_>) -> CompletedMarker {
    if p.at(VOID_KW) {
        let m = p.start();
        p.bump(VOID_KW);
        return m.complete(p, VOID_TYPE);
    }
    if p.at(AUTO_KW) {
        let m = p.start();
        p.bump(AUTO_KW);
        return m.complete(p, AUTO_TYPE);
    }
    type2(p)
}
