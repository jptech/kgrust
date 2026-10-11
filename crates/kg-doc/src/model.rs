//! What a module's page holds, read off its syntax tree and the
//! documentation attached to it (`kg_syntax::file_docs`).
//!
//! The structure is the reference generator's (`third_party/kanagawa/doc/sandcastle.md`,
//! "Documentation structure"): the module's title and description; an index
//! of what it exposes, in the export list's order, under the `//| # Heading`
//! sections written there; then each exposed declaration in source order. A
//! file without a module header exposes every top-level declaration. Class
//! members are shown where public, and where private only if documented or a
//! callback — sandcastle's rule.

use camino::Utf8PathBuf;
use kg_base::{FileId, TextRange, TextSize};
use kg_db::db::Db;
use kg_syntax::{FileDocs, NodeOrToken, SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken};

/// One module's page.
#[derive(Debug, Clone)]
pub struct Module {
    /// `data.array`, or the file's stem where it declares no module.
    pub name: String,
    pub file: FileId,
    pub path: Utf8PathBuf,
    /// The module's own documentation, Markdown.
    pub doc: Option<String>,
    /// The export list, in its order.
    pub index: Vec<Entry>,
    /// The exposed declarations, in source order.
    pub decls: Vec<Decl>,
}

/// One line of a module's index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A section heading written in the export list, `//| # Heading`.
    Section(String),
    /// An exposed name.
    Name(String),
    /// A re-exposed module, `module a.b`.
    Module(String),
}

/// One declaration, or one member of one.
#[derive(Debug, Clone)]
pub struct Decl {
    pub name: String,
    pub kind: &'static str,
    /// Unique within its page.
    pub anchor: String,
    pub signature: Vec<Piece>,
    pub doc: Option<String>,
    /// Template and function parameters that carry documentation.
    pub params: Vec<Param>,
    /// A class's members, a struct's or union's fields, an enum's constants.
    pub members: Vec<Decl>,
    /// The name token, which is what a reference resolves to.
    pub name_range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub doc: String,
}

/// A piece of a signature: text, or a name to link if it resolves to a
/// documented declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Name { text: String, at: TextSize },
}

/// The page for `file`.
pub fn module(db: &Db, file: FileId, path: Utf8PathBuf) -> Module {
    let parse = db.parse(file);
    let root = parse.syntax_node();
    let docs = kg_syntax::file_docs(&root);
    let header = root.children().find(|n| n.kind() == MODULE_DECL);
    let name = header
        .as_ref()
        .and_then(|h| h.children().find(|n| n.kind() == MODULE_PATH))
        .map(|p| compact(&p))
        .unwrap_or_else(|| path.file_stem().unwrap_or("module").to_owned());
    let index = header
        .as_ref()
        .and_then(|h| h.children().find(|n| n.kind() == MODULE_EXPORT_LIST))
        .map(|list| index_of(&list, &docs))
        .unwrap_or_default();
    let exposed: Option<Vec<&str>> = (!index.is_empty()).then(|| {
        index
            .iter()
            .filter_map(|e| match e {
                Entry::Name(n) => Some(n.as_str()),
                _ => None,
            })
            .collect()
    });
    let mut anchors = Anchors::default();
    let decls = root
        .children()
        .filter_map(|node| declaration(&node, &docs, file, &mut anchors))
        .filter(|d| exposed.as_ref().is_none_or(|names| names.contains(&d.name.as_str())))
        .collect();
    Module { name, file, path, doc: docs.module().map(kg_syntax::Doc::text), index, decls }
}

/// The export list's entries: each comma-separated segment, preceded by its
/// section heading where a `//|` run documents it.
fn index_of(list: &SyntaxNode, docs: &FileDocs) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut segment: Vec<SyntaxToken> = Vec::new();
    let flush = |segment: &mut Vec<SyntaxToken>, out: &mut Vec<Entry>| {
        let (Some(first), Some(last)) = (segment.first(), segment.last()) else { return };
        let range = TextRange::new(first.text_range().start(), last.text_range().end());
        if let Some(doc) = docs.of(range).and_then(|d| d.before.clone()) {
            out.push(Entry::Section(doc));
        }
        let text: String = segment.iter().map(|t| t.text().to_owned()).collect::<Vec<_>>().join(" ");
        out.push(match text.strip_prefix("module ") {
            Some(module) => Entry::Module(module.split_whitespace().collect()),
            None => Entry::Name(text),
        });
        segment.clear();
    };
    for element in list.descendants_with_tokens() {
        let NodeOrToken::Token(token) = element else { continue };
        match token.kind() {
            k if k.is_trivia() => {}
            L_CURLY => {}
            COMMA | R_CURLY => flush(&mut segment, &mut out),
            _ => segment.push(token),
        }
    }
    flush(&mut segment, &mut out);
    out
}

#[derive(Default)]
struct Anchors(std::collections::HashMap<String, usize>);

impl Anchors {
    /// `name`, or `name-2`, `name-3`… where the page has it already: an
    /// overloaded name is one anchor per declaration.
    fn take(&mut self, name: &str) -> String {
        let n = self.0.entry(name.to_owned()).or_insert(0);
        *n += 1;
        if *n == 1 { name.to_owned() } else { format!("{name}-{n}") }
    }
}

/// The declaration `node` is, if it is one a page shows.
fn declaration(node: &SyntaxNode, docs: &FileDocs, file: FileId, anchors: &mut Anchors) -> Option<Decl> {
    // `template <…> decl`: the template is the documented element, the
    // declaration inside it the named one.
    let (outer, inner) = match node.kind() {
        TEMPLATE_DECL => (node.clone(), node.children().find(|n| kind_of(n.kind()).is_some())?),
        k if kind_of(k).is_some() => (node.clone(), node.clone()),
        _ => return None,
    };
    let kind = kind_of(inner.kind())?;
    let name_token = name_of(&inner)?;
    let name = name_token.text().to_owned();
    let name_range = name_token.text_range();
    let doc = docs.around(name_range).map(kg_syntax::Doc::text);
    let anchor = anchors.take(&name);

    let mut params = Vec::new();
    if let Some(list) = outer.children().find(|n| n.kind() == TEMPLATE_PARAM_LIST) {
        params.extend(documented_params(&list, &[TYPE_PARAM, VALUE_PARAM], docs));
    }
    if let Some(list) = inner.children().find(|n| n.kind() == PARAM_LIST) {
        params.extend(documented_params(&list, &[PARAM], docs));
    }

    let members = match inner.kind() {
        CLASS_DECL => class_members(&inner, docs, file, &anchor),
        STRUCT_DECL | UNION_DECL => fields(&inner, docs, file, &anchor),
        ENUM_DECL => constants(&inner, docs, &anchor),
        _ => Vec::new(),
    };
    Some(Decl { name, kind, anchor, signature: signature(&outer, file), doc, params, members, name_range })
}

fn kind_of(kind: SyntaxKind) -> Option<&'static str> {
    Some(match kind {
        FN_DECL => "function",
        CLASS_DECL => "class",
        STRUCT_DECL => "struct",
        UNION_DECL => "union",
        ENUM_DECL => "enum",
        ALIAS_DECL => "type",
        VAR_DECL => "variable",
        _ => return None,
    })
}

/// A declaration's own name: its `NAME` child's identifier.
fn name_of(node: &SyntaxNode) -> Option<SyntaxToken> {
    let name = node.children().find(|n| n.kind() == NAME)?;
    name.descendants_with_tokens().filter_map(NodeOrToken::into_token).find(|t| !t.kind().is_trivia())
}

fn documented_params(list: &SyntaxNode, kinds: &[SyntaxKind], docs: &FileDocs) -> Vec<Param> {
    list.children()
        .filter(|n| kinds.contains(&n.kind()))
        .filter_map(|param| {
            let name = name_of(&param)?;
            let doc = docs.around(name.text_range())?.text();
            Some(Param { name: name.text().to_owned(), doc })
        })
        .collect()
}

/// A class's members a reader of its interface wants: the public ones, and
/// private ones that are documented or callbacks (sandcastle's rule).
fn class_members(class: &SyntaxNode, docs: &FileDocs, file: FileId, parent: &str) -> Vec<Decl> {
    let Some(body) = class.children().find(|n| n.kind() == DECL_LIST) else { return Vec::new() };
    let mut public = false;
    let mut anchors = Anchors::default();
    let mut out = Vec::new();
    for member in body.children() {
        if member.kind() == VISIBILITY {
            public = member.text().to_string().trim_start().starts_with("public");
            continue;
        }
        let Some(mut decl) = declaration(&member, docs, file, &mut anchors) else { continue };
        let callback = member.descendants().any(|n| n.kind() == FN_TYPE) && decl.kind == "variable";
        if public || decl.doc.is_some() || callback {
            if callback {
                decl.kind = "callback";
            }
            decl.anchor = format!("{parent}.{}", decl.anchor);
            out.push(decl);
        }
    }
    out
}

fn fields(record: &SyntaxNode, docs: &FileDocs, file: FileId, parent: &str) -> Vec<Decl> {
    let Some(body) = record.children().find(|n| n.kind() == DECL_LIST) else { return Vec::new() };
    let mut anchors = Anchors::default();
    body.children()
        .filter(|n| n.kind() == VAR_DECL)
        .filter_map(|field| {
            let mut decl = declaration(&field, docs, file, &mut anchors)?;
            decl.kind = "field";
            decl.anchor = format!("{parent}.{}", decl.anchor);
            Some(decl)
        })
        .collect()
}

fn constants(enumeration: &SyntaxNode, docs: &FileDocs, parent: &str) -> Vec<Decl> {
    let Some(body) = enumeration.children().find(|n| n.kind() == DECL_LIST) else { return Vec::new() };
    body.children()
        .filter(|n| n.kind() == ENUM_VARIANT)
        .filter_map(|constant| {
            let name = name_of(&constant)?;
            Some(Decl {
                name: name.text().to_owned(),
                kind: "constant",
                anchor: format!("{parent}.{}", name.text()),
                signature: vec![Piece::Text(compact(&constant))],
                doc: docs.around(name.text_range()).map(kg_syntax::Doc::text),
                params: Vec::new(),
                members: Vec::new(),
                name_range: name.text_range(),
            })
        })
        .collect()
}

/// The declaration as written, without its body, its comments or its line
/// breaks: a function up to its body, a class, struct, union or enum up to
/// its members (listed apart), anything else whole. Names in it are pieces
/// of their own, for the page to link.
fn signature(node: &SyntaxNode, file: FileId) -> Vec<Piece> {
    let _ = file;
    let mut tokens: Vec<(SyntaxToken, bool)> = Vec::new();
    let mut space = false;
    let mut skip_until: Option<TextSize> = None;
    for element in node.descendants_with_tokens() {
        match element {
            NodeOrToken::Node(n) => {
                if skip_until.is_some_and(|end| n.text_range().start() < end) {
                    continue;
                }
                let body = match n.kind() {
                    BLOCK_STMT => n.parent().is_some_and(|p| p.kind() == FN_DECL),
                    DECL_LIST => n.parent().is_some_and(|p| matches!(p.kind(), CLASS_DECL | STRUCT_DECL | UNION_DECL | ENUM_DECL)),
                    _ => false,
                };
                if body {
                    skip_until = Some(n.text_range().end());
                }
            }
            NodeOrToken::Token(t) => {
                if skip_until.is_some_and(|end| t.text_range().start() < end) {
                    continue;
                }
                if t.kind().is_trivia() {
                    space = true;
                } else {
                    tokens.push((t, space));
                    space = false;
                }
            }
        }
    }
    // A trailing `;` says nothing a reader needs.
    while tokens.last().is_some_and(|(t, _)| t.kind() == SEMICOLON) {
        tokens.pop();
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut text = String::new();
    for (i, (token, spaced)) in tokens.iter().enumerate() {
        let previous = i.checked_sub(1).map(|j| &tokens[j].0);
        if i > 0 && *spaced && spaced_between(previous, token) {
            text.push(' ');
        }
        if token.kind() == IDENT && token.parent().is_some_and(|p| p.kind() == NAME_REF) {
            if !text.is_empty() {
                pieces.push(Piece::Text(std::mem::take(&mut text)));
            }
            pieces.push(Piece::Name { text: token.text().to_owned(), at: token.text_range().start() });
        } else {
            text.push_str(token.text());
        }
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    pieces
}

/// Whether the space written between two tokens is kept once comments and
/// line breaks are gone: not inside brackets' edges nor before a separator.
fn spaced_between(previous: Option<&SyntaxToken>, token: &SyntaxToken) -> bool {
    let bracket = |t: &SyntaxToken| {
        t.parent().is_some_and(|p| matches!(p.kind(), TEMPLATE_PARAM_LIST | TEMPLATE_ARG_LIST))
    };
    if matches!(token.kind(), COMMA | SEMICOLON | R_PAREN | R_BRACK | DOT | COLON2) {
        return false;
    }
    if token.kind() == R_ANGLE && bracket(token) {
        return false;
    }
    match previous {
        Some(p) if matches!(p.kind(), L_PAREN | L_BRACK | DOT | COLON2) => false,
        Some(p) if p.kind() == L_ANGLE && bracket(p) => false,
        _ => true,
    }
}

/// A node's tokens without trivia, spaced as [`signature`] spaces them.
fn compact(node: &SyntaxNode) -> String {
    signature(node, FileId::SYNTHETIC)
        .into_iter()
        .map(|p| match p {
            Piece::Text(t) | Piece::Name { text: t, .. } => t,
        })
        .collect()
}
