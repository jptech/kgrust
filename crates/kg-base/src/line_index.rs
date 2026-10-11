//! Offset <-> line/column mapping.
//!
//! Two consumers with different conventions:
//!
//! * the legacy terminal renderer, which prints 1-based `line:col` in **bytes**
//!   the way the reference compiler's megaparsec output does, and
//! * LSP, whose `Position.character` counts **UTF-16 code units** by default.
//!
//! Both are served from one table so a file is scanned exactly once.

use crate::{FxHashMap, TextRange, TextSize};

/// A 0-based line and a 0-based **byte** column within that line.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// The code-unit width an editor counts columns in.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WideEncoding {
    Utf16,
    Utf32,
}

/// A 0-based line and a 0-based column measured in [`WideEncoding`] units.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WideLineCol {
    pub line: u32,
    pub col: u32,
}

#[derive(Copy, Clone, Debug)]
struct WideChar {
    /// Byte offset of the char, relative to the start of its line.
    start: u32,
    /// Byte offset just past the char, relative to the start of its line.
    end: u32,
}

impl WideChar {
    fn wide_len(self, enc: WideEncoding) -> u32 {
        match enc {
            // 1-3 UTF-8 bytes fit in one UTF-16 unit; 4 bytes need a surrogate pair.
            WideEncoding::Utf16 => {
                if self.end - self.start == 4 {
                    2
                } else {
                    1
                }
            }
            WideEncoding::Utf32 => 1,
        }
    }
}

/// Maps byte offsets to line/column positions for one file's text.
#[derive(Clone, Debug)]
pub struct LineIndex {
    /// Byte offset at which each line starts. Always begins with 0, so
    /// `line_starts.len()` is the number of lines.
    line_starts: Vec<TextSize>,
    /// Non-ASCII characters, bucketed by line. Absent for the common
    /// all-ASCII line, which is why lookup is a plain arithmetic subtraction
    /// for nearly every position in a real Kanagawa source file.
    wide_chars: FxHashMap<u32, Vec<WideChar>>,
    len: TextSize,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![TextSize::from(0)];
        let mut wide_chars: FxHashMap<u32, Vec<WideChar>> = FxHashMap::default();

        let mut line = 0u32;
        let mut line_start = 0usize;
        let mut cur_wide: Vec<WideChar> = Vec::new();

        for (offset, c) in text.char_indices() {
            if c == '\n' {
                line_starts.push(TextSize::from(offset as u32 + 1));
                if !cur_wide.is_empty() {
                    wide_chars.insert(line, std::mem::take(&mut cur_wide));
                }
                line += 1;
                line_start = offset + 1;
                continue;
            }
            if !c.is_ascii() {
                let rel = (offset - line_start) as u32;
                cur_wide.push(WideChar { start: rel, end: rel + c.len_utf8() as u32 });
            }
        }
        if !cur_wide.is_empty() {
            wide_chars.insert(line, cur_wide);
        }

        LineIndex { line_starts, wide_chars, len: TextSize::of(text) }
    }

    /// Number of lines. A file ending in `\n` has a final empty line, matching
    /// what editors show.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_col(&self, offset: TextSize) -> LineCol {
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let col = offset - self.line_starts[line];
        LineCol { line: line as u32, col: col.into() }
    }

    pub fn offset(&self, pos: LineCol) -> Option<TextSize> {
        let start = *self.line_starts.get(pos.line as usize)?;
        let offset = start + TextSize::from(pos.col);
        (offset <= self.len).then_some(offset)
    }

    /// Byte range of `line`, excluding its terminating newline.
    pub fn line_range(&self, line: u32) -> Option<TextRange> {
        let start = *self.line_starts.get(line as usize)?;
        let end = match self.line_starts.get(line as usize + 1) {
            // Drop the '\n'. `line_text` additionally trims a '\r'.
            Some(&next) => next - TextSize::from(1),
            None => self.len,
        };
        Some(TextRange::new(start, end))
    }

    /// The text of `line` with any trailing `\r` removed, for caret rendering.
    pub fn line_text<'a>(&self, text: &'a str, line: u32) -> Option<&'a str> {
        let range = self.line_range(line)?;
        Some(text[range.start().into()..range.end().into()].trim_end_matches('\r'))
    }

    pub fn to_wide(&self, enc: WideEncoding, pos: LineCol) -> WideLineCol {
        let col = self.wide_chars.get(&pos.line).map_or(pos.col, |chars| {
            let mut col = pos.col;
            for c in chars {
                if c.end <= pos.col {
                    col -= (c.end - c.start) - c.wide_len(enc);
                } else {
                    break;
                }
            }
            col
        });
        WideLineCol { line: pos.line, col }
    }

    pub fn to_utf8(&self, enc: WideEncoding, pos: WideLineCol) -> LineCol {
        let col = self.wide_chars.get(&pos.line).map_or(pos.col, |chars| {
            let mut col = pos.col;
            for c in chars {
                // `c.start` is a byte offset; once the running byte column has
                // passed it, earlier chars have already been accounted for.
                if col > c.start {
                    col += (c.end - c.start) - c.wide_len(enc);
                } else {
                    break;
                }
            }
            col
        });
        LineCol { line: pos.line, col }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, offset: u32) -> LineCol {
        LineIndex::new(text).line_col(TextSize::from(offset))
    }

    #[test]
    fn ascii_positions() {
        let text = "inline void main()\n{\n}\n";
        assert_eq!(at(text, 0), LineCol { line: 0, col: 0 });
        assert_eq!(at(text, 7), LineCol { line: 0, col: 7 });
        assert_eq!(at(text, 19), LineCol { line: 1, col: 0 });
        assert_eq!(at(text, 21), LineCol { line: 2, col: 0 });
        // A trailing newline yields one more (empty) line.
        assert_eq!(LineIndex::new(text).line_count(), 4);
    }

    #[test]
    fn round_trips_offsets() {
        let text = "a\nbb\r\nccc\n";
        let index = LineIndex::new(text);
        for offset in 0..=text.len() as u32 {
            let pos = index.line_col(TextSize::from(offset));
            assert_eq!(index.offset(pos), Some(TextSize::from(offset)));
        }
    }

    #[test]
    fn line_text_trims_carriage_return() {
        let text = "a\r\nb\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_text(text, 0), Some("a"));
        assert_eq!(index.line_text(text, 1), Some("b"));
    }

    #[test]
    fn wide_chars_convert_both_ways() {
        // 'é' is 2 UTF-8 bytes / 1 UTF-16 unit; '𝄞' is 4 bytes / 2 UTF-16 units.
        let text = "// é𝄞 x\n";
        let index = LineIndex::new(text);
        let utf8 = index.line_col(TextSize::from(text.find('x').unwrap() as u32));
        let wide = index.to_wide(WideEncoding::Utf16, utf8);
        assert_eq!(wide.col, 7); // "// " + 1 + 2 + " "
        assert_eq!(index.to_utf8(WideEncoding::Utf16, wide), utf8);
    }
}
