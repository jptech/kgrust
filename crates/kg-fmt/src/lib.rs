//! A comment-preserving formatter for Kanagawa.
//!
//! The existing `kanagawa pretty` is an AST re-printer: it drops every comment,
//! parenthesizes every subexpression, emits `bitoffset` where the language
//! spells it `bitoffsetof`, has a typo in its own output, and is wired into no
//! test or CI target. This is not a port of it.
//!
//! Instead the formatter works over the lossless CST, where comments are real
//! tokens, and lowers it to a [`doc`] algebra that describes where breaks are
//! *allowed*. The renderer then chooses which to take.
//!
//! # The pipeline
//!
//! ```text
//!   text ──parse──▶ CST ──lower──▶ Doc ──render──▶ text ──align──▶ text
//!                    │                                              │
//!                    └──────────────── verify ───────────────────────┘
//! ```
//!
//! * [`lower`] walks the tree and states, per construct, where breaks may fall
//!   and what sits on either side of them. Every decision cites the measurement
//!   in `docs/spec/style.md` that fixed it.
//! * [`doc::render`] takes the breaks that the line width demands, and every
//!   break a comment forces.
//! * [`align`] restores the hand-built columns the corpus is full of — 144
//!   aligned trailing-comment runs — which no width-driven layout can express.
//! * [`verify`] proves the result is the same program: same tokens, same
//!   comments, same order.
//!
//! # Refusing to format
//!
//! [`format_file`] returns [`FormatError::Unparseable`] rather than formatting a
//! file with syntax errors. A half-typed file has a tree full of `ERROR` nodes
//! whose shape is a guess, and laying that guess out is how a formatter
//! destroys work in progress. The same rule keeps the 69 multi-case harness
//! fixtures under `test/syntax/` (`docs/spec/style.md` §0.3) safe without any
//! special case: they are not translation units, so they do not parse, so they
//! are not touched.

pub mod doc;

mod align;
mod lower;
mod reattach;
mod trivia;
mod verify;

pub use doc::{Doc, RenderOpts, render};

use kg_base::language::Features;
use kg_base::{TextRange, TextSize};
use kg_config::Fmt;
use kg_syntax::{Parse, ParseOptions, SyntaxNode, parse_file_with};

/// A replacement of one span of the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: TextRange,
    pub new_text: String,
}

/// One reason the input could not be formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Byte offset the parser reported the error at.
    pub offset: u32,
    pub message: String,
}

/// Why formatting did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The file has syntax errors. Formatting it would be laying out a guess.
    Unparseable { errors: Vec<Problem> },
    /// The formatter produced text that is not the same program — a bug in
    /// this crate, reported instead of written to disk.
    Verification { detail: String },
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Unparseable { errors } => {
                let first = errors.first();
                write!(
                    f,
                    "cannot format a file with syntax errors ({} found{})",
                    errors.len(),
                    match first {
                        Some(e) => format!("; first at offset {}: {}", e.offset, e.message),
                        None => String::new(),
                    }
                )
            }
            FormatError::Verification { detail } => {
                write!(f, "the formatter changed more than layout: {detail}")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// Formats a whole file in the reference language ([`format_file_in`]).
pub fn format_file(text: &str, config: &Fmt) -> Result<String, FormatError> {
    format_file_in(text, config, Features::NONE)
}

/// Formats a whole file written in a language with `features`
/// (`kg_base::language`) — its own package's.
///
/// Refuses a file with syntax errors: see the module docs. So a file that
/// uses a feature its package has not turned on is refused, as it would be
/// by every other command.
pub fn format_file_in(text: &str, config: &Fmt, features: Features) -> Result<String, FormatError> {
    let parse = parse_file_with(text, ParseOptions { features, ..ParseOptions::default() });
    if !parse.errors().is_empty() {
        return Err(FormatError::Unparseable {
            errors: parse
                .errors()
                .iter()
                .map(|e| Problem {
                    offset: u32::from(e.range.start()),
                    message: e.message.clone(),
                })
                .collect(),
        });
    }
    let root = parse.syntax_node();
    let out = render_file(&root, config);
    verify::check(&root, &out, !config.sort_imports)
        .map_err(|detail| FormatError::Verification { detail })?;
    Ok(out)
}

/// Whether `text` is already formatted.
///
/// A file that cannot be formatted is not "already formatted": `kg fmt --check`
/// should draw attention to it rather than pass it silently.
pub fn check_file(text: &str, config: &Fmt) -> bool {
    format_file(text, config).is_ok_and(|out| out == text)
}

/// Formats the declarations and statements that overlap `range`.
///
/// Returns `None` if the file does not parse, or if the result would not have
/// been the same program — range formatting is a convenience, and declining is
/// always better than a wrong edit.
///
/// The edit covers whole items: an editor's selection rarely lands on a
/// construct boundary, and reformatting half of an expression is meaningless.
pub fn format_range(text: &str, range: TextRange, config: &Fmt) -> Option<Vec<TextEdit>> {
    format_range_in(text, range, config, Features::NONE)
}

/// [`format_range`] for a file written in a language with `features`.
pub fn format_range_in(text: &str, range: TextRange, config: &Fmt, features: Features) -> Option<Vec<TextEdit>> {
    let parse = parse_file_with(text, ParseOptions { features, ..ParseOptions::default() });
    if !parse.errors().is_empty() {
        return None;
    }
    let root = parse.syntax_node();
    let host = enclosing_list(&root, range)?;

    let lowerer = lower::Lowerer::new(config);
    let (doc, span) = lowerer.range_doc(&host, range)?;

    // The first line keeps the indentation already in the file; the rest are
    // shifted to match it.
    let indent = column_of(text, span.start());
    let opts = RenderOpts {
        width: config.line_width.saturating_sub(indent).max(20),
        indent: config.indent,
        use_tabs: config.use_tabs,
        newline: config.newline.as_str(),
    };
    let body = render(&doc, opts);
    let pad = " ".repeat(indent);
    let nl = config.newline.as_str();
    let new_text = body
        .split('\n')
        .enumerate()
        .map(|(i, l)| {
            if i == 0 || l.is_empty() {
                l.to_owned()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join(nl);

    let mut patched = String::with_capacity(text.len() + new_text.len());
    patched.push_str(&text[..usize::from(span.start())]);
    patched.push_str(&new_text);
    patched.push_str(&text[usize::from(span.end())..]);
    // Lifted comments land inside the edited span — their anchor tokens are the
    // ones just re-rendered — so the whole file is the right thing to reattach
    // against, and the tail beyond the span is untouched.
    let patched = reattach::reattach(&root, &lowerer.lifted(), patched);
    verify::check(&root, &patched, !config.sort_imports).ok()?;

    let tail = text.len() - usize::from(span.end());
    let new_text = patched.get(usize::from(span.start())..patched.len() - tail)?.to_owned();
    if text[usize::from(span.start())..usize::from(span.end())] == new_text {
        return Some(Vec::new());
    }
    Some(vec![TextEdit { range: span, new_text }])
}

/// The formatter's own output, parsed again — to align it, reattach its
/// comments and verify it — with every feature on: the input parsed clean in
/// its language, the output holds the same tokens, and a feature never
/// changes the parse of a program the language without it accepts
/// (`docs/plans/editions.md` §1), so the language need not travel here.
pub(crate) fn reparse(text: &str) -> Parse {
    parse_file_with(text, ParseOptions { features: Features::all(), ..ParseOptions::default() })
}

/// Lowers, renders and tidies a parsed file.
fn render_file(root: &SyntaxNode, config: &Fmt) -> String {
    let lowerer = lower::Lowerer::new(config);
    let doc = lowerer.file(root);
    let opts = RenderOpts {
        width: config.line_width,
        indent: config.indent,
        use_tabs: config.use_tabs,
        newline: config.newline.as_str(),
    };
    let mut out = reattach::reattach(root, &lowerer.lifted(), render(&doc, opts));
    if !config.use_tabs {
        out = align::realign(root, out);
    }
    // `render` never leaves trailing whitespace on a line it ends, but the last
    // line has no such break behind it.
    let end = out.trim_end().len();
    out.truncate(end);
    if config.final_newline && !out.is_empty() {
        out.push_str(config.newline.as_str());
    }
    out
}

/// The innermost node whose children are a list of items covering `range`.
fn enclosing_list(root: &SyntaxNode, range: TextRange) -> Option<SyntaxNode> {
    use kg_syntax::SyntaxKind::*;
    let start = root
        .covering_element(TextRange::new(
            range.start().min(root.text_range().end()),
            range.end().min(root.text_range().end()),
        ))
        .into_node()
        .unwrap_or_else(|| root.clone());
    std::iter::successors(Some(start), |n| n.parent())
        .find(|n| matches!(n.kind(), SOURCE_FILE | BLOCK_STMT | DECL_LIST))
}

fn column_of(text: &str, at: TextSize) -> usize {
    let at = usize::from(at).min(text.len());
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..at].chars().count()
}

#[cfg(test)]
mod profile {
    //! A stopwatch, not a test.
    //!
    //! `xtask bench` times the formatter as one stage, which is the right
    //! granularity for spotting a regression but the wrong one for fixing it.
    //! This breaks `format_file` into its six steps against the largest file in
    //! the corpus, so an optimisation can be aimed rather than guessed at.
    //!
    //! ```text
    //! cargo test -p kg-fmt --release --lib -- --ignored --nocapture profile
    //! ```
    //!
    //! It found the one that mattered: `Doc::drop` was allocating a fresh
    //! `Rc<DocKind::Nil>` per node, so tearing a document down cost as many
    //! allocations as building it. It also refuted the obvious guess first —
    //! the token-walking predicates in `trivia` are only ~10% of lowering.

    use super::*;
    use kg_syntax::parse_file;

    #[test]
    #[ignore = "timing, not correctness"]
    fn profile_the_stages() {
        let path = kg_base::reference_root().join("library/numeric/float32.k");
        let Ok(text) = std::fs::read_to_string(&path) else {
            kg_base::corpus_absent(&format!("no corpus at {}; set KANAGAWA_ROOT", path.display()));
            return;
        };
        let config = Fmt::default();
        let parse = parse_file(&text);
        let root = parse.syntax_node();
        let out = format_file(&text, &config).expect("formats");

        let time = |name: &str, f: &dyn Fn()| {
            f();
            let t0 = std::time::Instant::now();
            for _ in 0..5 {
                f();
            }
            println!("{name:<24} {:>8.2} ms", t0.elapsed().as_secs_f64() * 1000.0 / 5.0);
        };

        time("parse", &|| {
            std::hint::black_box(parse_file(&text));
        });
        time("lower -> Doc", &|| {
            let l = lower::Lowerer::new(&config);
            std::hint::black_box(l.file(&root));
        });
        let lowerer = lower::Lowerer::new(&config);
        let doc = lowerer.file(&root);
        let opts = RenderOpts {
            width: config.line_width,
            indent: config.indent,
            use_tabs: config.use_tabs,
            newline: config.newline.as_str(),
        };
        time("render(Doc)", &|| {
            std::hint::black_box(render(&doc, opts));
        });
        let rendered = render(&doc, opts);
        time("reattach", &|| {
            std::hint::black_box(reattach::reattach(&root, &lowerer.lifted(), rendered.clone()));
        });
        let reattached = reattach::reattach(&root, &lowerer.lifted(), rendered.clone());
        time("align::realign", &|| {
            std::hint::black_box(align::realign(&root, reattached.clone()));
        });
        time("verify::check", &|| {
            std::hint::black_box(verify::check(&root, &out, true)).ok();
        });
        time("format_file (total)", &|| {
            std::hint::black_box(format_file(&text, &config)).ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(text: &str) -> String {
        format_file(text, &Fmt::default()).expect("should format")
    }

    /// A named argument's colon hugs its label (`lower`'s `ARG_LABEL`), the
    /// range path agrees, and a file whose language lacks the feature is
    /// refused as unparseable rather than laid out as a guess. Red with the
    /// label laid out by the generic spacing, `y : 1`.
    #[test]
    fn named_arguments_are_formatted_in_their_language() {
        use kg_base::language::Feature;
        let named = Features::NONE.with(Feature::NamedArguments);
        let text = "inline void f()\n{\n    g( y :1,x:  h(z : 2) );\n}\n";
        let want = "inline void f()\n{\n    g(y: 1, x: h(z: 2));\n}\n";
        assert_eq!(format_file_in(text, &Fmt::default(), named).unwrap(), want);
        let whole = TextRange::up_to(TextSize::of(text));
        let edits = format_range_in(text, whole, &Fmt::default(), named).expect("formats the range");
        assert!(edits.iter().any(|e| e.new_text.contains("g(y: 1, x: h(z: 2));")), "{edits:?}");
        assert!(matches!(format_file(text, &Fmt::default()), Err(FormatError::Unparseable { .. })));
    }

    /// Edition 1.1's trailing commas are kept, a list that holds one laid
    /// out as any other — inline where it fits, one element a line where it
    /// does not — and the layout is a fixpoint.
    #[test]
    fn a_trailing_comma_is_kept() {
        use kg_base::language::Feature;
        let trailing = Features::NONE.with(Feature::TrailingCommas);
        let text = "enum E : uint2 { A, B, }\n\ninline void f(uint8 x,uint8 y ,)\n{\n    g( 1,2 , );\n}\n";
        let once = format_file_in(text, &Fmt::default(), trailing).unwrap();
        assert!(once.contains("g(1, 2,);"), "{once}");
        assert!(once.contains("inline void f(uint8 x, uint8 y,)"), "{once}");
        assert_eq!(format_file_in(&once, &Fmt::default(), trailing).unwrap(), once, "a fixpoint");
        let long = format!("inline void f()\n{{\n    g({});\n}}\n", (0..30).map(|i| format!("argument_{i},")).collect::<String>());
        let once = format_file_in(&long, &Fmt::default(), trailing).unwrap();
        assert_eq!(format_file_in(&once, &Fmt::default(), trailing).unwrap(), once, "a broken list is a fixpoint too");
    }

    #[test]
    fn an_empty_file_is_left_empty() {
        assert_eq!(fmt(""), "");
    }

    #[test]
    fn a_file_with_syntax_errors_is_refused() {
        let err = format_file("class {", &Fmt::default()).unwrap_err();
        assert!(matches!(err, FormatError::Unparseable { .. }), "{err}");
        assert!(err.to_string().contains("syntax errors"));
    }

    #[test]
    fn formatting_is_idempotent_on_a_realistic_file() {
        let src = "// Copyright (c) Microsoft Corporation.\n\
                   // Licensed under the MIT License.\n\n\
                   module data.fifo\n    { FIFO\n    , fifo_nb\n    }\n\n\
                   import data.counter\n\n\
                   //| Basic FIFO.\nclass FIFO\n{\npublic:\n    \
                   inline void put(uint8 x)\n    {\n        _n += 1;\n    }\n}\n";
        let once = fmt(src);
        assert_eq!(fmt(&once), once, "not idempotent:\n{once}");
    }

    #[test]
    fn a_trailing_comment_keeps_its_line() {
        let out = fmt("inline void f()\n{\n    g(); // done\n}\n");
        assert!(out.contains("g(); // done"), "{out}");
    }

    #[test]
    fn a_file_that_is_only_a_comment_gains_nothing_but_a_final_newline() {
        assert_eq!(fmt("// only a comment"), "// only a comment\n");
        assert_eq!(fmt("/* block */"), "/* block */\n");
        assert_eq!(fmt("\n\n\n"), "");
    }

    #[test]
    fn a_string_literal_is_never_touched() {
        // §10.10: tabs and `{{` escapes inside a string are load-bearing —
        // `test/library/processor/risc-v.k` compares disassembler output
        // against tab-containing strings.
        let src = "inline void f()\n{\n    auto s = \"lui\\ta4, {x:b3} {{ literal\";\n}\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn a_missing_final_newline_is_added_and_trailing_space_stripped() {
        // §1.5 and §1.6: 228 lines with trailing whitespace in 60 files, and
        // 86 of 480 files end without a newline.
        assert_eq!(fmt("using A = uint8;  "), "using A = uint8;\n");
    }

    #[test]
    fn crlf_output_is_available_but_not_the_default() {
        let cfg = Fmt { newline: kg_config::Newline::Crlf, ..Fmt::default() };
        let out = format_file("using A = uint8;\nusing B = uint9;\n", &cfg).unwrap();
        assert_eq!(out, "using A = uint8;\r\nusing B = uint9;\r\n");
    }

    #[test]
    fn check_file_agrees_with_format_file() {
        let src = "inline void f()\n{\n    g();\n}\n";
        let formatted = fmt(src);
        assert!(check_file(&formatted, &Fmt::default()));
        assert!(!check_file("inline void f(){g();}", &Fmt::default()));
    }
}
