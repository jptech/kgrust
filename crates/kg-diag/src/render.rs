//! The legacy terminal renderer.
//!
//! Reproduces megaparsec's `errorBundlePretty` layout, which is what the
//! reference compiler prints and what every human and script downstream has
//! learned to read:
//!
//! ```text
//! Error 1: library/data/fifo.k:88:12:
//!    |
//! 88 |     uint32 x = "s";
//!    |            ^
//! unexpected string literal
//! ```
//!
//! Two properties are load-bearing for `test/syntax/syntax-test.py`:
//! the first line must contain `Error <n>`, and a warning's message line must
//! **end** with `[--Wno-<name>]` (the harness regex is anchored with `$`).

use kg_base::{FileRange, LineIndex, TextSize};

use crate::{Diagnostic, Severity};

/// Everything the renderer needs to know about one file.
#[derive(Debug)]
pub struct SourceView<'a> {
    pub name: &'a str,
    pub text: &'a str,
    pub line_index: &'a LineIndex,
}

/// A file a label points into, looked up by the caller — the label's file is
/// not always the diagnostic's: a `while compiling` note names the
/// instantiation site, which may be in the importing module.
#[derive(Debug)]
pub struct Located {
    pub name: String,
    pub text: String,
}

/// Renders one diagnostic, including its trailing newline. A label into
/// another file renders without an excerpt; [`render_legacy_located`] is the
/// entry that can find one.
pub fn render_legacy(diag: &Diagnostic, src: &SourceView<'_>) -> String {
    render_legacy_located(diag, src, &|_| None)
}

/// [`render_legacy`], with `locate` answering for a label's file when it is
/// not the diagnostic's own.
pub fn render_legacy_located(
    diag: &Diagnostic,
    src: &SourceView<'_>,
    locate: &dyn Fn(FileRange) -> Option<Located>,
) -> String {
    let mut out = String::new();
    if diag.severity == Severity::Error {
        // The prefix the harness greps for. Warnings carry no `Error` prefix —
        // they are rendered as a bare bundle (`Main.hs:91`).
        out.push_str(&format!("Error {}: ", diag.code.number()));
    }
    out.push_str(&render_block(diag, src, locate));
    out
}

/// Renders a list the way `concatMap errorBundlePretty` does: back to back, with
/// no separator beyond each block's own trailing newline.
pub fn render_legacy_all(diags: &[Diagnostic], src: &SourceView<'_>) -> String {
    diags.iter().map(|d| render_legacy(d, src)).collect()
}

/// The located half of a bundle: `file:line:col:`, the empty gutter line, the
/// source line, the caret. `errorBundlePretty`'s layout, shared by the
/// diagnostic and by every note nested under it — `addErrorContext` makes
/// each note a bundle of its own (`Error.hs:192-194`).
fn excerpt(name: &str, text: &str, index: &LineIndex, offset: TextSize) -> String {
    let pos = index.line_col(offset);
    let line_no = pos.line + 1;
    let col_no = pos.col + 1;
    let line_no_text = line_no.to_string();
    // megaparsec pads the gutter to the width of the line number plus one space.
    let pad = " ".repeat(line_no_text.len() + 1);

    let mut out = format!("{name}:{line_no}:{col_no}:\n");
    out.push_str(&format!("{pad}|\n"));
    let line_text = index.line_text(text, pos.line).unwrap_or("");
    // The line as megaparsec slices it, up to the `\n` only: a CRLF line
    // keeps its `\r` in the excerpt (`reachOffset`; the reference binary,
    // 2026-10-10), though the caret is counted without it.
    let raw_line = index
        .line_range(pos.line)
        .and_then(|r| text.get(usize::from(r.start())..usize::from(r.end())))
        .unwrap_or(line_text);
    // Tabs are expanded to a single space so the caret lines up with what
    // megaparsec prints (it uses a fixed tab width of 1 by default).
    let display_line: String = raw_line.chars().map(|c| if c == '\t' { ' ' } else { c }).collect();
    out.push_str(&format!("{line_no_text} | {display_line}\n"));

    // Caret column counts characters, not bytes: a non-ASCII prefix must not
    // shift the caret right.
    let caret_col = line_text
        .get(..pos.col as usize)
        .map_or(pos.col as usize, |prefix| prefix.chars().count());
    out.push_str(&format!("{pad}| {}^\n", " ".repeat(caret_col)));
    out
}

fn render_block(
    diag: &Diagnostic,
    src: &SourceView<'_>,
    locate: &dyn Fn(FileRange) -> Option<Located>,
) -> String {
    let mut out = excerpt(src.name, src.text, src.line_index, diag.range.range.start());
    out.push_str(&message_line(diag));
    out.push('\n');

    // `"note: " ++ errorBundlePretty x` (`Error.hs:70`): a label is a nested
    // bundle with its own excerpt, exactly as the reference prints a `while
    // compiling` frame. A label whose file nobody can open renders as its
    // message alone rather than being dropped.
    for label in &diag.labels {
        out.push_str("note: ");
        if label.range.file == diag.range.file {
            out.push_str(&excerpt(src.name, src.text, src.line_index, label.range.range.start()));
        } else if let Some(there) = locate(label.range) {
            let index = LineIndex::new(&there.text);
            out.push_str(&excerpt(&there.name, &there.text, &index, label.range.range.start()));
        }
        out.push_str(&label.message);
        out.push('\n');
    }
    for note in &diag.notes {
        out.push_str(&format!("note: {note}\n"));
    }
    out
}

fn message_line(diag: &Diagnostic) -> String {
    match diag.warning {
        // `warning: <msg> [--Wno-<kind>]` — `Error.hs:145`. The suffix must be
        // last on the line.
        Some(kind) => format!("warning: {} [--Wno-{}]", diag.message, kind.name()),
        None => diag.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, FileRange, LineIndex, TextRange, TextSize};

    use super::*;
    use crate::{Diagnostic, WarningKind};

    fn view<'a>(text: &'a str, index: &'a LineIndex) -> SourceView<'a> {
        SourceView { name: "syntax_test_temp_file.k", text, line_index: index }
    }

    fn at(text: &str, needle: &str) -> FileRange {
        let offset = text.find(needle).unwrap() as u32;
        FileRange::new(FileId(0), TextRange::at(TextSize::from(offset), TextSize::of(needle)))
    }

    /// A CRLF line keeps its `\r` in the excerpt, as megaparsec's slice of
    /// the line does (the reference binary, 2026-10-10); the caret is
    /// counted without it. Red with the trimmed `line_text` printed.
    #[test]
    fn a_crlf_line_keeps_its_carriage_return_in_the_excerpt() {
        let text = "inline void f()\r\n{\r\n    uint32 x = y;\r\n}\r\n";
        let index = LineIndex::new(text);
        let d = Diagnostic::error(at(text, "y;"), "Symbol name not in scope: `y`");
        let out = render_legacy_located(&d, &view(text, &index), &|_| None);
        assert!(out.contains("3 |     uint32 x = y;\r\n  |                ^\n"), "{out:?}");
    }

    #[test]
    fn error_block_matches_the_harness_regex() {
        let text = "inline void main()\n{\n    uint32 x = \"s\";\n}\n";
        let index = LineIndex::new(text);
        let diag = Diagnostic::error(at(text, "\"s\""), "unexpected string literal");
        let rendered = render_legacy(&diag, &view(text, &index));

        assert_eq!(
            rendered,
            "Error 1: syntax_test_temp_file.k:3:16:\n  |\n3 |     uint32 x = \"s\";\n  |                ^\nunexpected string literal\n"
        );
        // What `syntax-test.py` actually extracts.
        let first = rendered.lines().next().unwrap();
        assert!(first.contains("Error 1"));
    }

    #[test]
    fn warning_suffix_is_last_on_its_line() {
        let text = "uint8 x = 300;\n";
        let index = LineIndex::new(text);
        let diag = Diagnostic::warning(
            at(text, "300"),
            WarningKind::Conversion,
            "implicit conversion loses precision",
        );
        let rendered = render_legacy(&diag, &view(text, &index));

        let line = rendered.lines().find(|l| l.starts_with("warning:")).unwrap();
        assert!(line.ends_with("[--Wno-conversion]"), "harness regex is anchored at end of line: {line}");
        assert!(!rendered.contains("Error "), "warnings carry no Error prefix");
    }

    /// `kg-oracle --errors` on a template body error, 2026-09-07: the note
    /// is `note: ` and a whole nested bundle — header, gutter, caret, then
    /// the literal `while compiling` (`Error.hs:70,192-194`). A label into
    /// another file gets its excerpt from `locate`, and none at all when
    /// nobody can open the file.
    #[test]
    fn a_label_renders_as_a_nested_bundle() {
        let text = "class H\n{\n    bool b = v;\n}\nH<uint32> h;\n";
        let index = LineIndex::new(text);
        let diag = Diagnostic::error(at(text, "v;"), "Invalid conversion")
            .with_label(at(text, "H<uint32>"), "while compiling");
        let rendered = render_legacy(&diag, &view(text, &index));
        assert_eq!(
            rendered,
            "Error 1: syntax_test_temp_file.k:3:14:\n  |\n3 |     bool b = v;\n  |              ^\nInvalid conversion\n\
             note: syntax_test_temp_file.k:5:1:\n  |\n5 | H<uint32> h;\n  | ^\nwhile compiling\n"
        );

        let elsewhere = FileRange::new(FileId(7), TextRange::at(TextSize::from(0), TextSize::from(1)));
        let diag = Diagnostic::error(at(text, "v;"), "Invalid conversion").with_label(elsewhere, "while compiling");
        let unlocated = render_legacy(&diag, &view(text, &index));
        assert!(unlocated.ends_with("Invalid conversion\nnote: while compiling\n"), "{unlocated}");
        let located = render_legacy_located(&diag, &view(text, &index), &|r| {
            (r.file == FileId(7)).then(|| Located { name: "main.k".to_owned(), text: "X<1> x;\n".to_owned() })
        });
        assert!(located.ends_with("note: main.k:1:1:\n  |\n1 | X<1> x;\n  | ^\nwhile compiling\n"), "{located}");
    }

    #[test]
    fn caret_counts_characters_not_bytes() {
        let text = "// é\nx\n";
        let index = LineIndex::new(text);
        let diag = Diagnostic::error(at(text, "x"), "boom");
        let rendered = render_legacy(&diag, &view(text, &index));
        assert!(rendered.contains("2 | x\n  | ^\n"), "{rendered}");
    }
}
