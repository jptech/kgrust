//! Constants folding at the rooted door: a file's own, an import's, a
//! member's, a decided `static if` branch's — read through a `static
//! assert` or a conversion, the loop's verdicts (`kg_sem::elaborate`'s
//! `evaluateConstexpr`, `Constexpr.hs:286-293`).
//!
//! These were `scope::FileScope`'s tests, the CST passes' one-file fold
//! environment; the rooted door read the loop's fold for them since pass
//! 16's CST code went (sprint W3 step 1), and they moved here when the
//! environment was deleted with the CST engine (sprint W3.7). Each still
//! names the bug it was written against.

use crate::tests::{program, rooted_accepts as accepts, rooted_rejects as rejects};

const H: &str = "module h { S, Bar, Ten }\n\nconst string S = \"foo\";\n\nenum Bar : int32\n{\n    B1,\n    B2 = 3\n}\n\nconst auto Ten = 10;\n";

fn messages(files: &[(&str, &str)]) -> Vec<String> {
    program(files).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
}

/// An imported constant folds like a local one (`findInImports`,
/// `SymbolTable.hs:263-270`; `Constexpr.hs:286-293`): a string, an enum
/// constant, through an alias, and inside an enum of the importing
/// module's own (`string.k::25`, `enum.k::8,9,11`; `kg-oracle --errors`
/// 2026-09-10).
#[test]
fn imported_constants_fold() {
    assert_eq!(
        messages(&[("main.k", "import h\n\nstatic assert(\"foo\" != S);\n"), ("h.k", H)]),
        vec!["static assertion failed @ main.k:static assert(\"foo\" != S);"]
    );
    assert_eq!(
        messages(&[("main.k", "import h\n\nstatic assert(Bar::B1 != 0);\n"), ("h.k", H)]),
        vec!["static assertion failed @ main.k:static assert(Bar::B1 != 0);"]
    );
    assert_eq!(
        messages(&[("main.k", "import h as hh\n\nstatic assert(hh::Ten == 11);\n"), ("h.k", H)]),
        vec!["static assertion failed @ main.k:static assert(hh::Ten == 11);"]
    );
    assert_eq!(
        messages(&[("main.k", "import h\n\nenum Foo : int32\n{\n    F = Bar::B1 + Bar::B2,\n    G = F\n}\n\nstatic assert(Foo::G == 4);\n"), ("h.k", H)]),
        vec!["static assertion failed @ main.k:static assert(Foo::G == 4);"]
    );
    assert_eq!(
        messages(&[("main.k", "import h\n\nenum Foo : int32\n{\n    F = Bar::B1 + Bar::B2,\n    G = F\n}\n\nstatic assert(Foo::G == 3);\n"), ("h.k", H)]),
        Vec::<String>::new()
    );
}

/// `removeNestedScope` (`Constexpr.hs:358-370`): a decided `static if`
/// is its taken branch, and a declaration in a braced branch loses the
/// branch's `$quasiN` — so `y` below is a file-level constant, and the
/// reference fails both assertions (`kg-oracle --errors`, 2026-09-23;
/// `static-if.k::50`'s `x += y`). An undecided branch stays out of the
/// fold.
#[test]
fn a_decided_static_if_block_declares_into_the_file() {
    let head = "const auto a = true;\n\nstatic if (a)\n{\n    const uint32 y = 10;\n}\n\n";
    rejects(&format!("{head}static assert(y == 11);\n"));
    rejects(&format!("{head}inline void f()\n{{\n    static assert(y == 12);\n}}\n"));
    accepts(&format!("{head}static assert(y == 10);\n"));
}

/// `Constexpr.hs:286-293` — the file's own constants participate in the
/// fold, and the fixpoint chains them (`test/syntax/basics.k`, "global
/// static assert that evaluates to false").
/// A member `const auto c = a * b` beside a `static C c;` in another
/// function: `c` is not a unique simple name, and the simple-name key
/// left the member unfolded — `bool z = c` said the width algebra's
/// `const uint14` where the folded literal is `uint13`
/// (`kg-oracle --errors`, scratchpad/l9/w5.k, 2026-09-08). Keyed by
/// qualified name and looked up from the use's own scope, it folds. The
/// conversion is the loop's now (sprint W3 step 1), so the fold is read
/// through a `static assert`: one that fails only once `c` folds.
#[test]
fn a_member_constant_folds_beside_a_clashing_name() {
    let text = |assert: &str| {
        format!("class C\n{{\npublic:\nconst auto a = 1024;\nconst auto b = 4;\nconst auto c = a * b;\ninline void Go()\n{{\nstatic assert({assert});\n}}\n}}\ninline void Run()\n{{\nstatic C c;\nc.Go();\n}}\n")
    };
    assert_eq!(crate::tests::rooted_errors(&text("c == 4097")), vec!["static assertion failed"]);
    accepts(&text("c == 4096"));
}

/// A local declaration hides a file-scope constant of the same name, and
/// the fold has to stop there rather than walk past it
/// (`FileScope::resolved_key` then, `lookupSymbol` now). `test/logic/test_cases_2.k:129`
/// is the corpus's own: `uint3[16] x` in the function, `const uint3 x =
/// 2` at the end of the file, and `bytesizeof(x)` read the constant's
/// width — "bytesizeof is undefined for a type with bit size of 3" on a
/// program the reference compiles. The type follows the fold, so the
/// conversion names the array (`kg-oracle --errors`, 2026-09-19).
///
/// Red with the walk taken past the shadowing declaration: the first
/// `accepts` reports the `bytesizeof` verdict and the conversion reads
/// `uint7`.
#[test]
fn a_local_declaration_hides_a_file_scope_constant_from_the_fold() {
    let body = "inline void A()\n{\nuint3[16] x;\n";
    accepts(&format!("{body}static assert(bytesizeof(x) == 6);\n}}\nconst uint3 x = 2;\n"));
    accepts(&format!("{body}static assert(bitsizeof(x) == 48);\n}}\nconst uint7 x = 2;\n"));
    // The conversion is the loop's (sprint W3 step 1): its types, read
    // at the rooted door.
    let rooted = crate::tests::program(&[("main.k", &format!("{body}uint2 q = x;\n}}\nconst uint7 x = 2;\n"))]);
    assert_eq!(rooted.into_iter().map(|(m, _, _)| m).collect::<Vec<_>>(), vec!["Invalid conversion from `uint3[16]` to `uint2`"]);
    // The constant is still the one a use *outside* the shadowing scope
    // finds, which is what stops the rule being "drop a shadowed
    // constant".
    accepts("inline void A()\n{\nuint3[16] x;\n}\nconst uint7 x = 2;\nstatic assert(x == 2);\n");
}

/// Two copies of a template class both declare `c`, so neither was a
/// unique simple name and neither folded; each copy's member is its
/// own key now, and each conversion reads its own literal — `uint13`
/// for `C<4>`, `uint12` for `C<2>` (scratchpad/l9/w4.k, `kg-oracle
/// --errors` 2026-09-08).
#[test]
fn each_copys_member_constant_folds_by_its_own_key() {
    let main = "template <auto N>\nclass C\n{\npublic:\nconst auto a = 1024;\nconst auto c = a * N;\ninline void Go()\n{\nbool z = c;\n}\n}\ninline void Run()\n{\nstatic C<4> c4;\nstatic C<2> c2;\nc4.Go();\nc2.Go();\n}\n";
    let reports: Vec<String> = crate::tests::program(&[("main.k", main)])
        .into_iter()
        .map(|(m, at, labels)| format!("{m} @ {at} {labels:?}"))
        .collect();
    assert_eq!(
        reports,
        vec![
            "Invalid conversion from `uint12` to `bool` @ main.k:c [\"main.k:C<2>: while compiling\"]",
            "Invalid conversion from `uint13` to `bool` @ main.k:c [\"main.k:C<4>: while compiling\"]",
        ]
    );
}

/// The declared type of `const auto c = a * b` is the folded literal's
/// — `uint13` for 4096 — not the width algebra's `uint14` over the two
/// names (`inferExpType` sees the `IntLiteral` `evaluateConstexpr` left,
/// `Inference.hs:47,50`), at home and in a class member alike; and a
/// product over *typed* literals takes `inferBinaryOp` — `const uint32`
/// times `const int16` is `int48`, never the untyped fold's `int9`
/// (`literal-operand.k::8`; scratchpad/l9/w14.k–w17.k, `kg-oracle
/// --errors` 2026-09-08). Red with the literal typing cut, red with a
/// typed folded operand counted as a literal.
#[test]
fn a_folded_constants_declared_type_is_the_literals() {
    let f = "inline void Go()\n{\nconst auto a = 1024;\nconst auto b = 4;\nconst auto c = a * b;\nstatic assert(decltype(c) == ";
    accepts(&format!("{f}uint13);\n}}\n"));
    rejects(&format!("{f}uint14);\n}}\n"));
    accepts("class C\n{\npublic:\nconst auto a = 1024;\nconst auto b = 4;\nconst auto c = a * b;\ninline void Go()\n{\nstatic assert(decltype(c) == uint13);\n}\n}\ninline void Run()\n{\nstatic C c;\nc.Go();\n}\n");
    let t = "inline void Go()\n{\nconst uint32 a = 10;\nconst int16 b = -20;\nconst auto s = a * b;\nstatic assert(decltype(s) == ";
    accepts(&format!("{t}int48);\n}}\n"));
    rejects(&format!("{t}int9);\n}}\n"));
}

/// A fold with a suffixed operand keeps `inferBinaryOp`'s type
/// (`typedIntLiteral`, `Constexpr.hs:200-202`): `0u16 << 16` is `uint32`
/// by `tShiftLLiteral`, `0u4 << 12` is `uint16` — `kg-oracle --errors`
/// agrees on both, 2026-09-23 — and `unit::test_case`'s `Flags << …`
/// over a stamped `0u16` is this. No environment answered the stamp, so
/// the fold came out untyped. Red with `binary_value_type` answering
/// `None`.
#[test]
fn a_fold_over_a_suffixed_operand_keeps_the_operators_type() {
    let f = "inline void Go()\n{\nconst auto T = 0u16 << (12 + 4);\nconst auto A = 0u4 << 12;\nstatic assert(";
    accepts(&format!("{f}decltype(T) == uint32 && decltype(A) == uint16);\n}}\n"));
    rejects(&format!("{f}decltype(T) == uint31);\n}}\n"));
}

#[test]
fn file_local_constants_fold() {

    rejects("const auto Foo = 3 + 4;\nstatic assert(Foo == 8);");
    accepts("const auto Foo = 3 + 4;\nstatic assert(Foo == 7);");
    rejects("const auto A = 1;\nconst auto B = A + 1;\nstatic assert(B == 3);");
    // `castIntegralConstexpr`: the declared type truncates the initializer.
    rejects("const uint8 X = 300;\nstatic assert(X == 300);");
    accepts("const uint8 X = 300;\nstatic assert(X == 44);");
    // Two declarations of one name: the use's own scope decides, innermost
    // first (`lookupSymbol`, `Symbols.hs:83-87`) — the local `N` is 2
    // (`kg-oracle --errors`, scratchpad/l9/n1.k, n2.k, 2026-09-08). The
    // simple-name key had declined both and left this silent.
    rejects("const auto N = 1;\ninline void f() { const auto N = 2; static assert(N == 9); }");
    accepts("const auto N = 1;\ninline void f() { const auto N = 2; static assert(N == 2); }");
}
