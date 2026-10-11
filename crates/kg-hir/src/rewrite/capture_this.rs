//! `captureThis` (`Frontend.hs:185-193`) — frontend pass **3**, and the second
//! rewrite of this port to land.
//!
//! Nine lines of Haskell, and the whole of it:
//!
//! ```haskell
//! captureThis e@ClassF{} = e { body = desugarShallow captureThis' $ body e }
//!   where
//!     captureThis' l@LambdaF{..} = Just $ l { lambdaCaptures = cons this lambdaCaptures }
//!       where
//!         this = copyNote (ThisF declScopeName) lambdaCaptures
//!     captureThis' ClassF{}      = Nothing
//!     captureThis' e'            = Just e'
//! captureThis e = e
//! ```
//!
//! # It is unconditional, and that is the whole point
//!
//! Nothing here asks whether the lambda touches a member. Every `Lambda`
//! lexically inside a `Class` is given a `This` capture — **648 of the corpus's
//! 1306 lambdas**, and **111 of those 648 reference no member of their class at
//! all**. For those 111 the consequence of omitting it is not a slipped
//! argument index: `makeCapture` picks its *module* on `null lambdaCaptures`
//! (`Function.hs:152-154`), so the call becomes
//! `@data@closure@core::make_capture0` where the reference emits
//! `@data@closure::make_capture1`. A different function, silently.
//!
//! For the other 537 it is the arity, and one more thing: `uncaptureVar`
//! (`Function.hs:256-261`) names its local from `getName x`, and
//! `getName ThisF{}` is `$this` (`Parser/Syntax.hs:739,267-268`). That local is
//! the *only* definition of `$this` in the moved lambda's scope, and
//! `moveFunctions` rewrites every member reference to `$this._x` off the symbol
//! map (`memberAccess`, `Function.hs:52-64`, inside `moveToParentScope` — not
//! `:152-158`, which is `makeCapture`; that citation was wrong here and in two
//! other places until 2026-08-13) whether or not this pass ran. So a missing
//! `This` capture does not stay inside pass 8 — it reaches `undefinedSymbols`
//! (frontend pass 9) as a spurious `Symbol name not in scope: $this`. The claim
//! in `crates/kg-db/src/lib.rs` that this pass "contributes no frontend
//! diagnostic" is true only for as long as pass 8 stays unwired.
//!
//! # Two things the transcription must not get wrong
//!
//! **The donor is the capture list, not the lambda.** `copyNote` is
//! `renote . const` (`Desugar.hs:104`) applied to `lambdaCaptures`, so the new
//! node wears the capture-list `Seq`'s note. Read off the reference for
//! `[]() -> uint32 { … }` inside a class at column 20 — the `ThisF` and the
//! `SeqF` around it print the *same* `Src {offset = 69, 6:20-6:20}`, and the
//! enclosing `LambdaF`'s is a different one. Pinned by
//! [`tests::the_this_wears_the_capture_lists_src_and_not_the_lambdas`], which
//! goes red if the donor is swapped for the lambda.
//!
//! **The scope is the lambda's, not the class's.** `l@LambdaF{..}` binds
//! `declScopeName` from the lambda being rewritten, so a lambda in
//! `Foo::Run` gets `["Foo","Run","$"]` and a lambda nested inside *it* gets
//! `["Foo","Run","$","?lambda0","$"]` — each `This` names the scope it will be
//! unpacked into, which is what makes nesting work.
//!
//! # `Nothing` at a nested class
//!
//! The inner walk declines at `ClassF` and nowhere else. Under `desugarShallow`
//! that restores the original subtree (`Desugar.hs:76-79`,
//! `Recursion.hs:73-74`), so a class nested inside a member function does not
//! have its lambdas captured twice: the outer `desugar` (a `cata`, so
//! bottom-up) already gave them the inner class's `This`, and the outer class's
//! shallow walk stops at the boundary. Verified against the reference — a
//! lambda in `Host::Go`'s local `class Nested` comes out with exactly one
//! `ThisF`, scoped `["Host","Go","$","Nested","Inner","$"]`.
//!
//! Both arms match `ClassF` and nothing else, so `Struct` and `Union` are
//! neither triggers nor boundaries — a distinction with no corpus behind it,
//! because a `struct` body is `memberVariable \`sepEndBy\` semi`
//! (`Parser.hs:299`) and cannot hold a function or a lambda at all. Recorded so
//! that a later reader does not "fix" the match to cover all three aggregates.
//!
//! # Why this cannot live in the lowering
//!
//! `kg dump ir`'s round trip is `lower(parse(pretty(ir))) == ir`
//! (`xtask ir`, gate 2). `Exp::This` prints as `this`
//! (`PrettyPrint.hs:165`), which reparses as an ordinary identifier and lowers
//! to `Exp::NamedValue` — so a lowering that inserted `This` would break the
//! round trip on all 648 sites. As a rewrite over a [`ModuleIr::fork`] the gate
//! keeps grading unrewritten IR and stays green, which is also where the
//! reference puts it: in the pass list, not the parser.

use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;
use crate::rewrite::{desugar, desugar_shallow, Rewriter, ShallowRewrite};

/// `captureThis'` (`Frontend.hs:188-191`) — the inner `desugarShallow` walk
/// that runs over one class body.
#[derive(Debug, Default)]
pub struct CaptureThis;

impl ShallowRewrite for CaptureThis {
    /// `captureThis' ClassF{} = Nothing` (`Frontend.hs:190`). The only arm.
    fn reverts(&self, arena: &ExpArena, id: ExpId) -> bool {
        matches!(arena[id].exp, Exp::Class { .. })
    }

    fn rewrite(&mut self, rw: &mut Rewriter, _id: ExpId, exp: Exp) -> Exp {
        let Exp::Lambda { scope, name, captures, params, ret, body } = exp else {
            // `captureThis' e' = Just e'` (`Frontend.hs:191`) — every other
            // node is handed back untouched, which the driver reads as "no
            // change" and answers with the original id.
            return exp;
        };
        let Exp::Seq(items) = &rw.arena[captures].exp else {
            unreachable!("a lambda's capture list is always a `Seq` (`lower.rs:1783-1786`)")
        };
        let mut items = items.clone();

        // `cons` is a *prepend* (`Desugar.hs:106-109`), which is why the
        // `This` is argument 0 of `make_captureN` and why `uncaptureVar` reads
        // it out of the capture struct's `x0`.
        let this = rw.donate(captures, Exp::This { scope: scope.clone() });
        items.insert(0, this);

        // `cons x = renote cons'` keeps the *original* `Seq`'s note, and the
        // original goes unreachable in the same step — so this is a `replace`
        // and the rebuilt list keeps `Own`.
        let captures = rw.replace(captures, Exp::Seq(items));
        Exp::Lambda { scope, name, captures, params, ret, body }
    }
}

/// `desugar $ captureThis . reifyEnums` (`Frontend.hs:72`) minus the half this
/// port already has: `reifyEnums`' numbering lives in
/// `kg_db::scope::enum_constant_value`, so what is left is the outer `cata`
/// firing at every `Class`.
///
/// The outer walk must be the `cata` and not a one-shot descent, because
/// bottom-up is what makes the `Nothing` at a nested class correct rather than
/// a hole: the nested class has already been rewritten by the time its parent's
/// shallow walk refuses to enter it.
pub fn apply(rw: &mut Rewriter, root: ExpId) -> ExpId {
    desugar(rw, root, &mut |rw, _id, exp| match exp {
        Exp::Class { scope, name, body } => {
            let body = desugar_shallow(rw, body, &mut CaptureThis);
            Exp::Class { scope, name, body }
        }
        e => e,
    })
}

/// **The pass's own invariant**, recomputed rather than trusted: a `Lambda` in
/// `ir` carries exactly one `This`, and it is at index 0, iff the lambda is
/// lexically inside a `Class`.
///
/// The enclosure is carried *down* here — a plain top-down flag over the
/// rewritten tree — where the pass derives it from firing at `Class` and then
/// declining at the next one. Two derivations of one predicate, which is the
/// point: a lost boundary shows up as a lambda with two `This` captures, a
/// `Class` the outer `cata` failed to reach (under a `Template`, say) shows up
/// as one with none.
///
/// It is not redundant with the reference's own grade in `xtask ir`
/// (`capture diff:`, the `ThisF` count at `--passes 3`), and the falsification
/// says which covers what: planting `Exp::Struct` in place of the boundary
/// leaves `capture diff` at 122/122 — no `library/` file nests a class inside a
/// class — and turns this red on three `test/` files the oracle cannot parse.
/// The reference grades the extent, this grades the shape, and the corpus each
/// can see is different.
pub fn this_captures_match_class_enclosure(ir: &crate::ModuleIr) -> Result<(), String> {
    fn go(ir: &crate::ModuleIr, id: ExpId, in_class: bool) -> Result<(), String> {
        let exp = &ir.arena[id].exp;
        if let Exp::Lambda { captures, .. } = exp {
            let Exp::Seq(items) = &ir.arena[*captures].exp else {
                return Err(format!("lambda #{}'s capture list is not a Seq", id.index()));
            };
            let n = items
                .iter()
                .filter(|&&c| matches!(ir.arena[c].exp, Exp::This { .. }))
                .count();
            let want = usize::from(in_class);
            if n != want {
                return Err(format!(
                    "lambda #{} at {:?} has {n} `This` capture(s), wanted {want} \
                     (Frontend.hs:186-190)",
                    id.index(),
                    ir.arena[id].src.head.range
                ));
            }
            if in_class && !matches!(ir.arena[items[0]].exp, Exp::This { .. }) {
                return Err(format!(
                    "lambda #{}'s `This` is not capture 0 — `cons` prepends \
                     (Desugar.hs:106-109)",
                    id.index()
                ));
            }
        }
        let in_class = in_class || matches!(exp, Exp::Class { .. });
        let mut kids = Vec::new();
        exp.children(&mut kids);
        for k in kids {
            go(ir, k, in_class)?;
        }
        Ok(())
    }
    go(ir, ir.root, false)
}

/// Every expectation below was read off the reference frontend built by
/// `oracle/build.sh`, at `kg-oracle --tree --notes --no-base --passes 3`,
/// captured 2026-08-13. `--passes 3` is the smallest slice that contains this
/// pass; `--passes 2` prints the same trees with every `lambdaCaptures` as
/// written, which is the negative half of each assertion.
#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::ast;
    use kg_syntax::AstNode;

    use super::*;
    use crate::rewrite::capture_this;
    use crate::{lower, ModuleIr};

    fn run(text: &str) -> (ModuleIr, ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "probe must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let before = lower(&file, FileId(0), &interner);
        let after = capture_this(&before, &interner);
        (before, after, interner)
    }

    /// Every `Lambda` in the tree, in arena order, with its capture list.
    fn lambdas(ir: &ModuleIr) -> Vec<(ExpId, Vec<ExpId>)> {
        let mut out = Vec::new();
        walk(ir, ir.root, &mut |id, exp| {
            if let Exp::Lambda { captures, .. } = exp {
                let Exp::Seq(items) = &ir.arena[*captures].exp else { panic!() };
                out.push((id, items.clone()));
            }
        });
        out
    }

    fn walk(ir: &ModuleIr, id: ExpId, f: &mut impl FnMut(ExpId, &Exp)) {
        let exp = &ir.arena[id].exp;
        f(id, exp);
        let mut kids = Vec::new();
        exp.children(&mut kids);
        for k in kids {
            walk(ir, k, f);
        }
    }

    fn scopes(ir: &ModuleIr, interner: &Interner) -> Vec<Vec<String>> {
        lambdas(ir)
            .into_iter()
            .map(|(_, caps)| {
                caps.iter()
                    .map(|&c| match &ir.arena[c].exp {
                        Exp::This { scope } => format!(
                            "This[{}]",
                            scope
                                .iter()
                                .map(|s| interner.resolve(*s).to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                        Exp::Capture { name, .. } => match &ir.arena[*name].exp {
                            Exp::Identifier(s) => interner.resolve(*s).to_string(),
                            other => format!("{other:?}"),
                        },
                        other => format!("{other:?}"),
                    })
                    .collect()
            })
            .collect()
    }

    /// The headline. `class Foo { inline uint32 Run() { auto fn = [a]() …; } }`
    /// plus the same lambda in a free function, at `--passes 3`:
    ///
    /// ```text
    /// lambdaCaptures = SeqF [ThisF {declScopeName = ["Foo","Run","$"]}, CaptureF … "a"]
    /// lambdaCaptures = SeqF [CaptureF … "a"]
    /// ```
    #[test]
    fn a_lambda_in_a_class_is_captured_and_one_in_a_free_function_is_not() {
        let (_, ir, interner) = run(
            "class Foo\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       auto fn = [a]() -> uint32 { return a + _x; };\n\
             \x20       return 0;\n    }\n\n    uint32 _x = 3;\n}\n\n\
             inline uint32 g()\n{\n\
             \x20   auto h = [a]() -> uint32 { return a; };\n    return 0;\n}\n",
        );
        assert_eq!(
            scopes(&ir, &interner),
            vec![vec!["This[Foo,Run,$]".to_owned(), "a".to_owned()], vec!["a".to_owned()]]
        );
    }

    /// **Unconditional.** The lambda reads nothing the class declares and is
    /// captured anyway — 111 of the corpus's 648 in-class lambdas are this
    /// shape. `--passes 3` on `[]() -> uint32 { return 5; }` in a class:
    /// `lambdaCaptures = SeqF [ThisF {declScopeName = ["Foo","Run","$"]}]`.
    #[test]
    fn a_lambda_that_touches_no_member_is_captured_too() {
        let (_, ir, interner) = run(
            "class Foo\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       auto fn = []() -> uint32 { return 5; };\n\
             \x20       return 0;\n    }\n}\n",
        );
        assert_eq!(scopes(&ir, &interner), vec![vec!["This[Foo,Run,$]".to_owned()]]);
    }

    /// `cons` prepends (`Desugar.hs:106-109`), so the `This` is argument 0.
    /// Anything else slides every explicit capture's index by one and
    /// `uncaptureVar` reads the wrong field out of the capture struct.
    #[test]
    fn the_this_is_prepended_not_appended() {
        let (_, ir, interner) = run(
            "class Foo\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       auto fn = [a, b]() -> uint32 { return a + b; };\n\
             \x20       return 0;\n    }\n}\n",
        );
        assert_eq!(
            scopes(&ir, &interner),
            vec![vec!["This[Foo,Run,$]".to_owned(), "a".to_owned(), "b".to_owned()]]
        );
    }

    /// Each `This` names the scope of the lambda it was put on, not the class's
    /// — `l@LambdaF{..}` binds `declScopeName` from the lambda. Reference at
    /// `--passes 3`: `["Outer","Run","$"]` for the outer lambda and
    /// `["Outer","Run","$","?lambda0","$"]` for the one nested inside it.
    #[test]
    fn a_nested_lambda_gets_its_own_scope() {
        let (_, ir, interner) = run(
            "class Outer\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       auto fn = []() -> uint32 {\n\
             \x20           auto inner = []() -> uint32 { return 1; };\n\
             \x20           return 2;\n        };\n\
             \x20       return 0;\n    }\n}\n",
        );
        assert_eq!(
            scopes(&ir, &interner),
            vec![
                vec!["This[Outer,Run,$]".to_owned()],
                vec!["This[Outer,Run,$,?lambda0,$]".to_owned()],
            ]
        );
    }

    /// `captureThis' ClassF{} = Nothing`. A class declared inside a member
    /// function is a boundary: its lambda is captured once, by the inner class,
    /// with the inner scope. Reference at `--passes 3` on the same shape:
    /// one `ThisF`, `declScopeName = ["Host","Go","$","Nested","Inner","$"]`.
    #[test]
    fn a_class_nested_in_a_member_function_is_a_boundary() {
        let (_, ir, interner) = run(
            "class Host\n{\npublic:\n    inline uint32 Go()\n    {\n\
             \x20       class Nested\n        {\n        public:\n\
             \x20           inline uint32 Inner()\n            {\n\
             \x20               auto fn = []() -> uint32 { return 1; };\n\
             \x20               return 0;\n            }\n        }\n\
             \x20       return 0;\n    }\n}\n",
        );
        assert_eq!(scopes(&ir, &interner), vec![vec!["This[Host,Go,$,Nested,Inner,$]".to_owned()]]);
    }

    /// The `Template` wrapper is transparent in both directions: the outer
    /// `cata` reaches the `Class` under it, and the inner walk still stops at a
    /// nested `Class` under one. Reference at `--passes 3` on a template class
    /// holding a member function *and* a nested class, both with a lambda:
    ///
    /// ```text
    /// lambdaCaptures = SeqF [ThisF {declScopeName = ["Outer","Go","$"]}]
    /// lambdaCaptures = SeqF [ThisF {declScopeName = ["Outer","Inner","In","$"]}]
    /// ```
    ///
    /// This is the one place where a `Nothing` arm interacts with the other
    /// rewrite in this module — `abbreviatedFunctionTemplate` declines at
    /// `Template` and this pass does not, so the two walks disagree on that
    /// node by design.
    #[test]
    fn a_template_wrapper_is_transparent_but_the_class_under_it_is_not() {
        let (_, ir, interner) = run(
            "template <typename T>\nclass Outer\n{\npublic:\n    inline uint32 Go()\n    {\n\
             \x20       auto fn = []() -> uint32 { return 1; };\n\
             \x20       return 0;\n    }\n\n    class Inner\n    {\n    public:\n\
             \x20       inline uint32 In()\n        {\n\
             \x20           auto g = []() -> uint32 { return 2; };\n\
             \x20           return 0;\n        }\n    }\n}\n",
        );
        assert_eq!(
            scopes(&ir, &interner),
            vec![
                vec!["This[Outer,Go,$]".to_owned()],
                vec!["This[Outer,Inner,In,$]".to_owned()],
            ]
        );
    }

    /// **The donor test** (`CLAUDE.md`, "A rewrite's `Src` donors need a unit
    /// test read off `--notes`"). `copyNote (ThisF …) lambdaCaptures` hands the
    /// new node the *capture list's* note. Read off the reference for
    /// `[]() -> uint32 { … }` at line 6 column 19:
    ///
    /// ```text
    /// ThisF {…} @Src {offset = 69, 6:20-6:20}
    /// ]        @Src {offset = 69, 6:20-6:20}     -- the SeqF around it
    /// ```
    ///
    /// — identical, and both different from the `LambdaF`'s. Swapping the donor
    /// for the lambda passes every gate in the tree and fails only this.
    #[test]
    fn the_this_wears_the_capture_lists_src_and_not_the_lambdas() {
        let (before, after, _) = run(
            "class Foo\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       auto fn = []() -> uint32 { return 5; };\n\
             \x20       return 0;\n    }\n}\n",
        );
        let (lambda, caps) = lambdas(&after).remove(0);
        let this = caps[0];

        // The capture list was rebuilt, so read the donor's range off the IR
        // this pass was handed rather than off its own output.
        let (_, before_caps) = lambdas(&before).remove(0);
        assert!(before_caps.is_empty(), "the fixture writes `[]`");

        let Exp::Lambda { captures: written, .. } = before.arena[lambdas(&before)[0].0].exp else {
            panic!()
        };
        assert_eq!(
            after.arena[this].src.head, before.arena[written].src.head,
            "copyNote of `lambdaCaptures` (Desugar.hs:104)"
        );
        assert_ne!(
            after.arena[this].src.head, after.arena[lambda].src.head,
            "and not of the lambda — Frontend.hs:190 binds `lambdaCaptures`"
        );
        assert_eq!(after.arena[this].kind, after.arena[written].kind);
    }

    /// The `written` half of the same rule. The `This` is invented, so it is
    /// `Donated`; the capture `Seq` it is prepended to replaces a node that
    /// goes unreachable in the same step, so it keeps `Own`.
    #[test]
    fn the_this_is_donated_and_the_rebuilt_list_keeps_own() {
        let (_, ir, _) = run(
            "class Foo\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       auto fn = []() -> uint32 { return 5; };\n\
             \x20       return 0;\n    }\n}\n",
        );
        let (lambda, caps) = lambdas(&ir).remove(0);
        let Exp::Lambda { captures, .. } = ir.arena[lambda].exp else { panic!() };
        assert_eq!(ir.arena[caps[0]].src.written, kg_base::Written::Donated);
        assert_eq!(ir.arena[captures].src.written, kg_base::Written::Own);
    }

    /// A file with no class allocates nothing: the `cata`'s
    /// `out == original → keep the id` rule (`rewrite/mod.rs`) makes an
    /// untouched tree free, and the whole-corpus `rewrite:` gate depends on it.
    #[test]
    fn a_file_with_no_class_is_not_rebuilt() {
        let (before, after, _) = run(
            "inline uint32 g()\n{\n\
             \x20   auto h = [a]() -> uint32 { return a; };\n    return 0;\n}\n",
        );
        assert_eq!(before.root, after.root);
        assert_eq!(before.arena.len(), after.arena.len(), "no node was appended");
    }
}
