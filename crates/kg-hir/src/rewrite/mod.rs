//! Pass 8's rewriting substrate: the reference's recursion schemes over an
//! arena, and the one rule that decides `Src::written` for a node a rewrite
//! makes.
//!
//! `instantiateTemplates` runs nine inner sub-passes (`Template.hs:71-80`),
//! five of which rewrite the tree. All five are written against the same three
//! combinators — `desugar` (a `cata`), `desugarShallow'` (a `para` whose
//! algebra may decline) and `removeNestedSeq` — so this module ports those
//! once instead of letting each pass carry its own walk.
//!
//! # One arena, appended to
//!
//! A module's sorted `ModuleIr` (`kg_db::sorted`) is [`ModuleIr::fork`]ed
//! once at the top of a pass-8 run and every sub-pass appends into that
//! fork. A fresh arena per
//! sub-pass rebuilds nine whole-file copies per round, and a literal `cata`
//! reallocates a subtree at every site even where nothing changed. So the
//! driver's rule is: **a node keeps its id iff the algebra's result equals its
//! original `Exp`.** [`Exp`] is `PartialEq` (`exp.rs:82`), so that is exact,
//! and it makes an unchanged subtree free — the identity Haskell gets from
//! sharing for nothing.
//!
//! # `written`, decided once
//!
//! The field is not in the reference, which cannot tell a node's own text from
//! a range it was handed and does not need to. Here it is the claim that
//! `(head, kind)` is a legal `TypeKey`, so it has one rule:
//!
//! > `Own` iff the node is the sole claimant of that `(range, kind)` in the
//! > tree it lives in.
//!
//! Mechanically, that is three constructors and nothing else:
//!
//! * [`Rewriter::replace`] — a node standing in for one that goes unreachable
//!   in the same step. Keeps the original's `Src` and `kind` verbatim, `Own`
//!   included, because a replacement leaves no second claimant. The driver
//!   uses it for every rebuild.
//! * [`Rewriter::donate`] — a node the algebra invented. `newNote`
//!   (`Type.hs:983-984`), which hands a new node a donor's `Src` verbatim;
//!   here it also marks it `Donated`, so no position question is answered from
//!   it.
//! * [`Rewriter::deep_copy`] — a subtree that must appear in **two** positions,
//!   which an arena cannot do without a DAG. The copy is `Donated` throughout.
//!
//! Four independent specifications of the five sub-passes each derived their
//! own locally-correct answer for `written`; the rule lives here so a fifth is
//! not derived. Note the deliberate asymmetry with `Src::expanded`
//! (`kg-base/src/src.rs:76-88`): an *instantiated* copy of a body node keeps
//! `written`, because it is the user's own text compiled again. A rewrite's
//! copy is not.
//!
//! # `None` reverts, and reverts top-down
//!
//! `desugarShallow' f = para f'` with `f' e' (Note n e) = maybe e' (NotedExp n)
//! (f n e)` (`Desugar.hs:81-84`) hands back `para`'s **first** argument — the
//! original, unrewritten subtree (`Recursion.hs:73-74`) — so a declining arm
//! discards its children's rewrites too, however deep they were. In Haskell
//! those rewrites are simply never forced; here, computing and discarding them
//! would leave superseded nodes in the arena, so [`ShallowRewrite::reverts`] is
//! asked *before* descending. Same answer, cheaper, and — the reason it is a
//! combinator rather than an algebra option — it is the only reading that is:
//! "skip this node but keep the rewritten children" would hoist declarations
//! out of templates, `static if`s and local classes.

pub mod abbreviated;
pub mod capture_this;
pub mod higher_order;
pub mod instantiate;
pub mod lambdas;
pub mod move_functions;
pub mod resolve;
mod static_for;
pub use static_for::stands_unrolled;

use kg_base::{ExpansionInterner, Interner, Src, Symbol};
use kg_ty::QualifiedName;

pub use abbreviated::{is_abbreviated_template, AbbreviatedFunctionTemplate};
pub use capture_this::CaptureThis;
pub use instantiate::{decl_scope_of, deduplicate_instances, deduplicate_instances_in_place, remove_nested_type_identifier, InstanceSpec, InstantiateReport, InstantiateTemplates, TemplateInstances};
pub use lambdas::LambdaRef;
pub use move_functions::{Declarations, MoveFunctions};
pub use resolve::{fingerprint_of, mangle_args, DecidedInstances, DecidedSite, Fingerprint, ResolveInstances, Site};

use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;
use crate::ModuleIr;

/// The arena a rewrite appends into, plus the interner its synthesized names
/// need. `Interner::intern` takes `&self` (`kg-base/src/interner.rs:49`), so a
/// rewrite never needs it mutably.
#[derive(Debug)]
pub struct Rewriter<'a> {
    pub arena: &'a mut ExpArena,
    pub interner: &'a Interner,
    /// The combinators' child lists, one stack for the whole recursion:
    /// every node of every pass is visited, and a list per node was a
    /// tenth of a whole-program check.
    kids: Vec<ExpId>,
}

impl<'a> Rewriter<'a> {
    pub fn new(arena: &'a mut ExpArena, interner: &'a Interner) -> Rewriter<'a> {
        Rewriter { arena, interner, kids: Vec::new() }
    }

    /// A node standing in for `original`, which the same step makes
    /// unreachable — see the module comment.
    pub fn replace(&mut self, original: ExpId, exp: Exp) -> ExpId {
        let node = &self.arena[original];
        let (src, kind) = (node.src, node.kind);
        self.arena.alloc_for(original, exp, src, kind)
    }

    /// A node the algebra invented, wearing `donor`'s range and kind and marked
    /// `Donated` — `newNote` (`Type.hs:983-984`).
    pub fn donate(&mut self, donor: ExpId, exp: Exp) -> ExpId {
        let node = &self.arena[donor];
        let (src, kind) = (Src::donated(node.src), node.kind);
        self.arena.alloc(exp, src, kind)
    }

    /// [`Rewriter::donate`] for a node that stands in for `original` and
    /// keeps its note — the reference's `NotedExp n x` over the node's own
    /// `n` — where provenance wants it `Donated` all the same: recorded in
    /// the arena's lineage like a [`Rewriter::replace`].
    pub fn donate_keeping_note(&mut self, original: ExpId, exp: Exp) -> ExpId {
        let node = &self.arena[original];
        let (src, kind) = (Src::donated(node.src), node.kind);
        self.arena.alloc_for(original, exp, src, kind)
    }

    /// A structural copy, `Donated` throughout.
    pub fn deep_copy(&mut self, id: ExpId) -> ExpId {
        self.arena.deep_copy(id)
    }

    /// `suffixName` (`Desugar.hs:164-165`, `mapName (<> x)`).
    pub fn suffix_name(&self, name: kg_base::Symbol, suffix: &str) -> kg_base::Symbol {
        self.interner.intern(&format!("{}{suffix}", self.interner.resolve(name)))
    }

    /// `removeNestedSeq` (`Desugar.hs:132-137`): splice a `Seq`'s `Seq`
    /// children into it, **one level**. Every rewriting sub-pass ends with
    /// this, because every one of them emits `SeqF [a, b]` where one
    /// declaration stood; composed bottom-up over a subtree the one level
    /// becomes total, which is why the reference never needs a recursive
    /// version and why porting one would diverge if it were ever applied
    /// standalone.
    ///
    /// A `{ … }` block is **not** a bare `Seq` — it is `NestedScope` with a
    /// `Seq` body (`Parser.hs:467-468`) — so this does not collapse scopes.
    pub fn remove_nested_seq(&self, exp: Exp) -> Exp {
        let Exp::Seq(items) = exp else { return exp };
        let mut out: Vec<ExpId> = Vec::with_capacity(items.len());
        for id in items {
            match &self.arena[id].exp {
                Exp::Seq(inner) => out.extend(inner.iter().copied()),
                _ => out.push(id),
            }
        }
        Exp::Seq(out)
    }
}

/// `modifyDeclScopeName fn` (`Desugar.hs:139-162`), arm for arm: the fifteen
/// `declScopeName` records, then `TypeIdentifierF`/`TypeParamF`/`ScopedNameF`
/// whose scope is positional. `TemplateParamF` is deliberately absent there
/// and therefore here. `f` returns `None` for "leave it alone" so an
/// untouched node compares equal and keeps its id.
pub fn modify_decl_scope_name(
    mut exp: Exp,
    f: &mut impl FnMut(&QualifiedName) -> Option<QualifiedName>,
) -> Exp {
    {
        let slot: Option<&mut QualifiedName> = match &mut exp {
            Exp::Capture { scope, .. }
            | Exp::FuncParam { scope, .. }
            | Exp::Struct { scope, .. }
            | Exp::EnumConstant { scope, .. }
            | Exp::Enum { scope, .. }
            | Exp::Union { scope, .. }
            | Exp::Alias { scope, .. }
            | Exp::Class { scope, .. }
            | Exp::Variable { scope, .. }
            | Exp::Function { scope, .. }
            | Exp::This { scope }
            | Exp::NestedScope { scope, .. }
            | Exp::Decltype { scope, .. }
            | Exp::Lambda { scope, .. }
            | Exp::FunctionDecl { scope, .. }
            | Exp::TypeIdentifier(scope, _)
            | Exp::TypeParam { scope, .. }
            | Exp::ScopedName { scope, .. } => Some(scope),
            _ => None,
        };
        if let Some(scope) = slot
            && let Some(next) = f(scope)
        {
            *scope = next;
        }
    }
    exp
}

/// `scopedQualifier` (`Desugar.hs:182-185`): a qualifier as a left-nested
/// `ScopedName` chain, innermost first, every node `copyNote`d from `donor`.
/// `None` for an empty qualifier — an unqualified name has no prefix at all.
///
/// Lifted out of `lambdas.rs` when `move_functions` became its second caller:
/// `qualifyName` (`Function.hs:75`) and `fixScopedName` (`:81`) build these
/// chains for every reference the move re-roots.
pub fn scoped_qualifier(
    rw: &mut Rewriter,
    scope: &QualifiedName,
    donor: ExpId,
    names: &[Symbol],
) -> Option<ExpId> {
    let mut prev: Option<ExpId> = None;
    for &component in names {
        let ident = rw.donate(donor, Exp::Identifier(component));
        prev = Some(rw.donate(
            donor,
            Exp::ScopedName { scope: scope.clone(), prefix: prev, ident },
        ));
    }
    prev
}

/// `localIdentifier` (`Type.hs:986-989`): a `QualifiedIdentifier` around an
/// unqualified `ScopedName`, both donated by the identifier they wrap.
///
/// Lifted out of `lambdas.rs` when `move_functions` became its second caller:
/// `thisNamedValue` (`Type.hs:991-994`) wraps the `$this` leaf with it.
pub fn local_identifier(
    rw: &mut Rewriter,
    scope: &QualifiedName,
    ident: ExpId,
) -> ExpId {
    let sn = rw.donate(ident, Exp::ScopedName { scope: scope.clone(), prefix: None, ident });
    rw.donate(ident, Exp::QualifiedIdentifier(sn))
}

/// `Identifier`'s payload, `None` for any other node. Promoted from
/// `move_functions.rs` when `resolve` became its second caller.
pub fn ident_sym(arena: &ExpArena, id: ExpId) -> Option<Symbol> {
    match &arena[id].exp {
        Exp::Identifier(s) => Some(*s),
        _ => None,
    }
}

/// `getQualifier` (`Parser/Syntax.hs:746-765`), name-node arms only: the
/// segments a written prefix spells, empty for an unprefixed name. Promoted
/// from `move_functions.rs` when `resolve` became its second caller — its
/// fingerprint of an instance's base is this chain plus [`get_name`]'s leaf.
pub fn get_qualifier(arena: &ExpArena, id: ExpId) -> Option<Vec<Symbol>> {
    match &arena[id].exp {
        Exp::Identifier(_) => Some(Vec::new()),
        Exp::ScopedName { prefix: None, .. } => Some(Vec::new()),
        Exp::ScopedName { prefix: Some(p), .. } => {
            let mut q = get_qualifier(arena, *p)?;
            q.push(get_name(arena, *p)?);
            Some(q)
        }
        Exp::TypeIdentifier(_, a)
        | Exp::QualifiedIdentifier(a)
        | Exp::NamedValue(a)
        | Exp::TemplateInstance { base: a, .. }
        | Exp::FunctionSpecifier { function: a, .. } => get_qualifier(arena, *a),
        _ => None,
    }
}

/// `getName` (`Parser/Syntax.hs:727-744`), name-node arms only. Promoted from
/// `move_functions.rs` when `resolve` became its second caller.
pub fn get_name(arena: &ExpArena, id: ExpId) -> Option<Symbol> {
    match &arena[id].exp {
        Exp::Identifier(s) => Some(*s),
        Exp::ScopedName { ident, .. } => get_name(arena, *ident),
        Exp::TypeIdentifier(_, a)
        | Exp::QualifiedIdentifier(a)
        | Exp::NamedValue(a)
        | Exp::TemplateInstance { base: a, .. }
        | Exp::FunctionSpecifier { function: a, .. } => get_name(arena, *a),
        _ => None,
    }
}

/// `desugarShallow'` (`Desugar.hs:81-84`) as a trait, so a pass's declining
/// arms and its rewriting arms stay in one place and neither can be read
/// without the other.
pub trait ShallowRewrite {
    /// The `Nothing` arms. Asked **before** descending — see the module
    /// comment for why that is the same answer as `para`'s revert.
    ///
    /// Must depend only on the node's own `Exp`: a guard that reads a child's
    /// *rewritten* shape is not equivalent to the bottom-up form.
    fn reverts(&self, arena: &ExpArena, id: ExpId) -> bool;

    /// The `Just` arms. `exp` is the node with its children already rewritten;
    /// `id` is the original, for `Src` donation and for the caller's own
    /// `deep_copy`.
    fn rewrite(&mut self, rw: &mut Rewriter, id: ExpId, exp: Exp) -> Exp;

    /// The shapes ([`crate::facts`]) outside which [`ShallowRewrite::rewrite`]
    /// hands every node back as it came, doing nothing else: a subtree
    /// holding none of them is passed over whole, as [`desugar_within`]
    /// passes one over. `None`, by default: every node is visited.
    fn needs(&self) -> Option<crate::facts::Flags> {
        None
    }
}

/// Run a [`ShallowRewrite`] over the subtree at `root`, returning its new id.
pub fn desugar_shallow(rw: &mut Rewriter, root: ExpId, pass: &mut impl ShallowRewrite) -> ExpId {
    kg_base::cancel::checkpoint_strided();
    if pass.reverts(rw.arena, root) || pass.needs().is_some_and(|needs| rw.arena.lacks(root, needs)) {
        return root;
    }
    // The arena is appended to, never rewritten, so the original is still
    // `root`'s own expression to compare with: one clone, not two.
    let rebuilt = rebuild(rw, root, &mut |rw, kid| desugar_shallow(rw, kid, pass));
    let out = pass.rewrite(rw, root, rebuilt);
    if out == rw.arena[root].exp {
        return root;
    }
    rw.replace(root, out)
}

/// `root`'s expression with each child replaced by `each`'s answer for it,
/// children in `Exp::children` order. The ids go through the rewriter's
/// one stack, and the slots are written only when an answer differs.
fn rebuild(rw: &mut Rewriter, root: ExpId, each: &mut dyn FnMut(&mut Rewriter, ExpId) -> ExpId) -> Exp {
    let base = rw.kids.len();
    rw.arena[root].exp.children(&mut rw.kids);
    let mut changed = false;
    for i in base..rw.kids.len() {
        let kid = rw.kids[i];
        let answer = each(rw, kid);
        if answer != kid {
            rw.kids[i] = answer;
            changed = true;
        }
    }
    let mut rebuilt = rw.arena[root].exp.clone();
    if changed {
        let slots = rebuilt.children_mut();
        debug_assert_eq!(slots.len(), rw.kids.len() - base, "children and children_mut list the same slots");
        for (slot, &answer) in slots.into_iter().zip(&rw.kids[base..]) {
            *slot = answer;
        }
    }
    rw.kids.truncate(base);
    rebuilt
}

/// `desugar'` (`Desugar.hs:71-74`) — a `cata` whose algebra sees the note.
/// No declining arm: every node is visited and every node may be replaced.
pub fn desugar(
    rw: &mut Rewriter,
    root: ExpId,
    f: &mut impl FnMut(&mut Rewriter, ExpId, Exp) -> Exp,
) -> ExpId {
    kg_base::cancel::checkpoint_strided();
    let rebuilt = rebuild(rw, root, &mut |rw, kid| desugar(rw, kid, f));
    let out = f(rw, root, rebuilt);
    if out == rw.arena[root].exp {
        return root;
    }
    rw.replace(root, out)
}

/// [`desugar`] for an algebra that hands back, as it came and doing nothing
/// else, every node outside the shapes `needs` ([`crate::facts`]): a
/// subtree holding none of them is the cata's identity, and is passed over
/// whole where the arena keeps its facts ([`ExpArena::track_facts`]) —
/// the cost of a pass that rewrites a few calls, rather than of the
/// program it walks.
pub fn desugar_within(
    rw: &mut Rewriter,
    root: ExpId,
    needs: crate::facts::Flags,
    f: &mut impl FnMut(&mut Rewriter, ExpId, Exp) -> Exp,
) -> ExpId {
    debug_assert!(needs != 0, "a pass that needs no shape is the identity everywhere");
    if rw.arena.lacks(root, needs) {
        return root;
    }
    kg_base::cancel::checkpoint_strided();
    let rebuilt = rebuild(rw, root, &mut |rw, kid| desugar_within(rw, kid, needs, f));
    let out = f(rw, root, rebuilt);
    if out == rw.arena[root].exp {
        return root;
    }
    rw.replace(root, out)
}

/// `parallel $ desugarShallow' abbreviatedFunctionTemplate` (`Template.hs:77`)
/// over a fork of `ir`.
///
/// The program-wide loop runs it with the others each round
/// (`kg_sem::elaborate`); this door is `xtask ir`'s and `kg dump ir
/// --rewrite`'s.
pub fn abbreviated_function_template(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    let mut out = ir.fork();
    let mut pass = AbbreviatedFunctionTemplate::default();
    let mut rw = Rewriter::new(&mut out.arena, interner);
    out.root = desugar_shallow(&mut rw, ir.root, &mut pass);
    out
}

/// `passPar $ desugar $ captureThis . reifyEnums` (`Frontend.hs:72`) over a
/// fork of `ir`. Frontend pass 3 — it runs five passes *before* the one above,
/// which is why [`pipeline`] exists and why neither entry point is the door a
/// caller should reach for.
/// `higherOrderFunctions` (`Function.hs:313-361`), frontend pass 2 — see
/// [`higher_order`].
pub fn higher_order_functions(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    let mut out = ir.fork();
    let mut rw = Rewriter::new(&mut out.arena, interner);
    out.root = higher_order::apply(&mut rw, ir.root);
    out
}

pub fn capture_this(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    let mut out = ir.fork();
    let mut rw = Rewriter::new(&mut out.arena, interner);
    out.root = capture_this::apply(&mut rw, ir.root);
    out
}

/// `parallel $ desugarLambdas` (`Template.hs:78`) over a fork of `ir` —
/// execution sub-pass 2 of frontend pass 8, one *before*
/// [`abbreviated_function_template`], which is what turns a generic lambda's
/// synthesized `FunctionKind::LambdaFunction` into the abbreviated template
/// the reference fires on. Expects `captureThis` to have run already: the
/// capture arity this pass reads includes the implicit `This`.
pub fn desugar_lambdas(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    let mut out = ir.fork();
    let mut rw = Rewriter::new(&mut out.arena, interner);
    out.root = lambdas::apply(&mut rw, ir.root);
    out
}

/// `parallel $ moveFunctions` (`Template.hs:76`) over a fork of `ir` —
/// execution sub-pass 4 of frontend pass 8, one *after*
/// [`abbreviated_function_template`], whose leftover half is exactly what this
/// pass's abbreviated-`Function` defer (`Function.hs:89-90`) exists to skip.
/// `--template-passes 3` vs `4` isolates it on the oracle.
pub fn move_functions(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    let mut out = ir.fork();
    let mut pass = MoveFunctions::new(interner);
    let mut rw = Rewriter::new(&mut out.arena, interner);
    out.root = desugar_shallow(&mut rw, ir.root, &mut pass);
    out
}

/// `parallel $ desugar (instantiate . resolve)` (`Template.hs:58`), the
/// `resolve` half only, over a fork of `ir` — the per-round rewrite that runs
/// *after* the nine sub-passes, not among them. `instantiate` materialises
/// declarations and is a later slice (blocked on `TypeKey`'s expansion
/// discriminator); this half rewrites decided *mentions* into their mangled
/// names and touches nothing else.
///
/// `decided` is an input parameter, exactly as `moveFunctions` takes a symbol
/// map — the caller (xtask's gate, later `Db`) derives it from
/// `kg_db::instances`, and `kg-hir` stays free of `kg-db`. Not part of
/// [`pipeline`], whose signature has no decided map and whose callers grade
/// the sub-pass window alone.
pub fn resolve_instances(
    ir: &ModuleIr,
    interner: &Interner,
    decided: &DecidedInstances,
) -> ModuleIr {
    let mut out = ir.fork();
    // No site in this module means nothing can seed a resolution — here,
    // unseeded, spread fingerprints only grow from this module's own sites
    // (the IR loop seeds across modules, `resolve_instances_seeded_in_place`)
    // — so the walks are skipped, the way `moveFunctions`' `null selected`
    // skip is exact.
    if decided.keys().any(|site| site.head.file == ir.file) {
        let mut pass = ResolveInstances::new(decided);
        let mut rw = Rewriter::new(&mut out.arena, interner);
        out.root = pass.apply(&mut rw, ir.root);
    }
    out
}

/// [`resolve_instances`] without its shortcut: a module whose tree holds
/// nodes carried in from another file — a template parameter's default,
/// substituted where the instance is written — has sites whose head is not
/// its own file, so every module is walked (the IR loop, `kg_sem`).
pub fn resolve_instances_everywhere(ir: &ModuleIr, interner: &Interner, decided: &DecidedInstances) -> ModuleIr {
    let mut out = ir.fork();
    resolve_instances_everywhere_in_place(&mut out, interner, decided);
    out
}

/// [`resolve_instances_everywhere`] appending into `ir`'s own arena — the
/// arena is append-only, so the tree is the same, without a copy of the
/// module.
pub fn resolve_instances_everywhere_in_place(ir: &mut ModuleIr, interner: &Interner, decided: &DecidedInstances) {
    if !decided.is_empty() {
        let mut pass = ResolveInstances::new(decided);
        let root = ir.root;
        let mut rw = Rewriter::new(&mut ir.arena, interner);
        ir.root = pass.apply(&mut rw, root);
    }
}

/// [`resolve_instances_everywhere_in_place`] with the round's decisions in
/// every module as spread seeds ([`ResolveInstances::seeded`]): the IR
/// loop's `resolve`, whose decided map is the whole program's
/// (`Template.hs:58,158-159`).
pub fn resolve_instances_seeded_in_place(
    ir: &mut ModuleIr,
    interner: &Interner,
    decided: &DecidedInstances,
    seeds: &kg_base::FxHashMap<resolve::Fingerprint, DecidedSite>,
) {
    resolve_instances_seeded_logged_in_place(ir, interner, decided, seeds, None);
}

/// [`resolve_instances_seeded_in_place`], each spread match it makes
/// hashed into `log` where one is given ([`ResolveInstances::logged`]).
pub fn resolve_instances_seeded_logged_in_place(
    ir: &mut ModuleIr,
    interner: &Interner,
    decided: &DecidedInstances,
    seeds: &kg_base::FxHashMap<resolve::Fingerprint, DecidedSite>,
    log: Option<&mut Vec<u64>>,
) {
    if !decided.is_empty() || !seeds.is_empty() {
        let mut pass = ResolveInstances::seeded(decided, seeds.clone());
        if log.is_some() {
            pass = pass.logged();
        }
        let root = ir.root;
        let mut rw = Rewriter::new(&mut ir.arena, interner);
        ir.root = pass.apply(&mut rw, root);
        if let Some(log) = log {
            log.extend(pass.take_log());
        }
    }
}

/// `isLocalScope` (`Desugar.hs:176-180`): the scope's last component starts
/// with `$` — a function body, or a block inside one.
pub fn is_local_scope(interner: &Interner, scope: &QualifiedName) -> bool {
    scope.segments().last().is_some_and(|&s| interner.resolve(s).starts_with('$'))
}

/// `staticControl` (`Constexpr.hs:41-56`) at one node whose children are
/// already rewritten, as the reference's inference applies it
/// (`Inference.hs:47,50`, after `evaluateConstexpr`): the `static if`'s
/// condition is a literal by then, so nothing here folds. The loop's
/// inference is its one caller (`kg_sem::infer`) — the one place a `static
/// if` or a `static for` is decided (`single-source-of-truth.md` D2.4).
///
/// A loop whose variable reaches no constant position is the reference's
/// `_ -> UnrolledForF a b c` (`Constexpr.hs:54`): `decided` walks an
/// `UnrolledForF`'s body where it stops at a `StaticForF`'s limit
/// (`Template.hs:106`).
pub fn static_control_node(rw: &mut Rewriter, id: ExpId, exp: Exp, expansions: Option<&ExpansionInterner>) -> Exp {
    match static_eval(rw, id, exp, expansions) {
        Exp::StaticFor { var, limit, body } if static_for::stands_unrolled(rw.arena, var, body) => {
            Exp::UnrolledFor { var, limit, body }
        }
        other => other,
    }
}

fn static_eval(rw: &mut Rewriter, id: ExpId, exp: Exp, expansions: Option<&ExpansionInterner>) -> Exp {
    match exp {
        // `eval (StaticForF a b c)` (`Constexpr.hs:48-56`) — see
        // [`static_for`].
        Exp::StaticFor { var, limit, body } => {
            static_for::unroll(rw, id, var, limit, body, expansions).unwrap_or(Exp::StaticFor { var, limit, body })
        }
        // `eval (StaticIfF (BoolLiteral b) …)` (`Constexpr.hs:43-47`).
        Exp::StaticIf { cond, then_branch, else_branch } => {
            let Exp::BoolLiteral(taken) = rw.arena[cond].exp else {
                return Exp::StaticIf { cond, then_branch, else_branch };
            };
            let chosen = if taken { Some(then_branch) } else { else_branch };
            match chosen {
                None => Exp::Seq(Vec::new()),
                Some(branch) => remove_nested_scope(rw, branch),
            }
        }
        Exp::Seq(_) => rw.remove_nested_seq(exp),
        other => other,
    }
}

/// `removeNestedScope` (`Constexpr.hs:358-370`): a braced branch's body,
/// every declaration scope under the branch's `$quasiN` taken out of it, a
/// name chain through the `$quasiN` link joined around it — so a member
/// declared under `static if` in a class is keyed `[Foo, Do]`, where a
/// hoisted lambda's `memberAccess` finds it (`this-capture.k::17`). An
/// unbraced branch is itself.
fn remove_nested_scope(rw: &mut Rewriter, branch: ExpId) -> Exp {
    let Exp::NestedScope { scope, name, body } = rw.arena[branch].exp.clone() else {
        return rw.arena[branch].exp.clone();
    };
    let Some(leaf) = ident_sym(rw.arena, name) else { return rw.arena[body].exp.clone() };
    let mut nested: Vec<Symbol> = scope.segments().to_vec();
    nested.push(leaf);
    let n = nested.len();
    let strip = |s: &QualifiedName| -> Option<QualifiedName> {
        let segments = s.segments();
        segments.starts_with(&nested).then(|| {
            QualifiedName::new(segments[..n - 1].iter().chain(&segments[n..]).copied())
        })
    };
    let qualified = |arena: &ExpArena, id: ExpId| -> Option<Vec<Symbol>> {
        let mut q = get_qualifier(arena, id)?;
        q.push(get_name(arena, id)?);
        Some(q)
    };
    let body = desugar(rw, body, &mut |rw, _, exp| {
        if let Exp::ScopedName { scope, prefix: Some(b), ident } = &exp {
            let mut whole = qualified(rw.arena, *b);
            if let (Some(w), Some(leaf)) = (&mut whole, get_name(rw.arena, *ident)) {
                w.push(leaf);
            }
            if whole.as_deref() == Some(nested.as_slice()) {
                return rw.arena[*b].exp.clone();
            }
            if qualified(rw.arena, *b).as_deref() == Some(nested.as_slice()) {
                return Exp::ScopedName { scope: scope.clone(), prefix: None, ident: *ident };
            }
        }
        modify_decl_scope_name(exp, &mut |q| strip(q))
    });
    rw.arena[body].exp.clone()
}

/// `parallel $ desugar (instantiate . resolve)` (`Template.hs:58`), the
/// `instantiate` half only, over a fork of `ir` — **one round** of
/// materialisation; the module doc of [`instantiate`] states why one and what
/// the loop needs that this cannot supply. Runs after [`resolve_instances`]
/// the way the reference composes the two per node — equivalent because they
/// consume disjoint nodes.
///
/// Like `resolve`, the decided data is a parameter (`kg-hir` stays free of
/// `kg-db`): `instances` maps a template's qualified path to its
/// [`InstanceSpec`]s, `expansions` is the interner the copies' frames live
/// in, and `types` — when the caller has a table — feeds `setTypeOfInstance`'s
/// `tArg` reads. The report carries the counters and the `TypeKey` writes the
/// caller may install.
pub fn instantiate_templates(
    ir: &ModuleIr,
    interner: &Interner,
    expansions: &ExpansionInterner,
    instances: &TemplateInstances,
    types: Option<&dyn crate::NodeTypes>,
) -> (ModuleIr, InstantiateReport) {
    let mut out = ir.fork();
    let report = instantiate_templates_in_place(&mut out, interner, expansions, instances, types);
    (out, report)
}

/// [`instantiate_templates`] appending into `ir`'s own arena.
pub fn instantiate_templates_in_place(
    ir: &mut ModuleIr,
    interner: &Interner,
    expansions: &ExpansionInterner,
    instances: &TemplateInstances,
    types: Option<&dyn crate::NodeTypes>,
) -> InstantiateReport {
    let mut pass = InstantiateTemplates::new(instances, expansions, types);
    let root = ir.root;
    let mut rw = Rewriter::new(&mut ir.arena, interner);
    ir.root = pass.apply(&mut rw, root);
    pass.report
}

/// Every rewrite this port has, in the reference's execution order, over **one**
/// fork.
///
/// One fork and not one per pass: the arena is append-only, so a per-pass fork
/// copies the whole file per pass and — worse — makes a later pass's `Own`
/// claim collide with the earlier pass's superseded node at the same key. The
/// substrate's rule (a node keeps its id iff the algebra's result equals its
/// original `Exp`) is what makes a shared arena cheap; `docs/journal.md`
/// records why the alternative was rejected.
///
/// The order is the reference's, not this module's: `captureThis` is frontend
/// pass 3, and within pass 8 `Template.hs:71-81`'s `foldr ($)` runs
/// `desugarLambdas` (`:78`) *before* `abbreviatedFunctionTemplate` (`:77`) and
/// `moveFunctions` (`:76`) after both — the list is written in reverse
/// execution order, so each landed sub-pass is an *append* here. The order is
/// load-bearing at every seam: `captureThis` decides the arity `make_captureN`
/// names, a generic lambda's synthesized function is the abbreviated-template
/// input the later pass fires on, and `moveFunctions`' abbreviated-`Function`
/// defer (`Function.hs:89-90`) exists precisely because the leftover half of
/// the previous sub-pass is still in the tree. The unported gap —
/// `addLambdaWrappers` runs *before* `desugarLambdas`, and five whole frontend
/// passes sit between `captureThis` and pass 8 — is the reason
/// [`crate::ModuleIr`]'s rewritten form is still graded per-pass against
/// `kg-oracle --template-passes N` rather than compared whole.
pub fn pipeline(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    pipeline_tail(&pipeline_head(ir, interner), interner)
}

/// The rewrites before `addLambdaWrappers`: frontend pass 2
/// (`higherOrderFunctions`, `Frontend.hs:75`) and pass 3's `captureThis`.
/// `kg-db` runs the wrappers between the halves — they read the type table.
pub fn pipeline_head(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    let mut out = ir.fork();
    let mut rw = Rewriter::new(&mut out.arena, interner);
    let root = higher_order::apply(&mut rw, ir.root);
    out.root = capture_this::apply(&mut rw, root);
    out
}

/// [`pipeline_head`], `higherOrderFunctions` resolving an alias-typed
/// parameter through `aliases` (`resolvedFuncParams symbols`,
/// `Function.hs:133-141`) — the elaboration's, which holds the program.
pub fn pipeline_head_with<'s>(ir: &ModuleIr, interner: &Interner, aliases: &'s higher_order::AliasTypes<'s>) -> ModuleIr {
    pipeline_head_refusing(ir, interner, Some(aliases)).0
}

/// [`pipeline_head`], or [`pipeline_head_with`] given `aliases`, and the
/// range of every function `higherOrderFunctions` left as written for want
/// of `inline` — pass 2's verdict (`higher_order::HigherOrderFunctions::
/// refused`).
pub fn pipeline_head_refusing<'s>(
    ir: &ModuleIr,
    interner: &Interner,
    aliases: Option<&'s higher_order::AliasTypes<'s>>,
) -> (ModuleIr, Vec<kg_base::FileRange>) {
    let mut out = ir.fork();
    let refused = {
        let mut rw = Rewriter::new(&mut out.arena, interner);
        let (root, refused) = higher_order::apply_refusing(&mut rw, ir.root, aliases);
        out.root = capture_this::apply(&mut rw, root);
        refused
    };
    let heads = refused.iter().map(|&id| out.arena[id].src.head).collect();
    (out, heads)
}

/// A later round's `desugarLambdas`, `abbreviatedFunctionTemplate` and
/// `moveFunctions` (`Template.hs:76-78`) over the last round's copies: the
/// reference runs the whole list every round (`Template.hs:37-81`), so a
/// lambda function a template body kept nested — `moveFunctions` defers
/// inside a template — is hoisted out of its copy (`compose2<|…|>@?lambda0`),
/// and a generic one is a template first (`TNameArg TTemplate`, what its
/// `Closure` is decided by).
pub fn round_prefix(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    let mut out = ir.fork();
    round_prefix_in_place(&mut out, interner);
    out
}

/// [`round_prefix`] appending into `ir`'s own arena.
pub fn round_prefix_in_place(ir: &mut ModuleIr, interner: &Interner) {
    let root = ir.root;
    let mut rw = Rewriter::new(&mut ir.arena, interner);
    let root = lambdas::apply(&mut rw, root);
    let mut abbreviated = AbbreviatedFunctionTemplate::later_round(rw.arena, root);
    let root = desugar_shallow(&mut rw, root, &mut abbreviated);
    ir.root = desugar_shallow(&mut rw, root, &mut MoveFunctions::new(interner));
}

/// Pass 8's list after the wrappers: `desugarLambdas`,
/// `abbreviatedFunctionTemplate`, `moveFunctions` (`Template.hs:76-78`).
pub fn pipeline_tail(ir: &ModuleIr, interner: &Interner) -> ModuleIr {
    let mut out = ir.fork();
    pipeline_tail_in_place(&mut out, interner);
    out
}

/// [`pipeline_tail`] appending into `ir`'s own arena.
pub fn pipeline_tail_in_place(ir: &mut ModuleIr, interner: &Interner) {
    let root = ir.root;
    let mut rw = Rewriter::new(&mut ir.arena, interner);
    let root = lambdas::apply(&mut rw, root);
    let root = desugar_shallow(&mut rw, root, &mut AbbreviatedFunctionTemplate::default());
    ir.root = desugar_shallow(&mut rw, root, &mut MoveFunctions::new(interner));
}
