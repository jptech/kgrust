//! Pass 9, `undefinedSymbols` (`Frontend.hs:82-157`), at the rooted door —
//! the loop's at a module's text since sprint W3 step 1
//! (`kg_sem::check::TextPass::UndefinedSymbols`). Each shape here was read
//! off `kg-oracle --errors`; where the reference's parser rejects first
//! (divergence #7), off what this port reports in its place. The modules
//! below the first tests were the CST pass's own, at the rooted door all
//! along, and moved here when its code was deleted (2026-10-10); each was
//! green against the loop's verdicts before the deletion.

use super::program;

fn errors(files: &[(&str, &str)]) -> Vec<(String, String)> {
    program(files).into_iter().map(|(m, at, _)| (m, at)).collect()
}

fn pairs(want: &[(&str, &str)]) -> Vec<(String, String)> {
    want.iter().map(|(m, at)| ((*m).to_owned(), (*at).to_owned())).collect()
}

/// `higherOrderFunctions` copies a function-typed parameter's type into
/// the template parameter it makes of it, so the undefined `is_last` in
/// `([[last]] is_last) -> void Do` is judged on the copy, whose identifier
/// the rewrite made — and the text at its range spells it, so it is
/// reported there (`higher-order-functions.k::210`: "Type name not in
/// scope: `is_last`" at 1:27). The rewrite's `make_capture0`, which no base
/// library declares here, is the reference's error too — at a note no text
/// spells — and is left out (`kg_sem::check::CopiedName`). Red with a
/// copied name never placed.
#[test]
fn a_name_a_rewrite_copied_is_reported_where_the_text_spells_it() {
    let main = "inline void Foo(([[last]] is_last) -> void Do)\n{\n}\n\ninline void Bar([[last]] bool is_last)\n{\n}\n\n\
        inline void main()\n{\n    Foo(Bar);\n}\n";
    assert_eq!(errors(&[("main.k", main)]), pairs(&[("Type name not in scope: `is_last`", "main.k:is_last")]));
}

/// A path the sorter refused — `opt<uint32>` where no parse-time template
/// `opt` opens the `<`, the reference's "unexpected '<'" (divergence #7) —
/// whose head nothing in scope declares: `h` declares `opt` and `make`, and
/// `main` reaches `h` only through `mid`, which exposes neither. Reported
/// as the type or the value it is written as, at the head, with pass 9's
/// hints. A head the parse-time table found is not called undefined:
/// `Do<3>(1)` over an imported variable `Do` is the reference's comparison
/// chain, which its parser rejects at the second `>` (`kg-oracle
/// --errors`), and which this port does not (divergence #7) — but `Do` is
/// in scope, and `lookupInScope` from the use's scope alone would not find
/// `[@h, Do]`. Red with the refused path unjudged, and with the head's
/// parse-time answer ignored ("Symbol name not in scope: `Do`, Perhaps you
/// meant: `Do` defined in h").
#[test]
fn a_refused_path_whose_head_nothing_declares_is_not_in_scope() {
    let h = "module h\n    { opt\n    , make\n    , Do\n    }\n\ntemplate <typename T>\nstruct opt\n{\n    T x;\n}\n\n\
        template <typename T>\ninline T make(T x)\n{\n    return x;\n}\n\nconst uint32 Do = 3;\n";
    let mid = "module mid\n    { g\n    }\n\nimport h\n\ninline uint32 g()\n{\n    return make<uint32>(1);\n}\n";
    let main = "import mid\n\ninline void f()\n{\n    opt<uint32> x;\n    auto y = make<uint32>(1);\n    uint32 z = g();\n}\n";
    let want = [
        ("Type name not in scope: `opt`", "main.k:opt"),
        ("Symbol name not in scope: `make`\nPerhaps you meant: `make` defined in h", "main.k:make"),
    ];
    assert_eq!(errors(&[("main.k", main), ("mid.k", mid), ("h.k", h)]), pairs(&want));
    let compare = "import h\n\ninline void f()\n{\n    uint32 y = Do<3>(1);\n}\n";
    let said = errors(&[("main.k", compare), ("h.k", h)]);
    assert!(!said.iter().any(|(m, _)| m.contains("not in scope")), "{said:?}");
}

/// What the CST pass judged by syntax and the reference never judges
/// (`kg-oracle --errors --no-base`, 2026-10-10, this program): a body
/// inference never types — a function whose parameter's type is unresolved
/// is inferred no further than its parameters (`Type/Inference.hs:58`),
/// so `static_cast`, `nope2` and `nope4` are no one's verdict, only `nope`
/// and `nope3` are; a template's body — a higher-order function's, a
/// generic lambda's — judged in its copies alone, framed, and never where
/// nothing instantiates it (`Frontend.hs:93-94`). And what the CST pass
/// skipped and the reference judges: every case and the selector of a
/// `mux` no fold decides, which stays standing (`Constexpr.hs:311-314`
/// replaces only a constant one). The reference's `make_capture0` and
/// `make_capture1`, minted by the rewrites with no base library to declare
/// them, are left out here (`kg_sem::check::CopiedName`). Red with a body
/// whose parameters are unresolved inferred, and with a `mux` skipped.
#[test]
fn pass_9_judges_what_inference_typed_and_every_case_of_a_standing_mux() {
    let main = "inline uint4 Inc(uint4 x)\n{\n    return x + 1;\n}\n\n\
        inline void Test((uint4)->uint4 oracle)\n{\n    nope_for(256, oracle(3));\n}\n\n\
        inline void Never((uint4)->uint4 oracle)\n{\n    nope_never(256, oracle(3));\n}\n\n\
        inline void main()\n{\n    Test(Inc);\n}\n\n\
        inline void Generic(uint8 t)\n{\n    const auto check = [t](auto x)\n    {\n        uint8 m = nope_generic(x);\n    };\n}\n\n\
        inline void Untyped(nope x)\n{\n    uint8 m = static_cast(x);\n    uint8 n = nope2;\n}\n\n\
        class C\n{\npublic:\n    void Run(nope3 x)\n    {\n        uint8 m = nope4;\n    }\n}\n\n\
        inline uint8 Ternary(uint8 x)\n{\n    return x == 2 ? nope_then : nope_else;\n}\n\n\
        inline uint8 Mux(uint8 x)\n{\n    return mux(nope_sel, nope_a, nope_b);\n}\n";
    let want = [
        ("Type name not in scope: `nope`", "main.k:nope"),
        ("Type name not in scope: `nope3`", "main.k:nope3"),
        ("Symbol name not in scope: `nope_then`", "main.k:nope_then"),
        ("Symbol name not in scope: `nope_else`", "main.k:nope_else"),
        ("Symbol name not in scope: `nope_sel`", "main.k:nope_sel"),
        ("Symbol name not in scope: `nope_a`", "main.k:nope_a"),
        ("Symbol name not in scope: `nope_b`", "main.k:nope_b"),
    ];
    assert_eq!(errors(&[("main.k", main)]), pairs(&want));
}

/// A hint offers a name whose *type* fits (`filter (cond . typeOf . snd)`,
/// `Frontend.hs:141`): an alias whose target is undefined is no type, so
/// `pair` is offered `pair_u` alone where the CST pass, partitioning by the
/// declaration's kind, offered `pair_t` too (`kg-oracle --errors`,
/// 2026-10-10). Red with an unresolved declaration counted as a type.
#[test]
fn a_hint_offers_only_a_name_whose_type_fits() {
    let main = "using pair_t = nope;\nusing pair_u = uint32;\n\ninline void f()\n{\n    pair x;\n}\n";
    let want = [
        ("Type name not in scope: `nope`", "main.k:nope"),
        ("Type name not in scope: `pair`\nPerhaps you meant: `pair_u`", "main.k:pair"),
    ];
    assert_eq!(errors(&[("main.k", main)]), pairs(&want));
}

/// The editor's per-file gate (`Db::diagnostics`) runs `main.k`'s passes
/// while another module of its program failed to load — `b`'s `import c`
/// finds nothing — and every name `c` would have supplied looks undefined
/// then. The reference never enters the frontend (`Main.hs:73-75`), and
/// pass 9 stands down program-wide, as the CST pass did; `main.k`'s other
/// verdicts stay. The same while another module does not parse — its
/// error symbol sets the flag `checkPar` reads (`Frontend.hs:48-52,85`).
/// Red with either half of the stand-down taken out.
#[test]
fn pass_9_stands_down_while_any_module_fails_to_load() {
    use kg_base::Utf8PathBuf;
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    db.set_file_text(&Utf8PathBuf::from("/proj/b.k"), "module b\n    { module c\n    }\n\nimport c\n".to_owned());
    let text = "import b\n\ninline void main()\n{\n    uint32 x = cthing;\n    static assert(false);\n}\n";
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
    let said: Vec<String> = db.diagnostics(&[main], main).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect();
    assert!(!said.iter().any(|m| m.contains("not in scope")), "{said:?}");
    assert!(!said.is_empty(), "the module's own verdicts stay");
    db.set_file_text(&Utf8PathBuf::from("/proj/b.k"), "module b\n    { g\n    }\n\ninline void g(\n".to_owned());
    let text = "import b\n\ninline void main()\n{\n    uint32 x = nosuch;\n    static assert(false);\n}\n";
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
    let said: Vec<String> = db.diagnostics(&[main], main).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect();
    assert!(!said.iter().any(|m| m.contains("not in scope")), "{said:?}");
    assert!(!said.is_empty(), "the module's own verdicts stay");
}

mod scope {
    use crate::tests::program;

    fn first(files: &[(&str, &str)]) -> Vec<String> {
        program(files).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    /// `hint` (`Frontend.hs:144-147`): a name its own module declares is
    /// in scope, and spelled bare — "Perhaps you meant: `foo`" — where one
    /// another module declares says which (`kg-oracle --errors`,
    /// 2026-10-10, both). Red with every module-scope name spelled with
    /// its module.
    #[test]
    fn a_hint_names_the_module_only_of_another_modules_name() {
        let main = "module a\n    { main\n    }\n\ninline void foo()\n{\n}\n\ninline void main()\n{\n    fob();\n}\n";
        assert_eq!(first(&[("main.k", main)]), ["Symbol name not in scope: `fob`\nPerhaps you meant: `foo` @ main.k:fob"]);
        let b = "module b\n    { foo\n    }\n\ninline void foo()\n{\n}\n";
        let main = "module a\n    { main\n    }\n\nimport b\n\ninline void main()\n{\n    fob();\n}\n";
        let out: Vec<String> = first(&[("main.k", main), ("b.k", b)]).into_iter().filter(|m| !m.starts_with("The import is redundant")).collect();
        assert_eq!(out, ["Symbol name not in scope: `fob`\nPerhaps you meant: `foo` defined in b @ main.k:fob"]);
    }

    /// A `case` body is `localScoped` (`Parser.hs:602`): its `y` is keyed
    /// `[main,$,$quasi0,y]`, and a use after the `switch` — at
    /// `[main,$,$quasi1]` once the block has closed — walks outward to
    /// nothing (`basics.k::114`; `kg-oracle --errors` 2026-09-10).
    #[test]
    fn a_case_block_s_local_is_not_visible_after_the_switch() {
        let text = "inline void main()\n{\n    uint32 x = 3;\n    switch (x)\n    {\n        case 3:\n            uint32 y = 8;\n            break;\n        default:\n            break;\n    }\n    uint32 z = y;\n}\n";
        assert_eq!(first(&[("main.k", text)]), vec!["Symbol name not in scope: `y` @ main.k:y"]);
    }

    /// A nested class from outside its class (`basics.k::47`), a class's
    /// alias from outside (`::48`), another function's local (`::373`).
    #[test]
    fn a_name_declared_in_a_scope_the_use_is_not_in_is_not_in_scope() {
        let nested = "class Foo\n{\npublic:\n    class Bar\n    {\n    }\n}\n\ninline void main()\n{\n    static Bar b;\n}\n";
        assert_eq!(first(&[("main.k", nested)]), vec!["Type name not in scope: `Bar` @ main.k:Bar"]);
        let alias = "class Holder\n{\npublic:\n    using MyType = uint3;\n}\n\ninline void main()\n{\n    static MyType x;\n}\n";
        assert_eq!(first(&[("main.k", alias)]), vec!["Type name not in scope: `MyType` @ main.k:MyType"]);
        let other = "inline uint32 Foo()\n{\n    myvar = 1;\n    return 3;\n}\n\ninline void main()\n{\n    uint32 myvar = 10;\n    uint32 result = Foo();\n}\n";
        assert_eq!(first(&[("main.k", other)]), vec!["Symbol name not in scope: `myvar` @ main.k:myvar"]);
    }

    /// A template's parameter list is judged, its body is not
    /// (`Frontend.hs:93-94`): a parameter's type naming nothing
    /// (`template.k::46`), a function type's parameter naming nothing
    /// (`::19`), and a parameter mentioned before its own declaration
    /// (`::4`) — while an earlier parameter, and everything in the body,
    /// stays silent.
    #[test]
    fn a_template_parameter_list_is_judged_in_order() {
        assert_eq!(
            first(&[("main.k", "template <Foo N>\nclass Bar\n{\n}\n")]),
            vec!["Type name not in scope: `Foo` @ main.k:Foo"]
        );
        assert_eq!(
            first(&[("main.k", "template <typename T, typename R, (S, T) -> R F>\nclass Foo\n{\n}\n")]),
            vec!["Type name not in scope: `S` @ main.k:S"]
        );
        assert_eq!(
            first(&[("main.k", "template <uint8 N, uint<N>[M] A, uint32 M>\nclass Foo\n{\n}\n")]),
            vec!["Symbol name not in scope: `M` @ main.k:M"]
        );
        assert_eq!(
            first(&[("main.k", "template <uint8 N, uint<N>[N] A, auto M = N>\nclass Foo\n{\n    uint<N> x = undeclared;\n}\n")]),
            Vec::<String>::new()
        );
    }

    /// A function's scope is its *body*: a return type spelled with an alias
    /// the body declares is not in scope, and no hint names that alias
    /// (`using.k::29`); the same alias inside the body is fine.
    #[test]
    fn a_function_s_signature_is_outside_its_body_s_scope() {
        assert_eq!(
            first(&[("main.k", "inline Return Foo()\n{\n    using Return = uint32;\n    Return x;\n    return x;\n}\n")]),
            vec!["Type name not in scope: `Return` @ main.k:Return"]
        );
        assert_eq!(
            first(&[("main.k", "inline uint32 Foo()\n{\n    using Return = uint32;\n    Return x = 1;\n    return x;\n}\n")]),
            Vec::<String>::new()
        );
    }

    /// A capture of a name nothing declares (`lambdas.k::21`, at the
    /// capture); a capture of an enclosing local is fine.
    #[test]
    fn a_capture_of_an_undeclared_name_is_not_in_scope() {
        let bad = "inline void Go()\n{\n    const auto fn = [a](uint32 x) -> uint32\n    {\n        return x - a;\n    };\n    fn(10);\n}\n";
        assert_eq!(first(&[("main.k", bad)]), vec!["Symbol name not in scope: `a` @ main.k:a"]);
        let good = "inline void Go()\n{\n    uint32 a = 1;\n    const auto fn = [a](uint32 x) -> uint32\n    {\n        return x - a;\n    };\n    fn(10);\n}\n";
        assert_eq!(first(&[("main.k", good)]), Vec::<String>::new());
    }

    /// What must stay in scope: a declaration after a closed
    /// block, a member from a method and from a lambda inside it, a loop
    /// variable, a capture, a module-level forward reference, a name a
    /// `static if` declares, an enum constant naming an earlier one.
    ///
    /// Corrected 2026-09-27: the loop's local was read as `uint32[k] arr;`,
    /// which the reference rejects — `k` is computed from a `static for`
    /// that stays a loop, so the dimension never folds ("Array dimension
    /// must be a positive integer" at the `[`, `kg-oracle --errors` with
    /// the base library; `static-for.k::10`). It is read as a value now,
    /// which keeps the name the probe must see.
    #[test]
    fn what_is_in_scope_stays_in_scope() {
        let text = "static if (true)\n{\n    struct Late\n    {\n        uint32 v;\n    }\n}\n\nenum E : uint8\n{\n    A,\n    B = A + 1\n}\n\nclass C\n{\npublic:\n    uint32 m;\n\n    uint32 Get()\n    {\n        const auto f = [m](uint32 k) { return k + m; };\n        return f(m) + Later(1);\n    }\n}\n\ninline uint32 Later(uint32 x)\n{\n    return x;\n}\n\ninline void main()\n{\n    for (const auto i : 4)\n    {\n        uint32 a = i;\n    }\n    uint32 b = 1;\n    static Late l;\n    static C c;\n    b = c.Get() + b + l.v;\n    static for (const auto j : 2)\n    {\n        const auto k = j + 1;\n        uint32 arr = k;\n    }\n}\n";
        assert_eq!(first(&[("main.k", text)]), Vec::<String>::new());
    }

    /// A nested function is hoisted out of its enclosing one
    /// (`moveFunctions`), so the enclosing function's parameter is in no
    /// scope there, and the reference offers no hint for it
    /// (`local-functions.k::1`). The nested function's own parameter is
    /// fine. The loop's tree has the hoisting done, so its scope is the
    /// reference's; the CST pass had to reconstruct it.
    #[test]
    fn an_enclosing_parameter_is_not_in_a_nested_functions_scope() {
        let run = |e: &str| format!("inline uint32 Run(uint32 x)\n{{\n    inline uint32 Go(uint32 y)\n    {{\n        return {e};\n    }}\n\n    return Go(x);\n}}\n");
        assert_eq!(first(&[("main.k", &run("x"))]), vec!["Symbol name not in scope: `x` @ main.k:x"]);
        assert_eq!(first(&[("main.k", &run("y"))]), Vec::<String>::new());
    }

    /// The case a folding mux selects is what pass 9 sees
    /// (`Constexpr.hs:311-314`): `mux(1, 0, undefined)` names `undefined`
    /// (`constexpr.k::179`), `mux(0, 0, undefined)` drops it. An undecided
    /// selector keeps every case, and each is judged
    /// (`pass_9_judges_what_inference_typed_and_every_case_of_a_standing_mux`,
    /// above), where the CST pass judged none.
    #[test]
    fn a_folding_mux_is_judged_in_the_case_it_selects() {
        let assert = |e: &str| format!("static assert(4 == bitsizeof({e}));\n");
        assert_eq!(
            first(&[("main.k", &assert("mux(1, 0, undefined)"))]),
            vec!["Symbol name not in scope: `undefined` @ main.k:undefined"]
        );
        assert!(!first(&[("main.k", &assert("mux(0, 0, undefined)"))]).iter().any(|e| e.contains("not in scope")));
    }

    /// A `default = e;` is `DefaultInitializationF` (`Parser.hs:314`), not an
    /// `ErrorF`, though this CST spells it `ERROR`: pass 9 judges the names
    /// in it, and in the rest of the file (`callbacks.k::89`).
    #[test]
    fn a_default_initialization_is_no_error_node() {
        const A: &str = "struct A\n{\n    uint32 x;\n    uint32 y;\n}\n\n";
        assert_eq!(
            first(&[("main.k", &format!("{A}inline void Main()\n{{\n    static default = {{ .x = 1, .y = Go }};\n    static A a;\n}}\n"))]),
            vec!["Symbol name not in scope: `Go` @ main.k:Go"]
        );
        assert_eq!(
            first(&[("main.k", &format!("{A}inline void Main()\n{{\n    static default = {{ .x = 1, .y = 2 }};\n    static A a;\n    A b = {{ .x = 1, .y = Go }};\n}}\n"))]),
            vec!["Symbol name not in scope: `Go` @ main.k:Go"]
        );
    }
}

mod qualified {
    use crate::tests::{accepts, program};

    fn first(files: &[(&str, &str)]) -> Vec<String> {
        program(files).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    const H: &str = "module h { Outer, Bar }\n\nclass Outer\n{\npublic:\n    class Inner\n    {\n    public:\n        using alias = uint32;\n    }\n}\n\nenum Bar : uint8\n{\n    A\n}\n";

    /// `test/syntax/type-identifier.k::14,15,20` and `enum.k::12` against
    /// `kg-oracle --errors`, 2026-09-10: at the last segment, naming it, with
    /// the members of the resolved qualifier as hints.
    #[test]
    fn a_qualified_name_no_member_answers_is_reported_at_its_last_segment() {
        assert_eq!(
            first(&[("main.k", "import h\n\ninline void main()\n{\n    static Outer::Inne inner;\n}\n"), ("h.k", H)]),
            vec!["Type name not in scope: `Inne`\nPerhaps you meant: `Inner` @ main.k:Inne"]
        );
        assert_eq!(
            first(&[("main.k", "import h\n\ninline void main()\n{\n    static Outer::Inner::alis value;\n}\n"), ("h.k", H)]),
            vec!["Type name not in scope: `alis`\nPerhaps you meant: `alias` @ main.k:alis"]
        );
        assert_eq!(
            first(&[("main.k", "import h\n\ntemplate <auto N>\nstruct C\n{\n    uint<N> x;\n}\n\ninline void main()\n{\n    static C<Outer::Inne> c;\n}\n"), ("h.k", H)]),
            // A template argument is an expression, so the miss is a "Symbol"
            // and `Inner`, a type, is not offered (`nonType`, `Frontend.hs:120`).
            vec!["Symbol name not in scope: `Inne` @ main.k:Inne"]
        );
        assert_eq!(
            first(&[("main.k", "import h\n\nstatic assert(Bar::Undefined != 0);\n"), ("h.k", H)]),
            vec!["Symbol name not in scope: `Undefined` @ main.k:Undefined"]
        );
        // A callee's note is the whole path (`freeFunctionSpecifier`,
        // `Parser.hs:772-780`): `E::Nope()` at `E` (`kg-oracle --errors`
        // 2026-09-27).
        assert_eq!(
            first(&[("main.k", "import h\n\ninline void main()\n{\n    Bar::Nope();\n}\n"), ("h.k", H)]),
            vec!["Symbol name not in scope: `Nope` @ main.k:Bar::Nope"]
        );
        assert_eq!(first(&[("main.k", "import h\n\ninline void main()\n{\n    static Outer::Inner inner;\n    static assert(Bar::A == 0);\n}\n"), ("h.k", H)]), Vec::<String>::new());
    }

    /// `type-identifier.k::22`: the hint is drawn from the members of the
    /// resolved qualifier — `Foo::Bar` — not from a same-named class declared
    /// inside the method.
    #[test]
    fn the_hint_is_a_member_of_the_resolved_qualifier() {
        let text = "struct ali{};\n\nclass Foo\n{\npublic:\n    class Bar\n    {\n    public:\n        using dahlia = uint32;\n    }\n\n    inline void Run()\n    {\n        Bar::alia x;\n\n        class Foo\n        {\n            using alias = uint32;\n        }\n    }\n}\n";
        assert_eq!(
            first(&[("main.k", text)]),
            vec!["Type name not in scope: `alia`\nPerhaps you meant: `dahlia` @ main.k:alia"]
        );
        accepts("class Foo\n{\npublic:\n    class Bar\n    {\n    public:\n        using dahlia = uint32;\n    }\n\n    inline void Run()\n    {\n        Bar::dahlia x;\n    }\n}\n");
        // A template instance among the segments resolves through the copy.
        accepts("template <typename T>\nclass Foo\n{\npublic:\n    struct S\n    {\n        T x;\n    }\n}\n\ninline void main()\n{\n    Foo<uint32>::S s;\n}\n");
    }

    /// A path led by an import alias is asked of what the import exposes
    /// (`findInImport`, `SymbolTable.hs:272-279`), and a miss is the
    /// `QualifiedIdentifier`'s: its last name at its last segment for a
    /// value, at the path for a callee, the alias itself for a type
    /// (`kg-oracle --errors` 2026-09-27; `Config::HARTS` over `.cmdargs`
    /// is `test/library/processor/risc-v/testbench.k`'s). A member walked
    /// through the answer is judged like a class's.
    #[test]
    fn an_alias_qualified_name_the_import_does_not_expose_is_undefined() {
        let m = "module m.n { K, E, f }\n\nconst uint32 K = 3;\n\nenum E : uint2\n{\n    A,\n    B\n}\n\ninline uint32 f()\n{\n    return 1;\n}\n";
        let run = |body: &str| {
            let text = format!("import m.n as M\n\ninline void main()\n{{\n{body}\n}}\n");
            first(&[("main.k", &text), ("m/n.k", m)]).into_iter().filter(|x| !x.starts_with("The import is redundant")).collect::<Vec<_>>()
        };
        assert_eq!(run("    uint32 x = M::NOPE;"), ["Symbol name not in scope: `NOPE` @ main.k:NOPE"]);
        assert_eq!(run("    uint32 x = M::g();"), ["Symbol name not in scope: `g`\nPerhaps you meant one of these:\n\t`K` defined in m.n\n\t`f` defined in m.n @ main.k:M::g"]);
        assert_eq!(run("    uint32 x = M::E::C;"), ["Symbol name not in scope: `C`\nPerhaps you meant one of these:\n\t`K` defined in m.n\n\t`f` defined in m.n @ main.k:C"]);
        assert_eq!(run("    M::Nope n;"), ["Type name not in scope: `M`\nPerhaps you meant: `E` defined in m.n @ main.k:M"]);
        assert_eq!(run("    uint32 x = M::K;\n    uint32 y = M::f();\n    M::E e = M::E::A;"), Vec::<String>::new());
        let cmdargs = "import .cmdargs as Config\n\ninline void main()\n{\n    uint32 x = Config::HARTS;\n}\n";
        let out: Vec<String> = first(&[("main.k", cmdargs)]).into_iter().filter(|x| !x.starts_with("The import is redundant")).collect();
        assert_eq!(out, ["Symbol name not in scope: `HARTS` @ main.k:HARTS"]);
    }

    /// A qualified value whose leading segment nothing answers is the whole
    /// `QualifiedIdentifier` undefined — named by its last segment, there
    /// for a value and at the path for a callee; a type names the leading
    /// one (`Frontend.hs:97-102`; `kg-oracle --errors` 2026-09-27). A
    /// module path is no qualifier: `data::bits::NOPE` names `NOPE`, and
    /// `m::n::K` is undefined though `K` exists.
    #[test]
    fn a_qualified_value_nothing_leads_is_named_by_its_last_segment() {
        let m = "module m.n { K }\n\nconst uint32 K = 3;\n";
        let run = |body: &str| {
            let text = format!("import m.n\n\ninline void main()\n{{\n{body}\n}}\n");
            first(&[("main.k", &text), ("m/n.k", m)]).into_iter().filter(|x| !x.starts_with("The import is redundant")).collect::<Vec<_>>()
        };
        assert_eq!(run("    uint32 x = Foo::Bar;"), ["Symbol name not in scope: `Bar` @ main.k:Bar"]);
        assert_eq!(run("    uint32 x = Foo::Bar();"), ["Symbol name not in scope: `Bar` @ main.k:Foo::Bar"]);
        assert_eq!(run("    uint32 x = m::n::K;"), ["Symbol name not in scope: `K`\nPerhaps you meant: `K` defined in m.n @ main.k:K"]);
        assert_eq!(run("    Foo::Bar x;"), ["Type name not in scope: `Foo` @ main.k:Foo"]);
    }
}

mod rooted {
    use kg_base::{FileId, Utf8PathBuf};

    /// The reference's `hasErrors` gate (`Frontend.hs:44-52`,
    /// `type-identifier.k::12`): pass 9's error stops pass 19, so the
    /// "Global state" three lines above the undefined name is not reported.
    #[test]
    fn an_undefined_name_gates_the_later_passes() {
        let out = crate::tests::program(&[("main.k", "uint32 otter;\n\ninline void main()\n{\n    static Otter outer;\n}\n")]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].0.starts_with("Type name not in scope: `Otter`"), "{out:?}");
    }

    /// `basics.k::183`: a call through a resolved receiver with no such
    /// member is the member's name not in scope (`Frontend.hs:106-108`),
    /// not "Invalid member access".
    #[test]
    fn a_call_through_a_receiver_with_no_such_member_names_the_member() {
        let out = crate::tests::program(&[("main.k", "inline void main()\n{\n    uint32 x;\n    x.Foo();\n}\n")]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Symbol name not in scope: `Foo`");
        assert_eq!(out[0].1, "main.k:Foo");
    }

    /// `inScope` (`Frontend.hs:125-127`) admits a local of the same function
    /// (`type-identifier.k::16`), and the declaration holding the undefined
    /// name is not offered for it (`function-constants.k::3`); the enclosing
    /// function is (`type-check.k::56`), a local function is not
    /// (`type-identifier.k::17`).
    #[test]
    fn hints_follow_the_referring_scope() {
        let out = crate::tests::program(&[("main.k", "inline void main()\n{\n    uint32 x = min(10, 12);\n}\n")]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Symbol name not in scope: `min`\nPerhaps you meant: `main`");
        let out = crate::tests::program(&[("main.k", "inline void main()\n{\n    uint32 value;\n    inline void valeus()\n    {\n    }\n    auto x = values;\n}\n")]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Symbol name not in scope: `values`\nPerhaps you meant: `value`");
        let out = crate::tests::program(&[("main.k", "inline void main()\n{\n    uint32 value;\n    auto x = values;\n}\n")]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Symbol name not in scope: `values`\nPerhaps you meant: `value`");
        let out = crate::tests::program(&[("main.k", "inline uint32 fn1(uint32 x)\n{\n    return x;\n}\n\ninline void main()\n{\n    const auto fn = fn2;\n}\n")]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Symbol name not in scope: `fn2`\nPerhaps you meant: `fn1`");
    }
    use kg_config::Settings;

    use crate::Db;

    /// A two-file program rooted at `main.k`, with `/proj` on the import path so
    /// a bare `import a` finds `/proj/a.k`, and no base library — the shape
    /// `tests/query.rs` uses.
    fn program(files: &[(&str, &str)]) -> (Db, FileId) {
        let mut settings = Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        let mut db = Db::new(settings);
        let mut root = FileId::SYNTHETIC;
        for (name, text) in files {
            let file = db.set_file_text(
                &Utf8PathBuf::from(format!("/proj/{name}")),
                (*text).to_owned(),
            );
            if *name == "main.k" {
                root = file;
            }
        }
        (db, root)
    }

    #[track_caller]
    fn errors(files: &[(&str, &str)]) -> Vec<String> {
        let (db, root) = program(files);
        db.check(&[root]).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect()
    }

    #[track_caller]
    fn accepts(files: &[(&str, &str)]) {
        let errs = errors(files);
        assert!(errs.is_empty(), "expected a clean compile, got {errs:?}");
    }

    #[track_caller]
    fn rejects(files: &[(&str, &str)]) {
        assert!(!errors(files).is_empty(), "expected a rejection of {files:?}");
    }

    /// The whole point: a name no module in the program declares
    /// (`test/syntax/imports.k:4-9`, `expected:1`), against the same snippet
    /// with the declaration present (`:19-26`, `expected:0`).
    #[test]
    fn a_name_the_program_does_not_declare_is_rejected() {
        rejects(&[("main.k", "inline void main()\n{\n    Aa x;\n}\n")]);
        accepts(&[(
            "main.k",
            "struct Aa\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    Aa x;\n}\n",
        )]);
    }

    /// A **forward** reference resolves: the parse-time table has not reached
    /// the declaration yet, but `getSymbols` is built over the finished program
    /// (`docs/spec/resolve.md` §7.2). Reading only the parse-time answer would
    /// reject this.
    #[test]
    fn a_forward_reference_is_in_scope() {
        accepts(&[(
            "main.k",
            "inline void main()\n{\n    Aa x;\n}\n\nstruct Aa\n{\n    uint32 x;\n}\n",
        )]);
    }

    /// `findInImports` (`SymbolTable.hs:263-270`) over an export list of the
    /// shape `library/` uses throughout — `module a { module a }`, which exposes
    /// every exposable symbol (`SymbolTable.hs:152,176-178`).
    ///
    /// This is also the pass's one **external** dependency: it is the first
    /// consumer of [`kg_resolve::ModuleInterface::visible`], so an export list
    /// that lowers to nothing turns every cross-module name into a false
    /// positive. Pinned here rather than trusted.
    #[test]
    fn a_name_an_import_exposes_is_in_scope() {
        accepts(&[
            ("a.k", "module a\n    { module a\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n"),
            ("main.k", "import a\n\ninline void main()\n{\n    Aa x;\n}\n"),
        ]);
    }

    /// The class that needs the whole program and nothing less: `Bb` **is**
    /// declared in a module this program loaded, and is not exposed by it, so
    /// the reference rejects the use. Oracle: `test/syntax/imports.k:27-34`
    /// (`expected:1`) beside `:19-26` (`expected:0`) — `a1/a1.k:5-7` exposes
    /// `a1` and `Aa` and not the `b1` it imports.
    #[test]
    fn a_name_an_import_declares_but_does_not_expose_is_rejected() {
        let a = "module a\n    { Aa\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n\nstruct Bb\n{\n    uint32 y;\n}\n";
        accepts(&[("a.k", a), ("main.k", "import a\n\ninline void main()\n{\n    Aa x;\n}\n")]);
        rejects(&[("a.k", a), ("main.k", "import a\n\ninline void main()\n{\n    Bb x;\n}\n")]);
    }

    /// `import a as m` binds `m` as a **qualifier**, not a declaration
    /// (`SymbolTable.hs:276-278`). It is in no symbol map, so a pass that only
    /// consulted the maps would report every `m::X` in `library/mini-base.k:22`.
    #[test]
    fn an_import_alias_is_not_an_undefined_symbol() {
        accepts(&[
            ("a.k", "module a\n    { module a\n    }\n\nconst auto Ten = 10;\n"),
            ("main.k", "import a as m\n\nstatic assert(m::Ten == 10);\n"),
        ]);
    }

    /// `Frontend.hs:93-94` — a template's body is restored from the
    /// pre-pass expression, so no diagnostic raised inside it reaches the
    /// output; it is judged in the copies instantiation makes, framed.
    #[test]
    fn a_template_body_is_not_judged() {
        accepts(&[(
            "main.k",
            "template <typename T>\nstruct Wrapper\n{\n    Undeclared x;\n}\n",
        )]);
    }

    /// `isAbbreviatedTemplate` (`Frontend.hs:84`, `Parser/Syntax.hs:722-724`):
    /// a function with an `auto` parameter is an implicit template and its whole
    /// subtree is returned unchanged.
    #[test]
    fn an_abbreviated_template_is_not_judged() {
        accepts(&[("main.k", "inline void f(auto x)\n{\n    Undeclared y;\n}\n")]);
    }

    /// A declaration under a `static if` exists only if the predicate holds, and
    /// the predicate is pass 16's answer. Both `test/syntax/static-if.k` cases
    /// — one `expected:0`, one `expected:1`, differing in nothing but `True` vs
    /// `False` — must therefore be accepted here.
    #[test]
    fn a_declaration_under_a_static_if_counts_as_declaring() {
        accepts(&[(
            "main.k",
            "const auto P = true;\n\nstatic if (P)\nstruct C\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    static C c;\n}\n",
        )]);
    }

    /// `staticControl` runs inside pass 6 (`Constexpr.hs:41-47`), so a dead
    /// branch's declarations are gone before this pass: `static if (Cond)
    /// class C {…}` under `Cond = false` declares no `C`, and a `Do` declared
    /// under `static if (Ten != 10)` is not in scope at its call — with the
    /// hint the reference gives (`static-if.k::31,33`; `kg-oracle --errors`
    /// 2026-09-14, `scratchpad/fn/s3.k`, `sc/si_31.k`). A `static if` whose
    /// predicate does not fold keeps both branches as hedges.
    #[test]
    fn a_dead_static_if_branch_declares_nothing() {
        let first = |files: &[(&str, &str)]| -> Vec<String> {
            crate::tests::program(files).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
        };
        assert_eq!(
            first(&[("main.k", "const bool Cond = false;\n\nstatic if (Cond)\nclass C\n{\npublic:\n    uint32 x;\n}\n\ninline void main()\n{\n    static C c;\n    c.x = 10;\n}\n")]),
            vec!["Type name not in scope: `C` @ main.k:C"]
        );
        assert_eq!(
            first(&[("main.k", "const auto Ten = 10;\n\nstatic if (Ten != 10)\n{\n    const uint32 x = 0;\n    inline void Do(uint32 y)\n    {\n        y = x;\n    }\n}\n\ninline void Go(uint32 y)\n{\n}\n\ninline void main()\n{\n    Do(10);\n}\n")]),
            vec!["Symbol name not in scope: `Do`\nPerhaps you meant: `Go` @ main.k:Do"]
        );
        // Without braces the declaration is a plain item under the `static
        // if` — dead all the same.
        assert_eq!(
            first(&[("main.k", "const auto Ten = 10;\n\nstatic if (Ten != 10)\ninline void Do(uint32 y)\n{\n}\n\ninline void Go(uint32 y)\n{\n}\n\ninline void main()\n{\n    Do(10);\n}\n")]),
            vec!["Symbol name not in scope: `Do`\nPerhaps you meant: `Go` @ main.k:Do"]
        );
        accepts(&[("main.k", "const auto Ten = 10;\n\nstatic if (Ten == 10)\n{\n    inline void Do(uint32 y)\n    {\n    }\n}\n\ninline void main()\n{\n    Do(10);\n}\n")]);
        // A use inside the dead branch is deleted with it, whatever it names.
        accepts(&[("main.k", "const bool Cond = false;\n\nstatic if (Cond)\n{\n    inline void Run()\n    {\n        Undeclared(10);\n    }\n}\n")]);
    }

    /// `eval (MuxF x y) = maybe e (mux y) $ fromIntegralConstexpr x`
    /// (`Constexpr.hs:311-314`) deletes every unselected case *before* pass 9
    /// runs. Oracle: `static assert(4 == bitsizeof(mux(0, 10, undefined)));`
    /// (`test/syntax/constexpr.k`, `expected:0`).
    #[test]
    fn an_unselected_mux_case_is_not_judged() {
        accepts(&[("main.k", "static assert(4 == bitsizeof(mux(0, 10, undefined)));\n")]);
    }

    /// `intrinsics` (`Type.hs:1074-1082`) types five names out of a table, and
    /// none of them is in any symbol map.
    #[test]
    fn the_intrinsics_are_in_scope() {
        accepts(&[(
            "main.k",
            "inline void main()\n{\n    __print(\"hi\");\n    assert(true);\n}\n",
        )]);
    }

    /// **`this` is not a sixth intrinsic.** The grammar has no `this` keyword,
    /// so a written `this` parses as an ordinary `PATH` and resolves to nothing
    /// — and the reference says so. Graded against the oracle on exactly this
    /// text:
    ///
    /// ```text
    /// 6 |         return this._x;
    ///   |                ^
    /// Symbol name not in scope: `this`
    /// ```
    ///
    /// The `ThisF` the reference *does* carry is synthesized by `captureThis`
    /// (`Frontend.hs:185-192`), is a node of its own rather than an identifier,
    /// and is named `$this` (`Parser/Syntax.hs:267-268,739`) — a spelling no
    /// source file can write. It was never reachable from this pass, which
    /// judges names; suppressing `this` here only ever hid a real rejection.
    /// `this.Go()` is a method call whose receiver the parser reads as the
    /// `This` node, and `this.x` beside it is a written name no scope
    /// holds — one report, at the second (`scratchpad/mm/e2.k`,
    /// `kg-oracle --errors` 2026-09-17).
    #[test]
    fn a_written_this_is_a_call_receiver_and_nothing_else() {
        let main = "class Foo\n{\npublic:\n    uint32 x = 1;\n    inline void Go()\n    {\n    }\n    inline uint32 Run()\n    {\n        this.Go();\n        return this.x;\n    }\n}\n\ninline void main()\n{\n    static Foo f;\n    f.Run();\n}\n";
        let out: Vec<(String, String)> =
            crate::tests::program(&[("main.k", main)]).into_iter().map(|(m, at, _)| (m, at)).collect();
        assert_eq!(out, vec![("Symbol name not in scope: `this`".to_owned(), "main.k:this".to_owned())]);
    }

    /// `f.Bar()` and `f.z` on a class with neither: the map declares the
    /// class in full, so the miss is `TUndefined` and the member is the
    /// report — of this module's class and of an import's alike
    /// (`basics.k::184`; `scratchpad/mm/{a,c,d}.k`, `kg-oracle --errors`
    /// 2026-09-17).
    #[test]
    fn a_member_the_class_lacks_is_undefined_at_the_member() {
        let own = "class Foo\n{\npublic:\n    uint32 x = 1;\n}\n\ninline void main()\n{\n    static Foo f;\n    f.Bar();\n    uint32 y = f.z;\n}\n";
        let out: Vec<String> = crate::tests::program(&[("main.k", own)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(
            out,
            vec!["Symbol name not in scope: `Bar` @ main.k:Bar", "Symbol name not in scope: `z` @ main.k:z"]
        );
        let h = "module h { Foo }\n\nclass Foo\n{\npublic:\n    uint32 x = 1;\n    inline void Go()\n    {\n    }\n}\n";
        let main = "import h\n\ninline void main()\n{\n    static Foo f;\n    f.Bar();\n    f.Go();\n}\n";
        let out: Vec<String> = crate::tests::program(&[("h.k", h), ("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(out, vec!["Symbol name not in scope: `Bar` @ main.k:Bar"]);
    }

    /// `c2.zz.Go(x)`: the receiver of a method call is not judged, and
    /// `c2.zz.ww` beside it is — the reference reports the conversion from
    /// `unresolved` for the first (silence here) and `zz` for the second
    /// (`scratchpad/pt/q{1,7}.k`, `kg-oracle --errors` 2026-09-17).
    #[test]
    fn the_receiver_of_a_method_call_is_not_judged() {
        let main = "class C2\n{\npublic:\n    uint32 x = 1;\n}\n\ninline uint32 Do(uint32 v)\n{\n    static C2 c2;\n    return c2.zz.Go(v);\n}\n";
        assert_eq!(crate::tests::program(&[("main.k", main)]), Vec::new());
        let main = "class C2\n{\npublic:\n    uint32 x = 1;\n}\n\ninline uint32 Do(uint32 v)\n{\n    static C2 c2;\n    return c2.zz.ww;\n}\n";
        let out: Vec<String> = crate::tests::program(&[("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(out, vec!["Symbol name not in scope: `zz` @ main.k:zz"]);
    }

    #[test]
    fn a_written_this_is_not_in_scope() {
        rejects(&[(
            "main.k",
            "class Foo\n{\npublic:\n    inline uint32 Run()\n    {\n        return this._x;\n    }\n\n    uint32 _x = 3;\n}\n",
        )]);
    }

    /// [`crate::check_source`] is the program its text makes alone, since
    /// it became a `Db` (sprint W3.7): a name nothing declares is pass 9's
    /// there too — `kg-oracle --no-base --errors` prints "Type name not in
    /// scope: `Aa`" at the name (2026-10-10). Corrected from the text door's
    /// premise, which ran no pass 9 for want of the base library and pinned
    /// that silence ("the structural rule"); the door it was about is gone.
    #[test]
    fn check_source_is_the_text_as_a_program_alone() {
        let text = "inline void main()\n{\n    Aa x;\n}\n";
        let diagnostics = crate::check_source(text);
        let at: Vec<(&str, &str)> = diagnostics.iter().map(|d| (d.message.as_str(), &text[d.range.range])).collect();
        assert_eq!(at, [("Type name not in scope: `Aa`", "Aa")]);
    }

    /// A loading failure means some module contributed no symbols at all, so
    /// every name it would have supplied looks undefined. The reference never
    /// enters the frontend in that case (`Main.hs:73-75`); the assertion here is
    /// that the *only* error is the import failure.
    #[test]
    fn a_failed_import_suppresses_the_pass() {
        let errs = errors(&[("main.k", "import missing\n\ninline void main()\n{\n    Aa x;\n}\n")]);
        assert!(
            errs.iter().all(|e| !e.contains("not in scope")),
            "pass 9 ran on a program that failed to load: {errs:?}"
        );
    }
}

mod members {
    use crate::tests::{accepts, program};

    fn first(files: &[(&str, &str)]) -> Vec<String> {
        program(files).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    /// `Frontend.hs:104-106` (`kg-oracle --errors`, 2026-09-10): a field a
    /// struct does not have is "Undefined member", at the member; on a class
    /// it is a symbol miss, called or not.
    #[test]
    fn a_member_the_receiver_does_not_have() {
        let s = "struct Foo\n{\n    uint32 x;\n}\n\ninline uint32 main()\n{\n    Foo foo;\n    uint32 a = foo.y;\n    foo.z = 1;\n    return a;\n}\n";
        assert_eq!(
            first(&[("main.k", s)]),
            vec![
                "Undefined member `y` of type `Foo` @ main.k:y",
                "Undefined member `z` of type `Foo` @ main.k:z",
            ]
        );
        // On a class the reference says "Symbol name not in scope" for `y`
        // and `q` alike (probed 2026-09-10; `scratchpad/mm/{a,c}.k`
        // 2026-09-17): the map declares the class in full, so `method`'s
        // miss is `Undefined` and both are reported at the member.
        let c = "class C\n{\npublic:\n    uint32 x;\n    uint32 Go()\n    {\n        return x;\n    }\n}\n\ninline uint32 main()\n{\n    static C c;\n    uint32 a = c.y;\n    return c.Go() + c.q();\n}\n";
        assert_eq!(
            first(&[("main.k", c)]),
            vec!["Symbol name not in scope: `y` @ main.k:y", "Symbol name not in scope: `q` @ main.k:q"]
        );
        accepts("struct Foo\n{\n    uint32 x;\n}\n\ninline uint32 main()\n{\n    Foo foo;\n    return foo.x;\n}\n");
    }
}
