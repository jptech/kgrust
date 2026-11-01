//! From text to a tree.
//!
//! `kg-parser` produces a flat event stream; this turns it into a `rowan` green
//! tree. Green nodes are `Send + Sync` and structurally shared, so a parse can
//! be cached and handed between threads; the red tree that accessors walk is
//! built per-thread and is O(1) to create.

use kg_base::language::Features;
use kg_base::{FileId, FileRange, TextRange};
use kg_diag::Diagnostic;
use kg_parser::{LexedStr, Step};
use rowan::GreenNode;

use crate::{SyntaxNode, ast};

/// The result of parsing one file: a tree, plus everything that went wrong.
///
/// A `Parse` is always produced. There is no failure mode — a file of pure
/// garbage yields a tree of `ERROR` nodes and a list of diagnostics, because a
/// language server has to describe a file that is halfway through being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<SyntaxError>,
    /// The language the file was parsed as. Part of the parse's identity:
    /// `kg_db` keeps an equal re-parse's old value, and the item tree, the
    /// sorted lowering and the kept loop are keyed on it, so a file whose
    /// language changed must never be "the same parse" even where its text
    /// parses to the same tree.
    features: Features,
}

/// A syntax error with the offset it was detected at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub range: TextRange,
}

impl Parse {
    /// The untyped tree root. Cheap: building a red node is a pointer wrap.
    pub fn syntax_node(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// The typed root.
    pub fn source_file(&self) -> ast::SourceFile {
        use crate::AstNode;
        ast::SourceFile::cast(self.syntax_node()).expect("the root is always a SOURCE_FILE")
    }

    pub fn green(&self) -> &GreenNode {
        &self.green
    }

    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }

    /// What the file was parsed as accepting beyond the reference.
    pub fn features(&self) -> Features {
        self.features
    }

    /// The source text, reconstructed from the tree.
    ///
    /// Equal to the input for every input, which is the losslessness property
    /// the formatter and every code action depend on.
    pub fn text(&self) -> String {
        self.syntax_node().text().to_string()
    }

    /// Renders the syntax errors as compiler diagnostics.
    ///
    /// Every one is `Error 1`: the reference has exactly one frontend error code
    /// (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114`), so the message
    /// text is ours to make good and the *code* is fixed.
    pub fn diagnostics(&self, file: FileId) -> Vec<Diagnostic> {
        self.errors
            .iter()
            .map(|e| Diagnostic::error(FileRange::new(file, e.range), e.message.clone()))
            .collect()
    }
}

/// How a file is parsed. The reference's parser `Config` (`Parser/Lexer.hs:67-73`),
/// minus `skipMemberBody` / `skipClassBody`, which are `chkdoc`'s outlining
/// switches and have no consumer here yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// `parseDocs`: reject documentation comments that sit where no
    /// declaration can carry them (`Parser/Lexer.hs:76-88`, `Parser.hs:92-97`).
    /// Off by default, like the reference's `--parse-docs`
    /// (`Options/CmdArgs.hs:66`), where every comment is whitespace.
    pub parse_docs: bool,
    /// The language beyond the reference the file may use
    /// (`kg_base::language`): its package's edition and features. Empty —
    /// the reference — unless the file's own manifest opts in.
    pub features: Features,
}

/// Parses a whole source file with the default [`ParseOptions`].
pub fn parse_file(text: &str) -> Parse {
    parse_file_with(text, ParseOptions::default())
}

/// Parses a whole source file.
///
/// `parse_docs` changes **only** the error list, never the tree: doc comments
/// stay in the tree as trivia either way, so losslessness and the formatter
/// are flag-independent, and the flag's whole observable effect is the
/// [`crate::doc_comments`] placement errors the reference's grammar produces
/// with `parseDocs = True`.
pub fn parse_file_with(text: &str, options: ParseOptions) -> Parse {
    parse_with(text, options)
}

fn parse_with(text: &str, options: ParseOptions) -> Parse {
    let lexed = LexedStr::new(text);
    let mut input = kg_parser::Input::from_lexed(&lexed);
    input.set_features(options.features);
    let mut parse = build(&lexed, kg_parser::parse_input(&input), options);

    // The binding-aware re-read (`reread.rs`): every `name <` the angle scan
    // took for a template argument list although the tree shows `name` bound
    // as a value is marked a comparison, and the file parsed again. A marked
    // `<` stays marked, so each round only grows the set and the loop ends;
    // the bound is for a grammar bug, not for any file — one round settles
    // every corpus file that needs one.
    for _ in 0..8 {
        let found = crate::reread::value_headed_angles(&parse.syntax_node());
        if found.is_empty() {
            break;
        }
        let mut marked = false;
        let mut index = 0usize;
        for i in 0..lexed.len() {
            if lexed.kind(i).is_trivia() {
                continue;
            }
            if found.contains(&lexed.range(i).start()) && !input.is_marked_comparison(index) {
                input.mark_comparison(index);
                marked = true;
            }
            index += 1;
        }
        if !marked {
            break;
        }
        parse = build(&lexed, kg_parser::parse_input(&input), options);
    }
    parse
}

/// The tree and the error list for one run of the parser over `lexed`.
fn build(lexed: &LexedStr<'_>, events: Vec<kg_parser::Event>, options: ParseOptions) -> Parse {
    let mut builder = rowan::GreenNodeBuilder::new();
    let mut errors = Vec::new();
    let mut depth = 0usize;

    // Lex errors are positional and independent of the tree, so collect them up
    // front rather than trying to thread them through the event stream.
    for err in lexed.errors() {
        errors.push(SyntaxError { message: err.msg.clone(), range: lexed.range(err.token) });
    }

    kg_parser::build_tree(lexed, events, &mut |step| match step {
        Step::Enter { kind } => {
            depth += 1;
            builder.start_node(rowan::SyntaxKind(kind.into()));
        }
        Step::Token { kind, text } => builder.token(rowan::SyntaxKind(kind.into()), text),
        Step::Exit => {
            depth -= 1;
            builder.finish_node();
        }
        Step::Error { msg, offset } => {
            errors.push(SyntaxError {
                message: msg.to_owned(),
                // A parse error is reported *at* a position, not over a span;
                // an empty range there is what LSP and the caret renderer both
                // want.
                range: TextRange::empty(offset),
            });
        }
    });
    debug_assert_eq!(depth, 0, "unbalanced Enter/Exit from the parser");

    let green = builder.finish();
    if options.parse_docs {
        let root = SyntaxNode::new_root(green.clone());
        errors.extend(crate::doc_comments::errors(&root));
    }

    errors.sort_by_key(|e| e.range.start());
    Parse { green, errors, features: options.features }
}

/// Parses a file and asserts the round-trip property.
///
/// The losslessness invariant — `parse(t).text() == t` for every input — is the
/// one property the whole CST design rests on, and it is cheap enough to assert
/// on every parse. Exported so fuzz targets and downstream tests can hold the
/// parser to it without reimplementing the check.
pub fn parse_and_check_lossless(text: &str) -> Parse {
    let parse = parse_file(text);
    assert_eq!(parse.text(), text, "the tree lost or invented text");
    parse
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_parses() {
        let parse = parse_file("");
        assert_eq!(parse.text(), "");
        assert_eq!(parse.syntax_node().kind(), crate::SyntaxKind::SOURCE_FILE);
    }

    #[test]
    fn whitespace_and_comments_survive() {
        let text = "// a comment\n\n/* nested /* inner */ outer */\n";
        let parse = parse_file(text);
        assert_eq!(parse.text(), text);
    }

    #[test]
    fn garbage_still_produces_a_tree() {
        for text in ["}", ">>>>", "\"unterminated", "/* unterminated", "{{{{{{"] {
            let parse = parse_file(text);
            assert_eq!(parse.text(), text, "round trip failed for {text:?}");
        }
    }
}
