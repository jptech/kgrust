//! What a file's text says of the edition's constructs, read off its tree:
//! the parts that are syntax alone — a trailing comma, a parameter's
//! default — and every list whose arguments the loop places.

use kg_base::{TextRange, TextSize};
use kg_syntax::{NodeOrToken, SyntaxKind, SyntaxNode, SyntaxToken};

/// One file's edition constructs.
#[derive(Default)]
pub(crate) struct Scan {
    /// Each trailing comma (`docs/editions/1.1/trailing-commas.md`): the
    /// comma's range. The parser drops it from the tree's lists, so the
    /// list without it is the program.
    pub commas: Vec<TextRange>,
    /// Each parameter's default (`default-arguments.md`): from the end of
    /// the parameter's name to the end of the default, ` = Step + 1`. The
    /// calls that lean on it are given it, written ([`crate::placed`]).
    pub defaults: Vec<TextRange>,
    /// Every call's argument list and every template argument list, in text
    /// order.
    pub lists: Vec<List>,
}

impl Scan {
    /// Whether any list names an argument, which only the loop can place.
    pub fn labelled(&self) -> bool {
        self.lists.iter().any(|l| l.labelled())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ListKind {
    Call,
    Template,
}

/// A call's argument list, or a template's.
pub(crate) struct List {
    pub kind: ListKind,
    /// The list's node, brackets included: the range the lowering gives a
    /// call's arguments (`kg_hir::lower`'s `args`), which every copy the loop
    /// makes keeps.
    pub range: TextRange,
    /// A template argument list's path — `Buffer<uint8, Wide: true>`, the
    /// range the lowering gives the instance it names.
    pub path: Option<TextRange>,
    /// The callee or the template, as written, for messages.
    pub name: String,
    /// Just past the opening bracket.
    pub open: TextSize,
    pub items: Vec<Item>,
}

impl List {
    pub fn labelled(&self) -> bool {
        self.items.iter().any(|i| i.label.is_some())
    }
}

/// One argument as written.
pub(crate) struct Item {
    /// `x` of `x: 1`.
    pub label: Option<String>,
    /// The label and the value, every token of both: what a node the loop
    /// made of the value lies within.
    pub full: TextRange,
    /// The label and the value without the trivia around them: what a
    /// rewrite of the list replaces.
    pub span: TextRange,
    /// The value as written, its label taken off: from the first character
    /// after the label that is no whitespace — a comment there stays with
    /// the value — to the value's end.
    pub value: TextRange,
}

/// The scan of a file's tree, whose text is `text`.
pub(crate) fn scan(root: &SyntaxNode, text: &str) -> Scan {
    let mut out = Scan::default();
    for element in root.descendants_with_tokens() {
        match element {
            NodeOrToken::Token(t) if t.kind() == SyntaxKind::COMMA => {
                if next_solid(&t).is_some_and(|n| closes(n.kind())) {
                    out.commas.push(t.text_range());
                }
            }
            NodeOrToken::Node(n) if n.kind() == SyntaxKind::PARAM => {
                if let Some(range) = default_of(&n) {
                    out.defaults.push(range);
                }
            }
            NodeOrToken::Node(n) if n.kind() == SyntaxKind::ARG_LIST => {
                // `mux(…)`, `concat(…)` and the other keyword calls take no
                // label (a parse error in every edition) and no default;
                // only a call's list is placed.
                if let Some(call) = n.parent().filter(|p| p.kind() == SyntaxKind::CALL_EXPR) {
                    let name = call.children().find(|c| c.text_range() != n.text_range()).map(|c| trimmed_text(&c, text));
                    out.lists.push(list(&n, ListKind::Call, None, name.unwrap_or_default(), text));
                }
            }
            NodeOrToken::Node(n) if n.kind() == SyntaxKind::TEMPLATE_ARG_LIST => {
                let segment = n.parent().filter(|p| p.kind() == SyntaxKind::PATH_SEGMENT);
                let path = segment.as_ref().and_then(SyntaxNode::parent).filter(|p| p.kind() == SyntaxKind::PATH);
                let name = segment
                    .as_ref()
                    .and_then(|s| s.children().find(|c| c.kind() == SyntaxKind::NAME_REF))
                    .map(|c| trimmed_text(&c, text))
                    .unwrap_or_default();
                out.lists.push(list(&n, ListKind::Template, path.map(|p| p.text_range()), name, text));
            }
            _ => {}
        }
    }
    out
}

/// A closing bracket of a list that may end in a comma.
fn closes(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::R_PAREN | SyntaxKind::R_BRACK | SyntaxKind::R_CURLY | SyntaxKind::R_ANGLE)
}

fn next_solid(t: &SyntaxToken) -> Option<SyntaxToken> {
    let mut next = t.next_token();
    while let Some(n) = next {
        if !n.kind().is_trivia() {
            return Some(n);
        }
        next = n.next_token();
    }
    None
}

fn prev_solid(t: &SyntaxToken) -> Option<SyntaxToken> {
    let mut prev = t.prev_token();
    while let Some(p) = prev {
        if !p.kind().is_trivia() {
            return Some(p);
        }
        prev = p.prev_token();
    }
    None
}

/// A parameter's default, ` = e`: from the end of what precedes the `=` to
/// the end of the parameter.
fn default_of(param: &SyntaxNode) -> Option<TextRange> {
    let eq = param.children_with_tokens().filter_map(NodeOrToken::into_token).find(|t| t.kind() == SyntaxKind::EQ)?;
    let start = prev_solid(&eq)?.text_range().end();
    let end = kg_syntax::trimmed_range(param).end();
    (start < end).then(|| TextRange::new(start, end))
}

fn trimmed_text(node: &SyntaxNode, text: &str) -> String {
    let r = kg_syntax::trimmed_range(node);
    text[usize::from(r.start())..usize::from(r.end())].to_owned()
}

/// A list's items, split at its commas, as the lowering splits them
/// (`kg_hir::lower`'s `labelled_items`): a label, then a node.
fn list(node: &SyntaxNode, kind: ListKind, path: Option<TextRange>, name: String, text: &str) -> List {
    let open = node.first_token().map_or(node.text_range().start(), |t| t.text_range().end());
    let mut items = Vec::new();
    let mut label: Option<SyntaxNode> = None;
    for element in node.children_with_tokens() {
        let child = match element {
            NodeOrToken::Token(t) => {
                if t.kind() == SyntaxKind::COMMA {
                    label = None;
                }
                continue;
            }
            NodeOrToken::Node(n) => n,
        };
        if child.kind() == SyntaxKind::ARG_LABEL {
            label = Some(child);
            continue;
        }
        let value = kg_syntax::trimmed_range(&child);
        let item = match label.take() {
            Some(l) => {
                let name = l
                    .children_with_tokens()
                    .filter_map(NodeOrToken::into_token)
                    .find(|t| t.kind() == SyntaxKind::IDENT)
                    .map(|t| t.text().to_owned());
                let after = kg_syntax::trimmed_range(&l).end();
                let skipped = text[usize::from(after)..].len() - text[usize::from(after)..].trim_start().len();
                let start = (after + TextSize::from(skipped as u32)).min(value.start());
                Item {
                    label: name,
                    full: l.text_range().cover(child.text_range()),
                    span: TextRange::new(kg_syntax::trimmed_range(&l).start(), value.end()),
                    value: TextRange::new(start, value.end()),
                }
            }
            None => Item { label: None, full: child.text_range(), span: value, value },
        };
        items.push(item);
    }
    List { kind, range: node.text_range(), path, name, open, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned(text: &str) -> Scan {
        let all = kg_syntax::ParseOptions { features: kg_base::language::Features::all(), ..Default::default() };
        let parse = kg_syntax::parse_file_with(text, all);
        assert_eq!(parse.errors(), &[], "{text}");
        scan(&parse.syntax_node(), text)
    }

    /// A comma before a closing bracket of any kind is the construct; a comma
    /// between two items is not. Comment and line break between the two do
    /// not hide it. Red with the next token read without skipping trivia.
    #[test]
    fn a_comma_before_a_closer_is_trailing() {
        let text = "enum E : uint2 { A, B, }\nconst S<uint8, 3 /* c */ ,\n> s = {};\nconst uint8[2] t = { 1, 2, // two\n};\n";
        let commas = scanned(text).commas;
        assert_eq!(commas.len(), 3, "{commas:?}");
        assert!(commas.iter().all(|&c| &text[c] == ","));
    }

    /// A default runs from what precedes its `=` to its end, the comment before
    /// the next parameter staying. Red with the range starting at the `=`,
    /// which leaves a space after the name.
    #[test]
    fn a_default_is_cut_from_its_name_to_its_end() {
        let text = "inline uint8 f(uint8 x = 1 + 2, /* y */ uint8 y) { return x; }\n";
        let defaults = scanned(text).defaults;
        assert_eq!(defaults.len(), 1);
        assert_eq!(&text[defaults[0]], " = 1 + 2");
    }

    /// A list's items as the lowering splits them: a label, the value it names —
    /// with a comment after the label, which moves with it — and the range the
    /// lowering gives the list and the path. Red with the value starting at the
    /// value's node, which drops the comment.
    #[test]
    fn a_list_is_split_at_its_commas() {
        let text = "inline void f() { g(2 + 3, a: /* c */ 1,); static S<uint8, N: 4> s; }\n";
        let lists = scanned(text).lists;
        assert_eq!(lists.len(), 2);
        let call = &lists[0];
        assert_eq!((call.kind, call.name.as_str()), (ListKind::Call, "g"));
        assert_eq!(&text[call.range], "(2 + 3, a: /* c */ 1,)");
        assert_eq!(usize::from(call.open), text.find("(2").unwrap() + 1);
        let items: Vec<_> = call.items.iter().map(|i| (i.label.as_deref(), &text[i.span], &text[i.value])).collect();
        assert_eq!(items, vec![(None, "2 + 3", "2 + 3"), (Some("a"), "a: /* c */ 1", "/* c */ 1")]);
        let template = &lists[1];
        assert_eq!((template.kind, template.name.as_str()), (ListKind::Template, "S"));
        assert_eq!(text[template.path.unwrap()].trim(), "S<uint8, N: 4>");
        assert!(template.labelled() && call.items[0].label.is_none());
    }
}
