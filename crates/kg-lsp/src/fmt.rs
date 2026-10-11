//! The formatter seam, wired to `kg-fmt`.
//!
//! The server asks [`available`] whether a backend is linked in and *only then*
//! advertises `documentFormattingProvider`. A greyed-out "Format Document" is
//! better than one that silently does nothing: an editor that believes the
//! server formats will happily overwrite a file with an empty edit list.
//!
//! The `Fmt` settings come from the same `kanagawa.toml` the batch formatter
//! reads, so `kg fmt` and format-on-save cannot disagree — same crate, same
//! settings, same output.
//!
//! # Declining is a real answer
//!
//! [`kg_fmt::format_file`] refuses a file with syntax errors, and
//! [`kg_fmt::format_range`] additionally refuses an edit it cannot prove is the
//! same program. Both refusals arrive here as an empty edit list, which the
//! protocol defines as "nothing to change".
//!
//! That matters most in an editor, because format-on-save runs against a buffer
//! that is *mid-edit* far more often than a batch run does. A formatter that
//! guessed at a half-typed declaration would silently rewrite it; this one
//! leaves the buffer alone until it parses.

use kg_base::{LineIndex, TextRange};
use kg_config::Fmt;
use lsp_types::TextEdit;

use crate::convert::{self, PositionEncoding};

/// Whether a formatter backend is linked in. See the module docs.
const AVAILABLE: bool = true;

/// Whether to advertise `documentFormattingProvider` at all.
///
/// A function rather than a `const` so that every call site is a runtime check
/// that keeps compiling when the constant becomes a real backend probe.
#[inline]
pub fn available() -> bool {
    AVAILABLE
}

/// Formats `text`, or the `range` of it, returning the edits to apply.
///
/// Returns an empty edit list when no backend is available. An empty list is
/// well-defined in the protocol — "nothing to change" — and is the only safe
/// answer: anything else would rewrite the user's file from a formatter that
/// does not exist.
pub fn format(
    text: &str,
    index: &LineIndex,
    enc: PositionEncoding,
    settings: &Fmt,
    range: Option<TextRange>,
) -> Vec<TextEdit> {
    if !available() {
        log_once();
        return Vec::new();
    }

    match range {
        // `formatting`: the whole buffer, as one replacement edit.
        None => match kg_fmt::format_file(text, settings) {
            // An edit that changes nothing is not worth sending. Clients treat a
            // non-empty edit list as a document modification — it marks the file
            // dirty, pushes an undo entry, and can retrigger format-on-save.
            Ok(formatted) if formatted == text => Vec::new(),
            Ok(formatted) => vec![whole_file_edit(index, enc, formatted)],
            // No edit, and the reason said: a file with syntax errors is not
            // laid out, and a layout that would change the program is the
            // formatter's bug — either way the user asked and is told why.
            Err(e) => {
                crate::log(lsp_types::MessageType::WARNING, format!("formatting declined: {e}"));
                Vec::new()
            }
        },
        // `rangeFormatting`: whole items overlapping the selection, or nothing.
        Some(range) => kg_fmt::format_range(text, range, settings)
            .unwrap_or_default()
            .into_iter()
            .map(|e| TextEdit {
                range: convert::range(index, enc, e.range),
                new_text: e.new_text,
            })
            .collect(),
    }
}

/// The whole-document replacement edit a formatter's output becomes.
///
/// Split out because it is the part that has to be right regardless of who
/// produces the formatted text: the range must cover the entire buffer in the
/// negotiated encoding, or the client will splice the new text into the middle
/// of the old.
pub fn whole_file_edit(index: &LineIndex, enc: PositionEncoding, formatted: String) -> TextEdit {
    let whole = TextRange::new(kg_base::TextSize::new(0), convert::text_len(index));
    TextEdit { range: convert::range(index, enc, whole), new_text: formatted }
}

fn log_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        crate::log(
            lsp_types::MessageType::INFO,
            "formatting requested, but no `kg-fmt` backend is linked in; \
             the capability is not advertised and no edits will be produced",
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edits(text: &str, range: Option<TextRange>) -> Vec<TextEdit> {
        let index = LineIndex::new(text);
        format(text, &index, PositionEncoding::Utf16, &Fmt::default(), range)
    }

    /// The capability and the behaviour have to agree in *both* directions:
    /// advertising what `format` will not do greys out nothing and produces
    /// nothing, and not advertising what it would do hides a working feature.
    #[test]
    fn the_advertised_capability_matches_what_format_actually_does() {
        assert!(available());
        assert!(!edits("class  A{}\n", None).is_empty(), "a backend is linked in");
    }

    #[test]
    fn a_whole_file_format_replaces_the_file() {
        let text = "class  A{}\n";
        let out = edits(text, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].range.start, lsp_types::Position::new(0, 0));
        // Same crate as `kg fmt`, so the two cannot drift.
        assert_eq!(out[0].new_text, kg_fmt::format_file(text, &Fmt::default()).unwrap());
    }

    /// Format-on-save fires against buffers that are mid-edit. `kg-fmt` refuses
    /// a file that does not parse, and that refusal has to reach the client as
    /// "nothing to change" rather than as a guess.
    #[test]
    fn an_unparseable_buffer_is_left_alone() {
        assert!(edits("class A { void f( \n", None).is_empty());
    }

    /// An edit list that changes nothing still marks the document dirty and
    /// pushes an undo entry, so an already-formatted file must produce none.
    #[test]
    fn an_already_formatted_file_produces_no_edits() {
        let formatted = kg_fmt::format_file("class  A{}\n", &Fmt::default()).unwrap();
        assert!(edits(&formatted, None).is_empty());
    }

    #[test]
    fn a_whole_file_edit_covers_the_whole_file() {
        let text = "class A {}\nclass B {}\n";
        let index = LineIndex::new(text);
        let edit = whole_file_edit(&index, PositionEncoding::Utf16, "x".to_owned());
        assert_eq!(edit.range.start, lsp_types::Position::new(0, 0));
        assert_eq!(edit.range.end, lsp_types::Position::new(2, 0));
        assert_eq!(edit.new_text, "x");
    }
}
