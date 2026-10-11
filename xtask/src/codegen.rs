//! `cargo xtask codegen` — the generated files of the syntax layer and of
//! the ParseTree ABI.
//!
//! Every output is derived, never edited:
//!
//! * `crates/kg-parser/src/syntax_kind_table.rs` — every [`SyntaxKind`] variant
//!   in discriminant order, so a `u16` can be turned back into a kind without
//!   `unsafe`. Derived by scanning the enum body of `syntax_kind.rs`.
//! * `crates/kg-syntax/src/ast/generated.rs` — one typed wrapper per rule of
//!   `grammar/kanagawa.ungram`, with an `AstNode` impl and typed accessors.
//! * `editors/vscode/syntaxes/kanagawa.tmLanguage.json` — the editor's
//!   fallback highlighter ([`emit_tmlanguage`]).
//! * `crates/kg-backend-sys/src/bindings.rs` and
//!   `crates/kg-ptbridge/src/backend.rs` — the ABI's bindings and the
//!   `Backend` trait, from the pinned reference's headers through
//!   `kg_abi_gen` ([`abi`]). Checked in, where build scripts once wrote them,
//!   so that building `kg` needs neither the reference nor libclang.
//!
//! The pass is idempotent: files are read back and only rewritten when the
//! bytes actually change, so `cargo xtask codegen` twice in a row is a no-op.
//!
//! [`SyntaxKind`]: https://docs.rs/
//!
//! # Why the accessors look the way they do
//!
//! An ungrammar rule is a tree of sequences, alternations, repetitions and
//! options. Lowering it to accessors means flattening that tree into a list of
//! *fields*, each of which is a node type or a set of token kinds plus a
//! cardinality. Two decisions carry the weight:
//!
//! * **Sequence and alternation combine differently.** Two occurrences of a
//!   name in sequence are two children (`'[' '['`, or the `X (',' X)*` list
//!   idiom), so the field becomes unbounded; two in an alternation are one
//!   child reached two ways, so the field stays singular and only widens the
//!   kinds it accepts. See [`fields_of`].
//! * **A field steps over earlier fields whose children it would also claim.**
//!   Accessors find children by casting, which normally filters precisely — but
//!   `Expr` subsumes `Type`, so `VarDecl`'s naive `expr()` would return the
//!   declared type. See [`collision_skip`]. Codegen refuses to emit a rule
//!   where the position could not be determined at all.

use std::{collections::HashMap, fmt::Write as _, fs, path::Path, process::Command};

use anyhow::{Context, Result, bail};
use ungrammar::{Grammar, Rule};

// ---------------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------------

/// Regenerates every derived file; with `check`, fails on the first that
/// differs instead. The ABI's are read from `kanagawa_root`'s headers.
pub fn run(check: bool, kanagawa_root: &Path) -> Result<()> {
    let root = project_root()?;

    let kinds = SyntaxKinds::parse(&root)?;

    let table_path = root.join("crates/kg-parser/src/syntax_kind_table.rs");
    let table = emit_kind_table(&kinds);
    write_if_changed(&table_path, &table, check)?;

    let grammar_path = root.join("grammar/kanagawa.ungram");
    let grammar_src = fs::read_to_string(&grammar_path)
        .with_context(|| format!("reading {}", grammar_path.display()))?;
    let grammar: Grammar = grammar_src
        .parse()
        .map_err(|e| anyhow::anyhow!("{}: {e}", grammar_path.display()))?;

    let ast = emit_ast(&grammar, &kinds)?;
    let ast = rustfmt(&ast);
    let ast_path = root.join("crates/kg-syntax/src/ast/generated.rs");
    write_if_changed(&ast_path, &ast, check)?;

    let tm_path = root.join("editors/vscode/syntaxes/kanagawa.tmLanguage.json");
    let tm = emit_tmlanguage(&kinds)?;
    write_if_changed(&tm_path, &tm, check)?;

    abi(&root, kanagawa_root, check)
}

/// The ABI's two generated files, from the reference's headers.
///
/// Only from the *pinned* reference: the files name the commit they came
/// from, and bindings generated from a reference the gates do not grade
/// against would describe another backend. So the pin is checked as every
/// gate checks it, and the headers — outside `library/` and `test/`, which
/// the pin hashes — must be as that commit has them.
///
/// Without the headers, `--check` fails: a check that passes with its
/// input absent is indistinguishable from one that passed (sprint W0.4).
/// Writing skips them, saying so, so that the syntax files can still be
/// regenerated in a checkout without the reference.
fn abi(root: &Path, reference: &Path, check: bool) -> Result<()> {
    let bindings_path = root.join("crates/kg-backend-sys/src/bindings.rs");
    let backend_path = root.join("crates/kg-ptbridge/src/backend.rs");
    let cpp = reference.join(kg_abi_gen::CPP);
    if let Some(absent) = kg_abi_gen::HEADERS.iter().map(|h| cpp.join(h)).find(|p| !p.is_file()) {
        let why = format!(
            "they are generated from the reference's headers, and {} is not there (--kanagawa-root)",
            absent.display()
        );
        if check {
            bail!("cannot check {} and {}: {why}", rel(&bindings_path), rel(&backend_path));
        }
        println!("skipped    {} and {}: {why}", rel(&bindings_path), rel(&backend_path));
        return Ok(());
    }
    crate::guard::check_pin(reference, &crate::fresh::workspace().join(crate::guard::LOCK))?;
    let headers: Vec<String> = kg_abi_gen::HEADERS.iter().map(|h| format!("{}/{h}", kg_abi_gen::CPP)).collect();
    let mut args = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    args.extend(headers.iter().map(String::as_str));
    if let Some(dirty) = crate::guard::git(reference, &args)
        && !dirty.is_empty()
    {
        bail!("the reference's ABI headers are not as its pinned commit has them:\n{dirty}");
    }
    let commit = crate::guard::git(reference, &["rev-parse", "HEAD"]).unwrap_or_else(|| "none".to_owned());

    let bindings = kg_abi_gen::bindings(&cpp, &commit).map_err(anyhow::Error::msg)?;
    write_if_changed(&bindings_path, &bindings, check)?;
    let backend = kg_abi_gen::backend(&cpp, &commit).map_err(anyhow::Error::msg)?;
    write_if_changed(&backend_path, &backend, check)
}

/// The editor's *fallback* highlighter.
///
/// The server's semantic tokens are the real answer; this grammar only has to
/// be right for the seconds before the server attaches, and for anyone who has
/// the extension but not the binary. It is generated rather than written
/// because the reference's hand-written one drifted from the language in three
/// ways that are all visible in a normal file:
///
/// * it lists `continue` as a keyword — Kanagawa has no such word, which is
///   pinned by a test in `kg-parser`'s `syntax_kind.rs`;
/// * it scopes `float` as a builtin type — the type is spelled `float32`, so
///   `float` is an ordinary identifier and colouring it lies about the program;
/// * it scopes every `<` and `>` as a comparison operator, so the angle
///   brackets of `reverse<uint<Width>>(x)` come out as arithmetic.
///
/// Generating the keyword list from `SyntaxKind` makes the first impossible.
/// The second is fixed by listing the type keywords explicitly. The third is
/// fixed by *not* scoping bare `<`/`>` at all: an unscoped token inherits the
/// surrounding colour, which is a far smaller lie than "operator", and the
/// server overrides it the moment it attaches.
fn emit_tmlanguage(kinds: &SyntaxKinds) -> Result<String> {
    let keywords: Vec<String> = kinds
        .all
        .iter()
        .filter(|k| k.ends_with("_KW"))
        .map(|k| k.trim_end_matches("_KW").to_lowercase())
        .collect();

    if keywords.len() != 53 {
        bail!(
            "expected the 53 reserved words of `Parser/Lexer.hs:164-219`, found {}",
            keywords.len()
        );
    }

    // Scope by role, so a theme can tell a control-flow word from a type.
    const CONTROL: &[&str] = &[
        "break", "case", "default", "do", "else", "for", "if", "return", "switch", "while",
        "unrolled_for",
    ];
    const TYPES: &[&str] =
        &["auto", "bool", "float32", "int", "string", "typename", "uint", "void"];
    const LITERALS: &[&str] = &["false", "true"];
    const MODIFIERS: &[&str] = &[
        "const", "export", "extern", "inline", "noinline", "private", "public", "static",
    ];

    let group = |names: &[&str]| -> String {
        let mut v: Vec<&str> =
            keywords.iter().map(String::as_str).filter(|k| names.contains(k)).collect();
        v.sort_unstable();
        v.join("|")
    };
    let rest: String = {
        let mut v: Vec<&str> = keywords
            .iter()
            .map(String::as_str)
            .filter(|k| {
                !CONTROL.contains(k)
                    && !TYPES.contains(k)
                    && !LITERALS.contains(k)
                    && !MODIFIERS.contains(k)
            })
            .collect();
        // Longest first, so `static_assert` is not eaten by `static`.
        v.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        v.join("|")
    };

    let json = serde_json::json!({
        "$comment": "GENERATED by `cargo xtask codegen` from SyntaxKind. Do not edit.",
        "name": "Kanagawa",
        "scopeName": "source.kanagawa",
        "fileTypes": ["k"],
        "patterns": [
            {"include": "#comments"},
            {"include": "#strings"},
            {"include": "#numbers"},
            {"include": "#keywords"},
            {"include": "#types"},
            {"include": "#attributes"},
            {"include": "#operators"},
        ],
        "repository": {
            "comments": {"patterns": [
                // Doc comments first: `//|` documents what follows, `//<` what precedes.
                {"name": "comment.line.documentation.kanagawa",
                 "match": "//[|<].*$"},
                {"name": "comment.line.double-slash.kanagawa",
                 "match": "//.*$"},
                {"name": "comment.block.documentation.kanagawa",
                 "begin": "/\\*[|<]", "end": "\\*/"},
                // Block comments nest in this language (`Parser/Lexer.hs:88`),
                // which TextMate cannot express exactly; self-inclusion gets
                // one level of nesting right, which covers the corpus.
                {"name": "comment.block.kanagawa",
                 "begin": "/\\*", "end": "\\*/",
                 "patterns": [{"include": "#comments"}]},
            ]},
            "strings": {"patterns": [{
                "name": "string.quoted.double.kanagawa",
                "begin": "\"", "end": "\"",
                "patterns": [
                    {"name": "constant.character.escape.kanagawa",
                     "match": "\\\\(?:[0-9]+|x[0-9A-Fa-f]+|o[0-7]+|\\^[A-Z@\\[\\]\\\\^_]|[A-Za-z&\"'\\\\])"},
                    // `{{` escapes a brace; `}}` does NOT (unlike C#).
                    {"name": "constant.character.escape.kanagawa", "match": "\\{\\{"},
                    {"name": "meta.embedded.kanagawa",
                     "begin": "\\{", "end": "\\}",
                     "patterns": [{"include": "$self"}]},
                ],
            }]},
            "numbers": {"patterns": [
                {"name": "constant.numeric.float.kanagawa",
                 "match": "\\b[0-9][0-9_]*(?:\\.[0-9][0-9_]*(?:[eE][-+]?[0-9]+)?|[eE][-+]?[0-9]+)"},
                {"name": "constant.numeric.integer.kanagawa",
                 "match": "\\b(?:0[xX][0-9A-Fa-f_]+|0[oO][0-7_]+|0[bB][01_]+|[0-9][0-9_]*)(?:[iu][0-9]+)?"},
            ]},
            "keywords": {"patterns": [
                {"name": "keyword.control.kanagawa", "match": format!("\\b(?:{})\\b", group(CONTROL))},
                {"name": "constant.language.kanagawa", "match": format!("\\b(?:{})\\b", group(LITERALS))},
                {"name": "storage.modifier.kanagawa", "match": format!("\\b(?:{})\\b", group(MODIFIERS))},
                {"name": "keyword.other.kanagawa", "match": format!("\\b(?:{rest})\\b")},
            ]},
            "types": {"patterns": [
                // `int8`/`uint32` are single tokens (`Parser/Lexer.hs:241-243`);
                // `int<N>`/`uint<N>` are the parameterised forms.
                {"name": "storage.type.kanagawa", "match": "\\b(?:u?int)[0-9]+\\b"},
                {"name": "storage.type.kanagawa", "match": format!("\\b(?:{})\\b", group(TYPES))},
            ]},
            "attributes": {"patterns": [{
                "name": "meta.attribute.kanagawa",
                "begin": "\\[\\[", "end": "\\]\\]",
                "patterns": [
                    {"name": "entity.name.function.attribute.kanagawa", "match": "[A-Za-z_][A-Za-z0-9_]*"},
                    {"include": "#numbers"},
                    {"include": "#strings"},
                ],
            }]},
            // Deliberately absent: bare `<` and `>`. See this function's docs.
            "operators": {"patterns": [{
                "name": "keyword.operator.kanagawa",
                "match": "(?:\\|\\|=|&&=|\\^\\^=|<<|>>|<=|>=|==|!=|&&|\\|\\||\\^\\^|::|->|\\+\\+|--|[-+*/%&|^~!?:]=?|=)",
            }]},
        },
    });

    let mut out = serde_json::to_string_pretty(&json)?;
    out.push('\n');
    Ok(out)
}

fn project_root() -> Result<std::path::PathBuf> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = Path::new(manifest)
        .parent()
        .context("xtask has no parent directory")?
        .to_path_buf();
    Ok(root)
}

fn write_if_changed(path: &Path, contents: &str, check: bool) -> Result<()> {
    let current = fs::read_to_string(path).ok();
    if current.as_deref() == Some(contents) {
        println!("unchanged  {}", rel(path));
        return Ok(());
    }
    if check {
        bail!(
            "{} is out of date; run `cargo xtask codegen`",
            rel(path)
        );
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    println!("wrote      {}", rel(path));
    Ok(())
}

fn rel(path: &Path) -> String {
    let root = project_root().ok();
    match root.and_then(|r| path.strip_prefix(r).ok().map(Path::to_path_buf)) {
        Some(p) => p.display().to_string(),
        None => path.display().to_string(),
    }
}

// ---------------------------------------------------------------------------
// the SyntaxKind vocabulary
// ---------------------------------------------------------------------------

/// The variants of `SyntaxKind`, in declaration (= discriminant) order.
struct SyntaxKinds {
    all: Vec<String>,
    set: std::collections::HashSet<String>,
}

impl SyntaxKinds {
    /// Scans the body of `pub enum SyntaxKind { ... }`.
    ///
    /// A hand-rolled scan rather than a Rust parser: the enum is a flat list of
    /// bare `SCREAMING_CASE,` variants interleaved with comments and one
    /// `#[allow]`, and depending on `syn` here would be the only heavy
    /// dependency in the whole workspace.
    fn parse(root: &Path) -> Result<SyntaxKinds> {
        let path = root.join("crates/kg-parser/src/syntax_kind.rs");
        let src =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;

        let start = src
            .find("pub enum SyntaxKind {")
            .context("`pub enum SyntaxKind {` not found in syntax_kind.rs")?;
        let body = &src[start..];
        let end = body
            .find("\n}")
            .context("unterminated `enum SyntaxKind` body")?;
        let body = &body[..end];

        let mut all = Vec::new();
        for line in body.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
                continue;
            }
            let Some(name) = line.strip_suffix(',') else {
                bail!("unexpected line in the `SyntaxKind` enum body: {line:?}");
            };
            if name == "__LAST" {
                continue;
            }
            if !name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                bail!("unexpected variant in the `SyntaxKind` enum body: {name:?}");
            }
            all.push(name.to_string());
        }
        if all.is_empty() {
            bail!("no variants found in the `SyntaxKind` enum body");
        }

        let set = all.iter().cloned().collect();
        Ok(SyntaxKinds { all, set })
    }

    fn contains(&self, kind: &str) -> bool {
        self.set.contains(kind)
    }
}

fn emit_kind_table(kinds: &SyntaxKinds) -> String {
    let mut out = String::new();
    out.push_str("// @generated by `cargo xtask codegen` from the `SyntaxKind` enum. Do not edit.\n");
    out.push('\n');
    out.push_str("/// Every kind in discriminant order, so a `u16` can be turned back into a\n");
    out.push_str("/// `SyntaxKind` without `unsafe`.\n");
    let _ = writeln!(
        out,
        "const KIND_TABLE: [SyntaxKind; {}] = [",
        kinds.all.len()
    );
    for kind in &kinds.all {
        let _ = writeln!(out, "    {kind},");
    }
    out.push_str("];\n");
    out
}

// ---------------------------------------------------------------------------
// tokens
// ---------------------------------------------------------------------------

/// The quoted-token spelling used in the ungrammar, and what it means.
struct TokenInfo {
    /// The `SyntaxKind` variant the token lexes as.
    kind: &'static str,
    /// The stem of the generated accessor, e.g. `l_paren` -> `l_paren_token()`.
    method: &'static str,
}

/// The complete `'quoted'` -> `SyntaxKind` map.
///
/// Punctuation and keywords use their source spelling; token *classes* whose
/// spelling varies (identifiers, numbers, string pieces) use a descriptive
/// name. Two entries deliberately share a kind:
///
/// * `'assert'` is `IDENT` — `assert` is not reserved, so `static assert(...)`
///   sees a contextual keyword (`Lexer.hs:255-256`).
/// * `'string'` is the `string` *type keyword*; the string *literal* token is
///   spelled `'string_literal'`, so that all 52 live keywords keep their source
///   spelling uniformly.
fn token_table() -> HashMap<&'static str, TokenInfo> {
    const ENTRIES: &[(&str, &str, &str)] = &[
        // ------------------------------------------------------- punctuation
        ("(", "L_PAREN", "l_paren"),
        (")", "R_PAREN", "r_paren"),
        ("{", "L_CURLY", "l_curly"),
        ("}", "R_CURLY", "r_curly"),
        ("[", "L_BRACK", "l_brack"),
        ("]", "R_BRACK", "r_brack"),
        ("<", "L_ANGLE", "l_angle"),
        (">", "R_ANGLE", "r_angle"),
        (";", "SEMICOLON", "semicolon"),
        (",", "COMMA", "comma"),
        (":", "COLON", "colon"),
        ("::", "COLON2", "colon2"),
        (".", "DOT", "dot"),
        ("=", "EQ", "eq"),
        ("==", "EQ2", "eq2"),
        ("!", "BANG", "bang"),
        ("!=", "NEQ", "neq"),
        ("+", "PLUS", "plus"),
        ("-", "MINUS", "minus"),
        ("*", "STAR", "star"),
        ("/", "SLASH", "slash"),
        ("%", "PERCENT", "percent"),
        ("&", "AMP", "amp"),
        ("&&", "AMP2", "amp2"),
        ("|", "PIPE", "pipe"),
        ("||", "PIPE2", "pipe2"),
        ("^", "CARET", "caret"),
        ("^^", "CARET2", "caret2"),
        ("~", "TILDE", "tilde"),
        ("?", "QUESTION", "question"),
        ("->", "THIN_ARROW", "thin_arrow"),
        ("\\", "BACKSLASH", "backslash"),
        ("<=", "LTEQ", "lteq"),
        (">=", "GTEQ", "gteq"),
        ("<<", "SHL", "shl"),
        (">>", "SHR", "shr"),
        ("++", "PLUS2", "plus2"),
        ("--", "MINUS2", "minus2"),
        ("+=", "PLUSEQ", "pluseq"),
        ("-=", "MINUSEQ", "minuseq"),
        ("*=", "STAREQ", "stareq"),
        ("/=", "SLASHEQ", "slasheq"),
        ("%=", "PERCENTEQ", "percenteq"),
        ("&=", "AMPEQ", "ampeq"),
        ("|=", "PIPEEQ", "pipeeq"),
        ("^=", "CARETEQ", "careteq"),
        ("&&=", "AMP2EQ", "amp2eq"),
        ("||=", "PIPE2EQ", "pipe2eq"),
        ("^^=", "CARET2EQ", "caret2eq"),
        // ---------------------------------------------------------- keywords
        ("as", "AS_KW", "as_kw"),
        ("atomic", "ATOMIC_KW", "atomic_kw"),
        ("auto", "AUTO_KW", "auto_kw"),
        ("barrier", "BARRIER_KW", "barrier_kw"),
        ("bitsizeof", "BITSIZEOF_KW", "bitsizeof_kw"),
        ("bitoffsetof", "BITOFFSETOF_KW", "bitoffsetof_kw"),
        ("bool", "BOOL_KW", "bool_kw"),
        ("break", "BREAK_KW", "break_kw"),
        ("bytesizeof", "BYTESIZEOF_KW", "bytesizeof_kw"),
        ("byteoffsetof", "BYTEOFFSETOF_KW", "byteoffsetof_kw"),
        ("cast", "CAST_KW", "cast_kw"),
        ("case", "CASE_KW", "case_kw"),
        ("class", "CLASS_KW", "class_kw"),
        ("clog2", "CLOG2_KW", "clog2_kw"),
        ("concat", "CONCAT_KW", "concat_kw"),
        ("const", "CONST_KW", "const_kw"),
        ("decltype", "DECLTYPE_KW", "decltype_kw"),
        ("default", "DEFAULT_KW", "default_kw"),
        ("do", "DO_KW", "do_kw"),
        ("else", "ELSE_KW", "else_kw"),
        ("enum", "ENUM_KW", "enum_kw"),
        ("export", "EXPORT_KW", "export_kw"),
        ("extern", "EXTERN_KW", "extern_kw"),
        ("false", "FALSE_KW", "false_kw"),
        ("fan_out", "FAN_OUT_KW", "fan_out_kw"),
        ("float32", "FLOAT32_KW", "float32_kw"),
        ("for", "FOR_KW", "for_kw"),
        ("if", "IF_KW", "if_kw"),
        ("import", "IMPORT_KW", "import_kw"),
        ("inline", "INLINE_KW", "inline_kw"),
        ("int", "INT_KW", "int_kw"),
        ("lutmul", "LUTMUL_KW", "lutmul_kw"),
        ("module", "MODULE_KW", "module_kw"),
        ("mux", "MUX_KW", "mux_kw"),
        ("noinline", "NOINLINE_KW", "noinline_kw"),
        ("private", "PRIVATE_KW", "private_kw"),
        ("public", "PUBLIC_KW", "public_kw"),
        ("reorder", "REORDER_KW", "reorder_kw"),
        ("return", "RETURN_KW", "return_kw"),
        ("static", "STATIC_KW", "static_kw"),
        ("static_assert", "STATIC_ASSERT_KW", "static_assert_kw"),
        ("string", "STRING_KW", "string_kw"),
        ("struct", "STRUCT_KW", "struct_kw"),
        ("switch", "SWITCH_KW", "switch_kw"),
        ("template", "TEMPLATE_KW", "template_kw"),
        ("true", "TRUE_KW", "true_kw"),
        ("typename", "TYPENAME_KW", "typename_kw"),
        ("uint", "UINT_KW", "uint_kw"),
        ("union", "UNION_KW", "union_kw"),
        ("using", "USING_KW", "using_kw"),
        ("void", "VOID_KW", "void_kw"),
        ("while", "WHILE_KW", "while_kw"),
        // -------------------------------------------- contextual and classes
        ("assert", "IDENT", "assert"),
        ("ident", "IDENT", "ident"),
        ("int_type", "INT_TYPE_TOKEN", "int_type"),
        ("uint_type", "UINT_TYPE_TOKEN", "uint_type"),
        ("int_number", "INT_NUMBER", "int_number"),
        ("float_number", "FLOAT_NUMBER", "float_number"),
        ("string_literal", "STRING", "string_literal"),
        ("string_text", "STRING_TEXT", "string_text"),
        ("string_end", "STRING_END", "string_end"),
    ];

    ENTRIES
        .iter()
        .map(|&(spelling, kind, method)| (spelling, TokenInfo { kind, method }))
        .collect()
}

// ---------------------------------------------------------------------------
// lowering an ungrammar rule to a field list
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Cardinality {
    Optional,
    Many,
}

#[derive(Debug)]
enum FieldKind {
    /// One or more token kinds; more than one means the rule wrote `('+'|'-')`.
    Token(Vec<String>),
    /// A child node of the named type.
    Node(String),
}

#[derive(Debug)]
struct Field {
    /// The accessor name, before the `_token` suffix that token fields get.
    name: String,
    kind: FieldKind,
    cardinality: Cardinality,
}

/// A rule that is a bare alternation of node references becomes a Rust enum;
/// everything else becomes a struct.
enum Shape {
    Enum(Vec<String>),
    Struct(Vec<Field>),
}

fn lower(grammar: &Grammar, rule: &Rule) -> Result<Shape> {
    if let Rule::Alt(alts) = rule
        && let Some(variants) = alt_of_nodes(grammar, alts)
    {
        return Ok(Shape::Enum(variants));
    }
    Ok(Shape::Struct(fields_of(grammar, rule, None, false)?))
}

/// `Some(variants)` when every alternative is a plain node reference.
fn alt_of_nodes(grammar: &Grammar, alts: &[Rule]) -> Option<Vec<String>> {
    let mut variants = Vec::new();
    for alt in alts {
        match alt {
            Rule::Node(node) => variants.push(grammar[*node].name.clone()),
            _ => return None,
        }
    }
    Some(variants)
}

fn alt_of_tokens(grammar: &Grammar, alts: &[Rule]) -> Option<Vec<String>> {
    let mut spellings = Vec::new();
    for alt in alts {
        match alt {
            Rule::Token(token) => spellings.push(grammar[*token].name.clone()),
            _ => return None,
        }
    }
    Some(spellings)
}

/// Flattens a rule into the accessor list it should generate.
///
/// The two combining directions are deliberately different, and getting them
/// backwards is the classic ungrammar-codegen bug:
///
/// * **In sequence**, two occurrences of the same name are two children that
///   both exist — `'[' '['` is the `[[` of an attribute group, `X (',' X)*` is
///   a list — so the field becomes unbounded.
/// * **In an alternation**, they are the same child reached two ways — the
///   three shapes of `AssignStmt` all bind `lhs` — so the field stays singular
///   and merely widens the set of kinds it accepts.
fn fields_of(
    grammar: &Grammar,
    rule: &Rule,
    label: Option<&str>,
    in_rep: bool,
) -> Result<Vec<Field>> {
    let card = if in_rep {
        Cardinality::Many
    } else {
        Cardinality::Optional
    };

    let fields = match rule {
        Rule::Labeled { label, rule } => fields_of(grammar, rule, Some(label), in_rep)?,
        Rule::Node(node) => {
            let node = grammar[*node].name.clone();
            vec![Field {
                name: label.map(str::to_string).unwrap_or_else(|| to_snake(&node)),
                kind: FieldKind::Node(node),
                cardinality: card,
            }]
        }
        Rule::Token(token) => {
            let spelling = grammar[*token].name.clone();
            vec![Field {
                name: label.map(str::to_string).unwrap_or_else(|| spelling.clone()),
                kind: FieldKind::Token(vec![spelling]),
                cardinality: card,
            }]
        }
        Rule::Opt(rule) => fields_of(grammar, rule, label, in_rep)?,
        Rule::Rep(rule) => fields_of(grammar, rule, label, true)?,
        Rule::Seq(rules) => {
            let mut acc: Vec<Field> = Vec::new();
            for rule in rules {
                for field in fields_of(grammar, rule, None, in_rep)? {
                    absorb(&mut acc, field, Combine::Sequence);
                }
            }
            acc
        }
        Rule::Alt(alts) => {
            // A labelled alternation of tokens — `op:('+' | '-')` — is one
            // field that accepts any of them.
            if let Some(label) = label
                && let Some(spellings) = alt_of_tokens(grammar, alts)
            {
                vec![Field {
                    name: label.to_string(),
                    kind: FieldKind::Token(spellings),
                    cardinality: card,
                }]
            } else {
                let mut acc: Vec<Field> = Vec::new();
                for alt in alts {
                    for field in fields_of(grammar, alt, label, in_rep)? {
                        absorb(&mut acc, field, Combine::Alternative);
                    }
                }
                acc
            }
        }
    };
    Ok(fields)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Combine {
    /// Both occurrences produce a child; the field becomes unbounded.
    Sequence,
    /// At most one occurrence produces a child; the field only widens.
    Alternative,
}

fn absorb(acc: &mut Vec<Field>, field: Field, how: Combine) {
    if let Some(existing) = acc.iter_mut().find(|f| f.name == field.name) {
        existing.cardinality = match how {
            Combine::Sequence => Cardinality::Many,
            Combine::Alternative => existing.cardinality.max(field.cardinality),
        };
        if let (FieldKind::Token(have), FieldKind::Token(more)) = (&mut existing.kind, &field.kind)
        {
            for spelling in more {
                if !have.contains(spelling) {
                    have.push(spelling.clone());
                }
            }
        }
        return;
    }
    acc.push(field);
}

// ---------------------------------------------------------------------------
// emitting Rust
// ---------------------------------------------------------------------------

const PREAMBLE: &str = r#"//! Typed AST accessors, generated by `cargo xtask codegen` from
//! `grammar/kanagawa.ungram`. Do not edit; edit the grammar instead.
//!
//! Every rule of the grammar becomes either a `struct` wrapping a `SyntaxNode`
//! of one kind, or an `enum` over several. Accessors are total in the sense
//! that they never panic: a malformed or partially recovered tree simply yields
//! `None` or an empty iterator, which is what lets the same types serve the
//! parser's error-recovery output, the formatter and the language server.

#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::needless_lifetimes)]

use crate::{AstNode, SyntaxKind, SyntaxNode, SyntaxToken};

/// Child lookups shared by every accessor below.
///
/// Which of these the grammar happens to need varies with the grammar, so the
/// set is emitted whole rather than pruned per run.
#[allow(dead_code)]
mod support {
    use crate::{AstNode, SyntaxKind, SyntaxNode, SyntaxToken};

    /// The first child node castable to `N`.
    pub(super) fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
        parent.children().find_map(N::cast)
    }

    /// The `n`-th child node castable to `N`, for rules that mention the same
    /// node type in several distinct positions (`lhs:Expr ... rhs:Expr`).
    pub(super) fn nth_child<N: AstNode>(parent: &SyntaxNode, n: usize) -> Option<N> {
        parent.children().filter_map(N::cast).nth(n)
    }

    /// Every child node castable to `N`, in source order.
    pub(super) fn children<N: AstNode>(parent: &SyntaxNode) -> impl Iterator<Item = N> + use<N> {
        parent.children().filter_map(N::cast)
    }

    /// The first direct child token whose kind is in `kinds`.
    pub(super) fn token(parent: &SyntaxNode, kinds: &[SyntaxKind]) -> Option<SyntaxToken> {
        parent
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|it| kinds.contains(&it.kind()))
    }

    /// The `n`-th direct child token whose kind is in `kinds`.
    pub(super) fn nth_token(
        parent: &SyntaxNode,
        kinds: &[SyntaxKind],
        n: usize,
    ) -> Option<SyntaxToken> {
        parent
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .filter(|it| kinds.contains(&it.kind()))
            .nth(n)
    }

    /// Every direct child token whose kind is in `kinds`, in source order.
    pub(super) fn tokens<'a>(
        parent: &SyntaxNode,
        kinds: &'a [SyntaxKind],
    ) -> impl Iterator<Item = SyntaxToken> + use<'a> {
        parent
            .children_with_tokens()
            .filter_map(|it| it.into_token())
            .filter(move |it| kinds.contains(&it.kind()))
    }
}
"#;

fn emit_ast(grammar: &Grammar, kinds: &SyntaxKinds) -> Result<String> {
    let tokens = token_table();

    // Validate the whole grammar before emitting a byte, so a mistake in the
    // ungrammar shows up as one clear message rather than a compile error in
    // several hundred lines of generated code.
    // Only struct-shaped rules become real nodes in the tree; a rule that is a
    // bare alternation (`Expr`, `Stmt`, `Type`, `Item`) is a Rust enum over
    // other kinds and never appears as a node kind of its own.
    let mut missing_nodes = Vec::new();
    for node in grammar.iter() {
        if matches!(lower(grammar, &grammar[node].rule)?, Shape::Enum(_)) {
            continue;
        }
        let kind = to_screaming(&grammar[node].name);
        if !kinds.contains(&kind) {
            missing_nodes.push(format!("{} (needs SyntaxKind::{kind})", grammar[node].name));
        }
    }
    let mut missing_tokens = Vec::new();
    for token in grammar.tokens() {
        let spelling = &grammar[token].name;
        match tokens.get(spelling.as_str()) {
            None => missing_tokens.push(format!("'{spelling}' has no entry in `token_table()`")),
            Some(info) if !kinds.contains(info.kind) => {
                missing_tokens.push(format!("'{spelling}' maps to unknown kind {}", info.kind));
            }
            Some(_) => {}
        }
    }
    if !missing_nodes.is_empty() || !missing_tokens.is_empty() {
        bail!(
            "grammar refers to vocabulary `SyntaxKind` does not have:\n  {}",
            missing_nodes
                .into_iter()
                .chain(missing_tokens)
                .collect::<Vec<_>>()
                .join("\n  ")
        );
    }

    let kind_sets = kind_sets(grammar)?;

    let mut out = String::from(PREAMBLE);

    let mut enums = Vec::new();
    for node in grammar.iter() {
        let name = grammar[node].name.clone();
        match lower(grammar, &grammar[node].rule)? {
            Shape::Enum(variants) => enums.push((name, variants)),
            Shape::Struct(fields) => {
                emit_struct(&mut out, &name, &fields, &tokens, &kind_sets)?;
            }
        }
    }

    // Enums come second so that the file reads leaves-first, and because an
    // enum's `can_cast` may delegate to another enum defined above or below it.
    let enum_names: std::collections::HashSet<&str> =
        enums.iter().map(|(name, _)| name.as_str()).collect();
    for (name, variants) in &enums {
        emit_enum(&mut out, name, variants, &enum_names);
    }

    Ok(out)
}

/// For every rule, the set of concrete `SyntaxKind`s a value of that type can
/// wrap. A struct wraps exactly one; an enum wraps the union of its variants',
/// transitively — which is how `Expr` comes to subsume every `Type` kind.
fn kind_sets(grammar: &Grammar) -> Result<HashMap<String, Vec<String>>> {
    fn visit(
        grammar: &Grammar,
        node: ungrammar::Node,
        out: &mut HashMap<String, Vec<String>>,
        stack: &mut Vec<String>,
    ) -> Result<Vec<String>> {
        let name = grammar[node].name.clone();
        if let Some(known) = out.get(&name) {
            return Ok(known.clone());
        }
        if stack.contains(&name) {
            bail!(
                "cyclic alternation through `{name}`: {} -> {name}",
                stack.join(" -> ")
            );
        }
        stack.push(name.clone());

        let kinds = match lower(grammar, &grammar[node].rule)? {
            Shape::Struct(_) => vec![to_screaming(&name)],
            Shape::Enum(variants) => {
                let mut kinds = Vec::new();
                for variant in variants {
                    let child = grammar
                        .iter()
                        .find(|n| grammar[*n].name == variant)
                        .with_context(|| format!("`{name}` names unknown node `{variant}`"))?;
                    for kind in visit(grammar, child, out, stack)? {
                        if !kinds.contains(&kind) {
                            kinds.push(kind);
                        }
                    }
                }
                kinds
            }
        };

        stack.pop();
        out.insert(name, kinds.clone());
        Ok(kinds)
    }

    let mut out = HashMap::new();
    for node in grammar.iter() {
        visit(grammar, node, &mut out, &mut Vec::new())?;
    }
    Ok(out)
}

/// How many earlier children an accessor has to step over.
///
/// An accessor finds its child by casting, so it silently walks past every
/// child whose kind it rejects. That breaks down when an *earlier* field of the
/// same rule is guaranteed to produce a child this accessor would accept — the
/// motivating case being `VarDecl`'s `Type` then `Expr`, since a type is a
/// legal expression term, so "first child castable to `Expr`" would hand back
/// the declared type. Such a field is stepped over explicitly.
///
/// The test is *subset*, not mere overlap: `SwitchCase`'s trailing `BreakStmt`
/// follows a `Stmt*` whose kinds include `BREAK_STMT`, but a `Stmt` child is
/// not necessarily a `BreakStmt`, so casting stays exact and nothing is
/// skipped. An unbounded earlier field that *is* a subset would make the index
/// unknowable, and codegen refuses to emit it.
///
/// This is sound because the ungrammar's field order is source order. It is
/// approximate for a partially recovered tree, where an absent earlier child
/// shifts the count by one; that is the price of not storing child slots.
fn collision_skip(
    rule: &str,
    fields: &[Field],
    at: usize,
    accepts: &dyn Fn(&Field) -> Vec<String>,
) -> Result<usize> {
    let mine = accepts(&fields[at]);
    if mine.is_empty() {
        return Ok(0);
    }
    let mut skip = 0;
    for earlier in &fields[..at] {
        let theirs = accepts(earlier);
        if theirs.is_empty() || !theirs.iter().all(|k| mine.contains(k)) {
            continue;
        }
        if earlier.cardinality == Cardinality::Many {
            bail!(
                "rule `{rule}`: field `{}` follows the unbounded field `{}`, every child \
                 of which it would also accept, so its position is not determined",
                fields[at].name,
                earlier.name
            );
        }
        skip += 1;
    }
    Ok(skip)
}

fn emit_struct(
    out: &mut String,
    name: &str,
    fields: &[Field],
    tokens: &HashMap<&'static str, TokenInfo>,
    kind_sets: &HashMap<String, Vec<String>>,
) -> Result<()> {
    let kind = to_screaming(name);

    // What each field accepts, as concrete kinds: node fields resolve through
    // `kind_sets`, token fields map their spellings.
    let node_accepts = |field: &Field| -> Vec<String> {
        match &field.kind {
            FieldKind::Node(node) => kind_sets.get(node).cloned().unwrap_or_default(),
            FieldKind::Token(_) => Vec::new(),
        }
    };
    let token_accepts = |field: &Field| -> Vec<String> {
        match &field.kind {
            FieldKind::Node(_) => Vec::new(),
            FieldKind::Token(spellings) => spellings
                .iter()
                .filter_map(|s| tokens.get(s.as_str()).map(|info| info.kind.to_string()))
                .collect(),
        }
    };

    let _ = write!(
        out,
        "
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct {name} {{
    pub(crate) syntax: SyntaxNode,
}}

impl AstNode for {name} {{
    #[inline]
    fn can_cast(kind: SyntaxKind) -> bool {{
        kind == SyntaxKind::{kind}
    }}
    #[inline]
    fn cast(syntax: SyntaxNode) -> Option<Self> {{
        if Self::can_cast(syntax.kind()) {{ Some({name} {{ syntax }}) }} else {{ None }}
    }}
    #[inline]
    fn syntax(&self) -> &SyntaxNode {{
        &self.syntax
    }}
}}

impl {name} {{
"
    );

    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (at, field) in fields.iter().enumerate() {
        let method = accessor_name(field);
        if !used.insert(method.clone()) {
            bail!("rule `{name}` generates two accessors named `{method}()`");
        }
        match &field.kind {
            FieldKind::Node(node) => {
                let skip = collision_skip(name, fields, at, &node_accepts)?;
                match (field.cardinality, skip) {
                    (Cardinality::Many, 0) => {
                        let _ = write!(
                            out,
                            "    #[inline]
    pub fn {method}(&self) -> impl Iterator<Item = {node}> + use<> {{
        support::children(&self.syntax)
    }}
"
                        );
                    }
                    (Cardinality::Many, n) => {
                        let _ = write!(
                            out,
                            "    #[inline]
    pub fn {method}(&self) -> impl Iterator<Item = {node}> + use<> {{
        support::children(&self.syntax).skip({n})
    }}
"
                        );
                    }
                    (Cardinality::Optional, 0) => {
                        let _ = write!(
                            out,
                            "    #[inline]
    pub fn {method}(&self) -> Option<{node}> {{
        support::child(&self.syntax)
    }}
"
                        );
                    }
                    (Cardinality::Optional, n) => {
                        let _ = write!(
                            out,
                            "    #[inline]
    pub fn {method}(&self) -> Option<{node}> {{
        support::nth_child(&self.syntax, {n})
    }}
"
                        );
                    }
                }
            }
            FieldKind::Token(spellings) => {
                let skip = collision_skip(name, fields, at, &token_accepts)?;
                let kinds = spellings
                    .iter()
                    .map(|s| {
                        let info = tokens
                            .get(s.as_str())
                            .unwrap_or_else(|| unreachable!("validated above"));
                        format!("SyntaxKind::{}", info.kind)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                match (field.cardinality, skip) {
                    (Cardinality::Many, 0) => {
                        let _ = write!(
                            out,
                            "    #[inline]
    pub fn {method}(&self) -> impl Iterator<Item = SyntaxToken> + use<> {{
        support::tokens(&self.syntax, &[{kinds}])
    }}
"
                        );
                    }
                    (Cardinality::Many, n) => {
                        let _ = write!(
                            out,
                            "    #[inline]
    pub fn {method}(&self) -> impl Iterator<Item = SyntaxToken> + use<> {{
        support::tokens(&self.syntax, &[{kinds}]).skip({n})
    }}
"
                        );
                    }
                    (Cardinality::Optional, 0) => {
                        let _ = write!(
                            out,
                            "    #[inline]
    pub fn {method}(&self) -> Option<SyntaxToken> {{
        support::token(&self.syntax, &[{kinds}])
    }}
"
                        );
                    }
                    (Cardinality::Optional, n) => {
                        let _ = write!(
                            out,
                            "    #[inline]
    pub fn {method}(&self) -> Option<SyntaxToken> {{
        support::nth_token(&self.syntax, &[{kinds}], {n})
    }}
"
                        );
                    }
                }
            }
        }
    }

    out.push_str("}\n");
    Ok(())
}

fn emit_enum(
    out: &mut String,
    name: &str,
    variants: &[String],
    enum_names: &std::collections::HashSet<&str>,
) {
    let _ = write!(
        out,
        "
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum {name} {{
"
    );
    for variant in variants {
        let _ = writeln!(out, "    {variant}({variant}),");
    }
    out.push_str("}\n\n");

    // `can_cast` splits into a plain kind test for struct variants and a
    // delegation for variants that are themselves enums (`Expr` subsumes
    // `Type`, because a type is a legal term).
    let leaves: Vec<&String> = variants
        .iter()
        .filter(|v| !enum_names.contains(v.as_str()))
        .collect();
    let nested: Vec<&String> = variants
        .iter()
        .filter(|v| enum_names.contains(v.as_str()))
        .collect();

    let _ = write!(out, "impl AstNode for {name} {{\n    fn can_cast(kind: SyntaxKind) -> bool {{\n");
    if leaves.is_empty() {
        out.push_str("        false\n");
    } else {
        out.push_str("        matches!(\n            kind,\n            ");
        let kinds = leaves
            .iter()
            .map(|v| format!("SyntaxKind::{}", to_screaming(v)))
            .collect::<Vec<_>>()
            .join("\n                | ");
        out.push_str(&kinds);
        out.push_str("\n        )\n");
    }
    for nested_name in &nested {
        let _ = writeln!(out, "            || {nested_name}::can_cast(kind)");
    }
    out.push_str("    }\n\n    fn cast(syntax: SyntaxNode) -> Option<Self> {\n");
    if leaves.is_empty() && nested.is_empty() {
        out.push_str("        let _ = syntax;\n        None\n");
    } else {
        out.push_str("        let res = match syntax.kind() {\n");
        for variant in &leaves {
            let kind = to_screaming(variant);
            let _ = writeln!(
                out,
                "            SyntaxKind::{kind} => {name}::{variant}({variant} {{ syntax }}),"
            );
        }
        if nested.is_empty() {
            out.push_str("            _ => return None,\n        };\n        Some(res)\n");
        } else {
            out.push_str("            _ => {\n");
            for nested_name in &nested {
                let _ = write!(
                    out,
                    "                if let Some(it) = {nested_name}::cast(syntax.clone()) {{
                    return Some({name}::{nested_name}(it));
                }}\n"
                );
            }
            out.push_str("                return None;\n            }\n        };\n        Some(res)\n");
        }
    }
    out.push_str("    }\n\n    fn syntax(&self) -> &SyntaxNode {\n        match self {\n");
    for variant in variants {
        let _ = writeln!(out, "            {name}::{variant}(it) => it.syntax(),");
    }
    out.push_str("        }\n    }\n}\n");

    for variant in variants {
        let _ = write!(
            out,
            "
impl From<{variant}> for {name} {{
    #[inline]
    fn from(node: {variant}) -> {name} {{
        {name}::{variant}(node)
    }}
}}
"
        );
    }
}

// ---------------------------------------------------------------------------
// naming
// ---------------------------------------------------------------------------

/// The accessor name for a field: node fields keep their name, token fields
/// gain a `_token`/`_tokens` suffix so `Name = 'ident'` reads `ident_token()`.
fn accessor_name(field: &Field) -> String {
    match &field.kind {
        FieldKind::Node(_) => {
            let base = escape(&field.name);
            match field.cardinality {
                Cardinality::Many => pluralize(&base),
                Cardinality::Optional => base,
            }
        }
        FieldKind::Token(spellings) => {
            // An unlabelled token field is named after its spelling; a labelled
            // one (`op:('+' | '-')`) keeps the label.
            let base = if spellings.len() == 1 && field.name == spellings[0] {
                token_method_stem(&spellings[0])
            } else {
                escape(&field.name)
            };
            match field.cardinality {
                Cardinality::Many => format!("{base}_tokens"),
                Cardinality::Optional => format!("{base}_token"),
            }
        }
    }
}

fn token_method_stem(spelling: &str) -> String {
    token_table()
        .get(spelling)
        .map(|info| info.method.to_string())
        .unwrap_or_else(|| escape(spelling))
}

/// Rust keywords and near-misses that cannot be method names.
fn escape(name: &str) -> String {
    match name {
        "type" => "ty".to_string(),
        "if" | "else" | "for" | "while" | "loop" | "match" | "fn" | "let" | "const" | "static"
        | "struct" | "enum" | "impl" | "trait" | "mod" | "use" | "as" | "in" | "ref" | "move"
        | "box" | "true" | "false" | "self" | "super" | "crate" | "where" | "return" | "break"
        | "continue" | "do" | "union" | "default" => format!("{name}_"),
        _ => name.to_string(),
    }
}

fn pluralize(name: &str) -> String {
    if name.ends_with('s') || name.ends_with('x') || name.ends_with("ch") || name.ends_with("sh") {
        format!("{name}es")
    } else {
        format!("{name}s")
    }
}

/// `SourceFile` -> `source_file`, `LutMulExpr` -> `lut_mul_expr`.
fn to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.char_indices() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// `SourceFile` -> `SOURCE_FILE`. The inverse of how `SyntaxKind` is spelled.
fn to_screaming(name: &str) -> String {
    to_snake(name).to_ascii_uppercase()
}

// ---------------------------------------------------------------------------
// formatting
// ---------------------------------------------------------------------------

/// Runs `rustfmt` over the generated text, or returns it unchanged if `rustfmt`
/// is unavailable or unhappy. Generated code that cannot be formatted is still
/// better than no generated code.
fn rustfmt(src: &str) -> String {
    use std::io::Write as _;
    use std::process::Stdio;

    let child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let Ok(mut child) = child else {
        eprintln!("note: rustfmt not found; emitting unformatted");
        return src.to_string();
    };

    if let Some(mut stdin) = child.stdin.take()
        && stdin.write_all(src.as_bytes()).is_err()
    {
        return src.to_string();
    }

    match child.wait_with_output() {
        Ok(out) if out.status.success() => {
            String::from_utf8(out.stdout).unwrap_or_else(|_| src.to_string())
        }
        Ok(out) => {
            eprintln!(
                "note: rustfmt failed, emitting unformatted:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            src.to_string()
        }
        Err(_) => src.to_string(),
    }
}
