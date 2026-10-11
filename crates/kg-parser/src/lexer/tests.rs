//! Lexer tests.
//!
//! The unit tests pin one lexeme class each against the reference rules; the
//! corpus tests re-run the whole thing over every `.k` file the reference
//! compiler ships and assert the three global invariants (lossless, no errors,
//! no parser-only token kinds).

use super::*;
use expect_test::{Expect, expect};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `(kind, text)` for every token, trivia included.
fn lex(input: &str) -> Vec<(SyntaxKind, &str)> {
    LexedStr::new(input).iter().collect()
}

/// Kinds only, for tests where the text is obvious from the input.
fn kinds(input: &str) -> Vec<SyntaxKind> {
    lex(input).into_iter().map(|(k, _)| k).collect()
}

/// Asserts the input is one single token of `kind` spanning all of it.
#[track_caller]
fn single(input: &str, kind: SyntaxKind) {
    let lexed = LexedStr::new(input);
    assert_eq!(lexed.len(), 1, "{input:?} did not lex as one token: {:?}", lex(input));
    assert_eq!(lexed.kind(0), kind, "{input:?}");
    assert_eq!(lexed.text(0), input, "{input:?}");
    assert!(lexed.errors().is_empty(), "{input:?}: {:?}", lexed.errors());
}

#[track_caller]
fn errors(input: &str) -> Vec<String> {
    LexedStr::new(input).errors().iter().map(|e| e.msg.clone()).collect()
}

// ---------------------------------------------------------------- structure

#[test]
fn empty_input() {
    let lexed = LexedStr::new("");
    assert!(lexed.is_empty());
    assert_eq!(lexed.len(), 0);
    assert_eq!(lexed.as_str(), "");
    assert!(lexed.errors().is_empty());
    assert_eq!(lexed.iter().count(), 0);
    assert!(!lexed.glued(0));
}

#[test]
fn ranges_are_contiguous_and_glued() {
    let lexed = LexedStr::new("a<<b");
    assert_eq!(kinds("a<<b"), vec![IDENT, L_ANGLE, L_ANGLE, IDENT]);
    let mut end = TextSize::new(0);
    for i in 0..lexed.len() {
        assert_eq!(lexed.range(i).start(), end);
        assert_eq!(lexed.text_start(i), end);
        end = lexed.range(i).end();
    }
    assert_eq!(usize::from(end), "a<<b".len());
    assert!(lexed.glued(1));
    assert!(!lexed.glued(lexed.len() - 1), "the last token has no successor");
}

// -------------------------------------------------------------- whitespace

#[test]
fn whitespace_runs_merge() {
    assert_eq!(lex(" \t\n\r\x0b\x0c a"), vec![(WHITESPACE, " \t\n\r\x0b\x0c "), (IDENT, "a")]);
    // NBSP is `isSpace` in GHC; U+0085 NEXT LINE is not, despite Rust saying so.
    assert_eq!(kinds("\u{a0}"), vec![WHITESPACE]);
    assert_eq!(kinds("\u{85}"), vec![LEX_ERROR]);
    assert_eq!(kinds("\u{2003}"), vec![WHITESPACE]);
}

// ------------------------------------------------------------- identifiers

#[test]
fn identifiers_and_keywords() {
    assert_eq!(kinds("foo _bar b4z _ __"), {
        vec![IDENT, WHITESPACE, IDENT, WHITESPACE, IDENT, WHITESPACE, IDENT, WHITESPACE, IDENT]
    });
    assert_eq!(kinds("class"), vec![CLASS_KW]);
    assert_eq!(kinds("classy"), vec![IDENT]);
    assert_eq!(kinds("static_assert"), vec![STATIC_ASSERT_KW]);
    // Not reserved, despite looking like it should be.
    assert_eq!(kinds("continue"), vec![IDENT]);
    assert_eq!(kinds("namespace"), vec![IDENT]);
}

#[test]
fn every_keyword_lexes_as_itself() {
    for (text, kind) in SyntaxKind::KEYWORDS {
        single(text, kind);
    }
}

#[test]
fn unicode_identifiers() {
    // `letterChar` is `Data.Char.isAlpha`, so this is legal (Lexer.hs:224-225).
    single("café", IDENT);
    single("Ωmega", IDENT);
    single("_ünïcödé2", IDENT);
    // A Unicode digit is `isAlphaNum` but not `isAlpha`: legal inside, not at
    // the start.
    single("a٣", IDENT);
    assert_eq!(kinds("٣"), vec![LEX_ERROR]);
}

#[test]
fn int_type_tokens() {
    single("int8", INT_TYPE_TOKEN);
    single("int32", INT_TYPE_TOKEN);
    single("uint1", UINT_TYPE_TOKEN);
    single("uint0", UINT_TYPE_TOKEN);
    // Bare `int`/`uint` stay keywords: `int<N>` is the parameterised form
    // (Parser.hs:544-545).
    single("int", INT_KW);
    single("uint", UINT_KW);
    // `intSuffix`'s `notFollowedBy (alphaNumChar <|> char '_')` fails, and
    // `intType` then rejects, so these are ordinary identifiers.
    single("int32x", IDENT);
    single("int32_", IDENT);
    single("intx", IDENT);
    single("uinteger", IDENT);
}

#[test]
fn module_path_dashes_are_separate_tokens() {
    // `dotSepName` (Lexer.hs:230-233) allows `-` inside a segment, but that is a
    // parser-level regrouping; the lexer keeps it in pieces.
    assert_eq!(
        lex("intel.arria-10.config"),
        vec![
            (IDENT, "intel"),
            (DOT, "."),
            (IDENT, "arria"),
            (MINUS, "-"),
            (INT_NUMBER, "10"),
            (DOT, "."),
            (IDENT, "config"),
        ]
    );
}

// ----------------------------------------------------------------- numbers

#[test]
fn integer_bases() {
    single("0", INT_NUMBER);
    single("42", INT_NUMBER);
    single("305_441_741", INT_NUMBER);
    single("0xff", INT_NUMBER);
    single("0XFF", INT_NUMBER);
    single("0x_1234_abcd", INT_NUMBER);
    single("0b_1111_0000", INT_NUMBER);
    single("0B0", INT_NUMBER);
    single("0o777", INT_NUMBER);
    single("0O_7_7", INT_NUMBER);
    // A trailing separator is legal: `takeWhileP isDigitOrUnderscore`.
    single("1_", INT_NUMBER);
    single("0b0_", INT_NUMBER);
    // No leading-zero rule.
    single("007", INT_NUMBER);
}

#[test]
fn integer_prefix_without_digits_falls_back_to_decimal() {
    // Each radix is `try`ed, so `0b` backtracks to the decimal branch and the
    // marker becomes an identifier (Lexer.hs:134).
    assert_eq!(lex("0b"), vec![(INT_NUMBER, "0"), (IDENT, "b")]);
    assert_eq!(lex("0b_"), vec![(INT_NUMBER, "0"), (IDENT, "b_")]);
    assert_eq!(lex("0x"), vec![(INT_NUMBER, "0"), (IDENT, "x")]);
    assert_eq!(lex("0x__1"), vec![(INT_NUMBER, "0"), (IDENT, "x__1")]);
    assert_eq!(lex("0o8"), vec![(INT_NUMBER, "0"), (IDENT, "o8")]);
}

#[test]
fn integer_suffixes() {
    single("42i32", INT_NUMBER);
    single("0xFFu8", INT_NUMBER);
    single("0b1u1", INT_NUMBER);
    // `L.decimal` needs at least one digit, so a bare `i` is not a suffix.
    assert_eq!(lex("42i"), vec![(INT_NUMBER, "42"), (IDENT, "i")]);
    assert_eq!(lex("42z8"), vec![(INT_NUMBER, "42"), (IDENT, "z8")]);
    // The width is plain decimal, so the trailing `x` is a separate token.
    assert_eq!(lex("42i32x"), vec![(INT_NUMBER, "42i32"), (IDENT, "x")]);
}

#[test]
fn floats() {
    single("1.5", FLOAT_NUMBER);
    single("0.0", FLOAT_NUMBER);
    single("2147483647.0", FLOAT_NUMBER);
    single("2.80259692864963414184745916658E-45", FLOAT_NUMBER);
    single("1.5e10", FLOAT_NUMBER);
    single("1.5E+3", FLOAT_NUMBER);
    // No fractional part, no float: `L.float` needs the `.` and digits after it.
    assert_eq!(lex("1."), vec![(INT_NUMBER, "1"), (DOT, ".")]);
    assert_eq!(lex(".5"), vec![(DOT, "."), (INT_NUMBER, "5")]);
    // `_` separators are an `integer` feature only.
    assert_eq!(lex("1_0.5"), vec![(INT_NUMBER, "1_0"), (DOT, "."), (INT_NUMBER, "5")]);
    // The exponent is `option (try ...)`, so a dangling `e` backtracks out.
    assert_eq!(lex("1.5e"), vec![(FLOAT_NUMBER, "1.5"), (IDENT, "e")]);
    assert_eq!(lex("1.5e+"), vec![(FLOAT_NUMBER, "1.5"), (IDENT, "e"), (PLUS, "+")]);
    // Hex is not a float even with a dot after it.
    assert_eq!(lex("0x1.5"), vec![(INT_NUMBER, "0x1"), (DOT, "."), (INT_NUMBER, "5")]);
}

// ----------------------------------------------------------------- strings

#[test]
fn plain_strings() {
    single(r#""""#, STRING);
    single(r#""hello""#, STRING);
    single(r#""a\"b""#, STRING);
}

#[test]
fn every_escape_form() {
    // Simple character escapes.
    for e in [r"\a", r"\b", r"\f", r"\n", r"\r", r"\t", r"\v", r"\\", r#"\""#, r"\'"] {
        single(&format!("\"x{e}y\""), STRING);
    }
    // Numeric escapes: decimal, octal, hexadecimal.
    single(r#""\65 \o101 \x41""#, STRING);
    // The empty escape, which separates an escape from following digits.
    single(r#""\65\&7""#, STRING);
    // Control escapes.
    single(r#""\^A\^@\^_""#, STRING);
    // Every ASCII mnemonic, and the longest-match rule (`\SOH`, not `\SO` + H).
    for m in ASCII_MNEMONICS {
        single(&format!("\"\\{m}\""), STRING);
    }
    // String gaps: a backslash, whitespace, a backslash.
    single("\"ab\\   \\cd\"", STRING);
    single("\"ab\\\n  \\cd\"", STRING);

    assert_eq!(errors(r#""\q""#), vec!["unknown escape sequence `\\q`"]);
    assert_eq!(errors(r#""\x""#), vec!["`\\hexadecimal` escape has no digits"]);
    assert_eq!(errors(r#""\o""#), vec!["`\\octal` escape has no digits"]);
    assert_eq!(errors(r#""\^ ""#), vec!["invalid control escape; expected `\\^@` through `\\^_`"]);
    assert_eq!(errors("\"ab\\   "), vec!["unterminated string gap", "unterminated string literal"]);
}

#[test]
fn escaped_quote_does_not_end_the_string() {
    let input = r#""\"" x"#;
    assert_eq!(lex(input), vec![(STRING, r#""\"""#), (WHITESPACE, " "), (IDENT, "x")]);
}

#[test]
fn interpolation() {
    // The text, the hole's own tokens, the text after it.
    assert_eq!(
        lex(r#""a{b}c""#),
        vec![(STRING_TEXT, "\"a"), (L_CURLY, "{"), (IDENT, "b"), (R_CURLY, "}"), (STRING_END, "c\"")]
    );
    // A hole straight after the quote, two holes with nothing between them,
    // and a hole straight before the closing quote: the opening quote is
    // text, the empty middle is no token, the closing quote is the end.
    assert_eq!(
        kinds(r#""{a}{b}""#),
        vec![STRING_TEXT, L_CURLY, IDENT, R_CURLY, L_CURLY, IDENT, R_CURLY, STRING_END]
    );
    assert_eq!(lex(r#""{a}""#)[0], (STRING_TEXT, "\""));
    assert_eq!(lex(r#""{a}""#)[4], (STRING_END, "\""));
    // Code inside a hole is lexed as code, whitespace and all.
    assert_eq!(
        kinds(r#""{ x + 1 }""#),
        vec![STRING_TEXT, L_CURLY, WHITESPACE, IDENT, WHITESPACE, PLUS, WHITESPACE, INT_NUMBER, WHITESPACE, R_CURLY, STRING_END]
    );

    // `{{` is an escaped brace and opens nothing (Parser.hs:892-893).
    single(r#""{{}}""#, STRING);
    // `{{{` is a literal `{` followed by a hole.
    assert_eq!(lex(r#""{{{x}""#)[0], (STRING_TEXT, "\"{{"));
    // A bare `}` in literal text is ordinary, unlike C#/Rust format strings.
    single(r#""}""#, STRING);

    // Braces nest inside a hole (initializer lists): only the `}` at depth
    // zero closes it.
    assert_eq!(
        kinds(r#""{f({1})}x""#),
        vec![STRING_TEXT, L_CURLY, IDENT, L_PAREN, L_CURLY, INT_NUMBER, R_CURLY, R_PAREN, R_CURLY, STRING_END]
    );
    // Format specifiers and `=` are code too.
    assert_eq!(
        kinds(r#""0x{pc << 2:x8}\n""#),
        vec![STRING_TEXT, L_CURLY, IDENT, WHITESPACE, L_ANGLE, L_ANGLE, WHITESPACE, INT_NUMBER, COLON, IDENT, R_CURLY, STRING_END]
    );
    assert_eq!(kinds(r#""{x=,10}""#), vec![STRING_TEXT, L_CURLY, IDENT, EQ, COMMA, INT_NUMBER, R_CURLY, STRING_END]);
    assert!(LexedStr::new(r#""{expected=}\n{actual=}\n""#).errors().is_empty());
}

#[test]
fn nested_string_inside_interpolation() {
    // library/processor/risc_v/trace.k:651
    let input = r#""{"0x{addr:x8}",indentation} +{replicate<N-2>("-")}+\n""#;
    let lexed = lex(input);
    assert_eq!(lexed.iter().map(|(_, t)| *t).collect::<String>(), input);
    assert_eq!(
        lexed.iter().filter(|(k, _)| matches!(k, STRING | STRING_TEXT | STRING_END)).collect::<Vec<_>>(),
        vec![
            &(STRING_TEXT, "\""),
            &(STRING_TEXT, "\"0x"),
            &(STRING_END, "\""),
            &(STRING_TEXT, " +"),
            &(STRING, "\"-\""),
            &(STRING_END, "+\\n\""),
        ]
    );
    // test/syntax/interpolated-string.k:187
    assert_eq!(
        kinds(r#""{stringFun("{boolConst:b3}")}""#),
        vec![
            STRING_TEXT, L_CURLY, IDENT, L_PAREN, STRING_TEXT, L_CURLY, IDENT, COLON, IDENT, R_CURLY,
            STRING_END, R_PAREN, R_CURLY, STRING_END
        ]
    );
    // A comment inside a hole is a comment: its quote opens nothing.
    assert_eq!(kinds(r#""{ /* " */ x }""#)[3], BLOCK_COMMENT);
    assert!(LexedStr::new(r#""{ /* " */ x }""#).errors().is_empty());
}

#[test]
fn unterminated_strings_run_to_eof() {
    for input in ["\"", "\"abc", "\"\\"] {
        let lexed = LexedStr::new(input);
        assert_eq!(lexed.len(), 1, "{input:?}");
        assert_eq!(lexed.kind(0), STRING, "{input:?}");
        assert_eq!(lexed.text(0), input, "{input:?}");
        // `"\` reports the dangling escape too, so only the last message is
        // fixed; every message must point at the one token.
        assert!(lexed.errors().iter().all(|e| e.token == 0), "{input:?}");
        assert_eq!(
            lexed.errors().last().map(|e| e.msg.as_str()),
            Some("unterminated string literal"),
            "{input:?}"
        );
    }
    // With a hole open, or a hole closed and no quote after it, the message
    // points at the string's opening piece — once, however deep the nesting.
    for input in ["\"a{b", "\"a{\"b", "\"a{b}c", "x \"a{\"{b}"] {
        let lexed = LexedStr::new(input);
        assert_eq!(lex(input).iter().map(|(_, t)| *t).collect::<String>(), input, "{input:?}");
        let msgs: Vec<_> = lexed.errors().iter().filter(|e| e.msg == "unterminated string literal").collect();
        assert_eq!(msgs.len(), 1, "{input:?}: {:?}", lexed.errors());
        assert_eq!(lexed.kind(msgs[0].token), STRING_TEXT, "{input:?}");
        assert!(lexed.text(msgs[0].token).starts_with('"'), "{input:?}");
    }
}

// ---------------------------------------------------------------- comments

#[test]
fn line_comments() {
    assert_eq!(lex("// hi\nx"), vec![(LINE_COMMENT, "// hi"), (WHITESPACE, "\n"), (IDENT, "x")]);
    assert_eq!(kinds("//"), vec![LINE_COMMENT]);
    assert_eq!(kinds("///"), vec![LINE_COMMENT]);
    assert_eq!(kinds("//| doc"), vec![DOC_LINE_PRE]);
    assert_eq!(kinds("//< doc"), vec![DOC_LINE_POST]);
    // `hspace` between the opener and the marker still makes it a doc comment
    // (Lexer.hs:110-111) — library/processor/risc_v/isa.k:231 relies on it.
    assert_eq!(kinds("//   |  funct7  |"), vec![DOC_LINE_PRE]);
    assert_eq!(kinds("//\t< doc"), vec![DOC_LINE_POST]);
    // `hspace` is horizontal only, so a marker on the next line does not count.
    assert_eq!(kinds("//\n| x"), vec![LINE_COMMENT, WHITESPACE, PIPE, WHITESPACE, IDENT]);
}

#[test]
fn block_comments_nest() {
    single("/* a */", BLOCK_COMMENT);
    single("/* a /* b */ c */", BLOCK_COMMENT);
    single("/*/*/**/*/*/", BLOCK_COMMENT);
    single("/**/", BLOCK_COMMENT);
    assert_eq!(lex("/*a*/b"), vec![(BLOCK_COMMENT, "/*a*/"), (IDENT, "b")]);
}

#[test]
fn doc_block_comments_do_not_nest() {
    // `manyTill anySingle (string "*/")` at Lexer.hs:88 stops at the first
    // terminator, unlike the nesting non-doc scanner at Lexer.hs:116-119.
    assert_eq!(kinds("/*| a */"), vec![DOC_BLOCK_PRE]);
    assert_eq!(kinds("/*< a */"), vec![DOC_BLOCK_POST]);
    assert_eq!(kinds("/*  | a */"), vec![DOC_BLOCK_PRE]);
    assert_eq!(
        lex("/*| a /* b */ c */"),
        vec![
            (DOC_BLOCK_PRE, "/*| a /* b */"),
            (WHITESPACE, " "),
            (IDENT, "c"),
            (WHITESPACE, " "),
            (STAR, "*"),
            (SLASH, "/"),
        ]
    );
}

#[test]
fn unterminated_block_comments_run_to_eof() {
    for (input, kind) in [("/*", BLOCK_COMMENT), ("/*/*/*", BLOCK_COMMENT), ("/*| x", DOC_BLOCK_PRE)] {
        let lexed = LexedStr::new(input);
        assert_eq!(lexed.len(), 1, "{input:?}");
        assert_eq!(lexed.kind(0), kind, "{input:?}");
        assert_eq!(lexed.text(0), input, "{input:?}");
        assert_eq!(lexed.errors().len(), 1, "{input:?}");
    }
    assert_eq!(errors("/*"), vec!["unterminated block comment"]);
}

/// Pins the contract the doc extractor has to implement: continuation lines of
/// a doc comment are ordinary `LINE_COMMENT` tokens, and only the run that
/// opens with a marker makes them documentation (`Parser/Lexer.hs:81-88`).
#[test]
fn doc_comment_continuation_lines_stay_plain() {
    // library/intel/config.k:37
    let input = "//| This value is multiplied by the number of muxes.\n\
                 // the relative cost of synthesizing a memory structure.\n";
    let comments: Vec<_> =
        LexedStr::new(input).iter().filter(|(k, _)| k.is_comment()).map(|(k, _)| k).collect();
    assert_eq!(comments, vec![DOC_LINE_PRE, LINE_COMMENT]);
}

#[test]
fn comment_classification_covers_all_four_doc_forms() {
    let input = "//| a\n//< b\n/*| c */\n/*< d */\n// e\n/* f */";
    let doc: Vec<_> = LexedStr::new(input)
        .iter()
        .filter(|(k, _)| k.is_comment())
        .map(|(k, _)| k)
        .collect();
    assert_eq!(doc, vec![DOC_LINE_PRE, DOC_LINE_POST, DOC_BLOCK_PRE, DOC_BLOCK_POST, LINE_COMMENT, BLOCK_COMMENT]);
}

// ------------------------------------------------------------- punctuation

#[test]
fn operators_are_maximal_munch_except_angles() {
    assert_eq!(kinds("::"), vec![COLON2]);
    assert_eq!(kinds(": :"), vec![COLON, WHITESPACE, COLON]);
    assert_eq!(kinds("->"), vec![THIN_ARROW]);
    assert_eq!(kinds("=="), vec![EQ2]);
    assert_eq!(kinds("!="), vec![NEQ]);
    assert_eq!(kinds("&&"), vec![AMP2]);
    assert_eq!(kinds("||"), vec![PIPE2]);
    assert_eq!(kinds("^^"), vec![CARET2]);
    assert_eq!(kinds("==="), vec![EQ2, EQ]);
    // Angles never merge, which is what makes `uint<8>>` need no splitting.
    assert_eq!(kinds("<<"), vec![L_ANGLE, L_ANGLE]);
    assert_eq!(kinds(">>"), vec![R_ANGLE, R_ANGLE]);
    assert_eq!(kinds("<="), vec![L_ANGLE, EQ]);
    assert_eq!(kinds(">="), vec![R_ANGLE, EQ]);
    assert_eq!(kinds(">>="), vec![R_ANGLE, R_ANGLE, EQ]);
    // Brackets stay single so `[[...]]` is rebuilt from adjacency.
    assert_eq!(kinds("[[x]]"), vec![L_BRACK, L_BRACK, IDENT, R_BRACK, R_BRACK]);
}

#[test]
fn every_punctuation_kind() {
    let table: &[(&str, SyntaxKind)] = &[
        ("(", L_PAREN),
        (")", R_PAREN),
        ("{", L_CURLY),
        ("}", R_CURLY),
        ("[", L_BRACK),
        ("]", R_BRACK),
        ("<", L_ANGLE),
        (">", R_ANGLE),
        (";", SEMICOLON),
        (",", COMMA),
        (":", COLON),
        ("::", COLON2),
        (".", DOT),
        ("=", EQ),
        ("==", EQ2),
        ("!", BANG),
        ("!=", NEQ),
        ("+", PLUS),
        ("-", MINUS),
        ("*", STAR),
        ("/", SLASH),
        ("%", PERCENT),
        ("&", AMP),
        ("&&", AMP2),
        ("|", PIPE),
        ("||", PIPE2),
        ("^", CARET),
        ("^^", CARET2),
        ("~", TILDE),
        ("?", QUESTION),
        ("->", THIN_ARROW),
        ("#", HASH),
        ("\\", BACKSLASH),
    ];
    for (text, kind) in table {
        single(text, *kind);
    }
}

#[test]
fn unknown_characters_become_lex_errors() {
    assert_eq!(lex("a $ b"), {
        vec![(IDENT, "a"), (WHITESPACE, " "), (LEX_ERROR, "$"), (WHITESPACE, " "), (IDENT, "b")]
    });
    assert_eq!(errors("$"), vec!["unexpected character `$`"]);
    // A multi-byte char is consumed whole, so the stream stays lossless.
    assert_eq!(lex("→"), vec![(LEX_ERROR, "→")]);
}

// --------------------------------------------------------------- lossless

/// Snippets chosen to hit every scanner branch, including the ones that end in
/// an error.
const SNIPPETS: &[&str] = &[
    "",
    " ",
    "\n\n",
    "x",
    "\"",
    "\"\"\"",
    "/*",
    "/*/*",
    "/*| x",
    "//",
    "\\",
    "$$$",
    "0x",
    "1.",
    "\"\\",
    "\"a{b",
    "int32 x = 0xFF_u8;",
    "import intel.arria-10.config as cfg;",
    r#"print("0x{pc << 2:x8}:\t{instr:x8}\t{asm}{update}\n");"#,
    r#"print("{"0x{addr:x8}",indentation} +{replicate<N-2>("-")}+\n");"#,
    "//| doc\n//< doc\n/*| doc */\n/*< doc */\n// plain\n/* plain /* nested */ */",
    "template<typename T, uint32 N> class Foo : Bar<uint<N>> { public: [[schedule(1)]] void f(); };",
    "for (uint8 i : 10) { a[i] = b[i] << 2 >> 1 <= c ? ~d : !e ^^ f; }",
    "1.5e10 2.5E-3 0b_1010 0o777 42i32 305_441_741 1_ 007",
    "café ٣ Ωmega → \u{a0}x",
    "a\r\nb\r\n",
];

/// Lossless, and ranges that tile the input with no gaps, overlaps or empty
/// tokens. Every other test in this file rests on this holding.
#[track_caller]
fn check_lossless(input: &str) {
    let lexed = LexedStr::new(input);
    let round_trip: String = lexed.iter().map(|(_, t)| t).collect();
    assert_eq!(&round_trip, input, "round trip failed for {input:?}");
    let mut end = TextSize::new(0);
    for i in 0..lexed.len() {
        assert_eq!(lexed.range(i).start(), end, "gap before token {i} of {input:?}");
        assert!(lexed.range(i).end() > end, "empty token {i} of {input:?}");
        end = lexed.range(i).end();
    }
    assert_eq!(usize::from(end), input.len(), "trailing text lost in {input:?}");
}

#[test]
fn lossless() {
    for input in SNIPPETS {
        check_lossless(input);
    }
}

/// Truncation is where a scanner that trusts its lookahead falls over, so every
/// prefix of every snippet gets lexed too.
#[test]
fn every_prefix_lexes() {
    for input in SNIPPETS {
        for cut in 0..=input.len() {
            if input.is_char_boundary(cut) {
                check_lossless(&input[..cut]);
            }
        }
    }
}

/// A deterministic stand-in for a fuzzer: random strings over the characters
/// that steer the scanner. Asserts only totality and losslessness, which is
/// what has to hold for input the language does not even accept.
#[test]
fn random_input_is_total() {
    const ALPHABET: &[char] = &[
        '"', '\\', '{', '}', '/', '*', '\n', ' ', '\t', 'a', 'n', 'x', 'o', 'i', 'u', '0', '1',
        '8', '.', 'e', '-', '+', '_', '<', '>', ':', '&', '^', '$', 'é', '٣',
    ];
    // A 64-bit LCG; the constants are Knuth's MMIX. Fixed seed, so a failure
    // reproduces.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    let mut input = String::new();
    for _ in 0..20_000 {
        input.clear();
        let len = next() % 24;
        for _ in 0..len {
            input.push(ALPHABET[next() % ALPHABET.len()]);
        }
        check_lossless(&input);
    }
}

// ----------------------------------------------------------------- corpus

/// The reference compiler's own sources, used as the lexer's acceptance suite.
///
/// `doc/` holds no `.k` files today; it is walked anyway so new examples there
/// are picked up automatically. `tools/` is included for the same reason — it
/// is where the documentation-generator fixtures live.
const CORPUS_DIRS: &[&str] = &["library", "test", "doc", "tools"];

/// `test/syntax/imports/binary.k` is four bytes of `0xC0DEDEAD`: a fixture for
/// "the compiler was handed a non-source file". It is not UTF-8 and so cannot
/// even be handed to [`LexedStr::new`], whose input is `&str`.
const NOT_UTF8: &str = "binary.k";

/// The one corpus file that legitimately cannot lex clean, and why.
///
/// `test/syntax/*.k` are not standalone translation units: each is a series of
/// snippets separated by `expected:<exit code>` markers that the CMake harness
/// (`test/syntax/CMakeLists.txt`) feeds to the compiler one at a time. One
/// snippet in `basics.k` is *"c comment that isn't closed at the end of a
/// file"* with `expected:1`, so a whole-file lex necessarily swallows
/// everything after it into one unterminated block comment. Rather than skip
/// the file, the exact error is pinned here and
/// [`syntax_fixture_cases_lex_cleanly`] re-lexes the file snippet by snippet so
/// the swallowed remainder is still covered.
const EXPECTED_ERRORS: &[(&str, &str)] =
    &[("test/syntax/basics.k", "unterminated block comment")];

/// A corpus path relative to the corpus root, so failure output and
/// [`EXPECTED_ERRORS`] do not depend on where the checkout lives.
fn relative(path: &Path) -> String {
    path.strip_prefix(corpus_root())
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn corpus_root() -> PathBuf {
    kg_base::reference_root()
}

fn collect_k_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_k_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

/// Every `.k` file of the reference corpus, or `None` when it is not checked out
/// next to this workspace.
fn corpus() -> Option<Vec<PathBuf>> {
    let root = corpus_root();
    if !root.exists() {
        kg_base::corpus_absent(&format!("no corpus at {}; set KANAGAWA_ROOT", root.display()));
        return None;
    }
    let mut files = Vec::new();
    for dir in CORPUS_DIRS {
        collect_k_files(&root.join(dir), &mut files);
    }
    Some(files)
}

#[test]
fn corpus_lexes_cleanly() {
    let Some(files) = corpus() else { return };
    assert!(!files.is_empty(), "corpus root exists but holds no .k files");

    let mut lexed_count = 0usize;
    let mut skipped = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for path in &files {
        let name = relative(path);
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                failures.push(format!("{name}: unreadable: {e}"));
                continue;
            }
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(e) => {
                skipped.push(format!("{name}: not UTF-8: {e}"));
                continue;
            }
        };

        let lexed = LexedStr::new(text);
        lexed_count += 1;

        // (a) lossless
        let round_trip: String = lexed.iter().map(|(_, t)| t).collect();
        if round_trip != text {
            failures.push(format!("{name}: not lossless"));
        }
        // (b) no lex errors, bar the one documented fixture
        let allowed: Vec<&str> =
            EXPECTED_ERRORS.iter().filter(|(f, _)| *f == name).map(|(_, m)| *m).collect();
        let seen: Vec<&str> = lexed.errors().iter().map(|e| e.msg.as_str()).collect();
        if seen != allowed {
            for e in lexed.errors() {
                let at = lexed.range(e.token);
                failures.push(format!("{name}: lex error at {at:?}: {}", e.msg));
            }
            if seen.len() < allowed.len() {
                failures.push(format!("{name}: expected errors {allowed:?} but saw {seen:?}"));
            }
        }
        // (c) no LEX_ERROR tokens, and (d) no parser-only kinds
        for i in 0..lexed.len() {
            let kind = lexed.kind(i);
            if matches!(kind, LEX_ERROR | LTEQ | GTEQ | SHL | SHR) {
                failures.push(format!("{name}: {kind:?} token {:?} at {:?}", lexed.text(i), lexed.range(i)));
            }
        }
    }

    for s in &skipped {
        println!("skipped {s}");
    }
    assert_eq!(
        skipped.len(),
        1,
        "expected exactly the {NOT_UTF8} fixture to be skipped, got {skipped:#?}"
    );
    assert!(skipped[0].contains(NOT_UTF8), "unexpected skip: {:?}", skipped[0]);

    if !failures.is_empty() {
        let shown: Vec<_> = failures.iter().take(50).collect();
        panic!("{} corpus failure(s), first {}:\n{shown:#?}", failures.len(), shown.len());
    }

    println!("lexed {lexed_count} corpus files ({} found, {} skipped)", files.len(), skipped.len());
    assert_eq!(files.len(), 482, "the reference corpus changed size");
    assert_eq!(lexed_count, 481);
}

/// Splits a `test/syntax` fixture into the snippets the CMake harness compiles
/// one at a time, dropping the `expected:<exit code>` marker lines.
fn split_fixture_cases(text: &str) -> Vec<&str> {
    let mut cases = Vec::new();
    let mut case_start = 0usize;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if line.trim_end().starts_with("expected:") {
            cases.push(&text[case_start..offset]);
            case_start = offset + line.len();
        }
        offset += line.len();
    }
    cases.push(&text[case_start..]);
    cases.retain(|c| !c.trim().is_empty());
    cases
}

/// Lexes `test/syntax/*.k` the way the reference harness reads them — one
/// snippet at a time — which is the only way the ~78% of `basics.k` hidden
/// behind its deliberately unterminated comment gets covered at all.
///
/// The expected output is every deliberate lexical error in the whole
/// directory, and each one sits in a snippet the fixture marks `expected:1`:
/// the unclosed comment at the top of the file, two unterminated string
/// literals (`println("foobar);` and `const string str = "foobar;`), and a
/// `/*` nested inside a `/*` with only one closer. It should stay this short; a
/// new line here means the lexer started rejecting something the reference
/// accepts.
#[test]
fn syntax_fixture_cases_lex_cleanly() {
    let dir = corpus_root().join("test/syntax");
    if !dir.exists() {
        kg_base::corpus_absent(&format!("no corpus at {}; set KANAGAWA_ROOT", dir.display()));
        return;
    }
    let mut files = Vec::new();
    collect_k_files(&dir, &mut files);

    let mut reported = Vec::new();
    let mut case_count = 0usize;
    for path in &files {
        // `binary.k` is not UTF-8 and drops out here.
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        for (n, case) in split_fixture_cases(&text).into_iter().enumerate() {
            case_count += 1;
            let lexed = LexedStr::new(case);
            assert_eq!(
                lexed.iter().map(|(_, t)| t).collect::<String>(),
                case,
                "{} case {n} is not lossless",
                relative(path)
            );
            for e in lexed.errors() {
                reported.push(format!("{} case {n}: {}", relative(path), e.msg));
            }
            for i in 0..lexed.len() {
                assert!(
                    !matches!(lexed.kind(i), LEX_ERROR | LTEQ | GTEQ | SHL | SHR),
                    "{} case {n}: {:?} {:?}",
                    relative(path),
                    lexed.kind(i),
                    lexed.text(i)
                );
            }
        }
    }
    assert!(case_count > 1000, "only {case_count} fixture cases found");
    expect![[r#"
        test/syntax/basics.k case 82: unterminated block comment
        test/syntax/basics.k case 398: unterminated string literal
        test/syntax/basics.k case 400: unterminated string literal
        test/syntax/basics.k case 402: unterminated block comment"#]]
    .assert_eq(&reported.join("\n"));
}

#[test]
fn corpus_token_histogram() {
    let Some(files) = corpus() else { return };
    let mut histogram: BTreeMap<SyntaxKind, usize> = BTreeMap::new();
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let Ok(text) = std::str::from_utf8(&bytes) else { continue };
        let lexed = LexedStr::new(text);
        for i in 0..lexed.len() {
            *histogram.entry(lexed.kind(i)).or_default() += 1;
        }
    }
    let rendered =
        histogram.iter().map(|(k, n)| format!("{k:?} {n}\n")).collect::<String>();
    expect_histogram().assert_eq(&rendered);
}

fn expect_histogram() -> Expect {
    expect![[r#"
        WHITESPACE 253353
        LINE_COMMENT 7956
        BLOCK_COMMENT 305
        DOC_LINE_PRE 1280
        DOC_LINE_POST 551
        DOC_BLOCK_PRE 60
        DOC_BLOCK_POST 44
        INT_NUMBER 50901
        FLOAT_NUMBER 288
        STRING 776
        STRING_TEXT 571
        STRING_END 354
        IDENT 122584
        INT_TYPE_TOKEN 781
        UINT_TYPE_TOKEN 8457
        L_PAREN 31235
        R_PAREN 31234
        L_CURLY 24058
        R_CURLY 24058
        L_BRACK 7743
        R_BRACK 7743
        L_ANGLE 11618
        R_ANGLE 11499
        SEMICOLON 29294
        COMMA 48617
        COLON 6689
        COLON2 8188
        DOT 10507
        EQ 12437
        EQ2 3458
        BANG 637
        NEQ 383
        PLUS 2745
        MINUS 7386
        STAR 528
        SLASH 234
        PERCENT 250
        AMP 226
        AMP2 460
        PIPE 133
        PIPE2 323
        CARET 118
        CARET2 13
        TILDE 80
        QUESTION 599
        THIN_ARROW 1098
        BACKSLASH 17
        AS_KW 286
        ATOMIC_KW 537
        AUTO_KW 4425
        BARRIER_KW 27
        BITSIZEOF_KW 617
        BITOFFSETOF_KW 32
        BOOL_KW 2355
        BREAK_KW 353
        BYTESIZEOF_KW 59
        BYTEOFFSETOF_KW 10
        CAST_KW 1492
        CASE_KW 324
        CLASS_KW 1697
        CLOG2_KW 112
        CONCAT_KW 212
        CONST_KW 3133
        DECLTYPE_KW 240
        DEFAULT_KW 55
        DO_KW 171
        ELSE_KW 471
        ENUM_KW 112
        EXPORT_KW 266
        EXTERN_KW 75
        FALSE_KW 1702
        FAN_OUT_KW 3
        FLOAT32_KW 492
        FOR_KW 738
        IF_KW 1401
        IMPORT_KW 1069
        INLINE_KW 4855
        INT_KW 189
        LUTMUL_KW 21
        MODULE_KW 327
        MUX_KW 434
        NOINLINE_KW 36
        PRIVATE_KW 797
        PUBLIC_KW 1408
        REORDER_KW 8
        RETURN_KW 2751
        STATIC_KW 2593
        STATIC_ASSERT_KW 6
        STRING_KW 203
        STRUCT_KW 419
        SWITCH_KW 68
        TEMPLATE_KW 1472
        TRUE_KW 1478
        TYPENAME_KW 1425
        UINT_KW 486
        UNION_KW 41
        USING_KW 546
        VOID_KW 4663
        WHILE_KW 171
    "#]]
}

/// Regression: megaparsec's `L.float` has a second branch, `decimal_` followed
/// by a bare exponent, so a float needs no fractional part
/// (`Text.Megaparsec.Char.Lexer.float`, reached from `Parser/Lexer.hs:158-159`).
/// Missing it silently turned `1e5` into an integer followed by an identifier.
#[test]
fn exponent_only_floats_are_floats() {
    for src in ["1e5", "1E5", "1e+5", "1e-5", "0e5", "2e10", "1.5e10", "1.5E-3", "12."] {
        let lexed = LexedStr::new(src);
        let expected = if src == "12." { SyntaxKind::INT_NUMBER } else { SyntaxKind::FLOAT_NUMBER };
        assert_eq!(lexed.kind(0), expected, "{src:?} lexed as {:?}", lexed.kind(0));
    }

    // A dangling exponent is not part of the number: `1e` is `1` then `e`.
    let lexed = LexedStr::new("1e");
    assert_eq!(lexed.kind(0), SyntaxKind::INT_NUMBER);
    assert_eq!(lexed.text(0), "1");
    assert_eq!(lexed.kind(1), SyntaxKind::IDENT);
    assert_eq!(lexed.text(1), "e");

    // Hex must not be mistaken for an exponent float.
    for src in ["0x1e5", "0xE5", "0b1e5", "0o7e2"] {
        let lexed = LexedStr::new(src);
        assert_eq!(lexed.kind(0), SyntaxKind::INT_NUMBER, "{src:?}");
    }
    assert_eq!(LexedStr::new("0x1e5").text(0), "0x1e5");
    // `0b1e5` is the binary literal `0b1`, then the identifier `e5`.
    assert_eq!(LexedStr::new("0b1e5").text(0), "0b1");
    assert_eq!(LexedStr::new("0b1e5").kind(1), SyntaxKind::IDENT);
}
