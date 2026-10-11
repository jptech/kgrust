//! The two decisions that let this parser be context-free.
//!
//! The reference parser consults its symbol table *while parsing*: `Foo<X>` is a
//! template instantiation only if `findSymbol Foo` says `Foo` is a template
//! (`Parser.hs:668-680`), and `Foo x;` is a declaration only if `Foo` resolves
//! to a type (`Parser.hs:712-726`). That coupling is what stops the reference
//! from having error recovery, parallel parsing, or a language server.
//!
//! It is also avoidable, because of two properties of the language:
//!
//! 1. `<` and `>` are declared `no_assoc` (`Parser.hs:915-916`), so `a < b > c`
//!    is **not a valid expression at all**; and
//! 2. `<`, `>`, `<<`, `>>` are removed from the operator table used inside
//!    template arguments (`opsTemplate = ops' [] []`, `Parser.hs:913`).
//!
//! Together those mean an angle region that closes at depth zero without
//! crossing a statement boundary is a template argument list *wherever a
//! template argument list could go* — a decision a forward token scan can
//! make, given one more piece of information: whether the position wants a
//! type or an expression. See [`AngleCtx`], which is what keeps
//! `a < b && c > d` the pair of comparisons the reference reads it as.
//!
//! Everything else that needed the symbol table becomes a name-resolution
//! diagnostic later, which is still `Error 1` and therefore still
//! indistinguishable to the conformance suite.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::{Parser, SyntaxKind, TokenSet};
use SyntaxKind::*;

/// Which reading of `name < … >` the caller is able to accept.
///
/// This is the discriminator the "an angle region that balances is a template
/// argument list" premise lacked. `assert(pos < v.size && v.size > n)` balances
/// without crossing a statement boundary — `&&` joins two *separate*
/// comparisons rather than forming the non-associative `a < b > c` the language
/// forbids — so balancing alone cannot decide it.
///
/// What decides it is the position. A template argument list may be followed by
/// a declared name only where a *type* was expected; in an expression there is
/// nothing an identifier could be, so `a < b && c > d` can only be the pair of
/// comparisons the reference reads it as (`no_assoc Less` / `no_assoc Greater`
/// at `Parser.hs:915-916`, `binary And` at `:953`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum AngleCtx {
    /// A type position: `Foo<X> name` is exactly what is expected here.
    Type,
    /// An expression position: whatever follows the `>` must continue an
    /// expression, so anything that can only *begin* an operand proves the `<`
    /// was a comparison.
    Expr,
}

/// Tokens that cannot occur inside a template argument list at brace depth 0.
///
/// Seeing one means the `<` was a comparison after all.
const NOT_IN_TEMPLATE_ARGS: TokenSet = TokenSet::new(&[
    SEMICOLON,
    CLASS_KW,
    STRUCT_KW,
    UNION_KW,
    ENUM_KW,
    IMPORT_KW,
    MODULE_KW,
    PUBLIC_KW,
    PRIVATE_KW,
    USING_KW,
    EXPORT_KW,
    EXTERN_KW,
    RETURN_KW,
    IF_KW,
    ELSE_KW,
    FOR_KW,
    WHILE_KW,
    DO_KW,
    SWITCH_KW,
    CASE_KW,
    DEFAULT_KW,
    BREAK_KW,
    BARRIER_KW,
    REORDER_KW,
    ATOMIC_KW,
    STATIC_ASSERT_KW,
]);

/// Tokens that can never follow a template argument list *anywhere*.
///
/// `Foo<X> 3` is not valid under any reading, so a literal after the closing
/// `>` proves the `<` was a comparison. A binary operator, by contrast, *can*
/// follow — `bitsizeof S<uint6> - 2` (`test/logic/test_cases_24.k:153`) is a
/// template instance used as an operand — so the test has to be a deny-list,
/// not an allow-list.
const CANNOT_FOLLOW_TEMPLATE_ARGS: TokenSet = TokenSet::new(&[
    INT_NUMBER,
    FLOAT_NUMBER,
    STRING,
    STRING_TEXT,
    TRUE_KW,
    FALSE_KW,
    BANG,
    TILDE,
    // Keyword-headed expression forms: `Foo<X> cast<Y>(z)` is not valid.
    CAST_KW,
    MUX_KW,
    CONCAT_KW,
    FAN_OUT_KW,
    LUTMUL_KW,
    BITSIZEOF_KW,
    BYTESIZEOF_KW,
    BITOFFSETOF_KW,
    BYTEOFFSETOF_KW,
    CLOG2_KW,
]);

/// Tokens that begin an operand and so cannot follow a *complete expression*.
///
/// In a type position each of these may follow a template argument list — it is
/// the declared name, or the next type — which is why the set is applied only
/// under [`AngleCtx::Expr`].
const CANNOT_FOLLOW_IN_EXPR: TokenSet = TokenSet::new(&[
    IDENT,
    TEMPLATE_KW,
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
    STATIC_KW,
]);

fn cannot_follow(kind: SyntaxKind, ctx: AngleCtx) -> bool {
    CANNOT_FOLLOW_TEMPLATE_ARGS.contains(kind)
        || (ctx == AngleCtx::Expr && CANNOT_FOLLOW_IN_EXPR.contains(kind))
}

// ------------------------------------------------------------- angle matching

thread_local! {
    /// Absolute token index of a `<` → index of the `>` that closes it, or
    /// `None` when it never closes. See [`scan_angles`] for why one scan can
    /// answer for every `<` it walks over.
    static ANGLES: RefCell<HashMap<usize, Option<usize>>> = RefCell::new(HashMap::new());
}

/// Drops everything the previous parse learned. Called once per file by
/// [`super::source_file`], which is the crate's only entry point, so an index
/// recorded here always refers to the token stream being parsed now.
pub(crate) fn reset() {
    ANGLES.with(|m| m.borrow_mut().clear());
}

/// Absolute index of the `>` matching the `<` at `p.nth(rel)`.
///
/// The scan is memoized, and one scan records an answer for *every* `<` it
/// walks over, not just the one it was asked about. That is what keeps
/// `a0 < b0 && a1 < b1 && …` linear: the first query walks to the `;`, learns
/// that none of the `<`s in between close, and every later query is a lookup.
///
/// Soundness of recording more than was asked: a `<` is only ever pushed at
/// paren/bracket/brace depth 0 relative to the scan's own start, so a scan
/// begun at that `<` would see exactly the same relative depths from there on,
/// and therefore stop for exactly the same reason at exactly the same token.
fn match_angle(p: &Parser<'_>, rel: usize) -> Option<usize> {
    let open = p.token_pos() + rel;
    if let Some(hit) = ANGLES.with(|m| m.borrow().get(&open).copied()) {
        return hit;
    }
    let found = scan_angles(p, open);
    let mut answer = None;
    ANGLES.with(|m| {
        let mut m = m.borrow_mut();
        for &(k, v) in &found {
            if k == open {
                answer = v;
            }
            m.insert(k, v);
        }
    });
    answer
}

/// Walks forward from the `<` at absolute index `open`, tracking angle depth
/// and ignoring anything nested inside `(`, `[` or `{` — which is exactly the
/// escape hatch the reference documents at `Parser.hs:910-912`, where a
/// parenthesized sub-expression re-enables the full operator table.
///
/// Returns one entry per `<` seen at depth 0, in no particular order. The scan
/// is bounded only by the end of input; every construct that could make it run
/// away — a statement keyword, an unmatched closer — stops it first.
fn scan_angles(p: &Parser<'_>, open: usize) -> Vec<(usize, Option<usize>)> {
    let base = p.token_pos();
    let mut out: Vec<(usize, Option<usize>)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let (mut paren, mut brack, mut brace) = (0usize, 0usize, 0usize);
    let mut abs = open;

    loop {
        let rel = abs - base;
        let kind = p.nth(rel);
        let nested = paren > 0 || brack > 0 || brace > 0;
        match kind {
            // Past end of input: nothing left can close.
            k if !k.is_token() => break,

            L_PAREN => paren += 1,
            R_PAREN => {
                if paren == 0 {
                    // Ran out of the enclosing `(`: `f(a < b)`.
                    break;
                }
                paren -= 1;
            }
            L_BRACK => brack += 1,
            R_BRACK => {
                if brack == 0 {
                    break;
                }
                brack -= 1;
            }
            L_CURLY => brace += 1,
            R_CURLY => {
                if brace == 0 {
                    break;
                }
                brace -= 1;
            }

            // A `<` glued to `=` is the `<=` operator, not an angle
            // (`Parser/Lexer.hs:272-273`).
            L_ANGLE if !nested && p.nth_at_angle(rel, L_ANGLE) => stack.push(abs),
            R_ANGLE if !nested && p.nth_at_angle(rel, R_ANGLE) => {
                if let Some(o) = stack.pop() {
                    out.push((o, Some(abs)));
                    if stack.is_empty() {
                        return out;
                    }
                }
            }

            k if brace == 0 && NOT_IN_TEMPLATE_ARGS.contains(k) => break,
            _ => {}
        }
        abs += 1;
    }

    out.extend(stack.into_iter().map(|o| (o, None)));
    out
}

/// True when the `<` at the cursor opens a template argument list.
pub(crate) fn at_template_arg_list(p: &Parser<'_>, ctx: AngleCtx) -> bool {
    if !p.at_angle(L_ANGLE) || p.at_marked_comparison() {
        return false;
    }
    let Some(close) = match_angle(p, 0) else { return false };
    // At end of input this is a kind no token has, and so not in either
    // deny-list: the file is truncated and recovery wants the richer tree.
    !cannot_follow(p.nth(close + 1 - p.token_pos()), ctx)
}

/// Index just past the `>` closing the `<` at `i`, for a scan that has already
/// decided it is looking at a type.
pub(crate) fn skip_angles(p: &Parser<'_>, i: usize) -> Option<usize> {
    if !p.nth_at_angle(i, L_ANGLE) {
        return None;
    }
    let close = match_angle(p, i)?;
    Some(close + 1 - p.token_pos())
}

/// True when a declaration starts at the cursor, in a position where a
/// statement could also start.
///
/// The reference decides this by ordered choice with unbounded backtracking:
/// `statement`'s `try (expression <* semi)` runs first and fails at the
/// declared name, after which `try (localVariable <* semi)` succeeds
/// (`Parser.hs:471-493`). Here it is a bounded lookahead instead: scan a
/// type-shaped prefix, and if an identifier follows it, this is a declaration.
///
/// The `a < b > c` case resolves correctly for free — the scan reads it as the
/// type `a<b>` followed by the name `c`, which is exactly what the reference
/// concludes, because `a < b > c` cannot be an expression.
pub(crate) fn at_decl(p: &Parser<'_>) -> bool {
    let mut i = 0usize;

    match p.current() {
        // `static x` declares; `static(e)` is an expression and
        // `static if/for/assert` are statements (`Parser.hs:475, 573, 591, 593`).
        STATIC_KW => {
            return match p.nth(1) {
                L_PAREN | IF_KW | FOR_KW => false,
                IDENT if p.nth_text(1) == "assert" => false,
                _ => true,
            };
        }
        // `[[...]]` may prefix either an annotated statement or an array type
        // with a memory attribute, so skip the groups and re-decide.
        L_BRACK => {
            while p.nth_at(i, L_BRACK) && p.nth_at(i + 1, L_BRACK) {
                match skip_balanced(p, i, L_BRACK, R_BRACK) {
                    Some(next) => i = next,
                    None => return false,
                }
            }
        }
        _ => {}
    }

    // A function definition puts its `inline`/`noinline` modifier before the
    // return type (`funcModifier`, `Parser.hs:399-404`), and a nested function
    // definition is legal wherever a statement is (`statementOrDecl`,
    // `Parser.hs:471-479`). Without this, `inline bool f() { … }` inside a
    // function body falls through to the expression parser.
    if matches!(p.nth(i), INLINE_KW | NOINLINE_KW) {
        i += 1;
    }

    if p.nth_at(i, CONST_KW) {
        i += 1;
    }

    let Some(after_type) = skip_type_head(p, i) else { return false };
    let Some(after_dims) = skip_array_dims(p, after_type) else { return false };

    // A declaration is a type followed by a name.
    p.nth_at(after_dims, IDENT)
}

/// Consumes one type head, returning the index just past it.
fn skip_type_head(p: &Parser<'_>, mut i: usize) -> Option<usize> {
    match p.nth(i) {
        VOID_KW | BOOL_KW | FLOAT32_KW | STRING_KW | AUTO_KW | INT_TYPE_TOKEN | UINT_TYPE_TOKEN => {
            Some(i + 1)
        }
        // `int<N>` / `uint<N>`
        INT_KW | UINT_KW => {
            if p.nth_at_angle(i + 1, L_ANGLE) {
                skip_balanced_angles(p, i + 1)
            } else {
                Some(i + 1)
            }
        }
        DECLTYPE_KW => skip_balanced(p, i + 1, L_PAREN, R_PAREN),
        TYPENAME_KW => skip_type_head(p, i + 1),
        // A function type, `(T, U) -> R` (`functionType`, `Parser.hs:549-553`):
        // a local `const (uint32) -> uint32 f = foo;` is a declaration like
        // any other, and fell through to the expression parser without this.
        L_PAREN => {
            let after = skip_balanced(p, i, L_PAREN, R_PAREN)?;
            if p.nth_at(after, THIN_ARROW) { skip_type_head(p, after + 1) } else { None }
        }
        IDENT | TEMPLATE_KW => {
            // A path: `a::b::c`, each segment optionally template-instantiated
            // and each — not only the first — optionally `template`-qualified
            // (`prefixedScopedNameAndSymbol` recurses into itself after `::`,
            // `Parser.hs:653, 683`), which is what makes
            // `X::template Y<uint8> v;` a local variable.
            loop {
                let mut head = p.nth(i);
                if head == TEMPLATE_KW && !p.nth_at_angle(i + 1, L_ANGLE) {
                    i += 1;
                    head = p.nth(i);
                }
                if head != IDENT {
                    return None;
                }
                i += 1;
                if p.nth_at_angle(i, L_ANGLE) {
                    i = skip_balanced_angles(p, i)?;
                }
                if p.nth_at(i, COLON2) {
                    i += 1;
                    continue;
                }
                return Some(i);
            }
        }
        _ => None,
    }
}

/// Consumes any number of `[expr]` array dimensions.
fn skip_array_dims(p: &Parser<'_>, mut i: usize) -> Option<usize> {
    // `[[` starts an attribute group, not a dimension.
    while p.nth_at(i, L_BRACK) && !p.nth_at(i + 1, L_BRACK) {
        i = skip_balanced(p, i, L_BRACK, R_BRACK)?;
    }
    Some(i)
}

/// Skips a balanced `open`..`close` region starting at `i`, which must be at
/// `open`. Returns the index just past the matching `close`.
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

/// Skips a balanced `<`..`>` region in a type position.
fn skip_balanced_angles(p: &Parser<'_>, i: usize) -> Option<usize> {
    let next = skip_angles(p, i)?;
    if cannot_follow(p.nth(next), AngleCtx::Type) {
        return None;
    }
    Some(next)
}
