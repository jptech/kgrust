//! Pass 2, `higherOrderFunctions`' one verdict (`Function.hs:313-325`), at
//! the rooted door — the loop's at a module's text since sprint W3 step 1
//! (`kg_sem::check::TextPass::HigherOrderFunctions`). Each case is a
//! `kg-oracle --errors` probe (2026-10-10), the position included.

use crate::tests::program;

const MESSAGE: &str = "Non-inline higher order functions are not supported";

/// `message @ file:the text its range covers` for every diagnostic.
fn at(main: &str) -> Vec<String> {
    program(&[("main.k", main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
}

/// A member function taking a function and not `inline` — written so, or
/// through an alias (`resolvedFuncParams`, `:133-141`), which the CST pass
/// could not see — at the function from its first token: past its
/// documentation, from its attributes. A template's body is judged too:
/// pass 2 runs before any instantiation, and `removeTemplate` keeps a
/// template's errors (`Frontend.hs:298-299`). An `inline` one is a
/// template, no error.
#[test]
fn a_non_inline_function_taking_a_function_is_refused_at_its_first_token() {
    let text = "using F = (uint32) -> uint32;\n\nclass C\n{\n    // documented\n    void Run(() -> void cb)\n    {\n    }\n\n    uint32 Go(F f, uint32 x)\n    {\n        return f(x);\n    }\n\n    [[async]] void Pipe(() -> void cb)\n    {\n    }\n\n    inline void Fine(() -> void cb)\n    {\n    }\n}\n\ntemplate <typename T>\nclass D\n{\n    void Do(() -> void cb)\n    {\n    }\n}\n";
    let got = at(text);
    let starts: Vec<&str> = got.iter().map(|d| d.strip_prefix(&format!("{MESSAGE} @ main.k:")).unwrap_or(d)).collect();
    assert_eq!(starts.len(), 4, "{got:#?}");
    assert!(starts[0].starts_with("void Run("), "{got:#?}");
    assert!(starts[1].starts_with("uint32 Go(F f"), "{got:#?}");
    assert!(starts[2].starts_with("[[async]] void Pipe("), "{got:#?}");
    assert!(starts[3].starts_with("void Do("), "{got:#?}");
}

/// The reference collects errors from the final tree: a refused member of a
/// `static if` branch pass 6 deleted is gone with it, and says nothing — the
/// CST pass reported it. A function template's own body is the template's,
/// and is judged.
#[test]
fn a_refused_function_a_dead_branch_took_says_nothing() {
    let text = "class C\n{\n    static if (false)\n    {\n        void Dead(() -> void cb)\n        {\n        }\n    }\n\n    template <typename T>\n    void Tmpl(T x, () -> void cb)\n    {\n    }\n}\n";
    let got = at(text);
    assert_eq!(got.len(), 1, "{got:#?}");
    assert!(got[0].starts_with(&format!("{MESSAGE} @ main.k:void Tmpl(")), "{got:#?}");
}

/// Pass 2 is before `instantiateTemplates`' `hasErrors'` (`Template.hs:
/// 55,84`): its error stops every pass from 9 on.
#[test]
fn a_refused_function_closes_the_run_before_pass_9() {
    let text = "class C\n{\n    void Run(() -> void cb)\n    {\n    }\n\n    inline void Go()\n    {\n        nosuch();\n    }\n}\n";
    let got = at(text);
    assert_eq!(got.len(), 1, "{got:#?}");
    assert!(got[0].starts_with(&format!("{MESSAGE} @ main.k:void Run(")), "{got:#?}");
}

/// `Function.hs:314-325`, the CST pass's own test: unlike
/// `freeNonInlineFunctions`, this one reaches class methods; an `inline`
/// one is a template, and a method taking no function is no business of
/// this pass.
#[test]
fn higher_order_functions_must_be_inline() {
    crate::tests::rooted_rejects("class C\n{\nvoid Run(() -> void cb) {}\n}");
    crate::tests::rooted_accepts("class C\n{\ninline void Run(() -> void cb) {}\n}");
    crate::tests::rooted_accepts("class C\n{\nvoid Run(uint32 x) {}\n}");
}
