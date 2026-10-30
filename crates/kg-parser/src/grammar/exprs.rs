//! Expressions: a Pratt parser over the reference's `makeExprParser` table.
//!
//! The table is at `Parser.hs:913-968` and is given **tightest first**, which
//! is the opposite of the usual convention, so it is transcribed here into
//! binding powers where *larger binds tighter*. Two things about it are
//! unusual enough to be worth naming:
//!
//! * **Prefix operators sit at three different levels** — `-` tighter than
//!   `!`/`~`, tighter than `bitsizeof`/`bytesizeof`/`clog2`
//!   (`Parser.hs:921-930`). A prefix operator's operand is drawn from a
//!   *strictly* tighter level than its own, which is why `bitsizeof -x` parses,
//!   `-bitsizeof x` does not (grammar.md §9 trap 15), and `!~x` — two operators
//!   sharing one level — does not either. See [`unary_expr`].
//! * **Four levels are non-associative** — `<= >= < >` and `== !=`
//!   (`Parser.hs:915-916, 939-945`). `a < b < c` is not a parse error in the
//!   Pratt sense, it is a *rejected* one, so it is diagnosed explicitly rather
//!   than folded left. That non-associativity is also load-bearing for
//!   [`lookahead::at_template_arg_list`]: it is why an angle region that
//!   balances can only have been a template argument list.
//!
//! [`ExprMode::TemplateArg`] removes `<`, `>`, `<<` and `>>` from the table
//! (`opsTemplate = ops' [] []`, `Parser.hs:913`); a parenthesized
//! sub-expression restores the full table (`Parser.hs:845`), which is the
//! documented escape hatch for writing a comparison inside `<...>`.

use super::lookahead::AngleCtx;
use super::{ExprMode, attrs, items, lookahead, paths, stmts, types};
use crate::{CompletedMarker, Marker, Parser, SyntaxKind, TokenSet};
use kg_base::language::Feature;
use SyntaxKind::*;

/// Where a malformed expression stops rather than eating a delimiter.
///
/// The statement set is folded in because nothing that can begin a statement
/// can continue an expression, so stopping there is always the better guess.
pub(crate) const EXPR_RECOVERY: TokenSet =
    TokenSet::new(&[SEMICOLON, COMMA, R_PAREN, R_BRACK, R_CURLY, R_ANGLE, COLON, L_CURLY])
        .union(super::STMT_RECOVERY);

// Binding powers, from `Parser.hs:921-960` read bottom-up. Odd numbers are
// left free so a left-associative level can ask for `bp + 1` on its right.
const BP_NEG: u8 = 30;
const BP_NOT: u8 = 28;
const BP_SIZEOF: u8 = 26;
const BP_MUL: u8 = 24;
const BP_ADD: u8 = 22;
const BP_SHIFT: u8 = 20;
const BP_REL: u8 = 18;
const BP_EQ: u8 = 16;
const BP_BIT_AND: u8 = 14;
const BP_BIT_XOR: u8 = 12;
const BP_BIT_OR: u8 = 10;
const BP_AND: u8 = 8;
const BP_XOR: u8 = 6;
const BP_OR: u8 = 4;
const BP_TERNARY: u8 = 2;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Fixity {
    Left,
    /// `infixN`: `a < b < c` is rejected outright.
    None,
    Right,
}

#[derive(Copy, Clone, Debug)]
struct Infix {
    kind: SyntaxKind,
    bp: u8,
    assoc: Fixity,
    /// `<=`, `>=`, `<<`, `>>`: two lexer tokens the parser fuses
    /// (`Parser/Lexer.hs:269-273` never merges angles).
    composite: bool,
}

/// The only entry point. Returns the completed expression, or `None` when
/// nothing at the cursor could start one — in which case a diagnostic has
/// already been reported.
pub(crate) fn expr(p: &mut Parser<'_>, mode: ExprMode) -> Option<CompletedMarker> {
    expr_bp(p, mode, 0)
}

fn expr_bp(p: &mut Parser<'_>, mode: ExprMode, min_bp: u8) -> Option<CompletedMarker> {
    // Every level of nesting is native stack; see [`super::depth`].
    let Some(_guard) = super::depth::enter() else {
        p.err_recover(super::depth::TOO_DEEP, EXPR_RECOVERY);
        return None;
    };
    let mut lhs = unary_expr(p, mode, min_bp)?;
    // The level of the non-associative operator just applied, if any, so the
    // next one at the same level can be rejected.
    let mut nonassoc_at: Option<u8> = None;

    while let Some(op) = peek_infix(p, mode) {
        if op.bp < min_bp {
            break;
        }
        if op.assoc == Fixity::None && nonassoc_at == Some(op.bp) {
            p.error(format!(
                "`{}` is not associative; parenthesize one of the comparisons",
                describe_op(op.kind)
            ));
        }

        let m = lhs.precede(p);
        if op.kind == QUESTION {
            // `ternary "?" ":" ternaryMux` (`Parser.hs:959, 967`). The middle
            // arm is delimited, so it is parsed with no precedence floor.
            p.bump(QUESTION);
            expr_bp(p, mode, 0);
            if p.at_operator(COLON) {
                p.bump(COLON);
            } else {
                p.error("expected `:`");
            }
            expr_bp(p, mode, BP_TERNARY);
            lhs = m.complete(p, TERNARY_EXPR);
            nonassoc_at = None;
            continue;
        }

        if op.composite {
            p.bump_composite(op.kind);
        } else {
            p.bump(op.kind);
        }
        let rhs_min = match op.assoc {
            Fixity::Right => op.bp,
            _ => op.bp + 1,
        };
        expr_bp(p, mode, rhs_min);
        lhs = m.complete(p, BIN_EXPR);
        nonassoc_at = (op.assoc == Fixity::None).then_some(op.bp);
    }
    Some(lhs)
}

/// The prefix levels.
///
/// A prefix operator is only available when its own level is at least as tight
/// as the floor, and its operand is drawn from the level *above* its own — the
/// two halves of `makeExprParser`'s `addPrecLevel`, which builds
/// `term' = pTerm (choice prefix) term (choice postfix)` with `pTerm` applying
/// `option id prefix` exactly once and taking `term` from the next tighter
/// level. So a prefix binds looser than everything tighter than it
/// (`bitsizeof -x` parses, `-bitsizeof x` does not, grammar.md §9 trap 15) and
/// **cannot repeat within its own level**: `!~x` is as much a syntax error as
/// `!!x`, because `prefix Not` and `prefix Invert` share one level
/// (`Parser.hs:924-925`), as do `bitsizeof`/`bytesizeof`/`clog2` (`:927-929`).
fn unary_expr(p: &mut Parser<'_>, mode: ExprMode, min_bp: u8) -> Option<CompletedMarker> {
    let bp = match p.current() {
        MINUS if p.at_operator(MINUS) => BP_NEG,
        BANG if p.at_operator(BANG) => BP_NOT,
        TILDE if p.at_operator(TILDE) => BP_NOT,
        BITSIZEOF_KW | BYTESIZEOF_KW | CLOG2_KW => BP_SIZEOF,
        _ => return postfix_expr(p),
    };
    if bp < min_bp {
        return postfix_expr(p);
    }
    let m = p.start();
    p.bump_any();
    expr_bp(p, mode, bp + 1);
    Some(m.complete(p, PREFIX_EXPR))
}

/// The infix operator at the cursor, if any.
///
/// Dispatched on the current token rather than by trying each operator in turn:
/// every `at_*` query is a parser step, this runs once per operand, and the
/// step budget is global.
fn peek_infix(p: &Parser<'_>, mode: ExprMode) -> Option<Infix> {
    let full = mode == ExprMode::Full;
    let plain = |kind, bp, assoc| Some(Infix { kind, bp, assoc, composite: false });
    let fused = |kind, bp, assoc| Some(Infix { kind, bp, assoc, composite: true });

    match p.current() {
        // The fused pairs must be tested before the single angles they start
        // with; `<` and `>` themselves leave the table inside `<...>`
        // (`opsTemplate`, `Parser.hs:913`) but `<=` and `>=` do not.
        L_ANGLE => {
            if p.at_composite(LTEQ) {
                fused(LTEQ, BP_REL, Fixity::None)
            } else if full && p.at_composite(SHL) {
                fused(SHL, BP_SHIFT, Fixity::Left)
            } else if full && p.at_operator(L_ANGLE) {
                plain(L_ANGLE, BP_REL, Fixity::None)
            } else {
                None
            }
        }
        R_ANGLE => {
            if p.at_composite(GTEQ) {
                fused(GTEQ, BP_REL, Fixity::None)
            } else if full && p.at_composite(SHR) {
                fused(SHR, BP_SHIFT, Fixity::Left)
            } else if full && p.at_operator(R_ANGLE) {
                plain(R_ANGLE, BP_REL, Fixity::None)
            } else {
                None
            }
        }
        STAR if p.at_operator(STAR) => plain(STAR, BP_MUL, Fixity::Left),
        SLASH if p.at_operator(SLASH) => plain(SLASH, BP_MUL, Fixity::Left),
        PERCENT if p.at_operator(PERCENT) => plain(PERCENT, BP_MUL, Fixity::Left),
        PLUS if p.at_operator(PLUS) => plain(PLUS, BP_ADD, Fixity::Left),
        MINUS if p.at_operator(MINUS) => plain(MINUS, BP_ADD, Fixity::Left),
        EQ2 if p.at_operator(EQ2) => plain(EQ2, BP_EQ, Fixity::None),
        NEQ if p.at_operator(NEQ) => plain(NEQ, BP_EQ, Fixity::None),
        AMP if p.at_operator(AMP) => plain(AMP, BP_BIT_AND, Fixity::Left),
        CARET if p.at_operator(CARET) => plain(CARET, BP_BIT_XOR, Fixity::Left),
        PIPE if p.at_operator(PIPE) => plain(PIPE, BP_BIT_OR, Fixity::Left),
        AMP2 if p.at_operator(AMP2) => plain(AMP2, BP_AND, Fixity::Left),
        CARET2 if p.at_operator(CARET2) => plain(CARET2, BP_XOR, Fixity::Left),
        PIPE2 if p.at_operator(PIPE2) => plain(PIPE2, BP_OR, Fixity::Left),
        QUESTION if p.at_operator(QUESTION) => plain(QUESTION, BP_TERNARY, Fixity::Right),
        _ => None,
    }
}

fn describe_op(kind: SyntaxKind) -> &'static str {
    kind.static_text().unwrap_or("operator")
}

// ------------------------------------------------------------------ postfix

/// `value`/`lvalue` plus their access chain (`Parser.hs:783-819`) and
/// `functionCall` (`:871`), unified.
///
/// The reference keeps them apart — `access` refuses a member name followed by
/// `(` or by `<...>` precisely so those fall through to `functionCall` and
/// `methodInstanceSpecifier` — but the resulting trees are the same shape, and
/// a uniform postfix loop is what lets a half-typed `a.` still produce a
/// [`FIELD_EXPR`] for the language server to complete against.
fn postfix_expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    // `callModifiers` come *before* the callee (`Parser.hs:871`), so the node
    // that will become the call has to be opened before the primary is read.
    let mut pending: Option<Marker> = None;
    if p.at(L_BRACK) && p.nth_at(1, L_BRACK) && attrs::at_attrs(p, attrs::CALL_ATTRS) {
        let m = p.start();
        attrs::opt_attrs(p, attrs::CALL_ATTRS);
        pending = Some(m);
    }

    let Some(mut lhs) = primary(p) else {
        if let Some(m) = pending {
            m.abandon(p);
        }
        return None;
    };

    loop {
        lhs = match p.current() {
            L_PAREN => {
                let m = match pending.take() {
                    Some(m) => m,
                    None => lhs.precede(p),
                };
                arg_list(p);
                m.complete(p, CALL_EXPR)
            }
            // `[[` is an attribute group, never an index.
            L_BRACK if !p.nth_at(1, L_BRACK) => {
                let m = lhs.precede(p);
                p.bump(L_BRACK);
                expr(p, ExprMode::Full);
                p.expect(R_BRACK);
                m.complete(p, INDEX_EXPR)
            }
            DOT => {
                let m = lhs.precede(p);
                p.bump(DOT);
                if paths::at_template_qualifier(p) {
                    p.bump(TEMPLATE_KW);
                }
                paths::name_ref(p);
                if lookahead::at_template_arg_list(p, AngleCtx::Expr) {
                    paths::template_arg_list(p);
                }
                m.complete(p, FIELD_EXPR)
            }
            _ => break,
        };
    }

    // Attributes with no call after them: keep them in the tree, drop the
    // wrapper. The reference would have failed the whole `try` here.
    if let Some(m) = pending {
        m.abandon(p);
    }
    Some(lhs)
}

fn arg_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_PAREN);
    if p.has(Feature::NamedArguments) {
        named_args(p);
    } else {
        comma_list(p, R_PAREN, |p| {
            expr(p, ExprMode::Full);
        });
    }
    p.expect(R_PAREN);
    m.complete(p, ARG_LIST);
}

/// Edition 1.1's arguments (`docs/editions/1.1.md`): each may be named by
/// its parameter, `x: e`, once the positional ones are done. The label is an
/// [`ARG_LABEL`] *before* its expression, not around it, so every consumer
/// of an argument list that reads its expressions reads the same ones as
/// in the call with the labels erased.
///
/// `IDENT ':'` cannot begin a reference argument — `::` is one token, and a
/// ternary's `:` follows its `?` — which is why the reference rejects every
/// call this accepts (`kg-oracle --errors`: "unexpected ':'", 2026-10-29).
fn named_args(p: &mut Parser<'_>) {
    let mut labels: Vec<String> = Vec::new();
    comma_list(p, R_PAREN, |p| {
        if p.at(IDENT) && p.nth_at(1, COLON) {
            let text = p.token_text();
            if labels.iter().any(|l| l == text) {
                p.error(format!("the argument `{text}` is named twice"));
            }
            labels.push(text.to_owned());
            let l = p.start();
            p.bump(IDENT);
            p.bump(COLON);
            l.complete(p, ARG_LABEL);
        } else if !labels.is_empty() && !p.at(R_PAREN) {
            p.error("a positional argument cannot follow a named one");
        }
        expr(p, ExprMode::Full);
    });
}

/// Parses `elem ("," elem)*` up to `close`, guaranteeing forward progress.
fn comma_list(p: &mut Parser<'_>, close: SyntaxKind, mut elem: impl FnMut(&mut Parser<'_>)) -> usize {
    if p.at(close) {
        return 0;
    }
    let mut n = 0usize;
    loop {
        let pos = p.token_pos();
        elem(p);
        n += 1;
        if p.token_pos() == pos {
            p.err_recover("expected an expression", EXPR_RECOVERY);
            if p.token_pos() == pos {
                break;
            }
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    n
}

// ------------------------------------------------------------------ primary

/// A string with holes: `'string_text' (Interpolation | 'string_text')*
/// 'string_end'` (`Parser.hs:883-903`). The lexer has already split the text
/// from the holes (`Lexer::string_piece`), so every piece arrives as its own
/// token and a hole's `{` is always the next token after a piece or a `}`.
pub(crate) fn interpolated_string(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(STRING_TEXT);
    loop {
        match p.current() {
            L_CURLY => interpolation(p),
            STRING_TEXT => p.bump(STRING_TEXT),
            STRING_END => {
                p.bump(STRING_END);
                break;
            }
            // End of file inside the string: the lexer reported it.
            _ => break,
        }
    }
    m.complete(p, INTERPOLATED_STRING)
}

/// `between (L.symbol "{") (char '}') formatedExpr` (`Parser.hs:888,895-899`):
/// an expression, an optional `=`, an optional `, alignment`, an optional
/// `: format`. The expression is parsed in full mode wherever the string
/// sits — the hole is its own bracket, as a parenthesis is.
///
/// The format is taken as whatever stands between the `:` and the hole's
/// `}`, unparsed: its grammar (`Parser.hs:900-903`) is character-level and
/// is checked, at the character the reference rejects, by `kg_db`'s pass 0.
/// Anything else before the `}` is an error, and the parser skips to the
/// `}` that closes the hole — the lexer has already decided which one that
/// is, and braces between balance.
fn interpolation(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_CURLY);
    expr(p, ExprMode::Full);
    p.eat(EQ);
    if p.eat(COMMA) {
        expr(p, ExprMode::Full);
    }
    if p.eat(COLON) {
        let f = p.start();
        skip_to_hole_end(p);
        f.complete(p, FORMAT_SPEC);
    } else if !p.at(R_CURLY) && !p.at_end() {
        p.error(format!("expected {}", crate::parser::describe(R_CURLY)));
        let e = p.start();
        skip_to_hole_end(p);
        e.complete(p, ERROR);
    }
    p.expect(R_CURLY);
    m.complete(p, INTERPOLATION);
}

/// Consumes tokens up to, not including, the `}` at this hole's depth.
fn skip_to_hole_end(p: &mut Parser<'_>) {
    let mut depth = 0u32;
    while !p.at_end() {
        match p.current() {
            R_CURLY if depth == 0 => break,
            R_CURLY => depth -= 1,
            L_CURLY => depth += 1,
            _ => {}
        }
        p.bump_any();
    }
}

fn primary(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let cm = match p.current() {
        INT_NUMBER | FLOAT_NUMBER | STRING | TRUE_KW | FALSE_KW => {
            let m = p.start();
            p.bump_any();
            m.complete(p, LITERAL)
        }
        STRING_TEXT => interpolated_string(p),

        // `mux(sel, e0, e1, ...)` needs at least two arms (`Parser.hs:870`).
        MUX_KW => keyword_call(p, MUX_EXPR, 2, usize::MAX),
        LUTMUL_KW => keyword_call(p, LUT_MUL_EXPR, 2, 2),
        CONCAT_KW => keyword_call(p, CONCAT_EXPR, 1, usize::MAX),
        STATIC_KW => keyword_call(p, STATIC_EXPR, 1, 1),

        BITOFFSETOF_KW => offset_of(p, BIT_OFFSET_EXPR),
        BYTEOFFSETOF_KW => offset_of(p, BYTE_OFFSET_EXPR),

        // `cast <typ> (e)` — a *type* in the angles (`Parser.hs:872`).
        CAST_KW => {
            let m = p.start();
            p.bump(CAST_KW);
            angle_bracketed(p, |p| {
                types::type_(p);
            });
            p.expect(L_PAREN);
            expr(p, ExprMode::Full);
            p.expect(R_PAREN);
            m.complete(p, CAST_EXPR)
        }
        // `fan_out <e> (e)` — an *expression* in the angles (`Parser.hs:868`).
        FAN_OUT_KW => {
            let m = p.start();
            p.bump(FAN_OUT_KW);
            angle_bracketed(p, |p| {
                expr(p, ExprMode::TemplateArg);
            });
            p.expect(L_PAREN);
            expr(p, ExprMode::Full);
            p.expect(R_PAREN);
            m.complete(p, FAN_OUT_EXPR)
        }

        L_PAREN => {
            // `(a, b) -> c` is a function type used as a term
            // (`try (typ' False)`, `Parser.hs:843`); anything else is a
            // parenthesized expression, which always restores the full
            // operator table (`Parser.hs:845`).
            if types::at_fn_type(p) {
                types::type_(p)
            } else {
                let m = p.start();
                p.bump(L_PAREN);
                expr(p, ExprMode::Full);
                p.expect(R_PAREN);
                m.complete(p, PAREN_EXPR)
            }
        }

        // `{}` is a *designated* initializer list, not a positional one
        // (grammar.md §9 trap 7); both shapes land in one node here.
        L_CURLY => init_list(p),

        L_BRACK => {
            if p.nth_at(1, L_BRACK) {
                // Already ruled out as call modifiers by `postfix_expr`, so
                // this can only be a type's attributes.
                types::type_(p)
            } else {
                lambda(p)
            }
        }

        // A type in expression position. The reference gates this on
        // `findSymbol`; here the keyword-introduced forms are unambiguous and
        // a bare path is left as a `PATH` for the resolver to classify.
        VOID_KW | CONST_KW | AUTO_KW | BOOL_KW | FLOAT32_KW | STRING_KW | INT_KW | UINT_KW
        | INT_TYPE_TOKEN | UINT_TYPE_TOKEN | DECLTYPE_KW | TYPENAME_KW => types::type_(p),

        IDENT | TEMPLATE_KW => paths::path(p, AngleCtx::Expr),

        _ => {
            p.err_recover("expected an expression", EXPR_RECOVERY);
            return None;
        }
    };
    Some(cm)
}

/// `kw "(" e ("," e)* ")"`, checking the reference's arity.
fn keyword_call(p: &mut Parser<'_>, node: SyntaxKind, min: usize, max: usize) -> CompletedMarker {
    let m = p.start();
    let kw = p.current();
    p.bump(kw);
    p.expect(L_PAREN);
    let n = comma_list(p, R_PAREN, |p| {
        expr(p, ExprMode::Full);
    });
    if n < min || n > max {
        p.error(format!(
            "`{}` takes {}",
            kw.static_text().unwrap_or("this"),
            arity(min, max)
        ));
    }
    p.expect(R_PAREN);
    m.complete(p, node)
}

fn arity(min: usize, max: usize) -> String {
    match (min, max) {
        (a, b) if a == b => format!("exactly {a} argument(s)"),
        (a, usize::MAX) => format!("at least {a} argument(s)"),
        (a, b) => format!("between {a} and {b} arguments"),
    }
}

/// `bitoffsetof(typ, identifier)` / `byteoffsetof(...)` (`Parser.hs:864-865`).
/// The first argument is a type and the second a bare member name.
fn offset_of(p: &mut Parser<'_>, node: SyntaxKind) -> CompletedMarker {
    let m = p.start();
    p.bump_any();
    p.expect(L_PAREN);
    types::type_(p);
    p.expect(COMMA);
    paths::name_ref(p);
    p.expect(R_PAREN);
    m.complete(p, node)
}

/// `<...>` around one item, using the angle rules rather than the operator
/// ones: `_angle` only forbids a following `=` (`Parser/Lexer.hs:272-273`).
fn angle_bracketed(p: &mut Parser<'_>, inner: impl FnOnce(&mut Parser<'_>)) {
    if p.at_angle(L_ANGLE) {
        p.bump(L_ANGLE);
    } else {
        p.error("expected `<`");
    }
    inner(p);
    if p.at_angle(R_ANGLE) {
        p.bump(R_ANGLE);
    } else {
        p.error("expected `>`");
    }
}

/// `{ e, ... }` or `{ .x = e, ... }` or `{}` (`Parser.hs:851-863`).
///
/// The two shapes are separate productions —
/// `try designatedInitializerList <|> initializerList` (`Parser.hs:850-852`) —
/// and neither admits an element of the other kind, so the choice is made once
/// for the whole list rather than per element. `{a, .x = 1}` fails the
/// designated form at `a`, then parses `a` positionally, backtracks the
/// `try (comma >> positionalList)` at `.x`, and leaves `braces` staring at a
/// stray `,`.
///
/// Trailing commas are not allowed in either shape, for that same reason
/// (`Parser.hs:860`).
fn init_list(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(L_CURLY);
    let designated = p.at(DOT);
    comma_list(p, R_CURLY, |p| {
        if p.at(DOT) {
            if !designated {
                p.error("a positional initializer list cannot also designate a field");
            }
            let d = p.start();
            p.bump(DOT);
            paths::name_ref(p);
            p.expect(EQ);
            expr(p, ExprMode::Full);
            d.complete(p, DESIGNATOR);
        } else {
            if designated {
                p.error("a designated initializer list needs `.field = ` on every element");
            }
            expr(p, ExprMode::Full);
        }
    });
    p.expect(R_CURLY);
    m.complete(p, INIT_LIST_EXPR)
}

/// `[captures] params? ("->" typ)? { body }` (`Parser.hs:436-447`).
///
/// Captures are bare identifiers — no `&`, no `=`, no `this`.
fn lambda(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    let caps = p.start();
    p.bump(L_BRACK);
    comma_list(p, R_BRACK, |p| {
        let c = p.start();
        paths::name_ref(p);
        c.complete(p, CAPTURE);
    });
    p.expect(R_BRACK);
    caps.complete(p, CAPTURE_LIST);

    if p.at(L_PAREN) {
        items::param_list(p);
    }
    if p.at(THIN_ARROW) {
        p.bump(THIN_ARROW);
        types::type_(p);
    }
    if p.at(L_CURLY) {
        stmts::block(p);
    } else {
        p.error("expected a lambda body");
    }
    m.complete(p, LAMBDA_EXPR)
}
