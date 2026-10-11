//! Crate-level tests, and the accept/reject helpers every pass module's own
//! tests are written in.
//!
//! Each pass keeps its oracles next to its algebra, so a pass and its evidence
//! move together. What lives here is only what spans passes: the two properties
//! the whole sink must have, and the shape of the helpers.

use crate::check_source;

mod constant_fold;
mod conversion;
mod deduce_template_args;
mod higher_order;
mod infer_type;
mod post_desugar;
mod rooted_types;
mod shadow;
mod template_type_check;
mod undefined_symbols;
mod unresolved_templates;
mod validate_program;

/// Every error message [`check_source`] produces for `text`.
pub(crate) fn errors(text: &str) -> Vec<String> {
    check_source(text).into_iter().filter(|x| x.is_error()).map(|x| x.message).collect()
}

/// Asserts a clean compile — the direction a false positive breaks.
#[track_caller]
pub(crate) fn accepts(text: &str) {
    let errs = errors(text);
    assert!(errs.is_empty(), "expected a clean compile of\n{text}\ngot {errs:?}");
}

/// Asserts a rejection — the direction a false negative merely leaves
/// undecidable.
#[track_caller]
pub(crate) fn rejects(text: &str) {
    assert!(!errors(text).is_empty(), "expected a rejection of\n{text}");
}

/// Every error message the rooted door gives `text` as `main.k`, alone
/// under `/proj` with no base library — where a pass the loop judges at a
/// module's text is read since its CST code went (sprint W3 step 1).
pub(crate) fn rooted_errors(text: &str) -> Vec<String> {
    use kg_base::Utf8PathBuf;
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
    db.check(&[main]).into_iter().filter(kg_diag::Diagnostic::is_error).map(|x| x.message).collect()
}

/// [`accepts`] at the rooted door.
#[track_caller]
pub(crate) fn rooted_accepts(text: &str) {
    let errs = rooted_errors(text);
    assert!(errs.is_empty(), "expected a clean compile of\n{text}\ngot {errs:?}");
}

/// [`rejects`] at the rooted door.
#[track_caller]
pub(crate) fn rooted_rejects(text: &str) {
    assert!(!rooted_errors(text).is_empty(), "expected a rejection of\n{text}");
}

/// Every error message the rooted door gives `main` — `import data.closure`
/// written first — with stubs of the two `data.closure` modules a lambda
/// and a callback are desugared against (`make_capture0`, `make_capture1`;
/// the reference's `library/base.k:36` imports them, which no base library
/// here does). The redundant-import warning the stubs draw is not an error.
pub(crate) fn closured_errors(main: &str) -> Vec<String> {
    use kg_base::Utf8PathBuf;
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
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    db.set_file_text(&Utf8PathBuf::from("/proj/data/closure/core.k"), core.to_owned());
    db.set_file_text(&Utf8PathBuf::from("/proj/data/closure.k"), closure.to_owned());
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), format!("import data.closure\n\n{main}"));
    db.check(&[main]).into_iter().filter(kg_diag::Diagnostic::is_error).map(|x| x.message).collect()
}

/// The rooted door over an in-memory program under `/proj`, no base library,
/// `main.k` the root. Every diagnostic comes back as `(message, "<file>:<the
/// text its range covers>", labels)`, so a test asserts *where* a report
/// landed and not only that it did.
pub(crate) fn program(files: &[(&str, &str)]) -> Vec<(String, String, Vec<String>)> {
    use kg_base::Utf8PathBuf;
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    let mut main = None;
    for (name, text) in files {
        let id = db.set_file_text(&Utf8PathBuf::from(format!("/proj/{name}")), (*text).to_owned());
        if *name == "main.k" {
            main = Some(id);
        }
    }
    let main = main.expect("a main.k");
    let spelled = |r: kg_base::FileRange| {
        let text = db.parse(r.file).text().to_owned();
        let name = db.path(r.file).map(|p| p.file_name().unwrap_or("").to_owned()).unwrap_or_default();
        format!("{name}:{}", &text[r.range])
    };
    db.check(&[main])
        .into_iter()
        .map(|d| {
            let labels = d.labels.iter().map(|l| format!("{}: {}", spelled(l.range), l.message)).collect();
            (d.message.clone(), spelled(d.range), labels)
        })
        .collect()
}

#[test]
fn a_clean_snippet_produces_nothing() {
    accepts("inline void main()\n{\n}\n");
}

#[test]
fn a_parse_error_is_still_reported() {
    rejects("inline void main(\n");
}

/// The single seam property: nothing below `check_source` may report a
/// backend code. Every diagnostic the reference frontend produces is
/// `Error 1` (`Parser/IO.hs:114`), and 241 corpus assertions depend on the
/// frontend not pre-empting `compiler/cpp/errors.h`.
#[test]
fn every_diagnostic_is_a_frontend_code() {
    let sources = [
        "inline void main() {}",
        "void main() {}",
        "inline void main() { uint8 x = 300u8; }",
        "class C { void Run() { } }",
        "inline void main() { static assert(false); }",
        "inline void main() { uint32 x = 1 / 0; }",
    ];
    for src in sources {
        for x in check_source(src) {
            assert_eq!(x.code, kg_diag::Code::Frontend, "in {src:?}: {}", x.message);
        }
    }
}

#[test]
fn diagnostics_come_back_in_source_order() {
    let diagnostics = check_source("void a() {}\nvoid b() {}\n");
    let starts: Vec<_> = diagnostics.iter().map(|x| x.range.range.start()).collect();
    let mut sorted = starts.clone();
    sorted.sort();
    assert_eq!(starts, sorted);
}




/// Step M4: the CST passes read the type table the program-wide loop's
/// engine derives (`crate::looped`). A function template's instance named
/// in a designated initializer — `template Go<uint33>` inside a `static if`
/// — is typed only once the loop has made its copy, and then converts to
/// the field's `() -> uint32` as the reference refuses: "Invalid conversion
/// from `() -> uint33` to `() -> uint32`" (`dependent-template.k::15`,
/// `TypeCheck.hs:162-178`). The base table leaves it `TUnresolved`, and
/// nothing was reported. Red with the base table handed to the passes.
#[test]
fn a_conversion_only_the_loop_types_is_checked() {
    let main = "class E\n{\npublic:\n    () -> uint32 callback;\n}\n\nstatic if (true)\n{\n    template <typename T>\n    inline T Go()\n    {\n        return {};\n    }\n}\n\ninline void main()\n{\n    static E e = {.callback = template Go<uint33>};\n}\n";
    let out = program(&[("main.k", main)]);
    assert!(
        out.iter().any(|(m, at, _)| m.starts_with("Invalid conversion from `() -> uint33` to `() -> uint32`") && at.starts_with("main.k:template Go")),
        "{out:#?}"
    );
}

/// Pass 19's conversions at a module's own text are the loop's, admitted
/// in the pass's place (`TextPass::CheckConversion`) and never seeded into
/// the CST run ahead of pass 1: an error is in the program's error set at
/// the pass that raises it (`Frontend.hs:38-52`), so a conversion stops
/// nothing before it. `kg-oracle --errors` (2026-10-10) prints both
/// errors here, pass 16's and pass 19's. Red with the text's conversions
/// raised as the copies' are, which closed the run at pass 1 and printed
/// the conversion alone.
#[test]
fn a_conversion_at_the_text_stops_no_earlier_pass() {
    let main = "inline void f(string s)\n{\n    uint8 x = s;\n}\n\nstatic assert(clog2(0) == 1);\n";
    let out: Vec<String> = program(&[("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
    assert_eq!(
        out,
        [
            "Invalid conversion from `string` to `uint8` @ main.k:s",
            "static assertion expression did not evaluate to a boolean constant @ main.k:clog2(0) == 1",
        ]
    );
}

/// A verdict the loop raises inside a copy enters the CST run at the pass
/// it is the reference's (`passes::Carried`, `kg_sem::check::Stage`), so
/// the run's checkpoints are the reference's (`Frontend.hs:38-52`): a
/// copy's conversion — pass 19's — closes none of the module's own passes
/// before it, and is not shown where the module's own pass 9 stopped the
/// run; a refusal — pass 8's — is not shown beside a pass-1 error. Each
/// as `kg-oracle --errors` prints it (2026-10-10). Red with the copies'
/// verdicts in the sink from the start (the first and third), and with
/// the passes after 9's shown however far the run got (the second).
#[test]
fn a_copys_verdict_enters_the_run_at_its_own_pass() {
    let messages = |text: &str| -> Vec<String> { program(&[("main.k", text)]).into_iter().map(|(m, _, _)| m).collect() };
    let f = "template <typename T>\ninline void f()\n{\n    T y = true;\n}\n\n";
    assert_eq!(
        messages(&format!("{f}inline void main()\n{{\n    f<uint32>();\n}}\n\nstatic assert(clog2(0) == 1);\n")),
        ["Invalid conversion from `bool` to `uint32`", "static assertion expression did not evaluate to a boolean constant"]
    );
    assert_eq!(messages(&format!("{f}inline void main()\n{{\n    f<uint32>();\n    nosuch();\n}}\n")), ["Symbol name not in scope: `nosuch`"]);
    let refused = "template <typename T>\nclass C\n{\n}\n\nvoid f()\n{\n}\n\ninline void main()\n{\n    static C<3> c;\n}\n";
    assert_eq!(messages(refused), ["Free functions must be declared `inline`"]);
}

/// The same conversion through a *method* template instance named as a
/// value, `obj.template Go<uint33>` over an `obj` only the `static if`
/// declares (`dependent-template.k::12`; `kg-oracle --errors`: at `obj`,
/// 9:27): lowered as `methodInstanceSpecifier`'s `FunctionSpecifier`
/// (`Parser.hs:765-769`), the loop instantiates and types it. `uint32` is
/// the field's type and converts. Red with the lowering's
/// method-specifier arm taken out.
#[test]
fn a_method_template_instance_named_as_a_value_is_converted() {
    let main = |t: &str| {
        format!("class B\n{{\npublic:\n    template <typename T>\n    T Go()\n    {{\n        return {{}};\n    }}\n}}\n\nclass E\n{{\npublic:\n    () -> uint32 callback;\n}}\n\ninline void main()\n{{\n    static if (true)\n    {{\n        static B obj;\n    }}\n    static E e = {{.callback = obj.template Go<{t}>}};\n}}\n")
    };
    let out = program(&[("main.k", &main("uint33"))]);
    assert!(
        out.iter().any(|(m, at, _)| m.starts_with("Invalid conversion from `() -> uint33` to `() -> uint32`") && at.starts_with("main.k:obj.template Go")),
        "{out:#?}"
    );
    assert_eq!(program(&[("main.k", &main("uint32"))]), Vec::new());
}

/// `reduceClosure` (`TypeCheck.hs:129,146-148`) over a closure only the loop
/// types: a global initialized by a *call* returning a capturing lambda is
/// rejected at the call (`higher-order-functions.k::195`'s `bind2nd(xor,
/// 0xc0de)`; `kg-oracle --errors`, 2026-09-27), one returning a lambda that
/// captures nothing is not. The written lambda stays the CST pass's, once.
/// Red with `kg_sem::check`'s `global_closure` answering `false`.
#[test]
fn a_global_closure_a_call_returns_is_checked_by_the_loop() {
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
        program(&[("main.k", main), ("data/closure.k", closure), ("data/closure/core.k", core)])
            .into_iter()
            .filter(|(m, _, _)| !m.starts_with("The import is redundant"))
            .map(|(m, at, _)| format!("{m} @ {at}"))
            .collect()
    };
    let capturing = "import data.closure\n\ninline auto make(uint32 y)\n{\n    return [y](uint32 x) { return x + y; };\n}\n\nconst auto f = make(3);\n";
    assert_eq!(run(capturing), ["Closures that capture value(s) are not allowed in global scope @ main.k:make(3)"]);
    let empty = "import data.closure\n\ninline auto make()\n{\n    return [](uint32 x) { return x; };\n}\n\nconst auto f = make();\n";
    assert_eq!(run(empty), Vec::<String>::new());
    let written = "import data.closure\n\nconst uint32 y = 4;\nconst auto f = [y](uint32 x) { return x + y; };\n";
    assert_eq!(run(written).len(), 1, "{:?}", run(written));
}

/// Pass 10 over the `Binary` a compound assignment desugars to, which the
/// text has no node for (`Parser.hs:631-638`): reported at the lvalue, as
/// `memory.k::19`'s `mem[0] += 10` is (`kg-oracle --errors`, 2026-09-27).
/// `b++`'s note-less `Binary` crashes the reference and is left alone; a
/// well-typed `+=` is clean. Red with `kg_sem::check`'s
/// `desugared_operation_error` returning at once.
#[test]
fn a_compound_assignments_operation_is_type_checked() {
    let reports = |main: &str| -> Vec<String> {
        program(&[("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    };
    let s = "struct S\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    S s;\n    s += 1;\n}\n";
    assert_eq!(reports(s), ["Invalid operands to binary expression: `S` and `uint1` @ main.k:s"]);
    assert_eq!(reports("inline void main()\n{\n    bool b = false;\n    b++;\n}\n"), Vec::<String>::new());
    assert_eq!(reports("inline void main()\n{\n    uint4 a = 0;\n    a += 1;\n}\n"), Vec::<String>::new());
}

/// `main.k`'s diagnostics as `message @ text`.
fn checked(main: &str) -> Vec<String> {
    use kg_base::Utf8PathBuf;
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), main.to_owned());
    let text = db.parse(main).text().to_owned();
    db.check(&[main]).into_iter().map(|d| format!("{} @ {}", d.message, &text[d.range.range])).collect()
}

/// Pass 14 at a module's own text is the loop's (sprint W3 step 1): an
/// `extern` of a struct an uninstantiated class template's body declares
/// is refused at the name (`kg-oracle --errors`, 2026-10-10) — a body the
/// CST pass never judged — and a class is no error. Red with the loop's
/// text verdicts left unadmitted.
#[test]
fn an_extern_in_a_templates_body_is_the_loops_verdict() {
    let body = "template <typename T>\nclass Foo\n{\n    struct S\n    {\n        T x;\n    }\n\n    class C\n    {\n    }\n\n    extern S;\n    extern C;\n}\n";
    let refused = ["Invalid extern declaration. Only classes and class templates can be declared as external. @ S"];
    assert_eq!(checked(body), refused);
}

/// Pass 13 at a module's own text is the loop's (sprint W3 step 1): the
/// public fields of a class this file `extern`s are refused at each field,
/// from its first token — the comment above one is in no note — and noted
/// at the `extern`; the class of a public object is restricted too, noted
/// at the object and then the `extern` (`kg-oracle --errors`, 2026-10-10:
/// 4:5 noted 12:5 and 15:1, 11:5 and 12:5 noted 15:1) — where the CST
/// pass gave no note and never followed the object. Red with the loop's
/// text verdicts unadmitted, and with the first token not sought.
#[test]
fn an_exportable_classes_members_are_the_loops_verdicts_with_their_notes() {
    use kg_base::Utf8PathBuf;
    let main = "class Inner\n{\npublic:\n    uint32 y;\n}\n\nclass Foo\n{\npublic:\n    // a field\n    uint32 x;\n    Inner inner;\n}\n\nextern Foo;\n";
    let run = || -> Vec<String> {
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.implicit_base = false;
        let mut db = crate::Db::new(settings);
        let file = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), main.to_owned());
        let text = db.parse(file).text().to_owned();
        db.check(&[file])
            .into_iter()
            .map(|d| {
                let notes: Vec<&str> = d.labels.iter().map(|l| &text[l.range.range]).collect();
                format!("{} @ {} {notes:?}", d.message, &text[d.range.range])
            })
            .collect()
    };
    let field = "Public field is not allowed in an exportable class";
    let refused = [
        format!("{field} @ uint32 y; [\"Inner inner;\", \"extern Foo;\"]"),
        format!("{field} @ uint32 x; [\"extern Foo;\"]"),
        format!("{field} @ Inner inner; [\"extern Foo;\"]"),
    ];
    assert_eq!(run(), refused);
}

/// `export-class.k`'s helper cases: the class is in the helper, the
/// export in the snippet, and the reference reports at the *member* in
/// the helper with `note: … while compiling` at the export
/// (`Frontend.hs:388`, `kg-oracle --errors` 2026-09-07) — filed under the
/// exporting module, whose run admits it (`kg_sem::check::TextVerdict`).
/// Kept from the CST pass's tests when pass 13 moved onto the loop.
#[test]
fn an_exported_class_from_another_module_is_checked_where_it_is_declared() {
    let helper = "module helper.exports\n    { PublicData\n    }\n\nclass PublicData\n{\npublic:\n    uint32 x;\n}\n";
    let out = program(&[("helper/exports.k", helper), ("main.k", "import helper.exports\n\nexport PublicData;\n")]);
    assert_eq!(
        out,
        [(
            "Public field is not allowed in an exportable class".to_owned(),
            "exports.k:uint32 x;".to_owned(),
            vec!["main.k:export PublicData;: while compiling".to_owned()],
        )]
    );
    let out = program(&[("helper/exports.k", helper), ("main.k", "import helper.exports\n\nextern PublicData;\n")]);
    assert_eq!(out.len(), 1, "{out:?}");
}

/// Pass 10 at a module's own text is the loop's (sprint W3 step 1): `x /
/// -1` is "Division with negative denominator is undefined" over the
/// loop's types, noted at the outer `(` as `parens` notes it — the
/// reference's second error after pass 17's (`auto-placeholder.k::33`'s
/// shape; `kg-oracle --errors`, 2026-10-10: 4:20), which the CST pass did
/// not give. Red with the loop's text verdicts left unadmitted, and with
/// the parentheses not climbed.
#[test]
fn a_type_error_is_the_loops_verdict_at_the_outer_paren() {
    let main = "inline void main()\n{\n    static uint32 x;\n    const auto y = ((x / -1)) + 1;\n}\n";
    let errors = checked(main);
    assert!(errors.contains(&"Division with negative denominator is undefined @ ((x / -1))".to_owned()), "{errors:?}");
}

/// Pass 12 at a module's own text (sprint W1.4): an instance the loop
/// left there — `Foo<int32, cast<int32[2]>(1000)>`, whose `X` no fold
/// resolves — is "Unresolved argument for the parameter X of template Foo"
/// at the argument (`unresolveArgError`, `Frontend.hs:423-427`); beside an
/// undefined name, pass 9 has set the reference's flag and pass 12 never
/// runs (`Frontend.hs:86-87`), so the name is the only report. Both read
/// off `kg-oracle --errors` (2026-09-27). Red with the loop's late
/// verdicts dropped (the first), or admitted before the pass-9 checkpoint
/// (the second).
#[test]
fn pass_12_at_the_modules_text_speaks_only_past_pass_9() {
    let template = "template <typename T, T X>\nclass Foo\n{\npublic:\n    T get() { return X; }\n}\n\n";
    let left = format!("{template}inline void main()\n{{\n    static Foo<int32, cast<int32[2]>(1000)> x;\n}}\n");
    let reports: Vec<(String, String)> = program(&[("main.k", &left)]).into_iter().map(|(m, at, _)| (m, at)).collect();
    assert_eq!(
        reports,
        [("Unresolved argument for the parameter X of template Foo".to_owned(), "main.k:cast<int32[2]>(1000)".to_owned())]
    );
    let gated = format!("{template}inline void main()\n{{\n    static Foo<int32, cast<int32[2]>(1000)> x;\n    uint32 y = nope;\n}}\n");
    let messages: Vec<String> = program(&[("main.k", &gated)]).into_iter().map(|(m, _, _)| m).collect();
    assert_eq!(messages, ["Symbol name not in scope: `nope`"]);
}

/// A verdict switch is a setting, inside the memo keys (sprint W8.2): on a
/// warm database the leftover verdicts speak and fall silent again with
/// nothing between but [`crate::Db::set_settings`] — a settings change is a
/// new program, and the loop (memoized on the program) runs again. The
/// program is `kg_sem::check`'s own for the leftover `static if`
/// (`Frontend.hs:406`), its predicate joined by `||` so that it is a
/// leftover only: over a bare runtime parameter the verdict is asked
/// always (sprint W2 row 3, `Cx::certainly_runtime`), and an absorbing
/// operator is where that rule declines. Red with the loop's check reading
/// anything but the settings, and red with `set_settings` leaving the
/// settings revision where it was.
#[test]
fn a_verdict_switch_is_a_setting_every_memo_sees() {
    use kg_base::Utf8PathBuf;
    let text = "template <typename T>\ninline void Foo(const T x)\n{\n    static if (x != 0 || false)\n    {\n    }\n}\n\ninline void main()\n{\n    Foo(10);\n}\n";
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    // Cleared first: on by default since W1.4, and the test is the flip.
    settings.verdicts.leftovers = false;
    let mut db = crate::Db::new(settings.clone());
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
    let errors = |db: &crate::Db| db.check(&[main]).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect::<Vec<_>>();
    assert_eq!(errors(&db), Vec::<String>::new());
    settings.verdicts.leftovers = true;
    db.set_settings(settings.clone());
    assert_eq!(errors(&db), ["static if predicate did not evaluate to a boolean constant"]);
    settings.verdicts.leftovers = false;
    db.set_settings(settings);
    assert_eq!(errors(&db), Vec::<String>::new());
}

/// [`program`], at the editor's door: [`crate::Db::diagnostics`] for
/// `main.k` alone, its errors as `(message, "main.k:<text>")`.
fn editor_errors(files: &[(&str, &str)]) -> Vec<(String, String)> {
    use kg_base::Utf8PathBuf;
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    let mut main = None;
    for (name, text) in files {
        let id = db.set_file_text(&Utf8PathBuf::from(format!("/proj/{name}")), (*text).to_owned());
        if *name == "main.k" {
            main = Some(id);
        }
    }
    let main = main.expect("a main.k");
    db.diagnostics(&[main], main)
        .into_iter()
        .filter(|d| d.is_error())
        .map(|d| (d.message.clone(), format!("main.k:{}", &db.parse(main).text()[d.range.range])))
        .collect()
}

/// An `auto` initialized with itself is left undecided however many
/// rounds run, and pass 17's verdict (`constexpr.k::0`) — when the program
/// gets that far: pass 9's undefined name in *another* module sets the
/// reference's one flag, and every pass after is skipped, program-wide
/// (`Frontend.hs:44-52,85-86`). `kg-oracle --errors` (2026-10-10): with
/// `a.k`'s `g()` undefined, that error alone; with `a.k` clean, "Could not
/// deduce variable type" at `main.k`'s `const auto`. At both doors: `kg check`'s,
/// and the editor's, which runs `a.k`'s passes through 9 only to ask. Red
/// with the flag never set, and with it always set.
#[test]
fn an_undecided_auto_waits_on_every_modules_passes_through_9() {
    let main = "import a\n\nconst auto X = X;\n\ninline void main()\n{\n    uint8 y = f();\n}\n";
    let erring = "module a { f }\n\ninline uint8 f()\n{\n    return g();\n}\n";
    let clean = "module a { f }\n\ninline uint8 f()\n{\n    return 1;\n}\n";
    // The message's first line: the hint below it is pass 9's own business.
    let errors = |a: &str| -> Vec<(String, String)> {
        let first = |m: String| m.lines().next().unwrap_or_default().to_owned();
        program(&[("main.k", main), ("a.k", a)]).into_iter().map(|(m, at, _)| (first(m), at)).collect()
    };
    let deduce = ("Could not deduce variable type".to_owned(), "main.k:const auto".to_owned());
    assert_eq!(errors(erring), [("Symbol name not in scope: `g`".to_owned(), "a.k:g".to_owned())]);
    assert_eq!(errors(clean), std::slice::from_ref(&deduce));
    assert_eq!(editor_errors(&[("main.k", main), ("a.k", erring)]), Vec::<(String, String)>::new());
    assert_eq!(editor_errors(&[("main.k", main), ("a.k", clean)]), [deduce]);
}

/// The fallout the flag exists for: `Bar<uint14, uint12>`'s default `Fn =
/// F1` is refused in the helper (`(uint14) -> uint13` for `(T) -> U`), the
/// instance is never made, and `main.k`'s `const auto x` is left undecided
/// only because of that — `template-param-default.k::13`, where the
/// reference reports the refusal alone (`kg-oracle --errors`). The loop's
/// own part of the flag (`Looped::erred_through_pass_9`), at both doors.
/// Red with the flag never set.
#[test]
fn an_auto_another_modules_refusal_left_undecided_is_silent() {
    let helper = "module h { Bar }\n\ninline uint13 F1(uint14 x)\n{\n    return x;\n}\n\n\
                  template <typename T, typename U, (T) -> U Fn = F1>\ninline U Bar(T x)\n{\n    return Fn(x);\n}\n";
    let main = "import h\n\ninline void main()\n{\n    const auto x = Bar<uint14, uint12>(10);\n}\n";
    let messages: Vec<String> = program(&[("main.k", main), ("h.k", helper)]).into_iter().map(|(m, _, _)| m).collect();
    assert!(!messages.is_empty(), "the refusal is reported");
    assert!(!messages.iter().any(|m| m.starts_with("Could not deduce")), "{messages:?}");
    let editor = editor_errors(&[("main.k", main), ("h.k", helper)]);
    assert!(!editor.iter().any(|(m, _)| m.starts_with("Could not deduce")), "{editor:?}");
}

/// Pass 16's loop verdicts where the CST run admits them. A width is
/// replaced whole, so pass 10's error inside it goes with it: `int<bitsizeof
/// (int32 + 1)>` is the width's error alone (`type-check.k::1-3`;
/// `kg-oracle --errors`, 2026-10-10). And a fold the CST pass makes that
/// raises the folder's own error — pass 6's, which the loop's tree does not
/// hold at the text — keeps the loop's verdicts out: the reference's flag
/// is set by it, and pass 16 never runs (`uint<2 * bytesizeof 1>` is the
/// `bytesizeof` error alone, `constexpr.k::124`). Red with either rule cut.
#[test]
fn a_width_pass_16_replaces_takes_what_is_inside_and_a_fold_error_stops_it() {
    let at = |text: &str| -> Vec<String> {
        program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    };
    assert_eq!(
        at("inline void main()\n{\n    int<bitsizeof (int32 + 1)> x;\n}\n"),
        ["int width did not evaluate to a positive integer constant @ main.k:int<bitsizeof (int32 + 1)>"]
    );
    assert_eq!(
        at("inline void main()\n{\n    uint<2 * bytesizeof 1> x;\n}\n"),
        ["bytesizeof is undefined for a type with bit size of 1 @ main.k:bytesizeof 1"]
    );
}

/// Pass 17, `deduceAuto` (`Type.hs:1595-1610`), at the rooted door — the
/// loop's since sprint W3 step 1 (`kg_sem::check::deduce_auto`); these were
/// the CST pass's own tests, at the text door, which runs no loop and has
/// no pass 17 now. Each is a corpus shape or a `kg-oracle --errors` probe
/// the CST pass recorded.
mod deduce_auto {
    use super::program;

    /// `main.k`'s errors, `message @ text`.
    fn errors(text: &str) -> Vec<String> {
        use kg_base::Utf8PathBuf;
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        let mut db = crate::Db::new(settings);
        let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
        db.check(&[main])
            .into_iter()
            .filter(|d| d.is_error())
            .map(|d| format!("{} @ {}", d.message, &text[d.range.range]))
            .collect()
    }

    #[track_caller]
    fn accepts(text: &str) {
        let errs = errors(text);
        assert!(errs.is_empty(), "expected a clean compile of\n{text}\ngot {errs:?}");
    }

    /// `const auto X = X;` at module scope is its own type (`constexpr.k::0`);
    /// a local of the same shape reads an outer name.
    #[test]
    fn a_module_auto_initialized_by_itself_is_not_deduced() {
        assert_eq!(errors("const auto X = X;\n"), ["Could not deduce variable type @ const auto"]);
        accepts("const uint32 x = 1;\n\ninline void main()\n{\n    const auto y = x;\n}\n");
    }

    /// `inline auto Run() { return Fn_T_N(); }` reports the return type
    /// first, then the call's LT verdict (`kg-oracle --errors`, 2026-09-07);
    /// with the callee's return type written, `T[N] Fn_T_N()`, the same: an
    /// `auto` function's `return` is no context that informs the call
    /// (`Type.hs:1121,1161-1166`, `return-type-polymorphism.k::19`). A
    /// written return type informs it, and deduces.
    #[test]
    fn a_return_of_an_undeduced_call_leaves_the_return_type_undeduced() {
        for callee in [
            "template <typename T, auto N>\ninline auto Fn_T_N()\n{\n    T[N] a;\n    return a;\n}\n\n",
            "template <typename T, auto N>\ninline T[N] Fn_T_N()\n{\n    T[N] a;\n    return a;\n}\n\n",
        ] {
            assert_eq!(
                errors(&format!("{callee}inline auto Run()\n{{\n    return Fn_T_N();\n}}\n")),
                ["Could not deduce function return type @ auto", "Could not deduce all arguments for template `Fn_T_N` @ Fn_T_N"]
            );
        }
        accepts("template <typename T, auto N>\ninline T[N] Fn_T_N()\n{\n    T[N] a;\n    return a;\n}\n\ninline uint8[4] Run()\n{\n    return Fn_T_N();\n}\n");
    }

    /// Pass 12's LT verdict on an alias instance in the returned expression
    /// leaves the return type undeduced as a call's does (`kg-oracle
    /// --errors` on exactly this text, 2026-09-27).
    #[test]
    fn a_return_holding_an_under_long_alias_instance_leaves_the_return_type_undeduced() {
        let text = "template <uint8 N>\nusing MyInt = int<N>;\n\ninline auto Run()\n{\n    return cast<MyInt<>>(3);\n}\n\ninline void main()\n{\n    Run();\n}\n";
        let messages: Vec<String> = errors(text).into_iter().map(|m| m.split(" @ ").next().unwrap_or_default().to_owned()).collect();
        assert_eq!(messages, ["Could not deduce function return type", "Could not deduce all arguments for template `MyInt`"]);
    }

    /// No initializer (`try f Nothing = TUnresolved`, `Type.hs:1108`;
    /// `basics.k:1535-1538`), `const auto` too (`isConstAuto`,
    /// `Type.hs:1606`); an initializer list, empty or not
    /// (`TInitializer`, `TUntyped`; `initializer-list.k:129-140`); a
    /// return of one (`function-auto-return.k:95-100`).
    #[test]
    fn what_expfromtype_cannot_write_is_rejected_at_the_auto() {
        let at = |body: &str| errors(&format!("inline void main()\n{{\n    {body}\n}}\n"));
        assert_eq!(at("auto x;"), ["Could not deduce variable type @ auto"]);
        assert_eq!(at("const auto x;"), ["Could not deduce variable type @ const auto"]);
        assert_eq!(at("auto x = { 3 };"), ["Could not deduce variable type @ auto"]);
        assert_eq!(at("auto x = {};"), ["Could not deduce variable type @ auto"]);
        assert_eq!(at("const auto x = {0};"), ["Could not deduce variable type @ const auto"]);
        assert_eq!(errors("inline auto Run()\n{\n    return {0};\n}\n"), ["Could not deduce function return type @ auto"]);
    }

    /// `static for (const auto x : y)` over a parameter, or a local the loop
    /// cannot fold, cannot deduce `x` — at the `auto`, past the `const`
    /// (`uninitConst`, `Parser.hs:451`); over a `const` local, or in a plain
    /// `for`, it can (`kg-oracle --errors` 2026-09-17). An induction
    /// variable has no initializer and is not supposed to
    /// (`auto-placeholder.k:9-16`).
    #[test]
    fn a_static_for_over_a_runtime_value_cannot_deduce_its_variable() {
        let run = |body: &str| errors(&format!("inline void Run(uint32 y)\n{{\n{body}}}\n"));
        assert_eq!(
            run("    static for (const auto x : y)\n    {\n        uint32 z = x;\n    }\n"),
            ["Could not deduce variable type @ auto"]
        );
        assert_eq!(
            run("    uint32 w = y + 1;\n    static for (const auto x : w)\n    {\n        uint32 z = x;\n    }\n"),
            ["Could not deduce variable type @ auto"]
        );
        assert_eq!(run("    const uint32 w = 3;\n    static for (const auto x : w)\n    {\n        uint32 z = x;\n    }\n"), Vec::<String>::new());
        assert_eq!(run("    for (const auto x : y)\n    {\n        uint32 z = x;\n    }\n"), Vec::<String>::new());
        accepts("inline void main()\n{\n    for (const auto x : 8)\n    {\n        static assert(bitsizeof(x) == 3);\n    }\n    static for (const auto x : 8)\n    {\n        static assert(bitsizeof(x) == 3);\n    }\n}\n");
    }

    /// The first return-bearing statement under a `static if` the fold
    /// cannot decide: the return type first, the predicate second; a
    /// decided `static if` or a runtime `if` deduces (`kg-oracle --errors`
    /// 2026-09-17). `find returnExp $ unfix body` reads the body's direct
    /// statements only (`Type.hs:1211-1217`), and a body with no `return`
    /// deduces `void`.
    #[test]
    fn the_return_type_is_the_first_top_level_return_bearing_statements() {
        let messages: Vec<String> = errors("inline auto Fn(uint32 y)\n{\n    static if (y > 1)\n    {\n        return true;\n    }\n    else\n    {\n        return 10;\n    }\n}\n\ninline void main()\n{\n    Fn(2);\n}\n")
            .into_iter()
            .map(|m| m.split(" @ ").next().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(messages, ["Could not deduce function return type", "static if predicate did not evaluate to a boolean constant"]);
        accepts("inline auto Fn(uint32 y)\n{\n    static if (true)\n    {\n        return true;\n    }\n    else\n    {\n        return 10;\n    }\n}\n\ninline void main()\n{\n    Fn(2);\n}\n");
        accepts("inline auto Fn(uint32 y)\n{\n    if (y > 1)\n    {\n        return true;\n    }\n    return false;\n}\n\ninline void main()\n{\n    Fn(2);\n}\n");
        // Pass 17's verdicts only: the nested `return {0}` is pass 19's to
        // judge against the `uint1` the outer one deduced, and its
        // conversion error here is not the reference's (which accepts the
        // program, `kg-oracle --errors` 2026-10-10) — a gap of that pass's.
        let seventeen: Vec<String> = errors("inline auto Run(bool c)\n{\n    if (c)\n    {\n        return {0};\n    }\n    return 1;\n}\n\ninline void main()\n{\n    Run(true);\n}\n")
            .into_iter()
            .filter(|m| m.starts_with("Could not deduce"))
            .collect();
        assert_eq!(seventeen, Vec::<String>::new());
        accepts("inline auto Foo()\n{\n}\n\ninline void main()\n{\n    Foo();\n}\n");
    }

    /// What is not pass 17's: `inline const auto` (`isAuto (Const Auto)` is
    /// false, `Parser/Syntax.hs:706-707` — pass 19's `check AutoF{}`), a
    /// function type that merely holds an `auto` (`callbacks.k:885-887`),
    /// a template's own body (`removeTemplate`, pass 15), a branch
    /// `staticControl` deleted, and every initialised binding.
    #[test]
    fn what_is_not_pass_17s_is_left_alone() {
        let seventeen = |text: &str| errors(text).into_iter().filter(|m| m.starts_with("Could not deduce variable") || m.starts_with("Could not deduce function")).collect::<Vec<_>>();
        assert_eq!(seventeen("inline const auto Foo()\n{\n    return {0};\n}\n"), Vec::<String>::new());
        assert_eq!(seventeen("class Foo\n{\n    (uint32) -> auto callback;\n}\n"), Vec::<String>::new());
        accepts("template <typename T>\ninline void Foo()\n{\n    auto x;\n}\n");
        accepts("inline void Foo()\n{\n    static if (false)\n    {\n        auto x;\n    }\n}\n");
        accepts("inline uint32 Foo()\n{\n    return 1;\n}\n\ninline void main()\n{\n    uint32 y = 3;\n    auto a = y + 1;\n    const auto b = 5;\n    auto c = a;\n    auto d = Foo();\n}\n");
    }

    /// A division or remainder by a literal not positive has an error type
    /// (`tDivLiteral`, `Type.hs:837-840`), which `expFromType` cannot write:
    /// the declaration first, then pass 10's error at the division
    /// (`kg-oracle --errors`, 2026-10-10: 4:5, then 4:20); a mux whose arms
    /// have no common supertype likewise (`initializer-list.k`).
    #[test]
    fn a_division_by_a_non_positive_literal_has_no_type_to_deduce() {
        let at = |text: &str| -> Vec<String> {
            program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
        };
        assert_eq!(
            at("inline void main()\n{\n    static uint32 x;\n    const auto y = x / -1;\n}\n"),
            vec!["Could not deduce variable type @ main.k:const auto", "Division with negative denominator is undefined @ main.k:x / -1"]
        );
        assert_eq!(
            at("inline void main()\n{\n    static uint32 x;\n    const auto y = x % 0;\n}\n")[0],
            "Could not deduce variable type @ main.k:const auto"
        );
        let bar = "struct Bar\n{\n    uint32 x;\n    uint32 y;\n}\n\n";
        assert_eq!(
            at(&format!("{bar}inline void Run(bool c)\n{{\n    Bar bar;\n    auto x = c ? bar : {{10, false}};\n    auto y = x.y;\n}}\n"))[0],
            "Could not deduce variable type @ main.k:auto"
        );
        accepts(&format!("{bar}inline void Run(bool c)\n{{\n    Bar bar;\n    auto x = c ? bar : {{10, 11}};\n    auto y = x.y;\n}}\n"));
        accepts(&format!("{bar}inline void Run(bool c)\n{{\n    Bar bar;\n    auto x = c ? {{.x = 10, .y = 11}} : bar;\n    auto y = x.y;\n}}\n"));
        accepts("inline void main()\n{\n    static uint32 x;\n    const auto y = x / 2;\n    auto z = x % 3;\n}\n");
    }
}
