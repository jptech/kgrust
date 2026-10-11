//! The interpolated string's format, as the reference's *parser* reads it
//! (`Parser.hs:886-903`) — execution-order pass 0, before pass 1's
//! checkpoint — and the scanner that finds a string's holes.
//!
//! Pass 18, `interpolatedStringPost` (`Frontend.hs:57`, `:241-271`), was
//! this module's until sprint W3 step 1 (2026-10-10): it is the loop's now,
//! `kg_sem::check`'s `interpolated_strings`, which reads each hole's type
//! where this pass scanned the string's text and declined every hole but a
//! literal or a name it could type. Its verdicts at a module's text are
//! admitted in its place (`passes::FromLoop`); the text door, which runs no
//! loop, has none. What is left here is what the *parser* rejects:
//!
//! ```haskell
//! fe <- optional $ between (L.symbol "{") (char '}') formatedExpr
//! formatedExpr = do
//!     e <- expression
//!     n <- optional equal
//!     a <- optional (comma >> expression)
//!     f <- optional (colon >> format)
//! format = L.lexeme $ Format <$> choiceViaShow [Bin, Oct, Dec, Hex, HexUpper] <*> optional L.decimal
//! ```
//!
//! A body containing `<` or `?` is not split — the template-argument `<…>`
//! and the ternary `? :` (`Parser.hs:959`) make a depth-0 comma or colon
//! ambiguous without an expression parser — so its format, if any, is not
//! judged here: silence where the split would be a guess.

use std::ops::Range;

use kg_base::{TextRange, TextSize};
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, ast};

use crate::sink::Sink;

/// The `format` grammar's two rejections, raised as the parse errors they
/// are (`Parser.hs:895,900-903`): `Format <$> choiceViaShow [Bin, Oct, Dec,
/// Hex, HexUpper] <*> optional L.decimal`, then `char '}'`. `{x=:}` is
/// "unexpected '}' expecting 'X', 'b', 'd', 'o', or 'x'", `{x=:d 10}` is
/// "unexpected '1' expecting '}'" — each at the offending character, where
/// the reference reports it (`interpolated-string.k::8,13`, `kg-oracle
/// --errors` 2026-09-10). Runs before pass 1's checkpoint, since
/// a parse error keeps the reference out of the frontend altogether.
pub(crate) fn pass_0_interpolation_format(root: &ast::SourceFile, d: &mut Sink) {
    for string in outermost_strings(root.syntax()) {
        let text = &string.text().to_string();
        let base = string.text_range().start();
        for body in interpolations(text) {
            let body_text = &text[body.clone()];
            let Some(scan) = scan(body_text) else { continue };
            let Some(colon) = scan.colon else { continue };
            let Some((offset, message)) = format_error(&body_text[colon + 1..]) else { continue };
            let at = base + TextSize::from((body.start + colon + 1 + offset) as u32);
            d.error(TextRange::new(at, at + TextSize::from(1)), message);
        }
    }
}

/// Where `format` (`Parser.hs:900-903`) then `char '}'` (`:895`) first fail
/// on the text after the colon: the byte offset of the unexpected
/// character (the body's end standing for the `}`) and megaparsec's
/// message for it.
fn format_error(after: &str) -> Option<(usize, String)> {
    let bytes = after.as_bytes();
    let mut i = 0;
    // `colon` is a lexeme: the whitespace before the specifier is its.
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    match bytes.get(i) {
        Some(b'b' | b'o' | b'd' | b'x' | b'X') => i += 1,
        other => {
            let shown = other.map_or_else(|| "}".to_string(), |b| (*b as char).to_string());
            return Some((i, format!("unexpected '{shown}'\nexpecting 'X', 'b', 'd', 'o', or 'x'")));
        }
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // `L.lexeme` eats the whitespace after the precision; `char '}'` does
    // not skip anything else.
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    (i < bytes.len()).then(|| (i, format!("unexpected '{}'\nexpecting '}}'", bytes[i] as char)))
}

/// Every `INTERPOLATED_STRING` not inside another one's hole, in source
/// order. The scanner below finds the holes of a string at every depth
/// ([`interpolations`]), so a nested string is read as part of its outer
/// one's text — once — as it was when the whole string was one token.
///
/// Only a string with a hole is an `INTERPOLATED_STRING`: one without is a
/// plain `STRING` token (`kg_parser::lexer`), and `{{` is the escape, so no
/// hole exists without one of these nodes.
pub(crate) fn outermost_strings(root: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> + use<> {
    let mut preorder = root.preorder();
    std::iter::from_fn(move || {
        while let Some(event) = preorder.next() {
            if let kg_syntax::WalkEvent::Enter(node) = event
                && node.kind() == SyntaxKind::INTERPOLATED_STRING
            {
                preorder.skip_subtree();
                return Some(node);
            }
        }
        None
    })
}

// ---------------------------------------------------------------------------
// the scanner
// ---------------------------------------------------------------------------

/// Which construct the scanner is inside — the lexer's own `Frame`
/// (`kg_parser::lexer`), which is itself `Parser.hs:883-893` read as a state
/// machine.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// Between quotes: escapes apply, a single `{` opens an interpolation.
    Str,
    /// Inside `{ … }`: expression text, where `"` opens a nested string and
    /// braces nest (initializer lists).
    Expr,
}

/// Byte ranges of every interpolation body in one whole string token, at every
/// nesting depth, in source order.
///
/// `pub`, re-exported as [`crate::interpolations`], for the one consumer
/// outside this pass: the language server's rename, which must know where a
/// string names something the parser never split out. One scanner, so the two
/// cannot disagree about what a hole is.
///
/// Nesting matters: `test/syntax/interpolated-string.k:45` is
/// `"{stringFun("{boolConst:x}")}"`, whose rejection belongs to the *inner*
/// interpolation. Because the stack is flat, one pass finds interpolations at
/// every depth and each carries its own absolute range, which is why the
/// diagnostics land on the right column without recursion.
///
/// A `{` seen in [`Frame::Expr`] is an initializer list, **not** an
/// interpolation — that is the whole difference between `"{ {1, 0} }"`, which is
/// one interpolation holding an initializer list, and two nested ones.
pub fn interpolations(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut frames = vec![Frame::Str];
    // `(is_interpolation, body_start)` for each open brace.
    let mut open: Vec<(bool, usize)> = Vec::new();
    let mut out = Vec::new();
    // The token always starts at its opening quote; `Frame::Str` is already on
    // the stack for it.
    let mut i = 1;

    while i < bytes.len() {
        let Some(&frame) = frames.last() else { break };
        match (frame, bytes[i]) {
            (Frame::Str, b'"') => {
                i += 1;
                frames.pop();
            }
            // Only the *extent* matters here. Skipping the backslash and one
            // more byte is enough to keep a `\"` from ending the string early;
            // the numeric and control escapes `Parser.hs:892` accepts are longer
            // but contain neither `"` nor `{`.
            (Frame::Str, b'\\') => i += 2,
            (Frame::Str, b'{') if bytes.get(i + 1) == Some(&b'{') => i += 2,
            (Frame::Str, b'{') => {
                i += 1;
                frames.push(Frame::Expr);
                open.push((true, i));
            }
            (Frame::Expr, b'"') => {
                i += 1;
                frames.push(Frame::Str);
            }
            (Frame::Expr, b'{') => {
                i += 1;
                frames.push(Frame::Expr);
                open.push((false, i));
            }
            (Frame::Expr, b'}') => {
                if let Some((true, start)) = open.pop() {
                    out.push(start..i);
                }
                i += 1;
                frames.pop();
            }
            // Every decision above is on ASCII, and a multi-byte character can
            // only be ordinary text, so advancing by one byte cannot land the
            // scanner on a boundary that matters. Ranges are still byte ranges
            // into a `&str` and are only ever sliced at ASCII positions.
            _ => i += 1,
        }
    }
    out
}

/// The depth-0 `:` of one body — `formatedExpr`'s separator before the
/// format (`Parser.hs:894-899`).
struct Scan {
    colon: Option<usize>,
}

/// Locates [`Scan`]'s separator, or declines the body.
fn scan(body: &str) -> Option<Scan> {
    // Conservative on purpose (the module docs) — a `<` inside a nested
    // string cannot confuse the split, but excluding it costs only silence
    // and the test for "at depth 0, outside a string" would have to be
    // right for free.
    if body.contains('<') || body.contains('?') {
        return None;
    }

    let bytes = body.as_bytes();
    let mut depth = 0usize;
    let mut colon: Option<usize> = None;
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'"' => i = string_end(body, i),
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                i += 1;
            }
            // `a::b` is one `ScopedNameF`, not a format spec.
            // `logic/context_saver_depth.k` interpolates `{opt::stall}` and
            // `logic/string.k` interpolates `{Color::Red:b}`.
            b':' if bytes.get(i + 1) == Some(&b':') => i += 2,
            b':' if depth == 0 => {
                if colon.is_none() {
                    colon = Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return None;
    }
    Some(Scan { colon })
}

/// The index just past the string literal that starts at `at`, which must be a
/// `"`. Uses the same rules as [`interpolations`] so the two agree about where a
/// nested string ends.
fn string_end(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();
    let mut frames = vec![Frame::Str];
    let mut i = at + 1;
    while i < bytes.len() {
        let Some(&frame) = frames.last() else { break };
        match (frame, bytes[i]) {
            (Frame::Str, b'"') => {
                i += 1;
                frames.pop();
            }
            (Frame::Str, b'\\') => i += 2,
            (Frame::Str, b'{') if bytes.get(i + 1) == Some(&b'{') => i += 2,
            (Frame::Str, b'{') | (Frame::Expr, b'{') => {
                i += 1;
                frames.push(Frame::Expr);
            }
            (Frame::Expr, b'"') => {
                i += 1;
                frames.push(Frame::Str);
            }
            (Frame::Expr, b'}') => {
                i += 1;
                frames.pop();
            }
            _ => i += 1,
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use crate::tests::program;

    const H: &str = "module h { intConst, boolConst, stringFun, id }\n\nconst auto intConst = 0xF00;\nconst auto boolConst = true;\n\ninline auto stringFun(string x)\n{\n    return x;\n}\n\ntemplate <typename T>\ninline auto id(T x)\n{\n    return x;\n}\n";

    fn whole(main: &str) -> Vec<String> {
        program(&[("main.k", main), ("h.k", H)])
            .into_iter()
            .map(|(m, at, _)| format!("{m} @ {at}"))
            .collect()
    }

    /// The names `interpolated-string.k`'s holes read, declared as there,
    /// then `const auto s = "<body>";` — pass 18's verdicts at the rooted
    /// door, the loop's since its move (`kg_sem::check`).
    fn hole(body: &str) -> Vec<String> {
        let main = format!(
            "import h\n\nenum Color : uint2\n{{\n    Red,\n    Green\n}}\n\nconst auto enumConst = Color::Red;\nconst auto floatConst = 1.0;\n\
             inline uint32 intFun(uint32 x)\n{{\n    return x;\n}}\n\ninline void use_h()\n{{\n    auto b = boolConst;\n    auto f = stringFun(\"x\");\n}}\n\n\
             const auto s = \"{body}\";\n"
        );
        whole(&main)
    }

    #[track_caller]
    fn silent(body: &str) {
        let errs = hole(body);
        assert!(errs.is_empty(), "expected silence for {body:?}, got {errs:?}");
    }

    /// A plain function's body does not see the function: `{boolConst:b3}`
    /// inside `inline void boolConst()` formats the imported `bool`, which the
    /// reference accepts (difffuzz, 2026-09-24).
    #[test]
    fn a_hole_naming_its_own_function_is_not_the_function() {
        assert_eq!(whole("import h\n\ninline void boolConst()\n{\n    auto s = \"{boolConst:b3}\";\n}\n"), Vec::<String>::new());
    }

    /// The named clauses, each a corpus shape probed against `kg-oracle
    /// --errors` (2026-09-10): a format on a `const bool` (`::40`), on a
    /// call returning `string` (`::44`) and inside a nested string (`::45`);
    /// a class, an array of one, and a `void` call as the value (`::48,49,52`).
    /// The note is the expression's own.
    #[test]
    fn a_named_interpolation_is_checked_against_its_declared_type() {
        assert_eq!(
            whole("import h\n\nconst auto s = \"{boolConst:x}\";\n"),
            vec!["Invalid format specifier `x` for the value of type `const bool` @ main.k:boolConst"]
        );
        assert_eq!(
            whole("import h\n\ninline void foo()\n{\n    const auto s = \"{stringFun(\"{boolConst:b3}\"):d}\";\n}\n"),
            vec!["Invalid format specifier `d` for the value of type `string` @ main.k:stringFun(\"{boolConst:b3}\")"]
        );
        assert_eq!(
            whole("import h\n\ninline void foo()\n{\n    const auto s = \"{stringFun(\"{boolConst:x}\")}\";\n}\n"),
            vec!["Invalid format specifier `x` for the value of type `const bool` @ main.k:boolConst"]
        );
        assert_eq!(
            whole("class C{}\n\ninline void foo()\n{\n    static C c;\n    const auto s = \"{c}\";\n}\n"),
            vec!["The type of interpolation expression `C` is not a value type @ main.k:c"]
        );
        assert_eq!(
            whole("class C{}\n\ninline void foo()\n{\n    static C[2] c;\n    const auto s = \"{c}\";\n}\n"),
            vec!["The type of interpolation expression `C[2]` is not a value type @ main.k:c"]
        );
        assert_eq!(
            whole("inline void foo()\n{\n}\n\nconst auto s = \"{foo()}\";\n"),
            vec!["The type of interpolation expression `void` is not a value type @ main.k:foo()"]
        );
        // The accepting shapes with their real types behind them.
        assert_eq!(whole("import h\n\nconst auto s = \"{intConst:x} {boolConst:b3} {stringFun(\"a\")} {intConst,intConst}\";\n"), Vec::<String>::new());
    }

    /// A hole's bare name is pass 9's (`interpolated-string.k::32,33`): in
    /// the expression, and in the alignment.
    #[test]
    fn an_undeclared_name_in_a_hole_is_not_in_scope() {
        // Nothing the import supplies is used, so the parser's
        // `checkRedundantImports` warns as well (`Parser.hs:234-241`).
        assert_eq!(
            whole("import h\n\nconst auto s = \"{intConst1}\";\n"),
            vec![
                "The import is redundant @ main.k:import",
                "Symbol name not in scope: `intConst1`\nPerhaps you meant: `intConst` defined in h @ main.k:intConst1",
            ]
        );
        assert_eq!(
            whole("import h\n\nconst auto s = \"{intConst,intConst1}\";\n"),
            vec!["Symbol name not in scope: `intConst1`\nPerhaps you meant: `intConst` defined in h @ main.k:intConst1"]
        );
    }

    /// `format` failing is a parse error at the offending character
    /// (`interpolated-string.k::8,13`; `Parser.hs:895,900-903`), and the
    /// only error: pass 18 never runs past it, so the hole is not also
    /// judged under a format the grammar refused.
    #[test]
    fn a_format_the_grammar_rejects_is_a_parse_error_at_the_character() {
        assert_eq!(
            whole("import h\n\nconst auto s = \"{intConst=:}\";\n"),
            vec!["unexpected '}'\nexpecting 'X', 'b', 'd', 'o', or 'x' @ main.k:}"]
        );
        assert_eq!(
            whole("import h\n\nconst auto s = \"{intConst=:d 10}\";\n"),
            vec!["unexpected '1'\nexpecting '}' @ main.k:1"]
        );
        assert_eq!(hole("{ {1, 0} :d 10}"), vec!["unexpected '1'\nexpecting '}' @ main.k:1"]);
        // Legal spellings, for contrast (`:9-12`, all `expected:0`).
        silent("{intConst=:d}");
        silent("{intConst=: d}");
        silent("{intConst=:d10}");
        silent("{intConst=:d10 }");
    }

    /// `interpolated-string.k:34` — `"{intConst,intFun(intConst)}"`,
    /// `expected:1`: the alignment is a call, which `Constexpr.hs` never
    /// folds, so `isIntLiteral` is false (`Frontend.hs:259-261`). `:35`,
    /// the two swapped, is `expected:0`: a call is a good thing to print.
    #[test]
    fn an_alignment_that_is_a_call_is_rejected() {
        assert_eq!(
            hole("{intConst,intFun(intConst)}"),
            vec!["Interpolated string alignment did not evaluate to an integer constant @ main.k:intFun(intConst)"]
        );
        silent("{intFun(intConst),intConst}");
    }

    /// `interpolated-string.k:46-47`, both `expected:1`. `TInitializer` is
    /// absent from `tIsValueType` (`Type.hs:686`), and the message carries the
    /// reference's own `showPretty`: `braces (sepBy comma ts)` over the element
    /// types (`Type.hs:383`). The note is the expression's, from its first
    /// character (`kg-oracle --errors` prints column 19 for `"{ {} }"`).
    #[test]
    fn an_initializer_list_is_not_a_value_type() {
        assert_eq!(hole("{ {} }"), vec!["The type of interpolation expression `{}` is not a value type @ main.k:{}"]);
        assert_eq!(
            hole("{ {1, 0} }"),
            vec!["The type of interpolation expression `{uint1, uint1}` is not a value type @ main.k:{1, 0}"]
        );
    }

    /// The other half of `validateFormat` (`Frontend.hs:266-270`). A `bool`
    /// is legal with `b` and with nothing else (`interpolated-string.k:
    /// 40-42`); a `float` with every specifier (`:39`, `expected:0`).
    #[test]
    fn a_format_specifier_is_checked_against_a_literal_type() {
        assert_eq!(hole("{true:x}"), vec!["Invalid format specifier `x` for the value of type `bool` @ main.k:true"]);
        silent("{true:b}");
        silent("{true:b3}");
        assert_eq!(
            hole("{\"abc\":d}"),
            vec!["Invalid format specifier `d` for the value of type `string` @ main.k:\"abc\""]
        );
        silent("{0xf00:x}");
        silent("{3.14:x}");
        // No specifier: `string` is a value type (`Type.hs:686`).
        silent("{\"abc\"}");
    }

    /// Every interpolation `test/syntax/interpolated-string.k` marks
    /// `expected:0` that reaches a clause, with the names it reads declared:
    /// the direction a false positive breaks.
    #[test]
    fn the_accepting_corpus_shapes_stay_silent() {
        for body in [
            "{intConst}",
            " {intConst} ",
            "{ intConst }",
            "{ intConst= }",
            "{intConst=}",
            "{ intConst = }",
            "{intConst=,10}",
            "{intConst=, 10}",
            "foo{intConst + 3}",
            "foo{ intConst + 3 }",
            "foo{intConst+3=}",
            "foo{intFun(intConst)+3}",
            "foo{intConst + 3:d4}",
            "foo{intConst + 3:b32}",
            "foo{intConst + 3:o10}",
            "foo{intConst + 3:x8}",
            "foo{intConst + 3:X8}",
            "{intConst=,intConst}",
            "{enumConst:x}",
            "{floatConst:x}",
            "{boolConst:b3}",
            "{stringFun(\"x\")}",
            "{Color::Red:b}",
            "{intConst + 3,2}",
            "{intConst + 3,intConst + 2}",
            "{intConst=,intConst / 10}",
            "{intConst,-7}",
            "{intConst,clog2(intConst)}",
            "{intConst,bitsizeof uint8}",
            "{intConst,(intConst + 2)}",
            "{ cast<uint32[2]>({0, 1}) }",
        ] {
            silent(body);
        }
    }

    /// The scanner's own obligations, each one a corpus shape.
    ///
    /// `{{` is an escaped brace and opens nothing (`Parser.hs:892-893`), so
    /// `interpolated-string.k:36` (`"{{}"`, `expected:0`) has no interpolation
    /// at all — and a scanner that thought otherwise would read a body of `}`.
    #[test]
    fn the_scanner_finds_the_interpolations_the_grammar_defines() {
        use super::interpolations;

        assert_eq!(interpolations("\"{{}\""), Vec::<std::ops::Range<usize>>::new());
        // `"{{}"` is silent either way — with the escape arm gone the inner
        // `{` opens a *nested* brace whose `}` closes it, and nothing is
        // recorded. `"{{x}}"` is what separates them: without the arm the
        // second `}` closes an interpolation that the grammar never opened.
        assert_eq!(interpolations("\"{{x}}\""), Vec::<std::ops::Range<usize>>::new());
        assert_eq!(interpolations("\"no braces here\""), Vec::<std::ops::Range<usize>>::new());
        // One interpolation whose body is an initializer list, not two.
        assert_eq!(interpolations("\"{ {1, 0} }\""), vec![2..10]);
        // `interpolated-string.k:45`, `"{stringFun("{boolConst:x}")}"` — the
        // inner string's interpolation is found too, and it is the one the
        // reference rejects.
        let nested = "\"{stringFun(\"{boolConst:x}\")}\"";
        assert_eq!(
            interpolations(nested).iter().map(|r| &nested[r.clone()]).collect::<Vec<_>>(),
            vec!["boolConst:x", "stringFun(\"{boolConst:x}\")"]
        );
        // An escaped quote must not end the string early.
        assert_eq!(interpolations("\"a \\\" {1,f()} b\"").len(), 1);
    }
}
