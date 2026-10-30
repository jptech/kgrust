//! The syntax vocabulary: every token and node kind in the tree.
//!
//! This enum is the coordination contract between the lexer, the parser, the
//! syntax-tree layer, the formatter and the language server. It is deliberately
//! frozen early and deliberately generous — an unused kind costs nothing, while
//! a missing one forces every consumer to re-agree on a name.
//!
//! Token kinds are grounded in the reference lexer
//! (`compiler/hs/lib/Language/Kanagawa/Parser/Lexer.hs`) and the reference
//! operator table (`compiler/hs/lib/Language/Kanagawa/Parser.hs:913-960`).
//! Node kinds are grounded in the reference AST functor `ExpF`
//! (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:282-583`).

#![allow(non_camel_case_types)]

/// A token or node kind.
///
/// The discriminants are stable within a build only; nothing serializes them.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u16)]
pub enum SyntaxKind {
    // ---------------------------------------------------------------- trivia
    /// Spaces, tabs, newlines.
    WHITESPACE,
    /// `// ...` that is not a doc comment.
    LINE_COMMENT,
    /// `/* ... */` (nestable) that is not a doc comment.
    BLOCK_COMMENT,
    /// `//| ...` — documents the item that *follows*.
    DOC_LINE_PRE,
    /// `//< ...` — documents the item that *precedes*.
    DOC_LINE_POST,
    /// `/*| ... */` — documents the item that *follows*.
    DOC_BLOCK_PRE,
    /// `/*< ... */` — documents the item that *precedes*.
    DOC_BLOCK_POST,

    // ------------------------------------------------------------- literals
    /// An integer literal including any base prefix, `_` separators and
    /// `i<W>`/`u<W>` suffix. Value is arbitrary precision.
    INT_NUMBER,
    /// A floating point literal.
    FLOAT_NUMBER,
    /// A complete `"..."` literal with no `{...}` hole, escapes included.
    STRING,
    /// A string's text up to a hole: the opening piece (quote included) or
    /// the text between two holes. See `Lexer::string_piece`.
    STRING_TEXT,
    /// A string's text after its last hole, closing quote included — the
    /// token that ends an `INTERPOLATED_STRING`.
    STRING_END,

    // ---------------------------------------------------------- identifiers
    IDENT,
    /// `int8`, `int32`, ... — reserved by the reference lexer's `intType`
    /// check, so never an identifier (`Parser/Lexer.hs:241-243`).
    INT_TYPE_TOKEN,
    /// `uint8`, `uint32`, ...
    UINT_TYPE_TOKEN,

    // -------------------------------------------------------- punctuation
    L_PAREN,
    R_PAREN,
    L_CURLY,
    R_CURLY,
    L_BRACK,
    R_BRACK,
    /// Always a single `<`. Never merged by the lexer, which is what makes
    /// `reverse<uint<Width>>(x)` need no `>>` splitting.
    L_ANGLE,
    /// Always a single `>`.
    R_ANGLE,
    SEMICOLON,
    COMMA,
    COLON,
    COLON2,
    DOT,
    EQ,
    EQ2,
    BANG,
    NEQ,
    PLUS,
    MINUS,
    STAR,
    SLASH,
    PERCENT,
    AMP,
    AMP2,
    PIPE,
    PIPE2,
    CARET,
    CARET2,
    TILDE,
    QUESTION,
    THIN_ARROW,
    HASH,
    BACKSLASH,
    /// `<=` — produced by the *parser* merging `<` `=`, never by the lexer.
    LTEQ,
    /// `>=` — parser-merged.
    GTEQ,
    /// `<<` — parser-merged.
    SHL,
    /// `>>` — parser-merged.
    SHR,

    // Increment/decrement and compound assignment. These are parsed with
    // megaparsec's own `L.symbol`, not the local `operator` combinator
    // (`Parser.hs:608-609,616-629`), so they carry no `notFollowedBy` rule.
    // There is deliberately no `<<=` or `>>=`: the reference list at
    // `Parser.hs:617-629` is complete.
    PLUS2,
    MINUS2,
    PLUSEQ,
    MINUSEQ,
    STAREQ,
    SLASHEQ,
    PERCENTEQ,
    AMPEQ,
    PIPEEQ,
    CARETEQ,
    AMP2EQ,
    PIPE2EQ,
    CARET2EQ,

    // ----------------------------------------------------------- keywords
    // The 53 reserved words of `Parser/Lexer.hs:164-219`, in source order.
    AS_KW,
    ATOMIC_KW,
    AUTO_KW,
    BARRIER_KW,
    BITSIZEOF_KW,
    BITOFFSETOF_KW,
    BOOL_KW,
    BREAK_KW,
    BYTESIZEOF_KW,
    BYTEOFFSETOF_KW,
    CAST_KW,
    CASE_KW,
    CLASS_KW,
    CLOG2_KW,
    CONCAT_KW,
    CONST_KW,
    DECLTYPE_KW,
    DEFAULT_KW,
    DO_KW,
    ELSE_KW,
    ENUM_KW,
    EXPORT_KW,
    EXTERN_KW,
    FALSE_KW,
    FAN_OUT_KW,
    FLOAT32_KW,
    FOR_KW,
    IF_KW,
    IMPORT_KW,
    INLINE_KW,
    INT_KW,
    LUTMUL_KW,
    MODULE_KW,
    MUX_KW,
    NOINLINE_KW,
    PRIVATE_KW,
    PUBLIC_KW,
    REORDER_KW,
    RETURN_KW,
    STATIC_KW,
    STATIC_ASSERT_KW,
    STRING_KW,
    STRUCT_KW,
    SWITCH_KW,
    TEMPLATE_KW,
    TRUE_KW,
    TYPENAME_KW,
    UINT_KW,
    UNION_KW,
    UNROLLED_FOR_KW,
    USING_KW,
    VOID_KW,
    WHILE_KW,

    // -------------------------------------------------------------- errors
    /// A character the lexer could not classify. Kept in the tree so text
    /// round-trips; always accompanied by a diagnostic.
    LEX_ERROR,

    // =================================================================== nodes
    SOURCE_FILE,

    // ------------------------------------------------------------- modules
    /// `import a.b.c;` / `import a.b.c as d;`
    IMPORT_DECL,
    /// The dotted module path of an import or module declaration.
    MODULE_PATH,
    /// `module a.b { x, y };`
    MODULE_DECL,
    /// The `{ a, b }` export list of a module declaration.
    MODULE_EXPORT_LIST,

    // -------------------------------------------------------- declarations
    CLASS_DECL,
    STRUCT_DECL,
    UNION_DECL,
    ENUM_DECL,
    ENUM_VARIANT,
    /// A function with a body.
    FN_DECL,
    /// A function signature with no body (`FunctionDeclF`).
    FN_PROTO,
    VAR_DECL,
    /// `using X = T;`
    ALIAS_DECL,
    /// `template<...> <decl>`
    TEMPLATE_DECL,
    TEMPLATE_PARAM_LIST,
    /// `typename T` / `typename T = D`
    TYPE_PARAM,
    /// `uint32 N` / `uint32 N = 8`
    VALUE_PARAM,
    /// `export class X ...`
    EXPORT_DECL,
    /// `extern class X ...`
    EXTERN_DECL,
    /// `static_assert(e);` and `static assert(e);`
    STATIC_ASSERT_DECL,
    /// `public:` / `private:`
    VISIBILITY,
    /// The `{ ... }` member list of a class/struct/union/enum.
    DECL_LIST,
    /// `: Base1, Base2`
    BASE_LIST,
    PARAM_LIST,
    PARAM,
    /// `inline` / `noinline`
    FN_MODIFIER,

    // -------------------------------------------------------------- attributes
    /// One `[[ ... ]]` group.
    ATTR_LIST,
    /// A single attribute inside a group.
    ATTR,
    /// The `(...)` argument of an attribute.
    ATTR_ARGS,

    // ------------------------------------------------------------ statements
    BLOCK_STMT,
    EXPR_STMT,
    ASSIGN_STMT,
    /// A variable declaration in statement position.
    DECL_STMT,
    IF_STMT,
    STATIC_IF_STMT,
    ELSE_BRANCH,
    /// `for (T i : limit) ...`
    FOR_STMT,
    UNROLLED_FOR_STMT,
    STATIC_FOR_STMT,
    WHILE_STMT,
    DO_WHILE_STMT,
    SWITCH_STMT,
    SWITCH_CASE,
    SWITCH_DEFAULT,
    RETURN_STMT,
    BREAK_STMT,
    BARRIER_STMT,
    REORDER_STMT,
    /// `atomic <stmt>` — desugars to `[[schedule(1)]]` at the FFI boundary.
    ATOMIC_STMT,
    /// A statement carrying a leading `[[...]]` attribute group.
    ANNOTATED_STMT,
    /// A lone `;`.
    EMPTY_STMT,

    // ----------------------------------------------------------- expressions
    LITERAL,
    INTERPOLATED_STRING,
    /// One `{expr[=][,align][:fmt[prec]]}` inside an interpolated string.
    INTERPOLATION,
    /// The `:b3` tail of an interpolation.
    FORMAT_SPEC,
    /// A bare identifier used as a value.
    NAME_REF,
    /// A name being introduced by a declaration.
    NAME,
    /// `a::b::c`, optionally with template arguments per segment.
    PATH,
    PATH_SEGMENT,
    /// `<...>` in a type or call position.
    TEMPLATE_ARG_LIST,
    BIN_EXPR,
    PREFIX_EXPR,
    /// `c ? a : b`
    TERNARY_EXPR,
    PAREN_EXPR,
    CALL_EXPR,
    ARG_LIST,
    /// `x:` before an argument, naming its parameter — edition 1.1's named
    /// arguments (`docs/editions/1.1.md`); never in a reference file.
    ARG_LABEL,
    /// `a[i]`
    INDEX_EXPR,
    /// `a.b`
    FIELD_EXPR,
    /// `cast<T>(e)`
    CAST_EXPR,
    /// `concat(a, b, ...)`
    CONCAT_EXPR,
    /// `mux(sel, a, b, ...)`
    MUX_EXPR,
    /// `fan_out<N>(e)`
    FAN_OUT_EXPR,
    /// `static(e)`
    STATIC_EXPR,
    /// `bitoffsetof(T, field)`
    BIT_OFFSET_EXPR,
    /// `byteoffsetof(T, field)`
    BYTE_OFFSET_EXPR,
    /// `lutmul(a, b)`
    LUT_MUL_EXPR,
    /// `{ a, b }` or `{ .x = 1, .y = 2 }`
    INIT_LIST_EXPR,
    /// `.x = 1`
    DESIGNATOR,
    LAMBDA_EXPR,
    CAPTURE_LIST,
    CAPTURE,

    // ---------------------------------------------------------------- types
    VOID_TYPE,
    BOOL_TYPE,
    FLOAT_TYPE,
    STRING_TYPE,
    AUTO_TYPE,
    TYPENAME_TYPE,
    /// `int32` / `uint8`
    INT_TYPE,
    /// `int<Expr>` / `uint<Expr>`
    PARAM_INT_TYPE,
    /// `const T`
    CONST_TYPE,
    /// `T[N]`
    ARRAY_TYPE,
    /// A named (possibly template-instantiated) type.
    PATH_TYPE,
    /// `(params) -> R`
    FN_TYPE,
    FN_TYPE_PARAM_LIST,
    FN_TYPE_PARAM,
    /// `decltype(e)`
    DECLTYPE_TYPE,

    // ------------------------------------------------------------ recovery
    /// Unparseable input, retained so the tree stays lossless.
    ERROR,

    /// Sentinel: not a real kind, and always last. `TOMBSTONE`-style guards and
    /// `u16` conversions rely on this being the count.
    __LAST,
}

use SyntaxKind::*;

impl SyntaxKind {
    /// Maps an identifier-shaped lexeme to its keyword kind, if reserved.
    ///
    /// Mirrors the `reserved` set of `Parser/Lexer.hs:164-219` exactly. The
    /// `int<N>`/`uint<N>` forms are *also* reserved there, but are classified by
    /// the lexer as [`SyntaxKind::INT_TYPE_TOKEN`] / [`SyntaxKind::UINT_TYPE_TOKEN`]
    /// rather than as keywords.
    pub fn from_keyword(text: &str) -> Option<SyntaxKind> {
        let kw = match text {
            "as" => AS_KW,
            "atomic" => ATOMIC_KW,
            "auto" => AUTO_KW,
            "barrier" => BARRIER_KW,
            "bitsizeof" => BITSIZEOF_KW,
            "bitoffsetof" => BITOFFSETOF_KW,
            "bool" => BOOL_KW,
            "break" => BREAK_KW,
            "bytesizeof" => BYTESIZEOF_KW,
            "byteoffsetof" => BYTEOFFSETOF_KW,
            "cast" => CAST_KW,
            "case" => CASE_KW,
            "class" => CLASS_KW,
            "clog2" => CLOG2_KW,
            "concat" => CONCAT_KW,
            "const" => CONST_KW,
            "decltype" => DECLTYPE_KW,
            "default" => DEFAULT_KW,
            "do" => DO_KW,
            "else" => ELSE_KW,
            "enum" => ENUM_KW,
            "export" => EXPORT_KW,
            "extern" => EXTERN_KW,
            "false" => FALSE_KW,
            "fan_out" => FAN_OUT_KW,
            "float32" => FLOAT32_KW,
            "for" => FOR_KW,
            "if" => IF_KW,
            "import" => IMPORT_KW,
            "inline" => INLINE_KW,
            "int" => INT_KW,
            "lutmul" => LUTMUL_KW,
            "module" => MODULE_KW,
            "mux" => MUX_KW,
            "noinline" => NOINLINE_KW,
            "private" => PRIVATE_KW,
            "public" => PUBLIC_KW,
            "reorder" => REORDER_KW,
            "return" => RETURN_KW,
            "static" => STATIC_KW,
            "static_assert" => STATIC_ASSERT_KW,
            "string" => STRING_KW,
            "struct" => STRUCT_KW,
            "switch" => SWITCH_KW,
            "template" => TEMPLATE_KW,
            "true" => TRUE_KW,
            "typename" => TYPENAME_KW,
            "uint" => UINT_KW,
            "union" => UNION_KW,
            "unrolled_for" => UNROLLED_FOR_KW,
            "using" => USING_KW,
            "void" => VOID_KW,
            "while" => WHILE_KW,
            _ => return None,
        };
        Some(kw)
    }

    /// The 53 reserved words, in the order the reference lists them.
    pub const KEYWORDS: [(&'static str, SyntaxKind); 53] = [
        ("as", AS_KW),
        ("atomic", ATOMIC_KW),
        ("auto", AUTO_KW),
        ("barrier", BARRIER_KW),
        ("bitsizeof", BITSIZEOF_KW),
        ("bitoffsetof", BITOFFSETOF_KW),
        ("bool", BOOL_KW),
        ("break", BREAK_KW),
        ("bytesizeof", BYTESIZEOF_KW),
        ("byteoffsetof", BYTEOFFSETOF_KW),
        ("cast", CAST_KW),
        ("case", CASE_KW),
        ("class", CLASS_KW),
        ("clog2", CLOG2_KW),
        ("concat", CONCAT_KW),
        ("const", CONST_KW),
        ("decltype", DECLTYPE_KW),
        ("default", DEFAULT_KW),
        ("do", DO_KW),
        ("else", ELSE_KW),
        ("enum", ENUM_KW),
        ("export", EXPORT_KW),
        ("extern", EXTERN_KW),
        ("false", FALSE_KW),
        ("fan_out", FAN_OUT_KW),
        ("float32", FLOAT32_KW),
        ("for", FOR_KW),
        ("if", IF_KW),
        ("import", IMPORT_KW),
        ("inline", INLINE_KW),
        ("int", INT_KW),
        ("lutmul", LUTMUL_KW),
        ("module", MODULE_KW),
        ("mux", MUX_KW),
        ("noinline", NOINLINE_KW),
        ("private", PRIVATE_KW),
        ("public", PUBLIC_KW),
        ("reorder", REORDER_KW),
        ("return", RETURN_KW),
        ("static", STATIC_KW),
        ("static_assert", STATIC_ASSERT_KW),
        ("string", STRING_KW),
        ("struct", STRUCT_KW),
        ("switch", SWITCH_KW),
        ("template", TEMPLATE_KW),
        ("true", TRUE_KW),
        ("typename", TYPENAME_KW),
        ("uint", UINT_KW),
        ("union", UNION_KW),
        ("unrolled_for", UNROLLED_FOR_KW),
        ("using", USING_KW),
        ("void", VOID_KW),
        ("while", WHILE_KW),
    ];

    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            WHITESPACE
                | LINE_COMMENT
                | BLOCK_COMMENT
                | DOC_LINE_PRE
                | DOC_LINE_POST
                | DOC_BLOCK_PRE
                | DOC_BLOCK_POST
        )
    }

    #[inline]
    pub fn is_comment(self) -> bool {
        self.is_trivia() && self != WHITESPACE
    }

    #[inline]
    pub fn is_doc_comment(self) -> bool {
        matches!(self, DOC_LINE_PRE | DOC_LINE_POST | DOC_BLOCK_PRE | DOC_BLOCK_POST)
    }

    /// True for `//<` and `/*<`, which document the *preceding* item.
    #[inline]
    pub fn is_trailing_doc(self) -> bool {
        matches!(self, DOC_LINE_POST | DOC_BLOCK_POST)
    }

    #[inline]
    pub fn is_keyword(self) -> bool {
        AS_KW <= self && self <= WHILE_KW
    }

    #[inline]
    pub fn is_literal(self) -> bool {
        matches!(self, INT_NUMBER | FLOAT_NUMBER | STRING | STRING_TEXT | STRING_END | TRUE_KW | FALSE_KW)
    }

    /// Everything the lexer can emit: kinds strictly below [`SyntaxKind::SOURCE_FILE`].
    #[inline]
    pub fn is_token(self) -> bool {
        self < SOURCE_FILE
    }

    #[inline]
    pub fn is_node(self) -> bool {
        !self.is_token()
    }

    /// The exact source text of a token kind with a fixed spelling.
    pub fn static_text(self) -> Option<&'static str> {
        let text = match self {
            L_PAREN => "(",
            R_PAREN => ")",
            L_CURLY => "{",
            R_CURLY => "}",
            L_BRACK => "[",
            R_BRACK => "]",
            L_ANGLE => "<",
            R_ANGLE => ">",
            SEMICOLON => ";",
            COMMA => ",",
            COLON => ":",
            COLON2 => "::",
            DOT => ".",
            EQ => "=",
            EQ2 => "==",
            BANG => "!",
            NEQ => "!=",
            PLUS => "+",
            MINUS => "-",
            STAR => "*",
            SLASH => "/",
            PERCENT => "%",
            AMP => "&",
            AMP2 => "&&",
            PIPE => "|",
            PIPE2 => "||",
            CARET => "^",
            CARET2 => "^^",
            TILDE => "~",
            QUESTION => "?",
            THIN_ARROW => "->",
            HASH => "#",
            BACKSLASH => "\\",
            LTEQ => "<=",
            GTEQ => ">=",
            SHL => "<<",
            SHR => ">>",
            PLUS2 => "++",
            MINUS2 => "--",
            PLUSEQ => "+=",
            MINUSEQ => "-=",
            STAREQ => "*=",
            SLASHEQ => "/=",
            PERCENTEQ => "%=",
            AMPEQ => "&=",
            PIPEEQ => "|=",
            CARETEQ => "^=",
            AMP2EQ => "&&=",
            PIPE2EQ => "||=",
            CARET2EQ => "^^=",
            _ => {
                return SyntaxKind::KEYWORDS
                    .iter()
                    .find(|(_, k)| *k == self)
                    .map(|(text, _)| *text);
            }
        };
        Some(text)
    }
}

impl From<SyntaxKind> for u16 {
    #[inline]
    fn from(kind: SyntaxKind) -> u16 {
        kind as u16
    }
}

impl TryFrom<u16> for SyntaxKind {
    type Error = ();
    #[inline]
    fn try_from(raw: u16) -> Result<SyntaxKind, ()> {
        if raw < SyntaxKind::__LAST as u16 {
            // SAFETY-free alternative to `transmute`: the enum is `repr(u16)`
            // with contiguous discriminants starting at 0, so the bounds check
            // above is the whole invariant. Done without `unsafe` because the
            // workspace denies it.
            Ok(ALL[raw as usize])
        } else {
            Err(())
        }
    }
}

/// Every kind, indexed by discriminant. Built once by the test below to prove
/// it stays in sync with the enum.
static ALL: [SyntaxKind; SyntaxKind::__LAST as usize] = {
    let mut all = [SyntaxKind::__LAST; SyntaxKind::__LAST as usize];
    let mut i = 0;
    while i < SyntaxKind::__LAST as usize {
        // `const`-compatible construction: discriminants are contiguous, so the
        // i-th kind is the one whose `as u16` is i. Verified exhaustively in tests.
        all[i] = KIND_TABLE[i];
        i += 1;
    }
    all
};

include!("syntax_kind_table.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_table_is_in_sync() {
        for (i, kind) in KIND_TABLE.iter().enumerate() {
            assert_eq!(*kind as u16 as usize, i, "KIND_TABLE out of order at {i}: {kind:?}");
        }
        assert_eq!(KIND_TABLE.len(), SyntaxKind::__LAST as usize);
    }

    #[test]
    fn keyword_lookup_round_trips() {
        assert_eq!(SyntaxKind::KEYWORDS.len(), 53);
        for (text, kind) in SyntaxKind::KEYWORDS {
            assert_eq!(SyntaxKind::from_keyword(text), Some(kind));
            assert!(kind.is_keyword(), "{text} not classified as a keyword");
            assert_eq!(kind.static_text(), Some(text));
        }
        assert_eq!(SyntaxKind::from_keyword("namespace"), None);
        assert_eq!(SyntaxKind::from_keyword("continue"), None, "the TextMate grammar's phantom keyword");
    }

    #[test]
    fn token_node_split_is_where_we_think() {
        assert!(IDENT.is_token());
        assert!(WHILE_KW.is_token());
        assert!(LEX_ERROR.is_token());
        assert!(SOURCE_FILE.is_node());
        assert!(ERROR.is_node());
        assert!(!SOURCE_FILE.is_keyword());
    }
}
