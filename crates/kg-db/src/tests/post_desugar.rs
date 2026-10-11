//! Pass 16, `postDesugar` (`Frontend.hs:176-183`), at the rooted door —
//! the loop's at a module's text since sprint W3 step 1
//! (`kg_sem::check::TextPass::PostDesugar`). These were the CST pass's own
//! tests, at the text door, which runs no loop and has no pass 16 now. Each
//! is a corpus shape or a `kg-oracle --errors` probe the CST pass recorded;
//! where the rooted door reads a case otherwise, the test says why.

use kg_diag::Diagnostic;

use crate::tests::program;

/// Every diagnostic the rooted door gives `main.k`, alone under `/proj`
/// with no base library.
fn check(text: &str) -> Vec<Diagnostic> {
    use kg_base::Utf8PathBuf;
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
    db.check(&[main])
}

/// Every error message for `main.k`.
fn errors(text: &str) -> Vec<String> {
    check(text).into_iter().filter(Diagnostic::is_error).map(|x| x.message).collect()
}

#[track_caller]
fn accepts(text: &str) {
    let errs = errors(text);
    assert!(errs.is_empty(), "expected a clean compile of\n{text}\ngot {errs:?}");
}

#[track_caller]
fn rejects(text: &str) {
    assert!(!errors(text).is_empty(), "expected a rejection of\n{text}");
}

/// `decltype` of a typed variable or call is a type the fold compares
/// (`Constexpr.hs:223-250`): a `const uint32 v` reads `uint32` — the
/// `const` is the variable's, not the type's — a local `uint32 v` is
/// not `!= uint32`, and `x()` over `inline uint32 x()` is `uint32`
/// (`kg-oracle --errors`, 2026-09-10, `scratchpad/sc/dt5-8`;
/// `type-constexpr.k::49`).
#[test]
fn decltype_of_a_variable_or_a_call_compares_as_its_type() {
    rejects("const uint32 v = 1;\nstatic assert(!(decltype(v) == uint32));\n");
    accepts("const uint32 v = 1;\nstatic assert(decltype(v) == uint32);\n");
    rejects("inline void main()\n{\n    uint32 v = 1;\n    static assert(decltype(v) != uint32);\n}\n");
    rejects("inline uint32 x()\n{\n    return 1;\n}\n\nstatic assert(!(decltype(x()) == uint32));\n");
    rejects("inline uint32 x()\n{\n    return {};\n}\n\nstatic assert(!(decltype(x()) == decltype(x())));\n");
}

/// The verdict sits at the statement — `desugarError` renotes the
/// `StaticIf` node — not at the predicate (`static-if.k::39`).
#[test]
fn an_unresolved_static_if_is_reported_at_the_statement() {
    let out = program(&[("main.k", "const auto Ten = 10;\n\nstatic if (Ten)\n{\n    const uint32 x = 0;\n}\n")]);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].0, "static if predicate did not evaluate to a boolean constant");
    assert!(out[0].1.starts_with("main.k:static if (Ten)"), "{}", out[0].1);
}

/// `kg-oracle --errors`, 2026-09-27: a bare-`auto` function whose first
/// return-bearing statement is a `static if` asking the function's own
/// type is the predicate's error at `static`, and pass 17's at `auto`
/// before it (`type-constexpr.k::37`). Nested under a runtime `if`, or
/// after a `return`, the `static if` is not the first; with a written
/// return type the type is known — all three compile. Red with
/// the CST pass's `asks_the_type_it_decides` cut. At the rooted door since
/// pass 17 is the loop's (sprint W3 step 1): the text door runs no loop
/// and has no pass 17.
#[test]
fn a_static_if_asking_the_type_it_decides_is_no_constant() {
    let text = "inline auto main()\n{\n    static if (decltype(main) == () -> bool)\n    {\n        return true;\n    }\n    else\n    {\n        return 10;\n    }\n}\n";
    let got = program(&[("main.k", text)]);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!((got[0].0.as_str(), got[0].1.as_str()), ("Could not deduce function return type", "main.k:auto"));
    assert_eq!(got[1].0, "static if predicate did not evaluate to a boolean constant");
    assert!(got[1].1.starts_with("main.k:static if (decltype(main)"), "{got:?}");
    let messages: Vec<String> =
        program(&[("main.k", "inline auto main()\n{\n    static if (bitsizeof(decltype(main())) == 1)\n    {\n        return true;\n    }\n}\n")])
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
    assert_eq!(messages, ["Could not deduce function return type", "static if predicate did not evaluate to a boolean constant"]);
    accepts("inline auto main()\n{\n    if (true)\n    {\n        static if (decltype(main) == () -> bool)\n        {\n            return true;\n        }\n    }\n    return 1;\n}\n");
    accepts("inline auto main()\n{\n    return 1;\n    static if (decltype(main) == () -> bool)\n    {\n    }\n}\n");
    accepts("inline uint4 main()\n{\n    static if (decltype(main) == () -> bool)\n    {\n        return true;\n    }\n    else\n    {\n        return 10;\n    }\n}\n");
}


/// `static_assert(x == 1)` over a function's *parameter* is a clean
/// compile for the reference (`test/logic/const.k:17-22`): the legacy
/// spelling is `StaticAssertLegacy` (`Parser.hs:571-573`), a node
/// `staticAssert` never matches, left for the backend to check per
/// call. The statement `static assert` over a parameter is folded where
/// it is written and is this error (`constexpr.k::145`, where `Run` is
/// never called), as over a local mutable (`basics.k::67`). Red with
/// the legacy skip cut, and red with the old parameter hedge back.
#[test]
fn only_the_static_assert_statement_is_folded_where_it_stands() {
    accepts("inline uint32 Helper(uint32 x)\n{\nstatic_assert(x == 1);\n\nreturn x;\n}\n\ninline void main()\n{\nHelper(1);\n}\n");
    let not_constant = vec!["static assertion expression did not evaluate to a boolean constant"];
    assert_eq!(errors("inline void main()\n{\nbool b;\n\nstatic assert(b);\n}\n"), not_constant);
    assert_eq!(errors("inline void Run(uint64 i)\n{\nstatic assert(i == 0);\n}\n"), not_constant);
}

/// A width that no later pass can fold: literals an arm of the fold
/// lacks (`1 && 1` over integers, `false >= true` over booleans,
/// `Constexpr.hs:176-216`; `constexpr.k::122,125`), or a mutable name.
/// `bitsizeof` of a mutable reads its type and folds (`::116`). Red with
/// `only_literals` cut; the last line red with the root of a width read
/// as sitting under its own `uint<…>`.
#[test]
fn a_width_that_can_never_fold_did_not_evaluate() {
    let width = |w: &str| format!("inline void main()\n{{\n    uint8 m;\n    uint<{w}> x;\n}}\n");
    let not_positive = vec!["uint width did not evaluate to a positive integer constant"];
    assert_eq!(errors(&width("1 && 1 && 1")), not_positive);
    assert_eq!(errors(&width("false >= true == true")), not_positive);
    assert_eq!(errors(&width("m")), not_positive);
    accepts(&width("bitsizeof m"));
}

/// A predicate or an assertion that no later pass can fold: a size of
/// `void`, which has none (`Type.hs:700`; `constexpr.k::154,155`),
/// literals an arm of the fold lacks, and a size folded over the type
/// table to an integer where a boolean is due. Red with `only_literals`
/// cut from either check, or the typed fold cut from `static if`.
#[test]
fn a_predicate_that_can_never_fold_did_not_evaluate() {
    let si = |c: &str| format!("inline void main()\n{{\n    static if ({c})\n    {{\n    }}\n}}\n");
    let predicate = vec!["static if predicate did not evaluate to a boolean constant"];
    assert_eq!(errors(&si("bitsizeof void")), predicate);
    assert_eq!(errors(&si("bitsizeof uint8")), predicate);
    accepts(&si("bitsizeof uint8 == 8"));
    assert_eq!(
        errors("inline void main()\n{\n    static assert(1 && 1);\n}\n"),
        vec!["static assertion expression did not evaluate to a boolean constant"]
    );
    // A negative shift crashes the reference, with no `Error` line
    // (`basics.k::4,5`): no verdict here either.
    accepts("inline void main()\n{\n    static assert((3 << -1) == 1);\n}\n");
}

/// `clog2` of a runtime value never folds (`constexpr.k::138`), except
/// as a call's argument, where the callee speaks first (`basics.k::20`);
/// and a width whose fold errors is that error alone — pass 6's, whose
/// checkpoint stops pass 16 (`::124`).
#[test]
fn a_runtime_clog2_and_an_erring_width() {
    assert_eq!(
        errors("inline void main()\n{\n    int32 x;\n    if (clog2(x) == 2)\n    {\n    }\n}\n"),
        vec!["clog2 argument did not evaluate to a positive integer constant"]
    );
    assert_eq!(
        errors("inline void main()\n{\n    uint<2 * bytesizeof 1> x;\n}\n"),
        vec!["bytesizeof is undefined for a type with bit size of 1"]
    );
}

/// The absorbing identities fold a runtime operand away by width
/// (`evalBinaryInt`/`evalBinaryBool`, `Constexpr.hs:229-242`), and the
/// typed fold decides them where every operand's width is trusted:
/// `uint64 i & (const uint32)0` is not constexpr (`64 <= 32` is false,
/// `constexpr.k::145`), `uint29 i | 0x2fffffff` is not (bit 28 is clear,
/// `::150`), `i & 0` under a `uint1`, `i | 0xff` under a `uint8` and
/// `b && false` are (`::139-149`, `expected:0`).
#[test]
fn an_absorbing_operator_over_a_runtime_name_is_decided_by_width() {
    let not_constant = vec!["static assertion expression did not evaluate to a boolean constant"];
    let body = |params: &str, pre: &str, e: &str| {
        format!("inline void Run({params})\n{{\n{pre}static assert({e});\n}}\n")
    };
    assert_eq!(errors(&body("uint64 i", "const uint32 x = 0;\n", "(i & x) == 0")), not_constant);
    assert_eq!(errors(&body("uint29 i", "const uint32 x = 0x2fffffff;\n", "(i | x) == x")), not_constant);
    accepts(&body("uint1 i", "", "(i & 0) == 0"));
    accepts(&body("uint8 i", "", "(i | 0xff) == 0xff"));
    accepts(&body("bool b", "", "!(b && false)"));
}

/// A `clog2` in a mux case the selector does not take is gone before
/// `unresolved` runs (`Constexpr.hs:311-315`): the reference accepts
/// both (`kg-oracle --errors`, 2026-09-18, `scratchpad/fold/c.k`), and
/// rejects the taken one.
#[test]
fn a_clog2_in_a_dead_mux_case_is_not_unresolved() {
    accepts("const auto k = (0 > 1) ? clog2(0) : 1;\n");
    accepts("const auto j = mux(0, 2, clog2(0));\n");
    rejects("const auto k = (1 > 0) ? clog2(0) : 1;\n");
}

/// `type-check.k::1-3` (`kg-oracle --errors`, 2026-09-08): a width
/// with a type error inside cannot fold, so pass 16's verdict stands
/// at the declaration. The error inside goes with the width it was
/// in — `desugarError` replaced both — and this test said it stayed
/// until 2026-10-10, when the oracle on this very program printed the
/// width's error alone.
#[test]
fn a_width_that_cannot_fold_is_reported_at_the_declaration() {
    let text = "using foo = uint5;\n\ninline void main()\n{\n    int<bitsizeof (foo + 1)> bar;\n}\n";
    let out = check(text);
    assert_eq!(out[0].message, "int width did not evaluate to a positive integer constant", "{out:?}");
    assert!(text[out[0].range.range].starts_with("int<"));
    assert_eq!(out.len(), 1, "nothing inside the replaced width is reported: {out:?}");
}

/// `type-check.k::4,14,15`: a type where a width wants a value never
/// folds, so pass 16 replaces the width before pass 19 could call the
/// type "Invalid use of a type as an expression" — the reference prints
/// the width's error, at the `int`, and nothing else (`kg-oracle
/// --errors`, 2026-10-10). Under a mux the type may be the case
/// dropped: `int<(1 ? 8 : int32)>` compiles there, so no verdict — and
/// none from pass 19, which never meets the arm the fold deleted.
#[test]
fn a_type_in_a_width_is_the_widths_error() {
    let decl = |d: &str| format!("using foo = uint5;\n\ninline void main()\n{{\n    {d}\n}}\n");
    let width = |d: &str| {
        let text = decl(d);
        check(&text)
            .into_iter()
            .filter(|x| x.is_error())
            .map(|x| (x.message, text[x.range.range].to_owned()))
            .collect::<Vec<_>>()
    };
    let int = "int width did not evaluate to a positive integer constant".to_owned();
    let uint = "uint width did not evaluate to a positive integer constant".to_owned();
    assert_eq!(width("int<int32> x;"), [(int.clone(), "int<int32>".to_owned())]);
    assert_eq!(width("uint<int<foo>> x;"), [(uint, "uint<int<foo>>".to_owned())]);
    assert_eq!(
        width("int<bitsizeof (clog2(foo))> bar;"),
        [(int, "int<bitsizeof (clog2(foo))>".to_owned())]
    );
    // A ternary the fold decides keeps only its arm: `int<(1 ? 8 :
    // int32)>` is `int<8>` and compiles (`kg-oracle --errors`,
    // 2026-10-10), where the text door's pass 19 once judged the arm the
    // fold deletes; at the rooted door pass 19 is the loop's.
    assert_eq!(program(&[("main.k", &decl("int<(1 ? 8 : int32)> x;"))]), Vec::new());
}

/// `desugar` is a `cata` (`Desugar.hs:66-69`): a `clog2` or an `int<…>`
/// inside a width is replaced first, and the width around it then
/// cannot fold — the outer width's error alone, at its start
/// (`kg-oracle --errors`, 2026-10-10).
#[test]
fn a_width_around_an_unresolved_one_is_the_outer_widths_error() {
    let first = |d: &str| {
        let text = format!("inline void main()\n{{\n    {d}\n}}\n");
        let out = check(&text);
        assert_eq!(out.len(), 1, "{out:?}");
        (out[0].message.clone(), text[out[0].range.range].to_owned())
    };
    assert_eq!(
        first("int<clog2(0)> x;"),
        ("int width did not evaluate to a positive integer constant".to_owned(), "int<clog2(0)>".to_owned())
    );
    assert_eq!(
        first("uint<bitsizeof(uint<0>)> x;"),
        ("uint width did not evaluate to a positive integer constant".to_owned(), "uint<bitsizeof(uint<0>)>".to_owned())
    );
}

/// `basics.k::3,21`: as the argument of a call to a template's instance
/// left standing — `println(clog2(-1))`, `println` an abbreviated template
/// no deduction completes — a `clog2` that did not fold is pass 11's
/// "Unresolved argument" (`Frontend.hs:436-444`), beside pass 12's
/// verdict on the instance; elsewhere it is pass 16's own clause (`:403`):
/// at a variable, and as the argument of a plain function's call or of an
/// instance a round made (`kg-oracle --errors --no-base`, 2026-10-10). The
/// text door had every call's argument as pass 11's; without `println`'s
/// base library the rooted door asks an abbreviated template of its own.
/// Red with every call's arguments walked as pass 11's.
#[test]
fn a_clog2_that_does_not_fold_is_an_unresolved_argument_in_a_call() {
    let callees = "inline void p(auto x)\n{\n}\n\ntemplate <typename T>\ninline void q(uint32 x)\n{\n}\n\ninline void r(uint32 x)\n{\n}\n\n";
    let messages = |call: &str| -> Vec<String> {
        program(&[("main.k", &format!("{callees}inline void main()\n{{\n    {call}\n}}\n"))]).into_iter().map(|(m, _, _)| m).collect()
    };
    assert_eq!(messages("p(clog2(-1));"), ["Could not deduce all arguments for template `p`", "Unresolved argument"]);
    let not_positive = ["clog2 argument did not evaluate to a positive integer constant"];
    assert_eq!(messages("q<uint8>(clog2(0));"), not_positive);
    assert_eq!(messages("r(clog2(-2));"), not_positive);
    assert_eq!(messages("uint32 x = clog2(0);"), not_positive);
}

/// `Frontend.hs:393-398`. Only a fold that reaches `false` is an error; a
/// fold that cannot finish must stay silent, because in this entry point it
/// cannot finish for almost anything.
/// `kg-oracle --errors`, 2026-09-07: every one of the four rejects, and
/// the `const auto` line is what the reference folds first and this
/// table reads unfolded — left unjudged, as `decltype` of it is.
#[test]
fn a_static_assert_over_a_trusted_bitsizeof_folds_through_the_type_table() {
    rejects("inline void f()\n{\n    static assert(bitsizeof uint16 == 32);\n}\n");
    rejects("inline void f()\n{\n    static assert((bitsizeof uint16) == 32u16);\n}\n");
    rejects("inline void f(uint16 x)\n{\n    static assert(bitsizeof x == 32);\n}\n");
    rejects("inline void f()\n{\n    uint16 y;\n    static assert(bytesizeof y == 4);\n}\n");
    accepts("inline void f(uint16 x)\n{\n    uint16 y;\n    static assert(bitsizeof x == 16 && bytesizeof y == 2);\n}\n");
    // Declared `auto` from a folded product: the CST table's width and the
    // reference's differed, and the text door said nothing either way. The
    // loop folds the product first, as the reference does, and `-200` is 9
    // bits wide: "static assertion failed" (`kg-oracle --errors`,
    // 2026-10-10).
    rejects("inline void f()\n{\n    const auto o = 10 * (-20);\n    static assert(bitsizeof o == 1);\n}\n");
    accepts("inline void f()\n{\n    const auto o = 10 * (-20);\n    static assert(bitsizeof o == 9);\n}\n");
    // A file-level constant with a written type is found by the same
    // walk; one from another module is not, and stays unjudged.
    rejects("const uint32 g = 0;\ninline void f()\n{\n    static assert(bitsizeof g == 1);\n}\n");
}

/// At the text door a name it could not resolve stayed silent; at the
/// rooted door a name no module declares is pass 9's error, which stops
/// pass 16, so the undecided shapes are a template's parameter in a body
/// no instance reaches — clean in the reference too (`kg-oracle --errors`,
/// 2026-10-10).
#[test]
fn static_assert_reports_only_a_decided_falsehood() {
    rejects("inline void main() { static assert(1 == 2); }");
    accepts("inline void main() { static assert(1 == 1); }");
    accepts("template <auto N>\ninline void f() { static assert(bitsizeof(N) == 63); }");
    accepts("template <auto N>\ninline void f() { static assert(N); }");
}

/// `evalBinaryType` (`Constexpr.hs:244-249`): a comparison of two types is
/// `<:` both ways. Every verdict here is the oracle's, probed 2026-09-07
/// (`type-constexpr.k::4,5,11,12,29,45-50` are the corpus's copies):
/// `uint32 == uint30` fails (neither is a subtype of the other),
/// `uint32 < uint30` fails, `uint30 < uint32` holds, `S < uint32` fails,
/// `decltype({}) < uint1` holds (`Type.hs:337`) so its negation fails,
/// and a conjunction folds through the same arm.
#[test]
fn a_type_comparison_folds_through_the_type_table() {
    rejects("static assert(uint32 == uint30);");
    rejects("static assert(uint32 < uint30);");
    rejects("static assert(!(decltype({}) < uint1));");
    rejects("struct S\n{\nuint32 x;\n}\nstatic assert(S < uint32);");
    rejects("static assert(uint32 == uint32 && uint32 < uint30);");
    accepts("static assert(uint30 < uint32);\nstatic assert(uint32 <= uint32);\nstatic assert(uint32 != int32);\nstatic assert(bool == bool);");
    accepts("struct S\n{\nuint32 x;\n}\nstatic assert(S == S);\nstatic assert(S != uint32);");
    accepts("static assert(uint32 == uint32 && uint30 < uint32);");
}

/// The operand this verdict must not read: `decltype` of a `const auto`
/// whose initializer the reference folds first (`literal-operand.k`,
/// ninth snippet, `warning:0`). Both sides type `-200` alike; only the
/// unfolded product differs, and the assertion holds in the reference.
#[test]
fn a_decltype_of_a_folded_expression_is_not_judged() {
    accepts(
        "inline void main()\n{\nconst auto o = 10 * (-20);\nstatic assert(decltype(o) == decltype(-200));\n}\n",
    );
    accepts("inline void main()\n{\nstatic assert(decltype(10 * (-20)) == int9);\n}\n");
}

/// The two contexts pass 16 never sees, because passes 6 and 15 have already
/// deleted them.
#[test]
fn static_assert_skips_branches_and_templates_the_reference_deletes_first() {
    accepts("inline void main() { static if (false) { static assert(1 == 2); } }");
    accepts("template <auto N>\ninline void f() { static assert(1 == 2); }");
}

/// `Frontend.hs:400-407` with `Constexpr.hs:168-169`: a `clog2` survives to
/// pass 16 only when its argument is not a positive integer constant.
#[test]
fn clog2_rejects_a_folded_non_natural() {
    rejects("inline void main() { uint32 x = clog2(-1); }");
    rejects("inline void main() { uint32 x = clog2(0); }");
    accepts("inline void main() { uint32 x = clog2(8); }");
    // An undeclared `N` is pass 9's at the rooted door; a template's
    // parameter in a body no instance reaches is never judged.
    accepts("template <auto N>\ninline void f() { uint32 x = clog2(N); }");
}

/// `Frontend.hs:404-406`, the decidable half of the `unresolved` clauses.
#[test]
fn widths_and_static_if_predicates_must_be_constants() {
    rejects("inline void main() { uint<0> x; }");
    rejects("inline void main() { int<-1> x; }");
    accepts("inline void main() { uint<8> x; }");
    rejects("inline void main() { static if (1) { } }");
    accepts("inline void main() { static if (true) { } }");
}


/// `Frontend.hs:397` for an expression that cannot fold: over a mutable
/// local (`basics.k::67`) or a `static` one compared with a type
/// (`type-constexpr.k::2`), at the expression (`kg-oracle --errors`).
#[test]
fn a_static_assert_over_a_runtime_value_did_not_evaluate() {
    let at = |text: &str| -> Vec<String> {
        program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    };
    assert_eq!(
        at("inline void main()\n{\n    uint8 a;\n    bool b = (a == 2);\n    static assert(b);\n}\n"),
        vec!["static assertion expression did not evaluate to a boolean constant @ main.k:b"]
    );
    assert_eq!(
        at("inline void main()\n{\n    static uint32 x;\n    static assert(x == uint32);\n}\n"),
        vec!["static assertion expression did not evaluate to a boolean constant @ main.k:x == uint32"]
    );
    accepts("inline void main()\n{\n    const uint32 x = 3;\n    static assert(x == 3);\n    uint8 a;\n    static assert(bitsizeof(a) == 8);\n}\n");
}

/// `Frontend.hs:404-405` in a copy: `uint<N>` in `S<0>`, `int<N>` in
/// `S<0>` beside a fine `S<3>`, `uint<N - 2>` in `S<1>`, and an imported
/// template's (difffuzz 2026-10-10 [43], `c1<0>`) — at the type in the
/// template's text (`kg-oracle --errors`, 2026-10-10). Red with the copy
/// check unwired.
#[test]
fn a_width_a_copy_folds_to_no_positive_integer() {
    let at = |files: &[(&str, &str)]| -> Vec<String> {
        program(files).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    };
    let s = |ty: &str, uses: &str| format!("template <auto N>\nstruct S\n{{\n    {ty} x;\n}}\n\ninline void main()\n{{\n{uses}}}\n");
    let uint = "uint width did not evaluate to a positive integer constant";
    assert_eq!(at(&[("main.k", &s("uint<N>", "    S<0> s;\n"))]), vec![format!("{uint} @ main.k:uint<N>")]);
    assert_eq!(
        at(&[("main.k", &s("int<N>", "    S<3> a;\n    S<0> s;\n"))]),
        vec!["int width did not evaluate to a positive integer constant @ main.k:int<N>"]
    );
    assert_eq!(at(&[("main.k", &s("uint<N - 2>", "    S<1> s;\n"))]), vec![format!("{uint} @ main.k:uint<N - 2>")]);
    accepts(&s("uint<N>", "    S<3> s;\n"));
    let h = "module h { c1 }\n\ntemplate <auto N>\nstruct c1\n{\n    uint<N> x;\n}\n";
    assert_eq!(
        at(&[("main.k", "import h\n\ninline void main()\n{\n    c1<0> c;\n}\n"), ("h.k", h)]),
        vec![format!("{uint} @ h.k:uint<N>")]
    );
}

/// `staticAssert`'s `errorMsg` replaces the operand whole (`Frontend.hs:
/// 396-400`, `Error.hs:147`), so a type error pass 10 reported inside it
/// is gone and the assertion's verdict stands at the operand: `!0xff`,
/// `uint32 * uint30`, a `!` deep in a comparison (`kg-oracle --errors`,
/// 2026-10-10; difffuzz [16], [17], [25]). Red with pass 10's verdict
/// kept, and with the assertion silent over it.
#[test]
fn a_static_assert_replaces_a_type_error_inside_its_operand() {
    let at = |text: &str| -> Vec<String> {
        program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    };
    let not_constant = "static assertion expression did not evaluate to a boolean constant";
    assert_eq!(at("static assert(!0xff);\n"), vec![format!("{not_constant} @ main.k:!0xff")]);
    assert_eq!(at("static assert(uint32 * uint30);\n"), vec![format!("{not_constant} @ main.k:uint32 * uint30")]);
    assert_eq!(at("static assert(1 == (!0x01));\n"), vec![format!("{not_constant} @ main.k:1 == (!0x01)")]);
}
