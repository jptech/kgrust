//! One resolved compilation: which files it contains, in which order, and the
//! module graph over them.
//!
//! This module owns the *loader* — the thing `check_source` never had. It is a
//! transcription of `parseProgram` / `parseFile'` (`Parser/IO.hs:66-98`) with
//! the filesystem behind [`kg_vfs::Vfs`] and the parse behind
//! [`Db::item_tree`](crate::Db::item_tree), so an unsaved editor buffer and a
//! file on disk go down the same path.

use kg_base::{FileId, FxHashMap, Utf8PathBuf};
use kg_diag::Diagnostic;
use kg_resolve::module::{ImportFailure, walk_imports};
use kg_resolve::{
    ImportDecl, ImportError, ImportId, ImportNeeded, ItemTree, ModuleGraph, ModuleSource,
    Namespace,
};

use crate::db::Db;

/// One module's imports paired with what each resolved to, in source order.
///
/// The shape [`kg_resolve::module::walk_imports`] asks its callback for, kept
/// so the graph can be built from the walk's answers rather than by resolving
/// every import a second time.
type ResolvedImports = Vec<(ImportId, Result<ModuleSource, ImportError>)>;

/// The implicit base import that `parseProgram` adds to the symbol table every
/// command-line file starts from — `addImport Needed 0 Nothing emptySymbolTable`
/// (`Parser/IO.hs:77`).
///
/// It carries an [`ImportId`] no lowered import can collide with, because
/// lowering numbers a file's own imports `0..n` in source order
/// (`kg_resolve::ImportId`). It is inserted **before** them, so
/// `find_in_imports`' reverse-source-order iteration reaches it **last** — which
/// is the reference's order, where the base is consed first and therefore sits
/// at the tail of `imports` (`SymbolTable.hs:110,263-270`).
pub const IMPLICIT_BASE_IMPORT: ImportId = ImportId(u32::MAX);

/// One compilation: the entry files, everything reachable from them, and the
/// module graph.
///
/// # Why the file *order* is part of the value
///
/// `files` is a **post-order DFS of the import graph with the base library
/// first** — the order in which files finished parsing (`Parser/IO.hs:96`,
/// `Main.hs:70,76`). That order decides duplicate-key precedence in the global
/// symbol map (`docs/spec/resolve.md` §3.1): the *earlier* file wins. Reordering
/// it silently changes which declaration a name resolves to.
#[derive(Clone, Debug)]
pub struct Program {
    /// The command-line files, in the order given.
    pub roots: Vec<FileId>,
    /// The implicit base library, if it resolved (`Parser/IO.hs:71-77`).
    pub base: Option<ModuleSource>,
    /// Post-order DFS, base library first.
    pub files: Vec<ModuleSource>,
    /// Imports resolved, interfaces built, re-exports applied.
    pub graph: ModuleGraph,
    /// **Loading** diagnostics only: a base library that could not be found, an
    /// import that could not be resolved, an import cycle, `checkExposed`'s
    /// module half, and the `redundant-import` warning
    /// ([`crate::imports`]). Per-file syntax and frontend diagnostics are not
    /// here — they belong to [`Db::check`](crate::Db::check).
    pub diagnostics: Vec<Diagnostic>,
    /// Every import that did not resolve, with the module it was written in.
    pub failures: Vec<ImportFailure>,
    /// In `files` order. Owned rather than shared because
    /// [`kg_resolve::lower::Program`] and [`kg_resolve::global_symbols`] both
    /// take `&[(ModuleSource, ItemTree)]`; the per-file trees stay memoized in
    /// [`Db`] and this is one clone per program build, not per query.
    trees: Vec<(ModuleSource, ItemTree)>,
}

impl Program {
    /// The item trees, in `files` order — the slice every whole-program
    /// `kg_resolve` entry point takes.
    pub fn trees(&self) -> &[(ModuleSource, ItemTree)] {
        &self.trees
    }

    /// A [`DefId`](kg_resolve::DefId)-addressable view over the whole program.
    pub fn items(&self) -> kg_resolve::lower::Program<'_> {
        kg_resolve::lower::Program::new(&self.trees)
    }

    pub fn tree(&self, module: ModuleSource) -> Option<&ItemTree> {
        self.trees.iter().find(|(m, _)| *m == module).map(|(_, t)| t)
    }

    /// Whether `module` is part of this compilation at all — false for a file
    /// nothing imports.
    pub fn contains(&self, module: ModuleSource) -> bool {
        self.files.contains(&module)
    }
}

/// `parseProgram` (`Parser/IO.hs:66-77`) — load `roots`, the implicit base
/// library, and everything they transitively import.
///
/// Four things this has to get right, each with an oracle in
/// `test/syntax/imports.k`:
///
/// * **the base library is import #0 of every command-line file**, not a
///   separate program — `addImport Needed 0 Nothing` (`Parser/IO.hs:77`). The
///   base itself is parsed with an *empty* symbol table and gets no base of its
///   own, which is why `library/base.k:30` imports `.cmdargs` explicitly;
/// * **the search starts at the importing file's own directory**, then every
///   `-I dir/device/<target>`, then every `-I dir`
///   (`Parser/IO.hs:107,110`) — [`kg_resolve::search_dirs`] already builds that
///   list, and the per-import half is which directory counts as "own";
/// * **a cycle terminates** and is reported at the offending `import`
///   (`Parser/IO.hs:90`, `docs/spec/resolve.md` §8);
/// * **each file is parsed once**, whatever the diamond
///   (`Parser/IO.hs:91-95`) — here that is [`Db::item_tree`]'s memo, so the
///   guarantee holds *across* programs too, not just within one.
pub(crate) fn load(db: &Db, roots: &[FileId]) -> Program {
    let settings = db.settings();
    let vfs = db.vfs();
    let interner = db.interner();

    let mut diagnostics = Vec::new();

    // `importBase` (`Parser/IO.hs:71-77`). The base file name is `base.k` unless
    // `--no-implicit-base` or an empty `--import-dir` demotes it to `.cmdargs.k`,
    // which is synthetic and therefore always resolvable — so a bare
    // `Settings::fallback` never fails here.
    let base_file = kg_resolve::module::implicit_base_file(settings);
    let base = match kg_resolve::module::resolve_import_file(
        &base_file,
        // `parseFile` starts every root at the working directory
        // (`Parser/IO.hs:81`); `Settings::root` is this toolchain's stand-in for
        // it, being the directory relative manifest paths resolve against.
        &settings.root,
        settings,
        vfs,
    ) {
        Ok(source) => Some(source),
        Err(error) => {
            // `either (Ex.throw . FileException) return` (`Parser/IO.hs:83`) —
            // the reference aborts the compiler. A diagnostic is the same
            // information without taking the process down with it.
            let at = roots.first().map_or(kg_base::FileId::SYNTHETIC, |&f| f);
            diagnostics.push(Diagnostic::error(
                ModuleSource::File(at).start(),
                error.to_string(),
            ));
            None
        }
    };

    let mut entries: Vec<ModuleSource> = Vec::with_capacity(1 + roots.len());
    entries.extend(base);
    entries.extend(roots.iter().map(|&f| ModuleSource::File(f)));

    // The directory each module's *own* imports search first — `fileDir`
    // (`Parser/IO.hs:93,107`). A real file answers its own directory; the two
    // synthetic modules answer the *importing* file's, which is why `.options`'
    // `import compiler.options` resolves relative to whoever pulled it in.
    let mut dirs: FxHashMap<ModuleSource, Utf8PathBuf> = FxHashMap::default();
    for &entry in &entries {
        dirs.insert(entry, kg_resolve::module::module_dir(entry, &settings.root, vfs));
    }

    // Which module each import resolved to, recorded during the walk so the
    // graph can be built from it afterwards without resolving anything twice.
    let mut resolved: FxHashMap<ModuleSource, ResolvedImports> = FxHashMap::default();

    let walk = walk_imports(&entries, |module| {
        let from_dir = dirs.get(&module).cloned().unwrap_or_else(|| settings.root.clone());
        let tree = db.item_tree(module);
        let mut out = Vec::with_capacity(tree.imports.len());
        for decl in &tree.imports {
            let target =
                kg_resolve::resolve_module_path(&decl.name, &from_dir, settings, vfs);
            if let Ok(target) = target {
                // `or_insert`: a module reached twice keeps the directory of the
                // route that reached it *first*, because the reference caches the
                // whole parse result on that route (`Parser/IO.hs:91-95`).
                dirs.entry(target)
                    .or_insert_with(|| kg_resolve::module::module_dir(target, &from_dir, vfs));
            }
            out.push((decl.id, target));
        }
        resolved.insert(module, out.clone());
        out
    });

    // Everything the base library's own DFS reached, base included. Those
    // modules were parsed from `emptySymbolTable` and so do **not** carry the
    // implicit base import; every module first reached from a command-line root
    // does (`Parser/IO.hs:71-77`). Post-order is what makes this a prefix: a
    // module is appended only once its whole subtree has finished.
    let base_subtree = base
        .and_then(|b| walk.files.iter().position(|&m| m == b))
        .map_or(0, |i| i + 1);

    let mut graph = ModuleGraph::new();
    let mut trees: Vec<(ModuleSource, ItemTree)> = Vec::with_capacity(walk.files.len());

    for (index, &module) in walk.files.iter().enumerate() {
        let tree = db.item_tree(module);
        graph.add_module(module, tree.module_content());

        if index >= base_subtree
            && let Some(base) = base
        {
            graph.add_import(module, implicit_base_import(module), base, interner);
        }
        for decl in &tree.imports {
            let target = resolved
                .get(&module)
                .and_then(|rs| rs.iter().find(|(id, _)| *id == decl.id))
                .and_then(|(_, t)| t.as_ref().ok());
            let Some(&target) = target else { continue };

            // `updateSymbolTable` (`Parser.hs:200-206`, `docs/spec/resolve.md`
            // §1.4): the file that was found must *declare* the module that was
            // imported. On a mismatch the reference calls `parseFileFailure`
            // instead of `updateImports`, so the import contributes no symbols
            // at all — which is why this `continue`s rather than merely warning.
            // A file with no `module` declaration has no namespace and therefore
            // never matches, which is what makes every importable file carry
            // one. Oracle: `test/syntax/imports/a1/a2.k:4` declares `module
            // a1.a3` while living at `a1/a2.k` (`imports.k:270-272`,
            // `expected:1`).
            let declared = db.item_tree(target).module_header.as_ref().and_then(|h| h.namespace);
            if declared != Some(Namespace::intern(&decl.name, interner)) {
                diagnostics.push(Diagnostic::error(
                    decl.range,
                    ImportError::NameMismatch { expected: decl.name.clone() }.to_string(),
                ));
                continue;
            }

            graph.add_import(module, decl.clone(), target, interner);
        }

        // `updateVisible` at end of parse, once every import is in
        // (`Parser.hs:108,213`). Post-order guarantees each import's interface
        // already exists — except across a cycle, where the reference has
        // already failed the import.
        graph.build_interface(module, interner);

        trees.push((module, (*tree).clone()));
    }

    // `checkExposed`'s module half (`Parser.hs:224-231`), reported at offset 0.
    // The third check, `checkSymbols`, needs `find_symbol` and belongs to
    // whoever wires name resolution.
    for &module in &walk.files {
        diagnostics.extend(graph.check_exposed_modules(module, interner));
    }

    // `updateNeeded` (`SymbolTable.hs:263-270`), always: the reference marks
    // an import `Needed` inside `findSymbol` while it parses, whatever the
    // warning switch says, and `checkRedundantImports` (`Parser.hs:234-241`)
    // wraps the parser's seed in one `Warning` node per import still
    // `Redundant` — a node the backend walk emits (`kg-ptbridge`), so which
    // imports are redundant is a fact of the tree, not of the report. Until
    // 2026-09-17 the replay ran only with the warning enabled, and under
    // `--Wno-redundant-import` — every syntax test's flag — every import of
    // the base library read as redundant and the seed fired on every file.
    // Runs after the loop above so that every interface `find_in_import`
    // reads already exists; the replay early-outs for a module whose
    // imports `updateImports`' seeds already justified.
    let is_type = |def: kg_resolve::DefId| {
        trees
            .iter()
            .find(|(m, _)| *m == def.module)
            .and_then(|(_, t)| t.item(def.item))
            .is_some_and(|i| i.kind.is_type())
    };
    for &module in &walk.files {
        let parse = db.parse_module(module);
        let tree = db.item_tree(module);
        crate::imports::mark_used_imports(
            &mut graph,
            module,
            &parse.source_file(),
            &tree,
            interner,
            &is_type,
        );
    }
    // The report alone is behind the switch.
    if settings.warnings.is_enabled(kg_diag::WarningKind::RedundantImport) {
        for &module in &walk.files {
            diagnostics.extend(crate::imports::redundant_import_warnings(&graph, module));
        }
    }

    // `parseFileFailure offset err` (`Parser.hs:201`) — reported at the `import`
    // keyword of the importing file, in visit order. The keyword is looked up
    // in the importer's *item tree*, not in `graph`: a failed import is exactly
    // the one the loop above never added to the graph, so a graph lookup found
    // nothing and every missing-import error landed at offset 0 — the first
    // import's keyword, or a copyright comment. Only the implicit base import
    // has no declaration to point at, and the reference reports that one at
    // offset 0 too (`addImport Needed 0 Nothing`, `Parser/IO.hs:77`).
    for failure in &walk.errors {
        let at = db
            .item_tree(failure.importer)
            .imports
            .iter()
            .find(|d| d.id == failure.import)
            .map_or_else(|| failure.importer.start(), |d| d.range);
        diagnostics.push(Diagnostic::error(at, failure.error.to_string()));
    }

    Program {
        roots: roots.to_vec(),
        base,
        files: walk.files,
        graph,
        diagnostics,
        failures: walk.errors,
        trees,
    }
}

/// The synthetic `import <base>` every command-line file starts with.
fn implicit_base_import(module: ModuleSource) -> ImportDecl {
    ImportDecl {
        id: IMPLICIT_BASE_IMPORT,
        name: Vec::new(),
        alias: None,
        range: module.start(),
        // `addImport Needed` — seeded needed, which is why the base library and
        // `.cmdargs` never produce a `redundant-import` warning
        // (`Parser/IO.hs:77`).
        needed: ImportNeeded::Needed,
    }
}
