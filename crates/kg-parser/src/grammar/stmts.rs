//! Statements and blocks.
//!
//! `statementOrDecl` (`Parser.hs:471-479`) is an ordered choice with unbounded
//! backtracking whose whole purpose is to tell `Foo x;` from `Foo;`. Here that
//! is a single bounded predicate, [`lookahead::at_decl`], so a statement is
//! dispatched on its first token and nothing is ever re-parsed.
//!
//! Facts about this grammar that surprise everyone (grammar.md §3.9, §9):
//!
//! * `break` exists **only** as the mandatory terminator of a `switch` case,
//!   and there is no `continue` at all (`Parser.hs:602-603`).
//! * `do … while (…)` needs no trailing `;` (`Parser.hs:587-589`); one that is
//!   written is swallowed as a stray semicolon.
//! * There is no `<<=` or `>>=` — the compound-assignment list at
//!   `Parser.hs:616-629` is complete.
//! * `atomic stmt` synthesizes `[[atomic]]`; the attribute cannot be written
//!   (`Parser.hs:559-561`).
//! * `while (…)` is not a loop form on its own. [`WHILE_STMT`] is therefore
//!   never produced.

use super::items::Ctx;
use super::lookahead::AngleCtx;
use super::{ExprMode, attrs, exprs, items, lookahead, paths, types};
use crate::{CompletedMarker, Marker, Parser, SyntaxKind, TokenSet};
use SyntaxKind::*;

const CASE_END: TokenSet = TokenSet::new(&[BREAK_KW, CASE_KW, DEFAULT_KW, R_CURLY]);

/// `{ SEQ<statementOrDecl> }` — a function body, a lambda body or a nested
/// scope (`Parser.hs:469`).
pub(crate) fn block(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.expect(L_CURLY);
    while !p.at_end() && !p.at(R_CURLY) {
        let pos = p.token_pos();
        stmt_or_decl(p);
        if p.token_pos() == pos {
            p.err_and_bump("expected a statement");
        }
    }
    p.expect(R_CURLY);
    m.complete(p, BLOCK_STMT)
}

/// One `statement` (`Parser.hs:481-493`).
///
/// `statement` has **no declaration alternative at all**. Declarations live
/// only in `statementOrDecl` (`:471-479`), which is used for function and
/// lambda bodies (`:424, :469`), for nested `{ … }` scopes (`:489`), for the
/// body of a `switch` case (`:602-603`) and for the branches of a `static if`
/// (`:493`) — and nowhere else. Every other statement slot takes `statement`:
/// `reorder` (`:496`), `atomic` (`:559`), `annotatedStmt` (`:567`), `rangeFor`
/// (`:584`), `doWhile` (`:586-589`), `staticFor` (`:591`) and both branches of
/// `ifStmt` (`:597`). So `if (true) uint32 x;` is a syntax error, and so is
/// `reorder bool x;` (`test/syntax/statement.k:4`).
pub(crate) fn stmt(p: &mut Parser<'_>) {
    dispatch(p, false);
}

/// One `statementOrDecl` (`Parser.hs:471-479`).
pub(crate) fn stmt_or_decl(p: &mut Parser<'_>) {
    dispatch(p, true);
}

fn dispatch(p: &mut Parser<'_>, decls: bool) {
    // Statements nest without bound; see [`super::depth`].
    let Some(_guard) = super::depth::enter() else {
        p.err_and_bump(super::depth::TOO_DEEP);
        return;
    };
    match p.current() {
        SEMICOLON => {
            let m = p.start();
            p.bump(SEMICOLON);
            m.complete(p, EMPTY_STMT);
        }
        // `localNestedScope statementOrDecl` (`Parser.hs:489`): a braced scope
        // holds declarations even where the statement itself may not.
        L_CURLY => {
            block(p);
        }
        BARRIER_KW => {
            let m = p.start();
            p.bump(BARRIER_KW);
            p.expect(SEMICOLON);
            m.complete(p, BARRIER_STMT);
        }
        REORDER_KW => {
            let m = p.start();
            p.bump(REORDER_KW);
            stmt(p);
            m.complete(p, REORDER_STMT);
        }
        ATOMIC_KW => {
            let m = p.start();
            p.bump(ATOMIC_KW);
            stmt(p);
            m.complete(p, ATOMIC_STMT);
        }
        RETURN_KW => {
            let m = p.start();
            p.bump(RETURN_KW);
            if !p.at(SEMICOLON) {
                exprs::expr(p, ExprMode::Full);
            }
            p.expect(SEMICOLON);
            m.complete(p, RETURN_STMT);
        }
        // Legal only inside a `switch` block, where `switch_block` consumes it
        // first; reaching it here is an error the tree still has to hold.
        BREAK_KW => {
            let m = p.start();
            p.bump(BREAK_KW);
            p.expect(SEMICOLON);
            p.error("`break` is only allowed at the end of a `switch` case");
            m.complete(p, BREAK_STMT);
        }
        IF_KW => if_stmt(p),
        FOR_KW => {
            let m = p.start();
            for_stmt(p, m);
        }
        DO_KW => {
            let m = p.start();
            do_while_stmt(p, m);
        }
        SWITCH_KW => switch_stmt(p),
        STATIC_KW => static_stmt(p, decls),
        L_BRACK if p.nth_at(1, L_BRACK) => attributed(p, decls),
        // `template` heads a declaration only when a parameter list follows.
        // Bare `template` is the *disambiguator* of a path segment —
        // `template Go<bool>();` — which the reference spells
        // `try (L.symbol "template" <* notFollowedBy langle)` (`Parser.hs:646`).
        TEMPLATE_KW if !p.nth_at_angle(1, L_ANGLE) => expr_stmt(p),
        CLASS_KW | STRUCT_KW | UNION_KW | ENUM_KW | USING_KW | EXPORT_KW | EXTERN_KW
        | STATIC_ASSERT_KW | TEMPLATE_KW | PUBLIC_KW | PRIVATE_KW | DEFAULT_KW | IMPORT_KW
        | MODULE_KW => {
            if !decls {
                p.error(DECL_IN_STMT);
            }
            items::body_item(p);
        }
        _ => {
            // A function type is the one declaration shape `at_decl` cannot
            // see, because it starts with `(` — `() -> uint32 callback = …;`
            // (`test/syntax/dependent-template.k:121`).
            if lookahead::at_decl(p) || types::at_fn_type(p) {
                if !decls {
                    p.error(DECL_IN_STMT);
                }
                items::decl(p, Ctx::Body);
            } else {
                expr_stmt(p);
            }
        }
    }
}

/// Said once, wherever a declaration turns up in a slot that takes only a
/// `statement`. Parsing continues as if it were legal, so the reader gets this
/// one diagnostic instead of a shredded tree.
const DECL_IN_STMT: &str =
    "a declaration needs a `{ }` block here — this position takes a statement";

// ------------------------------------------------------------------- `static`

/// Everything that can start with `static`, in the reference's order.
///
/// `static if`, `static for` and `static assert` are all `try`-guarded
/// two-token forms (`Parser.hs:591, 593`, `keyword2` at `:573`), which is why
/// `assert` is not a reserved word and `static` alone still declares a
/// variable.
///
/// Only `static if` and `static for` are `statement`s (`controlFlow`,
/// `Parser.hs:578-583`). `static assert` reaches a body through `commonDecl`,
/// and `static x;` / `static default = e;` through `staticVariable` and
/// `try (keyword "static" *> defaultInitialization)` — all three of which are
/// `statementOrDecl` alone (`:471-479`), hence `decls`.
pub(crate) fn static_stmt(p: &mut Parser<'_>, decls: bool) {
    if p.nth_at(1, IF_KW) {
        // `staticIf statementOrDecl` (`Parser.hs:582`): the branches always
        // admit declarations, even when the `static if` itself sits in a
        // statement-only slot.
        static_if(p, stmt_or_decl);
    } else if p.nth_at(1, FOR_KW) {
        static_for(p);
    } else if p.nth_at(1, L_PAREN) {
        // `static(e)` is an expression (`Parser.hs:869`).
        expr_stmt(p);
    } else if p.nth_at(1, IDENT) && p.nth_text(1) == "assert" {
        if !decls {
            p.error(DECL_IN_STMT);
        }
        items::static_assert(p);
    } else if p.nth_at(1, DEFAULT_KW) {
        if !decls {
            p.error(DECL_IN_STMT);
        }
        items::default_init(p);
    } else {
        if !decls {
            p.error(DECL_IN_STMT);
        }
        items::decl(p, Ctx::Body);
    }
}

/// `staticIf<P>` (`Parser.hs:593-595`), parameterized by what its branches
/// hold: declarations at item level, members in a class body, `statementOrDecl`
/// in a function body, `case` blocks in a `switch`.
pub(crate) fn static_if(p: &mut Parser<'_>, body: fn(&mut Parser<'_>)) {
    let m = p.start();
    p.bump(STATIC_KW);
    p.bump(IF_KW);
    p.expect(L_PAREN);
    exprs::expr(p, ExprMode::Full);
    p.expect(R_PAREN);
    static_if_body(p, body);
    if p.at(ELSE_KW) {
        let e = p.start();
        p.bump(ELSE_KW);
        static_if_body(p, body);
        e.complete(p, ELSE_BRANCH);
    }
    m.complete(p, STATIC_IF_STMT);
}

fn static_if_body(p: &mut Parser<'_>, body: fn(&mut Parser<'_>)) {
    if !p.at(L_CURLY) {
        body(p);
        return;
    }
    let m = p.start();
    p.bump(L_CURLY);
    while !p.at_end() && !p.at(R_CURLY) {
        let pos = p.token_pos();
        body(p);
        if p.token_pos() == pos {
            p.err_and_bump("expected a declaration");
        }
    }
    p.expect(R_CURLY);
    m.complete(p, BLOCK_STMT);
}

fn static_for(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(STATIC_KW);
    p.bump(FOR_KW);
    range_head(p);
    stmt(p);
    m.complete(p, STATIC_FOR_STMT);
}

// --------------------------------------------------------------------- loops

fn for_stmt(p: &mut Parser<'_>, m: Marker) {
    p.bump(FOR_KW);
    range_head(p);
    stmt(p);
    m.complete(p, FOR_STMT);
}

/// `"(" uninitConst ":" expression ")"` — shared by `for` and `static for`
/// (`Parser.hs:584, 591`).
fn range_head(p: &mut Parser<'_>) {
    p.expect(L_PAREN);
    uninit_const(p);
    p.expect(COLON);
    exprs::expr(p, ExprMode::Full);
    p.expect(R_PAREN);
}

/// `uninitConst = "const" typ identifier` (`Parser.hs:451`). The `const` is
/// mandatory and wraps the parsed type in a `ConstF`, so it is a real
/// [`CONST_TYPE`] here too.
fn uninit_const(p: &mut Parser<'_>) {
    let m = p.start();
    if p.at(CONST_KW) {
        let t = p.start();
        p.bump(CONST_KW);
        types::type_(p);
        t.complete(p, CONST_TYPE);
    } else {
        p.error("expected `const` — a loop induction variable is always const");
        types::type_(p);
    }
    paths::name(p);
    m.complete(p, VAR_DECL);
}

fn do_while_stmt(p: &mut Parser<'_>, m: Marker) {
    p.bump(DO_KW);
    stmt(p);
    p.expect(WHILE_KW);
    p.expect(L_PAREN);
    exprs::expr(p, ExprMode::Full);
    p.expect(R_PAREN);
    // No trailing `;` in the production (`Parser.hs:587-589`); one that is
    // written becomes a stray `EMPTY_STMT`.
    m.complete(p, DO_WHILE_STMT);
}

// ------------------------------------------------------------------ branching

fn if_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(IF_KW);
    p.expect(L_PAREN);
    exprs::expr(p, ExprMode::Full);
    p.expect(R_PAREN);
    stmt(p);
    if p.at(ELSE_KW) {
        let e = p.start();
        p.bump(ELSE_KW);
        stmt(p);
        e.complete(p, ELSE_BRANCH);
    }
    m.complete(p, IF_STMT);
}

fn switch_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(SWITCH_KW);
    p.expect(L_PAREN);
    exprs::expr(p, ExprMode::Full);
    p.expect(R_PAREN);
    p.expect(L_CURLY);
    while !p.at_end() && !p.at(R_CURLY) {
        let pos = p.token_pos();
        switch_block(p);
        if p.token_pos() == pos {
            p.err_and_bump("expected `case`, `default` or `}`");
        }
    }
    p.expect(R_CURLY);
    m.complete(p, SWITCH_STMT);
}

/// `switchBlock` (`Parser.hs:601-604`).
fn switch_block(p: &mut Parser<'_>) {
    match p.current() {
        CASE_KW => {
            let m = p.start();
            p.bump(CASE_KW);
            exprs::expr(p, ExprMode::Full);
            p.expect(COLON);
            case_body(p);
            m.complete(p, SWITCH_CASE);
        }
        DEFAULT_KW => {
            let m = p.start();
            p.bump(DEFAULT_KW);
            p.expect(COLON);
            case_body(p);
            m.complete(p, SWITCH_DEFAULT);
        }
        STATIC_KW if p.nth_at(1, IF_KW) => static_if(p, switch_block),
        _ => {}
    }
}

/// The statements of a `case`, then its mandatory `break;`.
///
/// `localScoped (many statementOrDecl)` (`Parser.hs:602-603`): a case body is
/// one of the four places that does admit a declaration.
fn case_body(p: &mut Parser<'_>) {
    while !p.at_end() && !p.at_ts(CASE_END) {
        let pos = p.token_pos();
        stmt_or_decl(p);
        if p.token_pos() == pos {
            p.err_and_bump("expected a statement");
        }
    }
    let m = p.start();
    if p.eat(BREAK_KW) {
        p.expect(SEMICOLON);
    } else {
        p.error("expected `break;` — every `switch` case must end with one");
    }
    m.complete(p, BREAK_STMT);
}

// ----------------------------------------------------------------- attributed

/// A statement introduced by `[[ ... ]]`.
///
/// Four constructs compete for the same opening bracket, and the reference
/// separates them by which attribute names each permits: `annotatedStmt`
/// (`Parser.hs:563-567`), then — through `try (expression <* semi)` — a call
/// with `callModifiers` (`:871-876`), then `loop` (`:580-589`), then a local
/// variable whose *type* carries memory attributes (`:529`).
///
/// Only one name is shared: `fifo_depth` is both a loop attribute and a call
/// modifier. The loop branch is therefore additionally guarded on the `for` or
/// `do` that must follow, which makes the four tests disjoint and lets them run
/// in whichever order reads best.
fn attributed(p: &mut Parser<'_>, decls: bool) {
    if attrs::at_attrs(p, attrs::STMT_ATTRS) {
        let m = p.start();
        attrs::opt_attrs(p, attrs::STMT_ATTRS);
        // `annotatedStmt = Annotated <$> try stmtAttr <*> statement`
        // (`Parser.hs:567`) — a `statement`, so `[[schedule(1)]] uint32 x;` is
        // an error however the surrounding position is spelled.
        stmt(p);
        m.complete(p, ANNOTATED_STMT);
        return;
    }
    if let Some(end) = attrs::attrs_end(p, attrs::LOOP_ATTRS)
        && matches!(p.nth(end), FOR_KW | DO_KW)
    {
        let m = p.start();
        attrs::opt_attrs(p, attrs::LOOP_ATTRS);
        match p.current() {
            FOR_KW => for_stmt(p, m),
            _ => do_while_stmt(p, m),
        }
        return;
    }
    if attrs::at_attrs(p, attrs::CALL_ATTRS) {
        expr_stmt(p);
        return;
    }
    if !decls {
        p.error(DECL_IN_STMT);
    }
    items::decl(p, Ctx::Body);
}

// ---------------------------------------------------------------- expressions

/// `incDecStmt` / `assignStmt` / `expression ";"` (`Parser.hs:607-638`).
///
/// The target of `=`, of a compound assignment and of `++`/`--` is an
/// `lvalue` (`Parser.hs:803-805`), not an expression: `namedLValue` — a
/// qualified identifier — followed by an `access` chain of `[e]` and `.name`
/// (`:811-819`). A call, a literal, a parenthesized expression, a `cast<>` or
/// anything the operator table built can never be one.
fn expr_stmt(p: &mut Parser<'_>) {
    // Prefix `--x` / `++x`. `operator s *> lvalue` (`Parser.hs:612`): the
    // operand is a bare lvalue with no operator table behind it, so
    // `++x + 1;` is an error and *not* `x = (x + 1) + 1`.
    if at_inc_dec(p) {
        let m = p.start();
        p.bump_any();
        p.bump_any();
        lvalue(p);
        p.expect(SEMICOLON);
        m.complete(p, ASSIGN_STMT);
        return;
    }
    let Some(cm) = exprs::expr(p, ExprMode::Full) else {
        // The expression parser already reported and, where it could, consumed.
        return;
    };
    if let Some(tokens) = at_assign_op(p) {
        if !is_lvalue(cm.kind()) {
            p.error(NOT_AN_LVALUE);
        }
        let m = cm.precede(p);
        for _ in 0..tokens {
            p.bump_any();
        }
        exprs::expr(p, ExprMode::Full);
        p.expect(SEMICOLON);
        m.complete(p, ASSIGN_STMT);
    } else if at_inc_dec(p) {
        // Postfix `x++` / `x--`: `try (lvalue <* operator s)` (`:612`).
        if !is_lvalue(cm.kind()) {
            p.error(NOT_AN_LVALUE);
        }
        let m = cm.precede(p);
        p.bump_any();
        p.bump_any();
        p.expect(SEMICOLON);
        m.complete(p, ASSIGN_STMT);
    } else {
        let m = cm.precede(p);
        p.expect(SEMICOLON);
        m.complete(p, EXPR_STMT);
    }
}

const NOT_AN_LVALUE: &str = "this is not assignable — an assignment target is a \
                             name, a field or an element of one";

/// The node kinds `lvalue` can produce: a path, and the `[e]` / `.name` chain
/// `access` wraps around one.
fn is_lvalue(kind: SyntaxKind) -> bool {
    matches!(kind, PATH | INDEX_EXPR | FIELD_EXPR)
}

/// `lvalue` (`Parser.hs:803-805`), parsed directly rather than checked
/// afterwards, because `++`'s operand has no expression parser behind it at
/// all.
fn lvalue(p: &mut Parser<'_>) {
    if !p.at_ts(paths::PATH_FIRST) {
        p.err_recover(NOT_AN_LVALUE, super::STMT_RECOVERY);
        return;
    }
    let mut cm = paths::path(p, AngleCtx::Expr);
    loop {
        cm = match p.current() {
            // `[[` is an attribute group, never an index.
            L_BRACK if !p.nth_at(1, L_BRACK) => {
                let m = cm.precede(p);
                p.bump(L_BRACK);
                exprs::expr(p, ExprMode::Full);
                p.expect(R_BRACK);
                m.complete(p, INDEX_EXPR)
            }
            DOT => {
                let m = cm.precede(p);
                p.bump(DOT);
                paths::name_ref(p);
                m.complete(p, FIELD_EXPR)
            }
            _ => return,
        };
    }
}

fn at_inc_dec(p: &Parser<'_>) -> bool {
    match p.current() {
        PLUS => at_glued(p, PLUS, PLUS),
        MINUS => at_glued(p, MINUS, MINUS),
        _ => false,
    }
}

/// How many tokens the assignment operator at the cursor spans, if any.
///
/// `=` is one token; every compound form is two, because the lexer never
/// merges them. There is deliberately no `<<=` or `>>=`.
fn at_assign_op(p: &Parser<'_>) -> Option<usize> {
    let op = p.current();
    match op {
        EQ => Some(1),
        PLUS | MINUS | STAR | SLASH | PERCENT | AMP | PIPE | CARET | AMP2 | PIPE2 | CARET2
            if at_glued(p, op, EQ) =>
        {
            Some(2)
        }
        _ => None,
    }
}

/// True when `first` is immediately followed by `second` with no gap.
///
/// The reference reads `+=`, `++`, `&&=` and friends with `L.symbol` over raw
/// characters (`Parser.hs:608-609, 616-629`), so the lexer never builds them
/// and there is no `SyntaxKind` to ask for. `at_operator` supplies the missing
/// adjacency test: `operator o` fails exactly when the next character is glued
/// and is one of `o`'s own characters or `=` (`Parser/Lexer.hs:262`) — which is
/// precisely the set of pairs formed here, since `second` is always `=` or a
/// repeat of `first`.
fn at_glued(p: &Parser<'_>, first: SyntaxKind, second: SyntaxKind) -> bool {
    p.at(first) && p.nth_at(1, second) && !p.at_operator(first)
}
