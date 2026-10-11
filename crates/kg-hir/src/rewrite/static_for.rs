//! `staticControl`'s `static for` arm (`Constexpr.hs:48-121`): a loop whose
//! induction variable reaches a constant-expression position is unrolled
//! into one nested scope per iteration, each declaring the variable as the
//! iteration's literal; any other `static for` is an `UnrolledFor`, which
//! the backend unrolls ([`super::static_control_node`]).
//!
//! The reference runs this inside `inferType` (`Inference.hs:47`), after
//! `evaluateConstexpr` has folded the limit, and so does the loop's
//! inference: a limit that is not a literal yet leaves the loop standing,
//! as the reference leaves one it cannot fold.

use kg_base::Symbol;
use kg_ty::QualifiedName;

use super::{desugar, scoped_qualifier, Rewriter};
use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;

/// `data Dependency` (`Constexpr.hs:31-40`), with its semigroup: a
/// `DependentConstexpr` absorbs everything, a `Dependent` absorbs an
/// `Independent`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dependency {
    Independent,
    Dependent,
    DependentConstexpr,
}

impl Dependency {
    fn and(self, other: Dependency) -> Dependency {
        match (self, other) {
            (Dependency::DependentConstexpr, _) => Dependency::DependentConstexpr,
            (Dependency::Dependent, Dependency::Independent) => Dependency::Dependent,
            (_, y) => y,
        }
    }
}

/// `inductionVarDependency = para depends c` (`Constexpr.hs:58-82`): whether
/// the loop variable `name` is read somewhere only a constant can stand — a
/// template argument, an array dimension, an `int<N>` width, a
/// `static assert`, a `static` expression, a nested static loop's limit or a
/// `static if`'s predicate. The match is by *name* (`depends (Identifier y)
/// _ | getName a == y`), shadowing and member names included, as written.
fn dependency(arena: &ExpArena, id: ExpId, name: Symbol) -> Dependency {
    let exp = &arena[id].exp;
    if let Exp::Identifier(y) = exp {
        return if *y == name { Dependency::Dependent } else { Dependency::Independent };
    }
    // A name no pass has resolved yet is a `Path`, its identifier a segment
    // rather than a node: `i` alone is the loop variable still.
    if let Exp::Path(p) = exp
        && let [seg] = p.segments.as_slice()
        && seg.name == name
        && seg.args.is_none()
    {
        return Dependency::Dependent;
    }
    let mut kids = Vec::new();
    exp.children(&mut kids);
    let folded = kids.iter().fold(Dependency::Independent, |acc, &k| acc.and(dependency(arena, k, name)));
    let constexpr_position = match exp {
        Exp::TemplateInstance { .. }
        | Exp::Array { .. }
        | Exp::ParamInt(_)
        | Exp::ParamUint(_)
        | Exp::StaticAssert(_)
        | Exp::Static(_) => folded == Dependency::Dependent,
        Exp::UnrolledFor { limit, .. } | Exp::StaticFor { limit, .. } => {
            dependency(arena, *limit, name) == Dependency::Dependent
        }
        Exp::StaticIf { cond, .. } => dependency(arena, *cond, name) == Dependency::Dependent,
        _ => false,
    };
    if constexpr_position { Dependency::DependentConstexpr } else { folded }
}

/// Whether `staticControl` makes a `static for` an `UnrolledForF` — its
/// induction variable reaches no constant-expression position, so the loop
/// stands whole, whatever its limit (`_ -> UnrolledForF a b c`,
/// `Constexpr.hs:54`). `decided` restricts only a `StaticForF` to its limit
/// (`Template.hs:106`); an `UnrolledForF`'s body is decided like any other
/// code. `false` for a variable that is not a declaration.
pub fn stands_unrolled(arena: &ExpArena, var: ExpId, body: ExpId) -> bool {
    let Exp::Variable { name: var_name, .. } = &arena[var].exp else { return false };
    let Exp::Identifier(name) = arena[*var_name].exp else { return false };
    dependency(arena, body, name) != Dependency::DependentConstexpr
}

/// The most nodes one `static for` may unroll into — iterations times the
/// body's size. The corpus's largest literal limit is 128 (`grep` over
/// `library/` and `test/`, 2026-09-24), a limit substituted from a template
/// parameter aside; a million nodes is far past every program the gates
/// grade, which is what the gates' unchanged verdicts check.
pub const UNROLL_NODES: u64 = 1 << 20;

/// The body's node count, stopping once past `cap`.
fn subtree_size(rw: &Rewriter, id: ExpId, cap: u64) -> u64 {
    let mut n = 0u64;
    let mut stack = vec![id];
    let mut kids = Vec::new();
    while let Some(at) = stack.pop() {
        n += 1;
        if n > cap {
            break;
        }
        kids.clear();
        rw.arena[at].exp.children(&mut kids);
        stack.extend(kids.iter().copied());
    }
    n
}

/// `eval (StaticForF a b c)` (`Constexpr.hs:48-56`) for a loop whose parts
/// are `var`, `limit` and `body`. `None` leaves the loop
/// as it is — a dependent loop whose limit is not a literal yet, or one the
/// reference would reject (a negative count, an induction variable that is
/// not a declaration).
pub(super) fn unroll(
    rw: &mut Rewriter,
    at: ExpId,
    var: ExpId,
    limit: ExpId,
    body: ExpId,
    expansions: Option<&kg_base::ExpansionInterner>,
) -> Option<Exp> {
    let Exp::Variable { scope: loop_scope, name: var_name, .. } = rw.arena[var].exp.clone() else { return None };
    let Exp::Identifier(name) = rw.arena[var_name].exp else { return None };
    // `_ -> UnrolledForF a b c`: the caller's ([`stands_unrolled`]).
    if dependency(rw.arena, body, name) != Dependency::DependentConstexpr {
        return None;
    }
    // `IntLiteral _ i <- b` (`Constexpr.hs:52`) — the limit as
    // `evaluateConstexpr` left it, so it must *be* a literal: `2*5-1` folds
    // its `2*5` in one round and the subtraction only in the next, once
    // `10` is typed (`typeResolved a`, `Constexpr.hs:172`), and the loop's
    // `const auto` variable is typed `constIndexType 9` (`Type.hs:1139-1141`)
    // in that round before the unrolling copies it — unrolled a round
    // early, every iteration's variable was typed by its own literal
    // (`data/array.k`'s `intersperse`, `uint1` where the reference has
    // `uint4`).
    let Exp::IntLiteral { value: count, .. } = &rw.arena[limit].exp else { return None };
    let count = count.clone();
    if count.is_negative() {
        return None;
    }
    let iterations = count.to_u64()?;
    // A loop bigger than any program is left standing, as a loop whose limit
    // did not fold is: silence, never a guess. The corpus's largest literal
    // limit is 128; a fuzzer's `static for (… : 65535)` with a lambda in its
    // body grew `kg check` past 10 GB in five seconds — every iteration a
    // closure, a hoisted function and a copy — where the reference itself ran
    // out its time limit (difffuzz, 2026-09-24).
    if iterations.saturating_mul(subtree_size(rw, body, UNROLL_NODES)) > UNROLL_NODES {
        return None;
    }
    // `bodyDeclScopeName`: the body block's qualified name, or the loop's
    // own scope for an unbraced body.
    let body_scope: Vec<Symbol> = match &rw.arena[body].exp {
        Exp::NestedScope { scope, name: n, .. } => {
            let Exp::Identifier(n) = rw.arena[*n].exp else { return None };
            scope.segments().iter().copied().chain([n]).collect()
        }
        _ => loop_scope.segments().to_vec(),
    };
    let (&last, outer) = body_scope.split_last()?;
    let mut induction: Vec<Symbol> = loop_scope.segments().to_vec();
    induction.push(name);

    let mut items = Vec::with_capacity(iterations as usize);
    for i in 0..iterations {
        let iter = rw.interner.intern(&format!("${}{i}", rw.interner.resolve(name)));
        let mut iter_scope = body_scope.clone();
        iter_scope.push(iter);
        let iter_q = QualifiedName::new(iter_scope.iter().copied());
        // `renote inductionVar a`: the variable, declared in the iteration's
        // scope and initialised to its index — `IntLiteralF Nothing i`
        // wearing the body's note.
        let init = rw.donate(body, Exp::IntLiteral { ty: None, value: kg_const::ConstInt::from(i) });
        // The type `inferType` gave the loop's `const auto` variable
        // (`constIndexType`, `Type.hs:1141`) rides on its note through the
        // copy; the declaration keeps its written `const auto`.
        let Exp::Variable { attr, ty, name: n, flags, .. } = rw.arena[var].exp.clone() else { unreachable!() };
        let declared = rw.replace(var, Exp::Variable { scope: iter_q.clone(), attr, ty, name: n, init: Some(init), flags });
        // Each iteration is its own copy of the body, under a frame of its
        // own ([`kg_base::ExpansionData::iteration`]): a mention inside it
        // is decided per iteration — `AndLiteral<x, N>()` is `AndLiteral<0,
        // 32>` in one and `<9, 32>` in another — and a decision is keyed by
        // where it stands, head and frame.
        let body_i = match expansions {
            Some(exps) => {
                let site = rw.arena[at].src.head;
                let instance = kg_base::InstanceId::of(&format!("static for {site:?}"), &i.to_string());
                reframe(rw, body, &mut |parent| {
                    exps.intern(kg_base::ExpansionData { site, instance, parent, iteration: true, round: 0 })
                })
            }
            None => body,
        };
        let copied = copy_iteration(rw, body_i, &iter_scope, &body_scope, &induction, iter);
        let seq = rw.donate(body, Exp::Seq(vec![declared, copied]));
        let ident = rw.donate(body, Exp::Identifier(iter));
        let body_q = QualifiedName::new(body_scope.iter().copied());
        items.push(rw.donate(body, Exp::NestedScope { scope: body_q, name: ident, body: seq }));
    }
    let seq = rw.donate(body, Exp::Seq(items));
    let ident = rw.donate(body, Exp::Identifier(last));
    Some(Exp::NestedScope { scope: QualifiedName::new(outer.iter().copied()), name: ident, body: seq })
}

/// A deep copy of `id`'s subtree, every node under the frame `frame` gives
/// its own: the same head, the same `written`, one more link of stack.
fn reframe(rw: &mut Rewriter, id: ExpId, frame: &mut dyn FnMut(kg_base::Expansion) -> kg_base::Expansion) -> ExpId {
    let node = &rw.arena[id];
    let (mut exp, src, kind) = (node.exp.clone(), node.src, node.kind);
    for slot in exp.children_mut() {
        *slot = reframe(rw, *slot, frame);
    }
    let via = frame(src.via);
    rw.arena.alloc(exp, kg_base::Src { head: src.head, written: src.written, via }, kind)
}

/// `desugar rewrite c` (`Constexpr.hs:100-121`): one iteration's copy of the
/// body, `replicateLambda` then `fixScopedName` then `modifyDeclScopeName
/// fixDeclScopeName` at every node, bottom-up.
fn copy_iteration(
    rw: &mut Rewriter,
    body: ExpId,
    iter_scope: &[Symbol],
    body_scope: &[Symbol],
    induction: &[Symbol],
    iter: Symbol,
) -> ExpId {
    let fix_decl_scope = |q: &QualifiedName| -> Option<QualifiedName> {
        let s = q.segments();
        (s.len() >= body_scope.len() && &s[..body_scope.len()] == body_scope)
            .then(|| QualifiedName::new(iter_scope.iter().chain(&s[body_scope.len()..]).copied()))
    };
    let iter_text = rw.interner.resolve(iter).to_string();
    desugar(rw, body, &mut |rw, _id, exp| {
        let exp = replicate_lambda(rw, exp, &iter_text);
        let exp = match exp {
            Exp::ScopedName { scope, prefix: Some(y), ident } => {
                let qualifier = super::get_qualifier(rw.arena, y).and_then(|mut q| {
                    q.push(super::get_name(rw.arena, y)?);
                    Some(q)
                });
                let whole = super::get_qualifier(rw.arena, y).and_then(|mut q| {
                    q.push(super::get_name(rw.arena, y)?);
                    q.push(super::get_name(rw.arena, ident)?);
                    Some(q)
                });
                if qualifier.as_deref() == Some(body_scope) || whole.as_deref() == Some(induction) {
                    let prefix = scoped_qualifier(rw, &scope, y, iter_scope);
                    Exp::ScopedName { scope, prefix, ident }
                } else {
                    Exp::ScopedName { scope, prefix: Some(y), ident }
                }
            }
            other => other,
        };
        super::modify_decl_scope_name(exp, &mut |q| fix_decl_scope(q))
    })
}

/// `replicateLambda` (`Constexpr.hs:110-119`): a lambda's own name, and every
/// scope segment spelling it, suffixed with the iteration's name inside its
/// children, so each iteration's lambda is a distinct declaration.
fn replicate_lambda(rw: &mut Rewriter, exp: Exp, iter: &str) -> Exp {
    let Exp::Lambda { scope, name, captures, params, ret, body } = exp else { return exp };
    let Some(lambda) = super::get_name(rw.arena, name) else {
        return Exp::Lambda { scope, name, captures, params, ret, body };
    };
    let renamed = rw.suffix_name(lambda, iter);
    let fix = |rw: &mut Rewriter, child: ExpId| {
        desugar(rw, child, &mut |_rw, _id, exp| {
            let exp = match exp {
                Exp::Identifier(x) if x == lambda => Exp::Identifier(renamed),
                other => other,
            };
            super::modify_decl_scope_name(exp, &mut |q| {
                q.segments()
                    .contains(&lambda)
                    .then(|| QualifiedName::new(q.segments().iter().map(|&s| if s == lambda { renamed } else { s })))
            })
        })
    };
    let name = fix(rw, name);
    let captures = fix(rw, captures);
    let params = fix(rw, params);
    let ret = fix(rw, ret);
    let body = fix(rw, body);
    Exp::Lambda { scope, name, captures, params, ret, body }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::ast;
    use kg_syntax::AstNode;

    use crate::rewrite::{desugar, static_control_node, Rewriter};
    use crate::{lower_sorted, pretty, Exp, ModuleIr, NoImports};

    /// `staticControl` at every node, bottom-up, as the loop's inference
    /// applies it ([`static_control_node`]) — without its postponements,
    /// which these fixtures do not reach.
    fn control(before: &ModuleIr, interner: &Interner, exps: Option<&kg_base::ExpansionInterner>) -> ModuleIr {
        let mut out = before.fork();
        let root = out.root;
        let mut rw = Rewriter::new(&mut out.arena, interner);
        out.root = desugar(&mut rw, root, &mut |rw, id, exp| static_control_node(rw, id, exp, exps));
        out
    }

    fn run(text: &str) -> (ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "probe must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let before = lower_sorted(&file, FileId(0), &interner, &NoImports);
        let after = control(&before, &interner, None);
        (after, interner)
    }

    /// A `static for` past [`super::UNROLL_NODES`] stays a loop, silence
    /// rather than a million-node body every later pass pays for, while one
    /// its size allows unrolls. The fuzzer's `static for (… : 65535)` with a
    /// lambda in its body grew `kg check` past 10 GB in five seconds
    /// (2026-09-24). Red with the bound removed: 65535 nested scopes.
    #[test]
    fn a_static_for_past_the_unroll_bound_stays_a_loop() {
        let big = "inline void main()\n{\n    static for (const auto i : 65535)\n    {\n        uint8[i + 1] a;\n    }\n}\n";
        let (ir, interner) = run(big);
        assert!(pretty(&ir, &interner).contains("static for"), "{}", pretty(&ir, &interner));
        let small = "inline void main()\n{\n    static for (const auto i : 4)\n    {\n        uint8[i + 1] a;\n    }\n}\n";
        let (ir, interner) = run(small);
        assert!(!pretty(&ir, &interner).contains("static for"), "{}", pretty(&ir, &interner));
    }

    /// `Constexpr.hs:48-121`: a loop whose variable a `static assert` reads
    /// is unrolled into one scope per iteration under the body's name, the
    /// variable declared there as the index (its `const auto` kept: the type
    /// `constIndexType` gave it rides on the note) and the body copied with
    /// its scopes re-rooted — the name the assertion reads now resolves to
    /// the iteration's declaration. A loop whose variable reaches no
    /// constant position is an `UnrolledFor`, printed as written.
    #[test]
    fn a_loop_whose_variable_reaches_a_constant_is_unrolled_per_iteration() {
        let (ir, interner) = run("inline void f()\n{\n    static for (const auto x : 2)\n    {\n        static assert(x < 2);\n    }\n    static for (const auto y : 2)\n    {\n        uint8 z = y;\n    }\n}\n");
        assert_eq!(
            pretty(&ir, &interner),
            "inline void f() {\n{\n{\nconst auto x = 0;\n{\nstatic assert(x < 2);\n}\n}\n{\nconst auto x = 1;\n{\nstatic assert(x < 2);\n}\n}\n}\nstatic for (const auto y : 2) {\nuint8 z = y;\n}\n}\n\n"
        );
        let spell = |q: &kg_ty::QualifiedName| {
            q.segments().iter().map(|s| interner.resolve(*s).to_string()).collect::<Vec<_>>().join("::")
        };
        let mut declared = Vec::new();
        let mut read = Vec::new();
        let mut unrolled = 0;
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            match &ir.arena[id].exp {
                Exp::UnrolledFor { .. } => unrolled += 1,
                Exp::Variable { scope, .. } => declared.push(spell(scope)),
                Exp::ScopedName { scope, ident, .. }
                    if matches!(ir.arena[*ident].exp, Exp::Identifier(s) if interner.resolve(s) == "x") =>
                {
                    read.push(spell(scope));
                }
                _ => {}
            }
            kids.clear();
            ir.arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().rev());
        }
        assert_eq!(
            declared,
            ["f::$::$quasi0::$quasi1::$x0", "f::$::$quasi0::$quasi1::$x1", "f::$::$quasi1::$quasi0", "f::$::$quasi1::$quasi0::$quasi1"]
        );
        // `fixDeclScopeName` re-roots every scope the body's own prefixes: a
        // read in the body block is in the iteration's scope, where the
        // iteration declares the variable.
        assert_eq!(read, ["f::$::$quasi0::$quasi1::$x0", "f::$::$quasi0::$quasi1::$x1"]);
        assert_eq!(unrolled, 1, "the loop over `y` is an `UnrolledFor`");
    }

    /// Each iteration is its own copy of the body under a frame of its own:
    /// the two reads of `x` stand at one written head, and a decision keyed
    /// by head alone would be one decision for both — `AndLiteral<x, N>()`
    /// is a different instance in every iteration (`literal-operand.k`).
    /// The frame is no instance: the reference notes nothing for an
    /// iteration, so no `while compiling` comes of it.
    #[test]
    fn each_iteration_stands_under_a_frame_of_its_own() {
        let interner = Interner::new();
        let text = "inline void f()\n{\n    static for (const auto x : 2)\n    {\n        static assert(x < 2);\n    }\n}\n";
        let parse = kg_syntax::parse_file(text);
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let before = lower_sorted(&file, FileId(0), &interner, &NoImports);
        let exps = kg_base::ExpansionInterner::new();
        let ir = control(&before, &interner, Some(&exps));
        let mut framed = Vec::new();
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let node = &ir.arena[id];
            if let Exp::ScopedName { ident, .. } = &node.exp
                && matches!(ir.arena[*ident].exp, Exp::Identifier(s) if interner.resolve(s) == "x")
            {
                framed.push(node.src);
            }
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().rev());
        }
        assert_eq!(framed.len(), 2);
        assert_eq!(framed[0].head, framed[1].head);
        assert_ne!(framed[0].via, framed[1].via);
        for src in &framed {
            assert!(exps.frames(src.via).iter().all(|f| f.iteration));
            assert!(exps.copy_frames(src.via).is_empty());
        }
    }

    /// Every declaration named `leaf` still reachable, by its scope.
    fn scopes_of(ir: &ModuleIr, interner: &Interner, leaf: &str) -> Vec<Vec<String>> {
        scopes_under(ir, ir.root, interner, leaf)
    }

    /// [`scopes_of`] below `root`.
    fn scopes_under(ir: &ModuleIr, root: crate::ExpId, interner: &Interner, leaf: &str) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let node = &ir.arena[id];
            if let Exp::Function { scope, name, .. } | Exp::Variable { scope, name, .. } = &node.exp
                && matches!(ir.arena[*name].exp, Exp::Identifier(s) if interner.resolve(s) == leaf)
            {
                out.push(scope.segments().iter().map(|s| interner.resolve(*s).to_string()).collect());
            }
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().rev());
        }
        out
    }

    /// `removeNestedScope` (`Constexpr.hs:358-370`): the declarations of a
    /// decided branch are keyed where the branch stood, its `$quasiN`
    /// segment gone — so `lookupSymbol` finds them from the enclosing
    /// scope. Red with the branch's body spliced as it is.
    #[test]
    fn a_decided_branch_leaves_its_scope_behind() {
        let (ir, interner) = run("inline void f()\n{\n    static if (true)\n    {\n        uint8 a;\n    }\n    else\n    {\n        uint8 b;\n    }\n}\n");
        assert_eq!(scopes_of(&ir, &interner, "a"), vec![vec!["f".to_owned(), "$".to_owned()]]);
        assert!(scopes_of(&ir, &interner, "b").is_empty());
    }
}
