//! The parsing API: token inspection, error recovery, and the marker protocol.
//!
//! The grammar modules are written entirely against this type. They never touch
//! the event vector directly and never see trivia.

use std::cell::Cell;

use crate::{Event, Input, SyntaxKind, TokenSet};
use SyntaxKind::*;

/// How many parser steps may pass with no forward progress before we conclude
/// the grammar has a bug and abort. Generous enough that no legitimate parse
/// hits it; small enough that a fuzzer finding a loop fails fast.
const STEP_LIMIT: u32 = 15_000_000;

pub struct Parser<'t> {
    inp: &'t Input<'t>,
    pos: usize,
    events: Vec<Event>,
    steps: Cell<u32>,
}

impl std::fmt::Debug for Parser<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parser").field("pos", &self.pos).field("events", &self.events.len()).finish()
    }
}

impl<'t> Parser<'t> {
    pub fn new(inp: &'t Input<'t>) -> Parser<'t> {
        Parser { inp, pos: 0, events: Vec::new(), steps: Cell::new(0) }
    }

    pub fn finish(self) -> Vec<Event> {
        self.events
    }

    // ------------------------------------------------------------ inspection

    #[inline]
    pub fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    /// Kind of the token `n` positions ahead. Past the end this is a kind no
    /// real token has, so every `at` check is false and parsing terminates.
    pub fn nth(&self, n: usize) -> SyntaxKind {
        // Progress guard: any lookahead is a "step", so a rule that neither
        // bumps nor returns will trip this rather than hang.
        let steps = self.steps.get();
        assert!(steps <= STEP_LIMIT, "the parser is stuck at token {}", self.pos);
        self.steps.set(steps + 1);
        self.inp.kind(self.pos + n)
    }

    #[inline]
    pub fn at(&self, kind: SyntaxKind) -> bool {
        self.nth_at(0, kind)
    }

    #[inline]
    pub fn nth_at(&self, n: usize, kind: SyntaxKind) -> bool {
        self.nth(n) == kind
    }

    #[inline]
    pub fn at_ts(&self, set: TokenSet) -> bool {
        set.contains(self.current())
    }

    #[inline]
    pub fn nth_at_ts(&self, n: usize, set: TokenSet) -> bool {
        set.contains(self.nth(n))
    }

    /// Source text of the current token.
    #[inline]
    pub fn token_text(&self) -> &'t str {
        self.nth_text(0)
    }

    #[inline]
    pub fn nth_text(&self, n: usize) -> &'t str {
        self.inp.text(self.pos + n)
    }

    /// True when the current token is an identifier spelled exactly `text`.
    ///
    /// Attribute names (`call_rate`, `fifo_depth`, ...) and format specifiers
    /// are ordinary identifiers in the reference grammar, so the parser has to
    /// match on text for them.
    pub fn at_contextual_kw(&self, text: &str) -> bool {
        self.at(IDENT) && self.token_text() == text
    }

    #[inline]
    pub fn at_end(&self) -> bool {
        self.pos >= self.inp.len()
    }

    /// Current byte position in the token stream. Only for progress assertions.
    #[inline]
    pub fn token_pos(&self) -> usize {
        self.pos
    }

    // ------------------------------------------------ operator disambiguation

    /// True when the current token opens or closes a *template* angle bracket.
    ///
    /// The reference distinguishes template angles from comparison operators
    /// lexically: `_angle o = string o *> notFollowedBy (char '=')`
    /// (`Parser/Lexer.hs:272-273`) forbids only a following `=`, while the
    /// binary `<` operator additionally forbids a following `<`. So `Foo<X>>`
    /// closes two argument lists with no token splitting, but `Foo<X>=y` does
    /// not parse — reproduced here exactly.
    pub fn at_angle(&self, kind: SyntaxKind) -> bool {
        self.nth_at_angle(0, kind)
    }

    /// Whether the input marks the current `<` a comparison
    /// ([`Input::mark_comparison`]).
    pub(crate) fn at_marked_comparison(&self) -> bool {
        self.inp.is_marked_comparison(self.pos)
    }

    /// Whether token `n` and token `n + 1` are adjacent in the source, with no
    /// trivia between them — for the character-level lexemes the reference
    /// reads where this lexer splits tokens (`dotSepName`).
    pub fn nth_glued(&self, n: usize) -> bool {
        self.inp.glued(self.pos + n)
    }

    /// Whether the current token is adjacent to the one before it.
    pub fn nth_glued_back(&self) -> bool {
        self.pos > 0 && self.inp.glued(self.pos - 1)
    }

    pub fn nth_at_angle(&self, n: usize, kind: SyntaxKind) -> bool {
        debug_assert!(matches!(kind, L_ANGLE | R_ANGLE));
        self.nth_at(n, kind) && self.inp.next_char_if_glued(self.pos + n) != Some('=')
    }

    /// True when the current token is the operator `kind` under the reference's
    /// maximal-munch rule.
    ///
    /// `operator o = string o *> notFollowedBy (oneOf (o ++ "="))`
    /// (`Parser/Lexer.hs:262`): an operator does not match if the very next
    /// character is one of its own characters or `=`. That is what rejects
    /// `a === b`, `a <<< b` and `a +++ b`.
    pub fn at_operator(&self, kind: SyntaxKind) -> bool {
        self.nth_at_operator(0, kind)
    }

    pub fn nth_at_operator(&self, n: usize, kind: SyntaxKind) -> bool {
        let Some(text) = kind.static_text() else { return false };
        if !self.nth_at(n, kind) {
            return false;
        }
        match self.inp.next_char_if_glued(self.pos + n) {
            Some(c) => c != '=' && !text.contains(c),
            None => true,
        }
    }

    /// True when the next two tokens form the composite operator `kind`
    /// (`<=`, `>=`, `<<`, `>>`), which the lexer deliberately never builds.
    pub fn at_composite(&self, kind: SyntaxKind) -> bool {
        self.nth_at_composite(0, kind)
    }

    pub fn nth_at_composite(&self, n: usize, kind: SyntaxKind) -> bool {
        let (first, second, text) = match kind {
            LTEQ => (L_ANGLE, EQ, "<="),
            GTEQ => (R_ANGLE, EQ, ">="),
            SHL => (L_ANGLE, L_ANGLE, "<<"),
            SHR => (R_ANGLE, R_ANGLE, ">>"),
            _ => return false,
        };
        if !(self.nth_at(n, first) && self.inp.glued(self.pos + n) && self.nth_at(n + 1, second)) {
            return false;
        }
        // Same `notFollowedBy (oneOf (o ++ "="))` rule, applied past the pair.
        match self.inp.next_char_if_glued(self.pos + n + 1) {
            Some(c) => c != '=' && !text.contains(c),
            None => true,
        }
    }

    /// Consumes a composite operator as one tree token.
    pub fn bump_composite(&mut self, kind: SyntaxKind) {
        assert!(self.at_composite(kind), "not at a composite {kind:?}");
        self.push_event(Event::Token { kind, n_raw_tokens: 2 });
        self.pos += 2;
    }

    // -------------------------------------------------------------- consuming

    pub fn bump(&mut self, kind: SyntaxKind) {
        assert_eq!(self.current(), kind, "expected to bump {kind:?}");
        self.do_bump(kind, 1);
    }

    pub fn bump_any(&mut self) {
        if self.at_end() {
            return;
        }
        let kind = self.current();
        self.do_bump(kind, 1);
    }

    /// Consumes the current token but records it under a different kind.
    ///
    /// Used where one lexeme plays several grammatical roles — an `IDENT` that
    /// turns out to name an attribute, for instance.
    pub fn bump_remap(&mut self, kind: SyntaxKind) {
        self.do_bump(kind, 1);
    }

    pub fn eat(&mut self, kind: SyntaxKind) -> bool {
        if !self.at(kind) {
            return false;
        }
        self.do_bump(kind, 1);
        true
    }

    /// Consumes `kind`, or reports "expected ..." without consuming anything.
    pub fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(format!("expected {}", describe(kind)));
        false
    }

    fn do_bump(&mut self, kind: SyntaxKind, n_raw_tokens: u8) {
        self.pos += n_raw_tokens as usize;
        self.push_event(Event::Token { kind, n_raw_tokens });
    }

    // ---------------------------------------------------------------- errors

    pub fn error(&mut self, msg: impl Into<String>) {
        self.push_event(Event::Error { msg: msg.into() });
    }

    /// Reports an error and consumes the offending token inside an `ERROR`
    /// node, guaranteeing forward progress.
    pub fn err_and_bump(&mut self, msg: &str) {
        self.err_recover(msg, TokenSet::EMPTY);
    }

    /// Reports an error and consumes one token unless it is in `recovery`.
    ///
    /// Stopping at a recovery token lets an enclosing rule resynchronize — the
    /// whole point of an error-recovering parser, and the reason a language
    /// server can still offer completion in a half-typed file.
    pub fn err_recover(&mut self, msg: &str, recovery: TokenSet) {
        if self.at_ts(recovery) || self.at_end() {
            self.error(msg);
            return;
        }
        let m = self.start();
        self.error(msg);
        self.bump_any();
        m.complete(self, ERROR);
    }

    // --------------------------------------------------------------- markers

    pub fn start(&mut self) -> Marker {
        let pos = self.events.len() as u32;
        self.push_event(Event::tombstone());
        Marker::new(pos)
    }

    fn push_event(&mut self, event: Event) {
        self.events.push(event);
    }
}

/// An open node whose kind is not yet decided.
///
/// Must be completed or abandoned; dropping one is a grammar bug and panics in
/// debug builds.
#[derive(Debug)]
pub struct Marker {
    pos: u32,
    live: bool,
}

impl Marker {
    fn new(pos: u32) -> Marker {
        Marker { pos, live: true }
    }

    /// Closes the node with `kind`.
    pub fn complete(mut self, p: &mut Parser<'_>, kind: SyntaxKind) -> CompletedMarker {
        self.live = false;
        let idx = self.pos as usize;
        match &mut p.events[idx] {
            slot @ Event::Tombstone => *slot = Event::Start { kind, forward_parent: None },
            other => unreachable!("marker slot held {other:?}"),
        }
        p.push_event(Event::Finish);
        CompletedMarker { pos: self.pos, kind }
    }

    /// Discards the node, as if `start()` had never been called.
    pub fn abandon(mut self, p: &mut Parser<'_>) {
        self.live = false;
        let idx = self.pos as usize;
        // Only a marker opened last can vanish outright; anything earlier stays
        // as a tombstone that `process` skips.
        if idx + 1 == p.events.len() {
            match p.events.pop() {
                Some(Event::Tombstone) => (),
                other => unreachable!("abandoning a marker that held {other:?}"),
            }
        }
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        if self.live && !std::thread::panicking() {
            panic!("a Marker was dropped without complete() or abandon()");
        }
    }
}

/// A node that has been closed, and can still be wrapped in a parent.
#[derive(Debug, Clone, Copy)]
pub struct CompletedMarker {
    pos: u32,
    kind: SyntaxKind,
}

impl CompletedMarker {
    /// Opens a new node that will become this one's parent.
    ///
    /// This is what makes a plain recursive-descent expression parser possible:
    /// after parsing `a` you discover a `+`, and only then does `a` need to
    /// become the left child of a `BIN_EXPR`.
    pub fn precede(self, p: &mut Parser<'_>) -> Marker {
        let new_pos = p.start();
        let idx = self.pos as usize;
        match &mut p.events[idx] {
            Event::Start { forward_parent, .. } => {
                *forward_parent = Some(new_pos.pos - self.pos);
            }
            other => unreachable!("preceding a marker that held {other:?}"),
        }
        new_pos
    }

    pub fn kind(self) -> SyntaxKind {
        self.kind
    }
}

/// Human-readable name of a kind, for "expected ..." messages.
///
/// All frontend errors are reported as `Error 1` by the reference
/// (`compiler/hs/app/Main.hs:99`), so the wording is free — but it is what a
/// language-server user actually reads, so it should be good.
pub fn describe(kind: SyntaxKind) -> String {
    match kind.static_text() {
        Some(text) => format!("`{text}`"),
        None => match kind {
            IDENT => "an identifier".to_owned(),
            INT_NUMBER => "an integer literal".to_owned(),
            FLOAT_NUMBER => "a floating-point literal".to_owned(),
            STRING | STRING_TEXT => "a string literal".to_owned(),
            INT_TYPE_TOKEN | UINT_TYPE_TOKEN => "an integer type".to_owned(),
            other => format!("{other:?}"),
        },
    }
}
