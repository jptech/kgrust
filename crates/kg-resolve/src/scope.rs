//! Owner-topic: **scopes** — the scope kinds, the scope stack with its index
//! seeding and pop actions, redefinition, shadowing, and name lookup.
//!
//! Spec: `docs/spec/resolve.md` §4 (§4.1 the scope record, §4.2 the complete set
//! of scope kinds, §4.3 redefinition, §4.4 the `shadow` warning), §2.7
//! (`findSymbol`), §3.3 (`lookupInScope`), §10 (the keying table).
//! Reference: `SymbolTable.hs:51-66,183-278`, `Parser.hs:119-153`,
//! `TypeCheck.hs:110-121`.

use kg_base::{FileRange, FxHashMap, Interner, Symbol, SymbolPath, TextRange, TextSize};
use kg_diag::{Diagnostic, WarningKind};

use crate::def::{DeclareFlag, DefId, DefKind, ItemId};
use crate::lower::{Item, ItemTree, Program, names};
use crate::module::{ImportId, ModuleGraph, ModuleSource};

/// Index of a scope within one file's [`crate::lower::ItemTree`]. Scope 0 is the
/// file scope, whose qualifier is empty (`emptySymbolTable`,
/// `SymbolTable.hs:94`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ScopeId(pub u32);

impl ScopeId {
    /// `emptySymbolTable = SymbolTable [Scope mempty mempty 0 0 0] …`
    /// (`SymbolTable.hs:94`).
    pub const FILE: ScopeId = ScopeId(0);

    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// How a scope was pushed.
///
/// The reference has **four** primitive combinators built on `parseScoped`
/// (`Parser.hs:119-153`): `unscopedWith`, `scopedWith`, `quasiScopedWith` and
/// `localScoped`. `docs/spec/resolve.md` §11.3 sketches six variants while its
/// own comment says "five kinds"; the six-variant form is kept here because
/// `Module` and `Function` are worth distinguishing at the call sites even though
/// both are `scopedWith` underneath, and because the spec's count is wrong either
/// way. Each variant documents which combinator it is.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ScopeKind {
    /// `scoped ident` for a `module A.B { … }` body (`Parser.hs:189`).
    /// Qualifier gains the single mangled namespace component `@A@B`.
    Module,
    /// `unscopedWith syms` (`Parser.hs:119`) — **same qualifier as the parent**,
    /// indices seeded to `0`, popped with `tail`.
    ///
    /// Used by: `using T = …` around the RHS, each enum constant (so a later
    /// constant can reference an earlier one), each template parameter, and the
    /// two wrappers around a function definition.
    Unscoped,
    /// `scopedWith syms ident` (`Parser.hs:123`) — qualifier gains the
    /// identifier. Used by `enum E`, `class C`, a function definition, and a
    /// lambda (`?lambdaN`).
    Named,
    /// `quasiScopedWith` (`Parser.hs:136-143`) — qualifier gains
    /// `$quasi<parent.scopeIndex>`; `lambdaIndex` inherits from the parent and
    /// `scopeIndex` is the parent's `+ 1`. Used by `struct`, `union`, a function
    /// *type*'s parameter list, and a `static if` branch block.
    Quasi,
    /// `localScoped` (`Parser.hs:147-153`) — like [`ScopeKind::Quasi`], **and on
    /// pop it rewrites the parent's own qualifier** to
    /// `parent ++ ["$quasi<parent.scopeIndex + 1>"]`.
    ///
    /// This is the key non-obvious rule: after a block, loop or switch case,
    /// *subsequent declarations in the enclosing scope live under a deeper
    /// qualifier*. It is what makes
    /// `inline void main() { for (const auto x : 10) {} bool x; }` warning-free
    /// (`test/syntax/shadow-warning.k:77-86`, `warning:0`) — the loop's `x` is
    /// keyed `[main,$,$quasi0,x]` and the later `x` is `[main,$,$quasi1,x]`.
    ///
    /// Used by `{ … }` statement blocks, `for`, `static for`, and `case`/`default`
    /// bodies.
    Local,
    /// `functionScopedWith syms = scopedWith syms (Identifier "$")`
    /// (`Parser.hs:127`) — a function *body*. The `$` component is what
    /// `isLocalScope` tests (`Desugar.hs:176-180`).
    Function,
}

impl ScopeKind {
    /// Whether the qualifier gains a component when this scope is pushed.
    pub fn extends_qualifier(self) -> bool {
        !matches!(self, ScopeKind::Unscoped)
    }
}

/// One scope — `data Scope` (`SymbolTable.hs:58-66`).
#[derive(Clone, Debug)]
pub struct ScopeData {
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    /// `scopeQualifier` — already includes `$quasiN`, `$` and `?lambdaN`
    /// components. This *is* the key prefix declarations are filed under
    /// (`docs/spec/resolve.md` §10).
    ///
    /// **It is not constant for the life of the scope.** Every `localScoped` child
    /// that closes appends another `$quasiN` to it (`Parser.hs:153`), so the value
    /// stored in a finished [`crate::lower::ItemTree`] is the one in effect at the
    /// scope's *end*. A declaration's key is captured when the declaration is
    /// reached, which is why [`crate::lower::Item::qualified_name`] is stored per
    /// item and not recomputed from here. Anything resolving a name at a given
    /// source offset must re-walk with a live [`ScopeStack`] rather than read this;
    /// [`ScopeData::entry_qualifier`] is the only stable half.
    pub qualifier: SymbolPath,
    /// `qualifier` as it was when the scope was pushed — before any `localScoped`
    /// child deepened it. Not in the reference, which discards a `Scope` on pop
    /// and so never has to distinguish the two.
    pub entry_qualifier: SymbolPath,
    /// `symbolsInScope`. Insertion-ordered so that diagnostics are deterministic;
    /// the reference uses a `HashMap` and never iterates it for output.
    pub names: FxHashMap<Symbol, ItemId>,
    /// `lambdaIndex` — feeds `?lambdaN` (`Parser.hs:434`,
    /// `SymbolTable.hs:216-225`).
    pub lambda_index: u32,
    /// `scopeIndex` — feeds `$quasiN` (`Parser.hs:131-134`).
    pub scope_index: u32,
    /// `exportIndex` — feeds `$exportN` (`SymbolTable.hs:213`).
    pub export_index: u32,
    /// The syntax node this scope was pushed for, when the lowering said
    /// ([`ScopeStack::push_at`]); `None` for the file scope. What lets a
    /// finished tree answer "which scope was live at this offset"
    /// ([`scope_at`]).
    pub range: Option<TextRange>,
    /// Every `localScoped` child's close, in order: the offset it closed at
    /// and this scope's qualifier from then on (`Parser.hs:153-155`). With
    /// [`ScopeData::entry_qualifier`] this replays the qualifier a
    /// declaration or a use at any offset was keyed under ([`qualifier_at`]),
    /// which [`ScopeData::qualifier`] — the end state — cannot.
    pub deepened: Vec<(TextSize, SymbolPath)>,
}

/// The innermost scope whose range contains `offset` — the one live there.
/// The file scope, which has no range, is the answer when no other does.
pub fn scope_at(scopes: &[ScopeData], offset: TextSize) -> ScopeId {
    let mut best = ScopeId::FILE;
    let mut best_len: Option<TextSize> = None;
    for (i, s) in scopes.iter().enumerate() {
        let Some(r) = s.range else { continue };
        if r.start() > offset || r.end() < offset {
            continue;
        }
        if best_len.is_none_or(|l| r.len() <= l) {
            best = ScopeId(i as u32);
            best_len = Some(r.len());
        }
    }
    best
}

/// `scope`'s qualifier as it stood at `offset`: its entry qualifier, deepened
/// by every local child that had closed by then (`Parser.hs:153-155`).
pub fn qualifier_at(scopes: &[ScopeData], scope: ScopeId, offset: TextSize) -> SymbolPath {
    let s = &scopes[scope.index()];
    let mut q = s.entry_qualifier.clone();
    for (at, deeper) in &s.deepened {
        if *at <= offset {
            q = deeper.clone();
        }
    }
    q
}

/// The scope stack — `data SymbolTable` (`SymbolTable.hs:51-56`).
///
/// The push/pop protocol is `parseScoped` (`SymbolTable.hs:183-192`) plus the
/// four combinators' index seeding and pop actions
/// (`docs/spec/resolve.md` §4.2 table). Getting the *pop* action wrong is
/// invisible until a shadow-warning oracle disagrees.
#[derive(Clone, Debug)]
pub struct ScopeStack {
    scopes: Vec<ScopeData>,
    stack: Vec<ScopeId>,
}

/// `emptySymbolTable` (`SymbolTable.hs:94`) already has one scope on the stack.
/// Deriving `Default` would give a stack with *none*, and `head . scopes` — which
/// every operation here performs — is partial on the reference side too.
impl Default for ScopeStack {
    fn default() -> ScopeStack {
        ScopeStack::new()
    }
}

impl ScopeStack {
    /// One global scope with an empty qualifier (`SymbolTable.hs:94`).
    pub fn new() -> ScopeStack {
        let file = ScopeData {
            parent: None,
            kind: ScopeKind::Unscoped,
            qualifier: SymbolPath::empty(),
            entry_qualifier: SymbolPath::empty(),
            names: FxHashMap::default(),
            lambda_index: 0,
            scope_index: 0,
            export_index: 0,
            range: None,
            deepened: Vec::new(),
        };
        ScopeStack { scopes: vec![file], stack: vec![ScopeId::FILE] }
    }

    pub fn current(&self) -> ScopeId {
        *self.stack.last().expect("the file scope is never popped")
    }

    pub fn get(&self, id: ScopeId) -> &ScopeData {
        &self.scopes[id.index()]
    }

    pub fn get_mut(&mut self, id: ScopeId) -> &mut ScopeData {
        &mut self.scopes[id.index()]
    }

    /// Innermost → outermost, as `findInScopes` walks it
    /// (`SymbolTable.hs:256-258`).
    pub fn chain(&self) -> impl Iterator<Item = ScopeId> + '_ {
        self.stack.iter().rev().copied()
    }

    pub fn scopes(&self) -> &[ScopeData] {
        &self.scopes
    }

    /// `parseScoped` (`SymbolTable.hs:183-192`), specialised by `kind`.
    ///
    /// `scopeQualifier = parentQualifier ++ concatMap getQualifiedName s`, where
    /// `s` is the `Maybe ExpSrc` identifier each combinator supplies:
    ///
    /// | kind | `s` | seeding |
    /// |---|---|---|
    /// | [`ScopeKind::Unscoped`] | `Nothing` | `0`, `0` |
    /// | [`ScopeKind::Module`] / [`ScopeKind::Named`] | `name` | `0`, `0` |
    /// | [`ScopeKind::Function`] | `Identifier "$"` (`Parser.hs:127`) | `0`, `0` |
    /// | [`ScopeKind::Quasi`] / [`ScopeKind::Local`] | `Identifier "$quasi<parent.scopeIndex>"` (`Parser.hs:131-134`) | `parent.lambdaIndex`, `parent.scopeIndex + 1` |
    ///
    /// `name` is *ignored* for `Unscoped`, `Function`, `Quasi` and `Local`, all of
    /// which name themselves. A `Module`/`Named` push with `name == None` — which
    /// only a CST containing an error node can produce — leaves the qualifier
    /// alone rather than inventing a component; the scope is still pushed, so the
    /// pop protocol stays balanced.
    ///
    /// The `syms` pre-seeded by `unscopedWith` / `scopedWith` / `quasiScopedWith`
    /// are inserted afterwards with [`ScopeStack::seed`]: they bypass
    /// `parseSymbol`, and therefore the redefinition check and the module's flat
    /// symbol map, which is exactly what `parseScoped` does with them.
    pub fn push(&mut self, interner: &Interner, kind: ScopeKind, name: Option<Symbol>) -> ScopeId {
        let parent_id = self.current();
        let parent = &self.scopes[parent_id.index()];

        let component = match kind {
            ScopeKind::Unscoped => None,
            ScopeKind::Module | ScopeKind::Named => name,
            ScopeKind::Function => Some(interner.intern(names::FUNCTION_SCOPE)),
            ScopeKind::Quasi | ScopeKind::Local => Some(quasi_name(interner, parent.scope_index)),
        };
        let qualifier = match component {
            Some(c) => parent.qualifier.child(c),
            None => parent.qualifier.clone(),
        };
        let (lambda_index, scope_index) = match kind {
            ScopeKind::Quasi | ScopeKind::Local => (parent.lambda_index, parent.scope_index + 1),
            _ => (0, 0),
        };

        let id = ScopeId(self.scopes.len() as u32);
        self.scopes.push(ScopeData {
            parent: Some(parent_id),
            kind,
            entry_qualifier: qualifier.clone(),
            qualifier,
            names: FxHashMap::default(),
            lambda_index,
            scope_index,
            // `Scope … 0` — every new scope starts numbering its `export T;`
            // declarations at zero (`SymbolTable.hs:189`).
            export_index: 0,
            range: None,
            deepened: Vec::new(),
        });
        self.stack.push(id);
        id
    }

    /// [`ScopeStack::push`], remembering the node the scope is for — see
    /// [`ScopeData::range`].
    pub fn push_at(
        &mut self,
        interner: &Interner,
        kind: ScopeKind,
        name: Option<Symbol>,
        range: TextRange,
    ) -> ScopeId {
        let id = self.push(interner, kind, name);
        self.scopes[id.index()].range = Some(range);
        id
    }

    /// The pop action, which differs per kind
    /// (`docs/spec/resolve.md` §4.2, rightmost column):
    ///
    /// * `Unscoped`, `Named`, `Module`, `Function` → plain `tail`;
    /// * `Quasi` → restore the parent's `lambdaIndex`, bump its `scopeIndex`;
    /// * `Local` → the above **and** rewrite the parent's `scopeQualifier` to
    ///   `parent ++ ["$quasi<scopeIndex + 1>"]`, resetting `scopeIndex` to 0.
    ///
    /// The file scope is never popped (`updateScopes _ = []` would empty the
    /// stack, but no combinator is ever applied to it).
    pub fn pop(&mut self, interner: &Interner) {
        if self.stack.len() <= 1 {
            debug_assert!(false, "the file scope is never popped");
            return;
        }
        let popped = self.stack.pop().expect("checked above");
        let (kind, child_lambda_index, child_end) = {
            let s = &self.scopes[popped.index()];
            (s.kind, s.lambda_index, s.range.map(|r| r.end()))
        };
        let parent_id = self.current();
        let parent = &mut self.scopes[parent_id.index()];
        match kind {
            // `tail` — the parent is untouched (`Parser.hs:119,123`).
            ScopeKind::Unscoped | ScopeKind::Module | ScopeKind::Named | ScopeKind::Function => {}
            // `y { lambdaIndex = lambdaIndex x, scopeIndex = scopeIndex y + 1 }`
            // (`Parser.hs:142`).
            ScopeKind::Quasi => {
                parent.lambda_index = child_lambda_index;
                parent.scope_index += 1;
            }
            // `y { scopeQualifier = scopeQualifier y ++ ["$quasi<scopeIndex y + 1>"]
            //    , lambdaIndex = lambdaIndex x, scopeIndex = 0 }` (`Parser.hs:153-155`).
            ScopeKind::Local => {
                let deeper = quasi_name(interner, parent.scope_index + 1);
                parent.qualifier = parent.qualifier.child(deeper);
                parent.lambda_index = child_lambda_index;
                parent.scope_index = 0;
                if let Some(end) = child_end {
                    parent.deepened.push((end, parent.qualifier.clone()));
                }
            }
        }
    }

    /// `parseSymbol` (`SymbolTable.hs:194-214`): a hard, immediate error on
    /// redefinition. **Only the innermost scope is consulted.**
    ///
    /// Returns the *previous* declaration on conflict; feed it to
    /// [`redefinition_error`] to build the diagnostic. (The reference's message
    /// embeds `symbolLoc e`, a `file:line:column` string this crate cannot render
    /// — it has no line index — so the location travels as a label instead.)
    ///
    /// Template parameter lists have their own check with a different message,
    /// `"Redefinition of template parameter(s) […]"` (`Parser.hs:352-359`); they
    /// go through [`ScopeStack::seed`], not this.
    pub fn declare(&mut self, name: Symbol, item: ItemId) -> Result<(), ItemId> {
        let scope = self.current();
        let names = &mut self.scopes[scope.index()].names;
        match names.get(&name) {
            Some(&previous) => Err(previous),
            None => {
                names.insert(name, item);
                Ok(())
            }
        }
    }

    /// The `symbols` argument of `unscopedWith` / `scopedWith` /
    /// `quasiScopedWith` — pre-seeded into `symbolsInScope` by `parseScoped`
    /// itself (`SymbolTable.hs:188`), *not* through `parseSymbol`.
    ///
    /// Three consequences, all load-bearing: seeding cannot report a
    /// redefinition, it does not reach the module's flat symbol map, and a later
    /// `declare` of the same name in the same scope *does* collide with it — which
    /// is how `enum E : uint32 { A, A }` is caught (`Parser.hs:291`) and how a
    /// template parameter collides with a class member (`Parser.hs:336`).
    pub fn seed(&mut self, name: Symbol, item: ItemId) {
        let scope = self.current();
        self.scopes[scope.index()].names.insert(name, item);
    }

    /// `makeSymbolName "?lambda" lambdaIndex` (`Parser.hs:434`,
    /// `SymbolTable.hs:216-225`): read the innermost scope's `lambdaIndex`,
    /// increment it, and return `?lambda<i>`.
    pub fn make_lambda_name(&mut self, interner: &Interner) -> Symbol {
        let scope = self.current();
        let s = &mut self.scopes[scope.index()];
        let i = s.lambda_index;
        s.lambda_index += 1;
        interner.intern(&format!("{}{i}", names::LAMBDA_PREFIX))
    }

    /// `makeSymbolName "$export" exportIndex` (`SymbolTable.hs:213`).
    pub fn make_export_name(&mut self, interner: &Interner) -> Symbol {
        let scope = self.current();
        let s = &mut self.scopes[scope.index()];
        let i = s.export_index;
        s.export_index += 1;
        interner.intern(&format!("{}{i}", names::EXPORT_PREFIX))
    }

    /// The innermost scope's `scopeQualifier` — `getScopeQualifier`
    /// (`SymbolTable.hs:96-97`), the prefix every declaration parsed *now* is
    /// keyed under.
    pub fn qualifier(&self) -> &SymbolPath {
        &self.scopes[self.current().index()].qualifier
    }

    /// Hands the scope tree to the [`crate::lower::ItemTree`] that owns it.
    pub fn into_scopes(self) -> Vec<ScopeData> {
        self.scopes
    }
}

/// `indexedName i "$quasi"` (`Parser.hs:131-134`, `Desugar.hs:167-168`:
/// `indexedName x = fromString . (++ show x)`).
fn quasi_name(interner: &Interner, index: u32) -> Symbol {
    interner.intern(&format!("{}{index}", names::QUASI_PREFIX))
}

/// `"symbol redefinition. See previous declaration at <loc>"`
/// (`SymbolTable.hs:200-201`), with `registerSymbolError`'s `` `<name>`: ``
/// prefix (`Error.hs:134`).
pub fn redefinition_error(
    range: FileRange,
    name: &str,
    previous: FileRange,
    location: Option<&str>,
) -> Diagnostic {
    let at = location.map_or_else(String::new, |l| format!(" at {l}"));
    Diagnostic::error(range, format!("`{name}`: symbol redefinition. See previous declaration{at}"))
        .with_label(previous, "previous declaration")
}

/// `"Redefinition of template parameter(s) […]"` (`Parser.hs:352-359`).
///
/// A separate check with a separate message: `names \\ nub names` over the whole
/// parameter list, so it catches *every* repeat, unlike [`ScopeStack::declare`],
/// which only ever compares against one scope.
pub fn template_parameter_redefinition_error(range: FileRange, duplicates: &[&str]) -> Diagnostic {
    let list =
        duplicates.iter().map(|d| format!("\"{d}\"")).collect::<Vec<_>>().join(",");
    Diagnostic::error(range, format!("Redefinition of template parameter(s) [{list}]"))
}

/// The result of resolving one name (`docs/spec/resolve.md` §11.4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Resolution {
    Found(DefId),
    NotFound,
    /// `nub` left more than one distinct declaration —
    /// `` "`X`: symbol is ambiguous. It could refer to the definition at … or …" ``
    /// (`SymbolTable.hs:233-235`).
    ///
    /// Note this fires even for a *locally declared* symbol whose fully-qualified
    /// name collides with an imported one, because step 2 below concatenates
    /// rather than shadows (`test/syntax/imports.k:75-86`, `expected:1`).
    Ambiguous(Vec<DefId>),
    /// `checkImportQualifiers` (`SymbolTable.hs:246-254`): resolution produced
    /// exactly one symbol and some import alias equals that symbol's name.
    /// Reported **at the import's offset**, not at the use site.
    ImportQualifierConflict { def: DefId, import: crate::module::ImportId },
}

/// Everything `findSymbol` needs about the file it is resolving *in*
/// (`SymbolTable.hs:242`, the `SymbolTable{..}` pattern).
///
/// The reference reads these three off one mutable record; here they are three
/// independently-cached query results, so they are passed together instead.
#[derive(Copy, Clone, Debug)]
pub struct ModuleSymbols<'a> {
    pub origin: ModuleSource,
    /// The file's declarations — needed because step 2 below re-queries the
    /// imports with the **found declaration's** qualified name, not the name that
    /// was written (`SymbolTable.hs:257`, `findInImport … getQualifiedName x`).
    pub items: &'a ItemTree,
    /// `symbols (module_ t)` (`SymbolTable.hs:211`) —
    /// [`ItemTree::module_symbols`].
    pub symbols: &'a FxHashMap<SymbolPath, ItemId>,
}

/// `findSymbol` (`SymbolTable.hs:227-278`) — the complete lookup.
///
/// Unqualified (`qualifier` empty):
/// 1. walk the scope stack innermost → outermost; the **first** scope containing
///    the name wins;
/// 2. if found in a scope, *also* run `find_in_imports` on the **found
///    declaration's qualified name** and concatenate — this is what turns a
///    local/imported collision into an ambiguity rather than shadowing;
/// 3. if no scope has it, `lookup_in_scope` over the module's own symbols, then
///    all imports;
/// 4. `checkImportQualifiers`.
///
/// Qualified (`A::B`): **skip the scope stack entirely**
/// (`SymbolTable.hs:244`).
///
/// Then `nub` — identical declarations reached through two re-export chains are
/// not ambiguous.
pub fn find_symbol(
    scopes: &ScopeStack,
    module: ModuleSymbols<'_>,
    graph: &ModuleGraph,
    qualifier: &[Symbol],
    name: Symbol,
) -> Resolution {
    let raw = if qualifier.is_empty() {
        find_in_scopes(scopes, module, graph, name)
    } else {
        let key = SymbolPath::new(qualifier.iter().copied().chain([name]));
        find_in_symbols(scopes, module, graph, &key)
    };

    // `findInScopes scopes >>= checkImportQualifiers` (`SymbolTable.hs:243`) —
    // applied to the *pre-`nub`* list, and only on the unqualified path.
    let conflict = if qualifier.is_empty() && raw.len() == 1 {
        conflicting_import(graph, module.origin, name)
    } else {
        None
    };

    let hits = nub(raw);
    match hits.len() {
        0 => Resolution::NotFound,
        1 => match conflict {
            Some(import) => Resolution::ImportQualifierConflict { def: hits[0], import },
            None => Resolution::Found(hits[0]),
        },
        _ => Resolution::Ambiguous(hits),
    }
}

/// `findInScopes` (`SymbolTable.hs:256-259`): the first scope that has the bare
/// name wins, and its hit is **concatenated** with everything the imports offer
/// for that declaration's *qualified* name. Concatenation, not shadowing — which
/// is why a local declaration colliding with an imported one is an ambiguity
/// error (`test/syntax/imports.k:75-86`, `expected:1`).
fn find_in_scopes(
    scopes: &ScopeStack,
    module: ModuleSymbols<'_>,
    graph: &ModuleGraph,
    name: Symbol,
) -> Vec<DefId> {
    for id in scopes.chain() {
        if let Some(&item) = scopes.get(id).names.get(&name) {
            let mut hits = vec![DefId::new(module.origin, item)];
            let qualified = module.items.get(item).qualified_name.clone();
            hits.extend(find_in_imports(graph, module.origin, &qualified));
            return hits;
        }
    }
    find_in_symbols(scopes, module, graph, &SymbolPath::single(name))
}

/// `findInSymbols` (`SymbolTable.hs:261`):
/// `maybeToList (lookupSymbol s (symbols module_)) <> findInImports s`, where
/// `lookupSymbol e = lookupInScope (getScope e) e` and `getScope` of a use site
/// is the scope qualifier recorded when it was parsed (`Parser/Syntax.hs:769`).
fn find_in_symbols(
    scopes: &ScopeStack,
    module: ModuleSymbols<'_>,
    graph: &ModuleGraph,
    key: &SymbolPath,
) -> Vec<DefId> {
    let mut hits = Vec::new();
    if let Some(&item) = lookup_in_scope(module.symbols, scopes.qualifier(), key) {
        hits.push(DefId::new(module.origin, item));
    }
    hits.extend(find_in_imports(graph, module.origin, key));
    hits
}

/// `findInImports` (`SymbolTable.hs:263-270`), delegated to
/// [`ModuleGraph::find_in_imports`], which owns the reverse iteration and the
/// running set difference.
///
/// The reference performs one more thing here that this does not: it *mutates*
/// the imports, marking `Needed` any whose residual set is non-empty
/// (`updateNeeded`, `SymbolTable.hs:266-270`). `docs/spec/resolve.md` §11.4 makes
/// that a separate accumulator (`import_usage`) precisely so that resolving a name
/// stays a pure query; [`ModuleGraph::import_hits`] is the attributed form to feed
/// it.
fn find_in_imports(graph: &ModuleGraph, origin: ModuleSource, name: &SymbolPath) -> Vec<DefId> {
    graph.find_in_imports(origin, name)
}

/// `checkImportQualifiers` (`SymbolTable.hs:246-254`): a resolution that produced
/// exactly one symbol, whose *written* name equals some import's alias.
///
/// The reference `registerFancyError`s and then returns `[x]` unchanged — the
/// name still resolves. [`Resolution::ImportQualifierConflict`] carries the
/// resolved [`DefId`] for the same reason. `find` takes the **first** match in
/// `imports module_`, which the reference builds by cons and this crate stores in
/// source order, so an aliased name that collides twice is attributed to the last
/// `import` in the file there and the first here — a difference no oracle can see,
/// because the message names the symbol, not the import.
fn conflicting_import(
    graph: &ModuleGraph,
    origin: ModuleSource,
    name: Symbol,
) -> Option<ImportId> {
    graph.imports(origin).iter().find(|i| i.alias == Some(name)).map(|i| i.id)
}

/// Haskell `nub`: keep the first occurrence of each element, in order
/// (`SymbolTable.hs:231`). Quadratic, like `nub`; the lists are 0-2 long in
/// practice and the *order* is what the ambiguity message prints.
fn nub(mut hits: Vec<DefId>) -> Vec<DefId> {
    let mut seen = Vec::with_capacity(hits.len());
    hits.retain(|d| {
        if seen.contains(d) {
            false
        } else {
            seen.push(*d);
            true
        }
    });
    hits
}

/// `lookupInScope` (`Symbols.hs:79-87`) — the *post-parse* analogue of the scope
/// stack, and what makes forward references inside a module work.
///
/// ```text
/// lookupInScope s e symbols = case lookup (s ++ getQualifiedName e) symbols of
///     Just x        -> Just x
///     Nothing | null s -> Nothing
///             | otherwise -> lookupInScope (init s) e symbols
/// ```
///
/// It walks **outward** by dropping the last scope component, trying
/// `scope ++ qualifier ++ [name]` at each level, and finally the empty scope.
pub fn lookup_in_scope<'a, T>(
    symbols: &'a FxHashMap<SymbolPath, T>,
    scope: &SymbolPath,
    name: &SymbolPath,
) -> Option<&'a T> {
    let mut scope = scope.clone();
    loop {
        if let Some(hit) = symbols.get(&scope.concat(name)) {
            return Some(hit);
        }
        if scope.is_empty() {
            return None;
        }
        scope = scope.init();
    }
}

/// The `shadow` warning (`TypeCheck.hs:110-121`,
/// `docs/spec/resolve.md` §4.4).
///
/// **All five** conditions must hold:
/// 1. the node is a `Variable` — never a function, type, alias or enum constant;
/// 2. its `declScopeName` is **non-empty** — a declaration at the unqualified
///    global scope never warns (`test/syntax/shadow-warning.k:44-53`);
/// 3. its `DeclareFlag` is neither `DeclareMember` nor `DeclareClass`;
/// 4. `lookupInScope (init declScopeName) declName` finds something — i.e. drop
///    **one** level of scope, *then* walk outward;
/// 5. the hit is a `Variable` with a **non-empty** scope, or a `FuncParam`.
///
/// Timing: `validateProgram` is the last frontend pass, so this sees the AST
/// **after** `moveFunctions` has hoisted nested functions and renamed them
/// `<parent>@<name>` — which is why a nested function re-declaring `x` does not
/// warn (`test/syntax/shadow-warning.k:133-147`).
pub fn shadow_warning(range: FileRange, name: &str, shadowed: FileRange) -> Diagnostic {
    Diagnostic::warning(
        range,
        WarningKind::Shadow,
        format!("This declaration of `{name}` shadows the existing declaration"),
    )
    .with_label(shadowed, "previous declaration")
}

/// The five conditions of `checkShadowDecl` (`TypeCheck.hs:110-121`), applied to
/// one declaration. Returns what it shadows, or `None`.
///
/// Note condition 4 drops **one** scope component *before* the outward walk, so a
/// declaration never shadows a sibling in the same scope — that would be a
/// redefinition, caught much earlier by [`ScopeStack::declare`].
pub fn shadowed_declaration(
    program: Program<'_>,
    symbols: &FxHashMap<SymbolPath, DefId>,
    decl: &Item,
) -> Option<DefId> {
    // 1. the node is a `Variable`.
    let DefKind::Variable(flags) = decl.kind else { return None };
    // 3. `declFlags == DeclareMember || declFlags == DeclareClass = a`.
    if matches!(flags, DeclareFlag::DeclareMember | DeclareFlag::DeclareClass) {
        return None;
    }
    // 2. `null declScopeName = a`.
    let scope = decl.scope_name();
    if scope.is_empty() {
        return None;
    }
    // 4. `lookupInScope (init declScopeName) declName symbols`.
    let name = decl.qualified_name.last()?;
    let hit = *lookup_in_scope(symbols, &scope.init(), &SymbolPath::single(name))?;
    // 5. `shadowable`.
    let found = program.item(hit)?;
    let shadowable = match found.kind {
        DefKind::Variable(_) => !found.scope_name().is_empty(),
        DefKind::FuncParam => true,
        _ => false,
    };
    shadowable.then_some(hit)
}

/// `checkShadowDecl` over a whole program — one of `validateProgram`'s rewrites
/// (`TypeCheck.hs:50`), reported as diagnostics instead of as `WarningF` nodes
/// spliced into the tree.
///
/// # Timing is part of the specification
///
/// `validateProgram` is the **last** frontend pass to execute (`Frontend.hs:38`
/// folds the pass list with `foldr ($)`, so element 0 runs last —
/// `docs/spec/resolve.md` §3.2). It therefore sees an AST in which
/// `moveFunctions` has already hoisted every function declared inside a function
/// body to its parent scope and renamed it `<parent>@<name>`
/// (`Function.hs:39,47-50,97`). Running this over a *freshly lowered*
/// [`ItemTree`] reports one warning the reference does not: the nested-function
/// row of the oracle (`test/syntax/shadow-warning.k:133-147`, `warning:0`), whose
/// inner `x` is keyed `[main,$,Bar,$,x]` before hoisting and `[main@Bar,$,x]`
/// after. Only the second fails to reach `main`'s `x`. See the test
/// `nested_functions_only_stop_shadowing_after_hoisting`.
pub fn shadow_warnings(
    program: Program<'_>,
    symbols: &FxHashMap<SymbolPath, DefId>,
    interner: &Interner,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (_, item) in program.items() {
        let Some(shadowed) = shadowed_declaration(program, symbols, item) else { continue };
        let Some(previous) = program.item(shadowed) else { continue };
        let Some(name) = item.qualified_name.last() else { continue };
        out.push(shadow_warning(item.range, &interner.resolve(name), previous.range));
    }
    out
}

/// The `redundant-import` warning (`Parser.hs:234-241`, `Error.hs:142-145`).
///
/// Fires **iff** the import is still [`crate::module::ImportNeeded::Redundant`]
/// at end of file. The implicit base import is added with `Needed`
/// (`Parser/IO.hs:77`), so `base` / `.cmdargs` never warn.
pub fn redundant_import_warning(range: FileRange) -> Diagnostic {
    Diagnostic::warning(range, WarningKind::RedundantImport, "The import is redundant")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_base::Interner;

    #[test]
    fn lookup_walks_outward_one_component_at_a_time() {
        // Symbols.hs:79-87.
        let i = Interner::new();
        let (m, f, dollar, x) =
            (i.intern("@m"), i.intern("f"), i.intern("$"), i.intern("x"));
        let mut symbols: FxHashMap<SymbolPath, u32> = FxHashMap::default();
        symbols.insert(SymbolPath::new([m, x]), 1);

        let inner = SymbolPath::new([m, f, dollar]);
        assert_eq!(lookup_in_scope(&symbols, &inner, &SymbolPath::single(x)), Some(&1));

        // ... and finds nothing once there is nothing left to drop.
        assert_eq!(
            lookup_in_scope(&symbols, &inner, &SymbolPath::single(i.intern("zz"))),
            None
        );
    }

    #[test]
    fn exact_scope_beats_an_outer_one() {
        let i = Interner::new();
        let (m, f, x) = (i.intern("@m"), i.intern("f"), i.intern("x"));
        let mut symbols: FxHashMap<SymbolPath, u32> = FxHashMap::default();
        symbols.insert(SymbolPath::new([m, x]), 1);
        symbols.insert(SymbolPath::new([m, f, x]), 2);
        assert_eq!(
            lookup_in_scope(&symbols, &SymbolPath::new([m, f]), &SymbolPath::single(x)),
            Some(&2)
        );
    }

    #[test]
    fn only_unscoped_leaves_the_qualifier_alone() {
        // docs/spec/resolve.md §4.2, second column.
        assert!(!ScopeKind::Unscoped.extends_qualifier());
        for k in [
            ScopeKind::Module,
            ScopeKind::Named,
            ScopeKind::Quasi,
            ScopeKind::Local,
            ScopeKind::Function,
        ] {
            assert!(k.extends_qualifier(), "{k:?}");
        }
    }

    #[test]
    fn file_scope_qualifier_is_empty() {
        // SymbolTable.hs:94.
        let s = ScopeStack::new();
        assert_eq!(s.current(), ScopeId::FILE);
        assert!(s.get(ScopeId::FILE).qualifier.is_empty());
    }

    // --- the scope stack ---------------------------------------------------

    /// `functionScopedWith syms = scopedWith syms (Identifier "$")`
    /// (`Parser.hs:127`) — the component is `$` whatever the caller passes.
    #[test]
    fn a_function_scope_is_always_named_dollar() {
        let i = Interner::new();
        let mut s = ScopeStack::new();
        s.push(&i, ScopeKind::Named, Some(i.intern("f")));
        s.push(&i, ScopeKind::Function, Some(i.intern("ignored")));
        assert_eq!(s.qualifier().segments(), [i.intern("f"), i.intern("$")]);
    }

    /// `quasiScopedWith` (`Parser.hs:136-143`) versus `localScoped`
    /// (`Parser.hs:147-155`): both are named `$quasi<parent.scopeIndex>` and both
    /// restore the parent's `lambdaIndex`, but only `localScoped` rewrites the
    /// parent's *qualifier* on the way out.
    #[test]
    fn quasi_pop_bumps_the_index_and_local_pop_deepens_the_qualifier() {
        let i = Interner::new();
        let (f, q0, q1) = (i.intern("f"), i.intern("$quasi0"), i.intern("$quasi1"));

        let mut quasi = ScopeStack::new();
        quasi.push(&i, ScopeKind::Named, Some(f));
        quasi.push(&i, ScopeKind::Quasi, None);
        assert_eq!(quasi.qualifier().segments(), [f, q0]);
        quasi.pop(&i);
        assert_eq!(quasi.qualifier().segments(), [f], "the parent qualifier is untouched");
        assert_eq!(quasi.get(quasi.current()).scope_index, 1, "but its scopeIndex moved");
        // The next sibling is therefore `$quasi1`.
        quasi.push(&i, ScopeKind::Quasi, None);
        assert_eq!(quasi.qualifier().segments(), [f, q1]);

        let mut local = ScopeStack::new();
        local.push(&i, ScopeKind::Named, Some(f));
        local.push(&i, ScopeKind::Local, None);
        assert_eq!(local.qualifier().segments(), [f, q0]);
        local.pop(&i);
        assert_eq!(local.qualifier().segments(), [f, q1], "the parent moved under $quasi1");
        assert_eq!(local.get(local.current()).scope_index, 0, "and its index reset");
        // So the next sibling is `$quasi0` *of the deeper qualifier* — distinct
        // from its predecessor either way.
        local.push(&i, ScopeKind::Local, None);
        assert_eq!(local.qualifier().segments(), [f, q1, q0]);
    }

    /// `qualifier` drifts as `localScoped` children close (`Parser.hs:153`);
    /// `entry_qualifier` does not. A finished [`ItemTree`] therefore stores a
    /// function-body scope whose `qualifier` is deeper than any key ever written
    /// under it — which is why an [`Item`] carries its own `qualified_name`.
    #[test]
    fn the_entry_qualifier_is_the_stable_half() {
        let i = Interner::new();
        let f = i.intern("f");
        let mut s = ScopeStack::new();
        s.push(&i, ScopeKind::Named, Some(f));
        let body = s.current();
        s.push(&i, ScopeKind::Local, None);
        s.pop(&i);
        s.push(&i, ScopeKind::Local, None);
        s.pop(&i);
        assert_eq!(s.get(body).entry_qualifier.segments(), [f]);
        assert_eq!(
            s.get(body).qualifier.segments(),
            [f, i.intern("$quasi1"), i.intern("$quasi1")],
            "two closed blocks, two appended components"
        );
    }

    /// `unscopedWith` leaves the qualifier alone and seeds both counters to zero
    /// (`Parser.hs:119`), and pops with a plain `tail`.
    #[test]
    fn an_unscoped_scope_is_transparent_to_the_qualifier() {
        let i = Interner::new();
        let mut s = ScopeStack::new();
        s.push(&i, ScopeKind::Named, Some(i.intern("f")));
        s.push(&i, ScopeKind::Quasi, None);
        s.pop(&i); // f.scopeIndex == 1
        s.push(&i, ScopeKind::Unscoped, None);
        assert_eq!(s.qualifier().segments(), [i.intern("f")]);
        assert_eq!(s.get(s.current()).scope_index, 0, "unscopedWith seeds 0");
        s.pop(&i);
        assert_eq!(s.get(s.current()).scope_index, 1, "and restores nothing");
    }

    /// `makeSymbolName` reads *then* increments, per scope
    /// (`SymbolTable.hs:216-225`), and `indexedName x = (++ show x)`
    /// (`Desugar.hs:167-168`).
    #[test]
    fn lambda_and_export_indices_count_up_per_scope() {
        let i = Interner::new();
        let mut s = ScopeStack::new();
        assert_eq!(s.make_lambda_name(&i), i.intern("?lambda0"));
        assert_eq!(s.make_lambda_name(&i), i.intern("?lambda1"));
        assert_eq!(s.make_export_name(&i), i.intern("$export0"));
        // A fresh named scope starts over (`parseScoped … (const 0)`).
        s.push(&i, ScopeKind::Named, Some(i.intern("f")));
        assert_eq!(s.make_lambda_name(&i), i.intern("?lambda0"));
    }

    /// A `quasiScoped` pop restores the parent's `lambdaIndex` from the *child*
    /// (`Parser.hs:142`), so a lambda written inside a block still advances the
    /// enclosing counter.
    #[test]
    fn a_block_hands_its_lambda_index_back_to_its_parent() {
        let i = Interner::new();
        let mut s = ScopeStack::new();
        s.push(&i, ScopeKind::Local, None);
        assert_eq!(s.make_lambda_name(&i), i.intern("?lambda0"));
        s.pop(&i);
        assert_eq!(s.make_lambda_name(&i), i.intern("?lambda1"));
    }

    /// `parseSymbol` consults only `head scopes` (`SymbolTable.hs:198`), and the
    /// seeded symbols of `parseScoped` are in that same map
    /// (`SymbolTable.hs:188`) — so a seed collides with a later `declare`.
    #[test]
    fn declare_collides_with_a_seed_but_seeding_never_reports() {
        let i = Interner::new();
        let mut s = ScopeStack::new();
        let x = i.intern("x");
        s.seed(x, ItemId(0));
        s.seed(x, ItemId(1)); // seeding twice is silent
        assert_eq!(s.declare(x, ItemId(2)), Err(ItemId(1)));
        // An outer scope does not collide.
        s.push(&i, ScopeKind::Local, None);
        assert_eq!(s.declare(x, ItemId(3)), Ok(()));
    }

    // --- the shadow warning ------------------------------------------------

    /// Lower one `test/syntax/shadow-warning.k` snippet and count the warnings
    /// `validateProgram` would attach.
    fn shadow_count(src: &str) -> usize {
        let i = Interner::new();
        let trees = [(
            ModuleSource::File(kg_base::FileId(0)),
            crate::lower::lower_str(src, &i),
        )];
        let symbols =
            crate::lower::global_symbols(crate::def::SymbolsConfig::AllSymbols, &trees, &i);
        shadow_warnings(Program::new(&trees), &symbols, &i).len()
    }

    /// `test/syntax/shadow-warning.k`, the rows that assert `warning:shadow`
    /// (registered at `test/syntax/CMakeLists.txt:77`). The runner asserts only
    /// that the *first* warning is a `shadow`, so these assert "at least one".
    #[test]
    fn the_shadow_oracle_rows_that_warn() {
        // 4-12: local `bool x`, nested block `uint32 x`.
        assert!(shadow_count("inline void main()\n{\n bool x;\n {\n uint32 x;\n }\n}\n") > 0);
        // 14-22: outer `const bool x`, inner `uint32 x`.
        assert!(
            shadow_count("inline void main()\n{\n const bool x = false;\n {\n uint32 x;\n }\n}\n") > 0
        );
        // 24-32: outer `bool x`, inner `const uint32 x`.
        assert!(
            shadow_count("inline void main()\n{\n bool x;\n {\n const uint32 x = 1;\n }\n}\n") > 0
        );
        // 34-42: outer `bool x`, inner `auto x`.
        assert!(shadow_count("inline void main()\n{\n bool x;\n {\n auto x = 0;\n }\n}\n") > 0);
        // 55-62: parameter `bool x`, block-local `uint32 x`.
        assert!(shadow_count("inline void Run(bool x)\n{\n {\n uint32 x;\n }\n}\n") > 0);
        // 68-75: parameter `x`, `for (const auto x : …)`.
        assert!(
            shadow_count("inline void Run(bool x)\n{\n for (const auto x : 10)\n {\n }\n}\n") > 0
        );
        // 88-98: nested `for` with the same induction name.
        assert!(
            shadow_count(
                "inline void main()\n{\n for (const auto x : 10)\n {\n for (const auto x : 6)\n {\n }\n }\n}\n"
            ) > 0
        );
        // 114-131: nested `if` inside a branch that already declares `y`.
        assert!(
            shadow_count(
                "inline void Run(bool x)\n{\n if (x)\n {\n uint32 y;\n\n if (x)\n {\n uint32 y;\n }\n else\n {\n uint32 y;\n }\n }\n}\n"
            ) > 0
        );
        // 149-159: class field `x`, member-function local `x`. `DeclareClass` is
        // exempt from *being warned about* (condition 3) but is still shadowable
        // (condition 5).
        assert!(
            shadow_count("class Foo\n{\n uint32 x;\n\n inline void Bar()\n {\n bool x;\n }\n}\n") > 0
        );
    }

    /// The four `warning:0` rows — the ones that catch over-reporting, and the
    /// reason every detail of the qualifier algebra has to be exact.
    #[test]
    fn the_shadow_oracle_rows_that_do_not_warn() {
        // 44-53: a file-scope declaration has an *empty* `getScope`, so
        // `shadowable` rejects it (`TypeCheck.hs:119`).
        assert_eq!(
            shadow_count("const bool x = true;\n\ninline void main()\n{\n {\n uint32 x;\n }\n}\n"),
            0
        );
        // 77-86: `localScoped`'s parent rewrite puts the loop's `x` under
        // `$quasi0` and the later `x` under `$quasi1` — siblings, never nested.
        assert_eq!(
            shadow_count(
                "inline void main()\n{\n for (const auto x : 10)\n {\n }\n\n bool x;\n}\n"
            ),
            0
        );
        // 100-112: `if`/`else` branches each declaring `y`.
        assert_eq!(
            shadow_count(
                "inline void Run(bool x)\n{\n if (x)\n {\n uint32 y;\n }\n else\n {\n uint32 y;\n }\n}\n"
            ),
            0
        );
        // A declaration cannot shadow a *sibling* in the same scope: condition 4
        // drops one scope component before walking out. (That case is a
        // redefinition, caught by `parseSymbol`.)
        assert_eq!(shadow_count("inline void main()\n{\n bool x;\n bool x;\n}\n"), 0);
    }

    /// `test/syntax/shadow-warning.k:133-147` is `warning:0`, and it is the one
    /// row this pass cannot reproduce on its own: it depends on `moveFunctions`
    /// having already hoisted `Bar` out of `main` and renamed it `main@Bar`
    /// (`Function.hs:39,47-50,97`), which happens before `validateProgram`
    /// (`docs/spec/resolve.md` §3.2, §4.4).
    ///
    /// Both halves are asserted: run over freshly lowered keys the warning fires,
    /// and the query the reference actually performs — `lookupInScope
    /// [main@Bar] x` — finds nothing in the very same map.
    #[test]
    fn nested_functions_only_stop_shadowing_after_hoisting() {
        let i = Interner::new();
        let src = "inline void main()\n{\n uint32 x;\n\n inline void Bar()\n {\n uint32 x;\n\n x = 0;\n }\n\n x = 0;\n}\n";
        let trees =
            [(ModuleSource::File(kg_base::FileId(0)), crate::lower::lower_str(src, &i))];
        let symbols =
            crate::lower::global_symbols(crate::def::SymbolsConfig::AllSymbols, &trees, &i);

        // Un-hoisted, `Bar`'s `x` is keyed [main,$,Bar,$,x] and `init` of its
        // scope is [main,$,Bar], which walks out to [main,$,x].
        assert_eq!(shadow_count(src), 1);

        // Hoisted, it is keyed [main@Bar,$,x]; `init` is [main@Bar], and neither
        // [main@Bar,x] nor [x] exists.
        let hoisted = SymbolPath::single(i.intern("main@Bar"));
        assert!(
            lookup_in_scope(&symbols, &hoisted, &SymbolPath::single(i.intern("x"))).is_none()
        );
    }

    /// Condition 3 (`TypeCheck.hs:112`): a class field (`DeclareClass`) never
    /// *reports* a shadow, even when conditions 4 and 5 both hold.
    ///
    /// The two snippets differ only in the flag the enclosing construct assigns.
    /// `class C`'s field is keyed `[@m,C,x]`, whose `init` walks out to `[@m,x]`
    /// and finds a shadowable `Variable`; a function local is keyed `[@m,C,$,x]`
    /// and finds the same declaration — but only the second warns.
    #[test]
    fn a_class_field_is_exempt_from_reporting_a_shadow() {
        let field = "module m {}\nconst uint32 x = 1;\n\nclass C\n{\n uint32 x;\n}\n";
        let local = "module m {}\nconst uint32 x = 1;\n\ninline void C()\n{\n uint32 x;\n}\n";
        assert_eq!(shadow_count(field), 0, "DeclareClass is exempt");
        assert_eq!(shadow_count(local), 1, "DeclareLocal is not");
    }

    // --- findSymbol --------------------------------------------------------

    fn item(kind: DefKind, key: SymbolPath) -> Item {
        Item {
            decl: FileRange::new(kg_base::FileId(0), kg_base::TextRange::empty(kg_base::TextSize::from(0))),
            scope: ScopeId::FILE,
            qualified_name: key,
            kind,
            is_template: false,
            parent: None,
            range: FileRange::new(kg_base::FileId(0), kg_base::TextRange::empty(0.into())),
            exposable: true,
        }
    }

    /// Step 1 (`SymbolTable.hs:256-258`): the innermost scope holding the bare
    /// name wins, and the scopes are walked innermost → outermost.
    #[test]
    fn an_unqualified_name_resolves_through_the_scope_chain() {
        let i = Interner::new();
        let (f, x) = (i.intern("f"), i.intern("x"));
        let mut tree = ItemTree::default();
        tree.items.push(item(DefKind::Variable(DeclareFlag::DeclareGlobal), SymbolPath::single(x)));
        tree.items.push(item(
            DefKind::Variable(DeclareFlag::DeclareLocal),
            SymbolPath::new([f, i.intern("$"), x]),
        ));

        let mut scopes = ScopeStack::new();
        scopes.declare(x, ItemId(0)).unwrap();
        scopes.push(&i, ScopeKind::Named, Some(f));
        scopes.push(&i, ScopeKind::Function, None);
        scopes.declare(x, ItemId(1)).unwrap();

        let symbols = tree.module_symbols();
        let module = ModuleSymbols {
            origin: ModuleSource::File(kg_base::FileId(0)),
            items: &tree,
            symbols: &symbols,
        };
        let graph = ModuleGraph::default();
        assert_eq!(
            find_symbol(&scopes, module, &graph, &[], x),
            Resolution::Found(DefId::new(ModuleSource::File(kg_base::FileId(0)), ItemId(1))),
            "the innermost `x` wins"
        );
    }

    /// Step 3 (`SymbolTable.hs:259,261`): a name in no scope at all falls through
    /// to `lookupSymbol` over the module's own map, which is order-independent.
    /// That is what makes `test/syntax/declaration-order.k` — `const auto Bar =
    /// Foo;` where `Foo` is declared later in an imported module — `expected:0`.
    #[test]
    fn a_name_in_no_scope_falls_through_to_the_module_map() {
        let i = Interner::new();
        let (f, later) = (i.intern("f"), i.intern("Later"));
        let mut tree = ItemTree::default();
        tree.items.push(item(
            DefKind::Variable(DeclareFlag::DeclareGlobal),
            SymbolPath::single(later),
        ));

        // Resolving from inside `f`'s body, where nothing is in scope.
        let mut scopes = ScopeStack::new();
        scopes.push(&i, ScopeKind::Named, Some(f));
        scopes.push(&i, ScopeKind::Function, None);

        let symbols = tree.module_symbols();
        let module = ModuleSymbols {
            origin: ModuleSource::File(kg_base::FileId(0)),
            items: &tree,
            symbols: &symbols,
        };
        let graph = ModuleGraph::default();
        assert_eq!(
            find_symbol(&scopes, module, &graph, &[], later),
            Resolution::Found(DefId::new(ModuleSource::File(kg_base::FileId(0)), ItemId(0)))
        );
        assert_eq!(
            find_symbol(&scopes, module, &graph, &[], i.intern("Missing")),
            Resolution::NotFound
        );
    }

    /// Step 2 (`SymbolTable.hs:257`): a hit in the scope stack is
    /// **concatenated** with the imports' hits for that declaration's qualified
    /// name, not preferred over them — so a local declaration whose name is also
    /// imported is *ambiguous*, not shadowing.
    ///
    /// Oracle: `test/syntax/imports.k:75-86` — `import a1.b2.c1.c1` followed by a
    /// local `struct c1` and a use of `c1` is `expected:1`.
    #[test]
    fn a_local_declaration_colliding_with_an_imported_one_is_ambiguous() {
        let i = Interner::new();
        let (here, there) =
            (ModuleSource::File(kg_base::FileId(0)), ModuleSource::File(kg_base::FileId(1)));
        let c1 = i.intern("c1");
        let namespace = crate::module::Namespace(i.intern("@a1@b2@c1"));

        let mut tree = ItemTree::default();
        tree.items.push(item(DefKind::Struct, SymbolPath::single(c1)));
        let mut scopes = ScopeStack::new();
        scopes.declare(c1, ItemId(0)).unwrap();

        let imported = DefId::new(there, ItemId(0));
        let mut graph = ModuleGraph::default();
        graph.imports.insert(
            here,
            vec![crate::module::ImportDecl {
                id: crate::module::ImportId(0),
                name: vec![kg_base::SmolStr::new("a1")],
                alias: None,
                range: FileRange::new(kg_base::FileId(0), kg_base::TextRange::empty(0.into())),
                needed: crate::module::ImportNeeded::Redundant,
            }],
        );
        graph.targets.insert(here, vec![there]);
        let mut visible = FxHashMap::default();
        let mut in_ns = FxHashMap::default();
        in_ns.insert(SymbolPath::new([namespace.0, c1]), imported);
        visible.insert(namespace, in_ns);
        graph.interfaces.insert(
            there,
            crate::module::ModuleInterface {
                namespace: Some(namespace),
                visible,
                needed: false,
            },
        );

        let symbols = tree.module_symbols();
        let module = ModuleSymbols { origin: here, items: &tree, symbols: &symbols };
        assert_eq!(
            find_symbol(&scopes, module, &graph, &[], c1),
            Resolution::Ambiguous(vec![DefId::new(here, ItemId(0)), imported])
        );

        // `checkImportQualifiers` (`SymbolTable.hs:246-254`) only fires on a
        // *single* hit, and only when an alias equals the name.
        graph.imports.get_mut(&here).unwrap()[0].alias = Some(c1);
        assert!(matches!(
            find_symbol(&scopes, module, &graph, &[], i.intern("nothing")),
            Resolution::NotFound
        ));
    }

    /// `| null (getQualifier s) = findInScopes …; | otherwise = findInSymbols`
    /// (`SymbolTable.hs:243-244`): a **qualified** name skips the scope stack
    /// entirely, so a local `x` cannot answer `C::x`.
    #[test]
    fn a_qualified_name_skips_the_scope_stack() {
        let i = Interner::new();
        let (c, x) = (i.intern("C"), i.intern("x"));
        let mut tree = ItemTree::default();
        tree.items.push(item(DefKind::Variable(DeclareFlag::DeclareLocal), SymbolPath::single(x)));
        tree.items
            .push(item(DefKind::Variable(DeclareFlag::DeclareClass), SymbolPath::new([c, x])));

        let mut scopes = ScopeStack::new();
        scopes.declare(x, ItemId(0)).unwrap();

        let symbols = tree.module_symbols();
        let module = ModuleSymbols {
            origin: ModuleSource::File(kg_base::FileId(0)),
            items: &tree,
            symbols: &symbols,
        };
        let graph = ModuleGraph::default();
        assert_eq!(
            find_symbol(&scopes, module, &graph, &[c], x),
            Resolution::Found(DefId::new(ModuleSource::File(kg_base::FileId(0)), ItemId(1))),
            "C::x is the class member, not the local"
        );
        assert_eq!(
            find_symbol(&scopes, module, &graph, &[i.intern("Nope")], x),
            Resolution::NotFound
        );
    }
}
