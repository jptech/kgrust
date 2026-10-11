//! The parse-time symbol oracle path sorting consults — slice 3 of pass 8.
//!
//! The reference's parser sorts every written name into whichever `ExpF` shape
//! the symbol table says it denotes, *as it parses* (`Parser.hs:652-695`), so
//! the answer depends on what has been declared **before this point of the
//! file** plus everything the imports expose. This module carries the state
//! that walk needs:
//!
//! * [`SymShape`] — what the sorter must know about a resolved symbol: the
//!   predicates `isType` / `isTemplate` / `isFunction`
//!   (`Parser/Syntax.hs:676-722`), the alias-body target
//!   (`Parser.hs:667-668`), and the variable facts `resolveTypeName` and
//!   `freeFunctionSpecifier` read (`Parser.hs:736-762,772-780`).
//! * [`SymTab`] — the incremental table: one entry per declaration lowered so
//!   far, the module's flat map (`addToModule`, `SymbolTable.hs:203-211`), and
//!   the import door. Scope membership itself lives in the
//!   [`kg_resolve::ScopeStack`] the lowering already walks with — `declare`
//!   and `seed` there are `parseSymbol` and `parseScoped`'s seeding
//!   (`SymbolTable.hs:188,194-214`).
//! * [`SymTab::find`] — `findSymbol` (`SymbolTable.hs:227-278`), the same
//!   algorithm `kg_resolve::find_symbol` implements over post-parse trees,
//!   restated here over the *incremental* table because the whole point is
//!   declaration order: a name written before its declaration resolves to
//!   `Nothing` in the reference, and the sorted tree shows it (no qualifier
//!   materialised, no `TemplateInstance` wrapper).
//!
//! Cross-file symbols arrive through [`ImportSymbols`], implemented by `kg-db`
//! over its module graph; [`NoImports`] is the empty world a self-contained
//! file sorts against. When no context is supplied at all, `lower` does not
//! sort — see divergence #22: from a single file *any* local hit could still
//! be ambiguous with an import (`findInScopes` concatenates import hits even
//! over scope hits, `SymbolTable.hs:257`), so no sort is safe alone.

use std::sync::Arc;

use kg_base::{FileRange, FxHashMap, Interner, Symbol, SymbolPath};
use kg_resolve::{ItemId, ScopeStack, lookup_in_scope};

use crate::arena::ExpId;

/// `unresolvedName = "$unresolved"` (`Parser/Syntax.hs:270-271`) — the
/// placeholder written prefix of every method callee (`Parser.hs:764`).
pub const UNRESOLVED_NAME: &str = "$unresolved";

/// What the sorter needs to know about one resolved symbol.
///
/// The variants are the `case sym of` scrutinees of `Parser.hs:652-695` and
/// the sorters at `:702-801`, not the full `DefKind` — a shape distinction
/// with no sorting consequence is deliberately not represented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymShape {
    /// `struct` / `union` / `class` / `enum` — `isType`, not a template, not
    /// a function (`Parser/Syntax.hs:687-690`).
    Type,
    /// `using T = …`. `target` is the qualified name of the aliased
    /// `TypeIdentifier` payload when the body sorted to one — the `t` of
    /// `Alias _ (TypeIdentifier _ t) _` (`Parser.hs:667-668`), which replaces
    /// the resolution prefix for any `::` segment that follows. A body that
    /// is not a named type (`using W = uint32`) has no target and takes the
    /// generic branch.
    Alias { target: Option<SymbolPath> },
    /// `template <…> decl`. `function`/`type_like` are `isFunction`/`isType`
    /// of the wrapped declaration (`Parser/Syntax.hs:695,719`).
    Template { function: bool, type_like: bool },
    /// A function or a bodiless declaration — `isFunction`
    /// (`Parser/Syntax.hs:716-719`).
    Function,
    /// A `variable` of any flavour. The three fields feed, in order:
    /// `namedLValue`'s const check (`Parser.hs:799-800`), `resolveTypeName`
    /// (`Parser.hs:736-757`), and `freeFunctionSpecifier`'s const-function
    /// substitution (`Parser.hs:776-777`).
    Variable { const_ty: bool, type_name: Option<TypeName>, fn_init: Option<FnInit> },
    /// `FuncParam`, `EnumConstant`, `Capture` — value symbols that sort like
    /// variables but are invisible to `resolveTypeName`, whose `Variable`
    /// pattern they do not match (`Parser.hs:741-748`).
    OtherValue,
    /// A template parameter. Always same-file: the reference seeds them into
    /// the scope stack only (`Parser.hs:363`) and they never reach a module
    /// map or an interface.
    TemplateParam(TemplateParamSym),
}

/// The declaration half of `TemplateParamF n a b c _` — everything a use site
/// needs to rebuild `TypeParamF n a b c` (`Parser.hs:660`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateParamSym {
    /// `n` — the qualifier captured before the parameter list (`Parser.hs:353`).
    pub scope: SymbolPath,
    pub name: Symbol,
    pub index: u32,
    /// The declared kind node (`a`), deep-copied into each use.
    pub kind: ExpId,
    /// `isType a` — false for `typename` (so the *parameter* is a type,
    /// `Parser/Syntax.hs:697`).
    pub kind_is_type: bool,
    /// `isTemplate a` — a template-template signature (`Parser.hs:661`).
    pub kind_is_template: bool,
    /// `a@FunctionType{}` (`Parser.hs:774,788`).
    pub kind_is_function_type: bool,
}

impl SymShape {
    /// `isType` (`Parser/Syntax.hs:676-699`), restricted to declaration forms.
    pub fn is_type(&self) -> bool {
        match self {
            SymShape::Type | SymShape::Alias { .. } => true,
            SymShape::Template { type_like, .. } => *type_like,
            SymShape::TemplateParam(p) => !p.kind_is_type,
            _ => false,
        }
    }

    /// `isTemplate` (`Parser/Syntax.hs:701-704`).
    pub fn is_template(&self) -> bool {
        match self {
            SymShape::Template { .. } => true,
            SymShape::TemplateParam(p) => p.kind_is_template,
            _ => false,
        }
    }

    /// `isFunction` (`Parser/Syntax.hs:716-720`).
    pub fn is_function(&self) -> bool {
        match self {
            SymShape::Function => true,
            SymShape::Template { function, .. } => *function,
            _ => false,
        }
    }
}

/// `isType` (`Parser/Syntax.hs:676-699`) of a node: what the parser asks of
/// an expression it parsed (`expression`, `Parser.hs:823-828`) and the
/// passes of an operand (`checkInvalidType`, `TypeCheck.hs:180-185`).
/// [`SymShape::is_type`] is the same predicate of a declaration's shape.
pub fn is_type(arena: &crate::arena::ExpArena, id: ExpId) -> bool {
    use crate::exp::Exp;
    match &arena[id].exp {
        Exp::Auto
        | Exp::Void
        | Exp::Boolean
        | Exp::Float
        | Exp::Str
        | Exp::Const(_)
        | Exp::Reference(_)
        | Exp::Integer(_)
        | Exp::Unsigned(_)
        | Exp::ParamInt(_)
        | Exp::ParamUint(_)
        | Exp::Enum { .. }
        | Exp::Struct { .. }
        | Exp::Class { .. }
        | Exp::Union { .. }
        | Exp::Alias { .. }
        | Exp::TypeIdentifier(..)
        | Exp::Array { .. }
        | Exp::FunctionType { .. } => true,
        Exp::Template { decl, .. } => is_type(arena, *decl),
        Exp::TypeParam { kind, .. } | Exp::TemplateParam { kind, .. } => !is_type(arena, *kind),
        Exp::TemplateInstance { base, .. } => is_type(arena, *base),
        _ => false,
    }
}

/// A variable's written type, as `resolveTypeName`'s `typeName` reads it
/// (`Parser.hs:752-757`): the local form is the lowered type subtree, walked
/// lazily at each method call; the foreign form is the pre-walked qualified
/// name `kg-db` harvested from the defining module's sorted IR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeName {
    Local(ExpId),
    Key(SymbolPath),
}

/// A const variable's function-valued initializer — the `x@NamedValue{}` that
/// `freeFunctionSpecifier` splices in place of the callee (`Parser.hs:776-777`).
/// The local form is copied verbatim; the foreign form cannot be (the subtree
/// lives in another module's arena), so the callee stays unsorted and counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FnInit {
    Local(ExpId),
    Foreign,
}

/// One hit the imports offer for a qualified name — `findInImport`'s output
/// (`SymbolTable.hs:272-278`), flattened to what the sorter reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalHit {
    /// Identity for `nub` (`SymbolTable.hs:231`): two re-export chains
    /// reaching the same declaration are one hit, not an ambiguity.
    pub token: u64,
    /// `declScopeName` of the found declaration — what `makeQualified`
    /// materialises as prefix `ScopedName`s (`Desugar.hs:188-201`).
    pub decl_scope: SymbolPath,
    pub shape: SymShape,
    /// [`fingerprint`] of the declaration's tree in its home module, when the
    /// harvest saw one — `nub`'s structural equality across modules.
    pub fingerprint: Option<Fingerprint>,
}

/// The import half of `findSymbol` — `findInImports` (`SymbolTable.hs:263-270`),
/// including alias-qualifier stripping, which `kg_resolve::ModuleGraph` already
/// owns. `kg-db` implements this over a loaded program.
pub trait ImportSymbols {
    fn find(&self, key: &SymbolPath) -> Vec<ExternalHit>;

    /// `find(key)`'s answer met another hit in `nub` ([`SymTab::one`]), so
    /// the lowering read its hits' tokens and fingerprints — which it reads
    /// nowhere else. `kg-db`'s memo of a module's lowering compares those
    /// only for an answer this was said of: they move with a body edit
    /// above or inside the declaration, and an importer that never weighed
    /// two hits against each other did not read them.
    fn compared(&self, _key: &SymbolPath) {}
}

/// The empty import world: a self-contained file, or the oracle's `--no-base`.
#[derive(Debug)]
pub struct NoImports;

impl ImportSymbols for NoImports {
    fn find(&self, _key: &SymbolPath) -> Vec<ExternalHit> {
        Vec::new()
    }
}

/// Why one path stayed unsorted. Each is a case where producing a sorted
/// shape would be a guess rather than the reference's answer; `xtask ir` and
/// `kg dump ir` report the counts.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum UnsortedReason {
    /// A type-position name resolved to a value symbol. The reference's
    /// `typeSpecifier` fails the parse there (`unexpectedSymbol`,
    /// `Parser.hs:723`) and reparses down another grammar path our CST did
    /// not take, so there is no reference shape for this tree to copy.
    ValueInTypePosition,
    /// An unresolved name nested inside a type written in expression position
    /// (`typ' False`): the reference backtracks out of the *whole* type and
    /// restructures (`a[i]` becomes an array access), which a committed CST
    /// cannot follow.
    UnresolvedNestedTypeInExpr,
    /// A written `<…>` list on a segment the reference would not have parsed
    /// one for — no `template` keyword and a symbol that is not a template —
    /// leaving the angle text unconsumed there (`Parser.hs:690-695`).
    ArgsOnNonTemplate,
    /// `freeFunctionSpecifier`'s const-function substitution
    /// (`Parser.hs:776-777`) where the initializer subtree lives in another
    /// module's arena.
    ForeignConstFnInit,
}

/// The sorter's scoreboard, carried on [`crate::ModuleIr`].
#[derive(Clone, Default, Debug)]
pub struct SortStats {
    /// Whether this lowering ran with a symbol context at all.
    pub attempted: bool,
    /// Paths sorted into a reference shape.
    pub sorted: u32,
    /// Paths left as [`crate::Exp::Path`], each with its reason.
    pub unsorted: Vec<(FileRange, UnsortedReason)>,
}

/// One local declaration, indexed by the [`ItemId`]s stored in the scope
/// stack's name maps.
#[derive(Clone, Debug)]
pub struct SymData {
    pub name: Symbol,
    /// `declScopeName` — the qualifier at the declaration
    /// (`Parser/Syntax.hs:780`).
    pub decl_scope: SymbolPath,
    pub shape: SymShape,
    /// The declared node — what the reference stores as the table entry and
    /// what `nub` compares (`parseSymbol`, `SymbolTable.hs:194-214`). `None`
    /// for seeds, which have no completed tree yet.
    pub decl: Option<ExpId>,
}

/// A resolved name: where it lives decides how its facts are read.
#[derive(Clone, Debug)]
pub enum Hit {
    Local(ItemId),
    External(ExternalHit),
}

/// The incremental symbol table — `data SymbolTable`'s three lookup domains
/// (`SymbolTable.hs:51-56`), minus the scope stack itself, which stays on the
/// lowering (`Lower::scopes`) because both walks share it.
///
/// `Debug` is manual: `imports` is a `dyn` door with nothing to print.
pub struct SymTab<'a> {
    pub imports: &'a dyn ImportSymbols,

    pub syms: Vec<SymData>,
    /// `symbols (module_ t)` — the flat map `parseSymbol` grows as it goes
    /// (`addToModule`, `SymbolTable.hs:211`). Insert-overwrites like the
    /// reference's `HashMap.insert`.
    pub module_map: FxHashMap<SymbolPath, ItemId>,
    pub stats: SortStats,
}

impl<'a> SymTab<'a> {
    pub fn new(imports: &'a dyn ImportSymbols) -> SymTab<'a> {
        SymTab {
            imports,
            syms: Vec::new(),
            module_map: FxHashMap::default(),
            stats: SortStats { attempted: true, sorted: 0, unsorted: Vec::new() },
        }
    }

    pub fn get(&self, id: ItemId) -> &SymData {
        &self.syms[id.index()]
    }

    /// A new entry, not yet in any scope: the caller decides whether it is
    /// `declare`d (`parseSymbol`) or `seed`ed (`parseScoped`).
    pub fn push(&mut self, data: SymData) -> ItemId {
        let id = ItemId(self.syms.len() as u32);
        self.syms.push(data);
        id
    }

    /// `parseSymbol`'s module-map half (`addToModule`, `SymbolTable.hs:211`):
    /// the key is the *current* qualifier plus the name. The scope half is the
    /// caller's `scopes.declare`, whose redefinition answer is dropped — the
    /// sorter diagnoses nothing.
    pub fn file_symbol(&mut self, scopes: &mut ScopeStack, id: ItemId) {
        let data = &self.syms[id.index()];
        let key = data.decl_scope.child(data.name);
        let name = data.name;
        self.module_map.insert(key, id);
        let _ = scopes.declare(name, id);
    }

    /// `findSymbol` (`SymbolTable.hs:227-278`) over the incremental table.
    /// `None` covers both `Nothing` and the ambiguous case, which the
    /// reference reports and then treats as `Nothing` too (`:233-237`).
    ///
    /// The arena and interner feed the [`fingerprint`] half of `nub` — see
    /// [`SymTab::one`].
    pub fn find(
        &self,
        arena: &crate::ExpArena,
        interner: &Interner,
        scopes: &ScopeStack,
        qualifier: &[Symbol],
        name: Symbol,
    ) -> Option<Hit> {
        let (raw, asked) = if qualifier.is_empty() {
            self.find_in_scopes(scopes, name)
        } else {
            let key = SymbolPath::new(qualifier.iter().copied().chain([name]));
            self.find_in_symbols(scopes, key)
        };
        if raw.len() > 1 {
            self.imports.compared(&asked);
        }
        self.one(arena, interner, raw)
    }

    /// `findInScopes` (`SymbolTable.hs:256-259`): first scope wins, and the
    /// hit is *concatenated* with the imports' answer for the declaration's
    /// qualified name — a local/imported collision is ambiguous, never
    /// shadowed.
    /// With the hits, the one question put to the imports.
    fn find_in_scopes(&self, scopes: &ScopeStack, name: Symbol) -> (Vec<Hit>, SymbolPath) {
        for id in scopes.chain() {
            if let Some(&item) = scopes.get(id).names.get(&name) {
                let mut hits = vec![Hit::Local(item)];
                let qualified = self.syms[item.index()].decl_scope.child(name);
                hits.extend(self.imports.find(&qualified).into_iter().map(Hit::External));
                return (hits, qualified);
            }
        }
        self.find_in_symbols(scopes, SymbolPath::single(name))
    }

    /// `findInSymbols` (`SymbolTable.hs:261`): `lookupInScope` over the flat
    /// map, walking outward from the current qualifier, then the imports.
    fn find_in_symbols(&self, scopes: &ScopeStack, key: SymbolPath) -> (Vec<Hit>, SymbolPath) {
        let mut hits = Vec::new();
        if let Some(&item) = lookup_in_scope(&self.module_map, scopes.qualifier(), &key) {
            hits.push(Hit::Local(item));
        }
        hits.extend(self.imports.find(&key).into_iter().map(Hit::External));
        (hits, key)
    }

    /// `nub` then "exactly one" (`SymbolTable.hs:229-237`).
    ///
    /// `nub`'s equality is the symbol *tree* — `Note`'s `Eq` ignores the
    /// annotation (`Parser/Syntax.hs:594-599`) — so two structurally identical
    /// declarations are ONE symbol even when they come from different files.
    /// That is what makes a device override work: the root's `hardware.dsp`
    /// and the `device/mock` copy `base` re-exposes both declare `fmul32`'s
    /// locals, and every lookup of one would be ambiguous with the other if
    /// identity were positional. Identity here is the [`fingerprint`] of the
    /// declared tree; hits with no tree (seeds, unharvested imports) never
    /// collapse, which errs toward the unresolved shape.
    fn one(&self, arena: &crate::ExpArena, interner: &Interner, hits: Vec<Hit>) -> Option<Hit> {
        if hits.len() <= 1 {
            return hits.into_iter().next();
        }
        let mut distinct: Vec<(Hit, Option<u64>)> = Vec::with_capacity(hits.len());
        for h in hits {
            let fp = self.hit_fp(arena, interner, &h);
            if !distinct.iter().any(|(d, dfp)| same(d, *dfp, &h, fp)) {
                distinct.push((h, fp));
            }
        }
        match distinct.len() {
            1 => distinct.pop().map(|(h, _)| h),
            _ => None,
        }
    }

    fn hit_fp(&self, arena: &crate::ExpArena, interner: &Interner, h: &Hit) -> Option<u64> {
        match h {
            Hit::Local(id) => {
                self.syms[id.index()].decl.map(|d| fingerprint(arena, interner, d))
            }
            Hit::External(e) => e.fingerprint.as_ref().map(Fingerprint::get),
        }
    }

    pub fn shape<'h>(&'h self, hit: &'h Hit) -> &'h SymShape {
        match hit {
            Hit::Local(id) => &self.syms[id.index()].shape,
            Hit::External(e) => &e.shape,
        }
    }

    pub fn decl_scope<'h>(&'h self, hit: &'h Hit) -> &'h SymbolPath {
        match hit {
            Hit::Local(id) => &self.syms[id.index()].decl_scope,
            Hit::External(e) => &e.decl_scope,
        }
    }
}



/// One `nub` comparison: same declaration site, or same declared tree.
fn same(a: &Hit, afp: Option<u64>, b: &Hit, bfp: Option<u64>) -> bool {
    if afp.zip(bfp).is_some_and(|(x, y)| x == y) {
        return true;
    }
    match (a, b) {
        (Hit::Local(x), Hit::Local(y)) => x == y,
        (Hit::External(x), Hit::External(y)) => x.token == y.token,
        _ => false,
    }
}

/// The tree a declaration would `show` as, hashed — `nub`'s equality
/// (`SymbolTable.hs:229`), which compares symbol trees with notes ignored
/// (`Parser/Syntax.hs:594-599`). The position-free structural rendering is
/// exactly "the tree, notes ignored", so its hash stands in for tree equality
/// across arenas.
pub fn fingerprint(arena: &crate::ExpArena, interner: &Interner, id: ExpId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    crate::structure::render_exp(arena, interner, id).hash(&mut h);
    h.finish()
}

/// `getScopeQualifier`'s value rendered for messages/tests.
pub fn path_names(interner: &Interner, path: &SymbolPath) -> Vec<String> {
    path.segments().iter().map(|s| interner.resolve(*s).to_string()).collect()
}

impl std::fmt::Debug for SymTab<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymTab")
            .field("syms", &self.syms.len())
            .field("module_map", &self.module_map.len())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// harvesting a sorted module's exportable facts
// ---------------------------------------------------------------------------

/// [`spine_qualname`]'s last segment, where it is the node's own leaf —
/// read without the prefix. `None` where the path is empty, or where its
/// last segment is its prefix's (a leaf that is no name).
pub fn spine_last(arena: &crate::ExpArena, id: ExpId) -> Option<Symbol> {
    use crate::Exp;
    match &arena[id].exp {
        Exp::ScopedName { ident, .. } => spine_leaf_name(arena, *ident),
        Exp::Identifier(s) => Some(*s),
        Exp::TypeParam { name, .. } => Some(*name),
        Exp::TemplateInstance { base, .. }
        | Exp::QualifiedIdentifier(base)
        | Exp::NamedValue(base)
        | Exp::TypeIdentifier(_, base) => spine_last(arena, *base),
        _ => None,
    }
}

/// [`spine_qualname`]'s only segment, where it has exactly one — without
/// building the path, for a caller that asks it of every name node
/// (`intrinsics`, `Type.hs:1065-1081`).
pub fn spine_single(arena: &crate::ExpArena, id: ExpId) -> Option<Symbol> {
    // How many segments `spine_qualname` would give, and the first.
    fn push(seen: &mut (u32, Option<Symbol>), s: Symbol) {
        seen.0 += 1;
        seen.1.get_or_insert(s);
    }
    fn visit(arena: &crate::ExpArena, id: ExpId, seen: &mut (u32, Option<Symbol>)) {
        use crate::Exp;
        match &arena[id].exp {
            Exp::ScopedName { prefix, ident, .. } => {
                if let Some(p) = prefix {
                    visit(arena, *p, seen);
                }
                if let Some(s) = spine_leaf_name(arena, *ident) {
                    push(seen, s);
                }
            }
            Exp::Identifier(s) => push(seen, *s),
            Exp::TypeParam { scope, name, .. } => {
                for &s in scope.segments() {
                    push(seen, s);
                }
                push(seen, *name);
            }
            Exp::TemplateInstance { base, .. }
            | Exp::QualifiedIdentifier(base)
            | Exp::NamedValue(base)
            | Exp::TypeIdentifier(_, base) => visit(arena, *base, seen),
            _ => {}
        }
    }
    let mut seen = (0, None);
    visit(arena, id, &mut seen);
    if seen.0 == 1 { seen.1 } else { None }
}

/// `getQualifiedName` over a sorted name spine (`Parser/Syntax.hs:673,744-763`)
/// — free-standing so `kg-db` can read another module's IR with it.
pub fn spine_qualname(arena: &crate::ExpArena, id: ExpId) -> Vec<Symbol> {
    use crate::Exp;
    match &arena[id].exp {
        Exp::ScopedName { prefix, ident, .. } => {
            let mut out = prefix.map(|p| spine_qualname(arena, p)).unwrap_or_default();
            out.extend(spine_leaf_name(arena, *ident));
            out
        }
        Exp::Identifier(s) => vec![*s],
        Exp::TypeParam { scope, name, .. } => {
            let mut out: Vec<Symbol> = scope.segments().to_vec();
            out.push(*name);
            out
        }
        Exp::TemplateInstance { base, .. }
        | Exp::QualifiedIdentifier(base)
        | Exp::NamedValue(base)
        | Exp::TypeIdentifier(_, base) => spine_qualname(arena, *base),
        _ => Vec::new(),
    }
}

fn spine_leaf_name(arena: &crate::ExpArena, id: ExpId) -> Option<Symbol> {
    use crate::Exp;
    match &arena[id].exp {
        Exp::Identifier(s) => Some(*s),
        Exp::ScopedName { ident, .. } => spine_leaf_name(arena, *ident),
        Exp::TypeParam { name, .. } => Some(*name),
        Exp::TemplateInstance { base, .. }
        | Exp::QualifiedIdentifier(base)
        | Exp::NamedValue(base)
        | Exp::TypeIdentifier(_, base) => spine_leaf_name(arena, *base),
        _ => None,
    }
}

/// The cross-file half of one harvested declaration: its [`SymShape`] plus
/// the [`fingerprint`] `nub` compares.
#[derive(Clone, Debug)]
pub struct Harvested {
    pub shape: SymShape,
    pub fingerprint: Fingerprint,
}

/// A declaration's [`fingerprint`], hashed the first time it is read.
///
/// **Read rarely, and it was the dearest thing in a keystroke.** The harvest
/// keys every declaration a module makes — each parameter, each local — and
/// a fingerprint renders the declaration's whole tree, so a class's body is
/// rendered once for the class and again for every declaration inside it:
/// 18% of a keystroke on `risc-v.k` (`cargo xtask bench --filter
/// keystroke-phases` under `sample`, 2026-10-10), for the edited module
/// alone, every keystroke. It is read only where `nub` weighs two hits
/// against each other ([`SymTab::one`]), where a lowering's memo re-asks an
/// answer `nub` compared, and by `kg-db`'s exposure check. Deferred, it is
/// the same number: the harvested IR is never written again (`kg-db` keeps
/// it behind an `Arc` and the loop forks it), and the interner only grows.
#[derive(Clone)]
pub struct Fingerprint(Repr);

#[derive(Clone)]
enum Repr {
    Known(u64),
    Deferred { harvest: Arc<Deferred>, slot: u32 },
}

/// What a harvest's deferred fingerprints are computed from, and each one's
/// value once read.
struct Deferred {
    ir: Arc<crate::ModuleIr>,
    interner: Arc<Interner>,
    decls: Vec<ExpId>,
    values: Vec<std::sync::OnceLock<u64>>,
}

impl Fingerprint {
    pub fn known(value: u64) -> Fingerprint {
        Fingerprint(Repr::Known(value))
    }

    pub fn get(&self) -> u64 {
        match &self.0 {
            Repr::Known(v) => *v,
            Repr::Deferred { harvest, slot } => {
                let i = *slot as usize;
                *harvest.values[i].get_or_init(|| fingerprint(&harvest.ir.arena, &harvest.interner, harvest.decls[i]))
            }
        }
    }

    /// The value alone, read now: what a memo keeps, so that it holds no
    /// other module's IR alive.
    pub fn read(&self) -> Fingerprint {
        Fingerprint::known(self.get())
    }
}

impl PartialEq for Fingerprint {
    fn eq(&self, other: &Fingerprint) -> bool {
        self.get() == other.get()
    }
}

impl Eq for Fingerprint {}

impl std::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

/// The facts another module's sorter needs about this module's declarations,
/// read off its **sorted** IR and keyed by qualified name — the cross-file
/// half of what [`SymData`] carries locally. Insertion order makes the
/// `Template` node win over the declaration it wraps, the same first-wins the
/// parse-time map has (`parseSymbol` never sees the wrapped declaration,
/// `Parser.hs:373-375`): the wrapped node allocates first, the `Template`
/// second, and the later insert overwrites.
pub fn harvest_shapes(
    ir: &Arc<crate::ModuleIr>,
    interner: &Arc<Interner>,
) -> FxHashMap<SymbolPath, Harvested> {
    use crate::Exp;
    let arena = &ir.arena;
    // Each entry's shape and declaration; the fingerprints are deferred
    // ([`Fingerprint`]) over the declarations, in insertion order.
    let mut out: FxHashMap<SymbolPath, (SymShape, u32)> = FxHashMap::default();
    let mut decls: Vec<ExpId> = Vec::new();
    let mut slot = |id: ExpId| -> u32 {
        decls.push(id);
        u32::try_from(decls.len() - 1).expect("an arena's ids fit in u32")
    };
    let name_of = |id: ExpId| -> Option<Symbol> { spine_leaf_name(arena, id) };

    for (id, node) in arena.iter() {
        match &node.exp {
            Exp::Struct { scope, name, .. }
            | Exp::Union { scope, name, .. }
            | Exp::Class { scope, name, .. }
            | Exp::Enum { scope, name, .. } => {
                if let Some(n) = name_of(*name) {
                    out.insert(scope.child(n), (SymShape::Type, slot(id)));
                }
            }
            Exp::EnumConstant { scope, name, .. } => {
                if let Some(n) = name_of(*name) {
                    out.insert(scope.child(n), (SymShape::OtherValue, slot(id)));
                }
            }
            // `symbol param` (`Parser.hs:378`): parameters are table entries
            // too, and a re-exposed module's nested keys reach importers
            // (`filterExposed` keeps every key whose second component is
            // exposed, `SymbolTable.hs:163-164`), so a lookup can collide
            // with one and needs its tree to `nub` against.
            Exp::FuncParam { scope, name, .. } => {
                if let Some(n) = name_of(*name) {
                    out.insert(scope.child(n), (SymShape::OtherValue, slot(id)));
                }
            }
            Exp::Alias { scope, body, name } => {
                if let Some(n) = name_of(*name) {
                    let target = match &arena[*body].exp {
                        Exp::TypeIdentifier(_, payload) => {
                            Some(SymbolPath::new(spine_qualname(arena, *payload)))
                        }
                        _ => None,
                    };
                    out.insert(scope.child(n), (SymShape::Alias { target }, slot(id)));
                }
            }
            Exp::Function { scope, name, .. } | Exp::FunctionDecl { scope, name, .. } => {
                if let Some(n) = name_of(*name) {
                    out.insert(scope.child(n), (SymShape::Function, slot(id)));
                }
            }
            Exp::Variable { scope, ty, name, init, flags, .. } => {
                // Struct and union fields never enter any table
                // (`Parser.hs:299,463`).
                if *flags == kg_resolve::DeclareFlag::DeclareMember {
                    continue;
                }
                let Some(n) = name_of(*name) else { continue };
                let const_ty = matches!(arena[*ty].exp, Exp::Const(_));
                let type_name = variable_type_key(arena, *ty).map(TypeName::Key);
                let fn_init = init.and_then(|i| {
                    matches!(arena[i].exp, Exp::NamedValue(_)).then_some(FnInit::Foreign)
                });
                out.insert(scope.child(n), (SymShape::Variable { const_ty, type_name, fn_init }, slot(id)));
            }
            Exp::Template { decl, .. } => {
                let inner = &arena[*decl].exp;
                let (scope, name) = match inner {
                    Exp::Struct { scope, name, .. }
                    | Exp::Union { scope, name, .. }
                    | Exp::Class { scope, name, .. }
                    | Exp::Enum { scope, name, .. }
                    | Exp::Alias { scope, name, .. }
                    | Exp::Function { scope, name, .. }
                    | Exp::FunctionDecl { scope, name, .. } => (scope, *name),
                    _ => continue,
                };
                let Some(n) = name_of(name) else { continue };
                let function = matches!(
                    inner,
                    Exp::Function { .. } | Exp::FunctionDecl { .. }
                );
                let type_like = matches!(
                    inner,
                    Exp::Struct { .. }
                        | Exp::Union { .. }
                        | Exp::Class { .. }
                        | Exp::Enum { .. }
                        | Exp::Alias { .. }
                );
                out.insert(scope.child(n), (SymShape::Template { function, type_like }, slot(id)));
            }
            _ => {}
        }
    }
    let values = decls.iter().map(|_| std::sync::OnceLock::new()).collect();
    let harvest = Arc::new(Deferred { ir: ir.clone(), interner: interner.clone(), decls, values });
    out.into_iter()
        .map(|(key, (shape, slot))| {
            let fingerprint = Fingerprint(Repr::Deferred { harvest: harvest.clone(), slot });
            (key, Harvested { shape, fingerprint })
        })
        .collect()
}

/// `typeName`'s array unwrap over a stored type (`Parser.hs:752`): the named
/// payload's qualified name, or nothing — `const` and built-ins are opaque to
/// the receiver-type walk.
fn variable_type_key(arena: &crate::ExpArena, mut id: ExpId) -> Option<SymbolPath> {
    use crate::Exp;
    loop {
        match &arena[id].exp {
            Exp::Array { elem, .. } => id = *elem,
            Exp::TypeIdentifier(_, payload) => {
                return Some(SymbolPath::new(spine_qualname(arena, *payload)));
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    use super::{spine_last, spine_qualname, spine_single, NoImports};

    /// [`spine_single`] is [`spine_qualname`] of one segment, or nothing, at
    /// every node of a module whose names take each spine's shape — bare,
    /// qualified, a template instance, a type, a member's scope; and
    /// [`spine_last`], where it answers, is the path's last segment. Red
    /// with a path of two counted as its first, or with a qualified name's
    /// last read off its prefix.
    #[test]
    fn the_single_segment_is_the_qualified_names_only_one() {
        let text = "struct S\n{\n    uint8 a;\n}\n\nenum E : uint8\n{\n    A,\n    B\n}\n\n\
                    template <typename T>\ninline T id(T x)\n{\n    return x;\n}\n\n\
                    class C\n{\npublic:\n    inline void go()\n    {\n        assert(true);\n    }\n}\n\n\
                    inline void main()\n{\n    S s;\n    E e = E::A;\n    uint8 y = id<uint8>(1);\n    __print(\"a\");\n}\n";
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "fixture must parse clean: {:?}", parse.errors());
        let interner = Interner::new();
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = crate::lower_sorted(&file, FileId(0), &interner, &NoImports);
        let (mut single, mut longer) = (0, 0);
        for (id, _) in ir.arena.iter() {
            let path = spine_qualname(&ir.arena, id);
            let want = match path.as_slice() {
                [only] => Some(*only),
                _ => None,
            };
            assert_eq!(spine_single(&ir.arena, id), want, "{path:?}");
            if let Some(last) = spine_last(&ir.arena, id) {
                assert_eq!(path.last(), Some(&last), "{path:?}");
            }
            single += usize::from(want.is_some());
            longer += usize::from(path.len() > 1);
        }
        assert!(single > 5 && longer > 0, "the fixture holds both: {single} single, {longer} longer");
    }
}
