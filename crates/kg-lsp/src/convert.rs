//! Byte offsets ⇄ LSP positions.
//!
//! **Every** position that crosses the wire goes through here. LSP's
//! `Position.character` counts UTF-16 code units unless the client and server
//! negotiate otherwise (`general.positionEncodings`, LSP 3.17), while the
//! compiler counts bytes. Skipping the conversion does not fail loudly: it
//! silently shifts every position in every file that contains one non-ASCII
//! character, which is most real files with a comment in them.
//!
//! [`kg_base::LineIndex`] already stores the per-line table of wide characters,
//! so this module is just the protocol adapter over `to_wide` / `to_utf8`, plus
//! the clamping the protocol requires: an out-of-range position is defined to
//! saturate, not to be an error.

use kg_base::{LineIndex, TextRange, TextSize, WideEncoding, WideLineCol};
use lsp_types::{Position, PositionEncodingKind, Range};

/// The negotiated meaning of `Position.character`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum PositionEncoding {
    /// Bytes. Only ever selected when the client asks for it.
    Utf8,
    /// UTF-16 code units. The protocol default and the only encoding a client
    /// is *required* to support, so it is ours too.
    #[default]
    Utf16,
    /// Unicode scalar values.
    Utf32,
}

impl PositionEncoding {
    /// Picks the best encoding the client offered, defaulting to UTF-16.
    ///
    /// UTF-8 first: it makes every conversion below a no-op, which is both
    /// faster and impossible to get wrong.
    pub fn negotiate(offered: Option<&[PositionEncodingKind]>) -> PositionEncoding {
        let Some(offered) = offered else { return PositionEncoding::Utf16 };
        for kind in offered {
            if *kind == PositionEncodingKind::UTF8 {
                return PositionEncoding::Utf8;
            }
        }
        // UTF-32 is only worth taking if UTF-16 is absent, which the spec says
        // cannot happen — but a client that lies should still get an encoding
        // it named.
        if offered.contains(&PositionEncodingKind::UTF16) || offered.is_empty() {
            return PositionEncoding::Utf16;
        }
        if offered.contains(&PositionEncodingKind::UTF32) {
            return PositionEncoding::Utf32;
        }
        PositionEncoding::Utf16
    }

    pub fn kind(self) -> PositionEncodingKind {
        match self {
            PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
            PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
            PositionEncoding::Utf32 => PositionEncodingKind::UTF32,
        }
    }

    fn wide(self) -> Option<WideEncoding> {
        match self {
            PositionEncoding::Utf8 => None,
            PositionEncoding::Utf16 => Some(WideEncoding::Utf16),
            PositionEncoding::Utf32 => Some(WideEncoding::Utf32),
        }
    }
}

/// Byte length of the indexed text.
///
/// [`LineIndex`] does not expose it directly, but the last line's range ends at
/// exactly the end of the text.
pub fn text_len(index: &LineIndex) -> TextSize {
    let last = index.line_count().saturating_sub(1) as u32;
    index.line_range(last).map(|r| r.end()).unwrap_or_default()
}

/// Byte offset → LSP position, saturating at the end of the text.
pub fn position(index: &LineIndex, enc: PositionEncoding, offset: TextSize) -> Position {
    let offset = offset.min(text_len(index));
    let lc = index.line_col(offset);
    match enc.wide() {
        None => Position::new(lc.line, lc.col),
        Some(wide) => {
            let w = index.to_wide(wide, lc);
            Position::new(w.line, w.col)
        }
    }
}

/// LSP position → byte offset, clamped to the end of the addressed line.
///
/// The protocol defines both saturations: a line past the end of the file is
/// the end of the file, and "if the character value is greater than the line
/// length it defaults back to the line length".
pub fn offset(index: &LineIndex, enc: PositionEncoding, pos: Position) -> TextSize {
    let max_line = index.line_count().saturating_sub(1) as u32;
    let line = pos.line.min(max_line);
    let Some(line_range) = index.line_range(line) else { return TextSize::new(0) };

    let col = match enc.wide() {
        None => pos.character,
        Some(wide) => index.to_utf8(wide, WideLineCol { line, col: pos.character }).col,
    };
    // Saturating, not `+`. "End of line" has no spelling in the protocol, so
    // clients reach for a huge column instead, and `line_start + u32::MAX`
    // overflows for every line but the first — a panic in debug, and in release a
    // *wrap* to a small number that then survives the clamp below and reads as a
    // plausible offset near the top of the file.
    let raw = line_range
        .start()
        .checked_add(TextSize::from(col))
        .unwrap_or_else(|| line_range.end());
    raw.min(line_range.end())
}

pub fn range(index: &LineIndex, enc: PositionEncoding, range: TextRange) -> Range {
    Range::new(position(index, enc, range.start()), position(index, enc, range.end()))
}

pub fn text_range(index: &LineIndex, enc: PositionEncoding, range: Range) -> TextRange {
    let start = offset(index, enc, range.start);
    let end = offset(index, enc, range.end);
    // A client that sends a reversed range gets an empty one rather than a
    // panic out of `TextRange::new`.
    TextRange::new(start.min(end), start.max(end))
}

/// Column of `offset` within its line, in the negotiated encoding.
///
/// Semantic tokens are encoded as (line, column, length) triples rather than as
/// ranges, so they need the halves separately.
pub fn line_col(index: &LineIndex, enc: PositionEncoding, offset: TextSize) -> (u32, u32) {
    let p = position(index, enc, offset);
    (p.line, p.character)
}

/// Nudges `offset` down to the nearest `char` boundary of `text`.
///
/// Only reachable from a client that sends a column pointing into the middle of
/// a multi-byte character. Splicing there would panic, so it does not happen.
pub fn snap_to_boundary(text: &str, offset: TextSize) -> TextSize {
    let mut i = usize::from(offset).min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    TextSize::new(i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 'é' is 2 UTF-8 bytes / 1 UTF-16 unit; '🦀' is 4 bytes / 2 UTF-16 units
    /// (a surrogate pair) / 1 UTF-32 unit. A file with both is the whole test
    /// matrix in one line.
    const TEXT: &str = "// é🦀 ok\nclass Ünicøde {}\n";

    #[test]
    fn every_offset_round_trips_in_utf16() {
        let index = LineIndex::new(TEXT);
        for (byte, _) in TEXT.char_indices() {
            let off = TextSize::new(byte as u32);
            let pos = position(&index, PositionEncoding::Utf16, off);
            assert_eq!(
                offset(&index, PositionEncoding::Utf16, pos),
                off,
                "round trip failed at byte {byte}"
            );
        }
    }

    #[test]
    fn utf16_columns_are_code_units_not_bytes() {
        let index = LineIndex::new(TEXT);
        let at_ok = TextSize::new(TEXT.find("ok").unwrap() as u32);

        // "// " is 3 units, 'é' is 1, '🦀' is 2, ' ' is 1 => column 7.
        let utf16 = position(&index, PositionEncoding::Utf16, at_ok);
        assert_eq!(utf16, Position::new(0, 7));

        // The byte column is larger, because 'é' costs 2 bytes and '🦀' 4.
        let utf8 = position(&index, PositionEncoding::Utf8, at_ok);
        assert_eq!(utf8, Position::new(0, 10));

        // UTF-32 counts the crab once.
        let utf32 = position(&index, PositionEncoding::Utf32, at_ok);
        assert_eq!(utf32, Position::new(0, 6));

        for enc in [PositionEncoding::Utf8, PositionEncoding::Utf16, PositionEncoding::Utf32] {
            assert_eq!(offset(&index, enc, position(&index, enc, at_ok)), at_ok);
        }
    }

    #[test]
    fn a_range_over_a_non_ascii_identifier_round_trips() {
        let index = LineIndex::new(TEXT);
        let start = TEXT.find("Ünicøde").unwrap() as u32;
        let original = TextRange::new(TextSize::new(start), TextSize::new(start + "Ünicøde".len() as u32));

        let lsp = range(&index, PositionEncoding::Utf16, original);
        assert_eq!(lsp.start, Position::new(1, 6));
        // 7 characters, all one UTF-16 unit each.
        assert_eq!(lsp.end, Position::new(1, 13));
        assert_eq!(text_range(&index, PositionEncoding::Utf16, lsp), original);
    }

    #[test]
    fn out_of_range_positions_saturate() {
        let index = LineIndex::new(TEXT);
        let enc = PositionEncoding::Utf16;
        assert_eq!(offset(&index, enc, Position::new(999, 0)), text_len(&index));
        // Past the end of line 0 clamps to just before its newline.
        let eol = offset(&index, enc, Position::new(0, 999));
        assert_eq!(&TEXT[usize::from(eol)..usize::from(eol) + 1], "\n");
    }

    #[test]
    fn utf8_is_preferred_when_offered() {
        assert_eq!(
            PositionEncoding::negotiate(Some(&[
                PositionEncodingKind::UTF16,
                PositionEncodingKind::UTF8
            ])),
            PositionEncoding::Utf8
        );
        assert_eq!(PositionEncoding::negotiate(None), PositionEncoding::Utf16);
        assert_eq!(
            PositionEncoding::negotiate(Some(&[PositionEncodingKind::UTF16])),
            PositionEncoding::Utf16
        );
        assert_eq!(
            PositionEncoding::negotiate(Some(&[PositionEncodingKind::UTF32])),
            PositionEncoding::Utf32
        );
    }

    #[test]
    fn boundaries_snap_down_into_a_multibyte_char() {
        // Byte 4 is inside '🦀'.
        let inside = TextSize::new(4);
        assert_eq!(usize::from(snap_to_boundary(TEXT, inside)), 3);
    }

    /// A column near `u32::MAX` is a real thing to receive: "end of line" has no
    /// spelling in the protocol, and clients reach for a huge column instead.
    ///
    /// `line_start + character` overflowed before it was clamped, which panics in
    /// debug and — worse — *wraps* in release, so the clamp then compared a small
    /// wrapped number and let it through as a plausible offset.
    #[test]
    fn a_column_near_the_integer_limit_clamps_to_the_line_end() {
        let text = "line one\nline two\n";
        let index = LineIndex::new(text);
        // Line 1, not line 0: line 0 starts at offset 0, so `0 + huge` cannot
        // overflow and the bug hides. The second line starts at 9.
        for col in [u32::MAX, u32::MAX - 1, 1 << 31] {
            let at = offset(&index, PositionEncoding::Utf8, Position::new(1, col));
            assert_eq!(at, index.line_range(1).unwrap().end(), "col {col}");
        }
    }
}

