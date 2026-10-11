//! `instantiate` — `makeInstance`, `deduplicateInstances`, and the
//! `removeNestedSeq` in its catch-all arm (`Template.hs:110-156`) — with
//! `setTypeOfInstance` (`Type.hs:426-440`), `resolveTypeParam`
//! (`TypeCheck.hs:331-337`) and `modifyDeclScopeName` (`Desugar.hs:139-162`):
//! the half of the per-round rewrite `desugar (instantiate . resolve)`
//! (`Template.hs:58`) that **materialises declarations**. Its sibling half,
//! `resolve`, is [`super::resolve`]; the two run as separate catas here where
//! the reference composes them per node, which is equivalent because they
//! consume disjoint nodes — `resolve` eats decided *mentions*, this pass eats
//! `Template` *declarations* — and a shape born inside a copy this round is
//! new to both, exactly as it is new to the reference's single walk.
//!
//! # One round, stated as one
//!
//! The reference recurses until `Map.null decidedInstances`, with `inferType`
//! between rounds (`Template.hs:56-58`) — 53% of oracle-readable roots need
//! two or more rounds, the maximum is twelve, and round 2's decided set can
//! only be computed by re-running the census and inference *over round 1's
//! copies* (`docs/journal.md`, "Materialisation is not blocked on
//! inference…"). This pass is **one round**: it materialises exactly the
//! instances it is handed. The entry point is re-callable — the arena is a
//! fork and the decided map a parameter — so the loop lands with the caller
//! that can recompute the census over a rewritten tree, and its convergence
//! test must be the reference's own (an empty decided map), never "two rounds
//! agree": 11 of 351 roots have a false plateau.
//!
//! # What `makeInstance` is here (`Template.hs:125-139`)
//!
//! Three steps over the copy, as in the reference:
//!
//! 1. **The copy walk** — `desugarWithNote n rewrite instanceDecl` (`:127`),
//!    a paramorphism that copies *every* node of the declaration and pushes
//!    the deciding site onto each copy's `SrcStack`. Here that is
//!    [`Src::expanded`]: the copy keeps its own head **and its own
//!    `written`** — an instantiated body is the user's own text compiled
//!    again — and gains one [`ExpansionData`] frame. The rewrite arms ride
//!    the walk:
//!    * a self-referential `TemplateInstance` (`:133-134`) keeps its
//!      **original** base — the very child node of the template body,
//!      shared, unframed — with rewritten args, and stays an instance for
//!      the next round;
//!    * a `ScopedName` that *is* the template (`:135-136`) gets its leaf
//!      mangled; one whose *qualifier* is the template (`:137`) gets its
//!      prefix's leaf mangled — an arm this port keeps for fidelity though
//!      it is shadowed in composition: bottom-up, the prefix node already
//!      matched the previous arm when it was visited (both spellings
//!      produce identical trees; measured on the oracle);
//!    * the catch-all is `resolveTypeParam t args'` (`:138`,
//!      `TypeCheck.hs:331-337`): a `TypeParam` whose scope is the
//!      template's qualifier becomes `argAt index = unNote . unfix . (args
//!      !!)` — the argument's **`Exp`**, wearing the *`TypeParam`'s* head
//!      plus the frame, never the argument's root range, while the
//!      argument's children ride along as the **same arena nodes**, shared
//!      and unframed. Everything else runs `removeNestedTypeIdentifier`
//!      (`Desugar.hs:126-131`), which is what collapses a substituted
//!      `TypeIdentifier` argument into `TemplateInstance`-base and
//!      `ScopedName`-prefix positions — oracle-verified: `W<Box>`'s copy
//!      holds the argument's own `ScopedName` unframed where the body
//!      spelled `TT`.
//! 2. **The scope walk** — `desugar (modifyDeclScopeName mangleScope)`
//!    (`:126`, `Desugar.hs:139-162`): any scope path the template's
//!    qualified name prefixes gets component `len - 1` replaced by the
//!    instance name (`setAt`, `Internal.hs:43-46`). Interior declarations
//!    keep their *text* and change only their scope.
//! 3. **`setTypeOfInstance`** (`:125`, `Type.hs:426-440`): the instance root
//!    is typed `TInstance TUnresolved template [(param, tArg)]`. This is the
//!    first production write of a [`TypeKey`] whose `via` is not `ROOT`.
//!
//! The mangled *declaration* name (`renote mangleDeclName`, `:130-131`) uses
//! the same `mangleTemplateIdentifier t args` bytes as `resolve` uses for
//! mentions ([`super::resolve::mangle_args`]) — they must agree to the byte —
//! and wears the **unmangled** name's range: its text is not its range's
//! text, so it is `Donated` inside an `Own` body.
//!
//! # `written` for the three invented shapes — a policy this port adds
//!
//! The reference has no `written`. The substrate's rule (`super`, module
//! comment) is "`Own` iff head is the node's own text", and three shapes in
//! a copy fail it: the mangled declaration name, a substituted argument root
//! (the argument's `Exp` wearing the `TypeParam`'s range), and a shared node
//! the scope walk rewrites in place (below). All three are `Donated`; every
//! plain body copy stays `Own`. The provenance gate cannot tell `Own` from
//! `Donated` on a substituted root — both are sole claimants inside their
//! frame — so this is pinned by the donor unit tests, not by any corpus
//! gate.
//!
//! # Copy-on-write where the reference has sharing
//!
//! Two places a copy references nodes it did not mint: a substituted
//! argument's children, and a self-referential instance's kept base. The
//! reference shares immutable structure; an arena shares the `ExpId` — one
//! node, one claimant, which is also the only legal answer under the
//! provenance gate (a second `Own` node at the same key is clause 3). When
//! the *scope walk* must rewrite such a shared node — the kept base's chain
//! is scope-mangled, oracle-verified on `fact.k` — the rewrite is allocated
//! fresh while the original stays where it was: the new node is `Donated`
//! (the survivor template still owns that text), where a rewrite of a node
//! this instance minted is a plain `replace`. The boundary is the arena
//! length at the start of the copy — an address used as a birthmark, never
//! as a key.
//!
//! # `deduplicateInstances` (`Template.hs:144-155`)
//!
//! Applied, with `removeNestedSeq` before it, at **every** non-`Template`
//! node (`:142`) — it is the reference's backstop for the mangler being
//! non-injective on purpose (`mangle FunctionType | isClosure = ""`,
//! `Mangle.hs:42-43`; `data.closure.core.Closure` alone has 5,117 distinct
//! argument lists). `nubBy` keeps the **first** of any two same-`Seq`
//! siblings that are the same declaration class (`Class`/`Struct`/`Union`/
//! `Alias`/`Template`/`Function`) with the same qualified name. Do not "fix"
//! the mangler; the collision is load-bearing.
//!
//! # Where this port deliberately diverges
//!
//! * **One call makes several rounds' instances.** The reference makes each
//!   round's copies in that round, `Map.toList` ordering them by the
//!   argument `Seq`'s structural `Ord` (`crate::order::ExpOrd`); a caller
//!   here may hand one call instances the reference decides in different
//!   rounds — an owner handed a foreign module's decisions all at once —
//!   so each spec carries its round ([`InstanceSpec::round`]) and the order
//!   is round first, arguments second. That also decides *which* of two
//!   colliding instances dedup keeps: the `Closure` copies collide as a
//!   matter of course (a lambda's instance and a higher-order formal's,
//!   one mangled name), and the earlier round's is the copy.
//! * **A declined instance is skipped, whole.** The reference crashes on an
//!   argument its mangler has no arm for (`Mangle.hs:63`) and on a
//!   non-`Identifier` name leaf (`:30`); this port skips that instance and
//!   counts it, and a template all of whose instances decline is left
//!   untouched, as if undecided. A missing declaration is silence; a
//!   wrongly-named one is a lie.
//! * **The body keeps the written literal.** `validateArg`
//!   (`TypeCheck.hs:281-283`) re-stamps an out-of-range literal argument *in
//!   the reference's tree* before any of this runs; here the stamp travels
//!   in [`InstanceSpec::stamps`] and reaches the mangled bytes and
//!   `setTypeOfInstance`'s `TIntArg`, while the substituted body carries the
//!   literal as written — `W2<300>` materialises as `W2<|@44|>` with `300`
//!   in its body where the reference holds `44`.
//! * **`setTypeOfInstance` declines what it cannot spell.** `tArg`'s
//!   fallback is `TArg (showPretty e)` (`Type.hs:437`), rendered here for a
//!   boolean and a literal aggregate (`{10, false}`, [`shown_value`]); bytes
//!   this IR cannot reproduce (string literals stored verbatim with quotes,
//!   expressions with no pinned rendering) decline the whole write rather
//!   than invent a rendering — counted, toward silence. A written builtin
//!   type the caller's table left untyped is the `TTypeArg` inference would
//!   give it (`written_builtin`).
//!
//! # Oracle commands every expectation below was read from
//!
//! ```text
//! kg-oracle --tree  --roots-only --passes 8 --template-passes 9 \
//!           --template-iterations 1 FILE
//! kg-oracle --notes --roots-only --passes 8 --template-passes 9 \
//!           --template-iterations 1 FILE
//! ```
//!
//! `--passes 8` or `removeTemplate` deletes the output (`Frontend.hs:300-302`).
//! One caveat the fixtures are chosen around: `--template-iterations 1` shows
//! the tree *after* the inter-round `inferType` (`Template.hs:58`), so a
//! fixture whose copies fold (`static if`, constexpr) prints post-fold shapes
//! this port does not produce; every fixture here either cannot fold or the
//! test says which nodes the fold owns.

use kg_base::{ExpansionInterner, FileRange, FxHashMap, FxHashSet, InstanceId, Interner, Src,
    Symbol};
use kg_const::{LiteralSign, LiteralType};
use kg_ty::TypeKey;
use kg_ty::{QualifiedName, TArg, Ty, TyRef};

use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;
use crate::rewrite::{
    desugar_within, get_name, get_qualifier, ident_sym, modify_decl_scope_name, resolve::mangle_args, Rewriter,
};

/// One decided instance of one template — `Map.toList`'s `(args, n)` pair
/// (`Template.hs:120-121`) plus what this arena needs to stand in for it.
#[derive(Clone, Debug)]
pub struct InstanceSpec {
    /// Content-addressed from the census's `(template, argument key)` pair —
    /// [`InstanceId::of`]. Two specs must never share an id unless they *are*
    /// the same instance; the provenance gate goes red if they do.
    pub id: InstanceId,
    /// The deciding occurrence's head — the note `n` that `desugarWithNote n`
    /// pushes, first in program order (`Map.union` is left-biased,
    /// `Template.hs:88`).
    pub site: FileRange,
    /// The argument `Seq`, resolvable in the module being rewritten. After
    /// `resolve` consumed the mention, the subtree is unreachable but alive —
    /// the fork's ancestry keeps every pre-pass id resolvable — and
    /// substitution puts it back in the tree, shared.
    pub args: ExpId,
    /// `validateArg`'s literal re-stamp per argument position
    /// (`TypeCheck.hs:281-283`), same contract as
    /// [`super::resolve::DecidedSite::stamps`].
    pub stamps: Vec<Option<LiteralType>>,
    /// The reference's round that decides this instance: 1 for a mention in
    /// a module's own text, one more per copy the mention stands in — an
    /// instance inside another template's body waits for that template's
    /// copy (`Template.hs:86-90`). A round's copies stand before the next
    /// round's, since each round's `SeqF $ map makeInstance … ++ [template]`
    /// wraps the last (`Template.hs:115`); so one template's instances order
    /// by round before their arguments, and of two that mangle alike —
    /// `Closure<(C) -> uint7, f, C>` from a lambda and `Closure<(C) -> auto,
    /// f, C>` from a higher-order formal, one name since a function type
    /// mangles to nothing — `deduplicateInstances` keeps the earlier
    /// round's (`return-type-polymorphism.k::23`).
    pub round: u32,
}

/// Template qualified path — `declScopeName ++ [declName]`, in IR components
/// (a module namespace is one mangled `@a@b` segment) — to its decided
/// instances, in the order they should materialise. See the module comment on
/// that order.
pub type TemplateInstances = FxHashMap<Vec<Symbol>, Vec<InstanceSpec>>;

/// What one run materialised — the counters the tests and the gate read.
#[derive(Debug, Default)]
pub struct InstantiateReport {
    /// Instances materialised.
    pub made: u32,
    /// Instances skipped whole because an argument reached a declined mangle
    /// arm or the declaration has no manglable name. The reference crashes on
    /// both.
    pub declined: u32,
    /// `Seq` items removed by `deduplicateInstances`.
    pub deduplicated: u32,
    /// `TypeParam`s substituted by `resolveTypeParam`.
    pub substituted: u32,
    /// Self-referential `TemplateInstance`s kept for the next round.
    pub kept_instances: u32,
    /// `setTypeOfInstance` writes, by position. The keys are the first
    /// production `TypeKey`s whose `via` is not `ROOT`.
    pub type_writes: Vec<(TypeKey, Ty)>,
    /// The same writes by node, for a caller whose notes are keyed by node
    /// (`kg_sem::TypeMap`) — a copy's root has a key only while its head
    /// is its own text.
    pub typed_roots: Vec<(ExpId, Ty)>,
    /// Each copy's root and the declaration it was copied from.
    /// `desugarWithNote n rewrite` (`Template.hs:127`, `Desugar.hs:92-94`)
    /// copies every note, so a copy of a copy — a later round's instance of
    /// the template an abbreviated copy became (`Template.hs:77`) — starts
    /// out typed as the first copy's instance, and `setTypeOf` nests the new
    /// instance inside it (`Type.hs:1050-1057`): the note the reference's
    /// walk names the copy by is the first round's. The caller, which holds
    /// the notes, lays the declaration's note on the root first.
    pub copied_from: Vec<(ExpId, ExpId)>,
    /// Each `Closure` copy's mangled name and the instance type
    /// `setTypeOfInstance` wrote at it — the note `infer AliasF` reads to
    /// type the copy as its closure (`Type.hs:1175-1177`), which a twin's
    /// table has no other way to learn.
    pub closure_copies: Vec<(Symbol, Ty)>,
    /// Every other alias copy's mangled name and its note — what `infer
    /// AliasF`'s result is threaded into (`setTypeOf`, `Type.hs:1053-1058`):
    /// `index_t<32>` is `TInstance uint5 index_t [(N, 32)]` wherever it is
    /// named.
    pub alias_copies: Vec<(Symbol, Ty)>,
    /// Instance types declined because an argument reached `tArg`'s
    /// unreproducible fallback.
    pub type_declines: u32,
}

/// `desugar instantiate` (`Template.hs:58,110-156`) as a stateful pass.
#[derive(Debug)]
pub struct InstantiateTemplates<'a> {
    instances: &'a TemplateInstances,
    exps: &'a ExpansionInterner,
    types: Option<&'a dyn crate::NodeTypes>,
    pub report: InstantiateReport,
}

/// Per-`makeInstance` context for the copy walk.
struct CopyCx<'s> {
    site: FileRange,
    iid: InstanceId,
    /// [`InstanceSpec::round`], which the copy's frame records.
    round: u32,
    /// `templateQualifiedName` (`Template.hs:117`).
    t_path: &'s [Symbol],
    /// `getQualifier template` — the declaration's `declScopeName`, the
    /// `TypeParam` guard (`TypeCheck.hs:335`).
    decl_scope: &'s QualifiedName,
    /// `args'` — the argument `Seq`'s items (`Template.hs:125`).
    args: &'s [ExpId],
    /// `"<|" <> cata mangle args <> "|>"` body, shared with `resolve`.
    suffix: &'s str,
}

impl<'a> InstantiateTemplates<'a> {
    pub fn new(
        instances: &'a TemplateInstances,
        exps: &'a ExpansionInterner,
        types: Option<&'a dyn crate::NodeTypes>,
    ) -> InstantiateTemplates<'a> {
        InstantiateTemplates { instances, exps, types, report: InstantiateReport::default() }
    }

    /// One round over the subtree at `root`. `step` hands back as it came
    /// anything but a `Template` and a `Seq` with something to splice or
    /// drop: one holding a `Seq`, or two declarations `dedup_key` names.
    pub fn apply(&mut self, rw: &mut Rewriter, root: ExpId) -> ExpId {
        use crate::facts::{DEDUP_SEQ, NESTED_SEQ, TEMPLATE};
        desugar_within(rw, root, TEMPLATE | NESTED_SEQ | DEDUP_SEQ, &mut |rw, _id, exp| self.step(rw, exp))
    }

    /// The algebra. `instantiate t@TemplateF{..} | … ; instantiate x =
    /// deduplicateInstances $ removeNestedSeq x` (`Template.hs:112-142`).
    fn step(&mut self, rw: &mut Rewriter, exp: Exp) -> Exp {
        match &exp {
            Exp::Template { decl, .. } => {
                // `| Typename <- templateDecl = t` (`:113`) — a
                // template-template *parameter*'s kind is a `Template` whose
                // decl is `Typename`, and it is never instantiated.
                if matches!(rw.arena[*decl].exp, Exp::Typename) {
                    return exp;
                }
                self.instantiate_template(rw, exp)
            }
            _ => {
                let spliced = rw.remove_nested_seq(exp);
                self.deduplicate(rw.arena, spliced)
            }
        }
    }

    /// The `Template` arm (`Template.hs:112-115`): `SeqF $ map makeInstance
    /// templateInstances ++ [copyNote t templateDecl]` — instances first,
    /// original last.
    fn instantiate_template(&mut self, rw: &mut Rewriter, exp: Exp) -> Exp {
        let Exp::Template { params, decl } = &exp else { unreachable!("checked by step") };
        let (params, decl) = (params.clone(), *decl);
        let Some(t_path) = template_path(rw.arena, decl) else { return exp };
        let Some(specs) = self.instances.get(&t_path) else {
            // `| null templateInstances = t` (`:114`).
            return exp;
        };
        if specs.is_empty() {
            return exp;
        }
        // `Map.toList` (`Template.hs:120`): the copies in the order of their
        // argument lists' derived `Ord` ([`crate::order`]), each literal as
        // `validateArg` re-stamped it (`TypeCheck.hs:280-284`) — the tree
        // keeps the written one, the reference's key holds the stamped one.
        let mut specs = specs.clone();
        specs.sort_by(|a, b| a.round.cmp(&b.round).then_with(|| cmp_args(rw, a, b)));
        let mut items = Vec::with_capacity(specs.len() + 1);
        for spec in specs {
            match self.make_instance(rw, &t_path, &params, decl, &spec) {
                Some(inst) => {
                    self.report.made += 1;
                    items.push(inst);
                }
                None => self.report.declined += 1,
            }
        }
        if items.is_empty() {
            // Every instance declined: leave the template as if undecided —
            // a stated divergence (the reference cannot reach this).
            return exp;
        }
        // `copyNote t templateDecl` (`:115`): the surviving original wears
        // the *declaration's* note. Donated — the decl child, still reachable
        // below it, owns that text.
        let survivor = rw.donate(decl, Exp::Template { params, decl });
        items.push(survivor);
        Exp::Seq(items)
    }

    /// Whether `param` is a value template parameter declared `auto`.
    fn is_auto_value_param(arena: &ExpArena, param: ExpId) -> bool {
        matches!(&arena[param].exp, Exp::TemplateParam { kind, .. } if matches!(arena[*kind].exp, Exp::Auto))
    }

    /// `makeInstance (args@(Seq args'), n)` (`Template.hs:125-139`).
    fn make_instance(
        &mut self,
        rw: &mut Rewriter,
        t_path: &[Symbol],
        params: &[ExpId],
        decl: ExpId,
        spec: &InstanceSpec,
    ) -> Option<ExpId> {
        // `makeInstance _ = undefined` (`:140`) — the decided value is always
        // a `Seq`; anything else declines.
        let Exp::Seq(arg_items) = rw.arena[spec.args].exp.clone() else { return None };
        // `validateArg`'s literal rewrite (`TypeCheck.hs:281-283`): an int
        // literal under a sized integer parameter is re-stamped to the
        // parameter's width and truncated to it — `78` under a `uint5`
        // parameter is `14u5` — *before* `instantiate` reads the list, so the
        // copy's every substituted `X` folds at the narrowed value. The
        // reference rewrites the mention's `Seq` in place at pass 8's first
        // sub-pass; here the mention keeps its written literal (the mangler
        // reads the stamp itself, `resolve::mangle_args`) and the copy gets
        // the rewritten one, donated from it.
        let arg_items: Vec<ExpId> = arg_items
            .iter()
            .enumerate()
            .map(|(i, &arg)| match (spec.stamps.get(i).copied().flatten(), &rw.arena[arg].exp) {
                (Some(stamp), Exp::IntLiteral { value, .. }) => {
                    let value = stamp.truncate(value);
                    rw.donate(arg, Exp::IntLiteral { ty: Some(stamp), value })
                }
                // `validate Auto (IntLiteralF _ i) = IntLiteralF Nothing i`
                // (`TypeCheck.hs:283`): under an `auto` parameter a literal
                // argument loses its type, so a fold that kept one — `x - 4`
                // over an unrolled `uint3 x`, `-0i6` — substitutes a bare
                // `0`, and the copy's `u & N` is `tAndLiteral` (`literal-
                // operand.k::0`).
                (None, Exp::IntLiteral { ty: Some(_), value })
                    if params.get(i).is_some_and(|&p| Self::is_auto_value_param(rw.arena, p)) =>
                {
                    let value = value.clone();
                    rw.donate(arg, Exp::IntLiteral { ty: None, value })
                }
                _ => arg,
            })
            .collect();
        // The declaration must have a name this pass can mangle.
        let name_child = decl_name(rw.arena, decl)?;
        ident_sym(rw.arena, name_child)?;
        let decl_scope = decl_scope(rw.arena, decl)?.clone();

        // `mangleTemplateIdentifier t args` — the same bytes `resolve` mints
        // for mentions, stamps included. `None` declines the instance whole.
        let is_closure = is_closure_path(rw.interner, t_path);
        let suffix = mangle_args(rw.arena, rw.interner, spec.args, is_closure, &spec.stamps)?;

        let copy_start = rw.arena.len();
        let cx = CopyCx {
            site: spec.site,
            iid: spec.id,
            round: spec.round,
            t_path,
            decl_scope: &decl_scope,
            args: &arg_items,
            suffix: &suffix,
        };

        // Walk 1: the framed copy, rewrite arms riding (`:127-138`).
        let copied = self.copy(rw, decl, &cx);

        // `renote mangleDeclName` (`:130-131`): only the outermost `declName`,
        // wearing the unmangled name's range — `Donated` inside an `Own` body.
        let copied_name = decl_name(rw.arena, copied).expect("copy preserves shape");
        let leaf = ident_sym(rw.arena, copied_name).expect("checked above");
        let instance_name = rw.suffix_name(leaf, &super::resolve::instance_suffix(&suffix));
        let mangled_name = rw.donate(copied_name, Exp::Identifier(instance_name));
        let mut root_exp = rw.arena[copied].exp.clone();
        set_decl_name(&mut root_exp, mangled_name);
        let root = rw.replace(copied, root_exp);

        // Walk 2: `desugar (modifyDeclScopeName mangleScope)` (`:126,131-132`).
        let root = self.mangle_scopes(rw, root, t_path, instance_name, copy_start);

        self.report.copied_from.push((root, decl));
        // `setTypeOfInstance templateQualifiedName (zip …)` (`:125`).
        let writes = self.report.type_writes.len();
        self.set_type_of_instance(rw, root, t_path, params, &arg_items, &spec.stamps);
        if self.report.type_writes.len() > writes {
            let ty = self.report.type_writes[writes].1.clone();
            if is_closure {
                self.report.closure_copies.push((instance_name, ty));
            } else if matches!(rw.arena[root].exp, Exp::Alias { .. }) {
                self.report.alias_copies.push((instance_name, ty));
            }
        }
        Some(root)
    }

    /// Walk 1 — `desugarWithNote n rewrite` (`Template.hs:127`,
    /// `Desugar.hs:92-94`): every node is copied with [`Src::expanded`]; the
    /// arms are checked in the reference's order.
    ///
    /// Every copy is allocated *for* the node it copies, so the loop's
    /// `carry_types` gives it that node's type: `n' <> n` keeps the copied
    /// node's own type (`Typed`'s `Semigroup`, `Type.hs:452-453`). Inside a
    /// template body only what a rewrite minted there is typed, and that
    /// can be a note the text no longer says — `expFromType`'s
    /// `tuple2<|…|>` noted `pair<…>` (`Type.hs:1638,1647`), which a copy of
    /// a copy re-inferred from its text loses (`memory/pipelined.k`).
    fn copy(&mut self, rw: &mut Rewriter, id: ExpId, cx: &CopyCx<'_>) -> ExpId {
        let node = rw.arena[id].clone();
        let src = Src::expanded(node.src, cx.site, cx.iid, cx.round, self.exps);

        // `rewrite (TemplateInstance a _) (TemplateInstanceF _ b)` (`:133-134`)
        // — the guard reads the *original* base (the paramorphism's first
        // argument), and the result keeps it: shared, unframed, still owned
        // by the surviving template body. Only the args are copied.
        if let Exp::TemplateInstance { base, args } = node.exp
            && q_path(rw.arena, base).is_some_and(|p| p == cx.t_path)
        {
            let args = self.copy(rw, args, cx);
            self.report.kept_instances += 1;
            return rw.arena.alloc_for(id, Exp::TemplateInstance { base, args }, src, node.kind);
        }

        // `resolveTypeParam`'s substitution arm (`TypeCheck.hs:334-336`):
        // `argAt = unNote . unfix . (args !!)` — the argument's `Exp` wearing
        // the `TypeParam`'s head plus the frame (`Donated`: the text at that
        // head is the parameter, not the argument), its children shared and
        // unframed. The `TypeParam`'s own kind child is never copied, exactly
        // as the reference discards it with the constructor.
        if let Exp::TypeParam { scope, index, .. } = &node.exp
            && scope == cx.decl_scope
        {
            // `args !! index` out of range crashes the reference; a decided
            // instance always has full arity (`templateArgsResolved`,
            // `TypeCheck.hs:323`), so a miss here falls through to a plain
            // copy rather than inventing a substitution.
            if let Some(&arg) = cx.args.get(*index as usize) {
                let exp = rw.arena[arg].exp.clone();
                self.report.substituted += 1;
                return rw.arena.alloc_for(id, exp, Src::donated(src), node.kind);
            }
        }

        // Plain copy, children first (the walk is bottom-up), then the name
        // arms and the `removeNestedTypeIdentifier` catch-all.
        let mut rebuilt = node.exp.clone();
        for slot in rebuilt.children_mut() {
            *slot = self.copy(rw, *slot, cx);
        }
        let rebuilt = self.name_arms(rw, rebuilt, cx);
        rw.arena.alloc_for(id, rebuilt, src, node.kind)
    }

    /// The two `ScopedName` arms (`Template.hs:135-137`) and the catch-all's
    /// `removeNestedTypeIdentifier` (`TypeCheck.hs:337`, `Desugar.hs:126-131`),
    /// applied to the rebuilt node the way the reference applies them to the
    /// functor with rewritten children.
    fn name_arms(&mut self, rw: &mut Rewriter, exp: Exp, cx: &CopyCx<'_>) -> Exp {
        let is_template = |p: Vec<Symbol>| p.as_slice() == cx.t_path;
        if let Exp::ScopedName { scope, prefix, ident } = &exp {
            // `getQualifiedName x == templateQualifiedName` (`:135-136`):
            // this mention *is* the template — mangle its leaf
            // (`mangleTemplateScopedName`, `Mangle.hs:18-19`).
            if q_path_scoped(rw.arena, *prefix, *ident).is_some_and(is_template) {
                if let Some(mangled) = self.mangle_leaf(rw, *ident, cx) {
                    return Exp::ScopedName { scope: scope.clone(), prefix: *prefix, ident: mangled };
                }
                return exp;
            }
            // `getQualifier x == templateQualifiedName` (`:137`): the prefix
            // spells the template. Kept for fidelity; in composition the
            // prefix node already matched the arm above when the walk visited
            // it (both produce the same tree — see the module comment).
            if let Some(p) = prefix
                && q_path(rw.arena, *p).is_some_and(is_template)
                && let Exp::ScopedName { scope: ps, prefix: pp, ident: pident } =
                    rw.arena[*p].exp.clone()
                && let Some(mangled) = self.mangle_leaf(rw, pident, cx)
            {
                let new_prefix = rw.donate(
                    *p,
                    Exp::ScopedName { scope: ps, prefix: pp, ident: mangled },
                );
                return Exp::ScopedName {
                    scope: scope.clone(),
                    prefix: Some(new_prefix),
                    ident: *ident,
                };
            }
        }
        remove_nested_type_identifier(rw.arena, exp)
    }

    /// `mangleIdentifier'` (`Mangle.hs:29-30`), `renote`d: the mangled leaf
    /// wears the unmangled leaf's note — `Donated`, its text is not its
    /// range's. `None` where the reference's `undefined` fires (a
    /// non-`Identifier` leaf); the mention is left as written and counted by
    /// the caller's arm falling through.
    fn mangle_leaf(&mut self, rw: &mut Rewriter, ident: ExpId, cx: &CopyCx<'_>) -> Option<ExpId> {
        let leaf = ident_sym(rw.arena, ident)?;
        let name = rw.suffix_name(leaf, &super::resolve::instance_suffix(cx.suffix));
        Some(rw.donate(ident, Exp::Identifier(name)))
    }

    /// Walk 2 — `desugar (modifyDeclScopeName mangleScope)`
    /// (`Template.hs:126,131-132`, `Desugar.hs:139-162`). Bottom-up; a node
    /// this instance minted (`id.index() >= copy_start`, the arena length
    /// birthmark) is replaced in place, while a rewrite of *shared* structure
    /// — the kept base of a self-referential instance is the one that occurs
    /// (oracle-verified on `fact.k`) — is allocated `Donated` beside the
    /// original it must not displace.
    fn mangle_scopes(
        &mut self,
        rw: &mut Rewriter,
        id: ExpId,
        t_path: &[Symbol],
        instance_name: Symbol,
        copy_start: usize,
    ) -> ExpId {
        let original = rw.arena[id].exp.clone();
        let mut rebuilt = original.clone();
        for slot in rebuilt.children_mut() {
            *slot = self.mangle_scopes(rw, *slot, t_path, instance_name, copy_start);
        }
        let out = modify_decl_scope_name(rebuilt, &mut |q| mangle_scope(q, t_path, instance_name));
        if out == original {
            return id;
        }
        if id.index() >= copy_start {
            rw.replace(id, out)
        } else {
            rw.donate(id, out)
        }
    }

    /// `setTypeOfInstance s (zip (map getName templateParams) args')`
    /// (`Template.hs:125`, `Type.hs:426-440`): type the instance root
    /// `TInstance TUnresolved s [(name, tArg)]`. The key's `via` is the
    /// copy's frame — the fifth field's first production write.
    fn set_type_of_instance(
        &mut self,
        rw: &Rewriter,
        root: ExpId,
        t_path: &[Symbol],
        params: &[ExpId],
        args: &[ExpId],
        stamps: &[Option<LiteralType>],
    ) {
        let mut pairs = Vec::with_capacity(args.len());
        for (i, &arg) in args.iter().enumerate() {
            let name = match params.get(i).map(|&p| &rw.arena[p].exp) {
                Some(Exp::TemplateParam { name, .. }) => *name,
                // Arity was checked by the census; a shape mismatch here
                // declines the write, never invents a pairing.
                _ => {
                    self.report.type_declines += 1;
                    return;
                }
            };
            let Some(read) = t_arg(rw, self.types, arg, stamps.get(i).copied().flatten()) else {
                self.report.type_declines += 1;
                return;
            };
            pairs.push((name, read));
        }
        let node = &rw.arena[root];
        let range = node.src.head.range;
        let key = TypeKey::at(node.src.head.file, range, node.kind, node.src.via);
        let ty = Ty::Instance {
            concrete: TyRef::new(Ty::Unresolved),
            template: QualifiedName::new(t_path.iter().copied()),
            args: pairs,
        };
        self.report.typed_roots.push((root, ty.clone()));
        self.report.type_writes.push((key, ty));
    }

    /// `deduplicateInstances (SeqF a) = SeqF (nubBy dup a)`
    /// (`Template.hs:144-155`): first occurrence wins; `dup` is "same
    /// declaration class AND same qualified name", so the keyed set below is
    /// the same answer as the reference's pairwise `nubBy`.
    fn deduplicate(&mut self, arena: &ExpArena, exp: Exp) -> Exp {
        let Exp::Seq(items) = &exp else { return exp };
        let mut seen: FxHashSet<(u8, Vec<Symbol>)> = FxHashSet::default();
        let mut out = Vec::with_capacity(items.len());
        // Name to name: a later round's tree holds the last round's copy
        // under the print's alias beside this round's fresh copy of the same
        // instance, which a later census decided again as the reference's
        // does — `optional<|$uint@32|>` twice in `memory.k::12` until
        // 2026-09-19.
        for &item in items {
            if let Some((class, key)) = dedup_key(arena, item)
                && !seen.insert((class, key))
            {
                self.report.deduplicated += 1;
                continue;
            }
            out.push(item);
        }
        if out.len() == items.len() { exp } else { Exp::Seq(out) }
    }
}

/// `parallel (desugar deduplicateInstances)` (`Template.hs:57`): the pass
/// the reference runs over the program when a round decides nothing — each
/// `Seq`'s later declaration of a class and qualified name an earlier one
/// already has is dropped, first occurrence wins, no other rewrite.
pub fn deduplicate_instances(ir: &crate::ModuleIr, interner: &kg_base::Interner) -> crate::ModuleIr {
    let mut out = ir.fork();
    deduplicate_instances_in_place(&mut out, interner);
    out
}

/// [`deduplicate_instances`] appending into `ir`'s own arena.
pub fn deduplicate_instances_in_place(ir: &mut crate::ModuleIr, interner: &kg_base::Interner) {
    let root = ir.root;
    let mut rw = Rewriter::new(&mut ir.arena, interner);
    // Only a `Seq` with two declarations `dedup_key` names can lose one.
    ir.root = desugar_within(&mut rw, root, crate::facts::DEDUP_SEQ, &mut |rw, _id, exp| {
        let Exp::Seq(items) = &exp else { return exp };
        let mut seen: FxHashSet<(u8, Vec<Symbol>)> = FxHashSet::default();
        let kept: Vec<ExpId> = items
            .iter()
            .copied()
            .filter(|&item| dedup_key(rw.arena, item).is_none_or(|k| seen.insert(k)))
            .collect();
        if kept.len() == items.len() { exp } else { Exp::Seq(kept) }
    });
}

/// Two specs' argument lists under the reference's key order: position by
/// position, a literal the spec re-stamps compared as the stamped literal.
fn cmp_args(rw: &Rewriter, a: &InstanceSpec, b: &InstanceSpec) -> std::cmp::Ordering {
    let ord = crate::order::ExpOrd { arena: rw.arena, interner: rw.interner };
    let (Exp::Seq(xs), Exp::Seq(ys)) = (&rw.arena[a.args].exp, &rw.arena[b.args].exp) else {
        return ord.cmp(a.args, b.args);
    };
    let stamped = |id: ExpId, stamp: Option<LiteralType>| -> Option<(Option<(LiteralSign, u32)>, kg_const::ConstInt)> {
        let Exp::IntLiteral { value, .. } = &rw.arena[id].exp else { return None };
        let s = stamp?;
        Some((Some((s.sign, s.width)), s.truncate(value)))
    };
    let literal = |id: ExpId| -> Option<(Option<(LiteralSign, u32)>, kg_const::ConstInt)> {
        match &rw.arena[id].exp {
            Exp::IntLiteral { ty, value } => Some((ty.map(|t| (t.sign, t.width)), value.clone())),
            _ => None,
        }
    };
    for (i, (x, y)) in xs.iter().zip(ys).enumerate() {
        let sx = a.stamps.get(i).copied().flatten();
        let sy = b.stamps.get(i).copied().flatten();
        let o = match (stamped(*x, sx).or_else(|| literal(*x)), stamped(*y, sy).or_else(|| literal(*y))) {
            (Some(p), Some(q)) => p.cmp(&q),
            _ => ord.cmp(*x, *y),
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    xs.len().cmp(&ys.len())
}

/// `tArg` (`Type.hs:431-437`). `None` declines the whole instance-type write
/// — see the module comment's divergence list.
fn t_arg(
    rw: &Rewriter,
    types: Option<&dyn crate::NodeTypes>,
    arg: ExpId,
    stamp: Option<LiteralType>,
) -> Option<TArg> {
    let node = &rw.arena[arg];
    match &node.exp {
        // `tArg (IntLiteral _ x) = TIntArg x` — over the value `validateArg`
        // left in the reference's tree, which is the stamped one.
        Exp::IntLiteral { value, .. } => {
            let v = match stamp {
                Some(s) => s.truncate(value),
                None => value.clone(),
            };
            Some(TArg::Int(v))
        }
        // `tArg (StringLiteral x) = TStrArg x`, over the parsed content — a
        // hole-less string expression is one after `interpolatedStringPre`.
        Exp::StringLiteral(tok) | Exp::InterpolatedString(tok) => {
            Some(TArg::Str(kg_base::SmolStr::new(crate::exp::plain_string_value(tok)?)))
        }
        // `tArg e@QualifiedIdentifier{} = TNameArg (typeOf e) (getQualifiedName e)`.
        Exp::QualifiedIdentifier(_) => {
            let path = q_path(rw.arena, arg)?;
            Some(TArg::Name(
                TyRef::new(type_of(rw, types, arg)),
                QualifiedName::new(path.iter().copied()),
            ))
        }
        _ => {
            // `case typeOf e of TType x -> TTypeArg x; _ -> TArg (showPretty e)`.
            if let Ty::Type(x) = type_of(rw, types, arg) {
                return Some(TArg::Type(x));
            }
            if let Some(b) = written_builtin(rw.arena, arg) {
                return Some(TArg::Type(TyRef::new(b)));
            }
            if let Exp::BoolLiteral(b) = &node.exp {
                // `showPretty (BoolLiteral …)` is the source spelling
                // (`PrettyPrint.hs:82-83`).
                return Some(TArg::Expr(kg_base::SmolStr::new(if *b { "true" } else { "false" })));
            }
            // An aggregate value — `Go<Foo, {10, false}>` — is shown as
            // written: `braces' (commaSep a)` (`PrettyPrint.hs:122`), whose
            // `group` lays a short list on one line, `{10, false}`, and an
            // empty one `{}` (`struct-constexpr.k::0,7`, the recorded walks'
            // display names).
            // A float, `pretty x` — GHC's `show` (`PrettyPrint.hs:105`):
            // `Bar<float, 1.0>` (`type-check.k::35`, the recorded walk).
            if let Exp::InitializerList(_) | Exp::FloatLiteral(_) = &node.exp {
                return shown_value(rw.arena, rw.interner, arg).map(|s| TArg::Expr(kg_base::SmolStr::new(s)));
            }
            // A template named as an argument: `typeOf e` is `TTemplate`,
            // and the reference falls to `TArg $ showPretty e`, which the
            // backend's `parseInstanceType` spells `nullPtr`
            // (`ParseTree.hs:252`) — what matters is that the note exists.
            // Held as the template's path, so two such copies stay apart.
            if matches!(type_of(rw, types, arg), Ty::Template) {
                return pretty_name(rw.arena, rw.interner, arg).map(|s| TArg::Expr(kg_base::SmolStr::new(s)));
            }
            // An enumerator folded to its value: `TArg (showPretty e)`, and
            // `src (EnumValue a _) = pretty a` (`PrettyPrint.hs:98`) — its
            // written name, a module qualifier left out: `Denorm::Off`.
            if let Exp::EnumValue { ty: name, .. } = &node.exp {
                return pretty_name(rw.arena, rw.interner, *name).map(|s| TArg::Expr(kg_base::SmolStr::new(s)));
            }
            None
        }
    }
}

/// `showPretty` (`Type.hs:423-424`) of a name as `PrettyPrint` spells it:
/// `QualifiedIdentifier a` and `TypeIdentifier _ a` as `a`
/// (`PrettyPrint.hs:144,167`), and a qualified `ScopedName` as `a::b` —
/// unless its qualifier is one internal segment, a module namespace
/// (`@…`) or `$unresolved`, which is left out (`:146-148`,
/// `isInternalQualifier`, `:191-194`). So a template named as an argument,
/// `read_mem<bool, memory_ecc>`, is `memory_ecc` in the instance's note
/// though the tree holds `@data@memory::memory_ecc`.
fn pretty_name(arena: &ExpArena, interner: &kg_base::Interner, id: ExpId) -> Option<String> {
    match &arena[id].exp {
        Exp::Identifier(s) => Some(interner.resolve(*s).to_string()),
        Exp::QualifiedIdentifier(a) | Exp::TypeIdentifier(_, a) => pretty_name(arena, interner, *a),
        Exp::ScopedName { prefix: None, ident, .. } => pretty_name(arena, interner, *ident),
        Exp::ScopedName { prefix: Some(p), ident, .. } => {
            let internal = match &arena[*p].exp {
                Exp::ScopedName { prefix: None, ident: q, .. } => match &arena[*q].exp {
                    Exp::Identifier(s) => {
                        let text = interner.resolve(*s);
                        text.starts_with('@') || &*text == "$unresolved"
                    }
                    _ => false,
                },
                _ => false,
            };
            let leaf = pretty_name(arena, interner, *ident)?;
            if internal { Some(leaf) } else { Some(format!("{}::{leaf}", pretty_name(arena, interner, *p)?)) }
        }
        _ => None,
    }
}

/// `showPretty` (`PrettyPrint.hs:82-137`) of a literal value, for the forms
/// whose printing is pinned: an unstamped integer, a boolean, a float, a
/// designator and an initializer list of them. `None` for anything else — a
/// stamped literal, an enumerator — which declines the note, as before.
fn shown_value(arena: &crate::arena::ExpArena, interner: &kg_base::Interner, id: ExpId) -> Option<String> {
    match &arena[id].exp {
        Exp::IntLiteral { ty: None, value } => Some(value.to_string()),
        Exp::BoolLiteral(b) => Some(if *b { "true" } else { "false" }.to_string()),
        // `src (FloatLiteral x) = pretty x` (`:105`), `viaShow`:
        // `Bar<float, 1.0>`.
        Exp::FloatLiteral(bits) => Some(bits.haskell_show()),
        // `dot <> pretty a <+> equals <+> pretty b` (`:92`).
        Exp::Designator { name, value } => {
            let Exp::Identifier(n) = arena[*name].exp else { return None };
            Some(format!(".{} = {}", interner.resolve(n), shown_value(arena, interner, *value)?))
        }
        // `src (Positional _ a) = pretty a` (`:137`).
        Exp::Positional { value, .. } => shown_value(arena, interner, *value),
        Exp::InitializerList(items) => {
            let Exp::Seq(items) = &arena[*items].exp else { return None };
            let parts = items.iter().map(|&i| shown_value(arena, interner, i)).collect::<Option<Vec<_>>>()?;
            Some(format!("{{{}}}", parts.join(", ")))
        }
        _ => None,
    }
}

/// `typeOf` (`Type.hs:452`) over the caller's notes; `Ty::Unresolved` where
/// there are none, which is `typeOf`'s own answer for an unnoted node.
fn type_of(rw: &Rewriter, types: Option<&dyn crate::NodeTypes>, arg: ExpId) -> Ty {
    types.map_or(Ty::Unresolved, |t| t.type_at(rw.arena, arg))
}

/// What inference resolves a written builtin type to, read off the spelling,
/// for an argument the loop's map leaves untyped: one the census minted for
/// an indirect instance (`Mint::exp_from_type` under a `newNote`, so
/// `TUnresolved`), where the reference substitutes the written argument
/// itself and reads its `TType` note (`resolveTemplateParam`,
/// `TypeCheck.hs:334-339`) — `template.k`'s `Bar<uint6, 101>` over `Foo<T,
/// 100> N`, the corpus's two (an instrumented `ptdiff`, 2026-10-10). Kept,
/// in `single-source-of-truth.md`'s register, until the census mints its
/// arguments typed.
fn written_builtin(arena: &ExpArena, id: ExpId) -> Option<Ty> {
    match &arena[id].exp {
        Exp::Boolean => Some(Ty::Boolean),
        Exp::Float => Some(Ty::Float),
        Exp::Str => Some(Ty::Str),
        Exp::Void => Some(Ty::Void),
        Exp::Integer(w) => Some(Ty::Signed(*w)),
        Exp::Unsigned(w) => Some(Ty::Unsigned(*w)),
        Exp::ParamInt(width) | Exp::ParamUint(width) => {
            let signed = matches!(&arena[id].exp, Exp::ParamInt(_));
            let Exp::IntLiteral { value, .. } = &arena[*width].exp else { return None };
            let w = value.to_u32()?;
            Some(if signed { Ty::Signed(w) } else { Ty::Unsigned(w) })
        }
        _ => None,
    }
}

/// `removeNestedTypeIdentifier` (`Desugar.hs:126-131`): collapse a
/// `TypeIdentifier` a substitution nested under another name node. The inner
/// wrapper node drops out of the tree with its note — which is how a
/// substituted `TypeIdentifier` argument's child ends up standing, unframed,
/// where the body spelled the parameter (oracle-verified, `W<Box>`).
pub fn remove_nested_type_identifier(arena: &ExpArena, exp: Exp) -> Exp {
    match &exp {
        Exp::TypeIdentifier(s, a) => {
            if let Exp::TypeIdentifier(_, inner) = &arena[*a].exp {
                return Exp::TypeIdentifier(s.clone(), *inner);
            }
            exp
        }
        Exp::TemplateInstance { base, args } => {
            if let Exp::TypeIdentifier(_, inner) = &arena[*base].exp {
                return Exp::TemplateInstance { base: *inner, args: *args };
            }
            exp
        }
        Exp::ScopedName { scope, prefix: Some(p), ident } => {
            if let Exp::TypeIdentifier(_, inner) = &arena[*p].exp {
                return Exp::ScopedName {
                    scope: scope.clone(),
                    prefix: Some(*inner),
                    ident: *ident,
                };
            }
            exp
        }
        _ => exp,
    }
}

/// `mangleScope` (`Template.hs:131-132`): a path the template's qualified
/// name prefixes gets component `length templateQualifiedName - 1` replaced
/// by the instance name (`setAt`, `Internal.hs:43-46`).
///
/// Compared name to name: a template a later round made of a copy is
/// declared under the print's alias (`__kg_m7`) while the scopes inside it
/// still say `reinterpret_cast<|$bool|>`.
fn mangle_scope(
    q: &QualifiedName,
    t_path: &[Symbol],
    instance_name: Symbol,
) -> Option<QualifiedName> {
    if q.segments().len() < t_path.len() || q.segments()[..t_path.len()] != *t_path
    {
        return None;
    }
    let mut segments = q.segments().to_vec();
    segments[t_path.len() - 1] = instance_name;
    Some(QualifiedName::new(segments))
}

/// `getQualifiedName t` for the declaration a `Template` wraps:
/// `declScopeName ++ [declName]` (`Parser/Syntax.hs:727-765`, the
/// `getQualifier e = declScopeName e` and `getName e = getName (declName e)`
/// catch-alls). A member function's prototype is templatable —
/// `functionDef` answers a `FunctionDecl` inside `template` when the
/// member ends in `;` (`Parser.hs:417-430`) — so `FunctionDecl` is among
/// the kinds (`interface/external_class.k`'s `template <typename T> T
/// Add(T a, T b);`).
fn template_path(arena: &ExpArena, decl: ExpId) -> Option<Vec<Symbol>> {
    let name = decl_name(arena, decl)?;
    let sym = ident_sym(arena, name)?;
    let mut path = decl_scope(arena, decl)?.segments().to_vec();
    path.push(sym);
    Some(path)
}

fn decl_name(arena: &ExpArena, decl: ExpId) -> Option<ExpId> {
    match &arena[decl].exp {
        Exp::Function { name, .. }
        | Exp::FunctionDecl { name, .. }
        | Exp::Class { name, .. }
        | Exp::Struct { name, .. }
        | Exp::Union { name, .. }
        | Exp::Enum { name, .. }
        | Exp::Alias { name, .. } => Some(*name),
        _ => None,
    }
}

/// `mangleDeclName x = x { declName = … }` (`Template.hs:131`) — the record
/// update, on the kinds [`decl_name`] admits.
fn set_decl_name(exp: &mut Exp, new_name: ExpId) {
    match exp {
        Exp::Function { name, .. }
        | Exp::FunctionDecl { name, .. }
        | Exp::Class { name, .. }
        | Exp::Struct { name, .. }
        | Exp::Union { name, .. }
        | Exp::Enum { name, .. }
        | Exp::Alias { name, .. } => *name = new_name,
        _ => unreachable!("decl_name admitted this node"),
    }
}

/// [`decl_scope`] for a caller outside this module: a declaration's
/// `declScopeName`.
pub fn decl_scope_of(arena: &ExpArena, decl: ExpId) -> Option<&QualifiedName> {
    decl_scope(arena, decl)
}

fn decl_scope(arena: &ExpArena, decl: ExpId) -> Option<&QualifiedName> {
    match &arena[decl].exp {
        Exp::Function { scope, .. }
        | Exp::FunctionDecl { scope, .. }
        | Exp::Class { scope, .. }
        | Exp::Struct { scope, .. }
        | Exp::Union { scope, .. }
        | Exp::Enum { scope, .. }
        | Exp::Alias { scope, .. } => Some(scope),
        _ => None,
    }
}

/// `getQualifiedName` for a *mention* — `getQualifier ++ [getName]`
/// (`Parser/Syntax.hs:727-765`) — with the `TypeParamF` arms the shared
/// helpers do not carry (`getName (TypeParamF _ _ s _) = s`, `getQualifier
/// (TypeParamF a _ _ _) = a`). A `TypeParam` nested deeper than the head of a
/// chain still answers `None`, which declines toward not renaming.
fn q_path(arena: &ExpArena, id: ExpId) -> Option<Vec<Symbol>> {
    if let Exp::TypeParam { scope, name, .. } = &arena[id].exp {
        let mut p = scope.segments().to_vec();
        p.push(*name);
        return Some(p);
    }
    let mut p = get_qualifier(arena, id)?;
    p.push(get_name(arena, id)?);
    Some(p)
}

/// [`q_path`] for a rebuilt `ScopedName` that has no id yet.
fn q_path_scoped(arena: &ExpArena, prefix: Option<ExpId>, ident: ExpId) -> Option<Vec<Symbol>> {
    let mut p = match prefix {
        Some(pre) => q_path(arena, pre)?,
        None => Vec::new(),
    };
    p.push(get_name(arena, ident)?);
    Some(p)
}

/// `userDefinedTypes` + `getQualifiedName` (`Template.hs:146-155`) as one
/// key: the declaration class and its qualified name. `&&` in the reference
/// is lazy, so `getQualifiedName` is only ever asked of these six classes —
/// everything else has no key and is always kept.
fn dedup_key(arena: &ExpArena, id: ExpId) -> Option<(u8, Vec<Symbol>)> {
    let (class, scope, name) = match &arena[id].exp {
        Exp::Class { scope, name, .. } => (0u8, scope, *name),
        Exp::Struct { scope, name, .. } => (1, scope, *name),
        Exp::Union { scope, name, .. } => (2, scope, *name),
        Exp::Alias { scope, name, .. } => (3, scope, *name),
        Exp::Template { decl, .. } => {
            // `getQualifiedName (TemplateF _ a)` delegates to the decl.
            return Some((4, template_path(arena, *decl)?));
        }
        Exp::Function { scope, name, .. } => (5, scope, *name),
        _ => return None,
    };
    let sym = ident_sym(arena, name)?;
    let mut key = scope.segments().to_vec();
    key.push(sym);
    Some((class, key))
}

/// `isClosure = getQualifiedName template == data'closure'core'Closure`
/// (`Mangle.hs:31`, `Type.hs:121-122`) — `["@data@closure@core","Closure"]`
/// in IR components.
fn is_closure_path(interner: &Interner, t_path: &[Symbol]) -> bool {
    t_path.len() == 2
        && interner.resolve(t_path[0]) == "@data@closure@core"
        && interner.resolve(t_path[1]) == "Closure"
}

// --- the whole-corpus invariant ----------------------------------------------

/// **The pass's own invariant**, recomputed rather than trusted: a decided
/// instance either becomes exactly one materialised declaration bearing its
/// mangled name, or its mangling provably declines — never both, never
/// neither — the original template survives *after* its instances, and every
/// expansion frame in the output belongs to an instance the pass was handed,
/// at that instance's deciding site.
///
/// Per reachable `Template` of `before` whose qualified path the map claims
/// (a map entry naming no template of this module is another module's and is
/// skipped, the way `resolve` skips foreign sites):
///
/// * For every spec whose arguments re-mangle to `Some(bytes)`: **exactly
///   one** reachable declaration of `after` has the same class and scope as
///   the template's declaration and the name `leaf<|bytes|>`. Two specs
///   mangling to one name expect one declaration — `deduplicateInstances`'
///   collision is legal (`Template.hs:144-155`); a count of two means dedup
///   did not run, zero means the instance vanished.
/// * A spec whose arguments re-mangle to `None` expects **no** such
///   declaration — the decline must not have materialised anyway.
/// * A `Template` with the same path still stands in `after`, and within the
///   `Seq` that holds it every expected instance declaration precedes it —
///   instances first, original **last** (`Template.hs:115`).
///
/// Globally over `after`'s reachable nodes:
///
/// * every `via != ROOT` frame chain resolves to frames whose `instance` is
///   one of the map's ids and whose `site` is that spec's site — a copy that
///   wears a frame nobody minted, or the wrong deciding site, is caught here;
/// * every reachable declaration whose name contains `<|` is either one of
///   the expected names or already stood in `before` (a previous round's
///   instance) — materialisation mints declarations from specs and from
///   nothing else.
///
/// What it cannot see, having tried: the mangled *bytes* between the frame —
/// the decline clause reuses this module's own [`mangle_args`], so a wrong
/// byte in an arm passes both sides (the oracle-read unit tests own bytes);
/// the *contents* of an instance body — substitution, scope mangling, donors
/// — which the oracle-read and donor unit tests own; `Own`/`Donated` policy
/// inside a frame (invisible to every gate by design — donor tests own it);
/// sibling order *among* one template's instances (the caller's list order, a
/// stated divergence); and a copy that forgot to push a frame at all, which
/// wears `ROOT` and is this clause's blind spot precisely because `ROOT` is
/// every ordinary node — the provenance gate's clause 3 owns that failure
/// (two structurally different `Own` claimants at one key).
pub fn materialised_declarations_are_conserved(
    before: &crate::ModuleIr,
    after: &crate::ModuleIr,
    interner: &Interner,
    expansions: &ExpansionInterner,
    instances: &TemplateInstances,
) -> Result<(), String> {
    fn reachable(ir: &crate::ModuleIr, mut f: impl FnMut(&crate::ModuleIr, ExpId)) {
        let mut seen = vec![false; ir.arena.len()];
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            if std::mem::replace(&mut seen[id.index()], true) {
                continue;
            }
            f(ir, id);
            ir.arena[id].exp.children(&mut kids);
            stack.append(&mut kids);
        }
    }
    /// The declaration's class discriminant, name text and scope — the same
    /// identity `dedup_key` compares, resolved to text so `before`'s symbols
    /// and `after`'s compare across nothing (one interner serves both).
    fn decl_identity(
        ir: &crate::ModuleIr,
        interner: &Interner,
        id: ExpId,
    ) -> Option<(u8, String, Vec<Symbol>)> {
        let name = decl_name(&ir.arena, id)?;
        let sym = ident_sym(&ir.arena, name)?;
        let class = match &ir.arena[id].exp {
            Exp::Class { .. } => 0u8,
            Exp::Struct { .. } => 1,
            Exp::Union { .. } => 2,
            Exp::Alias { .. } => 3,
            Exp::Enum { .. } => 6,
            Exp::Function { .. } => 5,
            _ => return None,
        };
        let scope = decl_scope(&ir.arena, id)?.segments().to_vec();
        Some((class, interner.resolve(sym).to_string(), scope))
    }

    // Which templates of this module the map claims, and what each spec
    // expects.
    let mut spec_sites: FxHashMap<InstanceId, FileRange> = FxHashMap::default();
    for specs in instances.values() {
        for spec in specs {
            spec_sites.insert(spec.id, spec.site);
        }
    }

    let mut templates: Vec<(Vec<Symbol>, ExpId)> = Vec::new();
    reachable(before, |ir, id| {
        if let Exp::Template { decl, .. } = &ir.arena[id].exp
            && !matches!(ir.arena[*decl].exp, Exp::Typename)
            && let Some(path) = template_path(&ir.arena, *decl)
            && instances.contains_key(&path)
        {
            templates.push((path, *decl));
        }
    });

    // `after`'s declarations by identity, and its `Seq` spines for the order
    // clause.
    let mut after_decls: FxHashMap<(u8, String, Vec<Symbol>), u32> = FxHashMap::default();
    let mut after_seqs: Vec<Vec<ExpId>> = Vec::new();
    reachable(after, |ir, id| {
        if let Some(identity) = decl_identity(ir, interner, id) {
            *after_decls.entry(identity).or_default() += 1;
        }
        if let Exp::Seq(items) = &ir.arena[id].exp {
            after_seqs.push(items.clone());
        }
    });
    let mut before_mangled: FxHashSet<String> = FxHashSet::default();
    reachable(before, |ir, id| {
        if let Some((_, name, _)) = decl_identity(ir, interner, id)
            && kg_base::mentions_instance(&name)
        {
            before_mangled.insert(name);
        }
    });

    let mut expected_names: FxHashSet<String> = FxHashSet::default();
    for (t_path, decl) in &templates {
        let specs = &instances[t_path];
        if specs.is_empty() {
            continue;
        }
        let Some((class, leaf, scope)) = decl_identity(before, interner, *decl) else {
            continue;
        };
        let is_closure = is_closure_path(interner, t_path);
        let mut wanted: FxHashSet<String> = FxHashSet::default();
        for spec in specs {
            if let Some(bytes) =
                mangle_args(&after.arena, interner, spec.args, is_closure, &spec.stamps)
            {
                wanted.insert(format!("{leaf}{}", super::resolve::instance_suffix(&bytes)));
            }
        }
        for name in &wanted {
            expected_names.insert(name.clone());
            let count = after_decls
                .get(&(class, name.clone(), scope.clone()))
                .copied()
                .unwrap_or(0);
            if count != 1 {
                return Err(format!(
                    "template {t_path:?}: expected exactly one declaration named \
                     {name:?} in its scope, found {count} — one per decided \
                     instance, dedup collapsing collisions (Template.hs:115,144-155)"
                ));
            }
        }
        // The original survives, after its instances (`Template.hs:115`).
        let mut survivor_seen = false;
        for items in &after_seqs {
            let survivor = items.iter().position(|&i| {
                matches!(&after.arena[i].exp, Exp::Template { decl, .. }
                    if template_path(&after.arena, *decl).as_deref() == Some(t_path))
            });
            let Some(survivor) = survivor else { continue };
            survivor_seen = true;
            for (pos, &item) in items.iter().enumerate() {
                if let Some((c, n, s)) = decl_identity(after, interner, item)
                    && c == class
                    && s == scope
                    && wanted.contains(&n)
                    && pos > survivor
                {
                    return Err(format!(
                        "template {t_path:?}: instance {n:?} stands after the \
                         surviving original — instances first, original last \
                         (Template.hs:115)"
                    ));
                }
            }
        }
        if !survivor_seen {
            return Err(format!(
                "template {t_path:?} did not survive its own instantiation — \
                 the original is kept last (Template.hs:115)"
            ));
        }
    }

    // Frame discipline and minting, whole tree.
    let mut err = None;
    reachable(after, |ir, id| {
        if err.is_some() {
            return;
        }
        let via = ir.arena[id].src.via;
        if via != kg_base::Expansion::ROOT {
            for frame in expansions.frames(via) {
                match spec_sites.get(&frame.instance) {
                    None => {
                        err = Some(format!(
                            "node {} wears a frame for instance {:?} the pass was \
                             never handed",
                            id.index(),
                            frame.instance
                        ));
                    }
                    Some(site) if *site != frame.site => {
                        err = Some(format!(
                            "node {} wears a frame whose site {:?} is not its \
                             instance's deciding site {:?} (Map.union is \
                             left-biased, Template.hs:88)",
                            id.index(),
                            frame.site,
                            site
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
        if let Some((_, name, _)) = decl_identity(ir, interner, id)
            && kg_base::mentions_instance(&name)
            && !expected_names.contains(&name)
            && !before_mangled.contains(&name)
        {
            err = Some(format!(
                "declaration {name:?} appeared without a spec — materialisation \
                 mints declarations from decided instances and nothing else"
            ));
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// The reference is the grader here, not this port's reading of it.
///
/// Every expectation below was read off the reference frontend built by
/// `oracle/build.sh`, captured 2026-08-13, at
///
/// ```text
/// kg-oracle --tree  --roots-only --passes 8 --template-passes 9 \
///           --template-iterations 1 FILE
/// kg-oracle --notes --roots-only --passes 8 --template-passes 9 \
///           --template-iterations 1 FILE
/// ```
///
/// (`-I DIR --base EMPTY` for the module-headered closure fixture, whose
/// path must match its `module` header). `--passes 8` is not optional —
/// `removeTemplate` (`Frontend.hs:300-302`) deletes the output — and
/// `--template-iterations 1` prints the tree *after* the inter-round
/// `inferType`, so each fixture either cannot fold or its test says which
/// differences the fold owns. The transcripts were a session's scratch and
/// are not kept; each test's expectation is its own record of them.
///
/// Because our `Src` ranges and the reference's are different objects
/// (divergences #25, #26), every `--notes` expectation is asserted as donor
/// *identity* — the new node wears the same head as a named donor node —
/// never as a raw offset. The decided maps are built by the tests from the
/// fixtures' own instance nodes, the same `(site, args, stamps)` shape the
/// caller derives from `kg_db::instances`, with `InstanceId::of` minting the
/// content-addressed id. Fixtures lower through `lower_sorted` + `NoImports`,
/// like `resolve`'s: this pass's name arms fire on sorted forms only.
#[cfg(test)]
mod tests {
    use kg_base::{Expansion, FileId, Written};
    use kg_syntax::ast;
    use kg_syntax::AstNode;

    use super::*;
    use crate::rewrite::instantiate_templates;
    use crate::sort::NoImports;
    use crate::{lower_sorted, ModuleIr};

    fn build(text: &str) -> (ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "fixture must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        (lower_sorted(&file, FileId(0), &interner, &NoImports), interner)
    }

    fn walk(ir: &ModuleIr, mut f: impl FnMut(&ModuleIr, ExpId)) {
        let mut seen = vec![false; ir.arena.len()];
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            if std::mem::replace(&mut seen[id.index()], true) {
                continue;
            }
            f(ir, id);
            ir.arena[id].exp.children(&mut kids);
            stack.append(&mut kids);
        }
    }

    /// [`walk`] from one node down, subtree only.
    fn walk_from(ir: &ModuleIr, from: ExpId, mut f: impl FnMut(&ModuleIr, ExpId)) {
        let mut stack = vec![from];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            f(ir, id);
            ir.arena[id].exp.children(&mut kids);
            stack.append(&mut kids);
        }
    }

    /// Reachable `TemplateInstance`s whose base leaf is `name`, text order.
    fn instances_named(ir: &ModuleIr, interner: &Interner, name: &str) -> Vec<ExpId> {
        let mut out = Vec::new();
        walk(ir, |ir, id| {
            if let Exp::TemplateInstance { base, .. } = &ir.arena[id].exp
                && get_name(&ir.arena, *base).is_some_and(|s| interner.resolve(s) == name)
            {
                out.push(id);
            }
        });
        out.sort_by_key(|&id| ir.arena[id].src.head.range.start());
        out
    }

    /// Reachable declarations (the six templatable kinds) whose name text is
    /// `name`, text order.
    fn decls_named(ir: &ModuleIr, interner: &Interner, name: &str) -> Vec<ExpId> {
        let mut out = Vec::new();
        walk(ir, |ir, id| {
            if let Some(n) = decl_name(&ir.arena, id)
                && ident_sym(&ir.arena, n).is_some_and(|s| interner.resolve(s) == name)
            {
                out.push(id);
            }
        });
        out.sort_by_key(|&id| ir.arena[id].src.head.range.start());
        out
    }

    /// The one `Template` node whose declaration is named `name`.
    fn template_named(ir: &ModuleIr, interner: &Interner, name: &str) -> ExpId {
        let mut out = Vec::new();
        walk(ir, |ir, id| {
            if let Exp::Template { decl, .. } = &ir.arena[id].exp
                && let Some(n) = decl_name(&ir.arena, *decl)
                && ident_sym(&ir.arena, n).is_some_and(|s| interner.resolve(s) == name)
            {
                out.push(id);
            }
        });
        assert_eq!(out.len(), 1, "exactly one template named {name}");
        out[0]
    }

    /// An `InstanceSpec` decided at the given occurrence, `InstanceId` minted
    /// content-addressed the way `kg_db::instances`' `(template, key)` pair
    /// mints it.
    fn spec_at(ir: &ModuleIr, inst: ExpId, template: &str, key: &str) -> InstanceSpec {
        let Exp::TemplateInstance { args, .. } = ir.arena[inst].exp else {
            panic!("spec_at wants a TemplateInstance");
        };
        InstanceSpec {
            id: InstanceId::of(template, key),
            site: ir.arena[inst].src.head,
            args,
            stamps: Vec::new(),
            round: 1,
        }
    }

    fn scope_texts(interner: &Interner, q: &QualifiedName) -> Vec<String> {
        q.segments().iter().map(|s| interner.resolve(*s).to_string()).collect()
    }

    fn run(
        ir: &ModuleIr,
        interner: &Interner,
        exps: &ExpansionInterner,
        map: &TemplateInstances,
    ) -> (ModuleIr, InstantiateReport) {
        instantiate_templates(ir, interner, exps, map, None)
    }

    const HOLDER: &str = "template <typename T>\nstruct Holder\n{\n    T value;\n}\n\n\
         inline uint32 use()\n{\n    Holder<uint8> h;\n    return 0;\n}\n";

    /// A one-entry map deciding every written `name<…>` occurrence's shape as
    /// one instance of the template `name` — fixtures below write exactly one
    /// shape per template unless the test says otherwise.
    fn decide_one(ir: &ModuleIr, interner: &Interner, name: &str, key: &str) -> TemplateInstances {
        let t = template_named(ir, interner, name);
        let Exp::Template { decl, .. } = &ir.arena[t].exp else { unreachable!() };
        let path = template_path(&ir.arena, *decl).expect("named template has a path");
        let occurrences = instances_named(ir, interner, name);
        let site = *occurrences.last().expect("fixture writes a use site last");
        let mut map = TemplateInstances::default();
        map.insert(path, vec![spec_at(ir, site, name, key)]);
        map
    }

    /// `--tree`, iterations 1, `holder.k` (`sb/holder.it1.tree`):
    ///
    /// ```text
    /// SeqF [StructF {declScopeName = [], declName = IdentifierF "Holder<|$uint@8|>",
    ///        body = SeqF [VariableF {declScopeName = ["$quasi0"], …
    ///          varType = UnsignedF 8, declName = IdentifierF "value" …}]},
    ///       TemplateF {… declName = IdentifierF "Holder" …},
    ///       FunctionF {… declName = IdentifierF "use" …}]
    /// ```
    ///
    /// Instances **first**, original **last** (`Template.hs:115`), spliced
    /// into the enclosing `Seq` by the catch-all's `removeNestedSeq`
    /// (`:142`); the instance is a `Struct` whose only member kept its name
    /// (`value`) and its `$quasi0` scope (a struct member's scope does not
    /// spell the template name, so `mangleScope`'s prefix guard leaves it),
    /// and whose `varType` is the substituted `UnsignedF 8`
    /// (`resolveTypeParam`, `TypeCheck.hs:334-336`). The mention inside `use`
    /// stays written — `resolve` is the sibling pass, graded in its own
    /// module.
    #[test]
    fn instances_come_first_and_the_original_survives_last() {
        let (ir, interner) = build(HOLDER);
        let map = decide_one(&ir, &interner, "Holder", "<uint8>");
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);

        assert_eq!((report.made, report.declined, report.substituted), (1, 0, 1));

        // Top-level Seq: instance struct, template, use — in that order.
        let Exp::Seq(items) = &out.arena[out.root].exp else { panic!("root Seq") };
        assert_eq!(items.len(), 3, "the instance Seq was spliced into the root Seq");
        let inst = decls_named(&out, &interner, "Holder<|$uint@8|>");
        assert_eq!(inst.len(), 1);
        assert_eq!(items[0], inst[0], "instance first");
        assert_eq!(items[1], template_named(&out, &interner, "Holder"), "original last");

        // The instance body: `value` kept its text and its `$quasi0` scope;
        // its type is the substituted argument.
        let Exp::Struct { body, scope, .. } = &out.arena[inst[0]].exp else {
            panic!("instance is a Struct");
        };
        assert!(scope.is_empty(), "the root keeps its scope; only its name mangles");
        let Exp::Seq(members) = &out.arena[*body].exp else { panic!() };
        let Exp::Variable { ty, name, scope: var_scope, .. } = &out.arena[members[0]].exp else {
            panic!("member Variable");
        };
        assert!(matches!(out.arena[*ty].exp, Exp::Unsigned(8)), "T value → UnsignedF 8");
        assert_eq!(interner.resolve(ident_sym(&out.arena, *name).unwrap()), "value");
        assert_eq!(scope_texts(&interner, var_scope), ["$quasi0"]);

        // The survivor shares the original declaration node — `copyNote t
        // templateDecl` wears the decl's note and keeps its children.
        let Exp::Template { decl, .. } = &out.arena[items[1]].exp else { panic!() };
        let original_t = template_named(&ir, &interner, "Holder");
        let Exp::Template { decl: original_decl, .. } = &ir.arena[original_t].exp else {
            panic!()
        };
        assert_eq!(decl, original_decl, "the survivor's decl is the original node");
        assert_eq!(out.arena[items[1]].src.head, ir.arena[*original_decl].src.head);
        assert_eq!(out.arena[items[1]].src.written, Written::Donated);

        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
    }

    /// `--tree`, iterations 1, `interior.k` (`sb/interior.it1.tree`) — the
    /// class fixture, where members' scopes *do* spell the template name:
    ///
    /// ```text
    /// ClassF {declScopeName = [], declName = "Holder<|$uint@8|>", body = SeqF [
    ///   PublicF,
    ///   StructF {declScopeName = ["Holder<|$uint@8|>"], declName = "Pair", body = SeqF [
    ///     VariableF {declScopeName = ["Holder<|$uint@8|>","$quasi0"], varType = UnsignedF 8,
    ///                declName = "first" …} …]},
    ///   FunctionF {declScopeName = ["Holder<|$uint@8|>"], declName = "get" …}]}
    /// ```
    ///
    /// Only the outermost `declName` is mangled (`renote mangleDeclName`,
    /// `Template.hs:130-131`); `Pair`, `get`, `first`, `second` keep their
    /// text and their scopes gain the instance name at component
    /// `len(templateQualifiedName) - 1` (`mangleScope`, `:131-132`).
    #[test]
    fn interior_names_keep_their_text_and_gain_a_mangled_scope() {
        let (ir, interner) = build(
            "template <typename T>\nclass Holder\n{\npublic:\n    struct Pair\n    {\n\
             \x20       T first;\n        T second;\n    }\n\n    uint32 get()\n    {\n\
             \x20       return 0;\n    }\n}\n\n\
             inline uint32 use()\n{\n    Holder<uint8> h;\n    return 0;\n}\n",
        );
        let map = decide_one(&ir, &interner, "Holder", "<uint8>");
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);
        assert_eq!(report.made, 1);
        assert_eq!(report.substituted, 2, "T first; T second;");

        let inst = decls_named(&out, &interner, "Holder<|$uint@8|>");
        assert_eq!(inst.len(), 1);
        // The interior struct: name text kept, scope mangled.
        let pairs: Vec<ExpId> = decls_named(&out, &interner, "Pair")
            .into_iter()
            .filter(|&d| {
                let s = decl_scope(&out.arena, d).unwrap();
                scope_texts(&interner, s) == ["Holder<|$uint@8|>"]
            })
            .collect();
        assert_eq!(pairs.len(), 1, "the copied Pair wears the instance scope");
        // The scope walk rewrote this copy — and the rewrite keeps the
        // copy's provenance: still Own (the user's own text), still framed.
        // Donating here instead would empty the `written == Own` index for
        // exactly the declarations the auto-hints live on.
        assert_eq!(out.arena[pairs[0]].src.written, Written::Own);
        assert_eq!(exps.frames(out.arena[pairs[0]].src.via).len(), 1);
        // Its members: two levels deep, both components after the head kept.
        let Exp::Struct { body, .. } = &out.arena[pairs[0]].exp else { panic!() };
        let Exp::Seq(members) = &out.arena[*body].exp else { panic!() };
        let Exp::Variable { scope, ty, .. } = &out.arena[members[0]].exp else { panic!() };
        assert_eq!(scope_texts(&interner, scope), ["Holder<|$uint@8|>", "$quasi0"]);
        assert!(matches!(out.arena[*ty].exp, Exp::Unsigned(8)));
        // The copied member function too.
        let gets: Vec<ExpId> = decls_named(&out, &interner, "get")
            .into_iter()
            .filter(|&d| {
                let s = decl_scope(&out.arena, d).unwrap();
                scope_texts(&interner, s) == ["Holder<|$uint@8|>"]
            })
            .collect();
        assert_eq!(gets.len(), 1, "the copied get wears the instance scope");
        // And the original template body is untouched: Pair still stands in
        // scope ["Holder"].
        let originals: Vec<ExpId> = decls_named(&out, &interner, "Pair")
            .into_iter()
            .filter(|&d| {
                scope_texts(&interner, decl_scope(&out.arena, d).unwrap()) == ["Holder"]
            })
            .collect();
        assert_eq!(originals.len(), 1);

        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
    }

    /// The self-referential arm (`Template.hs:133-134`), `fact.k`. `--tree`
    /// at iterations 1 (`sb/fact.it1.tree`) holds, inside `fact<|@2|>`:
    ///
    /// ```text
    /// TemplateInstanceF ScopedNameF ["fact<|@2|>","$"] Nothing IdentifierF "fact"
    ///   SeqF [IntLiteralF {… literalValue = 1}]
    /// ```
    ///
    /// — still a `TemplateInstance`, base leaf still `fact` (the *original*
    /// head, `TemplateInstanceF a b` keeps the paramorphism's first `a`),
    /// args substituted, base chain scope-mangled. Two folds in that print
    /// belong to the inter-round `inferType`, not to this pass: `N - 1`
    /// folded to `1` (`evaluateConstexpr`) and the `$quasi1` scope component
    /// dropped with the `static if` (`staticControl`) — our copy keeps
    /// `2 - 1` written out and the full `["fact<|@2|>","$","$quasi1"]` scope.
    #[test]
    fn a_self_referential_instance_stays_an_instance_for_the_next_round() {
        let (ir, interner) = build(
            "template <auto N>\ninline uint32 fact()\n{\n    static if (N == 0)\n    {\n\
             \x20       return 1;\n    }\n    else\n    {\n        return N * fact<N - 1>();\n\
             \x20   }\n}\n\ninline uint32 use()\n{\n    return fact<2>();\n}\n",
        );
        let facts = instances_named(&ir, &interner, "fact");
        assert_eq!(facts.len(), 2, "the body's fact<N-1> and the use site's fact<2>");
        let body_inst = facts[0];
        let use_site = facts[1];
        let Exp::TemplateInstance { base: original_base, .. } = ir.arena[body_inst].exp else {
            panic!()
        };

        let t = template_named(&ir, &interner, "fact");
        let Exp::Template { decl, .. } = &ir.arena[t].exp else { unreachable!() };
        let path = template_path(&ir.arena, *decl).unwrap();
        let mut map = TemplateInstances::default();
        map.insert(path, vec![spec_at(&ir, use_site, "fact", "<int:2>")]);

        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);
        assert_eq!((report.made, report.kept_instances), (1, 1));

        // The materialised fact<|@2|> holds a TemplateInstance still named
        // `fact` — an instance for the next round.
        let kept = instances_named(&out, &interner, "fact");
        // Text order puts the original body's instance and the kept copy at
        // the same head; find the copy by its mangled base scope.
        let copy: Vec<ExpId> = kept
            .iter()
            .copied()
            .filter(|&id| {
                let Exp::TemplateInstance { base, .. } = &out.arena[id].exp else {
                    return false;
                };
                matches!(&out.arena[*base].exp, Exp::ScopedName { scope, .. }
                    if scope_texts(&interner, scope)
                        == ["fact<|@2|>", "$", "$quasi1"])
            })
            .collect();
        assert_eq!(copy.len(), 1, "the kept instance's base chain is scope-mangled");
        let Exp::TemplateInstance { base, args } = &out.arena[copy[0]].exp else { panic!() };

        // The kept base wears the original base's head — `TemplateInstanceF a b`
        // keeps `a` — but its scope rewrite is an invention standing beside
        // the survivor's own base: Donated, unframed.
        assert_ne!(*base, original_base, "the scope walk had to rewrite the shared base");
        assert_eq!(out.arena[*base].src.head, ir.arena[original_base].src.head);
        assert_eq!(out.arena[*base].src.written, Written::Donated);
        assert_eq!(out.arena[*base].src.via, Expansion::ROOT, "shared structure is unframed");

        // The original base still stands untouched in the survivor's body.
        let survivors: Vec<ExpId> = kept
            .iter()
            .copied()
            .filter(|&id| {
                let Exp::TemplateInstance { base, .. } = &out.arena[id].exp else {
                    return false;
                };
                *base == original_base
            })
            .collect();
        assert_eq!(survivors.len(), 1, "the template body's own instance is byte-identical");

        // The kept args: `N - 1` with N substituted by the argument `2`,
        // written out — the fold to `1` is inference's, not ours.
        let Exp::Seq(arg_items) = &out.arena[*args].exp else { panic!() };
        let Exp::Binary(_, lhs, _) = &out.arena[arg_items[0]].exp else {
            panic!("our copy keeps 2 - 1 unfolded");
        };
        assert!(
            matches!(&out.arena[*lhs].exp, Exp::IntLiteral { value, .. } if value.to_u32() == Some(2)),
            "N substituted by the argument 2"
        );

        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
    }

    /// The bare self-mention arm (`Template.hs:135-136`), `selfmention.k`.
    /// `--tree`, iterations 1 (`sb/selfmention.it1.tree`), inside
    /// `Node<|$uint@8|>`:
    ///
    /// ```text
    /// TemplateInstanceF ScopedNameF ["$quasi1"] Nothing IdentifierF "Wrap"
    ///   SeqF [NamedValueF QualifiedIdentifierF ScopedNameF ["$quasi1"] Nothing
    ///         IdentifierF "Node<|$uint@8|>"]
    /// ```
    ///
    /// — the bare `Node` argument mention *is* the template
    /// (`getQualifiedName x == templateQualifiedName`), so its leaf mangles
    /// in place, while `Wrap<…>` stays an instance for the next round.
    #[test]
    fn a_bare_mention_of_the_template_mangles_its_leaf() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Wrap\n{\n    uint8 x;\n}\n\n\
             template <typename T>\nstruct Node\n{\n    Wrap<Node> w;\n}\n\n\
             inline uint32 use()\n{\n    Node<uint8> n;\n    return 0;\n}\n",
        );
        let map = decide_one(&ir, &interner, "Node", "<uint8>");
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);
        assert_eq!(report.made, 1);

        // The copy's Wrap argument now mentions the instance by its mangled
        // name; the original body's mention is untouched.
        let mut mangled_mentions = 0;
        let mut written_mentions = 0;
        walk(&out, |ir, id| {
            if let Exp::Identifier(s) = &ir.arena[id].exp {
                match interner.resolve(*s).as_str() {
                    "Node<|$uint@8|>" => mangled_mentions += 1,
                    "Node" => written_mentions += 1,
                    _ => {}
                }
            }
        });
        // Mangled: the instance's declName and the argument mention. Written:
        // the survivor's declName, its body's argument mention, and the use
        // site's base — `resolve`, the sibling pass, is what renames the use
        // site, and it did not run here.
        assert_eq!(mangled_mentions, 2, "declName + the mangled argument mention");
        assert_eq!(written_mentions, 3, "survivor name + survivor mention + use site");
        // Wrap materialised nothing — its shape is undecided this round.
        assert!(decls_named(&out, &interner, "Wrap<|@Node|>").is_empty());
        assert_eq!(instances_named(&out, &interner, "Wrap").len(), 2, "copy + original");

        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
    }

    /// The `Typename` early return (`Template.hs:113`) and the
    /// `null templateInstances` early return (`:114`), on `ttparam.k`. The
    /// oracle at iterations 1 (`sb/ttparam.it1.tree`) keeps the
    /// template-template *parameter*'s own `TemplateF {… templateDecl =
    /// TypenameF}` untouched inside the survivor's params while `W<|@Box|>`
    /// materialises beside it; `Box`, with no decided instance, is untouched
    /// entirely.
    #[test]
    fn typename_parameters_and_undecided_templates_are_left_alone() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Box\n{\n    T inner;\n}\n\n\
             template <template <typename> typename TT>\nstruct W\n{\n    TT<uint8> item;\n}\n\n\
             inline uint32 use()\n{\n    W<Box> w;\n    return 0;\n}\n",
        );
        let map = decide_one(&ir, &interner, "W", "<Box>");
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);
        assert_eq!(report.made, 1);

        // Box — `null templateInstances` — kept its node identity: nothing
        // about it changed, so the driver's identity rule kept its id.
        assert_eq!(
            template_named(&out, &interner, "Box"),
            template_named(&ir, &interner, "Box"),
            "an undecided template is byte-for-byte the node it was"
        );

        // The parameter's own Template-of-Typename survives inside the
        // surviving W. Its id is unchanged: the Typename guard declined it.
        let w = template_named(&out, &interner, "W");
        let Exp::Template { params, .. } = &out.arena[w].exp else { panic!() };
        let Exp::TemplateParam { kind, .. } = &out.arena[params[0]].exp else { panic!() };
        let Exp::Template { decl, .. } = &out.arena[*kind].exp else {
            panic!("a template-template parameter's kind is a Template");
        };
        assert!(matches!(out.arena[*decl].exp, Exp::Typename));
        let w_before = template_named(&ir, &interner, "W");
        let Exp::Template { params: params_before, .. } = &ir.arena[w_before].exp else {
            panic!()
        };
        assert_eq!(params[0], params_before[0], "the parameter node is untouched");

        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
    }

    /// An empty map is a whole-module no-op: every node keeps its id and
    /// nothing is appended — the `desugar` driver's identity rule, end to
    /// end.
    #[test]
    fn an_empty_map_changes_nothing() {
        let (ir, interner) = build(HOLDER);
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &TemplateInstances::default());
        assert_eq!(out.root, ir.root);
        assert_eq!(out.arena.len(), ir.arena.len());
        assert_eq!(report.made, 0);
    }

    /// `deduplicateInstances` (`Template.hs:144-155`) collapsing the closure
    /// mangler collision (`Mangle.hs:42-43`). Module-headered fixture at
    /// `sb/mods/data/closure/core.k`, oracle with `-I sb/mods --base` an
    /// empty `base.k`, iterations 1 (`sb/closure.it1.tree`):
    ///
    /// ```text
    /// StructF {declScopeName = ["@data@closure@core"], declName = "Closure<||>",
    ///   body = SeqF [VariableF {… varType = FunctionTypeF SeqF []
    ///     SeqF [FunctionTypeParamF SeqF [] BooleanF Nothing] BooleanF …}]}
    /// ```
    ///
    /// — **one** materialised `Closure<||>` for two written argument lists,
    /// and its body is the **`(bool) -> bool`** one: `Map.toList` orders by
    /// the argument tree's structural `Ord` and `BooleanF` sorts before
    /// `UnsignedF`, so `nubBy` keeps it. This port's caller supplies the
    /// order; the test hands specs in the oracle's order and pins
    /// first-wins.
    #[test]
    fn deduplicate_instances_collapses_a_mangler_collision() {
        let (ir, interner) = build(
            "module data.closure.core\n{\n    Closure\n}\n\n\
             template <typename FT>\nstruct Closure\n{\n    FT f;\n}\n\n\
             inline uint32 use()\n{\n    Closure<(uint8) -> uint8> a;\n\
             \x20   Closure<(bool) -> bool> b;\n    return 0;\n}\n",
        );
        let sites = instances_named(&ir, &interner, "Closure");
        assert_eq!(sites.len(), 2);
        let t = template_named(&ir, &interner, "Closure");
        let Exp::Template { decl, .. } = &ir.arena[t].exp else { unreachable!() };
        let path = template_path(&ir.arena, *decl).unwrap();
        assert_eq!(
            path.iter().map(|&s| interner.resolve(s).to_string()).collect::<Vec<_>>(),
            ["@data@closure@core", "Closure"],
            "the module namespace is one mangled component"
        );
        // The oracle's order: (bool) -> bool first (structural Ord), then
        // (uint8) -> uint8. Distinct ids — they are distinct instances that
        // happen to collide in name.
        let mut map = TemplateInstances::default();
        map.insert(
            path,
            vec![
                spec_at(&ir, sites[1], "data.closure.core.Closure", "<(bool) -> bool>"),
                spec_at(&ir, sites[0], "data.closure.core.Closure", "<(uint8) -> uint8>"),
            ],
        );
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);
        assert_eq!(report.made, 2, "both instances materialise…");
        assert_eq!(report.deduplicated, 1, "…and the Seq keeps one");

        let survivors = decls_named(&out, &interner, "Closure<||>");
        assert_eq!(survivors.len(), 1);
        // First wins: the survivor's body is the (bool) -> bool substitution.
        let Exp::Struct { body, .. } = &out.arena[survivors[0]].exp else { panic!() };
        let Exp::Seq(members) = &out.arena[*body].exp else { panic!() };
        let Exp::Variable { ty, .. } = &out.arena[members[0]].exp else { panic!() };
        let Exp::FunctionType { ret, .. } = &out.arena[*ty].exp else {
            panic!("FT f; substituted with the function type argument");
        };
        assert!(
            matches!(out.arena[*ret].exp, Exp::Boolean),
            "nubBy keeps the first — the (bool) -> bool instance"
        );

        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
    }

    /// `validateArg`'s stamp reaches the declaration name (`stamp.k`).
    /// `--tree`, iterations 1 (`sb/stamp.it1.tree`): `W2<300>` and `W2<44>`
    /// under a `uint8 N` parameter are **one** instance named `W2<|@44|>` —
    /// the reference re-stamped the literal in its tree before deciding; here
    /// the census keys them together and the stamp travels in
    /// [`InstanceSpec::stamps`] to the mangled bytes — and, since the copy
    /// re-stamps the list it reads (`make_instance`), to the body: see
    /// [`the_copys_substituted_literal_is_the_narrowed_one`].
    #[test]
    fn the_stamped_name_and_one_instance_for_two_stamped_sites() {
        let (ir, interner) = build(
            "template <uint8 N>\nstruct W2\n{\n    uint8 v;\n}\n\n\
             inline uint32 use()\n{\n    W2<300> c;\n    W2<44> d;\n    return 0;\n}\n",
        );
        let sites = instances_named(&ir, &interner, "W2");
        assert_eq!(sites.len(), 2);
        let t = template_named(&ir, &interner, "W2");
        let Exp::Template { decl, .. } = &ir.arena[t].exp else { unreachable!() };
        let path = template_path(&ir.arena, *decl).unwrap();
        // One key, one spec, sited at the first occurrence (Map.union's left
        // bias), stamped to the parameter's width.
        let mut spec = spec_at(&ir, sites[0], "W2", "<uint8:44>");
        spec.stamps = vec![Some(LiteralType::unsigned(8))];
        let mut map = TemplateInstances::default();
        map.insert(path, vec![spec]);
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);
        assert_eq!(report.made, 1);
        assert_eq!(decls_named(&out, &interner, "W2<|@44|>").len(), 1, "300 truncated to @44");
        assert!(decls_named(&out, &interner, "W2<|@300|>").is_empty());

        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
    }

    /// `validateArg` rewrites the literal *in the tree* before `instantiate`
    /// reads it (`TypeCheck.hs:281-283`), so the reference's copy of
    /// `static assert(X == Y)` under `T5<uint5, 78, 14>` holds `14u5 == 14`
    /// and passes (`test/syntax/constexpr.k`, two `expected:0`s). Without
    /// the re-stamp the copy folds `78 == 14` and invents the failure —
    /// which is how this was found, by the twin's diagnostics landing.
    /// Falsified by substituting `arg` unchanged: the literal found is
    /// `300` with no stamp.
    #[test]
    fn the_copys_substituted_literal_is_the_narrowed_one() {
        let (ir, interner) = build(
            "template <uint8 N>\nstruct W2\n{\n    uint<N> v;\n}\n\n\
             inline uint32 use()\n{\n    W2<300> c;\n    return 0;\n}\n",
        );
        let sites = instances_named(&ir, &interner, "W2");
        let t = template_named(&ir, &interner, "W2");
        let Exp::Template { decl, .. } = &ir.arena[t].exp else { unreachable!() };
        let path = template_path(&ir.arena, *decl).unwrap();
        let mut spec = spec_at(&ir, sites[0], "W2", "<uint8:44>");
        spec.stamps = vec![Some(LiteralType::unsigned(8))];
        let mut map = TemplateInstances::default();
        map.insert(path, vec![spec]);
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);
        assert_eq!(report.made, 1);

        // The copy's `uint<N>` holds the narrowed, stamped literal; the
        // written mention (still in the arena, unreachable) keeps `300`.
        let copy = decls_named(&out, &interner, "W2<|@44|>")[0];
        let mut found = Vec::new();
        walk_from(&out, copy, |ir, id| {
            if let Exp::IntLiteral { ty, value } = &ir.arena[id].exp {
                found.push((*ty, value.to_u32()));
            }
        });
        assert_eq!(found, [(Some(LiteralType::unsigned(8)), Some(44))]);

        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
    }

    // --- donors, read off `--notes` ------------------------------------------

    /// `--notes`, iterations 1, `holder.k` (`sb/holder.it1.notes`), the
    /// substituted member type:
    ///
    /// ```text
    /// varType = UnsignedF 8 @Typed {… SrcStack (Src {offset = 42, … Pos 4 …})
    ///                                 (Src {offset = 80, … Pos 9 …})}
    /// ```
    ///
    /// — offset 42 is the **`T`** of `T value;`, framed by the deciding site
    /// at offset 80; the written `uint8` argument sits at a different offset
    /// entirely. `argAt` is `unNote . unfix` (`TypeCheck.hs:331-337`): the
    /// substituted node wears the `TypeParam`'s head plus a frame, never the
    /// argument's root range. Donated: the text at that head is `T`.
    /// Falsified by donating the argument's src instead — the first two
    /// asserts flip.
    #[test]
    fn the_substituted_argument_wears_the_type_params_head_framed() {
        let (ir, interner) = build(HOLDER);
        // The template body's TypeParam and the use site's argument node.
        let mut type_param = None;
        walk(&ir, |ir, id| {
            if matches!(ir.arena[id].exp, Exp::TypeParam { .. }) {
                type_param = Some(id);
            }
        });
        let type_param = type_param.expect("T value;");
        let use_site = *instances_named(&ir, &interner, "Holder").last().unwrap();
        let Exp::TemplateInstance { args, .. } = ir.arena[use_site].exp else { panic!() };
        let Exp::Seq(arg_items) = ir.arena[args].exp.clone() else { panic!() };
        let written_arg = arg_items[0];

        let map = decide_one(&ir, &interner, "Holder", "<uint8>");
        let exps = ExpansionInterner::new();
        let (out, _) = run(&ir, &interner, &exps, &map);

        let inst = decls_named(&out, &interner, "Holder<|$uint@8|>")[0];
        let Exp::Struct { body, .. } = &out.arena[inst].exp else { panic!() };
        let Exp::Seq(members) = &out.arena[*body].exp else { panic!() };
        let Exp::Variable { ty, .. } = &out.arena[members[0]].exp else { panic!() };

        let sub = &out.arena[*ty];
        assert_eq!(sub.src.head, ir.arena[type_param].src.head, "the TypeParam's head");
        assert_ne!(sub.src.head, ir.arena[written_arg].src.head, "never the argument's");
        assert_eq!(sub.src.written, Written::Donated);
        let frames = exps.frames(sub.src.via);
        assert_eq!(frames.len(), 1, "plus exactly one frame");
        assert_eq!(frames[0].site, ir.arena[use_site].src.head, "the deciding site");
    }

    /// `--notes`, iterations 1, `holder.k` (`sb/holder.it1.notes`), the
    /// mangled declaration name against the body copy beside it:
    ///
    /// ```text
    /// declName = IdentifierF "Holder<|$uint@8|>"
    ///   @Typed {… SrcStack (Src {offset = 29, …}) (Src {offset = 80, …})}
    /// declName = IdentifierF "value"
    ///   @Typed {… SrcStack (Src {offset = 44, …}) (Src {offset = 80, …})}
    /// ```
    ///
    /// — the mangled name wears the **unmangled** name's range (offset 29,
    /// six characters under a seventeen-character name): `Donated`, no
    /// position question answered from it. The body copy (`value`, offset
    /// 44) keeps its own head **and stays `Own`** — an instantiated copy is
    /// the user's own text — with the same single frame. Marking copies
    /// `Donated` empties every `written == Own` index inside instances;
    /// marking the name `Own` is a lie about its text.
    #[test]
    fn the_mangled_decl_name_is_donated_inside_an_own_body() {
        let (ir, interner) = build(HOLDER);
        let original = decls_named(&ir, &interner, "Holder")[0];
        let original_name = decl_name(&ir.arena, original).unwrap();
        let use_site = *instances_named(&ir, &interner, "Holder").last().unwrap();

        let map = decide_one(&ir, &interner, "Holder", "<uint8>");
        let exps = ExpansionInterner::new();
        let (out, _) = run(&ir, &interner, &exps, &map);

        let inst = decls_named(&out, &interner, "Holder<|$uint@8|>")[0];
        let name = decl_name(&out.arena, inst).unwrap();
        let name_node = &out.arena[name];
        assert_eq!(name_node.src.head, ir.arena[original_name].src.head);
        assert_eq!(name_node.src.written, Written::Donated, "its text is not its range's");
        assert_eq!(exps.frames(name_node.src.via).len(), 1, "framed like the copy around it");

        // The body copy beside it: own head, still Own, same frame.
        let Exp::Struct { body, .. } = &out.arena[inst].exp else { panic!() };
        let Exp::Seq(members) = &out.arena[*body].exp else { panic!() };
        let Exp::Variable { name: value_name, .. } = &out.arena[members[0]].exp else {
            panic!()
        };
        let value_node = &out.arena[*value_name];
        assert_eq!(value_node.src.written, Written::Own, "a copy is the user's own text");
        let frames = exps.frames(value_node.src.via);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].site, ir.arena[use_site].src.head);
        assert_eq!(frames[0].parent, Expansion::ROOT);
        // And the instance root itself: the declaration's own head, Own,
        // framed — `n' <> n` keeps the copied node's note on top
        // (sb/holder.it1.notes shows offset 22, the struct declaration).
        let root_node = &out.arena[inst];
        let Exp::Template { decl, .. } = &ir.arena[template_named(&ir, &interner, "Holder")].exp
        else {
            panic!()
        };
        assert_eq!(root_node.src.head, ir.arena[*decl].src.head);
        assert_eq!(root_node.src.written, Written::Own);
    }

    /// `--notes`, iterations 1, `ttparam.k` (`sb/ttparam.it1.notes`), the
    /// substituted template-template argument:
    ///
    /// ```text
    /// TemplateInstanceF ScopedNameF ["use","$"] Nothing IdentifierF "Box"
    ///     @Typed {… Src {offset = 156, … Pos 15 …}}   ← bare, unframed
    ///   SeqF [UnsignedF 8 @Typed {… SrcStack (Src {offset = 112, …}) …}]
    ///   @Typed {… SrcStack (Src {offset = 109, … Pos 10 …}) …}
    /// ```
    ///
    /// — the argument `Box` was written wrapped (`TypeIdentifierF`), the
    /// substitution put that wrapper at the `TypeParam`'s position, and
    /// `removeNestedTypeIdentifier`'s `TemplateInstanceF (TypeIdentifier _ a) b`
    /// arm (`Desugar.hs:127-128`) collapsed it — so what stands in base
    /// position is the argument's **child**, verbatim: the same arena node,
    /// its own use-site head, unframed, still `Own`. Only the argument's
    /// children survive; its root note is discarded with the wrapper.
    #[test]
    fn the_arguments_children_survive_unframed_as_the_same_nodes() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Box\n{\n    T inner;\n}\n\n\
             template <template <typename> typename TT>\nstruct W\n{\n    TT<uint8> item;\n}\n\n\
             inline uint32 use()\n{\n    W<Box> w;\n    return 0;\n}\n",
        );
        let use_site = *instances_named(&ir, &interner, "W").last().unwrap();
        let Exp::TemplateInstance { args, .. } = ir.arena[use_site].exp else { panic!() };
        let Exp::Seq(arg_items) = ir.arena[args].exp.clone() else { panic!() };
        // The written argument: a TypeIdentifier wrapping the name chain.
        let Exp::TypeIdentifier(_, arg_child) = ir.arena[arg_items[0]].exp else {
            panic!("the sorted lowering wraps a type argument, as the reference does");
        };

        let map = decide_one(&ir, &interner, "W", "<Box>");
        let exps = ExpansionInterner::new();
        let (out, _) = run(&ir, &interner, &exps, &map);

        let inst = decls_named(&out, &interner, "W<|@Box|>")[0];
        let Exp::Struct { body, .. } = &out.arena[inst].exp else { panic!() };
        let Exp::Seq(members) = &out.arena[*body].exp else { panic!() };
        let Exp::Variable { ty, .. } = &out.arena[members[0]].exp else { panic!() };
        let Exp::TypeIdentifier(_, ti) = &out.arena[*ty].exp else { panic!() };
        let Exp::TemplateInstance { base, args: inner_args } = &out.arena[*ti].exp else {
            panic!("TT<uint8> stays an instance — decided next round");
        };
        assert_eq!(
            *base, arg_child,
            "the collapsed base IS the argument's child node — shared, not copied"
        );
        // Verbatim: the shared node's Src is exactly what the lowering gave
        // it. (Its `written` happens to be `Donated` here — the sorted
        // lowering demotes a chain's inner nodes, divergence #24's
        // inner-leaf pair — which is the lowering's fact, not this pass's.)
        assert_eq!(out.arena[*base].src, ir.arena[arg_child].src);
        assert_eq!(out.arena[*base].src.via, Expansion::ROOT, "unframed");
        // While the copied argument list beside it is framed.
        let Exp::Seq(inner_items) = &out.arena[*inner_args].exp else { panic!() };
        assert!(matches!(out.arena[inner_items[0]].exp, Exp::Unsigned(8)));
        assert_eq!(exps.frames(out.arena[inner_items[0]].src.via).len(), 1);
    }

    // --- setTypeOfInstance ----------------------------------------------------

    /// `showPretty` of a name (`PrettyPrint.hs:144-148,191-194`): a
    /// qualifier that is one internal segment — a module namespace, `@…` —
    /// is left out, any other kept: `memory_ecc` and `Outer::Inner`. Red with
    /// the qualifier always kept, or always dropped.
    #[test]
    fn a_name_is_shown_without_its_module_namespace() {
        use kg_base::{FileId, FileRange, TextRange};
        let interner = Interner::new();
        let mut arena = ExpArena::new();
        let src = Src::own(FileRange::new(FileId(0), TextRange::new(0.into(), 1.into())));
        let k = kg_syntax::SyntaxKind::PATH;
        let id = |arena: &mut ExpArena, s: &str| arena.alloc(Exp::Identifier(interner.intern(s)), src, k);
        let scoped = |arena: &mut ExpArena, prefix: Option<ExpId>, ident: ExpId| {
            arena.alloc(Exp::ScopedName { scope: QualifiedName::new([]), prefix, ident }, src, k)
        };
        let ns = id(&mut arena, "@data@memory");
        let ns = scoped(&mut arena, None, ns);
        let leaf = id(&mut arena, "memory_ecc");
        let internal = scoped(&mut arena, Some(ns), leaf);
        let internal = arena.alloc(Exp::TypeIdentifier(QualifiedName::new([]), internal), src, k);
        assert_eq!(pretty_name(&arena, &interner, internal).as_deref(), Some("memory_ecc"));

        let outer = id(&mut arena, "Outer");
        let outer = scoped(&mut arena, None, outer);
        let inner = id(&mut arena, "Inner");
        let kept = scoped(&mut arena, Some(outer), inner);
        let kept = arena.alloc(Exp::QualifiedIdentifier(kept), src, k);
        assert_eq!(pretty_name(&arena, &interner, kept).as_deref(), Some("Outer::Inner"));
    }

    /// `setTypeOfInstance` (`Type.hs:426-440`). `--notes`, iterations 1,
    /// `holder.k` (`sb/holder.it1.notes`), the instance root's type:
    ///
    /// ```text
    /// typeof = TType (TInstance (TStruct …) ["Holder"] [("T",TTypeArg (TUnsigned 8))])
    /// ```
    ///
    /// — the concrete half (`TStruct …`, and the outer `TType`) is the
    /// inter-round `inferType` resolving what `setTypeOfInstance` installed
    /// as `TInstance TUnresolved`; the template name and the `TTypeArg`
    /// pairing are this pass's. The key is the copy's — same range and kind
    /// as the template declaration, discriminated by `via`: the fifth
    /// field's first production write.
    #[test]
    fn set_type_of_instance_types_the_root_under_its_expansion_key() {
        let (ir, interner) = build(HOLDER);
        let map = decide_one(&ir, &interner, "Holder", "<uint8>");
        let exps = ExpansionInterner::new();
        let (out, report) = run(&ir, &interner, &exps, &map);
        assert_eq!(report.type_declines, 0);
        assert_eq!(report.type_writes.len(), 1);

        let (key, ty) = &report.type_writes[0];
        let inst = decls_named(&out, &interner, "Holder<|$uint@8|>")[0];
        let node = &out.arena[inst];
        assert_eq!(key.offset, node.src.head.range.start());
        assert_eq!(key.end, node.src.head.range.end());
        assert_eq!(key.kind, node.kind);
        assert_eq!(key.via, node.src.via);
        assert_ne!(key.via, Expansion::ROOT, "the fifth field carries the expansion");

        let Ty::Instance { concrete, template, args } = ty else {
            panic!("TInstance TUnresolved s args (Type.hs:431)");
        };
        assert_eq!(**concrete, Ty::Unresolved);
        assert_eq!(scope_texts(&interner, template), ["Holder"]);
        assert_eq!(args.len(), 1);
        assert_eq!(interner.resolve(args[0].0), "T");
        assert_eq!(
            args[0].1,
            TArg::Type(TyRef::new(Ty::Unsigned(8))),
            "TTypeArg (TUnsigned 8) — a written builtin needs no table"
        );
    }

    /// `tArg (IntLiteral _ x) = TIntArg x` (`Type.hs:432`) over the value
    /// `validateArg` left in the reference's tree — the stamped one: the
    /// `stamp.k` instance types as `TIntArg 44` for the written `300`.
    /// An argument `tArg` cannot spell declines the whole write, counted.
    #[test]
    fn t_arg_stamps_literals_and_declines_what_it_cannot_spell() {
        let (ir, interner) = build(
            "template <uint8 N>\nstruct W2\n{\n    uint8 v;\n}\n\n\
             inline uint32 use()\n{\n    W2<300> c;\n    return 0;\n}\n",
        );
        let sites = instances_named(&ir, &interner, "W2");
        let t = template_named(&ir, &interner, "W2");
        let Exp::Template { decl, .. } = &ir.arena[t].exp else { unreachable!() };
        let path = template_path(&ir.arena, *decl).unwrap();
        let mut spec = spec_at(&ir, sites[0], "W2", "<uint8:44>");
        spec.stamps = vec![Some(LiteralType::unsigned(8))];
        let mut map = TemplateInstances::default();
        map.insert(path, vec![spec]);
        let exps = ExpansionInterner::new();
        let (_, report) = run(&ir, &interner, &exps, &map);
        assert_eq!(report.type_writes.len(), 1);
        let Ty::Instance { args, .. } = &report.type_writes[0].1 else { panic!() };
        assert!(
            matches!(&args[0].1, TArg::Int(v) if v.to_u32() == Some(44)),
            "TIntArg over the re-stamped literal, not the written 300"
        );
    }

    /// `tArg e = … TArg (showPretty e)` (`Type.hs:437`) for an aggregate
    /// value: the note holds the list as written, `{10, false}` — what the
    /// walk's display name reads (`Go<helper.struct_constexpr::Foo, {10,
    /// false}>`, `struct-constexpr.k::0`, the recorded walk) — and `{}`.
    #[test]
    fn t_arg_shows_an_aggregate_value_as_written() {
        let (ir, interner) = build(
            "struct S\n{\n    uint8 a;\n    bool b;\n}\n\n\
             template <S V>\nstruct W3\n{\n    uint8 v;\n}\n\n\
             inline uint32 use()\n{\n    W3<{10, false}> c;\n    W3<{}> d;\n    return 0;\n}\n",
        );
        let sites = instances_named(&ir, &interner, "W3");
        let t = template_named(&ir, &interner, "W3");
        let Exp::Template { decl, .. } = &ir.arena[t].exp else { unreachable!() };
        let path = template_path(&ir.arena, *decl).unwrap();
        let mut map = TemplateInstances::default();
        map.insert(path, vec![spec_at(&ir, sites[0], "W3", "<{10, false}>"), spec_at(&ir, sites[1], "W3", "<{}>")]);
        let exps = ExpansionInterner::new();
        let (_, report) = run(&ir, &interner, &exps, &map);
        let mut shown: Vec<String> = report
            .type_writes
            .iter()
            .map(|(_, ty)| match ty {
                Ty::Instance { args, .. } => match &args[0].1 {
                    TArg::Expr(s) => s.to_string(),
                    other => format!("{other:?}"),
                },
                other => format!("{other:?}"),
            })
            .collect();
        shown.sort();
        assert_eq!(shown, vec!["{10, false}", "{}"]);
    }

    /// The survey's falsification, kept: two instances sharing one
    /// `InstanceId` share every expansion key — the widened provenance gate
    /// goes red 180/180 on exactly this plant (`docs/journal.md`, "The fifth
    /// field works…") — and the invariant's frame clause catches it here
    /// without that gate: the shared frame's stored site (first intern wins)
    /// cannot match both specs' deciding sites.
    #[test]
    fn two_instances_sharing_an_id_are_caught() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Holder\n{\n    T value;\n}\n\n\
             inline uint32 use()\n{\n    Holder<uint8> h;\n    Holder<uint16> j;\n\
             \x20   return 0;\n}\n",
        );
        let sites = instances_named(&ir, &interner, "Holder");
        assert_eq!(sites.len(), 2);
        let t = template_named(&ir, &interner, "Holder");
        let Exp::Template { decl, .. } = &ir.arena[t].exp else { unreachable!() };
        let path = template_path(&ir.arena, *decl).unwrap();
        let mut a = spec_at(&ir, sites[0], "Holder", "<uint8>");
        let b = spec_at(&ir, sites[1], "Holder", "<uint8>");
        a.id = b.id; // the collision — content-addressing exists to prevent this
        let mut map = TemplateInstances::default();
        map.insert(path, vec![a, b]);
        let exps = ExpansionInterner::new();
        let (out, _) = run(&ir, &interner, &exps, &map);
        let err = materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map)
            .expect_err("two instances share one id, so they share one frame");
        assert!(err.contains("deciding site"), "{err}");
    }

    // --- the invariant bites --------------------------------------------------

    /// Both directions of [`materialised_declarations_are_conserved`] with no
    /// unit assertion in front: pairing the input with itself reports the
    /// missing instance declaration, and a node planted with a frame for an
    /// unknown instance trips the minting clause.
    #[test]
    fn the_invariant_bites() {
        let (ir, interner) = build(HOLDER);
        let map = decide_one(&ir, &interner, "Holder", "<uint8>");
        let exps = ExpansionInterner::new();

        let err = materialised_declarations_are_conserved(&ir, &ir, &interner, &exps, &map)
            .expect_err("the unrewritten input lacks its instance declaration");
        assert!(err.contains("expected exactly one declaration"), "{err}");

        // A frame nobody minted: fork the output and hang one node off an
        // instance id the map does not know.
        let (out, _) = run(&ir, &interner, &exps, &map);
        materialised_declarations_are_conserved(&ir, &out, &interner, &exps, &map).unwrap();
        let mut tampered = out.fork();
        let rogue = kg_base::ExpansionData {
            site: ir.arena[ir.root].src.head,
            instance: InstanceId::of("Nobody", "<never>"),
            parent: Expansion::ROOT,
            iteration: false,
            round: 1,
        };
        let via = exps.intern(rogue);
        let Exp::Seq(mut items) = tampered.arena[tampered.root].exp.clone() else { panic!() };
        let head = tampered.arena[ir.root].src.head;
        let node = tampered.arena.alloc(
            Exp::Barrier,
            Src { head, written: Written::Own, via },
            kg_syntax::SyntaxKind::BARRIER_STMT,
        );
        items.push(node);
        let new_root = tampered.arena.alloc(
            Exp::Seq(items),
            tampered.arena[tampered.root].src,
            tampered.arena[tampered.root].kind,
        );
        tampered.root = new_root;
        let err =
            materialised_declarations_are_conserved(&ir, &tampered, &interner, &exps, &map)
                .expect_err("a frame the pass never minted");
        assert!(err.contains("never handed"), "{err}");

        // And the minting clause: a mangled declaration no spec expects.
        let mut minted = out.fork();
        let ghost_sym = interner.intern("Ghost<|@x|>");
        let ghost_src = minted.arena[ir.root].src;
        let name =
            minted.arena.alloc(Exp::Identifier(ghost_sym), ghost_src, kg_syntax::SyntaxKind::NAME);
        let body = minted.arena.alloc(
            Exp::Seq(Vec::new()),
            ghost_src,
            kg_syntax::SyntaxKind::STRUCT_DECL,
        );
        let ghost = minted.arena.alloc(
            Exp::Struct { scope: QualifiedName::empty(), name, body },
            ghost_src,
            kg_syntax::SyntaxKind::STRUCT_DECL,
        );
        let Exp::Seq(mut items) = minted.arena[minted.root].exp.clone() else { panic!() };
        items.push(ghost);
        let new_root = minted.arena.alloc(
            Exp::Seq(items),
            minted.arena[minted.root].src,
            minted.arena[minted.root].kind,
        );
        minted.root = new_root;
        let err = materialised_declarations_are_conserved(&ir, &minted, &interner, &exps, &map)
            .expect_err("a mangled declaration no spec expects");
        assert!(err.contains("without a spec"), "{err}");
    }
}
