//! The rest of pass 19, `validateProgram` (`TypeCheck.hs:38-71,180-215`),
//! at the rooted door — the loop's at a module's text since sprint W3 step
//! 1 (`kg_sem::check::TextPass::ValidateProgram`). These were the CST
//! pass's own tests, at the text door, which runs no loop and has no pass
//! 19 now. Each is a corpus shape or a `kg-oracle --errors` probe the CST
//! pass recorded; where the rooted door reads a case otherwise — a name no
//! module declares is pass 9's error there, which stops pass 19 — the test
//! says so.

use kg_diag::Diagnostic;

/// Every diagnostic the rooted door gives `main.k`, alone under `/proj`
/// with no base library, every warning enabled.
fn check(text: &str) -> Vec<Diagnostic> {
    use kg_base::Utf8PathBuf;
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    settings.warnings = kg_diag::WarningSet::all();
    let mut db = crate::Db::new(settings);
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
    db.check(&[main])
}

/// Every error message for `main.k`.
fn errors(text: &str) -> Vec<String> {
    check(text).into_iter().filter(Diagnostic::is_error).map(|x| x.message).collect()
}

/// Every error as `message @ the text it is at`.
fn located(text: &str) -> Vec<String> {
    check(text).into_iter().filter(Diagnostic::is_error).map(|d| format!("{} @ {}", d.message, &text[d.range.range])).collect()
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

/// `kg-oracle --errors`, 2026-09-07: `enum Bar : bool { A = 10 }` is
/// rejected at the `bool`; an alias of an integral base is not; inside a
/// template body the base is `T`, no verdict until a copy.
#[test]
fn an_enum_must_be_based_on_an_integral_type() {
    assert_eq!(located("enum E : bool\n{\n    A\n}\n"), ["Enums must be based on an integral type @ bool"]);
    rejects("enum E : float32\n{\n    A\n}\n");
    rejects("struct S\n{\n    uint8 x;\n}\n\nenum E : S\n{\n    A\n}\n");
    accepts("enum E : uint8\n{\n    A\n}\n");
    accepts("using T = int4;\n\nenum E : T\n{\n    A\n}\n");
    accepts("template <typename T>\nclass Foo\n{\npublic:\n    enum Bar : T\n    {\n        A = 10\n    }\n}\n");
}

/// `overridingInitializerWarning` (`TypeCheck.hs:59-65`), read off
/// `kg-oracle --errors --Werror` (2026-09-27, `designated-initializer.k
/// ::41`): each list on its own, the warning at the later designator's `.`
/// and the note at the nearest earlier one's; a field named once is
/// silent, and an uninstantiated template's body is never read.
#[test]
fn a_designator_named_twice_in_one_list_warns() {
    let text = "struct X { int32 x; int32 y; }\nstruct Y { X x; X y; }\ninline void main()\n{\n    \
                Y v = {.x = {.x = 1, .y = 2, .x = 3, .x = 5}, .y = {.x = 1}, .x = {.y = 4}};\n}\n";
    let out = check(text);
    let warned: Vec<(&str, &str)> = out
        .iter()
        .filter(|d| d.warning == Some(kg_diag::WarningKind::InitializerOverrides))
        .map(|d| (&text[d.range.range], &text[d.labels[0].range.range]))
        .collect();
    assert_eq!(
        warned,
        [(".x = 3", ".x = 1"), (".x = 5", ".x = 3"), (".x = {.y = 4}", ".x = {.x = 1, .y = 2, .x = 3, .x = 5}")],
        "{out:?}"
    );
    assert!(out.iter().all(|d| !d.is_error()), "{out:?}");
    let template = "struct X { int32 x; }\ntemplate <auto N>\ninline X f()\n{\n    X v = {.x = N, .x = 2};\n    return v;\n}\n";
    assert!(check(template).is_empty());
}

/// `kg-oracle --errors` on the `type-check.k` disagreements, 2026-09-08: a
/// type as a mux arm is pass 19's error and nothing else (`::6,7`); an
/// attribute's argument is an expression position (`::17`); and an array
/// dimension is reported at its `[` whatever was written (`::11`,
/// `basics.k::282`).
#[test]
fn a_type_operand_is_reported_where_the_reference_reports_it() {
    assert_eq!(located("const int32 x = 2 ? bool : 3;\n"), ["Invalid use of a type as an expression @ bool"]);
    let attr = "[[max_threads(int32)]] void Go()\n{\n    return;\n}\n";
    assert_eq!(errors(attr)[0], "Invalid use of a type as an expression");
    assert_eq!(located("inline void main()\n{\n    uint2[0] x;\n}\n"), ["Array dimension must be a positive integer @ ["]);
}

/// A memory's one dimension is no `many` element and keeps its own note:
/// `basics.k::173` is reported at the `0`, past the `[` and any space, a
/// parenthesis included (`kg-oracle --errors`, 2026-10-10).
#[test]
fn a_memorys_dimension_is_reported_at_its_expression() {
    let at = |decl: &str| located(&format!("inline void main()\n{{\n    {decl}\n}}\n"));
    let positive = "Array dimension must be a positive integer";
    assert_eq!(at("static [[memory]] uint32[0] _mem;"), [format!("{positive} @ 0")]);
    assert_eq!(at("static [[memory]] uint32[ 0] _mem;"), [format!("{positive} @ 0")]);
    assert_eq!(at("static [[memory]] uint32[(1 - 1)] _mem;"), [format!("{positive} @ (1 - 1)")]);
    assert_eq!(at("static uint32[0] _mem;"), [format!("{positive} @ [")]);
}

/// `checkVar t Nothing | tIsConst t` precedes the one clause that asks
/// `declFlags` (`TypeCheck.hs:133-139`): a class's uninitialised
/// function-typed constant is an error there too (`callbacks.k::6`), where
/// a mutable one — a callback — is a class member's right.
#[test]
fn a_class_members_function_constant_must_be_initialized() {
    assert_eq!(errors("class Foo\n{\n    const () -> void function;\n}\n"), ["A constant of a function type must be initialized"]);
    accepts("class Foo\n{\n    () -> void callback;\n}\n");
}

/// An `identAttr`'s argument is a `qualifiedIdentifier`, never an
/// `expression` (`Parser.hs:175`): `ecc(foo)` naming a struct is not a type
/// used as a value, where `max_threads(foo)` is (`memory.k::20`).
#[test]
fn only_an_integer_attributes_argument_is_an_expression() {
    let decl = |attr: &str| {
        format!("struct foo\n{{\n}}\n\nusing m = [[memory, {attr}]] uint32[4];\n\n[[max_threads(2)]] inline void Go()\n{{\n    return;\n}}\n")
    };
    assert!(!errors(&decl("ecc(foo)")).iter().any(|e| e.contains("type as an expression")));
    assert!(errors("struct foo\n{\n}\n\n[[max_threads(foo)]] inline void Go()\n{\n    return;\n}\n")
        .iter()
        .any(|e| e == "Invalid use of a type as an expression"));
}

/// `TypeCheck.hs:187-196` — a dimension that folds to a non-positive
/// integer (`test/syntax/basics.k`, "0 array size" and "negative array
/// size"); an undeclared one is pass 9's error, which stops pass 19.
#[test]
fn array_dimensions_must_be_positive() {
    rejects("inline void main() { uint2[0] x; }");
    rejects("inline void main() { uint2[-2] x; }");
    accepts("inline void main() { uint2[2] x; }");
    assert_eq!(errors("inline void main() { uint2[N] x; }"), ["Symbol name not in scope: `N`"]);
}

/// A dimension no pass can ever fold, each at its `[` (`kg-oracle
/// --errors`, 2026-09-27): a class's mutable member (`constexpr.k::166`),
/// and a constant computed from the variable of a `static for` that stays
/// a loop (`static-for.k::10`). Mentioned in the dimension itself, or in a
/// `static assert`, the variable makes the loop unroll and the constant
/// fold; a constant that does not read it folds anyway.
#[test]
fn a_dimension_nothing_can_fold_is_not_positive() {
    let positive = ["Array dimension must be a positive integer @ ["];
    assert_eq!(located("class Foo\n{\nprivate:\n    uint32[Size] a;\npublic:\n    auto Size = 32;\n}\n"), positive);
    let unrolled = |body: &str| format!("inline void main()\n{{\n    static for (const auto j : 2)\n    {{\n{body}    }}\n}}\n");
    assert_eq!(located(&unrolled("        const auto a = j + 1;\n        uint32[a] b;\n")), positive);
    accepts(&unrolled("        uint32[j + 1] b;\n"));
    accepts(&unrolled("        const auto a = 2;\n        uint32[a] b;\n"));
    accepts(&unrolled("        const auto a = j + 1;\n        uint32[a] b;\n        static assert(j < 2);\n"));
}

/// `TypeCheck.hs:205-215`, pinned by `test/syntax/enum.k:28-32` (`int8`:
/// `-129` and `128` reject, `uint8`: `-1` rejects); `reifyEnums`'
/// implicit successor is checked too; a base read through an alias is the
/// alias's integer type.
#[test]
fn enum_constants_must_fit_their_base_type() {
    rejects("enum Foo : int8\n{\nE1 = -129\n}");
    rejects("enum Foo : int8\n{\nE1 = 128\n}");
    rejects("enum Foo : uint8\n{\nE1 = -1\n}");
    accepts("enum Foo : int8\n{\nE1 = -128\n}");
    accepts("enum Foo : int8\n{\nE1 = 127\n}");
    rejects("enum Foo : uint8\n{\nE1 = 255,\nE2\n}");
    rejects("using Base = int8;\nenum Foo : Base\n{\nE1 = 999\n}");
}

/// `TypeCheck.hs:125-127`, at the declaration (`kg-oracle --errors`,
/// 2026-10-10: 1:1). `DeclareGlobal` is module scope only
/// (`Parser.hs:249,462`), so a class member and a local are unaffected; a
/// type no module declares is pass 9's error instead.
#[test]
fn global_state_is_not_allowed() {
    assert_eq!(located("uint32 g = 3;"), ["Global state is not allowed @ uint32 g = 3;"]);
    accepts("const uint32 g = 3;");
    accepts("inline void main() { uint32 g = 3; }");
    accepts("class C\n{\nuint32 g = 3;\n}");
    assert_eq!(errors("SomeImportedType g;"), ["Type name not in scope: `SomeImportedType`"]);
}

/// `TypeCheck.hs:54,105-108` over every `AssignF` shape `Parser.hs:607-638`
/// produces, pinned by `test/syntax/const-correctness.k`; a non-const
/// lvalue and a const *read* stay silent.
#[test]
fn assignment_to_a_const_lvalue_is_illegal() {
    rejects("inline void Do()\n{\nconst uint32 x = 0;\n\nx = 1;\n}");
    rejects("inline void Do()\n{\nconst uint32 x = 0;\n\nx += 1;\n}");
    rejects("inline void Do()\n{\nconst uint32 x = 0;\n\nx++;\n}");
    rejects("inline void Do()\n{\nconst uint32 x = 0;\n\n--x;\n}");
    rejects("inline void Do()\n{\nconst bool x = false;\n\nx &&= true;\n}");
    assert_eq!(errors("inline void Do()\n{\nconst uint32 x = 0;\n\nx = 1;\n}"), ["`x`: illegal use of this constant as an lvalue"]);
    accepts("inline void Do()\n{\nuint32 x = 0;\n\nx = 1;\n}");
    accepts("inline void Do()\n{\nconst uint32 x = 0;\n\nuint32 y = x + 1;\n}");
}

/// The member and array shapes: `tMember` re-wraps a hit in `TConst`
/// (`Type.hs:940`) and `tElementLValue (TConst a)` is `TConst $ tElement a`
/// (`Type.hs:949`). The spelling is the parser twin's: `symbolError s`
/// names the `Const` base symbol (`Parser.hs:794-801`), so `s.x = 1` on a
/// `const s` reads `` `s` `` (`kg-oracle --errors` on
/// `const-correctness::10`).
#[test]
fn assignment_through_a_const_member_or_element_is_illegal() {
    rejects("struct S\n{\nuint32 x;\n}\n\ninline void Do()\n{\nconst S s = {};\n\ns.x = 1;\n}");
    rejects("struct S\n{\nuint32 x;\n}\n\ninline void Do()\n{\nconst S[2] s = {};\n\ns[0].x = 1;\n}");
    assert_eq!(
        errors("struct S\n{\nuint32 x;\n}\n\ninline void Do()\n{\nconst S s = {};\n\ns.x = 1;\n}"),
        ["`s`: illegal use of this constant as an lvalue"]
    );
    accepts("struct S\n{\nuint32 x;\n}\n\ninline void Do()\n{\nS s = {};\n\ns.x = 1;\n}");
}

/// `const-correctness::18` — a method call on a `const` receiver is *not*
/// an `AssignF`; the reference rejects it in `namedLValue`
/// (`Parser.hs:732,800`), which pass 0 imitates.
#[test]
fn a_method_call_on_a_const_receiver_is_not_an_assignment() {
    assert_eq!(
        errors("class C\n{\npublic:\ninline void Foo(uint32 a)\n{\nx = a;\n}\nuint32 x = 0;\n}\n\ninline void Do()\n{\nconst C c = {};\n\nc.Foo(1);\n}"),
        ["`c`: illegal use of this constant as an lvalue"]
    );
}

/// `TypeCheck.hs:180-185` via `Parser.hs:823-828`'s three corpus-pinned
/// positions (`type-check::9`, `::10`, `::12`), and a name that denotes a
/// type; a value in each position stays silent.
#[test]
fn a_type_in_an_expression_position_is_illegal() {
    rejects("static assert(uint32);");
    rejects("inline void main()\n{\nuint32[2][uint32] x;\n}");
    rejects("inline void main()\n{\nint<4>;\n}");
    rejects("using Foo = uint8;\n\ninline void main()\n{\nFoo;\n}");
    assert_eq!(errors("static assert(uint32);"), ["Invalid use of a type as an expression"]);
    accepts("static assert(true);");
    accepts("inline void main()\n{\nuint32[2] x;\n}");
}

/// `checkDesignatedInitializer` (`TypeCheck.hs:69-71`): `default = e;`
/// takes only a list of designators (`kg-oracle --errors`, 2026-09-23:
/// `{ f }` is "Expected designated initializer" at its `{`, `{ .a = 1 }`
/// and `{}` compile).
#[test]
fn a_default_initialization_takes_only_designators() {
    let class = |init: &str| format!("struct S\n{{\n    uint8 a;\n}}\n\nclass Main\n{{\nprivate:\n    S s;\n    default = {init};\n}}\n");
    rejects(&class("{ 1 }"));
    rejects(&class("10"));
    accepts(&class("{ .a = 1 }"));
    accepts(&class("{}"));
    assert_eq!(errors(&class("{ 1 }")), ["Expected designated initializer"]);
}

/// Inside a `bitsizeof` operand that has a width nothing is judged: the
/// operator folds to a literal before pass 19 (`Constexpr.hs:271-275`;
/// difffuzz [04]); the same comparison outside is still an error, and an
/// operand with no width does not fold (`type-check::4`).
#[test]
fn a_bitsizeof_operand_is_folded_before_it_could_be_judged() {
    accepts("using foo = uint5;\n\ninline void main()\n{\nint<bitsizeof (foo < 1)> bar;\n}");
    rejects("using foo = uint5;\n\ninline void main()\n{\nstatic assert(foo < 1);\n}");
    rejects("using foo = uint5;\n\ninline void main()\n{\nint<bitsizeof (clog2(foo))> bar;\n}");
}

/// The operand set of `checkInvalidType` (`TypeCheck.hs:41-46`) — every
/// rejection oracle-checked; on some the reference's error is an earlier
/// pass's, which replaced the operation before pass 19 looked, and a type
/// as a `mux`'s argument is the parser's (`Parser.hs:823-828,870`), whose
/// arm a fold may have deleted before pass 19 — as an initializer, an argument, a
/// returned value — pass 0's now (`kg-oracle --errors`, 2026-10-10, each a
/// parse error at the `uint32`).
#[test]
fn a_type_as_an_operand_is_illegal() {
    let invalid = |text: &str| located(text) == ["Invalid use of a type as an expression @ uint32"];
    assert!(invalid("inline void f(uint32 x)\n{\n}\ninline void main()\n{\nf(mux(1, uint32, 2));\n}\n"));
    assert!(invalid("inline uint32 f()\n{\nreturn mux(1, uint32, 2);\n}\n"));
    assert!(invalid("inline void main()\n{\nuint32 a = mux(1, uint32, 2);\n}\n"));
    assert!(invalid("inline uint32 f()\n{\n    return uint32;\n}\n"));
    rejects("static assert(uint32 == 4);");
    rejects("static assert(int32 + int32);");
    rejects("inline void main()\n{\nuint32 a = mux(1, uint32, 2);\n}");
    rejects("inline void main()\n{\nuint32 b = fan_out<2>(uint32);\n}");
    rejects("inline void main()\n{\nint<uint32> c = 0;\n}");
}

/// A *comparison* of two types folds to a boolean by subtyping before
/// pass 19 can look (`evalBinaryType`, `Constexpr.hs:245-251`) and is legal
/// — `library/base.k:52-55`'s `decltype` shape, and a template alias's
/// (`type-constexpr::38-41`). The text door's version asserted `foo_t<10>
/// == uint4` clean, which its pass 16 could not fold; it is `uint10 ==
/// uint4`, false, and the rooted door's pass 16 says so, as `kg-oracle
/// --errors` does (2026-10-10, at the `static assert`).
#[test]
fn a_comparison_of_two_types_is_not_a_type_as_expression() {
    accepts("static assert(uint32 <= uint64);");
    accepts("inline void main()\n{\nbool b = uint32 == uint64;\n}");
    accepts("using Foo = uint8;\n\nstatic assert(Foo == uint8);");
    accepts("inline void main()\n{\nuint32 x = 0;\n\nstatic assert(decltype(x) == uint32);\n}");
    accepts("template <auto N>\nusing foo_t = uint<N>;\n\nstatic assert(foo_t<10> == uint10);");
    accepts("template <auto N>\nusing foo_t = uint<N>;\n\nstatic assert(uint10 == foo_t<10>);");
    assert_eq!(errors("template <auto N>\nusing foo_t = uint<N>;\n\nstatic assert(foo_t<10> == uint4);"), ["static assertion failed"]);
}

/// `UnaryF BitSizeof` and `UnaryF ByteSizeof` are exempt
/// (`TypeCheck.hs:38-39`); `clog2` and `-` are not — though pass 16's or
/// pass 10's error there replaced the operation first.
#[test]
fn bitsizeof_and_bytesizeof_of_a_type_are_exempt() {
    accepts("inline void main()\n{\nuint32 x = bitsizeof(uint32);\n}");
    accepts("inline void main()\n{\nuint32 x = bytesizeof(uint32);\n}");
    rejects("inline void main()\n{\nuint32 x = clog2(uint32);\n}");
    rejects("inline void main()\n{\nuint32 x = -(uint32);\n}");
    accepts("inline void main()\n{\nuint32 x = clog2(8);\n}");
}

/// `TypeCheck.hs:73-82`, each probed against `kg-oracle --errors`
/// 2026-09-10; the error is at the `[[reset]]`, and the return type is the
/// deduced one.
#[test]
fn a_reset_function_takes_no_parameters_and_returns_void() {
    assert_eq!(
        located("class C\n{\npublic:\n    [[reset]]\n    bool foo()\n    {\n        return true;\n    }\n}\nexport C;\n"),
        vec!["Function with [[reset]] must have void return type @ [[reset]]\n    bool foo()\n    {\n        return true;\n    }"]
    );
    assert_eq!(
        errors("class C\n{\npublic:\n    [[reset]]\n    void foo(uint32 x)\n    {\n    }\n}\nexport C;\n"),
        vec!["Function with [[reset]] attribute can't have parameters"]
    );
    assert_eq!(errors("class C\n{\n    [[reset]]\n    auto foo()\n    {\n        return 3;\n    }\n}\n"), vec!["Function with [[reset]] must have void return type"]);
    assert_eq!(
        errors("class C\n{\npublic:\n    auto bar()\n    {\n        return true;\n    }\n\n    [[reset]]\n    auto foo()\n    {\n        return bar();\n    }\n}\n"),
        vec!["Function with [[reset]] must have void return type"]
    );
    accepts("class C\n{\npublic:\n    [[reset]] void foo()\n    {\n    }\n    [[reset]] auto bar()\n    {\n    }\n}\nexport C;\n");
}

/// `TypeCheck.hs:51`, the shapes `deduceAuto` leaves behind — each at the
/// `auto` (`kg-oracle --errors`, 2026-09-10) — and those it does not: a
/// function-typed parameter's, an abbreviated template's, a deduced one; a
/// member's function type holding one (`callbacks.k::66`).
#[test]
fn an_auto_no_pass_deduces_is_reported_at_the_auto() {
    assert_eq!(located("using fn = (auto) -> void;\n"), vec!["Could not deduce type @ auto"]);
    assert_eq!(located("using fn = () -> auto;\n"), vec!["Could not deduce type @ auto"]);
    assert_eq!(located("inline const auto Foo()\n{\n    return 10;\n}\n"), vec!["Could not deduce type @ auto"]);
    assert_eq!(located("class Foo\n{\npublic:\n    auto Do();\n}\n"), vec!["Could not deduce type @ auto"]);
    accepts("inline void F((auto) -> void cb)\n{\n}\ninline void main()\n{\n}\n");
    accepts("class Foo\n{\npublic:\n    auto Go(auto x);\n}\n");
    accepts("inline auto Foo()\n{\n    return 10;\n}\ninline void main()\n{\n    auto x = Foo();\n    const auto y = x;\n}\n");
    assert_eq!(located("class Foo\n{\n    (uint32) -> auto callback;\n}\n"), vec!["Could not deduce type @ auto"]);
    accepts("class Foo\n{\n    (uint32) -> uint32 callback;\n}\n");
}

/// `TypeCheck.hs:140` (`kg-oracle --errors`, 2026-09-10): `decltype(Go)`
/// over a function and an alias of a function type are function types;
/// `const` is fine.
#[test]
fn a_mutable_variable_of_an_aliased_function_type_needs_const() {
    let go = "inline uint32 Go(uint32 Do)\n{\n    return Do;\n}\n";
    let mutable = vec!["Expected 'const'. Mutable variables of a function type are not allowed"];
    assert_eq!(errors(&format!("{go}inline void main()\n{{\n    decltype(Go) x;\n}}\n")), mutable);
    assert_eq!(errors(&format!("using F = (uint32) -> uint32;\n{go}inline void main()\n{{\n    F x;\n}}\n")), mutable);
    accepts(&format!("using F = (uint32) -> uint32;\n{go}inline void main()\n{{\n    const F y = Go;\n    const decltype(Go) z = Go;\n}}\n"));
}

/// `TypeCheck.hs:198-199` for an argument that cannot be a constant
/// (`kg-oracle --errors`, 2026-09-10, at the argument): a mutable member, a
/// local, a parameter, an expression over one; and for one that folds to
/// the wrong kind of constant. A `const` member folds and is fine, as is
/// `bitsizeof` of the mutable local; `name(…)`'s string is never judged.
#[test]
fn an_integer_attribute_needs_a_constant_integer() {
    let refused = "Expected a constant expression that evaluates to non-negative integer";
    assert_eq!(
        located("class Main\n{\n    auto ThreadCount = 2;\n\npublic:\n    [[max_threads(ThreadCount)]] void main()\n    {\n    }\n}\n\nexport Main;\n"),
        vec![format!("{refused} @ ThreadCount")]
    );
    let foo = "inline void Foo()\n{\n}\n\n";
    assert_eq!(errors(&format!("{foo}inline void main()\n{{\n    uint32 x = 4;\n    [[call_rate(x)]] Foo();\n}}\n")), vec![refused]);
    assert_eq!(errors(&format!("{foo}inline void main()\n{{\n    uint32 x = 4;\n    [[call_rate(x + 1)]] Foo();\n}}\n")), vec![refused]);
    assert_eq!(errors(&format!("{foo}inline void main(uint32 p)\n{{\n    [[call_rate(p)]] Foo();\n}}\n")), vec![refused]);
    assert_eq!(
        errors(&format!("struct S\n{{\n    uint32 _x;\n}}\n\n{foo}inline void main()\n{{\n    S s;\n    [[call_rate(s)]] Foo();\n}}\n")),
        vec![refused]
    );
    assert_eq!(
        errors("class Main\n{\npublic:\n    [[max_threads(true)]] void b()\n    {\n    }\n    [[max_threads(3.5)]] void c()\n    {\n    }\n}\n\nexport Main;\n").len(),
        2
    );
    accepts("class Main\n{\n    const auto ThreadCount = 2;\n    const auto Two = ThreadCount * 2;\n\npublic:\n    [[max_threads(Two)]] void main()\n    {\n    }\n}\n\nexport Main;\n");
    accepts(&format!("{foo}inline void main()\n{{\n    uint32 x = 4;\n    [[call_rate(bitsizeof(x))]] Foo();\n}}\n"));
    accepts("class Main\n{\n    [[name(\"x\")]] uint32 x;\n\npublic:\n    void main()\n    {\n    }\n}\n\nexport Main;\n");
}

/// `TypeCheck.hs:205-215` over constants that name each other (`kg-oracle
/// --errors`, 2026-09-10): `E2 = E1 + 2` folds through `E1` and is out of
/// range for `uint1`; `G = F + 1, F = G + 1` is a cycle and neither
/// initializer evaluates; a forward reference folds a round later
/// (`enum.k`'s `G = 2 * F, F = 0`). A constant naming something no module
/// declares is pass 9's error.
#[test]
fn enum_constants_fold_through_their_siblings() {
    assert_eq!(
        located("enum Foo : uint1\n{\n    E1 = 0,\n    E2 = E1 + 2\n}\n"),
        vec!["Enum constant value is out of range for the base type @ E1 + 2"]
    );
    assert_eq!(
        located("enum Foo : int32\n{\n    G = F + 1,\n    F = G + 1\n}\n"),
        vec![
            "Enum constant value doesn't evaluate to an integer constant @ F + 1",
            "Enum constant value doesn't evaluate to an integer constant @ G + 1",
        ]
    );
    accepts("enum Foo : uint8\n{\n    E1 = 1,\n    E2 = E1 + 2,\n    E3\n}\nstatic assert(Foo::E2 == 3);\nstatic assert(Foo::E3 == 4);\n");
    accepts("enum Foo : int32\n{\n    G = 2 * F,\n    F = 0\n}\nstatic assert(Foo::G == 0);\n");
    accepts("enum Foo : int32\n{\n    G = F + 1,\n    F = 3,\n    H\n}\nstatic assert(Foo::G == 4);\nstatic assert(Foo::H == 4);\n");
    assert_eq!(errors("enum Foo : uint8\n{\n    E1 = X,\n    E2 = E1 + 2\n}\n"), ["Symbol name not in scope: `X`"]);
}

/// `evaluateConstexpr` folds no call (`Constexpr.hs:125-320`): a call in
/// an array dimension, a `static if` predicate or an integer attribute is
/// never a constant — `uint32[three()] a;` at its `[`, `i()` over a
/// `static for`'s induction variable (difffuzz 2026-10-10 [35]), `static
/// if (yes())` at the `static`, `[[schedule(three())]]` at the call
/// (`kg-oracle --errors`, 2026-10-10). Red with a call left foldable.
#[test]
fn a_call_never_folds_to_a_constant() {
    let three = "inline uint32 three()\n{\n    return 3;\n}\n\n";
    assert_eq!(
        located(&format!("{three}inline void main()\n{{\n    uint32[three()] a;\n}}\n")),
        vec!["Array dimension must be a positive integer @ ["]
    );
    assert_eq!(
        located("inline void main()\n{\n    static for (const auto i : 2)\n    {\n        uint32[i()] a;\n    }\n}\n"),
        vec!["Array dimension must be a positive integer @ ["]
    );
    assert_eq!(
        located("inline bool yes()\n{\n    return true;\n}\n\ninline void main()\n{\n    static if (yes())\n    {\n    }\n}\n"),
        vec!["static if predicate did not evaluate to a boolean constant @ static if (yes())\n    {\n    }"]
    );
    assert_eq!(
        located(&format!("{three}inline void main()\n{{\n    [[schedule(three())]]\n    {{\n    }}\n}}\n")),
        vec!["Expected a constant expression that evaluates to non-negative integer @ three()"]
    );
}

/// `checkEnumConsts`' `errorMsg` replaces the value whole (`Error.hs:147`):
/// a type error pass 10 reported inside an enum constant's value — `!F`,
/// `!0`, `1 + true` — is gone, and the constant "doesn't evaluate to an
/// integer constant" at the value, alone (`kg-oracle --errors`,
/// 2026-10-10; difffuzz [19]). Red with the earlier error left to silence
/// the clause, and with pass 10's kept.
#[test]
fn an_enum_constant_replaces_a_type_error_inside_its_value() {
    let none = "Enum constant value doesn't evaluate to an integer constant";
    assert_eq!(located("enum Foo : int32\n{\n    F = !F\n}\n"), vec![format!("{none} @ !F")]);
    assert_eq!(located("enum Foo : uint8\n{\n    E1 = !0\n}\n"), vec![format!("{none} @ !0")]);
    assert_eq!(located("enum Foo : uint8\n{\n    E1 = 1 + true\n}\n"), vec![format!("{none} @ 1 + true")]);
}

/// The fold's absorbing identities are four (`Constexpr.hs:217-242`): an
/// integer literal beside `&` or `|`, a boolean one beside `&&` or `||`.
/// `||` beside an integer is none — `uint32[p || 1]` over a parameter
/// never folds — and literals the fold left standing never will:
/// `uint32[2 || 1]`, and `0 || (1)` where a `static for` unrolled `i ||
/// (1)` (difffuzz 2026-10-10, the re-run's `static-for.k::1` mutant). `p |
/// 0xff` over a `uint8` folds. Each `kg-oracle --errors`, 2026-10-10. Red
/// with every `||` read as absorbing, and with literals left to fold.
#[test]
fn a_dimension_no_identity_of_the_fold_can_absorb() {
    let at = vec!["Array dimension must be a positive integer @ ["];
    assert_eq!(located("inline void f(uint32 p)\n{\n    uint32[p || 1] a;\n}\n"), at);
    assert_eq!(located("inline void main()\n{\n    uint32[2 || 1] a;\n}\n"), at);
    assert_eq!(located("inline void main()\n{\n    static for (const auto i : 2)\n    {\n        uint32[3][i || (1)][2] a;\n    }\n}\n"), at);
    accepts("inline void f(uint8 p)\n{\n    uint32[p | 0xff] a;\n}\n");
}

/// `checkVarDecl`'s function-type clauses (`TypeCheck.hs:123-139`) in a
/// copy, at the template's text: a mutable variable of a written function
/// type, one of a `typename` argument's, an uninitialized constant; a
/// class's member is fine (`kg-oracle --errors`, 2026-10-10). Red with
/// the copy's clause unwired.
#[test]
fn a_variable_of_a_function_type_in_a_copy() {
    let mutable = "Expected 'const'. Mutable variables of a function type are not allowed";
    let f = |body: &str, arg: &str| format!("template <typename T>\ninline void Foo()\n{{\n    {body}\n}}\n\ninline void main()\n{{\n    Foo<{arg}>();\n}}\n");
    assert_eq!(located(&f("() -> void bar1;", "bool")), vec![format!("{mutable} @ () -> void bar1;")]);
    assert_eq!(located(&f("T bar1;", "() -> void")), vec![format!("{mutable} @ T bar1;")]);
    assert_eq!(
        located(&f("const () -> void bar1;", "bool")),
        vec!["A constant of a function type must be initialized @ const () -> void bar1;"]
    );
    accepts("template <typename T>\nclass C\n{\n    T bar1;\n}\n\ninline void main()\n{\n    C<() -> void> c;\n}\n");
}

/// `errorIfOutOfRange'`'s last clause (`TypeCheck.hs:215`) over a constant
/// the fold finished to a literal that is no integer — a comparison, a
/// `bool`, a float, a `const bool` — each at the value (`kg-oracle
/// --errors`, 2026-10-10; difffuzz [37], `E1 = 0 != 2`), beside a sibling
/// that folds. Red with the clause removed.
#[test]
fn an_enum_constant_folded_to_no_integer() {
    let none = "Enum constant value doesn't evaluate to an integer constant";
    assert_eq!(located("enum Foo : uint1\n{\n    E1 = 0 != 2\n}\n"), vec![format!("{none} @ 0 != 2")]);
    assert_eq!(located("enum Foo : uint8\n{\n    E1 = true\n}\n"), vec![format!("{none} @ true")]);
    assert_eq!(located("enum Foo : uint8\n{\n    E1 = 1.5\n}\n"), vec![format!("{none} @ 1.5")]);
    assert_eq!(located("enum Foo : uint8\n{\n    E1 = 3,\n    E2 = E1 == 3\n}\n"), vec![format!("{none} @ E1 == 3")]);
    assert_eq!(located("const bool B = true;\nenum Foo : uint8\n{\n    E1 = B\n}\n"), vec![format!("{none} @ B")]);
}

/// `TypeCheck.hs:146-148` (`kg-oracle --errors` with the base library,
/// 2026-09-10): a captured `uint32` or `bool` is rejected at the lambda; an
/// empty capture list, or a capture of an empty struct, is not; inside a
/// function the capture is fine. With the `data.closure` the capture needs,
/// stubbed: without it the reference's pass 9 rejects the `make_capture1`
/// `desugarLambdas` writes, and pass 19 never runs.
#[test]
fn a_global_closure_may_not_capture_a_value() {
    let core = "module data.closure.core\n    { Closure\n    , make_capture0\n    }\n\n\
        template <typename FunctionType, FunctionType fn, typename Capture>\n\
        using Closure = Capture;\n\n\
        inline auto make_capture0()\n{\nstruct Capture\n{\n}\n\nCapture capture;\n\nreturn capture;\n}\n";
    let closure = "module data.closure\n    { module data.closure.core\n    , make_capture1\n    }\n\n\
        import data.closure.core\n\n\
        template <typename T0>\n\
        inline auto make_capture1(T0 x0)\n{\n\
        struct Capture\n{\nT0 x0;\n}\n\n\
        Capture capture = { x0 };\n\nreturn capture;\n}\n";
    let run = |main: &str| -> Vec<String> {
        super::program(&[("main.k", &format!("import data.closure\n\n{main}")), ("data/closure.k", closure), ("data/closure/core.k", core)])
            .into_iter()
            .filter(|(m, _, _)| !m.starts_with("The import is redundant"))
            .map(|(m, at, _)| format!("{m} @ {at}"))
            .collect()
    };
    let lambda = |name: &str| format!("Closures that capture value(s) are not allowed in global scope @ main.k:[{name}](uint32 x)\n{{\n    return x + 1;\n}}");
    assert_eq!(run("const uint32 y = 0;\n\nconst auto inc = [y](uint32 x)\n{\n    return x + 1;\n};\n"), vec![lambda("y")]);
    assert_eq!(run("const bool z = true;\nconst auto inc2 = [z](uint32 x)\n{\n    return x + 1;\n};\n"), vec![lambda("z")]);
    assert_eq!(run("const auto inc = [](uint32 x)\n{\n    return x + 1;\n};\n"), Vec::<String>::new());
    assert_eq!(run("struct E {}\nconst E e = {};\nconst auto inc = [e](uint32 x)\n{\n    return x + 1;\n};\n"), Vec::<String>::new());
    assert_eq!(
        run("inline void main()\n{\n    const uint32 y = 0;\n    const auto inc = [y](uint32 x)\n    {\n        return x + y;\n    };\n}\n"),
        Vec::<String>::new()
    );
}
