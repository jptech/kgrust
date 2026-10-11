//! Semantic tokens — where the CST beats a regular expression.
//!
//! The reference extension's TextMate grammar
//! (`third_party/kanagawa/tools/editors/vscode/syntaxes/kanagawa.tmLanguage.json`) is
//! wrong in three ways that a regex cannot fix:
//!
//! 1. it scopes every `<` and `>` as `keyword.operator.comparison`, including
//!    the ones delimiting `reverse<uint<Width>>(x)`;
//! 2. it lists `continue` as a control keyword, which the language does not
//!    have (`SyntaxKind::from_keyword("continue")` is `None`); and
//! 3. it scopes `float` as a builtin type, when the reserved word is `float32`
//!    and bare `float` is an ordinary identifier.
//!
//! All three fall out of classifying real tokens instead of matching text:
//! `<`/`>` are asked [`crate::angles`] what they are, `continue` lexes as an
//! `IDENT` and is highlighted as a variable, and `float32` is a `FLOAT32_KW`
//! while `float` is not.
//!
//! Everything here is decided from the token kind plus the node it sits in, so
//! it degrades cleanly: a file that is entirely `ERROR` nodes still gets
//! correct keywords, numbers, strings, comments and angle brackets.

use kg_base::{LineIndex, TextRange, TextSize};
use kg_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
};

use crate::angles::{AngleMap, AngleRole, angle_role};
use crate::convert::{self, PositionEncoding};

use SyntaxKind::*;

/// A token type the standard legend does not have.
///
/// LSP has no `punctuation` type, but the legend is the server's to define and
/// the VS Code client maps it back to a TextMate scope in
/// `contributes.semanticTokenScopes`. It exists for exactly one reason: to say
/// that the `<` in `Foo<T>` is a bracket and the `<` in `a < b` is not.
const PUNCTUATION: SemanticTokenType = SemanticTokenType::new("punctuation");

macro_rules! legend {
    ($($name:ident => $value:expr),* $(,)?) => {
        #[derive(Copy, Clone, PartialEq, Eq, Debug)]
        #[repr(u32)]
        #[allow(dead_code)]
        pub enum TokenType { $($name),* }

        impl TokenType {
            pub const ALL: &'static [SemanticTokenType] = &[$($value),*];
            #[inline]
            pub fn index(self) -> u32 { self as u32 }
        }
    };
}

legend! {
    Namespace     => SemanticTokenType::NAMESPACE,
    Type          => SemanticTokenType::TYPE,
    Class         => SemanticTokenType::CLASS,
    Enum          => SemanticTokenType::ENUM,
    Struct        => SemanticTokenType::STRUCT,
    TypeParameter => SemanticTokenType::TYPE_PARAMETER,
    Parameter     => SemanticTokenType::PARAMETER,
    Variable      => SemanticTokenType::VARIABLE,
    Property      => SemanticTokenType::PROPERTY,
    EnumMember    => SemanticTokenType::ENUM_MEMBER,
    Function      => SemanticTokenType::FUNCTION,
    Method        => SemanticTokenType::METHOD,
    Keyword       => SemanticTokenType::KEYWORD,
    Modifier      => SemanticTokenType::MODIFIER,
    Comment       => SemanticTokenType::COMMENT,
    String        => SemanticTokenType::STRING,
    Number        => SemanticTokenType::NUMBER,
    Operator      => SemanticTokenType::OPERATOR,
    Decorator     => SemanticTokenType::DECORATOR,
    Punctuation   => PUNCTUATION,
}

/// Modifier bit positions, in legend order.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum Modifier {
    Declaration = 0,
    Definition = 1,
    Readonly = 2,
    Static = 3,
    /// `//|`, `//<`, `/*|` and `/*<`. The reference lexer distinguishes doc
    /// comments from ordinary ones, so the server can too — a TextMate grammar
    /// can only guess by lookahead.
    Documentation = 4,
    DefaultLibrary = 5,
}

impl Modifier {
    pub const ALL: &'static [SemanticTokenModifier] = &[
        SemanticTokenModifier::DECLARATION,
        SemanticTokenModifier::DEFINITION,
        SemanticTokenModifier::READONLY,
        SemanticTokenModifier::STATIC,
        SemanticTokenModifier::DOCUMENTATION,
        SemanticTokenModifier::DEFAULT_LIBRARY,
    ];

    #[inline]
    pub fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TokenType::ALL.to_vec(),
        token_modifiers: Modifier::ALL.to_vec(),
    }
}

/// `textDocument/semanticTokens/full` and `.../range`.
///
/// `range` restricts output to tokens overlapping it; the delta encoding is
/// still relative to the start of the file, which is what the protocol wants.
pub fn semantic_tokens(
    root: &SyntaxNode,
    index: &LineIndex,
    enc: PositionEncoding,
    range: Option<TextRange>,
) -> SemanticTokens {
    let map = AngleMap::build(root);
    let mut builder = Builder::new(index, enc);

    for element in root.descendants_with_tokens() {
        let Some(token) = element.into_token() else { continue };
        let span = token.text_range();
        if let Some(range) = range {
            // `TextRange::intersect` yields an empty range for touching
            // neighbours, which must not count as an overlap.
            if span.end() <= range.start() || span.start() >= range.end() {
                continue;
            }
        }
        let Some((ty, mods)) = classify(&token, &map) else { continue };
        builder.push(span, token.text(), ty, mods);
    }

    SemanticTokens { result_id: None, data: builder.finish() }
}

/// The (type, modifier bitset) of one token, or `None` for whitespace and
/// anything else that should keep the client's own scoping.
fn classify(token: &SyntaxToken, map: &AngleMap) -> Option<(TokenType, u32)> {
    let kind = token.kind();

    if kind.is_comment() {
        let mods = if kind.is_doc_comment() { Modifier::Documentation.bit() } else { 0 };
        return Some((TokenType::Comment, mods));
    }

    let ty = match kind {
        WHITESPACE | LEX_ERROR => return None,

        INT_NUMBER | FLOAT_NUMBER => TokenType::Number,
        STRING | STRING_TEXT | STRING_END => TokenType::String,

        // `int32`/`uint8` are one reserved token each, not an identifier that
        // happens to look like a type (`Parser/Lexer.hs:241-243`).
        INT_TYPE_TOKEN | UINT_TYPE_TOKEN => {
            return Some((TokenType::Type, Modifier::DefaultLibrary.bit()));
        }
        // The builtin type keywords, including the `int`/`uint` of the
        // parametric `int<N>` form — and `float32`, which is the real spelling
        // the TextMate grammar misses.
        VOID_KW | BOOL_KW | FLOAT32_KW | STRING_KW | AUTO_KW | INT_KW | UINT_KW => {
            return Some((TokenType::Type, Modifier::DefaultLibrary.bit()));
        }

        L_ANGLE | R_ANGLE => match angle_role(token, map) {
            AngleRole::Punctuation => TokenType::Punctuation,
            AngleRole::Operator => TokenType::Operator,
        },

        IDENT => return Some(classify_ident(token)),

        k if k.is_keyword() => TokenType::Keyword,

        L_PAREN | R_PAREN | L_CURLY | R_CURLY | L_BRACK | R_BRACK | SEMICOLON | COMMA | COLON
        | COLON2 | DOT | BACKSLASH | HASH => TokenType::Punctuation,

        k if k.is_token() => TokenType::Operator,

        // Nodes never reach here; `descendants_with_tokens` filters them out.
        _ => return None,
    };
    Some((ty, 0))
}

/// An identifier means whatever the node above it says it means.
///
/// Falls back to `variable`, which is both the commonest answer and the safe
/// one while the grammar is incomplete: an identifier in an `ERROR` node is
/// still an identifier.
fn classify_ident(token: &SyntaxToken) -> (TokenType, u32) {
    let Some(parent) = token.parent() else { return (TokenType::Variable, 0) };
    match parent.kind() {
        NAME => classify_declared_name(&parent),
        NAME_REF => classify_name_ref(&parent),
        // `import intel.arria-10.config` — every component is a namespace.
        MODULE_PATH => (TokenType::Namespace, 0),
        // Attribute names lex as plain identifiers (`Parser.hs:171-177`), so
        // `[[latency(3)]]` is only distinguishable structurally.
        ATTR => (TokenType::Decorator, 0),
        // The `b3` of `"{x:b3}"`.
        FORMAT_SPEC => (TokenType::Modifier, 0),
        _ => (TokenType::Variable, 0),
    }
}

fn classify_declared_name(name: &SyntaxNode) -> (TokenType, u32) {
    let Some(decl) = name.parent() else { return (TokenType::Variable, 0) };
    let declaration = Modifier::Declaration.bit();
    let defines = Modifier::Declaration.bit() | Modifier::Definition.bit();

    match decl.kind() {
        CLASS_DECL => (TokenType::Class, defines),
        STRUCT_DECL | UNION_DECL => (TokenType::Struct, defines),
        ENUM_DECL => (TokenType::Enum, defines),
        ENUM_VARIANT => (TokenType::EnumMember, declaration | Modifier::Readonly.bit()),
        FN_DECL => (member_or(&decl, TokenType::Method, TokenType::Function), defines),
        FN_PROTO => (member_or(&decl, TokenType::Method, TokenType::Function), declaration),
        PARAM | FN_TYPE_PARAM => (TokenType::Parameter, declaration),
        VAR_DECL => {
            let mut mods = declaration;
            if decl.children_with_tokens().any(|e| e.kind() == STATIC_KW) {
                mods |= Modifier::Static.bit();
            }
            if decl.children().any(|c| c.kind() == CONST_TYPE) {
                mods |= Modifier::Readonly.bit();
            }
            (member_or(&decl, TokenType::Property, TokenType::Variable), mods)
        }
        ALIAS_DECL => (TokenType::Type, defines),
        // Both template parameter flavours: `typename T` and `uint32 N`. The
        // second is a value, but it is still a parameter *of a template*, and
        // clients theme both with the same colour.
        TYPE_PARAM => (TokenType::TypeParameter, declaration),
        VALUE_PARAM => (TokenType::TypeParameter, declaration | Modifier::Readonly.bit()),
        IMPORT_DECL => (TokenType::Namespace, declaration),
        ATTR => (TokenType::Decorator, 0),
        _ => (TokenType::Variable, declaration),
    }
}

fn classify_name_ref(name_ref: &SyntaxNode) -> (TokenType, u32) {
    let Some(parent) = name_ref.parent() else { return (TokenType::Variable, 0) };
    match parent.kind() {
        // `a.b` and `obj.template Go<uint32>`: a segment hanging directly off a
        // field access is a member.
        PATH_SEGMENT => classify_path_segment(&parent),
        FIELD_EXPR => (TokenType::Property, 0),
        DESIGNATOR => (TokenType::Property, 0),
        // `bitoffsetof(T, field)` names a field of `T`.
        BIT_OFFSET_EXPR | BYTE_OFFSET_EXPR => (TokenType::Property, 0),
        CAPTURE => (TokenType::Variable, 0),
        MODULE_EXPORT_LIST => (TokenType::Namespace, 0),
        _ => (TokenType::Variable, 0),
    }
}

fn classify_path_segment(segment: &SyntaxNode) -> (TokenType, u32) {
    // `FieldExpr = base '.' PathSegment`, so a segment whose parent is the
    // field access itself is the member being selected.
    let Some(path) = segment.parent() else { return (TokenType::Variable, 0) };
    if path.kind() == FIELD_EXPR {
        return if is_callee(&path) {
            (TokenType::Method, 0)
        } else {
            (TokenType::Property, 0)
        };
    }
    if path.kind() != PATH {
        return (TokenType::Variable, 0);
    }
    let Some(context) = path.parent() else { return (TokenType::Variable, 0) };
    match context.kind() {
        // One `Path` serves values, types, templates and callees alike; which
        // it is is decided by what encloses it, never by a symbol table.
        PATH_TYPE => (TokenType::Type, 0),
        FIELD_EXPR => (TokenType::Property, 0),
        _ if is_callee(&path) => (TokenType::Function, 0),
        _ => (TokenType::Variable, 0),
    }
}

/// True when `node` is the callee of a call, rather than one of its arguments.
fn is_callee(node: &SyntaxNode) -> bool {
    node.parent().is_some_and(|p| {
        p.kind() == CALL_EXPR && p.first_child().map(|c| c == *node).unwrap_or(false)
    })
}

/// True when a declaration sits directly in a class/struct/union member list.
fn member_or(decl: &SyntaxNode, member: TokenType, free: TokenType) -> TokenType {
    if decl.parent().is_some_and(|p| p.kind() == DECL_LIST) { member } else { free }
}

/// Accumulates the protocol's delta encoding.
///
/// Two things it must get right: lengths are counted in the negotiated encoding
/// (not bytes), and a token may not span lines unless the client opted in — so
/// a multi-line block comment is emitted as one token per line.
struct Builder<'a> {
    index: &'a LineIndex,
    enc: PositionEncoding,
    prev_line: u32,
    prev_start: u32,
    data: Vec<SemanticToken>,
}

impl<'a> Builder<'a> {
    fn new(index: &'a LineIndex, enc: PositionEncoding) -> Builder<'a> {
        Builder { index, enc, prev_line: 0, prev_start: 0, data: Vec::new() }
    }

    fn push(&mut self, range: TextRange, text: &str, ty: TokenType, mods: u32) {
        let mut start = range.start();
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            let len = TextSize::of(trimmed);
            if len > TextSize::new(0) {
                self.emit(TextRange::at(start, len), ty, mods);
            }
            start += TextSize::of(line);
        }
    }

    fn emit(&mut self, range: TextRange, ty: TokenType, mods: u32) {
        let (line, col) = convert::line_col(self.index, self.enc, range.start());
        let (end_line, end_col) = convert::line_col(self.index, self.enc, range.end());
        debug_assert_eq!(line, end_line, "a semantic token leaked across a line break");

        let delta_line = line.saturating_sub(self.prev_line);
        let delta_start = if delta_line == 0 { col.saturating_sub(self.prev_start) } else { col };

        self.data.push(SemanticToken {
            delta_line,
            delta_start,
            length: end_col.saturating_sub(col),
            token_type: ty.index(),
            token_modifiers_bitset: mods,
        });
        self.prev_line = line;
        self.prev_start = col;
    }

    fn finish(self) -> Vec<SemanticToken> {
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_syntax::parse_file;

    /// Re-expands the delta encoding into `(line, col, len, type, mods)`.
    fn decode(tokens: &SemanticTokens) -> Vec<(u32, u32, u32, TokenType, u32)> {
        let mut out = Vec::new();
        let (mut line, mut col) = (0u32, 0u32);
        for t in &tokens.data {
            if t.delta_line == 0 {
                col += t.delta_start;
            } else {
                line += t.delta_line;
                col = t.delta_start;
            }
            out.push((line, col, t.length, ALL_TYPES[t.token_type as usize], t.token_modifiers_bitset));
        }
        out
    }

    /// The same list the `legend!` macro generates, spelled out so that a
    /// reordering of the enum is caught rather than silently re-indexed.
    const ALL_TYPES: &[TokenType] = &[
        TokenType::Namespace,
        TokenType::Type,
        TokenType::Class,
        TokenType::Enum,
        TokenType::Struct,
        TokenType::TypeParameter,
        TokenType::Parameter,
        TokenType::Variable,
        TokenType::Property,
        TokenType::EnumMember,
        TokenType::Function,
        TokenType::Method,
        TokenType::Keyword,
        TokenType::Modifier,
        TokenType::Comment,
        TokenType::String,
        TokenType::Number,
        TokenType::Operator,
        TokenType::Decorator,
        TokenType::Punctuation,
    ];

    fn tokens(text: &str) -> Vec<(String, TokenType, u32)> {
        let parse = parse_file(text);
        let index = LineIndex::new(text);
        let tokens =
            semantic_tokens(&parse.syntax_node(), &index, PositionEncoding::Utf16, None);
        let decoded = decode(&tokens);

        // Recover the source text of each token by walking the same tokens
        // again; the two lists are in the same order by construction.
        let root = parse.syntax_node();
        let map = AngleMap::build(&root);
        let mut texts = Vec::new();
        for element in root.descendants_with_tokens() {
            let Some(t) = element.into_token() else { continue };
            if classify(&t, &map).is_none() {
                continue;
            }
            for line in t.text().split_inclusive('\n') {
                let trimmed = line.trim_end_matches(['\n', '\r']);
                if !trimmed.is_empty() {
                    texts.push(trimmed.to_owned());
                }
            }
        }
        assert_eq!(texts.len(), decoded.len(), "token/text lists diverged");
        texts
            .into_iter()
            .zip(decoded)
            .map(|(text, (_, _, _, ty, mods))| (text, ty, mods))
            .collect()
    }

    fn find(text: &str, needle: &str) -> (TokenType, u32) {
        let all = tokens(text);
        let found: Vec<_> = all.iter().filter(|(t, ..)| t == needle).collect();
        assert!(!found.is_empty(), "no token {needle:?} in {all:?}");
        (found[0].1, found[0].2)
    }

    #[test]
    fn the_legend_indices_match_the_enum() {
        assert_eq!(TokenType::ALL.len(), ALL_TYPES.len());
        for (i, ty) in ALL_TYPES.iter().enumerate() {
            assert_eq!(ty.index(), i as u32, "{ty:?} is out of order");
        }
        assert_eq!(legend().token_types.len(), TokenType::ALL.len());
        assert_eq!(legend().token_modifiers.len(), Modifier::ALL.len());
    }

    #[test]
    fn keywords_numbers_strings_and_comments_are_classified() {
        let src = "// a comment\nreturn 42;\n";
        assert_eq!(find(src, "// a comment").0, TokenType::Comment);
        assert_eq!(find(src, "return").0, TokenType::Keyword);
        assert_eq!(find(src, "42").0, TokenType::Number);

        let (ty, mods) = find("\"text\";", "\"text\"");
        assert_eq!(ty, TokenType::String);
        assert_eq!(mods, 0);
    }

    #[test]
    fn a_doc_comment_carries_the_documentation_modifier() {
        let (ty, mods) = find("//| documents what follows\nclass A {}\n", "//| documents what follows");
        assert_eq!(ty, TokenType::Comment);
        assert_eq!(mods, Modifier::Documentation.bit());

        // ...and an ordinary comment does not.
        assert_eq!(find("// plain\n", "// plain").1, 0);
    }

    #[test]
    fn builtin_types_are_types_and_float_is_not_one() {
        for kw in ["void", "bool", "float32", "string", "auto", "uint32", "int8"] {
            let src = format!("{kw} x;");
            let (ty, mods) = find(&src, kw);
            assert_eq!(ty, TokenType::Type, "{kw}");
            assert_eq!(mods, Modifier::DefaultLibrary.bit(), "{kw}");
        }
        // The TextMate grammar's third bug: it scopes `float` as a builtin
        // type. The reserved word is `float32`; bare `float` is an ordinary
        // identifier, so whatever it names, it is not from the standard
        // library — and that is exactly what the modifier says.
        let (_, mods) = find("float x;", "float");
        assert_eq!(mods & Modifier::DefaultLibrary.bit(), 0, "`float` claimed to be a builtin");
    }

    #[test]
    fn continue_is_not_a_keyword() {
        // The TextMate grammar's second bug: the language has no `continue`.
        assert_ne!(find("continue;", "continue").0, TokenType::Keyword);
    }

    #[test]
    fn template_angles_are_punctuation_and_comparisons_are_operators() {
        // The TextMate grammar's first bug, and the headline win.
        let ts = tokens("Foo<uint32> x;");
        let angles: Vec<_> = ts.iter().filter(|(t, ..)| t == "<" || t == ">").collect();
        assert_eq!(angles.len(), 2);
        assert!(angles.iter().all(|(_, ty, _)| *ty == TokenType::Punctuation), "{angles:?}");

        let ts = tokens("bool b = a < c;");
        let angles: Vec<_> = ts.iter().filter(|(t, ..)| t == "<").collect();
        assert_eq!(angles.len(), 1);
        assert_eq!(angles[0].1, TokenType::Operator);
    }

    #[test]
    fn nested_template_arguments_are_all_punctuation() {
        let ts = tokens("reverse<uint<Width>>(x);");
        let angles: Vec<_> = ts.iter().filter(|(t, ..)| t == "<" || t == ">").collect();
        assert_eq!(angles.len(), 4, "{ts:?}");
        assert!(angles.iter().all(|(_, ty, _)| *ty == TokenType::Punctuation), "{angles:?}");
    }

    /// The grammar is being written concurrently. Classification that depends
    /// on tree *shape* rather than on token kind is only assertable once the
    /// construct actually parses; token-kind classification is asserted
    /// unconditionally above.
    fn parsed(text: &str) -> bool {
        parse_file(text).errors().is_empty()
    }

    #[test]
    fn declarations_and_uses_are_told_apart_by_the_tree() {
        let src = "class Ring {\n    uint32 head;\n    inline void push(uint32 x) {\n        head = x;\n    }\n}\n";
        assert!(parsed(src), "the fixture must parse or this test asserts nothing");
        assert_eq!(find(src, "Ring").0, TokenType::Class);
        // A variable in a member list is a property, not a local.
        assert_eq!(find(src, "head").0, TokenType::Property);
        // A function in a member list is a method.
        assert_eq!(find(src, "push").0, TokenType::Method);
        assert_eq!(find(src, "x").0, TokenType::Parameter);

        let (_, mods) = find(src, "Ring");
        assert_eq!(
            mods,
            Modifier::Declaration.bit() | Modifier::Definition.bit(),
            "a class declaration both declares and defines"
        );
    }

    #[test]
    fn a_module_path_is_a_namespace_and_an_attribute_is_a_decorator() {
        let src = "import intel.config\n";
        assert!(parsed(src), "the fixture must parse or this test asserts nothing");
        assert_eq!(find(src, "intel").0, TokenType::Namespace);
        assert_eq!(find(src, "config").0, TokenType::Namespace);

        // An attributed array type requires exactly one dimension, and the
        // dimension belongs to the *type*: `Array <$> try memoryAttr <*> type''' <*>
        // (Seq . pure <$> brackets expression)` (`Parser.hs:529`). Written the
        // C way, `[[memory]] uint32 x`, this does not parse — and then the
        // assertion below never runs, because it is guarded on `parsed`.
        let src = "class C {\n    [[memory]] uint32[32] x;\n}\n";
        assert!(parsed(src), "the fixture must parse or the assertion below is vacuous");
        // Attribute names lex as ordinary identifiers; only the tree knows.
        assert_eq!(find(src, "memory").0, TokenType::Decorator);
    }

    #[test]
    fn a_block_comment_is_split_at_every_line_break() {
        let src = "/* one\n   two\n   three */\n";
        let parse = parse_file(src);
        let index = LineIndex::new(src);
        let out = semantic_tokens(&parse.syntax_node(), &index, PositionEncoding::Utf16, None);
        let decoded = decode(&out);
        assert_eq!(decoded.len(), 3, "{decoded:?}");
        assert_eq!(decoded.iter().map(|d| d.0).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(decoded.iter().all(|d| d.3 == TokenType::Comment));
    }

    #[test]
    fn lengths_are_counted_in_utf16_units() {
        // '🦀' is one character, two UTF-16 units, four bytes.
        let src = "// 🦀\n";
        let parse = parse_file(src);
        let index = LineIndex::new(src);

        let utf16 = decode(&semantic_tokens(
            &parse.syntax_node(),
            &index,
            PositionEncoding::Utf16,
            None,
        ));
        assert_eq!(utf16[0].2, 5, "\"// \" + 2 units");

        let utf8 = decode(&semantic_tokens(
            &parse.syntax_node(),
            &index,
            PositionEncoding::Utf8,
            None,
        ));
        assert_eq!(utf8[0].2, 7, "\"// \" + 4 bytes");
    }

    #[test]
    fn a_range_request_returns_only_overlapping_tokens() {
        let src = "class A {}\nclass B {}\n";
        let parse = parse_file(src);
        let index = LineIndex::new(src);
        let second_line = TextRange::new(TextSize::new(11), TextSize::new(21));

        let out = semantic_tokens(
            &parse.syntax_node(),
            &index,
            PositionEncoding::Utf16,
            Some(second_line),
        );
        let decoded = decode(&out);
        assert!(!decoded.is_empty());
        assert!(decoded.iter().all(|d| d.0 == 1), "{decoded:?}");
    }

    #[test]
    fn an_empty_file_yields_no_tokens() {
        let parse = parse_file("");
        let index = LineIndex::new("");
        let out = semantic_tokens(&parse.syntax_node(), &index, PositionEncoding::Utf16, None);
        assert!(out.data.is_empty());
    }
}
