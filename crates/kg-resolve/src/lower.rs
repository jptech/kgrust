//! Owner-topic: **lowering** — walking one file's CST into an [`ItemTree`]:
//! declarations, the scopes they live in, the synthetic index counters, the
//! module header and the import list.
//!
//! Spec: `docs/spec/resolve.md` §11.3 (the per-file item tree), §3.1 (what
//! becomes a symbol and how it is keyed), §7 (why two passes are needed), §10
//! (the keying table).
//! Reference: `Symbols.hs:37-71`, `Parser.hs:108-241`, `SymbolTable.hs:183-225`.
//!
//! # Why this is a separate pass from parsing
//!
//! In the reference the *parser* needs a symbol oracle: `<` is ambiguous between
//! a template instantiation and a comparison, and the decision is made by looking
//! the name up in the symbol table mid-parse (`Parser.hs:645-681`,
//! `docs/spec/resolve.md` §7.1). Reproducing that literally would make every
//! file's parse depend on every transitive import's *contents*, which is
//! incompatible with incremental editing.
//!
//! So the split is: `kg_parser` parses purely, leaving `<` ambiguous;
//! [`lower_file`] builds an `ItemTree` with no cross-file input; and a later
//! **disambiguation pass** consumes the `ItemTree` plus the imported interfaces
//! and rewrites the ambiguous sites. `docs/spec/resolve.md` §11.3.
//!
//! This is possible because *every* scope in `Parser.hs:119-153` is introduced by
//! a syntactic construct — never by a name lookup — so the whole qualifier
//! algebra of §4.2 is computable from the CST alone.
//!
//! # The synthetic names, all generated here
//!
//! | name | source | cite |
//! |---|---|---|
//! | `?lambdaN` | `lambdaIndex` | `Parser.hs:434`, `SymbolTable.hs:216-225` |
//! | `$quasiN` | `scopeIndex` | `Parser.hs:131-134` |
//! | `$exportN` | `exportIndex` | `SymbolTable.hs:213` |
//! | `$` | function body scope | `Desugar.hs:173-174` |
//! | `$this` | second key for every class | `Symbols.hs:66`, `Parser/Syntax.hs:267-268` |
//! | `$errors` | every error node | `Symbols.hs:67`, `Parser/Syntax.hs:778` |
//!
//! # What this walk deliberately does not do
//!
//! * **Struct and union bodies are not entered.** `getSymbols''` replaces their
//!   children with `mempty` (`Symbols.hs:48-49`) and `memberVariable` is
//!   `uninitVar`, which is never wrapped in `symbol` (`Parser.hs:465`), so a field
//!   is in neither symbol map — field access resolves through the *type*
//!   (`tField`, `Type.hs:1236`). Skipping the body is also invisible to the scope
//!   algebra: only a `quasiScoped` pop can affect the parent, and the struct's own
//!   pop happens either way.
//! * **Name *references* are not resolved.** An `ItemTree` is a table of
//!   declarations; turning a `Path` into a [`DefId`] is
//!   [`crate::scope::find_symbol`]'s job, and needs the whole module graph.

use kg_base::{
    FileId, FileRange, FxHashMap, Interner, LineIndex, SmolStr, Symbol, SymbolPath, TextRange, TextSize,
};
use kg_diag::Diagnostic;
use kg_syntax::{AstNode, NodeOrToken, SyntaxKind, SyntaxNode, ast};

use crate::def::{DeclareFlag, DefData, DefId, DefKind, ItemId, SymbolsConfig};
use crate::module::{
    ExportItem, ImportDecl, ImportId, ImportNeeded, ModuleHeader, ModuleName, ModuleSource,
    Namespace,
};
use crate::scope::{
    ScopeData, ScopeId, ScopeKind, ScopeStack, redefinition_error,
    template_parameter_redefinition_error,
};

/// The synthetic name prefixes and literals the lowering must produce verbatim.
pub mod names {
    /// `Parser.hs:434` — `"?lambda" ++ show lambdaIndex`.
    pub const LAMBDA_PREFIX: &str = "?lambda";
    /// `Parser.hs:131-134` — `"$quasi" ++ show scopeIndex`.
    pub const QUASI_PREFIX: &str = "$quasi";
    /// `SymbolTable.hs:213` — `"$export" ++ show exportIndex`.
    pub const EXPORT_PREFIX: &str = "$export";
    /// `Desugar.hs:173-174` — `functionScope = (++ ["$"])`.
    pub const FUNCTION_SCOPE: &str = "$";
    /// `Parser/Syntax.hs:267-268` — the second key every `class` inserts.
    pub const THIS: &str = "$this";
    /// `Symbols.hs:67` — the single key every error node collapses to.
    pub const ERRORS: &str = "$errors";
}

/// One declaration (`docs/spec/resolve.md` §11.3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Item {
    pub scope: ScopeId,
    /// `declScopeName ++ [name]` — the global symbol map key (`Symbols.hs:70-71`).
    pub qualified_name: SymbolPath,
    pub kind: DefKind,
    /// True when this declaration is the **direct** target of a `template <...>`,
    /// i.e. the `a` of `Template params a` (`Parser.hs:373-375`) — *not* merely
    /// nested somewhere below one. The distinction is what makes
    /// [`SymbolsConfig::NoTemplate`] expressible: that config drops the `Template`
    /// node's key *and* `tail`s off its target's key, and keeps everything deeper
    /// (`Symbols.hs:59-60`).
    pub is_template: bool,
    /// The enclosing declaration, if any — a `Template`'s target points at the
    /// `Template`, a class member at its class. Not present in the reference,
    /// which re-derives the relation from the AST every time; here it is what
    /// makes [`SymbolsConfig::TemplateDecl`]'s "body is not traversed" a test
    /// rather than a second traversal.
    pub parent: Option<ItemId>,
    /// The declaration's name token, for `symbolLoc` in redefinition, ambiguity
    /// and shadow messages.
    pub range: FileRange,
    /// Where the reference reports this declaration: the note of the
    /// `Variable`/`Function`/`Class`/… node itself, which begins at the
    /// declaration's first token — after `static` for a variable
    /// (`kg-oracle --errors`, `basics.k::182`), never at a comment above it.
    /// [`Item::range`] is the *name*, which is what navigation wants.
    pub decl: FileRange,
    /// `exposable` (`SymbolTable.hs:168-169`): `isConst typ` for a `Variable`,
    /// `True` for everything else. Recorded here because it is the one fact
    /// `updateVisible` needs that only the CST has — the declared type does not
    /// survive into [`crate::module::ModuleSymbol`].
    pub exposable: bool,
}

impl Item {
    /// A struct or union field — an item for navigation, never a symbol
    /// (`Symbols.hs:48-49`, `Parser.hs:465`). Every consumer that means "the
    /// declarations `getSymbols` files" must skip these.
    pub fn is_field(&self) -> bool {
        self.kind == DefKind::Variable(DeclareFlag::DeclareMember)
    }

    /// `getScope e` = `declScopeName` (`Parser/Syntax.hs:780`) — the key with the
    /// declaration's own name dropped.
    ///
    /// Deliberately *not* called `scope`: [`Item::scope`] is the [`ScopeId`] this
    /// declaration was written in, which is a different thing. They can disagree —
    /// a lambda capture is keyed one component deeper than the scope it is written
    /// in (`Parser.hs:447`).
    pub fn scope_name(&self) -> SymbolPath {
        self.qualified_name.init()
    }

    /// Whether this declaration went through `parseSymbol` and so appears in the
    /// module's flat parse-time map (`SymbolTable.hs:203-211`).
    ///
    /// On top of [`DefKind::is_parse_time_symbol`] there is one item-level
    /// exception: the declaration wrapped by a `template <...>` is never
    /// `symbol`-ed on its own — `template def = symbol $ … Template params <$>
    /// noted (def params)` wraps the *`Template` node* (`Parser.hs:373-375`).
    pub fn is_declared(&self) -> bool {
        self.kind.is_parse_time_symbol() && !self.is_template
    }

    pub fn to_def_data(&self) -> DefData {
        DefData {
            kind: self.kind,
            qualified_name: self.qualified_name.clone(),
            range: self.range,
            is_template: self.is_template,
        }
    }
}

/// One file's declarations, in source order, with the scope tree they live in.
///
/// This is the unit that must stay *stable* under an edit to a function body, so
/// that a downstream `module_interface` query can stop invalidation at the module
/// boundary (`docs/spec/resolve.md` §11.2).
///
/// `items` is in **pre-order**: a declaration precedes everything declared inside
/// it. That is not cosmetic — `getSymbols''` conses a node's own entry ahead of
/// its children's (`symbol e . concat`, `Symbols.hs:44,50`) and the whole list is
/// then `reverse`d into a last-wins `HashMap.fromList`, so *first in pre-order
/// wins* (`docs/spec/resolve.md` §3.1). [`global_symbols`] depends on it.
#[derive(Clone, Debug, Default)]
pub struct ItemTree {
    /// `ItemId` is the index.
    pub items: Vec<Item>,
    /// `ScopeId` is the index; `scopes[0]` is the file scope.
    pub scopes: Vec<ScopeData>,
    pub module_header: Option<ModuleHeader>,
    /// **In source order.** See [`crate::module::ImportId`] for why the reference's
    /// reversed storage must not be copied.
    pub imports: Vec<ImportDecl>,
    /// Redefinitions found while building the scope tree — `parseSymbol`'s
    /// immediate, hard error (`SymbolTable.hs:200-201`) and the template
    /// parameter list's own check (`Parser.hs:352-359`). These are the only
    /// diagnostics that are decidable from one file alone.
    pub diagnostics: Vec<Diagnostic>,
    /// `any exported symbols` (`SymbolTable.hs:149,175-181`): the file contains an
    /// `export T;` or a `[[reset]]` function, so importing it is never redundant.
    /// Computed here because `[[reset]]` is an attribute, and attributes do not
    /// survive into the item tree.
    pub has_export_or_reset: bool,
    /// The name [`lower_file_named`] was given — what `symbolLoc`
    /// (`Error.hs:108-112`) prints in front of `:line:col` when another
    /// file's message points at a declaration here.
    pub display_name: Option<String>,
}

impl ItemTree {
    pub fn get(&self, id: ItemId) -> &Item {
        &self.items[id.index()]
    }

    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.get(id.index())
    }

    pub fn scope(&self, id: ScopeId) -> Option<&ScopeData> {
        self.scopes.get(id.index())
    }

    /// `hasErrorSymbol = HashMap.member ["$errors"]` (`Symbols.hs:31-35`) — the
    /// pipeline's "did anything fail" flag.
    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|i| i.kind == DefKind::Error)
    }

    /// `symbols (module_ t)` (`SymbolTable.hs:211`): the module's **flat
    /// parse-time** map, keyed `scopeQualifier ++ [name]`.
    ///
    /// This is *not* [`global_symbols`] restricted to one file. Two differences,
    /// both from the reference:
    ///
    /// * an `export T;` **is** here, under `$exportN` (`SymbolTable.hs:213`), and
    ///   is not in `getSymbols` at all — `updateVisible`'s
    ///   `exported ExportType{} = True` (`SymbolTable.hs:175`) reads *this* map;
    /// * a lambda capture and a template parameter are **not** here, because
    ///   neither goes through `parseSymbol` (`Parser.hs:441,362`), while both
    ///   (captures, at least) are in `getSymbols`.
    ///
    /// Last-wins on duplicate keys, matching `insertSymbol = HashMap.insert`
    /// (`Symbols.hs:73`) applied left to right.
    pub fn module_symbols(&self) -> FxHashMap<SymbolPath, ItemId> {
        let mut map = FxHashMap::default();
        for (i, item) in self.items.iter().enumerate() {
            if item.is_declared() {
                map.insert(item.qualified_name.clone(), ItemId(i as u32));
            }
        }
        map
    }

    /// The half of the reference's `Module` record that never leaves the file —
    /// the bridge from this walk to [`crate::module::ModuleGraph::add_module`].
    pub fn module_content(&self) -> crate::module::ModuleContent {
        let symbols = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_declared())
            .map(|(i, item)| {
                (
                    item.qualified_name.clone(),
                    crate::module::ModuleSymbol {
                        item: ItemId(i as u32),
                        exposable: item.exposable,
                    },
                )
            })
            .collect();
        crate::module::ModuleContent {
            header: self.module_header.clone().unwrap_or_default(),
            symbols,
            has_export_or_reset: self.has_export_or_reset,
        }
    }
}

/// The whole program's item trees, addressable by [`DefId`].
///
/// `trees` is in [`crate::module::ModuleGraph::files`] order — post-order DFS of
/// the import graph, base library first (`Parser/IO.hs:96`, `Main.hs:70,76`) —
/// which is the order duplicate-key precedence depends on.
#[derive(Copy, Clone, Debug)]
pub struct Program<'a> {
    pub trees: &'a [(ModuleSource, ItemTree)],
}

impl<'a> Program<'a> {
    pub fn new(trees: &'a [(ModuleSource, ItemTree)]) -> Program<'a> {
        Program { trees }
    }

    pub fn tree(&self, module: ModuleSource) -> Option<&'a ItemTree> {
        self.trees.iter().find(|(m, _)| *m == module).map(|(_, t)| t)
    }

    pub fn item(&self, def: DefId) -> Option<&'a Item> {
        self.tree(def.module)?.item(def.item)
    }

    /// Every declaration in the program, in `trees` × pre-order order.
    pub fn items(&self) -> impl Iterator<Item = (DefId, &'a Item)> {
        self.trees.iter().flat_map(|(module, tree)| {
            tree.items
                .iter()
                .enumerate()
                .map(move |(i, item)| (DefId::new(*module, ItemId(i as u32)), item))
        })
    }
}

/// **The entry point.** Walk one parsed file into an [`ItemTree`].
///
/// The scope-pushing table is `docs/spec/resolve.md` §4.2 (from `Parser.hs`),
/// transcribed construct by construct:
///
/// | construct | scopes pushed, outermost first | line |
/// |---|---|---|
/// | `module A.B` header | `Module` (`@A@B`) | 189 |
/// | `using T = …;` | `Unscoped` around the RHS type | 276 |
/// | `enum E : t { … }` | `Named` (`E`), then one `Unscoped` per constant after the first | 284, 291 |
/// | `struct` / `union` | `Quasi` | 299, 305 |
/// | `class C` | `Unscoped` (self), then `Named` (`C`) | 335-336 |
/// | template parameter list | `Unscoped` | 363 |
/// | function definition | `Unscoped` (template params), `Unscoped` (self), `Named` (`f`), `Function` (`$`) | 408-424 |
/// | lambda | `Named` (`?lambdaN`), then `Function` (`$`) | 440, 444 |
/// | function **type** | `Quasi` around the parameter list | 551 |
/// | `{ … }` statement block | `Local` | 469 |
/// | `static if` braced branch | `Quasi` | 467 |
/// | `for` / `static for` | `Local` | 584, 591 |
/// | `case` / `default` body | `Local` | 602-603 |
///
/// Only the node kinds of `Symbols.hs:52-68` become [`Item`]s.
pub fn lower_file(
    source: ModuleSource,
    file: &ast::SourceFile,
    interner: &Interner,
) -> ItemTree {
    lower_file_named(source, file, interner, None)
}

/// [`lower_file`] for text that has a display name — the path a message such
/// as `See previous declaration at <name>:<line>:<col>` prints.
pub fn lower_file_named(
    source: ModuleSource,
    file: &ast::SourceFile,
    interner: &Interner,
    name: Option<&str>,
) -> ItemTree {
    let file_id = match source {
        ModuleSource::File(id) => id,
        // The two synthetic modules are generated text with no on-disk home
        // (`Parser/IO.hs:101-102`).
        ModuleSource::Cmdargs | ModuleSource::Options => FileId::SYNTHETIC,
    };
    let mut lo = Lowering::new(interner, file_id, name.map(str::to_owned));
    if name.is_some() {
        lo.text = file.syntax().text().to_string();
    }

    // `moduleDecl` runs `updateModule` *before* `scoped ident declarations`
    // (`Parser.hs:188-189`), so the export list is known before any import.
    let header = file.module_decl().map(|m| module_header(&m, interner));
    let module_scope = header.as_ref().and_then(|h| h.namespace.as_ref()).map(|ns| {
        lo.scopes.push_at(interner, ScopeKind::Module, Some(ns.0), file.syntax().text_range());
    });

    let imports = lower_imports(file, file_id, interner);

    for item in file.items() {
        lo.walk(item.syntax());
    }

    // Every `ErrorF` node collapses to the one key `["$errors"]`, because
    // `getScope ErrorF{} = []` (`Symbols.hs:67`, `Parser/Syntax.hs:778`).
    if has_syntax_error(file.syntax()) {
        let key = SymbolPath::single(interner.intern(names::ERRORS));
        let range = file.syntax().text_range();
        lo.emit_key(key, DefKind::Error, range);
    }

    if module_scope.is_some() {
        lo.scopes.pop(interner);
    }

    ItemTree {
        display_name: lo.name.clone(),
        items: lo.items,
        scopes: lo.scopes.into_scopes(),
        module_header: header,
        imports,
        diagnostics: lo.diagnostics,
        has_export_or_reset: lo.has_export_or_reset,
    }
}

/// The global symbol map — `getSymbols'` (`Symbols.hs:26,37-71`).
///
/// Built by concatenating `ItemTree`s in [`crate::module::ModuleGraph::files`]
/// order with **first-wins** on duplicate keys. That is not an accident of
/// `HashMap`: `HashMap.fromList` is last-wins and the per-file list is
/// `reverse`d, and `HashMap.unions` is left-biased, so *the earlier AST in the
/// merged program list wins* and *a parent beats its descendants*
/// (`Symbols.hs:41,44,50`, `docs/spec/resolve.md` §3.1).
///
/// The parent-beats-descendant rule is exactly what makes a `template <…> struct
/// Foo` resolve to the `Template` node rather than to the `struct`: both are
/// filed under the same key, and `Template` is consed first.
///
/// `docs/spec/resolve.md` §11.5: build this **once per config**, not the 15+
/// times the reference does (`docs/spec/resolve.md` §3.2 counts them). The
/// rebuild-per-pass is an artefact of purity; what is semantic is that later
/// passes observe a tree earlier passes rewrote.
pub fn global_symbols(
    config: SymbolsConfig,
    trees: &[(ModuleSource, ItemTree)],
    interner: &Interner,
) -> FxHashMap<SymbolPath, DefId> {
    let this = interner.intern(names::THIS);
    let mut map: FxHashMap<SymbolPath, DefId> = FxHashMap::default();
    for (module, tree) in trees {
        for (i, item) in tree.items.iter().enumerate() {
            if !in_config(config, tree, item) {
                continue;
            }
            let def = DefId::new(*module, ItemId(i as u32));
            map.entry(item.qualified_name.clone()).or_insert(def);
            // `symbol e@Class{} = insert e . insert' e [getName e, thisName]`
            // (`Symbols.hs:66`) — two keys for one node; the second backs
            // `infer x@ThisF{}` (`Type.hs:1269-1272`).
            if item.kind == DefKind::Class {
                map.entry(item.qualified_name.child(this)).or_insert(def);
            }
        }
    }
    map
}

/// The three `SymbolsConfig` rules of `Symbols.hs:46-60`, as a membership test.
fn in_config(config: SymbolsConfig, tree: &ItemTree, item: &Item) -> bool {
    // `symbol _ = id` — `TemplateParam` and `ExportType` have no case at all.
    if !item.kind.is_global_symbol() {
        return false;
    }
    // `update e@Struct{} = symbol e . const mempty` (`Symbols.hs:48-49`): a
    // struct's or union's fields contribute nothing to the map.
    if item.is_field() {
        return false;
    }
    match config {
        SymbolsConfig::AllSymbols => true,
        // `symbol e@Template{} | NoTemplate == config = tail`: the `Template`
        // contributes nothing *and* removes the first entry its child produced,
        // which is the wrapped declaration's own key. Everything deeper survives.
        SymbolsConfig::NoTemplate => item.kind != DefKind::Template && !item.is_template,
        // `update e@Template{} | TemplateDecl == config = symbol e . const mempty`:
        // the `Template`'s own key, and nothing below it.
        SymbolsConfig::TemplateDecl => !has_template_ancestor(tree, item),
    }
}

fn has_template_ancestor(tree: &ItemTree, item: &Item) -> bool {
    let mut parent = item.parent;
    while let Some(id) = parent {
        let Some(p) = tree.item(id) else { return false };
        if p.kind == DefKind::Template {
            return true;
        }
        parent = p.parent;
    }
    false
}

/// Whether a declaration must be declared *before* use, or may be used before
/// declaration (`docs/spec/resolve.md` §7.1-§7.2).
///
/// Parse-time lookups (the `<` disambiguation and `::` chains) see only what the
/// scope stack has already collected, so those need declaration order.
/// Post-parse lookups go through `global_symbols`, which is order-independent
/// within a file — that is what makes forward references inside a module work.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DeclarationOrder {
    /// Needed by the parser's symbol oracle.
    BeforeUse,
    /// Resolved post-parse through the global map.
    AnyOrder,
}

/// The `ScopeStack` a lowering walk threads through the tree. Re-exported here so
/// an implementer of [`lower_file`] has one obvious place to start.
pub type LoweringScopes = ScopeStack;

// ---------------------------------------------------------------------------
// the walk
// ---------------------------------------------------------------------------

struct Lowering<'a> {
    interner: &'a Interner,
    file: FileId,
    /// The display name of `file`, for a message that embeds a location
    /// (`symbolLoc`, `Error.hs:108-112`); `None` for text with no name.
    name: Option<String>,
    /// Built on the first message that needs a line and column.
    line_index: Option<LineIndex>,
    text: String,
    /// The declaration being walked, for [`Item::decl`].
    current_decl: Option<TextRange>,
    scopes: ScopeStack,
    items: Vec<Item>,
    diagnostics: Vec<Diagnostic>,
    /// The enclosing declaration, for [`Item::parent`].
    parent: Option<ItemId>,
    /// Which `variable` combinator we are inside (`Parser.hs:461-465`).
    var_flag: DeclareFlag,
    /// Set by a `template <…>` and taken by the declaration it wraps, which
    /// re-seeds the parameters into its own scope (`Parser.hs:276,299,336,408`).
    /// `Some` also means "you are a template's target: do not `parseSymbol`
    /// yourself".
    template_target: Option<TemplateTarget>,
    /// `exported` (`SymbolTable.hs:175-181`).
    has_export_or_reset: bool,
}

/// What a `template <…>` hands to the declaration it wraps.
struct TemplateTarget {
    /// The `Template` node's own item — what a self-reference inside the body
    /// must resolve to (`Parser.hs:327,429`).
    item: ItemId,
    /// The parameters, in source order, to be re-seeded wherever the wrapped
    /// declaration puts them (`Parser.hs:276,299,336,408`).
    params: Vec<(Symbol, ItemId)>,
}

impl<'a> Lowering<'a> {
    fn new(interner: &'a Interner, file: FileId, name: Option<String>) -> Lowering<'a> {
        Lowering {
            interner,
            file,
            name,
            line_index: None,
            text: String::new(),
            current_decl: None,
            scopes: ScopeStack::new(),
            items: Vec::new(),
            diagnostics: Vec::new(),
            parent: None,
            // A file with no `module` declares at the unqualified global scope
            // (`globalVariable`, `Parser.hs:461`).
            var_flag: DeclareFlag::DeclareGlobal,
            template_target: None,
            has_export_or_reset: false,
        }
    }

    // --- item construction -------------------------------------------------

    fn emit_key(&mut self, key: SymbolPath, kind: DefKind, range: TextRange) -> ItemId {
        let id = ItemId(self.items.len() as u32);
        let decl = FileRange::new(self.file, self.current_decl.unwrap_or(range));
        self.items.push(Item {
            decl,
            scope: self.scopes.current(),
            qualified_name: key,
            kind,
            is_template: false,
            parent: self.parent,
            range: FileRange::new(self.file, range),
            // `exposable _ = True` for everything that is not a `Variable`
            // (`SymbolTable.hs:169`); `var_decl` overrides it.
            exposable: true,
        });
        id
    }

    /// `insert e = insert' e [getName e]` with `getScope e = declScopeName`
    /// (`Symbols.hs:70-71`) — the key is the *current* scope qualifier plus the
    /// name, captured at the moment the declaration is reached.
    fn emit(&mut self, name: Symbol, kind: DefKind, range: TextRange) -> ItemId {
        let key = self.scopes.qualifier().child(name);
        self.emit_key(key, kind, range)
    }

    /// [`Lowering::emit`] for a parameter or an enum constant, noted at
    /// itself: `symbol param`, `notedWithDocs` and `symbolWithDocs` note a
    /// function's, a lambda's and a template's parameter and an enum's
    /// constant from its own first token (`Parser.hs:290,361-365,377-379`),
    /// so `f(uint32 a, bool a)` is the redefinition at `bool a`, naming
    /// `uint32 a`, a class member colliding with `template <auto N>`'s `N`
    /// names the `auto`, and `E1 = -1, E1` is at the second `E1`, naming
    /// the first (`kg-oracle --errors`, 2026-10-10; difffuzz [18], [24]) —
    /// where the walk's current declaration is the function, template or
    /// enum.
    fn emit_param(&mut self, name: Symbol, kind: DefKind, range: TextRange, param: &SyntaxNode) -> ItemId {
        let saved = self.current_decl.replace(kg_syntax::trimmed_range(param));
        let id = self.emit(name, kind, range);
        self.current_decl = saved;
        id
    }

    /// `parseSymbol` (`SymbolTable.hs:194-206`).
    fn declare(&mut self, name: Symbol, id: ItemId) {
        if let Err(previous) = self.scopes.declare(name, id) {
            // `symbolError result` reports at the *new* declaration and names
            // the previous one's location (`SymbolTable.hs:200-201`).
            let at = self.items[id.index()].decl;
            let before = self.items[previous.index()].decl;
            let text = self.interner.resolve(name);
            let location = self.location(before.range.start());
            self.diagnostics.push(redefinition_error(at, &text, before, location.as_deref()));
        }
    }

    /// `symbolLoc` (`Error.hs:108-112`): `name:line:col`, 1-based, or `None`
    /// when the text has no name to print.
    fn location(&mut self, offset: TextSize) -> Option<String> {
        let name = self.name.as_deref()?;
        let index = self.line_index.get_or_insert_with(|| LineIndex::new(&self.text));
        let pos = index.line_col(offset);
        Some(format!("{name}:{}:{}", pos.line + 1, pos.col + 1))
    }

    fn mark_template_target(&mut self, id: ItemId) {
        self.items[id.index()].is_template = true;
    }

    fn name_of(&self, name: Option<ast::Name>) -> Option<(Symbol, TextRange)> {
        let token = name?.ident_token()?;
        Some((self.interner.intern(token.text()), token.text_range()))
    }

    fn name_ref_of(&self, name: Option<ast::NameRef>) -> Option<(Symbol, TextRange)> {
        let token = name?.ident_token()?;
        Some((self.interner.intern(token.text()), token.text_range()))
    }

    fn push(&mut self, kind: ScopeKind, name: Option<Symbol>, node: &SyntaxNode) {
        self.scopes.push_at(self.interner, kind, name, node.text_range());
    }

    fn pop(&mut self) {
        self.scopes.pop(self.interner);
    }

    fn seed_all(&mut self, params: &[(Symbol, ItemId)]) {
        for (name, id) in params {
            self.scopes.seed(*name, *id);
        }
    }

    // --- dispatch ----------------------------------------------------------

    fn walk_children(&mut self, node: &SyntaxNode) {
        for child in node.children() {
            self.walk(&child);
        }
    }

    fn walk_opt(&mut self, node: Option<impl AstNode>) {
        if let Some(n) = node {
            self.walk(n.syntax());
        }
    }

    fn walk(&mut self, node: &SyntaxNode) {
        let saved = self.current_decl;
        if let Some(decl) = decl_range(node) {
            self.current_decl = Some(decl);
        }
        self.walk_dispatch(node);
        self.current_decl = saved;
    }

    fn walk_dispatch(&mut self, node: &SyntaxNode) {
        match node.kind() {
            SyntaxKind::CLASS_DECL => self.class_decl(node),
            SyntaxKind::STRUCT_DECL => self.aggregate_decl(node, DefKind::Struct),
            SyntaxKind::UNION_DECL => self.aggregate_decl(node, DefKind::Union),
            SyntaxKind::ENUM_DECL => self.enum_decl(node),
            SyntaxKind::FN_DECL => self.function(node, DefKind::Function),
            SyntaxKind::FN_PROTO => self.function(node, DefKind::FunctionDecl),
            SyntaxKind::VAR_DECL => self.var_decl(node),
            SyntaxKind::ALIAS_DECL => self.alias_decl(node),
            SyntaxKind::TEMPLATE_DECL => self.template_decl(node),
            SyntaxKind::EXPORT_DECL => self.export_decl(node),
            SyntaxKind::LAMBDA_EXPR => self.lambda(node),
            SyntaxKind::BLOCK_STMT => self.scoped_block(node, ScopeKind::Local),
            SyntaxKind::FOR_STMT | SyntaxKind::STATIC_FOR_STMT => self.for_stmt(node),
            SyntaxKind::SWITCH_CASE | SyntaxKind::SWITCH_DEFAULT => self.switch_case(node),
            SyntaxKind::STATIC_IF_STMT => self.static_if(node),
            SyntaxKind::FN_TYPE => self.fn_type(node),
            _ => self.walk_children(node),
        }
    }

    // --- declarations ------------------------------------------------------

    /// `class_` / `class'` (`Parser.hs:316-321,331-339`).
    fn class_decl(&mut self, node: &SyntaxNode) {
        let template = self.template_target.take();
        let decl = ast::ClassDecl::cast(node.clone()).expect("kind checked");
        let Some((name, range)) = self.name_of(decl.name()) else {
            return self.walk_children(node);
        };

        let id = self.emit(name, DefKind::Class, range);
        if template.is_some() {
            self.mark_template_target(id);
        }

        // `unscopedWith symbols` (`Parser.hs:335`). `symbols` is the *lookahead*
        // pre-pass's copy of the class plus its type/template/variable members
        // (`Parser.hs:317-321`) — a parse-time device for forward references
        // among members (`docs/spec/resolve.md` §7.2 item 4). Only the class
        // itself is seeded here: a member's forward reference resolves post-parse
        // through `lookupInScope`, which is order-independent.
        //
        // For a `classTemplate` the seeded `x` is the `Template` node
        // (`Parser.hs:327`), not the class, so a self-reference inside the body
        // still reads as a template.
        self.push(ScopeKind::Unscoped, None, node);
        let self_symbol = template.as_ref().map_or(id, |t| t.item);
        self.scopes.seed(name, self_symbol);
        // `scopedWith params ident` (`Parser.hs:336`) — the template parameters
        // live *in the class scope*, so a member may collide with one.
        self.push(ScopeKind::Named, Some(name), node);
        self.seed_all(template.as_ref().map_or(&[][..], |t| &t.params));

        let saved_parent = self.parent.replace(id);
        let saved_flag = std::mem::replace(&mut self.var_flag, DeclareFlag::DeclareClass);
        if let Some(list) = decl.decl_list() {
            for item in list.items() {
                self.walk(item.syntax());
            }
        }
        self.var_flag = saved_flag;
        self.parent = saved_parent;

        self.pop();
        self.pop();
        if template.is_none() {
            self.declare(name, id);
        }
    }

    /// `struct'` / `union'` (`Parser.hs:298-305`). The body is not entered — see
    /// the module docs.
    fn aggregate_decl(&mut self, node: &SyntaxNode, kind: DefKind) {
        let template = self.template_target.take();
        let name = ast::StructDecl::cast(node.clone())
            .and_then(|d| d.name())
            .or_else(|| ast::UnionDecl::cast(node.clone()).and_then(|d| d.name()));
        let Some((name, range)) = self.name_of(name) else {
            return self.walk_children(node);
        };

        let id = self.emit(name, kind, range);
        if template.is_some() {
            self.mark_template_target(id);
        }
        // `quasiScopedWith params (braces (many uninitVar))` (`Parser.hs:299`,
        // `:463-465`). The fields are walked for the *item tree* — navigation,
        // hover and the outline need a declaration to land on — and for nothing
        // else: `uninitVar` never calls `symbol`, so a field is not declared
        // into any scope, and `getSymbols` files a `Struct`/`Union` with
        // `const mempty` (`Symbols.hs:48-49`), so [`in_config`] keeps every
        // field out of the global map. Only `VAR_DECL` children are visited;
        // anything else in a struct body is a parse error the reference's
        // grammar never reaches.
        self.push(ScopeKind::Quasi, None, node);
        self.seed_all(template.as_ref().map_or(&[][..], |t| &t.params));
        let body = ast::StructDecl::cast(node.clone())
            .and_then(|d| d.decl_list())
            .or_else(|| ast::UnionDecl::cast(node.clone()).and_then(|d| d.decl_list()));
        if let Some(body) = body {
            let saved_parent = self.parent.replace(id);
            let saved_flag = std::mem::replace(&mut self.var_flag, DeclareFlag::DeclareMember);
            for field in body.items() {
                if field.syntax().kind() == SyntaxKind::VAR_DECL {
                    self.walk(field.syntax());
                }
            }
            self.var_flag = saved_flag;
            self.parent = saved_parent;
        }
        self.pop();
        if template.is_none() {
            self.declare(name, id);
        }
    }

    /// `enum` (`Parser.hs:282-292`).
    fn enum_decl(&mut self, node: &SyntaxNode) {
        let template = self.template_target.take();
        let decl = ast::EnumDecl::cast(node.clone()).expect("kind checked");
        let Some((name, range)) = self.name_of(decl.name()) else {
            return self.walk_children(node);
        };

        let id = self.emit(name, DefKind::Enum, range);
        if template.is_some() {
            self.mark_template_target(id);
        }
        // `Enum <$> getScopeQualifier <*> pure ident <* colon <*> typ <*> scoped
        // ident (…)`: the underlying type is parsed *outside* the enum's scope.
        self.walk_opt(decl.ty());

        self.push(ScopeKind::Named, Some(name), node);
        let saved_parent = self.parent.replace(id);

        // `xs <- unscopedWith [x] (comma >> enumConsts' scope)` (`Parser.hs:291`):
        // one nested scope per constant, each seeded with the *previous* one. The
        // nesting is why `enum E { A, B, A }` is accepted while `enum E { A, A }`
        // is a redefinition — `parseSymbol` only ever consults the innermost
        // scope (`SymbolTable.hs:198`).
        let mut previous: Option<(Symbol, ItemId)> = None;
        let mut pushed = 0usize;
        for variant in enum_variants(node) {
            if let Some((pname, pid)) = previous {
                self.push(ScopeKind::Unscoped, None, node);
                pushed += 1;
                self.scopes.seed(pname, pid);
            }
            // The value expression is parsed before `parseSymbol` runs, so the
            // constant being defined is not yet visible to itself.
            self.walk_opt(variant.expr());
            if let Some((vname, vrange)) = self.name_of(variant.name()) {
                let vid = self.emit_param(vname, DefKind::EnumConstant, vrange, variant.syntax());
                self.declare(vname, vid);
                previous = Some((vname, vid));
            }
        }
        for _ in 0..pushed {
            self.pop();
        }

        self.parent = saved_parent;
        self.pop();
        if template.is_none() {
            self.declare(name, id);
        }
    }

    /// `functionDef` (`Parser.hs:406-424`), for both `FnDecl` (with a body) and
    /// `FnProto` (`FunctionDecl`, `Parser.hs:420-421`).
    fn function(&mut self, node: &SyntaxNode, kind: DefKind) {
        let template = self.template_target.take();
        let (name_node, param_list, ret, body) = match kind {
            DefKind::Function => {
                let f = ast::FnDecl::cast(node.clone()).expect("kind checked");
                (f.name(), f.param_list(), f.ret(), f.body())
            }
            _ => {
                let f = ast::FnProto::cast(node.clone()).expect("kind checked");
                (f.name(), f.param_list(), f.ret(), None)
            }
        };
        let Some((name, range)) = self.name_of(name_node) else {
            return self.walk_children(node);
        };

        // `exported (Function _ _ attr _ _ _ _ _) | any reset (unfix attr)`
        // (`SymbolTable.hs:176-181`) — a `[[reset]]` function makes importing the
        // module non-redundant even if nothing in it is ever named.
        if kind == DefKind::Function && has_attr(node, "reset") {
            self.has_export_or_reset = true;
        }

        let id = self.emit(name, kind, range);
        if template.is_some() {
            self.mark_template_target(id);
        }
        let saved_parent = self.parent.replace(id);

        // `unscopedWith templateParams` (`Parser.hs:408`).
        self.push(ScopeKind::Unscoped, None, node);
        self.seed_all(template.as_ref().map_or(&[][..], |t| &t.params));
        // The return type is parsed here, inside the template parameter scope.
        self.walk_opt(ret);

        // `unscopedWith (maybeToList self)` (`Parser.hs:414`) — `self` is the
        // lookahead copy of the enclosing *function template*, and is `Nothing`
        // for a plain function (`Parser.hs:426`), which is why a non-template
        // recursive call resolves post-parse instead. It is the `Template` node
        // (`functionTemplate`, `Parser.hs:428-430`), not the function, so that a
        // recursive `f<…>` inside the body still sees a template.
        self.push(ScopeKind::Unscoped, None, node);
        if let Some(t) = &template {
            self.scopes.seed(name, t.item);
        }

        // `scoped ident` (`Parser.hs:415`) — parameters are keyed under the
        // function name, the body under `f/$`.
        self.push(ScopeKind::Named, Some(name), node);
        if let Some(list) = param_list {
            for param in list.params() {
                self.walk_opt(param.ty());
                if let Some((pname, prange)) = self.name_of(param.name()) {
                    let pid = self.emit_param(pname, DefKind::FuncParam, prange, param.syntax());
                    self.declare(pname, pid);
                }
            }
        }

        if let Some(body) = body {
            // `functionScoped $ braces $ many statementOrDecl` (`Parser.hs:424`):
            // the braces belong to the function scope itself, so this `BlockStmt`
            // is *not* also a `localScoped` nested scope. The scope's range is
            // the body's: the signature is outside it, so a return type
            // written as an alias the body declares is not in scope
            // (`using.k::29`).
            self.push(ScopeKind::Function, None, body.syntax());
            let saved_flag = std::mem::replace(&mut self.var_flag, DeclareFlag::DeclareLocal);
            self.walk_children(body.syntax());
            self.var_flag = saved_flag;
            self.pop();
        }

        self.pop();
        self.pop();
        self.pop();
        self.parent = saved_parent;
        if template.is_none() {
            self.declare(name, id);
        }
    }

    /// `variable` / `uninitVar` / `uninitConst` (`Parser.hs:449-465`).
    fn var_decl(&mut self, node: &SyntaxNode) {
        let decl = ast::VarDecl::cast(node.clone()).expect("kind checked");

        // `defaultInitialization` (`Parser.hs:314`) is a `DefaultInitialization`
        // node, not a `Variable`, and declares nothing.
        if decl.default_kw_token().is_some() {
            return self.walk_children(node);
        }

        let flags = if decl.static_kw_token().is_some() {
            DeclareFlag::DeclareStatic
        } else {
            self.var_flag
        };
        let Some((name, range)) = self.name_of(decl.name()) else {
            return self.walk_children(node);
        };

        // A struct/union field is `uninitVar`, never `symbol` (`Parser.hs:465`):
        // an item for the editor to land on, but never declared into a scope
        // and never a global symbol (see `aggregate_decl`). Its type is
        // walked — it can mention a template parameter — and it has no
        // initializer to walk.
        if flags == DeclareFlag::DeclareMember {
            let id = self.emit(name, DefKind::Variable(flags), range);
            self.items[id.index()].exposable = false;
            let saved_parent = self.parent.replace(id);
            self.walk_opt(decl.ty());
            self.parent = saved_parent;
            return;
        }

        let id = self.emit(name, DefKind::Variable(flags), range);
        // `exposable (Variable _ _ typ _ _ _) = isConst typ` (`SymbolTable.hs:168`).
        // `isConst ConstF{} = True` and nothing else (`Parser/Syntax.hs:658-659`),
        // so this is the outermost `const` of the *written* type — and the loop
        // induction variable, whose `const` `uninitConst` supplies
        // (`Parser.hs:451`).
        self.items[id.index()].exposable = matches!(decl.ty(), Some(ast::Type::ConstType(_)))
            || flags == DeclareFlag::DeclareUninitConst;
        let saved_parent = self.parent.replace(id);
        self.walk_opt(decl.ty());
        // The initializer is parsed *before* `symbol` adds the variable
        // (`Parser.hs:453-457`), so `uint32 x = x;` does not see itself.
        self.walk_opt(decl.expr());
        self.parent = saved_parent;
        self.declare(name, id);
    }

    /// `alias'` (`Parser.hs:274-276`).
    fn alias_decl(&mut self, node: &SyntaxNode) {
        let template = self.template_target.take();
        let decl = ast::AliasDecl::cast(node.clone()).expect("kind checked");
        let Some((name, range)) = self.name_of(decl.name()) else {
            return self.walk_children(node);
        };

        let id = self.emit(name, DefKind::Alias, range);
        if template.is_some() {
            self.mark_template_target(id);
        }
        let saved_parent = self.parent.replace(id);
        // `unscopedWith params typ` — the parameters are visible only in the RHS.
        self.push(ScopeKind::Unscoped, None, node);
        self.seed_all(template.as_ref().map_or(&[][..], |t| &t.params));
        self.walk_opt(decl.ty());
        self.pop();
        self.parent = saved_parent;
        if template.is_none() {
            self.declare(name, id);
        }
    }

    /// `template def = symbol $ do { params <- templateParameters; Template params
    /// <$> noted (def params) }` (`Parser.hs:352-375`).
    ///
    /// Two entries land in the item tree under one key: the `Template` node's, and
    /// the wrapped declaration's. That is what `getSymbols''` builds
    /// (`symbol e (concat cs)`), and the first-wins merge in [`global_symbols`] is
    /// what makes the `Template` the one a lookup finds.
    fn template_decl(&mut self, node: &SyntaxNode) {
        let decl = ast::TemplateDecl::cast(node.clone()).expect("kind checked");
        let Some(inner) = decl.decl() else { return self.walk_children(node) };
        // `getName (TemplateF _ a) = getName a` (`Parser/Syntax.hs:727`).
        let Some((name, range)) = self.name_of(item_name(&inner)) else {
            return self.walk_children(node);
        };

        let id = self.emit(name, DefKind::Template, range);
        let saved_parent = self.parent.replace(id);

        // `paramList` nests one `unscopedWith [x]` per parameter and pops them all
        // before the declaration is parsed (`Parser.hs:361-364`). One scope with
        // parameters seeded left to right has the same visibility: a default may
        // mention every *earlier* parameter and not itself.
        self.push(ScopeKind::Unscoped, None, node);
        let mut params: Vec<(Symbol, ItemId)> = Vec::new();
        let mut duplicates: Vec<Symbol> = Vec::new();
        if let Some(list) = decl.template_param_list() {
            for param in list.syntax().children() {
                if !matches!(param.kind(), SyntaxKind::TYPE_PARAM | SyntaxKind::VALUE_PARAM) {
                    continue;
                }
                self.walk_children(&param);
                let name_node = ast::TypeParam::cast(param.clone())
                    .and_then(|p| p.name())
                    .or_else(|| ast::ValueParam::cast(param.clone()).and_then(|p| p.name()));
                let Some((pname, prange)) = self.name_of(name_node) else { continue };
                // `names \\ nub names` (`Parser.hs:356`) — every repeat, not just
                // the first, and a *different* message from `parseSymbol`'s.
                if params.iter().any(|(n, _)| *n == pname) {
                    duplicates.push(pname);
                }
                let pid = self.emit_param(pname, DefKind::TemplateParam, prange, &param);
                self.scopes.seed(pname, pid);
                params.push((pname, pid));
            }
        }
        self.pop();

        if !duplicates.is_empty() {
            let text: Vec<SmolStr> =
                duplicates.iter().map(|d| self.interner.resolve(*d)).collect();
            let names: Vec<&str> = text.iter().map(|s| s.as_str()).collect();
            // `parseError` at the template's own offset (`Parser.hs:352-359`) —
            // the declaration's first token, not its name.
            let at = FileRange::new(self.file, kg_syntax::trimmed_range(inner.syntax()));
            self.diagnostics.push(template_parameter_redefinition_error(at, &names));
        }

        self.template_target = Some(TemplateTarget { item: id, params });
        self.walk(inner.syntax());
        // A declaration form that does not consume them (an `export`, say) leaves
        // them behind; they must not leak into the next declaration.
        self.template_target = None;

        self.parent = saved_parent;
        self.declare(name, id);
    }

    /// `exportType` (`Parser.hs:278`), named `$exportN` by `parseSymbol` itself
    /// (`SymbolTable.hs:213`) so that repeated `export T;` never collide.
    fn export_decl(&mut self, node: &SyntaxNode) {
        let decl = ast::ExportDecl::cast(node.clone()).expect("kind checked");
        // `exported ExportType{} = True` (`SymbolTable.hs:175`).
        self.has_export_or_reset = true;
        let name = self.scopes.make_export_name(self.interner);
        let id = self.emit(name, DefKind::ExportType, node.text_range());
        let saved_parent = self.parent.replace(id);
        self.walk_opt(decl.ty());
        self.parent = saved_parent;
        self.declare(name, id);
    }

    // --- expressions, statements, types ------------------------------------

    /// `lambda` (`Parser.hs:436-447`). A lambda is not itself a symbol —
    /// `getSymbols''` has no `Lambda` case — but its captures and parameters are.
    fn lambda(&mut self, node: &SyntaxNode) {
        let expr = ast::LambdaExpr::cast(node.clone()).expect("kind checked");
        // `makeSymbolName "?lambda" lambdaIndex` reads and bumps the *enclosing*
        // scope's counter (`Parser.hs:434`).
        let ident = self.scopes.make_lambda_name(self.interner);

        self.push(ScopeKind::Named, Some(ident), node);

        // `capture = (Capture . functionScope <$> getScopeQualifier) <*> identifier`
        // (`Parser.hs:447`): a capture's `declScopeName` already carries the `$`,
        // one scope deeper than where it is written.
        let capture_scope = self.scopes.qualifier().child(self.interner.intern(names::FUNCTION_SCOPE));
        let mut captures: Vec<(Symbol, ItemId)> = Vec::new();
        if let Some(list) = expr.capture_list() {
            for capture in list.captures() {
                if let Some((cname, crange)) = self.name_ref_of(capture.name_ref()) {
                    let cid =
                        self.emit_key(capture_scope.child(cname), DefKind::Capture, crange);
                    captures.push((cname, cid));
                }
            }
        }

        if let Some(list) = expr.param_list() {
            for param in list.params() {
                self.walk_opt(param.ty());
                if let Some((pname, prange)) = self.name_of(param.name()) {
                    let pid = self.emit_param(pname, DefKind::FuncParam, prange, param.syntax());
                    self.declare(pname, pid);
                }
            }
        }
        self.walk_opt(expr.ret());

        // `functionScopedWith cs` (`Parser.hs:444`) — over the body, as a
        // function's is.
        let body_node = expr.body().map(|b| b.syntax().clone()).unwrap_or_else(|| node.clone());
        self.push(ScopeKind::Function, None, &body_node);
        self.seed_all(&captures);
        let saved_flag = std::mem::replace(&mut self.var_flag, DeclareFlag::DeclareLocal);
        if let Some(body) = expr.body() {
            self.walk_children(body.syntax());
        }
        self.var_flag = saved_flag;
        self.pop();

        self.pop();
    }

    fn scoped_block(&mut self, node: &SyntaxNode, kind: ScopeKind) {
        self.push(kind, None, node);
        self.walk_children(node);
        self.pop();
    }

    /// `rangeFor` / `staticFor` (`Parser.hs:584,591`): `localScoped` wraps the
    /// induction variable, the limit expression *and* the body, so a `{ … }` body
    /// pushes a second, nested `Local` scope.
    fn for_stmt(&mut self, node: &SyntaxNode) {
        let (attrs, var, limit, body) = match ast::ForStmt::cast(node.clone()) {
            Some(f) => (true, f.var_decl(), f.limit(), f.body()),
            None => {
                let f = ast::StaticForStmt::cast(node.clone()).expect("kind checked");
                (false, f.var_decl(), f.limit(), f.body())
            }
        };
        if attrs {
            // `optionalAttributes` is read before `localScoped` (`Parser.hs:581-584`).
            for attr in ast::ForStmt::cast(node.clone()).expect("checked").attr_lists() {
                self.walk(attr.syntax());
            }
        }

        self.push(ScopeKind::Local, None, node);
        if let Some(var) = var {
            // `uninitConst` (`Parser.hs:451`).
            let saved = std::mem::replace(&mut self.var_flag, DeclareFlag::DeclareUninitConst);
            self.walk(var.syntax());
            self.var_flag = saved;
        }
        self.walk_opt(limit);
        self.walk_opt(body);
        self.pop();
    }

    /// `SwitchCase` / `SwitchDefault` (`Parser.hs:602-603`): the case *expression*
    /// is outside the `localScoped`, the statements inside.
    fn switch_case(&mut self, node: &SyntaxNode) {
        // Split on the `:` rather than on the typed accessors: the reference's
        // `localScoped` starts exactly there, and a case body holds bare
        // declarations as well as statements (`many statementOrDecl`).
        let mut in_body = false;
        let mut body = Vec::new();
        for element in node.children_with_tokens() {
            if let Some(token) = element.as_token() {
                if token.kind() == SyntaxKind::COLON {
                    in_body = true;
                }
            } else if let Some(child) = element.as_node() {
                if in_body {
                    body.push(child.clone());
                } else {
                    self.walk(child);
                }
            }
        }
        self.push(ScopeKind::Local, None, node);
        for child in body {
            self.walk(&child);
        }
        self.pop();
    }

    /// `staticIf p` with `p' = nestedScope p <|> p` (`Parser.hs:593-595`) — a
    /// braced branch is `quasiScoped` (`Parser.hs:467`), **not** `localScoped`, so
    /// it does not deepen the enclosing qualifier the way a plain block does.
    fn static_if(&mut self, node: &SyntaxNode) {
        let stmt = ast::StaticIfStmt::cast(node.clone()).expect("kind checked");
        self.walk_opt(stmt.cond());
        self.static_if_branch(stmt.then_branch());
        if let Some(alt) = stmt.else_branch() {
            self.static_if_branch(alt.stmt());
        }
    }

    fn static_if_branch(&mut self, stmt: Option<ast::Stmt>) {
        match stmt {
            Some(ast::Stmt::BlockStmt(block)) => {
                self.scoped_block(block.syntax(), ScopeKind::Quasi)
            }
            Some(other) => self.walk(other.syntax()),
            None => {}
        }
    }

    /// `functionType` (`Parser.hs:551-557`): `quasiScoped` around the parameter
    /// list. The parameter *names* are `symbol identifier` — plain `Identifier`
    /// nodes, which `getSymbols''` has no case for, so they are symbols in neither
    /// map and produce no [`Item`]. The scope is still pushed: its pop increments
    /// the enclosing scope's `scopeIndex`, which renames every later `$quasiN`.
    fn fn_type(&mut self, node: &SyntaxNode) {
        let ty = ast::FnType::cast(node.clone()).expect("kind checked");
        for attr in ty.attr_lists() {
            self.walk(attr.syntax());
        }
        self.push(ScopeKind::Quasi, None, node);
        // A named parameter of a function *type* is `parseSymbol`ed into the
        // quasi scope like a function's own (`functionTypeParam`,
        // `Parser.hs:551-556`), which is what makes `(uint32 x, uint32 x) ->
        // void` a redefinition at the second `x` (`kg-oracle --errors`,
        // 2026-09-10; `test/syntax/function-type.k::8`). It is *not* in
        // `getSymbols''` (`Symbols.hs:52-68`), so no item is emitted: the
        // collision is checked here and the maps stay as they were.
        let mut seen: Vec<(Symbol, TextRange)> = Vec::new();
        if let Some(list) = ty.fn_type_param_list() {
            for param in list.fn_type_params() {
                self.walk_opt(param.ty());
                if let Some((pname, prange)) = self.name_of(param.name()) {
                    match seen.iter().find(|(n, _)| *n == pname) {
                        Some(&(_, previous)) => {
                            let text = self.interner.resolve(pname);
                            let location = self.location(previous.start());
                            self.diagnostics.push(crate::scope::redefinition_error(
                                FileRange::new(self.file, prange),
                                &text,
                                FileRange::new(self.file, previous),
                                location.as_deref(),
                            ));
                        }
                        None => seen.push((pname, prange)),
                    }
                }
            }
        }
        self.pop();
        self.walk_opt(ty.ret());
    }
}

/// `getName` of the declaration a `template <…>` wraps.
fn item_name(item: &ast::Item) -> Option<ast::Name> {
    match item {
        ast::Item::ClassDecl(d) => d.name(),
        ast::Item::StructDecl(d) => d.name(),
        ast::Item::UnionDecl(d) => d.name(),
        ast::Item::EnumDecl(d) => d.name(),
        ast::Item::FnDecl(d) => d.name(),
        ast::Item::FnProto(d) => d.name(),
        ast::Item::VarDecl(d) => d.name(),
        ast::Item::AliasDecl(d) => d.name(),
        // `template <…> template <…> X` is not a form the reference parses
        // (`template def` takes a `def`, never another `template`).
        _ => None,
    }
}

/// The `EnumVariant`s of an `ENUM_DECL`.
///
/// `grammar/kanagawa.ungram` writes the body inline
/// (`'enum' Name BaseList '{' EnumVariant (',' EnumVariant)* '}'`) but
/// `kg_parser` wraps it in a `DECL_LIST`, so the generated
/// `EnumDecl::enum_variants` — which looks at *direct* children — finds nothing.
/// Accept both shapes rather than depending on which one wins; see the report.
fn enum_variants(node: &SyntaxNode) -> Vec<ast::EnumVariant> {
    node.children()
        .flat_map(|child| {
            if child.kind() == SyntaxKind::DECL_LIST {
                child.children().filter_map(ast::EnumVariant::cast).collect::<Vec<_>>()
            } else {
                ast::EnumVariant::cast(child).into_iter().collect()
            }
        })
        .collect()
}

/// Whether any `[[…]]` group directly on `node` names `attr`.
///
/// Attribute names are matched with `L.symbol`, not as keywords
/// (`Parser.hs:171-177`), so `reset` lexes as an ordinary identifier.
///
/// The name is read off the `ATTR` node's own `IDENT` token rather than through
/// `Attr::name()`: `grammar/kanagawa.ungram` writes `Attr = Name AttrArgs?`, but
/// `kg_parser` emits the identifier as a bare token with no `NAME` wrapper, so the
/// generated accessor always returns `None`. See the report.
fn has_attr(node: &SyntaxNode, attr: &str) -> bool {
    node.children()
        .filter(|c| c.kind() == SyntaxKind::ATTR_LIST)
        .flat_map(|list| list.children())
        .filter(|c| c.kind() == SyntaxKind::ATTR)
        .any(|a| {
            a.children_with_tokens()
                .filter_map(|e| e.into_token())
                .any(|t| t.kind() == SyntaxKind::IDENT && t.text() == attr)
        })
}

/// Any `ERROR` node or `LEX_ERROR` token means the reference would have produced
/// an `ErrorF` node, and therefore the `["$errors"]` key (`Symbols.hs:67`).
///
/// Except the one `ERROR` shape that is no error, a `default = e;`
/// ([`kg_syntax::is_default_init`]): the reference parses it into
/// `DefaultInitializationF`, and counting it made every file with one skip
/// pass 9 whole (`callbacks.k::89`).
fn has_syntax_error(root: &SyntaxNode) -> bool {
    root.descendants_with_tokens().any(|e| match e {
        NodeOrToken::Node(n) => n.kind() == SyntaxKind::ERROR && !kg_syntax::is_default_init(&n),
        NodeOrToken::Token(t) => t.kind() == SyntaxKind::LEX_ERROR,
    })
}

/// `updateModule` (`SymbolTable.hs:114-133`): the module name, its mangled
/// namespace, and the three disjoint buckets of the export list.
fn module_header(decl: &ast::ModuleDecl, interner: &Interner) -> ModuleHeader {
    let name = decl.module_path().map(|p| module_path(&p)).unwrap_or_default();
    let namespace = (!name.is_empty())
        .then(|| Namespace(interner.intern(&Namespace::mangle(&name))));

    let mut exports = Vec::new();
    let mut exported_at = Vec::new();
    if let Some(list) = decl.module_export_list() {
        // The entries are `ModuleDiff | ModuleIdentifier | identifier`
        // (`Parser.hs:187`), and the parser writes the first two **flat**: there
        // is no `MODULE_DIFF` kind and no nested `MODULE_DECL`, so `module a.b`
        // is a `MODULE_KW` token followed by one [`SyntaxKind::MODULE_PATH`] and
        // `module a.b \ c.d` is two of them either side of a `BACKSLASH`, all
        // direct children of the `MODULE_EXPORT_LIST`
        // (`kg_parser::grammar::items::exposed_item`). Entries are separated by
        // `COMMA`, so the flush points are every comma and the closing brace.
        //
        // Walking `children_with_tokens` in source order, rather than the typed
        // `module_decls()` accessor, is what makes this agree with the parser:
        // the accessor comes from `grammar/kanagawa.ungram:57-58`, which still
        // describes the nested shape and so yields nothing at all. Dropping
        // these two variants is silent — it disables `updateImports`' first two
        // `Needed` rules (`SymbolTable.hs:104-105`), the `exposedModules` and
        // `exposedDiffs` halves of `updateVisible` (`:152,150`), and
        // `checkExposed`'s `checkModules`/`checkDiffs` (`Parser.hs:224-231`) —
        // which is exactly why the test below pins the shape.
        let mut lhs: Option<ModuleName> = None;
        let mut rhs: Option<ModuleName> = None;
        let mut after_backslash = false;
        for element in list.syntax().children_with_tokens() {
            if let Some(token) = element.as_token() {
                match token.kind() {
                    SyntaxKind::BACKSLASH => after_backslash = true,
                    SyntaxKind::COMMA | SyntaxKind::R_CURLY => {
                        push_module_export(&mut exports, &mut lhs, &mut rhs);
                        after_backslash = false;
                    }
                    _ => {}
                }
                continue;
            }
            let Some(node) = element.as_node() else { continue };
            match node.kind() {
                SyntaxKind::MODULE_PATH => {
                    let path = ast::ModulePath::cast(node.clone())
                        .map(|p| module_path(&p))
                        .unwrap_or_default();
                    if after_backslash {
                        rhs = Some(path);
                    } else {
                        lhs = Some(path);
                    }
                }
                SyntaxKind::NAME_REF => {
                    if let Some(t) =
                        ast::NameRef::cast(node.clone()).and_then(|n| n.ident_token())
                    {
                        exports.push(ExportItem::Symbol(interner.intern(t.text())));
                        exported_at.push((interner.intern(t.text()), t.text_range()));
                    }
                }
                _ => {}
            }
        }
        // A list whose `}` was never reached — error recovery still has to yield
        // the entry that was parsed.
        push_module_export(&mut exports, &mut lhs, &mut rhs);
    }

    ModuleHeader { name, namespace, exports, exported_at }
}

/// Closes one `module a.b` / `module a.b \ c.d` export entry
/// (`SymbolTable.hs:124-128`). A `\` with nothing after it degrades to the plain
/// module form rather than being dropped.
fn push_module_export(
    exports: &mut Vec<ExportItem>,
    lhs: &mut Option<ModuleName>,
    rhs: &mut Option<ModuleName>,
) {
    let Some(a) = lhs.take() else {
        *rhs = None;
        return;
    };
    match rhs.take() {
        Some(b) => exports.push(ExportItem::Difference(a, b)),
        None => exports.push(ExportItem::Module(a)),
    }
}

/// `dotSepName` (`Parser/Lexer.hs:230-233`) run backwards: rebuild the dotted
/// components from the CST's tokens.
///
/// A `-` is *inside* a component (`module mini-base`, `library/mini-base.k:12`)
/// while a `.` separates components; a leading `.` belongs to the component, so
/// that `.cmdargs` is one name and not an empty one followed by `cmdargs`
/// (`Parser.hs:41-45`).
/// `dotSepName` read off a `MODULE_PATH`'s tokens: `.` separates components,
/// `-` and digits join within one, and a leading `.` belongs to the first
/// component (`.cmdargs`, `Parser.hs:179-183`; `Lexer.hs:230-233`).
///
/// `pub` because `kg_hir::lower` reads the same shape into its
/// `ModuleIdentifier` nodes; one reader, two consumers.
pub fn module_path(path: &ast::ModulePath) -> ModuleName {
    let mut parts: Vec<SmolStr> = Vec::new();
    let mut current = String::new();
    for element in path.syntax().children_with_tokens() {
        let Some(token) = element.into_token() else { continue };
        if token.kind().is_trivia() {
            continue;
        }
        if token.kind() == SyntaxKind::DOT {
            if parts.is_empty() && current.is_empty() {
                current.push('.');
            } else {
                parts.push(SmolStr::new(&current));
                current.clear();
            }
        } else {
            current.push_str(token.text());
        }
    }
    if !current.is_empty() {
        parts.push(SmolStr::new(&current));
    }
    parts
}

/// `parseImports` (`Parser.hs:194-199`) — the *name*, and nothing else.
///
/// The reference interleaves parsing the import with resolving and reading the
/// file (`Parser.hs:198-202`); here the two are split, exactly as
/// `docs/spec/resolve.md` §11.2 splits `resolve_module_path` from `parse`. So
/// `needed` is left at its default: all three `updateImports` rules
/// (`SymbolTable.hs:103-107`) need the *imported* module's interface, and
/// [`crate::module::ModuleGraph::add_import`] owns every one of them. Computing
/// two of the three here as well would give the rule two owners.
fn lower_imports(
    file: &ast::SourceFile,
    file_id: FileId,
    interner: &Interner,
) -> Vec<ImportDecl> {
    file.import_decls()
        .enumerate()
        .map(|(i, decl)| ImportDecl {
            id: ImportId(i as u32),
            name: decl.module_path().map(|p| module_path(&p)).unwrap_or_default(),
            // `optional (keyword "as" *> name)` (`Parser.hs:197`).
            alias: decl.alias().and_then(|a| a.ident_token()).map(|t| interner.intern(t.text())),
            // `offset <- getOffset` before the keyword (`Parser.hs:195-196`) —
            // where the `redundant-import` warning is reported.
            range: FileRange::new(
                file_id,
                decl.import_kw_token()
                    .map(|t| t.text_range())
                    .unwrap_or_else(|| decl.syntax().text_range()),
            ),
            needed: ImportNeeded::default(),
        })
        .collect()
}

/// The range the reference's note for a declaration covers, for
/// [`Item::decl`]. Trivia is trimmed (a comment above a declaration is inside
/// its node here), and a variable's note begins at its type: `static` is
/// parsed outside `variable` (`Parser.hs:461-465`), so the oracle reports
/// `static Foo f;` at `Foo`.
fn decl_range(node: &SyntaxNode) -> Option<TextRange> {
    match node.kind() {
        SyntaxKind::VAR_DECL => {
            let full = kg_syntax::trimmed_range(node);
            let after_static = ast::VarDecl::cast(node.clone())
                .and_then(|v| v.static_kw_token())
                .and_then(|t| {
                    let mut next = t.next_token();
                    while let Some(n) = next {
                        if !n.kind().is_trivia() {
                            return Some(TextRange::new(n.text_range().start(), full.end()));
                        }
                        next = n.next_token();
                    }
                    None
                });
            Some(after_static.unwrap_or(full))
        }
        // `enum = do ident <- keyword "enum" *> identifier; symbol (Enum …
        // <* colon …)` (`Parser.hs:287-289`): the symbol is noted from where
        // `symbol` begins, past the name — `enum DWORD : DWORD` redefines
        // at the `:` (`kg-oracle --errors`, 2026-10-10; difffuzz [15]).
        SyntaxKind::ENUM_DECL => {
            let full = kg_syntax::trimmed_range(node);
            let past_name = ast::EnumDecl::cast(node.clone())
                .and_then(|e| e.name())
                .and_then(|n| n.syntax().last_token())
                .and_then(|t| {
                    let mut next = t.next_token();
                    while let Some(n) = next {
                        if !n.kind().is_trivia() {
                            return Some(TextRange::new(n.text_range().start(), full.end()));
                        }
                        next = n.next_token();
                    }
                    None
                });
            Some(past_name.unwrap_or(full))
        }
        SyntaxKind::CLASS_DECL
        | SyntaxKind::STRUCT_DECL
        | SyntaxKind::UNION_DECL
        | SyntaxKind::FN_DECL
        | SyntaxKind::FN_PROTO
        | SyntaxKind::ALIAS_DECL
        | SyntaxKind::TEMPLATE_DECL => Some(kg_syntax::trimmed_range(node)),
        _ => None,
    }
}

/// Lower one snippet of Kanagawa. Shared with `crate::scope`'s tests, whose
/// oracle — `test/syntax/shadow-warning.k` — is a whole-program property.
#[cfg(test)]
pub(crate) fn lower_str(src: &str, interner: &Interner) -> ItemTree {
    let parse = kg_syntax::parse_file(src);
    assert!(parse.errors().is_empty(), "the snippet must parse: {:?}", parse.errors());
    lower_file(ModuleSource::File(FileId(0)), &parse.source_file(), interner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `kind qualified::name` for every item, in tree order.
    fn keys(tree: &ItemTree, interner: &Interner) -> Vec<String> {
        tree.items.iter().map(|item| show(item, interner)).collect()
    }

    fn show(item: &Item, interner: &Interner) -> String {
        let path = item
            .qualified_name
            .iter()
            .map(|s| interner.resolve(*s).to_string())
            .collect::<Vec<_>>()
            .join("::");
        format!("{:?} {path}", item.kind)
    }

    /// Every key of `global_symbols`, sorted so the assertion is order-free.
    fn symbol_keys(
        config: SymbolsConfig,
        tree: &ItemTree,
        interner: &Interner,
    ) -> Vec<String> {
        let trees = [(ModuleSource::File(FileId(0)), tree.clone())];
        let mut keys: Vec<String> = global_symbols(config, &trees, interner)
            .keys()
            .map(|k| {
                k.iter().map(|s| interner.resolve(*s).to_string()).collect::<Vec<_>>().join("::")
            })
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn synthetic_name_spellings_are_exact() {
        // A typo here is invisible until a shadow-warning or mangling oracle fails.
        assert_eq!(names::LAMBDA_PREFIX, "?lambda");
        assert_eq!(names::QUASI_PREFIX, "$quasi");
        assert_eq!(names::EXPORT_PREFIX, "$export");
        assert_eq!(names::FUNCTION_SCOPE, "$");
        assert_eq!(names::THIS, "$this");
        assert_eq!(names::ERRORS, "$errors");
    }

    #[test]
    fn an_empty_item_tree_has_no_errors() {
        assert!(!ItemTree::default().has_errors());
    }

    /// The `docs/spec/resolve.md` §10 keying table, top to bottom: a file with no
    /// `module` keys at `[]`, a function body at `[…,"f","$"]`, and a parameter
    /// one level shallower than the body (`Parser.hs:415-416`: `funcParams` runs
    /// inside `scoped ident`, the body inside a further `functionScoped`).
    #[test]
    fn the_keying_table_of_section_ten() {
        let i = Interner::new();
        let tree = lower_str("inline void f(uint32 p)\n{\n  bool x;\n}\n", &i);
        assert_eq!(
            keys(&tree, &i),
            [
                "Function f",
                "FuncParam f::p",
                "Variable(DeclareLocal) f::$::x",
            ]
        );
    }

    /// `module a.b` mangles to a **single** component `@a@b`
    /// (`moduleNamespace`, `Parser/Syntax.hs:273-274`), not to two.
    #[test]
    fn a_module_contributes_one_mangled_qualifier_component() {
        let i = Interner::new();
        let tree = lower_str("module a.b {Foo}\nconst auto Foo = 1;\n", &i);
        assert_eq!(keys(&tree, &i), ["Variable(DeclareGlobal) @a@b::Foo"]);
        let header = tree.module_header.as_ref().expect("a module header");
        assert_eq!(header.name, vec![SmolStr::new("a"), SmolStr::new("b")]);
        assert_eq!(header.exports.len(), 1);
    }

    /// The export list carries **three** disjoint buckets
    /// (`SymbolTable.hs:117-126`), and the parser writes two of them flat — a
    /// `MODULE_KW` token and one or two bare `MODULE_PATH` nodes, with no
    /// wrapper node of their own. Reading them through a wrapper that does not
    /// exist is silent: it costs `updateImports`' first two `Needed` rules
    /// (`SymbolTable.hs:104-105`), `updateVisible`'s re-export and difference
    /// algebra (`:150,152`) and `checkExposed`'s module half
    /// (`Parser.hs:224-231`) all at once, with no test failing anywhere.
    ///
    /// Oracle for the shape: `library/mini-base.k:12-16` (three module
    /// re-exports, no symbols) and
    /// `test/syntax/imports/diff/foo_self.k:4-7` (a difference beside a module).
    #[test]
    fn the_export_list_keeps_module_and_difference_entries() {
        let i = Interner::new();
        let tree = lower_str("module a.b\n  { module c.d\n  , x\n  , module e \\ f\n  }\n", &i);
        let header = tree.module_header.as_ref().expect("a module header");
        assert_eq!(
            header.exports,
            vec![
                ExportItem::Module(vec![SmolStr::new("c"), SmolStr::new("d")]),
                ExportItem::Symbol(i.intern("x")),
                ExportItem::Difference(vec![SmolStr::new("e")], vec![SmolStr::new("f")]),
            ]
        );
        assert_eq!(header.exposed_modules(&i).len(), 1);
        assert_eq!(header.exposed_diffs(&i).len(), 1);
        assert_eq!(header.exposed_symbols().len(), 1);
    }

    /// `localScoped`'s pop rewrites the *parent's* qualifier
    /// (`Parser.hs:153-155`), so a declaration after a block is keyed one level
    /// deeper than one before it. This is the mechanism that makes
    /// `test/syntax/shadow-warning.k:77-86` `warning:0`.
    #[test]
    fn a_block_deepens_the_enclosing_qualifier_when_it_closes() {
        let i = Interner::new();
        let tree = lower_str(
            "inline void main()\n{\n  for (const auto x : 10)\n  {\n  }\n\n  bool x;\n}\n",
            &i,
        );
        assert_eq!(
            keys(&tree, &i),
            [
                "Function main",
                "Variable(DeclareUninitConst) main::$::$quasi0::x",
                "Variable(DeclareLocal) main::$::$quasi1::x",
            ]
        );
    }

    /// Two sibling blocks land under *different* qualifiers, which is why
    /// `test/syntax/shadow-warning.k:100-112` — `if`/`else` branches each
    /// declaring `y` — is `warning:0`.
    #[test]
    fn sibling_blocks_get_disjoint_qualifiers() {
        let i = Interner::new();
        let tree = lower_str(
            "inline void Run(bool x)\n{\n  if (x) { uint32 y; } else { uint32 y; }\n}\n",
            &i,
        );
        assert_eq!(
            &keys(&tree, &i)[2..],
            [
                "Variable(DeclareLocal) Run::$::$quasi0::y",
                "Variable(DeclareLocal) Run::$::$quasi1::$quasi0::y",
            ]
        );
    }

    /// A `static if` branch is `nestedScope` = `quasiScoped` (`Parser.hs:467,595`),
    /// **not** `localScoped` (`Parser.hs:469`): its pop bumps the parent's
    /// `scopeIndex` but does not deepen the parent's qualifier, so a declaration
    /// after the `static if` keeps the plain function-body key.
    #[test]
    fn static_if_branches_are_quasi_scoped_not_local_scoped() {
        let i = Interner::new();
        let tree = lower_str(
            "inline void main()\n{\n  static if (true) { bool a; } else { bool b; }\n  bool c;\n}\n",
            &i,
        );
        assert_eq!(
            &keys(&tree, &i)[1..],
            [
                "Variable(DeclareLocal) main::$::$quasi0::a",
                "Variable(DeclareLocal) main::$::$quasi1::b",
                "Variable(DeclareLocal) main::$::c",
            ]
        );
    }

    /// `switch`'s `case`/`default` bodies are `localScoped` (`Parser.hs:602-603`),
    /// so *each* deepens the function-body qualifier as it closes.
    #[test]
    fn every_switch_case_body_deepens_the_qualifier() {
        let i = Interner::new();
        let tree = lower_str(
            "inline void main()\n{\n  switch (k) { case 1: bool a; break; default: bool b; break; }\n  bool c;\n}\n",
            &i,
        );
        assert_eq!(
            &keys(&tree, &i)[1..],
            [
                "Variable(DeclareLocal) main::$::$quasi0::a",
                "Variable(DeclareLocal) main::$::$quasi1::$quasi0::b",
                "Variable(DeclareLocal) main::$::$quasi1::$quasi1::c",
            ]
        );
    }

    /// A function *type* pushes `quasiScoped` around its parameter list
    /// (`Parser.hs:551`), and a `quasiScoped` pop increments the enclosing
    /// scope's `scopeIndex` (`Parser.hs:142`) — so the very next block is
    /// `$quasi1`, not `$quasi0`. Nothing inside the type is a symbol: the
    /// parameter names are bare `Identifier` nodes, which `getSymbols''` has no
    /// case for.
    #[test]
    fn a_function_type_consumes_a_quasi_index() {
        let i = Interner::new();
        let tree = lower_str(
            "inline void main()\n{\n  (uint32 q) -> void g;\n  { bool z; }\n}\n",
            &i,
        );
        assert_eq!(
            &keys(&tree, &i)[1..],
            [
                "Variable(DeclareLocal) main::$::g",
                "Variable(DeclareLocal) main::$::$quasi1::z",
            ]
        );
    }

    /// `capture = (Capture . functionScope <$> getScopeQualifier) <*> identifier`
    /// (`Parser.hs:447`): a capture is keyed one scope *deeper* than where it is
    /// written, because `functionScope` appends `"$"` before the lambda's own
    /// function scope is pushed. The lambda itself is not a symbol —
    /// `getSymbols''` has no `Lambda` case.
    #[test]
    fn lambda_captures_are_keyed_inside_the_lambda_body_scope() {
        let i = Interner::new();
        let tree = lower_str(
            "inline void main()\n{\n  bool c;\n  auto f = [c](uint32 a) -> void { bool y; };\n}\n",
            &i,
        );
        assert_eq!(
            &keys(&tree, &i)[1..],
            [
                "Variable(DeclareLocal) main::$::c",
                "Variable(DeclareLocal) main::$::f",
                "Capture main::$::?lambda0::$::c",
                "FuncParam main::$::?lambda0::a",
                "Variable(DeclareLocal) main::$::?lambda0::$::y",
            ]
        );
    }

    /// Two lambdas in one scope get `?lambda0` and `?lambda1`
    /// (`makeSymbolName`, `SymbolTable.hs:216-225`).
    #[test]
    fn lambda_indices_count_up_within_a_scope() {
        let i = Interner::new();
        let tree = lower_str(
            "inline void main()\n{\n  auto f = []() -> void { bool a; };\n  auto g = []() -> void { bool b; };\n}\n",
            &i,
        );
        let ks = keys(&tree, &i);
        assert!(ks.contains(&"Variable(DeclareLocal) main::$::?lambda0::$::a".to_string()), "{ks:?}");
        assert!(ks.contains(&"Variable(DeclareLocal) main::$::?lambda1::$::b".to_string()), "{ks:?}");
    }

    /// `enum` pushes `scoped ident` (`Parser.hs:284`) so constants are keyed under
    /// the enum, matching `static assert(E::E1 == 0)` in
    /// `test/syntax/enum.k:81-104`.
    #[test]
    fn enum_constants_are_keyed_under_the_enum() {
        let i = Interner::new();
        let tree = lower_str("enum E : uint32\n{\n  A,\n  B = A\n}\n", &i);
        assert_eq!(
            keys(&tree, &i),
            ["Enum E", "EnumConstant E::A", "EnumConstant E::B"]
        );
    }

    /// `enumConsts'` nests one `unscopedWith [x]` per constant (`Parser.hs:291`)
    /// and `parseSymbol` consults **only the innermost scope**
    /// (`SymbolTable.hs:198`), so a constant collides with its immediate
    /// predecessor and with nothing else.
    #[test]
    fn an_enum_constant_only_collides_with_its_predecessor() {
        let i = Interner::new();
        let adjacent = lower_str("enum E : uint32 { A, A }\n", &i);
        assert_eq!(adjacent.diagnostics.len(), 1, "adjacent duplicates are caught");

        let separated = lower_str("enum E : uint32 { A, B, A }\n", &i);
        assert!(
            separated.diagnostics.is_empty(),
            "the reference's per-constant scope nesting cannot see two steps back"
        );
    }

    /// `parseSymbol` (`SymbolTable.hs:198-201`): only the innermost scope is
    /// consulted, so a redeclaration in the same block is an error and the same
    /// name in a nested block is not.
    #[test]
    fn redefinition_is_per_innermost_scope() {
        let i = Interner::new();
        let same = lower_str("inline void main() { bool a; bool a; }\n", &i);
        assert_eq!(same.diagnostics.len(), 1);
        assert!(same.diagnostics[0].message.contains("symbol redefinition"), "{:?}", same.diagnostics[0].message);

        let nested = lower_str("inline void main() { bool a; { bool a; } }\n", &i);
        assert!(nested.diagnostics.is_empty());
    }

    /// The template parameter list has its own duplicate check with its own
    /// message (`Parser.hs:352-359`, `names \\ nub names`) — it is *not*
    /// `parseSymbol`'s, because parameters are seeded, never declared.
    #[test]
    fn duplicate_template_parameters_get_their_own_message() {
        let i = Interner::new();
        let tree = lower_str("template <typename T, typename T> struct S { uint32 x; }\n", &i);
        assert_eq!(tree.diagnostics.len(), 1);
        assert_eq!(
            tree.diagnostics[0].message,
            "Redefinition of template parameter(s) [\"T\"]"
        );
    }

    /// `class' symbols params = unscopedWith symbols $ scopedWith params ident`
    /// (`Parser.hs:335-336`): the parameters live *in the class scope*, so a
    /// member may collide with one — which `parseSymbol` reports.
    #[test]
    fn a_class_member_collides_with_a_template_parameter() {
        let i = Interner::new();
        let tree = lower_str("template <typename T> class C { uint32 T; }\n", &i);
        assert_eq!(tree.diagnostics.len(), 1, "{:?}", tree.diagnostics);
        assert!(tree.diagnostics[0].message.contains("symbol redefinition"));
    }

    /// Where `parseSymbol`'s redefinition lands and what it names: each
    /// symbol's note, from where `symbol` (or `notedWithDocs`) began — a
    /// parameter from its own first token, an enum past its name, at the
    /// `:` (`Parser.hs:287-289,361-365,377-379`; `kg-oracle --errors`,
    /// 2026-10-10, difffuzz [15], [18], [24]; an enum's constant, the
    /// re-run's). Red with a parameter or a constant noted at its owner, or
    /// an enum at its keyword.
    #[test]
    fn a_redefinition_is_noted_where_the_reference_notes_the_symbol() {
        let i = Interner::new();
        let at = |src: &str| -> Vec<(String, String)> {
            let tree = lower_str(src, &i);
            tree.diagnostics
                .iter()
                .map(|d| (src[d.range.range].to_owned(), src[d.labels[0].range.range].to_owned()))
                .collect()
        };
        let pair = |a: &str, b: &str| vec![(a.to_owned(), b.to_owned())];
        assert_eq!(at("inline void f(uint32 a, bool a)\n{\n}\n"), pair("bool a", "uint32 a"));
        assert_eq!(at("template <auto N>\nclass One\n{\n    uint3 N;\n}\n"), pair("uint3 N;", "auto N"));
        assert_eq!(at("using DWORD = bool;\nenum DWORD : DWORD\n{\n    X\n}\n"), pair(": DWORD\n{\n    X\n}", "using DWORD = bool;"));
        assert_eq!(at("enum E : uint8\n{\n    A\n}\nstruct E\n{\n    uint32 x;\n}\n"), pair("struct E\n{\n    uint32 x;\n}", ": uint8\n{\n    A\n}"));
        assert_eq!(at("enum Foo : int1\n{\n    E1 = -1,\n    E1\n}\n"), pair("E1", "E1 = -1"));
    }

    /// `update e@Struct{} = symbol e . const mempty` (`Symbols.hs:48-49`) and
    /// `memberVariable = uninitVar DeclareMember` (`Parser.hs:465`), which is
    /// never wrapped in `symbol`: a field is in neither symbol map. A class body,
    /// by contrast, *is* descended into.
    ///
    /// Since 2026-09-06 a field is still an **item** — the editor needs a
    /// declaration to land on — keyed under the struct's quasi scope and
    /// flagged `DeclareMember`, and it is [`global_symbols`] that keeps it out
    /// of the map. Both halves are asserted, because the first alone would pin
    /// the tree and say nothing about the symbols.
    #[test]
    fn struct_fields_are_items_but_not_symbols_while_class_fields_are_both() {
        let i = Interner::new();
        let s = lower_str("struct S { uint32 a; uint32 b; }\n", &i);
        assert_eq!(
            keys(&s, &i),
            ["Struct S", "Variable(DeclareMember) $quasi0::a", "Variable(DeclareMember) $quasi0::b"]
        );
        assert_eq!(symbol_keys(SymbolsConfig::AllSymbols, &s, &i), ["S"]);
        assert!(s.diagnostics.is_empty(), "a field is never `parseSymbol`ed: {:?}", s.diagnostics);

        let c = lower_str("class C { uint32 a; }\n", &i);
        assert_eq!(keys(&c, &i), ["Class C", "Variable(DeclareClass) C::a"]);
    }

    /// Two fields of one name: `uninitVar` never calls `parseSymbol`
    /// (`Parser.hs:465`), so the reference reports nothing here and neither
    /// must this — the items exist for navigation, not for scoping.
    #[test]
    fn duplicate_struct_fields_are_not_a_redefinition() {
        let i = Interner::new();
        let s = lower_str("struct S { uint32 a; uint32 a; }\n", &i);
        assert!(s.diagnostics.is_empty(), "{:?}", s.diagnostics);
    }

    /// `symbol e@Class{} = insert e . insert' e [getName e, thisName]`
    /// (`Symbols.hs:66`): one node, two keys, and the second is what
    /// `infer x@ThisF{}` looks up (`Type.hs:1269-1272`).
    #[test]
    fn a_class_is_filed_under_two_keys() {
        let i = Interner::new();
        let tree = lower_str("class C { uint32 a; }\n", &i);
        assert_eq!(symbol_keys(SymbolsConfig::AllSymbols, &tree, &i), ["C", "C::$this", "C::a"]);
    }

    /// A `Template` and the declaration it wraps are filed under the **same** key,
    /// with the `Template` consed first (`Symbols.hs:44,50`) — so first-wins makes
    /// `Foo` resolve to the template, which is what the parser's `<` oracle needs
    /// (`Parser.hs:670`, `docs/spec/resolve.md` §7.1).
    #[test]
    fn a_template_and_its_target_share_a_key_and_the_template_wins() {
        let i = Interner::new();
        let tree = lower_str("template <auto N> struct Aa { uint32 x; }\n", &i);
        assert_eq!(
            keys(&tree, &i),
            ["Template Aa", "TemplateParam N", "Struct Aa", "Variable(DeclareMember) $quasi0::x"]
        );

        let trees = [(ModuleSource::File(FileId(0)), tree.clone())];
        let symbols = global_symbols(SymbolsConfig::AllSymbols, &trees, &i);
        let key = SymbolPath::single(i.intern("Aa"));
        let found = symbols.get(&key).expect("Aa is in the map");
        assert_eq!(tree.get(found.item).kind, DefKind::Template);
        // A template parameter is in *neither* map: `getSymbols''` has no case for
        // it and it never reaches `parseSymbol` (`Parser.hs:362`).
        assert!(!symbols.contains_key(&SymbolPath::single(i.intern("N"))));
        assert!(!tree.module_symbols().contains_key(&SymbolPath::single(i.intern("N"))));
    }

    /// `functionTemplate` seeds `self` — the **`Template` node**, not the
    /// function it wraps (`Parser.hs:428-430`) — and `classTemplate` does the same
    /// with `x : filter …` (`Parser.hs:327`). Seeding the target instead would make
    /// a recursive `f<…>` inside the body read as a non-template, and the parser's
    /// `<` oracle turns on exactly that bit (`Parser.hs:670`).
    #[test]
    fn a_template_seeds_itself_not_the_declaration_it_wraps() {
        let i = Interner::new();
        for src in [
            "template <auto N> inline void f() {}\n",
            "template <auto N> class f { uint32 a; }\n",
        ] {
            let tree = lower_str(src, &i);
            let f = i.intern("f");
            let seeded: Vec<ItemId> = tree
                .scopes
                .iter()
                .filter_map(|s| s.names.get(&f).copied())
                .collect();
            assert!(!seeded.is_empty(), "{src}: nothing was seeded");
            for id in seeded {
                assert_eq!(
                    tree.get(id).kind,
                    DefKind::Template,
                    "{src}: the self-reference must be the Template node"
                );
            }
        }
    }

    /// The three `SymbolsConfig` rules (`Symbols.hs:46-60`) on one input.
    #[test]
    fn the_three_symbols_configs_differ_exactly_as_symbols_hs_says() {
        let i = Interner::new();
        let tree = lower_str("template <auto N> class Aa { uint32 x; }\n", &i);

        // Everything: the Template's key (which the class shares), `$this`, and
        // the member.
        assert_eq!(
            symbol_keys(SymbolsConfig::AllSymbols, &tree, &i),
            ["Aa", "Aa::$this", "Aa::x"]
        );
        // `NoTemplate`: `tail` drops the Template's key *and* the class's own,
        // leaving the body. `$this` goes with the class.
        assert_eq!(symbol_keys(SymbolsConfig::NoTemplate, &tree, &i), ["Aa::x"]);
        // `TemplateDecl`: `const mempty` keeps the Template's key and nothing
        // below it.
        assert_eq!(symbol_keys(SymbolsConfig::TemplateDecl, &tree, &i), ["Aa"]);
    }

    /// `exportType` is `parseSymbol`-ed under `$exportN` (`SymbolTable.hs:213`)
    /// so repeated `export T;` never collide — but `getSymbols''` has no
    /// `ExportType` case (`Symbols.hs:52-68`), so it is in the module's own map
    /// and *not* in the global one.
    #[test]
    fn exports_are_numbered_and_live_only_in_the_module_map() {
        let i = Interner::new();
        let tree = lower_str("export Foo;\nexport Bar;\n", &i);
        assert_eq!(keys(&tree, &i), ["ExportType $export0", "ExportType $export1"]);
        assert!(symbol_keys(SymbolsConfig::AllSymbols, &tree, &i).is_empty());

        let module = tree.module_symbols();
        assert!(module.contains_key(&SymbolPath::single(i.intern("$export0"))));
        assert!(module.contains_key(&SymbolPath::single(i.intern("$export1"))));
        // `exported ExportType{} = True` (`SymbolTable.hs:175`).
        assert!(tree.has_export_or_reset);
    }

    /// `exported (Function _ _ attr …) | any reset (unfix attr)`
    /// (`SymbolTable.hs:176-181`): a `[[reset]]` function alone makes importing
    /// the module non-redundant.
    #[test]
    fn a_reset_function_makes_the_module_needed() {
        let i = Interner::new();
        assert!(lower_str("[[reset]] void Init() {}\n", &i).has_export_or_reset);
        assert!(!lower_str("void Init() {}\n", &i).has_export_or_reset);
    }

    /// `exposable (Variable _ _ typ _ _ _) = isConst typ` (`SymbolTable.hs:168`),
    /// with `isConst ConstF{} = True` and nothing else
    /// (`Parser/Syntax.hs:658-659`). It is what stops
    /// `module a { … }` re-exporting mutable global state.
    #[test]
    fn only_const_variables_are_exposable() {
        let i = Interner::new();
        let tree = lower_str("const auto A = 1;\nuint32 B;\nvoid f() {}\n", &i);
        let exposable: Vec<(String, bool)> = tree
            .items
            .iter()
            .map(|it| (i.resolve(it.qualified_name.last().unwrap()).to_string(), it.exposable))
            .collect();
        assert_eq!(
            exposable,
            [("A".to_string(), true), ("B".to_string(), false), ("f".to_string(), true)]
        );
    }

    /// Imports are stored in **source** order with ascending [`ImportId`]s, and
    /// `needed` is left to [`crate::module::ModuleGraph::add_import`].
    #[test]
    fn imports_keep_source_order() {
        let i = Interner::new();
        let tree = lower_str("import a1.a1\nimport a1.a8 as q\n", &i);
        assert_eq!(tree.imports.len(), 2);
        assert_eq!(tree.imports[0].id, ImportId(0));
        assert_eq!(tree.imports[0].name, vec![SmolStr::new("a1"), SmolStr::new("a1")]);
        assert_eq!(tree.imports[0].alias, None);
        assert_eq!(tree.imports[1].id, ImportId(1));
        assert_eq!(tree.imports[1].alias, Some(i.intern("q")));
        assert_eq!(tree.imports[1].needed, ImportNeeded::Redundant);
    }

    /// A module-name component may contain `-` and the two synthetic modules
    /// begin with `.` (`Parser/Lexer.hs:230-233`, `Parser.hs:41-45`).
    #[test]
    fn module_paths_keep_dashes_and_the_leading_dot() {
        let i = Interner::new();
        let dashed = lower_str("import with-dash\n", &i);
        assert_eq!(dashed.imports[0].name, vec![SmolStr::new("with-dash")]);

        let synthetic = lower_str("import .cmdargs\n", &i);
        assert_eq!(synthetic.imports[0].name, vec![SmolStr::new(".cmdargs")]);

        let dotted = lower_str("import a1.b2.c1\n", &i);
        assert_eq!(
            dotted.imports[0].name,
            vec![SmolStr::new("a1"), SmolStr::new("b2"), SmolStr::new("c1")]
        );
    }

    /// `HashMap.unions` is left-biased over the program list (`Symbols.hs:41`) and
    /// the list is in parse-completion order (`Parser/IO.hs:96`), so the **earlier
    /// file wins** a duplicate key.
    #[test]
    fn the_earlier_file_wins_a_duplicate_key() {
        let i = Interner::new();
        let first = lower_str("const auto Dup = 1;\n", &i);
        let second = lower_str("void Dup() {}\n", &i);
        let trees = [
            (ModuleSource::File(FileId(0)), first),
            (ModuleSource::File(FileId(1)), second),
        ];
        let symbols = global_symbols(SymbolsConfig::AllSymbols, &trees, &i);
        let found = symbols[&SymbolPath::single(i.intern("Dup"))];
        assert_eq!(found.module, ModuleSource::File(FileId(0)));
    }

    /// `getScope ErrorF{} = []` (`Parser/Syntax.hs:778`), so every error node in
    /// the file collapses to the single key `["$errors"]` —
    /// `hasErrorSymbol`'s flag (`Symbols.hs:31-35`).
    #[test]
    fn any_error_node_produces_the_one_errors_key() {
        let i = Interner::new();
        let parse = kg_syntax::parse_file("void f() { ] }\n");
        let tree = lower_file(ModuleSource::File(FileId(0)), &parse.source_file(), &i);
        assert!(tree.has_errors());
        assert_eq!(
            symbol_keys(SymbolsConfig::AllSymbols, &tree, &i)
                .iter()
                .filter(|k| *k == "$errors")
                .count(),
            1
        );
    }

    /// The `Program` view addresses items across files by [`DefId`].
    #[test]
    fn a_program_addresses_items_across_files() {
        let i = Interner::new();
        let trees = [
            (ModuleSource::File(FileId(0)), lower_str("void a() {}\n", &i)),
            (ModuleSource::Cmdargs, lower_str("const auto N = 8;\n", &i)),
        ];
        let program = Program::new(&trees);
        assert_eq!(program.items().count(), 2);
        let (def, item) = program.items().nth(1).unwrap();
        assert_eq!(def.module, ModuleSource::Cmdargs);
        assert_eq!(program.item(def).unwrap().qualified_name, item.qualified_name);
    }
}


