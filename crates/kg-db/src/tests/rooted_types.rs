//! Types across modules, lambdas and closures, read at the rooted door —
//! the loop's (`kg_sem`), as `kg check` reads them.
//!
//! These lived beside the CST inference engine (`passes::types`), each
//! written against a bug in how that engine's environment answered an
//! import, a hoisted lambda's capture or a twin's table; the rooted door
//! read the loop for them since sprint W3, and they moved here when the
//! engine was deleted (sprint W3.7). The doc comments name the route each
//! was red against then: the routes are gone, and the tests stand as
//! regression checks over the loop that replaced them.

/// The two stub modules a lambda's rewrite needs to resolve: `Closure` and
/// the `make_captureN` the corpus's own `data.closure` declares
/// (`library/data/closure/core.k`, `library/data/closure.k`), cut to the
/// three arities one test uses. `implicit_base` is off in
/// [`crate::tests::program`], so the real library is not reachable there.
/// `make_capture1` was missing until 2026-09-27: `ClosureClass::empty`'s
/// lambda captures `this`, so the reference rejects the program without
/// it ("Symbol name not in scope: `make_capture1`", `kg-oracle
/// --errors`), and the test's "no diagnostic" held only while pass 12's
/// verdicts at the module's text were silent (sprint W1.4).
const CLOSURE_CORE: &str = "module data.closure.core\n    { Closure\n    , make_capture0\n    }\n\n\
    template <typename FunctionType, FunctionType fn, typename Capture>\n\
    using Closure = Capture;\n\n\
    inline auto make_capture0()\n{\nstruct Capture\n{\n}\n\nCapture capture;\n\nreturn capture;\n}\n";

const CLOSURE: &str = "module data.closure\n    { module data.closure.core\n    , make_capture1\n    , make_capture2\n    }\n\n\
    import data.closure.core\n\n\
    template <typename T0>\n\
    inline auto make_capture1(T0 x0)\n{\n\
    struct Capture\n{\nT0 x0;\n}\n\n\
    Capture capture = { x0 };\n\nreturn capture;\n}\n\n\
    template <typename T0, typename T1>\n\
    inline auto make_capture2(T0 x0, T1 x1)\n{\n\
    struct Capture\n{\nT0 x0;\nT1 x1;\n}\n\n\
    Capture capture = { x0, x1 };\n\nreturn capture;\n}\n";

/// A lambda hoisted out of a **class member** finds its own capture alias,
/// not another function's.
///
/// `?lambdaN$capture` is the name `desugarLambdas` gives every lambda's
/// capture alias, so one module's twin declares it once per lambda and the
/// printer spells a hoisted function's parameter type by its leaf alone
/// (see `passes::types::Declarations::alias_leaves`). Disambiguating by
/// `passes::types::Imported::hoisted_scope` over top-level functions alone found
/// `Interpolation` for `Interpolation@ClosureClass@test@?lambda0` — whose
/// parent is the class *member* `test`, because `moveFunctions` defers
/// inside a local class and hoists it first (`Function.hs:84-128`). The
/// capturing lambda's parameter was then typed as the nullary lambda's
/// `make_capture0::$::Capture` and pass 9 called its own fields undefined
/// (`test/logic/string.k:797`, 2026-09-20).
///
/// It was red with the class arm of `hoisted_scopes` cut; that code went
/// with the twin (`ir-elaboration.md` M4), and the test stands as a
/// regression check over the route that replaced it.
///
/// What it does **not** see, having been tried: matching a candidate on
/// its first segment instead of on the whole scope. The program holds two
/// members that each declare a `?lambda0`, so the two candidates `[C,
/// empty, $, …]` and `[C, test, $, …]` do tie on their first segment — and
/// the test stays green, as does the whole corpus (`ptdiff` 1583,
/// identical either way, measured 2026-09-20). The whole scope is kept
/// because it is what the parent *is*; nothing pins it.
#[test]
fn a_lambda_hoisted_out_of_a_member_finds_its_own_capture() {
    let main = "import data.closure\n\n\
        inline void Interpolation(uint8 tag)\n{\n\
        class ClosureClass\n{\nprivate:\nuint32 _member;\n\npublic:\n\
        void empty()\n{\nconst auto h = []()\n{\nuint32 y = 2;\n};\n}\n\
        void test(uint32 val)\n{\nconst auto fn = [val]()\n{\nuint32 z = _member + val;\n};\n}\n}\n\n\
        static ClosureClass c;\nc.empty();\nc.test(3);\n\n\
        const auto g = []()\n{\nuint32 w = 1;\n};\n}\n";
    let reports: Vec<String> = crate::tests::program(&[
        ("main.k", main),
        ("data/closure.k", CLOSURE),
        ("data/closure/core.k", CLOSURE_CORE),
    ])
    .into_iter()
    .filter(|(m, _, _)| !m.starts_with("The import is redundant"))
    .map(|(m, at, _)| format!("{m} @ {at}"))
    .collect();
    assert_eq!(reports, Vec::<String>::new());
}
/// Two `static for` loops of one function each declare `x`, under two
/// `$quasiN` scopes the reference keeps apart and this map's keys do
/// not. Each mention is typed by the loop it is in — the second `x` is
/// `const uint3`, not the first's `uint2` (`kg-oracle --errors`,
/// 2026-09-23) — where the duplicate key used to be poisoned and both
/// were silence; the backend met the untyped `x + 1` as an assertion
/// (`test/logic/test_cases_13.k`). Red with `passes::types::Declarations::in_its_block`
/// cut.
#[test]
fn a_local_declared_in_two_blocks_is_typed_by_its_own() {
    let main = "inline void main()\n{\nstatic for(const auto x : 4)\n{\nuint32 y = x;\n}\n\
        static for(const auto x : 8)\n{\nbool b = x;\n}\n}\n";
    let reports: Vec<String> =
        crate::tests::program(&[("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
    assert_eq!(reports, ["Invalid conversion from `const uint3` to `bool` @ main.k:x"]);
}

/// The reports of `main.k` over a base at `/proj/base.k`, every file
/// given as `(path, text)` under `/proj`: the library's shape, where a
/// module the base imports gets no implicit base of its own.
fn check_on_base(files: &[(&str, &str)]) -> Vec<String> {
    use kg_base::Utf8PathBuf;
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.base_library = Some(Utf8PathBuf::from("/proj/base.k"));
    let mut db = crate::Db::new(settings);
    let mut root = None;
    for (path, text) in files {
        let id = db.set_file_text(&Utf8PathBuf::from(format!("/proj/{path}")), (*text).to_owned());
        if *path == "main.k" {
            root = Some(id);
        }
    }
    let root = root.expect("a main.k");
    let at = |r: kg_base::FileRange| {
        let text = db.parse(r.file).text().to_owned();
        let name = db.path(r.file).map(|p| p.file_name().unwrap_or("").to_owned()).unwrap_or_default();
        format!("{name}:{}", &text[r.range])
    };
    db.check(&[root]).iter().map(|d| format!("{} @ {}", d.message, at(d.range))).collect()
}

/// A lambda named across modules is one closure. `constant(true)` given
/// to another module's `atomically((T) -> T modify)`: the owner of
/// `atomically`'s copy spells the function by a *qualified* alias
/// (`ArgShape::Function`, `RenamePool::alias_qualified`), and the
/// closure the copy's formal names took that alias as the function's
/// whole name — `[__kg_m54]` against the lambda's own `[@h@fn,
/// __kg_m3]`, two closures, and an "Invalid conversion" from the
/// closure to itself on a program the reference accepts (`kg-oracle
/// --errors`, `sync/atomic/init.k`'s `atomically(constant(true))`,
/// 2026-09-23). `TNameArg` is the qualified name (`getQualifiedName`),
/// which the alias stands for. Red with `passes::types::Imported::qualified_alias`
/// unasked.
#[test]
fn a_lambda_named_across_modules_is_one_closure() {
    let reports = check_on_base(&[
        ("data/closure/core.k", "module data.closure.core\n    { Closure\n    , make_capture0\n    }\n\ntemplate <typename FunctionType, FunctionType fn, typename Capture>\nusing Closure = Capture;\n\ninline auto make_capture0\n    (\n    )\n{\n    struct Capture\n    {\n    }\n\n    Capture capture;\n\n    return capture;\n}\n"),
        ("data/closure.k", "module data.closure\n    { module data.closure.core\n    , make_capture1\n    , make_capture2\n    }\n\nimport data.closure.core\n\ntemplate\n    < typename T0\n    >\ninline auto make_capture1\n    ( T0 x0\n    )\n{\n    struct Capture\n    {\n        T0 x0;\n    }\n\n    Capture capture =\n        { x0\n        };\n\n    return capture;\n}\n\ntemplate\n    < typename T0\n    , typename T1\n    >\ninline auto make_capture2\n    ( T0 x0\n    , T1 x1\n    )\n{\n    struct Capture\n    {\n        T0 x0;\n        T1 x1;\n    }\n\n    Capture capture =\n        { x0\n        , x1\n        };\n\n    return capture;\n}\n"),
        ("base.k", "module base\n    { module data.closure\n    , module h.fn\n    }\n\nimport data.closure\nimport h.fn\n"),
        ("h/fn.k", "module h.fn\n    { constant\n    }\n\ninline auto constant(auto x)\n{\n    using return_t = decltype(x);\n\n    return [x](auto y) -> return_t\n        {\n            return x;\n        };\n}\n"),
        ("h/at.k", "module h.at\n    { atomically\n    }\n\ntemplate <typename T>\ninline T atomically((T) -> T modify)\n{\n    T v;\n    return modify(v);\n}\n"),
        ("main.k", "import h.at\n\ninline void main()\n{\n    bool b = atomically(constant(true));\n}\n"),
    ]);
    assert_eq!(reports, Vec::<String>::new());
}

/// A closure called with no arguments in a copy is miscounted at its
/// `)`. `closureCalls` puts the closure, cast to its capture, in front
/// of the arguments and keeps the list's own note (`renote (cons cast)
/// c`, `Closure.hs:19-26`), so `Do()` is "expected 2" at the `)` the
/// call was written with — no node starts there, and the carry home
/// pairs it by the call it closes (`kg-oracle --errors`, 2026-09-23:
/// `go.k:10:19`; `higher-order-functions.k::209`). Red with the minted
/// cast taken as the first argument (the report lands on the cast), and
/// silent with the `)` carried by start alignment alone.
#[test]
fn a_closure_called_with_no_arguments_in_a_copy_is_miscounted_at_its_paren() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let go = "module h.go\n    { GoVar\n    }\n\ntemplate <typename T1, typename T2>\ninline auto GoVar((T1, T2) -> auto Do)\n{\n    static if (T1 == void)\n    {\n        return Do();\n    }\n    else\n    {\n        return Do(10, 11);\n    }\n}\n";
    let main = "import h.go\n\ninline void main()\n{\n    GoVar<void, uint32>([](uint32 x){});\n}\n";
    let reports = check_on_base(&[
        ("data/closure.k", CLOSURE),
        ("data/closure/core.k", CLOSURE_CORE),
        ("base.k", base),
        ("h/go.k", go),
        ("main.k", main),
    ]);
    assert_eq!(reports, vec!["Invalid number of arguments, expected 2 @ go.k:)"]);
}

/// A lambda hoisted out of a copy in a **namespaced** module finds its
/// own capture alias. Every copy's lambda declares `?lambda0$capture`,
/// one alias, so the leaf is told apart by the function the lambda was
/// hoisted out of (`passes::types::Imported::hoisted_scope`) — whose name is the
/// lookup's first segment *after* the module's namespace: in
/// `helper.fn` the scope is `[@helper@fn, compose<|…|>@?lambda0]`, and
/// asking for the parent of `@helper@fn` answered nothing. The capture
/// went untyped, and with it everything its fields feed: the reference's
/// "Invalid conversion" inside `compose`'s lambda (`kg-oracle --errors`,
/// 2026-09-23) was silence, in every library module that returns a
/// lambda (`data.function`). Red with the first segment taken whole.
#[test]
fn a_lambda_hoisted_out_of_a_namespaced_copy_finds_its_own_capture() {
    use kg_base::Utf8PathBuf;
    let base = "module base\n    { module data.closure\n    , module helper.fn\n    }\n\nimport data.closure\nimport helper.fn\n";
    let helper = "module helper.fn\n    { compose\n    , compose2\n    }\n\n\
        inline auto compose((auto) -> auto f, (auto) -> auto g)\n{\nreturn [f, g](auto x)\n{\nbool b = f(g(x));\nreturn b;\n};\n}\n\n\
        inline auto compose2((auto) -> auto f, (auto, auto) -> auto g)\n{\nreturn [f, g](auto x, auto y)\n{\nbool b = f(g(x, y));\nreturn b;\n};\n}\n";
    let main = "inline uint32 inc(uint32 x)\n{\nreturn x + 1;\n}\n\n\
        inline uint32 add(uint32 a, uint32 b)\n{\nreturn a + b;\n}\n\n\
        inline void main()\n{\nconst auto h = compose2(inc, add);\nconst auto k = compose(inc, inc);\nbool r = h(1, 2);\nbool s = k(1);\n}\n";
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.base_library = Some(Utf8PathBuf::from("/proj/base.k"));
    let mut db = crate::Db::new(settings);
    for (path, text) in [
        ("data/closure.k", CLOSURE),
        ("data/closure/core.k", CLOSURE_CORE),
        ("base.k", base),
        ("helper/fn.k", helper),
    ] {
        db.set_file_text(&Utf8PathBuf::from(format!("/proj/{path}")), text.to_owned());
    }
    let root = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), main.to_owned());
    let at = |r: kg_base::FileRange| {
        let text = db.parse(r.file).text().to_owned();
        let name = db.path(r.file).map(|p| p.file_name().unwrap_or("").to_owned()).unwrap_or_default();
        format!("{name}:{}", &text[r.range])
    };
    let reports: Vec<String> = db.check(&[root]).iter().map(|d| format!("{} @ {}", d.message, at(d.range))).collect();
    assert_eq!(
        reports,
        [
            "Invalid conversion from `uint32` to `bool` @ fn.k:f(g(x))",
            "Invalid conversion from `uint32` to `bool` @ fn.k:f(g(x, y))",
        ]
    );
}

/// A module's twin can resolve what its base table cannot, and
/// `passes::types::ModuleTypes::overlay` has to let it.
///
/// `using tag_t = newtype<uint32>` is an alias to an instance of an
/// *imported* template — a mention the census and the route decide, not
/// the table — so the declaring module's base `ModuleTypes` carries
/// `Unresolved` for it and its twin carries the copy. `overlay` laid the
/// base *over* the twin, contradicting its own doc comment, so every
/// importer read the `Unresolved`: `visit_function` postponed every body
/// that takes a `tag_t` (`Inference.hs:54-59`) and the calls inside
/// deduced nothing. 38 whole files under `test/logic` decided nothing
/// because of it (`docs/journal.md`, 2026-09-20).
///
/// With the base's unresolved entry giving way, `a::fail(tag, 3u32)`
/// deduces, the copy is made beside its template, and `bool b = x` inside
/// it is `Invalid conversion` carried home with the call as its `while
/// compiling` frame.
///
/// Red with `overlay`'s preference taken the other way round, and red with
/// `may_deduce` cut from `elaborate_module`'s early return — two pieces,
/// neither of which does anything alone.
#[test]
fn a_twins_resolved_name_beats_the_base_maps_unresolved_one() {
    let newtype = "module type.newtype { newtype }\ntemplate <typename T>\nstruct newtype\n{\nT unwrap;\n}\n";
    let alias = "module helper.a { tag_t, fail }\nimport type.newtype\n\
        using tag_t = newtype<uint32>;\n\
        template <typename T>\ninline void fail(tag_t tag, T x)\n{\nbool b = x;\n}\n";
    let main = "import helper.a as a\ninline void f(a::tag_t tag)\n{\na::fail(tag, 3u32);\n}\n";
    let reports: Vec<String> = crate::tests::program(&[
        ("main.k", main),
        ("helper/a.k", alias),
        ("type/newtype.k", newtype),
    ])
    .into_iter()
    .map(|(m, at, labels)| format!("{m} @ {at} {labels:?}"))
    .collect();
    assert_eq!(
        reports,
        vec!["Invalid conversion from `uint32` to `bool` @ a.k:x [\"main.k:fail: while compiling\"]"]
    );
}
