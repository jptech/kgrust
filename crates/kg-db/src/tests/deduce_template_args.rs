//! Pass 4's `deduceTemplateArgs` (`Type.hs:1385-1593`) at the rooted door
//! — the loop's deduction since sprint W3 step 1, its verdicts at a
//! module's text admitted as `kg_sem::check::TextPass::TemplateTypeCheck`
//! (a conflicting deduction, `reconcile`) and `TextPass::UnresolvedTemplates`
//! (what is left undeduced). The CST pass these were written against is
//! deleted; each case is its `kg-oracle --errors` probe, as before.

use crate::tests::{closured_errors, rooted_accepts as accepts, rooted_errors as errors, rooted_rejects as rejects};

/// [`closured_errors`] is empty.
#[track_caller]
fn closured_accepts(main: &str) {
    let errs = closured_errors(main);
    assert!(errs.is_empty(), "expected a clean compile of\n{main}\ngot {errs:?}");
}

/// `kg-oracle --errors` on seven probes, 2026-09-08 (`Function.hs:313-345`,
/// `:365-392`, `TypeCheck.hs:269-275`): a callback's closure type against
/// the declared one, the capture class spelled as the library declares
/// it; a callback position given no function is the LT verdict with the
/// function as its frame; a matching lambda compiles. At the rooted door
/// the closure library is the two stubs ([`closured_errors`]), under which
/// the same probes give every verdict here (`-I` over them, 2026-10-10).
#[test]
fn a_callback_is_checked_as_the_closure_the_reference_makes_of_it() {
    let hof = "inline uint32 Go((uint32) -> uint32 Do, uint32 x)\n{\n    return Do(x);\n}\n\n";
    let mismatch = |expected: &str, actual: &str| {
        format!("Invalid argument for template parameter `Do`.\nExpected a function of type:\n\t{expected}\nbut the argument type is:\n\t{actual}")
    };
    let capture0 = "data.closure.core::make_capture0::$::Capture";
    let no_capture = format!("{hof}inline uint32 main()\n{{\n    return Go([](bool b) -> uint32 {{ return 1; }}, 3);\n}}\n");
    assert_eq!(
        closured_errors(&no_capture),
        vec![mismatch(&format!("({capture0}, uint32) -> uint32"), &format!("({capture0}, bool) -> uint32"))]
    );
    let one_capture = format!("{hof}inline uint32 main()\n{{\n    bool b;\n    return Go([b](bool x) -> uint32 {{ return 1; }}, 3);\n}}\n");
    let capture1 = "data.closure::make_capture1<bool>::$::Capture";
    assert_eq!(
        closured_errors(&one_capture),
        vec![mismatch(&format!("({capture1}, uint32) -> uint32"), &format!("({capture1}, bool) -> uint32"))]
    );
    let named = format!("{hof}inline uint32 Bar(bool x)\n{{\n    return 1;\n}}\n\ninline uint32 main()\n{{\n    return Go(Bar, 3);\n}}\n");
    assert_eq!(
        closured_errors(&named),
        vec![mismatch(&format!("({capture0}, uint32) -> uint32"), &format!("({capture0}, bool) -> uint32"))]
    );
    // The LT verdict is pass 12's, the loop's since sprint W3 step 1:
    // read at the rooted door.
    for call in ["Go(3)", "Go()"] {
        let text = format!("{hof}inline uint32 main()\n{{\n    return {call};\n}}\n");
        let out = crate::tests::program(&[("main.k", &text)]);
        assert_eq!(out.len(), 1, "{call}: {out:?}");
        assert_eq!(out[0].0, "Could not deduce all arguments for template `Go`");
        assert_eq!(out[0].1, "main.k:Go");
        assert!(out[0].2[0].starts_with("main.k:inline uint32 Go("), "{call}: {out:?}");
    }
    closured_accepts(&format!("{hof}inline uint32 main()\n{{\n    return Go([](uint32 y) -> uint32 {{ return y; }}, 3);\n}}\n"));
    // A `const auto` holding the lambda is the lambda
    // (`higher-order-functions.k::4`).
    let held = format!("{hof}inline uint32 main()\n{{\n    const auto fn = [](uint32 y) -> void\n    {{\n    }};\n    return Go(fn, 3);\n}}\n");
    assert_eq!(
        closured_errors(&held),
        vec![mismatch(&format!("({capture0}, uint32) -> uint32"), &format!("({capture0}, uint32) -> void"))]
    );
    // `(void)` is the empty list (`Type.hs:1353-1355`): `closure<void,
    // uint32>([]() -> uint32 {…})` compiles (`higher-order-functions.k::88`).
    closured_accepts("template <typename T, typename R>\ninline uint32 Wrap((T) -> R fn)\n{\n    return 0;\n}\n\ninline uint32 main()\n{\n    return Wrap<void, uint32>([]() -> uint32 { return 0; });\n}\n");
    // The lambda's function type takes part in deduction: `T` from `10`
    // and from `(uint3)` is the conflict, not the closure mismatch
    // (`template-argument-deduction.k::167`).
    let conflict = "template <typename T>\ninline T Fn_T_Fn1(T x, (T) -> T fn)\n{\n    return fn(x);\n}\n\ninline uint4 main()\n{\n    return Fn_T_Fn1(10, [](uint3 x) -> uint4 { return 0; });\n}\n";
    assert_eq!(
        closured_errors(conflict),
        vec!["Conflicting template argument deduction results for parameter `T`. Could be `uint3` or `uint4`."]
    );
    // A templated higher-order function: the declared type with the
    // explicit arguments substituted.
    let templated = "template <typename T, typename U>\ninline U Go4((T) -> U Do, T x)\n{\n    return Do(x);\n}\n\ninline bool main()\n{\n    uint32 x;\n    return Go4<uint32, bool>([](bool b) -> bool { return b; }, x);\n}\n";
    assert_eq!(
        closured_errors(templated),
        vec![mismatch(&format!("({capture0}, uint32) -> bool"), &format!("({capture0}, bool) -> bool"))]
    );
}

/// `test/syntax/template-argument-deduction.k:70` — the corpus's own
/// irreconcilable deduction. `N` is bound to 3 by the first argument and to
/// 2 by the second, and `reconcile`'s last clause turns that into a `TError`
/// (`Type.hs:1504-1506`) which `resolveTemplateArgs` plants as a
/// `desugarError` (`Type.hs:1415-1416`).
#[test]
fn two_extents_for_one_parameter_are_a_conflict() {
    rejects(
        "template <auto N>\n\
         inline void Foo(bool[N] x, uint32[N] y)\n\
         {\n\
         }\n\
         \n\
         inline void main()\n\
         {\n\
         bool[3] x;\n\
         uint32[2] y;\n\
         Foo(x, y);\n\
         }\n",
    );
}

/// The same call with agreeing extents — `test/syntax/template-argument-deduction.k:69`,
/// which the corpus marks `expected:0`.
#[test]
fn one_extent_for_one_parameter_is_not() {
    accepts(
        "template <auto N>\n\
         inline void Foo(bool[N] x, uint32[N] y)\n\
         {\n\
         }\n\
         \n\
         inline void main()\n\
         {\n\
         bool[3] x;\n\
         uint32[3] y;\n\
         Foo(x, y);\n\
         }\n",
    );
}

/// The message is the reference's, word for word (`Type.hs:1504-1506`), with
/// the parameter named and both candidates shown in `reconcile`'s order —
/// new first, then old.
///
/// `float32` renders as `float`: `pretty TFloat = "float"` (`Type.hs:388`),
/// because the type has one width and the spelling drops it. This test first
/// asserted the source spelling and was wrong.
#[test]
fn the_conflict_names_the_parameter_and_both_candidates() {
    assert_eq!(
        errors(
            "template <typename T>\n\
             inline void Foo(T x, T y)\n\
             {\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             bool a;\n\
             float32 b;\n\
             Foo(a, b);\n\
             }\n"
        ),
        vec![
            "Conflicting template argument deduction results for parameter `T`. \
             Could be `float` or `bool`."
        ]
    );
}

/// `setTypeOfTemplateCall` (`Type.hs:1120-1129`) stamps the callee of a
/// **bare** call too — the reference's lowering wraps every callee in a
/// `TemplateInstance`, written arguments or not — so the type the context
/// expects is deduction's third source (`Type.hs:1479-1480`) at an
/// initialiser and at a `return`. The conflict it produces is the one the
/// oracle reports for `template-argument-deduction.k::7,8,10,15`, spelled
/// with the expected type first. Until 2026-09-06 only a written `<…>`
/// was stamped, and these compiled clean.
#[test]
fn the_expected_type_is_a_deduction_source_for_a_bare_call() {
    assert_eq!(
        errors(
            "template <typename T>\ninline T Fn_T(T x)\n{\nreturn x;\n}\n\n\
             inline void main()\n{\nbool b = Fn_T(3);\n}\n"
        ),
        vec![
            "Conflicting template argument deduction results for parameter `T`. \
             Could be `bool` or `uint2`."
        ]
    );
    assert_eq!(
        errors(
            "enum E : uint4\n{\nZero\n}\n\ntemplate <typename T>\ninline T Fn_T(T x)\n{\nreturn x;\n}\n\n\
             inline bool Run()\n{\nreturn Fn_T(E::Zero);\n}\n"
        ),
        vec![
            "Conflicting template argument deduction results for parameter `T`. \
             Could be `bool` or `E`."
        ]
    );
    // The stamp never invents a conflict where the context agrees.
    accepts(
        "template <typename T>\ninline T Fn_T(T x)\n{\nreturn x;\n}\n\n\
         inline uint2 Run()\n{\nuint2 a = 1;\nreturn Fn_T(a);\n}\n",
    );
}

/// Trap 28 from the driver's side. With `T` written at the call there is no
/// deduction to conflict with it, however much the arguments disagree —
/// `unresolvedArgs` never gave index 0 a slot (`Type.hs:1431-1432`). What
/// the reference reports is pass 19's conversion of `b` to `bool y`
/// (`kg-oracle --errors`, 2026-10-10: 10:14), which the CST pass's text
/// door, running no pass 19, never showed.
#[test]
fn explicit_arguments_beat_deduced_ones() {
    let text = "template <typename T>\n\
         inline void Foo(T x, T y)\n\
         {\n\
         }\n\
         \n\
         inline void main()\n\
         {\n\
         bool a;\n\
         float32 b;\n\
         Foo<bool>(a, b);\n\
         }\n";
    assert_eq!(errors(text), ["Invalid conversion from `float` to `bool`"]);
}

/// `rewrite`'s first clause (`Type.hs:1390-1391`) turns `o.Fn_T(…)` into
/// a call of `Class::Fn_T`, and the deduction is the same one — the
/// conflict here is between the return context and the argument, spelled
/// new-first as always. Oracle: this message at the member name, column
/// 10 (`template-argument-deduction.k::87` is the corpus's own copy).
#[test]
fn a_member_template_deduces_through_the_receivers_class() {
    assert_eq!(
        errors(
            "enum E : uint4\n{\nZero\n}\n\n\
             class Class\n{\npublic:\ntemplate <typename T>\nT Fn_T(T x)\n{\nreturn x;\n}\n}\n\n\
             inline bool Run()\n{\nstatic Class o;\nreturn o.Fn_T(E::Zero);\n}\n"
        ),
        vec![
            "Conflicting template argument deduction results for parameter `T`. \
             Could be `bool` or `E`."
        ]
    );
}

/// `lookupInScope` walks outward from the call's own scope and stops at the
/// **first** hit (`Symbols.hs:82-87`), so `C::Foo` — two parameters, no
/// conflict — is what this call names. The top-level one-parameter `Foo` is
/// never reached.
///
/// The conflict this used to report named `T`, which is the *top-level*
/// template's parameter and the fingerprint of the misresolution: the port
/// had deduced against a signature the program does not call.
#[test]
fn a_member_template_shadows_the_top_level_one_of_the_same_name() {
    accepts(
        "template <typename T>\n\
         inline void Foo(T x, T y)\n\
         {\n\
         }\n\
         \n\
         class C\n\
         {\n\
         public:\n\
         template <typename T, typename U>\n\
         inline void Foo(T x, U y)\n\
         {\n\
         }\n\
         \n\
         inline void go()\n\
         {\n\
         bool a;\n\
         float32 b;\n\
         Foo(a, b);\n\
         }\n\
         }\n",
    );
}

/// The shadowing declaration does not have to be a template. `lookupSymbol`
/// finds `C::Foo`, a `Function`, and `deduceInstance' _ e = e`
/// (`Type.hs:1448`) does no deduction at all — the last clause is a
/// catch-all, not a "keep looking".
#[test]
fn a_member_function_shadows_a_top_level_template_of_the_same_name() {
    accepts(
        "template <typename T>\n\
         inline void Foo(T x, T y)\n\
         {\n\
         }\n\
         \n\
         class C\n\
         {\n\
         public:\n\
         inline void Foo(bool x, float32 y)\n\
         {\n\
         }\n\
         \n\
         inline void go()\n\
         {\n\
         bool a;\n\
         float32 b;\n\
         Foo(a, b);\n\
         }\n\
         }\n",
    );
}

/// `symbol e@Variable{} = insert e` (`Symbols.hs:64`) keys a local under
/// `…::main::$::Foo`, which is the first key `lookupInScope` probes from
/// inside the block. A local binding shadows a top-level template exactly as
/// a member does.
#[test]
fn a_local_binding_shadows_a_top_level_template_of_the_same_name() {
    accepts(
        "template <typename T>\n\
         inline void Foo(T x, T y)\n\
         {\n\
         }\n\
         \n\
         inline void main()\n\
         {\n\
         bool a;\n\
         float32 b;\n\
         auto Foo = [](bool p, float32 q) { };\n\
         Foo(a, b);\n\
         }\n",
    );
}

/// The control for the three above: the same call from inside a class that
/// declares *nothing* named `Foo` still reaches the top-level template. The
/// guard declines a nearer binding, not a nested call site.
#[test]
fn a_call_from_a_scope_that_declares_no_such_name_still_reaches_module_scope() {
    rejects(
        "template <typename T>\n\
         inline void Foo(T x, T y)\n\
         {\n\
         }\n\
         \n\
         class C\n\
         {\n\
         public:\n\
         inline void go()\n\
         {\n\
         bool a;\n\
         float32 b;\n\
         Foo(a, b);\n\
         }\n\
         }\n",
    );
}

/// A call to a name nothing declares is pass 9's verdict and nothing
/// else: `lookupSymbol`'s miss is `deduceInstance' _ e = e`
/// (`Type.hs:1448`), no deduction, so the arguments' disagreement is no
/// conflict — against the file's own template or any other (`kg-oracle
/// --errors`, 2026-10-10: "Symbol name not in scope: `Foo`" at 10:1). The
/// CST pass's text door had no pass 9 and an `import` it could not load;
/// the rooted door loads it, and fails it, so it is left out here.
///
/// The file declares a template of its own so that the program's templates
/// are *not* empty.
#[test]
fn an_unknown_template_is_left_alone() {
    let text = "template <typename T>\n\
         inline void Bar(T x, T y)\n\
         {\n\
         }\n\
         \n\
         inline void main()\n\
         {\n\
         bool a;\n\
         float32 b;\n\
         Foo(a, b);\n\
         }\n";
    assert_eq!(errors(text), ["Symbol name not in scope: `Foo`"]);
}

/// `withSymbols` is `getSymbols`, which folds every file of the program
/// (`Frontend.hs:71`, `Symbols.hs:37-41`) — so the template may live in a
/// module the caller merely imports. This is the whole reason the corpus
/// moves: `test/syntax/template-argument-deduction.k` declares every one of
/// its templates in `helper/template_argument_deduction.k`.
///
/// It also pins the `Templates` top-level rule. The helper below declares
/// `Fn_T_T` twice, once at module scope and once inside `class C`, exactly
/// as the real helper does; an index that took both would see a duplicate,
/// poison the name and report nothing.
#[test]
fn a_template_from_an_imported_module_is_deduced() {
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![kg_base::Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    db.set_file_text(
        &kg_base::Utf8PathBuf::from("/proj/helper.k"),
        "module helper\n    { Fn_T_T\n    , C\n    }\n\n\
         template <typename T>\n\
         inline T Fn_T_T(T x, T y)\n\
         {\n\
         return x;\n\
         }\n\n\
         class C\n\
         {\n\
         public:\n\
         template <typename T>\n\
         T Fn_T_T(T x, T y)\n\
         {\n\
         return x;\n\
         }\n\
         }\n"
            .to_owned(),
    );
    let main = db.set_file_text(
        &kg_base::Utf8PathBuf::from("/proj/main.k"),
        "import helper\n\n\
         inline void main()\n\
         {\n\
         bool a;\n\
         float32 b;\n\
         Fn_T_T(a, b);\n\
         }\n"
            .to_owned(),
    );
    let messages: Vec<String> =
        db.check(&[main]).into_iter().filter(|x| x.is_error()).map(|x| x.message).collect();
    assert_eq!(
        messages,
        vec![
            "Conflicting template argument deduction results for parameter `T`. \
             Could be `float` or `bool`."
        ]
    );
}

mod deduced_value {
    use crate::tests::program;

    /// A deduced `auto N` completes the call, so the instance materialises
    /// and the call's type reaches pass 19 (`kg-oracle --errors`,
    /// 2026-09-10, `scratchpad/el/b.k`, `c.k`; the corpus's
    /// `template-argument-deduction.k::55,76`).
    #[test]
    fn a_deduced_dimension_completes_the_call() {
        let at = |text: &str| -> Vec<String> {
            program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
        };
        assert_eq!(
            at("template <typename T, auto N>\ninline T[N+1] Fn_T_N_T_N(T[N] x, T[N] y)\n{\n    T[N+1] z;\n    z[0] = x[0];\n    return z;\n}\n\ninline void main()\n{\n    uint3[10] x;\n    uint2[10] y;\n\n    Fn_T_N_T_N(x, y);\n}\n"),
            vec!["Invalid conversion from `uint2[10]` to `uint3[10]` @ main.k:y"]
        );
        assert_eq!(
            at("enum Enum : uint8\n{\n    Zero\n}\n\ntemplate <typename T, auto N>\ninline T[N+1] Fn_T_T_N(T x, T[N] y)\n{\n    T[N+1] z;\n    z[0] = x;\n    z[1] = y[0];\n    return z;\n}\n\ninline Enum[10] Run()\n{\n    Enum[10] es;\n\n    return Fn_T_T_N(Enum::Zero, es);\n}\n"),
            vec!["Invalid conversion from `Enum[11]` to `Enum[10]` @ main.k:Fn_T_T_N(Enum::Zero, es)"]
        );
    }

    /// `Go(Second<bool, uint32>)` against `Go((Tuple2<T1, T2>) -> T1 fn)`:
    /// the argument is the copy `Second<|$bool|$uint@32|>`, typed
    /// `(Tuple2<|$bool|$uint@32|>) -> uint32`; the return type binds `T1` to
    /// `uint32`, the parameter's mangled leaf binds it to `bool`
    /// (the CST engine's `mangled_instance` then), and the reconciliation is the
    /// conflict, new first (`template-argument-deduction-from-instance.k::6`;
    /// `kg-oracle --errors` 2026-09-17).
    ///
    /// `Go` is higher-order, so the argument is wrapped and its closure is
    /// `data.closure.core::Closure` — which the reference always has, the
    /// base library importing `data.closure` (`library/base.k:36`). The unit
    /// harness loads no base library; without the module the program-wide
    /// loop's `Closure` is undefined and nothing is typed to conflict (M4,
    /// 2026-09-26), so the module is given as its neighbours give it.
    #[test]
    fn a_copy_typed_argument_deduces_through_its_mangled_leaf() {
        let core = "module data.closure.core\n    { Closure\n    , make_capture0\n    }\n\ntemplate <typename FunctionType, FunctionType fn, typename Capture>\nusing Closure = Capture;\n\ninline auto make_capture0\n    (\n    )\n{\n    struct Capture\n    {\n    }\n\n    Capture capture;\n\n    return capture;\n}\n";
        let h = "module h { Tuple2, Second, Go }\n\ntemplate <typename T1, typename T2>\nstruct Tuple2\n{\n    T1 first;\n    T2 second;\n}\n\ntemplate <typename T1, typename T2>\ninline T2 Second(Tuple2<T1, T2> x)\n{\n    return x.second;\n}\n\ntemplate <typename T1, typename T2>\ninline void Go((Tuple2<T1, T2>) -> T1 fn)\n{\n}\n";
        let main = "import h\nimport data.closure.core\n\ninline void main()\n{\nGo(Second<bool, uint32>);\n}\n";
        let out: Vec<String> = crate::tests::program(&[("data/closure/core.k", core), ("h.k", h), ("main.k", main)])
            .into_iter()
            .filter(|(m, _, _)| !m.starts_with("The import is redundant"))
            .map(|(m, at, _)| format!("{m} @ {at}"))
            .collect();
        assert_eq!(
            out,
            vec!["Conflicting template argument deduction results for parameter `T1`. Could be `bool` or `uint32`. @ main.k:Go"]
        );
    }
}
