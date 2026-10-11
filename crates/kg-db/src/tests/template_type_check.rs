//! Pass 8's `templateTypeCheck` (`TypeCheck.hs:218-297`) at the rooted
//! door — the loop's census at a module's text since sprint W3 step 1
//! (`kg_sem::check::TextPass::TemplateTypeCheck`). Each case is a
//! `kg-oracle --errors` probe (2026-10-10), the position included.

use crate::tests::{program, rooted_accepts};

/// `message @ file:the text its range covers` for every diagnostic.
fn at(main: &str) -> Vec<String> {
    program(&[("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
}

const FOO: &str = "template <typename T, T X>\nclass Foo\n{\npublic:\n    T x;\n}\n\n";

/// `validate typ arg = validateType (theType typ) …` (`TypeCheck.hs:
/// 290-293`) over any argument, a float literal included: `Foo<int32,
/// 10.0>` is refused at `10.0` (`type-check.k::25`), where the census
/// asked a written integer, boolean or string only and instantiated the
/// copy, reporting a conversion inside it.
#[test]
fn a_float_argument_against_an_integer_parameter_is_refused() {
    assert_eq!(
        at(&format!("{FOO}inline void main()\n{{\n    static Foo<int32, 10.0> a;\n}}\n")),
        ["Invalid argument for template parameter `X`.\nExpected a constant expression of type:\n\tint32\nbut the argument type is:\n\tfloat @ main.k:10.0"]
    );
}

/// `zipWith validateArg params args` (`TypeCheck.hs:226`): each argument
/// is its own verdict. `Foo<10, 10>` refuses `T`, which wants a type, and
/// `X`, whose type is `T` substituted — `resolveTypeParam` puts the
/// argument's node there, and `theType` of a value is `TError "Expected a
/// type"` (`Type.hs:458-464`; `type-check.k::19`).
#[test]
fn each_argument_is_validated_on_its_own() {
    assert_eq!(
        at(&format!("{FOO}inline void main()\n{{\n    static Foo<10, 10> b;\n}}\n")),
        [
            "Invalid argument for template parameter `T`.\nExpected a type @ main.k:10",
            "Invalid argument for template parameter `X`.\nExpected a constant expression of type:\n\tExpected a type\nbut the argument type is:\n\tuint4 @ main.k:10",
        ]
    );
}

/// `infer (ParamIntF x) = try (TType . TSigned) $ fromIntegralConstexpr x`
/// (`Type.hs:1246`): a zero width is a type, `int0`, and a literal is
/// re-stamped only to a width above zero (`TypeCheck.hs:281-282`), so `10`
/// for `int<0> X` is refused (`type-check.k::40`).
#[test]
fn a_zero_width_parameter_is_int0() {
    let nfoo = "template <auto N, int<N> X>\nclass NFoo\n{\npublic:\n    uint32 x;\n}\n\n";
    assert_eq!(
        at(&format!("{nfoo}inline void main()\n{{\n    static NFoo<0, 10> c;\n}}\n")),
        ["Invalid argument for template parameter `X`.\nExpected a constant expression of type:\n\tint0\nbut the argument type is:\n\tuint4 @ main.k:10"]
    );
    rooted_accepts(&format!("{nfoo}inline void main()\n{{\n    static NFoo<8, 10> c;\n}}\n"));
}

/// `validate Auto InitializerListF{}` (`TypeCheck.hs:277`;
/// `initializer-list.k::48`).
#[test]
fn an_initializer_list_for_an_auto_parameter_is_refused() {
    assert_eq!(
        at("template <auto N>\nstruct Bar\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    Bar<{0}> d;\n}\n"),
        ["Invalid argument for template parameter `N`.\nInitializer list can't be used as an argument for `auto` parameter @ main.k:{0}"]
    );
}

/// `validateTemplate` (`TypeCheck.hs:243-257`): a template-template
/// argument's parameters match the parameter's own kind by kind — as
/// written, or as `theType` with the template's parameters substituted —
/// any left over defaulted (`template-template-parameter.k::3,16`).
#[test]
fn a_template_template_argument_matches_kind_by_kind() {
    let head = "template <typename T>\nstruct S1\n{\n    T x;\n}\n\ntemplate <typename T, auto N, bool B = true>\nstruct S3\n{\n    T[N] x;\n}\n\ntemplate <bool B>\nstruct S4\n{\n    uint32 x;\n}\n\ntemplate <template <typename, auto> typename TT>\nclass T1\n{\npublic:\n    TT<uint32, 4> x;\n}\n\ntemplate <typename T, template <T> typename TT>\nclass T4\n{\npublic:\n    TT<false> x;\n}\n\n";
    let main = |body: &str| at(&format!("{head}inline void main()\n{{\n    {body}\n}}\n"));
    assert_eq!(
        main("static T1<S1> a;"),
        ["Invalid argument for template parameter `TT`.\nExpected a template with parameters:\n\t<typename, auto>\nbut the argument has parameters:\n\t<typename T> @ main.k:S1"]
    );
    assert_eq!(
        main("static T4<uint32, S4> c;"),
        ["Invalid argument for template parameter `TT`.\nExpected a template with parameters:\n\t<uint32>\nbut the argument has parameters:\n\t<bool B> @ main.k:S4"]
    );
    assert_eq!(main("static T1<S3> b;"), Vec::<String>::new());
    assert_eq!(main("static T4<bool, S4> d;"), Vec::<String>::new());
    // A parameter past the wanted list's without a default does not match,
    // and a default is spelled after its parameter.
    let t2 = "template <template <typename> typename TT>\nclass T2\n{\npublic:\n    TT<uint32> x;\n}\n\n";
    assert_eq!(
        at(&format!("{head}{t2}inline void main()\n{{\n    static T2<S3> e;\n}}\n")),
        ["Invalid argument for template parameter `TT`.\nExpected a template with parameters:\n\t<typename>\nbut the argument has parameters:\n\t<typename T, auto N, bool B = true> @ main.k:S3"]
    );
}

// The CST pass's own tests, at the rooted door since its deletion (sprint
// W3 step 1): the census's verdicts are the reference's, each case its
// `kg-oracle --errors` probe as written.

mod validate {
    use crate::tests::{rooted_accepts as accepts, rooted_errors as errors};

    /// `Foo<10, 10>` for `template <typename T, T X>`: `T` is not given a
    /// type, and `X`'s declared type — `T` — resolves to the error type
    /// `tType` makes of it, spelled in the second report
    /// (`type-check.k::19`; `kg-oracle --errors` 2026-09-17).
    #[test]
    fn a_constant_for_a_typename_parameter_is_the_error_type_its_mentions_resolve_to() {
        assert_eq!(
            errors("template <typename T, T X>\nclass Foo\n{\n}\n\ninline void main()\n{\nstatic Foo<10, 10> x;\n}\n"),
            vec![
                "Invalid argument for template parameter `T`.\nExpected a type",
                "Invalid argument for template parameter `X`.\nExpected a constant expression of type:\n\tExpected a type\nbut the argument type is:\n\tuint4",
            ]
        );
    }

    /// `member-function-template.k::12`, `kg-oracle --errors` 2026-09-08: a
    /// member template's written instance is validated like a path's.
    #[test]
    fn a_member_templates_written_instance_is_validated() {
        let text = "class T2\n{\npublic:\n    template <typename T, (T) -> uint32 Fn>\n    uint32 Bar(T x)\n    {\n        return Fn(x);\n    }\n}\n\ninline uint32 Go(uint32 x)\n{\n    return 2 * x;\n}\n\ninline uint32 Do(uint32 x)\n{\n    static T2 c;\n    return c.Bar<bool, Go>(false);\n}\n";
        assert_eq!(
            crate::tests::rooted_errors(text),
            vec!["Invalid argument for template parameter `Fn`.\nExpected a function of type:\n\t(bool) -> uint32\nbut the argument type is:\n\t(uint32) -> uint32"]
        );
        crate::tests::rooted_accepts("class T2\n{\npublic:\n    template <typename T, (T) -> uint32 Fn>\n    uint32 Bar(T x)\n    {\n        return Fn(x);\n    }\n}\n\ninline uint32 Go(bool x)\n{\n    return 2;\n}\n\ninline uint32 Do(uint32 x)\n{\n    static T2 c;\n    return c.Bar<bool, Go>(false);\n}\n");
    }

    /// `TypeCheck.hs:269-275`, `kg-oracle --errors` 2026-09-07 on the four
    /// probes: a parameter type mismatch, a substituted `(T) -> T`, a method
    /// where a free function is expected, and a match.
    #[test]
    fn a_function_argument_is_checked_against_the_parameters_function_type() {
        let mismatch = "template <(bool) -> uint32 Fn>\ninline uint32 Foo()\n{\n    return Fn(true);\n}\n\ninline uint32 Bar(uint4 x)\n{\n    return x;\n}\n\ninline uint32 main()\n{\n    return Foo<Bar>();\n}\n";
        assert_eq!(
            crate::tests::rooted_errors(mismatch),
            vec!["Invalid argument for template parameter `Fn`.\nExpected a function of type:\n\t(bool) -> uint32\nbut the argument type is:\n\t(uint4) -> uint32"]
        );
        let substituted = "template <typename T, (T) -> T Fn>\ninline T Foo(T x)\n{\n    return Fn(x);\n}\n\ninline uint3 Id(uint3 x)\n{\n    return x;\n}\n\ninline uint32 main()\n{\n    return Foo<uint32, Id>(1);\n}\n";
        assert_eq!(
            crate::tests::rooted_errors(substituted),
            vec!["Invalid argument for template parameter `Fn`.\nExpected a function of type:\n\t(uint32) -> uint32\nbut the argument type is:\n\t(uint3) -> uint3"]
        );
        let method = "class C\n{\npublic:\n    uint32 M(bool x)\n    {\n        return 1;\n    }\n}\n\ntemplate <(bool) -> uint32 Fn>\ninline uint32 Foo()\n{\n    return Fn(true);\n}\n\ninline uint32 main()\n{\n    static C c;\n    return Foo<c.M>();\n}\n";
        assert_eq!(
            crate::tests::rooted_errors(method),
            vec!["Invalid argument for template parameter `Fn`.\nExpected a non-member function"]
        );
        crate::tests::rooted_accepts("template <(bool) -> uint32 Fn>\ninline uint32 Foo()\n{\n    return Fn(true);\n}\n\ninline uint32 Baz(bool x)\n{\n    return 1;\n}\n\ninline uint32 main()\n{\n    return Foo<Baz>();\n}\n");
        // A named type bound to `T` reads through the table: `(E) -> E`
        // (`enum.k::21`, `kg-oracle --errors` 2026-09-08).
        let named = "enum E : uint8\n{\n    A\n}\n\ntemplate <typename T, (T) -> T F>\ninline T Fn1(T x)\n{\n    return F(x);\n}\n\ninline int32 F2(int32 x)\n{\n    return x;\n}\n\ninline void main()\n{\n    Fn1<E, F2>(E::A);\n}\n";
        assert_eq!(
            crate::tests::rooted_errors(named),
            vec!["Invalid argument for template parameter `F`.\nExpected a function of type:\n\t(E) -> E\nbut the argument type is:\n\t(int32) -> int32"]
        );
        // A lambda's reference type is the desugared closure's, a call no
        // template argument resolves to: pass 11's "Unresolved argument"
        // (`kg-oracle --errors` with the closure library, 2026-10-10:
        // 11:16), where the text door, running no pass 11, said nothing.
        assert_eq!(
            crate::tests::closured_errors("template <(bool) -> uint32 Fn>\ninline uint32 Foo()\n{\n    return Fn(true);\n}\n\ninline uint32 main()\n{\n    return Foo<[](uint4 x) -> uint32 { return x; }>();\n}\n"),
            ["Unresolved argument for the parameter Fn of template Foo"]
        );
    }

    /// `N::T1<10>` through an import alias: the qualifier is stripped as
    /// `lookupSymbol` strips it (`Cx::instance_qualified`), and the instance
    /// is validated like a bare one (`kg-oracle --errors` 2026-09-13,
    /// `scratchpad/seam/m3.k`, 5:18).
    #[test]
    fn an_alias_qualified_instance_s_arguments_are_validated() {
        let h = "module h { T1 }\n\ntemplate <typename T>\nclass T1\n{\npublic:\n    T v;\n}\n";
        assert_eq!(
            crate::tests::program(&[("main.k", "import h as N\n\ninline void main()\n{\n    static N::T1<10> c;\n}\n"), ("h.k", h)])
                .into_iter()
                .map(|(m, at, _)| format!("{m} @ {at}"))
                .collect::<Vec<_>>(),
            vec!["Invalid argument for template parameter `T`.\nExpected a type @ main.k:10"]
        );
    }

    /// The corpus's own `template <typename T> inline T F4()` called as
    /// `F4<4>()` — `test/syntax/constexpr.k:397`, `expected:1`. A literal is
    /// `templateArgResolved` (`TypeCheck.hs:320-321`), so the guard passes and
    /// `validate Typename arg | otherwise` fires (`TypeCheck.hs:239`).
    #[test]
    fn a_literal_for_a_typename_parameter_is_rejected() {
        assert_eq!(
            errors(
                "template <typename T>\n\
                 inline T F4()\n\
                 {\n\
                 return 3;\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 F4<4>();\n\
                 }\n"
            ),
            vec!["Invalid argument for template parameter `T`.\nExpected a type"]
        );
    }

    /// `helper/type_check.k:22` with `test/syntax/type-check.k:137` —
    /// `template <typename T, T X>` given `Foo<int32, int32>`. `X`'s declared
    /// type is `T`, which `Instance::substitute` replaces with the
    /// written `int32`; the argument is then a type in a non-type position,
    /// `validate _ _ | tIsType` (`TypeCheck.hs:266-267`).
    #[test]
    fn a_type_for_a_non_type_parameter_is_rejected() {
        assert_eq!(
            errors(
                "template <typename T, T X>\n\
                 class Foo\n\
                 {\n\
                 const T x = X;\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 static Foo<int32, int32> x;\n\
                 }\n"
            ),
            vec![
                "Invalid argument for template parameter `X`.\n\
                 The type can't be used as an argument for non-type template parameter"
            ]
        );
    }

    /// A function-typed parameter's position folded over the earlier
    /// arguments (`resolveTemplateParamType`, `TypeCheck.hs:349-362`):
    /// `(int<clog2(bytesizeof (uint<bitsizeof T>)) + 1>) -> void F` under `T
    /// = uint32` expects `(int3) -> void` (`template.k::27`). Red with the
    /// `dependent` fallback cut.
    #[test]
    fn a_function_parameters_dependent_position_is_folded() {
        let foo = |arg: &str| {
            format!(
                "template <typename T, (int<clog2(bytesizeof (uint<bitsizeof T>)) + 1>) -> void F>\nclass Foo\n{{\n}}\n\ninline void Fn({arg} x)\n{{\n}}\n\ninline void main()\n{{\n    static Foo<uint32, Fn> foo;\n}}\n"
            )
        };
        assert_eq!(
            errors(&foo("int2")),
            vec!["Invalid argument for template parameter `F`.\nExpected a function of type:\n\t(int3) -> void\nbut the argument type is:\n\t(int2) -> void"]
        );
        accepts(&foo("int3"));
    }

    /// A function-typed parameter given a type meets the same clause first:
    /// it precedes `validate typ@FunctionType{}` (`TypeCheck.hs:266-269`),
    /// so `T2<int32>` is not a function mismatch (`type-check.k::45`).
    #[test]
    fn a_type_for_a_function_parameter_is_rejected() {
        assert_eq!(
            errors(
                "template <(uint32) -> int32 F>\n\
                 class T2\n\
                 {\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 static T2<int32> t2;\n\
                 }\n"
            ),
            vec![
                "Invalid argument for template parameter `F`.\n\
                 The type can't be used as an argument for non-type template parameter"
            ]
        );
    }

    /// `test/syntax/type-check.k:172`. `bool` is not a subtype of `int32`, so
    /// `validate typ arg` reports both types unmangled
    /// (`TypeCheck.hs:289-294`).
    #[test]
    fn a_constant_of_the_wrong_type_names_both_types() {
        assert_eq!(
            errors(
                "template <typename T, T X>\n\
                 class Foo\n\
                 {\n\
                 const T x = X;\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 static Foo<int32, false> x;\n\
                 }\n"
            ),
            vec![
                "Invalid argument for template parameter `X`.\n\
                 Expected a constant expression of type:\n\tint32\n\
                 but the argument type is:\n\tbool"
            ]
        );
    }

    /// The other side of the same clause: an int literal against a sized
    /// integer parameter is *re-stamped* with the parameter's width and
    /// **truncated**, not rejected (`TypeCheck.hs:280-285`). `1000` types as
    /// `uint10` (`Type.hs:485-490`) and is not a subtype of `uint8`, so without
    /// that clause the subtyping one below rejects a program the reference
    /// silently narrows.
    ///
    /// Synthetic: `type-check.k:141`'s `Foo<int32, 10>` is the corpus's own
    /// `expected:0` for this clause, but a literal that already fits proves
    /// nothing — `uint4 <: int32` accepts it either way.
    #[test]
    fn an_oversized_int_literal_is_truncated_not_rejected() {
        accepts(
            "template <uint8 N>\n\
             class Foo\n\
             {\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static Foo<1000> x;\n\
             }\n",
        );
    }

    /// `test/syntax/template-template-parameter.k:230`, `expected:1`. `T1`'s
    /// parameter takes one `typename`; `Foo` declares an `auto`, and
    /// `matchParamKind` falls through both of its clauses
    /// (`TypeCheck.hs:249-251`).
    ///
    /// The expectation was right and its stated reason was not: `theType` is
    /// `tType . typeOf` (`Type.hs:458-464`), so `typeOf Typename = TUntyped`
    /// (`Type.hs:1319`) is *resolved* (`Type.hs:1005`) and not a `TType`,
    /// leaving `theType Typename = TError "Expected a type"`. `theType Auto` is
    /// `TUnresolved` (`Type.hs:1174`, `:1004`). The two still differ — which is
    /// why the test passed while the reasoning under it was wrong, and why
    /// `the_type_written` exists.
    #[test]
    fn a_template_argument_with_the_wrong_parameter_kind_is_rejected() {
        assert_eq!(
            errors(
                "template <typename X>\n\
                 struct S1\n\
                 {\n\
                 X x;\n\
                 }\n\
                 \n\
                 template <template <typename> typename TT = S1>\n\
                 class T1\n\
                 {\n\
                 public:\n\
                 TT<uint32> x;\n\
                 }\n\
                 \n\
                 template <auto N>\n\
                 class Foo{}\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 static T1<Foo> t;\n\
                 }\n"
            ),
            vec![
                "Invalid argument for template parameter `TT`.\n\
                 Expected a template with parameters:\n\t<typename>\n\
                 but the argument has parameters:\n\t<auto N>"
            ]
        );
    }

    /// `test/syntax/template-template-parameter.k:33`, `expected:1`. Arity, not
    /// kind: `matchParams` runs out of the argument's parameters with one of its
    /// own left, which matches only the error clause (`TypeCheck.hs:247-248`).
    #[test]
    fn a_template_argument_with_too_few_parameters_is_rejected() {
        assert_eq!(
            errors(
                "template <typename X>\n\
                 struct S2\n\
                 {\n\
                 X x;\n\
                 }\n\
                 \n\
                 template <auto N, template <typename, auto> typename TT>\n\
                 class T3\n\
                 {\n\
                 public:\n\
                 TT<uint32, N> x;\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 static T3<5, S2> t;\n\
                 }\n"
            ),
            vec![
                "Invalid argument for template parameter `TT`.\n\
                 Expected a template with parameters:\n\t<typename, auto>\n\
                 but the argument has parameters:\n\t<typename X>"
            ]
        );
    }

    /// The same shape with the extra parameter *defaulted* — `matchParams []
    /// ys | all hasDefault ys` (`TypeCheck.hs:247`). This is
    /// `test/syntax/template-template-parameter.k:24`'s `S3`, `expected:0`, and
    /// it is what stops the arity rule from rejecting every abbreviated
    /// argument.
    #[test]
    fn a_template_argument_whose_extra_parameters_default_is_accepted() {
        accepts(
            "template <typename X>\n\
             struct S1\n\
             {\n\
             X x;\n\
             }\n\
             \n\
             template <typename X, auto N = 10>\n\
             struct S3\n\
             {\n\
             X[N] x;\n\
             }\n\
             \n\
             template <template <typename> typename TT = S1>\n\
             class T1\n\
             {\n\
             public:\n\
             TT<uint32> x;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static T1<S3> t;\n\
             }\n",
        );
    }

    /// `validate Auto InitializerListF{}` (`TypeCheck.hs:277`) —
    /// `test/syntax/initializer-list.k:431`, `expected:1`.
    #[test]
    fn an_initializer_list_for_an_auto_parameter_is_rejected() {
        assert_eq!(
            errors(
                "template <auto N>\n\
                 struct Foo\n\
                 {\n\
                 uint32 x;\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 Foo<{0}> foo;\n\
                 }\n"
            ),
            vec![
                "Invalid argument for template parameter `N`.\n\
                 Initializer list can't be used as an argument for `auto` parameter"
            ]
        );
    }

    /// Silence 5. `S1<Foo>` names a struct where a type is wanted and is
    /// correct; the reference's parser has sorted it into `TypeIdentifierF`,
    /// and only its `NamedValueF` sibling is "did you forget 'typename`"
    /// (`TypeCheck.hs:234-235`). Both are one `PATH` here, so the clause is not
    /// reproduced — and this is the program that would break if it were.
    #[test]
    fn a_named_type_as_a_typename_argument_is_accepted() {
        accepts(
            "struct Foo\n\
             {\n\
             uint32 x;\n\
             }\n\
             \n\
             template <typename T>\n\
             struct S1\n\
             {\n\
             T x;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static S1<Foo> s;\n\
             }\n",
        );
    }

    /// Written arguments shorter than the parameter list are what
    /// `deduceTemplateArgs` fills in two entries earlier in pass 8's own list
    /// (`Type.hs:1422-1428`) — here the default `N = 10` — and the written
    /// argument is then refused (`kg-oracle --errors`, 2026-10-10: 9:12).
    /// The CST pass, with no rewritten list, declined the instance (its
    /// silence 1); the loop's census has the list.
    #[test]
    fn a_short_argument_list_is_completed_and_validated() {
        let text = "template <typename T, auto N = 10>\n\
             struct Foo\n\
             {\n\
             T[N] x;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static Foo<10> f;\n\
             }\n";
        assert_eq!(errors(text), ["Invalid argument for template parameter `T`.\nExpected a type"]);
    }

    /// The subtyping clause's `typeOf argExp` is the *folded* literal's own
    /// type — `inferIntLiteralType` of the value (`Type.hs:485-491`) — never
    /// the table's entry for the unfolded spelling, which for a `bitsizeof`
    /// is `TUnresolved` (`kg-ty/src/width.rs` types the unfolded operator so).
    /// Message pinned against `kg-oracle --errors`, 2026-08-14: the argument
    /// type is `uint4`, the minimal type of the folded `8`.
    #[test]
    fn a_folded_argument_fails_the_subtyping_clause_at_its_folded_type() {
        let errs = crate::tests::rooted_errors(
            "template <bool B>\n\
             struct WB\n\
             {\n\
             uint32 x;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static WB<bitsizeof(uint8)> w;\n\
             }\n",
        );
        assert_eq!(
            errs,
            vec![
                "Invalid argument for template parameter `B`.\n\
                 Expected a constant expression of type:\n\tbool\n\
                 but the argument type is:\n\tuint4"
                    .to_string()
            ],
        );
    }

    /// The re-stamp clause matches the *folded* literal (`TypeCheck.hs:280`
    /// runs after `evaluateConstexpr`), so a constant under a sized parameter
    /// is truncated in silence, never pushed to the subtyping clause — where
    /// its minimal type (`uint9` for 300) would fail `<: uint8` and invent an
    /// error on a program the reference accepts. Oracle, 2026-08-14:
    /// `W2<K>` for `const uint32 K = 300` compiles clean and materialises
    /// `W2<|@44|>`.
    #[test]
    fn a_folded_constant_under_a_sized_parameter_is_restamped_not_rejected() {
        accepts(
            "template <uint8 N>\n\
             struct W2\n\
             {\n\
             uint32 x;\n\
             }\n\
             \n\
             const uint32 K = 300;\n\
             \n\
             inline void main()\n\
             {\n\
             static W2<K> w;\n\
             }\n",
        );
    }

    /// Silence 2, lifted. This test asserted a clean compile while the
    /// silence held; the reference *rejects* — `evaluateConstexpr` rewrites
    /// `K` to `4` (`Constexpr.hs:286-293`), the instance resolves, and
    /// `validate Typename arg` fires on the literal (`TypeCheck.hs:237-239`).
    /// Verdict and message pinned against
    /// `kg-oracle --errors` on this exact text, 2026-08-14:
    ///
    /// ```text
    /// Invalid argument for template parameter `T`.
    /// Expected a type
    /// ```
    #[test]
    fn a_named_constant_argument_resolves_and_fails_the_typename_clause() {
        let errs = crate::tests::rooted_errors(
            "const uint32 K = 4;\n\
             \n\
             template <typename T>\n\
             inline T F4()\n\
             {\n\
             return 3;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             F4<K>();\n\
             }\n",
        );
        assert_eq!(
            errs,
            vec![
                "Invalid argument for template parameter `T`.\nExpected a type"
                    .to_string()
            ],
        );
    }

    /// `lookupInScope` tries every enclosing scope before module scope
    /// (`Symbols.hs:82-87`), so a local `S1` is what this block's `S1<…>`
    /// names — not the top-level template. The two take the same number of
    /// arguments in the opposite order, so the arity gate does not save it:
    /// without the shadow guard the pass reports two invented errors against a
    /// declaration the reference never reaches. The reference's parser
    /// rejects the line outright — "unexpected S1" at 10:8, `S1` being no
    /// type there (`kg-oracle --errors`, 2026-10-10) — and the loop rejects
    /// it at the same name as pass 12's (divergence #7), never with the
    /// top-level template's verdicts.
    #[test]
    fn a_local_name_shadowing_a_template_is_declined() {
        let text = "template <auto N, typename X>\n\
             struct S1\n\
             {\n\
             X[N] x;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             uint32 S1;\n\
             static S1<uint32, 4> y;\n\
             }\n";
        assert_eq!(errors(text), ["`S1`: undefined template"]);
    }

    /// The untaken branch of a `static if` **does not exist** by the time
    /// `templateTypeCheck` runs: `staticControl` rewrites
    /// `StaticIfF (BoolLiteral False) _ x` to the else-branch or `SeqF []`
    /// (`Constexpr.hs:47-48`) inside `infer` (`Inference.hs:50`), and
    /// `templateTypeCheck` is the *last* of `instantiateTemplates`' nine, after
    /// the `inferType` at `Template.hs:72`.
    ///
    /// This is the same body as
    /// [`a_template_argument_with_the_wrong_parameter_kind_is_rejected`] — the
    /// corpus's own `expected:1` — moved into a dead branch, so it pins the
    /// guard and nothing else. Without it this was `Error 1` on a correct file.
    #[test]
    fn an_untaken_static_if_branch_is_not_checked() {
        accepts(
            "template <typename X>\n\
             struct S1\n\
             {\n\
             X x;\n\
             }\n\
             \n\
             template <template <typename> typename TT = S1>\n\
             class T1\n\
             {\n\
             public:\n\
             TT<uint32> x;\n\
             }\n\
             \n\
             template <auto N>\n\
             class Foo{}\n\
             \n\
             inline void main()\n\
             {\n\
             static if (false)\n\
             {\n\
             static T1<Foo> t;\n\
             }\n\
             }\n",
        );
    }

    /// The other half of the same guard, and the other clause: a value
    /// parameter given a template name in a dead branch. `theType` of the
    /// argument is `TUnresolved` there, which the subtyping clause printed into
    /// the user's face as the literal word **unresolved** — a guard failing out
    /// loud.
    ///
    /// It reached that clause because `Cx::names_a_template` answers
    /// `tIsTemplate` syntactically, so `templateArgResolved` (`TypeCheck.hs:317`)
    /// passed in a region the reference's type table never visited.
    #[test]
    fn a_value_parameter_in_an_untaken_branch_is_not_checked() {
        accepts(
            "template <typename T>\n\
             struct Box\n\
             {\n\
             T x;\n\
             }\n\
             \n\
             template <uint32 N>\n\
             struct Arr\n\
             {\n\
             uint32 y;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static if (false)\n\
             {\n\
             static Arr<Box> a;\n\
             }\n\
             }\n",
        );
    }

    /// An **uninstantiated** template body is never inferred —
    /// `go (NotedExp n (TemplateF a b))` traverses `templateParams` only
    /// (`Inference.hs:63`) — so every node in it keeps the `Typed TUnresolved`
    /// that `noteType` stamped (`Type.hs:973-976`) and `templateArgsResolved`
    /// declines. The definition is then deleted by `removeTemplate`
    /// (`Frontend.hs:299`).
    ///
    /// `Never` is never instantiated, so `T1<Foo>` inside it is written but
    /// never read. Same wrong-kind pair as above; without the guard, an
    /// invented `Error 1`.
    #[test]
    fn an_uninstantiated_template_body_is_not_checked() {
        accepts(
            "template <typename X>\n\
             struct S1\n\
             {\n\
             X x;\n\
             }\n\
             \n\
             template <template <typename> typename TT = S1>\n\
             class T1\n\
             {\n\
             public:\n\
             TT<uint32> x;\n\
             }\n\
             \n\
             template <auto N>\n\
             class Foo{}\n\
             \n\
             template <typename Unused>\n\
             class Never\n\
             {\n\
             public:\n\
             T1<Foo> t;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             uint32 z = 0;\n\
             }\n",
        );
    }

    /// `matchParamKind`'s second clause is `theType t1 == theType t2`
    /// (`TypeCheck.hs:251`), and `theType` is **not** `typeOf`
    /// (`Type.hs:458-464`). `typeOf` of a `template <…> typename` signature is
    /// `TTemplate` (`Type.hs:1267`), which is *not* `tResolved`
    /// (`Type.hs:1002`), so `tType` collapses it to `TUnresolved` — exactly what
    /// `theType auto` is (`Type.hs:1174`, `:1004`).
    ///
    /// So an `auto` slot in the expected signature **matches** a
    /// template-template parameter in the argument, and `Z<W>` compiles. Reading
    /// `Type.hs:1267` as `theType` rather than `typeOf` had this hard-coded as a
    /// mismatch; see `TheType`. The census read the `auto` slot as
    /// `TAuto`, its reading inside a function type: red with that back.
    #[test]
    fn an_auto_slot_matches_a_template_template_parameter() {
        accepts(
            "template <typename X, template <typename> typename Y>\n\
             struct W\n\
             {\n\
             X a;\n\
             }\n\
             \n\
             template <template <typename, auto> typename TT>\n\
             class Z\n\
             {\n\
             public:\n\
             uint32 dummy;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static Z<W> z;\n\
             }\n",
        );
    }

    /// A **template** name where a type is wanted. `tIsType TTemplate` is
    /// `False` — `tInner TTemplate = Nothing` (`Type.hs:599-602`), so
    /// `maybeInner` fails and `tIsType` (`Type.hs:620-621`) says no — and
    /// `validate Typename`'s `otherwise` fires (`TypeCheck.hs:239`).
    ///
    /// Its sibling [`a_named_type_as_a_typename_argument_is_accepted`] is the
    /// program this must *not* reject: a struct name, whose `typeOf` is `TType`.
    /// Together they pin the one predicate that separates them. The census
    /// asked `tResolved` of the argument first, which `TTemplate` is not
    /// (`Type.hs:1002`), and said nothing: red with that guard back
    /// (`kg-oracle --errors`, 2026-10-10: 15:11).
    #[test]
    fn a_template_name_as_a_typename_argument_is_rejected() {
        assert_eq!(
            errors(
                "template <typename T>\n\
                 struct S1\n\
                 {\n\
                 T x;\n\
                 }\n\
                 \n\
                 template <typename T>\n\
                 struct S2\n\
                 {\n\
                 T y;\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 static S1<S2> s;\n\
                 }\n"
            ),
            vec!["Invalid argument for template parameter `T`.\nExpected a type"]
        );
    }

    /// `constexprResolved (Designator _ e)` is `constexprResolved e` — the name
    /// is thrown away (`Constexpr.hs:388`). Reading the `DESIGNATOR`'s children
    /// uniformly asked it of the `NAME_REF` too, which is `False`, so a
    /// *designated* initializer declined where a positional one did not:
    /// `Q<{0}>` reported the `auto` clause and `Q<{.z = 1}>` said nothing.
    #[test]
    fn a_designated_initializer_for_an_auto_parameter_is_rejected() {
        assert_eq!(
            errors(
                "template <auto N>\n\
                 struct Q\n\
                 {\n\
                 uint32 y;\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 static Q<{.z = 1}> a;\n\
                 }\n"
            ),
            vec![
                "Invalid argument for template parameter `N`.\n\
                 Initializer list can't be used as an argument for `auto` parameter"
            ]
        );
    }

    /// Silence 6. `test/syntax/struct.k:22-38` is `expected:0` and instantiates
    /// a **member** template through its class: `static Foo::Bar<uint3, 10> b;`.
    /// Transposed here so the member and a top-level template of the same simple
    /// name take their parameters in the opposite order, which is what makes the
    /// misresolution visible rather than merely wrong — the arity gate hides it
    /// when the counts differ.
    ///
    /// `lookupSymbol` resolves the *written* name, qualifier included
    /// (`TypeCheck.hs:224`, `Symbols.hs:37-44`); `Templates` is keyed on
    /// the simple name over top-level declarations, so it answered with the
    /// wrong template and invented two errors. It now declines every qualified
    /// path.
    #[test]
    fn a_qualified_path_is_declined_rather_than_resolved_by_its_last_segment() {
        accepts(
            "template <typename T, auto N>\n\
             struct Bar\n\
             {\n\
             T[N] y;\n\
             }\n\
             \n\
             class Foo\n\
             {\n\
             public:\n\
             template <auto N, typename T>\n\
             struct Bar\n\
             {\n\
             T[N] x;\n\
             }\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static Foo::Bar<10, uint3> b;\n\
             }\n",
        );
    }

    /// The same guard on the *argument* side. `Cx::names_a_template`
    /// is `lookupSymbol arg symbols` (`TypeCheck.hs:245`) and resolved by the
    /// last segment too, so `T1<Foo::Bar>` read the **top-level** `Bar`'s
    /// parameter list and reported a kind mismatch against a signature the
    /// program never named. The instance itself is unqualified here, so only the
    /// argument's guard can be what saves it.
    #[test]
    fn a_qualified_path_is_declined_as_an_argument_too() {
        accepts(
            "template <auto N>\n\
             struct Bar\n\
             {\n\
             uint32[N] y;\n\
             }\n\
             \n\
             class Foo\n\
             {\n\
             public:\n\
             template <typename U>\n\
             struct Bar\n\
             {\n\
             U x;\n\
             }\n\
             }\n\
             \n\
             template <template <typename> typename TT>\n\
             class T1\n\
             {\n\
             public:\n\
             uint32 dummy;\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             static T1<Foo::Bar> t;\n\
             }\n",
        );
    }

    /// The control for the one above, and the reason it is not just "accept
    /// everything": `typename` in the expected signature against the same
    /// template-template parameter is `TError "Expected a type"` against
    /// `TUnresolved`, which differ — still `Error 1`. `showPretty` spells
    /// the template-template parameter with the hard break `src (Template a
    /// b)` puts after its list (`PrettyPrint.hs:159`; `kg-oracle --errors`,
    /// 2026-10-10: 16:10), where the CST pass collapsed it to one line; the
    /// census spelled no such parameter at all, and said nothing.
    #[test]
    fn a_typename_slot_does_not_match_a_template_template_parameter() {
        assert_eq!(
            errors(
                "template <typename X, template <typename> typename Y>\n\
                 struct W\n\
                 {\n\
                 X a;\n\
                 }\n\
                 \n\
                 template <template <typename, typename> typename TT>\n\
                 class Z\n\
                 {\n\
                 public:\n\
                 uint32 dummy;\n\
                 }\n\
                 \n\
                 inline void main()\n\
                 {\n\
                 static Z<W> z;\n\
                 }\n"
            ),
            vec![
                "Invalid argument for template parameter `TT`.\n\
                 Expected a template with parameters:\n\t<typename, typename>\n\
                 but the argument has parameters:\n\t\
                 <typename X, template <typename>\ntypename Y>"
            ]
        );
    }
}

mod member_through_instance {
    use crate::tests::{rooted_accepts as accepts, rooted_errors as errors};

    const FOO: &str = "template <typename T>\nclass Foo\n{\npublic:\n    template <T N>\n    struct S\n    {\n        uint32 x;\n    }\n    T v;\n}\n\n";

    /// `Foo<bool>::S<4>`: the member template's own list is validated
    /// under the class template's bindings — the copy `Foo<|bool|>` holds
    /// `template <bool N> struct S` (`kg-oracle --tree --passes 8`,
    /// 2026-09-11) — so `4` against `T = bool` is the reference's verdict at
    /// the argument (`nested-templates.k::8`); `true` is fine, and so is a
    /// plain class's member reached by name.
    #[test]
    fn a_member_template_through_a_class_instance_is_validated_under_its_bindings() {
        assert_eq!(
            errors(&format!("{FOO}inline void main()\n{{\n    static Foo<bool>::S<4> s;\n}}\n")),
            vec!["Invalid argument for template parameter `N`.\nExpected a constant expression of type:\n\tbool\nbut the argument type is:\n\tuint3"]
        );
        accepts(&format!("{FOO}inline void main()\n{{\n    static Foo<bool>::S<true> s;\n    static Foo<uint8>::S<4> t;\n}}\n"));
        assert_eq!(
            errors("class C\n{\npublic:\n    template <bool N>\n    struct S\n    {\n        uint32 x;\n    }\n}\n\ninline void main()\n{\n    static C::S<4> s;\n}\n"),
            vec!["Invalid argument for template parameter `N`.\nExpected a constant expression of type:\n\tbool\nbut the argument type is:\n\tuint3"]
        );
    }
}

mod written_parameter_type {
    use crate::tests::program;

    /// `F2<Foo, F1<Bar>>(foo)` over two function-local structs: the helper's
    /// copy of `F1<Bar>` is named `F1<|@main@$@Bar|>` — the leaf alias it
    /// spells for `Bar` mangled as what it stands for — so this module's
    /// twin finds it, and the copy's function type is checked against
    /// `(T) -> uint32` with `T` bound to `Foo` (`local-structs.k::9`;
    /// `kg-oracle --errors` 2026-09-17).
    #[test]
    fn a_copy_named_through_a_leaf_alias_is_found_and_checked() {
        let h = "module h { F1, F2 }\n\ntemplate <typename T>\ninline uint32 F1(T x)\n{\n    return bitsizeof x;\n}\n\ntemplate <typename T, (T) -> uint32 Fn>\ninline uint32 F2(T x)\n{\n    return Fn(x);\n}\n";
        let main = "import h\n\ninline void main()\n{\nstruct Foo\n{\nuint32 x;\n}\n\nFoo foo;\n\nstruct Bar\n{\nuint32 x;\n}\n\nF2<Foo, F1<Bar>>(foo);\n}\n";
        let out: Vec<String> = program(&[("h.k", h), ("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(
            out,
            vec!["Invalid argument for template parameter `Fn`.\nExpected a function of type:\n\t(main::$::Foo) -> uint32\nbut the argument type is:\n\t(main::$::Bar) -> uint32 @ main.k:F1<Bar>"]
        );
    }

    /// `F2<Foo, F1<Foo>>(bar)`: a copy as a function-typed argument. The
    /// census shapes it once the twin types its alias as the copy's
    /// function (the round the mention's alias argument triggers, and a
    /// typename parameter bound to a named type for the expected function
    /// type), and the owner spells it under its namespace, so the copy is
    /// named `F2<|@main@$@Foo@@helper@local_structs@F1<|@main@$@Foo|>|>`
    /// on both sides — then the call's argument is checked against the
    /// copy (`local-structs.k::10`; `kg-oracle --errors` 2026-09-17).
    #[test]
    fn a_copy_as_a_function_typed_argument_is_decided_and_the_call_checked() {
        let h = "module h { F1, F2 }\n\ntemplate <typename T>\ninline uint32 F1(T x)\n{\n    return bitsizeof x;\n}\n\ntemplate <typename T, (T) -> uint32 Fn>\ninline uint32 F2(T x)\n{\n    return Fn(x);\n}\n";
        let main = "import h\n\ninline void main()\n{\nstruct Foo\n{\nuint32 x;\n}\n\nFoo foo;\n\nstruct Bar\n{\nuint32 x;\n}\n\nBar bar;\n\nF2<Foo, F1<Foo>>(bar);\n}\n";
        let out: Vec<String> = program(&[("h.k", h), ("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(out, vec!["Invalid conversion from `main::$::Bar` to `main::$::Foo` @ main.k:bar"]);
    }

    /// `Foo<Capture, bool, Do>(capture, true)` with `Capture` and `Do` local
    /// to `Go`: the copy names `Do` by its hoisted name `Go@Do` — what the
    /// reference mangles (`Foo<|@Go@$@Capture$bool@Go@Do|>`) — minted as an
    /// alias the owner's twin types and pass 9 knows, so the copy compiles
    /// clean (`local-functions.k::5,6`, must-accepts; 2026-09-17).
    #[test]
    fn a_local_function_as_an_argument_is_named_by_its_hoisted_name() {
        let h = "module h { Foo }\n\ntemplate <typename T, typename R, (T, R) -> R Fn>\ninline R Foo(T x, R y)\n{\n    return Fn(x, y);\n}\n";
        let main = "import h\n\ninline void Go()\n{\nstruct Capture\n{\nuint32 x;\n}\n\ninline bool Do(Capture x, bool y)\n{\nreturn y ? x.x > 0 : x.x < 0;\n}\n\nCapture capture;\ncapture.x = 10;\nauto x = Foo<Capture, bool, Do>(capture, true);\n}\n";
        assert_eq!(program(&[("h.k", h), ("main.k", main)]), Vec::new());
    }

    /// `From<S<uint32>, FromS<S<uint32>>>(x)`: the argument is a copy whose
    /// own argument is a copy — `FromS<|@@h@S<|$uint@32|>|>` — and the
    /// verdict's spelling reads both back (`function-template-param.k::30`;
    /// `kg-oracle --errors` 2026-09-17).
    #[test]
    fn a_copy_of_a_copy_is_named_and_spelled_through() {
        let h = "module h { S, FromS, From }\n\ntemplate <typename T>\nstruct S\n{\n    T x;\n}\n\ntemplate <typename T>\ninline void FromS(S<T> x)\n{\n}\n\ntemplate <typename T, (T) -> void Fn>\ninline void From(T x)\n{\n    Fn(x);\n}\n";
        let main = "import h\n\ninline void main()\n{\nS<uint32> x;\nFrom<S<uint32>, FromS<S<uint32>>>(x);\n}\n";
        let out: Vec<String> = program(&[("h.k", h), ("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(
            out,
            vec!["Invalid argument for template parameter `Fn`.\nExpected a function of type:\n\t(h::S<uint32>) -> void\nbut the argument type is:\n\t(h::S<h::S<uint32>>) -> void @ main.k:FromS<S<uint32>>"]
        );
    }

    const H: &str = "module h { foo, T3, Bar, S, Go }\n\nusing foo = uint5;\n\ntemplate <foo N>\nclass T3\n{\npublic:\n    uint32 v;\n}\n\nenum Bar : int32\n{\n    B1\n}\n\nstruct S\n{\n    uint32 a;\n    bool b;\n}\n\ntemplate <typename T, T x>\ninline T Go()\n{\n    return x;\n}\n";

    fn messages(main: &str) -> Vec<String> {
        program(&[("main.k", main), ("h.k", H)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    /// `resolveTemplateParamType` (`TypeCheck.hs:349-362`): a value
    /// parameter's written type is what its module's table says — an alias
    /// over `uint5` (`type-check.k::51-53`), an imported enum
    /// (`enum.k::16,17`), a struct under `T x` (`struct-constexpr.k::4`) —
    /// and the argument is validated against it (`kg-oracle --errors`,
    /// 2026-09-11). A matching argument stays silent.
    #[test]
    fn a_value_parameter_s_written_type_is_read_off_its_module() {
        assert_eq!(
            messages("import h\n\ninline void main()\n{\n    static T3<10 != 2> t;\n}\n"),
            vec!["Invalid argument for template parameter `N`.\nExpected a constant expression of type:\n\tuint5\nbut the argument type is:\n\tbool @ main.k:10 != 2"]
        );
        assert_eq!(
            messages("import h\n\ntemplate <Bar B>\nclass Holder\n{\npublic:\n    Bar b = B;\n}\n\ninline void main()\n{\n    static Holder<0> h;\n}\n"),
            vec!["Invalid argument for template parameter `B`.\nExpected a constant expression of type:\n\th::Bar\nbut the argument type is:\n\tuint1 @ main.k:0"]
        );
        assert_eq!(
            messages("import h\n\ninline void main()\n{\n    Go<S, {1, 2}>();\n}\n"),
            vec!["Invalid argument for template parameter `x`.\nExpected a constant expression of type:\n\th::S\nbut the argument type is:\n\t{uint1, uint2} @ main.k:{1, 2}"]
        );
        assert_eq!(
            messages("import h\n\ntemplate <Bar B>\nclass Holder\n{\npublic:\n    Bar b = B;\n}\n\ninline void main()\n{\n    static T3<10> t;\n    static Holder<Bar::B1> h;\n}\n"),
            Vec::<String>::new()
        );
    }
}

mod dependent_width {
    use crate::tests::{rooted_accepts as accepts, rooted_errors as errors};

    /// `test/syntax/type-check.k::40,41` (`kg-oracle --errors`, 2026-09-10):
    /// `int<N>` with `N` the first argument, checked against the second.
    #[test]
    fn a_value_parameter_whose_width_an_earlier_argument_decides() {
        let nfoo = "template <auto N, int<N> X>\nclass NFoo\n{\n    int<N> x = X;\n}\n\n";
        assert_eq!(
            errors(&format!("{nfoo}inline void main()\n{{\n    static NFoo<0, 10> x;\n}}\n")),
            vec!["Invalid argument for template parameter `X`.\nExpected a constant expression of type:\n\tint0\nbut the argument type is:\n\tuint4"]
        );
        assert_eq!(
            errors(&format!("{nfoo}inline void main()\n{{\n    static NFoo<10, 1.0> x;\n}}\n")),
            vec!["Invalid argument for template parameter `X`.\nExpected a constant expression of type:\n\tint10\nbut the argument type is:\n\tfloat"]
        );
        accepts(&format!("{nfoo}inline void main()\n{{\n    static NFoo<10, 3> x;\n}}\n"));
        accepts(&format!("{nfoo}inline void main()\n{{\n    static NFoo<4, -3> x;\n}}\n"));
    }
}
