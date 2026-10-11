//! Pass 12, `unresolvedTemplates` (`Frontend.hs:409-432`), at the rooted
//! door — the loop's at a module's text since sprint W3 step 1
//! (`kg_sem::check::TextPass::UnresolvedTemplates`). Each case is a
//! `kg-oracle --errors` probe (2026-10-10), the notes' positions included.

use crate::tests::{program, rooted_accepts as accepts, rooted_errors as errors, rooted_rejects as rejects};

/// `message @ file:the text its range covers`, and each note after it.
fn at(main: &str) -> Vec<String> {
    program(&[("main.k", main)])
        .into_iter()
        .map(|(m, at, labels)| {
            let notes: String = labels.iter().map(|l| format!(" [{l}]")).collect();
            format!("{m} @ {at}{notes}")
        })
        .collect()
}

/// `errorMsgWithContext (note $ unfix t)` (`Frontend.hs:419`): the
/// template's declaration, from its first token — past its documentation,
/// which the lexer drops and the IR's range takes in: an abbreviated
/// template's is its function's, comment and all (`basics.k::3`'s
/// `println`, noted at `inline`).
#[test]
fn the_template_is_noted_from_its_first_token() {
    let text = "// Takes anything.\n//\n// Twice documented.\ninline void take(auto x)\n{\n}\n\ninline void main()\n{\n    take(clog2(0));\n}\n";
    let got = at(text);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[0].starts_with("Could not deduce all arguments for template `take` @ main.k:take"), "{got:?}");
    assert!(got[0].ends_with("[main.k:inline void take(auto x)\n{\n}: while compiling]"), "{got:?}");
    assert_eq!(got[1], "Unresolved argument @ main.k:clog2(0)");
}

/// The reference's flag stops pass 12 once pass 9 erred (`Frontend.hs:
/// 85-87`): `take(1)`'s "Could not deduce all arguments" is not shown beside
/// `nosuch`. The CST pass 4 reported it where the list was decided, before
/// any checkpoint, and showed both.
#[test]
fn pass_12_says_nothing_once_pass_9_erred() {
    let text = "template <typename T, typename U>\ninline void take(T x)\n{\n}\n\ninline void main()\n{\n    take(1);\n    nosuch();\n}\n";
    assert_eq!(at(text), ["Symbol name not in scope: `nosuch` @ main.k:nosuch"]);
}

/// A template declared inside a copy carries the copy's note too: `note $
/// unfix t` is a `SrcStack` there (`Error.hs:185-189`), so `A<bool>::A2_t<2,
/// 3>` is noted at `A2_t`'s text and then at `A<bool>`.
#[test]
fn a_template_in_a_copy_is_noted_at_its_instance_too() {
    let text = "template <typename T>\nclass A\n{\npublic:\n    template <auto N>\n    using A2_t = T[N];\n}\n\nconst A<bool>::A2_t<2, 3> x = {false, true};\n";
    let got = at(text);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("Too many arguments specified for template `A2_t` @ main.k:A2_t<2, 3>"), "{got:?}");
    assert!(got[0].ends_with("[main.k:using A2_t = T[N];: while compiling] [main.k:A<bool>: while compiling]"), "{got:?}");
}

// The tests below were the CST passes' — pass 12's three arms
// (`passes/unresolved_templates.rs`) and pass 4's LT verdict
// (`passes/deduce_template_args.rs`) — at the text door, which runs no
// loop and has no pass 12 now. Each is a corpus shape or a `kg-oracle
// --errors` probe they recorded, read here against the loop's pass 12.

/// `deduceTemplateArgs` binds nothing from a `clog2` of a runtime value —
/// typed `TUnresolved` for good (`Type.hs:751`) — so an abbreviated callee
/// given one is "Could not deduce all arguments" (`basics.k::20,23`,
/// `println`), and a folding `clog2` deduces. The reference's second error
/// there, pass 11's "Unresolved argument" on the argument
/// (`unresolvedTemplateCall`, `Frontend.hs:437-444`), has no version at a
/// module's text but the folded `clog2`'s (`kg-oracle --errors`).
#[test]
fn a_never_typed_argument_is_a_deduction_verdict() {
    let show = "inline void Show(auto x)\n{\n}\n\n";
    let got = errors(&format!("{show}inline void main()\n{{\n    uint32 x = 3;\n    Show(clog2(x));\n}}\n"));
    assert_eq!(got.first().map(String::as_str), Some("Could not deduce all arguments for template `Show`"), "{got:?}");
    accepts(&format!("{show}inline void main()\n{{\n    Show(clog2(8));\n}}\n"));
}

/// `kg-oracle --errors`, 2026-09-07: `a.Go()` against `inline void
/// Go(auto x)` is "Could not deduce all arguments for template `Go`" at
/// the member name, `while compiling` at the function; the same for a
/// free abbreviated function. An argument for every `auto` binds.
#[test]
fn an_abbreviated_function_called_short_is_could_not_deduce_all() {
    let member = "class A\n{\npublic:\n    inline void Go(auto x)\n    {\n    }\n}\n\ninline void main()\n{\n    static A a;\n    a.Go();\n}\n";
    let got = at(member);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("Could not deduce all arguments for template `Go` @ main.k:Go [main.k:inline void Go(auto x)"), "{got:?}");
    assert!(got[0].ends_with(": while compiling]"), "{got:?}");
    rejects("inline void Go(auto x, auto y)\n{\n}\n\ninline void main()\n{\n    Go(1);\n}\n");
    accepts("inline void Go(auto x)\n{\n}\n\ninline void main()\n{\n    Go(1);\n}\n");
    accepts("class A\n{\npublic:\n    inline void Go(auto x)\n    {\n    }\n}\n\ninline void main()\n{\n    static A a;\n    a.Go(true);\n}\n");
}

/// The LT verdict's `while compiling` frame is the template's function
/// (`Type.hs:1409,1462`; `kg-oracle --errors` on `Fn_T_N()`).
#[test]
fn the_lt_verdict_names_the_template_as_its_frame() {
    let text = "template <typename T, auto N>\ninline auto Fn_T_N()\n{\n    T[N] a;\n    return a;\n}\n\ninline uint32 Run()\n{\n    return Fn_T_N();\n}\n";
    let got = at(text);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("Could not deduce all arguments for template `Fn_T_N` @ main.k:Fn_T_N [main.k:inline auto Fn_T_N()"), "{got:?}");
    assert!(got[0].ends_with(": while compiling]"), "{got:?}");
}

/// `sequence` (`Type.hs:1407`): one parameter that deduction could not bind
/// discards the whole argument list, error included. `U` appears nowhere in
/// the signature, so `Foo`'s conflict on `T` never reaches the tree — the
/// reference reports this program through pass 12's "Could not deduce all
/// arguments" instead (`Frontend.hs:421`), and *only* that: the conflict
/// was in the list `sequence` threw away. Oracle: exactly this message at
/// `Foo`.
#[test]
fn an_undeducible_parameter_withdraws_the_whole_argument_list() {
    assert_eq!(
        errors(
            "template <typename T, typename U>\n\
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
        vec!["Could not deduce all arguments for template `Foo`"]
    );
}

/// The LT verdict through the member path. Oracle: this message at the
/// member name (`template-argument-deduction.k::120`).
#[test]
fn a_member_call_that_binds_nothing_is_could_not_deduce_all() {
    assert_eq!(
        errors(
            "class Class\n{\npublic:\ntemplate <typename T>\nT Fn_T_10(T[10] x)\n{\nreturn x[0];\n}\n}\n\n\
             inline void main()\n{\nstatic Class o;\no.Fn_T_10(10);\n}\n"
        ),
        vec!["Could not deduce all arguments for template `Fn_T_10`"]
    );
}

/// `zipWithM_` (`Type.hs:1477`) stops at the shorter list: `U` is
/// mentioned only by the formal the call never supplies, so it stays
/// unbound and the verdict is the LT arm, not an arity error. Oracle:
/// exactly this message at `Foo`.
#[test]
fn too_few_arguments_leave_a_parameter_unbound() {
    assert_eq!(
        errors(
            "template <typename T, typename U>\n\
             inline void Foo(T x, U y)\n\
             {\n\
             }\n\
             \n\
             inline void main()\n\
             {\n\
             bool a;\n\
             Foo(a);\n\
             }\n"
        ),
        vec!["Could not deduce all arguments for template `Foo`"]
    );
}

/// `deduceType`'s `TInitializer` clause (`Type.hs:1535-1541`) deduces `T`
/// from an initializer list's elements, and the oracle *accepts* this text:
/// an LT verdict here would be invented.
#[test]
fn an_undecided_argument_declines_the_verdict() {
    accepts(
        "template <typename T>\ninline T Fn_T_10(T[10] x)\n{\nreturn x[0];\n}\n\n\
         inline void main()\n{\nFn_T_10({1, 2, 3});\n}\n",
    );
}

/// The sibling of [`an_undeducible_parameter_withdraws_the_whole_argument_list`]
/// with only the second parameter's *spelling* changed. `C` is a
/// template-template parameter with no written default, so
/// `substituteDefault` inserts `Nothing` for it (`Type.hs:1422-1423`),
/// `sequence` fails and `fromMaybe args` throws the argument list away —
/// error included. Oracle: the LT message at `Foo`, as for the sibling.
#[test]
fn an_undeducible_template_template_parameter_withdraws_the_argument_list() {
    assert_eq!(
        errors(
            "template <typename T, template <typename, auto> typename C>\n\
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
        vec!["Could not deduce all arguments for template `Foo`"]
    );
}

const GO: &str = "struct Foo\n{\n    uint32 a;\n    bool b;\n}\n\ntemplate <typename T, T x>\ninline T Go()\n{\n    return x;\n}\n\n";

/// A declaration whose type asks for its own waits on itself
/// (`type-identifier.k::23`); a `decltype` of another variable is an
/// ordinary argument.
#[test]
fn a_decltype_of_its_own_declaration_is_unresolved() {
    let foo = "template <typename T>\nclass Foo\n{\n}\n\n";
    assert_eq!(
        errors(&format!("{foo}inline void main()\n{{\n    static Foo<decltype(foo)> foo;\n}}\n")),
        vec!["Unresolved argument for the parameter T of template Foo"]
    );
    accepts(&format!("{foo}inline void main()\n{{\n    uint8 b;\n    static Foo<decltype(b)> foo;\n}}\n"));
}

/// A mux argument is decided where `evaluateConstexpr` decides it
/// (`Constexpr.hs:311-314`): a constant selector takes its case, a
/// runtime one leaves the mux unfolded (`constexpr.k::69,70`); the
/// case not taken is not looked at (`::72`, `expected:0`).
#[test]
fn a_mux_argument_is_unresolved_by_what_it_selects() {
    let t1 = "template <auto N>\nclass T1\n{\n}\n\n";
    let unresolved = vec!["Unresolved argument for the parameter N of template T1"];
    let main = |arg: &str| format!("{t1}inline void main()\n{{\n    int32 x;\n    static T1<{arg}> foo;\n}}\n");
    assert_eq!(errors(&main("x == 0 ? 1 : 0")), unresolved);
    assert_eq!(errors(&main("1 == 1 ? x : 0")), unresolved);
    accepts(&main("1 == 0 ? x : 1"));
}

/// A selector written as literals and out of range types the mux
/// `TUnresolved` (`Type.hs:1237-1238`) and it never folds, so the
/// argument is this pass's to report, not the folder's "Invalid
/// selector" (`constexpr.k::6,75`, `kg-oracle --errors` 2026-10-10) —
/// and outside a template argument nothing reports it at all: the
/// reference's frontend compiles `uint32 x = mux(4, 0, 1, 2, 3);`.
#[test]
fn an_out_of_range_literal_selector_is_an_unresolved_argument() {
    let t = "template <uint8 N>\nclass T1\n{\n}\n\ntemplate <bool B>\nclass T3\n{\n}\n\n";
    let main = |body: &str| {
        let text = format!("{t}inline void main()\n{{\n    {body}\n}}\n");
        program(&[("main.k", &text)]).into_iter().map(|(m, at, _)| (m, at)).collect::<Vec<_>>()
    };
    let unresolved = |p: &str, t: &str, at: &str| {
        vec![(format!("Unresolved argument for the parameter {p} of template {t}"), format!("main.k:{at}"))]
    };
    assert_eq!(main("static T1<2 ? 3 : 5> foo;"), unresolved("N", "T1", "2 ? 3 : 5"));
    assert_eq!(main("static T3<mux(4, true, true, true, true)> foo;"), unresolved("B", "T3", "mux(4, true, true, true, true)"));
    assert_eq!(main("uint32 x = mux(4, 0, 1, 2, 3);"), []);
}

/// `Go<Foo, x>` over a variable whose type is not `const`: the value is
/// the program's, not the compiler's, so `constexprResolved` refuses it
/// however constant its initializer looks (`struct-constexpr.k::2,10`;
/// `kg-oracle --errors` 2026-09-10, at `x`).
#[test]
fn a_runtime_variable_is_an_unresolved_argument() {
    let text = format!("{GO}inline void main()\n{{\n    Foo x = {{10, false}};\n    Go<Foo, x>();\n}}\n");
    assert_eq!(errors(&text), vec!["Unresolved argument for the parameter x of template Go"]);
}

/// An initializer list carrying a parameter, nested or not
/// (`struct-constexpr.k::9,12`).
#[test]
fn an_initializer_list_over_a_parameter_is_unresolved() {
    let text = format!("{GO}inline void Run(bool b)\n{{\n    Go<Foo, {{10, b}}>();\n}}\n");
    assert_eq!(errors(&text), vec!["Unresolved argument for the parameter x of template Go"]);
    let nested = format!("{GO}inline void Run(bool b)\n{{\n    struct Bar\n    {{\n        Foo x;\n        Foo y;\n    }}\n    Go<Bar, {{{{10, b}}, {{11, true}}}}>();\n}}\n");
    assert_eq!(errors(&nested), vec!["Unresolved argument for the parameter x of template Go"]);
}

/// `template Nope<bool>()` where nothing declares `Nope`: pass 9 judges an
/// instance's arguments only (`Frontend.hs:96-97`), and pass 12 reports the
/// reference's "Undefined template" at the name (`kg-oracle --errors`
/// 2026-09-15, 3:14; `dependent-template.k::8`, where the only `Go` is
/// under a dead `static if`).
#[test]
fn a_dependent_template_nothing_declares_is_undefined() {
    assert_eq!(
        program(&[("main.k", "inline void main()\n{\n    template Nope<bool>();\n}\n")])
            .into_iter()
            .map(|(m, at, _)| format!("{m} @ {at}"))
            .collect::<Vec<_>>(),
        vec!["Undefined template `Nope` @ main.k:Nope"]
    );
    // A template the keyword can reach is not undefined: under a live
    // top-level `static if`, or a member of the enclosing class under
    // one (`dependent-template.k::6`, `this-capture.k::15`); a member of
    // some other class is out of reach (`dependent-template.k::8`, where
    // the only in-scope `Go` sits under a dead branch).
    // `obj.template Go<uint32>()` with no `obj`: the receiver is the
    // reference's scoped name `obj::Go`, and pass 9 says nothing.
    assert_eq!(
        errors("class B\n{\npublic:\n    template <typename T>\n    T Go()\n    {\n        return 1;\n    }\n}\n\ninline void main()\n{\n    obj.template Go<uint32>();\n}\n"),
        vec!["Undefined template `Go`"]
    );
    // And on a receiver whose class has a plain `Go()` where a template is
    // named: the same lookup fails (`dependent-template.k::5`).
    assert_eq!(
        errors("class Foo\n{\npublic:\n    inline void Go()\n    {\n    }\n}\n\ninline void main()\n{\n    static Foo obj;\n    obj.template Go<uint32>();\n}\n"),
        vec!["Undefined template `Go`"]
    );
    accepts("static if (true)\n{\n    template <typename T>\n    inline void Go()\n    {\n        T x;\n    }\n}\n\ninline void main()\n{\n    template Go<bool>();\n}\n");
    accepts("class Foo\n{\npublic:\n    inline void Run()\n    {\n        auto x = template Do<10>();\n    }\n\n    static if (true)\n    {\n        template <auto N>\n        uint32 Do()\n        {\n            return N;\n        }\n    }\n}\n");
    assert_eq!(
        errors("class Other\n{\npublic:\n    template <typename T>\n    inline void Go()\n    {\n    }\n}\n\nstatic if (false)\n{\n    template <typename T>\n    inline void Go()\n    {\n    }\n}\n\ninline void main()\n{\n    template Go<bool>();\n}\n"),
        vec!["Undefined template `Go`"]
    );
}

/// `C::M<…>` — a plain class's member template, found by
/// `lookupSymbol` over the whole scoped name (`Frontend.hs:411-412`) —
/// is judged for arity at the member's own segment
/// (`kg-oracle --errors` 2026-09-13).
#[test]
fn a_class_member_template_is_judged_for_arity() {
    let text = "class C\n{\npublic:\n    template <auto N> using W = uint32[N];\n}\n\nconst C::W<2, 3> x = {0, 1};\n";
    assert_eq!(errors(text), vec!["Too many arguments specified for template `W`"]);
    accepts("class C\n{\npublic:\n    template <auto N> using W = uint32[N];\n}\n\nconst C::W<2> x = {0, 1};\n");
}

/// `auto` as a type argument to a class template (`type-check.k::38`).
#[test]
fn auto_as_a_type_argument_is_unresolved() {
    let text = "template <typename T, T X>\nclass Foo\n{\npublic:\n    T x;\n}\n\ninline void main()\n{\n    static Foo<auto, false> x;\n}\n";
    assert_eq!(errors(text), vec!["Unresolved argument for the parameter T of template Foo"]);
}

/// A `mux` whose predicate folds selects one case before anything judges
/// the other (`Constexpr.hs:311-314`): `1 == 0 ? x : 1` over a runtime
/// `x` is the constant `1` (`constexpr.k::72,73`, both `expected:0`).
#[test]
fn a_folded_mux_over_a_runtime_case_is_no_verdict() {
    let t1 = "template <auto N>\nclass T1\n{\npublic:\n    uint32 v;\n}\n\n";
    accepts(&format!("{t1}inline void main()\n{{\n    int32 x;\n    static T1<1 == 0 ? x : 1> foo;\n}}\n"));
    accepts(&format!("{t1}inline void main()\n{{\n    int32 x;\n    static T1<2 == 2 ? 1 : x> foo;\n}}\n"));
    // A `const` local is a constant.
    accepts(&format!("{t1}inline void main()\n{{\n    const uint32 c = 3;\n    static T1<c> foo;\n}}\n"));
}

/// `c.c1s.Foo<uint32>(x)` on an array of classes: the parser cannot
/// look `Foo` up in an array and accepts the list; pass 12 rejects
/// the name (`member-function-template.k::20`, `kg-oracle --errors`
/// 2026-09-17).
#[test]
fn a_member_template_named_on_an_array_is_undefined() {
    let text = "class C1\n{\npublic:\n    template <typename T>\n    T Foo(T x)\n    {\n        return x;\n    }\n}\n\nclass C2\n{\npublic:\n    C1[4] c1s;\n}\n\ninline uint32 Do(uint32 x)\n{\n    static C2 c;\n    return c.c1s.Foo<uint32>(x);\n}\n";
    assert_eq!(errors(text), vec!["Undefined template `Foo`"]);
}

/// `Frontend.hs:430`, `kg-oracle --errors` 2026-09-08: a template-template
/// parameter given a type, a literal or a function is "Failed to
/// instantiate template" at the instance, `while compiling` at the class;
/// given a template it compiles.
#[test]
fn a_template_template_parameter_given_no_template_fails_to_instantiate() {
    let head = "template <typename T>\nstruct S1\n{\n    T x;\n}\n\ntemplate <template <typename> typename TT>\nclass T1\n{\npublic:\n    TT<uint32> x;\n}\n\ninline uint32 Do()\n{\n    return 1;\n}\n\n";
    for arg in ["bool", "10", "Do"] {
        let got = at(&format!("{head}inline void main()\n{{\n    static T1<{arg}> a;\n}}\n"));
        assert_eq!(got.len(), 1, "{arg}: {got:?}");
        assert!(got[0].starts_with(&format!("Failed to instantiate template @ main.k:T1<{arg}> [main.k:class T1")), "{arg}: {got:?}");
    }
    accepts(&format!("{head}inline void main()\n{{\n    static T1<S1> a;\n}}\n"));
}

/// The GT arm in its two written positions — a type
/// (`test/syntax/basics.k:628`, `expected:1`) and a call
/// (`test/syntax/function-template.k:39`, `expected:1`, reduced to one
/// file) — and a correct instance silent.
#[test]
fn an_over_long_instance_is_rejected() {
    rejects("template <uint32 N>\nclass Foo\n{\n}\n\nusing A = Foo<3, 4>;\n");
    rejects("template <typename T>\ninline T f(T x)\n{\n    return x;\n}\n\ninline void main()\n{\n    uint32 y = f<uint32, 5>(3);\n}\n");
    accepts("template <uint32 N>\nclass Foo\n{\n}\n\nusing A = Foo<3>;\n");
}

/// The LT arm on a class template (`test/syntax/using.k:136`,
/// `expected:1`): nothing but a default can fill a class template's
/// missing slot, and `N` has none. Oracle: exactly this message at `Foo`.
/// A defaulted tail completes the list (`substituteDefault`,
/// `Type.hs:1422-1428`), and an alias template is the same arm
/// (`using.k`'s `MyInt<>`).
#[test]
fn an_under_long_class_instance_is_could_not_deduce_all() {
    assert_eq!(
        errors("template <typename T, uint32 N>\nclass Foo\n{\n}\n\nusing A = Foo<uint32>;\n"),
        vec!["Could not deduce all arguments for template `Foo`"]
    );
    accepts("template <typename T, uint32 N = 3>\nclass Foo\n{\n}\n\nusing A = Foo<uint32>;\n");
    assert_eq!(
        errors("template <uint8 N>\nusing MyInt = int<N>;\n\ninline void main()\n{\nMyInt<> x;\n}\n"),
        vec!["Could not deduce all arguments for template `MyInt`"]
    );
}

/// A function template's missing slots are pass 4's to fill from the call
/// (`Type.hs:1450-1462`): here `N` *is* deducible from the argument, and
/// the oracle accepts the text.
#[test]
fn a_function_templates_under_long_call_is_deduced() {
    accepts(
        "template <typename T, auto N>\ninline T f(T[N] x)\n{\n    return x[0];\n}\n\ninline void main()\n{\n    uint4[3] a;\n    uint4 y = f<uint4>(a);\n}\n",
    );
}

/// What pass 12 never judges, each accepted by the oracle: the leftover
/// original of an abbreviated template, deleted at pass 15 errors and all
/// (`removeTemplate`, `Frontend.hs:300-301`); a template's body and its
/// parameters' defaults (`Frontend.hs:414`, `para`, the whole subtree);
/// a `mux` case the fold did not select and an untaken `static if` branch
/// (`Constexpr.hs:311-314`, `Inference.hs:64`); a member template of an
/// enclosing class, which `lookupSymbol` finds before a top-level one of
/// another arity (`Symbols.hs:82-87`); and a qualified instance resolved
/// through a copy's member.
#[test]
fn what_pass_12_never_judges() {
    accepts("template <uint32 N>\nclass Foo\n{\n}\n\ninline auto f(auto x)\n{\n    static Foo<1, 2> y;\n    return x;\n}\n");
    accepts("template <uint32 N>\nclass Foo\n{\n}\n\ntemplate <typename T>\nstruct W\n{\n    Foo<1, 2> x;\n}\n");
    accepts("template <uint32 N>\nclass Foo\n{\n}\n\ntemplate <typename T, typename U = Foo<1, 2>>\nclass W\n{\n}\n");
    accepts("template <typename T>\ninline T f(T x)\n{\n    return x;\n}\n\ninline void main()\n{\n    uint32 y = mux(0, 10, f<uint32, 5>(3));\n}\n");
    accepts("template <uint32 N>\nclass Foo\n{\n}\n\nconst auto P = true;\n\ninline void main()\n{\n    static if (P)\n    {\n        uint32 x;\n    }\n    else\n    {\n        static Foo<1, 2> y;\n    }\n}\n");
    accepts("template <uint32 N>\nclass Foo\n{\n}\n\nclass Baz\n{\npublic:\n    template <typename T, typename S>\n    class Foo\n    {\n    }\n\n    inline void Run()\n    {\n        static Foo<uint32, uint32> x;\n    }\n}\n\ninline void main()\n{\n    static Baz b;\n}\n");
    accepts("template <typename T>\nclass A\n{\npublic:\n    template <typename U, typename V>\n    class Pair_t\n    {\n    }\n}\n\ntemplate <typename U>\nusing Pair_t = U[4];\n\ninline void main()\n{\n    static A<bool>::Pair_t<bool, bool> y;\n}\n");
}
