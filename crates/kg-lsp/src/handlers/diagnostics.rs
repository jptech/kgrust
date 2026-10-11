//! Diagnostics, published unprompted on every edit.
//!
//! This is the reason the server is built on `lsp-server` rather than on a
//! request/response abstraction: nobody asks for diagnostics, the server just
//! sends them. They come from `kg_db::Db::diagnostics` — the whole program
//! rooted at the file being edited, not just its parse — so an unresolved
//! import or a type error appears in the gutter alongside a syntax error. The
//! reference has exactly one frontend error code (`Parser/IO.hs:114`), so
//! severity and code are fixed and only the message and the span are ours to
//! get right.
//!
//! One adaptation the protocol needs: a parse error is reported *at* an offset,
//! not over a span, and a zero-width range renders as an invisible caret in
//! most editors. Empty ranges are widened to cover the token they precede.

use kg_base::{FileId, FileRange, LineIndex, TextRange, TextSize};
use kg_diag::Severity;
use kg_syntax::SyntaxNode;
use lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString,
};

use crate::convert::{self, PositionEncoding};

/// Cap on how many diagnostics one file may publish.
///
/// A file whose grammar has not landed yet reports one error per token; sending
/// a hundred thousand of them helps nobody and makes the editor unresponsive.
pub const MAX_DIAGNOSTICS: usize = 500;

/// `root` is the file's own syntax tree, used only to give an empty range
/// something to underline. A diagnostic that belongs to another file of the
/// program is dropped: this payload is for one `textDocument/publishDiagnostics`
/// and an editor cannot render a range it has no document for.
///
/// A label — a `while compiling` note at an instantiation site — becomes
/// related information, which an editor renders as a clickable secondary
/// location; `locate` turns its range into one, for any file of the program
/// the server can name. A label it cannot locate is dropped from the payload
/// (the message stays), never invented.
pub fn diagnostics(
    reported: Vec<kg_diag::Diagnostic>,
    root: &SyntaxNode,
    file: FileId,
    index: &LineIndex,
    enc: PositionEncoding,
    locate: &dyn Fn(FileRange) -> Option<Location>,
) -> Vec<Diagnostic> {
    reported
        .into_iter()
        .filter(|d| d.range.file == file)
        .take(MAX_DIAGNOSTICS)
        .map(|d| {
            let range = widen(root, d.range.range);
            let related: Vec<DiagnosticRelatedInformation> = d
                .labels
                .iter()
                .filter_map(|label| {
                    Some(DiagnosticRelatedInformation {
                        location: locate(label.range)?,
                        message: label.said(),
                    })
                })
                .collect();
            #[allow(deprecated)]
            Diagnostic {
                range: convert::range(index, enc, range),
                severity: Some(match d.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                }),
                code: Some(NumberOrString::Number(d.code.number() as i32)),
                code_description: None,
                source: Some("kanagawa".to_owned()),
                message: d.message,
                related_information: (!related.is_empty()).then_some(related),
                // `--Wno-<name>` is the reference's own spelling of a warning
                // switch; carrying it through means an editor can offer the
                // exact flag that silences the warning.
                tags: None,
                // The message's family, where a pass decides by it
                // (`kg_diag::DiagnosticKind`), rides along the same way.
                data: match (d.warning, d.kind) {
                    (None, None) => None,
                    (w, k) => {
                        let mut data = serde_json::Map::new();
                        if let Some(w) = w {
                            data.insert("warning".to_owned(), w.name().into());
                        }
                        if let Some(k) = k {
                            data.insert("kind".to_owned(), k.name().into());
                        }
                        Some(serde_json::Value::Object(data))
                    }
                },
            }
        })
        .collect()
}

/// Gives an empty range something to underline.
///
/// Prefers the token starting at the offset; failing that, the character
/// before it, so an error reported at end-of-file still shows.
fn widen(root: &SyntaxNode, range: TextRange) -> TextRange {
    if !range.is_empty() {
        return range;
    }
    let end = root.text_range().end();
    let at = range.start().min(end);

    if let Some(token) = root.token_at_offset(at).right_biased() {
        let span = token.text_range();
        if span.start() == at && !span.is_empty() && span.end() <= end {
            return span;
        }
    }
    if at < end {
        return TextRange::at(at, TextSize::new(1));
    }
    if at > TextSize::new(0) {
        return TextRange::new(at - TextSize::new(1), at);
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_syntax::parse_file;

    fn diags(text: &str) -> Vec<Diagnostic> {
        let parse = parse_file(text);
        diagnostics(
            parse.diagnostics(FileId(0)),
            &parse.syntax_node(),
            FileId(0),
            &LineIndex::new(text),
            PositionEncoding::Utf16,
            &|_| None,
        )
    }

    #[test]
    fn a_clean_empty_file_has_no_diagnostics() {
        assert!(diags("").is_empty());
    }

    #[test]
    fn every_diagnostic_has_a_non_empty_range() {
        // Whatever the parser makes of this today, nothing may be zero-width.
        for src in ["}", "class", "\"unterminated", "/* unterminated", "int32 x = ;"] {
            for d in diags(src) {
                assert!(
                    (d.range.start.line, d.range.start.character)
                        < (d.range.end.line, d.range.end.character),
                    "empty range for {src:?}: {d:?}"
                );
            }
        }
    }

    #[test]
    fn diagnostics_carry_the_frontend_error_code() {
        let out = diags("\"unterminated");
        assert!(!out.is_empty(), "an unterminated string must be reported");
        for d in out {
            assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
            assert_eq!(d.code, Some(NumberOrString::Number(1)));
            assert_eq!(d.source.as_deref(), Some("kanagawa"));
        }
    }

    #[test]
    fn positions_are_utf16_code_units() {
        // The lex error is on the `@`, which sits after a 4-byte emoji.
        let src = "// 🦀\n@\n";
        let out = diags(src);
        assert!(!out.is_empty());
        assert_eq!(out[0].range.start.line, 1);
        assert_eq!(out[0].range.start.character, 0);
    }

    #[test]
    fn the_flood_is_capped() {
        let src = "@".repeat(MAX_DIAGNOSTICS * 3);
        assert!(diags(&src).len() <= MAX_DIAGNOSTICS);
    }
}
