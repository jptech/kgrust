//! The Kanagawa lexer: source text in, a flat lossless token stream out.
//!
//! There is no lexer in the reference compiler — megaparsec parses characters
//! directly — so every rule here is derived from the reference's *character*
//! grammar in `compiler/hs/lib/Language/Kanagawa/Parser/Lexer.hs` and the
//! literal/operator tables in `compiler/hs/lib/Language/Kanagawa/Parser.hs`.
//! Citations name those files and the lines they came from.
//!
//! Three properties are load-bearing for everything downstream:
//!
//! * **Lossless.** Every byte of input belongs to exactly one token, tokens are
//!   contiguous and in order, so concatenating [`LexedStr::text`] over the whole
//!   stream reproduces the input. Trivia (whitespace, comments) are tokens, not
//!   holes. The formatter and the LSP depend on this.
//! * **Total.** No input makes the lexer panic or drop text. Unterminated
//!   strings and comments become a token that runs to end-of-file plus an entry
//!   in [`LexedStr::errors`].
//! * **An interpolated string is several tokens.** `"a{x}b"` is `STRING_TEXT`
//!   `"a`, `{`, `x`, `}`, `STRING_END` `b"` — the hole's expression is lexed as
//!   ordinary code, so the parser builds it as an ordinary expression under an
//!   `INTERPOLATED_STRING` (`Parser.hs:883-903`). A string with no hole is still
//!   one `STRING`. See [`Lexer::string_piece`].
//! * **`<` and `>` are never merged.** The reference distinguishes the *binary*
//!   operator `<` (which fails when followed by `<` or `=`,
//!   `Parser/Lexer.hs:261-262`) from the *template* angle `<` (which fails only
//!   when followed by `=`, `Parser/Lexer.hs:272-273`). That asymmetry is why
//!   `reverse<uint<Width>>(x)` parses with no `>>` splitting hack. A lexer
//!   cannot know which one it is looking at, so it always emits single
//!   [`SyntaxKind::L_ANGLE`]/[`SyntaxKind::R_ANGLE`] and lets the parser merge
//!   adjacent pairs into `SHL`/`SHR`/`LTEQ`/`GTEQ` using [`LexedStr::glued`].
//!   The lexer never emits those four kinds.
//!
//! Every other multi-character operator is maximal munch: `::`, `->`, `==`,
//! `!=`, `&&`, `||`, `^^`. `[` and `]` stay single so the parser can rebuild
//! `[[ ... ]]` attribute groups from adjacency the same way.
//!
//! # Known divergences from the reference
//!
//! None of these is reachable from the 480-file reference corpus, but they are
//! real and worth knowing before trusting this on adversarial input:
//!
//! * **`int0x10`.** `intType` (`Parser/Lexer.hs:241-243`) tests the suffix with
//!   `readMaybe :: Maybe Word`, and GHC's `Read Word` also accepts `0x`/`0o`
//!   forms, so the reference reserves `int0x10` and `uint0o7` as well. Here the
//!   suffix must be plain decimal, matching what the *type* grammar can
//!   actually parse (`intSuffix`, `Parser/Lexer.hs:258-259`), so `int0x10` is a
//!   plain identifier.
//! * **`1e5`.** A fractional part is required for a float. Depending on the
//!   megaparsec version, `L.float` may also accept the exponent-only form.
//! * **`\&` and string gaps.** Accepted here, but `MP.charLiteral` runs on
//!   `base`'s `Char.readLitChar`, which implements neither. See
//!   [`Lexer::eat_escape`].
//! * **Identifier characters.** `char::is_alphabetic` is Unicode's `Alphabetic`
//!   property, which is slightly wider than GHC's `isAlpha` (`Lu Ll Lt Lm Lo`):
//!   it also admits `Nl` and `Other_Alphabetic`.
//! * **Doc comments are always classified, and doc blocks never nest.** The
//!   reference only looks for `|` / `<` markers when `Config.parseDocs` is set
//!   (`Parser/Lexer.hs:76-79`). The *kinds* being produced unconditionally is
//!   free — a `DOC_LINE_PRE` is as much trivia as a `LINE_COMMENT` — but the
//!   *extent* of a doc-marked block comment is not: with the flag clear the
//!   reference skips every block comment with `L.skipBlockCommentNested`
//!   (`:105-107`), so `/*| a /* b */ c */` is one comment there and two tokens
//!   plus stray `*` `/` here. One token stream cannot serve both modes; this
//!   one serves `parseDocs = True`, whose placement rules live in
//!   `kg-syntax::doc_comments`, and the flag-clear nesting difference is
//!   divergence #28 — deliberately unfixed until `Db::parse_module` passes
//!   `parse_docs` through, because the conformance harness grades
//!   `doc-comments.k`'s nested-comment assertion against the default parse
//!   until then.

use crate::SyntaxKind::{self, *};
use kg_base::{TextRange, TextSize};

#[cfg(test)]
mod tests;

/// The whole file, tokenized.
///
/// Tokens are stored struct-of-arrays: a kind per token and a start offset per
/// token plus one sentinel, so a token's text is the slice between consecutive
/// starts and no length field is needed.
#[derive(Debug)]
pub struct LexedStr<'a> {
    text: &'a str,
    kind: Vec<SyntaxKind>,
    /// `kind.len() + 1` entries. The last is `text.len()`, which makes
    /// `start[i + 1]` the end of token `i` for every valid `i`.
    start: Vec<u32>,
    error: Vec<LexError>,
}

/// A problem found while tokenizing, attached to the token it occurred in.
///
/// The token still exists and still carries its text — losslessness is never
/// traded for error reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    /// Index into the [`LexedStr`], not a byte offset.
    pub token: usize,
    pub msg: String,
}

impl<'a> LexedStr<'a> {
    pub fn new(text: &'a str) -> LexedStr<'a> {
        Lexer::new(text).run()
    }

    /// Number of tokens, trivia included.
    #[inline]
    pub fn len(&self) -> usize {
        self.kind.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    #[inline]
    pub fn kind(&self, i: usize) -> SyntaxKind {
        self.kind[i]
    }

    #[inline]
    pub fn text(&self, i: usize) -> &'a str {
        let (lo, hi) = (self.start[i] as usize, self.start[i + 1] as usize);
        &self.text[lo..hi]
    }

    #[inline]
    pub fn range(&self, i: usize) -> TextRange {
        TextRange::new(self.text_start(i), TextSize::new(self.start[i + 1]))
    }

    #[inline]
    pub fn text_start(&self, i: usize) -> TextSize {
        TextSize::new(self.start[i])
    }

    #[inline]
    pub fn as_str(&self) -> &'a str {
        self.text
    }

    #[inline]
    pub fn errors(&self) -> &[LexError] {
        &self.error
    }

    /// True when token `i + 1` starts exactly where token `i` ends.
    ///
    /// Because the stream is lossless this holds for every token that has a
    /// successor; the check is written as an offset comparison anyway so it
    /// stays honest if the representation ever grows gaps. What makes it useful
    /// is the pairing: the parser asks `glued(i) && kind(i + 1) == L_ANGLE`, and
    /// any intervening space is itself a `WHITESPACE` token at `i + 1`, so the
    /// kind test is what actually rejects `< <`.
    pub fn glued(&self, i: usize) -> bool {
        i + 1 < self.len() && self.range(i).end() == self.text_start(i + 1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (SyntaxKind, &'a str)> + '_ {
        (0..self.len()).map(|i| (self.kind(i), self.text(i)))
    }
}

// ---------------------------------------------------------------------------
// character classes
// ---------------------------------------------------------------------------

/// GHC's `Data.Char.isSpace`, which is what megaparsec's `space1` — the
/// whitespace parser at `Parser/Lexer.hs:100` — tests each character with.
///
/// Reproduced exactly rather than delegating to [`char::is_whitespace`] because
/// the two disagree on U+0085 NEXT LINE: GHC's `uc <= 0x377` fast path rejects
/// it, Rust's `White_Space` property accepts it.
fn is_space_hs(c: char) -> bool {
    let uc = c as u32;
    if uc <= 0x377 {
        // ' ', then \t \n \v \f \r as a contiguous run, then NBSP.
        uc == 0x20 || uc.wrapping_sub(0x9) <= 4 || uc == 0xa0
    } else {
        // GHC defers to `iswspace` here; the Unicode space characters above
        // U+0377 are the same set Rust reports.
        c.is_whitespace()
    }
}

/// Horizontal space: what megaparsec's `hspace` skips between a comment opener
/// and its doc marker (`Parser/Lexer.hs:110-111`). Newlines are excluded, which
/// is what keeps `//` on one line from picking up a `|` on the next.
fn is_hspace_hs(c: char) -> bool {
    is_space_hs(c) && c != '\n' && c != '\r'
}

/// `letterChar <|> char '_'` (`Parser/Lexer.hs:224-225`). megaparsec's
/// `letterChar` is `Data.Char.isAlpha`, so Unicode letters start identifiers.
fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

/// `isAlphaNumOrUnderscore` (`Parser/Lexer.hs:221-222`), Unicode-aware.
fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// The Haskell ASCII control-code mnemonics accepted by `MP.charLiteral`,
/// longest first so `\SOH` wins over `\SO` the way the Haskell Report requires.
const ASCII_MNEMONICS: [&str; 34] = [
    "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "DLE", "DC1", "DC2", "DC3", "DC4",
    "NAK", "SYN", "ETB", "CAN", "SUB", "ESC", "DEL", "BS", "HT", "LF", "VT", "FF", "CR", "SO",
    "SI", "EM", "FS", "GS", "RS", "US", "SP",
];

// ---------------------------------------------------------------------------
// the lexer
// ---------------------------------------------------------------------------

/// Which construct of an interpolated string the lexer is inside.
///
/// A stack because interpolations nest arbitrarily:
/// `"{"0x{addr:x8}",indentation}"` is a string whose hole holds an expression
/// holding another string holding another hole
/// (`library/processor/risc_v/trace.k:651`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// Between quotes: escapes apply, `{` opens a hole, `"` closes. `first`
    /// is the token index of the string's opening piece, which an
    /// unterminated string's error points at.
    Str { first: usize },
    /// Inside a hole: ordinary tokens. `depth` counts the braces opened
    /// inside it (an initializer list), so only the `}` at depth zero closes
    /// the hole.
    Hole { depth: u32 },
}

struct Lexer<'a> {
    text: &'a str,
    /// The same text as bytes. All of the lexer's decisions are on ASCII, so
    /// byte dispatch is safe; `pos` is nevertheless kept on a `char` boundary
    /// at all times so slicing can never panic.
    bytes: &'a [u8],
    pos: usize,
    kind: Vec<SyntaxKind>,
    start: Vec<u32>,
    error: Vec<LexError>,
    /// The interpolated strings the cursor is inside, innermost last.
    frames: Vec<Frame>,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Lexer<'a> {
        // Rough token density for Kanagawa sources, to size the vectors once.
        let guess = text.len() / 4 + 1;
        Lexer {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            kind: Vec::with_capacity(guess),
            start: Vec::with_capacity(guess + 1),
            error: Vec::new(),
            frames: Vec::new(),
        }
    }

    fn run(mut self) -> LexedStr<'a> {
        while self.pos < self.bytes.len() {
            let start = self.pos;
            let kind = self.next_token();
            debug_assert!(self.pos > start, "lexer failed to advance at {start}");
            self.kind.push(kind);
            self.start.push(start as u32);
        }
        // A hole still open at the end of the file: its string never closed.
        if let Some(first) = self.frames.iter().find_map(|f| match f {
            Frame::Str { first } => Some(*first),
            Frame::Hole { .. } => None,
        }) {
            self.error.push(LexError { token: first, msg: "unterminated string literal".into() });
        }
        self.start.push(self.text.len() as u32);
        LexedStr { text: self.text, kind: self.kind, start: self.start, error: self.error }
    }

    // --- cursor -----------------------------------------------------------

    /// The `char` at `pos`, or `None` at end of input.
    fn first_char(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn byte_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    /// Advances past one whole `char`, keeping `pos` on a UTF-8 boundary.
    fn bump(&mut self) {
        if let Some(c) = self.first_char() {
            self.pos += c.len_utf8();
        }
    }

    fn eat_while(&mut self, pred: fn(char) -> bool) {
        while let Some(c) = self.first_char() {
            if !pred(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }

    /// Records a diagnostic against the token that is about to be pushed.
    fn error(&mut self, msg: impl Into<String>) {
        self.error.push(LexError { token: self.kind.len(), msg: msg.into() });
    }

    // --- dispatch ---------------------------------------------------------

    fn next_token(&mut self) -> SyntaxKind {
        if let Some(Frame::Str { .. }) = self.frames.last() {
            return self.string_piece();
        }
        let kind = self.code_token();
        // Inside a hole, count braces: the `}` that closes the hole hands the
        // cursor back to the string's text.
        if let Some(Frame::Hole { depth }) = self.frames.last_mut() {
            match kind {
                L_CURLY => *depth += 1,
                R_CURLY if *depth == 0 => {
                    self.frames.pop();
                }
                R_CURLY => *depth -= 1,
                _ => {}
            }
        }
        kind
    }

    /// One token of code — anything outside a string's text.
    fn code_token(&mut self) -> SyntaxKind {
        let Some(c) = self.first_char() else {
            // Unreachable: the caller only calls this while `pos` is inside the
            // text and `pos` is always on a `char` boundary. Consuming the rest
            // keeps the loop terminating rather than trusting that reasoning.
            self.pos = self.bytes.len();
            return LEX_ERROR;
        };
        match c {
            c if is_space_hs(c) => {
                self.eat_while(is_space_hs);
                WHITESPACE
            }
            '/' => self.slash(),
            '"' => {
                self.pos += 1;
                self.frames.push(Frame::Str { first: self.kind.len() });
                self.string_piece()
            }
            '0'..='9' => self.number(),
            c if is_ident_start(c) => self.ident(),
            _ => self.punct(c),
        }
    }

    // --- comments ---------------------------------------------------------
    //
    // A note the doc extractor needs, because it is not visible from the token
    // kinds alone: `docComment` (`Parser/Lexer.hs:81-88`) tests for the `|` /
    // `<` marker on the *first* comment of a run only, then swallows every
    // comment that follows with `many ((LineComment <$> some lineComment) <|>
    // ...)`. So in
    //
    //     //| This value is multiplied by the number of muxes ...
    //     // the relative cost of synthesizing a memory structure ...
    //
    // (`library/intel/config.k:37`, one of 566 such runs in `library/`) both
    // lines are one doc comment. The lexer cannot express that — a token has one
    // kind — so it emits `DOC_LINE_PRE` then `LINE_COMMENT` and the parser
    // regroups: a maximal run of comment tokens separated only by whitespace
    // belongs to whichever doc kind opens it.

    fn slash(&mut self) -> SyntaxKind {
        match self.byte_at(1) {
            Some(b'/') => self.line_comment(),
            Some(b'*') => self.block_comment(),
            _ => {
                self.pos += 1;
                SLASH
            }
        }
    }

    /// The doc marker after a comment opener, if there is one.
    ///
    /// `Parser/Lexer.hs:110-111` treats a comment as *non*-doc only when
    /// `string prefix *> hspace *> notFollowedBy (char '|' <|> char '<')`
    /// succeeds, so `//` and `/*` followed by horizontal space and then `|` or
    /// `<` are doc comments — `//   | funct7 | rs2 |` in
    /// `library/processor/risc_v/isa.k:231` is one, spaces and all.
    fn doc_marker(&self) -> Option<u8> {
        let mut i = self.pos + 2;
        while let Some(c) = self.text.get(i..).and_then(|s| s.chars().next()) {
            if !is_hspace_hs(c) {
                break;
            }
            i += c.len_utf8();
        }
        match self.bytes.get(i) {
            Some(&b @ (b'|' | b'<')) => Some(b),
            _ => None,
        }
    }

    /// `//` to end of line. The newline itself stays out of the token and
    /// becomes whitespace, which keeps blank-line detection in the formatter
    /// simple. `Parser/Lexer.hs:113-114`.
    fn line_comment(&mut self) -> SyntaxKind {
        let kind = match self.doc_marker() {
            Some(b'|') => DOC_LINE_PRE,
            Some(_) => DOC_LINE_POST,
            None => LINE_COMMENT,
        };
        self.pos += 2;
        while let Some(b) = self.byte_at(0) {
            if b == b'\n' {
                break;
            }
            self.pos += 1;
        }
        kind
    }

    /// `/* ... */`, with an asymmetry inherited from the reference: an ordinary
    /// block comment nests (`L.skipBlockCommentNested`, `Parser/Lexer.hs:116-119`)
    /// but a *doc* block comment is `manyTill anySingle (string "*/")`
    /// (`Parser/Lexer.hs:88`) and therefore ends at the first `*/`.
    fn block_comment(&mut self) -> SyntaxKind {
        let kind = match self.doc_marker() {
            Some(b'|') => DOC_BLOCK_PRE,
            Some(_) => DOC_BLOCK_POST,
            None => BLOCK_COMMENT,
        };
        let nests = kind == BLOCK_COMMENT;
        self.pos += 2;
        let mut depth = 1usize;
        while self.pos < self.bytes.len() {
            match (self.byte_at(0), self.byte_at(1)) {
                (Some(b'*'), Some(b'/')) => {
                    self.pos += 2;
                    depth -= 1;
                    if depth == 0 {
                        return kind;
                    }
                }
                (Some(b'/'), Some(b'*')) if nests => {
                    self.pos += 2;
                    depth += 1;
                }
                // Any other byte, continuation bytes of a multi-byte char
                // included: neither `*` nor `/` can occur inside one.
                _ => self.pos += 1,
            }
        }
        self.error("unterminated block comment");
        kind
    }

    // --- identifiers ------------------------------------------------------

    /// An identifier-shaped lexeme, resolved to a keyword, an `int<N>`/`uint<N>`
    /// type token, or a plain identifier.
    ///
    /// `Parser/Lexer.hs:235-243` rejects both the 53 reserved words and any
    /// `int`/`uint` followed by something `readMaybe :: Maybe Word` accepts, so
    /// neither can ever be a user identifier. The type grammar
    /// (`Parser.hs:542-545`) reads `int32` as one lexeme via
    /// `intSuffix = L.decimal <* notFollowedBy (alphaNumChar <|> char '_')`
    /// (`Parser/Lexer.hs:258-259`), which is why `int32x` falls back to a plain
    /// identifier while bare `int` stays the keyword.
    ///
    /// A note for the parser, not a lexer concern: `dotSepName`
    /// (`Parser/Lexer.hs:230-233`) additionally allows `-` inside each
    /// dot-separated segment of a module path, so `intel.arria-10.config` is one
    /// name *there*. The lexer still emits `IDENT MINUS INT_NUMBER` for it and
    /// the parser reassembles the path from adjacency.
    fn ident(&mut self) -> SyntaxKind {
        let start = self.pos;
        self.bump();
        self.eat_while(is_ident_continue);
        let text = &self.text[start..self.pos];

        if let Some(kw) = SyntaxKind::from_keyword(text) {
            return kw;
        }
        if let Some(width) = text.strip_prefix("int")
            && is_int_width(width)
        {
            return INT_TYPE_TOKEN;
        }
        if let Some(width) = text.strip_prefix("uint")
            && is_int_width(width)
        {
            return UINT_TYPE_TOKEN;
        }
        IDENT
    }

    // --- numbers ----------------------------------------------------------

    /// `Parser.hs:878-880` tries `float` before `intLiteral`, so the float shape
    /// wins whenever it matches and `1.5` is never `1` `.` `5`.
    fn number(&mut self) -> SyntaxKind {
        if let Some(end) = self.probe_float() {
            self.pos = end;
            return FLOAT_NUMBER;
        }
        self.eat_integer();
        self.eat_int_suffix();
        INT_NUMBER
    }

    /// megaparsec's `L.float` (`Parser/Lexer.hs:158-159`) without committing the
    /// cursor.
    ///
    /// `L.float` is `decimal_` followed by **either** `'.' digits [exponent]`
    /// **or** a bare `exponent` — the `<|> (scientific c' <$> exponent_ 0)`
    /// branch. So `1.5`, `1.5e10` and `1e5` are all floats, while `1.` is `1`
    /// then `.` (the fractional digits are required once a `.` is committed to)
    /// and `1e` is `1` then the identifier `e` (the exponent digits are
    /// required too).
    ///
    /// `_` separators are not allowed anywhere here — that is Kanagawa's own
    /// `integer`, a different parser — so `1_0.5` lexes as the integer `10`
    /// followed by a field access.
    ///
    /// The leading run is decimal digits only, which is what keeps `0x1e5` out:
    /// the run stops at `x`, and `x` starts neither a fraction nor an exponent.
    fn probe_float(&self) -> Option<usize> {
        let n = self.bytes.len();
        let mut i = self.pos;
        while i < n && self.bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == self.pos {
            return None;
        }

        // Branch 1: `. digits [exponent]`
        if self.bytes.get(i) == Some(&b'.') {
            let mut j = i + 1;
            if !self.bytes.get(j).is_some_and(u8::is_ascii_digit) {
                return None;
            }
            while j < n && self.bytes[j].is_ascii_digit() {
                j += 1;
            }
            return Some(self.probe_exponent(j).unwrap_or(j));
        }

        // Branch 2: a bare exponent, with no fractional part.
        self.probe_exponent(i)
    }

    /// `exponent_`: `('e'|'E') ['+'|'-'] digits`, with the digits required.
    /// Returns the end offset, or `None` when there is no complete exponent —
    /// `option e' (try …)` means a dangling `e` is simply not part of the
    /// number.
    fn probe_exponent(&self, at: usize) -> Option<usize> {
        let n = self.bytes.len();
        if !self.bytes.get(at).is_some_and(|b| b.eq_ignore_ascii_case(&b'e')) {
            return None;
        }
        let mut k = at + 1;
        if matches!(self.bytes.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        if !self.bytes.get(k).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        while k < n && self.bytes[k].is_ascii_digit() {
            k += 1;
        }
        Some(k)
    }

    /// `integer` from `Parser/Lexer.hs:133-156`: `0x`/`0b`/`0o` (case-insensitive
    /// via `char'`), one optional `_` right after the prefix, then **at least
    /// one** digit of the radix, then any mix of digits and `_`.
    ///
    /// Each radix is wrapped in `try`, so a prefix with no digits after it —
    /// `0x`, `0b_` — backtracks all the way to the decimal branch and yields
    /// just `0`, leaving `x` / `b_` to be lexed as an identifier.
    fn eat_integer(&mut self) {
        if self.byte_at(0) == Some(b'0')
            && let Some(radix) = self.byte_at(1).and_then(radix_of)
        {
            let mut i = self.pos + 2;
            if self.bytes.get(i) == Some(&b'_') {
                i += 1;
            }
            if self.bytes.get(i).is_some_and(|&b| radix(b)) {
                i += 1;
                while self.bytes.get(i).is_some_and(|&b| radix(b) || b == b'_') {
                    i += 1;
                }
                self.pos = i;
                return;
            }
        }
        // `takeWhile1P isDigit` then `takeWhileP isDigitOrUnderscore`; the two
        // collapse into one loop because the caller only enters on a digit.
        while self.byte_at(0).is_some_and(|b| b.is_ascii_digit() || b == b'_') {
            self.pos += 1;
        }
    }

    /// The optional `i<W>` / `u<W>` literal type suffix of `Parser.hs:904-908`,
    /// whose spellings come from `show SignedLiteral = "i"` and
    /// `show UnsignedLiteral = "u"` (`Parser/Syntax.hs:224-225`). The width is a
    /// bare `L.decimal`, so `42i32` is one token but `42i` is `42` then `i`.
    fn eat_int_suffix(&mut self) {
        if matches!(self.byte_at(0), Some(b'i' | b'u'))
            && self.byte_at(1).is_some_and(|b| b.is_ascii_digit())
        {
            self.pos += 1;
            while self.byte_at(0).is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
    }

    // --- strings ----------------------------------------------------------

    /// One piece of a string's text, or the `{` that opens a hole.
    ///
    /// `Parser.hs:883-893` parses a string as alternating literal segments and
    /// `{ expr }` holes, where each hole holds a full expression — which may
    /// hold further strings with further holes. The lexer splits the literal
    /// text from the holes and lexes each hole as ordinary code
    /// ([`Frame::Hole`]), so the expression grammar stays the parser's:
    ///
    /// * a string with no hole is one `STRING`, quotes included;
    /// * otherwise the text up to the first hole, opening quote included, is
    ///   a `STRING_TEXT`, each run of text between holes another, and the
    ///   text after the last hole, closing quote included, a `STRING_END` —
    ///   always present, if only as `"`, so the parser knows where the string
    ///   stops. Text between two adjacent holes is empty and is no token.
    ///
    /// `{{` is an escaped `{` (`Parser.hs:892-893`: `singleBrace` requires the
    /// brace *not* be doubled), and a lone `}` in literal text is ordinary.
    ///
    /// Called with the cursor just past the opening quote (the string's
    /// first piece), or just past a hole's closing `}`.
    fn string_piece(&mut self) -> SyntaxKind {
        let Some(&Frame::Str { first }) = self.frames.last() else {
            unreachable!("string_piece runs only inside a string's text");
        };
        let opening = first == self.kind.len();
        // A hole straight after the last one's `}`: nothing to emit but the
        // brace. (Straight after the opening quote the quote is text.)
        if !opening && self.byte_at(0) == Some(b'{') && self.byte_at(1) != Some(b'{') {
            self.pos += 1;
            self.frames.push(Frame::Hole { depth: 0 });
            return L_CURLY;
        }
        loop {
            match self.byte_at(0) {
                None => {
                    // Reported once, at the string's first piece, by `run`.
                    return if opening { STRING } else { STRING_END };
                }
                Some(b'"') => {
                    self.pos += 1;
                    self.frames.pop();
                    return if opening { STRING } else { STRING_END };
                }
                Some(b'\\') => self.eat_escape(),
                Some(b'{') if self.byte_at(1) == Some(b'{') => self.pos += 2,
                Some(b'{') => return STRING_TEXT,
                Some(_) => self.bump(),
            }
        }
    }

    /// One escape sequence, cursor on the backslash.
    ///
    /// `Parser.hs:892` reads string characters with megaparsec's
    /// `MP.charLiteral`, which is the **Haskell** char literal, not C's: on top
    /// of `\n \t \r \\ \" \' \a \b \f \v` it has decimal / `\o` octal / `\x` hex
    /// numeric escapes, the 34 ASCII control mnemonics and `\^A` control
    /// escapes.
    ///
    /// The empty escape `\&` and string gaps (a backslash, whitespace, and a
    /// closing backslash) are accepted here as well, which is *wider* than the
    /// reference: `charLiteral` delegates to `base`'s `Char.readLitChar`, whose
    /// `readEsc` has no case for either. Neither appears in the corpus, and
    /// being permissive costs a language server nothing, but a strict front end
    /// should reject them.
    ///
    /// Only the *extent* matters here — the parser decodes values later — but
    /// getting it wrong would let `\"` end a string early, so it is spelled out.
    fn eat_escape(&mut self) {
        self.pos += 1; // the backslash
        let Some(c) = self.first_char() else {
            self.error("string ends in a backslash");
            return;
        };
        match c {
            // A string gap: `\`, whitespace, `\`.
            c if is_space_hs(c) => {
                self.eat_while(is_space_hs);
                if self.byte_at(0) == Some(b'\\') {
                    self.pos += 1;
                } else {
                    self.error("unterminated string gap");
                }
            }
            // The empty escape, used to break up `\12` `\34`.
            '&' => self.pos += 1,
            '^' => {
                self.pos += 1;
                match self.first_char() {
                    Some('@'..='_') => self.pos += 1,
                    _ => self.error("invalid control escape; expected `\\^@` through `\\^_`"),
                }
            }
            'x' => self.eat_radix_escape(u8::is_ascii_hexdigit, "hexadecimal"),
            'o' => self.eat_radix_escape(|b| (b'0'..=b'7').contains(b), "octal"),
            '0'..='9' => {
                while self.byte_at(0).is_some_and(|b| b.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '\\' | '"' | '\'' => self.pos += 1,
            _ => match ASCII_MNEMONICS.iter().find(|m| self.text[self.pos..].starts_with(**m)) {
                Some(mnemonic) => self.pos += mnemonic.len(),
                None => {
                    self.error(format!("unknown escape sequence `\\{c}`"));
                    self.bump();
                }
            },
        }
    }

    /// The digits of a `\x` / `\o` numeric escape; the marker is already eaten.
    fn eat_radix_escape(&mut self, digit: fn(&u8) -> bool, what: &str) {
        self.pos += 1; // the `x` or `o`
        let start = self.pos;
        while self.byte_at(0).is_some_and(|b| digit(&b)) {
            self.pos += 1;
        }
        if self.pos == start {
            self.error(format!("`\\{what}` escape has no digits"));
        }
    }

    // --- operators and punctuation ----------------------------------------

    /// The complete symbol and operator inventory of the reference grammar.
    /// There are no compound assignment operators in Kanagawa.
    fn punct(&mut self, c: char) -> SyntaxKind {
        let second = self.byte_at(1);
        let (len, kind) = match self.bytes[self.pos] {
            b'(' => (1, L_PAREN),
            b')' => (1, R_PAREN),
            b'{' => (1, L_CURLY),
            b'}' => (1, R_CURLY),
            // `[[ ... ]]` attribute groups are rebuilt by the parser from two
            // adjacent brackets (`Parser/Lexer.hs:278-279`: `doubleBrackets =
            // brackets . brackets`), so brackets stay single here.
            b'[' => (1, L_BRACK),
            b']' => (1, R_BRACK),
            // Never merged; see the module docs.
            b'<' => (1, L_ANGLE),
            b'>' => (1, R_ANGLE),
            b';' => (1, SEMICOLON),
            b',' => (1, COMMA),
            b'.' => (1, DOT),
            b'#' => (1, HASH),
            b'\\' => (1, BACKSLASH),
            b'~' => (1, TILDE),
            b'?' => (1, QUESTION),
            b'+' => (1, PLUS),
            b'*' => (1, STAR),
            b'%' => (1, PERCENT),
            b':' if second == Some(b':') => (2, COLON2),
            b':' => (1, COLON),
            b'-' if second == Some(b'>') => (2, THIN_ARROW),
            b'-' => (1, MINUS),
            b'=' if second == Some(b'=') => (2, EQ2),
            b'=' => (1, EQ),
            b'!' if second == Some(b'=') => (2, NEQ),
            b'!' => (1, BANG),
            b'&' if second == Some(b'&') => (2, AMP2),
            b'&' => (1, AMP),
            b'|' if second == Some(b'|') => (2, PIPE2),
            b'|' => (1, PIPE),
            b'^' if second == Some(b'^') => (2, CARET2),
            b'^' => (1, CARET),
            _ => {
                self.error(format!("unexpected character `{c}`"));
                (c.len_utf8(), LEX_ERROR)
            }
        };
        self.pos += len;
        kind
    }
}

/// The digit predicate for a `0x` / `0b` / `0o` prefix, matched
/// case-insensitively the way `char'` does (`Parser/Lexer.hs:136`).
fn radix_of(marker: u8) -> Option<fn(u8) -> bool> {
    let pred: fn(u8) -> bool = match marker.to_ascii_lowercase() {
        b'x' => |b| b.is_ascii_hexdigit(),
        b'b' => |b| b == b'0' || b == b'1',
        b'o' => |b| (b'0'..=b'7').contains(&b),
        _ => return None,
    };
    Some(pred)
}

/// Whether `width` is what `intSuffix` accepts after `int` / `uint`: a
/// non-empty run of decimal digits and nothing else (`Parser/Lexer.hs:258-259`).
fn is_int_width(width: &str) -> bool {
    !width.is_empty() && width.bytes().all(|b| b.is_ascii_digit())
}
