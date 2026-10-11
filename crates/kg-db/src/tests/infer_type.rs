//! Pass 6's `evaluateConstexpr` errors (`Constexpr.hs:181-186,274-275,
//! 304-305,345-355`) at the rooted door — the loop's at a module's text
//! since sprint W3 step 1 (`kg_sem::check::TextPass::InferType`): the fold
//! makes the node it cannot fold the error, wherever inference reaches,
//! and the verdict enters the CST run in pass 6's place. Each case is a
//! `kg-oracle --errors` probe (2026-10-10), the position included.

use crate::tests::{program, rooted_accepts, rooted_errors};

/// `message @ file:the text its range covers` for every diagnostic, and
/// each `while compiling` note after it.
fn at(main: &str) -> Vec<String> {
    program(&[("main.k", main)])
        .into_iter()
        .map(|(m, at, labels)| {
            let notes: String = labels.iter().map(|l| format!(" [{l}]")).collect();
            format!("{m} @ {at}{notes}")
        })
        .collect()
}

/// `Div | y /= 0 … | otherwise -> desugarError "Division by zero is
/// undefined"`, and `Mod`'s (`Constexpr.hs:181-186`), at the node — from
/// its `(` where parentheses wrap it (`Parser.hs:836,845`).
#[test]
fn a_division_or_remainder_by_zero_is_the_folds_error() {
    assert_eq!(
        at("inline void main()\n{\n    uint32 x = 4 / 0;\n    uint32 y = (4 % 0);\n}\n"),
        ["Division by zero is undefined @ main.k:4 / 0", "Remainder by zero is undefined @ main.k:(4 % 0)"]
    );
    assert_eq!(at("inline void main()\n{\n    uint32 x = 4 / 2;\n    uint32 y = 4 % 3;\n}\n"), Vec::<String>::new());
}

/// `bytesFromBits` (`Constexpr.hs:274-275,349-351`): `bytesizeof` of a
/// width that is not whole bytes. Inside a width, the fold's error is the
/// only one — the width that holds it is never judged (`constexpr.k::124`).
#[test]
fn bytesizeof_of_a_width_that_is_not_whole_bytes_is_the_folds_error() {
    assert_eq!(
        at("inline void main()\n{\n    uint9 z;\n    uint32 y = bytesizeof(z) + 1;\n    uint<2 * bytesizeof 1> w;\n}\n"),
        [
            "bytesizeof is undefined for a type with bit size of 9 @ main.k:bytesizeof(z)",
            "bytesizeof is undefined for a type with bit size of 1 @ main.k:bytesizeof 1",
        ]
    );
    assert_eq!(at("inline void main()\n{\n    uint16 z;\n    static assert(bytesizeof(z) == 2);\n}\n"), Vec::<String>::new());
}

/// `mux x y = maybe err … $ maybeAt y …` (`Constexpr.hs:353-355`): a
/// selector out of range is the error only where it folded after the mux
/// was typed — a name's value. A literal one types the mux `TUnresolved`
/// (`Type.hs:1237-1238`), which never folds and never errs.
#[test]
fn a_mux_selector_a_name_folds_out_of_range_is_the_folds_error() {
    assert_eq!(
        at("const uint3 four = 4;\n\ninline void main()\n{\n    uint32 m = mux(four, 0, 1, 2, 3);\n}\n"),
        ["Invalid selector `4` for the `mux` with 4 elements @ main.k:mux(four, 0, 1, 2, 3)"]
    );
    assert_eq!(at("inline void main()\n{\n    uint32 m = mux(4, 0, 1, 2, 3);\n}\n"), Vec::<String>::new());
}

/// The reference folds a copy's body as its round infers it, and reports
/// the error at the template's text, framed by the instance (`Error.hs:
/// 176-195`). A template no instance reaches is never folded.
#[test]
fn a_fold_error_in_a_copy_is_framed_by_its_instance() {
    let template = "template <auto N>\ninline void f()\n{\n    uint32 x = 4 / N;\n}\n\n";
    assert_eq!(
        at(&format!("{template}inline void main()\n{{\n    f<0>();\n}}\n")),
        ["Division by zero is undefined @ main.k:4 / N [main.k:f<0>: while compiling]"]
    );
    assert_eq!(at(&format!("{template}inline void main()\n{{\n    f<2>();\n}}\n")), Vec::<String>::new());
    assert_eq!(at("template <auto N>\ninline void f()\n{\n    uint32 x = 4 / 0;\n}\n"), Vec::<String>::new());
}

/// `TemplateF (inferExpTree' alg <$> a) b` (`Inference.hs:63`): a
/// template's parameters are folded at the text whether or not it is ever
/// instantiated; a deleted `static if` branch and a `mux` case the fold
/// did not select are never seen, the case's error discarded with it.
#[test]
fn a_templates_parameters_are_folded_and_dead_code_is_not() {
    assert_eq!(
        at("template <uint<bytesizeof int4> N>\nclass Foo\n{\n}\n"),
        ["bytesizeof is undefined for a type with bit size of 4 @ main.k:bytesizeof int4"]
    );
    let dead = "inline void main()\n{\n    static if (false)\n    {\n        uint32 y = 1 % 0;\n    }\n    uint32 z = mux(1, 1 / 0, 2);\n}\n";
    assert_eq!(at(dead), Vec::<String>::new());
}

/// Pass 6 is before `instantiateTemplates`' `hasErrors'` (`Template.hs:
/// 55,84`): its error stops every pass from 9 on, so the `static assert`
/// beside it is never judged.
#[test]
fn a_fold_error_closes_the_run_before_pass_9() {
    assert_eq!(
        at("inline void main()\n{\n    uint32 x = 4 / 0;\n    static assert(false);\n}\n"),
        ["Division by zero is undefined @ main.k:4 / 0"]
    );
}

// The tests below were the CST pass 6's (`passes/infer_type.rs`), at the
// text door, which runs no loop and has no pass 6 now; read here against
// the loop's fold.

/// `Constexpr.hs:304-305,345` — `kg-oracle --errors`, 2026-09-08: a
/// `decltype` whose operand's type has no written form is the folder's
/// error at the `decltype` — at the `(` when a paren wraps it — and the
/// *only* report: the pass-8 checkpoint closes the pipeline before pass 17
/// can say the `auto` function's return type could not be deduced.
#[test]
fn a_decltype_of_a_type_with_no_written_form_is_illegal() {
    assert_eq!(
        rooted_errors("inline void main()\n{\n    using T = decltype({0});\n}\n"),
        vec!["Argument of type `{uint1}` is illegal for `decltype`"]
    );
    assert_eq!(
        rooted_errors("inline auto f(uint32 x)\n{\n    return {0};\n}\n\ninline void main()\n{\n    using T = decltype(f);\n}\n"),
        vec!["Argument of type `(uint32) -> {uint1}` is illegal for `decltype`"]
    );
    assert_eq!(
        at("inline void main()\n{\n    uint32 x = bitsizeof(decltype({0}));\n}\n"),
        ["Argument of type `{uint1}` is illegal for `decltype` @ main.k:(decltype({0}))"]
    );
    rooted_accepts("inline void main()\n{\n    uint32 y = 1;\n    using U = decltype(y);\n}\n");
    // Corrected 2026-10-10: this read `decltype(uint32)` as accepted too,
    // which the reference does not — `decltype`'s operand is an
    // `expression` (`Parser.hs:432,823-828`), and a type there is the
    // parser's "Invalid use of a type as an expression" at `uint32`
    // (`kg-oracle --errors`).
    assert_eq!(
        rooted_errors("inline void main()\n{\n    using T = decltype(uint32);\n}\n"),
        vec!["Invalid use of a type as an expression"]
    );
}

/// `Constexpr.hs:122,236-250` — `type-constexpr.k::45`: a comparison of
/// two types folds over the error node its `decltype` operand became —
/// the rewrite keeps the judged note, type included — so the subtree is
/// a boolean and the `decltype` is never reported: "static assertion
/// failed", and `uint32 x = decltype({0}) == uint32;` is the boolean's
/// conversion (`kg-oracle --errors` 2026-09-08).
#[test]
fn a_decltype_under_a_type_comparison_folds_away_with_it() {
    assert_eq!(
        rooted_errors("inline void main()\n{\n    static assert(!(decltype({}) < uint1));\n}\n"),
        vec!["static assertion failed"]
    );
    assert_eq!(
        rooted_errors("inline void main()\n{\n    uint32 x = decltype({0}) == uint32;\n}\n"),
        vec!["Invalid conversion from `bool` to `uint32`"]
    );
}

/// `Constexpr.hs:274-275` — `kg-oracle --errors`, 2026-09-10: `uint1 y =
/// bytesizeof(x);` over a `uint9 x` is rejected at the `bytesizeof`. A
/// name declared `auto` is typed by the loop as the reference types it.
#[test]
fn bytesizeof_of_a_width_that_is_not_a_multiple_of_eight_is_the_folders_error() {
    assert_eq!(
        rooted_errors("inline void main()\n{\n    uint9 x;\n    uint1 y = bytesizeof(x);\n}\n"),
        vec!["bytesizeof is undefined for a type with bit size of 9"]
    );
    rooted_accepts("inline void main()\n{\n    uint8 x;\n    uint1 y = bytesizeof(x);\n}\n");
    rooted_accepts("inline void main()\n{\n    uint16 x;\n    static assert(bytesizeof(x) == 2);\n}\n");
    // `auto x = 3` is `uint2` to the loop as to the reference: the CST
    // pass's table could not be trusted for an `auto` name and said
    // nothing; the reference's verdict is this (`kg-oracle --errors`,
    // 2026-10-10).
    assert_eq!(
        rooted_errors("inline void main()\n{\n    auto x = 3;\n    uint1 y = bytesizeof(x);\n}\n"),
        vec!["bytesizeof is undefined for a type with bit size of 2"]
    );
}

/// `Constexpr.hs:162-163,347` — `kg-oracle --errors`, 2026-09-10: a field
/// the struct does not have, and a type that is not a struct, are both
/// `Illegal operands for bitoffsetof`, at the expression.
#[test]
fn bitoffsetof_of_a_missing_field_or_a_non_struct_is_illegal() {
    let foo = "struct Foo\n{\n    uint32 _x;\n    uint32 _y;\n}\n\n";
    assert_eq!(
        rooted_errors(&format!("{foo}inline void main()\n{{\n    auto x = bitoffsetof(Foo, _z);\n}}\n")),
        vec!["Illegal operands for bitoffsetof: Foo, _z"]
    );
    assert_eq!(
        rooted_errors("inline void main()\n{\n    auto x = bitoffsetof(uint32, x);\n}\n"),
        vec!["Illegal operands for bitoffsetof: uint32, x"]
    );
    rooted_accepts(&format!("{foo}inline void main()\n{{\n    static assert(bitoffsetof(Foo, _y) == 32);\n}}\n"));
    rooted_accepts(&format!("{foo}inline void main()\n{{\n    static assert(byteoffsetof(Foo, _y) == 4);\n}}\n"));
}

/// Pass 10 in a copy (`Frontend.hs:169-174`): `cata typeErrors` runs over
/// the program with its instances in it, so a type error a copy's body
/// raises is reported at the template's text with the instantiation as
/// its note — in a called function template, in a class template's
/// member nothing calls (difffuzz 2026-10-10 [34]), an operator's operands
/// as well as a `!`'s (`kg-oracle --errors`, 2026-10-10, each the
/// reference's first error). A conversion pass 19 would judge inside one
/// says nothing more. Red with the copies' pass 10 unwired.
#[test]
fn a_type_error_in_a_copy_is_reported_at_the_template() {
    let copy = |main: &str| -> Vec<String> {
        program(&[("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    };
    assert_eq!(
        copy("template <typename T>\ninline void f(T x)\n{\n    !x;\n}\n\ninline void main()\n{\n    f<uint32>(1);\n}\n"),
        ["Expected a boolean operand @ main.k:!x"]
    );
    assert_eq!(
        copy("template <typename T>\nclass Foo\n{\n    void Do(T x)\n    {\n        !x;\n    }\n}\n\ninline void main()\n{\n    Foo<uint32> foo;\n}\n"),
        ["Expected a boolean operand @ main.k:!x"]
    );
    assert_eq!(
        copy("template <typename T>\nclass Foo\n{\n    void Do(T x)\n    {\n        T y = x + true;\n    }\n}\n\ninline void main()\n{\n    Foo<uint32> foo;\n}\n"),
        ["Invalid operands to binary expression: `uint32` and `bool` @ main.k:x + true"]
    );
    rooted_accepts("template <typename T>\nclass Foo\n{\n    void Do(T x)\n    {\n        !x;\n    }\n}\n\ninline void main()\n{\n    Foo<bool> foo;\n}\n");
}
