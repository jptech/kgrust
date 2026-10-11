//! The parser's view of the token stream.
//!
//! Trivia is stripped here — the parser must never see a comment — but the
//! *adjacency* of the surviving tokens is preserved, because Kanagawa's angle
//! brackets are disambiguated by whitespace. `Foo<Bar>>` closes two template
//! argument lists; `a >> b` is a shift. The only thing that distinguishes
//! `> >` from `>>` is the gap, so [`Input::glued`] carries it.

use crate::SyntaxKind;

#[derive(Debug, Default)]
pub struct Input<'a> {
    kind: Vec<SyntaxKind>,
    /// `glued[i]` is true when token `i + 1` starts at exactly the end of token
    /// `i` in the source, with no trivia between them.
    glued: Vec<bool>,
    text: Vec<&'a str>,
    /// Indices of `<` tokens the caller has proven are comparisons — see
    /// [`Input::mark_comparison`]. Sorted, and almost always empty.
    comparisons: Vec<usize>,
}

impl<'a> Input<'a> {
    /// Builds the parser's view of a lexed file: trivia dropped, adjacency kept.
    ///
    /// Note that this cannot use the lexer's own `glued`, which compares
    /// *raw* neighbours. The token stream is lossless and therefore contiguous,
    /// so raw adjacency is trivially true for every token — including one
    /// followed by whitespace. What the parser needs is adjacency to the next
    /// **non-trivia** token, because that is what separates `>>` from `> >`,
    /// and `int<2> == 2` from `int<2>= 2`.
    pub fn from_lexed(lexed: &crate::LexedStr<'a>) -> Input<'a> {
        let mut input = Input::default();
        let mut prev_end: Option<kg_base::TextSize> = None;
        for i in 0..lexed.len() {
            let kind = lexed.kind(i);
            if kind.is_trivia() {
                continue;
            }
            let range = lexed.range(i);
            if let Some(end) = prev_end {
                // Retroactively record whether the previous token was glued to
                // this one; `push` cannot know its successor yet.
                input.set_last_glued(end == range.start());
            }
            input.push(kind, lexed.text(i), false);
            prev_end = Some(range.end());
        }
        input
    }

    /// Sets the glue flag of the most recently pushed token.
    fn set_last_glued(&mut self, glued: bool) {
        if let Some(last) = self.glued.last_mut() {
            *last = glued;
        }
    }

    pub fn push(&mut self, kind: SyntaxKind, text: &'a str, glued_to_next: bool) {
        debug_assert!(!kind.is_trivia(), "trivia must be stripped before the parser sees it");
        self.kind.push(kind);
        self.text.push(text);
        self.glued.push(glued_to_next);
    }

    /// Kind of the `idx`-th token, or `EOF` past the end.
    ///
    /// There is no `EOF` variant in [`SyntaxKind`]; past-the-end reads return
    /// `__LAST`, which no real token can be, so `at(...)` is false for every
    /// real kind and the parser terminates naturally.
    #[inline]
    pub fn kind(&self, idx: usize) -> SyntaxKind {
        self.kind.get(idx).copied().unwrap_or(SyntaxKind::__LAST)
    }

    #[inline]
    pub fn text(&self, idx: usize) -> &'a str {
        self.text.get(idx).copied().unwrap_or("")
    }

    /// True when token `idx` and `idx + 1` are adjacent in the source.
    #[inline]
    pub fn glued(&self, idx: usize) -> bool {
        self.glued.get(idx).copied().unwrap_or(false)
    }

    /// First character of token `idx + 1` when the two are adjacent.
    ///
    /// This is exactly what the reference's
    /// `operator o = string o *> notFollowedBy (oneOf (o ++ "="))`
    /// (`Parser/Lexer.hs:262`) inspects.
    #[inline]
    pub fn next_char_if_glued(&self, idx: usize) -> Option<char> {
        self.glued(idx).then(|| self.text(idx + 1).chars().next())?
    }

    /// Records that the `<` at token `idx` is a comparison, whatever the
    /// angle scan would say.
    ///
    /// The reference reads `name <` as a template argument list only when
    /// `findSymbol` returns a template *while parsing* (`Parser.hs:652-690`).
    /// This parser has no symbol table and decides by lookahead instead,
    /// which cannot tell `g(x < y, z > -1)` — two comparisons over a value
    /// `x` — from `g(x<y, z> - 1)`. A caller that *can* tell, because it has
    /// seen `x` bound as a value, re-parses with the `<` marked here
    /// (`kg_syntax`'s binding-aware re-read).
    pub fn mark_comparison(&mut self, idx: usize) {
        if let Err(at) = self.comparisons.binary_search(&idx) {
            self.comparisons.insert(at, idx);
        }
    }

    /// Whether [`Input::mark_comparison`] marked token `idx`.
    #[inline]
    pub fn is_marked_comparison(&self, idx: usize) -> bool {
        !self.comparisons.is_empty() && self.comparisons.binary_search(&idx).is_ok()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.kind.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }
}
