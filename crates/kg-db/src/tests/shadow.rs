//! Pass 19's `checkShadowDecl` (`TypeCheck.hs:50,110-121`) at the rooted
//! door — the loop's at a module's text since sprint W3 step 1
//! (`kg_sem::check::TextPass::CheckShadowDecl`). These were the CST pass's
//! own tests, at the text door, which runs no loop and has no pass 19 shadow
//! check now: `test/syntax/shadow-warning.k`'s rows and the hoisting rules
//! the CST pass reconstructed, which the loop's tree has applied.

/// Every `shadow` warning the rooted door gives `main.k` (and the modules
/// it imports), alone under `/proj` with no base library and the
/// `data.closure` stubs a lambda needs, as `message @ text` — and no error.
fn shadows(text: &str) -> Vec<String> {
    use kg_base::Utf8PathBuf;
    let core = "module data.closure.core\n    { Closure\n    , make_capture0\n    , make_capture1\n    }\n\n\
        template <typename FunctionType, FunctionType fn, typename Capture>\n\
        using Closure = Capture;\n\n\
        inline auto make_capture0()\n{\nstruct Capture\n{\n}\n\nCapture capture;\n\nreturn capture;\n}\n\n\
        template <typename T0>\n\
        inline auto make_capture1(T0 x0)\n{\nstruct Capture\n{\nT0 x0;\n}\n\nCapture capture = { x0 };\n\nreturn capture;\n}\n";
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    db.set_file_text(&Utf8PathBuf::from("/proj/data/closure/core.k"), core.to_owned());
    let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned());
    let out = db.check(&[main]);
    let errors: Vec<&str> = out.iter().filter(|d| d.is_error()).map(|d| d.message.as_str()).collect();
    assert!(errors.is_empty(), "a shadowing declaration still compiles; got {errors:?} for\n{text}");
    out.iter()
        .filter(|d| d.warning == Some(kg_diag::WarningKind::Shadow))
        .map(|d| format!("{} @ {}", d.message, &text[d.range.range]))
        .collect()
}

#[track_caller]
fn warns(text: &str) {
    assert!(!shadows(text).is_empty(), "expected a shadow warning for\n{text}");
}

#[track_caller]
fn silent(text: &str) {
    let got = shadows(text);
    assert!(got.is_empty(), "expected no shadow warning for\n{text}\ngot {got:?}");
}

/// `test/syntax/shadow-warning.k`, the nine registered rows that assert
/// `warning:shadow` and are decidable from one text.
#[test]
fn the_oracle_rows_that_warn() {
    warns("inline void main()\n{\n    bool x;\n    {\n        uint32 x;\n    }\n}\n");
    warns("inline void main()\n{\n    const bool x = false;\n    {\n        uint32 x;\n    }\n}\n");
    warns("inline void main()\n{\n    bool x;\n    {\n        const uint32 x = 1;\n    }\n}\n");
    warns("inline void main()\n{\n    bool x;\n    {\n        auto x = 0;\n    }\n}\n");
    warns("inline void Run(bool x)\n{\n    {\n        uint32 x;\n    }\n}\n");
    warns("inline void Run(bool x)\n{\n    for (const auto x : 10)\n    {\n    }\n}\n");
    warns("inline void main()\n{\n    for (const auto x : 10)\n    {\n        for (const auto x : 6)\n        {\n        }\n    }\n}\n");
    warns(
        "inline void Run(bool x)\n{\n    if (x)\n    {\n        uint32 y;\n\n        if (x)\n        {\n            uint32 y;\n        }\n        else\n        {\n            uint32 y;\n        }\n    }\n}\n",
    );
    warns("class Foo\n{\n    uint32 x;\n\n    inline void Bar()\n    {\n        bool x;\n    }\n}\n");
}

/// The four `warning:0` rows — the ones a wrong qualifier algebra breaks.
#[test]
fn the_oracle_rows_that_do_not_warn() {
    // `shadowable` needs a non-empty `getScope` (`TypeCheck.hs:118`), and a
    // declaration in a file with no `module` header has none.
    silent("const bool x = true;\n\ninline void main()\n{\n    {\n        uint32 x;\n    }\n}\n");
    // `localScoped` deepens the *parent's* qualifier on pop (`Parser.hs:153`),
    // so the loop's `x` and the later `x` are siblings, not nested.
    silent("inline void main()\n{\n    for (const auto x : 10)\n    {\n    }\n\n    bool x;\n}\n");
    silent("inline void Run(bool x)\n{\n    if (x)\n    {\n        uint32 y;\n    }\n    else\n    {\n        uint32 y;\n    }\n}\n");
    // The `moveFunctions` row: `Bar` is hoisted to `main@Bar` long before
    // `validateProgram` looks, and `[main@Bar]` cannot reach `[main,$,x]`.
    silent("inline void main()\n{\n    uint32 x;\n\n    inline void Bar()\n    {\n        uint32 x;\n\n        x = 0;\n    }\n\n    x = 0;\n}\n");
}

/// A nested function still shadows anything at or outside its parent's own
/// qualifier, where `moveToParentScope` re-roots it (`Function.hs:47`), and
/// within its own body, which moves with it.
#[test]
fn a_hoisted_function_still_shadows_module_scope() {
    warns("module m {}\n\nconst uint32 x = 1;\n\ninline void main()\n{\n    inline void Bar()\n    {\n        uint32 x;\n    }\n}\n");
    warns("inline void main()\n{\n    inline void Bar()\n    {\n        uint32 x;\n        {\n            uint32 x;\n        }\n    }\n}\n");
}

/// Conditions 1 and 3 (`TypeCheck.hs:110-112`): only a `Variable` reports,
/// and never a `DeclareMember`/`DeclareClass` one; a local is the control.
#[test]
fn fields_and_functions_never_report() {
    silent("module m {}\n\nconst uint32 x = 1;\n\nclass C\n{\n    uint32 x;\n}\n");
    silent("module m {}\n\nconst uint32 x = 1;\n\nstruct S\n{\n    uint32 x;\n};\n");
    warns("module m {}\n\nconst uint32 x = 1;\n\ninline void C()\n{\n    uint32 x;\n}\n");
}

/// The hoisted declaration keeps the *parent's* qualifier, not the parent's
/// body, so `F1`'s parameters go out of reach while `Fk`'s stay in it
/// (`Function.hs:47-50`).
#[test]
fn hoisting_moves_the_boundary_but_does_not_remove_it() {
    silent("inline void Run(bool x)\n{\n    inline void Bar()\n    {\n        uint32 x;\n    }\n}\n");
    warns("inline void main()\n{\n    inline void Bar(bool x)\n    {\n        {\n            uint32 x;\n        }\n    }\n}\n");
}

/// A lambda body is a function body, so the same boundary applies: the
/// enclosing function's parameters are out of reach from inside it
/// (`library/data/buffer/gearbox.k:120-144`'s `bool last`; this shape
/// `kg-oracle --errors --Wall --Werror`, 2026-10-10: clean). The text
/// door's version captured `last` too, which the rooted door's parse-time
/// table rejects as a redefinition.
#[test]
fn a_lambda_does_not_shadow_its_enclosing_functions_parameters() {
    silent("import data.closure.core\n\ninline void Run(bool last)\n{\n    const auto l = [](bool tid)\n    {\n        bool last = tid;\n    };\n    l(true);\n}\n");
}

/// `test/syntax/shadow-warning.k:64-66`: the snippet is one import, and the
/// warning belongs to the imported file — `validateProgram` runs over every
/// module of the merged program (`Frontend.hs:54`).
#[test]
fn an_imported_module_warns_about_its_own_declarations() {
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![kg_base::Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = crate::Db::new(settings);
    // `test/syntax/helper/shadow_warning.k`, verbatim: the module header
    // gives every declaration below it a non-empty scope (`TypeCheck.hs:118`).
    db.set_file_text(
        &kg_base::Utf8PathBuf::from("/proj/helper/shadow_warning.k"),
        "module helper.shadow_warning\n    {\n    }\n\nconst uint32 x = 0;\n\ninline void Foo()\n{\n    bool x;\n}\n".to_owned(),
    );
    let main = db.set_file_text(&kg_base::Utf8PathBuf::from("/proj/main.k"), "import helper.shadow_warning\n".to_owned());
    let diagnostics = db.check(&[main]);
    assert!(!diagnostics.iter().any(kg_diag::Diagnostic::is_error), "{diagnostics:?}");
    assert_eq!(diagnostics.iter().filter_map(|d| d.warning).next(), Some(kg_diag::WarningKind::Shadow), "{diagnostics:?}");
}

/// Condition 4 drops one scope component before the outward walk, so a
/// sibling in the same scope is never "shadowed": that is a redefinition,
/// which `parseSymbol` has already reported (`SymbolTable.hs:200-201`) —
/// the program's one error, and no warning.
#[test]
fn a_sibling_is_a_redefinition_not_a_shadow() {
    let out = super::program(&[("main.k", "inline void main()\n{\n    bool x;\n    bool x;\n}\n")]);
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(!out[0].0.contains("shadows"), "{out:?}");
}
