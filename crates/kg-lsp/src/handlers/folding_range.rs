//! `textDocument/foldingRange`.
//!
//! Three sources, all decidable from tokens alone so that folding keeps working
//! while the grammar is unfinished:
//!
//! * **braced bodies** — matched `{`/`}` pairs, which covers class and struct
//!   member lists, function and lambda bodies, `switch` bodies, initializer
//!   lists and module export lists without enumerating any of them;
//! * **comment blocks** — a multi-line `/* */`, or a run of `//` lines; and
//! * **the import block** — a run of `import` declarations, which is the one
//!   fold users actually want collapsed by default.
//!
//! Braces fold from just after the `{` to just before the `}`, so the closing
//! brace stays visible; comments and imports fold whole lines, which is what
//! the `Comment` and `Imports` kinds mean.

use kg_base::LineIndex;
use kg_syntax::{SyntaxKind, SyntaxNode};
use lsp_types::{FoldingRange, FoldingRangeKind};

use crate::convert::{self, PositionEncoding};

use SyntaxKind::*;

pub fn folding_ranges(
    root: &SyntaxNode,
    index: &LineIndex,
    enc: PositionEncoding,
) -> Vec<FoldingRange> {
    let mut out = Vec::new();
    let mut braces: Vec<lsp_types::Position> = Vec::new();
    let mut comment_run: Option<(u32, u32)> = None;
    let mut import_run: Option<(u32, u32)> = None;

    for element in root.descendants_with_tokens() {
        let Some(token) = element.into_token() else { continue };
        let start = convert::position(index, enc, token.text_range().start());
        let end = convert::position(index, enc, token.text_range().end());

        // A run of single-line comments is broken by any non-trivia token, and
        // (in the `LINE_COMMENT` arm below) by a blank line between two of them.
        if !token.kind().is_trivia() {
            flush(&mut comment_run, FoldingRangeKind::Comment, &mut out);
        }

        match token.kind() {
            L_CURLY => braces.push(end),
            R_CURLY => {
                if let Some(open) = braces.pop()
                    && start.line > open.line
                {
                    out.push(FoldingRange {
                        start_line: open.line,
                        start_character: Some(open.character),
                        end_line: start.line,
                        end_character: Some(start.character),
                        kind: None,
                        collapsed_text: None,
                    });
                }
            }
            LINE_COMMENT | DOC_LINE_PRE | DOC_LINE_POST => {
                comment_run = match comment_run {
                    Some((first, last)) if start.line == last + 1 => Some((first, start.line)),
                    Some(run) => {
                        emit(run, FoldingRangeKind::Comment, &mut out);
                        Some((start.line, start.line))
                    }
                    None => Some((start.line, start.line)),
                };
            }
            BLOCK_COMMENT | DOC_BLOCK_PRE | DOC_BLOCK_POST => {
                flush(&mut comment_run, FoldingRangeKind::Comment, &mut out);
                emit((start.line, end.line), FoldingRangeKind::Comment, &mut out);
            }
            IMPORT_KW => {
                import_run = match import_run {
                    Some((first, last)) if start.line <= last + 1 => Some((first, start.line)),
                    Some(run) => {
                        emit(run, FoldingRangeKind::Imports, &mut out);
                        Some((start.line, start.line))
                    }
                    None => Some((start.line, start.line)),
                };
            }
            _ => {}
        }
    }

    flush(&mut comment_run, FoldingRangeKind::Comment, &mut out);
    flush(&mut import_run, FoldingRangeKind::Imports, &mut out);

    // Clients want them in source order; the brace stack emits inner folds
    // first, so a final sort is cheaper than threading order through.
    out.sort_by_key(|f| (f.start_line, f.end_line));
    out
}

fn flush(run: &mut Option<(u32, u32)>, kind: FoldingRangeKind, out: &mut Vec<FoldingRange>) {
    if let Some(run) = run.take() {
        emit(run, kind, out);
    }
}

fn emit((first, last): (u32, u32), kind: FoldingRangeKind, out: &mut Vec<FoldingRange>) {
    if last > first {
        out.push(FoldingRange {
            start_line: first,
            start_character: None,
            end_line: last,
            end_character: None,
            kind: Some(kind),
            collapsed_text: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_syntax::parse_file;

    fn folds(text: &str) -> Vec<FoldingRange> {
        let parse = parse_file(text);
        folding_ranges(&parse.syntax_node(), &LineIndex::new(text), PositionEncoding::Utf16)
    }

    fn of_kind(text: &str, kind: Option<FoldingRangeKind>) -> Vec<(u32, u32)> {
        folds(text)
            .into_iter()
            .filter(|f| f.kind == kind)
            .map(|f| (f.start_line, f.end_line))
            .collect()
    }

    #[test]
    fn a_braced_body_folds_between_the_braces() {
        let src = "class A {\n    uint32 x;\n}\n";
        let blocks = of_kind(src, None);
        assert_eq!(blocks, vec![(0, 2)]);

        let f = &folds(src)[0];
        // Just after the `{` on line 0 ...
        assert_eq!(f.start_character, Some(9));
        // ... to just before the `}` on line 2, which stays visible.
        assert_eq!(f.end_character, Some(0));
    }

    #[test]
    fn a_one_line_body_does_not_fold() {
        assert!(of_kind("class A { uint32 x; }\n", None).is_empty());
    }

    #[test]
    fn nested_bodies_each_fold() {
        let src = "class A {\n    void f() {\n        return;\n    }\n}\n";
        assert_eq!(of_kind(src, None), vec![(0, 4), (1, 3)]);
    }

    #[test]
    fn a_run_of_line_comments_folds_as_a_comment() {
        let src = "// one\n// two\n// three\nclass A {}\n";
        assert_eq!(of_kind(src, Some(FoldingRangeKind::Comment)), vec![(0, 2)]);
    }

    #[test]
    fn a_blank_line_breaks_a_comment_run() {
        let src = "// one\n// two\n\n// four\n// five\n";
        assert_eq!(of_kind(src, Some(FoldingRangeKind::Comment)), vec![(0, 1), (3, 4)]);
    }

    #[test]
    fn a_single_comment_line_does_not_fold() {
        assert!(of_kind("// alone\nclass A {}\n", Some(FoldingRangeKind::Comment)).is_empty());
    }

    #[test]
    fn a_multiline_block_comment_folds() {
        let src = "/* one\n   two */\nclass A {}\n";
        assert_eq!(of_kind(src, Some(FoldingRangeKind::Comment)), vec![(0, 1)]);
    }

    #[test]
    fn the_import_block_folds() {
        let src = "import a.b\nimport c.d\nimport e.f\n\nclass A {}\n";
        assert_eq!(of_kind(src, Some(FoldingRangeKind::Imports)), vec![(0, 2)]);
    }

    #[test]
    fn a_lone_import_does_not_fold() {
        assert!(of_kind("import a.b\nclass A {}\n", Some(FoldingRangeKind::Imports)).is_empty());
    }

    #[test]
    fn unbalanced_braces_do_not_panic() {
        for src in ["}\n}\n", "{\n{\n", "{ } }\n{"] {
            let _ = folds(src);
        }
    }
}
