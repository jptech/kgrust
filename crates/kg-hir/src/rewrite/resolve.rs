//! `resolve` (`Template.hs:158-164`) and the mangler it calls
//! (`Mangle.hs:17-64`) — the half of the per-round rewrite `desugar
//! (instantiate . resolve)` (`Template.hs:58`) that rewrites a *decided*
//! `TemplateInstance` mention into the mangled name of its instance.
//! `instantiate` — `makeInstance`, `deduplicateInstances`, and the
//! `removeNestedSeq` in its own catch-all arm (`Template.hs:112-156`) — is
//! [`super::instantiate`], which materialises declarations under `TypeKey`'s
//! expansion discriminator (`docs/journal.md`, "Materialisation is not
//! blocked on inference…"). This pass creates **no declarations**; every node
//! it makes is a name.
//!
//! # The decided map is a parameter, and membership is structural
//!
//! The reference guards with ``b `Map.member` decidedInstances !
//! templateQualifiedName`` where `b` is the instance's argument `Seq` and the
//! map key equality is `NotedExp`'s — which **ignores the annotation**
//! (`Parser/Syntax.hs:593-599`, "Equality ignores annotation") but compares
//! every `ExpF` field, `declScopeName`s included. Three consequences, each
//! verified on the oracle 2026-08-13 (fixtures in the test module):
//!
//! * An occurrence whose *shape* was decided resolves **wherever it sits** —
//!   including inside a template body, where `decidedInstances` itself never
//!   collects (`Template.hs:105`, the `TemplateF` arm keeps only
//!   `templateParams`). `Box<uint8>` decided at a use site also resolves
//!   inside an unrelated template's body.
//! * The spread is *scope-exact*: `Box<My>` does not spread, because the
//!   argument subtree carries `declScopeName` fields and they differ between
//!   scopes; `Box<uint8>` does, because a builtin type carries none.
//! * A `validateArg`-re-stamped literal argument (`TypeCheck.hs:281-283`,
//!   run under `templateTypeCheck`'s `templateResolved` guard) is structurally
//!   different from every written literal, so a stamped instance never
//!   spreads: `W2<44>` under a `uint8 N` parameter stays unresolved in a
//!   template body while `W3<7>` under `auto M` resolves there.
//!
//! So the pass takes [`DecidedInstances`] — occurrence site → [`DecidedSite`]
//! — as an input parameter, exactly as `moveFunctions` takes a symbol map,
//! and re-derives the structural closure itself: the first walk resolves the
//! sites and records each one's *fingerprint* (base chain + argument
//! rendering, stamps applied); later walks also resolve any remaining
//! occurrence whose fingerprint matches, iterated to a fixpoint the way
//! `instantiateTemplates` iterates rounds (`Template.hs:56-58`). One bottom-up
//! walk composes what the reference needs a round per nesting level for: a
//! nested decided mention is already the mangled `ScopedName` by the time its
//! parent's arguments are read.
//!
//! The closure crosses modules as the reference's whole-program map does: the
//! IR loop seeds every module's pass with the fingerprint of each instance
//! the round decided anywhere ([`ResolveInstances::seeded`],
//! [`fingerprint_of`]). Measured 2026-08-13 over the corpus's root sets, a
//! cross-module spread had no witness; under the program-wide loop it has
//! (2026-09-27): `type.coerce.reinterpret_cast<bool>` decided at
//! `risc-v.k`'s own call renames the call in `decode_instr`'s body in
//! `processor.risc_v.internal.decoder` that same round — a body the census
//! cannot yet read, its `Closure` parameter unresolved — and the copies
//! deduced from it are the reference's round's.
//!
//! # `resolve e = e` is load-bearing
//!
//! The fallthrough at `Template.hs:164` is what leaves an *undecided*
//! instance alone for a later round. Here that is the untouched-`Exp` path of
//! the algebra — an occurrence that is neither a site nor a fingerprint match
//! returns its own `Exp` unchanged, and the driver's identity rule keeps its
//! id, so an undecided instance is byte-for-byte the node it was.
//!
//! # The mangler, and where this port refuses to follow it
//!
//! `mangleTemplateIdentifier` (`Mangle.hs:26-64`) is `suffixName ("<|" <>
//! cata mangle args <> "|>")`, `renote`d, with three rules in the reference's
//! own comment: arguments wrapped in `<|`/`|>`, compiler-contributed strings
//! prefixed `$`, user names and scalar literals prefixed `@`. The arms are
//! ported verbatim in the reference's order, with the traps spelled out:
//!
//! * **`show` is Haskell's.** `BoolLiteral` mangles `@True`/`@False`
//!   (capitalised); `Attribute` and `Flag` have hand-written `Show` instances
//!   that print the *source spelling* (`Parser/Syntax.hs:136-146,166-178`), so
//!   the arms here use the `text()` methods `kg-ty` already carries, not
//!   `Debug`.
//! * **`isClosure` makes the mangler non-injective, on purpose.** A
//!   `FunctionType` argument of `data.closure.core.Closure` mangles to `""`
//!   (`Mangle.hs:42-43`), so distinct argument lists share a name; the
//!   reference's own backstop is `deduplicateInstances`, which is the later
//!   slice. Do not "fix" it here — the corpus's most-instantiated template
//!   (5,117 distinct argument lists) depends on the collision.
//! * **`mangle _ = undefined`: the reference crashes, this port declines.**
//!   Only ~30 of the 92 variants have an arm. Measured over every decided
//!   occurrence in the corpus (104 sites, 2026-08-13): the argument subtrees
//!   reach `Seq`, `Unsigned`, `IntLiteral`, `Integer`, `BoolLiteral`,
//!   `Boolean`, `TypeIdentifier`, `ScopedName`, `Identifier` — all with arms —
//!   and nothing else. An arm the reference has but whose bytes this IR
//!   cannot reproduce is also declined rather than guessed:
//!   `StringLiteral` (this IR stores the token verbatim, quotes included;
//!   the reference stores parsed content). Declining aborts the whole
//!   occurrence: the mention stays as written and is counted, never renamed
//!   wrongly — a missed name is honest, an invented one is a lie.
//! * **The literal re-stamp happens at mangle time, not in the tree.** The
//!   reference mangles `W2<300>` under a `uint8 N` parameter as `@44` because
//!   `validateArg` rewrote the literal *in the tree* before any of this ran.
//!   That rewrite is `templateTypeCheck`'s unported half, so here the stamp
//!   travels in [`DecidedSite::stamps`] and is applied to the mangled bytes
//!   only — the tree keeps the written `300`. A reader diffing trees against
//!   the oracle will see that divergence; the mangled names do not.
//!
//! # What a resolution writes, and what it wears
//!
//! `mangleTemplateScopedName` (`Mangle.hs:18-19`) keeps the base
//! `ScopedName`'s scope and prefix and swaps its identifier. Here the
//! replacement `ScopedName` stands where the `TemplateInstance` stood — the
//! driver's `replace`, wearing that node's `Src` verbatim — its prefix chain
//! and scope ride along as the same arena nodes, and the mangled `Identifier`
//! is donated by the *unmangled* one (`renote`, `Mangle.hs:27`): it wears a
//! range whose text is the bare name, so it is `Donated` and no position
//! query is answered from it. The argument subtree drops out of the tree with
//! the base — nothing else references it.
//!
//! # Scale
//!
//! The decided set this pass can be handed today is 49 distinct instances —
//! 0.2% of the reference's 23,931 mangled declarations. The gap is upstream:
//! the three highest-volume templates are closure machinery instantiating
//! itself on arguments synthesized by `addLambdaWrappers`, which is unported.
//! Whatever a gate reads off this pass, it reads over that small set.

use kg_base::{FileRange, FxHashMap, FxHashSet, Interner, Symbol};
use kg_const::LiteralType;

use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;
use crate::rewrite::{desugar_within, get_name, get_qualifier, ident_sym, Rewriter};
use crate::structure::render_exp;

/// Whether an instance's base is spelled with the decided template's own
/// leaf.
fn names_template(arena: &ExpArena, interner: &kg_base::Interner, base: ExpId, template: &str) -> bool {
    let Some(leaf) = template.rsplit('.').next() else { return false };
    let Exp::ScopedName { ident, .. } = &arena[base].exp else { return true };
    let Some(sym) = ident_sym(arena, *ident) else { return true };
    // By name: a later round's template can itself be a copy — the
    // abbreviated template `Fn_T_auto<|$bool|>` became the round after it
    // was made (`return-type-polymorphism.k::3`).
    sym == interner.intern(leaf)
}

/// `data'closure'core'Closure` (`Type.hs:121-122`), dotted — `isClosure`'s
/// right-hand side (`Mangle.hs:31`).
const CLOSURE_QNAME: &str = "data.closure.core.Closure";

/// One decided occurrence — the value of [`DecidedInstances`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DecidedSite {
    /// The template's dotted qualified name — everything `resolve` reads off
    /// the symbol it looks up (`Template.hs:161-162`): the outer map key, and
    /// `isClosure`'s comparison (`Mangle.hs:31`).
    pub template: String,
    /// `validateArg`'s literal re-stamp (`TypeCheck.hs:281-283`), one slot per
    /// argument position: `Some` where an int literal sits under a value-typed
    /// parameter and the reference would have rewritten it to the parameter's
    /// width before mangling, `None` where it leaves the literal alone
    /// (`auto`, `typename`, non-literal arguments). Shorter than the argument
    /// list reads as all-`None`. Supplied by the caller because the parameter
    /// types live in the template declaration, which this pass never sees.
    pub stamps: Vec<Option<LiteralType>>,
}

/// The decided-instance map: occurrence site → its decision. Sites are the
/// `src.head` of the *plain-lowered* candidate paths (`kg_db::instances`
/// records them); the sorted lowering's `TemplateInstance` for a one-segment
/// path wears the same range (measured 104/104 over the corpus, 2026-08-13),
/// which is the bridge that lets one map serve both lowerings. The map is
/// whole-program — sites in other files simply never match this module's
/// nodes.
pub type DecidedInstances = FxHashMap<Site, DecidedSite>;

/// Where a decided occurrence stands: its head, and the copy it stands in.
/// Two copies of one body share every head (`renote`, `Template.hs:127`), so
/// once a round rewrites a tree that already holds copies, only the
/// expansion tells their mentions apart; everywhere else it is
/// [`Expansion::ROOT`] and a site is its head ([`Site::from`]).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Site {
    pub head: FileRange,
    pub via: kg_base::Expansion,
}

impl From<FileRange> for Site {
    fn from(head: FileRange) -> Site {
        Site { head, via: kg_base::Expansion::ROOT }
    }
}

/// The structural key one resolved site seeds: the base's sorted chain (the
/// stand-in for `lookupSymbol`'s answer — two sorted references spell the same
/// chain iff the sorter resolved them to the same declaration) plus the
/// argument `Seq`'s rendering with stamps applied.
pub type Fingerprint = (Vec<Symbol>, String);

/// The fingerprint a decided occurrence seeds — the base's chain and the
/// argument `Seq`'s rendering, stamps applied — read off the instance node
/// the census decided, in its own module's arena. `None` for a node that is
/// no `TemplateInstance` or whose base has no chain.
pub fn fingerprint_of(
    arena: &ExpArena,
    interner: &Interner,
    instance: ExpId,
    stamps: &[Option<LiteralType>],
) -> Option<Fingerprint> {
    let Exp::TemplateInstance { base, args } = &arena[instance].exp else { return None };
    Some((base_path(arena, *base)?, args_fingerprint(arena, interner, *args, stamps)))
}

/// `desugar resolve` (`Template.hs:58,158-164`) as a stateful pass — see the
/// module comment for why it is a full `cata` (no declining arm) run to a
/// fixpoint rather than one walk.
#[derive(Debug)]
pub struct ResolveInstances<'a> {
    decided: &'a DecidedInstances,
    /// Occurrences resolved because their own site is in the map.
    pub resolved: u32,
    /// Occurrences resolved by structural membership alone.
    pub spread: u32,
    /// Sites whose occurrence was found but left unresolved — an argument
    /// with no mangle arm, or a base the mangler cannot take apart. The
    /// reference crashes on both; this pass counts them instead.
    declined_sites: FxHashSet<Site>,
    fingerprints: FxHashMap<Fingerprint, DecidedSite>,
    /// Every base path [`ResolveInstances::fingerprints`] holds a key
    /// under. A spread match needs the path first, and an argument list's
    /// rendering is the pass's whole cost (84% of it on `risc-v.k`,
    /// 2026-10-10): a mention whose base no decision names is passed
    /// without rendering what it could never match.
    seeded: FxHashSet<Vec<Symbol>>,
    /// Each argument list's spread rendering, by node: the fixpoint's walks
    /// meet every mention it leaves standing again, and a node's subtree is
    /// what it was for the whole pass (the rewriter only appends).
    rendered: FxHashMap<ExpId, String>,
    /// Where it is kept ([`ResolveInstances::logged`]): each spread match
    /// the pass made — the walk, the mention, the base's chain, the
    /// argument list's rendering and the decision found — hashed. A
    /// module's spread reads the decisions every module seeded (the loop's
    /// read door, `kg_sem::reads`); what it finds is all that moves the
    /// module, so a lookup that finds nothing — or is not made, its base
    /// seeded nowhere — is not kept: two runs that matched alike, walk by
    /// walk, resolved alike.
    log: Option<Vec<u64>>,
    /// The walks of the fixpoint so far.
    walk: u32,
}

impl<'a> ResolveInstances<'a> {
    pub fn new(decided: &'a DecidedInstances) -> ResolveInstances<'a> {
        ResolveInstances {
            decided,
            resolved: 0,
            spread: 0,
            declined_sites: FxHashSet::default(),
            fingerprints: FxHashMap::default(),
            seeded: FxHashSet::default(),
            rendered: FxHashMap::default(),
            log: None,
            walk: 0,
        }
    }

    /// Keep, from now on, each spread match the pass makes
    /// ([`ResolveInstances::take_log`]).
    pub fn logged(mut self) -> ResolveInstances<'a> {
        self.log = Some(Vec::new());
        self
    }

    /// The spread matches made, hashed, since the pass began.
    pub fn take_log(&mut self) -> Vec<u64> {
        self.log.take().unwrap_or_default()
    }

    /// [`ResolveInstances::new`] with fingerprints seeded from the round's
    /// decisions in *every* module: `resolve` asks ``b `Map.member`
    /// decidedInstances ! templateQualifiedName`` of each `TemplateInstance`
    /// in the program (`Template.hs:158-159`), wherever the decision was
    /// made.
    pub fn seeded(decided: &'a DecidedInstances, seeds: FxHashMap<Fingerprint, DecidedSite>) -> ResolveInstances<'a> {
        let seeded = seeds.keys().map(|(path, _)| path.clone()).collect();
        ResolveInstances { fingerprints: seeds, seeded, ..ResolveInstances::new(decided) }
    }

    /// Sites this pass saw and left as written — for a gate's report.
    pub fn declined(&self) -> usize {
        self.declined_sites.len()
    }

    /// Run to the fixpoint. The first walk resolves **sites only** and seeds
    /// the fingerprints; later walks add spread matches and retry declined
    /// sites whose arguments a spread has since resolved. The driver's
    /// identity rule makes "nothing changed" the same root id, which ends the
    /// loop the way `Map.null decidedInstances` ends the reference's
    /// (`Template.hs:57`).
    ///
    /// Sites-only-first is **not** load-bearing for the answer — measured:
    /// letting the first walk spread against the half-seeded set turns every
    /// test exactly as green, because a match missed while its seed is still
    /// ahead in tree order (a template body precedes the use site that
    /// decides its shape) is simply picked up by the next round. It is kept
    /// because it makes each walk's *outcome* independent of tree order, so a
    /// reader never has to reason about seeding order — the fixpoint is the
    /// correctness mechanism, this is the legibility one.
    ///
    /// Termination rests on the algebra's only rewriting arm *consuming* a
    /// `TemplateInstance`: a walk that changes the tree has strictly fewer
    /// instances than the last, so rounds are bounded by the instance count.
    /// The falsification plant that made the catch-all rewrite (instead of
    /// `resolve e = e`) turned this loop into a hang — the loud form of that
    /// argument, and one more reason the fallthrough is load-bearing.
    pub fn apply(&mut self, rw: &mut Rewriter, root: ExpId) -> ExpId {
        // `step` rewrites a `TemplateInstance` and hands back anything else.
        let needs = crate::facts::TEMPLATE_INSTANCE;
        let mut root = desugar_within(rw, root, needs, &mut |rw, id, exp| self.step(rw, id, exp, false));
        if self.fingerprints.is_empty() {
            return root;
        }
        loop {
            self.walk += 1;
            let next = desugar_within(rw, root, needs, &mut |rw, id, exp| self.step(rw, id, exp, true));
            if next == root {
                return next;
            }
            root = next;
        }
    }

    /// One algebra application. `exp` is the node with its children already
    /// rewritten; `id` is the original, whose `src.head` is the site key.
    fn step(&mut self, rw: &mut Rewriter, id: ExpId, exp: Exp, spread: bool) -> Exp {
        let Exp::TemplateInstance { base, args } = exp else { return exp };
        let site = Site { head: rw.arena[id].src.head, via: rw.arena[id].src.via };

        // A site is not a node: a donated head is shared, and the
        // `make_capture1<uint8>` a deduction minted and the `Closure<…>`
        // `desugarLambdas` wrote beside it wear the capture list's one
        // range. A decision is the instance's whose base names its template.
        let decided = 
            self.decided.get(&site).filter(|d| names_template(rw.arena, rw.interner, base, &d.template));
        if let Some(decided) = decided {
            let decided = decided.clone();
            return match self.resolve_one(rw, base, args, &decided) {
                Some(out) => {
                    self.declined_sites.remove(&site);
                    self.resolved += 1;
                    if let Some(path) = base_path(rw.arena, base) {
                        let render =
                            args_fingerprint(rw.arena, rw.interner, args, &decided.stamps);
                        self.seeded.insert(path.clone());
                        self.fingerprints.insert((path, render), decided);
                    }
                    out
                }
                None => {
                    self.declined_sites.insert(site);
                    Exp::TemplateInstance { base, args }
                }
            };
        }

        if spread
            && let Some(path) = base_path(rw.arena, base)
            && self.seeded.contains(&path)
        {
            let render = self.rendered.entry(args).or_insert_with(|| args_fingerprint(rw.arena, rw.interner, args, &[])).clone();
            let key = (path, render);
            if let Some(seed) = self.fingerprints.get(&key) {
                let seed = seed.clone();
                if let Some(log) = self.log.as_mut() {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    (self.walk, id.index(), &key, &seed).hash(&mut h);
                    log.push(h.finish());
                }
                if let Some(out) = self.resolve_one(rw, base, args, &seed) {
                    self.spread += 1;
                    return out;
                }
            }
        }

        // `resolve e = e` (`Template.hs:164`) — the undecided instance is
        // left alone, byte for byte.
        Exp::TemplateInstance { base, args }
    }

    /// `mangleTemplateScopedName template args (unNote (unfix a))`
    /// (`Template.hs:159`, `Mangle.hs:18-19`): the base `ScopedName` with its
    /// identifier swapped for the mangled one. `None` where the reference's
    /// `undefined`s fire — a non-`ScopedName` base (`Mangle.hs:19`), a
    /// non-`Identifier` leaf (`:30`) — or where an argument reaches a
    /// declined arm.
    fn resolve_one(
        &mut self,
        rw: &mut Rewriter,
        base: ExpId,
        args: ExpId,
        decided: &DecidedSite,
    ) -> Option<Exp> {
        let Exp::ScopedName { scope, prefix, ident } = rw.arena[base].exp.clone() else {
            return None;
        };
        let leaf = ident_sym(rw.arena, ident)?;
        let is_closure = decided.template == CLOSURE_QNAME;
        let body = mangle_args(rw.arena, rw.interner, args, is_closure, &decided.stamps)?;
        // `suffixName ("<|" <> cata mangle args <> "|>") a` (`Mangle.hs:29`),
        // onto the *name*: a template a later round made of a copy — the
        // leftover `reinterpret_cast<|$bool|>` still has its `auto` — wears
        // the print's alias in that round's tree.
        let name = rw.suffix_name(leaf, &instance_suffix(&body));
        // `renote` (`Mangle.hs:27`): the mangled identifier wears the
        // unmangled one's note — donated, since its text is not its range's.
        let ident = rw.donate(ident, Exp::Identifier(name));
        Some(Exp::ScopedName { scope, prefix, ident })
    }
}

/// The base's identity: `getQualifier` plus `getName` over the sorted chain —
/// what stands in for `lookupSymbol a symbols` (`Template.hs:161`).
fn base_path(arena: &ExpArena, base: ExpId) -> Option<Vec<Symbol>> {
    let mut path = get_qualifier(arena, base)?;
    path.push(get_name(arena, base)?);
    Some(path)
}

/// `"<|" <> cata mangle args <> "|>"` (`Mangle.hs:29`): what `suffixName`
/// appends to a template's name to name its instance, `args` the
/// arguments' mangle ([`mangle_args`]). The one writer of the bracket
/// (`single-source-of-truth.md` D1.1): a mention and the copy it names
/// must agree to the byte.
pub fn instance_suffix(args: &str) -> String {
    format!("<|{args}|>")
}

/// `cata mangle args` (`Mangle.hs:29`) over the argument `Seq`, stamps
/// applied at the top-level positions `validateArg` would have rewritten.
///
/// `pub(crate)` for `instantiate`: `makeInstance` mangles the *declaration*
/// name with the same `mangleTemplateIdentifier t args` this pass mangles
/// mentions with (`Template.hs:139` vs `:158-159`), and the two must agree to
/// the byte or a materialised instance is unreachable from its own mentions.
/// Public for the IR census, which names a copy an instance would have.
pub fn mangle_args(
    arena: &ExpArena,
    interner: &Interner,
    args: ExpId,
    is_closure: bool,
    stamps: &[Option<LiteralType>],
) -> Option<String> {
    let mut out = String::new();
    match &arena[args].exp {
        Exp::Seq(items) => {
            for (i, &item) in items.iter().enumerate() {
                match (stamps.get(i).copied().flatten(), &arena[item].exp) {
                    (Some(stamp), Exp::IntLiteral { value, .. }) => {
                        // `IntLiteralF (Just …) (truncate… width i)`
                        // (`TypeCheck.hs:281-283`) — applied to the bytes
                        // only; the tree keeps the written literal.
                        out.push('@');
                        out.push_str(&stamp.truncate(value).to_string());
                    }
                    _ => mangle(arena, interner, item, is_closure, &mut out)?,
                }
            }
        }
        _ => mangle(arena, interner, args, is_closure, &mut out)?,
    }
    Some(out)
}

/// The structural identity of an argument list, as the reference's map key
/// holds it: position-free (`Note` equality ignores the annotation), every
/// field compared (scopes included) — which is exactly what
/// [`render_exp`] prints. A stamped position renders as a `\0`-marked token
/// no written occurrence can produce, because a stamped literal is
/// structurally different from every unstamped one and this IR never
/// re-stamps its tree.
fn args_fingerprint(
    arena: &ExpArena,
    interner: &Interner,
    args: ExpId,
    stamps: &[Option<LiteralType>],
) -> String {
    let Exp::Seq(items) = &arena[args].exp else {
        return render_exp(arena, interner, args);
    };
    let mut out = String::new();
    for (i, &item) in items.iter().enumerate() {
        match (stamps.get(i).copied().flatten(), &arena[item].exp) {
            (Some(stamp), Exp::IntLiteral { value, .. }) => {
                let t = stamp.truncate(value);
                out.push('\0');
                out.push_str(&format!("stamp {stamp} {t}\n"));
            }
            _ => out.push_str(&render_exp(arena, interner, item)),
        }
    }
    out
}

/// `mangle` (`Mangle.hs:35-63`), arm for arm in the reference's order. The
/// output is pushed onto one buffer so a deep composition costs the bytes it
/// writes, never a re-walk. `None` is the reference's `undefined`, plus the
/// two arms declined on purpose — see the module comment.
#[allow(clippy::too_many_lines)]
fn mangle(
    arena: &ExpArena,
    interner: &Interner,
    id: ExpId,
    is_closure: bool,
    out: &mut String,
) -> Option<()> {
    match &arena[id].exp {
        Exp::Auto => out.push_str("$auto"),
        Exp::Array { attr, elem, dims } => {
            out.push_str("$array");
            mangle(arena, interner, *attr, is_closure, out)?;
            mangle(arena, interner, *elem, is_closure, out)?;
            mangle(arena, interner, *dims, is_closure, out)?;
        }
        // `"@" <> show x <> y` — the hand-written `Show` prints the source
        // spelling (`Parser/Syntax.hs:136-146`), which `text()` is.
        Exp::Attr(a, e) => {
            out.push('@');
            out.push_str(a.text());
            mangle(arena, interner, *e, is_closure, out)?;
        }
        // Haskell `show :: Bool` — capitalised. Oracle: `Flag<|@True|>`.
        Exp::BoolLiteral(b) => {
            out.push('@');
            out.push_str(if *b { "True" } else { "False" });
        }
        Exp::Const(e) => {
            out.push_str("$const");
            mangle(arena, interner, *e, is_closure, out)?;
        }
        Exp::Designator { name, value } => {
            out.push('@');
            mangle(arena, interner, *name, is_closure, out)?;
            mangle(arena, interner, *value, is_closure, out)?;
        }
        // `mangle (EnumValue x _) = x` — the value is dropped.
        Exp::EnumValue { ty, .. } => mangle(arena, interner, *ty, is_closure, out)?,
        Exp::FlagAttr(f) => {
            out.push('$');
            out.push_str(f.text());
        }
        // `"@" <> show x` (`Mangle.hs:41`): GHC's `show :: Float`, not
        // Rust's shortest round trip (`F32Bits::haskell_show`) —
        // `Bar<|$float@1.0|>`, `T4<|@0.0@32|>` (`type-check.k::35`,
        // `constexpr.k::87`, the recorded walks).
        Exp::FloatLiteral(bits) => {
            out.push('@');
            out.push_str(&bits.haskell_show());
        }
        // `isClosure` drops the argument outright — the deliberate
        // non-injectivity (`Mangle.hs:42-43`); see the module comment.
        Exp::FunctionType { attr, params, ret } => {
            if !is_closure {
                out.push_str("$fun");
                mangle(arena, interner, *attr, is_closure, out)?;
                mangle(arena, interner, *params, is_closure, out)?;
                mangle(arena, interner, *ret, is_closure, out)?;
            }
        }
        // `FunctionTypeParam x y _` — the parameter name is dropped.
        Exp::FunctionTypeParam { attr, ty, .. } => {
            mangle(arena, interner, *attr, is_closure, out)?;
            mangle(arena, interner, *ty, is_closure, out)?;
        }
        Exp::Identifier(s) => {
            out.push('@');
            out.push_str(&interner.resolve(*s));
        }
        Exp::InitializerList(e) => {
            out.push('@');
            mangle(arena, interner, *e, is_closure, out)?;
        }
        Exp::IntAttr(a, e) => {
            out.push('@');
            out.push_str(a.text());
            mangle(arena, interner, *e, is_closure, out)?;
        }
        Exp::Integer(w) => {
            out.push_str("$int@");
            out.push_str(&w.to_string());
        }
        // `"@" <> show x` — `ConstInt`'s `Display` is Haskell's `show ::
        // Integer`: plain decimal, `-` for negatives.
        Exp::IntLiteral { value, .. } => {
            out.push('@');
            out.push_str(&value.to_string());
        }
        Exp::Positional { value, .. } => mangle(arena, interner, *value, is_closure, out)?,
        Exp::QualifiedIdentifier(e) => mangle(arena, interner, *e, is_closure, out)?,
        Exp::Reference(e) => {
            out.push_str("$ref");
            mangle(arena, interner, *e, is_closure, out)?;
        }
        // The qualifier contributes its own mangling, so nesting composes —
        // `Box<Outer::Inner>` is `Box<|@Outer@Inner|>`.
        Exp::ScopedName { prefix, ident, .. } => {
            if let Some(p) = prefix {
                mangle(arena, interner, *p, is_closure, out)?;
            }
            mangle(arena, interner, *ident, is_closure, out)?;
        }
        Exp::Seq(items) => {
            for &item in items {
                mangle(arena, interner, item, is_closure, out)?;
            }
        }
        // `"$stringliteral" <> x` (`Mangle.hs:57`) over the parsed content.
        // This IR keeps the token, quotes included, and a string
        // *expression* — a template argument — as `InterpolatedString`
        // until `interpolatedStringPre` would have made it a
        // `StringLiteralF` (`Frontend.hs:233-234`); one with a hole is not a
        // literal, and the reference would not reach here with it.
        Exp::StringLiteral(tok) | Exp::InterpolatedString(tok) => {
            out.push_str("$stringliteral");
            out.push_str(&crate::exp::plain_string_value(tok)?);
        }
        Exp::TypeIdentifier(_, e) => mangle(arena, interner, *e, is_closure, out)?,
        Exp::Unsigned(w) => {
            out.push_str("$uint@");
            out.push_str(&w.to_string());
        }
        // `uint<e>` with a closed width — `uint<bitsizeof T>` in a copy,
        // where `T` is `uint32` — is the fixed-width type pass 6 folded it
        // to before pass 8 mangled anything (`crate::width`). Unfolded, the
        // occurrence declines and the copy is never made.
        Exp::ParamInt(_) | Exp::ParamUint(_) => {
            out.push_str(&crate::width::mangled_param_int(arena, id, interner)?);
        }
        Exp::Boolean => out.push_str("$bool"),
        Exp::Float => out.push_str("$float"),
        Exp::Str => out.push_str("$string"),
        Exp::Void => out.push_str("$void"),
        // `mangle _ = undefined` — the reference crashes; this port declines
        // the whole occurrence. See the module comment for the measurement
        // behind leaving these unhandled.
        _ => return None,
    }
    Some(())
}

// --- the whole-corpus invariant ----------------------------------------------

/// **The pass's own invariant**, recomputed rather than trusted: a written
/// instance either becomes a mangled mention or survives — never both, never
/// neither — and mentions are never minted from anything else.
///
/// Per reachable `TemplateInstance` of `before`, at its `src.head` range:
///
/// * `after` holds a mention there (a `ScopedName` whose identifier contains
///   `<|`) **xor** still holds a `TemplateInstance` there — unless the range
///   sits inside another mention's before-subtree, where the argument tree
///   legitimately drops out with the base (`Mangle.hs:29` consumes it into
///   the identifier's text).
/// * A mention's identifier is framed `leaf<|…|>` by the leaf the base spelled
///   in `before` — `suffixName` appends, never replaces (`Desugar.hs:164-165`).
/// * A *site* (`decided`, this file) that survives must be one the mangler
///   genuinely declines: its after-arguments re-mangle to `None`. A site with
///   manglable arguments still standing is a pass that did not run to its
///   fixpoint.
/// * Every mention of `after` stands at a range that held an instance in
///   `before` — resolution renames references and creates nothing else.
///
/// What it cannot see, having tried: the mangled bytes between the frame
/// (`$uint@8` vs `$uint@9` — the oracle-read unit tests and the
/// reference-graded gate own those); wrong-direction *spread* (an occurrence
/// resolved or missed against scope fields — both look like legal outcomes
/// here; the spread unit tests own it); every `Src` donor (`Donated` is
/// invisible to provenance gates by design — the donor unit tests own it);
/// and a site whose range never lowered to a sorted `TemplateInstance` at all
/// (divergence #22 keeps such paths unsorted, and the pass's silence there is
/// deliberate). The decline clause reuses this module's own `mangle`, so a
/// bug in an arm's *bytes* passes both sides — the frame check and the oracle
/// tests are the independent instruments.
pub fn resolved_mentions_are_conserved(
    before: &crate::ModuleIr,
    after: &crate::ModuleIr,
    interner: &Interner,
    decided: &DecidedInstances,
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

    // Every reachable instance of `before`, and every instance/mention of
    // `after`, by head range.
    let mut before_instances: Vec<(ExpId, FileRange)> = Vec::new();
    reachable(before, |ir, id| {
        if matches!(ir.arena[id].exp, Exp::TemplateInstance { .. }) {
            before_instances.push((id, ir.arena[id].src.head));
        }
    });
    let mut after_instances: FxHashSet<FileRange> = FxHashSet::default();
    let mut mentions: FxHashMap<FileRange, String> = FxHashMap::default();
    reachable(after, |ir, id| match &ir.arena[id].exp {
        Exp::TemplateInstance { .. } => {
            after_instances.insert(ir.arena[id].src.head);
        }
        Exp::ScopedName { ident, .. } => {
            if let Some(sym) = ident_sym(&ir.arena, *ident) {
                let text = interner.resolve(sym);
                if kg_base::mentions_instance(&text) {
                    mentions.insert(ir.arena[id].src.head, text.to_string());
                }
            }
        }
        _ => {}
    });

    // Ranges exempt from "neither": inside a resolved instance's
    // before-subtree, the argument tree vanishes with the base.
    let mut exempt: FxHashSet<FileRange> = FxHashSet::default();
    for &(id, range) in &before_instances {
        if !mentions.contains_key(&range) {
            continue;
        }
        let mut stack = Vec::new();
        before.arena[id].exp.children(&mut stack);
        let mut kids = Vec::new();
        while let Some(inner) = stack.pop() {
            if matches!(before.arena[inner].exp, Exp::TemplateInstance { .. }) {
                exempt.insert(before.arena[inner].src.head);
            }
            before.arena[inner].exp.children(&mut kids);
            stack.append(&mut kids);
        }
    }

    let before_ranges: FxHashSet<FileRange> =
        before_instances.iter().map(|&(_, r)| r).collect();
    for &(id, range) in &before_instances {
        let survived = after_instances.contains(&range);
        let mention = mentions.get(&range);
        match (survived, mention) {
            (true, Some(text)) => {
                return Err(format!(
                    "instance at {range:?} both survived and resolved ({text}) — a \
                     replacement leaves no second claimant (Template.hs:158-164)"
                ));
            }
            (false, None) => {
                if !exempt.contains(&range) {
                    return Err(format!(
                        "instance at {range:?} neither survived nor resolved, and it \
                         is not inside a resolved instance's arguments"
                    ));
                }
            }
            (false, Some(text)) => {
                let Exp::TemplateInstance { base, .. } = &before.arena[id].exp else {
                    unreachable!("collected as an instance");
                };
                let leaf = get_name(&before.arena, *base)
                    .map(|s| interner.resolve(s).to_string())
                    .unwrap_or_default();
                if !text.starts_with(&format!("{leaf}<|")) || !text.ends_with("|>") {
                    return Err(format!(
                        "mention at {range:?} is {text:?}, not framed \
                         `{leaf}<|…|>` — suffixName appends (Desugar.hs:164-165)"
                    ));
                }
            }
            (true, None) => {
                let Some(site) = decided.get(&Site::from(range)) else { continue };
                if before.file != range.file {
                    continue;
                }
                let Exp::TemplateInstance { base, args } =
                    after_node_at(after, range).map(|id| after.arena[id].exp.clone())
                        .unwrap_or(Exp::Unsupported)
                else {
                    continue;
                };
                // Another template's decision at a shared head is not this
                // instance's (`Resolve::step`).
                if !names_template(&after.arena, interner, base, &site.template) {
                    continue;
                }
                let is_closure = site.template == CLOSURE_QNAME;
                let manglable = matches!(after.arena[base].exp, Exp::ScopedName { ident, .. }
                        if ident_sym(&after.arena, ident).is_some())
                    && mangle_args(&after.arena, interner, args, is_closure, &site.stamps)
                        .is_some();
                if manglable {
                    return Err(format!(
                        "decided site at {range:?} survived with manglable arguments — \
                         the pass did not reach its fixpoint (Template.hs:158-163)"
                    ));
                }
            }
        }
    }

    for (range, text) in &mentions {
        if !before_ranges.contains(range) {
            return Err(format!(
                "mention {text:?} at {range:?} stands where `before` held no \
                 instance — resolution renames references, it mints nothing"
            ));
        }
    }
    Ok(())
}

/// The reachable `TemplateInstance` of `after` wearing `range`, if any.
fn after_node_at(after: &crate::ModuleIr, range: FileRange) -> Option<ExpId> {
    let mut found = None;
    let mut seen = vec![false; after.arena.len()];
    let mut stack = vec![after.root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        if matches!(after.arena[id].exp, Exp::TemplateInstance { .. })
            && after.arena[id].src.head == range
        {
            found = Some(id);
            break;
        }
        after.arena[id].exp.children(&mut kids);
        stack.append(&mut kids);
    }
    found
}

/// The reference is the grader here, not this port's reading of it.
///
/// Every expectation below was read off the reference frontend built by
/// `oracle/build.sh`, captured 2026-08-13, at
///
/// ```text
/// kg-oracle --tree  --roots-only --passes 8 --template-passes 9 \
///           --template-iterations N FILE
/// kg-oracle --notes --roots-only --passes 8 --template-passes 9 \
///           --template-iterations {0,1} FILE
/// ```
///
/// with `N` at least the fixture's nesting depth — the compiler's real
/// default is *unbounded* (`app/Options/CmdArgs.hs:104-106`), and one round
/// resolves one nesting level. `--passes 8` is not optional: without it
/// `removeTemplate` (`Frontend.hs:300-302`) deletes the output. Fixtures
/// carry no `module` header (the oracle rejects one whose path does not match)
/// and spell nested closers `> >` (the reference lexes `>>` as a shift).
///
/// The oracle's printed tree has also been through `instantiate` — the
/// mangled *declarations* it shows (`StructF {declName = "Holder<|$uint@8|>"…`)
/// are the later slice; what these tests read off it are the *mentions*:
/// `TypeIdentifierF … ScopedNameF … IdentifierF "Holder<|$uint@8|>"`. Because
/// our `Src` ranges and the reference's are different objects (divergences
/// #25, #26), every `--notes` expectation is asserted as donor *identity* —
/// the new node wears the same `Src` as a named donor node — never as a raw
/// offset.
///
/// The decided maps are built by the tests themselves, from the fixture's own
/// instance nodes — the same `(site → template, stamps)` shape the caller
/// derives from `kg_db::instances`, whose ranges the sorted lowering was
/// measured to share (104/104 corpus occurrence sites, 2026-08-13). The
/// fixtures lower through `lower_sorted` + `NoImports`: this pass fires on
/// the sorted `TemplateInstance` form only, and plain `lower` keeps every
/// written path as an unsorted `Exp::Path` (divergence #22).
#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner, Written};
    use kg_syntax::ast;
    use kg_syntax::AstNode;

    use super::*;
    use crate::rewrite::resolve_instances;
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

    /// Reachable `TemplateInstance`s whose base leaf is `name`, in text order.
    fn instances_named(ir: &ModuleIr, interner: &Interner, name: &str) -> Vec<ExpId> {
        let mut out = Vec::new();
        walk(ir, |ir, id| {
            if let Exp::TemplateInstance { base, .. } = &ir.arena[id].exp
                && get_name(&ir.arena, *base)
                    .is_some_and(|s| interner.resolve(s) == name)
            {
                out.push(id);
            }
        });
        out.sort_by_key(|&id| ir.arena[id].src.head.range.start());
        out
    }

    /// A decided map claiming every named instance for `template`, no stamps.
    fn decide(ir: &ModuleIr, interner: &Interner, names: &[(&str, &str)]) -> DecidedInstances {
        let mut map = DecidedInstances::default();
        for &(name, template) in names {
            for id in instances_named(ir, interner, name) {
                map.insert(
                    crate::rewrite::Site::from(ir.arena[id].src.head),
                    DecidedSite { template: template.to_string(), stamps: Vec::new() },
                );
            }
        }
        map
    }

    /// Reachable mangled mentions, `(identifier text, ScopedName id)`, text
    /// order.
    fn mentions(ir: &ModuleIr, interner: &Interner) -> Vec<(String, ExpId)> {
        let mut out = Vec::new();
        walk(ir, |ir, id| {
            if let Exp::ScopedName { ident, .. } = &ir.arena[id].exp
                && let Some(sym) = ident_sym(&ir.arena, *ident)
            {
                let text = interner.resolve(sym);
                if text.contains("<|") {
                    out.push((text.to_string(), id));
                }
            }
        });
        out.sort_by_key(|&(_, id)| ir.arena[id].src.head.range.start());
        out
    }

    fn run(
        ir: &ModuleIr,
        interner: &Interner,
        decided: &DecidedInstances,
    ) -> (ModuleIr, u32, u32, usize) {
        let mut out = ir.fork();
        let mut pass = ResolveInstances::new(decided);
        let mut rw = Rewriter::new(&mut out.arena, interner);
        out.root = pass.apply(&mut rw, ir.root);
        let (resolved, spread, declined) = (pass.resolved, pass.spread, pass.declined());
        (out, resolved, spread, declined)
    }

    const HOLDER: &str = "template <typename T>\nstruct Holder\n{\n    T value;\n}\n\n\
         inline uint32 use()\n{\n    Holder<uint8> h;\n    return 0;\n}\n";

    /// The scalar argument, `--tree` at `--template-iterations 1`:
    ///
    /// ```text
    /// varType = TypeIdentifierF ["use","$"] ScopedNameF ["use","$"] Nothing
    ///           IdentifierF "Holder<|$uint@8|>"
    /// ```
    #[test]
    fn a_scalar_argument_mangles_to_uint_at_width() {
        let (ir, interner) = build(HOLDER);
        let decided = decide(&ir, &interner, &[("Holder", "Holder")]);
        let (out, resolved, spread, declined) = run(&ir, &interner, &decided);
        let m = mentions(&out, &interner);
        assert_eq!(
            m.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Holder<|$uint@8|>"]
        );
        assert_eq!((resolved, spread, declined), (1, 0, 0));
        assert!(instances_named(&out, &interner, "Holder").is_empty());
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// Arguments concatenate with no separator; literals wear `@`; a signed
    /// width is `$int@`. `--tree` at `--template-iterations 4`:
    ///
    /// ```text
    /// IdentifierF "Pair<|$int@32@250|>"
    /// IdentifierF "Pair<|$uint@16@3|>"
    /// ```
    #[test]
    fn arguments_concatenate_bare_and_literals_wear_at() {
        let (ir, interner) = build(
            "template <typename T, auto N>\nstruct Pair\n{\n    T[N] value;\n}\n\n\
             inline uint32 use()\n{\n    Pair<uint16, 3> p;\n    Pair<int32, 250> q;\n\
             \x20   return 0;\n}\n",
        );
        let decided = decide(&ir, &interner, &[("Pair", "Pair")]);
        let (out, resolved, ..) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Pair<|$uint@16@3|>", "Pair<|$int@32@250|>"]
        );
        assert_eq!(resolved, 2);
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// `show :: Bool` is Haskell's — capitalised. `--tree` at
    /// `--template-iterations 4`:
    ///
    /// ```text
    /// IdentifierF "Flag<|@False|>"
    /// IdentifierF "Flag<|@True|>"
    /// ```
    #[test]
    fn a_bool_literal_shows_capitalised() {
        let (ir, interner) = build(
            "template <auto B>\nstruct Flag\n{\n    uint8 v;\n}\n\n\
             inline uint32 use()\n{\n    Flag<true> f;\n    Flag<false> g;\n\
             \x20   return 0;\n}\n",
        );
        let decided = decide(&ir, &interner, &[("Flag", "Flag")]);
        let (out, ..) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Flag<|@True|>", "Flag<|@False|>"]
        );
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// A user-defined type wears `@`; a qualified argument's prefix mangles
    /// *before* its leaf — `ScopedName (Just x) y = x <> y` (`Mangle.hs:53`)
    /// — so the chain flattens. `--tree` at `--template-iterations 4`, two
    /// fixtures:
    ///
    /// ```text
    /// IdentifierF "Box<|@My|>"          IdentifierF "Box<|$bool|>"
    /// IdentifierF "Box<|@Outer@Inner|>"
    /// ```
    #[test]
    fn a_qualified_argument_flattens_its_chain() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Box\n{\n    T inner;\n}\n\n\
             struct My\n{\n    uint8 f;\n}\n\n\
             class Outer\n{\npublic:\n    struct Inner\n    {\n        uint8 f;\n    }\n}\n\n\
             inline uint32 use()\n{\n    Box<My> b;\n    Box<bool> c;\n\
             \x20   Box<Outer::Inner> d;\n    return 0;\n}\n",
        );
        let decided = decide(&ir, &interner, &[("Box", "Box")]);
        let (out, resolved, ..) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Box<|@My|>", "Box<|$bool|>", "Box<|@Outer@Inner|>"]
        );
        assert_eq!(resolved, 3);
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// A nested decided argument is already the mangled `ScopedName` when the
    /// outer instance's arguments are read — one bottom-up walk does what the
    /// reference needs a round per level for — and the inner mention's node
    /// drops out of the tree with the outer base. `--tree` at
    /// `--template-iterations 4`:
    ///
    /// ```text
    /// IdentifierF "Box<|@Box<|$uint@8|>|>"
    /// IdentifierF "Box<|$uint@8|>"
    /// ```
    #[test]
    fn a_nested_decided_argument_composes_through_the_identifier_arm() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Box\n{\n    T inner;\n}\n\n\
             inline uint32 use()\n{\n    Box<Box<uint8> > bb;\n    return 0;\n}\n",
        );
        let decided = decide(&ir, &interner, &[("Box", "Box")]);
        let (out, resolved, ..) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Box<|@Box<|$uint@8|>|>"]
        );
        assert_eq!(resolved, 2, "both sites fired; the inner mention was consumed");
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// The `isClosure` arm: a `FunctionType` argument of
    /// `data.closure.core.Closure` mangles to `""` (`Mangle.hs:42-43`), and to
    /// `$fun`… for anyone else — so the mangler is non-injective by
    /// construction, and two distinct argument lists share a name under the
    /// closure template. The `$fun` bytes, `--tree` at
    /// `--template-iterations 4`:
    ///
    /// ```text
    /// IdentifierF "FnBox<|$fun$uint@8$bool$uint@8|>"
    /// ```
    ///
    /// The `""` behaviour on the reference (a real lambda through the closure
    /// machinery, `-I library --base library/base.k`): the `FunctionType`
    /// argument contributes nothing between `<|` and the next argument's `@`:
    ///
    /// ```text
    /// IdentifierF "Closure<|@f@?lambda0@@data@closure@make_capture1<|$uint@8|>@$@Capture|>"
    /// ```
    #[test]
    fn the_closure_function_type_argument_mangles_to_nothing() {
        let text = "template <typename FT>\nstruct FnBox\n{\n    uint8 v;\n}\n\n\
             inline uint32 use()\n{\n    FnBox<(uint8, bool) -> uint8> a;\n\
             \x20   FnBox<(uint8) -> uint8> b;\n    return 0;\n}\n";

        // As an ordinary template: the $fun arm, bytes off the oracle.
        let (ir, interner) = build(text);
        let decided = decide(&ir, &interner, &[("FnBox", "FnBox")]);
        let (out, ..) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["FnBox<|$fun$uint@8$bool$uint@8|>", "FnBox<|$fun$uint@8$uint@8|>"]
        );
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();

        // As the closure template: both distinct argument lists collapse to
        // one name — the collision `deduplicateInstances` exists to absorb
        // (`Template.hs:144-155`). Do not "fix" it. The template is spelled
        // `Closure` here: a decision is the instance's whose base names its
        // template (`names_template`), and until 2026-09-19 this half
        // decided `FnBox`'s mentions under the closure's name, which is the
        // shared-site collision `resolve` now refuses.
        let closure_text = text.replace("FnBox", "Closure");
        let (ir, interner) = build(&closure_text);
        let decided = decide(&ir, &interner, &[("Closure", "data.closure.core.Closure")]);
        let (out, ..) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Closure<||>", "Closure<||>"]
        );
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// A site is not a node: a donated head is shared, and a deduction's
    /// `make_capture1<uint8>` and the `Closure<…>` beside it wear one range.
    /// A decision resolves only the instance whose base names its template —
    /// here `A<uint8>`'s site decided for `B` leaves `A<uint8>` standing, as
    /// a `Closure` mangled under `make_capture1`'s decision once stood as
    /// `Closure<|$fun@…|>` (scratchpad/t3/, 2026-09-19). Red with the
    /// template check cut: `A<|$uint@8|>`.
    #[test]
    fn a_decision_resolves_only_its_own_templates_instance() {
        let text = "template <typename T>\nstruct A\n{\n    T v;\n}\n\ninline void use()\n{\n    A<uint8> a;\n}\n";
        let (ir, interner) = build(text);
        let decided = decide(&ir, &interner, &[("A", "B")]);
        let (out, ..) = run(&ir, &interner, &decided);
        assert!(mentions(&out, &interner).is_empty());
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// `resolve e = e` (`Template.hs:164`): an occurrence that is neither a
    /// site nor a structural match is byte-for-byte the node it was — same
    /// id, nothing appended for it — and an empty map skips the walks
    /// entirely.
    #[test]
    fn an_undecided_instance_is_left_alone() {
        let (ir, interner) = build(HOLDER);

        // Empty map: the no-op fast path — same root, nothing appended.
        let out = resolve_instances(&ir, &interner, &DecidedInstances::default());
        assert_eq!(out.root, ir.root);
        assert_eq!(out.arena.len(), ir.arena.len());

        // A map deciding a different file: still the fast path.
        let mut foreign = DecidedInstances::default();
        let site = ir.arena[instances_named(&ir, &interner, "Holder")[0]].src.head;
        foreign.insert(
            Site::from(FileRange::new(FileId(7), site.range)),
            DecidedSite { template: "Holder".to_string(), stamps: Vec::new() },
        );
        let out = resolve_instances(&ir, &interner, &foreign);
        assert_eq!(out.root, ir.root);
        assert_eq!(out.arena.len(), ir.arena.len());
    }

    /// Structural spread, seeded and bounded by the oracle:
    ///
    /// * `Box<uint8>` decided at the use site **also resolves inside an
    ///   unrelated template's body** — `Map.member` compares the argument
    ///   trees with `Note` equality, which ignores annotations
    ///   (`Parser/Syntax.hs:593-599`) — even though `decidedInstances` never
    ///   collects there (`Template.hs:105`). `--tree`, iterations 1:
    ///
    ///   ```text
    ///   templateDecl = StructF {… declName = IdentifierF "Wrap", body = SeqF [
    ///     VariableF {… varType = TypeIdentifierF ["$quasi1"] ScopedNameF
    ///       ["$quasi1"] Nothing IdentifierF "Box<|$uint@8|>", declName = "fixed" …},
    ///     VariableF {… TemplateInstanceF … "Box" SeqF [TypeParamF [] TypenameF "U" 0] …}]}
    ///   ```
    ///
    /// * `Box<U>` in the same body stays a `TemplateInstanceF` — undecidable,
    ///   untouched.
    #[test]
    fn a_decided_shape_spreads_into_a_template_body() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Box\n{\n    T inner;\n}\n\n\
             template <typename U>\nstruct Wrap\n{\n    Box<uint8> fixed;\n\
             \x20   Box<U> open;\n}\n\n\
             inline uint32 use()\n{\n    Box<uint8> b;\n    return 0;\n}\n",
        );
        // Only the use-site occurrence is a site — the census never decides
        // inside a template body; the body copy must resolve by shape alone.
        let boxes = instances_named(&ir, &interner, "Box");
        assert_eq!(boxes.len(), 3);
        let use_site = *boxes.last().expect("the use site is last in text order");
        let mut decided = DecidedInstances::default();
        decided.insert(
            Site::from(ir.arena[use_site].src.head),
            DecidedSite { template: "Box".to_string(), stamps: Vec::new() },
        );
        let (out, resolved, spread, declined) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Box<|$uint@8|>", "Box<|$uint@8|>"]
        );
        assert_eq!((resolved, spread, declined), (1, 1, 0));
        // `resolve e = e`, per node: the survivor is not merely still there,
        // it is the *same* node — the identity rule kept its id.
        assert_eq!(
            instances_named(&out, &interner, "Box"),
            vec![boxes[1]],
            "Box<U> survives untouched"
        );
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// A decision made in another module seeds this one's spread
    /// ([`ResolveInstances::seeded`]): `resolve` asks `Map.member` of every
    /// `TemplateInstance` in the program, wherever the decision was made
    /// (`Template.hs:158-159`). With no site here, both `Box<uint8>`s
    /// resolve by shape. Red with the seeds' paths not offered to the
    /// spread.
    #[test]
    fn a_seed_from_another_module_spreads_without_a_site() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Box\n{\n    T inner;\n}\n\n\
             template <typename U>\nstruct Wrap\n{\n    Box<uint8> fixed;\n}\n\n\
             inline uint32 use()\n{\n    Box<uint8> b;\n    return 0;\n}\n",
        );
        let decision = DecidedSite { template: "Box".to_string(), stamps: Vec::new() };
        let at = *instances_named(&ir, &interner, "Box").last().expect("the use site");
        let key = fingerprint_of(&ir.arena, &interner, at, &decision.stamps).expect("a seed");
        let seeds: FxHashMap<Fingerprint, DecidedSite> = std::iter::once((key, decision)).collect();
        let mut out = ir.fork();
        crate::rewrite::resolve_instances_seeded_in_place(&mut out, &interner, &DecidedInstances::default(), &seeds);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Box<|$uint@8|>", "Box<|$uint@8|>"]
        );
    }

    /// Membership is per *template*, never per argument shape alone:
    /// `decidedInstances` is keyed template-first (`Template.hs:88,158`), so
    /// an identical argument list under a different template does not
    /// resolve. `--tree`, iterations 3 — `A<uint8>` decided at the use site,
    /// `Wrap`'s body holds both:
    ///
    /// ```text
    /// … ScopedNameF ["$quasi2"] Nothing IdentifierF "A<|$uint@8|>" …
    /// … TemplateInstanceF ScopedNameF ["$quasi2"] Nothing IdentifierF "B"
    ///     SeqF [UnsignedF 8] …
    /// ```
    ///
    /// Added by falsification: the base-blind fingerprint plant (args-only
    /// matching) left every earlier test green.
    #[test]
    fn spread_is_per_template_not_per_shape() {
        let (ir, interner) = build(
            "template <typename T>\nstruct A\n{\n    T v;\n}\n\n\
             template <typename T>\nstruct B\n{\n    T v;\n}\n\n\
             template <typename U>\nstruct Wrap\n{\n    A<uint8> a;\n    B<uint8> b;\n}\n\n\
             inline uint32 use()\n{\n    A<uint8> x;\n    return 0;\n}\n",
        );
        let a_sites = instances_named(&ir, &interner, "A");
        assert_eq!(a_sites.len(), 2);
        let use_site = *a_sites.last().expect("use site last");
        let mut decided = DecidedInstances::default();
        decided.insert(
            Site::from(ir.arena[use_site].src.head),
            DecidedSite { template: "A".to_string(), stamps: Vec::new() },
        );
        let (out, resolved, spread, _) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["A<|$uint@8|>", "A<|$uint@8|>"]
        );
        assert_eq!((resolved, spread), (1, 1));
        assert_eq!(
            instances_named(&out, &interner, "B").len(),
            1,
            "B<uint8> shares A's argument list and must not resolve under A's decision"
        );
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// Spread is scope-exact: the argument subtree carries `declScopeName`
    /// fields and the reference compares them (`ExpF`'s derived `Eq`), so
    /// `Box<My>` — whose argument is a scoped name — does **not** spread into
    /// a template body. `--tree`, iterations 3, the body still holds:
    ///
    /// ```text
    /// TemplateInstanceF ScopedNameF ["$quasi1"] Nothing IdentifierF "Box"
    ///   SeqF [TypeIdentifierF ["$quasi1"] ScopedNameF ["$quasi1"] Nothing IdentifierF "My"]
    /// ```
    #[test]
    fn spread_respects_scope_fields() {
        let (ir, interner) = build(
            "template <typename T>\nstruct Box\n{\n    T inner;\n}\n\n\
             struct My\n{\n    uint8 f;\n}\n\n\
             template <typename U>\nstruct Wrap\n{\n    Box<My> fixed;\n}\n\n\
             inline uint32 use()\n{\n    Box<My> b;\n    return 0;\n}\n",
        );
        let boxes = instances_named(&ir, &interner, "Box");
        assert_eq!(boxes.len(), 2);
        let use_site = *boxes.last().expect("use site last");
        let mut decided = DecidedInstances::default();
        decided.insert(
            Site::from(ir.arena[use_site].src.head),
            DecidedSite { template: "Box".to_string(), stamps: Vec::new() },
        );
        let (out, resolved, spread, _) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            ["Box<|@My|>"]
        );
        assert_eq!((resolved, spread), (1, 0));
        assert_eq!(
            instances_named(&out, &interner, "Box").len(),
            1,
            "the template-body Box<My> survives — its argument's scope differs"
        );
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// The literal re-stamp is applied to the mangled bytes (`validateArg`,
    /// `TypeCheck.hs:281-283`), and it gates spread: a stamped literal is
    /// structurally unlike every written one. `--tree`, iterations 4:
    ///
    /// ```text
    /// IdentifierF "W2<|@44|>"     (written W2<300>, uint8 N — truncated)
    /// IdentifierF "W3<|@300|>"    (auto M — untouched)
    /// ```
    ///
    /// and in the spread fixture the body's `W2<44>` stays a
    /// `TemplateInstanceF … IntLiteralF {literalType = Nothing, …}` while
    /// `W3<7>` resolves there.
    #[test]
    fn the_literal_restamp_truncates_and_gates_spread() {
        let (ir, interner) = build(
            "template <uint8 N>\nstruct W2\n{\n    uint8 v;\n}\n\n\
             template <auto M>\nstruct W3\n{\n    uint8 v;\n}\n\n\
             template <typename U>\nstruct Wrap\n{\n    W2<44> a;\n    W3<7> b;\n}\n\n\
             inline uint32 use()\n{\n    W2<300> c;\n    W2<44> d;\n    W3<300> e;\n\
             \x20   W3<7> f;\n    return 0;\n}\n",
        );
        let mut decided = DecidedInstances::default();
        let w2 = instances_named(&ir, &interner, "W2");
        let w3 = instances_named(&ir, &interner, "W3");
        assert_eq!((w2.len(), w3.len()), (3, 3));
        // Sites: the four in `use` only (text order puts Wrap's copies first).
        for &id in &w2[1..] {
            decided.insert(
                crate::rewrite::Site::from(ir.arena[id].src.head),
                DecidedSite {
                    template: "W2".to_string(),
                    stamps: vec![Some(LiteralType::unsigned(8))],
                },
            );
        }
        for &id in &w3[1..] {
            decided.insert(
                crate::rewrite::Site::from(ir.arena[id].src.head),
                DecidedSite { template: "W3".to_string(), stamps: vec![None] },
            );
        }
        let (out, resolved, spread, declined) = run(&ir, &interner, &decided);
        assert_eq!(
            mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
            // Wrap's body: W2<44> survives (stamped shapes never spread),
            // W3<7> resolves by shape; then the four written in `use`.
            ["W3<|@7|>", "W2<|@44|>", "W2<|@44|>", "W3<|@300|>", "W3<|@7|>"]
        );
        assert_eq!((resolved, spread, declined), (4, 1, 0));
        assert_eq!(
            instances_named(&out, &interner, "W2").len(),
            1,
            "the template-body W2<44> survives — the decided key's literal is stamped"
        );
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// Depth pinned on the oracle at 4, linearity pinned at 64. `--tree`,
    /// iterations 8:
    ///
    /// ```text
    /// IdentifierF "B0<|@B0<|@B0<|@B0<|$uint@8|>|>|>|>"
    /// ```
    ///
    /// Corpus-wide the deepest mangled name is 5,040 characters at depth 10;
    /// the arena bound is what fails if resolution ever costs a round per
    /// level (each round replaces the whole spine again) instead of one
    /// bottom-up walk.
    #[test]
    fn deep_nesting_composes_in_one_walk() {
        fn expected(depth: usize) -> String {
            let mut m = "$uint@8".to_string();
            for _ in 0..depth {
                m = format!("@B0<|{m}|>");
            }
            m[1..].to_string()
        }
        assert_eq!(expected(4), "B0<|@B0<|@B0<|@B0<|$uint@8|>|>|>|>");

        for depth in [4usize, 64] {
            let mut arg = "uint8".to_string();
            for _ in 0..depth {
                arg = format!("B0<{arg} >");
            }
            let text = format!(
                "template <typename T>\nstruct B0\n{{\n    T v;\n}}\n\n\
                 inline uint32 use()\n{{\n    {arg} d;\n    return 0;\n}}\n"
            );
            let (ir, interner) = build(&text);
            let decided = decide(&ir, &interner, &[("B0", "B0")]);
            let (out, resolved, ..) = run(&ir, &interner, &decided);
            assert_eq!(
                mentions(&out, &interner).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
                [expected(depth).as_str()]
            );
            assert_eq!(resolved as usize, depth);
            assert!(
                out.arena.len() <= ir.arena.len() + 6 * depth + 32,
                "appends must be linear in depth: {} nodes for depth {depth} over {}",
                out.arena.len() - ir.arena.len(),
                ir.arena.len(),
            );
            resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
        }
    }

    /// `mangle _ = undefined` (`Mangle.hs:63`) — but note what the oracle
    /// actually does with this fixture: it prints `W3<|@3|>`, because
    /// `evaluateConstexpr` folded the named constant to a literal long before
    /// anything mangled. The `undefined` never fires there. Our IR keeps the
    /// `NamedValue` (constexpr folding is unported), the census therefore
    /// never decides such an instance (silence 2 of `kg_db::instances`), and
    /// the map below fabricates the decision precisely to pin the decline
    /// path: the occurrence is counted, survives as written, and the
    /// invariant accepts the decline as justified — never mangled `@K`,
    /// which no pipeline produces. When constexpr folding lands, the folded
    /// literal mangles like any other and this path stops being reachable
    /// from honest input.
    #[test]
    fn an_unmangleable_argument_declines_the_whole_occurrence() {
        let (ir, interner) = build(
            "const uint8 K = 3;\n\n\
             template <auto M>\nstruct W3\n{\n    uint8 v;\n}\n\n\
             inline uint32 use()\n{\n    W3<K> a;\n    return 0;\n}\n",
        );
        let decided = decide(&ir, &interner, &[("W3", "W3")]);
        assert_eq!(decided.len(), 1);
        let (out, resolved, spread, declined) = run(&ir, &interner, &decided);
        assert_eq!((resolved, spread, declined), (0, 0, 1));
        assert!(mentions(&out, &interner).is_empty());
        assert_eq!(instances_named(&out, &interner, "W3").len(), 1);
        resolved_mentions_are_conserved(&ir, &out, &interner, &decided).unwrap();
    }

    /// The mangled identifier wears the *unmangled* one's note — `renote`
    /// (`Mangle.hs:27`) — so its text is not its range's text, and it is
    /// `Donated` so no position query is answered from it. `--notes`,
    /// iterations 1, the resolved mention:
    ///
    /// ```text
    /// IdentifierF "Holder<|$uint@8|>" @Typed {…
    ///   untype = Src {offset = 80, … sourceColumn = Pos 5 … Pos 11 …}}
    /// ```
    ///
    /// — offset 80 columns 5–11 is the written `Holder`, six characters, worn
    /// by a seventeen-character name.
    #[test]
    fn the_mangled_identifier_wears_the_unmangled_ones_note() {
        let (ir, interner) = build(HOLDER);
        let inst = instances_named(&ir, &interner, "Holder")[0];
        let Exp::TemplateInstance { base, .. } = ir.arena[inst].exp else { panic!() };
        let Exp::ScopedName { ident: written_ident, .. } = ir.arena[base].exp else { panic!() };

        let decided = decide(&ir, &interner, &[("Holder", "Holder")]);
        let (out, ..) = run(&ir, &interner, &decided);
        let (text, mention) = mentions(&out, &interner).remove(0);
        let Exp::ScopedName { ident, .. } = out.arena[mention].exp else { panic!() };

        assert_eq!(out.arena[ident].src.head, ir.arena[written_ident].src.head);
        assert_eq!(out.arena[ident].src.written, Written::Donated);
        let range = out.arena[ident].src.head.range;
        assert_ne!(
            u32::from(range.end() - range.start()) as usize,
            text.len(),
            "the name outgrew the range it wears — its text is not its range's text"
        );
    }

    /// The replacement `ScopedName` stands where the `TemplateInstance` stood
    /// and wears its `Src` verbatim — the driver's `replace`. `--notes` at
    /// iterations 0 vs 1: the `TemplateInstanceF`'s own note (offset 80,
    /// columns 5–11) is exactly the note the resolved `ScopedNameF` wears.
    #[test]
    fn the_replacement_wears_the_template_instance_note() {
        let (ir, interner) = build(HOLDER);
        let inst = instances_named(&ir, &interner, "Holder")[0];
        let decided = decide(&ir, &interner, &[("Holder", "Holder")]);
        let (out, ..) = run(&ir, &interner, &decided);
        let (_, mention) = mentions(&out, &interner).remove(0);
        assert_eq!(out.arena[mention].src.head, ir.arena[inst].src.head);
        assert_eq!(out.arena[mention].src.written, ir.arena[inst].src.written);
        assert_eq!(out.arena[mention].kind, ir.arena[inst].kind);
    }

    /// Both halves of [`resolved_mentions_are_conserved`] bite with no unit
    /// assertion in front: pairing the pass's *input* with itself trips the
    /// fixpoint clause (a decided site with manglable arguments survived),
    /// and pairing the *output* with itself trips the minting clause (a
    /// mention stands where "before" held no instance).
    #[test]
    fn the_invariant_bites_on_both_halves() {
        let (ir, interner) = build(HOLDER);
        let decided = decide(&ir, &interner, &[("Holder", "Holder")]);
        let err = resolved_mentions_are_conserved(&ir, &ir, &interner, &decided)
            .expect_err("the unresolved input is not at fixpoint");
        assert!(err.contains("fixpoint"), "{err}");

        let (out, ..) = run(&ir, &interner, &decided);
        let err = resolved_mentions_are_conserved(&out, &out, &interner, &decided)
            .expect_err("the output holds a mention its own 'before' cannot justify");
        assert!(err.contains("mint"), "{err}");

        // Pairing the input with an unrelated program trips the third clause:
        // the instance is gone with nothing standing for it. The second
        // program must share the first's interner — symbols do not compare
        // across interners.
        let parse = kg_syntax::parse_file("inline void solo()\n{\n}\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let other = lower_sorted(&file, FileId(0), &interner, &NoImports);
        let err = resolved_mentions_are_conserved(&ir, &other, &interner, &decided)
            .expect_err("the instance vanished without a mention");
        assert!(err.contains("neither survived nor resolved"), "{err}");
    }
}
