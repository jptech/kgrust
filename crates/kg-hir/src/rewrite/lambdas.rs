//! `desugarLambdas` (`Function.hs:189-262`) — inner sub-pass **2** of
//! `instantiateTemplates` in execution order (`Template.hs:71-81` is a
//! `foldr ($)` list, so it is written in *reverse* execution order), and the
//! third rewrite of this port to land. It runs bare — no symbol map, no type
//! table (`Template.hs:78` wraps it in neither `withSymbols` nor
//! `withSymbols'`); every name it emits is derived from the lambda node itself
//! or is a fixed library constant.
//!
//! For each written lambda it makes three things:
//!
//! ```text
//! using ?lambda0$capture = decltype(@data@closure::make_capture2(p, q));   -- alias
//! inline uint8 ?lambda0(?lambda0$capture ?lambda0$capture_param, uint8 a)  -- function
//! {
//!     auto p = ?lambda0$capture_param.x0;                                  -- prologue
//!     auto q = ?lambda0$capture_param.x1;
//!     …original body…
//! }
//! …cast<Closure<|decltype(f::$::?lambda0), f::$::?lambda0,                 -- use site
//!                f::$::?lambda0$capture|>>(make_capture2(p, q))…
//! ```
//!
//! The alias and the function are *prepended* to the enclosing target's `Seq`
//! and the `Cast` *replaces* the lambda where it stood. The `?lambdaN` name is
//! minted by the parser (`Parser.hs:434`; here `lower.rs:1760`), never by this
//! pass — it only reads `declName`.
//!
//! # Where the declarations land, which is not "the nearest block"
//!
//! `lambdaDefs` (`Function.hs:229-235`) is a `para` that stops at `Function`,
//! `StaticIf`, `StaticFor` and (collecting, without descending) `Lambda` — and
//! at nothing else. So a lambda written inside an `if` block is collected by
//! the enclosing *function's* clause and its declarations land in the function
//! body's `Seq`, while their `declScopeName` still says `$quasi0` — the two
//! disagree on purpose. The `expand` targets are exactly: a `Function` body
//! (`:192`), a `Lambda` body (`:193`, nested lambdas, bottom-up), each branch
//! `NestedScope` body of a `StaticIf` (`:196-201` — necessary because
//! `lambdaDefs` refuses to cross a `StaticIf`), and a `DeclareGlobal`
//! variable, which is wrapped in a transient one-element `Seq`, rewritten, and
//! spliced flat by the parent `Seq`'s `removeNestedSeq` (`:194-195`, `:202`).
//!
//! A lambda under a *surviving* `static for` is left entirely alone — not
//! collected (`:234`), not replaced (`:219`) — because the reference desugars
//! it only after the loop unrolls during template instantiation. On plain
//! constant bounds `staticControl` has already turned the loop into
//! `UnrolledFor` by pass 8 (which stops nothing), so the surviving case is a
//! bound that needs a template parameter; measured on the corpus, 20 lambdas
//! sit under a `static for`, across 6 files. The protection is *shallow*,
//! which nothing in the Haskell says locally: the cata still visits a lambda
//! inside the loop and clause 2 still rewrites its **body**, so a lambda
//! nested inside it is desugared — into the surviving lambda's body — and
//! only the outer one is left standing, bare, with no alias and no function.
//! Measured against the reference on `library/data/vector.k`: of its three
//! static-for lambdas one is nested inside another, and exactly the two outer
//! ones survive at `--template-passes 2` (5 survivors across all of
//! `library/`, 3 in `data/hash_table.k`, 2 in `data/vector.k`).
//!
//! # `make_captureN` is a real library symbol, and `N` is the arity
//!
//! `Function.hs:150-154`: `N = length lambdaCaptures` — the *arity*, not the
//! lambda number — and the module flips on it: arity 0 names
//! `@data@closure@core::make_capture0` (`library/data/closure/core.k:6`),
//! arity ≥ 1 names `@data@closure::make_captureN`
//! (`library/data/closure.k:6-26`, which stops at `make_capture21`). The
//! corpus saturates that ceiling exactly — one 21-capture closure, in
//! `test/syntax/closures.k`, which has no in-class lambdas, so no implicit
//! `this` pushes it to a nonexistent `make_capture22`. A 22nd capture would be
//! an undefined-symbol error in the reference too, so there is no cliff to
//! implement. `captureThis` (frontend pass 3) has already prepended any
//! implicit `This` by the time this pass runs, which is why it is capture 0
//! and why its local is named `$this` (`getName ThisF`,
//! `Parser/Syntax.hs:739,267-268`).
//!
//! # What `FunctionKind::LambdaFunction` is for
//!
//! Exactly one reader exists in the reference: `lambdaFunctions`
//! (`Function.hs:126-127`), which `moveFunctions` uses at `Function.hs:101` to
//! hoist lambda bodies out of a `class` so a lambda written in a member
//! function does not become a member. No type rule, mangler or backend
//! selector reads it. This pass is its only constructor, here as there.
//!
//! # Sharing in the reference, copies here
//!
//! The Haskell shares one binding across positions freely; an arena cannot
//! without a DAG (`arena.rs`, `deep_copy`). Each shared occurrence beyond the
//! first is a [`Rewriter::deep_copy`] (or an equal fresh donation), and the
//! choice of which position keeps the original id follows one rule: *the node
//! standing where text still stands keeps it*. Concretely: the synthesized
//! function keeps the lambda's `declName` and `funcReturnType` nodes
//! (`Function.hs:248` reuses both), and the `Cast`'s two `?lambdaN` leaves are
//! copies; the use-site `make_captureN` call keeps the written capture-name
//! and `This` nodes (`arg x@This{} = x`, `:156`) and its argument `Seq` keeps
//! `Own` at the capture list's key, while the alias's structurally identical
//! call — the reference calls `makeCapture` twice, once noted `L` at `:244`
//! and once noted by the capture list at `:161` — carries copies and is
//! `Donated` throughout.
//!
//! # Testing donors, not offsets
//!
//! Our `Src` for the lambda's `declName` and its capture-list `Seq` is the CST
//! node's range where the reference holds zero-width points (divergence #26),
//! so every expectation below asserts donor *identity* — "this leaf wears the
//! same `Src` as the `declName` node" — never a raw offset. One consequence
//! bites the tests themselves: `declName` is synthesized off the whole
//! `LAMBDA_EXPR`, so the `NAME` and `L` donors *coincide* in this IR, and a
//! swap between exactly those two is unpinnable until the ranges split.

use kg_base::Symbol;
use kg_resolve::DeclareFlag;
use kg_ty::{FunctionKind, QualifiedName, Ty};

use crate::arena::{ExpArena, ExpId};
use crate::exp::{Exp, Modifier};
use crate::facts::{Flags, LAMBDA, NESTED_SEQ};
use crate::rewrite::{
    desugar_shallow, desugar_within, local_identifier, scoped_qualifier, Rewriter, ShallowRewrite,
};

/// `data'closure` / `data'closure'core` (`Type.hs:115-119`), pre-mangled by
/// `moduleNamespace = fromString . concatMap ('@':)` (`Parser/Syntax.hs:273-274`)
/// — one segment each, not a path.
const DATA_CLOSURE: &str = "@data@closure";
const DATA_CLOSURE_CORE: &str = "@data@closure@core";

/// `desugarLambdas = desugar' expand` (`Function.hs:190`) over the subtree at
/// `root`, returning its new id.
///
/// `expand` hands back every node of a subtree holding no lambda and no
/// `Seq` directly in a `Seq` as it came: a function's, a global variable's
/// and a `static if` branch's clauses find no lambda to hoist or refer to,
/// and `removeNestedSeq` nothing to splice.
pub fn apply(rw: &mut Rewriter, root: ExpId) -> ExpId {
    const NEEDS: Flags = LAMBDA | NESTED_SEQ;
    desugar_within(rw, root, NEEDS, &mut |rw, id, exp| expand(rw, id, exp))
}

/// `expand` (`Function.hs:192-202`). `exp` arrives with its children already
/// rewritten — the cata is bottom-up, so a nested lambda's own body was
/// finished before its enclosing function's clause runs.
fn expand(rw: &mut Rewriter, id: ExpId, exp: Exp) -> Exp {
    match exp {
        // `Function.hs:192`
        Exp::Function { scope, kind, attr, modifier, ret, name, params, body } => {
            let body = lambda_defs_and_refs(rw, body);
            Exp::Function { scope, kind, attr, modifier, ret, name, params, body }
        }
        // `Function.hs:193`
        Exp::Lambda { scope, name, captures, params, ret, body } => {
            let body = lambda_defs_and_refs(rw, body);
            Exp::Lambda { scope, name, captures, params, ret, body }
        }
        // `Function.hs:194-195` — module-scope variables only
        // (`globalVariable = variable DeclareGlobal`, `Parser.hs:461`). The
        // reference wraps *every* global variable in a transient `SeqF` and
        // unwraps; doing that for the lambda-free majority would append a
        // garbage node per variable, so the wrap happens only when there is a
        // lambda to move — observationally identical, the transient is spliced
        // away by the parent `Seq`'s `removeNestedSeq` either way.
        e @ Exp::Variable { flags: DeclareFlag::DeclareGlobal, .. } => {
            let mut kids = Vec::new();
            e.children(&mut kids);
            if !kids.iter().any(|&k| has_lambda(rw.arena, k)) {
                return e;
            }
            let var = if e == rw.arena[id].exp { id } else { rw.replace(id, e) };
            let wrapper = rw.donate(id, Exp::Seq(vec![var]));
            let out = lambda_defs_and_refs(rw, wrapper);
            // `unNote $ unfix` — hand back the `SeqF`'s expression; the driver
            // re-notes it at the variable's own note, and the parent splices.
            rw.arena[out].exp.clone()
        }
        // `Function.hs:196-201` — each branch's `NestedScope` body, in place,
        // because `lambdaDefs` will not cross the `StaticIf` boundary.
        Exp::StaticIf { cond, then_branch, else_branch } => Exp::StaticIf {
            cond,
            then_branch: static_if_branch(rw, then_branch),
            else_branch: else_branch.map(|b| static_if_branch(rw, b)),
        },
        // `Function.hs:202`
        other => rw.remove_nested_seq(other),
    }
}

/// `go` (`Function.hs:200-201`): a `NestedScope` branch gets its body
/// rewritten under its own note (`renote go`); anything else is handed back.
fn static_if_branch(rw: &mut Rewriter, branch: ExpId) -> ExpId {
    let Exp::NestedScope { scope, name, body } = rw.arena[branch].exp.clone() else {
        return branch;
    };
    let new_body = lambda_defs_and_refs(rw, body);
    if new_body == body {
        return branch;
    }
    rw.replace(branch, Exp::NestedScope { scope, name, body: new_body })
}

/// `lambdaDefsAndRefs e = desugarShallow lambdaRef $ foldr cons e $
/// lambdaDefs e` (`Function.hs:204`). The `foldr cons` prepends in source
/// order — `[alias₀, fn₀, alias₁, fn₁, …]` — and `cons` is `renote`, so the
/// block `Seq` keeps its own `Src`.
fn lambda_defs_and_refs(rw: &mut Rewriter, block: ExpId) -> ExpId {
    // No lambda below: nothing for `lambdaDefs` to collect, nor for
    // `lambdaRef` to refer to.
    if rw.arena.lacks(block, LAMBDA) {
        return block;
    }
    let mut lambdas = Vec::new();
    collect(rw.arena, block, &mut lambdas);
    let block = if lambdas.is_empty() {
        block
    } else {
        let mut items = Vec::with_capacity(lambdas.len() * 2 + 4);
        for &l in &lambdas {
            let (alias, func) = lambda_def(rw, l);
            items.push(alias);
            items.push(func);
        }
        let Exp::Seq(rest) = rw.arena[block].exp.clone() else {
            unreachable!("every expand target hands a `Seq` body (Parser.hs:467-468)")
        };
        items.extend(rest);
        rw.replace(block, Exp::Seq(items))
    };
    desugar_shallow(rw, block, &mut LambdaRef)
}

/// `para lambda` (`Function.hs:229-235`): collect every lambda reachable
/// without crossing a `Function`, `StaticIf` or `StaticFor`, in source order,
/// stopping *at* a `Lambda` without descending — a lambda nested inside
/// another lambda's body was already handled by the bottom-up `expand`.
fn collect(arena: &ExpArena, id: ExpId, out: &mut Vec<ExpId>) {
    match &arena[id].exp {
        // A lambda whose `declName` is not an `Identifier` cannot be given
        // its `$capture` names. The lowering always mints one
        // (`lower.rs:1760-1761`); on the malformed trees where it cannot,
        // decline the lambda — [`LambdaRef`] declines the same node, so the
        // two halves stay consistent. The reference would crash instead
        // (`indetifierAppend _ _ = undefined`, `Desugar.hs:203-204`).
        Exp::Lambda { name, .. } => {
            if matches!(arena[*name].exp, Exp::Identifier(_)) {
                out.push(id);
            }
        }
        Exp::Function { .. } | Exp::StaticIf { .. } | Exp::StaticFor { .. } => {}
        e => {
            let mut kids = Vec::new();
            e.children(&mut kids);
            for k in kids {
                collect(arena, k, out);
            }
        }
    }
}

/// Whether [`collect`] starting at `id` would find anything — the guard for
/// the global-variable clause's transient wrapper.
fn has_lambda(arena: &ExpArena, id: ExpId) -> bool {
    let mut found = Vec::new();
    collect(arena, id, &mut found);
    !found.is_empty()
}

/// Which of the two `makeCapture` call sites is being built. The reference
/// evaluates `makeCapture l` twice — `renote makeCapture l` at
/// `Function.hs:244` and `new capture` at `:161` — and the two results share
/// the written capture-name and `This` nodes. The cast stands at the written
/// text, so it keeps the originals (and its argument `Seq` keeps `Own` at the
/// capture list's key); the alias carries `Donated` copies.
#[derive(Copy, Clone, PartialEq, Eq)]
enum CallSite {
    AliasDecl,
    CastValue,
}

/// `makeCapture` (`Function.hs:145-159`): the `make_captureN(…)` call. Every
/// frame node is donated by the capture-list `Seq` (`new = flip newNote
/// lambdaCaptures`), each argument by its own capture node; the call head's
/// donor is the caller's — `l` for the alias, the capture list for the cast.
fn make_capture(rw: &mut Rewriter, l: ExpId, site: CallSite) -> ExpId {
    let Exp::Lambda { scope, captures, .. } = rw.arena[l].exp.clone() else {
        unreachable!("checked by collect")
    };
    let Exp::Seq(caps) = rw.arena[captures].exp.clone() else {
        unreachable!("a lambda's capture list is always a `Seq` (`lower.rs:1783-1786`)")
    };

    // `:152-154` — module flips on `null lambdaCaptures`, `N` is the arity.
    let module = if caps.is_empty() { DATA_CLOSURE_CORE } else { DATA_CLOSURE };
    let names = [
        rw.interner.intern(module),
        rw.interner.intern(&format!("make_capture{}", caps.len())),
    ];
    let ident = qualified_identifier(rw, &scope, &names, captures);
    let spec = rw.donate(captures, Exp::FunctionSpecifier { receiver: None, function: ident });
    let attr = rw.donate(captures, Exp::Seq(Vec::new()));

    let args: Vec<ExpId> = caps
        .iter()
        .map(|&x| match rw.arena[x].exp.clone() {
            // `arg x@This{} = x` (`:156`) — passed through, its own note kept.
            Exp::This { .. } => match site {
                CallSite::CastValue => x,
                CallSite::AliasDecl => rw.deep_copy(x),
            },
            // `arg x@(Capture _ a) = newNote (NamedValueF (localIdentifier
            // declScopeName a)) x` (`:157`) — the written name leaf is reused.
            Exp::Capture { name, .. } => {
                let leaf = match site {
                    CallSite::CastValue => name,
                    CallSite::AliasDecl => rw.deep_copy(name),
                };
                let value = local_identifier(rw, &scope, leaf);
                rw.donate(x, Exp::NamedValue(value))
            }
            // The reference's `arg _ = undefined` — unreachable through the
            // grammar; on a malformed tree carry the node rather than panic.
            _ => x,
        })
        .collect();

    // `renote (fmap arg) lambdaCaptures` (`:146`) keeps the capture list's
    // `Src`. Only the cast's copy stands in for the written list, so only it
    // keeps `Own`; the alias's is an equal-ranged `Donated` twin. When the
    // mapping changed nothing — arity 0, or a `this`-only list whose one node
    // passes through — the cast reuses the written list's id outright: an
    // `Own` *replacement* that equals the node it replaces is
    // indistinguishable from a copy, which provenance clause four forbids
    // (`abbreviated::own_nodes_are_not_copies`), and the substrate's economy
    // rule (`rewrite/mod.rs`) gives the same answer.
    let arglist = match site {
        CallSite::CastValue if args == caps => captures,
        CallSite::CastValue => rw.replace(captures, Exp::Seq(args)),
        CallSite::AliasDecl => rw.donate(captures, Exp::Seq(args)),
    };
    let head_donor = match site {
        // `renote makeCapture l` (`:244`) — the call wears the lambda's note.
        CallSite::AliasDecl => l,
        // `new capture` (`:161`) — the call wears the capture list's note.
        CallSite::CastValue => captures,
    };
    rw.donate(head_donor, Exp::FunctionCall { attr, function: spec, args: arglist })
}

/// `captureTypeIdent scope l` (`Function.hs:224-226`): the qualified
/// `…::?lambdaN$capture` type. The outer `TypeIdentifier` and `ScopedName`
/// wear `outer_donor`'s note (`newNote scopedName l`); the qualifier chain and
/// the leaf wear the *leaf's* — `addQualifierFrom` resolves to `setQualifier`
/// whose `scopedQualifier n b qualifier` donates from `b`, the appended
/// identifier (`Desugar.hs:182-185,196-198`). That asymmetry is the bug a port
/// writes by accident, and the donor tests pin it.
///
/// `qualifier` is `getQualifier l` — a lambda has no arm of its own, so the
/// catch-all answers `declScopeName` (`Parser/Syntax.hs:764`).
fn capture_type_ident(
    rw: &mut Rewriter,
    ty_scope: &QualifiedName,
    outer_donor: ExpId,
    qualifier: &QualifiedName,
    name: ExpId,
    capture_ty_sym: Symbol,
) -> ExpId {
    let leaf = rw.donate(name, Exp::Identifier(capture_ty_sym));
    let prefix = scoped_qualifier(rw, ty_scope, leaf, qualifier.segments());
    let sn = rw.donate(
        outer_donor,
        Exp::ScopedName { scope: ty_scope.clone(), prefix, ident: leaf },
    );
    rw.donate(outer_donor, Exp::TypeIdentifier(ty_scope.clone(), sn))
}

/// `qualifiedIdentifier` (`Type.hs:997-998`): a `QualifiedIdentifier` around a
/// [`scoped_qualifier`] chain, everything donated by `donor`. Still private
/// here — `scoped_qualifier` and `local_identifier` were lifted to
/// `rewrite/mod.rs` when `move_functions` became their second caller, and this
/// one still has only this caller.
fn qualified_identifier(
    rw: &mut Rewriter,
    scope: &QualifiedName,
    names: &[Symbol],
    donor: ExpId,
) -> ExpId {
    let chain = scoped_qualifier(rw, scope, donor, names)
        .expect("qualifiedIdentifier is never called with an empty path");
    rw.donate(donor, Exp::QualifiedIdentifier(chain))
}

/// `lambdaDef` (`Function.hs:237-262`): the capture-type alias and the lambda
/// function, in that order. Donor tags in the comments name the source lambda's
/// fields: `L` the whole lambda, `NAME` its `declName`, `CAPS` its capture
/// list, `Cᵢ` capture *i*.
fn lambda_def(rw: &mut Rewriter, l: ExpId) -> (ExpId, ExpId) {
    let Exp::Lambda { scope, name, captures, params, ret, body } = rw.arena[l].exp.clone() else {
        unreachable!("checked by collect")
    };
    let Exp::Identifier(name_sym) = rw.arena[name].exp else {
        unreachable!("checked by collect")
    };
    let capture_ty_sym = rw.suffix_name(name_sym, "$capture");

    // --- captureTypeAlias (`:242-244`) ---
    // `captureTypeName l` (`:222`) — `indetifierAppend` copyNotes from NAME.
    let alias_name = rw.donate(name, Exp::Identifier(capture_ty_sym));
    let call = make_capture(rw, l, CallSite::AliasDecl);
    let decl = rw.donate(l, Exp::Decltype { scope: scope.clone(), expr: call });
    let alias = rw.donate(l, Exp::Alias { scope: scope.clone(), body: decl, name: alias_name });

    // --- lambdaFunction (`:246-262`) ---
    // `attr = emptySeq` (`:239-240`) is one shared binding in the reference,
    // used for the function, the capture parameter and every prologue
    // variable; equal fresh donations stand in for the shares.
    let attr = rw.donate(l, Exp::Seq(Vec::new()));
    let inline = rw.donate(l, Exp::FunctionModifier(Modifier::Inline));
    let sig_scope = scope.child(name_sym);
    let body_scope = sig_scope.child(rw.interner.intern(kg_resolve::names::FUNCTION_SCOPE));
    let cap_param_sym = rw.suffix_name(name_sym, "$capture_param");
    let cap_param_name = rw.donate(name, Exp::Identifier(cap_param_sym));

    // `captureType = newNote (captureTypeIdent signatureDeclScopeName l) l`
    // (`:255`) — outer nodes L, chain and leaf NAME.
    let cap_ty = capture_type_ident(rw, &sig_scope, l, &scope, name, capture_ty_sym);
    let param_attr = rw.donate(l, Exp::Seq(Vec::new()));
    let cap_param = rw.donate(
        l,
        Exp::FuncParam { scope: sig_scope, attr: param_attr, ty: cap_ty, name: cap_param_name },
    );

    // `captureParam \`cons\` funcParams` (`:248`) — prepend, params `Seq`
    // keeps its own `Src`.
    let Exp::Seq(ps) = rw.arena[params].exp.clone() else {
        unreachable!("a lambda's parameter list is always a `Seq`")
    };
    let mut new_ps = Vec::with_capacity(ps.len() + 1);
    new_ps.push(cap_param);
    new_ps.extend(ps);
    let new_params = rw.replace(params, Exp::Seq(new_ps));

    // `lambdaBody = foldr (cons . flip newNote l . uncaptureVar) body $
    // zip [0..] captures` (`:256-262`) — one local per capture, in order, on
    // top of the (already rewritten) body; the body `Seq` keeps its own `Src`.
    let Exp::Seq(caps) = rw.arena[captures].exp.clone() else {
        unreachable!("a lambda's capture list is always a `Seq` (`lower.rs:1783-1786`)")
    };
    let new_body = if caps.is_empty() {
        body
    } else {
        let Exp::Seq(stmts) = rw.arena[body].exp.clone() else {
            unreachable!("a lambda's body is always a `Seq` (`lower.rs:1801-1804`)")
        };
        let mut items = Vec::with_capacity(caps.len() + stmts.len());
        for (i, &x) in caps.iter().enumerate() {
            items.push(uncapture_var(rw, l, name, &body_scope, cap_param_sym, i, x));
        }
        items.extend(stmts);
        rw.replace(body, Exp::Seq(items))
    };

    // `:247-248`. `funcReturnType` and `declName` are the lambda's own nodes,
    // reused — the lambda itself is replaced by the cast in the same pass, so
    // this is where the written text's nodes live on.
    let func = rw.donate(
        l,
        Exp::Function {
            scope,
            kind: FunctionKind::LambdaFunction,
            attr,
            modifier: Some(inline),
            ret,
            name,
            params: new_params,
            body: new_body,
        },
    );
    (alias, func)
}

/// `uncaptureVar` (`Function.hs:256-262`): `auto <name> =
/// ?lambdaN$capture_param.xI;`. The variable wears `L`; its type, name, member
/// access, field and `NamedValue` wear capture *i*'s note; the
/// `?lambdaN$capture_param` leaf and its `localIdentifier` wrappers wear
/// `NAME` — three donors in one statement, pinned by
/// [`tests::the_uncapture_initializer_mixes_three_donors`].
fn uncapture_var(
    rw: &mut Rewriter,
    l: ExpId,
    name: ExpId,
    body_scope: &QualifiedName,
    cap_param_sym: Symbol,
    index: usize,
    x: ExpId,
) -> ExpId {
    let var_name_sym = match &rw.arena[x].exp {
        // `getName ThisF{} = thisName` (`Parser/Syntax.hs:739,267-268`).
        Exp::This { .. } => rw.interner.intern(kg_resolve::names::THIS),
        Exp::Capture { name, .. } => match rw.arena[*name].exp {
            Exp::Identifier(s) => s,
            // Error recovery only — the reference's `getName` would crash.
            _ => rw.interner.intern(""),
        },
        _ => rw.interner.intern(""),
    };
    let var_attr = rw.donate(l, Exp::Seq(Vec::new()));
    let auto = rw.donate(x, Exp::Auto);
    let var_name = rw.donate(x, Exp::Identifier(var_name_sym));
    // The reference shares one `captureParamName` node between the parameter
    // and every prologue local (`:253`, `:262`); each local gets a fresh
    // equal-noted leaf here.
    let leaf = rw.donate(name, Exp::Identifier(cap_param_sym));
    let value = local_identifier(rw, body_scope, leaf);
    let named = rw.donate(x, Exp::NamedValue(value));
    let field_sym = rw.interner.intern(&format!("x{index}"));
    let field = rw.donate(x, Exp::Identifier(field_sym));
    let access = rw.donate(x, Exp::MemberAccess { base: named, member: field });
    rw.donate(
        l,
        Exp::Variable {
            scope: body_scope.clone(),
            attr: var_attr,
            ty: auto,
            name: var_name,
            init: Some(access),
            flags: DeclareFlag::DeclareLocal,
        },
    )
}

/// `lambdaRef` (`Function.hs:206-220`) as the shallow walk over one rewritten
/// block: replace each lambda with its `Closure` cast, decline at the
/// boundaries whose own `expand` clause already ran (or deliberately never
/// runs — `static for`).
#[derive(Debug, Default)]
pub struct LambdaRef;

impl ShallowRewrite for LambdaRef {
    /// `lambdaRef FunctionF{} = Nothing; lambdaRef StaticForF{} = Nothing`
    /// (`Function.hs:218-219`). The `StaticFor` arm is load-bearing — it is
    /// half of what keeps a `static for` lambda alive. The `Function` arm is
    /// transcription plus defense, and — measured, not argued — *unobservable
    /// on parseable input*: by the time an outer walk reaches a nested
    /// function, the bottom-up cata has already replaced every bare lambda in
    /// its body, and every non-body position a lambda could hide in (return
    /// `decltype`, parameter `decltype`, attribute argument) is a
    /// `PARSE-ERROR` in the reference's grammar. Deleting it — or `lambdaDefs`'
    /// twin stop at `Function.hs:232` — turns no test and no gate red, so
    /// nothing pins either; they stay because the reference wrote them. There
    /// is no `StaticIf` arm: clause 4 rewrote the branches and left nothing
    /// for the walk to find.
    fn reverts(&self, arena: &ExpArena, id: ExpId) -> bool {
        matches!(arena[id].exp, Exp::Function { .. } | Exp::StaticFor { .. })
    }

    fn rewrite(&mut self, rw: &mut Rewriter, id: ExpId, exp: Exp) -> Exp {
        let Exp::Lambda { name, .. } = &exp else {
            // `lambdaRef e = Just e` (`:220`) — descend through everything
            // else, `If` and `NestedScope` blocks included.
            return exp;
        };
        if !matches!(rw.arena[*name].exp, Exp::Identifier(_)) {
            // The same malformed-name guard as `collect`, kept in lockstep.
            return exp;
        }
        // `exp` and the node at `id` agree on every field read below: the
        // walk rewrites only lambdas, and none is left inside this one — its
        // body was finished by the bottom-up `expand` before the enclosing
        // walk began, and a `static for` inside it reverts.
        make_closure(rw, id, &exp)
    }

    /// Only a lambda is rewritten.
    fn needs(&self) -> Option<Flags> {
        Some(LAMBDA)
    }
}

/// `isGenericFunction` (`Function.hs:186-187`): any parameter written bare
/// `auto`. `isAuto` looks through nothing — `const auto` is `ConstF AutoF` and
/// does not count (`Parser/Syntax.hs:706-707`).
fn is_generic(arena: &ExpArena, params: ExpId) -> bool {
    let Exp::Seq(ps) = &arena[params].exp else { return false };
    ps.iter().any(|&p| match &arena[p].exp {
        Exp::FuncParam { ty, .. } => matches!(arena[*ty].exp, Exp::Auto),
        _ => false,
    })
}

/// `makeClosure` (`Function.hs:161-171`) plus `lambdaRef`'s argument
/// assembly (`:206-216`): the `Cast` that replaces the lambda. The `Cast`
/// itself is allocated by the driver at the lambda's own note; everything
/// under it is donated by the capture list except the leaves the donor table
/// sends elsewhere.
fn make_closure(rw: &mut Rewriter, l: ExpId, exp: &Exp) -> Exp {
    let Exp::Lambda { scope, name, captures, params, ret, .. } = exp.clone() else {
        unreachable!("checked by the caller")
    };
    let Exp::Identifier(name_sym) = rw.arena[name].exp else {
        unreachable!("checked by the caller")
    };
    let capture_ty_sym = rw.suffix_name(name_sym, "$capture");

    // Argument 1 (`FunctionType`): `fromMaybe (decltype lambda) lambdaType` —
    // a `decltype` of the function's name, or, for a generic lambda, a written
    // `FunctionType` so the closure's type does not name `auto` through a
    // `decltype` that cannot resolve yet (`:207,212-216`).
    let arg1 = if is_generic(rw.arena, params) {
        let attr = rw.donate(captures, Exp::Seq(Vec::new()));
        // `captureParam = FunctionTypeParamF (newSeq []) (new captureType)
        // Nothing` (`:215`) — the same `captureTypeIdent` value as argument 3
        // in the reference, built a second time here (no DAG).
        let cap_ty = capture_type_ident(rw, &scope, captures, &scope, name, capture_ty_sym);
        let ftp_attr = rw.donate(captures, Exp::Seq(Vec::new()));
        let mut ftps =
            vec![rw.donate(captures, Exp::FunctionTypeParam { attr: ftp_attr, ty: cap_ty, name: None })];
        let Exp::Seq(ps) = rw.arena[params].exp.clone() else { unreachable!() };
        for p in ps {
            // `functionTypeParam p` (`:216`) reuses the parameter's attr and
            // type nodes; the originals stay with the synthesized function's
            // parameters, so the type gets copies.
            let Exp::FuncParam { attr, ty, .. } = rw.arena[p].exp.clone() else {
                ftps.push(rw.deep_copy(p));
                continue;
            };
            let attr = rw.deep_copy(attr);
            let ty = rw.deep_copy(ty);
            ftps.push(rw.donate(captures, Exp::FunctionTypeParam { attr, ty, name: None }));
        }
        let plist = rw.donate(captures, Exp::Seq(ftps));
        // `funcReturnType` is reused by the reference (`:212`); the original
        // stays with the synthesized function (`:248`), so this is the copy.
        let ret = rw.deep_copy(ret);
        rw.donate(captures, Exp::FunctionType { attr, params: plist, ret })
    } else {
        let lambda_ref = lambda_qualified_ref(rw, &scope, captures, name);
        rw.donate(captures, Exp::Decltype { scope: scope.clone(), expr: lambda_ref })
    };

    // Argument 2: the same `lambda` value a second time (`:166`).
    let arg2 = lambda_qualified_ref(rw, &scope, captures, name);
    // Argument 3: `captureTypeIdent declScopeName $ new l` (`:214`) — outer
    // nodes wear the capture list's note this time, chain and leaf still NAME.
    let arg3 = capture_type_ident(rw, &scope, captures, &scope, name, capture_ty_sym);

    // `closureInstance`/`templateInstance` (`:173-184`):
    // `Closure<|arg1, arg2, arg3|>` from `data.closure.core` (`Type.hs:121-122`).
    let args = rw.donate(captures, Exp::Seq(vec![arg1, arg2, arg3]));
    let closure_path = Ty::closure_template_path(rw.interner);
    let template = scoped_qualifier(rw, &scope, captures, closure_path.segments())
        .expect("the Closure path has two segments");
    let instance = rw.donate(captures, Exp::TemplateInstance { base: template, args });
    let ty = rw.donate(captures, Exp::TypeIdentifier(scope, instance));

    let value = make_capture(rw, l, CallSite::CastValue);
    Exp::Cast { ty, value }
}

/// `lambda = QualifiedIdentifierF $ new $ ScopedNameF declScopeName
/// (scopedQualifier declScopeName e declScopeName) declName`
/// (`Function.hs:169`): the qualified reference `<scope>::?lambdaN`. Built
/// once per use — the reference shares the one binding between arguments 1
/// and 2. The leaf is a copy of `declName`; the original stays as the
/// synthesized function's name.
fn lambda_qualified_ref(
    rw: &mut Rewriter,
    scope: &QualifiedName,
    captures: ExpId,
    name: ExpId,
) -> ExpId {
    let prefix = scoped_qualifier(rw, scope, captures, scope.segments());
    let leaf = rw.deep_copy(name);
    let sn = rw.donate(captures, Exp::ScopedName { scope: scope.clone(), prefix, ident: leaf });
    rw.donate(captures, Exp::QualifiedIdentifier(sn))
}

/// **The pass's own invariant**, recomputed rather than trusted: the lambdas
/// this pass should have desugared — derived from the *input* by a top-down
/// re-reading of the placement rules alone (`Function.hs:192-201,229-235`) —
/// match, one for one, the `?lambdaN$capture` aliases, the
/// `FunctionKind::LambdaFunction` declarations and the surviving `Lambda`
/// nodes in the *output*; and on each pair, three independent spellings of the
/// capture arity agree: the input capture list's length, the `N` of the
/// `make_captureN` the alias calls (module flipped to `@data@closure@core`
/// exactly at `N = 0`, `Function.hs:152-154`), and the length of the
/// function's `xI` unpacking prologue.
///
/// `before` must be the IR the pass was handed — after `captureThis`, so the
/// arity includes the implicit `This` — and `after` this pass's **own**
/// output, not the whole pipeline's: `abbreviatedFunctionTemplate` runs next
/// and duplicates every generic lambda's function (template copy plus
/// leftover), which moves the count by design, not by defect. The derivation
/// here shares nothing with the pass but the `Exp` type: the pass collects
/// bottom-up through `expand` targets, this walks top-down carrying a
/// "reachable by a target" flag, so a lost boundary shows up as a pair the
/// derivation did not predict (a `static for` leak, a double desugar through
/// a broken `Function` revert) and a lost target as a `Lambda` that should
/// have died (a skipped `static if` branch, a missed global variable).
///
/// What it cannot see: `Src` donors (the donor unit tests own those), the
/// `Cast`'s internal shape and its generic-vs-`decltype` first argument, and
/// *where* in the enclosing `Seq` a pair landed — existence and arity only.
pub fn lambda_functions_match_lambda_sites(
    before: &crate::ModuleIr,
    after: &crate::ModuleIr,
    interner: &kg_base::Interner,
) -> Result<(), String> {
    let key = |ir: &crate::ModuleIr, scope: &QualifiedName, name: ExpId| {
        let scope = scope
            .segments()
            .iter()
            .map(|s| interner.resolve(*s).to_string())
            .collect::<Vec<_>>()
            .join("::");
        let name = match &ir.arena[name].exp {
            Exp::Identifier(s) => interner.resolve(*s).to_string(),
            other => format!("{other:?}"),
        };
        (scope, name)
    };

    // --- derivation A: what the input says must happen -----------------------
    let mut expected: Vec<(String, String, usize)> = Vec::new();
    let mut expected_alive: Vec<(String, String)> = Vec::new();
    expect_lambdas(before, before.root, false, &mut |id, live| {
        let Exp::Lambda { scope, name, captures, .. } = &before.arena[id].exp else {
            unreachable!()
        };
        let named = matches!(before.arena[*name].exp, Exp::Identifier(_));
        let (scope, name) = key(before, scope, *name);
        if live && named {
            let Exp::Seq(caps) = &before.arena[*captures].exp else {
                return;
            };
            expected.push((scope, name, caps.len()));
        } else {
            expected_alive.push((scope, name));
        }
    });

    // --- derivation B: what the output holds ---------------------------------
    let mut functions: Vec<(String, String, usize)> = Vec::new();
    let mut aliases: Vec<(String, String, usize)> = Vec::new();
    let mut alive: Vec<(String, String)> = Vec::new();
    let mut module_flip: Option<String> = None;
    walk(after, |ir, id| match &ir.arena[id].exp {
        Exp::Function { kind: FunctionKind::LambdaFunction, scope, name, body, .. } => {
            let (scope, name) = key(ir, scope, *name);
            let arity = prologue_len(ir, interner, *body);
            functions.push((scope, name, arity));
        }
        Exp::Alias { scope, name, body } => {
            let (scope, name) = key(ir, scope, *name);
            let Some(base) = name.strip_suffix("$capture") else { return };
            match alias_capture_call(ir, interner, *body) {
                Some((n, core)) if core == (n == 0) => {
                    aliases.push((scope, base.to_string(), n));
                }
                Some((n, _)) => {
                    module_flip = Some(format!(
                        "alias {scope}::{name} calls make_capture{n} from the wrong \
                         module — core iff arity 0 (Function.hs:152-154)"
                    ));
                }
                None => module_flip = Some(format!(
                    "alias {scope}::{name} does not wrap a make_captureN call \
                     (Function.hs:242-244)"
                )),
            }
        }
        Exp::Lambda { scope, name, .. } => alive.push(key(ir, scope, *name)),
        _ => {}
    });
    if let Some(msg) = module_flip {
        return Err(msg);
    }

    expected.sort();
    functions.sort();
    aliases.sort();
    expected_alive.sort();
    alive.sort();
    if functions != expected {
        return Err(format!(
            "LambdaFunction declarations do not match the collectible lambdas \
             (Function.hs:229-248): expected {expected:?}, found {functions:?} \
             (scope, name, arity-by-prologue)"
        ));
    }
    if aliases != expected {
        return Err(format!(
            "$capture aliases do not match the collectible lambdas \
             (Function.hs:222,242-244): expected {expected:?}, found {aliases:?} \
             (scope, name, arity-by-make_captureN)"
        ));
    }
    if alive != expected_alive {
        return Err(format!(
            "surviving Lambda nodes do not match the uncollectible sites \
             (Function.hs:218-219,234): expected {expected_alive:?}, found {alive:?}"
        ));
    }
    Ok(())
}

/// Derivation A's walk: `live` says "a target's `lambdaDefs` reaches here
/// without crossing a stop node". Targets set it, stop nodes clear it, and a
/// `Lambda`'s or `Function`'s *body* is always a fresh target — the cata fires
/// clause 1/2 on every one it visits, wherever it sits (`Function.hs:192-193`).
fn expect_lambdas(
    ir: &crate::ModuleIr,
    id: ExpId,
    live: bool,
    on_lambda: &mut impl FnMut(ExpId, bool),
) {
    match &ir.arena[id].exp {
        Exp::Lambda { name, captures, params, ret, body, .. } => {
            on_lambda(id, live);
            for &k in &[*name, *captures, *params, *ret] {
                expect_lambdas(ir, k, false, on_lambda);
            }
            expect_lambdas(ir, *body, true, on_lambda);
        }
        Exp::Function { attr, modifier, ret, name, params, body, .. } => {
            let mut cold = vec![*attr, *ret, *name, *params];
            cold.extend(modifier.iter().copied());
            for k in cold {
                expect_lambdas(ir, k, false, on_lambda);
            }
            expect_lambdas(ir, *body, true, on_lambda);
        }
        // `Function.hs:196-201`: only a branch that is a `NestedScope` has its
        // body rewritten; the condition is out of reach.
        Exp::StaticIf { cond, then_branch, else_branch } => {
            expect_lambdas(ir, *cond, false, on_lambda);
            for b in [Some(*then_branch), *else_branch].into_iter().flatten() {
                match &ir.arena[b].exp {
                    Exp::NestedScope { name, body, .. } => {
                        expect_lambdas(ir, *name, false, on_lambda);
                        expect_lambdas(ir, *body, true, on_lambda);
                    }
                    _ => expect_lambdas(ir, b, false, on_lambda),
                }
            }
        }
        // `Function.hs:234` and `:219` — nothing under a surviving `static
        // for` is touched, and no clause restarts inside it short of a
        // `Function` or `Lambda`, which the arms above handle wherever found.
        Exp::StaticFor { var, limit, body } => {
            for &k in &[*var, *limit, *body] {
                expect_lambdas(ir, k, false, on_lambda);
            }
        }
        // `Function.hs:194-195` — every child of a global variable is inside
        // the transient wrapper `lambdaDefs` walks.
        Exp::Variable { flags: DeclareFlag::DeclareGlobal, attr, ty, name, init, .. } => {
            let mut kids = vec![*attr, *ty, *name];
            kids.extend(init.iter().copied());
            for k in kids {
                expect_lambdas(ir, k, true, on_lambda);
            }
        }
        e => {
            let mut kids = Vec::new();
            e.children(&mut kids);
            for k in kids {
                expect_lambdas(ir, k, live, on_lambda);
            }
        }
    }
}

/// How many leading statements of `body` are `uncaptureVar` locals — a
/// `DeclareLocal` variable initialized from `<…$capture_param>.xI` with `I`
/// equal to its position (`Function.hs:256-262`). Unforgeable by written code:
/// `$` cannot appear in a written identifier.
fn prologue_len(ir: &crate::ModuleIr, interner: &kg_base::Interner, body: ExpId) -> usize {
    let Exp::Seq(stmts) = &ir.arena[body].exp else { return 0 };
    let mut n = 0;
    for (i, &s) in stmts.iter().enumerate() {
        let Exp::Variable { flags: DeclareFlag::DeclareLocal, init: Some(init), .. } =
            &ir.arena[s].exp
        else {
            break;
        };
        let Exp::MemberAccess { base, member } = &ir.arena[*init].exp else { break };
        let Exp::Identifier(m) = &ir.arena[*member].exp else { break };
        if interner.resolve(*m) != format!("x{i}") {
            break;
        }
        let Exp::NamedValue(qi) = &ir.arena[*base].exp else { break };
        let Some(leaf) = spine_leaf(ir, *qi) else { break };
        if !interner.resolve(leaf).ends_with("$capture_param") {
            break;
        }
        n = i + 1;
    }
    n
}

/// The `N` of the `make_captureN` a `$capture` alias's `decltype` calls, and
/// whether the module segment is `@data@closure@core` (`Function.hs:150-154`).
fn alias_capture_call(
    ir: &crate::ModuleIr,
    interner: &kg_base::Interner,
    alias_body: ExpId,
) -> Option<(usize, bool)> {
    let Exp::Decltype { expr, .. } = &ir.arena[alias_body].exp else { return None };
    let Exp::FunctionCall { function, .. } = &ir.arena[*expr].exp else { return None };
    let Exp::FunctionSpecifier { function: qi, .. } = &ir.arena[*function].exp else {
        return None;
    };
    let Exp::QualifiedIdentifier(sn) = &ir.arena[*qi].exp else { return None };
    let Exp::ScopedName { prefix, ident, .. } = &ir.arena[*sn].exp else { return None };
    let Exp::Identifier(leaf) = &ir.arena[*ident].exp else { return None };
    let n: usize = interner.resolve(*leaf).strip_prefix("make_capture")?.parse().ok()?;
    let module = prefix.and_then(|p| spine_leaf(ir, p))?;
    Some((n, interner.resolve(module) == DATA_CLOSURE_CORE))
}

/// The final `Identifier` under a name spine.
fn spine_leaf(ir: &crate::ModuleIr, id: ExpId) -> Option<Symbol> {
    match &ir.arena[id].exp {
        Exp::Identifier(s) => Some(*s),
        Exp::QualifiedIdentifier(a) | Exp::TypeIdentifier(_, a) | Exp::NamedValue(a) => {
            spine_leaf(ir, *a)
        }
        Exp::ScopedName { ident, .. } => spine_leaf(ir, *ident),
        _ => None,
    }
}

/// A reachable walk, visited-set guarded like `abbreviated::walk`.
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
/// Every expectation below was read off the reference frontend built by
/// `oracle/build.sh`, captured 2026-08-13, at
///
/// ```text
/// kg-oracle --tree  --roots-only --no-base --passes 8 \
///           --template-passes {1,2} --template-iterations 0 FILE
/// kg-oracle --notes --roots-only --no-base --passes 8 \
///           --template-passes 2 --template-iterations 0 FILE
/// ```
///
/// `--template-passes 1` vs `2` isolates `desugarLambdas` exactly (sub-pass 2
/// in execution order; `Template.hs:62-64` drops from the *front* of the
/// reversed list). `--passes 8` is not optional: without it `removeTemplate`
/// (`Frontend.hs:300-302`) deletes the output and a fixture prints `SeqF []`.
/// Because our `Src` for the lambda's `declName` and capture-list `Seq`
/// deliberately differs from the reference's zero-width points (divergence
/// #26), every `--notes` expectation is asserted as donor *identity* — the
/// synthesized node wears the same `Src` as a named donor node — never as an
/// offset.
#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner, Written};
    use kg_syntax::ast;
    use kg_syntax::AstNode;

    use super::*;
    use crate::rewrite::{capture_this, desugar_lambdas, pipeline};
    use crate::{lower, ModuleIr};

    /// `before` is the IR the pass is handed in [`crate::rewrite::pipeline`] —
    /// after `captureThis`, which decides the arity — and `after` its output.
    fn run(text: &str) -> (ModuleIr, ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "probe must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let lowered = lower(&file, FileId(0), &interner);
        let before = capture_this(&lowered, &interner);
        let after = desugar_lambdas(&before, &interner);
        (before, after, interner)
    }

    fn kids(ir: &ModuleIr, id: ExpId) -> Vec<ExpId> {
        let mut out = Vec::new();
        ir.arena[id].exp.children(&mut out);
        out
    }

    fn text(ir: &ModuleIr, interner: &Interner, id: ExpId) -> String {
        match &ir.arena[id].exp {
            Exp::Identifier(s) => interner.resolve(*s).to_string(),
            other => format!("{other:?}"),
        }
    }

    fn scope_of(ir: &ModuleIr, interner: &Interner, id: ExpId) -> Vec<String> {
        let scope = match &ir.arena[id].exp {
            Exp::Alias { scope, .. }
            | Exp::Function { scope, .. }
            | Exp::Variable { scope, .. }
            | Exp::Lambda { scope, .. } => scope,
            other => panic!("no scope on {other:?}"),
        };
        scope.segments().iter().map(|s| interner.resolve(*s).to_string()).collect()
    }

    /// Every `Lambda` in `ir`, in walk order, as `(id, capture ids)`.
    fn lambdas(ir: &ModuleIr) -> Vec<(ExpId, Vec<ExpId>)> {
        let mut out = Vec::new();
        super::walk(ir, |ir, id| {
            if let Exp::Lambda { captures, .. } = &ir.arena[id].exp {
                let Exp::Seq(items) = &ir.arena[*captures].exp else { panic!() };
                out.push((id, items.clone()));
            }
        });
        out
    }

    fn lambda_functions(ir: &ModuleIr) -> Vec<ExpId> {
        let mut out = Vec::new();
        super::walk(ir, |ir, id| {
            if matches!(
                ir.arena[id].exp,
                Exp::Function { kind: FunctionKind::LambdaFunction, .. }
            ) {
                out.push(id);
            }
        });
        out
    }

    /// The `Cast` in the first module-level function's `auto g = …;` variable —
    /// where every fixture below writes its lambda.
    fn the_cast(ir: &ModuleIr) -> ExpId {
        let mut out = None;
        super::walk(ir, |ir, id| {
            if matches!(ir.arena[id].exp, Exp::Cast { .. }) && out.is_none() {
                out = Some(id);
            }
        });
        out.expect("a Cast was emitted")
    }

    /// A subtree walk rooted anywhere, for probing one declaration.
    fn walk_from(ir: &ModuleIr, from: ExpId, f: &mut impl FnMut(&ModuleIr, ExpId)) {
        f(ir, from);
        for k in kids(ir, from) {
            walk_from(ir, k, f);
        }
    }

    /// The `make_captureN` call under a node: `(leaf, module, arg ids)`.
    fn capture_call(ir: &ModuleIr, interner: &Interner, from: ExpId) -> (String, String, Vec<ExpId>) {
        let mut found = None;
        walk_from(ir, from, &mut |ir: &ModuleIr, id: ExpId| {
            if found.is_some() {
                return;
            }
            if let Exp::FunctionCall { function, args, .. } = &ir.arena[id].exp {
                let Exp::FunctionSpecifier { function: qi, .. } = &ir.arena[*function].exp else {
                    return;
                };
                let Exp::QualifiedIdentifier(sn) = &ir.arena[*qi].exp else { return };
                let Exp::ScopedName { prefix, ident, .. } = &ir.arena[*sn].exp else { return };
                let leaf = text(ir, interner, *ident);
                if !leaf.starts_with("make_capture") {
                    return;
                }
                let module = prefix
                    .and_then(|p| super::spine_leaf(ir, p))
                    .map(|s| interner.resolve(s).to_string())
                    .unwrap_or_default();
                let Exp::Seq(items) = &ir.arena[*args].exp else { return };
                found = Some((leaf, module, items.clone()));
            }
        });
        found.expect("a make_captureN call")
    }

    /// A zero-capture lambda still gets the alias, the `$capture_param` and
    /// the `Closure` cast, and its `make_capture0` comes from the *core*
    /// module. `--template-passes 2` on `auto g = [](uint8 a) -> uint8 {…}`:
    ///
    /// ```text
    /// body = SeqF
    ///   [ AliasF {…, declName = IdentifierF "?lambda0$capture",
    ///       body = DecltypeF {declExp = FunctionCallF {… (Just ScopedNameF …
    ///         IdentifierF "@data@closure@core") IdentifierF "make_capture0",
    ///         callArgs = SeqF []}}}
    ///   , FunctionF {funcKind = LambdaFunction, declName = IdentifierF "?lambda0",
    ///       funcParams = SeqF [FuncParamF {… declName = IdentifierF
    ///         "?lambda0$capture_param"}, FuncParamF {… "a"}],
    ///       body = SeqF [ReturnF …]}                       -- no prologue
    ///   , VariableF {… varInit = Just CastF … IdentifierF "Closure" SeqF
    ///       [DecltypeF …, QualifiedIdentifierF …, TypeIdentifierF …] …}
    ///   , ReturnF … ]
    /// ```
    #[test]
    fn a_zero_capture_lambda_still_gets_the_full_treatment() {
        let (_, ir, interner) = run(
            "inline uint8 f()\n{\n    auto g = [](uint8 a) -> uint8\n    {\n\
             \x20       return a;\n    };\n\n    return g(1);\n}\n",
        );
        let f = kids(&ir, ir.root)[0];
        let Exp::Function { body, .. } = &ir.arena[f].exp else { panic!() };
        let stmts = kids(&ir, *body);
        assert_eq!(stmts.len(), 4, "alias, function, variable, return");

        let Exp::Alias { name, .. } = &ir.arena[stmts[0]].exp else {
            panic!("first is the alias, got {:?}", ir.arena[stmts[0]].exp)
        };
        assert_eq!(text(&ir, &interner, *name), "?lambda0$capture");
        let (leaf, module, args) = capture_call(&ir, &interner, stmts[0]);
        assert_eq!((leaf.as_str(), module.as_str()), ("make_capture0", "@data@closure@core"));
        assert!(args.is_empty());

        let Exp::Function { kind, name, params, body, .. } = &ir.arena[stmts[1]].exp else {
            panic!("second is the lambda function")
        };
        assert_eq!(*kind, FunctionKind::LambdaFunction);
        assert_eq!(text(&ir, &interner, *name), "?lambda0");
        let ps = kids(&ir, *params);
        assert_eq!(ps.len(), 2, "capture param prepended to the written one");
        let Exp::FuncParam { name, .. } = &ir.arena[ps[0]].exp else { panic!() };
        assert_eq!(text(&ir, &interner, *name), "?lambda0$capture_param");
        let body_stmts = kids(&ir, *body);
        assert!(
            matches!(ir.arena[body_stmts[0]].exp, Exp::Return(_)),
            "an empty capture list unpacks nothing (zip [0..] [] — Function.hs:256)"
        );

        let Exp::Variable { init: Some(init), .. } = &ir.arena[stmts[2]].exp else { panic!() };
        let Exp::Cast { ty, .. } = &ir.arena[*init].exp else { panic!("the use site is a Cast") };
        let Exp::TypeIdentifier(_, inst) = &ir.arena[*ty].exp else { panic!() };
        let Exp::TemplateInstance { base, args } = &ir.arena[*inst].exp else { panic!() };
        let Exp::ScopedName { prefix, ident, .. } = &ir.arena[*base].exp else { panic!() };
        assert_eq!(text(&ir, &interner, *ident), "Closure");
        let module = super::spine_leaf(&ir, prefix.unwrap()).unwrap();
        assert_eq!(interner.resolve(module), "@data@closure@core", "Type.hs:121-122");
        let targs = kids(&ir, *args);
        assert_eq!(targs.len(), 3);
        assert!(
            matches!(ir.arena[targs[0]].exp, Exp::Decltype { .. }),
            "non-generic: fromMaybe (decltype lambda) Nothing (Function.hs:208)"
        );
    }

    /// Written captures unpack positionally: `[p, q]` becomes `x0`/`x1` and
    /// `make_capture2` from `@data@closure` — the module flips on arity 0, not
    /// on the lambda number. `--template-passes 2` on `[p, q](uint8 a)`:
    ///
    /// ```text
    /// VariableF {declScopeName = ["f","$","?lambda0","$"], varType = AutoF,
    ///   declName = IdentifierF "p", varInit = Just MemberAccessF NamedValueF …
    ///   IdentifierF "?lambda0$capture_param" IdentifierF "x0", declFlags = DeclareLocal}
    /// VariableF {… declName = IdentifierF "q", … IdentifierF "x1" …}
    /// … IdentifierF "@data@closure") IdentifierF "make_capture2",
    ///   callArgs = SeqF [NamedValueF … "p", NamedValueF … "q"]
    /// ```
    #[test]
    fn written_captures_unpack_in_order_as_x0_x1() {
        let (_, ir, interner) = run(
            "inline uint8 f(uint8 p, uint8 q)\n{\n\
             \x20   auto g = [p, q](uint8 a) -> uint8\n    {\n\
             \x20       return a + p + q;\n    };\n\n    return g(1);\n}\n",
        );
        let func = lambda_functions(&ir)[0];
        let Exp::Function { body, scope, .. } = &ir.arena[func].exp else { panic!() };
        assert_eq!(
            scope.segments().iter().map(|s| interner.resolve(*s)).collect::<Vec<_>>(),
            ["f", "$"],
            "the declaration copies the lambda's declScopeName"
        );
        let stmts = kids(&ir, *body);
        let unpacked: Vec<(String, String)> = stmts[..2]
            .iter()
            .map(|&s| {
                let Exp::Variable { name, init: Some(init), scope, flags, .. } =
                    &ir.arena[s].exp
                else {
                    panic!("prologue is variables")
                };
                assert_eq!(*flags, DeclareFlag::DeclareLocal);
                assert_eq!(
                    scope.segments().iter().map(|s| interner.resolve(*s)).collect::<Vec<_>>(),
                    ["f", "$", "?lambda0", "$"],
                    "bodyDeclScopeName = functionScope signatureDeclScopeName (Function.hs:252)"
                );
                let Exp::MemberAccess { member, .. } = &ir.arena[*init].exp else { panic!() };
                (text(&ir, &interner, *name), text(&ir, &interner, *member))
            })
            .collect();
        assert_eq!(
            unpacked,
            [("p".to_owned(), "x0".to_owned()), ("q".to_owned(), "x1".to_owned())],
            "zip [0..] in source order (Function.hs:256)"
        );

        let (leaf, module, args) = capture_call(&ir, &interner, the_cast(&ir));
        assert_eq!((leaf.as_str(), module.as_str()), ("make_capture2", "@data@closure"));
        let arg_names: Vec<String> = args
            .iter()
            .map(|&a| {
                let Exp::NamedValue(qi) = &ir.arena[a].exp else { panic!() };
                interner.resolve(super::spine_leaf(&ir, *qi).unwrap()).to_string()
            })
            .collect();
        assert_eq!(arg_names, ["p", "q"]);
    }

    /// A `this` capture counts toward the arity, keeps its own node at the use
    /// site (`arg x@This{} = x`, `Function.hs:156`), and unpacks as a local
    /// named `$this`. `--template-passes 2` on a class inside a free function
    /// (`captureThis` ran at frontend order 3, so `tp=1` already shows
    /// `lambdaCaptures = SeqF [ThisF …]`):
    ///
    /// ```text
    /// … IdentifierF "@data@closure") IdentifierF "make_capture1",
    ///   callArgs = SeqF [ThisF {declScopeName = ["h","$","Helper","test","$"]}]
    /// VariableF {… declName = IdentifierF "$this", varInit = Just MemberAccessF …
    ///   IdentifierF "?lambda0$capture_param" IdentifierF "x0", declFlags = DeclareLocal}
    /// ```
    ///
    /// — `make_capture1` from `@data@closure`, **not** `make_capture0` from
    /// core: the implicit capture moves the module too.
    #[test]
    fn a_this_capture_is_arity_module_and_dollar_this() {
        let (before, ir, interner) = run(
            "inline void h()\n{\n    class Helper\n    {\n    private:\n\
             \x20       uint8 _x;\n\n    public:\n        void test()\n        {\n\
             \x20           _x = 4;\n\n            auto cb = [] () -> uint8\n\
             \x20           {\n                return _x;\n            };\n        }\n\
             \x20   }\n}\n",
        );
        let func = lambda_functions(&ir)[0];
        let Exp::Function { body, .. } = &ir.arena[func].exp else { panic!() };
        let first = kids(&ir, *body)[0];
        let Exp::Variable { name, init: Some(init), .. } = &ir.arena[first].exp else {
            panic!()
        };
        assert_eq!(text(&ir, &interner, *name), "$this", "getName ThisF (Parser/Syntax.hs:739)");
        let Exp::MemberAccess { member, .. } = &ir.arena[*init].exp else { panic!() };
        assert_eq!(text(&ir, &interner, *member), "x0", "cons prepends, so `this` is capture 0");

        let (_, caps) = lambdas(&before).remove(0);
        let written_this = caps[0];
        assert!(matches!(before.arena[written_this].exp, Exp::This { .. }));

        // The cast's argument is the This node itself; the alias's is a copy
        // wearing the same Src.
        let (leaf, module, cast_args) = capture_call(&ir, &interner, the_cast(&ir));
        assert_eq!((leaf.as_str(), module.as_str()), ("make_capture1", "@data@closure"));
        assert_eq!(cast_args, vec![written_this], "arg x@This{{}} = x — passed through");

        let alias = kids(&ir, *find_alias_holder(&ir))[0];
        let (_, _, alias_args) = capture_call(&ir, &interner, alias);
        assert_ne!(alias_args[0], written_this, "two positions cannot share one arena node");
        assert!(matches!(ir.arena[alias_args[0]].exp, Exp::This { .. }));
        assert_eq!(
            ir.arena[alias_args[0]].src.head, before.arena[written_this].src.head,
            "the copy keeps the This's own note"
        );
    }

    /// The body `Seq` that holds the first alias — the innermost rewritten
    /// block, for fixtures where the lambda is not at the top level.
    fn find_alias_holder(ir: &ModuleIr) -> Box<ExpId> {
        let mut out = None;
        super::walk(ir, |ir, id| {
            if out.is_some() {
                return;
            }
            if let Exp::Seq(items) = &ir.arena[id].exp
                && items.iter().any(|&i| matches!(ir.arena[i].exp, Exp::Alias { .. }))
            {
                out = Some(id);
            }
        });
        Box::new(out.expect("an alias was prepended somewhere"))
    }

    /// `isGenericFunction` (`Function.hs:186-187,207,212-216`): any bare
    /// `auto` parameter swaps the cast's first `Closure` argument from a
    /// `decltype` to a written `FunctionType`. `--template-passes 2` on
    /// `[p](auto a) -> uint8`:
    ///
    /// ```text
    /// CastF TypeIdentifierF ["f","$"] TemplateInstanceF … IdentifierF "Closure" SeqF
    ///  [ FunctionTypeF SeqF [] SeqF
    ///      [ FunctionTypeParamF SeqF [] TypeIdentifierF … "?lambda0$capture" Nothing
    ///      , FunctionTypeParamF SeqF [] AutoF Nothing ] UnsignedF 8
    ///  , QualifiedIdentifierF … "?lambda0"
    ///  , TypeIdentifierF … "?lambda0$capture" ]
    /// ```
    #[test]
    fn a_generic_lambda_swaps_the_decltype_for_a_function_type() {
        let (before, ir, interner) = run(
            "inline uint8 f(uint8 p)\n{\n    auto g = [p](auto a) -> uint8\n    {\n\
             \x20       return a + p;\n    };\n\n    return g(1);\n}\n",
        );
        let Exp::Cast { ty, .. } = &ir.arena[the_cast(&ir)].exp else { panic!() };
        let Exp::TypeIdentifier(_, inst) = &ir.arena[*ty].exp else { panic!() };
        let Exp::TemplateInstance { args, .. } = &ir.arena[*inst].exp else { panic!() };
        let targs = kids(&ir, *args);
        let Exp::FunctionType { params, ret, .. } = &ir.arena[targs[0]].exp else {
            panic!("generic: a written FunctionType, got {:?}", ir.arena[targs[0]].exp)
        };
        let ftps = kids(&ir, *params);
        assert_eq!(ftps.len(), 2, "captureParam : one per lambda param (Function.hs:213)");
        let Exp::FunctionTypeParam { ty: t0, name, .. } = &ir.arena[ftps[0]].exp else {
            panic!()
        };
        assert!(name.is_none());
        assert_eq!(
            interner.resolve(super::spine_leaf(&ir, *t0).unwrap()),
            "?lambda0$capture"
        );
        let Exp::FunctionTypeParam { ty: t1, .. } = &ir.arena[ftps[1]].exp else { panic!() };
        assert!(matches!(ir.arena[*t1].exp, Exp::Auto));
        assert!(matches!(ir.arena[*ret].exp, Exp::Unsigned(8)));

        // `functionTypeParam` reuses the parameter's type node and `:212`
        // reuses the return type; the originals stay with the synthesized
        // function, so these are copies wearing the same notes.
        let (lambda, _) = lambdas(&before).remove(0);
        let Exp::Lambda { params: lp, ret: lret, .. } = &before.arena[lambda].exp else {
            panic!()
        };
        let Exp::FuncParam { ty: written_auto, .. } = &before.arena[kids(&before, *lp)[0]].exp
        else {
            panic!()
        };
        assert_ne!(*t1, *written_auto, "two positions cannot share one arena node");
        assert_eq!(ir.arena[*t1].src.head, before.arena[*written_auto].src.head);
        assert_ne!(*ret, *lret);
        assert_eq!(ir.arena[*ret].src.head, before.arena[*lret].src.head);
        let func = lambda_functions(&ir)[0];
        let Exp::Function { ret: kept, params: kept_ps, .. } = &ir.arena[func].exp else {
            panic!()
        };
        assert_eq!(*kept, *lret, "the original return type lives on in the function");
        let Exp::FuncParam { ty: kept_auto, .. } = &ir.arena[kids(&ir, *kept_ps)[1]].exp else {
            panic!()
        };
        assert_eq!(*kept_auto, *written_auto, "the original auto lives on in the parameter");
    }

    /// The declarations land in the enclosing **function** body's `Seq` — not
    /// the `if` block's — while their `declScopeName` still says `$quasi0`.
    /// The two disagree on purpose (`lambdaDefs` stops at `Function` only,
    /// `Function.hs:229-235`). `--template-passes 2`, in order, inside `f`'s
    /// body:
    ///
    /// ```text
    /// [0] AliasF    {declScopeName = ["f","$","$quasi0"], …}
    /// [1] FunctionF {declScopeName = ["f","$","$quasi0"], funcKind = LambdaFunction, …}
    /// [2] VariableF {declScopeName = ["f","$"], … "r" …}
    /// [3] IfF {…}
    /// [4] ReturnF (…)
    /// ```
    #[test]
    fn an_if_block_lambda_hoists_to_the_function_body_and_keeps_its_quasi_scope() {
        let (_, ir, interner) = run(
            "inline uint8 f(uint8 p)\n{\n    uint8 r = 0;\n\n    if (p > 1)\n    {\n\
             \x20       auto c = [p]() -> uint8 { return p; };\n        r = c();\n    }\n\n\
             \x20   return r;\n}\n",
        );
        let f = kids(&ir, ir.root)[0];
        let Exp::Function { body, .. } = &ir.arena[f].exp else { panic!() };
        let stmts = kids(&ir, *body);
        assert_eq!(stmts.len(), 5);
        assert!(matches!(ir.arena[stmts[0]].exp, Exp::Alias { .. }));
        assert!(matches!(
            ir.arena[stmts[1]].exp,
            Exp::Function { kind: FunctionKind::LambdaFunction, .. }
        ));
        assert!(matches!(ir.arena[stmts[2]].exp, Exp::Variable { .. }));
        assert!(matches!(ir.arena[stmts[3]].exp, Exp::If { .. }));
        assert_eq!(scope_of(&ir, &interner, stmts[0]), ["f", "$", "$quasi0"]);
        assert_eq!(scope_of(&ir, &interner, stmts[1]), ["f", "$", "$quasi0"]);
    }

    /// `expand StaticIfF` (`Function.hs:196-201`): each branch's `NestedScope`
    /// body gets its own declarations, *inside* the branch — `lambdaDefs`
    /// refuses to cross a `StaticIf`, so without this clause the branch's
    /// lambdas would never desugar. 27 corpus lambdas sit inside a
    /// `static if`. `--template-passes 2` on a template whose `static if`
    /// branch holds the lambda:
    ///
    /// ```text
    /// StaticIfF … NestedScopeF ["f","$"]                 -- then branch
    ///   [0] AliasF    {declScopeName = ["f","$","$quasi0"], …}
    ///   [1] FunctionF {declScopeName = ["f","$","$quasi0"], funcKind = LambdaFunction, …}
    ///   [2] VariableF {… "c" … CastF …}
    ///   [3] ReturnF (…)
    /// … NestedScopeF ["f","$"] [ReturnF …]               -- else branch, untouched
    /// ```
    #[test]
    fn a_static_if_branch_desugars_inside_the_branch() {
        let (_, ir, interner) = run(
            "template <auto B>\ninline uint8 f(uint8 p)\n{\n    static if (B)\n    {\n\
             \x20       auto c = [p]() -> uint8 { return p; };\n        return c();\n    }\n\
             \x20   else\n    {\n        return p;\n    }\n}\n",
        );
        let Exp::Template { decl, .. } = &ir.arena[kids(&ir, ir.root)[0]].exp else { panic!() };
        let Exp::Function { body, .. } = &ir.arena[*decl].exp else { panic!() };
        let stmts = kids(&ir, *body);
        let Exp::StaticIf { then_branch, else_branch, .. } = &ir.arena[stmts[0]].exp else {
            panic!("the static if is the only statement, got {:?}", ir.arena[stmts[0]].exp)
        };
        let Exp::NestedScope { body: then_body, .. } = &ir.arena[*then_branch].exp else {
            panic!()
        };
        let then_stmts = kids(&ir, *then_body);
        assert_eq!(then_stmts.len(), 4);
        assert!(matches!(ir.arena[then_stmts[0]].exp, Exp::Alias { .. }));
        assert!(matches!(
            ir.arena[then_stmts[1]].exp,
            Exp::Function { kind: FunctionKind::LambdaFunction, .. }
        ));
        assert_eq!(scope_of(&ir, &interner, then_stmts[0]), ["f", "$", "$quasi0"]);

        let Exp::NestedScope { body: else_body, .. } =
            &ir.arena[else_branch.unwrap()].exp
        else {
            panic!()
        };
        let else_stmts = kids(&ir, *else_body);
        assert_eq!(else_stmts.len(), 1, "the lambda-free branch is untouched");
        assert!(matches!(ir.arena[else_stmts[0]].exp, Exp::Return(_)));
    }

    /// Both `Nothing` arms of `lambdaRef`, with the corpus behind them. A
    /// lambda under a surviving `static for` is neither collected
    /// (`Function.hs:234`) nor replaced (`:219`) — the reference desugars it
    /// only after the loop unrolls. `--template-passes 1` and `2` print
    /// byte-identical trees for this fixture (`StaticForF` and `LambdaF` both
    /// still present), so the assertion is that nothing was allocated at all.
    #[test]
    fn a_lambda_under_a_surviving_static_for_is_left_alone() {
        let (before, after, _) = run(
            "template <auto N>\ninline uint8 f(uint8 p)\n{\n    uint8 r = 0;\n\n\
             \x20   static for (const auto i : N)\n    {\n\
             \x20       auto c = [p]() -> uint8 { return p; };\n        r = c();\n    }\n\n\
             \x20   return r;\n}\n",
        );
        assert_eq!(lambdas(&after).len(), 1, "the lambda survives");
        assert_eq!(before.root, after.root, "nothing changed, so nothing was rebuilt");
        assert_eq!(before.arena.len(), after.arena.len(), "the revert allocates nothing");
    }

    /// The `StaticFor` protection is shallow: the cata still fires clause 2 on
    /// a lambda inside the loop, so a lambda nested inside *it* is desugared —
    /// into the survivor's body — and the survivor stands bare, its body
    /// rewritten, with no alias and no function of its own. The reference's
    /// shape on `library/data/vector.k` (`--template-passes 2`): 22 `LambdaF`
    /// become 2, both direct children of a `static for`, one of which held a
    /// nested lambda at `tp=1`.
    #[test]
    fn a_static_for_survivors_nested_lambda_is_still_desugared() {
        let (before, ir, interner) = run(
            "template <auto N>\ninline uint8 f(uint8 p)\n{\n    uint8 r = 0;\n\n\
             \x20   static for (const auto i : N)\n    {\n\
             \x20       auto c = [p]() -> uint8\n        {\n\
             \x20           auto inner = [p]() -> uint8 { return p; };\n\
             \x20           return inner();\n        };\n        r = c();\n    }\n\n\
             \x20   return r;\n}\n",
        );
        let written = lambdas(&before);
        assert_eq!(written.len(), 2);
        let alive = lambdas(&ir);
        assert_eq!(alive.len(), 1, "only the outer lambda survives");
        let funcs = lambda_functions(&ir);
        assert_eq!(funcs.len(), 1, "the inner one was desugared into the survivor");
        // The declaration copies the *inner* lambda's declScopeName — the
        // reference prints ["f","$","$quasi0","$quasi1","?lambda0","$"].
        assert_eq!(
            scope_of(&ir, &interner, funcs[0]),
            scope_of(&before, &interner, written[1].0)
        );
        // The survivor's body was rewritten in place: prologue-free defs
        // first, then the inner lambda's variable now holds a Cast.
        let Exp::Lambda { body, .. } = &ir.arena[alive[0].0].exp else { panic!() };
        let stmts = kids(&ir, *body);
        assert!(matches!(ir.arena[stmts[0]].exp, Exp::Alias { .. }));
        assert!(matches!(
            ir.arena[stmts[1]].exp,
            Exp::Function { kind: FunctionKind::LambdaFunction, .. }
        ));
        let Exp::Variable { init: Some(init), .. } = &ir.arena[stmts[2]].exp else { panic!() };
        assert!(matches!(ir.arena[*init].exp, Exp::Cast { .. }));
    }

    /// A lambda in a function nested inside another function is desugared
    /// exactly once, by the inner function's own clause — 131 corpus lambdas
    /// sit in that position. What this pins is clause 1 firing on the inner
    /// function and the defs landing in *its* body; it deliberately does
    /// **not** claim to pin `lambdaRef FunctionF{}` (`Function.hs:218`) or
    /// `lambdaDefs`' stop at `:232` — planting either away stays green,
    /// because the bottom-up cata leaves no bare lambda inside `g` for the
    /// outer walk or collection to find (see [`LambdaRef::reverts`]).
    /// `--template-passes 2`: the pair lands in `g`'s body
    /// (`declScopeName = ["f","$","g","$"]`) and `f`'s body holds only `g` and
    /// its `return`.
    #[test]
    fn an_inner_functions_lambda_is_desugared_once_by_the_inner_function() {
        let (_, ir, interner) = run(
            "inline uint8 f(uint8 p)\n{\n    inline uint8 g(uint8 q)\n    {\n\
             \x20       auto c = [q]() -> uint8 { return q; };\n        return c();\n    }\n\n\
             \x20   return g(p);\n}\n",
        );
        assert_eq!(lambda_functions(&ir).len(), 1, "exactly one desugaring");
        assert!(lambdas(&ir).is_empty(), "and no lambda survives");
        let f = kids(&ir, ir.root)[0];
        let Exp::Function { body, .. } = &ir.arena[f].exp else { panic!() };
        let f_stmts = kids(&ir, *body);
        assert_eq!(f_stmts.len(), 2, "f's own body gained nothing");
        let Exp::Function { body: g_body, .. } = &ir.arena[f_stmts[0]].exp else { panic!() };
        let g_stmts = kids(&ir, *g_body);
        assert!(matches!(ir.arena[g_stmts[0]].exp, Exp::Alias { .. }));
        assert_eq!(scope_of(&ir, &interner, g_stmts[0]), ["f", "$", "g", "$"]);
    }

    /// A lambda inside a lambda: the inner one's declarations land in the
    /// outer lambda's body (clause 2, bottom-up) and ride into the outer
    /// synthesized function *under* the uncapture prologue — `foldr` prepends
    /// the prologue after the defs are already in place. Numbering restarts in
    /// the outer lambda's named scope, so both are `?lambda0`.
    /// `--template-passes 2`, inside the outer `LambdaFunction`'s body:
    ///
    /// ```text
    /// [0] VariableF {declScopeName = ["f","$","?lambda0","$"], … "p" … "x0" …}
    /// [1] AliasF    {declScopeName = ["f","$","?lambda0","$"], …}
    /// [2] FunctionF {declScopeName = ["f","$","?lambda0","$"], funcKind = LambdaFunction,
    ///                declName = IdentifierF "?lambda0", …}
    /// [3] VariableF {… "inner" …}
    /// [4] ReturnF (…)
    /// ```
    #[test]
    fn a_nested_lambda_lands_inside_the_outer_lambda_function() {
        let (_, ir, interner) = run(
            "inline uint8 f(uint8 p)\n{\n    auto outer = [p]() -> uint8\n    {\n\
             \x20       auto inner = [p]() -> uint8 { return p; };\n\
             \x20       return inner();\n    };\n\n    return outer();\n}\n",
        );
        let funcs = lambda_functions(&ir);
        assert_eq!(funcs.len(), 2);
        // Walk order from the root finds the outer one inside `f` first.
        let outer = funcs
            .iter()
            .copied()
            .find(|&x| scope_of(&ir, &interner, x) == ["f", "$"])
            .expect("the outer lambda's function");
        let Exp::Function { body, .. } = &ir.arena[outer].exp else { panic!() };
        let stmts = kids(&ir, *body);
        assert_eq!(stmts.len(), 5);
        assert!(matches!(ir.arena[stmts[0]].exp, Exp::Variable { .. }), "prologue first");
        assert!(matches!(ir.arena[stmts[1]].exp, Exp::Alias { .. }));
        let Exp::Function { kind, name, .. } = &ir.arena[stmts[2]].exp else { panic!() };
        assert_eq!(*kind, FunctionKind::LambdaFunction);
        assert_eq!(text(&ir, &interner, *name), "?lambda0", "numbering restarts (Parser.hs:122)");
        assert_eq!(scope_of(&ir, &interner, stmts[2]), ["f", "$", "?lambda0", "$"]);
    }

    /// Clause 3 (`Function.hs:194-195`): a module-scope variable's lambda is
    /// wrapped, rewritten, and spliced flat by the parent `Seq`'s
    /// `removeNestedSeq`. `--template-passes 2` on `auto gfn = [](uint8 a) ->
    /// uint8 {…};` at module scope:
    ///
    /// ```text
    /// [0] AliasF    {declScopeName = [], …}
    /// [1] FunctionF {declScopeName = [], funcKind = LambdaFunction, …}
    /// [2] VariableF {declScopeName = [], … varInit = Just CastF …}
    /// ```
    #[test]
    fn a_global_variables_lambda_splices_flat_at_module_scope() {
        let (_, ir, interner) = run(
            "auto gfn = [](uint8 a) -> uint8\n{\n    return a;\n};\n",
        );
        let top = kids(&ir, ir.root);
        assert_eq!(top.len(), 3, "spliced flat, not nested: {top:?}");
        assert!(matches!(ir.arena[top[0]].exp, Exp::Alias { .. }));
        assert!(matches!(
            ir.arena[top[1]].exp,
            Exp::Function { kind: FunctionKind::LambdaFunction, .. }
        ));
        let Exp::Variable { init: Some(init), .. } = &ir.arena[top[2]].exp else { panic!() };
        assert!(matches!(ir.arena[*init].exp, Exp::Cast { .. }));
        assert_eq!(scope_of(&ir, &interner, top[0]), Vec::<String>::new());
    }

    /// **Donor test** (`docs/gates.md`): the `Closure` cast's arguments spell the
    /// same qualifier `f::$` three times with two different donors, and inside
    /// one `ScopedName` the chain and the leaf disagree. Read off `--notes`
    /// for `[p, q]` (CAPS = the capture list's `Src`, NAME = the `declName`'s):
    ///
    /// ```text
    /// arg2: … Nothing IdentifierF "f" <@3:15-3:19#49>) IdentifierF "$" <@3:15-3:19#49>)
    ///         IdentifierF "?lambda0" <@3:14-3:14#48>          -- chain CAPS, leaf NAME
    /// arg3: … Nothing IdentifierF "f" <@3:14-3:14#48>) IdentifierF "$" <@3:14-3:14#48>)
    ///         IdentifierF "?lambda0$capture" <@3:14-3:14#48>  -- chain NAME (addQualifierFrom
    ///         <@3:15-3:19#49>                                 --  donates from the leaf), outer CAPS
    /// ```
    ///
    /// Swapping either donor leaves every structural gate green — `Donated`
    /// nodes only owe "resolves to some CST node" — and fails only this.
    #[test]
    fn the_closure_arguments_share_spelling_but_not_donors() {
        let (before, ir, _) = run(
            "inline uint8 f(uint8 p, uint8 q)\n{\n\
             \x20   auto g = [p, q](uint8 a) -> uint8\n    {\n\
             \x20       return a + p + q;\n    };\n\n    return g(1);\n}\n",
        );
        let (lambda, _) = lambdas(&before).remove(0);
        let Exp::Lambda { name, captures, .. } = &before.arena[lambda].exp else { panic!() };
        let name_src = before.arena[*name].src.head;
        let caps_src = before.arena[*captures].src.head;
        assert_ne!(name_src, caps_src, "the fixture must be able to tell the donors apart");

        let Exp::Cast { ty, .. } = &ir.arena[the_cast(&ir)].exp else { panic!() };
        let Exp::TypeIdentifier(_, inst) = &ir.arena[*ty].exp else { panic!() };
        let Exp::TemplateInstance { args, .. } = &ir.arena[*inst].exp else { panic!() };
        let targs = kids(&ir, *args);

        // arg 2 — `scopedQualifier declScopeName e declScopeName` donates the
        // chain from the capture list; the leaf is the declName (Function.hs:169).
        let Exp::QualifiedIdentifier(sn) = &ir.arena[targs[1]].exp else { panic!() };
        let Exp::ScopedName { prefix, ident, .. } = &ir.arena[*sn].exp else { panic!() };
        assert_eq!(ir.arena[*ident].src.head, name_src, "leaf wears NAME");
        let Exp::ScopedName { ident: dollar, prefix: inner, .. } =
            &ir.arena[prefix.unwrap()].exp
        else {
            panic!()
        };
        assert_eq!(ir.arena[*dollar].src.head, caps_src, "chain wears CAPS");
        let Exp::ScopedName { ident: eff, .. } = &ir.arena[inner.unwrap()].exp else { panic!() };
        assert_eq!(ir.arena[*eff].src.head, caps_src, "chain wears CAPS");

        // arg 3 — `addQualifierFrom` donates the chain from the *appended
        // identifier* (Desugar.hs:196-198), so the same spelling wears NAME;
        // the outer ScopedName wears the capture list's note (`new l`,
        // Function.hs:214).
        let Exp::TypeIdentifier(_, sn3) = &ir.arena[targs[2]].exp else { panic!() };
        assert_eq!(ir.arena[*sn3].src.head, caps_src, "outer ScopedName wears CAPS");
        let Exp::ScopedName { prefix, ident, .. } = &ir.arena[*sn3].exp else { panic!() };
        assert_eq!(ir.arena[*ident].src.head, name_src, "leaf wears NAME");
        let Exp::ScopedName { ident: dollar3, .. } = &ir.arena[prefix.unwrap()].exp else {
            panic!()
        };
        assert_eq!(ir.arena[*dollar3].src.head, name_src, "arg3's chain wears NAME, not CAPS");
    }

    /// **Donor test**: one uncapture initializer mixes three donors
    /// (`Function.hs:256-262`). Read off `--notes` for capture `p` at
    /// `3:15-3:16#49` (C0), lambda at `3:14-6:6#48` (L), declName at
    /// `3:14-3:14#48` (NAME):
    ///
    /// ```text
    /// VariableF {…}                                          <L>
    ///   varAttr AutoF declName                               <L> <C0> <C0>
    ///   varInit = Just MemberAccessF                         <C0>
    ///     NamedValueF                                        <C0>
    ///       QualifiedIdentifierF ScopedNameF …
    ///         IdentifierF "?lambda0$capture_param"           <NAME> <NAME> <NAME>
    ///     IdentifierF "x0"                                   <C0>
    /// ```
    #[test]
    fn the_uncapture_initializer_mixes_three_donors() {
        let (before, ir, _) = run(
            "inline uint8 f(uint8 p, uint8 q)\n{\n\
             \x20   auto g = [p, q](uint8 a) -> uint8\n    {\n\
             \x20       return a + p + q;\n    };\n\n    return g(1);\n}\n",
        );
        let (lambda, caps) = lambdas(&before).remove(0);
        let l_src = before.arena[lambda].src.head;
        let c0_src = before.arena[caps[0]].src.head;
        let Exp::Lambda { name, .. } = &before.arena[lambda].exp else { panic!() };
        let name_src = before.arena[*name].src.head;
        // Divergence #26: our `declName` wears the whole lambda's range, so
        // NAME and L coincide here where the reference holds a zero-width
        // point. The C0-vs-NAME and C0-vs-L seams are still real; the
        // NAME-vs-L seam (the Variable against its initializer's spine) is
        // not separable in this IR, and a donor swap between those two is
        // invisible by construction — the falsifiable plants are swaps toward
        // C0 or the capture list.
        assert_eq!(name_src, l_src, "divergence #26 — update this test if the ranges split");
        assert_ne!(c0_src, l_src);
        assert_ne!(c0_src, name_src);

        let func = lambda_functions(&ir)[0];
        let Exp::Function { body, .. } = &ir.arena[func].exp else { panic!() };
        let var = kids(&ir, *body)[0];
        assert_eq!(ir.arena[var].src.head, l_src, "the Variable wears L (:256)");
        let Exp::Variable { ty, init: Some(init), .. } = &ir.arena[var].exp else { panic!() };
        assert_eq!(ir.arena[*ty].src.head, c0_src, "AutoF wears C0 (:257)");
        assert_eq!(ir.arena[*init].src.head, c0_src, "MemberAccess wears C0 (:261)");
        let Exp::MemberAccess { base, member } = &ir.arena[*init].exp else { panic!() };
        assert_eq!(ir.arena[*member].src.head, c0_src, "x0 wears C0 (:261)");
        assert_eq!(ir.arena[*base].src.head, c0_src, "NamedValue wears C0 (:262)");
        let Exp::NamedValue(qi) = &ir.arena[*base].exp else { panic!() };
        assert_eq!(
            ir.arena[*qi].src.head, name_src,
            "localIdentifier donates from captureParamName — NAME (Type.hs:986-989)"
        );
    }

    /// The `written` half of the donor rule. The cast is the node standing in
    /// for the lambda's text, so it keeps `Own` at the lambda's key; the
    /// declarations are inventions, `Donated`; the lambda's written `declName`
    /// and return type live on, by identity, in the synthesized function; and
    /// of the two structurally identical `make_captureN` calls only the
    /// cast's argument `Seq` keeps `Own` at the capture list's key.
    #[test]
    fn the_cast_keeps_own_and_the_written_nodes_live_on() {
        let (before, ir, interner) = run(
            "inline uint8 f(uint8 p, uint8 q)\n{\n\
             \x20   auto g = [p, q](uint8 a) -> uint8\n    {\n\
             \x20       return a + p + q;\n    };\n\n    return g(1);\n}\n",
        );
        let (lambda, caps) = lambdas(&before).remove(0);
        let Exp::Lambda { name, captures, ret, .. } = before.arena[lambda].exp.clone() else {
            panic!()
        };

        let cast = the_cast(&ir);
        assert_eq!(ir.arena[cast].src.head, before.arena[lambda].src.head);
        assert_eq!(ir.arena[cast].src.written, Written::Own, "replace keeps Own");
        assert_eq!(ir.arena[cast].kind, before.arena[lambda].kind);

        let func = lambda_functions(&ir)[0];
        assert_eq!(ir.arena[func].src.written, Written::Donated);
        let Exp::Function { name: fname, ret: fret, .. } = &ir.arena[func].exp else { panic!() };
        assert_eq!(*fname, name, "declName reused by identity (Function.hs:248)");
        assert_eq!(*fret, ret, "funcReturnType reused by identity (Function.hs:248)");

        // The two calls: the cast's argument Seq stands in for the written
        // capture list (Own at its key, and it holds the written name nodes);
        // the alias's is a Donated twin holding copies.
        let Exp::Cast { value, .. } = &ir.arena[cast].exp else { panic!() };
        assert_eq!(ir.arena[*value].src.head, before.arena[captures].src.head);
        assert_eq!(ir.arena[*value].src.written, Written::Donated, "`new capture` (:161)");
        let Exp::FunctionCall { args: cast_args, .. } = &ir.arena[*value].exp else { panic!() };
        assert_eq!(ir.arena[*cast_args].src.written, Written::Own);
        let Exp::Capture { name: p_name, .. } = &before.arena[caps[0]].exp else { panic!() };
        let cast_arg0 = kids(&ir, *cast_args)[0];
        let Exp::NamedValue(qi) = &ir.arena[cast_arg0].exp else { panic!() };
        let Exp::QualifiedIdentifier(sn) = &ir.arena[*qi].exp else { panic!() };
        let Exp::ScopedName { ident, .. } = &ir.arena[*sn].exp else { panic!() };
        assert_eq!(*ident, *p_name, "the written capture name is the cast's leaf (:157)");

        let alias = kids(&ir, *find_alias_holder(&ir))[0];
        let (_, _, alias_args) = capture_call(&ir, &interner, alias);
        let Exp::NamedValue(aqi) = &ir.arena[alias_args[0]].exp else { panic!() };
        let Exp::QualifiedIdentifier(asn) = &ir.arena[*aqi].exp else { panic!() };
        let Exp::ScopedName { ident: aleaf, .. } = &ir.arena[*asn].exp else { panic!() };
        assert_ne!(*aleaf, *p_name, "the alias's leaf is a copy");
        assert_eq!(ir.arena[*aleaf].src.head, before.arena[*p_name].src.head);
        assert_eq!(ir.arena[*aleaf].src.written, Written::Donated);
    }

    /// A file with no lambda allocates nothing: the drivers' `out == original
    /// → keep the id` rule makes an untouched tree free, and the reference's
    /// global-variable wrap (`Function.hs:194-195`) is elided here exactly so
    /// that this stays true for every lambda-free global.
    #[test]
    fn a_file_with_no_lambdas_is_not_rebuilt() {
        let (before, after, _) = run(
            "const uint8 gv = 3;\n\ninline uint8 f(uint8 p)\n{\n    return p;\n}\n",
        );
        assert_eq!(before.root, after.root);
        assert_eq!(before.arena.len(), after.arena.len(), "no node was appended");
    }

    /// Provenance clause four over the full composed [`pipeline`] — no
    /// appended `Own` node may be a structural copy of a lowered node at its
    /// own key. The probes cover every clause of `expand` and both reverts.
    #[test]
    fn no_appended_own_node_is_a_copy_of_a_lowered_one() {
        for probe in PROBES {
            let interner = Interner::new();
            let parse = kg_syntax::parse_file(probe);
            assert!(parse.errors().is_empty());
            let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
            let ir = lower(&file, FileId(0), &interner);
            let out = pipeline(&ir, &interner);
            crate::rewrite::abbreviated::own_nodes_are_not_copies(&out, &interner)
                .unwrap_or_else(|e| panic!("{probe}: {e}"));
        }
    }

    /// The whole-corpus invariant, exercised on every probe so it fails here
    /// before `xtask ir` sees it.
    #[test]
    fn the_invariant_holds_on_every_probe() {
        for probe in PROBES {
            let (before, after, interner) = run(probe);
            lambda_functions_match_lambda_sites(&before, &after, &interner)
                .unwrap_or_else(|e| panic!("{probe}: {e}"));
        }
    }

    const PROBES: &[&str] = &[
        "inline uint8 f()\n{\n    auto g = [](uint8 a) -> uint8\n    {\n\
         \x20       return a;\n    };\n\n    return g(1);\n}\n",
        "inline uint8 f(uint8 p, uint8 q)\n{\n\
         \x20   auto g = [p, q](uint8 a) -> uint8\n    {\n\
         \x20       return a + p + q;\n    };\n\n    return g(1);\n}\n",
        "inline void h()\n{\n    class Helper\n    {\n    private:\n\
         \x20       uint8 _x;\n\n    public:\n        void test()\n        {\n\
         \x20           _x = 4;\n\n            auto cb = [] () -> uint8\n\
         \x20           {\n                return _x;\n            };\n        }\n\
         \x20   }\n}\n",
        "inline uint8 f(uint8 p)\n{\n    auto g = [p](auto a) -> uint8\n    {\n\
         \x20       return a + p;\n    };\n\n    return g(1);\n}\n",
        "inline uint8 f(uint8 p)\n{\n    uint8 r = 0;\n\n    if (p > 1)\n    {\n\
         \x20       auto c = [p]() -> uint8 { return p; };\n        r = c();\n    }\n\n\
         \x20   return r;\n}\n",
        "template <auto B>\ninline uint8 f(uint8 p)\n{\n    static if (B)\n    {\n\
         \x20       auto c = [p]() -> uint8 { return p; };\n        return c();\n    }\n\
         \x20   else\n    {\n        return p;\n    }\n}\n",
        "template <auto N>\ninline uint8 f(uint8 p)\n{\n    uint8 r = 0;\n\n\
         \x20   static for (const auto i : N)\n    {\n\
         \x20       auto c = [p]() -> uint8 { return p; };\n        r = c();\n    }\n\n\
         \x20   return r;\n}\n",
        "template <auto N>\ninline uint8 f(uint8 p)\n{\n    uint8 r = 0;\n\n\
         \x20   static for (const auto i : N)\n    {\n\
         \x20       auto c = [p]() -> uint8\n        {\n\
         \x20           auto inner = [p]() -> uint8 { return p; };\n\
         \x20           return inner();\n        };\n        r = c();\n    }\n\n\
         \x20   return r;\n}\n",
        "inline uint8 f(uint8 p)\n{\n    inline uint8 g(uint8 q)\n    {\n\
         \x20       auto c = [q]() -> uint8 { return q; };\n        return c();\n    }\n\n\
         \x20   return g(p);\n}\n",
        "inline uint8 f(uint8 p)\n{\n    auto outer = [p]() -> uint8\n    {\n\
         \x20       auto inner = [p]() -> uint8 { return p; };\n\
         \x20       return inner();\n    };\n\n    return outer();\n}\n",
        "auto gfn = [](uint8 a) -> uint8\n{\n    return a;\n};\n",
        "const uint8 gv = 3;\n\ninline uint8 f(uint8 p)\n{\n    return p;\n}\n",
    ];
}
