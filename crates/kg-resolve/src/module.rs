//! Owner-topic: **the module graph** — module names, the import search order,
//! export lists, re-exports, module difference, and the `redundant-import`
//! warning.
//!
//! Spec: `docs/spec/resolve.md` §1 (naming and the search order), §2 (`module`,
//! `import`, exports, re-exports), §5 (the synthetic `.cmdargs` / `.options`
//! modules), §6 (the implicit base), §8 (import cycles), §9.1 (how the device
//! config is reached), §11.2 (the Rust design).
//! Reference: `Parser/IO.hs:37-110` (the search, the synthetic modules, the
//! cycle-detecting DFS), `Parser.hs:179-241` (the grammar and the checks),
//! `SymbolTable.hs:94-181,272-279` (the module record and the re-export algebra).
//!
//! # The three records, and why there are three
//!
//! The reference keeps one mutable `Module` record per file, holding *both* the
//! module's own symbols and everything it can see. Splitting that is what makes
//! the graph incremental:
//!
//! | here | reference | produced by | consumed by |
//! |---|---|---|---|
//! | [`ModuleContent`] | `Module{namespace,symbols,exposed*}` | lowering one file | [`ModuleGraph::build_interface`] |
//! | [`ImportDecl`] + [`ModuleGraph::import_target`] | `Import` (`SymbolTable.hs:81-88`) | import resolution | [`ModuleGraph::find_in_import`] |
//! | [`ModuleInterface`] | `Module{visibleSymbols,moduleNeeded}` | [`ModuleGraph::build_interface`] | importers |
//!
//! Only the last crosses a module boundary, so editing a function body produces
//! an identical [`ModuleInterface`] and stops invalidation there.

use kg_base::{
    FileId, FileRange, FxHashMap, FxHashSet, Interner, SmolStr, Symbol, SymbolPath, TextRange,
    TextSize,
};
use kg_diag::Diagnostic;

use crate::def::{DefId, ItemId};

/// Canonical identity of a module. The two synthetic modules have no [`FileId`]
/// (`docs/spec/resolve.md` §11.2).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ModuleSource {
    File(FileId),
    /// `.cmdargs` — generated from `--define` / `--using`
    /// (`Parser/IO.hs:51-58`). Never touches the filesystem
    /// (`Parser/IO.hs:101`).
    Cmdargs,
    /// `.options` — generated from the backend options (`Parser/IO.hs:60-65`).
    Options,
}

impl ModuleSource {
    /// The file this module was read from, if any.
    pub fn file(self) -> Option<FileId> {
        match self {
            ModuleSource::File(f) => Some(f),
            _ => None,
        }
    }

    /// A [`FileId`] to hang a diagnostic on. The synthetic modules answer
    /// [`FileId::SYNTHETIC`], which is exactly what that constant is for: their
    /// `Src` `sourceName` is `.cmdargs.k` / `.options.k`, which are not real
    /// files (`docs/spec/resolve.md` §5.4).
    pub fn diagnostic_file(self) -> FileId {
        self.file().unwrap_or(FileId::SYNTHETIC)
    }

    /// `registerFancyError 0` — the offset both export-list checks report at
    /// (`Parser.hs:227`, `:231`).
    pub fn start(self) -> FileRange {
        FileRange::new(self.diagnostic_file(), TextRange::empty(TextSize::new(0)))
    }
}

/// The dotted module name **as written**, e.g. `["a1", "b1"]` or `[".cmdargs"]`.
///
/// Components are `SmolStr`, not [`Symbol`], because a module-name component may
/// contain `-` (`dotSepName`, `Parser/Lexer.hs:230-233`) and is therefore not
/// necessarily a legal identifier — `library/mini-base.k:12` declares
/// `module mini-base`. Interning them alongside identifiers would be a category
/// error.
pub type ModuleName = Vec<SmolStr>;

/// The mangled namespace: `["a1","b1"] → "@a1@b1"`
/// (`moduleNamespace`, `Parser/Syntax.hs:273-274`).
///
/// It is a **single flat name**, not a multi-component qualifier: `a1.b1.b1` and
/// `a1.b2.b2` are unrelated namespaces with no containment relation
/// (`docs/spec/resolve.md` §10, confirmed by `test/syntax/imports.k:164-193`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Namespace(pub Symbol);

impl Namespace {
    /// `moduleNamespace = fromString . concatMap ('@':)`
    /// (`Parser/Syntax.hs:273-274`).
    pub fn mangle(name: &[SmolStr]) -> String {
        let mut out = String::new();
        for part in name {
            out.push('@');
            out.push_str(part);
        }
        out
    }

    /// [`Namespace::mangle`] followed by interning — the only way to make one,
    /// because `Name` is `Hashed String` on the other side
    /// (`Parser/Syntax.hs:51`).
    pub fn intern(name: &[SmolStr], interner: &Interner) -> Namespace {
        Namespace(interner.intern(&Namespace::mangle(name)))
    }

    /// `unmangleModuleNamespace` (`Parser/Syntax.hs:276-279`): strip the leading
    /// `@`, then map every remaining `@` to `.`.
    pub fn unmangle(mangled: &str) -> String {
        kg_base::unmangle_namespace(mangled)
    }

    /// The dotted spelling used in the two export-list error messages. Equal to
    /// `unmangle(mangle(name))` for every name `dotSepName` accepts, since a
    /// component may not contain `@`.
    pub fn dotted(name: &[SmolStr]) -> String {
        name.join(".")
    }
}

/// The literal module-name token for the synthetic modules
/// (`Parser.hs:41-45`). They are matched *before* `dotSepName`, so a real module
/// can never be called this.
pub const CMDARGS_MODULE: &str = ".cmdargs";
pub const OPTIONS_MODULE: &str = ".options";

/// `cmdargsSpecialModule <.> "k"` (`Parser/IO.hs:101`) — the *file name* the two
/// short-circuits compare against, not the module name.
pub const CMDARGS_FILE: &str = ".cmdargs.k";
pub const OPTIONS_FILE: &str = ".options.k";

/// `baseLibrary = "base.k"` (`Parser/Options.hs:38`, `app/Options/CmdArgs.hs:66`).
pub const DEFAULT_BASE_LIBRARY: &str = "base.k";

/// Why an import failed to resolve.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ImportError {
    /// The exact text is
    /// `"Can't find file: <f> in any of the import directories\n" ++ unlines importDirs`
    /// (`Parser/IO.hs:109`). Note it lists `importDirs` and therefore **omits**
    /// the importing file's own directory.
    NotFound { file: String, searched: Vec<String> },
    /// `"Imported module name does not match, expected a.b.c"`
    /// (`Parser.hs:205`). Oracle: `test/syntax/imports/a1/a2.k:4` declares
    /// `module a1.a3` while living at `a1/a2.k` — `expected:1`.
    NameMismatch { expected: ModuleName },
    /// I/O or UTF-8 decode failure, rendered by `Ex.displayException`
    /// (`Parser/IO.hs:106`). Oracle: `test/syntax/imports/binary.k`.
    ReadFailed { message: String },
    /// `"Circular dependency"` — see `docs/spec/resolve.md` §8.
    Cycle,
}

impl std::fmt::Display for ImportError {
    /// The reference's exact strings; `parseFileFailure offset err`
    /// (`Parser.hs:201`) reports them verbatim at the `import` keyword.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // `unlines` terminates *every* element with a newline, including the
            // last (`Parser/IO.hs:108`).
            ImportError::NotFound { file, searched } => {
                writeln!(f, "Can't find file: {file} in any of the import directories")?;
                for dir in searched {
                    writeln!(f, "{dir}")?;
                }
                Ok(())
            }
            ImportError::NameMismatch { expected } => {
                write!(f, "Imported module name does not match, expected {}", expected.join("."))
            }
            ImportError::ReadFailed { message } => f.write_str(message),
            ImportError::Cycle => f.write_str("Circular dependency"),
        }
    }
}

/// Whether an `import` has justified itself (`SymbolTable.hs:81-88`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum ImportNeeded {
    #[default]
    Redundant,
    Needed,
}

impl ImportNeeded {
    pub fn is_needed(self) -> bool {
        self == ImportNeeded::Needed
    }

    /// `Needed` absorbs: an import that has justified itself once never goes
    /// back (`updateNeeded`, `SymbolTable.hs:269-270`).
    pub fn join(self, other: ImportNeeded) -> ImportNeeded {
        if self.is_needed() || other.is_needed() { ImportNeeded::Needed } else { ImportNeeded::Redundant }
    }
}

/// Index of an `import` within one file, **in source order**.
///
/// The reference builds its `imports` list by *cons*, so it is in **reverse**
/// source order (`SymbolTable.hs:110`), and `findInImports` iterates it in that
/// reverse order (`SymbolTable.hs:263-270`). Keep the storage in source order and
/// iterate backwards; do not store it reversed, or the `redundant-import`
/// attribution flips.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ImportId(pub u32);

/// One `import a.b [as X];` — the syntax (`Parser.hs:194-206`) and, once it is
/// inside a [`ModuleGraph`], the symbol-table entry `data Import`
/// (`SymbolTable.hs:81-88`).
///
/// The reference's `Import` additionally holds the imported `Module` record *by
/// value*, a snapshot taken when the imported file finished parsing. Here that
/// snapshot is [`ModuleGraph::interfaces`]`[target]` — the same thing minus the
/// parts an importer cannot see — reached through
/// [`ModuleGraph::import_target`], because lowering resolves no paths and cannot
/// fill a target in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImportDecl {
    pub id: ImportId,
    pub name: ModuleName,
    /// `as X` — a plain, non-reserved, non-dashed identifier (`Parser.hs:197`).
    ///
    /// An aliased import **hides its symbols from unqualified use**, because
    /// `findInImport` strips the alias with `stripPrefix` and
    /// `stripPrefix [] ns == Just ns` only for the unaliased case
    /// (`SymbolTable.hs:276-278`; `test/syntax/imports.k:334-353`).
    pub alias: Option<Symbol>,
    /// Location of the `import` keyword — where the `redundant-import` warning
    /// is reported (`SymbolTable.hs:86`).
    pub range: FileRange,
    /// `importNeeded` (`SymbolTable.hs:86`).
    ///
    /// As produced by lowering this is the **seed** `addImport` is called with:
    /// every written `import` seeds `Redundant`, and the seed exists at all
    /// because the implicit base import is added with `Needed` from outside the
    /// grammar — `addImport Needed 0 Nothing` (`Parser/IO.hs:77`) — which is why
    /// `base` and `.cmdargs` never warn.
    ///
    /// Inside a [`ModuleGraph`] it is live state:
    /// [`ModuleGraph::add_import`] joins the seed with the three rules of
    /// `updateImports`, and [`ModuleGraph::mark_needed`] flips it as names
    /// resolve (`SymbolTable.hs:266-270`).
    pub needed: ImportNeeded,
}

/// One entry of a `module M { … }` export list (`Parser.hs:187-192`). The three
/// alternatives are tried in this order, the first two wrapped in `try`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ExportItem {
    /// A plain identifier — goes into `exposedSymbols`.
    Symbol(Symbol),
    /// `module a.b` — goes into `exposedModules`.
    Module(ModuleName),
    /// `module a.b \ c.d` — goes into `exposedDiffs`.
    Difference(ModuleName, ModuleName),
}

/// The `module` header of a file (`SymbolTable.hs:114-133`).
///
/// `updateModule` runs **before** the body is parsed (`Parser.hs:188` precedes
/// `Parser.hs:189`), so `exposed_modules` is already populated when imports are
/// processed — which `updateImports` relies on.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ModuleHeader {
    pub name: ModuleName,
    pub namespace: Option<Namespace>,
    /// The three **disjoint** buckets of `updateModule`
    /// (`SymbolTable.hs:117-126`).
    pub exports: Vec<ExportItem>,
    /// Every plain name of the export list with where it is written — what
    /// `checkSymbols` (`Parser.hs:219-223`) reports "undefined symbol" at.
    pub exported_at: Vec<(Symbol, kg_base::TextRange)>,
}

impl ModuleHeader {
    /// `exposedModules :: Set Name` (`SymbolTable.hs:118`). A `ModuleDiff` entry
    /// contributes **nothing** here — `exposedModule` matches `ModuleIdentifier`
    /// only (`SymbolTable.hs:124-125`), which is why
    /// `module diff.foo_self { module diff.foo \ diff.foo_self, module diff.foo_self }`
    /// has `exposedModules == {@diff@foo_self}` and *not* `@diff@foo`.
    pub fn exposed_modules(&self, interner: &Interner) -> FxHashSet<Namespace> {
        self.exports
            .iter()
            .filter_map(|e| match e {
                ExportItem::Module(n) => Some(Namespace::intern(n, interner)),
                _ => None,
            })
            .collect()
    }

    /// The key set of `exposedSymbols :: Map Name ExpSrc`
    /// (`SymbolTable.hs:119`). The values are the export items themselves, which
    /// only `checkSymbols` needs — to report `` `X`: undefined symbol `` *at the
    /// export item* (`Parser.hs:216-222`); that check needs `findSymbol` and so
    /// reads [`ModuleHeader::exports`] directly.
    pub fn exposed_symbols(&self) -> FxHashSet<Symbol> {
        self.exports
            .iter()
            .filter_map(|e| match e {
                ExportItem::Symbol(s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    /// `exposedDiffs :: [(Name, Name)]` (`SymbolTable.hs:120`), in source order.
    pub fn exposed_diffs(&self, interner: &Interner) -> Vec<(Namespace, Namespace)> {
        self.exports
            .iter()
            .filter_map(|e| match e {
                ExportItem::Difference(a, b) => {
                    Some((Namespace::intern(a, interner), Namespace::intern(b, interner)))
                }
                _ => None,
            })
            .collect()
    }
}

/// One entry of a module's own parse-time symbol map (`symbols`,
/// `SymbolTable.hs:73`) — everything `parseSymbol` filed under
/// `scopeQualifier ++ [name]` (`SymbolTable.hs:211`), *including* nested keys
/// like `["@a1@a5","N","Aa"]` and function-local `["@a1@a26","test","$","a"]`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ModuleSymbol {
    pub item: ItemId,
    /// `exposable` (`SymbolTable.hs:168-169`): `False` for a `Variable` whose
    /// type is not `const`, `True` for everything else. Only whole-module exports
    /// consult it — an explicitly named export goes through `checkSymbols`
    /// instead, which reports `"global, mutable state can't be exposed outside a
    /// module"` (`Parser.hs:221-222`).
    pub exposable: bool,
}

/// Everything [`ModuleGraph::build_interface`] reads out of one module besides
/// its imports — the half of the reference's `Module` record that never leaves
/// the file.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ModuleContent {
    pub header: ModuleHeader,
    /// `symbols` (`SymbolTable.hs:73`).
    pub symbols: FxHashMap<SymbolPath, ModuleSymbol>,
    /// `any exported symbols` (`SymbolTable.hs:149,175-181`): the module contains
    /// an `export T;` or a `[[reset]]` function. Precomputed by lowering because
    /// it needs the declaration's attributes, which do not survive into
    /// [`ModuleSymbol`].
    pub has_export_or_reset: bool,
}

/// namespace → (qualified name → definition) — `Map Name Symbols`
/// (`SymbolTable.hs:74`).
pub type VisibleSymbols = FxHashMap<Namespace, FxHashMap<SymbolPath, DefId>>;

/// A module's public surface — the Rust image of `SymbolTable.Module` minus the
/// parts a consumer cannot see (`docs/spec/resolve.md` §11.2).
///
/// Returning only the *interface*, never the bodies, is the single most important
/// incrementality decision: editing a function body in `library/data/array.k`
/// must produce an identical interface and stop invalidation at that boundary.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ModuleInterface {
    pub namespace: Option<Namespace>,
    /// `visibleSymbols` (`SymbolTable.hs:135-181`): namespace → (qualified name →
    /// definition). Built as `Map.unionsWith HashMap.union [exposed, reExposed, diffs]`.
    ///
    /// The value is a [`DefId`], **not** an [`ItemId`] as
    /// `docs/spec/resolve.md` §11.2 sketches: a re-exported symbol belongs to a
    /// different file, so an item index alone cannot name it.
    ///
    /// A file with no `module` declaration has no entry here at all, where the
    /// reference has one keyed by the empty namespace `""` (`SymbolTable.hs:94`,
    /// `:173`) mapping to an empty symbol map. The difference is unobservable:
    /// the inner map is empty either way, and §1.4's namespace check means such a
    /// file can never be imported in the first place.
    pub visible: VisibleSymbols,
    /// `moduleNeeded` (`SymbolTable.hs:149`): the module contains an `export T;`
    /// or a `[[reset]]` function, so importing it is never redundant
    /// (rule 3 of `docs/spec/resolve.md` §2.9).
    pub needed: bool,
}

impl ModuleInterface {
    pub fn lookup(&self, namespace: Namespace, key: &SymbolPath) -> Option<DefId> {
        self.visible.get(&namespace).and_then(|m| m.get(key)).copied()
    }
}

/// `symbolName = head . tail` (`SymbolTable.hs:138`) — the **second** component
/// of a symbol-map key, i.e. the top-level name inside the module's namespace,
/// even for a nested key: `["@a1@a5","N","Aa"] → "N"`.
///
/// Haskell's `head . tail` is partial and diverges on a key shorter than two
/// components. Such keys exist — a file with no `module` declaration has an empty
/// qualifier, so its keys are `["x"]` — and the reference gets away with it only
/// because `Data.Map.member` on the *empty* `exposedSymbols` map short-circuits
/// before forcing the key, and a file with no `module` declaration necessarily has
/// no exports. `None` reproduces the reachable behaviour without the trap.
fn symbol_name(key: &SymbolPath) -> Option<Symbol> {
    key.segments().get(1).copied()
}

/// Ordered candidate directories for one import (`docs/spec/resolve.md` §1.3).
///
/// ```text
/// [ dir_of_importing_file
/// , D1/device/<target>, …, Dn/device/<target>
/// , D1,                 …, Dn ]
/// ```
///
/// It is **not** interleaved: every `-I` directory's `device/<target>`
/// subdirectory is tried before *any* plain `-I` directory
/// (`importDirs = map (</> "device" </> targetDevice opt) (importDir opt) ++ importDir opt`,
/// `Parser/IO.hs:110`). `kg_config::Settings::import_search_path` already builds
/// the tail; this prepends the importing file's own directory, which is the part
/// that varies per import.
pub fn search_dirs(
    from_dir: &kg_base::Utf8Path,
    settings: &kg_config::Settings,
) -> Vec<kg_base::Utf8PathBuf> {
    let mut dirs = Vec::with_capacity(1 + settings.import_dirs.len() * 2);
    dirs.push(from_dir.to_owned());
    dirs.extend(settings.import_search_path());
    dirs
}

/// `joinPath ns <.> "k"` (`Parser.hs:194-199`) — the file name an import asks
/// for. The extension is always `k`, hardcoded.
///
/// `addExtension ".cmdargs" "k" == ".cmdargs.k"`: filepath prepends the `.`
/// because the argument does not already carry an extension separator.
pub fn module_file_name(name: &[SmolStr]) -> String {
    let mut path = name.join(std::path::MAIN_SEPARATOR_STR);
    path.push_str(".k");
    path
}

/// Resolve a module name to its source (`docs/spec/resolve.md` §1.2-§1.3).
///
/// Two short-circuits precede the search entirely: the literal names
/// `.cmdargs.k` and `.options.k` never touch the filesystem
/// (`Parser/IO.hs:101-102`).
pub fn resolve_module_path(
    name: &[SmolStr],
    from_dir: &kg_base::Utf8Path,
    settings: &kg_config::Settings,
    vfs: &kg_vfs::Vfs,
) -> Result<ModuleSource, ImportError> {
    resolve_import_file(&module_file_name(name), from_dir, settings, vfs)
}

/// `readFileContent` (`Parser/IO.hs:100-110`) — the same search applied to a
/// *file name* rather than a dotted module name.
///
/// The extra entry point exists because `--base-library` is a path, not a module
/// name, and goes through the identical list: it may be absolute
/// (`test/syntax/CMakeLists.txt:43`) or a relative multi-segment path
/// (`CMakeLists.txt:91`, `helper/custom_base.k`). An absolute `file` short-circuits
/// on the first probe, because filepath's `</>` — and `Utf8Path::join` — return
/// the second argument when it is absolute.
///
/// Existence is decided by reading, not by `stat`: `System.Directory.findFile`
/// stats, but the [`kg_vfs::Vfs`] must also see an unsaved editor buffer for a
/// file that is not on disk at all, and the found file is about to be read anyway.
/// A file the read finds but cannot decode or open still *exists*, so the
/// search stops at it, as `findFile` does, with [`ImportError::ReadFailed`]
/// (`Parser/IO.hs:103-107`; `test/syntax/imports/binary.k`). Until 2026-10-10
/// it read as absent and reported [`ImportError::NotFound`], or went on to a
/// same-named file further down the list.
pub fn resolve_import_file(
    file: &str,
    from_dir: &kg_base::Utf8Path,
    settings: &kg_config::Settings,
    vfs: &kg_vfs::Vfs,
) -> Result<ModuleSource, ImportError> {
    // Parser/IO.hs:101-102, before any filesystem access.
    if file == CMDARGS_FILE {
        return Ok(ModuleSource::Cmdargs);
    }
    if file == OPTIONS_FILE {
        return Ok(ModuleSource::Options);
    }

    for dir in search_dirs(from_dir, settings) {
        let candidate = dir.join(file);
        let id = vfs.intern(&candidate);
        if vfs.read(id).is_some() {
            return Ok(ModuleSource::File(id));
        }
        if let Some(failure) = vfs.read_failure(id) {
            return Err(read_failed(&candidate, failure));
        }
    }

    Err(ImportError::NotFound {
        file: file.to_owned(),
        // `unlines importDirs` — the importing file's own directory is *not*
        // listed, even though it was searched (`Parser/IO.hs:108`).
        searched: settings.import_search_path().iter().map(|d| d.to_string()).collect(),
    })
}

/// `displayException` of what `readFileUtf8 path'` throws (`Parser/IO.hs:106-107`,
/// `Internal.hs:24-32`). GHC shows an `IOException` as `<file>: <location>:
/// <type> (<detail>)`, and `<file>` is `path'` — the path as the search joined
/// it, not the canonical one. Both texts read off `kg-oracle --errors`
/// (GHC 9.6.6), 2026-10-10: the decode detail is the runtime's, not
/// Kanagawa's, and names the first byte it could not decode. The compat
/// binary prints the same text for a command-line file, which `parseFile`
/// throws instead of reporting (`Parser/IO.hs:83`).
pub fn read_failed(path: &kg_base::Utf8Path, failure: kg_vfs::ReadFailure) -> ImportError {
    let message = match failure {
        // `hGetContents` is lazy; `evaluate (length s)` forces the decode, and
        // `lazyRead` stamps its own name on the failure.
        kg_vfs::ReadFailure::Undecodable { byte } => {
            format!("{path}: hGetContents: invalid argument (cannot decode byte sequence starting from {byte})")
        }
        kg_vfs::ReadFailure::PermissionDenied => {
            format!("{path}: openFile: permission denied (Permission denied)")
        }
    };
    ImportError::ReadFailed { message }
}

/// The directory that `module`'s **own** imports search first
/// (`Parser/IO.hs:93,107`).
///
/// For a real file it is `takeDirectory path'` — the directory of the path *as
/// joined during the search*, not of the canonicalised one, so an import found
/// through a relative `-I ../library` keeps searching `../library` first. For the
/// two synthetic modules `readFileContent` returns the incoming `dir` unchanged
/// (`Parser/IO.hs:101-102`), i.e. the **importing** file's directory — which is
/// why `.options`' own `import compiler.options` resolves relative to whoever
/// imported it.
pub fn module_dir(
    source: ModuleSource,
    importer_dir: &kg_base::Utf8Path,
    vfs: &kg_vfs::Vfs,
) -> kg_base::Utf8PathBuf {
    match source.file() {
        Some(file) => vfs
            .path(file)
            .and_then(|p| p.parent().map(|d| d.to_owned()))
            .unwrap_or_else(|| importer_dir.to_owned()),
        None => importer_dir.to_owned(),
    }
}

/// `importBase` (`Parser/IO.hs:72-77`) — the file name of the implicit base
/// import, `docs/spec/resolve.md` §6.
///
/// **`--no-implicit-base` does not remove `.cmdargs`**: with either
/// `--no-implicit-base` or *no* `--import-dir` at all, the base file becomes
/// `.cmdargs.k`, so `--define`d symbols are always in scope. The base file is
/// itself parsed with an empty symbol table and gets no base of its own, which is
/// why `library/base.k:30` and `library/mini-base.k:18` `import .cmdargs`
/// explicitly.
pub fn implicit_base_file(settings: &kg_config::Settings) -> String {
    if !settings.implicit_base || settings.import_dirs.is_empty() {
        CMDARGS_FILE.to_owned()
    } else {
        settings
            .base_library
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| DEFAULT_BASE_LIBRARY.to_owned())
    }
}

/// The hits one name produced across a module's imports, and which imports they
/// came from.
///
/// `findInImports` (`SymbolTable.hs:263-270`) both computes and mutates; the two
/// halves are split so the lookup stays a pure, cacheable query and the
/// `Needed` marking is an explicit [`ModuleGraph::mark_needed`] call. Doing both
/// at once is [`ModuleGraph::resolve_in_imports`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ImportHits {
    /// `concat foundUnique`, in the reference's iteration order — **reverse
    /// source order**, so the last-written import's hits come first.
    pub defs: Vec<DefId>,
    /// The imports whose residual set was non-empty; `updateNeeded` flips exactly
    /// these to `Needed`.
    pub used: Vec<ImportId>,
}

/// The whole module graph for one compilation.
///
/// `files` is in **post-order DFS of the import graph** with the base library
/// first — the order in which files finished parsing (`Parser/IO.hs:96`,
/// `Main.hs:70,76`). That order decides duplicate-key precedence in the global
/// symbol map: **the earlier file wins** (`Symbols.hs:41`,
/// `docs/spec/resolve.md` §3.1).
#[derive(Clone, Debug, Default)]
pub struct ModuleGraph {
    pub files: Vec<ModuleSource>,
    pub contents: FxHashMap<ModuleSource, ModuleContent>,
    pub interfaces: FxHashMap<ModuleSource, ModuleInterface>,
    /// **In source order** — see [`ImportId`].
    pub imports: FxHashMap<ModuleSource, Vec<ImportDecl>>,
    /// The module each import resolved to, positionally parallel to
    /// [`ModuleGraph::imports`]: `targets[m][i]` is the target of the import with
    /// `ImportId(i)`.
    ///
    /// Kept beside the declarations rather than inside them because an
    /// [`ImportDecl`] is produced by lowering, which resolves no paths — the same
    /// split as `Parser.hs:194-199` (parse the name) versus `Parser/IO.hs:100-110`
    /// (find the file). An import that failed to resolve has **no** entry in
    /// either vector: `parseImports` never reaches `updateImports` for it
    /// (`Parser.hs:201-206`), so it contributes no symbols and cannot be
    /// redundant.
    pub targets: FxHashMap<ModuleSource, Vec<ModuleSource>>,
}

impl ModuleGraph {
    pub fn new() -> ModuleGraph {
        ModuleGraph::default()
    }

    pub fn content(&self, module: ModuleSource) -> Option<&ModuleContent> {
        self.contents.get(&module)
    }

    pub fn interface(&self, module: ModuleSource) -> Option<&ModuleInterface> {
        self.interfaces.get(&module)
    }

    /// In source order.
    pub fn imports(&self, module: ModuleSource) -> &[ImportDecl] {
        self.imports.get(&module).map_or(&[], Vec::as_slice)
    }

    /// The imports of `module` paired with what they resolved to, in source
    /// order. The reference's `imports` list is this, reversed.
    pub fn resolved_imports(
        &self,
        module: ModuleSource,
    ) -> impl DoubleEndedIterator<Item = (&ImportDecl, ModuleSource)> {
        let targets = self.targets.get(&module).map_or(&[][..], Vec::as_slice);
        self.imports(module).iter().zip(targets.iter().copied())
    }

    /// The module one import resolved to.
    pub fn import_target(&self, module: ModuleSource, import: ImportId) -> Option<ModuleSource> {
        self.resolved_imports(module).find(|(d, _)| d.id == import).map(|(_, t)| t)
    }

    /// Record one lowered file. Does **not** build the interface: `updateVisible`
    /// runs at end of parse, after every import has been added
    /// (`Parser.hs:108,213`).
    pub fn add_module(&mut self, module: ModuleSource, content: ModuleContent) {
        self.contents.insert(module, content);
        self.imports.entry(module).or_default();
        self.targets.entry(module).or_default();
    }

    /// `updateImports` + `addImport` (`SymbolTable.hs:99-112`).
    ///
    /// The import is pre-marked `Needed` when the **importing** module re-exposes
    /// it as a whole module, names it on either side of a difference, or when the
    /// **imported** module is itself `Needed`. Those three rules are why
    /// `import diff.foo` in `diff/foo_minus_bar.k` does not warn even though no
    /// name resolves through it at all (`test/syntax/imports.k:893-901`,
    /// `warning:0`).
    pub fn add_import(
        &mut self,
        importer: ModuleSource,
        mut decl: ImportDecl,
        target: ModuleSource,
        interner: &Interner,
    ) -> ImportId {
        decl.needed = decl.needed.join(self.initial_needed(importer, target, interner));
        let id = decl.id;
        self.imports.entry(importer).or_default().push(decl);
        self.targets.entry(importer).or_default().push(target);
        id
    }

    /// The three rules of `updateImports` (`SymbolTable.hs:103-107`).
    ///
    /// Rules 1 and 2 compare the **imported module's declared namespace** against
    /// the importer's export list. That namespace is also derivable from the
    /// import's written name — `Parser.hs:202` has already rejected the import
    /// unless the two agree — which is why lowering may legitimately seed
    /// [`ImportDecl::needed`] with them before any interface exists.
    pub fn initial_needed(
        &self,
        importer: ModuleSource,
        target: ModuleSource,
        interner: &Interner,
    ) -> ImportNeeded {
        let Some(header) = self.contents.get(&importer).map(|c| &c.header) else {
            return ImportNeeded::Redundant;
        };
        let target_ns = self
            .interfaces
            .get(&target)
            .and_then(|i| i.namespace)
            .or_else(|| self.contents.get(&target).and_then(|c| c.header.namespace));
        if let Some(ns) = target_ns {
            if header.exposed_modules(interner).contains(&ns) {
                return ImportNeeded::Needed;
            }
            if header.exposed_diffs(interner).iter().any(|(a, b)| *a == ns || *b == ns) {
                return ImportNeeded::Needed;
            }
        }
        match self.interfaces.get(&target) {
            Some(i) if i.needed => ImportNeeded::Needed,
            _ => ImportNeeded::Redundant,
        }
    }

    /// `updateVisible` — the complete re-export algebra
    /// (`SymbolTable.hs:135-181`, `docs/spec/resolve.md` §2.4) — caching the
    /// result in [`ModuleGraph::interfaces`].
    ///
    /// Every imported module's interface must already be built; the post-order
    /// DFS of [`walk_imports`] guarantees exactly that, and it is the same
    /// guarantee Haskell gets from parsing the import before `checkExposed` runs.
    pub fn build_interface(
        &mut self,
        module: ModuleSource,
        interner: &Interner,
    ) -> ModuleInterface {
        let built = self.compute_interface(module, interner);
        self.interfaces.insert(module, built.clone());
        built
    }

    /// The pure half of [`ModuleGraph::build_interface`].
    ///
    /// `visibleSymbols = Map.unionsWith HashMap.union [exposed, reExposed, diffs]`.
    /// `Map.unionsWith` folds from the left and `HashMap.union` is left-biased, so
    /// on a collision `exposed` beats `reExposed` beats `diffs`.
    pub fn compute_interface(&self, module: ModuleSource, interner: &Interner) -> ModuleInterface {
        let empty = ModuleContent::default();
        let content = self.contents.get(&module).unwrap_or(&empty);
        let header = &content.header;
        let namespace = header.namespace;
        let exposed_modules = header.exposed_modules(interner);
        let exposed_symbols = header.exposed_symbols();
        let exposed_diffs = header.exposed_diffs(interner);

        // `HashMap.filter exposable symbols` (SymbolTable.hs:172).
        let own_exposable = || -> FxHashMap<SymbolPath, DefId> {
            content
                .symbols
                .iter()
                .filter(|(_, s)| s.exposable)
                .map(|(k, s)| (k.clone(), DefId::new(module, s.item)))
                .collect()
        };
        // `filterExposed` (SymbolTable.hs:163-164): keep a key iff its *second*
        // component is a named export. Exposing a class `C` therefore also
        // exposes every `["@M","C",…]` member key.
        let own_named = || -> FxHashMap<SymbolPath, DefId> {
            content
                .symbols
                .iter()
                .filter(|(k, _)| symbol_name(k).is_some_and(|n| exposed_symbols.contains(&n)))
                .map(|(k, s)| (k.clone(), DefId::new(module, s.item)))
                .collect()
        };

        let mut visible: VisibleSymbols = FxHashMap::default();

        // `exposed` (SymbolTable.hs:171-173). Note it is a `Map.singleton` even
        // when the inner map is empty, and unlike `reExposed` it is never dropped
        // for being empty.
        if let Some(ns) = namespace {
            let own = if exposed_modules.contains(&ns) { own_exposable() } else { own_named() };
            visible.insert(ns, own);
        }

        // `reExposed` (SymbolTable.hs:158-166), over `imports` — reverse source
        // order, so the last-written import wins a key collision.
        let mut re_exposed: VisibleSymbols = FxHashMap::default();
        for (decl, target) in self.resolved_imports(module).rev() {
            let Some(imported) = self.interfaces.get(&target) else { continue };
            let imported_ns = imported.namespace;
            let contributed: VisibleSymbols =
                if imported_ns.is_some_and(|ns| exposed_modules.contains(&ns)) {
                    // Tested *first*, which is why an aliased import that is
                    // explicitly re-exposed still contributes
                    // (`test/syntax/imports/a1/a19.k`).
                    imported.visible.clone()
                } else if decl.alias.is_some() {
                    FxHashMap::default()
                } else {
                    imported
                        .visible
                        .iter()
                        .map(|(ns, syms)| {
                            let kept = syms
                                .iter()
                                .filter(|(k, _)| {
                                    symbol_name(k).is_some_and(|n| exposed_symbols.contains(&n))
                                })
                                .map(|(k, d)| (k.clone(), *d))
                                .collect();
                            (*ns, kept)
                        })
                        .collect()
                };
            union_visible(&mut re_exposed, contributed);
        }
        // `Map.filter (not . HashMap.null)` — applied to the union, not per
        // import (SymbolTable.hs:166).
        re_exposed.retain(|_, syms| !syms.is_empty());

        // `diffs` (SymbolTable.hs:152-156).
        let mut diffs: VisibleSymbols = FxHashMap::default();
        for (n1, n2) in &exposed_diffs {
            let a = self.module_symbols(module, *n1, namespace, &own_exposable());
            let b = self.module_symbols(module, *n2, namespace, &own_exposable());
            union_visible(&mut diffs, difference(&a, &b));
        }

        union_visible(&mut visible, re_exposed);
        union_visible(&mut visible, diffs);

        ModuleInterface { namespace, visible, needed: content.has_export_or_reset }
    }

    /// `moduleSymbols` (`SymbolTable.hs:154-156`): the module itself, or the
    /// `visibleSymbols` of the directly imported module with that namespace.
    ///
    /// The reference's `fromJust` is partial and is protected only by
    /// `checkDiffs` plus laziness — `test/syntax/imports/diff/self_missing.k`
    /// survives solely because the left operand is empty, so the predicate that
    /// would force the `fromJust` is never run. An unknown namespace answers the
    /// empty map here, which agrees on every input the reference does not crash
    /// on and reports the missing import through
    /// [`ModuleGraph::check_exposed_modules`] instead.
    fn module_symbols(
        &self,
        module: ModuleSource,
        namespace: Namespace,
        own_namespace: Option<Namespace>,
        own_exposable: &FxHashMap<SymbolPath, DefId>,
    ) -> VisibleSymbols {
        if own_namespace == Some(namespace) {
            let mut m: VisibleSymbols = FxHashMap::default();
            m.insert(namespace, own_exposable.clone());
            return m;
        }
        self.resolved_imports(module)
            .rev()
            .find_map(|(_, target)| {
                let iface = self.interfaces.get(&target)?;
                (iface.namespace == Some(namespace)).then(|| iface.visible.clone())
            })
            .unwrap_or_default()
    }

    /// `checkSymbols`' other failure (`Parser.hs:219-223`): an exported name
    /// `findSymbol` finds more than once. `findInSymbols` is the module's own
    /// symbols *then* every import's, concatenated (`SymbolTable.hs:261`), and
    /// the exported name is looked up bare, so in every namespace an import
    /// makes visible (`findInImport`, `SymbolTable.hs:272-280`): a name the
    /// module declares that an import also exposes is two definitions — a
    /// device's `hardware.dsp` compiled as the root while the base library
    /// imports the target device's
    /// (`library/device/agilex-7-f-023/hardware/dsp.k`), or `a1.a15`'s own
    /// `memory` beside `data.memory`'s (`test/syntax/imports.k::63`).
    ///
    /// Each export's position, name and definitions, the module's own first.
    /// Whether they are *distinct* is the caller's to decide: `nub` compares
    /// the declarations' trees with notes ignored (`SymbolTable.hs:231`,
    /// `Parser/Syntax.hs:594-595`), so two files declaring the same function
    /// the same way are one definition — which needs the lowered trees.
    pub fn exposed_ambiguities(&self, module: ModuleSource) -> Vec<(FileRange, Symbol, Vec<DefId>)> {
        let Some(content) = self.contents.get(&module) else { return Vec::new() };
        let Some(file) = module.file() else { return Vec::new() };
        let mut out = Vec::new();
        for (name, range) in &content.header.exported_at {
            let hits = self.exposed_definitions(module, *name);
            if hits.len() > 1 {
                out.push((FileRange::new(file, *range), *name, hits));
            }
        }
        out
    }

    /// What an exported name of `module` denotes: `checkSymbols`' lookup of
    /// it (`Parser.hs:216-223`) — the module's own symbol, then every
    /// import's (`findInSymbols`, `SymbolTable.hs:261`), each once. Empty is
    /// "undefined symbol"; more than one, an ambiguity
    /// ([`ModuleGraph::exposed_ambiguities`]); one, the declaration the
    /// name in the list stands for, which a rename of it must rewrite.
    pub fn exposed_definitions(&self, module: ModuleSource, name: Symbol) -> Vec<DefId> {
        let Some(content) = self.contents.get(&module) else { return Vec::new() };
        let module_key = content.header.namespace.map_or_else(SymbolPath::empty, |ns| SymbolPath::single(ns.0));
        let mut hits: Vec<DefId> =
            content.symbols.get(&module_key.child(name)).map(|s| DefId { module, item: s.item }).into_iter().collect();
        for d in self.find_in_imports(module, &SymbolPath::single(name)) {
            if !hits.contains(&d) {
                hits.push(d);
            }
        }
        hits
    }

    /// `checkModules` and `checkDiffs` (`Parser.hs:224-231`) — the half of
    /// `checkExposed` that needs only the module graph. The third check,
    /// `checkSymbols`, needs `findSymbol` and lives with it.
    ///
    /// Both are reported at **offset 0**, not at the export item
    /// (`registerFancyError 0`).
    pub fn check_exposed_modules(
        &self,
        module: ModuleSource,
        interner: &Interner,
    ) -> Vec<Diagnostic> {
        let Some(content) = self.contents.get(&module) else { return Vec::new() };
        let header = &content.header;

        // `moduleNamespaces = namespace : map namespaceImported imports`.
        let mut known: FxHashSet<Namespace> = FxHashSet::default();
        known.extend(header.namespace);
        known.extend(
            self.resolved_imports(module)
                .filter_map(|(_, t)| self.interfaces.get(&t).and_then(|f| f.namespace)),
        );

        let mut out = Vec::new();
        // `mapM_ (checkSymbols namespace) exposedSymbols` runs first
        // (`Parser.hs:212`): a plain exported name must be a symbol of this
        // module or one an import exposes — `base.k` re-exports `optional`
        // from `data.optional` — else it is "`x`: undefined symbol" at the
        // name, a parse-time rejection (`kg-oracle --errors`, 2026-09-10).
        // A name found more than once is [`ModuleGraph::exposed_ambiguities`]'.
        for (name, range) in &header.exported_at {
            if self.exposed_definitions(module, *name).is_empty() && let Some(file) = module.file() {
                out.push(Diagnostic::error(
                    FileRange::new(file, *range),
                    format!("`{}`: undefined symbol", interner.resolve(*name)),
                ));
            }
        }
        // `mapM_ (checkModules …) exposedModules` runs before
        // `mapM_ (checkDiffs …) exposedDiffs`; within each the reference's order
        // is `Data.Set`'s (mangled-string ascending) resp. source order. Source
        // order is used for both here — the set is only ever iterated to emit
        // these two messages.
        for item in &header.exports {
            if let ExportItem::Module(name) = item
                && !known.contains(&Namespace::intern(name, interner))
            {
                out.push(Diagnostic::error(
                    module.start(),
                    format!(
                        "Missing import for re-exposed module {}",
                        Namespace::dotted(name)
                    ),
                ));
            }
        }
        for item in &header.exports {
            if let ExportItem::Difference(a, b) = item {
                if !known.contains(&Namespace::intern(b, interner)) {
                    out.push(Diagnostic::error(
                        module.start(),
                        format!(
                            "Missing import for module used in re-exposed module difference {}",
                            Namespace::dotted(b)
                        ),
                    ));
                } else if !known.contains(&Namespace::intern(a, interner)) {
                    out.push(Diagnostic::error(
                        module.start(),
                        format!(
                            "Missing import for re-exposed module {}",
                            Namespace::dotted(a)
                        ),
                    ));
                }
            }
        }
        out
    }

    /// `findInImport` (`SymbolTable.hs:272-278`) — the two ways a name can hit
    /// one import:
    ///
    /// * the leading component already **is** the mangled namespace (after
    ///   `makeQualified` rewrote an earlier prefix); or
    /// * strip the import alias and prepend the namespace. With **no** alias,
    ///   `stripPrefix [] ns == Just ns`, so the plain name matches.
    ///
    /// Returns a **list**, as the reference does: one imported module can expose
    /// the same name under two namespaces, and that is precisely the ambiguity
    /// `findSymbol`'s `nub` is there to adjudicate. `docs/spec/resolve.md` §11.2
    /// types this as a single `Option`, which cannot express it.
    ///
    /// The order is the reference's `Map.toList` — ascending by mangled namespace
    /// *string*. [`Symbol`]'s `Ord` is interner-insertion order, so the hits are
    /// sorted by [`DefId`] instead; the two agree on the set, and the order is
    /// observable only in the text of an ambiguity message.
    ///
    /// Takes the target and the alias rather than an import record because that
    /// is all `findInImport` destructures out of one: `visibleSymbols
    /// importedModule` and `importedAs`.
    pub fn find_in_import(
        &self,
        target: ModuleSource,
        alias: Option<Symbol>,
        name: &SymbolPath,
    ) -> Vec<DefId> {
        let Some(imported) = self.interfaces.get(&target) else { return Vec::new() };
        let alias: &[Symbol] = match &alias {
            Some(a) => std::slice::from_ref(a),
            None => &[],
        };

        let mut hits = Vec::new();
        for (ns, syms) in &imported.visible {
            let key = match name.first() {
                // `withNamespace (n:ns) | n == namespace = Just $ n:ns`
                Some(first) if first == ns.0 => Some(name.clone()),
                // `withNamespace ns = (namespace:) <$> stripPrefix (maybeToList importedAs) ns`
                _ => name
                    .strip_prefix(alias)
                    .map(|rest| SymbolPath::single(ns.0).concat(&rest)),
            };
            if let Some(hit) = key.and_then(|k| syms.get(&k)) {
                hits.push(*hit);
            }
        }
        hits.sort_unstable();
        hits
    }

    /// `findInImports` (`SymbolTable.hs:263-270`), pure half.
    ///
    /// Three things must all be reproduced:
    /// 1. iterate imports in **reverse source order**;
    /// 2. subtract every earlier-iterated import's hits —
    ///    `foundUnique !! i = f_i \\ f_0 \\ … \\ f_{i-1}`, so the **last**
    ///    `import` in the file wins the attribution;
    /// 3. mark `needed` on any import whose residual set is non-empty.
    ///
    /// Oracle pair: `test/syntax/imports.k:503-512` warns and `:514-523` does
    /// not, purely because the two `import` lines are swapped.
    ///
    /// `\\` removes one occurrence per element rather than every occurrence; the
    /// distinction is unobservable because one import cannot yield the same
    /// [`DefId`] twice (`exposed`, `reExposed` and `diffs` are keyed by disjoint
    /// namespaces for any single declaration).
    pub fn find_in_imports(&self, origin: ModuleSource, name: &SymbolPath) -> Vec<DefId> {
        self.import_hits(origin, name).defs
    }

    /// [`ModuleGraph::find_in_imports`] with the attribution kept — which import
    /// each residual came from, i.e. the `updateNeeded` half.
    pub fn import_hits(&self, origin: ModuleSource, name: &SymbolPath) -> ImportHits {
        let mut hits = ImportHits::default();
        let mut earlier: Vec<DefId> = Vec::new();
        for (decl, target) in self.resolved_imports(origin).rev() {
            let found = self.find_in_import(target, decl.alias, name);
            let residual: Vec<DefId> =
                found.iter().copied().filter(|d| !earlier.contains(d)).collect();
            // `inits` supplies the *original* lists, not the residuals; the two
            // unions are equal, so accumulating the originals is literal.
            earlier.extend(found);
            if !residual.is_empty() {
                hits.used.push(decl.id);
                hits.defs.extend(residual);
            }
        }
        hits
    }

    /// `updateNeeded` (`SymbolTable.hs:269-270`).
    pub fn mark_needed(&mut self, origin: ModuleSource, used: &[ImportId]) {
        let Some(imports) = self.imports.get_mut(&origin) else { return };
        for import in imports.iter_mut() {
            if used.contains(&import.id) {
                import.needed = ImportNeeded::Needed;
            }
        }
    }

    /// `findInImports` in full — the lookup *and* its `Needed` side effect.
    pub fn resolve_in_imports(&mut self, origin: ModuleSource, name: &SymbolPath) -> Vec<DefId> {
        let hits = self.import_hits(origin, name);
        self.mark_needed(origin, &hits.used);
        hits.defs
    }

    /// The imports still `Redundant` at end of file — one `redundant-import`
    /// warning each (`Parser.hs:234-241`).
    ///
    /// The reference folds one `warning` wrapper per redundant import over the
    /// reversed list, so only the set and the offsets are observable, never an
    /// ordering; source order is this implementation's choice, for determinism.
    pub fn redundant_imports(&self, module: ModuleSource) -> impl Iterator<Item = &ImportDecl> {
        self.imports(module).iter().filter(|i| !i.needed.is_needed())
    }
}

/// `Map.unionWith HashMap.union`, left-biased: `into`'s entries win.
fn union_visible(into: &mut VisibleSymbols, from: VisibleSymbols) {
    for (ns, syms) in from {
        let slot = into.entry(ns).or_default();
        for (k, d) in syms {
            slot.entry(k).or_insert(d);
        }
    }
}

/// `difference a b` (`SymbolTable.hs:144-146`): remove from `a` every entry whose
/// *symbol name* (the second key component) occurs as a top-level name in **any**
/// namespace of `b`.
///
/// The membership probe builds the two-element key `[namespace_of_b, symbolName k]`,
/// so it matches only a *top-level* declaration of `b`, never a nested one.
fn difference(a: &VisibleSymbols, b: &VisibleSymbols) -> VisibleSymbols {
    a.iter()
        .map(|(ns, syms)| {
            let kept = syms
                .iter()
                .filter(|(k, _)| match symbol_name(k) {
                    None => true,
                    Some(name) => !b.iter().any(|(bns, bsyms)| {
                        bsyms.contains_key(&SymbolPath::new([bns.0, name]))
                    }),
                })
                .map(|(k, d)| (k.clone(), *d))
                .collect();
            (*ns, kept)
        })
        .collect()
}

/// One import that could not be satisfied.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImportFailure {
    pub importer: ModuleSource,
    pub import: ImportId,
    pub error: ImportError,
}

/// The result of the import DFS.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ImportWalk {
    /// **Post-order DFS**: dependencies precede dependents, base library first.
    /// This is the merge order of the whole-program AST (`Main.hs:70,76`) and
    /// therefore decides duplicate-key precedence in `getSymbols`
    /// (`docs/spec/resolve.md` §3.1) and the `--file-list` content.
    pub files: Vec<ModuleSource>,
    /// In visit order.
    pub errors: Vec<ImportFailure>,
}

/// `parseFile'` (`Parser/IO.hs:79-98`) — the DFS that discovers the program's
/// files and detects import cycles, `docs/spec/resolve.md` §8.
///
/// `imports_of` supplies one module's already-resolved imports, in source order;
/// separating it keeps the walk independent of the filesystem and of the parser.
///
/// The three rules that are easy to get wrong:
///
/// * `ancestorFiles` is the **DFS path**, not a visited set, and is reset to `[]`
///   for every root — the base library and each command-line file
///   (`Parser/IO.hs:82`). The cache, by contrast, spans the whole run.
/// * a module is appended to `files` **after** its subtree completes
///   (`Parser/IO.hs:96`), which is what makes the order post-order.
/// * the cycle check precedes the cache probe (`Parser/IO.hs:90-91`), and that
///   order is kept here — though it turns out to be unobservable: a module enters
///   the cache only when its subtree finishes, i.e. exactly when it leaves the
///   DFS path, so the two sets are disjoint. `docs/spec/resolve.md` §8's "a cycle
///   beats a cache hit" is true of the code and vacuous in effect.
///
/// Oracles: the 3-cycle `a1.a9 → a1.a10 → a1.a11 → a1.a9`
/// (`test/syntax/imports.k:525-535`) and the self-import `a1.a12`
/// (`imports.k:537-539`), all `expected:1`.
pub fn walk_imports<F>(roots: &[ModuleSource], mut imports_of: F) -> ImportWalk
where
    F: FnMut(ModuleSource) -> Vec<(ImportId, Result<ModuleSource, ImportError>)>,
{
    struct Frame {
        module: ModuleSource,
        imports: Vec<(ImportId, Result<ModuleSource, ImportError>)>,
        next: usize,
    }

    let mut walk = ImportWalk::default();
    let mut done: FxHashSet<ModuleSource> = FxHashSet::default();

    for &root in roots {
        if done.contains(&root) {
            continue;
        }
        let mut stack = vec![Frame { module: root, imports: imports_of(root), next: 0 }];
        // The DFS path — `ancestorFiles`, including the module being parsed.
        let mut path: FxHashSet<ModuleSource> = FxHashSet::default();
        path.insert(root);

        while let Some(frame) = stack.last_mut() {
            if frame.next == frame.imports.len() {
                let finished = stack.pop().expect("checked by last_mut");
                path.remove(&finished.module);
                done.insert(finished.module);
                walk.files.push(finished.module);
                continue;
            }
            let (id, target) = frame.imports[frame.next].clone();
            frame.next += 1;
            let importer = frame.module;
            match target {
                Err(error) => {
                    walk.errors.push(ImportFailure { importer, import: id, error });
                }
                Ok(target) if path.contains(&target) => {
                    walk.errors.push(ImportFailure {
                        importer,
                        import: id,
                        error: ImportError::Cycle,
                    });
                }
                Ok(target) if done.contains(&target) => {}
                Ok(target) => {
                    path.insert(target);
                    let imports = imports_of(target);
                    stack.push(Frame { module: target, imports, next: 0 });
                }
            }
        }
    }

    walk
}

/// The exact generated source of the two synthetic modules
/// (`Parser/IO.hs:37-65`, `docs/spec/resolve.md` §5).
///
/// ```text
/// buildModule self modules consts aliases =
///     "module " ++ self ++ "{" ++ expose (self:modules) ++ "}" ++
///     imports modules ++ "\n" ++
///     defs "const auto " consts ++ defs "using " aliases
/// ```
///
/// They are **derived inputs**: generate the text, then feed it through the
/// ordinary parse / item-tree / interface chain with [`ModuleSource::Cmdargs`] or
/// [`ModuleSource::Options`], keeping one code path for real and synthetic
/// modules (`Parser/IO.hs:100-102`).
pub fn build_synthetic_module(
    self_name: &str,
    modules: &[&str],
    consts: &[(String, String)],
    aliases: &[(String, String)],
) -> String {
    let mut exposed: Vec<String> = Vec::with_capacity(1 + modules.len());
    exposed.push(format!("module {self_name}"));
    exposed.extend(modules.iter().map(|m| format!("module {m}")));

    let mut out = format!("module {self_name}{{{}}}", exposed.join(",\n"));
    out.push_str(
        &modules.iter().map(|m| format!("import {m}")).collect::<Vec<_>>().join("\n"),
    );
    out.push('\n');
    out.push_str(&defs("const auto ", consts));
    out.push_str(&defs("using ", aliases));
    out
}

fn defs(prefix: &str, items: &[(String, String)]) -> String {
    items
        .iter()
        .map(|(n, v)| format!("{prefix}{n} = {v};"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `buildCmdargsSpecialModule` (`Parser/IO.hs:49-55`).
///
/// `--define name#value` → `const auto name = value;`, `--using name#type` →
/// `using name = type;`, both already split by [`split_name_value`]. The export
/// list is `{module .cmdargs}` — a self whole-module export — so everything
/// generated is visible to importers.
pub fn cmdargs_source(defines: &[(String, String)], usings: &[(String, String)]) -> String {
    build_synthetic_module(CMDARGS_MODULE, &[], defines, usings)
}

/// `data Backend = Sv | None` (`Parser/Options.hs:16-17`). The generated text uses
/// the *derived* `Show`, so the spelling is exactly `Sv` / `None`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Backend {
    Sv,
    /// `defaultOptions` sets `backend = None` (`Parser/Options.hs:46`).
    #[default]
    None,
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Backend::Sv => "Sv",
            Backend::None => "None",
        })
    }
}

/// The five constants `.options` exports (`Parser/IO.hs:57-65`), defaulted from
/// `defaultOptions` (`Parser/Options.hs:36-51`).
///
/// `target_device` and `target_device_name` are **not** here on purpose:
/// `doc/programming-guide.md:4541-4549` lists them, `Parser/IO.hs:60-65` generates
/// neither, and the code is the spec (`docs/spec/resolve.md` §5.3).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct OptionsModule {
    pub backend: Backend,
    pub stall: i64,
    /// `-1` unless `--optimize` was given; `--pretty` never sets it
    /// (`Parser/Options.hs:44`, `Main.hs:142-151`).
    pub optimize: i64,
    pub max_threads_default: i64,
    pub max_threads_limit: i64,
}

impl Default for OptionsModule {
    fn default() -> OptionsModule {
        OptionsModule {
            backend: Backend::None,
            stall: 0,
            optimize: -1,
            max_threads_default: 512,
            max_threads_limit: 512,
        }
    }
}

impl OptionsModule {
    /// `buildOptionsSpecialModule` (`Parser/IO.hs:57-65`).
    ///
    /// `.options` imports `compiler.options` because `Backend` is declared there
    /// (`library/compiler/options.k:4-12`) — so `.options` cannot be used without
    /// an `--import-dir` pointing at a tree containing `compiler/options.k`.
    pub fn source(&self) -> String {
        let consts = vec![
            ("backend".to_owned(), format!("Backend::{}", self.backend)),
            ("stall".to_owned(), self.stall.to_string()),
            ("optimize".to_owned(), self.optimize.to_string()),
            ("max_threads_default".to_owned(), self.max_threads_default.to_string()),
            ("max_threads_limit".to_owned(), self.max_threads_limit.to_string()),
        ];
        build_synthetic_module(OPTIONS_MODULE, &["compiler.options"], &consts, &[])
    }
}

/// `nameValue` (`Parser/IO.hs:54-58`): split a `--define` / `--using` argument at
/// the **first** `#` **or** `=`, whichever comes first. The delimiter is dropped
/// and everything after it is the value, including further `#`/`=`.
///
/// With no delimiter the whole argument is the *name* and the value is empty —
/// the recursion bottoms out at `nameValue [] = ([], [])` and rebuilds the name
/// on the way back up, so `--define X` generates `const auto X = ;`.
pub fn split_name_value(arg: &str) -> (String, String) {
    match arg.find(['#', '=']) {
        Some(i) => (arg[..i].to_string(), arg[i + 1..].to_string()),
        None => (arg.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_mangle_and_unmangle() {
        // Parser/Syntax.hs:273-279.
        let name = vec![SmolStr::new("a1"), SmolStr::new("b1")];
        assert_eq!(Namespace::mangle(&name), "@a1@b1");
        assert_eq!(Namespace::unmangle("@a1@b1"), "a1.b1");
        assert_eq!(Namespace::mangle(&[SmolStr::new("mini-base")]), "@mini-base");
        assert_eq!(Namespace::mangle(&[SmolStr::new(".cmdargs")]), "@.cmdargs");
        // `unmangleModuleNamespace` and the dotted spelling agree, which is what
        // lets the export-list messages be built from the name as written.
        assert_eq!(Namespace::unmangle(&Namespace::mangle(&name)), Namespace::dotted(&name));
        assert_eq!(Namespace::unmangle("@.cmdargs"), ".cmdargs");
    }

    #[test]
    fn module_file_names_append_dot_k() {
        // docs/spec/resolve.md §1.2 table.
        let sep = std::path::MAIN_SEPARATOR_STR;
        assert_eq!(
            module_file_name(&[SmolStr::new("a1"), SmolStr::new("b1"), SmolStr::new("c1")]),
            format!("a1{sep}b1{sep}c1.k")
        );
        assert_eq!(module_file_name(&[SmolStr::new("with-dash")]), "with-dash.k");
        assert_eq!(module_file_name(&[SmolStr::new(".cmdargs")]), CMDARGS_FILE);
        assert_eq!(module_file_name(&[SmolStr::new(".options")]), OPTIONS_FILE);
    }

    #[test]
    fn name_value_splits_at_the_first_delimiter() {
        // Parser/IO.hs:54-58 — everything after the first `#`/`=` is the value.
        assert_eq!(split_name_value("X#1/0"), ("X".into(), "1/0".into()));
        assert_eq!(split_name_value("T=uint32"), ("T".into(), "uint32".into()));
        assert_eq!(split_name_value("A#b=c"), ("A".into(), "b=c".into()));
        // No delimiter: the whole argument is the name (`nameValue [] = ([],[])`
        // rebuilds it on the way back up), not a silently-dropped define.
        assert_eq!(split_name_value("nodelimiter"), ("nodelimiter".into(), String::new()));
        assert_eq!(split_name_value(""), (String::new(), String::new()));
    }

    #[test]
    fn cmdargs_source_matches_build_module() {
        // Parser/IO.hs:37-51, docs/spec/resolve.md §5.2.
        let src = build_synthetic_module(
            CMDARGS_MODULE,
            &[],
            &[("N".to_string(), "8".to_string())],
            &[("T".to_string(), "uint32".to_string())],
        );
        assert_eq!(src, "module .cmdargs{module .cmdargs}\nconst auto N = 8;using T = uint32;");
    }

    #[test]
    fn cmdargs_source_is_byte_exact() {
        // docs/spec/resolve.md §5.2 quotes the produced text for
        // `--define A#1 --define B#2 --using X#uint32` verbatim. Note the missing
        // separator between the const group and the using group (`IO.hs:42-43`).
        let defines = ["A#1", "B#2"].map(split_name_value).to_vec();
        let usings = ["X#uint32"].map(split_name_value).to_vec();
        assert_eq!(
            cmdargs_source(&defines, &usings),
            "module .cmdargs{module .cmdargs}\nconst auto A = 1;\nconst auto B = 2;using X = uint32;"
        );
        // With no defines the `using` group starts the line after the header.
        assert_eq!(
            cmdargs_source(&[], &usings),
            "module .cmdargs{module .cmdargs}\nusing X = uint32;"
        );
        // And with nothing at all the module is still a well-formed self-export.
        assert_eq!(cmdargs_source(&[], &[]), "module .cmdargs{module .cmdargs}\n");
    }

    #[test]
    fn options_source_is_byte_exact() {
        // docs/spec/resolve.md §5.3, generated by Parser/IO.hs:57-65. `expose`
        // joins with ",\n" and `imports` is appended directly after `}`.
        let opts = OptionsModule { backend: Backend::Sv, optimize: 2, ..OptionsModule::default() };
        assert_eq!(
            opts.source(),
            "module .options{module .options,\nmodule compiler.options}import compiler.options\n\
             const auto backend = Backend::Sv;\n\
             const auto stall = 0;\n\
             const auto optimize = 2;\n\
             const auto max_threads_default = 512;\n\
             const auto max_threads_limit = 512;"
        );
        // defaultOptions: Parser/Options.hs:36-51 — `None`, and `optimize = -1`.
        assert!(OptionsModule::default().source().contains("const auto backend = Backend::None;"));
        assert!(OptionsModule::default().source().contains("const auto optimize = -1;"));
    }

    // ---------------------------------------------------------------------
    // The search order (§1.3) and the synthetic short-circuits (§5.4).
    // ---------------------------------------------------------------------

    /// `device: None` means *the default*, `mock` — not "no device". The
    /// reference has no such state (`Parser/Options.hs:29,47`).
    fn settings(dirs: &[&str], device: Option<&str>) -> kg_config::Settings {
        let mut s = kg_config::Settings::fallback("/proj");
        s.import_dirs = dirs.iter().map(kg_base::Utf8PathBuf::from).collect();
        if let Some(device) = device {
            s.target_device = device.to_owned();
        }
        s
    }

    /// A file that "exists" only in the VFS overlay — no disk, no temp dirs.
    fn touch(vfs: &kg_vfs::Vfs, path: &str) -> FileId {
        let id = vfs.intern(kg_base::Utf8Path::new(path));
        vfs.set_overlay(id, String::new());
        id
    }

    fn dotted(name: &str) -> Vec<SmolStr> {
        name.split('.').map(SmolStr::new).collect()
    }

    #[test]
    fn every_device_dir_precedes_every_plain_dir() {
        // Parser/IO.hs:110 + docs/spec/resolve.md §9.1 step 4: `import
        // hardware.config` must reach <-I>/device/<dev>/hardware/config.k and not
        // <-I>/hardware/config.k. Getting this backwards silently compiles for
        // the wrong device.
        let vfs = kg_vfs::Vfs::new();
        let device = touch(&vfs, "/lib/device/mock/hardware/config.k");
        touch(&vfs, "/lib/hardware/config.k");
        touch(&vfs, "/other/hardware/config.k");

        let s = settings(&["/other", "/lib"], Some("mock"));
        assert_eq!(
            resolve_module_path(&dotted("hardware.config"), "/src".into(), &s, &vfs),
            Ok(ModuleSource::File(device))
        );
    }

    #[test]
    fn the_importing_files_own_directory_is_searched_first() {
        // `findFile (dir:importDirs)` — Parser/IO.hs:103.
        let vfs = kg_vfs::Vfs::new();
        let local = touch(&vfs, "/src/a1/b1.k");
        touch(&vfs, "/lib/a1/b1.k");
        let s = settings(&["/lib"], Some("mock"));
        assert_eq!(
            resolve_module_path(&dotted("a1.b1"), "/src".into(), &s, &vfs),
            Ok(ModuleSource::File(local))
        );
    }

    #[test]
    fn the_synthetic_modules_never_touch_the_filesystem() {
        // Parser/IO.hs:101-102: both short-circuits precede the search entirely,
        // so they resolve with no import directories and an empty VFS.
        let vfs = kg_vfs::Vfs::new();
        let s = settings(&[], None);
        assert_eq!(
            resolve_module_path(&[SmolStr::new(CMDARGS_MODULE)], "/src".into(), &s, &vfs),
            Ok(ModuleSource::Cmdargs)
        );
        assert_eq!(
            resolve_module_path(&[SmolStr::new(OPTIONS_MODULE)], "/src".into(), &s, &vfs),
            Ok(ModuleSource::Options)
        );
        assert_eq!(vfs.len(), 0, "no path may be interned for a synthetic module");
    }

    #[test]
    fn not_found_lists_the_import_dirs_but_not_the_importing_dir() {
        // Parser/IO.hs:108 — the message shows `importDirs`, which is the
        // device-expanded list and excludes `dir`.
        let vfs = kg_vfs::Vfs::new();
        let s = settings(&["/lib"], Some("mock"));
        let err = resolve_module_path(&dotted("missing"), "/src".into(), &s, &vfs).unwrap_err();
        assert_eq!(
            err,
            ImportError::NotFound {
                file: "missing.k".to_owned(),
                searched: vec!["/lib/device/mock".to_owned(), "/lib".to_owned()],
            }
        );
        assert_eq!(
            err.to_string(),
            "Can't find file: missing.k in any of the import directories\n/lib/device/mock\n/lib\n"
        );
    }

    #[test]
    fn a_file_that_does_not_decode_stops_the_search_with_the_runtimes_message() {
        // `findFile` stops at the first `binary.k` that exists and
        // `readFileUtf8` throws on it (Parser/IO.hs:103-107): the valid one
        // further down the list is never reached. The text is
        // `kg-oracle --errors` on `test/syntax/imports.k`'s `import binary`.
        let root = std::env::temp_dir().join(format!("kg-resolve-binary-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(root.join("src/binary.k"), b"\xc0\xde\xde\xad").unwrap();
        std::fs::write(root.join("lib/binary.k"), "module binary {}\n").unwrap();
        let root = kg_base::Utf8PathBuf::from_path_buf(root).unwrap();

        let vfs = kg_vfs::Vfs::new();
        let lib = root.join("lib");
        let s = settings(&[lib.as_str()], None);
        let err = resolve_module_path(&dotted("binary"), &root.join("src"), &s, &vfs).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "{root}/src/binary.k: hGetContents: invalid argument \
                 (cannot decode byte sequence starting from 192)"
            )
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_absolute_base_library_resolves_on_the_first_probe() {
        // filepath's `</>` returns an absolute second argument unchanged, which is
        // how `--base-library=${CMAKE_SOURCE_DIR}/library/mini-base.k` works with
        // no matching `-I` (test/syntax/CMakeLists.txt:43).
        let vfs = kg_vfs::Vfs::new();
        let base = touch(&vfs, "/abs/library/mini-base.k");
        let s = settings(&[], None);
        assert_eq!(
            resolve_import_file("/abs/library/mini-base.k", "/src".into(), &s, &vfs),
            Ok(ModuleSource::File(base))
        );
    }

    #[test]
    fn the_implicit_base_falls_back_to_cmdargs() {
        // Parser/IO.hs:72-77 + docs/spec/resolve.md §6 rule 1.
        let mut s = settings(&["/lib"], None);
        s.base_library = Some(kg_base::Utf8PathBuf::from("/lib/base.k"));
        assert_eq!(implicit_base_file(&s), "/lib/base.k");

        // `--no-implicit-base` does NOT remove `.cmdargs`.
        s.implicit_base = false;
        assert_eq!(implicit_base_file(&s), CMDARGS_FILE);

        // No `--import-dir` at all has the same effect.
        let mut s = settings(&[], None);
        s.base_library = Some(kg_base::Utf8PathBuf::from("base.k"));
        assert_eq!(implicit_base_file(&s), CMDARGS_FILE);

        // Default `--base-library` is "base.k" (Parser/Options.hs:38).
        let s = settings(&["/lib"], None);
        assert_eq!(implicit_base_file(&s), DEFAULT_BASE_LIBRARY);
    }

    #[test]
    fn a_synthetic_module_keeps_the_importers_directory() {
        // Parser/IO.hs:101-102 return `dir` unchanged, so `.options`' own
        // `import compiler.options` searches from whoever imported it
        // (docs/spec/resolve.md §5.4).
        let vfs = kg_vfs::Vfs::new();
        let file = touch(&vfs, "/lib/a1/a1.k");
        assert_eq!(module_dir(ModuleSource::Options, "/src".into(), &vfs), "/src");
        assert_eq!(module_dir(ModuleSource::File(file), "/src".into(), &vfs), "/lib/a1");
    }

    // ---------------------------------------------------------------------
    // A fixture that transcribes `test/syntax/imports/**` module by module.
    // ---------------------------------------------------------------------

    /// One module, written to read like the corpus file it encodes.
    ///
    /// * `exports` is the text between the braces of the `module` line;
    /// * `imports` are `"a1.a1"` or `"a1.a5 as A5"`;
    /// * `symbols` are dot-separated key *tails* under the module namespace —
    ///   `"N.Aa"` is the key `["@a1@a5","N","Aa"]` — with a leading `!` marking a
    ///   declaration that fails `exposable` (a non-`const` `Variable`).
    #[derive(Default)]
    struct Desc<'a> {
        exports: &'a str,
        imports: &'a [&'a str],
        symbols: &'a [&'a str],
        /// `moduleNeeded`: an `export T;` or a `[[reset]]` function
        /// (`test/syntax/imports/export.k`, `reset.k`).
        exported: bool,
    }

    struct Fixture {
        interner: Interner,
        graph: ModuleGraph,
        sources: FxHashMap<String, ModuleSource>,
        next_file: u32,
    }

    impl Fixture {
        fn new() -> Fixture {
            Fixture {
                interner: Interner::new(),
                graph: ModuleGraph::new(),
                sources: FxHashMap::default(),
                next_file: 0,
            }
        }

        fn ns(&self, dotted: &str) -> Namespace {
            Namespace::intern(
                &dotted.split('.').map(SmolStr::new).collect::<Vec<_>>(),
                &self.interner,
            )
        }

        fn source(&self, dotted: &str) -> ModuleSource {
            self.sources[dotted]
        }

        /// The key a name written inside module `m` is filed under:
        /// `scopeQualifier ++ [name]` with the module body's qualifier
        /// `["@a@b"]` (`Parser.hs:189`, `SymbolTable.hs:211`).
        fn key(&self, module: &str, tail: &str) -> SymbolPath {
            let mut segs = vec![self.ns(module).0];
            segs.extend(tail.split('.').map(|s| self.interner.intern(s)));
            SymbolPath::new(segs)
        }

        fn parse_exports(&self, text: &str) -> Vec<ExportItem> {
            text.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|item| match item.strip_prefix("module ") {
                    None => ExportItem::Symbol(self.interner.intern(item)),
                    Some(rest) => match rest.split_once('\\') {
                        None => ExportItem::Module(dotted(rest.trim())),
                        Some((a, b)) => {
                            ExportItem::Difference(dotted(a.trim()), dotted(b.trim()))
                        }
                    },
                })
                .collect()
        }

        fn add(&mut self, name: &str, desc: Desc<'_>) -> ModuleSource {
            let source = ModuleSource::File(FileId(self.next_file));
            self.next_file += 1;
            self.sources.insert(name.to_owned(), source);

            let header = ModuleHeader {
                name: dotted(name),
                namespace: Some(self.ns(name)),
                exports: self.parse_exports(desc.exports),
                exported_at: Vec::new(),
            };
            let symbols = desc
                .symbols
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let (exposable, tail) = match s.strip_prefix('!') {
                        Some(rest) => (false, rest),
                        None => (true, *s),
                    };
                    (self.key(name, tail), ModuleSymbol { item: ItemId(i as u32), exposable })
                })
                .collect();
            self.graph.add_module(
                source,
                ModuleContent { header, symbols, has_export_or_reset: desc.exported },
            );

            for (i, spec) in desc.imports.iter().enumerate() {
                let (target_name, alias) = match spec.split_once(" as ") {
                    Some((n, a)) => (n.trim(), Some(self.interner.intern(a.trim()))),
                    None => (spec.trim(), None),
                };
                let target = self.source(target_name);
                let decl = ImportDecl {
                    id: ImportId(i as u32),
                    name: dotted(target_name),
                    alias,
                    range: source.start(),
                    needed: ImportNeeded::Redundant,
                };
                self.graph.add_import(source, decl, target, &self.interner);
            }

            self.graph.build_interface(source, &self.interner);
            source
        }

        /// What an importer of `module` sees for the unqualified name `name`,
        /// as a set of the fixture's dotted key spellings.
        fn visible_names(&self, module: &str) -> FxHashSet<String> {
            let iface = self.graph.interface(self.source(module)).unwrap();
            iface
                .visible
                .iter()
                .flat_map(|(ns, syms)| {
                    syms.keys().map(move |k| {
                        let mut out = Namespace::unmangle(&self.interner.resolve(ns.0));
                        for seg in k.segments().iter().skip(1) {
                            out.push(':');
                            out.push_str(&self.interner.resolve(*seg));
                        }
                        out
                    })
                })
                .collect()
        }

        /// `import <module>;` then use `name` unqualified, as every snippet in
        /// `test/syntax/imports.k` does.
        fn resolve_from(&self, importer: &str, name: &str) -> Vec<DefId> {
            let path = SymbolPath::single(self.interner.intern(name));
            self.graph.find_in_imports(self.source(importer), &path)
        }
    }

    /// A file with no `module` declaration — every `test/syntax/imports.k`
    /// snippet — importing the named modules.
    fn importer(fx: &mut Fixture, imports: &[&str]) -> ModuleSource {
        let source = ModuleSource::File(FileId(1000 + fx.next_file));
        fx.next_file += 1;
        fx.sources.insert("$snippet".to_owned(), source);
        fx.graph.add_module(source, ModuleContent::default());
        for (i, spec) in imports.iter().enumerate() {
            let (target_name, alias) = match spec.split_once(" as ") {
                Some((n, a)) => (n.trim(), Some(fx.interner.intern(a.trim()))),
                None => (spec.trim(), None),
            };
            let target = fx.source(target_name);
            let decl = ImportDecl {
                id: ImportId(i as u32),
                name: dotted(target_name),
                alias,
                range: source.start(),
                needed: ImportNeeded::Redundant,
            };
            fx.graph.add_import(source, decl, target, &fx.interner);
        }
        source
    }

    /// `test/syntax/imports/a1/{a1,b1/b1}.k` — the root of most of the corpus.
    fn a1_a1(fx: &mut Fixture) {
        fx.add("a1.b1.b1", Desc { exports: "b1", symbols: &["b1"], ..Desc::default() });
        fx.add(
            "a1.a1",
            Desc {
                exports: "a1, Aa",
                imports: &["a1.b1.b1"],
                symbols: &["a1", "Aa"],
                ..Desc::default()
            },
        );
    }

    #[test]
    fn a_named_export_re_exports_through_an_import_transitively() {
        // test/syntax/imports/a1/{a1,a3,a4}.k; imports.k:274-289, expected:0.
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        fx.add("a1.a3", Desc { exports: "a1", imports: &["a1.a1"], ..Desc::default() });
        fx.add("a1.a4", Desc { exports: "a1", imports: &["a1.a3"], ..Desc::default() });

        // `a1` stays keyed under its *defining* module's namespace all the way
        // through the chain (SymbolTable.hs:148 unions, it never rewrites).
        assert!(fx.visible_names("a1.a4").contains("a1.a1:a1"));
        // ... and `Aa`, which a1.a3 does not name, does not come along.
        assert!(!fx.visible_names("a1.a4").contains("a1.a1:Aa"));

        importer(&mut fx, &["a1.a4"]);
        assert_eq!(fx.resolve_from("$snippet", "a1").len(), 1);
        assert_eq!(fx.resolve_from("$snippet", "Aa").len(), 0);
    }

    #[test]
    fn a_whole_module_re_export_is_transitive_but_a_named_one_narrows_it() {
        // a1.a16 {module a1.a1} <- a1.a17 { Aa } <- a1.a18 {module a1.a17}.
        // imports.k:608-662: through a1.a18, `Aa` resolves (expected:0) and `a1`
        // does not (expected:1).
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        fx.add("a1.a16", Desc { exports: "module a1.a1", imports: &["a1.a1"], ..Desc::default() });
        fx.add("a1.a17", Desc { exports: "Aa", imports: &["a1.a16"], ..Desc::default() });
        fx.add("a1.a18", Desc { exports: "module a1.a17", imports: &["a1.a17"], ..Desc::default() });

        importer(&mut fx, &["a1.a16"]);
        assert_eq!(fx.resolve_from("$snippet", "a1").len(), 1, "imports.k:608-616");
        let mut fx2 = Fixture::new();
        a1_a1(&mut fx2);
        fx2.add("a1.a16", Desc { exports: "module a1.a1", imports: &["a1.a1"], ..Desc::default() });
        fx2.add("a1.a17", Desc { exports: "Aa", imports: &["a1.a16"], ..Desc::default() });
        fx2.add(
            "a1.a18",
            Desc { exports: "module a1.a17", imports: &["a1.a17"], ..Desc::default() },
        );
        importer(&mut fx2, &["a1.a18"]);
        assert_eq!(fx2.resolve_from("$snippet", "Aa").len(), 1, "imports.k:646-653 expected:0");
        assert_eq!(fx2.resolve_from("$snippet", "a1").len(), 0, "imports.k:655-662 expected:1");
    }

    #[test]
    fn an_aliased_import_is_not_re_exported_and_hides_its_symbols() {
        // test/syntax/imports/a1/{a5,a6}.k; imports.k:334-353.
        let mut fx = Fixture::new();
        fx.add("a1.a5", Desc { exports: "a1, N", symbols: &["a1", "N", "N.Aa", "N.Go"], ..Desc::default() });
        fx.add(
            "a1.a6",
            Desc { exports: "a1", imports: &["a1.a5 as A5"], symbols: &["a1"], ..Desc::default() },
        );

        // `isJust importedAs ⇒ mempty` (SymbolTable.hs:160): only a1.a6's own
        // `a1` is visible, never a1.a5's.
        assert_eq!(
            fx.visible_names("a1.a6"),
            ["a1.a6:a1"].into_iter().map(String::from).collect::<FxHashSet<_>>()
        );

        // Directly importing a1.a5 `as A5`: the bare name misses
        // (imports.k:334-342, expected:1) and the qualified one hits
        // (imports.k:344-353, expected:0). SymbolTable.hs:276-278.
        importer(&mut fx, &["a1.a5 as A5"]);
        assert_eq!(fx.resolve_from("$snippet", "N").len(), 0);
        let snippet = fx.source("$snippet");
        let qualified = SymbolPath::new([fx.interner.intern("A5"), fx.interner.intern("N")]);
        assert_eq!(fx.graph.find_in_imports(snippet, &qualified).len(), 1);
        // And a member of that class, reached as `A5::N::Go` — the whole key is
        // present because `filterExposed` keys on the *second* component
        // (imports.k:828-841, expected:0).
        let member = SymbolPath::new([
            fx.interner.intern("A5"),
            fx.interner.intern("N"),
            fx.interner.intern("Go"),
        ]);
        assert_eq!(fx.graph.find_in_imports(snippet, &member).len(), 1);
    }

    #[test]
    fn two_imports_of_two_different_declarations_produce_two_hits() {
        // test/syntax/imports/a1/{b1,b2}/c1/c1.k both declare `c1`, and both
        // expose it. imports.k:140-143 imports both and never uses `c1`
        // (expected:0); :154-162 uses it and is expected:1 — the ambiguity is
        // raised by `findSymbol`'s `nub`, which needs *both* hits to survive
        // `findInImports`' running set difference (SymbolTable.hs:231,265).
        let mut fx = Fixture::new();
        fx.add("a1.b1.c1.c1", Desc { exports: "c1", symbols: &["c1"], ..Desc::default() });
        fx.add("a1.b2.c1.c1", Desc { exports: "c1", symbols: &["c1"], ..Desc::default() });
        let m = importer(&mut fx, &["a1.b1.c1.c1", "a1.b2.c1.c1"]);

        let hits = fx.graph.import_hits(m, &SymbolPath::single(fx.interner.intern("c1")));
        assert_eq!(hits.defs.len(), 2, "two distinct declarations, not deduplicated");
        assert_eq!(hits.used.len(), 2, "and both imports are justified");
        // `b1`, declared in neither, resolves nowhere.
        assert!(fx.resolve_from("$snippet", "b1").is_empty());
    }

    #[test]
    fn one_import_can_yield_two_hits() {
        // `findInImport` is `mapMaybe … $ Map.toList $ visibleSymbols`
        // (SymbolTable.hs:273) — it scans *every* namespace the imported module
        // re-exposes, so a single import can produce two hits for one name. That
        // is why `docs/spec/resolve.md` §11.2's `Option<DefId>` return type cannot
        // be right, and it is the whole reason `test/syntax/imports/diff/foo_bar.k`
        // is written with a difference: without it, `x` would arrive twice.
        //
        // Derived from the Haskell — the corpus contains no module that re-exposes
        // two whole modules sharing a name, precisely because it would be an error.
        let mut fx = Fixture::new();
        diff_modules(&mut fx);
        fx.add(
            "diff.both",
            Desc {
                exports: "module diff.foo, module diff.bar",
                imports: &["diff.foo", "diff.bar"],
                ..Desc::default()
            },
        );
        importer(&mut fx, &["diff.both"]);
        assert_eq!(fx.resolve_from("$snippet", "x").len(), 2, "diff.foo::x and diff.bar::x");
        assert_eq!(fx.resolve_from("$snippet", "foo").len(), 1, "only diff.foo declares `foo`");

        // With the difference in place — diff/foo_bar.k — `x` arrives once, from
        // the right-hand side (imports.k:876-886 asserts x == 11, diff.bar's).
        let mut fx = Fixture::new();
        diff_modules(&mut fx);
        fx.add(
            "diff.foo_bar",
            Desc {
                exports: "module diff.foo \\ diff.bar, module diff.bar",
                imports: &["diff.foo", "diff.bar"],
                ..Desc::default()
            },
        );
        importer(&mut fx, &["diff.foo_bar"]);
        let hits = fx.resolve_from("$snippet", "x");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].module, fx.source("diff.bar"), "x == 11");
    }

    #[test]
    fn an_explicitly_re_exposed_module_is_re_exported_even_when_aliased() {
        // The guard order of `filterImported` (SymbolTable.hs:158-161):
        // `namespaceImported ∈ exposedModules` is tested *before*
        // `isJust importedAs`, so an alias does not suppress an explicit
        // whole-module re-export.
        //
        // Derived from the Haskell: `test/syntax/imports/a1/a19.k` imports a1.a16
        // both plain and `as A`, so its plain import alone satisfies
        // imports.k:618-626 whichever order the guards are in. Only a module whose
        // *sole* route is aliased can tell the two apart.
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        fx.add(
            "aliased",
            Desc { exports: "module a1.a1", imports: &["a1.a1 as A"], ..Desc::default() },
        );
        assert!(fx.visible_names("aliased").contains("a1.a1:a1"));
        importer(&mut fx, &["aliased"]);
        assert_eq!(fx.resolve_from("$snippet", "a1").len(), 1);
    }

    #[test]
    fn an_explicitly_re_exposed_module_beats_the_alias_rule() {
        // test/syntax/imports/a1/a19.k imports a1.a16 twice, plain and `as A`,
        // and exposes `module a1.a16`; imports.k:618-626 is expected:0.
        // `namespaceImported ∈ exposedModules` is tested *before* `isJust
        // importedAs` (SymbolTable.hs:158-161), so both entries contribute.
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        fx.add("a1.a16", Desc { exports: "module a1.a1", imports: &["a1.a1"], ..Desc::default() });
        fx.add(
            "a1.a19",
            Desc {
                exports: "module a1.a16",
                imports: &["a1.a16", "a1.a16 as A"],
                ..Desc::default()
            },
        );
        assert!(fx.visible_names("a1.a19").contains("a1.a1:a1"));

        importer(&mut fx, &["a1.a19"]);
        assert_eq!(fx.resolve_from("$snippet", "a1").len(), 1);
    }

    #[test]
    fn a_whole_module_export_skips_non_const_variables() {
        // `exposable` (SymbolTable.hs:168-169) filters a non-`const` `Variable`
        // out of a whole-module export. Derived from the Haskell, not from a
        // corpus assertion: `test/syntax/imports/a1/a26.k`'s function-local `a` is
        // filtered here, but the oracle at imports.k:853-857 would reject it for
        // its key shape alone.
        let mut fx = Fixture::new();
        fx.add(
            "a1.a26",
            Desc {
                exports: "module a1.a26",
                symbols: &["Foo", "Foo.Bar", "test", "!test.$.a"],
                ..Desc::default()
            },
        );
        let names = fx.visible_names("a1.a26");
        assert!(names.contains("a1.a26:Foo"));
        assert!(names.contains("a1.a26:Foo:Bar"));
        assert!(!names.contains("a1.a26:test:$:a"));
    }

    // ---------------------------------------------------------------------
    // Module difference — test/syntax/imports/diff/**, imports.k:861-910.
    // ---------------------------------------------------------------------

    fn diff_modules(fx: &mut Fixture) {
        fx.add(
            "diff.foo",
            Desc {
                exports: "module diff.foo",
                symbols: &["x", "y", "t", "foo_t", "foo"],
                ..Desc::default()
            },
        );
        fx.add(
            "diff.bar",
            Desc {
                exports: "module diff.bar",
                symbols: &["x", "y", "t", "bar_t", "bar"],
                ..Desc::default()
            },
        );
    }

    #[test]
    fn a_difference_removes_every_name_the_right_hand_side_defines() {
        // diff/foo_minus_bar.k {module diff.foo \ diff.bar}: imports.k:893-901
        // asserts `foo` and `foo_t` work (warning:0) and :902-906 asserts `x` does
        // not (expected:1) — `x`, `y`, `t` are exactly the names diff.bar also
        // defines.
        let mut fx = Fixture::new();
        diff_modules(&mut fx);
        fx.add(
            "diff.foo_minus_bar",
            Desc {
                exports: "module diff.foo \\ diff.bar",
                imports: &["diff.foo", "diff.bar"],
                ..Desc::default()
            },
        );

        importer(&mut fx, &["diff.foo_minus_bar"]);
        for name in ["foo", "foo_t"] {
            assert_eq!(fx.resolve_from("$snippet", name).len(), 1, "{name} must survive");
        }
        for name in ["x", "y", "t", "bar", "bar_t"] {
            assert_eq!(fx.resolve_from("$snippet", name).len(), 0, "{name} must be removed");
        }
    }

    #[test]
    fn a_difference_against_self_makes_the_module_the_default_or_the_override() {
        // imports.k:861-886, both warning:0.
        //   diff.foo_self { module diff.foo \ diff.foo_self, module diff.foo_self }
        //     ⇒ foo == false  (the module's own `foo` wins)
        //   diff.self_foo { module diff.self_foo \ diff.foo, module diff.foo }
        //     ⇒ foo == 3      (the import's `foo` wins)
        // This is the defaults-plus-override mechanism `library/compiler/device/
        // config.k:7-11` relies on.
        let mut fx = Fixture::new();
        diff_modules(&mut fx);
        fx.add(
            "diff.foo_self",
            Desc {
                exports: "module diff.foo \\ diff.foo_self, module diff.foo_self",
                imports: &["diff.foo"],
                symbols: &["foo_t", "foo", "self"],
                ..Desc::default()
            },
        );
        fx.add(
            "diff.self_foo",
            Desc {
                exports: "module diff.self_foo \\ diff.foo, module diff.foo",
                imports: &["diff.foo"],
                symbols: &["foo_t", "foo", "self"],
                ..Desc::default()
            },
        );

        let self_wins = fx.visible_names("diff.foo_self");
        assert!(self_wins.contains("diff.foo_self:foo"), "own `foo` is exposed");
        assert!(!self_wins.contains("diff.foo:foo"), "diff.foo's `foo` is differenced away");
        assert!(self_wins.contains("diff.foo:x"), "and `x`, which self does not define, is not");
        assert!(self_wins.contains("diff.foo:t"), "bitsizeof(t) == 32 comes from diff.foo");

        let import_wins = fx.visible_names("diff.self_foo");
        assert!(import_wins.contains("diff.foo:foo"), "diff.foo's `foo` is exposed");
        assert!(!import_wins.contains("diff.self_foo:foo"), "own `foo` is differenced away");
        assert!(import_wins.contains("diff.self_foo:self"), "`self` survives either way");

        // Exactly one `foo` in both cases — two would be an ambiguity error.
        for module in ["diff.foo_self", "diff.self_foo"] {
            let mut fx2 = Fixture::new();
            diff_modules(&mut fx2);
            fx2.add(
                "diff.foo_self",
                Desc {
                    exports: "module diff.foo \\ diff.foo_self, module diff.foo_self",
                    imports: &["diff.foo"],
                    symbols: &["foo_t", "foo", "self"],
                    ..Desc::default()
                },
            );
            fx2.add(
                "diff.self_foo",
                Desc {
                    exports: "module diff.self_foo \\ diff.foo, module diff.foo",
                    imports: &["diff.foo"],
                    symbols: &["foo_t", "foo", "self"],
                    ..Desc::default()
                },
            );
            importer(&mut fx2, &[module]);
            assert_eq!(fx2.resolve_from("$snippet", "foo").len(), 1, "{module}");
            assert_eq!(fx2.resolve_from("$snippet", "self").len(), 1, "{module}");
        }
    }

    #[test]
    fn a_module_named_in_a_difference_is_never_a_redundant_import() {
        // imports.k:893-901 is warning:0 even though nothing resolves through
        // `import diff.bar` — `updateImports`' second rule (SymbolTable.hs:105).
        let mut fx = Fixture::new();
        diff_modules(&mut fx);
        let m = fx.add(
            "diff.foo_minus_bar",
            Desc {
                exports: "module diff.foo \\ diff.bar",
                imports: &["diff.foo", "diff.bar"],
                ..Desc::default()
            },
        );
        assert_eq!(fx.graph.redundant_imports(m).count(), 0);
    }

    #[test]
    fn a_difference_with_no_import_for_its_right_hand_side_is_an_error() {
        // test/syntax/imports/diff/self_missing.k + imports.k:908-910, expected:1.
        // `checkDiffs` (Parser.hs:230-231).
        let mut fx = Fixture::new();
        diff_modules(&mut fx);
        let m = fx.add(
            "diff.self_missing",
            Desc { exports: "module diff.self_missing \\ diff.bar", ..Desc::default() },
        );
        let errors = fx.graph.check_exposed_modules(m, &fx.interner);
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "Missing import for module used in re-exposed module difference diff.bar"
        );
        // And it must not panic: the reference's `fromJust` survives this input
        // only by laziness (SymbolTable.hs:156).
        assert!(fx.visible_names("diff.self_missing").is_empty());
    }

    #[test]
    fn re_exposing_a_module_that_is_not_imported_is_an_error() {
        // test/syntax/imports/a1/a20.k exposes `module a1.a1` but imports only
        // a1.a19; imports.k:664-666, expected:1. `checkModules` (Parser.hs:224-227).
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        fx.add("a1.a16", Desc { exports: "module a1.a1", imports: &["a1.a1"], ..Desc::default() });
        fx.add(
            "a1.a19",
            Desc {
                exports: "module a1.a16",
                imports: &["a1.a16", "a1.a16 as A"],
                ..Desc::default()
            },
        );
        let m = fx.add(
            "a1.a20",
            Desc { exports: "module a1.a1", imports: &["a1.a19"], ..Desc::default() },
        );
        let errors = fx.graph.check_exposed_modules(m, &fx.interner);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "Missing import for re-exposed module a1.a1");

        // a1.a19 itself re-exposes a module it *does* import — no error.
        assert!(
            fx.graph.check_exposed_modules(fx.source("a1.a19"), &fx.interner).is_empty()
        );
    }

    // ---------------------------------------------------------------------
    // The `redundant-import` warning — imports.k:426-523.
    // ---------------------------------------------------------------------

    #[test]
    fn an_unused_import_is_redundant_and_a_used_one_is_not() {
        // imports.k:426-428 (warn) vs :430-437 (no warn).
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        let m = importer(&mut fx, &["a1.a1"]);
        assert_eq!(fx.graph.redundant_imports(m).count(), 1);

        let used = fx.graph.resolve_in_imports(m, &SymbolPath::single(fx.interner.intern("a1")));
        assert_eq!(used.len(), 1);
        assert_eq!(fx.graph.redundant_imports(m).count(), 0);
    }

    #[test]
    fn the_last_import_in_the_file_wins_the_attribution() {
        // The order-dependence pair: imports.k:503-512 warns, :514-523 does not,
        // and the only difference is which `import` line comes first
        // (SymbolTable.hs:263-270 + the reversed `imports` list).
        //
        // a1.a8 re-exposes a1.a1's `a1` by name and adds its own `a8`.
        let build = |fx: &mut Fixture, order: &[&str]| -> ModuleSource {
            a1_a1(fx);
            fx.add(
                "a1.a8",
                Desc {
                    exports: "a1, a8",
                    imports: &["a1.a1"],
                    symbols: &["a8"],
                    ..Desc::default()
                },
            );
            let m = importer(fx, order);
            for name in ["a1", "a8"] {
                fx.graph.resolve_in_imports(m, &SymbolPath::single(fx.interner.intern(name)));
            }
            m
        };

        let mut fx = Fixture::new();
        let m = build(&mut fx, &["a1.a1", "a1.a8"]);
        let redundant: Vec<_> =
            fx.graph.redundant_imports(m).map(|i| i.name.join(".")).collect();
        assert_eq!(redundant, ["a1.a1"], "the earlier import loses `a1` to the later one");

        let mut fx = Fixture::new();
        let m = build(&mut fx, &["a1.a8", "a1.a1"]);
        assert_eq!(fx.graph.redundant_imports(m).count(), 0, "imports.k:514-523, warning:");
    }

    #[test]
    fn two_imports_of_the_same_declaration_leave_the_earlier_one_redundant() {
        // test/syntax/imports/a1/a7.k imports a1.a3 and a1.a4, which both
        // re-expose the *same* `a1`; imports.k:466-473 is warning:redundant-import
        // and the use is not ambiguous, because `nub` collapses identical
        // declarations (SymbolTable.hs:231).
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        fx.add("a1.a3", Desc { exports: "a1", imports: &["a1.a1"], ..Desc::default() });
        fx.add("a1.a4", Desc { exports: "a1", imports: &["a1.a3"], ..Desc::default() });
        let m = importer(&mut fx, &["a1.a3", "a1.a4"]);

        let hits = fx.graph.import_hits(m, &SymbolPath::single(fx.interner.intern("a1")));
        assert_eq!(hits.defs.len(), 1, "the same declaration through two chains is one hit");
        assert_eq!(hits.used.len(), 1);
        fx.graph.mark_needed(m, &hits.used);
        let redundant: Vec<_> =
            fx.graph.redundant_imports(m).map(|i| i.name.join(".")).collect();
        assert_eq!(redundant, ["a1.a3"]);
    }

    #[test]
    fn an_import_of_a_module_with_an_export_or_reset_is_never_redundant() {
        // test/syntax/imports/{export,reset}.k, both warning:0 at
        // imports.k:912-918. `moduleNeeded` (SymbolTable.hs:149,175-181).
        let mut fx = Fixture::new();
        fx.add("export", Desc { symbols: &["Foo"], exported: true, ..Desc::default() });
        fx.add("reset", Desc { symbols: &["Foo"], ..Desc::default() });

        assert!(fx.graph.interface(fx.source("export")).unwrap().needed);
        assert!(!fx.graph.interface(fx.source("reset")).unwrap().needed);

        let m = importer(&mut fx, &["export", "reset"]);
        let redundant: Vec<_> =
            fx.graph.redundant_imports(m).map(|i| i.name.join(".")).collect();
        assert_eq!(redundant, ["reset"], "only the module without an `export T;` warns");
    }

    #[test]
    fn re_exposing_an_import_as_a_whole_module_makes_it_needed() {
        // `updateImports`' first rule (SymbolTable.hs:104): a1.a16 exposes
        // `module a1.a1`, so its own `import a1.a1` never warns even though a1.a16
        // uses nothing from it.
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        let m =
            fx.add("a1.a16", Desc { exports: "module a1.a1", imports: &["a1.a1"], ..Desc::default() });
        assert_eq!(fx.graph.redundant_imports(m).count(), 0);
    }

    #[test]
    fn the_implicit_base_import_seed_survives() {
        // `addImport Needed 0 Nothing` (Parser/IO.hs:77) — the base import is
        // marked Needed from outside the grammar, which is why `base` and
        // `.cmdargs` never warn.
        let mut fx = Fixture::new();
        fx.add("base", Desc::default());
        let m = ModuleSource::File(FileId(900));
        fx.graph.add_module(m, ModuleContent::default());
        fx.graph.add_import(
            m,
            ImportDecl {
                id: ImportId(0),
                name: dotted("base"),
                alias: None,
                range: m.start(),
                needed: ImportNeeded::Needed,
            },
            fx.source("base"),
            &fx.interner,
        );
        assert_eq!(fx.graph.redundant_imports(m).count(), 0);
    }

    // ---------------------------------------------------------------------
    // `findInImport` name matching — SymbolTable.hs:272-278.
    // ---------------------------------------------------------------------

    #[test]
    fn a_name_already_carrying_the_namespace_matches_directly() {
        // `withNamespace (n:ns) | n == namespace` — the shape `makeQualified`
        // leaves behind (Desugar.hs:190-200), e.g. `@a1@a1::a1`.
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        let m = importer(&mut fx, &["a1.a1"]);
        let qualified = SymbolPath::new([fx.ns("a1.a1").0, fx.interner.intern("a1")]);
        assert_eq!(fx.graph.find_in_imports(m, &qualified).len(), 1);
    }

    #[test]
    fn a_file_scope_declaration_collides_with_an_import_but_a_nested_one_does_not() {
        // `findInScopes` re-queries the imports with the found declaration's
        // *qualified* name (`SymbolTable.hs:257`), and for a declaration
        // `getQualifier e = declScopeName e` (`Parser/Syntax.hs:764`), so the key
        // is the full scoped one. That is the whole difference between:
        //
        //   import a1.b2.c1.c1 + file-scope `struct c1`        expected:1  (:75-86)
        //   import a1.b2.c1.c1 + `class Foo { struct c1 … }`   expected:0  (:128-138)
        //
        // A snippet has no `module` declaration, so its file-scope qualifier is
        // empty and the two keys are `["c1"]` and `["Foo","c1"]`.
        let mut fx = Fixture::new();
        fx.add("a1.b2.c1.c1", Desc { exports: "c1", symbols: &["c1"], ..Desc::default() });
        let m = importer(&mut fx, &["a1.b2.c1.c1"]);

        let bare = SymbolPath::single(fx.interner.intern("c1"));
        assert_eq!(
            fx.graph.find_in_imports(m, &bare).len(),
            1,
            "the import also offers `c1`; with the local one that is two, i.e. ambiguous"
        );
        let nested = SymbolPath::new([fx.interner.intern("Foo"), fx.interner.intern("c1")]);
        assert!(
            fx.graph.find_in_imports(m, &nested).is_empty(),
            "`Foo::c1` is prefixed with the namespace and misses"
        );
    }

    #[test]
    fn a_name_from_the_wrong_module_does_not_match() {
        // imports.k:30-37: `import a1.a1` alone does not make `b1` visible
        // (expected:1) — a1.a1 exposes only `a1` and `Aa`.
        let mut fx = Fixture::new();
        a1_a1(&mut fx);
        importer(&mut fx, &["a1.a1"]);
        assert_eq!(fx.resolve_from("$snippet", "b1").len(), 0);
    }

    // ---------------------------------------------------------------------
    // Cycles and the file order — §8.
    // ---------------------------------------------------------------------

    fn adjacency<'a>(
        edges: &'a [(ModuleSource, &'a [ModuleSource])],
    ) -> impl Fn(ModuleSource) -> Vec<(ImportId, Result<ModuleSource, ImportError>)> + 'a {
        move |m| {
            edges
                .iter()
                .find(|(from, _)| *from == m)
                .map(|(_, to)| {
                    to.iter()
                        .enumerate()
                        .map(|(i, t)| (ImportId(i as u32), Ok(*t)))
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    #[test]
    fn a_three_module_cycle_is_reported_at_the_offending_import() {
        // test/syntax/imports/a1/{a9,a10,a11}.k — a9 → a10 → a11 → a9;
        // imports.k:525-535, expected:1.
        let (top, a9, a10, a11) = (
            ModuleSource::File(FileId(0)),
            ModuleSource::File(FileId(9)),
            ModuleSource::File(FileId(10)),
            ModuleSource::File(FileId(11)),
        );
        let edges: &[(ModuleSource, &[ModuleSource])] =
            &[(top, &[a9]), (a9, &[a10]), (a10, &[a11]), (a11, &[a9])];
        let walk = walk_imports(&[top], adjacency(edges));

        assert_eq!(walk.errors.len(), 1);
        assert_eq!(walk.errors[0].importer, a11);
        assert_eq!(walk.errors[0].error, ImportError::Cycle);
        assert_eq!(walk.errors[0].error.to_string(), "Circular dependency");
        // Post-order: dependencies finish first (Parser/IO.hs:96).
        assert_eq!(walk.files, vec![a11, a10, a9, top]);
    }

    #[test]
    fn a_self_import_is_a_cycle() {
        // test/syntax/imports/a1/a12.k:6 imports itself; imports.k:537-539,
        // expected:1. The ancestor list already contains the module being parsed.
        let (top, a12) = (ModuleSource::File(FileId(0)), ModuleSource::File(FileId(12)));
        let edges: &[(ModuleSource, &[ModuleSource])] = &[(top, &[a12]), (a12, &[a12])];
        let walk = walk_imports(&[top], adjacency(edges));
        assert_eq!(walk.errors, vec![ImportFailure {
            importer: a12,
            import: ImportId(0),
            error: ImportError::Cycle
        }]);
        assert_eq!(walk.files, vec![a12, top]);
    }

    #[test]
    fn a_diamond_import_visits_each_module_once() {
        // The cache is keyed by canonical path and is probed after the cycle
        // check (Parser/IO.hs:91). A diamond parses `d` once, and the resulting
        // order is post-order DFS.
        let [a, b, c, d] = [0u32, 1, 2, 3].map(|i| ModuleSource::File(FileId(i)));
        let edges: &[(ModuleSource, &[ModuleSource])] =
            &[(a, &[b, c]), (b, &[d]), (c, &[d]), (d, &[])];
        let walk = walk_imports(&[a], adjacency(edges));
        assert!(walk.errors.is_empty());
        assert_eq!(walk.files, vec![d, b, c, a]);
    }

    #[test]
    fn the_ancestor_path_resets_per_root_but_the_cache_does_not() {
        // `parseFile' []` for every entry point (Parser/IO.hs:82) while the
        // `Cache` state thread persists across `forM (files opt)`
        // (Parser/IO.hs:70).
        let [base, f1, f2] = [0u32, 1, 2].map(|i| ModuleSource::File(FileId(i)));
        let edges: &[(ModuleSource, &[ModuleSource])] =
            &[(base, &[]), (f1, &[base]), (f2, &[base])];
        let walk = walk_imports(&[base, f1, f2], adjacency(edges));
        assert!(walk.errors.is_empty(), "importing an already-parsed root is not a cycle");
        assert_eq!(walk.files, vec![base, f1, f2]);
    }

    #[test]
    fn an_unresolvable_import_does_not_stop_the_walk() {
        // `parseFileFailure` is a *deferred* error (Parser.hs:201, Error.hs:130);
        // parsing continues, which is why one bad import does not hide the rest.
        let [top, good] = [0u32, 1].map(|i| ModuleSource::File(FileId(i)));
        let walk = walk_imports(&[top], |m| {
            if m == top {
                vec![
                    (ImportId(0), Err(ImportError::NotFound {
                        file: "missing.k".into(),
                        searched: vec![],
                    })),
                    (ImportId(1), Ok(good)),
                ]
            } else {
                vec![]
            }
        });
        assert_eq!(walk.errors.len(), 1);
        assert_eq!(walk.errors[0].import, ImportId(0));
        assert_eq!(walk.files, vec![good, top]);
    }
}
