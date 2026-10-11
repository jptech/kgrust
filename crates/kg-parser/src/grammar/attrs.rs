//! `[[ ... ]]` attribute groups.
//!
//! Two facts from the reference make this more than a bracket matcher.
//!
//! First, every attribute *position* has its own closed list of permitted
//! names (`Parser.hs:381, 384-397, 505-511, 563-565, 581, 874-876`), and
//! `optionalAttributes` (`Parser.hs:169`) wraps the group in `try`: a group
//! naming something outside that list is **not consumed** and yields nothing,
//! so the construct that follows fails at the `[`. There is deliberately no
//! "unknown attribute" diagnostic (grammar.md §9 trap 4). Reproducing that is
//! why [`opt_attrs`] scans a group with plain lookahead before committing to
//! it, and why the caller can ask "are these *memory* attributes or *loop*
//! attributes?" and get a truthful answer without backtracking.
//!
//! Second, `[[a]] [[b]]` is one group, not two: the separator is
//! `comma <|> try ("]]" "[[")` (`Parser.hs:165`), which is why
//! `[[async]] [[no_backpressure]] void f()` (`test/logic/function_call.k:34`)
//! parses at all.

use super::lookahead::AngleCtx;
use super::{ExprMode, exprs, paths};
use crate::{Parser, SyntaxKind};
use SyntaxKind::*;

/// The argument shape of one attribute (`Parser.hs:171-177`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Shape {
    /// `flagAttr` — the bare name.
    Flag,
    /// `intAttr` — `name(expression)`.
    Int,
    /// `identAttr` — `name(qualifiedIdentifier)`.
    Ident,
    /// `strAttr` — `name("...")`.
    Str,
}

use Shape::{Flag, Ident, Int, Str};

/// One permitted attribute: its source spelling and its argument shape.
///
/// The spelling is an ordinary identifier, not a keyword — the reference reads
/// it with `L.symbol` (`Parser.hs:171-177`), so `pure` and `memory` stay
/// available as user identifiers everywhere else.
#[derive(Debug)]
pub(crate) struct Spec(pub(crate) &'static str, pub(crate) Shape);

/// `funcTypeAttr` (`Parser.hs:384-392`) — legal on a function *and* on a
/// function type.
pub(crate) const FN_TYPE_ATTRS: &[Spec] = &[
    Spec("async", Flag),
    Spec("no_backpressure", Flag),
    Spec("pipelined", Flag),
    Spec("unordered", Flag),
    Spec("latency", Int),
    Spec("max_threads", Int),
    Spec("thread_rate", Int),
];

/// `paramAttr` (`Parser.hs:381`, `:556`).
pub(crate) const PARAM_ATTRS: &[Spec] = &[Spec("last", Flag)];

/// `memoryAttr` (`Parser.hs:505-511`) — the only attributes a *type* may
/// carry, and only on the array form.
pub(crate) const MEMORY_ATTRS: &[Spec] = &[
    Spec("initialize", Flag),
    Spec("memory", Flag),
    Spec("non_replicated", Flag),
    Spec("quad_port", Flag),
    Spec("ecc", Ident),
];

/// `stmtAttr` (`Parser.hs:563-565`).
pub(crate) const STMT_ATTRS: &[Spec] = &[Spec("schedule", Int)];

/// `loopAttr` (`Parser.hs:581`).
pub(crate) const LOOP_ATTRS: &[Spec] = &[
    Spec("unordered", Flag),
    Spec("reorder_by_looping", Flag),
    Spec("fifo_depth", Int),
];

/// `callModifiers` (`Parser.hs:874-876`).
pub(crate) const CALL_ATTRS: &[Spec] = &[
    Spec("call_rate", Int),
    Spec("fifo_depth", Int),
    Spec("transaction_size", Int),
];

/// Everything an *item*-leading attribute group may say: `strAttr Rename` from
/// `export` / `extern` / class-variable position (`Parser.hs:278, 280, 462`)
/// plus all of `funcAttr` (`:394-397`).
///
/// The reference keeps those two lists apart, but which kind of item this is
/// only becomes clear after its attributes, its type and its name have been
/// read, so they are checked as one list here and separated during lowering.
/// Type-level (`memory`, `ecc`, …) attributes are deliberately absent: they
/// belong to the array type that follows, not to the item.
pub(crate) const ITEM_ATTRS: &[Spec] = &[
    Spec("name", Str),
    Spec("async", Flag),
    Spec("no_backpressure", Flag),
    Spec("pipelined", Flag),
    Spec("unordered", Flag),
    Spec("latency", Int),
    Spec("max_threads", Int),
    Spec("thread_rate", Int),
    Spec("reset", Flag),
    Spec("pure", Flag),
];

/// True when a well-formed group drawn from `permitted` starts at the cursor.
///
/// This is the whole trick: the caller asks the question for each candidate
/// list in the reference's own order and takes the first that answers yes,
/// which reproduces `optionalAttributes`' ordered choice without backtracking.
///
/// One group is all any caller needs to see, and looking at only one is what
/// keeps a run of adjacent groups linear: asking about the whole run after
/// every group parsed would re-walk the tail each time.
pub(crate) fn at_attrs(p: &Parser<'_>, permitted: &[Spec]) -> bool {
    scan_group(p, 0, permitted).is_some()
}

/// Index of the token just past such a group, for callers that also need to
/// see what follows it — telling `[[fifo_depth(4)]] do {…}` (a loop) from
/// `[[fifo_depth(4)]] f(x);` (a call) needs exactly that.
pub(crate) fn attrs_end(p: &Parser<'_>, permitted: &[Spec]) -> Option<usize> {
    scan(p, 0, permitted)
}

/// Parses `[[ ... ]]` groups drawn from `permitted`, or nothing at all.
///
/// Returns whether an [`ATTR_LIST`] was produced.
pub(crate) fn opt_attrs(p: &mut Parser<'_>, permitted: &[Spec]) -> bool {
    if !at_attrs(p, permitted) {
        return false;
    }
    let m = p.start();
    // `[[a]] [[b]]` is one list, so keep going while the next group also
    // matches (`Parser.hs:165`). The check is one group wide, so N adjacent
    // groups cost one scan each rather than one scan of the whole remaining
    // run each — `attributes` parses the run in a single pass too.
    loop {
        group(p, permitted);
        if scan_group(p, 0, permitted).is_none() {
            break;
        }
    }
    m.complete(p, ATTR_LIST);
    true
}

fn group(p: &mut Parser<'_>, permitted: &[Spec]) {
    p.bump(L_BRACK);
    p.bump(L_BRACK);
    loop {
        attr(p, permitted);
        if !p.eat(COMMA) {
            break;
        }
    }
    p.expect(R_BRACK);
    p.expect(R_BRACK);
}

fn attr(p: &mut Parser<'_>, permitted: &[Spec]) {
    let m = p.start();
    match permitted.iter().find(|s| p.at_contextual_kw(s.0)).map(|s| s.1) {
        Some(shape) => {
            p.bump(IDENT);
            if shape != Flag {
                let args = p.start();
                p.expect(L_PAREN);
                match shape {
                    Int => {
                        exprs::expr(p, ExprMode::Full);
                    }
                    Ident => {
                        if p.at(IDENT) {
                            paths::path(p, AngleCtx::Type);
                        } else {
                            p.error("expected an identifier");
                        }
                    }
                    // `strAttr` takes `stringLiteral` (`Parser.hs:177, 821`),
                    // never an interpolated one — `[[name(10)]]` is a syntax
                    // error (`test/syntax/name-attr.k:37-41`).
                    Str => {
                        if p.at(STRING) {
                            let lit = p.start();
                            p.bump(STRING);
                            lit.complete(p, LITERAL);
                        } else if p.at(STRING_TEXT) {
                            // `stringLiteral` reads a `{` as text
                            // (`Parser/Lexer.hs:161-162`), but the lexer has
                            // already split one off as a hole. Kept as one
                            // `LITERAL` whose text is the whole string; no
                            // corpus attribute writes a brace.
                            let lit = p.start();
                            super::exprs::interpolated_string(p);
                            lit.complete(p, LITERAL);
                        } else {
                            p.error("expected a string literal");
                        }
                    }
                    Flag => unreachable!("flag attributes take no arguments"),
                }
                p.expect(R_PAREN);
                args.complete(p, ATTR_ARGS);
            }
        }
        // Unreachable after a successful scan; kept so a hand call cannot spin.
        None => p.error("expected an attribute"),
    }
    m.complete(p, ATTR);
}

// ---------------------------------------------------------------- the scan

/// Index just past a run of groups starting at `from`, or `None`.
fn scan(p: &Parser<'_>, from: usize, permitted: &[Spec]) -> Option<usize> {
    let mut i = scan_group(p, from, permitted)?;
    while let Some(next) = scan_group(p, i, permitted) {
        i = next;
    }
    Some(i)
}

fn scan_group(p: &Parser<'_>, from: usize, permitted: &[Spec]) -> Option<usize> {
    if !(p.nth_at(from, L_BRACK) && p.nth_at(from + 1, L_BRACK)) {
        return None;
    }
    let mut i = from + 2;
    loop {
        i = scan_attr(p, i, permitted)?;
        if p.nth_at(i, COMMA) {
            i += 1;
            continue;
        }
        break;
    }
    (p.nth_at(i, R_BRACK) && p.nth_at(i + 1, R_BRACK)).then_some(i + 2)
}

fn scan_attr(p: &Parser<'_>, i: usize, permitted: &[Spec]) -> Option<usize> {
    if !p.nth_at(i, IDENT) {
        return None;
    }
    let text = p.nth_text(i);
    let spec = permitted.iter().find(|s| s.0 == text)?;
    match spec.1 {
        Flag => Some(i + 1),
        _ => skip_parens(p, i + 1),
    }
}

/// Skips a balanced `( ... )`, which is all the scan needs to know about an
/// attribute argument — the real parse re-reads it as an expression.
fn skip_parens(p: &Parser<'_>, i: usize) -> Option<usize> {
    if !p.nth_at(i, L_PAREN) {
        return None;
    }
    let mut depth = 0usize;
    let mut n = i;
    loop {
        match p.nth(n) {
            k if !k.is_token() => return None,
            L_PAREN => depth += 1,
            R_PAREN => {
                depth -= 1;
                if depth == 0 {
                    return Some(n + 1);
                }
            }
            _ => {}
        }
        n += 1;
    }
}
