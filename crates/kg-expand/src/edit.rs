//! A file's text with its edits applied: each a region of the text replaced
//! by pieces — new text, or another region of the same text rendered with
//! its own edits, which is how an argument moves to another place and keeps
//! the edits inside it (`f(y: g(b: 1, a: 2), x: 0)`).

use kg_base::{TextRange, TextSize};

pub(crate) enum Piece {
    Text { text: String, what: String },
    /// The text at the range, with the edits inside it — except those of
    /// the list whose argument it is, which is being rewritten around it.
    Source { range: TextRange, list: usize },
}

pub(crate) struct Region {
    pub range: TextRange,
    /// The list rewrite this is one argument's place in.
    pub list: Option<usize>,
    pub pieces: Vec<Piece>,
}

/// A stretch of the output and where it came from: copied from the text at
/// `from`, or written for an edit (`what`) that replaced `from`.
pub(crate) struct Span {
    pub out: TextRange,
    pub from: TextRange,
    pub what: Option<String>,
}

pub(crate) struct Rendered {
    pub text: String,
    pub spans: Vec<Span>,
    /// The first region applied.
    pub first_edit: Option<TextRange>,
}

impl Rendered {
    /// Where an offset of the output stands in the text, and the edit that
    /// wrote it, if one did.
    pub fn origin(&self, at: TextSize) -> (TextRange, Option<&str>) {
        let span = self.spans.iter().rev().find(|s| s.out.start() <= at && (at < s.out.end() || s.out.is_empty() && at == s.out.start()));
        match span {
            Some(Span { from, what: Some(what), .. }) => (*from, Some(what)),
            Some(Span { out, from, what: None }) => {
                let p = (from.start() + (at - out.start())).min(from.end());
                (TextRange::empty(p), None)
            }
            None => (TextRange::empty(at), None),
        }
    }
}

pub(crate) fn render(text: &str, mut regions: Vec<Region>) -> Rendered {
    regions.sort_by_key(|r| (r.range.start(), std::cmp::Reverse(r.range.end())));
    let first_edit = regions.first().map(|r| r.range);
    let mut out = Rendered { text: String::with_capacity(text.len()), spans: Vec::new(), first_edit };
    let all = TextRange::new(TextSize::from(0), TextSize::of(text));
    Renderer { text, regions: &regions }.range(all, None, &mut out);
    out
}

struct Renderer<'a> {
    text: &'a str,
    regions: &'a [Region],
}

impl Renderer<'_> {
    fn copy(&self, from: TextRange, out: &mut Rendered) {
        if from.is_empty() {
            return;
        }
        let start = TextSize::of(out.text.as_str());
        out.text.push_str(&self.text[from]);
        out.spans.push(Span { out: TextRange::new(start, TextSize::of(out.text.as_str())), from, what: None });
    }

    fn range(&self, range: TextRange, except: Option<usize>, out: &mut Rendered) {
        let mut pos = range.start();
        let first = self.regions.partition_point(|r| r.range.start() < range.start());
        for r in &self.regions[first..] {
            if r.range.start() > range.end() {
                break;
            }
            // Inside one already applied, past the range, or the place of
            // an argument of the list being rewritten around this text.
            if r.range.start() < pos || r.range.end() > range.end() || (except.is_some() && r.list == except) {
                continue;
            }
            self.copy(TextRange::new(pos, r.range.start()), out);
            for piece in &r.pieces {
                match piece {
                    Piece::Text { text, what } => {
                        let start = TextSize::of(out.text.as_str());
                        out.text.push_str(text);
                        let span = TextRange::new(start, TextSize::of(out.text.as_str()));
                        out.spans.push(Span { out: span, from: r.range, what: Some(what.clone()) });
                    }
                    Piece::Source { range, list } => self.range(*range, Some(*list), out),
                }
            }
            pos = r.range.end();
        }
        self.copy(TextRange::new(pos, range.end()), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: u32, e: u32) -> TextRange {
        TextRange::new(s.into(), e.into())
    }

    /// Two arguments swapped, the inner call in one of them rewritten too,
    /// and a comma deleted: each argument moves with its own edits, and the
    /// list's places take the arguments in their new order. Red with a
    /// moved argument rendered without its edits, or with the list's own
    /// places applied inside an argument it moves.
    #[test]
    fn an_argument_moves_with_its_own_edits() {
        // 0123456789012345678901234
        // f(y: g(b, a), x: 0,)
        let text = "f(y: g(b, a), x: 0,)";
        let swap = |range, list, value| Region { range, list: Some(list), pieces: vec![Piece::Source { range: value, list }] };
        let regions = vec![
            // the outer list: `y: g(b, a)` at 2..12, `x: 0` at 14..18
            swap(at(2, 12), 0, at(17, 18)),
            swap(at(14, 18), 0, at(5, 12)),
            // the inner list `(b, a)` swapped
            swap(at(7, 8), 1, at(10, 11)),
            swap(at(10, 11), 1, at(7, 8)),
            Region { range: at(18, 19), list: None, pieces: Vec::new() },
        ];
        let out = render(text, regions);
        assert_eq!(out.text, "f(0, g(a, b))");
    }
}
