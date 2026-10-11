//! `moveFunctions` (`Function.hs:84-128`) and its only helper
//! `moveToParentScope` (`:28-82`) — inner sub-pass **4** of
//! `instantiateTemplates` in execution order (`Template.hs:71-81` is a
//! `foldr ($)` list written in reverse execution order; `moveFunctions` is
//! `:76`, one *after* `abbreviatedFunctionTemplate`), and the fourth rewrite
//! of this port to land. `--template-passes 3` vs `4` isolates it on the
//! oracle.
//!
//! For each `Function` or `NestedScope` it hoists every `Function`,
//! `Template`-of-`Function` and `Class` declared in the body out to the parent
//! scope; for each `Class` it hoists only `FunctionKind::LambdaFunction`
//! declarations (`functionsAndClasses` / `lambdaFunctions`,
//! `Function.hs:121-127`), so a lambda written in a member function does not
//! become a member. A moved declaration's name is mangled `parent@child`
//! (`:39`), and the mangling *composes* — the moves run bottom-up, so `Do`
//! inside `Go` inside member `Run` of `class C` lands in the **class body**
//! (the class arm selects only lambda functions) as `Run@Go@Do`:
//!
//! ```text
//! ClassF C body = [ Public
//!   , FunctionF { scope=["C"], name="Run@Go@Do", params=[c@["C","Run@Go@Do"]], … }
//!   , FunctionF { scope=["C"], name="Run@Go", … }
//!   , FunctionF { scope=["C"], name="Run", … }
//!   , VariableF { scope=["C"], name="m", flags=DeclareClass } ]
//! ```
//!
//! # The two catas over the body, and why they are two
//!
//! `moveToParentScope` first runs `desugar qualifyName` over the whole body —
//! every unqualified reference whose symbol the **body-local** map resolves
//! gains a full qualifier chain — and then `desugar fixScopedName` over the
//! result (`:35`). They cannot be fused into one algebra: `fixScopedName`
//! must fire on the *chain nodes `qualifyName` just built*. That is the whole
//! re-rooting mechanism — a reference to a moved item is a chain whose inner
//! node's qualified name equals the item's, and the fix rewrites that chain
//! node to `parent-qualifier ++ [parent@item]`; composed across arms it is
//! what turns the `c` reference in the example above into
//! `C::Run@Go@Do::c` in two steps. A single fused cata never revisits nodes
//! an algebra created and misses every one of them.
//!
//! # The symbol map is per-node, and its scope walk starts at the *renamed* use
//!
//! `symbols = getSymbols'' AllSymbols $ body parent` (`:33`) — built from the
//! body of the node being processed, never whole-program. Measured on the
//! reference (and recorded at `docs/spec/resolve.md:828`): a whole-program map
//! qualifies a name the reference leaves bare, because `body parent` excludes
//! the parent's own parameter list — `Run`'s `a` in the example stays a bare
//! `NamedValue a`. The map's precedence is pre-order first-wins (the reference
//! is `HashMap.fromList . reverse . para update`, `Symbols.hs:43-71`; later
//! entries win in `fromList`, so after `reverse` the shallowest, earliest
//! entry does). Inside `move` the lookups run *after* `modifyDeclScopeName`
//! renamed the use site's scope fields (the cata is bottom-up and the two are
//! composed per node, `:42`), so the walk starts at a scope the map has no
//! entries under and succeeds only at its unqualified tail — which is why
//! `qualifyName` having run first is load-bearing and not an ordering
//! accident.
//!
//! # Only the class arm can rewrite members, and deferral is not exemption
//!
//! `memberAccess` (`:52-64`) runs inside every `move`, but its lookups hit
//! `DeclareClass` variables only when the map was built from a class body — so
//! only lambda functions hoisted *out of a class* get `$this._x` values,
//! `this.Ping()` calls and const-member inlining; `m` in the example stays
//! bare. And the five deferral arms (`:88-94`) protect a subtree's *interior*
//! from the para's arms, not the node from being moved: an abbreviated
//! function's leftover, its `Template` twin and a local `class` are all still
//! *selected* by the enclosing arm, mangled and scope-renamed — with their
//! interiors carried verbatim (a local class keeps its lambda functions
//! inside its member bodies; the class arm never ran on it).
//!
//! # Where this port deliberately diverges
//!
//! * **The reference crashes on a shape this port declines.** A pre-window
//!   pass substitutes a const variable's initializer into callee position as
//!   `FunctionSpecifierF Nothing (NamedValueF …)`; when that name resolves to
//!   a `DeclareClass` variable, the value arm rewrites the callee child
//!   bottom-up and the call arm then applies `lookupSymbol` — i.e. `getScope`
//!   — to the rewritten `MemberAccessF`/`IntLiteralF`, and `getScope`'s
//!   catch-all is the partial selector `declScopeName` (`Parser/Syntax.hs:780`
//!   region). Reproduced on the reference 2026-08-13 (`const auto h = K;
//!   return h();` in an in-class lambda, `K` a const member: `kg-oracle: No
//!   match in record selector declScopeName`; profiling build blames
//!   `lookupSymbol` under `moveFunctions`). No oracle-readable corpus file
//!   reaches it. Here [`lookup_symbol`] answers `None` for a non-name node and
//!   the call arm declines — fail toward silence.
//! * **Eager, so the no-op case is computed first.** In Haskell `null
//!   selected` (`:30`) forces only the body items' top constructors; the
//!   symbol map and both catas stay unforced thunks. `cond` and
//!   `getQualifiedName` read only the top constructor, `funcKind`,
//!   `declScopeName` and `declName`, none of which the catas change — so
//!   [`MoveFunctions`] partitions on the *unrewritten* body and skips the map
//!   and both catas when nothing is selected, which is exact, not an
//!   approximation. 219 of the 357 oracle-readable corpus files are no-ops
//!   for this pass, and the reference's own whole-corpus sweep pays +20 % CPU
//!   for this one sub-pass; an eager port without the skip pays it on every
//!   file.
//! * **Unsorted names do not participate.** This IR keeps a path the lowering
//!   could not sort as [`Exp::Path`] (divergence #22). The reference has no
//!   such node; every arm here matches the sorted forms only, so an unsorted
//!   name is carried through untouched rather than guessed at.
//!
//! # What the reference runs right after, which is not this pass
//!
//! At `--template-iterations 0` the oracle applies `desugar
//! deduplicateInstances` after the sub-pass window (`Template.hs:57,144-155`),
//! which `nubBy`-collapses same-`Seq` siblings of equal constructor and
//! qualified name. Two same-named functions moved out of an `if`'s two
//! branches therefore print as **one** declaration in oracle output while this
//! pass — correctly — emits both. Any instrument that counts declarations
//! against the oracle must account for that step; the unit tests here assert
//! the pre-dedup truth (the ledger is a multiset).
//!
//! # Statement lifts, and the corpus that cannot see them
//!
//! The `DoWhile`/`RangeFor`/`If` arms (`:104-117`) fire only when a statement's
//! body is a bare `Seq` — which happens exactly when the `NestedScope` under
//! it was itself rewritten into `SeqF [moved…, remaining]` — and hoist the
//! moved declarations one statement further out. Removing any of them changes
//! zero oracle-readable corpus files (measured by mutating the reference and
//! re-sweeping): their only corpus home is `test/syntax/local-functions.k`,
//! which the oracle rejects whole, so **no oracle-graded whole-corpus gate can
//! see these arms** — nor the `NestedScope` arm (`:99`) or the `StaticFor`
//! defer (`:92`). The unit tests below are their only instrument; each
//! fixture's expectation was read off the oracle as a scratch extract, where
//! all of them fire. (On this port's own corpus run the arms *are* live —
//! our parser reads `local-functions.k` — and every written `static for`
//! reaches this pass undeferred by nothing: `staticControl` is unported, so
//! the `StaticFor` defer is exercised on every file that writes one.)

use kg_base::{FxHashMap, FxHashSet, Interner, Symbol};
use kg_resolve::DeclareFlag;
use kg_ty::{FunctionKind, QualifiedName};

use crate::arena::{ExpArena, ExpId};
use crate::exp::Exp;
use crate::rewrite::{
    desugar, get_name, get_qualifier, ident_sym, is_abbreviated_template, is_local_scope, local_identifier,
    scoped_qualifier, Rewriter, ShallowRewrite,
};

/// `getSymbols''` builds `HashMap QualifiedName (NotedExp n e)`
/// (`Symbols.hs:26,43-44`); the value here is the declaration's node id.
///
/// With it, the last segment of every key. A lookup probes only keys that
/// end in the name it looks up ([`lookup_key`]), so a name no key ends in
/// is answered `None` without a key built — most names of a body, which
/// `qualifyName` asks after one by one (with `fixScopedName`'s like test,
/// most of `moveFunctions` on `risc-v.k`, 2026-10-10). Nothing iterates the
/// map, so its hasher is free to be the fast one.
#[derive(Debug, Default)]
struct SymbolMap {
    keys: FxHashMap<Vec<Symbol>, ExpId>,
    last: FxHashSet<Symbol>,
}

impl SymbolMap {
    /// `key` filed, unless it already is: the first declaration wins.
    fn file(&mut self, key: Vec<Symbol>, id: ExpId) {
        if let Some(&last) = key.last() {
            self.last.insert(last);
        }
        self.keys.entry(key).or_insert(id);
    }
}

/// Which predicate `moveToParentScope` partitions with.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Select {
    /// `functionsAndClasses` (`Function.hs:121-124`) — the `Function` and
    /// `NestedScope` arms.
    FunctionsAndClasses,
    /// `lambdaFunctions` (`Function.hs:126-127`) — the `Class` arm.
    LambdaFunctions,
}

/// `moveFunctions = para move` (`Function.hs:84-119`) as a [`ShallowRewrite`]:
/// the five deferral arms are `reverts`, everything else is `rewrite`.
#[derive(Debug)]
pub struct MoveFunctions<'i> {
    interner: &'i Interner,
    /// How many declarations were moved — for a gate's report, not for any
    /// decision.
    pub moved: u32,
}

impl<'i> MoveFunctions<'i> {
    pub fn new(interner: &'i Interner) -> MoveFunctions<'i> {
        MoveFunctions { interner, moved: 0 }
    }
}

impl ShallowRewrite for MoveFunctions<'_> {
    /// What the arms below can change: a parent whose body holds something
    /// [`selects`] could pick, a loop or `if` whose body is a bare `Seq`, and
    /// a `Seq` in a `Seq` for the catch-all's `removeNestedSeq`.
    fn needs(&self) -> Option<crate::facts::Flags> {
        Some(crate::facts::MOVES | crate::facts::NESTED_SEQ)
    }

    /// The deferral arms (`Function.hs:88-94`): `Template`, an abbreviated
    /// `Function`, `StaticIf`, `StaticFor`, and a local `Class`. Each returns
    /// `para`'s first argument — the original subtree — so the interior stays
    /// as written until the template is instantiated. Note what this does
    /// *not* do: the node itself is still selected and moved by the enclosing
    /// arm (see the module comment).
    fn reverts(&self, arena: &ExpArena, id: ExpId) -> bool {
        match &arena[id].exp {
            Exp::Template { .. } | Exp::StaticIf { .. } | Exp::StaticFor { .. } => true,
            // `isAbbreviatedTemplate` (`Parser/Syntax.hs:722`) — any bare
            // `auto` parameter, no name requirement (unlike `abbreviated.rs`'s
            // firing guard, which folds one in for crash avoidance; a defer
            // needs no name).
            Exp::Function { params, .. } => is_abbreviated_template(arena, *params),
            Exp::Class { scope, .. } => is_local_scope(self.interner, scope),
            _ => false,
        }
    }

    fn rewrite(&mut self, rw: &mut Rewriter, id: ExpId, exp: Exp) -> Exp {
        match exp {
            // `Function.hs:97,99`
            e @ (Exp::Function { .. } | Exp::NestedScope { .. }) => {
                self.move_to_parent_scope(rw, id, e, Select::FunctionsAndClasses)
            }
            // `Function.hs:102`
            e @ Exp::Class { .. } => self.move_to_parent_scope(rw, id, e, Select::LambdaFunctions),
            // `Function.hs:104-105` — the body is a bare `Seq` only when the
            // `NestedScope` under the loop was itself rewritten; hoist the
            // moved declarations one statement further out and keep the last
            // item (always the `remaining` scope) as the loop body.
            Exp::DoWhile { attr, cond, body } => match seq_items(rw.arena, body) {
                Some(c) if !c.is_empty() => {
                    let (init, last) = (&c[..c.len() - 1], c[c.len() - 1]);
                    let inner = rw.donate(id, Exp::DoWhile { attr, cond, body: last });
                    let mut items = init.to_vec();
                    items.push(inner);
                    Exp::Seq(items)
                }
                _ => Exp::DoWhile { attr, cond, body },
            },
            // `Function.hs:107-108`
            Exp::RangeFor { attr, var, limit, body } => match seq_items(rw.arena, body) {
                Some(d) if !d.is_empty() => {
                    let (init, last) = (&d[..d.len() - 1], d[d.len() - 1]);
                    let inner = rw.donate(id, Exp::RangeFor { attr, var, limit, body: last });
                    let mut items = init.to_vec();
                    items.push(inner);
                    Exp::Seq(items)
                }
                _ => Exp::RangeFor { attr, var, limit, body },
            },
            // `Function.hs:110-117` — three arms in the reference's order:
            // both branches, then-only, else-only.
            Exp::If { cond, then_branch, else_branch } => {
                let b = seq_items(rw.arena, then_branch);
                let c = else_branch.and_then(|e| seq_items(rw.arena, e));
                match (b, c) {
                    (Some(b), Some(c)) if !b.is_empty() && !c.is_empty() => {
                        let inner = rw.donate(
                            id,
                            Exp::If {
                                cond,
                                then_branch: b[b.len() - 1],
                                else_branch: Some(c[c.len() - 1]),
                            },
                        );
                        let mut items = b[..b.len() - 1].to_vec();
                        items.extend_from_slice(&c[..c.len() - 1]);
                        items.push(inner);
                        Exp::Seq(items)
                    }
                    (Some(b), _) if !b.is_empty() => {
                        let inner = rw.donate(
                            id,
                            Exp::If { cond, then_branch: b[b.len() - 1], else_branch },
                        );
                        let mut items = b[..b.len() - 1].to_vec();
                        items.push(inner);
                        Exp::Seq(items)
                    }
                    (_, Some(c)) if !c.is_empty() => {
                        let inner = rw.donate(
                            id,
                            Exp::If { cond, then_branch, else_branch: Some(c[c.len() - 1]) },
                        );
                        let mut items = c[..c.len() - 1].to_vec();
                        items.push(inner);
                        Exp::Seq(items)
                    }
                    _ => Exp::If { cond, then_branch, else_branch },
                }
            }
            // `Function.hs:119`
            other => rw.remove_nested_seq(other),
        }
    }
}

impl MoveFunctions<'_> {
    /// `moveToParentScope` (`Function.hs:28-82`). `exp` is the parent node
    /// with its children already rewritten; the returned `Seq` is spliced flat
    /// by the enclosing `Seq`'s `removeNestedSeq` — or, in a statement
    /// position, lifted further by the statement arms above.
    fn move_to_parent_scope(
        &mut self,
        rw: &mut Rewriter,
        id: ExpId,
        exp: Exp,
        cond: Select,
    ) -> Exp {
        let (parent_qualifier, parent_name_id, body) = match &exp {
            Exp::Function { scope, name, body, .. }
            | Exp::NestedScope { scope, name, body }
            | Exp::Class { scope, name, body } => (scope.clone(), *name, *body),
            _ => unreachable!("checked by the caller"),
        };
        let Some(items) = seq_items(rw.arena, body) else { return exp };

        // `null selected` (`:30`), decided on the unrewritten body — see the
        // module comment for why that is exact and why an eager port must.
        if !items.iter().any(|&i| selects(rw.arena, i, cond)) {
            return exp;
        }
        // `getName parent` (`:39`); a parent whose name is not an `Identifier`
        // cannot mangle — the reference's `getName` would crash, this declines.
        let Some(parent_name) = ident_sym(rw.arena, parent_name_id) else { return exp };

        // `getSymbols'' AllSymbols $ body parent` (`:33`).
        let symbols = collect_symbols(rw.arena, rw.interner, body);

        // `desugar fixScopedName $ desugar qualifyName $ body parent` (`:35`)
        // — two catas, deliberately not fused (module comment).
        let body = desugar(rw, body, &mut |rw, _id, e| qualify_name(rw, &symbols, e));

        // `selectedQualifiedNames` (`:69-70`), read off the qualified body:
        // `fixScopedName` changes none of the fields these read, so reading
        // them before it runs is the same fixpoint Haskell's laziness ties.
        let items = seq_items(rw.arena, body).expect("a Seq stays a Seq under the catas");
        let sel_qnames: Vec<Vec<Symbol>> = items
            .iter()
            .filter(|&&i| selects(rw.arena, i, cond))
            .filter_map(|&i| item_qualified_name(rw.arena, i))
            .collect();

        // A set: checked at every scoped name of the body, a list was
        // quadratic in the items moved — a `static for` of 65535 iterations
        // with a lambda in each grew `kg check` past 11 GB in 90 s
        // (difffuzz, 2026-09-24).
        let sel_qnames: FxHashSet<Vec<Symbol>> = sel_qnames.into_iter().collect();
        let sel_leaves: FxHashSet<Symbol> = sel_qnames.iter().filter_map(|q| q.last().copied()).collect();
        let body = desugar(rw, body, &mut |rw, _id, e| {
            fix_scoped_name(rw, &sel_qnames, &sel_leaves, &parent_qualifier, parent_name, e)
        });

        // `partition cond` (`:35`) — now over the fully rewritten items.
        let items = seq_items(rw.arena, body).expect("a Seq stays a Seq under the catas");
        let (selected, rest): (Vec<ExpId>, Vec<ExpId>) =
            items.iter().partition(|&&i| selects(rw.arena, i, cond));

        // `map move selected ++ [NotedExp n remaining]` (`:31`).
        let mut out = Vec::with_capacity(selected.len() + 1);
        for &s in &selected {
            out.push(move_one(rw, &symbols, &parent_qualifier, parent_name, s));
        }
        self.moved += selected.len() as u32;

        // `remaining = parent { body = copyNote (SeqF rest) $ body parent }`
        // (`:37`) — the body `Seq` keeps its own note (a replacement; the old
        // list goes unreachable). The rebuilt parent wears the node's note but
        // is `Donated`: the driver's `Seq` replacement is the one claimant of
        // that key.
        let new_body = rw.replace(body, Exp::Seq(rest));
        let mut remaining = exp;
        match &mut remaining {
            Exp::Function { body, .. }
            | Exp::NestedScope { body, .. }
            | Exp::Class { body, .. } => *body = new_body,
            _ => unreachable!("checked above"),
        }
        out.push(rw.donate_keeping_note(id, remaining));
        Exp::Seq(out)
    }
}

/// `move e = renote mangleDeclName $ desugar (memberAccess .
/// modifyDeclScopeName newScope) e` (`Function.hs:42-50`).
fn move_one(
    rw: &mut Rewriter,
    symbols: &SymbolMap,
    parent_qualifier: &QualifiedName,
    parent_name: Symbol,
    e: ExpId,
) -> ExpId {
    let (Some(e_qualifier), Some(e_name)) =
        (item_qualifier(rw.arena, e), item_name(rw.arena, e))
    else {
        // `selects` requires a resolvable name, so this is unreachable through
        // it; decline rather than reproduce the reference's crash.
        return e;
    };
    let mangled = mangle(rw.interner, parent_name, e_name);
    let mut item_qname: Vec<Symbol> = e_qualifier.segments().to_vec();
    item_qname.push(e_name);
    let ns = NewScope {
        item_qualifier: e_qualifier,
        item_qname,
        parent_qualifier: parent_qualifier.clone(),
        // `newQualifiedName = getQualifier parent ++ [mangleName e]` (`:47`).
        new_qualified: parent_qualifier.child(mangled),
    };
    let moved = desugar(rw, e, &mut |rw, _id, x| {
        let x = modify_decl_scope_name(x, &ns);
        member_access(rw, symbols, x)
    });
    mangle_decl_name(rw, moved, mangled)
}

/// `mangleName e = fromString $ getName parent ++ "@" ++ getName e`
/// (`Function.hs:39`).
fn mangle(interner: &Interner, parent: Symbol, child: Symbol) -> Symbol {
    interner.intern(&format!("{}@{}", interner.resolve(parent), interner.resolve(child)))
}

/// `mangleDeclName` (`Function.hs:44-45`): swap the declaration's name for the
/// mangled `Identifier`, recursing through a `Template` wrapper. The new leaf
/// wears the old name's note and stands where it stood — a replacement, so it
/// keeps `Own`; the old leaf goes unreachable in the same step.
fn mangle_decl_name(rw: &mut Rewriter, id: ExpId, mangled: Symbol) -> ExpId {
    match rw.arena[id].exp.clone() {
        Exp::Template { params, decl } => {
            let decl = mangle_decl_name(rw, decl, mangled);
            rw.replace(id, Exp::Template { params, decl })
        }
        mut e @ (Exp::Function { .. } | Exp::Class { .. } | Exp::FunctionDecl { .. }) => {
            let slot = match &mut e {
                Exp::Function { name, .. }
                | Exp::Class { name, .. }
                | Exp::FunctionDecl { name, .. } => name,
                _ => unreachable!(),
            };
            *slot = rw.replace(*slot, Exp::Identifier(mangled));
            rw.replace(id, e)
        }
        // The reference's record update would crash here; `selects` admits
        // only the shapes above.
        _ => id,
    }
}

/// `newScope` (`Function.hs:48-50`): a scope equal to the item's own qualifier
/// re-roots to the parent's; a scope under the item's qualified name re-roots
/// under `parent-qualifier ++ [parent@item]`; anything else is left alone.
struct NewScope {
    item_qualifier: QualifiedName,
    item_qname: Vec<Symbol>,
    parent_qualifier: QualifiedName,
    new_qualified: QualifiedName,
}

impl NewScope {
    fn apply(&self, s: &QualifiedName) -> Option<QualifiedName> {
        if *s == self.item_qualifier {
            return Some(self.parent_qualifier.clone());
        }
        s.strip_prefix(&self.item_qname).map(|rest| self.new_qualified.concat(&rest))
    }
}

/// `modifyDeclScopeName newScope` (`Function.hs:48-50`).
fn modify_decl_scope_name(exp: Exp, ns: &NewScope) -> Exp {
    super::modify_decl_scope_name(exp, &mut |s| ns.apply(s))
}

/// `memberAccess` (`Function.hs:52-64`). Runs only inside `move`, so only on
/// hoisted declarations, and bites only when `symbols` was built from a class
/// body — see the module comment.
fn member_access(rw: &mut Rewriter, symbols: &SymbolMap, exp: Exp) -> Exp {
    match &exp {
        Exp::NamedValue(a) => {
            let a = *a;
            let Some(found) = lookup_symbol(rw.arena, symbols, a) else { return exp };
            let Exp::Variable { ty, name, init, flags: DeclareFlag::DeclareClass, .. } =
                rw.arena[found].exp.clone()
            else {
                return exp;
            };
            let is_const = matches!(rw.arena[ty].exp, Exp::Const(_));
            let const_fn_ty = matches!(&rw.arena[ty].exp,
                Exp::Const(inner) if matches!(rw.arena[*inner].exp, Exp::FunctionType { .. }));
            // `:53` — a function-typed const member is never inlined.
            if const_fn_ty {
                return member_of_this(rw, a, name);
            }
            if is_const && let Some(v) = init {
                // `:54` — nor is an initializer list.
                if matches!(rw.arena[v].exp, Exp::InitializerList(_)) {
                    return member_of_this(rw, a, name);
                }
                // `:55` — any other const initializer is spliced in whole.
                // `unNote $ unfix v` shares the initializer's children with
                // the class declaration, which survives — so the splice is a
                // copy (donor E4); the top node wears the use site's note via
                // the driver's replacement, exactly as the cata re-notes it.
                let copy = rw.deep_copy(v);
                return rw.arena[copy].exp.clone();
            }
            // `:56` — every other class variable, const-without-initializer
            // included.
            member_of_this(rw, a, name)
        }
        Exp::FunctionCall { attr, function, args } => {
            let (attr, function, args) = (*attr, *function, *args);
            let Exp::FunctionSpecifier { receiver: None, function: b } = rw.arena[function].exp
            else {
                return exp;
            };
            // `lookup_symbol` answering `None` for a non-name `b` is the
            // divergence the module comment records: the reference crashes
            // when the value arm has already rewritten a callee.
            let Some(found) = lookup_symbol(rw.arena, symbols, b) else { return exp };
            let is_member = match &rw.arena[found].exp {
                // `:59`
                Exp::Function { kind: FunctionKind::MemberFunction, .. } => true,
                // `:60`
                Exp::Template { decl, .. } => matches!(
                    rw.arena[*decl].exp,
                    Exp::Function { kind: FunctionKind::MemberFunction, .. }
                ),
                // `:61`
                Exp::Variable { flags: DeclareFlag::DeclareClass, .. } => true,
                _ => false,
            };
            if !is_member {
                return exp;
            }
            // `thisMethod b = functionSpecifier (Just $ thisValue b) b`
            // (`:67`): the new specifier wears `b`'s note (donor F2) and
            // reuses `b` itself as the callee; the old specifier is discarded.
            let tv = this_value(rw, b);
            let spec =
                rw.donate(b, Exp::FunctionSpecifier { receiver: Some(tv), function: b });
            Exp::FunctionCall { attr, function: spec, args }
        }
        _ => exp,
    }
}

/// `MemberAccessF (thisValue a) name` (`Function.hs:53-56`): `name` is the
/// class declaration's own `declName` node, and the declaration survives — so
/// the member leaf is a copy wearing the declaration's note (donor E3).
fn member_of_this(rw: &mut Rewriter, a: ExpId, decl_name: ExpId) -> Exp {
    let base = this_value(rw, a);
    let member = rw.deep_copy(decl_name);
    Exp::MemberAccess { base, member }
}

/// `thisValue x = thisNamedValue (getScope x) x` (`Function.hs:66`,
/// `Type.hs:991-994`): `NamedValue (QualifiedIdentifier (ScopedName scope
/// Nothing $this))`, the `$this` leaf and the `NamedValue` donated from `x`,
/// the wrappers from the leaf. The scope is read *after* `newScope` renamed
/// the use site — the cata is bottom-up.
fn this_value(rw: &mut Rewriter, x: ExpId) -> ExpId {
    let scope = get_scope(rw.arena, x).unwrap_or_default();
    let this_sym = rw.interner.intern(kg_resolve::names::THIS);
    let ident = rw.donate(x, Exp::Identifier(this_sym));
    let qi = local_identifier(rw, &scope, ident);
    rw.donate(x, Exp::NamedValue(qi))
}

/// `qualifyName` (`Function.hs:72-77`): give an unqualified reference the full
/// qualifier of whatever the body-local map resolves it to. The chain is
/// donated from `top`, the `QualifiedIdentifier`'s child (`scopedQualifier a x
/// …` — `x` is the child, not the leaf).
fn qualify_name(rw: &mut Rewriter, symbols: &SymbolMap, exp: Exp) -> Exp {
    let Exp::QualifiedIdentifier(x) = exp else { return exp };
    Exp::QualifiedIdentifier(qualify(rw, symbols, x, x))
}

fn qualify(rw: &mut Rewriter, symbols: &SymbolMap, top: ExpId, node: ExpId) -> ExpId {
    match rw.arena[node].exp.clone() {
        // `:74` — recurse through a template instance's base.
        Exp::TemplateInstance { base, args } => {
            let new_base = qualify(rw, symbols, top, base);
            if new_base == base {
                node
            } else {
                rw.replace(node, Exp::TemplateInstance { base: new_base, args })
            }
        }
        // `:75` — only a prefix-free name is qualified. A failed lookup gives
        // an empty qualifier and `scopedQualifier _ _ [] = Nothing`, so both
        // fall through unchanged.
        Exp::ScopedName { scope, prefix: None, ident } => {
            let Some(found) = lookup_symbol(rw.arena, symbols, top) else { return node };
            let Some(qualifier) = decl_qualifier(rw.arena, found) else { return node };
            let Some(prefix) = scoped_qualifier(rw, &scope, top, qualifier.segments()) else {
                return node;
            };
            rw.replace(node, Exp::ScopedName { scope, prefix: Some(prefix), ident })
        }
        _ => node,
    }
}

/// `fixScopedName` (`Function.hs:79-82`): a `ScopedName` — chain nodes
/// included, which is the re-rooting mechanism — whose qualified name equals a
/// selected item's is repointed at the moved declaration: prefix rebuilt over
/// the *parent's* qualifier (donated from the leaf, `scopedQualifier a b …`),
/// leaf swapped for the mangled name (a replacement wearing the old leaf's
/// note). A qualified name ends in its leaf, so one whose leaf ends no
/// selected name (`leaves`) is passed before its qualifier is read.
fn fix_scoped_name(
    rw: &mut Rewriter,
    selected: &FxHashSet<Vec<Symbol>>,
    leaves: &FxHashSet<Symbol>,
    parent_qualifier: &QualifiedName,
    parent_name: Symbol,
    exp: Exp,
) -> Exp {
    let Exp::ScopedName { scope, prefix, ident } = &exp else { return exp };
    let Some(leaf) = ident_sym(rw.arena, *ident) else { return exp };
    if !leaves.contains(&leaf) {
        return exp;
    }
    let mut qname = match prefix {
        None => Vec::new(),
        Some(p) => {
            let Some(mut q) = get_qualifier(rw.arena, *p) else { return exp };
            let Some(n) = get_name(rw.arena, *p) else { return exp };
            q.push(n);
            q
        }
    };
    qname.push(leaf);
    if !selected.contains(&qname) {
        return exp;
    }
    let mangled = mangle(rw.interner, parent_name, leaf);
    let new_prefix = scoped_qualifier(rw, scope, *ident, parent_qualifier.segments());
    let new_leaf = rw.replace(*ident, Exp::Identifier(mangled));
    Exp::ScopedName { scope: scope.clone(), prefix: new_prefix, ident: new_leaf }
}

// --- the body-local symbol map -----------------------------------------------

/// `getSymbols'' AllSymbols` (`Symbols.hs:43-71`) over one subtree. Pre-order
/// with `or_insert` reproduces the reference's precedence — shallower first,
/// earlier sibling first (see the module comment). `Struct` and `Union` insert
/// their own key and stop (`:48-49`); a `Class` also claims `[…, name,
/// $this]` (`:66`); a `Template` of anything but `Typename` claims its decl's
/// key (`:55-58`); an `Error` collapses to `$errors` (`:67`).
///
/// The precedence is transcription, pinned by nothing: flipping `or_insert`
/// to last-wins turns no test red (measured), because it is observable only
/// through two declarations sharing a full key whose *values* the arms then
/// tell apart — same scope, same name, different variant — and no legal
/// fixture writes that. Recorded rather than papered over.
fn collect_symbols(arena: &ExpArena, interner: &Interner, root: ExpId) -> SymbolMap {
    let mut map = SymbolMap::default();
    let this_sym = interner.intern(kg_resolve::names::THIS);
    let errors_sym = interner.intern(kg_resolve::names::ERRORS);
    // Recursion rather than an explicit stack: a stack would visit siblings in
    // reverse and flip the earlier-sibling-wins precedence.
    fn go(
        arena: &ExpArena,
        id: ExpId,
        map: &mut SymbolMap,
        this_sym: Symbol,
        errors_sym: Symbol,
    ) {
        let exp = &arena[id].exp;
        let mut insert = |scope: &QualifiedName, name: Symbol, extra: Option<Symbol>| {
            let mut key: Vec<Symbol> = scope.segments().to_vec();
            key.push(name);
            if let Some(x) = extra {
                key.push(x);
            }
            map.file(key, id);
        };
        match exp {
            Exp::Struct { scope, name, .. } | Exp::Union { scope, name, .. } => {
                if let Some(n) = ident_sym(arena, *name) {
                    insert(scope, n, None);
                }
                return; // `update e@Struct{}` stops (`Symbols.hs:48-49`)
            }
            Exp::Template { decl, .. } => {
                if !matches!(arena[*decl].exp, Exp::Typename)
                    && let (Some(q), Some(n)) =
                        (decl_qualifier(arena, *decl), item_name(arena, *decl))
                {
                    insert(&q, n, None);
                }
            }
            Exp::Alias { scope, name, .. }
            | Exp::Capture { scope, name }
            | Exp::Function { scope, name, .. }
            | Exp::FunctionDecl { scope, name, .. }
            | Exp::Enum { scope, name, .. }
            | Exp::EnumConstant { scope, name, .. }
            | Exp::Variable { scope, name, .. }
            | Exp::FuncParam { scope, name, .. } => {
                if let Some(n) = ident_sym(arena, *name) {
                    insert(scope, n, None);
                }
            }
            Exp::Class { scope, name, .. } => {
                if let Some(n) = ident_sym(arena, *name) {
                    insert(scope, n, None);
                    insert(scope, n, Some(this_sym));
                }
            }
            Exp::Error(_) => {
                map.file(vec![errors_sym], id);
            }
            _ => {}
        }
        let mut kids = Vec::new();
        arena[id].exp.children(&mut kids);
        for k in kids {
            go(arena, k, map, this_sym, errors_sym);
        }
    }
    go(arena, root, &mut map, this_sym, errors_sym);
    map
}

/// `lookupSymbol` over one module's own declarations ([`collect_symbols`]
/// of its root), for a pass that asks what a written name declares —
/// `addLambdaWrappers`' `wrap q` (`Function.hs:388`), whose member arms
/// only the module declaring the class reaches.
#[derive(Debug)]
pub struct Declarations(SymbolMap);

impl Declarations {
    pub fn of(ir: &crate::ModuleIr, interner: &Interner) -> Declarations {
        Declarations(collect_symbols(&ir.arena, interner, ir.root))
    }

    /// The declaration `name` — a name node — resolves to from its scope.
    pub fn lookup(&self, arena: &ExpArena, name: ExpId) -> Option<ExpId> {
        lookup_symbol(arena, &self.0, name)
    }

    /// The qualified name of every class an `extern T;` outside a template
    /// names (`externClasses`, `Frontend.hs:329-333`), each resolved from
    /// the `extern`'s own scope — a template's are untyped at pass 7.
    pub fn extern_classes(&self, ir: &crate::ModuleIr) -> Vec<Vec<Symbol>> {
        let mut out = Vec::new();
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            match &ir.arena[id].exp {
                Exp::Template { .. } => continue,
                Exp::Extern { ty, .. } => {
                    if let Exp::TypeIdentifier(_, name) = &ir.arena[*ty].exp
                        && let Some(found) = self.lookup(&ir.arena, *name)
                        && let Exp::Class { scope, name, .. } = &ir.arena[found].exp
                        && let Some(leaf) = ident_sym(&ir.arena, *name)
                    {
                        out.push(scope.segments().iter().copied().chain([leaf]).collect());
                    }
                }
                _ => {}
            }
            kids.clear();
            ir.arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
        out
    }

    /// Whether `path` names a class (a class template's too).
    pub fn is_class(&self, arena: &ExpArena, path: &[Symbol]) -> bool {
        self.0.keys.get(path).is_some_and(|&d| match &arena[d].exp {
            Exp::Class { .. } => true,
            Exp::Template { decl, .. } => matches!(arena[*decl].exp, Exp::Class { .. }),
            _ => false,
        })
    }

    /// `redundantNameScope`'s test (`Frontend.hs:161-163`) of a name node —
    /// its `ScopedName`, through a `QualifiedIdentifier`: the qualifier is
    /// written, and the bare leaf resolves from the name's scope to a
    /// declaration of that same qualifier, so pass 1 drops it. The pass-8
    /// tree keeps the written chain (the walk drops it at emission); a
    /// rewrite that reads `getQualifiedName` after pass 1 asks here.
    pub fn redundant_qualifier(&self, arena: &ExpArena, name: ExpId) -> bool {
        let name = match &arena[name].exp {
            Exp::QualifiedIdentifier(inner) => *inner,
            _ => name,
        };
        let Exp::ScopedName { scope, prefix: Some(_), ident } = &arena[name].exp else { return false };
        let Some(leaf) = get_name(arena, *ident) else { return false };
        let Some(found) = lookup_key(&self.0, scope.segments(), &[leaf]) else { return false };
        let Some(written) = get_qualifier(arena, name) else { return false };
        decl_qualifier(arena, found).map(|q| q.segments().to_vec()) == Some(written)
    }
}

/// `lookupSymbol` (`Symbols.hs:79-87`): try `scope ++ qualifiedName`, then
/// walk the scope up one segment at a time down to the bare qualified name.
/// `None` where the reference's `getScope`/`getQualifiedName` would crash —
/// the divergence the module comment records.
fn lookup_symbol(arena: &ExpArena, symbols: &SymbolMap, e: ExpId) -> Option<ExpId> {
    let scope = get_scope(arena, e)?;
    let mut qname = get_qualifier(arena, e)?;
    qname.push(get_name(arena, e)?);
    lookup_key(symbols, scope.segments(), &qname)
}

/// [`lookup_symbol`] of `qname` written in `scope`. Every key it probes
/// ends in `qname`'s last segment.
fn lookup_key(symbols: &SymbolMap, scope: &[Symbol], qname: &[Symbol]) -> Option<ExpId> {
    if qname.last().is_some_and(|last| !symbols.last.contains(last)) {
        return None;
    }
    let mut s: &[Symbol] = scope;
    loop {
        let mut key: Vec<Symbol> = Vec::with_capacity(s.len() + qname.len());
        key.extend_from_slice(s);
        key.extend_from_slice(qname);
        if let Some(&id) = symbols.keys.get(&key) {
            return Some(id);
        }
        if s.is_empty() {
            return None;
        }
        s = &s[..s.len() - 1];
    }
}

// --- `ExpLike` accessors, on the name spines this pass reads -----------------

/// `getScope` (`Parser/Syntax.hs:768-780`), name-node arms only.
fn get_scope(arena: &ExpArena, id: ExpId) -> Option<QualifiedName> {
    match &arena[id].exp {
        Exp::Identifier(_) => Some(QualifiedName::empty()),
        Exp::ScopedName { scope, .. } => Some(scope.clone()),
        Exp::This { scope } => Some(scope.clone()),
        Exp::TypeIdentifier(_, a)
        | Exp::QualifiedIdentifier(a)
        | Exp::NamedValue(a)
        | Exp::TemplateInstance { base: a, .. }
        | Exp::FunctionSpecifier { function: a, .. } => get_scope(arena, *a),
        _ => None,
    }
}

// `ident_sym`, `getQualifier` and `getName` lived here until `resolve` became
// their second caller; they are now `crate::rewrite`'s.

/// `getQualifier` on a *declaration* — `declScopeName e`, through a `Template`
/// wrapper (`Parser/Syntax.hs:753,765`).
fn decl_qualifier(arena: &ExpArena, id: ExpId) -> Option<QualifiedName> {
    match &arena[id].exp {
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
        | Exp::TypeParam { scope, .. }
        | Exp::TemplateParam { scope, .. } => Some(scope.clone()),
        Exp::Template { decl, .. } => decl_qualifier(arena, *decl),
        _ => None,
    }
}

// --- the partition predicate -------------------------------------------------

/// `functionsAndClasses` / `lambdaFunctions` (`Function.hs:121-127`), plus the
/// same malformed-name guard `lambdas.rs`'s `collect` carries: an item whose
/// `declName` is not an `Identifier` cannot be mangled, so it is not selected
/// where the reference would crash instead.
fn selects(arena: &ExpArena, id: ExpId, cond: Select) -> bool {
    let named = |name: &ExpId| matches!(arena[*name].exp, Exp::Identifier(_));
    match &arena[id].exp {
        Exp::Function { kind, name, .. } => {
            named(name)
                && (cond == Select::FunctionsAndClasses || *kind == FunctionKind::LambdaFunction)
        }
        Exp::Class { name, .. } => cond == Select::FunctionsAndClasses && named(name),
        Exp::Template { decl, .. } => match &arena[*decl].exp {
            Exp::Function { kind, name, .. } => {
                named(name)
                    && (cond == Select::FunctionsAndClasses
                        || *kind == FunctionKind::LambdaFunction)
            }
            _ => false,
        },
        _ => false,
    }
}

/// `getName` of a selected item — its `declName`, through a `Template`.
fn item_name(arena: &ExpArena, id: ExpId) -> Option<Symbol> {
    match &arena[id].exp {
        Exp::Function { name, .. } | Exp::Class { name, .. } | Exp::FunctionDecl { name, .. } => {
            ident_sym(arena, *name)
        }
        Exp::Template { decl, .. } => item_name(arena, *decl),
        _ => None,
    }
}

fn item_qualifier(arena: &ExpArena, id: ExpId) -> Option<QualifiedName> {
    decl_qualifier(arena, id)
}

/// `getQualifiedName` of a selected item (`Parser/Syntax.hs:672-673`).
fn item_qualified_name(arena: &ExpArena, id: ExpId) -> Option<Vec<Symbol>> {
    let mut q: Vec<Symbol> = item_qualifier(arena, id)?.segments().to_vec();
    q.push(item_name(arena, id)?);
    Some(q)
}


fn seq_items(arena: &ExpArena, id: ExpId) -> Option<Vec<ExpId>> {
    match &arena[id].exp {
        Exp::Seq(items) => Some(items.clone()),
        _ => None,
    }
}

// --- the whole-corpus invariant ----------------------------------------------

/// **The pass's own invariant**, recomputed rather than trusted, in two
/// halves.
///
/// **Fixpoint** (`Function.hs:97-102`, run to composition): in `after`,
/// walking top-down and stopping at every shape the pass defers on — a
/// `Template`, a `StaticIf`, a `StaticFor`, an abbreviated `Function`, and a
/// `Class` that is local-scoped *or* wears a mangled name (a moved local
/// class's scope was re-rooted, so only the `@` in its name still says its
/// interior was carried verbatim) — no `Function`/`NestedScope` body `Seq`
/// holds an item `functionsAndClasses` selects, and no `Class` body holds one
/// `lambdaFunctions` selects. Moved declarations parked in a non-body `Seq`
/// (a switch case's, say) are expected: the reference has no lift arm there.
///
/// **Conservation**: the multiset of `(variant, kind, trailing name)` over
/// every `Function` and `Class` in `before` equals the same multiset over
/// `after`, where the trailing name is everything after the last `@` — a move
/// mangles `parent@child` and changes nothing else, so declarations are
/// neither invented nor dropped, `funcKind` is untouched, and a collision
/// stays two entries (the ledger is a multiset; `deduplicateInstances`,
/// which would collapse it, is not this pass). `Alias` names are compared
/// whole — no alias is ever mangled, moved as an item, or dropped
/// (`functionsAndClasses` has no `AliasF` arm).
///
/// What it cannot see: the mangle *prefix* (a wrong parent name conserves the
/// trailing segment), where a moved declaration landed and in what order, the
/// scope renames, the reference-fixing catas, the `$this` rewrites, and every
/// `Src` donor — the unit tests own all of those. `before` must be the IR the
/// pass was handed (post-`abbreviatedFunctionTemplate` in composition) and
/// `after` this pass's own output.
pub fn moved_declarations_reach_fixpoint(
    before: &crate::ModuleIr,
    after: &crate::ModuleIr,
    interner: &Interner,
) -> Result<(), String> {
    // --- fixpoint over `after` ----------------------------------------------
    fn deferred(ir: &crate::ModuleIr, interner: &Interner, id: ExpId) -> bool {
        match &ir.arena[id].exp {
            Exp::Template { .. } | Exp::StaticIf { .. } | Exp::StaticFor { .. } => true,
            Exp::Function { params, .. } => is_abbreviated_template(&ir.arena, *params),
            Exp::Class { scope, name, .. } => {
                is_local_scope(interner, scope)
                    || ident_sym(&ir.arena, *name).is_some_and(|n| {
                        let n = interner.resolve(n);
                        n.contains('@') && !n.starts_with('@')
                    })
            }
            _ => false,
        }
    }
    fn check_body(
        ir: &crate::ModuleIr,
        interner: &Interner,
        body: ExpId,
        cond: Select,
        what: &str,
    ) -> Result<(), String> {
        let Exp::Seq(items) = &ir.arena[body].exp else { return Ok(()) };
        for &i in items {
            if selects(&ir.arena, i, cond) {
                return Err(format!(
                    "{what} body still holds movable declaration #{} ({}) — the pass \
                     did not reach its fixpoint (Function.hs:97-102)",
                    i.index(),
                    item_name(&ir.arena, i)
                        .map(|n| interner.resolve(n).to_string())
                        .unwrap_or_default(),
                ));
            }
        }
        Ok(())
    }
    fn walk_fixpoint(
        ir: &crate::ModuleIr,
        interner: &Interner,
        id: ExpId,
    ) -> Result<(), String> {
        if deferred(ir, interner, id) {
            return Ok(());
        }
        match &ir.arena[id].exp {
            Exp::Function { body, .. } | Exp::NestedScope { body, .. } => {
                check_body(ir, interner, *body, Select::FunctionsAndClasses, "a function's")?;
            }
            Exp::Class { body, .. } => {
                check_body(ir, interner, *body, Select::LambdaFunctions, "a class's")?;
            }
            _ => {}
        }
        let mut kids = Vec::new();
        ir.arena[id].exp.children(&mut kids);
        for k in kids {
            walk_fixpoint(ir, interner, k)?;
        }
        Ok(())
    }
    walk_fixpoint(after, interner, after.root)?;

    // --- conservation between `before` and `after` --------------------------
    fn ledger(ir: &crate::ModuleIr, interner: &Interner) -> (Vec<String>, Vec<String>) {
        let mut decls = Vec::new();
        let mut aliases = Vec::new();
        let mut seen = vec![false; ir.arena.len()];
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            if std::mem::replace(&mut seen[id.index()], true) {
                continue;
            }
            match &ir.arena[id].exp {
                Exp::Function { kind, name, .. } => {
                    if let Some(n) = ident_sym(&ir.arena, *name) {
                        let n = interner.resolve(n);
                        let trailing = n.rsplit('@').next().unwrap_or(&n);
                        decls.push(format!("fn/{kind:?}/{trailing}"));
                    }
                }
                Exp::Class { name, .. } => {
                    if let Some(n) = ident_sym(&ir.arena, *name) {
                        let n = interner.resolve(n);
                        let trailing = n.rsplit('@').next().unwrap_or(&n);
                        decls.push(format!("class/{trailing}"));
                    }
                }
                Exp::Alias { name, .. } => {
                    if let Some(n) = ident_sym(&ir.arena, *name) {
                        aliases.push(interner.resolve(n).to_string());
                    }
                }
                _ => {}
            }
            ir.arena[id].exp.children(&mut kids);
            stack.append(&mut kids);
        }
        decls.sort();
        aliases.sort();
        (decls, aliases)
    }
    let (decls_in, aliases_in) = ledger(before, interner);
    let (decls_out, aliases_out) = ledger(after, interner);
    if decls_in != decls_out {
        return Err(format!(
            "function/class declarations are not conserved up to `parent@` mangling \
             (Function.hs:39,44-45): before {decls_in:?}, after {decls_out:?}"
        ));
    }
    if aliases_in != aliases_out {
        return Err(format!(
            "aliases must survive a move untouched — `functionsAndClasses` has no Alias \
             arm (Function.hs:121-124): before {aliases_in:?}, after {aliases_out:?}"
        ));
    }
    Ok(())
}

/// The reference is the grader here, not this port's reading of it.
///
/// Every expectation below was read off the reference frontend built by
/// `oracle/build.sh`, captured 2026-08-13, at
///
/// ```text
/// kg-oracle --tree  --roots-only --no-base --passes 8 \
///           --template-passes {3,4} --template-iterations 0 FILE
/// kg-oracle --notes --roots-only --no-base --passes 8 \
///           --template-passes 4 --template-iterations 0 FILE
/// ```
///
/// `--template-passes 3` vs `4` isolates `moveFunctions` exactly (sub-pass 4
/// in execution order; `Template.hs:62-64` drops from the front of the
/// reversed list). `--passes 8` is not optional: without it `removeTemplate`
/// (`Frontend.hs:300-302`) deletes the output. One caveat the module comment
/// records: the oracle's printed tree has also been through
/// `deduplicateInstances` (`Template.hs:57`), which collapses same-`Seq`
/// name-equal siblings — the collision fixture asserts the pre-dedup truth.
/// Because our `Src` ranges and the reference's are different objects
/// (divergences #25, #26), every `--notes` expectation is asserted as donor
/// *identity* — the new node wears the same `Src` as a named donor node —
/// never as a raw offset.
///
/// The fixtures lower through `lower_sorted` + `NoImports`: this pass's name
/// machinery fires on the *sorted* forms only, and plain `lower` keeps every
/// written path as an unsorted `Exp::Path` (divergence #22).
#[cfg(test)]
mod tests {

    use kg_base::{FileId, Interner, Written};
    use kg_syntax::ast;
    use kg_syntax::AstNode;

    use super::*;
    use crate::rewrite::{
        abbreviated_function_template, capture_this, desugar_lambdas, move_functions,
    };
    use crate::sort::NoImports;
    use crate::{lower_sorted, ModuleIr};

    /// `before` is the IR the pass is handed in [`crate::rewrite::pipeline`] —
    /// after `captureThis`, `desugarLambdas` and `abbreviatedFunctionTemplate`
    /// — and `after` its output.
    /// `NotedExp n remaining` (`Function.hs:31,37`): the parent a local
    /// function moved out of keeps its note — donated, but recorded in a
    /// tracking arena's lineage, so a caller keeping types beside the arena
    /// (`kg_sem`) carries the parent's type over. Red with a plain
    /// `donate`: the rebuilt `f` stands for nothing, and its type — a
    /// copy's `setTypeOfInstance` note — is lost (`compose2<|…|>`'s walk
    /// name, `higher-order-functions.k::103` under `KG_LOOP`).
    #[test]
    fn the_parent_a_function_moved_out_of_keeps_its_note() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file("inline void f()\n{\n    inline void g()\n    {\n    }\n    g();\n}\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let mut lowered = lower_sorted(&file, FileId(0), &interner, &NoImports);
        lowered.arena.track_lineage();
        let f = lowered
            .arena
            .iter()
            .find_map(|(id, n)| match &n.exp {
                Exp::Function { name, .. } if get_name(&lowered.arena, *name) == Some(interner.intern("f")) => Some(id),
                _ => None,
            })
            .expect("f");
        let mut after = move_functions(&lowered, &interner);
        let lineage = after.arena.take_lineage();
        let rebuilt = after
            .arena
            .iter()
            .filter_map(|(id, n)| match &n.exp {
                Exp::Function { name, .. } if get_name(&after.arena, *name) == Some(interner.intern("f")) => Some(id),
                _ => None,
            })
            .last()
            .expect("the rebuilt f");
        assert_ne!(rebuilt.index(), f.index(), "f was rebuilt");
        let mut at = rebuilt.index();
        while let Some(&(_, o)) = lineage.iter().find(|(n, _)| n.index() == at) {
            at = o.index();
        }
        assert_eq!(at, f.index(), "the rebuilt f stands for the lowered one");
    }

    fn run(text: &str) -> (ModuleIr, ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "probe must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let lowered = lower_sorted(&file, FileId(0), &interner, &NoImports);
        let a = capture_this(&lowered, &interner);
        let b = desugar_lambdas(&a, &interner);
        let before = abbreviated_function_template(&b, &interner);
        let after = move_functions(&before, &interner);
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

    fn scope_strs(interner: &Interner, scope: &QualifiedName) -> Vec<String> {
        scope.segments().iter().map(|s| interner.resolve(*s).to_string()).collect()
    }

    /// The name a declaration wears, `Template`s looked through.
    fn decl_name(ir: &ModuleIr, interner: &Interner, id: ExpId) -> String {
        match &ir.arena[id].exp {
            Exp::Function { name, .. }
            | Exp::Class { name, .. }
            | Exp::FunctionDecl { name, .. }
            | Exp::NestedScope { name, .. }
            | Exp::Variable { name, .. }
            | Exp::Alias { name, .. } => text(ir, interner, *name),
            Exp::Template { decl, .. } => format!("template:{}", decl_name(ir, interner, *decl)),
            other => format!("{other:?}"),
        }
    }

    /// The `::`-joined segments a sorted name reference spells — prefix chain
    /// plus leaf — for asserting `qualifyName`/`fixScopedName` output.
    fn ref_path(ir: &ModuleIr, interner: &Interner, id: ExpId) -> String {
        let Some(mut q) = get_qualifier(&ir.arena, id) else {
            return format!("<unsorted {:?}>", ir.arena[id].exp);
        };
        q.push(get_name(&ir.arena, id).expect("a qualifier implies a leaf"));
        q.iter().map(|s| interner.resolve(*s).to_string()).collect::<Vec<_>>().join("::")
    }

    fn walk(ir: &ModuleIr, mut f: impl FnMut(&ModuleIr, ExpId)) {
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

    /// Every declaration matching `pred`, in walk order.
    fn find_all(ir: &ModuleIr, pred: impl Fn(&Exp) -> bool) -> Vec<ExpId> {
        let mut out = Vec::new();
        walk(ir, |ir, id| {
            if pred(&ir.arena[id].exp) {
                out.push(id);
            }
        });
        out.sort_by_key(|i| i.index());
        out
    }

    /// **The nested double-move**, `Function.hs:97,102` composed. `Do` inside
    /// `Go` inside member `Run` of `class C`, at `tp=4` (`--tree`, f3.k):
    ///
    /// ```text
    /// ClassF C body = SeqF
    ///  [ PublicF
    ///  , FunctionF {declScopeName=["C"], funcKind=MemberFunction, declName="Run@Go@Do",
    ///      funcParams=[FuncParamF {declScopeName=["C","Run@Go@Do"], … "c"}],
    ///      body=[ReturnF (Add
    ///        (NamedValueF … ScopedNameF ["C","Run@Go@Do","$"]
    ///           (Just … "C" … "Run@Go@Do") "c")
    ///        (NamedValueF … ScopedNameF ["C","Run@Go@Do","$"] Nothing "m"))]}
    ///  , FunctionF {… declName="Run@Go", body=[ReturnF (Add
    ///      (FunctionCallF {callFunction = … (Just … "C") "Run@Go@Do", …
    ///         callArgs=[NamedValueF … (Just … "C" … "Run@Go") "b"]}) …)]}
    ///  , FunctionF {… declName="Run", body=[ReturnF (FunctionCallF {… (Just … "C")
    ///      "Run@Go", callArgs=[NamedValueF … ScopedNameF ["C","Run","$"] Nothing "a"]})]}
    ///  , VariableF {declScopeName=["C"], declName="m", declFlags=DeclareClass} ]
    /// ```
    ///
    /// Mangling composes left; both land in the **class body** (the class arm
    /// selects only lambda functions); `m` stays a bare `NamedValue` — the
    /// class arm selected nothing, so `memberAccess` never saw it.
    #[test]
    fn the_nested_double_move_composes_left_and_lands_in_the_class_body() {
        let (before, ir, interner) = run(
            "class C\n{\npublic:\n    inline uint32 Run(uint32 a)\n    {\n\
             \x20       inline uint32 Go(uint32 b)\n        {\n\
             \x20           inline uint32 Do(uint32 c)\n            {\n\
             \x20               return c + m;\n            }\n\n\
             \x20           return Do(b) + m;\n        }\n\n\
             \x20       return Go(a);\n    }\n\n    uint32 m;\n}\n\nexport C;\n",
        );
        let class = kids(&ir, ir.root)[0];
        let Exp::Class { body, .. } = &ir.arena[class].exp else { panic!() };
        let items = kids(&ir, *body);
        let names: Vec<String> = items.iter().map(|&i| decl_name(&ir, &interner, i)).collect();
        assert_eq!(
            names,
            ["Public", "Run@Go@Do", "Run@Go", "Run", "m"]
                .map(|s| if s == "Public" { "Public".to_owned() } else { s.to_owned() }),
        );

        // Every moved declaration re-rooted to the class's qualifier and its
        // parameter scope to ["C", <mangled>] (`newScope`, Function.hs:48-50).
        for (i, expected_param_scope) in [(1, "Run@Go@Do"), (2, "Run@Go")] {
            let Exp::Function { scope, params, .. } = &ir.arena[items[i]].exp else {
                panic!()
            };
            assert_eq!(scope_strs(&interner, scope), ["C"]);
            let Exp::FuncParam { scope, .. } = &ir.arena[kids(&ir, *params)[0]].exp else {
                panic!()
            };
            assert_eq!(scope_strs(&interner, scope), ["C", expected_param_scope]);
        }

        // `fixScopedName` fired on *chain nodes* at two successive arms: the
        // `c` reference's qualifier is now C::Run@Go@Do, the call in Run@Go
        // names C::Run@Go@Do, its argument C::Run@Go::b.
        let Exp::Function { body: do_body, .. } = &ir.arena[items[1]].exp else { panic!() };
        let Exp::Seq(stmts) = &ir.arena[*do_body].exp else { panic!() };
        let Exp::Return(Some(add)) = &ir.arena[stmts[0]].exp else { panic!() };
        let (c_ref, m_ref) = {
            let ops = kids(&ir, *add);
            (ops[0], ops[1])
        };
        assert_eq!(ref_path(&ir, &interner, c_ref), "C::Run@Go@Do::c");
        // The reference prints `m` bare (`ScopedNameF … Nothing "m"`); our
        // sorted lowering pre-qualifies it to C::m at parse time (divergence
        // #22's sorted side), and this pass touches neither spelling. The
        // load-bearing half of the oracle's answer is that the class arm
        // selected nothing, so `memberAccess` never ran: never `$this.m`.
        assert_eq!(ref_path(&ir, &interner, m_ref), "C::m");
        assert!(
            matches!(ir.arena[m_ref].exp, Exp::NamedValue(_)),
            "a NamedValue, not a MemberAccess off $this"
        );

        let Exp::Function { body: go_body, .. } = &ir.arena[items[2]].exp else { panic!() };
        let Exp::Seq(stmts) = &ir.arena[*go_body].exp else { panic!() };
        let Exp::Return(Some(add)) = &ir.arena[stmts[0]].exp else { panic!() };
        let call = kids(&ir, *add)[0];
        let Exp::FunctionCall { function, args, .. } = &ir.arena[call].exp else { panic!() };
        let Exp::FunctionSpecifier { function: callee, .. } = &ir.arena[*function].exp else {
            panic!()
        };
        assert_eq!(ref_path(&ir, &interner, *callee), "C::Run@Go@Do");
        let Exp::NamedValue(b_ref) = &ir.arena[kids(&ir, *args)[0]].exp else { panic!() };
        assert_eq!(ref_path(&ir, &interner, *b_ref), "C::Run@Go::b");

        // Run's own call is fixed once. The reference prints its argument as
        // a *bare* `a` — the sharp witness that `body parent` excludes the
        // parent's own parameter list from the symbol map, which is why the
        // map must be per-node and not `Db::global_symbols`. Our sorted
        // lowering pre-qualifies the reference at parse time, so the
        // observable here is the weaker half: the pass leaves it untouched —
        // no `fixScopedName` chain fix (C::Run is no selected item) and no
        // re-qualification.
        let Exp::Function { body: run_body, .. } = &ir.arena[items[3]].exp else { panic!() };
        let Exp::Seq(stmts) = &ir.arena[*run_body].exp else { panic!() };
        let Exp::Return(Some(call)) = &ir.arena[stmts[0]].exp else { panic!() };
        let Exp::FunctionCall { function, args, .. } = &ir.arena[*call].exp else { panic!() };
        let Exp::FunctionSpecifier { function: callee, .. } = &ir.arena[*function].exp else {
            panic!()
        };
        assert_eq!(ref_path(&ir, &interner, *callee), "C::Run@Go");
        let a_ref = kids(&ir, *args)[0];
        assert_eq!(ref_path(&ir, &interner, a_ref), "C::Run::a");

        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// The class fixture every member-rewrite test below shares: a lambda in
    /// member `Run` touching the members. `--tree` at `tp=4` (excerpts, p_d.k
    /// and p_arms variants):
    ///
    /// ```text
    /// FunctionF {declScopeName = [], funcKind = LambdaFunction,
    ///   declName = "C@Run@?lambda0", …
    ///   AssignF (MemberAccessF (NamedValueF … ScopedNameF ["C@Run@?lambda0","$"]
    ///     Nothing "$this") "x") …
    /// ```
    ///
    /// — moved twice (`Run@?lambda0` at the `Run` arm, `C@Run@?lambda0` at the
    /// class arm), and only the class arm rewrites `x`, because only its map
    /// holds `DeclareClass` variables.
    fn member_fixture() -> (ModuleIr, ModuleIr, Interner) {
        run(
            "class C\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       const auto fn = []() -> uint32\n        {\n\
             \x20           Ping();\n            x += 2;\n            return x;\n\
             \x20       };\n\n        return fn();\n    }\n\n\
             \x20   inline void Ping()\n    {\n    }\n\n    uint32 x;\n}\n\nexport C;\n",
        )
    }

    fn the_moved_lambda(ir: &ModuleIr, interner: &Interner) -> ExpId {
        let found = find_all(ir, |e| {
            matches!(e, Exp::Function { kind: FunctionKind::LambdaFunction, .. })
        });
        assert_eq!(found.len(), 1);
        assert_eq!(decl_name(ir, interner, found[0]), "C@Run@?lambda0");
        found[0]
    }

    /// `memberAccess` arm 4 (`Function.hs:56`): a member value use becomes
    /// `$this.<member>`, at `tp=4`:
    ///
    /// ```text
    /// AssignF MemberAccessF NamedValueF QualifiedIdentifierF
    ///   ScopedNameF ["C@Run@?lambda0","$"] Nothing IdentifierF "$this"
    ///   IdentifierF "x" …
    /// ```
    ///
    /// **Donor E3** (`--notes`): the member leaf `IdentifierF "x"` wears
    /// `Src {offset = 254, 20:12}` — the class *declaration's* `declName`
    /// range — while the declaration itself still wears it too: one range,
    /// several wearers, so each use is a copy. The `$this` spine wears the use
    /// site's own note (`thisValue`, donated from `a`).
    #[test]
    fn a_member_value_use_becomes_this_member_access_with_a_copied_leaf() {
        let (before, ir, interner) = member_fixture();
        let func = the_moved_lambda(&ir, &interner);
        let Exp::Function { body, .. } = &ir.arena[func].exp else { panic!() };
        let accesses = {
            let mut out = Vec::new();
            fn go(ir: &ModuleIr, id: ExpId, out: &mut Vec<ExpId>) {
                if matches!(ir.arena[id].exp, Exp::MemberAccess { .. }) {
                    out.push(id);
                }
                let mut kids = Vec::new();
                ir.arena[id].exp.children(&mut kids);
                for k in kids {
                    go(ir, k, out);
                }
            }
            go(&ir, *body, &mut out);
            out
        };
        // The `$capture_param.x0` unpacking prologue is a MemberAccess too;
        // pick the ones whose member leaf is `x`.
        let x_accesses: Vec<ExpId> = accesses
            .iter()
            .copied()
            .filter(|&m| {
                let Exp::MemberAccess { member, .. } = &ir.arena[m].exp else { return false };
                text(&ir, &interner, *member) == "x"
            })
            .collect();
        assert_eq!(x_accesses.len(), 3, "the lvalue and rvalue of `x += 2`, and `return x`");

        // The class declaration of x, in `before` (it survives unchanged in
        // spirit; its name node id lives on in `after` too).
        let decls = find_all(&before, |e| {
            matches!(e, Exp::Variable { flags: DeclareFlag::DeclareClass, .. })
        });
        assert_eq!(decls.len(), 1);
        let Exp::Variable { name: decl_name_id, .. } = before.arena[decls[0]].exp else {
            panic!()
        };

        for &m in &x_accesses {
            let Exp::MemberAccess { base, member } = ir.arena[m].exp else { panic!() };
            // E3: a copy wearing the declaration's note, never the node itself.
            assert_ne!(member, decl_name_id, "two positions cannot share one arena node");
            assert_eq!(ir.arena[member].src.head, before.arena[decl_name_id].src.head);
            assert_eq!(ir.arena[member].src.written, Written::Donated);
            // thisValue: NamedValue($this) at the *renamed* use-site scope.
            let Exp::NamedValue(qi) = &ir.arena[base].exp else { panic!() };
            let Exp::QualifiedIdentifier(sn) = &ir.arena[*qi].exp else { panic!() };
            let Exp::ScopedName { scope, prefix, ident } = &ir.arena[*sn].exp else { panic!() };
            assert!(prefix.is_none());
            assert_eq!(text(&ir, &interner, *ident), "$this");
            assert_eq!(scope_strs(&interner, scope), ["C@Run@?lambda0", "$"]);
        }
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// `memberAccess` call arm (`Function.hs:58-59`): `Ping()` becomes a
    /// method call on `$this`. At `tp=4`:
    ///
    /// ```text
    /// FunctionSpecifierF (Just NamedValueF … Nothing IdentifierF "$this")
    ///   QualifiedIdentifierF ScopedNameF … (Just … "C") IdentifierF "Ping"
    /// ```
    ///
    /// **Donor F2**: the new `FunctionSpecifier` wears `b` — the callee name
    /// node, which it also *reuses* as its function child — and the written
    /// specifier is discarded. The reuse is the pinnable half: a port that
    /// deep-copies `b` goes red here (measured). The *donor* half is not:
    /// this lowering gives the written specifier and its callee spine the
    /// same `(range, kind)` — faithfully; the reference's `functionSpecifier`
    /// is `copyNote … f` too — so swapping the donate's donor for the old
    /// specifier is byte-identical and unpinnable until the ranges split,
    /// the same coincidence `lambdas.rs` records for `NAME`/`L`. The
    /// coincidence is asserted below so its ending announces itself.
    #[test]
    fn a_member_function_call_becomes_a_this_method_call() {
        let (before, ir, interner) = member_fixture();
        let func = the_moved_lambda(&ir, &interner);
        let mut spec = None;
        walk(&ir, |ir, id| {
            if let Exp::FunctionSpecifier { receiver: Some(_), function } = &ir.arena[id].exp
                && get_name(&ir.arena, *function).map(|s| interner.resolve(s).to_string())
                    == Some("Ping".to_owned())
                && spec.is_none()
            {
                spec = Some(id);
            }
        });
        let spec = spec.expect("Ping() was rewritten to a receiver call");
        let Exp::FunctionSpecifier { receiver: Some(rcv), function: b } = ir.arena[spec].exp
        else {
            panic!()
        };
        // The receiver is thisValue(b): NamedValue($this) at the renamed scope.
        let Exp::NamedValue(qi) = &ir.arena[rcv].exp else { panic!() };
        let Exp::QualifiedIdentifier(sn) = &ir.arena[*qi].exp else { panic!() };
        let Exp::ScopedName { scope, ident, .. } = &ir.arena[*sn].exp else { panic!() };
        assert_eq!(text(&ir, &interner, *ident), "$this");
        assert_eq!(scope_strs(&interner, scope), ["C@Run@?lambda0", "$"]);
        // F2: the specifier wears b's note, not the discarded one's, and is
        // Donated — b itself lives on as the function child, so the `Ping`
        // leaf under it is the *lowered* use-site node, still `Own`. A port
        // that deep-copies `b` instead of reusing it turns the leaf `Donated`
        // and appends it past the lowering's fence.
        assert_eq!(ir.arena[spec].src.head, ir.arena[b].src.head);
        assert_eq!(ir.arena[spec].src.written, Written::Donated);
        // The unpinnability witness (see the doc comment): the *written*
        // specifier this call discarded wore the same range as its callee
        // spine, so the donor swap plant stays green. If this assertion ever
        // fails, the ranges have split — strengthen the donor assertion above
        // to tell the two apart.
        let written_call = find_all(&before, |e| matches!(e, Exp::FunctionCall { .. }))
            .into_iter()
            .find(|&c| {
                let Exp::FunctionCall { function, .. } = &before.arena[c].exp else {
                    return false;
                };
                get_name(&before.arena, *function)
                    .map(|s| interner.resolve(s).to_string())
                    == Some("Ping".to_owned())
            })
            .expect("the written Ping call");
        let Exp::FunctionCall { function: written_spec, .. } = before.arena[written_call].exp
        else {
            panic!()
        };
        let Exp::FunctionSpecifier { function: written_b, .. } =
            before.arena[written_spec].exp
        else {
            panic!()
        };
        assert_eq!(
            before.arena[written_spec].src.head, before.arena[written_b].src.head,
            "the coincidence ended — the F2 donor swap is now pinnable; do it"
        );
        let mut leaf = b;
        loop {
            match &ir.arena[leaf].exp {
                Exp::QualifiedIdentifier(a) | Exp::NamedValue(a) => leaf = *a,
                Exp::ScopedName { ident, .. } => leaf = *ident,
                Exp::Identifier(_) => break,
                other => panic!("unexpected callee spine node {other:?}"),
            }
        }
        assert_eq!(ir.arena[leaf].src.written, Written::Own);
        assert!(
            (leaf.index() as u32) < before.lowered_len,
            "the written use-site leaf, not an appended copy"
        );
        let _ = func;
    }

    /// `memberAccess` arms 1-3 (`Function.hs:53-55`), one fixture each way:
    /// a plain-const member is **inlined**, an initializer-list one and a
    /// function-typed one are **not**. At `tp=4` (p_a1.k / p_a2v4.k):
    ///
    /// ```text
    /// ReturnF (Just BinaryF Add IntLiteralF {literalValue = 5} …)   -- K inlined
    /// … MemberAccessF NamedValueF … "$this" IdentifierF "A" …        -- A kept
    /// … MemberAccessF NamedValueF … "$this" IdentifierF "F" …        -- F kept
    /// ```
    ///
    /// **Donor E4** (`--notes`): the inlined `IntLiteralF 5` wears
    /// `Src {offset = 136, 9:20}` — the *use site's* note (the cata re-notes) —
    /// while the declaration keeps its own initializer at `offset = 240`
    /// untouched: the splice is a copy of a node that survives.
    #[test]
    fn a_const_member_initializer_is_inlined_but_lists_and_function_types_are_not() {
        let (before, ir, interner) = run(
            "class C\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       const auto fn = []() -> uint32\n        {\n\
             \x20           const auto h = F;\n            return K + K2 + A[0] + x;\n\
             \x20       };\n\n        return fn();\n    }\n\n\
             \x20   inline uint32 G()\n    {\n        return 3;\n    }\n\n\
             \x20   const () -> uint32 F = G;\n    const uint32 K = 5;\n\
             \x20   const uint32 K2 = 5 + 3;\n\
             \x20   const uint32[2] A = {1, 2};\n    uint32 x;\n}\n\nexport C;\n",
        );
        let func = {
            let found = find_all(&ir, |e| {
                matches!(e, Exp::Function { kind: FunctionKind::LambdaFunction, .. })
            });
            assert_eq!(found.len(), 1);
            found[0]
        };
        let Exp::Function { body, .. } = &ir.arena[func].exp else { panic!() };

        // Everything under the moved body, by shape.
        let mut inlined_k = None;
        let mut member_leaves = Vec::new();
        fn go(ir: &ModuleIr, id: ExpId, k: &mut Option<ExpId>, leaves: &mut Vec<String>, interner: &Interner) {
            match &ir.arena[id].exp {
                Exp::IntLiteral { value, .. } if format!("{value:?}").contains('5') && k.is_none() => {
                    *k = Some(id);
                }
                Exp::MemberAccess { member, .. } => {
                    if let Exp::Identifier(s) = &ir.arena[*member].exp {
                        leaves.push(interner.resolve(*s).to_string());
                    }
                }
                _ => {}
            }
            let mut kids = Vec::new();
            ir.arena[id].exp.children(&mut kids);
            for c in kids {
                go(ir, c, k, leaves, interner);
            }
        }
        go(&ir, *body, &mut inlined_k, &mut member_leaves, &interner);

        // Arm 3: K's value was spliced in; no `$this.K` access exists.
        let inlined = inlined_k.expect("K was inlined as its initializer literal");
        assert!(!member_leaves.iter().any(|l| l == "K"), "K must not be a member access");
        // Arms 1, 2, 4: F, A and x are member accesses off $this.
        for want in ["F", "A", "x"] {
            assert!(
                member_leaves.iter().any(|l| l == want),
                "{want} must be a $this member access, got {member_leaves:?}"
            );
        }

        // E4: the copy wears the use site's note; the declaration keeps its
        // own initializer node, value-equal, range apart.
        let k_decl = find_all(&before, |e| matches!(e, Exp::Variable { .. }))
            .into_iter()
            .find(|&v| decl_name(&before, &interner, v) == "K")
            .expect("K's declaration");
        let Exp::Variable { init: Some(k_init), .. } = before.arena[k_decl].exp else { panic!() };
        assert_ne!(inlined, k_init, "two positions cannot share one arena node");
        assert_eq!(ir.arena[inlined].exp, before.arena[k_init].exp, "same value");
        assert_ne!(
            ir.arena[inlined].src.head, before.arena[k_init].src.head,
            "the splice is re-noted at the use site (the cata's replacement), \
             not at the initializer"
        );
        walk(&ir, |ir, id| {
            if let Exp::Variable { init: Some(init), .. } = &ir.arena[id].exp
                && decl_name(ir, &interner, id) == "K"
            {
                assert_eq!(*init, k_init, "the declaration keeps its own initializer");
            }
        });

        // E4's copy-not-share half needs an initializer *with children*: the
        // spliced `unNote $ unfix v` shares `v`'s children in the reference,
        // and an arena must copy them. K2's `5 + 3` is a `Binary` whose two
        // literal children must be fresh `Donated` nodes, not the
        // declaration's own.
        let k2_decl = find_all(&before, |e| matches!(e, Exp::Variable { .. }))
            .into_iter()
            .find(|&v| decl_name(&before, &interner, v) == "K2")
            .expect("K2's declaration");
        let Exp::Variable { init: Some(k2_init), .. } = before.arena[k2_decl].exp else {
            panic!()
        };
        let decl_kids = kids(&before, k2_init);
        assert_eq!(decl_kids.len(), 2, "5 + 3");
        let mut inlined_k2 = None;
        fn find_binary(ir: &ModuleIr, id: ExpId, skip: ExpId, out: &mut Option<ExpId>) {
            if id != skip && matches!(ir.arena[id].exp, Exp::Binary(kg_const::BinOp::Add, ..))
                && kids(ir, id).len() == 2
                && kids(ir, id).iter().all(|&k| matches!(ir.arena[k].exp, Exp::IntLiteral { .. }))
                && out.is_none()
            {
                *out = Some(id);
            }
            let mut c = Vec::new();
            ir.arena[id].exp.children(&mut c);
            for k in c {
                find_binary(ir, k, skip, out);
            }
        }
        find_binary(&ir, *body, k2_init, &mut inlined_k2);
        let inlined_k2 = inlined_k2.expect("K2's initializer was spliced into the body");
        for (copy, original) in kids(&ir, inlined_k2).into_iter().zip(decl_kids) {
            assert_ne!(copy, original, "the splice's children are copies, not shares");
            assert_eq!(ir.arena[copy].src.head, before.arena[original].src.head);
            assert_eq!(ir.arena[copy].src.written, Written::Donated);
        }
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// `memberAccess` call arms 2 and 3 (`Function.hs:60-61`): a template
    /// member function and a `DeclareClass` variable called as a function both
    /// get the `$this` receiver. Verified against the oracle on the p_arms
    /// family (`FunctionSpecifierF (Just NamedValueF … "$this") …` for both).
    #[test]
    fn template_member_and_member_variable_calls_also_bind_this() {
        let (_, ir, interner) = run(
            "class C\n{\npublic:\n    inline uint32 Run()\n    {\n\
             \x20       const auto fn = []() -> uint32\n        {\n\
             \x20           Tmpl(1);\n            Held();\n            return 0;\n\
             \x20       };\n\n        return fn();\n    }\n\n\
             \x20   template <typename T>\n    inline void Tmpl(T t)\n    {\n    }\n\n\
             \x20   const auto Held = []() -> uint32 { return 4; };\n}\n\nexport C;\n",
        );
        for callee in ["Tmpl", "Held"] {
            let mut found = false;
            walk(&ir, |ir, id| {
                if let Exp::FunctionSpecifier { receiver: Some(rcv), function } =
                    &ir.arena[id].exp
                    && get_name(&ir.arena, *function)
                        .map(|s| interner.resolve(s).to_string())
                        == Some(callee.to_owned())
                {
                    let Exp::NamedValue(qi) = &ir.arena[*rcv].exp else { panic!() };
                    let Some(leaf) = get_name(&ir.arena, *qi) else { panic!() };
                    assert_eq!(interner.resolve(leaf).to_string(), "$this");
                    found = true;
                }
            });
            assert!(found, "{callee}() must have been rewritten to a $this method call");
        }
    }

    /// The `?lambdaN$capture` **alias does not move** — `functionsAndClasses`
    /// has no `AliasF` arm — and the moved function's capture-parameter type
    /// still names the alias at its *old* path while its scope field is
    /// re-rooted. At `tp=4` (f2.k):
    ///
    /// ```text
    /// … AliasF {declScopeName = ["C","Run","$"], …, declName = "?lambda0$capture"}
    /// … FuncParamF {declScopeName = ["C@Run@?lambda0"], paramType =
    ///     TypeIdentifierF ["C@Run@?lambda0"] ScopedNameF … (Just … "C" … "Run"
    ///     … "$") IdentifierF "?lambda0$capture", …}
    /// ```
    #[test]
    fn the_capture_alias_stays_behind_and_the_moved_type_names_the_old_path() {
        let (before, ir, interner) = member_fixture();
        // The alias is still inside Run's body, its scope untouched.
        let aliases = find_all(&ir, |e| matches!(e, Exp::Alias { .. }));
        assert_eq!(aliases.len(), 1);
        let Exp::Alias { scope, .. } = &ir.arena[aliases[0]].exp else { panic!() };
        assert_eq!(scope_strs(&interner, scope), ["C", "Run", "$"]);

        // The moved function's capture parameter: scope re-rooted, type spine
        // still spelling C::Run::$::?lambda0$capture.
        let func = the_moved_lambda(&ir, &interner);
        let Exp::Function { params, .. } = &ir.arena[func].exp else { panic!() };
        let p0 = kids(&ir, *params)[0];
        let Exp::FuncParam { scope, ty, .. } = &ir.arena[p0].exp else { panic!() };
        assert_eq!(scope_strs(&interner, scope), ["C@Run@?lambda0"]);
        let Exp::TypeIdentifier(ty_scope, sn) = &ir.arena[*ty].exp else { panic!() };
        assert_eq!(scope_strs(&interner, ty_scope), ["C@Run@?lambda0"]);
        assert_eq!(ref_path(&ir, &interner, *sn), "C::Run::$::?lambda0$capture");
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// `move t@Template{} _ = t` (`Function.hs:88`): a template's interior is
    /// carried verbatim — `--template-passes 3` and `4` print byte-identical
    /// trees for a local function inside a template function. The revert
    /// returns the original id, so the assertion is that nothing was
    /// allocated at all.
    #[test]
    fn a_template_defers_wholesale() {
        let (before, after, _) = run(
            "template <auto B>\ninline uint32 f(uint8 p)\n{\n\
             \x20   inline uint8 g(uint8 q)\n    {\n        return q;\n    }\n\n\
             \x20   return g(p);\n}\n",
        );
        assert_eq!(before.root, after.root, "nothing changed, so nothing was rebuilt");
        assert_eq!(before.arena.len(), after.arena.len(), "the revert allocates nothing");
    }

    /// `move t@Function{} _ | isAbbreviatedTemplate t = t` (`Function.hs:89-90`)
    /// — and what the defer does *not* do. At `tp=4` (p_abbrev2.k) both the
    /// `Template` twin and the abbreviated leftover are **selected, moved and
    /// mangled** `outer@inner`, while `deep` — a local function inside the
    /// abbreviated body — stays inside both copies: the defer protects the
    /// interior, not the node. The oracle also confirms `TemplateParamF`
    /// keeps its scope (`["outer","$"]`) while the `TypeParam` inside the
    /// moved function is renamed — `modifyDeclScopeName` has no
    /// `TemplateParamF` arm.
    #[test]
    fn an_abbreviated_function_moves_whole_but_keeps_its_interior_unprocessed() {
        let (before, ir, interner) = run(
            "inline void outer()\n{\n    inline void inner(auto a)\n    {\n\
             \x20       inline void deep()\n        {\n        }\n\n        deep();\n    }\n\n\
             \x20   inner(1);\n}\n",
        );
        let top: Vec<String> =
            kids(&ir, ir.root).iter().map(|&i| decl_name(&ir, &interner, i)).collect();
        assert_eq!(top, ["template:outer@inner", "outer@inner", "outer"]);

        // `deep` still sits inside each copy's body — never at the top.
        let deeps = find_all(&ir, |e| matches!(e, Exp::Function { .. }))
            .into_iter()
            .filter(|&f| decl_name(&ir, &interner, f) == "deep")
            .count();
        assert_eq!(deeps, 2, "one per carried interior");

        // The template parameter's scope is untouched; the leftover's param
        // scope is renamed.
        let Exp::Template { params, .. } = &ir.arena[kids(&ir, ir.root)[0]].exp else {
            panic!()
        };
        let Exp::TemplateParam { scope, .. } = &ir.arena[params[0]].exp else { panic!() };
        assert_eq!(scope_strs(&interner, scope), ["outer", "$"]);
        let Exp::Function { params: lp, .. } = &ir.arena[kids(&ir, ir.root)[1]].exp else {
            panic!()
        };
        let Exp::FuncParam { scope, .. } = &ir.arena[kids(&ir, *lp)[0]].exp else { panic!() };
        assert_eq!(scope_strs(&interner, scope), ["outer@inner"]);
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// `move t@StaticIf{} _ = t` (`Function.hs:91`). **No oracle-graded
    /// whole-corpus gate can see this arm on our side of the seam**: on any
    /// fixture the oracle can read, `staticControl` (frontend pass 6,
    /// unported) has already folded a constant `static if` away — p_sif.k's
    /// `static if (true)` prints `main@D` moved at `tp=4` — and the one corpus
    /// file whose condition survives folding (`test/logic/memory_test_cases_3.k`,
    /// the mutant sweep's single hit for this clause) is a whole-program
    /// shape. On this port's IR every written `static if` reaches the pass
    /// intact, so this unit test is the arm's instrument.
    #[test]
    fn a_static_if_defers_on_our_ir() {
        let (before, after, _) = run(
            "inline void main()\n{\n    static if (true)\n    {\n\
             \x20       inline void D(uint8 x)\n        {\n        }\n\n        D(1);\n    }\n}\n",
        );
        assert_eq!(before.root, after.root);
        assert_eq!(before.arena.len(), after.arena.len(), "the revert allocates nothing");
    }

    /// `move t@StaticFor{} _ = t` (`Function.hs:92`). **Unreachable through
    /// every oracle-readable corpus file** (the survey's mutation sweep: zero
    /// files change with the clause removed — a surviving `StaticFor` sits
    /// only under a `Template`, whose own defer shadows this one, and
    /// `staticControl` rewrites every other one to `UnrolledForF`, p_sfor.k
    /// measured). On this port's IR `staticControl` does not exist, so every
    /// written `static for` exercises this defer; this unit test is its only
    /// pin.
    #[test]
    fn a_static_for_defers_on_our_ir() {
        let (before, after, _) = run(
            "inline void main(uint8 p)\n{\n    static for (const auto i : p)\n    {\n\
             \x20       inline void D(uint8 x)\n        {\n        }\n\n        D(1);\n    }\n}\n",
        );
        assert_eq!(before.root, after.root);
        assert_eq!(before.arena.len(), after.arena.len(), "the revert allocates nothing");
    }

    /// `move t@Class{} _ | isLocalScope t = t` (`Function.hs:93-94`) — the
    /// local class is *moved and mangled* by the enclosing arm while its
    /// interior is carried verbatim. At `tp=4` (lc.k):
    ///
    /// ```text
    /// ClassF {declScopeName = [], declName = "main@Bar", body = … FunctionF
    ///   {declScopeName = ["main@Bar"], declName = "Do", body = SeqF
    ///     [AliasF …, FunctionF {declScopeName = ["main@Bar","Do","$"],
    ///       funcKind = LambdaFunction, declName = "?lambda0", …}, …]}}
    /// ```
    ///
    /// — the lambda function `desugarLambdas` left in `Do`'s body is **still
    /// there**: `Do`'s own arm did fire, and the class defer discarded it.
    #[test]
    fn a_local_class_moves_whole_but_keeps_its_lambda_functions_inside() {
        let (before, ir, interner) = run(
            "inline void main()\n{\n    class Bar\n    {\n    public:\n\
             \x20       inline void Do()\n        {\n\
             \x20           const auto fn = [](){};\n\n            fn();\n        }\n    }\n\n\
             \x20   static Bar bar;\n\n    bar.Do();\n}\n",
        );
        let top: Vec<String> =
            kids(&ir, ir.root).iter().map(|&i| decl_name(&ir, &interner, i)).collect();
        assert_eq!(top, ["main@Bar", "main"]);
        let class = kids(&ir, ir.root)[0];
        let Exp::Class { scope, body, .. } = &ir.arena[class].exp else { panic!() };
        assert!(scope.is_empty(), "re-rooted to the module");

        // Do still holds its lambda function — the class arm never ran on Bar.
        let mut lambda_fn_scopes = Vec::new();
        let mut sub = vec![*body];
        while let Some(id) = sub.pop() {
            if let Exp::Function { kind: FunctionKind::LambdaFunction, scope, .. } =
                &ir.arena[id].exp
            {
                lambda_fn_scopes.push(scope_strs(&interner, scope));
            }
            let mut kids2 = Vec::new();
            ir.arena[id].exp.children(&mut kids2);
            sub.append(&mut kids2);
        }
        assert_eq!(lambda_fn_scopes, [["main@Bar", "Do", "$"]]);
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// The `NestedScope` arm (`Function.hs:99`) — unreachable through every
    /// oracle-readable corpus file (its only corpus home is
    /// `test/syntax/local-functions.k`, which the oracle rejects whole), and
    /// verified against the oracle as a scratch extract. At `tp=4` (p_ns.k):
    ///
    /// ```text
    /// SeqF [FunctionF {declScopeName = [], declName = "main@$quasi0@Do", …}
    ///      ,FunctionF {… "main", body = SeqF [NestedScopeF {… "$quasi0", body =
    ///        SeqF [FunctionCallF {… ScopedNameF … Nothing "main@$quasi0@Do" …}]}]}]
    /// ```
    ///
    /// — mangled through *two* arms (the block's, then `main`'s), and the call
    /// is re-pointed with **no** prefix: the module parent's qualifier is
    /// empty, and `scopedQualifier _ _ [] = Nothing`.
    #[test]
    fn a_bare_block_hoists_through_the_nested_scope_arm() {
        let (before, ir, interner) = run(
            "inline void main()\n{\n    {\n        inline void Do(uint8 x)\n\
             \x20       {\n        }\n\n        Do(1);\n    }\n}\n",
        );
        let top: Vec<String> =
            kids(&ir, ir.root).iter().map(|&i| decl_name(&ir, &interner, i)).collect();
        assert_eq!(top, ["main@$quasi0@Do", "main"]);
        // The call inside the surviving block names it bare.
        let mut callee = None;
        walk(&ir, |ir, id| {
            if let Exp::FunctionSpecifier { receiver: None, function } = &ir.arena[id].exp
                && callee.is_none()
            {
                callee = Some(*function);
            }
        });
        assert_eq!(ref_path(&ir, &interner, callee.unwrap()), "main@$quasi0@Do");
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// The `DoWhile` lift (`Function.hs:104-105`) — unreachable through every
    /// oracle-readable corpus file (see the module comment), verified as a
    /// scratch extract. At `tp=4` (p_dw.k) the moved function reaches module
    /// scope as `main@$quasi0@Do` and the loop keeps the surviving
    /// `NestedScope` as its body:
    ///
    /// ```text
    /// … body = SeqF [DoWhileF {…, body = NestedScopeF {… "$quasi0", …}}] …
    /// ```
    #[test]
    fn a_do_while_lifts_moved_declarations_out_of_its_body() {
        let (before, ir, interner) = run(
            "inline void main()\n{\n    do\n    {\n        inline void Do(uint8 x)\n\
             \x20       {\n        }\n\n        Do(1);\n    }\n    while(false);\n}\n",
        );
        let top: Vec<String> =
            kids(&ir, ir.root).iter().map(|&i| decl_name(&ir, &interner, i)).collect();
        assert_eq!(top, ["main@$quasi0@Do", "main"]);
        let Exp::Function { body, .. } = &ir.arena[kids(&ir, ir.root)[1]].exp else { panic!() };
        let stmts = kids(&ir, *body);
        assert_eq!(stmts.len(), 1);
        let Exp::DoWhile { body: loop_body, .. } = &ir.arena[stmts[0]].exp else {
            panic!("the loop survives, got {:?}", ir.arena[stmts[0]].exp)
        };
        assert!(
            matches!(ir.arena[*loop_body].exp, Exp::NestedScope { .. }),
            "the lift keeps `last c` — the surviving scope — as the loop body"
        );
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// The two-branch `If` lift (`Function.hs:110-111`) — same reachability
    /// story as the `DoWhile` one. At `tp=4` (p_if.k) both branches' functions
    /// land before `main`, then-branch first (`init b ++ init c ++ …`).
    #[test]
    fn an_if_lifts_both_branches_moved_declarations_in_order() {
        let (before, ir, interner) = run(
            "inline void main(uint8 p)\n{\n    if (p > 1)\n    {\n\
             \x20       inline void T(uint8 x)\n        {\n        }\n\n        T(1);\n    }\n\
             \x20   else\n    {\n        inline void E(uint8 x)\n        {\n        }\n\n\
             \x20       E(2);\n    }\n}\n",
        );
        let top: Vec<String> =
            kids(&ir, ir.root).iter().map(|&i| decl_name(&ir, &interner, i)).collect();
        assert_eq!(top, ["main@$quasi0@T", "main@$quasi0@E", "main"]);
        let Exp::Function { body, .. } = &ir.arena[kids(&ir, ir.root)[2]].exp else { panic!() };
        let stmts = kids(&ir, *body);
        assert_eq!(stmts.len(), 1);
        let Exp::If { then_branch, else_branch, .. } = &ir.arena[stmts[0]].exp else {
            panic!("the if survives")
        };
        assert!(matches!(ir.arena[*then_branch].exp, Exp::NestedScope { .. }));
        assert!(matches!(ir.arena[else_branch.unwrap()].exp, Exp::NestedScope { .. }));
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// The `RangeFor` lift (`Function.hs:107-108`) — same reachability story.
    /// At `tp=4` (p_rf.k) the moved function is `main@$quasi1@R` — the loop
    /// *body* block is `$quasi1`; `$quasi0` is the loop variable's scope and
    /// contributes no mangle segment.
    #[test]
    fn a_range_for_lifts_moved_declarations_out_of_its_body() {
        let (before, ir, interner) = run(
            "inline void main(uint8 p)\n{\n    for (const auto i : p)\n    {\n\
             \x20       inline void R(uint8 x)\n        {\n        }\n\n        R(1);\n    }\n}\n",
        );
        let top: Vec<String> =
            kids(&ir, ir.root).iter().map(|&i| decl_name(&ir, &interner, i)).collect();
        assert_eq!(top, ["main@$quasi1@R", "main"]);
        let Exp::Function { body, .. } = &ir.arena[kids(&ir, ir.root)[1]].exp else { panic!() };
        let stmts = kids(&ir, *body);
        assert_eq!(stmts.len(), 1);
        assert!(matches!(ir.arena[stmts[0]].exp, Exp::RangeFor { .. }));
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// A mangled-name collision is kept as **two** declarations — the ledger
    /// is a multiset. The oracle prints only one `main@$quasi0@D` for this
    /// fixture, but that is `deduplicateInstances` (`Template.hs:144-155`)
    /// running *after* the sub-pass window, not `moveFunctions`: both
    /// branches' blocks are `$quasi0`, so both mangle identically, and the
    /// pass itself emits both. Corpus-side the same shape is forced 35 times
    /// (14 of them non-`$wrapper$`, in 5 files).
    #[test]
    fn a_mangled_name_collision_keeps_both_declarations() {
        let (before, ir, interner) = run(
            "inline void main(uint8 p)\n{\n    if (p > 1)\n    {\n\
             \x20       inline void D(uint8 x)\n        {\n        }\n\n        D(1);\n    }\n\
             \x20   else\n    {\n        inline void D(uint8 x)\n        {\n        }\n\n\
             \x20       D(2);\n    }\n}\n",
        );
        let top: Vec<String> =
            kids(&ir, ir.root).iter().map(|&i| decl_name(&ir, &interner, i)).collect();
        assert_eq!(top, ["main@$quasi0@D", "main@$quasi0@D", "main"]);
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }

    /// Both halves of [`moved_declarations_reach_fixpoint`] go red on their
    /// own, with no unit assertion in front: the fixpoint half rejects the
    /// pass's own *input* (which still holds the movable), and the
    /// conservation half rejects a pairing of two different programs. Without
    /// this, every plant that reddens the invariant also reddens an earlier
    /// assert in the same test, and the invariant's own bite is unproven.
    #[test]
    fn the_invariant_bites_on_both_halves() {
        let (before, after, interner) = run(
            "inline uint32 Run(uint32 x)\n{\n    inline uint32 Go(uint32 y)\n    {\n\
             \x20       return y;\n    }\n\n    return Go(x);\n}\n",
        );
        let err = moved_declarations_reach_fixpoint(&before, &before, &interner)
            .expect_err("the unmoved input is not at fixpoint");
        assert!(err.contains("fixpoint"), "{err}");

        // The second program must share the first's interner — symbols do not
        // compare across interners.
        let parse = kg_syntax::parse_file("inline void solo()\n{\n}\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let other = lower_sorted(&file, FileId(0), &interner, &NoImports);
        let err = moved_declarations_reach_fixpoint(&other, &after, &interner)
            .expect_err("two different programs cannot conserve declarations");
        assert!(err.contains("conserved"), "{err}");
    }

    /// A file with no movable declarations allocates nothing: the `null
    /// selected` guard fires before the symbol map and both catas are built —
    /// the eager port of `Function.hs:30`'s laziness, and the property
    /// `cargo xtask bench` leans on (219 of 357 oracle-readable corpus files
    /// are no-ops for this pass).
    #[test]
    fn a_file_with_no_movable_declarations_is_not_rebuilt() {
        let (before, after, _) = run(
            "inline uint32 f(uint32 a)\n{\n    uint32 r = a + 1;\n    return r;\n}\n\n\
             class C\n{\npublic:\n    inline uint32 Run(uint32 a)\n    {\n\
             \x20       return a;\n    }\n\n    uint32 m;\n}\n",
        );
        assert_eq!(before.root, after.root);
        assert_eq!(before.arena.len(), after.arena.len(), "no node was appended");
    }

    /// The moved declaration's name leaf is a *replacement* — it stands where
    /// the written name stood, wears its note, and keeps `Own`; the rebuilt
    /// parent (`remaining`) wears the node's note but is `Donated`, because
    /// the driver's `Seq` replacement is the one `Own` claimant of that key.
    #[test]
    fn the_mangled_name_keeps_own_and_the_remaining_parent_is_donated() {
        let (before, ir, interner) = run(
            "inline uint32 Run(uint32 x)\n{\n    inline uint32 Go(uint32 y)\n    {\n\
             \x20       return y;\n    }\n\n    return Go(x);\n}\n",
        );
        let top = kids(&ir, ir.root);
        assert_eq!(decl_name(&ir, &interner, top[0]), "Run@Go");
        assert_eq!(decl_name(&ir, &interner, top[1]), "Run");

        // The mangled leaf wears the written `Go`'s note, Own.
        let Exp::Function { name, .. } = &ir.arena[top[0]].exp else { panic!() };
        let written_go = find_all(&before, |e| matches!(e, Exp::Function { .. }))
            .into_iter()
            .find(|&f| decl_name(&before, &interner, f) == "Go")
            .expect("the written Go");
        let Exp::Function { name: go_name, .. } = before.arena[written_go].exp else { panic!() };
        assert_eq!(ir.arena[*name].src.head, before.arena[go_name].src.head);
        assert_eq!(ir.arena[*name].src.written, Written::Own);

        // The remaining Run wears the original node's note, Donated.
        let written_run = kids(&before, before.root)[0];
        assert_eq!(ir.arena[top[1]].src.head, before.arena[written_run].src.head);
        assert_eq!(ir.arena[top[1]].src.written, Written::Donated);
        moved_declarations_reach_fixpoint(&before, &ir, &interner).unwrap();
    }
}
