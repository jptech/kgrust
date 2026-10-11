//! Pass 19's `checkConversion` (`TypeCheck.hs:84-178`) at the rooted door —
//! the loop's at a module's text since sprint W3 step 1
//! (`kg_sem::check::TextPass::CheckConversion`). These were the CST pass's
//! own tests, at the text door, which runs no loop and has no pass 19
//! conversions now. Each is a corpus shape or a `kg-oracle --errors` probe
//! the CST pass recorded; where the rooted door reads a case otherwise, the
//! test says why.

use kg_diag::{Diagnostic, WarningKind};

/// Every diagnostic the rooted door gives `main.k`, alone under `/proj`
/// with no base library, every warning enabled (`conversion` is off by
/// default, `Options/CmdArgs.hs:111-117`).
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

#[track_caller]
fn accepts(text: &str) {
    let errs = errors(text);
    assert!(errs.is_empty(), "expected a clean compile of\n{text}\ngot {errs:?}");
}

#[track_caller]
fn rejects(text: &str) {
    assert!(!errors(text).is_empty(), "expected a rejection of\n{text}");
}

/// Every `conversion` warning, as `(message, has-a-cast-fix)`.
fn warnings(text: &str) -> Vec<(String, bool)> {
    check(text)
        .into_iter()
        .filter(|x| x.warning == Some(WarningKind::Conversion))
        .map(|x| (x.message.clone(), !x.fixes.is_empty()))
        .collect()
}

#[track_caller]
fn warns(text: &str, message: &str) {
    let got = warnings(text);
    assert!(got.iter().any(|(m, _)| m == message), "expected `{message}` in {got:?} for\n{text}");
    // The pass must never turn a warning into a rejection.
    accepts(text);
}

#[track_caller]
fn silent(text: &str) {
    assert!(warnings(text).is_empty(), "expected no conversion warning for\n{text}");
    accepts(text);
}

/// `main.k`'s diagnostics, `message @ text`.
fn at(text: &str) -> Vec<String> {
    super::program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
}

/// `static if (Cond) { using T = uint32; } else { struct T {…} }` under
/// `Cond = false`: the live branch is the spliced declaration it becomes
/// (`removeNestedScope`, `Constexpr.hs:44-47`), so `const T x = 10` is the
/// conversion the reference reports (`static-if.k::72`; `kg-oracle --errors`
/// 2026-09-14, 15:13).
#[test]
fn a_decided_static_if_s_live_branch_is_spliced_into_its_scope() {
    let text = "const bool Cond = false;\n\nstatic if (Cond)\n{\n    using T = uint32;\n}\nelse\n{\n    struct T\n    {\n        bool x;\n    }\n}\n\nconst T x = 10;\n";
    assert_eq!(errors(text), vec!["Invalid conversion from `uint4` to `const T`"]);
}

/// `kg-oracle --errors`, 2026-09-08: a function value against a
/// function-typed constant (`function-constants.k::13,15`).
#[test]
fn a_function_typed_constant_converts_its_initializer() {
    let foo = "inline uint32 Foo(bool x)\n{\n    return 1;\n}\n\n";
    assert_eq!(
        errors(&format!("{foo}const (bool) -> uint3 fn = Foo;\n")),
        vec!["Invalid conversion from `(bool) -> uint32` to `const (bool) -> uint3`"]
    );
    assert_eq!(
        errors(&format!("{foo}const auto fn = Foo;\nconst (bool) -> uint3 fn2 = fn;\n")),
        vec!["Invalid conversion from `const (bool) -> uint32` to `const (bool) -> uint3`"]
    );
    accepts(&format!("{foo}const (bool) -> uint32 fn = Foo;\n"));
}

/// `bitsizeof` of a class never folds, so the reference's source is
/// `unresolved` (`basics.k::110`; `kg-oracle --errors`, 2026-09-27): an
/// argument and a `return` are "Invalid conversion from `unresolved` to
/// `bool`" at the expression; a variable's initializer, plain or a list,
/// fails `checkVar`'s `tResolved s` (`TypeCheck.hs:131`) and is silent; and
/// a struct has a width.
#[test]
fn a_sizeof_of_a_class_is_unresolved_for_good() {
    let class = "class C\n{\n    uint32 x;\n}\n\n";
    let arg = format!("{class}inline void f(bool b)\n{{\n}}\n\ninline void main()\n{{\n    static C c;\n    f(bitsizeof(c) == 32);\n}}\n");
    assert_eq!(errors(&arg), ["Invalid conversion from `unresolved` to `bool`"]);
    let ret = format!("{class}inline bool g()\n{{\n    static C c;\n    return (bitsizeof(c) == 32);\n}}\n");
    assert_eq!(errors(&ret), ["Invalid conversion from `unresolved` to `bool`"]);
    accepts(&format!("{class}inline void main()\n{{\n    static C c;\n    bool y = bitsizeof(c) == 32;\n}}\n"));
    accepts(&format!("{class}struct P\n{{\n    bool a;\n    bool b;\n}}\n\ninline void main()\n{{\n    static C c;\n    P p = {{ true, bitsizeof(c) == 32 }};\n}}\n"));
    accepts("struct S\n{\n    uint32 x;\n}\n\ninline void f(bool b)\n{\n}\n\ninline void main()\n{\n    S s;\n    f(bitsizeof(s) == 32);\n}\n");
}

/// `| tIsEccMem t` is the first `Just s` guard (`TypeCheck.hs:128`): an ECC
/// memory with an initializer, at the declaration (`memory.k::0`).
#[test]
fn an_ecc_memory_with_initial_values_is_rejected() {
    let ecc = "inline uint32 fix(bool error, bool valid, uint32 value)\n{\n    return value;\n}\n\n";
    let class = |init: &str| format!("{ecc}class Main\n{{\n    const [[memory, ecc(fix)]] uint32[4] mem{init};\n}}\n");
    assert_eq!(errors(&class(" = { 1, 2, 3, 4 }")), vec!["ECC memories with initial values are not supported"]);
    assert!(!errors(&class("")).iter().any(|e| e.starts_with("ECC")));
}

/// `kg-oracle --errors`, 2026-09-08: a designated initializer's elements
/// take `errorNarrowingConversions` (`TypeCheck.hs:130,157`) — a narrowing
/// is the error with the cast hint, an unconvertible one the plain error
/// (`designated-initializer.k::1,13`); a designator the target cannot take
/// is reported on the designator (`designated-initializer.k::18`).
#[test]
fn a_designated_initializer_converts_each_field_with_the_error_continuation() {
    let foo = "struct Foo\n{\n    uint32 field1;\n    bool field2;\n}\n\n";
    assert_eq!(
        errors(&format!("{foo}inline void main()\n{{\n    static Foo foo = {{.field1 = -10, .field2 = false}};\n}}\n")),
        vec!["Invalid conversion from `int5` to `uint32`\nInsert an explicit cast to silence this issue `cast<uint32>()`"]
    );
    assert_eq!(
        errors(&format!("{foo}inline void main()\n{{\n    static Foo foo = {{.field1 = 3, .field2 = 7}};\n}}\n")),
        vec!["Invalid conversion from `uint3` to `bool`"]
    );
    accepts(&format!("{foo}inline void main()\n{{\n    static Foo foo = {{.field1 = 3, .field2 = true}};\n}}\n"));
    assert_eq!(
        errors("union Variant\n{\n    uint32 x;\n    bool y;\n}\n\ninline void main()\n{\n    static Variant v = {.x = false};\n}\n"),
        vec!["Invalid conversion from `bool` to `uint32`"]
    );
    let text = "inline void Go(uint32[2] a)\n{\n}\n\ninline void main()\n{\n    Go({.x = 10, .x = 11});\n}\n";
    // `.x` twice is also pass 19's `initializer-overrides` warning
    // (`TypeCheck.hs:59-65`); the one error is the point here.
    let out: Vec<String> = at(text).into_iter().filter(|d| !d.starts_with("Initializer overrides")).collect();
    assert_eq!(out, ["Member designator cannot initialize type uint32[2] @ main.k:.x = 10"]);
}

/// `kg-oracle --errors`, 2026-09-07: `bool b = c.M(true)` with `uint32
/// M(bool)` is "Invalid conversion from `uint32` to `bool`" at the call.
#[test]
fn a_member_calls_result_converts_like_any_other() {
    let class = "class C\n{\npublic:\n    uint32 M(bool x)\n    {\n        return 1;\n    }\n}\n\n";
    assert_eq!(
        errors(&format!("{class}inline void main()\n{{\n    static C c;\n    bool b = c.M(true);\n}}\n")),
        vec!["Invalid conversion from `uint32` to `bool`"]
    );
    accepts(&format!("{class}inline void main()\n{{\n    static C c;\n    uint32 b = c.M(true);\n}}\n"));
}

/// `tInitializedType`'s three error slots (`Type.hs:293-300`), each reported
/// through `checkConversion`'s first clause (`TypeCheck.hs:153`) on the
/// element it belongs to. An array reports **one** excess element — the one
/// at index `tDimension t` — because every later index falls through to
/// `tElement`; a struct reports every excess element; a scalar reports its
/// first (`initializer-list.k::11,16,22`, `basics.k`; probes 2026-09-07).
#[test]
fn an_initializer_slot_with_no_type_is_the_verdict() {
    assert_eq!(errors("inline void main()\n{\nuint32[4] x = { 1, 2, 3, 4, 5 };\n}\n"), ["Excess elements in array initializer"]);
    assert_eq!(
        errors("inline void main()\n{\nuint32[4] x = { 1, 2, 3, 4, 5, 6 };\n}\n"),
        ["Excess elements in array initializer"],
        "only the element at the dimension; the next reads `tElement` again"
    );
    assert_eq!(errors("inline uint32[4] Foo()\n{\nreturn {0, 1, 2, 3, 4};\n}\n"), ["Excess elements in array initializer"]);
    assert_eq!(
        errors("inline void main()\n{\nuint32[2][2] x = { {1, 2}, {3, 4}, {5, 6} };\n}\n"),
        ["Excess elements in array initializer"]
    );
    assert_eq!(
        errors("struct S\n{\nuint32 x;\nuint32 y;\n}\n\ninline void main()\n{\nS s = { 1, 2, 3 };\n}\n"),
        ["Excess element in initializer of type S"]
    );
    assert_eq!(errors("inline void main()\n{\nuint32 x = { 3 };\n}\n"), ["Non-empty initializer list cannot initialize type uint32"]);
    accepts("inline void main()\n{\nuint32[4] x = { 1, 2, 3, 4 };\n}\n");
}

/// `| otherwise = conversionError s t "" a` (`TypeCheck.hs:178`): a source
/// that is not even convertible is an error at all three sites — the
/// argument (`:97`), the initialiser (`:131`) and the `return` (`:84`),
/// whose `allowNarrowingConversions` silences only a *narrowing*.
#[test]
fn an_unconvertible_source_is_an_error_at_every_site() {
    assert_eq!(
        errors("inline void Foo(uint32 x)\n{\n}\n\ninline void main()\n{\nbool b;\nFoo(b);\n}\n"),
        ["Invalid conversion from `bool` to `uint32`"]
    );
    assert_eq!(errors("inline void main()\n{\nbool b;\nuint32 x = b;\n}\n"), ["Invalid conversion from `bool` to `uint32`"]);
    assert_eq!(errors("inline uint32 main()\n{\nbool b;\nreturn b;\n}\n"), ["Invalid conversion from `bool` to `uint32`"]);
    // A narrowing at the return site is silence (`allowNarrowingConversions`,
    // `TypeCheck.hs:102`).
    silent("inline uint8 main()\n{\nuint32 x;\nreturn x;\n}\n");
}

/// `returnVoid` (`Frontend.hs:293-296`) turns `return e` with a void-typed
/// `e` into `e; return;` in pass 16, so pass 19 never sees the mismatch —
/// the *backend* reports it as Error 21 (`function-auto-return.k::3,5`).
#[test]
fn a_void_return_expression_is_left_to_the_backend() {
    accepts("inline void Nothing()\n{\n}\n\ninline uint32 main()\n{\nreturn Nothing();\n}\n");
}

/// `test/syntax/conversion-warning.k:4-14` — the argument site,
/// `TypeCheck.hs:97`. `TSigned <: TUnsigned` has no clause (`Type.hs:317-350`).
#[test]
fn an_argument_narrows() {
    warns("inline void Foo(uint32 x)\n{\n}\ninline void main()\n{\nint32 x;\nFoo(x);\n}\n", "Narrowing conversion from `int32` to `uint32`");
}

/// The same file's `:16-26` and `:28-44`: an initializer-list argument is
/// checked element by element against the slot it fills
/// (`TypeCheck.hs:165-175`), and `:46-62` is the control.
#[test]
fn an_initializer_element_narrows() {
    warns(
        "inline void Foo(uint32[4] x)\n{\n}\ninline void main()\n{\nint32 x;\nFoo({x, 1, 2});\n}\n",
        "Narrowing conversion from `int32` to `uint32`",
    );
    warns(
        "struct S\n{\nint32 x;\nbool y;\n}\ninline void Foo(S x)\n{\n}\ninline void main()\n{\nuint32 x;\nFoo({x, false});\n}\n",
        "Narrowing conversion from `uint32` to `int32`",
    );
    silent("struct S\n{\nint32 x;\nbool y;\n}\ninline void Foo(S x)\n{\n}\ninline void main()\n{\nint32 x;\nFoo({x, false});\n}\n");
}

/// `:64-78` versus `:80-94` — the folded literal's minimal type decides
/// (`TypeCheck.hs:154-156`).
#[test]
fn an_enum_constant_argument_narrows_on_its_value() {
    warns(
        "enum E : uint3\n{\nX = 0b111\n}\ninline void Foo(uint2 x)\n{\n}\ninline void main()\n{\nFoo(E::X);\n}\n",
        "Narrowing conversion from `uint3` to `uint2`",
    );
    silent("enum E : uint2\n{\nX = 0b11\n}\ninline void Foo(uint2 x)\n{\n}\ninline void main()\n{\nFoo(E::X);\n}\n");
}

/// `:96-116` — an enum constant reaching a struct field through an
/// initializer list.
#[test]
fn an_enum_constant_initializer_element_narrows() {
    warns(
        "enum E : uint2\n{\nX = 0b11\n}\nstruct S\n{\nint2 x;\nbool y;\n}\ninline void Foo(S x)\n{\n}\ninline void main()\n{\nFoo({E::X, false});\n}\n",
        "Narrowing conversion from `uint2` to `int2`",
    );
}

/// `TypeCheck.hs:154`, pinned by `:118-136` against `:106-116`: the same
/// expression warns with mutable operands and not with `const` ones, which
/// fold to a literal whose *minimal* type is `uint4`.
#[test]
fn constant_folding_silences_a_variable_initializer() {
    warns("inline void main()\n{\nuint32 x = 10;\nuint32 y = 2;\nuint8 z = x + y;\n}\n", "Narrowing conversion from `uint33` to `uint8`");
    silent("const uint32 x = 10;\nconst uint32 y = 2;\ninline void main()\n{\nuint8 z = x + y;\n}\n");
    silent("const auto x = 10;\nconst auto y = 2;\ninline void main()\n{\nuint8 z = x + y;\n}\n");
}

/// `:146-174` — a declared `auto` is `deduceAuto`'s, never a conversion
/// site; the same expression against a written type is.
#[test]
fn a_declared_auto_is_not_a_conversion_site() {
    silent("inline void main()\n{\nauto x = -101 % 5;\n}\n");
    silent("inline void main()\n{\nauto x = -101 / 5;\n}\n");
    warns("inline void main()\n{\nuint8 x = -101 / 5;\n}\n", "Narrowing conversion from `int6` to `uint8`");
}

/// `:176-233`: an `EnumValue` folds down to its literal while it is still a
/// constant (`TypeCheck.hs:155-156`); through a non-`const` `auto` it is a
/// variable of type `Foo` and never folds.
#[test]
fn an_enum_value_unwraps_only_while_it_is_still_a_constant() {
    silent("enum Foo : uint16\n{\nOne = 1\n}\ninline void main()\n{\nuint3 x = Foo::One;\n}\n");
    silent("enum Foo : uint16\n{\nOne = 1\n}\ninline void Do(uint3 x)\n{\n}\ninline void main()\n{\nDo(Foo::One);\n}\n");
    warns(
        "enum Foo : uint16\n{\nOne = 1\n}\ninline void main()\n{\nauto x = Foo::One;\nuint3 y = x;\n}\n",
        "Narrowing conversion from `Foo` to `uint3`",
    );
    warns(
        "enum Foo : uint16\n{\nOne = 1\n}\ninline void Do(uint3 x)\n{\n}\ninline void main()\n{\nauto x = Foo::One;\nDo(x);\n}\n",
        "Narrowing conversion from `Foo` to `uint3`",
    );
}

/// `:246-260` — a template body is checked in its copies only
/// (`Frontend.hs:298-299`), so nothing in an uninstantiated one is.
#[test]
fn a_template_body_is_not_checked() {
    silent("enum Foo : uint16\n{\nOne = 1\n}\ntemplate <auto N>\nclass Bar\n{\nconst uint3 x = Foo::One;\n}\n");
}

/// `:262-274` — two arguments, one of which narrows, and a `return` that
/// must stay silent (`TypeCheck.hs:84`).
#[test]
fn a_return_never_warns_even_when_an_argument_does() {
    let source = "inline uint32 foo(uint32 x, uint4[8] y)\n{\nreturn x;\n}\ninline uint32 main()\n{\nint8 x;\nuint4[8] y;\nreturn foo(x, y);\n}\n";
    warns(source, "Narrowing conversion from `int8` to `uint32`");
    assert_eq!(warnings(source).len(), 1, "the `return` and `y` must both be silent");
}

/// `checkReturn` reaches the body's own statements only (`TypeCheck.hs:76`):
/// an `S` returned as `uint8` is rejected at the body's top level and
/// accepted one `if` or block down — `kg-oracle --errors`, 2026-10-10 — at a
/// function's own text and in a template's copies alike, and a taken
/// `static if` branch, spliced into the body, is its top level. Red with
/// the loop's check handing the function to every node.
#[test]
fn only_a_return_at_the_body_s_top_level_is_converted() {
    let s = "struct S\n{\n    uint32 a;\n}\n\n";
    let call = "inline void main()\n{\n    auto x = f(true);\n}\n";
    let body = |stmts: &str| format!("{s}inline uint8 f(bool c)\n{{\n    S s = {{1}};\n{stmts}}}\n\n{call}");
    rejects(&body("    return s;\n"));
    accepts(&body("    if (c)\n    {\n        return s;\n    }\n    return 1;\n"));
    accepts(&body("    {\n        return s;\n    }\n"));
    let one = ["Invalid conversion from `S` to `uint8` @ main.k:s"];
    assert_eq!(at(&body("    static if (true)\n    {\n        return s;\n    }\n    return 1;\n")), one);
    let template = |stmts: &str| {
        format!("{s}template <typename T>\ninline uint8 f(bool c)\n{{\n    T s = {{1}};\n{stmts}}}\n\ninline void main()\n{{\n    auto x = f<S>(true);\n}}\n")
    };
    assert_eq!(at(&template("    return s;\n")), one);
    assert_eq!(at(&template("    if (c)\n    {\n        return s;\n    }\n    return 1;\n")), Vec::<String>::new());
}

/// The fix-it `TypeCheck.hs:103` writes as prose, as a structured edit.
#[test]
fn the_warning_carries_a_cast_fix() {
    let source = "inline void Foo(uint32 x)\n{\n}\ninline void main()\n{\nint32 x;\nFoo(x);\n}\n";
    let warning = check(source).into_iter().find(|x| x.warning == Some(WarningKind::Conversion)).expect("must warn");
    let fix = &warning.fixes[0];
    assert_eq!(fix.message, "Insert an explicit cast to silence this issue `cast<uint32>()`");
    assert_eq!(fix.edits.len(), 2);
    assert_eq!(fix.edits[0].new_text, "cast<uint32>(");
    assert_eq!(fix.edits[1].new_text, ")");
    // The two edits bracket the argument: `Foo(cast<uint32>(x))`.
    assert_eq!(fix.edits[0].range.start(), warning.range.range.start());
    assert_eq!(fix.edits[1].range.start(), warning.range.range.end());
    // `warningNarrowingConversions` does not append the hint (`TypeCheck.hs:101`).
    assert!(!warning.message.contains("cast<"));
}

/// A callee, a type or a name no module declares is pass 9's error at the
/// rooted door, which sets the flag before pass 19 (`Frontend.hs:86-87`),
/// and `TUndefined` is no resolved side for a conversion anyway
/// (`Type.hs:1003`): never a conversion verdict.
#[test]
fn an_undefined_declaration_is_no_conversion() {
    for text in [
        "inline void main()\n{\nint32 x;\nSomethingImported(x);\n}\n",
        "inline void main()\n{\nImportedType x;\nuint2 y = x;\n}\n",
        "inline void Foo(uint2 x)\n{\n}\ninline void main()\n{\nFoo(Imported::Value);\n}\n",
    ] {
        assert!(warnings(text).is_empty(), "{text}");
        let errs = errors(text);
        assert!(errs.len() == 1 && errs[0].contains("not in scope"), "{errs:?}");
    }
}

/// Two locals of one name in sibling blocks, the call outside both: the
/// name names neither (`$quasiN`, `Parser.hs:135-140`), pass 9's error.
#[test]
fn a_name_two_sibling_blocks_declare_is_no_conversion() {
    let text = "inline void Foo(uint32 a)\n{\n}\ninline void main()\n{\n{\nint32 q;\n}\n{\nuint8 q;\n}\nFoo(q);\n}\n";
    assert!(warnings(text).is_empty());
    assert_eq!(errors(text), ["Symbol name not in scope: `q`"]);
}

/// `checkArguments`'s arity error (`TypeCheck.hs:91`), at the first
/// argument or the `)` of an empty list — `kg-oracle --errors`, 2026-09-10.
#[test]
fn an_arity_mismatch_is_the_reference_error() {
    assert_eq!(
        errors("inline void Foo(uint32 x, uint32 y)\n{\n}\ninline void main()\n{\nint32 x;\nFoo(x);\n}\n"),
        vec!["Invalid number of arguments, expected 2"]
    );
    assert_eq!(
        errors("inline uint32 foo()\n{\n    return 3;\n}\n\ninline uint32 main()\n{\n    return foo(3);\n}\n"),
        vec!["Invalid number of arguments, expected 0"]
    );
    // A method, called on an object and from a sibling method.
    let class = "class C\n{\npublic:\n    uint32 M(uint32 x)\n    {\n        return x;\n    }\n    uint32 N()\n    {\n        return M(1, 2);\n    }\n}\n";
    assert_eq!(
        errors(&format!("{class}inline uint32 main()\n{{\n    static C c;\n    return c.M();\n}}\n")),
        vec!["Invalid number of arguments, expected 1", "Invalid number of arguments, expected 1"]
    );
    // A function-typed constant.
    assert_eq!(
        errors("inline uint32 foo(uint32 x)\n{\n    return x;\n}\ninline uint32 main()\n{\n    const (uint32) -> uint32 f = foo;\n    return f(1, 2);\n}\n"),
        vec!["Invalid number of arguments, expected 1"]
    );
    assert_eq!(
        at("inline uint32 foo()\n{\n    return 3;\n}\n\ninline uint32 main()\n{\n    return foo(3, 4);\n}\n"),
        vec!["Invalid number of arguments, expected 0 @ main.k:3"]
    );
}

/// A designator naming no field: `tField` answers `TUndefined`
/// (`Type.hs:301-303`) and the conversion is against it (`designated-
/// initializer.k::21,29,30`; `kg-oracle --errors` 2026-09-11).
#[test]
fn a_designator_to_a_missing_field_converts_to_undefined() {
    let foo = "struct Foo\n{\n    uint32 field1;\n    bool field2;\n}\n\n";
    assert_eq!(
        at(&format!("{foo}inline void main()\n{{\n    static Foo foo = {{.field3 = 10, .field2 = false}};\n}}\n")),
        vec!["Invalid conversion from `uint4` to `undefined` @ main.k:10"]
    );
    accepts(&format!("{foo}inline void main()\n{{\n    static Foo foo = {{.field1 = 10, .field2 = false}};\n}}\n"));
}

/// `checkUnionInit` (`TypeCheck.hs:168-172`): a union initializer's second
/// element — designated or positional — is "Excess elements in union
/// initializer" on that element (`designated-initializer.k::15`).
#[test]
fn a_union_initializer_s_second_element_is_excess() {
    let variant = "union Variant\n{\n    uint32 x;\n    bool y;\n}\n\n";
    assert_eq!(
        errors(&format!("{variant}inline void main()\n{{\n    static Variant v = {{.x = 10, .y = false}};\n}}\n")),
        vec!["Excess elements in union initializer"]
    );
    assert_eq!(
        errors(&format!("{variant}inline void main()\n{{\n    static Variant v = {{10, false}};\n}}\n")),
        vec!["Excess elements in union initializer"]
    );
    accepts(&format!("{variant}inline void main()\n{{\n    static Variant v = {{.y = false}};\n    static Variant w = {{10}};\n}}\n"));
}

/// A callee through an import alias is its own declaration:
/// `unit::assert(tag, cond)` over `import u as unit` is the module's
/// two-parameter `assert`, not the intrinsic (`risc-v.k`, 2026-09-10).
#[test]
fn an_aliased_callee_is_its_own_declaration() {
    let out = super::program(&[
        ("main.k", "import u as unit\n\ninline void main()\n{\n    unit::assert(\"t\", true);\n}\n"),
        ("u.k", "module u { assert }\n\ninline void assert(string tag, bool cond)\n{\n}\n"),
    ]);
    assert_eq!(out, Vec::new());
}

/// The reference checks a template's body only on its instances, deduces a
/// call to a function with an `auto` parameter (pass 12's verdict, at the
/// call), and rewrites a function with a function-typed parameter into a
/// template before any call to it is checked (`Function.hs:313`).
#[test]
fn an_arity_mismatch_the_reference_checks_elsewhere_is_silent() {
    silent("template <typename T>\ninline T tf(T x)\n{\n    return tf(x, x);\n}\ninline void main()\n{\n}\n");
    assert_eq!(
        errors("inline void p(auto x)\n{\n}\n\ninline void main()\n{\n    p();\n}\n"),
        vec!["Could not deduce all arguments for template `p`"]
    );
    silent("inline uint32 go((uint32) -> uint32 cb)\n{\n    return cb(1, 2);\n}\ninline void main()\n{\n}\n");
}

/// `TypeCheck.hs:130` and `:157` both take `errorNarrowingConversions`, so a
/// designated initializer's narrowing is the error with the hint
/// (`kg-oracle --errors` on this text, 2026-09-08).
#[test]
fn a_designated_initializer_narrows_as_an_error() {
    assert_eq!(
        errors("struct S\n{\nint32 x;\nbool y;\n}\ninline void main()\n{\nuint32 a;\nS s = {.x = a, .y = false};\n}\n"),
        ["Invalid conversion from `uint32` to `int32`\nInsert an explicit cast to silence this issue `cast<int32>()`"]
    );
}

/// A call through a closure is counted as `closureCalls` left it — the
/// lambda's function, the capture prepended (`Closure.hs:19-26`) — so
/// `l(1, 2)` over a one-parameter lambda is "expected 2", at the first
/// argument written (`kg-oracle --errors` under `mini-base`, 2026-10-10:
/// 4:18). The CST pass's table typed the closure `Unresolved` and said
/// nothing.
#[test]
fn a_closure_call_counts_its_capture() {
    let core = "module data.closure.core\n    { Closure\n    , make_capture0\n    }\n\n\
        template <typename FunctionType, FunctionType fn, typename Capture>\n\
        using Closure = Capture;\n\n\
        inline auto make_capture0()\n{\nstruct Capture\n{\n}\n\nCapture capture;\n\nreturn capture;\n}\n";
    let closure = "module data.closure\n    { module data.closure.core\n    }\n\nimport data.closure.core\n";
    let main = "import data.closure\n\ninline uint32 main()\n{\n    const auto l = [](uint32 x) { return x; };\n    uint32 d = l(1, 2);\n    return d;\n}\n";
    let out: Vec<String> = super::program(&[("main.k", main), ("data/closure.k", closure), ("data/closure/core.k", core)])
        .into_iter()
        .filter(|(m, _, _)| !m.starts_with("The import is redundant"))
        .map(|(m, at, _)| format!("{m} @ {at}"))
        .collect();
    assert_eq!(out, ["Invalid number of arguments, expected 2 @ main.k:1"]);
}

/// `basics.k::199`: the supertype message spells the named types —
/// `kg-oracle --errors` on this text prints `Foo` then `Bar`, 2026-09-08.
/// Pass 10's, the loop's since sprint W3 step 1.
#[test]
fn two_named_types_with_no_common_supertype_are_spelled() {
    let text = "struct Foo\n{\n    uint8 a;\n}\n\nstruct Bar\n{\n    bool b;\n}\n\ninline void main()\n{\n    bool pred;\n    Bar x;\n    Foo y;\n    Foo z = pred ? x : y;\n}\n";
    let messages: Vec<String> = super::program(&[("main.k", text)]).into_iter().map(|(m, _, _)| m).collect();
    assert!(messages.iter().any(|m| m == "Types `Foo` and `Bar` have no common supertype"), "{messages:?}");
}
