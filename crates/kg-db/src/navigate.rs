//! Position → declaration: the one query every semantic editor feature is built
//! on.
//!
//! # Why this is not a call to pass 9
//!
//! Pass 9 (`passes::undefined_symbols`) already answers "does this name resolve"
//! — but only as a `bool`, and its first source is deliberately coarse: an
//! `FxHashSet<Symbol>` of every name the file declares *anywhere*
//! (`declared_here`). A yes/no answer does not need to know which declaration,
//! and the set is an over-approximation on purpose, because a missed rejection
//! costs one conformance assertion and an invented one costs 241.
//!
//! Navigation cannot use that set: it holds names, not declarations, and the
//! case it is hiding is the one an editor is asked about most — **locals and
//! parameters**. Those are function-scope, are in no [`SymbolMap`] key an
//! outside-in probe can reach, and need a real walk up the enclosing scopes from
//! the cursor.
//!
//! So sources 2-4 below *are* pass 9's sources 2-4, reached through the same
//! [`SymbolMap`] and [`kg_resolve::ModuleGraph`] calls. Source 1 is replaced by
//! the scope walk the reference performs at parse time.
//!
//! # The order, and where it comes from
//!
//! `findSymbol` (`SymbolTable.hs:227-278`) splits first on whether the written
//! name has a qualifier:
//!
//! ```haskell
//! findInSymbolTable s SymbolTable{..}
//!     | null $ getQualifier s = findInScopes scopes >>= checkImportQualifiers
//!     | otherwise             = findInSymbols
//! ```
//!
//! so a **qualified** name skips the scope stack entirely (`SymbolTable.hs:243-244`)
//! and an unqualified one starts there:
//!
//! | # | source | reference | pass 9 |
//! |---|---|---|---|
//! | 1 | the enclosing scopes at the cursor, innermost first | `findInScopes` (`SymbolTable.hs:256-259`) | source 1 |
//! | 2 | the whole-program map at the written key | `lookupSymbol` (`Symbols.hs:76`) | source 2 |
//! | 3 | `lookupInScope` outward from this module's namespace | `Symbols.hs:79-87` | source 3 |
//! | 4 | the imports | `findInImports` (`SymbolTable.hs:263-270`) | source 4 |
//!
//! Keeping the order identical is the reason this lives in `kg-db` and not in
//! the language server: goto-definition and the `"name not in scope"` diagnostic
//! are then two readings of one algorithm, and **cannot disagree**. The
//! invariant is pinned by `navigate::tests::a_name_pass_9_rejects_never_resolves`
//! at every offset pass 9 reports, not at a hand-picked one. Pass 9 is the
//! loop's since sprint W3 step 1 (`kg_sem::check`, `lookupInScope` over the
//! loop's program); the CST pass this table was first written against, and
//! its `is_known`, are deleted.
//!
//! # Decline rather than guess
//!
//! A wrong jump is worse than no jump — worse than the false-positive asymmetry
//! that governs the passes, because a diagnostic is at least visible as a claim,
//! while a jump to the wrong `x` silently teaches the reader something untrue.
//! Every case this cannot decide returns `None`; they are enumerated on
//! [`Db::definition_at`].
//!
//! # Why the scope walk is a CST walk and not a [`kg_resolve::ScopeStack`] read
//!
//! Because [`kg_resolve::ScopeData`] says so:
//!
//! > It is not constant for the life of the scope. […] Anything resolving a name
//! > at a given source offset must re-walk with a live `ScopeStack` rather than
//! > read this.
//!
//! A finished [`ItemTree`] keeps each scope's *end* state and records no source
//! extent for a scope, so there is no offset → [`kg_resolve::ScopeId`] map to
//! read. What it does record exactly is, per declaration, the **name token's
//! range** ([`kg_resolve::Item::range`]) — so the scope structure is recovered
//! from the CST and every declaration *fact* still comes from the item tree.
//! Nothing here re-decides what a declaration is or what it is called.

use kg_base::{FileId, FileRange, FxHashMap, Symbol, SymbolPath, TextRange, TextSize};
use kg_resolve::{DefId, DefKind, Item, ItemId, ItemTree, ModuleSource, SymbolsConfig};
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, SyntaxToken, ast};
use kg_ty::Ty;

use crate::Db;
use crate::program::Program;
use crate::symbols::SymbolMap;

/// A declaration, located.
///
/// [`Definition::range`] is the declaration's **name token**, not its whole
/// subtree: that is what [`kg_resolve::Item::range`] stores (`symbolLoc`,
/// `SymbolTable.hs:199`) and what an editor wants to select on a jump.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Definition {
    /// The name token of the declaration. Its `file` is
    /// [`FileId::SYNTHETIC`](kg_base::FileId::SYNTHETIC) when the declaration
    /// lives in one of the two generated modules (`.cmdargs`, `.options`), which
    /// have no on-disk home (`Parser/IO.hs:101-102`).
    pub range: FileRange,
    /// `getScope e ++ [getName e]` (`Symbols.hs:70-71`) — the global symbol map
    /// key, carrying the mangled namespace (`@a@b`) and every synthetic
    /// component (`$`, `$quasiN`, `?lambdaN`).
    pub qualified_name: SymbolPath,
    pub kind: DefKind,
    /// The declaration itself, for callers that need to ask the program more
    /// about it — hover wants [`kg_resolve::Item`], find-references wants a key
    /// to compare against.
    pub def: DefId,
}

impl Definition {
    /// The definition an already-resolved [`DefId`] denotes.
    pub(crate) fn of(def: DefId, data: &Item) -> Definition {
        Definition::new(def.module, def.item, data)
    }

    fn new(module: ModuleSource, item: ItemId, data: &Item) -> Definition {
        Definition {
            range: data.range,
            qualified_name: data.qualified_name.clone(),
            kind: data.kind,
            def: DefId::new(module, item),
        }
    }
}

impl Db {
    /// The declaration the name under `offset` in `file` resolves to, or `None`
    /// when this query cannot decide.
    ///
    /// `roots` is the entry-file set the program is loaded from — the same
    /// argument [`Db::check`] and [`Db::diagnostics`] take, because the answer
    /// depends on which modules are in the program.
    ///
    /// # What resolves
    ///
    /// A single-segment name in value or type position, resolved through the
    /// four sources of the module docs; the cursor sitting on a declaration's own
    /// name token, which returns that declaration; and a multi-segment path whose
    /// **prefix up to the segment under the cursor** is a key in the symbol map
    /// or is exposed by an import — `C::x`, `m::X`, `a::b::C`.
    ///
    /// # A member after a `.`
    ///
    /// `a.f` and `a.f(…)` name `f` inside the type of `a` — a field through
    /// `tLookupField` (`Type.hs:961-971`), a method through `method`'s
    /// `lookupSymbolByName (x ++ [y])` (`Type.hs:1358-1362`) — never a symbol
    /// in scope, so the answer is read off the receiver's type: the
    /// program-wide loop's where the editor has it (`Db::editor_loop`), which
    /// knows the instance a template's name denotes and what an
    /// instantiated call returns; where it has no type, no member is
    /// reached (the CST engine answered fields there until sprint W3.7).
    /// The declaration is the one
    /// the member was written as: for an instance's copy, the template's
    /// member, a member template's `template <…>` included.
    ///
    /// Inside a template's body the receiver has a type per instance, and
    /// two instances may reach two declarations — a class that declares
    /// `go` in each branch of a `static if`, two structs that each declare
    /// an `x`. This query answers only where they agree, a pick between
    /// them being a guess about which instance the reader means;
    /// [`Db::definitions_at`] answers with every one of them.
    ///
    /// # What returns `None`, and why
    ///
    /// * **a later path segment nothing keys.** Only the prefix ending at the
    ///   cursor is probed, never the bare leading segment: `C::nosuch` with the
    ///   cursor on `nosuch` declines rather than jumping to `C`. Pass 9 judges
    ///   only the leading segment for the opposite reason — it must not
    ///   *reject* what it cannot see (`undefined_symbols`, module docs);
    /// * **an import qualifier.** `m` in `m::X` is bound by `import a as m`
    ///   (`SymbolTable.hs:276-278`) and is not a declaration in any map;
    /// * **a member whose receiver the loop does not type** — text no loop
    ///   has read, a buffer opened broken — or whose instances reach more
    ///   than one declaration (above);
    /// * **the intrinsics** — `__print`, `assert`, `__cycles`, `__str_cnt`,
    ///   `__assert_str_eq` and `this` are typed from a table
    ///   (`Type.hs:1074-1082`) and have no declaration to jump to;
    /// * **a declaration under a `static if`**, seen from outside that branch. A
    ///   braced branch is `quasiScoped` (`Parser.hs:467,593-595`), so the
    ///   declaration is keyed `[…, $quasiN, x]` and no outward walk from the use
    ///   site reaches it. The reference does not reach it either until pass 16
    ///   has elaborated the branch away and the survivors have been re-resolved
    ///   — which is exactly the elaboration this crate does not perform
    ///   (`crate::facts::reached_only_after_elaboration`);
    /// * **an ambiguous name.** Two distinct declarations reached through the
    ///   imports is `Resolution::Ambiguous` (`SymbolTable.hs:233-235`), an error
    ///   in the reference; picking one of them would be the guess this query
    ///   exists not to make;
    /// * a cursor that is not on an identifier at all.
    ///
    /// # Not memoized
    ///
    /// Like [`Db::check`], and for the same reason: the expensive inputs
    /// ([`Db::parse`], [`Db::item_tree`], [`Db::program`],
    /// [`Db::global_symbols`]) are each memoized already, and what is left is one
    /// walk over one file's tokens per request.
    pub fn definition_at(
        &self,
        roots: &[FileId],
        file: FileId,
        offset: TextSize,
    ) -> Option<Definition> {
        let mut found = self.definitions_at(roots, file, offset);
        if found.len() == 1 { found.pop() } else { None }
    }

    /// Go to type definition: the declarations of the type the program-wide
    /// loop gives the name under `offset` — the struct, union, class or enum
    /// it is, through `const`, arrays and an instance to the template's
    /// text ([`crate::looped::Looped::type_declaration`]); inside a
    /// template's body each instance's, one per declaration. Empty where
    /// the caret names nothing, the loop has no type for it, or the type is
    /// no declaration's (a builtin).
    pub fn type_definitions_at(&self, roots: &[FileId], file: FileId, offset: TextSize) -> Vec<Definition> {
        use crate::looped::IrAnswer;
        let Some(def) = self.definition_at(roots, file, offset) else { return Vec::new() };
        let Some(looped) = self.navigation_loop(roots) else { return Vec::new() };
        let types = match self.ir_answer_of_definition(&looped, &def, &|_| true) {
            Some(IrAnswer::Written(ty)) => vec![ty],
            Some(IrAnswer::Copies { types, .. }) => types.into_iter().map(|(ty, _)| ty).collect(),
            None => Vec::new(),
        };
        let mut found: Vec<Definition> = Vec::new();
        for ty in types {
            let Some((module, range, kind)) = looped.type_declaration(&ty, self.interner()) else { continue };
            if let Some(d) = self.definition_of_declaration(module, range, kind)
                && !found.contains(&d)
            {
                found.push(d);
            }
        }
        found
    }

    /// The module file the `import` under `offset` brought in — the caret on
    /// its module path or its alias — as the program rooted at `roots`
    /// resolved it (`ModuleGraph::import_target`): the search the compiler
    /// made, not a second one. `None` off an import, for one that did not
    /// resolve, and for the generated modules, which have no file.
    /// `definitions_at` declines here, since an import names a module, not
    /// a declaration.
    pub fn import_at(&self, roots: &[FileId], file: FileId, offset: TextSize) -> Option<FileId> {
        let root = self.parse(file).syntax_node();
        let token = identifier_at(&root, offset)?;
        let decl = token.parent_ancestors().find_map(ast::ImportDecl::cast)?;
        let keyword = decl.import_kw_token()?.text_range();
        let program = self.program(roots);
        program
            .graph
            .resolved_imports(ModuleSource::File(file))
            .find(|(d, _)| d.range.file == file && d.range.range == keyword)
            .and_then(|(_, target)| target.file())
    }

    /// Every declaration the name under `offset` reaches: [`Db::definition_at`]'s
    /// answer, except that a member after a `.` whose receiver's instances
    /// reach several declarations answers with each of them — once, in the
    /// text's order — where `definition_at` declines. What go-to-definition
    /// offers: a list is a fact about the text, as a pick from it would not
    /// be.
    pub fn definitions_at(&self, roots: &[FileId], file: FileId, offset: TextSize) -> Vec<Definition> {
        self.definitions_at_through(&|| self.navigation_loop(roots), roots, file, offset)
    }

    /// [`Db::definitions_at`], a member after a `.` read through the loop
    /// `looped` hands back rather than the editor's for `roots` — asked
    /// only for such a member, so a search that never meets one never
    /// runs one. A workspace search reads a file nobody has open through
    /// [`Db::loop_once`], which leaves no loop behind it (`kg-lsp`'s
    /// `references`).
    pub fn definitions_at_through(
        &self,
        looped: &dyn Fn() -> Option<crate::looped::EditorLoop>,
        roots: &[FileId],
        file: FileId,
        offset: TextSize,
    ) -> Vec<Definition> {
        let module = ModuleSource::File(file);
        let root = self.parse(file).syntax_node();
        let tree = self.item_tree(module);
        let Some(token) = identifier_at(&root, offset) else { return Vec::new() };

        // Goto-definition *on* a definition returns it. Checked by range rather
        // than by node kind so that a lambda capture — whose declaration is a
        // `NAME_REF`, not a `NAME` (`Parser.hs:447`) — is covered by the same
        // test.
        let declarations = declarations_by_name_token(&tree);
        if let Some(&item) = declarations.get(&token.text_range()) {
            return vec![Definition::new(module, item, tree.get(item))];
        }

        // A `.` member access is not a path: `a.b` names `b` *inside the type
        // of `a`* (`tLookupField`, `Type.hs:961-971`; `method`,
        // `Type.hs:1358-1362`), and the reference's symbol map holds no struct
        // or union field at all (`Symbols.hs:48-49`). So the answer is read
        // off the base expression's type — the program-wide loop's where the
        // editor has it, pass 6's otherwise — and the aggregate's own
        // declaration, never off a name lookup.
        if let Some(members) = self.member_definitions(looped, module, &token) {
            return members;
        }

        // A name in the module's exposure list (`module m { x }`) stands for
        // what `checkSymbols` finds for it (`Parser.hs:216-223`) — the
        // module's own `x`, or an import's it re-exposes — and a rename of
        // `x` that left it would leave "undefined symbol" there. Every one,
        // where it finds several, which `definition_at` then declines.
        if let Some(header) = &tree.module_header
            && let Some(&(name, _)) = header.exported_at.iter().find(|(_, at)| *at == token.text_range())
        {
            let program = self.program(roots);
            return program.graph.exposed_definitions(module, name).into_iter().filter_map(|d| definition_of(&program, d)).collect();
        }

        let Some(prefix) = path_prefix(&token) else { return Vec::new() };
        let Some(last) = prefix.last() else { return Vec::new() };
        let name = self.interner().intern(last.text());
        let written =
            SymbolPath::new(prefix.iter().map(|t| self.interner().intern(t.text())));

        // 1. `findInScopes` (`SymbolTable.hs:256-259`) — and only for an
        //    unqualified name: `| null $ getQualifier s` is what selects it, and
        //    a qualified name goes straight to `findInSymbols`
        //    (`SymbolTable.hs:243-244`). Without that guard a local `x` would
        //    answer `C::x`.
        if written.len() == 1
            && let Some(item) = local_binding(&tree, &declarations, &token, name)
        {
            return vec![Definition::new(module, item, tree.get(item))];
        }

        let program = self.program(roots);
        let symbols = self.global_symbols(roots, SymbolsConfig::AllSymbols);
        resolve_globally(&program, &symbols, module, &written).into_iter().collect()
    }

    /// The loop navigation reads for `roots`' program: the one the
    /// diagnostics of a root run ([`Db::editor_loop`]) — on a root that does
    /// not parse, the last one, mapped — gated on the root and not on the
    /// file the name is in. A references search resolves every file a
    /// buffer's program reaches, and a clean import under a buffer that does
    /// not parse is a loop that buffer's diagnostics never ran — the 900 ms
    /// on `risc-v.k` a broken keystroke does not pay.
    fn navigation_loop(&self, roots: &[FileId]) -> Option<crate::looped::EditorLoop> {
        roots.iter().find_map(|&root| self.editor_loop(roots, root))
    }

    /// The declarations a `FIELD_EXPR`'s selector reaches, or `None` when
    /// `token` is not one or no engine types the base; an empty list where
    /// the base's type holds no such member.
    ///
    /// Class members resolve the same way: a class's members *are* in the
    /// map, keyed under the class (`Symbols.hs:66`), but `c.x` is still
    /// answered by `c`'s type rather than by a lookup of `x`, which is what the
    /// reference does too (`Type.hs:1232-1235`). The type is the loop's — on
    /// a buffer that does not parse, the last loop's, mapped (sprint W3.4);
    /// the CST engine's half, which answered fields where the loop had no
    /// type, went with the engine (sprint W3.7).
    fn member_definitions(
        &self,
        looped: &dyn Fn() -> Option<crate::looped::EditorLoop>,
        module: ModuleSource,
        token: &SyntaxToken,
    ) -> Option<Vec<Definition>> {
        let base = selector_base(token)?;
        looped().and_then(|looped| self.loop_member(&looped, module, &base, token.text()))
    }

    /// The declarations the member `name` of `base`'s type is written as,
    /// as `looped` has the type: `None` where the loop has no type for the
    /// base, an empty list where it has one and the member is not there.
    /// Inside a template's body the base has a type per instance, and each
    /// instance's member is listed once — usually one, the template's own
    /// written member; two where the instances' members are written apart —
    /// in the text's order, which a list of places to jump to is read in.
    fn loop_member(
        &self,
        looped: &crate::looped::EditorLoop,
        module: ModuleSource,
        base: &SyntaxNode,
        name: &str,
    ) -> Option<Vec<Definition>> {
        use crate::looped::IrAnswer;
        let interner = self.interner();
        let types = match self.receiver(looped, module, base)? {
            IrAnswer::Written(ty) => vec![ty],
            IrAnswer::Copies { types, .. } => types.into_iter().map(|(ty, _)| ty).collect(),
        };
        let mut found: Vec<Definition> = Vec::new();
        for ty in types {
            for m in looped.members(&ty, interner).unwrap_or_default() {
                if interner.resolve(m.name).as_str() != name {
                    continue;
                }
                let Some(def) = self.member_declaration(&m) else { continue };
                if !found.contains(&def) {
                    found.push(def);
                }
            }
        }
        found.sort_by_key(|d| (d.range.file, d.range.range.start()));
        Some(found)
    }

    /// The receiver's type as `looped` has it — inside a template's body,
    /// each instance's — or `None` where the loop has no type for the
    /// base, or names an aggregate it does not declare.
    fn receiver(
        &self,
        looped: &crate::looped::EditorLoop,
        module: ModuleSource,
        base: &SyntaxNode,
    ) -> Option<crate::looped::IrAnswer> {
        use crate::looped::IrAnswer;
        let interner = self.interner();
        let answer = self.base_answer(looped, module, base)?;
        if let IrAnswer::Written(ty) = &answer
            && looped.members(ty, interner).is_none()
            && crate::complete::is_aggregate(ty)
        {
            return None;
        }
        Some(answer)
    }

    /// The loop's type of a member access's base, to a reader that asks
    /// for an aggregate ([`crate::complete::decided`]): at the base's own
    /// node, or — where that has none, as for a base typed since the loop
    /// ran (`a.` mid-statement, which the loop never saw) — at the
    /// declaration a base that is a plain name resolves to, a name's type
    /// being its declaration's (`typeOfSymbol`, `Type.hs:1243-1245,1336`).
    /// Inside a template's body both are each instance's. `None` where
    /// neither has a type.
    pub(crate) fn base_answer(
        &self,
        looped: &crate::looped::EditorLoop,
        module: ModuleSource,
        base: &SyntaxNode,
    ) -> Option<crate::looped::IrAnswer> {
        let interner = self.interner();
        let decided = &crate::complete::decided;
        if let Some(answer) = looped.answer_at(module, base.text_range(), base.kind(), interner, decided) {
            return Some(answer);
        }
        let name = plain_name(base)?;
        let def = self.definition_at(looped.roots(), module.diagnostic_file(), name.text_range().start())?;
        self.ir_answer_of_definition(looped, &def, decided)
    }

    /// The declaration a member the loop listed was written as
    /// ([`Db::definition_of_declaration`]) — for an instance's copy the
    /// template's member, for a member template its `template <…>` — where
    /// that is a member a `.` can name: a field, or a method
    /// (`lookupSymbolByName (x ++ [y])`, `Type.hs:1359`).
    fn member_declaration(&self, m: &crate::looped::Member) -> Option<Definition> {
        let def = self.definition_of_declaration(m.module, m.range, m.kind)?;
        matches!(def.kind, DefKind::Variable(_) | DefKind::Function | DefKind::FunctionDecl | DefKind::Template)
            .then_some(def)
    }

    /// The type of the member the selector under `offset` names, as the
    /// receiver's instance has it in `looped`, to a reader that prints a
    /// type where `shown` says it can — the instance's own member, not the
    /// declaration's answer, which inside a class template is every
    /// instance's (`a.get` on a `Box<4>` is `Box<4>`'s `get`, whatever
    /// other `Box`es the program makes). Inside a template's body the
    /// receiver has a type per instance of the enclosing template, and the
    /// answer is each one's, grouped as [`crate::looped::IrAnswer::Copies`]
    /// groups a declaration's; an instance whose receiver has no such
    /// member is among the untyped. `None` where `offset` is on no selector,
    /// the loop has no type for the receiver, or nothing to show.
    pub fn member_answer_at(
        &self,
        looped: &crate::looped::EditorLoop,
        file: FileId,
        offset: TextSize,
        shown: &dyn Fn(&Ty) -> bool,
    ) -> Option<crate::looped::IrAnswer> {
        use crate::looped::IrAnswer;
        let root = self.parse(file).syntax_node();
        let token = identifier_at(&root, offset)?;
        let base = selector_base(&token)?;
        let interner = self.interner();
        let member = |ty: &Ty| {
            looped.members(ty, interner).unwrap_or_default().into_iter().find(|m| interner.resolve(m.name).as_str() == token.text())
        };
        let (receivers, mut untyped, budget_exhausted) = match self.receiver(looped, ModuleSource::File(file), &base)? {
            IrAnswer::Written(ty) => return IrAnswer::Written(member(&ty)?.ty).shown(shown),
            IrAnswer::Copies { types, untyped, budget_exhausted } => (types, untyped, budget_exhausted),
        };
        let mut types: Vec<(Ty, Vec<String>)> = Vec::new();
        for (ty, instances) in receivers {
            let Some(m) = member(&ty) else {
                untyped.extend(instances);
                continue;
            };
            match types.iter_mut().find(|(t, _)| *t == m.ty) {
                Some((_, given)) => given.extend(instances),
                None => types.push((m.ty, instances)),
            }
        }
        IrAnswer::Copies { types, untyped, budget_exhausted }.shown(shown)
    }

    /// The instance a member template's call compiles, where the selector
    /// under `offset` is the callee of a call — `fixture.Run<3>(t)` calls
    /// `Run<3>` — as [`crate::looped::Looped::callee_at`] reads it: the
    /// member the call's `FunctionSpecifier` names after `resolve` renamed
    /// it (`Template.hs:158-159`; `method`, `Type.hs:1358-1362`). `None`
    /// for a call of a plain method, whose member's own type
    /// ([`Db::member_answer_at`]) already says everything this would.
    pub fn member_callee_at(
        &self,
        looped: &crate::looped::EditorLoop,
        file: FileId,
        offset: TextSize,
    ) -> Option<crate::looped::IrAnswer> {
        let root = self.parse(file).syntax_node();
        let token = identifier_at(&root, offset)?;
        let access = selector_base(&token)?.parent()?;
        let call = ast::CallExpr::cast(access.parent()?)?;
        if call.callee()?.syntax() != &access {
            return None;
        }
        looped.callee_at(ModuleSource::File(file), call.syntax().text_range(), self.interner())
    }

    /// `typeOf` (`Type.hs:452`) at the declaration a [`Definition`] names,
    /// in the program-wide loop's written tree (`crate::looped`), reached
    /// through the module's position index (`kg_hir::position`) — outward
    /// from the name token to the node lowered from the declaration.
    ///
    /// # Why the declaration's node and not the cursor's
    ///
    /// A use site carries the type of the *expression*, which for `f` in `f(x)`
    /// is the call's result and for a name in type position is a `TType`
    /// wrapper. The declaration node carries what the name was declared to be,
    /// which is what "what is this?" asks — and what every other use of the
    /// name is inferred from (`Symbols.hs:70-71`), so the two are one value.
    ///
    /// `None` where the loop holds no such node (a module it did not lower,
    /// a declaration in a redefinition no program holds, one typed since the
    /// loop ran), which is "no answer", not [`Ty::Unresolved`]. The written
    /// tree's type only; the editor reads [`Db::ir_answer_of_definition`],
    /// which adds the copies' for a declaration inside a template's body,
    /// which the written tree never types (`Inference.hs:63`).
    ///
    /// Until sprint W3.7 the CST inference engine answered this too
    /// (`type_of_definition`), and the editor fell back on it where the
    /// loop had no answer; it is deleted, and the loop is the one engine.
    pub fn ir_type_of_definition(&self, looped: &crate::looped::EditorLoop, def: &Definition) -> Option<Ty> {
        let declaration = self.declaration_node(def)?;
        looped.written_at(def.def.module, declaration.text_range(), declaration.kind())
    }

    /// What `looped` says at the declaration `def` names, to a reader that
    /// prints a type where `shown` says it can
    /// ([`crate::looped::Looped::answer_at`]): the written declaration's
    /// type, or — for one inside a template's body — each instance's.
    /// `None` where the loop has nothing to show, and the editor shows no
    /// type.
    ///
    /// The loop is the caller's, read at the text the buffer holds: the
    /// editor's is [`Db::editor_loop`] — on a buffer that does not parse,
    /// the last loop, which has no answer at a declaration typed since.
    pub fn ir_answer_of_definition(
        &self,
        looped: &crate::looped::EditorLoop,
        def: &Definition,
        shown: &dyn Fn(&Ty) -> bool,
    ) -> Option<crate::looped::IrAnswer> {
        let declaration = self.declaration_node(def)?;
        looped.answer_at(def.def.module, declaration.text_range(), declaration.kind(), self.interner(), shown)
    }

    /// An enum constant's value, as `looped` folded it
    /// ([`crate::looped::Looped::enum_value_at`]); `None` for any other
    /// declaration, and where the loop has no literal for it.
    pub fn enum_constant_value(&self, looped: &crate::looped::EditorLoop, def: &Definition) -> Option<kg_const::ConstInt> {
        if def.kind != DefKind::EnumConstant {
            return None;
        }
        let declaration = self.declaration_node(def)?;
        looped.enum_value_at(def.def.module, declaration.text_range(), declaration.kind())
    }

    /// The declaration a [`Definition`] names: from [`Item::range`] — the name
    /// token (`symbolLoc`, `SymbolTable.hs:199`) — two steps up. The token's
    /// parent is the `NAME`, or the `NAME_REF` a lambda capture is declared
    /// by (`Parser.hs:447`), and its parent is the declaration; the same step
    /// `bound_in` takes to find a declaration's binder.
    fn declaration_node(&self, def: &Definition) -> Option<SyntaxNode> {
        let root = self.parse_module(def.def.module).syntax_node();
        let name = def.range.range;
        root.token_at_offset(name.start())
            .find(|t| t.text_range() == name)
            .and_then(|t| t.parent())
            .and_then(|n| n.parent())
    }

    /// [`Db::declaration_node`] read backwards: the declaration whose node
    /// is the CST node at `range` with `kind` in `module` — where an IR
    /// node's `Src::head` says it was written, a template's copy included
    /// (`Src::expanded` keeps the head). The item is found by its name
    /// token, a child of the node's `NAME` (or a capture's `NAME_REF`),
    /// first-wins as [`declarations_by_name_token`] keys it.
    pub(crate) fn definition_of_declaration(
        &self,
        module: ModuleSource,
        range: TextRange,
        kind: SyntaxKind,
    ) -> Option<Definition> {
        let root = self.parse_module(module).syntax_node();
        if !root.text_range().contains_range(range) {
            return None;
        }
        let node = match root.covering_element(range) {
            kg_syntax::NodeOrToken::Node(n) => n,
            kg_syntax::NodeOrToken::Token(t) => t.parent()?,
        };
        let node = node.ancestors().find(|n| n.text_range() == range && n.kind() == kind)?;
        let name = node
            .children()
            .filter(|c| matches!(c.kind(), SyntaxKind::NAME | SyntaxKind::NAME_REF))
            .find_map(|c| c.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::IDENT))?;
        let tree = self.item_tree(module);
        let (index, item) = tree
            .items
            .iter()
            .enumerate()
            .find(|(_, item)| item.range.range == name.text_range() && !matches!(item.kind, DefKind::Error | DefKind::ExportType))?;
        Some(Definition::new(module, ItemId(index as u32), item))
    }
}

/// The name a base expression is, where it is a plain one — a path of one
/// segment and no template arguments, `a` in `a.` — or `None`.
fn plain_name(base: &SyntaxNode) -> Option<SyntaxToken> {
    if base.kind() != SyntaxKind::PATH {
        return None;
    }
    let [segment] = base.children().collect::<Vec<_>>().try_into().ok()?;
    let [name] = segment.children().collect::<Vec<_>>().try_into().ok()?;
    if segment.kind() != SyntaxKind::PATH_SEGMENT || name.kind() != SyntaxKind::NAME_REF {
        return None;
    }
    name.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::IDENT)
}

/// Sources 2-4 — pass 9's lookup minus its first source, probing the
/// written path only.
///
/// Pass 9 asks each source twice, for the bare leading segment *and* for the
/// path as written, because either spelling is enough to prove a name is not
/// undefined. Here the bare probe would be an answer about a different name than
/// the one under the cursor, so only the written prefix is asked. That makes
/// every source strictly narrower than pass 9's, which is what
/// `navigate::tests::a_name_pass_9_rejects_never_resolves` rests on.
fn resolve_globally(
    program: &Program,
    symbols: &SymbolMap,
    module: ModuleSource,
    written: &SymbolPath,
) -> Option<Definition> {
    // 2. The whole-program map at the exact key. `Main.hs:70,80` merges every
    //    command-line file into one program, so a declaration in a sibling root
    //    with no `module` header is keyed bare and is reachable here.
    if let Some(def) = symbols.get(written) {
        return definition_of(program, def);
    }

    // 3. `lookupInScope` from the referring module's own namespace
    //    (`Symbols.hs:79-87`) — a declaration inside `module m` is filed under
    //    `[@m, X]`, and a use of `X` from the same namespace resolves by walking
    //    outward from that scope, not by an exact probe.
    if let Some(namespace) = program.graph.interface(module).and_then(|i| i.namespace)
        && let Some(def) =
            symbols.lookup_in_scope(&SymbolPath::single(namespace.0), written)
    {
        return definition_of(program, def);
    }

    // 4. `findInImports` (`SymbolTable.hs:263-270`). The implicit base import is
    //    import #0 of every command-line file (`Parser/IO.hs:77`), so every
    //    base-library name arrives through here — as does every `m::X`, because
    //    `findInImport` strips the import alias off the front before prepending
    //    the namespace (`SymbolTable.hs:276-278`).
    //
    //    `nub` first (`SymbolTable.hs:231`): one declaration reached through two
    //    re-export chains is not ambiguous. What is left over is
    //    `Resolution::Ambiguous` — an error in the reference
    //    (`SymbolTable.hs:233-235`) and a guess here — so it declines. Sorting
    //    rather than preserving `nub`'s order is safe because only the *count*
    //    and the singleton are read; the order exists in the reference to lay
    //    out the ambiguity message, which this query does not produce.
    let mut hits = program.graph.find_in_imports(module, written);
    hits.sort_unstable();
    hits.dedup();
    match hits.as_slice() {
        [def] => definition_of(program, *def),
        _ => None,
    }
}

fn definition_of(program: &Program, def: DefId) -> Option<Definition> {
    program.items().item(def).map(|item| Definition::new(def.module, def.item, item))
}

/// The identifier the caret is inside or immediately after — `foo|(` means
/// `foo`.
///
/// Anything that is not an `IDENT` declines: a keyword, a literal and a `;` have
/// no declaration, and offering the nearest identifier instead would be a guess
/// about what the user pointed at.
///
/// Public because [`Db::definition_at`]'s callers need the *same* answer to
/// "which word is the caret on": a hover that underlines one token while
/// describing another, or a highlight that lights a word it did not resolve, is
/// its own small lie. A second copy in the language server would be free to
/// drift from the one that decides what resolves.
pub fn identifier_at(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    // `token_at_offset` panics outside the tree, and a client is entitled to
    // send a position from a document state this database has already moved
    // past.
    let offset = offset.min(root.text_range().end());
    root.token_at_offset(offset).find(|t| t.kind() == SyntaxKind::IDENT)
}

/// Whether `token` names a member after a `.` — the `f` of `a.f` — which
/// resolves through the receiver's type rather than through a scope.
pub fn is_member_selector(token: &SyntaxToken) -> bool {
    selector_base(token).is_some()
}

/// The receiver of the member `token` selects — `a` of `a.f` and of
/// `a.f<3>(…)` — or `None` when `token` is no `FIELD_EXPR`'s selector.
fn selector_base(token: &SyntaxToken) -> Option<SyntaxNode> {
    let name_ref = token.parent().filter(|p| p.kind() == SyntaxKind::NAME_REF)?;
    // `a.f` puts a bare `NAME_REF` under the `FIELD_EXPR`; `a.f<T>()` puts a
    // `PATH_SEGMENT` in between.
    let selector = match name_ref.parent() {
        Some(p) if p.kind() == SyntaxKind::PATH_SEGMENT => p,
        _ => name_ref,
    };
    let field_expr = selector.parent().filter(|p| p.kind() == SyntaxKind::FIELD_EXPR)?;
    // The selector is the *last* such child; the first one belongs to the base.
    let last = field_expr
        .children()
        .filter(|c| matches!(c.kind(), SyntaxKind::NAME_REF | SyntaxKind::PATH_SEGMENT))
        .last()?;
    if last != selector {
        return None;
    }
    Some(ast::FieldExpr::cast(field_expr)?.base()?.syntax().clone())
}

/// Every declaration in one file, keyed by its **name token's range**.
///
/// [`kg_resolve::Item::range`] is exactly that token (`symbolLoc`,
/// `SymbolTable.hs:199`), so this is a bijection back from the CST to the item
/// tree and the only bridge between the two halves of this module.
///
/// First-wins on a shared range, matching [`kg_resolve::global_symbols`]: a
/// `template <…> struct Foo` files *two* items under `Foo`'s name token — the
/// `Template` node and the `struct` it wraps (`Parser.hs:373-375`) — and the
/// pre-order-first one is the one a lookup finds (`Symbols.hs:41,44,50`).
///
/// Two kinds are left out because neither is a name anyone can point at:
/// [`DefKind::Error`], whose range is the whole file (`Symbols.hs:67`), and
/// [`DefKind::ExportType`], whose range is the `export T;` declaration and whose
/// name is the synthetic `$exportN` (`SymbolTable.hs:213`).
fn declarations_by_name_token(tree: &ItemTree) -> FxHashMap<TextRange, ItemId> {
    let mut out = FxHashMap::default();
    for (i, item) in tree.items.iter().enumerate() {
        if matches!(item.kind, DefKind::Error | DefKind::ExportType) {
            continue;
        }
        out.entry(item.range.range).or_insert(ItemId(i as u32));
    }
    out
}

/// `findInScopes` (`SymbolTable.hs:256-259`) at a source offset: walk the
/// enclosing scopes innermost → outermost and take the first that binds `name`.
///
/// The scopes are the CST's, because a finished [`ItemTree`] records no source
/// extent for one (see the module docs). Which declarations a scope binds is not
/// re-derived: it is every item of the tree whose [`binder`] is that node.
///
/// # Where this is *wider* than the reference, and why that is the safe side
///
/// The parse-time table holds only what has been declared so far, so
/// `{ x = 1; uint32 x; }` does not see `x` at the assignment. The reference
/// resolves it anyway one step later — `findInSymbols` probes `[main,$,x]` in a
/// map built over the finished program (`Symbols.hs:79-87`), which is
/// order-independent, and is the same mechanism that makes a forward reference
/// to a top-level `struct` work. Ordering the walk would therefore not decline
/// the case, only route it through a source that answers identically.
fn local_binding(
    tree: &ItemTree,
    declarations: &FxHashMap<TextRange, ItemId>,
    token: &SyntaxToken,
    name: Symbol,
) -> Option<ItemId> {
    token
        .parent()?
        .ancestors()
        .filter(|scope| introduces_scope(scope.kind()))
        .find_map(|scope| bound_in(&scope, tree, declarations, name))
}

/// The declaration `scope` binds **directly** under `name`, if any.
///
/// The subtree is scanned rather than indexed because the walk in
/// [`local_binding`] stops at the first scope that answers, and the innermost
/// scopes are small: a query about a local reads that block and nothing else,
/// and only a name that turns out to be file-scope pays for reading the file.
///
/// "Directly" is the whole content of this function, and it is what
/// [`binder`] decides: a local of a *sibling* function is a descendant of the
/// file scope but is bound by that function's body, and returning it here would
/// be exactly the wrong jump.
fn bound_in(
    scope: &SyntaxNode,
    tree: &ItemTree,
    declarations: &FxHashMap<TextRange, ItemId>,
    name: Symbol,
) -> Option<ItemId> {
    scope
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| token.kind() == SyntaxKind::IDENT)
        .find_map(|token| {
            let &item = declarations.get(&token.text_range())?;
            if tree.get(item).qualified_name.last() != Some(name) {
                return None;
            }
            let declaring = token.parent().and_then(|n| n.parent())?;
            (binder(&declaring).as_ref() == Some(scope)).then_some(item)
        })
}

/// The scope a declaration is bound *in* — the nearest enclosing construct from
/// the table of [`kg_resolve::lower_file`], starting **above** the declaration
/// itself.
///
/// Starting above is what puts a member function's name in its class rather than
/// in its own body, and a top-level `struct`'s name in the file scope rather
/// than in the struct.
///
/// `template <…>` is transparent to the declaration it wraps and opaque to its
/// own parameters. Both halves are `Parser.hs:352-375`: `templateParameters`
/// nests one `unscopedWith [x]` per parameter around `def`, so the parameters
/// live in a scope of their own, while the wrapped declaration is `symbol`-ed
/// into the scope the `template` keyword was written in — which is why
/// `template <typename T> struct Foo` declares `Foo` at file scope and `T`
/// nowhere outside the template.
pub(crate) fn binder(declaration: &SyntaxNode) -> Option<SyntaxNode> {
    let mut previous = declaration.clone();
    for ancestor in declaration.ancestors().skip(1) {
        if introduces_scope(ancestor.kind())
            && (ancestor.kind() != SyntaxKind::TEMPLATE_DECL
                || previous.kind() == SyntaxKind::TEMPLATE_PARAM_LIST)
        {
            return Some(ancestor);
        }
        previous = ancestor;
    }
    None
}

/// The constructs that push a scope, from the table on
/// [`kg_resolve::lower_file`] — restricted to the ones that can *bind a name*,
/// which is what a lookup walks.
///
/// The bookkeeping-only pushes are absent on purpose. The two `Unscoped` wrappers
/// around a function definition (`Parser.hs:408,414`), the `Unscoped` around an
/// alias RHS (`:276`) and the `Quasi` around a function *type*'s parameter list
/// (`:551`) exist to seed template parameters, to seed a template's
/// self-reference, or to move `scopeIndex`; none of them holds a declaration that
/// is not also held by a construct listed here, and a scope with nothing in it
/// cannot change which binding a walk finds first.
///
/// `SOURCE_FILE` stands for both the file scope (`SymbolTable.hs:94`) and the
/// `module A.B` scope, which `lower_file` pushes around the entire file.
pub(crate) fn introduces_scope(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::SOURCE_FILE
            | SyntaxKind::CLASS_DECL
            | SyntaxKind::STRUCT_DECL
            | SyntaxKind::UNION_DECL
            | SyntaxKind::ENUM_DECL
            | SyntaxKind::FN_DECL
            | SyntaxKind::FN_PROTO
            | SyntaxKind::LAMBDA_EXPR
            | SyntaxKind::TEMPLATE_DECL
            | SyntaxKind::BLOCK_STMT
            | SyntaxKind::FOR_STMT
            | SyntaxKind::STATIC_FOR_STMT
            | SyntaxKind::UNROLLED_FOR_STMT
            | SyntaxKind::SWITCH_CASE
            | SyntaxKind::SWITCH_DEFAULT
    )
}

/// The path segments up to **and including** the one the cursor is on, as their
/// identifier tokens.
///
/// `a::b::C` with the cursor on `b` yields `[a, b]`: the question an editor asks
/// is what the name *under the caret* denotes, and that is the prefix ending
/// there, never the whole path and never just its head.
///
/// Declines for any identifier that is not a path segment — a `.` field access
/// (`FIELD_EXPR`), a designated initializer, an attribute name, an import's
/// module path — and for a malformed path whose segments cannot all be read,
/// because a prefix with a hole in it is not the name that was written.
fn path_prefix(token: &SyntaxToken) -> Option<Vec<SyntaxToken>> {
    let name_ref = token.parent()?;
    if name_ref.kind() != SyntaxKind::NAME_REF {
        return None;
    }
    let segment = name_ref.parent()?;
    if segment.kind() != SyntaxKind::PATH_SEGMENT {
        return None;
    }
    let path = ast::Path::cast(segment.parent()?)?;

    let mut out = Vec::new();
    for segment in path.path_segments() {
        let ident = segment.name_ref()?.ident_token()?;
        let last = ident.text_range() == token.text_range();
        out.push(ident);
        if last {
            return Some(out);
        }
    }
    None
}


#[cfg(test)]
mod tests {
    use kg_base::{FileId, TextSize, Utf8PathBuf};
    use kg_config::Settings;
    use kg_resolve::{DeclareFlag, DefKind};

    use super::Definition;
    use crate::Db;

    /// A database rooted at `/proj` with `/proj` on the import path and no base
    /// library — the shape `tests/query.rs` uses.
    fn db() -> Db {
        let mut settings = Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        Db::new(settings)
    }

    fn write(db: &mut Db, name: &str, text: &str) -> FileId {
        db.set_file_text(&Utf8PathBuf::from(format!("/proj/{name}")), text.to_owned())
    }

    /// Strips the two markers out of a fixture: `$0` is the caret, `$1` is where
    /// the jump must land.
    ///
    /// `$` cannot appear in Kanagawa source — it is reserved for the synthetic
    /// names the lowering generates (`$quasiN`, `$this`, `$exportN`) — so a
    /// marker can never collide with the fixture, and writing the expected
    /// answer *in the fixture* keeps these tests readable when a byte offset
    /// would not be.
    fn markers(text: &str) -> (String, Option<TextSize>, Option<TextSize>) {
        let (mut out, mut caret, mut target) = (String::with_capacity(text.len()), None, None);
        let mut rest = text;
        while let Some(i) = rest.find('$') {
            out.push_str(&rest[..i]);
            let here = TextSize::new(out.len() as u32);
            match rest.as_bytes().get(i + 1) {
                Some(b'0') => caret = Some(here),
                Some(b'1') => target = Some(here),
                other => panic!("unknown marker `${other:?}` in {text:?}"),
            }
            rest = &rest[i + 2..];
        }
        out.push_str(rest);
        (out, caret, target)
    }

    /// Load `files`, root the program at `roots`, and resolve the `$0` caret —
    /// which must be in exactly one of the files.
    #[track_caller]
    fn resolve(files: &[(&str, &str)], roots: &[&str]) -> (Db, Option<Definition>) {
        let mut db = db();
        let mut ids: Vec<(&str, FileId)> = Vec::new();
        let mut caret: Option<(FileId, TextSize)> = None;
        for (name, text) in files {
            let (text, at, _) = markers(text);
            let file = write(&mut db, name, &text);
            ids.push((name, file));
            if let Some(at) = at {
                assert!(caret.is_none(), "two `$0` carets in one fixture");
                caret = Some((file, at));
            }
        }
        let (file, at) = caret.expect("the fixture needs one `$0` caret");
        let roots: Vec<FileId> = roots
            .iter()
            .map(|r| ids.iter().find(|(n, _)| n == r).expect("unknown root").1)
            .collect();
        let found = db.definition_at(&roots, file, at);
        (db, found)
    }

    /// The one-file case: resolve `$0` and require it to land exactly on `$1`.
    #[track_caller]
    fn jumps(source: &str) -> Definition {
        let (db, found) = resolve(&[("main.k", source)], &["main.k"]);
        let found = found.unwrap_or_else(|| panic!("the caret did not resolve in {source:?}"));
        let (text, _, target) = markers(source);
        let target = target.expect("the fixture needs a `$1` target");
        assert_eq!(db.parse(found.range.file).text(), text, "the jump left the file");
        assert_eq!(
            found.range.range.start(),
            target,
            "jumped to `{}` at {:?}, expected the declaration at {target:?}",
            &text[found.range.range],
            found.range.range,
        );
        found
    }

    /// The one-file case that must decline.
    #[track_caller]
    fn declines(source: &str) {
        let (db, found) = resolve(&[("main.k", source)], &["main.k"]);
        if let Some(found) = found {
            let text = db.parse(found.range.file).text();
            panic!("expected no definition in {source:?}, jumped to `{}`", &text[found.range.range]);
        }
    }

    // -----------------------------------------------------------------------
    // Source 1 — the scope walk (`findInScopes`, `SymbolTable.hs:256-259`)
    // -----------------------------------------------------------------------

    /// The case pass 9 structurally cannot answer: a function-body local is
    /// under no [`crate::SymbolMap`] key an outside-in probe reaches, and pass
    /// 9's first source knows only that the *name* occurs somewhere in the file.
    #[test]
    fn a_local_variable_resolves_to_its_declaration() {
        let found =
            jumps("inline void main()\n{\n    uint32 $1total = 1;\n    total = $0total + 1;\n}\n");
        assert_eq!(found.kind, DefKind::Variable(DeclareFlag::DeclareLocal));
    }

    /// A struct field is not a symbol — `getSymbols` files a `Struct` with
    /// `const mempty` (`Symbols.hs:48-49`) — so `s.x` is answered from the type
    /// of `s` (`tLookupField`, `Type.hs:961-971`) and the struct's own item
    /// tree, never from a name lookup. The jump lands on the field's name.
    #[test]
    fn a_struct_field_resolves_through_the_base_type() {
        let found = jumps(
            "struct S\n{\n    uint32 $1x;\n    bool y;\n}\n\ninline void main()\n{\n    S s = {};\n    uint32 v = s.$0x;\n}\n",
        );
        assert_eq!(found.kind, DefKind::Variable(DeclareFlag::DeclareMember));
    }

    /// The base may be nested and `const`: `c.s.x` reads `s` off `c`'s class
    /// type and `x` off `s`'s struct type, and a `const` object's field still
    /// has a declaration to land on (`tMember (TConst a)`, `Type.hs:940`).
    #[test]
    fn a_nested_field_of_a_const_object_resolves() {
        jumps(
            "struct S\n{\n    uint32 $1x;\n}\n\nclass C\n{\npublic:\n    S s;\n}\n\ninline void main()\n{\n    const C c = {};\n    uint32 v = c.s.$0x;\n}\n",
        );
    }

    /// A field name is not a declaration anything in scope can see: the same
    /// spelling used bare declines rather than jumping to the field.
    #[test]
    fn a_field_name_used_bare_does_not_resolve_to_the_field() {
        declines("struct S\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    uint32 v = $0x;\n}\n");
    }

    /// A parameter is a `FuncParam` keyed under the function name
    /// (`Parser.hs:415`), one scope out from the body the caret is in.
    #[test]
    fn a_parameter_resolves_to_its_declaration() {
        let found = jumps("inline void run(uint32 $1width)\n{\n    uint32 w = $0width;\n}\n");
        assert_eq!(found.kind, DefKind::FuncParam);
    }

    /// `findInScopes` takes the **first** scope holding the name and stops
    /// (`SymbolTable.hs:256-258`), so the outer `x` must lose.
    #[test]
    fn the_innermost_binding_wins() {
        jumps(
            "inline void main()\n{\n    uint32 x;\n    {\n        bool $1x;\n        x = $0x;\n    }\n}\n",
        );
    }

    /// A block that has *closed* is not on the caret's chain, so its locals are
    /// invisible. This is precisely what pass 9's source 1 cannot distinguish
    /// from the test above — both spell `hidden` somewhere in the file — and is
    /// therefore the reason navigation could not be built on it.
    #[test]
    fn a_binding_in_a_sibling_scope_is_not_visible() {
        declines(
            "inline void main()\n{\n    {\n        uint32 hidden;\n    }\n    hidden = $0hidden;\n}\n",
        );
    }

    /// The same, one function over: `other`'s local *is* a descendant of the
    /// file scope, and only [`super::binder`] keeps it out of the file scope's
    /// bindings.
    #[test]
    fn a_local_of_another_function_is_not_visible() {
        declines(
            "inline void other()\n{\n    uint32 elsewhere;\n}\n\ninline void main()\n{\n    uint32 y = $0elsewhere;\n}\n",
        );
    }

    /// A `template <…>` parameter is seeded into the scope stack and reaches
    /// **neither** symbol map (`Parser.hs:362`, `Symbols.hs:68`), so nothing but
    /// the scope walk can find it.
    #[test]
    fn a_template_parameter_resolves_inside_the_template() {
        let found = jumps("template <typename $1T>\nstruct Wrap\n{\n    $0T v;\n}\n");
        assert_eq!(found.kind, DefKind::TemplateParam);
    }

    /// …and the declaration a `template` wraps is bound in the scope the
    /// `template` keyword was written in, not inside the template
    /// (`Parser.hs:373-375`). Both keys sit on the same name token; the
    /// pre-order-first item is the `Template`, matching `getSymbols`'
    /// parent-beats-descendant merge (`Symbols.hs:41,44,50`).
    #[test]
    fn a_templates_own_name_is_bound_at_file_scope() {
        let found = jumps(
            "template <typename T>\nstruct $1Wrap\n{\n    T v;\n}\n\ninline void main()\n{\n    $0Wrap<uint32> w;\n}\n",
        );
        assert_eq!(found.kind, DefKind::Template);
    }

    /// A class member function's body sees the class's fields: `DECL_LIST`'s
    /// declarations are bound by the `CLASS_DECL`, which is on the chain.
    #[test]
    fn a_class_field_resolves_from_a_member_function() {
        let found = jumps(
            "class C\n{\n    uint32 $1field;\n\n    inline void go()\n    {\n        field = $0field;\n    }\n}\n",
        );
        assert_eq!(found.kind, DefKind::Variable(DeclareFlag::DeclareClass));
    }

    /// `rangeFor`'s `localScoped` wraps the induction variable *and* the body
    /// (`Parser.hs:584`), so the body sees it and the enclosing block does not.
    #[test]
    fn a_loop_induction_variable_resolves_from_the_body() {
        jumps(
            "inline void main()\n{\n    for (const auto $1i : 10)\n    {\n        __print($0i);\n    }\n}\n",
        );
        declines(
            "inline void main()\n{\n    for (const auto i : 10)\n    {\n    }\n    __print($0i);\n}\n",
        );
    }

    /// A lambda capture is declared by a `NAME_REF`, not a `NAME`
    /// (`Parser.hs:447`) — which is why the "caret is on a declaration" test is
    /// by range and not by node kind — and its `declScopeName` already carries
    /// the body's `$`, so the body resolves to the capture and not to the local
    /// it captured.
    #[test]
    fn a_lambda_capture_is_a_declaration_its_body_can_reach() {
        let found = jumps(
            "inline void main()\n{\n    uint32 c = 1;\n    auto f = [$1c](uint32 z) -> uint32 { return $0c; };\n}\n",
        );
        assert_eq!(found.kind, DefKind::Capture);
    }

    /// One `unscopedWith` per constant, each seeded with the previous
    /// (`Parser.hs:291`), so a constant may name an earlier sibling.
    #[test]
    fn an_enum_constant_resolves_from_a_later_constants_initializer() {
        let found = jumps("enum E : uint8\n{\n    $1A = 1,\n    B = $0A + 1\n}\n");
        assert_eq!(found.kind, DefKind::EnumConstant);
    }

    /// A function declared *inside* another function body is keyed
    /// `[main, $, Bar]` (`Parser.hs:415,424`), so no exact probe of the global
    /// map reaches it from the call site — only the scope walk does, and only
    /// because [`super::binder`] starts **above** the declaration. Started at
    /// the declaration, `Bar` would be bound inside its own body and be
    /// invisible to the call two lines below it.
    #[test]
    fn a_nested_function_resolves_from_its_enclosing_body() {
        let found = jumps(
            "inline void main()\n{\n    inline void $1Bar()\n    {\n    }\n\n    $0Bar();\n}\n",
        );
        assert_eq!(found.kind, DefKind::Function);
    }

    /// The same rule one level out: a member function is keyed `[C, helper]`
    /// and its sibling reaches it through the `CLASS_DECL`, not through the
    /// global map.
    #[test]
    fn a_member_function_resolves_from_a_sibling_member() {
        jumps(
            "class C\n{\n    inline void $1helper()\n    {\n    }\n\n    inline void run()\n    {\n        $0helper();\n    }\n}\n",
        );
    }

    /// `template def` wraps the *declaration*, and `symbol` is applied to the
    /// `Template` node in the scope the `template` keyword was written in
    /// (`Parser.hs:373-375`) — so a member template is bound by its class, not
    /// by its own `template <…>`. At file scope the global map would answer
    /// either way; inside a class it is the only source that can.
    #[test]
    fn a_member_template_is_bound_by_its_class_and_not_by_its_template() {
        let found = jumps(
            "class C\n{\n    template <typename T>\n    inline void $1go(T v)\n    {\n    }\n\n    inline void run()\n    {\n        $0go(1);\n    }\n}\n",
        );
        assert_eq!(found.kind, DefKind::Template);
    }

    /// `case`/`default` bodies are `localScoped` (`Parser.hs:602-603`).
    #[test]
    fn a_declaration_in_a_switch_case_resolves_within_that_case() {
        jumps(
            "inline void main()\n{\n    switch (1)\n    {\n        case 0:\n            uint32 $1s = 1;\n            __print($0s);\n    }\n}\n",
        );
    }

    // -----------------------------------------------------------------------
    // The caret on a declaration
    // -----------------------------------------------------------------------

    /// Goto-definition on a definition returns that definition rather than
    /// nothing — what every editor expects, and what a use-sites-only
    /// implementation silently lacks.
    #[test]
    fn the_caret_on_a_declaration_returns_that_declaration() {
        jumps("struct $1$0Point\n{\n    uint32 x;\n}\n");
        jumps("inline void $1$0main()\n{\n}\n");
        jumps("inline void run(uint32 $1$0width)\n{\n}\n");
        jumps("inline void main()\n{\n    uint32 $1$0local;\n}\n");
        jumps("inline void main()\n{\n    auto f = [$1$0this]() -> uint32 { return 1; };\n}\n");
    }

    // -----------------------------------------------------------------------
    // Sources 2-4 — the whole-program map and the imports
    // -----------------------------------------------------------------------

    /// A top-level declaration used from a function body: in none of the caret's
    /// inner scopes, found at the file scope.
    #[test]
    fn a_top_level_struct_resolves_from_a_function_body() {
        let found = jumps(
            "struct $1Point\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    $0Point p;\n}\n",
        );
        assert_eq!(found.kind, DefKind::Struct);
    }

    /// The parse-time table has not reached the declaration yet, but a scope's
    /// bindings are collected in full and `getSymbols` is built over the finished
    /// program (`docs/spec/resolve.md` §7.2). Resolving only what is declared
    /// *above* the caret would decline this.
    #[test]
    fn a_forward_reference_resolves_to_the_later_declaration() {
        jumps("inline void main()\n{\n    $0Point p;\n}\n\nstruct $1Point\n{\n    uint32 x;\n}\n");
        // …and inside a function body, where the parse-time table is the only
        // thing that could have answered.
        jumps("inline void main()\n{\n    x = $0later;\n    uint32 $1later;\n}\n");
    }

    /// Source 4 — `findInImports` (`SymbolTable.hs:263-270`). The declaration is
    /// in another file, and the answer carries that file's `FileId`.
    #[test]
    fn a_name_an_import_exposes_resolves_across_the_module_boundary() {
        let (db, found) = resolve(
            &[
                ("a.k", "module a\n    { module a\n    }\n\nstruct $1Aa\n{\n    uint32 x;\n}\n"),
                ("main.k", "import a\n\ninline void main()\n{\n    $0Aa x;\n}\n"),
            ],
            &["main.k"],
        );
        let found = found.expect("the imported name must resolve");
        let (a, _, target) = markers(
            "module a\n    { module a\n    }\n\nstruct $1Aa\n{\n    uint32 x;\n}\n",
        );
        assert_eq!(db.parse(found.range.file).text(), a, "the jump stayed in the wrong file");
        assert_eq!(found.range.range.start(), target.unwrap());
        assert_eq!(found.kind, DefKind::Struct);
    }

    /// Go to type definition lands on the type the loop gives the name: a
    /// variable's struct, through `const` and an array; a template
    /// instance's class at the template's text, once for the instances
    /// that share it; nothing for a builtin. Red with the instance's copy
    /// not read back to the template's text (`Src::expanded`).
    #[test]
    fn a_name_s_type_definition_is_its_type_s_declaration() {
        let text = "struct Point\n{\n    uint32 x;\n}\n\ntemplate <auto N>\nclass Box\n{\n    uint32[N] items;\n}\n\n\
                    inline void main()\n{\n    const Point[2] ps = {{1}, {2}};\n    static Box<4> b;\n    static Box<8> c;\n    uint32 n = 1;\n}\n";
        let mut db = db();
        let main = write(&mut db, "main.k", text);
        let at = |needle: &str| TextSize::new(text.find(needle).expect("in the fixture") as u32);
        let landed = |needle: &str| -> Vec<String> {
            db.type_definitions_at(&[main], main, at(needle))
                .iter()
                .map(|d| text[d.range.range].to_owned())
                .collect()
        };
        assert_eq!(landed("ps ="), ["Point"]);
        assert_eq!(landed("b;"), ["Box"]);
        assert_eq!(landed("c;"), ["Box"]);
        assert_eq!(landed("n ="), Vec::<String>::new(), "a builtin has no declaration");
    }

    /// An import's module path, or its alias, leads to the module's file —
    /// the one the program loaded for it — and the keyword itself, like any
    /// caret off an import, leads nowhere. Red with the import matched by
    /// anything but its own keyword (every import answering the first's
    /// module).
    #[test]
    fn an_import_leads_to_the_module_it_loaded() {
        let mut db = db();
        let a = write(&mut db, "a.k", "module a\n    { module a\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n");
        let b = write(&mut db, "b.k", "module b\n    { module b\n    }\n\nstruct Bb\n{\n    uint32 x;\n}\n");
        let text = "import a\nimport b as bee\n\ninline void main()\n{\n    Aa x;\n    bee::Bb y;\n}\n";
        let main = write(&mut db, "main.k", text);
        let at = |needle: &str| TextSize::new(text.find(needle).expect("in the fixture") as u32);
        assert_eq!(db.import_at(&[main], main, at("a\n")), Some(a));
        assert_eq!(db.import_at(&[main], main, at("b as")), Some(b));
        assert_eq!(db.import_at(&[main], main, at("bee\n")), Some(b), "the alias names the module too");
        assert_eq!(db.import_at(&[main], main, at("import b")), None, "the keyword is no path");
        assert_eq!(db.import_at(&[main], main, at("Aa x")), None, "a name is `definitions_at`'s");
    }

    /// A name the imported module declares but does **not** expose is not
    /// reachable (`test/syntax/imports.k:27-34`, `expected:1`), so the jump has
    /// to decline exactly where the diagnostic fires.
    #[test]
    fn a_name_an_import_does_not_expose_is_not_reachable() {
        let (_, found) = resolve(
            &[
                ("a.k", "module a\n    { Aa\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n\nstruct Bb\n{\n    uint32 y;\n}\n"),
                ("main.k", "import a\n\ninline void main()\n{\n    $0Bb x;\n}\n"),
            ],
            &["main.k"],
        );
        assert!(found.is_none(), "an unexposed name was reachable");
    }

    /// Two imports exposing different declarations under one name is
    /// `Resolution::Ambiguous` (`SymbolTable.hs:231-235`) — an error in the
    /// reference. Jumping to whichever happened to come first would be the
    /// guess this query exists not to make.
    ///
    /// Pass 9 accepts this program: `findInImports` returned *something*, which
    /// is all pass 9 asks. So this is a case where the two readings differ
    /// in the safe direction — the diagnostic stays silent and the jump
    /// declines.
    #[test]
    fn a_name_two_imports_both_expose_is_ambiguous_and_declines() {
        let (_, found) = resolve(
            &[
                ("a.k", "module a\n    { module a\n    }\n\nstruct Dup\n{\n    uint32 x;\n}\n"),
                ("b.k", "module b\n    { module b\n    }\n\nstruct Dup\n{\n    uint32 y;\n}\n"),
                ("main.k", "import a\nimport b\n\ninline void main()\n{\n    $0Dup d;\n}\n"),
            ],
            &["main.k"],
        );
        assert!(found.is_none(), "an ambiguous name was resolved to one of its candidates");
    }

    /// Source 3 — two roots sharing a namespace. `Ten` is keyed `[@m, Ten]`, so
    /// the exact probe of source 2 misses it and `lookupInScope` walking outward
    /// from `[@m]` finds it (`Symbols.hs:79-87`). This is the gap that once made
    /// `kg check a.k b.k` reject a valid program.
    #[test]
    fn a_name_in_the_same_module_namespace_resolves_by_walking_outward() {
        let (db, found) = resolve(
            &[
                ("a.k", "module m\n{\n}\n\nconst auto $1Ten = 10;\n"),
                ("main.k", "module m\n{\n}\n\nstatic assert($0Ten == 10);\n"),
            ],
            // Both files on the command line — `Main.hs:70,80` merges them into
            // one program, which is the only way `a.k` is in it at all.
            &["a.k", "main.k"],
        );
        let found = found.expect("a name in the shared namespace must resolve");
        let (a, _, target) = markers("module m\n{\n}\n\nconst auto $1Ten = 10;\n");
        assert_eq!(db.parse(found.range.file).text(), a);
        assert_eq!(found.range.range.start(), target.unwrap());
    }

    // -----------------------------------------------------------------------
    // Multi-segment paths
    // -----------------------------------------------------------------------

    /// `import a as m` binds `m` as a **qualifier** (`SymbolTable.hs:276-278`)
    /// and `findInImport` strips it before prepending the namespace — so
    /// `m::Ten` resolves through source 4, while the caret on `m` itself has no
    /// declaration to offer and declines.
    #[test]
    fn an_aliased_qualified_name_resolves_and_the_alias_itself_does_not() {
        let library = "module a\n    { module a\n    }\n\nconst auto $1Ten = 10;\n";
        let (db, found) = resolve(
            &[("a.k", library), ("main.k", "import a as m\n\nstatic assert(m::$0Ten == 10);\n")],
            &["main.k"],
        );
        let found = found.expect("`m::Ten` must resolve to `Ten`");
        let (a, _, target) = markers(library);
        assert_eq!(db.parse(found.range.file).text(), a);
        assert_eq!(found.range.range.start(), target.unwrap());

        let (_, alias) = resolve(
            &[("a.k", library), ("main.k", "import a as m\n\nstatic assert($0m::Ten == 10);\n")],
            &["main.k"],
        );
        assert!(alias.is_none(), "an import qualifier is not a declaration");
    }

    /// A qualified name **skips the scope stack** (`SymbolTable.hs:243-244`), so
    /// a local `x` may not answer `C::x`.
    #[test]
    fn a_qualified_name_skips_the_scope_stack() {
        let found = jumps(
            "class C\n{\n    uint32 $1x;\n}\n\ninline void main()\n{\n    uint32 x;\n    x = C::$0x;\n}\n",
        );
        assert_eq!(found.kind, DefKind::Variable(DeclareFlag::DeclareClass));
    }

    /// Only the prefix ending at the caret is probed, so a last segment nothing
    /// keys declines instead of falling back to its qualifier — the difference
    /// between "I do not know" and a jump to the wrong place.
    #[test]
    fn an_unresolvable_last_segment_declines_rather_than_jumping_to_its_qualifier() {
        declines("class C\n{\n    uint32 x;\n}\n\nconst auto q = C::$0nope;\n");
        declines("const auto q = Nope::$0deeper;\n");
    }

    // -----------------------------------------------------------------------
    // Declining
    // -----------------------------------------------------------------------

    #[test]
    fn a_name_nothing_declares_resolves_to_nothing() {
        declines("inline void main()\n{\n    $0Undeclared u;\n}\n");
    }

    /// An aggregate body is replaced with `mempty` by `getSymbols''`
    /// (`Symbols.hs:48-49`) and a field is `uninitVar`, never `symbol`
    /// (`Parser.hs:465`) — so a struct field is in no map, and a same-named
    /// declaration elsewhere is never the answer. Since 2026-09-06 a field is
    /// an *item* the editor can land on (`Item::is_field`), so the caret on
    /// the field itself resolves to the field, and `p.x` resolves through
    /// `p`'s type to the same declaration; before that both declined.
    #[test]
    fn a_struct_field_never_resolves_to_a_look_alike() {
        let found = jumps("const auto x = 1;\n\nstruct Point\n{\n    uint32 $1$0x;\n}\n");
        assert_eq!(found.kind, DefKind::Variable(DeclareFlag::DeclareMember));
        let found = jumps(
            "const auto x = 1;\n\nstruct Point\n{\n    uint32 $1x;\n}\n\ninline void main()\n{\n    Point p;\n    auto v = p.$0x;\n}\n",
        );
        assert_eq!(found.kind, DefKind::Variable(DeclareFlag::DeclareMember));
    }

    /// A `static if` branch is `quasiScoped` (`Parser.hs:467,593-595`), so what
    /// it declares is keyed one component deeper than anything outside it and no
    /// outward walk finds it. The reference does not resolve this before pass 16
    /// either: it elaborates the branch away first and re-resolves the survivors.
    ///
    /// So this is a case where the jump and the diagnostic **differ** — pass 9
    /// accepts the program, because its first source only asks whether the name
    /// is declared somewhere in the file. They differ in the safe direction: the
    /// diagnostic stays silent and the jump declines rather than inventing a
    /// target. Both halves are asserted, so a future elaboration pass that makes
    /// this resolvable will fail here and be forced to say so.
    #[test]
    fn a_declaration_under_a_static_if_declines_until_elaboration() {
        let source = "const auto P = true;\n\nclass C\n{\n    static if (P)\n    {\n        uint32 stored;\n    }\n\n    inline void read()\n    {\n        stored = $0stored;\n    }\n}\n";
        declines(source);

        let mut db = db();
        let (text, _, _) = markers(source);
        let file = write(&mut db, "main.k", &text);
        assert!(
            db.check(&[file]).iter().all(|d| !d.is_error()),
            "pass 9 must still accept the program it cannot navigate"
        );
    }

    /// The intrinsics are typed from a hard-coded table (`Type.hs:1074-1082`)
    /// and have no declaration anywhere. Pass 9 special-cases them so they are
    /// not reported; this declines so they are not jumped to.
    #[test]
    fn an_intrinsic_has_no_declaration_to_jump_to() {
        declines("inline void main()\n{\n    $0__print(\"hi\");\n}\n");
    }

    /// A caret on anything that is not an identifier, and one past the end of
    /// the buffer — which a client is entitled to send from a document state
    /// this database has already moved past.
    #[test]
    fn a_caret_that_is_not_on_an_identifier_declines() {
        declines("inline void main()\n{\n$0}\n");
        declines("$0inline void main()\n{\n}\n");
        declines("const auto x = $01;\n");

        let mut db = db();
        let file = write(&mut db, "main.k", "inline void main() {}\n");
        assert!(db.definition_at(&[file], file, TextSize::new(9_999)).is_none());
    }

    // -----------------------------------------------------------------------
    // The invariant that is the reason this lives in `kg-db`
    // -----------------------------------------------------------------------

    /// **Goto-definition and `"name not in scope"` cannot disagree.**
    ///
    /// Every source of [`super::resolve_globally`] is the corresponding source
    /// of pass 9's lookup asked about a *narrower* key, and the scope walk
    /// only ever returns an item of this file's [`kg_resolve::ItemTree`] — every
    /// one of which contributes its name to `declared_here`. So a name pass 9
    /// reports as undefined can never be one this query resolves.
    ///
    /// Asserted the way an invariant should be: at every offset pass 9 reported,
    /// not at a hand-picked one.
    #[test]
    fn a_name_pass_9_rejects_never_resolves() {
        let fixtures = [
            "inline void main()\n{\n    Undeclared u;\n}\n",
            "inline void main()\n{\n    uint32 x = nosuch + 1;\n}\n",
            "struct Point\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    Point p;\n    Missing q;\n}\n",
            "const auto q = Nope::deeper;\n",
            "class C\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    C c;\n    uint32 y = absent;\n}\n",
        ];

        for source in fixtures {
            let mut db = db();
            let file = write(&mut db, "main.k", source);
            let rejected: Vec<kg_base::TextRange> = db
                .check(&[file])
                .into_iter()
                .filter(|d| d.is_error() && d.message.contains("not in scope"))
                .map(|d| d.range.range)
                .collect();
            assert!(!rejected.is_empty(), "fixture rejects nothing: {source:?}");

            for range in rejected {
                assert!(
                    db.definition_at(&[file], file, range.start()).is_none(),
                    "pass 9 rejects `{}` in {source:?} but navigation jumps somewhere",
                    &source[range]
                );
            }
        }
    }

    /// The other half of the pairing, and the one that would catch a query that
    /// resolved nothing at all: in a program the frontend accepts, **every**
    /// path segment that is not an intrinsic resolves.
    ///
    /// Written as a sweep rather than as offsets so that a name added to the
    /// fixture is automatically covered.
    #[test]
    fn every_name_in_an_accepted_program_resolves() {
        let source = concat!(
            "struct Point\n{\n    uint32 x;\n}\n\n",
            "const auto Ten = 10;\n\n",
            "using Alias = Point;\n\n",
            "class Holder\n{\n    uint32 count;\n\n",
            "    inline void bump(uint32 by)\n    {\n        count = count + by + Ten;\n    }\n}\n\n",
            "inline void run(uint32 width)\n{\n",
            "    uint32 total = width + Ten;\n",
            "    Alias p;\n",
            "    Holder h;\n",
            "    for (const auto i : 4)\n    {\n        total = total + i;\n    }\n",
            "    auto f = [total](uint32 z) -> uint32 { return z + total; };\n",
            "}\n",
        );
        let mut db = db();
        let file = write(&mut db, "main.k", source);
        let errors: Vec<String> =
            db.check(&[file]).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect();
        assert!(errors.is_empty(), "fixture must compile: {errors:?}");

        let root = db.parse(file).syntax_node();
        let mut checked = 0;
        for element in root.descendants_with_tokens() {
            let Some(token) = element.into_token() else { continue };
            if token.kind() != kg_syntax::SyntaxKind::IDENT {
                continue;
            }
            // Only *uses* — a `NAME` is covered by the caret-on-a-declaration
            // test, and a `.` field access is a documented decline.
            if super::path_prefix(&token).is_none() {
                continue;
            }
            assert!(
                db.definition_at(&[file], file, token.text_range().start()).is_some(),
                "`{}` at {:?} did not resolve",
                token.text(),
                token.text_range()
            );
            checked += 1;
        }
        // Every `PATH_SEGMENT` of the fixture, and no fewer — the guard against
        // a `path_prefix` that quietly stopped recognising anything, which would
        // otherwise leave this test passing vacuously.
        let segments = root
            .descendants()
            .filter(|n| n.kind() == kg_syntax::SyntaxKind::PATH_SEGMENT)
            .count();
        assert!(segments > 10, "the fixture only has {segments} path segments");
        assert_eq!(checked, segments, "a path segment was skipped");
    }

    /// The loop's type at a declaration, through its position index: a
    /// parameter, a local, a function, each its declared type (`typeOf`,
    /// `Type.hs:452`). Red with the walk outward taking the innermost node
    /// (the declaration's `NAME`, typed as nothing a tooltip prints) rather
    /// than the one lowered from the declaration. Until sprint W3.7 each
    /// was compared with the CST engine's answer, deleted since.
    #[test]
    fn the_ir_engine_types_a_declaration_through_the_position_index() {
        let mut db = db();
        let text = "inline uint8 f(uint16 p)\n{\n    uint32 x = p;\n    return x;\n}\n";
        let file = write(&mut db, "main.k", text);
        let looped = db.editor_loop(&[file], file).expect("the buffer parses");
        let mut typed = Vec::new();
        for name in ["p)", "x =", "f("] {
            let at = TextSize::new(text.find(name).expect("in the fixture") as u32);
            let def = db.definition_at(&[file], file, at).expect("resolves");
            let ir = db.ir_type_of_definition(&looped, &def).expect("the loop holds the declaration");
            typed.push(kg_ty::subtype::show_pretty(&ir, Some(db.interner())));
        }
        assert_eq!(typed, ["uint16", "uint32", "(uint16) -> uint8"]);
    }

    /// "No answer" is `None`, not a type: a declaration the loop's tree does
    /// not hold. The second `f` of a redefinition is dropped by the loop
    /// (the reference rejects the program, at parse time, so the loop is
    /// the buffer's own, `Db::editor_loop_or_own`), and the first is
    /// answered.
    #[test]
    fn a_declaration_the_loop_dropped_has_no_ir_answer() {
        let mut db = db();
        let text = "inline void f()\n{\n}\n\ninline void f()\n{\n}\n";
        let file = write(&mut db, "main.k", text);
        let looped = db.editor_loop_or_own(&[file], file);
        let second = TextSize::new(text.rfind("f()").expect("in the fixture") as u32);
        let def = db.definition_at(&[file], file, second).expect("resolves to itself");
        assert_eq!(def.range.range.start(), second);
        assert_eq!(db.ir_type_of_definition(&looped, &def), None);
        let first = TextSize::new(text.find("f()").expect("in the fixture") as u32);
        let def = db.definition_at(&[file], file, first).expect("resolves to itself");
        assert!(db.ir_type_of_definition(&looped, &def).is_some());
    }

    /// What a tooltip prints — `hover.rs`'s filter, which the editor and
    /// the resilience gate share.
    fn shown(ty: &kg_ty::Ty) -> bool {
        !matches!(ty, kg_ty::Ty::Untyped | kg_ty::Ty::Auto) && ty.resolved() && !ty.is_error()
    }

    const TAKE: &str = "template <typename T>\ninline void take(T v)\n{\n    auto w = v + 1;\n}\n\n\
                        inline void main()\n{\n    take<uint8>(1);\n    take<uint16>(2);\n    auto z = 3u4;\n}\n";

    /// Inside a template's body the written tree has no type — `inferType`
    /// visits a template for its condition only — and each instance's copy
    /// has its own: `w` is `v + 1`, so `uint9` in `take<uint8>` and
    /// `uint17` in `take<uint16>` (`tAdd` widens, `Type.hs:854`), each
    /// named as a message names the instance. A declaration outside every
    /// template is answered by its written type. Red with the copies' half
    /// of `answer_at` dropped (no answer for `w`), or with the instances
    /// left unnamed.
    #[test]
    fn a_template_body_is_answered_by_each_instance_s_copy() {
        use crate::looped::IrAnswer;
        let mut db = db();
        let file = write(&mut db, "main.k", TAKE);
        let looped = db.editor_loop(&[file], file).expect("the buffer parses");
        let at = |needle: &str| {
            let offset = TextSize::new(TAKE.find(needle).expect("in the fixture") as u32);
            let def = db.definition_at(&[file], file, offset).expect("resolves");
            db.ir_answer_of_definition(&looped, &def, &shown)
        };
        let spelled = |answer: Option<IrAnswer>| match answer {
            Some(IrAnswer::Copies { types, untyped, budget_exhausted }) => {
                assert!(untyped.is_empty() && !budget_exhausted);
                types.into_iter().map(|(t, i)| (kg_ty::subtype::show_pretty(&t, Some(db.interner())), i)).collect::<Vec<_>>()
            }
            other => panic!("expected the copies' answer, got {other:?}"),
        };
        let pair = |t: &str, i: &str| (t.to_owned(), vec![i.to_owned()]);
        assert_eq!(spelled(at("w =")), [pair("uint9", "take<uint8>"), pair("uint17", "take<uint16>")]);
        assert_eq!(spelled(at("v)")), [pair("uint8", "take<uint8>"), pair("uint16", "take<uint16>")]);
        assert_eq!(at("z ="), Some(IrAnswer::Written(kg_ty::Ty::Unsigned(4))));
    }

    /// A member of a class template is typed in each instance of the
    /// class, named by the class's instance; where every copy agrees the
    /// answer is one type with all of them. `head` is `items[0] + N`:
    /// `uint32` plus `uint3` or `uint4`, `uint33` either way. Red with the
    /// grouping by type dropped (two entries of one type).
    #[test]
    fn copies_that_agree_are_one_type_with_every_instance() {
        use crate::looped::IrAnswer;
        let text = "template <auto N>\nclass Box\n{\n    uint32[N] items;\n\n    inline auto first()\n    {\n        auto head = items[0] + N;\n        return head;\n    }\n}\n\n\
                    inline void main()\n{\n    static Box<4> a;\n    static Box<9> b;\n    auto r = a.first();\n}\n";
        let mut db = db();
        let file = write(&mut db, "main.k", text);
        let looped = db.editor_loop(&[file], file).expect("the buffer parses");
        let offset = TextSize::new(text.find("head =").expect("in the fixture") as u32);
        let def = db.definition_at(&[file], file, offset).expect("resolves");
        let Some(IrAnswer::Copies { types, .. }) = db.ir_answer_of_definition(&looped, &def, &shown) else {
            panic!("expected the copies' answer")
        };
        assert_eq!(types, [(kg_ty::Ty::Unsigned(33), vec!["Box<4>".to_owned(), "Box<9>".to_owned()])]);
    }

    /// A declaration a rewrite rebuilt answers for its text: `moveFunctions`
    /// lifts the class out of `main`'s body and mints `main` again round
    /// what is left (`Function.hs:88-119`), donating it the old node's
    /// `Src`, and the `Own` node lowered from the text is no longer in the
    /// tree (`kg_hir::position`'s rebuild). Hover on `main` was no answer
    /// at all (`test/syntax/export-class.k`, the resilience gate, sprint
    /// W3.7); its type is `() -> void`. Red with the position index taking
    /// `Own` nodes alone.
    #[test]
    fn a_function_a_rewrite_rebuilt_answers_for_its_text() {
        let text = "inline void main()\n{\n    class Foo\n    {\n    public:\n        void Bar()\n        {\n        }\n    }\n}\n";
        let mut db = db();
        let file = write(&mut db, "main.k", text);
        let looped = db.editor_loop(&[file], file).expect("the buffer parses");
        let def = db.definition_at(&[file], file, TextSize::new(text.find("main").expect("in the fixture") as u32)).expect("resolves");
        let answer = db.ir_answer_of_definition(&looped, &def, &shown);
        assert!(
            matches!(&answer, Some(crate::looped::IrAnswer::Written(kg_ty::Ty::Function { params, .. })) if params.is_empty()),
            "{answer:?}"
        );
    }

    /// A field inside a template's body is reached through each instance's
    /// type of the base — `b` is `Box<8>` in `take<uint8>` and `Box<16>` in
    /// `take<uint16>`, two copies of one written class — and every copy's
    /// `items` is the template's written field, so that is the jump. The
    /// CST engine had no type for `b` (a template's body) and declined.
    /// Red with the loop's half of `field_definition` dropped.
    #[test]
    fn a_field_inside_a_template_jumps_to_the_field_its_instances_share() {
        let text = "template <auto N>\nclass Box\n{\n    uint32[N] items;\n}\n\n\
                    template <typename T>\ninline void take(T v)\n{\n    static Box<bitsizeof(T)> b;\n    auto h = b.items[0];\n}\n\n\
                    inline void main()\n{\n    take<uint8>(1);\n    take<uint16>(2);\n}\n";
        let mut db = db();
        let file = write(&mut db, "main.k", text);
        assert!(db.diagnostics(&[file], file).iter().all(|d| !d.is_error()));
        let at = TextSize::new(text.find("items[0]").expect("in the fixture") as u32);
        let def = db.definition_at(&[file], file, at).expect("the field resolves");
        assert_eq!(def.range.range.start(), TextSize::new(text.find("items;").expect("the field") as u32));
    }

    /// Where the instances' fields are written apart — `s.x` is `A`'s `x`
    /// in `get<A>` and `B`'s in `get<B>` — no one jump is the field the
    /// text means, and the query declines rather than pick an instance;
    /// [`Db::definitions_at`] lists both, in the text's order. Red with
    /// `definition_at` taking the first of several, or with the list kept
    /// to one.
    #[test]
    fn a_field_two_instances_declare_apart_has_no_jump() {
        let text = "struct A\n{\n    uint8 x;\n}\n\nstruct B\n{\n    uint16 x;\n}\n\n\
                    template <typename S>\ninline auto get(S s)\n{\n    return s.x;\n}\n\n\
                    inline void main()\n{\n    A a = {};\n    B b = {};\n    get(a);\n    get(b);\n}\n";
        let mut db = db();
        let file = write(&mut db, "main.k", text);
        assert!(db.diagnostics(&[file], file).iter().all(|d| !d.is_error()));
        let looped = db.editor_loop(&[file], file).expect("the buffer parses");
        assert!(!looped.looped().out.report.budget_exhausted);
        let at = TextSize::new(text.find("x;\n}\n\ninline").expect("in the fixture") as u32);
        assert_eq!(db.definition_at(&[file], file, at), None);
        let both: Vec<TextSize> = db.definitions_at(&[file], file, at).iter().map(|d| d.range.range.start()).collect();
        let a = TextSize::new(text.find("x;").expect("`A`'s") as u32);
        let b = TextSize::new(text.find("x;\n}\n\ntemplate").expect("`B`'s") as u32);
        assert_eq!(both, [a, b]);
    }

    /// A field in a buffer that does not parse, and that no loop has run
    /// for, is no jump — no loop has typed the base — and no loop runs for
    /// it: rename and find-references ask this of every `.x` in a file.
    /// Once the buffer has parsed and its loop run, the same field after
    /// the breaking edit is the last loop's, read through the edit. Until
    /// sprint W3.7 the CST engine's type of the base answered the first.
    /// Red with `member_definitions` asking `Db::looped` directly (a loop
    /// runs for the broken buffer).
    #[test]
    fn a_field_in_a_buffer_that_does_not_parse_runs_no_loop() {
        let text = "struct S\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    S s = {};\n    auto v = s.x;\n    @@@\n}\n";
        let mut db = db();
        let file = write(&mut db, "main.k", text);
        let before = db.stats().misses(crate::stats::Query::Looped);
        let at = TextSize::new(text.find("x;\n    @@@").expect("in the fixture") as u32);
        assert_eq!(db.definition_at(&[file], file, at), None);
        assert_eq!(db.stats().misses(crate::stats::Query::Looped), before, "a loop ran for a broken buffer");

        let file = write(&mut db, "main.k", &text.replace("    @@@\n", ""));
        db.looped(&[file]);
        let file = write(&mut db, "main.k", text);
        let def = db.definition_at(&[file], file, at).expect("the last loop types `s`");
        assert_eq!(def.range.range.start(), TextSize::new(text.find("x;\n}").expect("the field") as u32));
    }

    /// A method is reached as a field is, through the receiver's type
    /// (`method`, `Type.hs:1358-1362`), and lands on the class's own
    /// declaration of it — for an instance, the template's member; for a
    /// member template, its `template <…>`. The CST engine answered none of
    /// these. Red with methods left out of `member_declaration`.
    #[test]
    fn a_method_resolves_through_the_receiver_s_type() {
        let found = jumps(
            "class C\n{\npublic:\n    inline void $1go()\n    {\n    }\n}\n\ninline void main()\n{\n    static C c;\n    c.$0go();\n}\n",
        );
        assert_eq!(found.kind, DefKind::Function);
        jumps(
            "template <auto N>\nclass Box\n{\n    uint32[N] items;\npublic:\n    inline uint32 $1get(uint32 i)\n    {\n        return items[i];\n    }\n}\n\n\
             using Box4 = Box<4>;\n\ninline void main()\n{\n    static Box4 b;\n    auto x = b.$0get(1);\n}\n",
        );
        let found = jumps(
            "class F\n{\npublic:\n    template <auto K>\n    inline uint32 $1run(uint32 t)\n    {\n        return t + K;\n    }\n}\n\n\
             inline void main()\n{\n    static F f;\n    auto r = f.$0run<3>(1);\n}\n",
        );
        assert_eq!(found.kind, DefKind::Template);
    }

    /// A method whose body holds a lambda is rebuilt by `desugarLambdas`
    /// round what is left of its body (`Function.hs:206-214`), and the
    /// rebuilt function wears its old range, `Donated` — still the member
    /// its text declares, and the commonest method in the library
    /// (`test/library/control/loop.k`'s `Run`s). Red with `Looped::members`
    /// admitting `Own` nodes alone.
    #[test]
    fn a_method_holding_a_lambda_resolves() {
        jumps(
            "class C\n{\n    uint32 x;\npublic:\n    inline void $1go()\n    {\n        auto f = [](uint32 a) -> uint32 { return a + 1; };\n        x = f(x);\n    }\n}\n\n\
             inline void main()\n{\n    static C c;\n    c.$0go();\n}\n",
        );
    }

    /// A class declaring `go` in each branch of a `static if`, reached
    /// inside a template's body whose instances take both branches: each
    /// instance's `go` is a declaration of its own, so `definitions_at`
    /// answers both, in the text's order, and `definition_at` — one answer
    /// or none — declines. Red with the list kept to the first instance's
    /// member (one answer, the `false` branch's: the loop decides
    /// `run<false>` first).
    #[test]
    fn a_member_two_instances_declare_apart_is_each_of_its_declarations() {
        let text = "template <bool B>\nclass E\n{\npublic:\n    static if (B)\n    {\n        inline uint32 go()\n        {\n            return 1;\n        }\n    }\n    else\n    {\n        inline uint8 go()\n        {\n            return 2;\n        }\n    }\n}\n\n\
                    template <bool F>\ninline uint32 run()\n{\n    static E<F> e;\n    return e.go();\n}\n\n\
                    inline void main()\n{\n    auto v = run<true>();\n    auto w = run<false>();\n}\n";
        let mut db = db();
        let file = write(&mut db, "main.k", text);
        assert!(db.diagnostics(&[file], file).iter().all(|d| !d.is_error()));
        let at = TextSize::new(text.find("go();").expect("in the fixture") as u32);
        let found: Vec<TextSize> = db.definitions_at(&[file], file, at).iter().map(|d| d.range.range.start()).collect();
        let first = TextSize::new(text.find("go()\n").expect("the first branch's") as u32);
        let second = TextSize::new(text.rfind("go()\n        {").expect("the second branch's") as u32);
        assert_eq!(found, [first, second]);
        assert_eq!(db.definition_at(&[file], file, at), None);
    }

    /// Navigation reads the loop of the buffer the program is rooted at,
    /// as that buffer's diagnostics do, whichever file the name is in: a
    /// references search resolves every `.x` in the files the buffer
    /// imports, and a clean import under a buffer that does not parse must
    /// not run the loop its diagnostics never ran. Red with the gate read
    /// at the name's file.
    #[test]
    fn a_member_in_an_import_of_a_buffer_that_does_not_parse_runs_no_loop() {
        let lib = "module lib\n    { module lib\n    }\n\nstruct S\n{\n    uint32 x;\n}\n\ninline uint32 peek(S s)\n{\n    return s.x;\n}\n";
        let mut db = db();
        let lib_file = write(&mut db, "lib.k", lib);
        let main = write(&mut db, "main.k", "import lib\n\ninline void main()\n{\n    @@@\n}\n");
        assert!(db.diagnostics(&[main], main).iter().any(|d| d.is_error()));
        let before = db.stats().misses(crate::stats::Query::Looped);
        let at = TextSize::new(lib.find("s.x").expect("the use") as u32 + 2);
        // No loop has typed `s`: no jump (the CST engine's, until sprint
        // W3.7), and none ran.
        assert_eq!(db.definition_at(&[main], lib_file, at), None);
        assert_eq!(db.stats().misses(crate::stats::Query::Looped), before, "a loop ran for a broken root");
    }

    /// The editor reads the loop the diagnostics run — the same memo — and
    /// runs none where they stop before it: a buffer that does not parse
    /// reads the last loop computed for it, mapped onto its text, not one
    /// a keystroke would then pay for. Red with `editor_loop`'s gate
    /// dropped (the broken buffer's own loop runs).
    #[test]
    fn the_editor_reads_the_loop_only_where_the_diagnostics_run_it() {
        let mut db = db();
        let file = write(&mut db, "main.k", TAKE);
        let looped = db.editor_loop(&[file], file).expect("a clean buffer");
        assert!(std::sync::Arc::ptr_eq(looped.looped(), &db.looped(&[file])), "the diagnostics' memo, not a second loop");
        assert!(looped.is_current());

        let broken = TAKE.replace("auto z = 3u4;", "auto z = ;");
        let file = write(&mut db, "main.k", &broken);
        assert!(db.diagnostics(&[file], file).iter().any(|d| d.is_error()));
        let before = db.stats().misses(crate::stats::Query::Looped);
        let last = db.editor_loop(&[file], file).expect("the last loop");
        assert!(std::sync::Arc::ptr_eq(last.looped(), looped.looped()), "the clean buffer's loop");
        assert!(!last.is_current(), "read through the edit");
        assert_eq!(db.stats().misses(crate::stats::Query::Looped), before, "no loop ran for the broken buffer");
    }

    /// On a buffer that does not parse, a declaration the edit moved is
    /// answered by the last loop at the range it had then — `w`, inside
    /// `take`'s body, each instance's type — and one the edit wrote is in
    /// no loop: no answer. Red with
    /// `EditorLoop::to_loop` reading the current range unmapped.
    #[test]
    fn a_declaration_an_edit_moved_is_answered_by_the_last_loop() {
        let mut db = db();
        let file = write(&mut db, "main.k", TAKE);
        db.editor_loop(&[file], file).expect("a clean buffer");
        let broken = format!("uint8 typed = ;\n\n{TAKE}");
        let file = write(&mut db, "main.k", &broken);
        assert!(db.diagnostics(&[file], file).iter().any(|d| d.is_error()));
        let looped = db.editor_loop(&[file], file).expect("the last loop");
        let w = TextSize::new(broken.find("w = v").expect("in the fixture") as u32);
        let def = db.definition_at(&[file], file, w).expect("resolves");
        let Some(crate::looped::IrAnswer::Copies { types, .. }) = db.ir_answer_of_definition(&looped, &def, &shown) else {
            panic!("each instance's type");
        };
        let spelled: Vec<String> = types.iter().map(|(t, _)| kg_ty::subtype::show_pretty(t, Some(db.interner()))).collect();
        assert_eq!(spelled, ["uint9", "uint17"]);
        let typed = TextSize::new(broken.find("typed").expect("in the fixture") as u32);
        let def = db.definition_at(&[file], file, typed).expect("resolves to itself");
        assert_eq!(db.ir_answer_of_definition(&looped, &def, &shown), None);
    }

    /// Two callers asking for one program's loop at once — the language
    /// server's diagnostics and an inlay hint on the same edit — run it
    /// once: the second waits for the first. Red with the in-flight slot
    /// bypassed (each computes; two misses whenever they overlap, which a
    /// barrier and a loop of this size make every run).
    ///
    /// The program is loaded first, so this pins the in-flight slot alone;
    /// callers that also race to load the program are the next test's.
    #[test]
    fn one_program_s_loop_runs_once_however_many_ask() {
        let mut text = String::from(TAKE);
        for i in 0..40 {
            text.push_str(&format!("\ninline void use{i}()\n{{\n    take<uint{}>(1);\n}}\n", i + 1));
        }
        let mut db = db();
        let file = write(&mut db, "main.k", &text);
        db.program(&[file]);
        let before = db.stats().misses(crate::stats::Query::Looped);
        let barrier = std::sync::Barrier::new(2);
        let (a, b) = std::thread::scope(|s| {
            let one = s.spawn(|| {
                barrier.wait();
                db.looped(&[file])
            });
            let two = s.spawn(|| {
                barrier.wait();
                db.looped(&[file])
            });
            (one.join().expect("no panic"), two.join().expect("no panic"))
        });
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        assert_eq!(db.stats().misses(crate::stats::Query::Looped) - before, 1);
    }

    /// Threads loading one program at once share it, and so its loop: each
    /// layer a racer misses — the parse, the item tree, the program —
    /// adopts what the first to finish stored, so the loop's in-flight
    /// slot, matched by program identity, sees one program and runs one
    /// loop. Eight callers behind a barrier, on a fresh edit each of five
    /// rounds, nothing loaded beforehand. Red with the program's adoption
    /// dropped, or the parse's (each racer's own `Arc`, a loop for each).
    #[test]
    fn threads_loading_one_program_at_once_run_one_loop() {
        let mut text = String::from(TAKE);
        for i in 0..40 {
            text.push_str(&format!("\ninline void use{i}()\n{{\n    take<uint{}>(1);\n}}\n", i + 1));
        }
        let mut db = db();
        for round in 0..5 {
            let file = write(&mut db, "main.k", &format!("{text}// round {round}\n"));
            let before = db.stats().misses(crate::stats::Query::Looped);
            let barrier = std::sync::Barrier::new(8);
            let loops: Vec<_> = std::thread::scope(|s| {
                let callers: Vec<_> = (0..8)
                    .map(|_| {
                        s.spawn(|| {
                            barrier.wait();
                            db.looped(&[file])
                        })
                    })
                    .collect();
                callers.into_iter().map(|c| c.join().expect("no panic")).collect()
            });
            assert!(loops.iter().all(|l| std::sync::Arc::ptr_eq(l, &loops[0])), "round {round}: two loops");
            assert_eq!(db.stats().misses(crate::stats::Query::Looped) - before, 1, "round {round}");
        }
    }

    /// A name in a module's exposure list stands for the declaration
    /// `checkSymbols` finds for it (`Parser.hs:216-223`): the module's own,
    /// or the one an import exposes, which the list re-exposes. A rename of
    /// the declaration reaches the list through this. Red with the list's
    /// names left unresolved.
    #[test]
    fn a_name_in_the_exposure_list_resolves_to_what_it_exposes() {
        jumps("module helper { $0twice }\n\ninline void $1twice()\n{\n}\n");
        let inner = "module inner { twice }\n\ninline void twice()\n{\n}\n";
        let (_, found) = resolve(&[("inner.k", inner), ("outer.k", "module outer { $0twice }\n\nimport inner\n")], &["outer.k"]);
        let found = found.expect("the re-exposed name resolves");
        assert_eq!(found.range.range.start(), TextSize::new(inner.rfind("twice").unwrap() as u32));
    }

    /// A member read through a loop the caller hands in: a search over a
    /// file nobody has open runs one ([`Db::loop_once`]) and reads it
    /// ([`Db::definitions_at_through`]), and the memo is left holding no
    /// loop for that program, nor did the database run one of its own. Red
    /// with `loop_once` going through `Db::looped`, which keeps it, or with
    /// the selector read through the editor's loop instead of the one
    /// handed in.
    #[test]
    fn a_member_is_read_through_the_loop_handed_in_and_that_loop_is_kept_nowhere() {
        let (text, at, target) = markers(
            "class C\n{\npublic:\n    inline void $1go()\n    {\n    }\n}\n\ninline void main()\n{\n    static C c;\n    c.$0go();\n}\n",
        );
        let mut db = db();
        let file = write(&mut db, "main.k", &text);
        let (at, target) = (at.unwrap(), target.unwrap());
        let before = db.stats();
        let looped = db.loop_once(&[file]);
        let found = db.definitions_at_through(&|| Some(looped.clone()), &[file], file, at);
        assert_eq!(found.iter().map(|d| d.range.range.start()).collect::<Vec<_>>(), vec![target]);
        assert!(db.definitions_at_through(&|| None, &[file], file, at).is_empty(), "no loop, no member");
        assert!(db.editor_loop_ready(&[file]).is_none(), "the loop was kept");
        assert_eq!(db.stats().since(&before).misses(crate::stats::Query::Looped), 0, "the database ran a loop of its own");
    }

    /// Forgetting a root set drops what is keyed by it — its program, and
    /// its symbol map, which pins the program too — and keeps what is keyed
    /// by module, which every other program holding the module reads, and
    /// every other root set's. Red with `forget_roots` dropping nothing,
    /// either memo's entry, or every root set's.
    #[test]
    fn forgetting_a_root_set_drops_its_program_and_keeps_its_modules() {
        use crate::stats::Query;
        use kg_resolve::SymbolsConfig::AllSymbols;
        let mut db = db();
        let helper = write(&mut db, "helper.k", "module helper { twice }\n\ninline void twice()\n{\n}\n");
        let main = write(&mut db, "main.k", "import helper\n\ninline void main()\n{\n    twice();\n}\n");
        db.global_symbols(&[main], AllSymbols);
        db.global_symbols(&[helper], AllSymbols);
        let forgotten = db.program(&[main]);
        let before = db.stats();
        db.forget_roots(&[main]);
        assert_eq!(std::sync::Arc::strong_count(&forgotten), 1, "a memo still pins the forgotten program");
        db.global_symbols(&[main], AllSymbols);
        db.global_symbols(&[helper], AllSymbols);
        let since = db.stats().since(&before);
        assert_eq!((since.misses(Query::Program), since.misses(Query::Symbols)), (1, 1), "`main`'s alone, rebuilt");
        assert_eq!((since.misses(Query::Parse), since.misses(Query::ItemTree)), (0, 0), "a module, reparsed");
    }
}
