//! Diagnostic rendering for `kg`.
//!
//! `kg-diag::render_legacy` reproduces megaparsec's layout, which the legacy
//! harness greps and which `kanagawa` must keep emitting. `kg` is not bound by
//! that, so it prints the rustc-shaped form instead: severity and message first,
//! then a located source excerpt.
//!
//! ```text
//! error: unexpected token
//!   --> src/top.k:3:16
//!    |
//!  3 |     uint32 x = ;
//!    |                ^
//! ```

use kg_base::{FileRange, LineIndex, TextSize};
use kg_diag::{Diagnostic, Severity};

use super::style::Styles;

/// A file a label points into, looked up by the caller: a `while compiling`
/// note names the instantiation site, which may be in another module.
#[derive(Debug)]
pub struct Located {
    pub name: String,
    pub text: kg_vfs::FileText,
}

/// `--error-format`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum ErrorFormat {
    /// Multi-line, with a source excerpt and a caret.
    #[default]
    Human,
    /// One line per diagnostic: `file:line:col: severity: message`.
    Short,
    /// One JSON object per line.
    Json,
}

/// One file's text and name, for rendering.
#[derive(Debug)]
pub struct Source<'a> {
    pub name: &'a str,
    pub text: &'a str,
    pub line_index: &'a LineIndex,
}

pub fn render(
    diagnostic: &Diagnostic,
    source: &Source<'_>,
    format: ErrorFormat,
    styles: &Styles,
) -> String {
    render_with(diagnostic, source, &|_| None, format, styles)
}

/// [`render`], with `locate` answering for a label's file when it is not
/// the diagnostic's own. A label nobody can locate renders as its message
/// alone rather than being dropped.
pub fn render_with(
    diagnostic: &Diagnostic,
    source: &Source<'_>,
    locate: &dyn Fn(FileRange) -> Option<Located>,
    format: ErrorFormat,
    styles: &Styles,
) -> String {
    match format {
        ErrorFormat::Human => human(diagnostic, source, locate, styles),
        ErrorFormat::Short => short(diagnostic, source, locate),
        ErrorFormat::Json => json(diagnostic, source, locate),
    }
}

/// `(file name, line, column)` of a label, 1-based, through the diagnostic's
/// own source when the label is in the same file and `locate` otherwise.
fn label_position(
    range: FileRange,
    own: FileRange,
    source: &Source<'_>,
    locate: &dyn Fn(FileRange) -> Option<Located>,
) -> Option<(String, u32, u32)> {
    if range.file == own.file {
        let pos = source.line_index.line_col(range.range.start());
        return Some((source.name.to_owned(), pos.line + 1, pos.col + 1));
    }
    let there = locate(range)?;
    let pos = there.text.line_index.line_col(range.range.start());
    Some((there.name, pos.line + 1, pos.col + 1))
}

fn severity_word(diagnostic: &Diagnostic) -> &'static str {
    match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn human(
    diagnostic: &Diagnostic,
    source: &Source<'_>,
    locate: &dyn Fn(FileRange) -> Option<Located>,
    styles: &Styles,
) -> String {
    let position = source.line_index.line_col(diagnostic.range.range.start());
    let line_no = position.line + 1;
    let col_no = position.col + 1;
    // One gutter for the whole diagnostic, wide enough for every excerpt it
    // prints — the primary and each label's — so the bars line up.
    let gutter = diagnostic
        .labels
        .iter()
        .filter_map(|l| label_position(l.range, diagnostic.range, source, locate))
        .map(|(_, line, _)| line.to_string().len())
        .fold(line_no.to_string().len(), usize::max);
    let pad = " ".repeat(gutter);

    let head = match diagnostic.severity {
        Severity::Error => styles.error(severity_word(diagnostic)),
        Severity::Warning => styles.warning(severity_word(diagnostic)),
    };

    // The `[--Wno-<name>]` suffix is the legacy switch spelling; `kg` shows the
    // modern one, since `kg` is where `-W no-shadow` is typed.
    let suffix = match diagnostic.warning {
        Some(kind) => format!(" {}", styles.dim(&format!("[-W no-{}]", kind.name()))),
        None => String::new(),
    };

    let mut out = format!("{head}: {}{suffix}\n", styles.bold(&diagnostic.message));
    out.push_str(&format!(
        "{pad}{} {}:{line_no}:{col_no}\n",
        styles.blue("-->"),
        source.name
    ));
    out.push_str(&format!("{pad} {}\n", styles.blue("|")));

    let line_text = source.line_index.line_text(source.text, position.line).unwrap_or("");
    let display: String = line_text.chars().map(|c| if c == '\t' { ' ' } else { c }).collect();
    out.push_str(&format!("{line_no:>gutter$} {} {display}\n", styles.blue("|")));

    // Carets count characters, not bytes, so a non-ASCII prefix does not shift
    // the marker to the right.
    let caret_col = line_text
        .get(..position.col as usize)
        .map_or(position.col as usize, |prefix| prefix.chars().count());
    let width = span_width(line_text, position.col as usize, diagnostic.range.range.len());
    let caret = match diagnostic.severity {
        Severity::Error => styles.error(&"^".repeat(width)),
        Severity::Warning => styles.warning(&"^".repeat(width)),
    };
    out.push_str(&format!(
        "{pad} {} {}{caret}\n",
        styles.blue("|"),
        " ".repeat(caret_col)
    ));

    // A label is a note with a place: its own `-->` and excerpt, the way
    // rustc prints a secondary span. Same file, same line index; another
    // file through `locate`; no file, the message alone.
    for label in &diagnostic.labels {
        out.push_str(&format!("{pad} {} note: {}\n", styles.blue("="), label.said()));
        let located: Located;
        let (name, text, index): (&str, &str, &LineIndex) = if label.range.file == diagnostic.range.file {
            (source.name, source.text, source.line_index)
        } else if let Some(there) = locate(label.range) {
            located = there;
            (located.name.as_str(), located.text.text.as_str(), &located.text.line_index)
        } else {
            continue;
        };
        let at = index.line_col(label.range.range.start());
        let line_no = at.line + 1;
        let line_no_text = format!("{line_no:>gutter$}");
        out.push_str(&format!("{pad}{} {name}:{line_no}:{}\n", styles.blue("-->"), at.col + 1));
        out.push_str(&format!("{pad} {}\n", styles.blue("|")));
        let line_text = index.line_text(text, at.line).unwrap_or("");
        let display: String = line_text.chars().map(|c| if c == '\t' { ' ' } else { c }).collect();
        out.push_str(&format!("{line_no_text} {} {display}\n", styles.blue("|")));
        let caret_col = line_text
            .get(..at.col as usize)
            .map_or(at.col as usize, |prefix| prefix.chars().count());
        let width = span_width(line_text, at.col as usize, label.range.range.len());
        out.push_str(&format!("{pad} {} {}{}\n", styles.blue("|"), " ".repeat(caret_col), styles.blue(&"-".repeat(width))));
    }
    for note in &diagnostic.notes {
        out.push_str(&format!("{pad} {} note: {note}\n", styles.blue("=")));
    }
    for fix in &diagnostic.fixes {
        out.push_str(&format!("{pad} {} help: {}\n", styles.blue("="), fix.message));
    }
    out
}

/// How many markers a range starting at byte `col` of `line_text` gets: one
/// per character it covers on that line — a range running onto later lines
/// is clipped to this one — and at least one, since a zero-length range is
/// how a parse error is reported.
fn span_width(line_text: &str, col: usize, len: TextSize) -> usize {
    let end = col.saturating_add(u32::from(len) as usize).min(line_text.len());
    line_text.get(col..end).map_or(0, |covered| covered.chars().count()).max(1)
}

fn short(
    diagnostic: &Diagnostic,
    source: &Source<'_>,
    locate: &dyn Fn(FileRange) -> Option<Located>,
) -> String {
    let position = source.line_index.line_col(diagnostic.range.range.start());
    let mut out = format!(
        "{}:{}:{}: {}: {}\n",
        source.name,
        position.line + 1,
        position.col + 1,
        severity_word(diagnostic),
        diagnostic.message
    );
    // One greppable line per note too, so `file:line:col: note: …` sorts
    // and filters beside the error it belongs to.
    for label in &diagnostic.labels {
        if let Some((name, line, col)) = label_position(label.range, diagnostic.range, source, locate) {
            out.push_str(&format!("{name}:{line}:{col}: note: {}\n", label.said()));
        }
    }
    out
}

fn json(
    diagnostic: &Diagnostic,
    source: &Source<'_>,
    locate: &dyn Fn(FileRange) -> Option<Located>,
) -> String {
    let position = source.line_index.line_col(diagnostic.range.range.start());
    let warning = match diagnostic.warning {
        Some(kind) => format!("\"{}\"", kind.name()),
        None => "null".to_owned(),
    };
    // Which message this is, where a pass decides by it (`DiagnosticKind`).
    let kind = match diagnostic.kind {
        Some(kind) => format!("\"{}\"", kind.name()),
        None => "null".to_owned(),
    };
    let notes: Vec<String> = diagnostic
        .labels
        .iter()
        .filter_map(|label| {
            let (name, line, col) = label_position(label.range, diagnostic.range, source, locate)?;
            Some(format!(
                "{{\"file\":{},\"line\":{line},\"column\":{col},\"message\":{}}}",
                quote(&name),
                quote(&label.said())
            ))
        })
        .collect();
    format!(
        "{{\"file\":{},\"line\":{},\"column\":{},\"severity\":\"{}\",\"code\":{},\"warning\":{},\"kind\":{},\"message\":{},\"notes\":[{}]}}\n",
        quote(source.name),
        position.line + 1,
        position.col + 1,
        severity_word(diagnostic),
        diagnostic.code.number(),
        warning,
        kind,
        quote(&diagnostic.message),
        notes.join(",")
    )
}

/// Minimal RFC 8259 string escaping. A JSON dependency for one string kind
/// would be a heavier commitment than the twelve lines it saves.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_base::{FileId, FileRange, TextRange, TextSize};
    use kg_diag::WarningKind;

    fn diag(text: &str, needle: &str, message: &str) -> Diagnostic {
        let offset = text.find(needle).unwrap() as u32;
        Diagnostic::error(
            FileRange::new(
                FileId(0),
                TextRange::at(TextSize::from(offset), TextSize::of(needle)),
            ),
            message,
        )
    }

    fn source<'a>(text: &'a str, index: &'a LineIndex) -> Source<'a> {
        Source { name: "src/top.k", text, line_index: index }
    }

    #[test]
    fn the_human_form_points_at_the_right_column() {
        let text = "inline void f()\n{\n    uint32 x = \"s\";\n}\n";
        let index = LineIndex::new(text);
        let rendered = render(
            &diag(text, "\"s\"", "unexpected string literal"),
            &source(text, &index),
            ErrorFormat::Human,
            &Styles::plain(),
        );
        assert_eq!(
            rendered.lines().collect::<Vec<_>>(),
            [
                "error: unexpected string literal",
                " --> src/top.k:3:16",
                "  |",
                "3 |     uint32 x = \"s\";",
                "  |                ^^^",
            ]
        );
    }

    /// The carets under a range count its characters, not its bytes:
    /// `"é"` is three characters and four bytes. Red with the width read
    /// off the range's length.
    #[test]
    fn a_range_s_carets_count_its_characters() {
        let text = "inline void f()\n{\n    uint32 x = \"\u{e9}\";\n}\n";
        let index = LineIndex::new(text);
        let rendered = render(&diag(text, "\"\u{e9}\"", "unexpected string literal"), &source(text, &index), ErrorFormat::Human, &Styles::plain());
        assert_eq!(rendered.lines().last(), Some("  |                ^^^"), "{rendered}");
    }

    /// A `while compiling` label prints as a note with its own excerpt —
    /// in the diagnostic's file through its index, in another file through
    /// `locate`, and as the bare note line when nobody can open the file.
    #[test]
    fn a_label_is_a_note_with_its_own_excerpt() {
        let text = "class H\n{\n    bool b = v;\n}\nH<uint32> h;\n";
        let index = LineIndex::new(text);
        let mut diag = diag(text, "v;", "Invalid conversion");
        diag.labels.push(kg_diag::Label {
            range: FileRange::new(FileId(0), TextRange::at(TextSize::from(text.find("H<uint32>").unwrap() as u32), TextSize::from(9))),
            message: "while compiling".to_owned(),
            instance: None,
        });
        let rendered = render(&diag, &source(text, &index), ErrorFormat::Human, &Styles::plain());
        assert!(
            rendered.ends_with(
                "  = note: while compiling\n --> src/top.k:5:1\n  |\n5 | H<uint32> h;\n  | ---------\n"
            ),
            "{rendered}"
        );
        let short = render(&diag, &source(text, &index), ErrorFormat::Short, &Styles::plain());
        assert_eq!(
            short,
            "src/top.k:3:14: error: Invalid conversion\nsrc/top.k:5:1: note: while compiling\n"
        );

        // Another file: the bare note without `locate`, the excerpt with it.
        diag.labels[0].range = FileRange::new(FileId(9), TextRange::at(TextSize::from(0), TextSize::from(1)));
        let bare = render(&diag, &source(text, &index), ErrorFormat::Human, &Styles::plain());
        assert!(bare.ends_with("  = note: while compiling\n"), "{bare}");
        let locate = |r: FileRange| {
            (r.file == FileId(9)).then(|| Located { name: "use.k".to_owned(), text: kg_vfs::FileText::new("X<1> x;\n".to_owned()) })
        };
        let located = render_with(&diag, &source(text, &index), &locate, ErrorFormat::Human, &Styles::plain());
        assert!(located.ends_with("  = note: while compiling\n --> use.k:1:1\n  |\n1 | X<1> x;\n  | -\n"), "{located}");
        let json = render_with(&diag, &source(text, &index), &locate, ErrorFormat::Json, &Styles::plain());
        assert!(json.contains("\"notes\":[{\"file\":\"use.k\",\"line\":1,\"column\":1,\"message\":\"while compiling\"}]"), "{json}");
    }

    #[test]
    fn the_short_form_is_one_greppable_line() {
        let text = "uint8 x = 300;\n";
        let index = LineIndex::new(text);
        let rendered = render(
            &diag(text, "300", "value does not fit"),
            &source(text, &index),
            ErrorFormat::Short,
            &Styles::plain(),
        );
        assert_eq!(rendered, "src/top.k:1:11: error: value does not fit\n");
    }

    #[test]
    fn the_json_form_escapes_the_message() {
        let text = "x\n";
        let index = LineIndex::new(text);
        let rendered = render(
            &diag(text, "x", "unexpected \"quote\"\nand a newline"),
            &source(text, &index),
            ErrorFormat::Json,
            &Styles::plain(),
        );
        assert!(rendered.contains(r#""message":"unexpected \"quote\"\nand a newline""#), "{rendered}");
        assert!(rendered.contains(r#""code":1"#), "{rendered}");
        assert!(rendered.ends_with("}\n"));
    }

    /// The JSON form names a diagnostic's kind where it has one
    /// (`kg_diag::DiagnosticKind`) and says `null` where it has none; the
    /// code stays the reference's `1`.
    #[test]
    fn the_json_form_carries_the_kind() {
        let text = "x\n";
        let index = LineIndex::new(text);
        let json = |d: &Diagnostic| render(d, &source(text, &index), ErrorFormat::Json, &Styles::plain());
        let plain = diag(text, "x", "m");
        assert!(json(&plain).contains(r#""kind":null"#), "{}", json(&plain));
        let kinded = plain.with_kind(kg_diag::DiagnosticKind::CouldNotDeduceAll);
        assert!(json(&kinded).contains(r#""code":1,"warning":null,"kind":"could-not-deduce-all""#), "{}", json(&kinded));
    }

    #[test]
    fn a_warning_shows_the_modern_switch_spelling() {
        let text = "uint8 x = 300;\n";
        let index = LineIndex::new(text);
        let offset = text.find("300").unwrap() as u32;
        let diagnostic = Diagnostic::warning(
            FileRange::new(FileId(0), TextRange::at(TextSize::from(offset), TextSize::from(3))),
            WarningKind::Conversion,
            "implicit conversion loses precision",
        );
        let rendered = render(
            &diagnostic,
            &source(text, &index),
            ErrorFormat::Human,
            &Styles::plain(),
        );
        assert!(rendered.starts_with("warning: implicit conversion loses precision [-W no-conversion]"), "{rendered}");
    }
}
