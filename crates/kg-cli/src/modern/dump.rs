//! `kg dump ast|cst|tokens|ir` — the debugging window into the frontend.
//!
//! These are the fastest way to answer "what did the parser actually do with
//! this file" — and, for `ir`, "what did the lowering make of it". Everything
//! else in `kg` is either a thin wrapper over them or not written yet.

use kg_parser::LexedStr;
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, SyntaxToken};

use super::style::Styles;

/// Which dump to print.
#[derive(Copy, Clone, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum What {
    /// Nodes only, with the identifier each one declares: the shape of the
    /// program without the punctuation.
    Ast,
    /// Every node and every token, including whitespace and comments — the
    /// lossless tree, exactly as the formatter and the language server see it.
    Cst,
    /// The lexer's output, before any tree is built.
    Tokens,
    /// The lowered IR (`kg-hir`), one node per line with its provenance —
    /// range, originating kind, and whether the range is the node's own text.
    Ir,
    /// The template instances the program-wide loop made of the program
    /// rooted at the file (`decidedInstances` round by round,
    /// `Template.hs:56-108`) — each as a message spells it, with the round
    /// that decided it and its first site. Reads the manifest like `kg
    /// check`, so imported templates count; `-` is stdin's text as a
    /// program alone, no imports.
    Instances,
}

/// How long a token's text may get before it is elided in a dump.
const MAX_TOKEN_TEXT: usize = 48;

/// The concrete syntax tree, indented, with kinds and ranges.
///
/// ```text
/// SOURCE_FILE@0..34
///   FUNCTION_ITEM@0..33
///     INLINE_KW@0..6 "inline"
///     WHITESPACE@6..7 " "
/// ```
pub fn cst(text: &str, styles: &Styles, trivia: bool, options: kg_syntax::ParseOptions) -> String {
    let parse = kg_syntax::parse_file_with(text, options);
    let mut out = String::new();
    write_node(&parse.syntax_node(), 0, &mut out, styles, trivia, false);
    out.push_str(&errors_section(&parse, styles));
    out
}

/// Nodes only, each labelled with the name it declares, where it declares one.
pub fn ast(text: &str, styles: &Styles, options: kg_syntax::ParseOptions) -> String {
    let parse = kg_syntax::parse_file_with(text, options);
    let mut out = String::new();
    write_node(&parse.syntax_node(), 0, &mut out, styles, false, true);
    out.push_str(&errors_section(&parse, styles));
    out
}

/// The lowered IR, in `kg-hir`'s annotated structural rendering.
///
/// Unstyled: the rendering is the same string `cargo xtask ir` compares IRs
/// by (plus provenance), and colour codes inside it would make "pipe two
/// dumps into `diff`" — its main use — lie about differences.
pub fn ir(text: &str, options: kg_syntax::ParseOptions) -> String {
    let parse = kg_syntax::parse_file_with(text, options);
    let interner = kg_base::Interner::new();
    let file = kg_syntax::ast::SourceFile::cast(parse.syntax_node()).expect("root is SOURCE_FILE");
    let lowered = kg_hir::lower(&file, kg_base::FileId(0), &interner);
    let mut out = lowered.annotated_string(&interner);
    let n = lowered.unsupported().len();
    if n > 0 {
        out.push_str(&format!("\n{n} unsupported node(s)\n"));
    }
    out
}

/// The template instances the program-wide loop made of the program rooted
/// at `root` in `db` (`kg_db::instances`). See [`render_instances`] for the
/// shape.
pub fn instances_rooted(db: &kg_db::Db, root: kg_base::FileId) -> String {
    render_instances(db, &kg_db::instances::instances(db, &[root]))
}

/// stdin's text as a program alone — no imports, no base library — the
/// way `kg_db::check_source` checks one: a template an import declares is
/// not there, so nothing instantiates it.
pub fn instances_standalone(text: &str, language: kg_base::language::Language) -> String {
    let mut settings = kg_config::Settings::fallback("/kg-dump-instances");
    settings.implicit_base = false;
    settings.language = settings.language.with_project(language);
    let mut db = kg_db::Db::new(settings);
    let root = db.set_file_text(kg_base::Utf8Path::new("/kg-dump-instances/stdin.k"), text.to_owned());
    instances_rooted(&db, root)
}

/// ```text
/// 2 template(s), 3 distinct instance(s), max round 2, 2 round(s)
///
/// a.Vec
///   Vec<uint8, 4>  round 1  main.k:52..64
/// ```
///
/// Each instance as a message spells it, the round that decided it, and
/// the first site that decided it.
fn render_instances(db: &kg_db::Db, set: &kg_db::instances::Instances) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if set.erred {
        out.push_str(
            "the program has errors, and a program carrying an error instantiates \
             nothing (Template.hs:55,84)\n",
        );
        return out;
    }
    let _ = writeln!(
        out,
        "{} template(s), {} distinct instance(s), max round {}, {} round(s)",
        set.templates.len(),
        set.total(),
        set.max_round(),
        set.rounds,
    );
    for (template, instances) in &set.templates {
        let _ = writeln!(out, "\n{template}");
        for i in instances {
            let path = db.path(i.site.file).map(|p| p.to_string()).unwrap_or_else(|| format!("file#{}", i.site.file.0));
            let _ = writeln!(
                out,
                "  {}  round {}  {path}:{}..{}",
                i.spelled,
                i.round,
                u32::from(i.site.range.start()),
                u32::from(i.site.range.end())
            );
        }
    }
    out
}

/// The token stream, one per line, with lexer errors listed at the end.
pub fn tokens(text: &str, styles: &Styles) -> String {
    let lexed = LexedStr::new(text);
    let mut out = String::new();
    let width = lexed.len().to_string().len();
    for i in 0..lexed.len() {
        let range = lexed.range(i);
        let line = format!(
            "{i:>width$}  {}@{}..{} {}",
            styles.kind(&format!("{:?}", lexed.kind(i))),
            u32::from(range.start()),
            u32::from(range.end()),
            styles.literal(&elide(lexed.text(i)))
        );
        out.push_str(&line);
        out.push('\n');
    }

    if !lexed.errors().is_empty() {
        out.push('\n');
        out.push_str(&format!("{} lexer error(s):\n", lexed.errors().len()));
        for error in lexed.errors() {
            let range = lexed.range(error.token);
            out.push_str(&format!(
                "  {} at {}..{}: {}\n",
                styles.error("error"),
                u32::from(range.start()),
                u32::from(range.end()),
                error.msg
            ));
        }
    }
    out
}

fn write_node(
    node: &SyntaxNode,
    depth: usize,
    out: &mut String,
    styles: &Styles,
    trivia: bool,
    nodes_only: bool,
) {
    let range = node.text_range();
    let indent = "  ".repeat(depth);
    let mut line = format!(
        "{indent}{}@{}..{}",
        styles.kind(&format!("{:?}", node.kind())),
        u32::from(range.start()),
        u32::from(range.end())
    );
    if nodes_only && let Some(name) = declared_name(node) {
        line.push_str(&format!(" {}", styles.literal(&format!("{name:?}"))));
    }
    out.push_str(&line);
    out.push('\n');

    for child in node.children_with_tokens() {
        if let Some(child_node) = child.as_node() {
            write_node(child_node, depth + 1, out, styles, trivia, nodes_only);
        } else if let Some(token) = child.as_token() {
            if nodes_only || (!trivia && token.kind().is_trivia()) {
                continue;
            }
            write_token(token, depth + 1, out, styles);
        }
    }
}

fn write_token(token: &SyntaxToken, depth: usize, out: &mut String, styles: &Styles) {
    let range = token.text_range();
    out.push_str(&format!(
        "{}{}@{}..{} {}\n",
        "  ".repeat(depth),
        styles.kind(&format!("{:?}", token.kind())),
        u32::from(range.start()),
        u32::from(range.end()),
        styles.literal(&elide(token.text()))
    ));
}

/// The name a node *declares*, which is exactly the identifier inside its
/// `NAME` child (`kg-parser/src/grammar/paths.rs:25-31`).
///
/// Deliberately not "the first `IDENT` anywhere underneath": that labels
/// `MODULE_PATH@330..339` for `mini-base` with `"mini"` and a static assert with
/// `"assert"`, which is worse than no label at all.
fn declared_name(node: &SyntaxNode) -> Option<String> {
    node.children()
        .find(|child| child.kind() == SyntaxKind::NAME)
        .and_then(|name| {
            name.children_with_tokens()
                .filter_map(|child| child.into_token())
                .find(|token| token.kind() == SyntaxKind::IDENT)
                .map(|token| token.text().to_owned())
        })
}

fn errors_section(parse: &kg_syntax::Parse, styles: &Styles) -> String {
    if parse.errors().is_empty() {
        return String::new();
    }
    let mut out = format!("\n{} syntax error(s):\n", parse.errors().len());
    for error in parse.errors() {
        out.push_str(&format!(
            "  {} at {}..{}: {}\n",
            styles.error("error"),
            u32::from(error.range.start()),
            u32::from(error.range.end()),
            error.message
        ));
    }
    out
}

/// Debug-quoted, so newlines and tabs stay on one line, and truncated so one
/// enormous comment cannot swamp the tree.
fn elide(text: &str) -> String {
    if text.chars().count() <= MAX_TOKEN_TEXT {
        return format!("{text:?}");
    }
    let head: String = text.chars().take(MAX_TOKEN_TEXT).collect();
    format!("{head:?}… ({} bytes)", text.len())
}

/// The sorted IR of one module of a loaded program — `kg dump ir FILE`.
///
/// The unsorted, single-file rendering (`ir`) stays for stdin, where the
/// imports are unknowable and no sort is safe (divergence #22). Here the
/// program is loaded, so every path carries the shape the reference's parser
/// would have built for it; whatever the sorter *declined* is listed with its
/// reason, because that count is the honest measure of the sort.
pub fn ir_sorted(
    program: &kg_db::sorted::SortedProgram,
    module: kg_db::ModuleSource,
    interner: &kg_base::Interner,
    rewrite: bool,
) -> String {
    let Some(ir) = program.ir(module) else {
        return "module not in program\n".to_owned();
    };
    // `--rewrite` runs the rewrites before the program-wide loop, in the
    // reference's execution order, into one fork of the IR
    // (`kg_hir::rewrite::pipeline`): what one module looks like going into
    // pass 8, without the loop's rounds.
    let rewritten;
    let ir = if rewrite {
        rewritten = kg_hir::rewrite::pipeline(ir, interner);
        &rewritten
    } else {
        ir
    };
    let mut out = ir.annotated_string(interner);
    let n = ir.unsupported().len();
    if n > 0 {
        out.push_str(&format!("\n{n} unsupported node(s)\n"));
    }
    if !ir.sort.unsorted.is_empty() {
        out.push_str(&format!("\n{} unsorted path(s):\n", ir.sort.unsorted.len()));
        for (range, reason) in &ir.sort.unsorted {
            out.push_str(&format!(
                "  {}..{} {reason:?}\n",
                u32::from(range.range.start()),
                u32::from(range.range.end()),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "// a comment\ninline void main()\n{\n}\n";

    #[test]
    fn the_cst_dump_is_an_indented_tree_with_kinds_and_ranges() {
        let dump = cst(SOURCE, &Styles::plain(), true, Default::default());
        let first = dump.lines().next().unwrap();
        assert_eq!(first, format!("SOURCE_FILE@0..{}", SOURCE.len()));
        // Trivia is present, quoted, and indented under its parent.
        assert!(dump.contains("LINE_COMMENT@0..12 \"// a comment\""), "{dump}");
        assert!(dump.lines().any(|l| l.starts_with("  ")), "{dump}");
        // Every line is `<indent><KIND>@<start>..<end>[ "text"]`.
        for line in dump.lines().filter(|l| !l.is_empty()) {
            let trimmed = line.trim_start();
            assert!(trimmed.contains('@'), "unexpected line: {line}");
        }
    }

    #[test]
    fn trivia_can_be_hidden() {
        let with = cst(SOURCE, &Styles::plain(), true, Default::default());
        let without = cst(SOURCE, &Styles::plain(), false, Default::default());
        assert!(with.contains("WHITESPACE@"));
        assert!(!without.contains("WHITESPACE@"));
        assert!(!without.contains("LINE_COMMENT@"));
        assert!(without.lines().count() < with.lines().count());
    }

    #[test]
    fn the_ast_dump_has_no_tokens_and_names_what_it_can() {
        let dump = ast(SOURCE, &Styles::plain(), Default::default());
        assert!(!dump.contains("WHITESPACE@"));
        assert!(!dump.contains("INLINE_KW@"));
        assert!(dump.contains("\"main\""), "{dump}");
    }

    #[test]
    fn the_token_dump_covers_the_whole_file_in_order() {
        let dump = tokens(SOURCE, &Styles::plain());
        let lines: Vec<&str> = dump.lines().collect();
        assert!(lines[0].contains("LINE_COMMENT@0..12"), "{}", lines[0]);
        // The stream is lossless: the last token ends at the file's length.
        assert!(dump.contains(&format!("..{}", SOURCE.len())), "{dump}");
    }

    #[test]
    fn syntax_errors_are_listed_after_the_tree() {
        let dump = cst("inline void f( {\n", &Styles::plain(), true, Default::default());
        assert!(dump.contains("syntax error(s):"), "{dump}");
        assert!(dump.contains("error at "), "{dump}");
    }

    #[test]
    fn a_giant_token_is_elided() {
        let long = format!("// {}\n", "x".repeat(200));
        let dump = cst(&long, &Styles::plain(), true, Default::default());
        assert!(dump.contains("… ("), "{dump}");
        for line in dump.lines() {
            assert!(line.chars().count() < 120, "{line}");
        }
    }

    #[test]
    fn an_empty_file_still_prints_a_root() {
        assert_eq!(cst("", &Styles::plain(), true, Default::default()), "SOURCE_FILE@0..0\n");
    }
}
