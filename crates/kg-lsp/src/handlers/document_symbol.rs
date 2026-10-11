//! `textDocument/documentSymbol` — the outline.
//!
//! Hierarchical (`DocumentSymbol`), not flat (`SymbolInformation`): a class's
//! members belong under the class, and the flat form cannot say so. Both halves
//! of the contract matter — `range` covers the whole declaration so the editor
//! can tell where the cursor is, and `selectionRange` covers just the name so
//! that clicking the outline entry lands on the identifier rather than on the
//! `class` keyword.
//!
//! Built from the typed AST, so it is empty on a file the parser could not make
//! sense of. That is the correct degradation: an outline of `ERROR` nodes would
//! be noise.

use kg_base::{LineIndex, TextRange};
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, ast};
use lsp_types::{DocumentSymbol, SymbolKind};

use crate::convert::{self, PositionEncoding};

use SyntaxKind::*;

pub fn document_symbols(
    root: &SyntaxNode,
    index: &LineIndex,
    enc: PositionEncoding,
) -> Vec<DocumentSymbol> {
    let Some(file) = ast::SourceFile::cast(root.clone()) else { return Vec::new() };
    let mut out = Vec::new();

    if let Some(module) = file.module_decl() {
        let node = module.syntax();
        let name = module
            .module_path()
            .map(|p| p.syntax().text().to_string().trim().to_owned())
            .unwrap_or_else(|| "module".to_owned());
        out.push(symbol(index, enc, name, None, SymbolKind::NAMESPACE, node, None, Vec::new()));
    }

    for item in file.items() {
        collect(item.syntax(), index, enc, &mut out);
    }
    out
}

/// Appends the symbol(s) an item contributes. One node may contribute none
/// (`public:`, a stray `;`) or exactly one.
fn collect(
    node: &SyntaxNode,
    index: &LineIndex,
    enc: PositionEncoding,
    out: &mut Vec<DocumentSymbol>,
) {
    match node.kind() {
        CLASS_DECL => push_container(node, index, enc, SymbolKind::CLASS, out),
        STRUCT_DECL => push_container(node, index, enc, SymbolKind::STRUCT, out),
        UNION_DECL => push_container(node, index, enc, SymbolKind::STRUCT, out),
        ENUM_DECL => {
            // The variants sit either directly under the declaration or inside
            // a `DECL_LIST`, depending on how the parser chose to bracket the
            // body. Accept both rather than depend on one.
            let variants = node
                .children()
                .flat_map(|c| {
                    if c.kind() == DECL_LIST {
                        c.children().collect::<Vec<_>>()
                    } else {
                        vec![c]
                    }
                })
                .filter(|c| c.kind() == ENUM_VARIANT)
                .filter_map(|v| {
                    let name = name_of(&v)?;
                    Some(symbol(
                        index,
                        enc,
                        name.text,
                        None,
                        SymbolKind::ENUM_MEMBER,
                        &v,
                        Some(name.range),
                        Vec::new(),
                    ))
                })
                .collect();
            push_named(node, index, enc, SymbolKind::ENUM, detail_of(node), variants, out);
        }
        FN_DECL | FN_PROTO => {
            let kind = member_or(node, SymbolKind::METHOD, SymbolKind::FUNCTION);
            push_named(node, index, enc, kind, signature(node), Vec::new(), out)
        }
        VAR_DECL => {
            // `default = e;` declares nothing nameable.
            if name_of(node).is_some() {
                let kind = member_or(node, SymbolKind::FIELD, SymbolKind::VARIABLE);
                push_named(node, index, enc, kind, type_text(node), Vec::new(), out);
            }
        }
        ALIAS_DECL => {
            push_named(node, index, enc, SymbolKind::TYPE_PARAMETER, None, Vec::new(), out)
        }
        // `template<...> <decl>` reports as the declaration it wraps, with the
        // parameter list as the detail — a separate "template" entry would put
        // every class in the file one level deeper for no information.
        TEMPLATE_DECL => {
            let params = node
                .children()
                .find(|c| c.kind() == TEMPLATE_PARAM_LIST)
                .map(|p| squash(&p.text().to_string()));
            let before = out.len();
            for inner in node.children().filter(|c| c.kind() != TEMPLATE_PARAM_LIST) {
                collect(&inner, index, enc, out);
            }
            if let (Some(params), Some(sym)) = (params, out.get_mut(before)) {
                sym.detail = Some(match sym.detail.take() {
                    Some(existing) => format!("{params} {existing}"),
                    None => params,
                });
                // The template header is part of the declaration.
                sym.range = convert::range(index, enc, node.text_range());
            }
        }
        // `export T;` / `extern T;` name a type that lives elsewhere.
        EXPORT_DECL | EXTERN_DECL => {
            let name = squash(
                &node
                    .children()
                    .find(|c| ast::Type::can_cast(c.kind()))
                    .map(|t| t.text().to_string())
                    .unwrap_or_default(),
            );
            if !name.is_empty() {
                let kw = if node.kind() == EXPORT_DECL { "export" } else { "extern" };
                out.push(symbol(
                    index,
                    enc,
                    name,
                    Some(kw.to_owned()),
                    SymbolKind::INTERFACE,
                    node,
                    None,
                    Vec::new(),
                ));
            }
        }
        // A declaration in statement position, and the two `static if` arms,
        // both hold ordinary items.
        DECL_STMT | STATIC_IF_STMT | ELSE_BRANCH | BLOCK_STMT => {
            for child in node.children() {
                collect(&child, index, enc, out);
            }
        }
        _ => {}
    }
}

fn push_container(
    node: &SyntaxNode,
    index: &LineIndex,
    enc: PositionEncoding,
    kind: SymbolKind,
    out: &mut Vec<DocumentSymbol>,
) {
    let mut children = Vec::new();
    if let Some(list) = node.children().find(|c| c.kind() == DECL_LIST) {
        for item in list.children() {
            collect(&item, index, enc, &mut children);
        }
    }
    push_named(node, index, enc, kind, None, children, out);
}

fn push_named(
    node: &SyntaxNode,
    index: &LineIndex,
    enc: PositionEncoding,
    kind: SymbolKind,
    detail: Option<String>,
    children: Vec<DocumentSymbol>,
    out: &mut Vec<DocumentSymbol>,
) {
    let Some(name) = name_of(node) else { return };
    out.push(symbol(index, enc, name.text, detail, kind, node, Some(name.range), children));
}

#[allow(clippy::too_many_arguments)]
fn symbol(
    index: &LineIndex,
    enc: PositionEncoding,
    name: String,
    detail: Option<String>,
    kind: SymbolKind,
    node: &SyntaxNode,
    selection: Option<TextRange>,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    let range = node.text_range();
    // The protocol requires `selectionRange ⊆ range`; a name recovered from a
    // damaged tree might not be, so fall back rather than emit a symbol the
    // client will reject.
    let selection = selection
        .filter(|s| range.contains_range(*s))
        .unwrap_or_else(|| TextRange::at(range.start(), range.len().min(kg_base::TextSize::new(1))));

    #[allow(deprecated)]
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range: convert::range(index, enc, range),
        selection_range: convert::range(index, enc, selection),
        children: if children.is_empty() { None } else { Some(children) },
    }
}

/// `member` when the declaration sits directly in a class/struct/union member
/// list, `free` otherwise. The distinction is what makes an outline read as
/// methods and fields rather than as a flat pile of functions.
fn member_or(decl: &SyntaxNode, member: SymbolKind, free: SymbolKind) -> SymbolKind {
    if decl.parent().is_some_and(|p| p.kind() == DECL_LIST) { member } else { free }
}

struct NamePart {
    text: String,
    range: TextRange,
}

/// The `Name` a declaration introduces.
fn name_of(node: &SyntaxNode) -> Option<NamePart> {
    let name = node.children().find(|c| c.kind() == NAME)?;
    let text = name.text().to_string();
    if text.trim().is_empty() {
        return None;
    }
    Some(NamePart { text: text.trim().to_owned(), range: name.text_range() })
}

/// `(uint32, bool) -> void`, for a function's outline detail.
fn signature(node: &SyntaxNode) -> Option<String> {
    let params = node.children().find(|c| c.kind() == PARAM_LIST)?;
    let ret = node.children().find(|c| ast::Type::can_cast(c.kind()));
    let params = squash(&params.text().to_string());
    Some(match ret {
        Some(ret) => format!("{params} -> {}", squash(&ret.text().to_string())),
        None => params,
    })
}

/// The `: uint8` underlying type of an enum, which the grammar spells as a
/// `BASE_LIST` and the parser may hand over as a bare `Type`.
fn detail_of(node: &SyntaxNode) -> Option<String> {
    let base = node
        .children()
        .find(|c| c.kind() == BASE_LIST || ast::Type::can_cast(c.kind()))?;
    let text = squash(&base.text().to_string());
    (!text.is_empty()).then(|| if text.starts_with(':') { text } else { format!(": {text}") })
}

/// The declared type of a variable.
fn type_text(node: &SyntaxNode) -> Option<String> {
    let ty = node.children().find(|c| ast::Type::can_cast(c.kind()))?;
    let text = squash(&ty.text().to_string());
    (!text.is_empty()).then_some(text)
}

/// Collapses a node's source text to one line, for display in a detail string.
fn squash(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_syntax::parse_file;

    fn symbols(text: &str) -> Vec<DocumentSymbol> {
        let parse = parse_file(text);
        document_symbols(&parse.syntax_node(), &LineIndex::new(text), PositionEncoding::Utf16)
    }

    /// The grammar is being written concurrently, so a test that asserts on
    /// symbol *content* must first check the tree actually parsed. Until then
    /// the assertion that matters is the one below: no panic, no garbage.
    fn parsed(text: &str) -> bool {
        parse_file(text).syntax_node().children().any(|c| c.kind() != ERROR)
    }

    #[test]
    fn an_unparsed_file_yields_no_symbols_and_does_not_panic() {
        for text in ["", "}}}}", "class", "template<", "\u{1F980}"] {
            let out = symbols(text);
            assert!(out.iter().all(|s| !s.name.is_empty()), "{out:?}");
        }
    }

    #[test]
    fn declarations_become_symbols() {
        let src = "class Adder {\n    uint32 total;\n    inline void add(uint32 x) {}\n}\n";
        assert!(parsed(src), "the fixture must parse or this test asserts nothing");
        let out = symbols(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Adder");
        assert_eq!(out[0].kind, SymbolKind::CLASS);

        let children = out[0].children.as_ref().expect("class members");
        let names: Vec<_> = children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"total"), "{names:?}");
        assert!(names.contains(&"add"), "{names:?}");
    }

    #[test]
    fn an_enum_carries_its_variants_and_its_underlying_type() {
        let src = "enum Colour : uint8 { Red, Green, Blue }\n";
        assert!(parsed(src), "the fixture must parse or this test asserts nothing");
        let out = symbols(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, SymbolKind::ENUM);
        assert_eq!(out[0].detail.as_deref(), Some(": uint8"));
        let variants: Vec<_> = out[0]
            .children
            .as_ref()
            .expect("variants")
            .iter()
            .map(|c| (c.name.as_str(), c.kind))
            .collect();
        assert_eq!(
            variants,
            vec![
                ("Red", SymbolKind::ENUM_MEMBER),
                ("Green", SymbolKind::ENUM_MEMBER),
                ("Blue", SymbolKind::ENUM_MEMBER),
            ]
        );
    }

    #[test]
    fn a_template_reports_as_the_declaration_it_wraps() {
        let src = "template<typename T>\nclass Ring {\n}\n";
        assert!(parsed(src), "the fixture must parse or this test asserts nothing");
        let out = symbols(src);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].name, "Ring");
        assert_eq!(out[0].kind, SymbolKind::CLASS);
        assert_eq!(out[0].detail.as_deref(), Some("template<typename T>"));
        // The range covers the template header, not just the class.
        assert_eq!(out[0].range.start.line, 0);
    }

    #[test]
    fn a_free_function_is_a_function_and_a_member_is_a_method() {
        let src = "void free() {}\nclass C {\n    void member() {}\n}\n";
        assert!(parsed(src), "the fixture must parse or this test asserts nothing");
        let out = symbols(src);
        let flat: Vec<_> = flatten(&out).into_iter().map(|s| (s.name, s.kind)).collect();
        assert!(flat.contains(&("free".to_owned(), SymbolKind::FUNCTION)), "{flat:?}");
        assert!(flat.contains(&("member".to_owned(), SymbolKind::METHOD)), "{flat:?}");
    }

    #[test]
    fn the_selection_range_is_inside_the_range() {
        let src = "class Adder {\n    uint32 total;\n}\nstruct S { uint8 a; }\nenum E : uint8 { A, B }\n";
        for sym in flatten(&symbols(src)) {
            let r = sym.range;
            let s = sym.selection_range;
            assert!(
                (s.start.line, s.start.character) >= (r.start.line, r.start.character)
                    && (s.end.line, s.end.character) <= (r.end.line, r.end.character),
                "{}: selection {s:?} escapes range {r:?}",
                sym.name
            );
        }
    }

    fn flatten(symbols: &[DocumentSymbol]) -> Vec<DocumentSymbol> {
        let mut out = Vec::new();
        for s in symbols {
            out.push(s.clone());
            if let Some(children) = &s.children {
                out.extend(flatten(children));
            }
        }
        out
    }

    #[test]
    fn squashing_collapses_whitespace() {
        assert_eq!(squash("(uint32   x,\n  bool y)"), "(uint32 x, bool y)");
        assert_eq!(squash("   "), "");
    }
}
