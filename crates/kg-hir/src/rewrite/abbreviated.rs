//! `abbreviatedFunctionTemplate` (`Function.hs:288-311`) — inner sub-pass 3 of
//! `instantiateTemplates` (`Template.hs:77`), and the first of pass 8's five
//! rewrites to land.
//!
//! `void f(auto a, uint8 b, auto c)` becomes **two** declarations, not one:
//!
//! ```text
//! template <typename a$T, typename c$T> void f(a$T a, uint8 b, c$T c) { … }
//! void f(auto a, uint8 b, auto c) { … }          // the leftover, kept
//! ```
//!
//! The leftover is not an oversight. `moveFunctions` defers on exactly it
//! (`move t@Function{} _ | isAbbreviatedTemplate t = t`, `Function.hs:89-90`),
//! `deduplicateInstances` does not collapse the pair because it compares a
//! `TemplateF` against a `FunctionF` (`Template.hs:147-154`), and it survives
//! until `removeTemplate` seven frontend passes later
//! (`removeTemplate x | isAbbreviatedTemplate x = SeqF []`, `Frontend.hs:300-302`).
//! Deleting it early breaks sub-pass 4's contract.
//!
//! # The three clauses, and the one that is a revert
//!
//! ```haskell
//! abbreviatedFunctionTemplate n f
//!     | isAbbreviatedTemplate f = Just $ SeqF [ … template … , NotedExp n f ]   -- :289-292
//! abbreviatedFunctionTemplate _ TemplateF{} = Nothing                            -- :310
//! abbreviatedFunctionTemplate _ e = Just $ removeNestedSeq e                     -- :311
//! ```
//!
//! Clause 2 returns `Nothing`, which under `desugarShallow'` restores `para`'s
//! *first* argument — the original subtree, children's rewrites discarded
//! (`Desugar.hs:81-84`, `Recursion.hs:73-74`). So an abbreviated member of a
//! **class template** comes out byte-identical even though the inner function
//! did rewrite and the `ClassF` did propagate it. Verified against the
//! reference: `--template-passes 2` and `3` (with `--template-iterations 0`)
//! print identical trees for `template <typename T> class D { void n(auto a); }`
//! and differ for the same declaration in a plain `class C`. That is why
//! [`ShallowRewrite::reverts`] exists and why this pass's guard is `Template`
//! and nothing else.
//!
//! # `isAuto` does not look through anything
//!
//! `isAuto AutoF = True; isAuto _ = False` (`Parser/Syntax.hs:706-707`). A
//! `const auto` parameter parses as `ConstF AutoF` and is **not** an
//! abbreviated template — confirmed against the reference, `inline void
//! f(const auto a) { }` is untouched at `--template-passes 3`. Descending
//! through `Exp::Const` here would fabricate a template parameter for every
//! `const auto` in the corpus.
//!
//! # Where the `Src`s come from, which is not uniform
//!
//! Four donors, and the asymmetry inside a single synthesized `TypeParam` is
//! the trap (`--notes` on `inline void f(auto a, uint8 b, auto c) { }`, ranges
//! as the reference prints them):
//!
//! | node | donor | range |
//! |---|---|---|
//! | `Template`, rewritten `Function`, leftover `Function` | the function's own note, `NotedExp n` (`:290-291`) | the whole declaration |
//! | `TemplateParam` (`:296`) | the **`FuncParam`** | `auto a`, 14..20 |
//! | its `Typename` (`:299`) | the **`FuncParam`** | `auto a`, 14..20 |
//! | `TypeParam` (`:307`) | the **`Auto`** node | `auto`, 14..19 |
//! | its `Typename` (`:299`, evaluated a second time) | the **`FuncParam`** | `auto a`, 14..20 |
//!
//! The two `Typename`s are **two nodes, not one**. `typeParam` reuses field `b`
//! of the `TemplateParamF` *it is handed* (`Function.hs:130-132`), and the one
//! handed to it at `:307` is a second, independently constructed value —
//! `templateParam (findParam p)` — never the one that landed in
//! `templateParams`. `docs/journal.md` said this pass "proposes to violate
//! [the no-sharing rule] by sharing one `Typename`"; it does not, there is no
//! DAG here, and no `deep_copy` is needed for it.
//!
//! # The one genuine DAG, and which half keeps `Own`
//!
//! `f' = f {funcParams = …}` is a record update: in Haskell the template's copy
//! and the leftover *share* the body, the name, the return type and the
//! attributes. An arena cannot, so one of the two is a [`Rewriter::deep_copy`].
//!
//! It is the **leftover**. The template body is what `makeInstance` copies and
//! what an instantiated diagnostic points into — `Src::expanded` deliberately
//! keeps `written` (`kg-base/src/src.rs:76-88`) — and the leftover is deleted
//! at `Frontend.hs:300-302` regardless. Copying the half that survives would
//! empty every `Own` index the editor answers from, for the half that lives.

use kg_base::{Symbol, Written};
use kg_ty::QualifiedName;

use super::{Rewriter, ShallowRewrite};
use crate::facts::{Flags, AUTO_PARAMS, NESTED_SEQ};
use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;

/// See the module comment.
#[derive(Debug, Default)]
pub struct AbbreviatedFunctionTemplate {
    /// How many declarations clause 1 rewrote — for the gate's report, not for
    /// any decision.
    pub fired: u32,
    /// A later round's leftovers, left alone: see
    /// [`AbbreviatedFunctionTemplate::later_round`].
    kept: kg_base::FxHashSet<ExpId>,
}

impl AbbreviatedFunctionTemplate {
    /// The pass as a later round runs it (`Template.hs:77`, every round).
    /// The reference rewrites the leftover it kept beside each template
    /// again, round after round — a second `TemplateF` of one name, whose
    /// copies `deduplicateInstances` then drops (`Template.hs:144-155`) and
    /// whose name `getSymbols` resolves to the first (`Symbols.hs:42`). The
    /// port's template index poisons a name declared twice, so the leftover
    /// — an abbreviated function standing right after the template made
    /// from it — is left as it is; what a later round does template is new:
    /// a hoisted generic lambda, or a copy that still has an `auto`
    /// parameter (`Fn_T_auto<|$bool|>`).
    ///
    /// Only what the pass reaches asks whether it is kept, and it reaches
    /// nothing it would hand back as it came ([`ShallowRewrite::needs`]):
    /// a leftover that matters holds the shapes the pass needs, and so does
    /// every node above it, so the search enters only those.
    pub fn later_round(arena: &mut ExpArena, root: ExpId) -> AbbreviatedFunctionTemplate {
        let mut kept = kg_base::FxHashSet::default();
        let mut stack = vec![root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            if arena.lacks(id, NEEDS) {
                continue;
            }
            if let Exp::Seq(items) = &arena[id].exp {
                for pair in items.windows(2) {
                    let [t, f] = pair else { continue };
                    let Exp::Template { decl, .. } = &arena[*t].exp else { continue };
                    let (Exp::Function { name: a, .. }, Exp::Function { name: b, .. }) =
                        (&arena[*decl].exp, &arena[*f].exp)
                    else {
                        continue;
                    };
                    if matches!((&arena[*a].exp, &arena[*b].exp), (Exp::Identifier(x), Exp::Identifier(y)) if x == y) {
                        kept.insert(*f);
                    }
                }
            }
            kids.clear();
            arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
        AbbreviatedFunctionTemplate { fired: 0, kept }
    }
}

/// What the pass rewrites: a function with an `auto` parameter, and a `Seq`
/// in a `Seq` for the catch-all's `removeNestedSeq` (`Function.hs:311`).
const NEEDS: Flags = AUTO_PARAMS | NESTED_SEQ;

impl ShallowRewrite for AbbreviatedFunctionTemplate {
    fn needs(&self) -> Option<Flags> {
        Some(NEEDS)
    }

    /// `abbreviatedFunctionTemplate _ TemplateF{} = Nothing` (`Function.hs:310`).
    fn reverts(&self, arena: &ExpArena, id: ExpId) -> bool {
        matches!(arena[id].exp, Exp::Template { .. }) || self.kept.contains(&id)
    }

    fn rewrite(&mut self, rw: &mut Rewriter, id: ExpId, exp: Exp) -> Exp {
        let Some(autos) = self.auto_params(rw.arena, &exp) else {
            // `Just $ removeNestedSeq e` (`Function.hs:311`) — the arm that
            // makes the pass propagate at all, and the only `Seq` flattener
            // this rewrite needs.
            return rw.remove_nested_seq(exp);
        };
        self.fired += 1;

        // `templateParams` (`Function.hs:296`).
        let template_params: Vec<ExpId> =
            autos.iter().map(|&(i, fp)| self.template_param(rw, i, fp)).collect();

        // `f' = f {funcParams = desugar autoParamType $ funcParams f}`
        // (`:304-308`). The reference runs a full `cata` over the whole
        // `funcParams` subtree; every node it rebuilds outside the `auto`
        // parameters is value-identical to its input, so rebuilding only the
        // changed parameters is observationally the same and leaves the rest
        // of the subtree sharing its ids with the leftover — which is safe
        // because the leftover is deep-copied below.
        //
        // The one case where the cata's extra reach is observable is a
        // *nested* `FuncParam` with an `auto` type: `autoParams` is built from
        // the parameter `Seq`'s top level only (`:294`), so `findParam`'s
        // `fromJust` (`:302`) would crash the reference there. Restricting to
        // the top level cannot invent a diagnostic and cannot crash.
        let params_seq = func_params(&exp).expect("checked by auto_params");
        let members = seq_items(rw.arena, params_seq).expect("checked by auto_params");
        let mut rebuilt: Vec<ExpId> = Vec::with_capacity(members.len());
        for p in members {
            if is_auto_param(rw.arena, p) {
                let ty = self.type_param(rw, &autos, p);
                let mut param = rw.arena[p].exp.clone();
                if let Exp::FuncParam { ty: slot, .. } = &mut param {
                    *slot = ty;
                }
                rebuilt.push(rw.replace(p, param));
            } else {
                rebuilt.push(p);
            }
        }
        let new_params = rw.replace(params_seq, Exp::Seq(rebuilt));
        let f_prime = rw.replace(id, with_params(exp.clone(), new_params));

        // `NotedExp n f` (`:291`) — the leftover carries the *children's*
        // rewrites, not the pristine node, so it is copied from `exp`.
        let mut leftover_exp = exp;
        for slot in leftover_exp.children_mut() {
            *slot = rw.deep_copy(*slot);
        }
        let leftover = rw.donate_keeping_note(id, leftover_exp);

        // `NotedExp n $ TemplateF …` (`:290`). Donated, not a replacement: the
        // node standing in for this declaration's text is `f_prime`, and two
        // `Own` claimants of one `(range, kind)` is the collision divergence
        // #21 warns about.
        let template = rw.donate_keeping_note(id, Exp::Template { params: template_params, decl: f_prime });

        Exp::Seq(vec![template, leftover])
    }
}

impl AbbreviatedFunctionTemplate {
    /// `isAbbreviatedTemplate` (`Parser/Syntax.hs:722-724`) and `autoParams`
    /// (`Function.hs:294`) in one pass: `None` when the clause-1 guard fails.
    ///
    /// The index is `zip [0..]` applied **after** the filter, so it counts
    /// `auto` parameters only — `c` in `f(auto a, uint8 b, auto c)` is written
    /// third and gets index 1. A port that uses the written position is
    /// invisible on any function whose `auto` parameters start at 0 and are
    /// contiguous, which is most of them.
    fn auto_params(&self, arena: &ExpArena, exp: &Exp) -> Option<Vec<(u32, ExpId)>> {
        let members = seq_items(arena, func_params(exp)?)?;
        let autos: Vec<(u32, ExpId)> = members
            .into_iter()
            .filter(|&p| is_auto_param(arena, p))
            .enumerate()
            .map(|(i, p)| (i as u32, p))
            .collect();
        (!autos.is_empty()).then_some(autos)
    }

    /// `templateParam` (`Function.hs:298-299`), donated by the `FuncParam` at
    /// both levels.
    fn template_param(&self, rw: &mut Rewriter, index: u32, param: ExpId) -> ExpId {
        let (scope, name) = self.param_key(rw, param);
        let kind = rw.donate(param, Exp::Typename);
        rw.donate(param, Exp::TemplateParam { scope, kind, name, index, default: None })
    }

    /// `typeParam $ templateParam $ findParam p` (`Function.hs:307`, `:130-132`).
    ///
    /// `findParam` matches by **name**, not by position or id
    /// (`findByName`, `Function.hs:283-286`), and its `fromJust` is why the
    /// lookup here cannot miss: `p` came out of the same filtered list.
    fn type_param(&self, rw: &mut Rewriter, autos: &[(u32, ExpId)], param: ExpId) -> ExpId {
        let name = param_name(rw.arena, param);
        let &(index, donor) = autos
            .iter()
            .find(|&&(_, q)| param_name(rw.arena, q) == name)
            .expect("an auto parameter is in the list it was filtered into");
        let (scope, suffixed) = self.param_key(rw, donor);
        // A *second* `Typename`, donated by the `FuncParam` — see the module
        // comment. The `TypeParam` around it is donated by the `Auto` node.
        let kind = rw.donate(donor, Exp::Typename);
        let auto = param_ty(rw.arena, param);
        rw.donate(auto, Exp::TypeParam { scope, kind, name: suffixed, index })
    }

    /// The scope and name a synthesized parameter wears: `init declScopeName`
    /// and `suffixName "$T" name` (`Function.hs:299`).
    ///
    /// A `FuncParam`'s scope is `enclosing ++ [functionName]` — the parameter
    /// list is parsed inside `scoped ident` (`Parser.hs:379`, `:406-412`) — so
    /// `init` recovers exactly the scope an explicitly written template
    /// parameter would have been given (`Parser.hs:352-354`).
    fn param_key(&self, rw: &Rewriter, param: ExpId) -> (QualifiedName, Symbol) {
        let Exp::FuncParam { scope, .. } = &rw.arena[param].exp else {
            unreachable!("checked by is_auto_param")
        };
        let scope = scope.init();
        (scope, rw.suffix_name(param_name(rw.arena, param), "$T"))
    }
}

fn func_params(exp: &Exp) -> Option<ExpId> {
    match exp {
        // `LambdaF` has `funcParams` too, but `isAbbreviatedTemplate _ = False`
        // for it (`Parser/Syntax.hs:724`), so the match is on the variant and
        // never on the kind.
        //
        // In the reference that is load-bearing: `desugarLambdas` runs
        // immediately before this pass and turns `[](auto x){…}` into a
        // `FunctionF` with `funcKind = LambdaFunction`, which then fires —
        // and since `rewrite::lambdas` landed, `rewrite::pipeline` reproduces
        // that here: 55 synthesized lambda functions across the corpus (23
        // under `library/`) are rewritten by this pass when composed. Run
        // standalone over the raw lowering — which is what `xtask ir`'s
        // `rewrite:` line grades — a generic lambda is still an `Exp::Lambda`
        // and this pass declines it. The composed count exceeds the
        // reference's own gap (12) because `higherOrderFunctions` is not
        // ported: in the reference it has already `Template`-wrapped
        // `library/data/function.k`'s `apply`/`bind` family by pass 8, so the
        // clause-2 revert discards the rewrites inside them (reference fires
        // on 3 lambda functions there; the composed pipeline, with nothing
        // wrapped, on 21).
        Exp::Function { params, .. } | Exp::FunctionDecl { params, .. } => Some(*params),
        _ => None,
    }
}

fn with_params(exp: Exp, new: ExpId) -> Exp {
    match exp {
        Exp::Function { scope, kind, attr, modifier, ret, name, body, .. } => {
            Exp::Function { scope, kind, attr, modifier, ret, name, params: new, body }
        }
        Exp::FunctionDecl { scope, attr, ret, name, .. } => {
            Exp::FunctionDecl { scope, attr, ret, name, params: new }
        }
        other => other,
    }
}

fn seq_items(arena: &ExpArena, id: ExpId) -> Option<Vec<ExpId>> {
    match &arena[id].exp {
        Exp::Seq(items) => Some(items.clone()),
        _ => None,
    }
}

fn param_ty(arena: &ExpArena, param: ExpId) -> ExpId {
    match &arena[param].exp {
        Exp::FuncParam { ty, .. } => *ty,
        _ => unreachable!("checked by is_auto_param"),
    }
}

/// `isAbbreviatedTemplate`'s `FunctionF`/`FunctionDeclF` arms
/// (`Parser/Syntax.hs:722-723`) over a parameter list: any parameter whose
/// written type is bare `auto` — `const auto` is not (`isAuto`, `:706-707`),
/// and no name is asked. What the deferrals and the walk ask of a function
/// (`moveFunctions`, `Function.hs:89-90`; `removeTemplate`,
/// `Frontend.hs:300-301`); the rewrite's own guard is [`is_auto_param`].
pub fn is_abbreviated_template(arena: &ExpArena, params: ExpId) -> bool {
    let Exp::Seq(ps) = &arena[params].exp else { return false };
    ps.iter().any(|&p| match &arena[p].exp {
        Exp::FuncParam { ty, .. } => matches!(arena[*ty].exp, Exp::Auto),
        _ => false,
    })
}

/// `isAuto . paramType` (`Parser/Syntax.hs:706-707,722`) — bare `Auto` only —
/// **and** a written name.
///
/// The reference's guard does not check the name; `templateParam _ = undefined`
/// (`Function.hs:300`) crashes instead if there is none. That arm is
/// unreachable through the grammar (`inline void f(auto) { }` is a parse error,
/// and `param` always lowers a written name, `Parser.hs:379`), so folding the
/// check into the guard cannot change behaviour on any parseable program — and
/// where the reference would crash, this declines the rewrite rather than
/// panicking inside an editor.
fn is_auto_param(arena: &ExpArena, param: ExpId) -> bool {
    match &arena[param].exp {
        Exp::FuncParam { ty, name, .. } => {
            matches!(arena[*ty].exp, Exp::Auto)
                && matches!(arena[*name].exp, Exp::Identifier(_))
        }
        _ => false,
    }
}

fn param_name(arena: &ExpArena, param: ExpId) -> Symbol {
    let Exp::FuncParam { name, .. } = &arena[param].exp else {
        unreachable!("checked by is_auto_param")
    };
    match &arena[*name].exp {
        Exp::Identifier(sym) => *sym,
        _ => unreachable!("checked by is_auto_param"),
    }
}

/// Clause four of the provenance invariant, stated so that it bites.
///
/// "Forbid an `Own` node the lowering did not create" is too strong as
/// written: in an append-only arena a *replacement* — a node whose children
/// were rewritten — is necessarily a fresh index, and it may legitimately keep
/// `Own`, because the node it replaced goes unreachable in the same step and
/// leaves no second claimant. The rewritten root `Seq` of any file containing
/// an abbreviated function is exactly that node.
///
/// What must be forbidden is a *copy* wearing `Own`: `moveFunctions`' spliced
/// member-name `Identifier` is shape-**equal** to the declaration it was copied
/// from, so the equal-shape tolerance in `xtask ir`'s clause three waves it
/// through. So:
///
/// > an `Own` node reachable from the root, appended after the lowering
/// > finished, must not be structurally equal to a node the lowering built at
/// > the same `(file, range, kind)`.
///
/// A replacement differs in shape from what it replaced, by definition. A copy
/// does not. This also makes the design entry's third falsification bite —
/// re-allocate an unchanged subtree and the new node is shape-equal to the
/// lowered one at its own key.
pub fn own_nodes_are_not_copies(
    ir: &crate::ModuleIr,
    interner: &kg_base::Interner,
) -> Result<(), String> {
    let lowered = ir.lowered_len as usize;
    let mut by_key: std::collections::HashMap<
        (kg_base::FileId, u32, u32, kg_syntax::SyntaxKind),
        Vec<ExpId>,
    > = std::collections::HashMap::new();
    for (id, data) in ir.arena.iter() {
        if id.index() >= lowered {
            continue;
        }
        let r = data.src.head.range;
        by_key
            .entry((data.src.head.file, r.start().into(), r.end().into(), data.kind))
            .or_default()
            .push(id);
    }

    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children = Vec::new();
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        let data = &ir.arena[id];
        if data.src.written == Written::Own && id.index() >= lowered {
            let r = data.src.head.range;
            let key = (data.src.head.file, r.start().into(), r.end().into(), data.kind);
            if let Some(originals) = by_key.get(&key) {
                let mine = crate::structure::render_subtree(ir, interner, id);
                for &other in originals {
                    if crate::structure::render_subtree(ir, interner, other) == mine {
                        return Err(format!(
                            "appended own node #{} {:?} at {}..{} is structurally equal to \
                             the lowered node #{} at the same key — a copy must be Donated",
                            id.index(),
                            data.kind,
                            key.1,
                            key.2,
                            other.index()
                        ));
                    }
                }
            }
        }
        data.exp.children(&mut children);
        stack.append(&mut children);
    }
    Ok(())
}

/// The pass's defining property, stated directly rather than inferred from a
/// collision.
///
/// `abbreviatedFunctionTemplate _ TemplateF{} = Nothing` (`Function.hs:310`)
/// restores para's original subtree, so **no `Template` node's id ever
/// changes** — not the node, not anything under it. Every `Template` reachable
/// in the input is reachable, by the same id, in the output.
///
/// This exists because the gate that was supposed to catch a broken revert
/// caught it for the wrong reason and therefore not everywhere. Deleting the
/// guard makes the rewrite emit a two-element `Seq` where a declaration was;
/// where that declaration's parent is itself a `Seq`, the enclosing
/// `remove_nested_seq` splices the pair flat and the tree is quietly wrong with
/// no `TypeKey` collision to trip over. Measured: 12 of the 14 affected corpus
/// files failed, and the two that passed — `library/processor/risc_v.k` and
/// `test/syntax/helper/template_argument_deduction.k` — were the
/// class-template-member form, which is the *sharpest* case of the revert, not
/// the weakest. This check sees all 14.
pub fn templates_are_untouched(
    before: &crate::ModuleIr,
    after: &crate::ModuleIr,
) -> Result<(), String> {
    let mut wanted = Vec::new();
    walk(before, |ir, id| {
        if matches!(ir.arena[id].exp, Exp::Template { .. }) {
            wanted.push(id);
        }
    });
    let mut kept = std::collections::HashSet::new();
    walk(after, |ir, id| {
        if matches!(ir.arena[id].exp, Exp::Template { .. }) {
            kept.insert(id);
        }
    });
    for id in wanted {
        if !kept.contains(&id) {
            return Err(format!(
                "Template #{} was rebuilt or dropped; the revert must return the \
                 original subtree (Function.hs:310, Desugar.hs:81-84)",
                id.index()
            ));
        }
    }
    Ok(())
}

fn walk(ir: &crate::ModuleIr, mut f: impl FnMut(&crate::ModuleIr, ExpId)) {
    let mut seen = vec![false; ir.arena.len()];
    let mut stack = vec![ir.root];
    let mut children = Vec::new();
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        f(ir, id);
        ir.arena[id].exp.children(&mut children);
        stack.append(&mut children);
    }
}

/// The reference is the grader here, not this port's reading of it.
///
/// Every `ORACLE_*` expectation below was read off
/// `kg-oracle --no-base --passes 8 --template-passes {2,3} --template-iterations 0`
/// (`--tree` and `--notes`), the reference frontend built by `oracle/build.sh`,
/// captured 2026-08-12. `--passes 8` is not optional: without it the frontend
/// runs all nineteen passes and `removeTemplate` (`Frontend.hs:300-302`)
/// deletes both halves of this pass's output, so a fixture prints `SeqF []`
/// and looks like a parse failure.
#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::ast;
    use kg_syntax::AstNode;

    use super::*;
    use crate::rewrite::abbreviated_function_template;
    use crate::{lower, ModuleIr};

    fn run(text: &str) -> (ModuleIr, ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "probe must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let before = lower(&file, FileId(0), &interner);
        let after = abbreviated_function_template(&before, &interner);
        (before, after, interner)
    }

    fn kids(ir: &ModuleIr, id: ExpId) -> Vec<ExpId> {
        let mut out = Vec::new();
        ir.arena[id].exp.children(&mut out);
        out
    }

    fn range(ir: &ModuleIr, id: ExpId) -> (u32, u32) {
        let r = ir.arena[id].src.head.range;
        (r.start().into(), r.end().into())
    }

    /// `inline void f(auto a, uint8 b, auto c) { }` at `--template-passes 3`:
    ///
    /// ```text
    /// SeqF [TemplateF {templateParams = [TemplateParamF [] TypenameF "a$T" 0 Nothing
    ///                                   ,TemplateParamF [] TypenameF "c$T" 1 Nothing]
    ///                 ,templateDecl = FunctionF {… paramType = TypeParamF [] TypenameF "a$T" 0
    ///                                            … paramType = UnsignedF 8
    ///                                            … paramType = TypeParamF [] TypenameF "c$T" 1 …}}
    ///      ,FunctionF {… AutoF … UnsignedF 8 … AutoF …}]
    /// ```
    #[test]
    fn one_declaration_becomes_a_template_and_the_original() {
        let (_, ir, interner) = run("inline void f(auto a, uint8 b, auto c) { }\n");
        let top = kids(&ir, ir.root);
        assert_eq!(top.len(), 2, "the leftover copy is kept (Function.hs:291)");
        let Exp::Template { params, decl } = &ir.arena[top[0]].exp else {
            panic!("first is the template, got {:?}", ir.arena[top[0]].exp)
        };
        assert!(matches!(ir.arena[top[1]].exp, Exp::Function { .. }), "second is the leftover");

        // `zip [0..]` is applied AFTER the filter (`Function.hs:294`): `c` is
        // written third and gets index 1.
        let names: Vec<(String, u32)> = params
            .iter()
            .map(|&p| match &ir.arena[p].exp {
                Exp::TemplateParam { name, index, kind, scope, default } => {
                    assert!(matches!(ir.arena[*kind].exp, Exp::Typename));
                    assert!(default.is_none());
                    assert!(scope.is_empty(), "init [\"f\"] = []");
                    (interner.resolve(*name).to_string(), *index)
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, vec![("a$T".to_owned(), 0), ("c$T".to_owned(), 1)]);

        let Exp::Function { params: seq, .. } = &ir.arena[*decl].exp else { panic!() };
        let tys: Vec<String> = kids(&ir, *seq)
            .iter()
            .map(|&p| match &ir.arena[p].exp {
                Exp::FuncParam { ty, .. } => match &ir.arena[*ty].exp {
                    Exp::TypeParam { name, index, .. } => {
                        format!("{}#{index}", interner.resolve(*name))
                    }
                    other => format!("{other:?}"),
                },
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(tys, vec!["a$T#0", "Unsigned(8)", "c$T#1"]);
    }

    /// `--template-passes 2` and `3` print byte-identical trees for
    /// `template <typename T> inline void g(T a, auto b) { }`. `Nothing`
    /// restores `para`'s first argument (`Desugar.hs:81-84`,
    /// `Recursion.hs:73-74`), so the assertion is `ExpId` identity, which is
    /// only expressible if the revert returns the *original* id.
    #[test]
    fn a_template_reverts_and_keeps_its_ids() {
        let (before, after, _) =
            run("template <typename T>\ninline void g(T a, auto b) { }\n");
        assert_eq!(before.root, after.root, "nothing changed, so nothing was rebuilt");
        assert_eq!(kids(&before, before.root), kids(&after, after.root));
        assert_eq!(before.arena.len(), after.arena.len(), "the revert allocates nothing");
    }

    /// The sharpest form: the rewrite fires two levels below the `Template`,
    /// and the revert throws it away anyway. A plain `class C` in the same file
    /// keeps its rewrite, so this is not "the pass did nothing".
    #[test]
    fn a_class_templates_member_reverts_while_a_plain_classs_does_not() {
        let (_, ir, _) = run(
            "class C\n{\n    inline void m(auto a) { }\n}\n\
             template <typename T>\nclass D\n{\n    inline void n(auto a) { }\n}\n",
        );
        let top = kids(&ir, ir.root);
        assert_eq!(top.len(), 2, "two classes");

        let Exp::Class { body, .. } = &ir.arena[top[0]].exp else { panic!("class C") };
        assert_eq!(kids(&ir, *body).len(), 2, "C::m became a template and a leftover");

        let Exp::Template { decl, .. } = &ir.arena[top[1]].exp else { panic!("template D") };
        let Exp::Class { body, .. } = &ir.arena[*decl].exp else { panic!("class D") };
        let members = kids(&ir, *body);
        assert_eq!(members.len(), 1, "D::n reverted");
        let Exp::Function { params, .. } = &ir.arena[members[0]].exp else { panic!() };
        let param = kids(&ir, *params)[0];
        let Exp::FuncParam { ty, .. } = &ir.arena[param].exp else { panic!() };
        assert!(matches!(ir.arena[*ty].exp, Exp::Auto), "still `auto`, not a TypeParam");
    }

    /// The `--notes` answer key. For `inline void f(auto a, uint8 b, auto c) { }`
    /// the reference prints the `TemplateParam` and its `Typename` at the
    /// **`FuncParam`**'s span and the `TypeParam` at the **`Auto`**'s, and its
    /// `Typename` back at the `FuncParam`'s. Ranges here are this port's
    /// (trivia-trimmed) equivalents of the reference's 14..20 / 14..19.
    #[test]
    fn the_two_donors_inside_one_synthesized_type_param_differ() {
        let (before, ir, _) = run("inline void f(auto a, uint8 b, auto c) { }\n");
        // `auto a` and `auto` as the lowering sees them, read off the input.
        let f = kids(&before, before.root)[0];
        let Exp::Function { params, .. } = &before.arena[f].exp else { panic!() };
        let param0 = kids(&before, *params)[0];
        let Exp::FuncParam { ty: auto0, .. } = &before.arena[param0].exp else { panic!() };
        let (param_range, auto_range) = (range(&before, param0), range(&before, *auto0));
        assert_ne!(param_range, auto_range, "the fixture must be able to tell them apart");

        let Exp::Template { params, decl } = &ir.arena[kids(&ir, ir.root)[0]].exp else {
            panic!()
        };
        // `newNote (templateParam x) $ snd x` and `newNote TypenameF e`.
        assert_eq!(range(&ir, params[0]), param_range);
        let Exp::TemplateParam { kind, .. } = &ir.arena[params[0]].exp else { panic!() };
        assert_eq!(range(&ir, *kind), param_range);

        // `newNote (typeParam …) paramType` — the `Auto` node, not the param.
        let Exp::Function { params: seq, .. } = &ir.arena[*decl].exp else { panic!() };
        let Exp::FuncParam { ty, .. } = &ir.arena[kids(&ir, *seq)[0]].exp else { panic!() };
        assert_eq!(range(&ir, *ty), auto_range, "Function.hs:307 donates the Auto node");
        let Exp::TypeParam { kind, .. } = &ir.arena[*ty].exp else { panic!() };
        assert_eq!(range(&ir, *kind), param_range, "…but its Typename is Function.hs:299's");
    }

    /// Two `Typename` nodes, never one. `typeParam` reuses field `b` of the
    /// `TemplateParamF` *it is handed* (`Function.hs:130-132`), and the one at
    /// `:307` is freshly constructed — so there is no DAG here, contrary to
    /// what the design entry claimed.
    #[test]
    fn the_two_typenames_are_two_nodes() {
        let (_, ir, _) = run("inline void f(auto a) { }\n");
        let Exp::Template { params, decl } = &ir.arena[kids(&ir, ir.root)[0]].exp else {
            panic!()
        };
        let Exp::TemplateParam { kind: a, .. } = &ir.arena[params[0]].exp else { panic!() };
        let Exp::Function { params: seq, .. } = &ir.arena[*decl].exp else { panic!() };
        let Exp::FuncParam { ty, .. } = &ir.arena[kids(&ir, *seq)[0]].exp else { panic!() };
        let Exp::TypeParam { kind: b, .. } = &ir.arena[*ty].exp else { panic!() };
        assert_ne!(a, b);
    }

    /// `isAuto AutoF = True; isAuto _ = False` (`Parser/Syntax.hs:706-707`).
    /// `const auto` parses as `ConstF AutoF` and the reference leaves it alone:
    /// `--template-passes 2` and `3` are byte-identical.
    #[test]
    fn const_auto_is_not_an_abbreviated_template() {
        let (before, after, _) = run("inline void f(const auto a) { }\n");
        assert_eq!(before.root, after.root);
        assert_eq!(before.arena.len(), after.arena.len());
    }

    /// `isAbbreviatedTemplate` has a `FunctionDeclF` arm (`Parser/Syntax.hs:723`)
    /// and the record update is total over both. Verified against the
    /// reference on `class Foo { public: void Do(auto x); }`.
    #[test]
    fn a_bodyless_member_declaration_fires_too() {
        let (_, ir, interner) = run("class Foo\n{\npublic:\n    void Do(auto x);\n}\n");
        let Exp::Class { body, .. } = &ir.arena[kids(&ir, ir.root)[0]].exp else { panic!() };
        let members = kids(&ir, *body);
        let template = members
            .iter()
            .find(|&&m| matches!(ir.arena[m].exp, Exp::Template { .. }))
            .copied()
            .expect("a template was emitted beside the declaration");
        let Exp::Template { params, decl } = &ir.arena[template].exp else { panic!() };
        assert!(matches!(ir.arena[*decl].exp, Exp::FunctionDecl { .. }));
        let Exp::TemplateParam { name, scope, .. } = &ir.arena[params[0]].exp else { panic!() };
        assert_eq!(interner.resolve(*name), "x$T");
        // `init ["Foo","Do"] = ["Foo"]`.
        assert_eq!(scope.len(), 1);
        assert!(members.iter().filter(|&&m| matches!(ir.arena[m].exp, Exp::FunctionDecl { .. }))
            .count() == 1, "the leftover declaration is kept");
    }

    /// The leftover is the copy, so it is `Donated` throughout; the template's
    /// half keeps the ids the lowering gave it and therefore keeps `Own` —
    /// which is the half `makeInstance` copies and `Src::expanded` preserves.
    #[test]
    fn the_leftover_is_the_donated_copy_and_the_template_keeps_own() {
        let (before, ir, _) = run("inline void f(auto a, uint8 b, auto c) { }\n");
        let top = kids(&ir, ir.root);
        let mut stack = vec![top[1]];
        let mut n = 0;
        while let Some(id) = stack.pop() {
            assert_eq!(
                ir.arena[id].src.written,
                Written::Donated,
                "leftover node #{} is Own",
                id.index()
            );
            n += 1;
            let mut c = Vec::new();
            ir.arena[id].exp.children(&mut c);
            stack.extend(c);
        }
        assert!(n > 5, "the whole leftover subtree was walked, not just its root");

        // The template's body node is the *same id* the lowering built.
        let Exp::Template { decl, .. } = &ir.arena[top[0]].exp else { panic!() };
        let Exp::Function { body, .. } = &ir.arena[*decl].exp else { panic!() };
        let f = kids(&before, before.root)[0];
        let Exp::Function { body: original, .. } = &before.arena[f].exp else { panic!() };
        assert_eq!(body, original);
        assert_eq!(ir.arena[*body].src.written, Written::Own);
    }

    /// Clause four of the provenance invariant, asserted here so the unit
    /// tests fail before the xtask does.
    #[test]
    fn no_appended_own_node_is_a_copy_of_a_lowered_one() {
        for probe in [
            "inline void f(auto a, uint8 b, auto c) { }\n",
            "class C\n{\n    inline void m(auto a) { }\n}\n",
            "class Foo\n{\npublic:\n    void Do(auto x);\n}\n",
            "template <typename T>\ninline void g(T a, auto b) { }\n",
            "inline void outer()\n{\n    inline void inner(auto a) { }\n}\n",
        ] {
            let (_, ir, interner) = run(probe);
            own_nodes_are_not_copies(&ir, &interner).unwrap_or_else(|e| panic!("{probe}: {e}"));
        }
    }

    /// The revert, stated as the pass's own invariant rather than inferred
    /// from a `TypeKey` collision — including the two shapes where the
    /// collision does not happen because the parent is a `Seq`.
    #[test]
    fn no_template_node_is_ever_rebuilt() {
        for probe in [
            "template <typename T>\ninline void g(T a, auto b) { }\n",
            "template <typename T>\nclass D\n{\n    inline void n(auto a) { }\n}\n",
            "class C\n{\n    template <typename T>\n    class E\n    {\n                     inline void m(auto a) { }\n    }\n}\n",
            "inline void f(auto a) { }\ntemplate <typename T>\ninline void h(T x) { }\n",
        ] {
            let (before, after, _) = run(probe);
            templates_are_untouched(&before, &after)
                .unwrap_or_else(|e| panic!("{probe}: {e}"));
        }
    }

    /// `removeNestedSeq` (`Function.hs:311`) is what splices the pair into the
    /// enclosing block; without it the function's slot would hold a `Seq`.
    #[test]
    fn the_pair_is_spliced_flat_into_the_enclosing_block() {
        let (_, ir, _) =
            run("inline void outer()\n{\n    inline void inner(auto a) { }\n}\n");
        let Exp::Function { body, .. } = &ir.arena[kids(&ir, ir.root)[0]].exp else { panic!() };
        let stmts = kids(&ir, *body);
        assert_eq!(stmts.len(), 2, "flat, not a nested Seq: {stmts:?}");
        assert!(matches!(ir.arena[stmts[0]].exp, Exp::Template { .. }));
        assert!(matches!(ir.arena[stmts[1]].exp, Exp::Function { .. }));
    }
}
