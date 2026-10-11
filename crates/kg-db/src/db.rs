//! The database: inputs, revisions, memo tables, and the rooted entry point.
//!
//! # The shape, and why it is this one
//!
//! Salsa's contract without salsa: **inputs are set through `&mut self`**, which
//! bumps a revision, and **derived queries take `&self`** and memoize behind
//! interior mutability. Because the borrow checker enforces the split, a query
//! cannot mutate an input and no query can observe a half-applied change. That
//! is the whole safety argument, and it costs one dependency less.
//!
//! # What is memoized, and what each memo is keyed on
//!
//! | query | key | valid while |
//! |---|---|---|
//! | [`Db::parse`] | [`ModuleSource`] | the file's text revision is unchanged |
//! | [`Db::item_tree`] | [`ModuleSource`] | the *same [`Arc<Parse>`] instance* comes back |
//! | [`Db::program`] | the root set | settings, world and every participating item tree are unchanged |
//! | [`Db::global_symbols`] | root set × [`SymbolsConfig`] | the *same [`Arc<Program>`] instance* comes back |
//! | `crate::sorted`'s per-module lowering | [`ModuleSource`] | the same [`Arc<Parse>`], and every question put to the imports answered the same, by value |
//! | [`Db::looped`] | the root set | the same [`Arc<Program>`] instance comes back |
//!
//! Keying a derived query on the **identity** of its input rather than on a
//! revision is what makes this incremental rather than merely cached. [`Db::parse`]
//! compares the new green tree against the old one and hands back the *previous*
//! `Arc` when they are equal (`GreenNode: Eq`), so an edit-then-undo, a
//! whitespace-only change the lexer folds away, or a `didChange` that re-sends
//! identical text stops right there: the item tree, the program and the symbol
//! map are all reused without being rebuilt.
//!
//! # Two revisions, not one
//!
//! Every file has its own text revision, so editing one buffer invalidates one
//! parse. Separately, a **world** revision moves whenever a file's *existence*
//! changes, because import resolution probes candidate paths and a negative
//! probe is as much a dependency as a positive one — creating
//! `library/foo.k` can change what `import foo` resolves to in a file nobody
//! touched. Tracking the probes individually would be finer; tracking existence
//! globally is exact and cannot go stale.
//!
//! The converse matters as much: a change that leaves existence alone — every
//! keystroke — must *not* move the world, or each one rebuilds every
//! whole-program memo. The database remembers the existence it last saw per
//! file for exactly that comparison ([`Db::file_changed`]).
//!
//! # What is deliberately *not* memoized
//!
//! [`Db::check`] and [`Db::diagnostics`] recompute. The passes are tree walks
//! over an already-parsed file; parsing, lowering and the loop are the
//! expensive parts and they are memoized. More importantly, a diagnostics memo
//! would have to be keyed on whatever environment the passes read, and a key
//! that silently stops matching its query is a staleness bug that looks like
//! a conformance regression. The sorted lowering's memo
//! (`crate::sorted::SortedMemo`) is the worked example of doing it properly:
//! every input it reads is in its key — the module's parse by pointer, and
//! what it asked of the imports with the answers, by value (sprint W7.4).
//! The CST inference engine's type table was the first such memo, keyed
//! the same way; it went with that engine (sprint W3.7).

use std::sync::{Arc, Mutex, RwLock};

use kg_base::{FileId, FxHashMap, Interner, Symbol, SymbolPath, Utf8Path};
use kg_config::Settings;
use kg_diag::Diagnostic;
use kg_resolve::{ItemTree, ModuleSource, SymbolsConfig};
use kg_syntax::Parse;
use kg_vfs::Vfs;

use crate::budget::PassBudget;
use crate::stats::{Query, Stats};
use crate::program::{self, Program};
use crate::symbols::{self, SymbolMap};

/// A monotonically increasing stamp. Never wraps in any realistic session.
pub type Revision = u64;

/// The query database.
///
/// Cheap to share: clone the [`Arc<Vfs>`] into it rather than handing it a
/// `Vfs`, and a language server can keep one `Db` for the session while the
/// file-watcher thread owns the same VFS.
#[derive(Debug)]
pub struct Db {
    vfs: Arc<Vfs>,
    settings: Arc<Settings>,
    interner: Arc<Interner>,
    /// Bumped by every input change; the value a new stamp is taken from.
    revision: Revision,
    /// Revision at which [`Db::settings`] last changed.
    settings_revision: Revision,
    /// Revision at which any file's *existence* last changed.
    world_revision: Revision,
    /// Revision at which each file's text last changed. Absent means "never".
    file_revisions: FxHashMap<FileId, Revision>,
    /// How far the instantiation loop may iterate — [`PassBudget`].
    pass_budget: PassBudget,
    /// Whether each file existed when this database was last told about it,
    /// which is what lets an edit leave [`Db::world_revision`] alone.
    ///
    /// Remembered rather than probed because the probe is not available: a
    /// language server writes the shared [`Vfs`] *first* and tells the
    /// database afterwards, so by the time [`Db::file_changed`] runs the "before"
    /// state is gone. Absent means "never told", and is answered by bumping the
    /// world, since whatever a query probed then is unknown.
    existence: FxHashMap<FileId, bool>,
    memo: Memo,
    /// Every memo's hits and misses and each phase's time
    /// ([`crate::stats`]); never reset, read as differences. Shared with
    /// the editor's loop, which runs on a thread of its own
    /// (`crate::layered`).
    pub(crate) stats: Arc<Stats>,
    /// Raised by whoever holds [`Db::cancel_token`] — a language server
    /// with an edit queued — to stop a running query at its next checkpoint
    /// (`kg_base::cancel`); lowered by every input change.
    cancel: kg_base::cancel::Cancel,
}

#[derive(Debug)]
struct Memo {
    parses: RwLock<FxHashMap<ModuleSource, ParseMemo>>,
    item_trees: RwLock<FxHashMap<ModuleSource, ItemTreeMemo>>,
    programs: RwLock<Bounded<Vec<FileId>, ProgramMemo>>,
    symbols: RwLock<Bounded<(Vec<FileId>, SymbolsConfig), SymbolsMemo>>,
    looped: RwLock<Bounded<Vec<FileId>, LoopedMemo>>,
    /// The loop being computed per root set, which a second caller waits on
    /// rather than running it again ([`Db::looped`]).
    looped_in_flight: Mutex<FxHashMap<Vec<FileId>, Arc<InFlight>>>,
    /// The editor's loop running per root set, under a layered budget
    /// (`crate::layered`).
    flights: Mutex<FxHashMap<Vec<FileId>, Arc<crate::layered::Flight>>>,
    /// The editor's kept loops, by root set, and the shadow's reports
    /// (`crate::snapshot`; sprint W7.5d).
    keeper: Arc<crate::snapshot::Keeper>,
    /// Keyed by what the lowering read, by value — `crate::sorted::SortedMemo`.
    sorted: RwLock<FxHashMap<ModuleSource, Arc<crate::sorted::SortedMemo>>>,
}

impl Default for Memo {
    fn default() -> Memo {
        Memo {
            parses: RwLock::default(),
            item_trees: RwLock::default(),
            programs: RwLock::default(),
            symbols: RwLock::default(),
            looped: RwLock::new(Bounded::with_cap(MAX_INSTANCE_ROOT_SETS)),
            looped_in_flight: Mutex::default(),
            flights: Mutex::default(),
            keeper: Arc::default(),
            sorted: RwLock::default(),
        }
    }
}

/// How many distinct **root sets** the two whole-program memos keep.
///
/// The other two memos are keyed by [`ModuleSource`] and are therefore bounded
/// by the program itself. These two are keyed by the *set of roots asked about*,
/// which in a language server is one entry per file the user has ever opened —
/// unbounded in a process that runs for days, and each entry pins an
/// `Arc<Program>` with its module graph and its whole list of item trees.
///
/// The number is chosen to be larger than any plausible set of open editor tabs
/// and small enough that the worst case is bounded memory rather than bounded
/// correctness: a miss costs one `program::load`, whose own inputs are still
/// memoized per module, so an evicted entry does not re-parse anything.
///
/// **The measured price.** `xtask bench --filter keystroke` sweeps all 479
/// corpus files in sequence, which is a cache-hostile pattern no editor
/// performs — every entry is evicted before it is asked for again. It costs 7%
/// there (112 → 120 ms; A/B'd by raising this to 4096, which recovers exactly
/// the 112). That is the price of the bound, paid in the one workload designed
/// to defeat it, in exchange for a language server that does not grow without
/// limit over a working day. Raising the number to make that benchmark faster
/// would be tuning the constant to the measurement.
const MAX_ROOT_SETS: usize = 64;

/// The bound for the loop's whole-program memo ([`Db::looped`]) —
/// deliberately **not** [`MAX_ROOT_SETS`], because the justification there
/// does not transfer: a program-memo miss costs one `program::load` over
/// still-memoized parses, while a loop's costs the elaboration of the whole
/// program, and each retained entry pins every module's final tree and its
/// copies — memory in a dimension no other memo pays (`docs/journal.md`,
/// "Provenance is a copy…", the memory note). An editor holds a handful of
/// root sets at once, so the bound is small on purpose. (The name is the
/// second census's, whose two memos it bounded until sprint W3.7.)
const MAX_INSTANCE_ROOT_SETS: usize = 8;

/// A map that forgets its oldest entry rather than growing.
///
/// **FIFO, not LRU, and not clear-on-full.** An LRU wants an access stamp
/// written on every *read*, which would put the write lock on the hot path of a
/// cache whose whole point is to be read. Clearing the map when it fills was the
/// first attempt and is worse than either: it turns a steady trickle of misses
/// into a periodic cliff, which the `keystroke` benchmark noticed immediately.
///
/// Eviction is cheap to be wrong about. A miss costs one `program::load`, and
/// that load's own inputs are memoized per module — so an evicted entry
/// re-resolves a module graph and re-parses nothing.
#[derive(Debug)]
struct Bounded<K, V> {
    cap: usize,
    map: FxHashMap<K, V>,
    /// Keys in insertion order. Only keys new to the map are pushed, so this
    /// never holds a duplicate and its length tracks `map`'s.
    order: std::collections::VecDeque<K>,
}

impl<K, V> Default for Bounded<K, V> {
    fn default() -> Self {
        Bounded::with_cap(MAX_ROOT_SETS)
    }
}

impl<K, V> Bounded<K, V> {
    fn with_cap(cap: usize) -> Self {
        Bounded { cap, map: FxHashMap::default(), order: std::collections::VecDeque::new() }
    }
}

impl<K: std::hash::Hash + Eq + Clone, V> Bounded<K, V> {
    fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    fn insert(&mut self, key: K, value: V) {
        if self.map.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
            while self.order.len() > self.cap {
                if let Some(oldest) = self.order.pop_front() {
                    self.map.remove(&oldest);
                }
            }
        }
    }

    /// Drops every entry whose key `gone` picks, from the map and the
    /// order both, so the bound keeps counting what is held.
    fn remove_where(&mut self, gone: impl Fn(&K) -> bool) {
        self.map.retain(|k, _| !gone(k));
        self.order.retain(|k| !gone(k));
    }
}

#[derive(Clone, Debug)]
struct ParseMemo {
    value: Arc<Parse>,
    /// The text revision this was parsed at.
    stamp: Revision,
    /// `Settings::parse_docs` at the time, because the parse now depends on
    /// it. [`Db::stamp`] is the *text* revision for a `File`, so without this
    /// a mid-session `set_settings` that flips the flag would serve a parse
    /// with the other mode's error list. Compared rather than folded into a
    /// revision so that changing an unrelated setting does not invalidate
    /// every parse in the program.
    parse_docs: bool,
}

#[derive(Clone, Debug)]
struct ItemTreeMemo {
    value: Arc<ItemTree>,
    /// The exact `Parse` it was lowered from, compared by pointer.
    parse: Arc<Parse>,
}

#[derive(Clone, Debug)]
struct ProgramMemo {
    value: Arc<Program>,
    settings_revision: Revision,
    world_revision: Revision,
    /// Every item tree the load observed, by identity.
    trees: Vec<(ModuleSource, Arc<ItemTree>)>,
}

#[derive(Clone, Debug)]
struct SymbolsMemo {
    value: Arc<SymbolMap>,
    program: Arc<Program>,
}

#[derive(Clone, Debug)]
struct LoopedMemo {
    value: Arc<crate::looped::Looped>,
    /// Program identity: the sorted lowerings and the base tables the loop
    /// and its derived tables read are keyed on it.
    program: Arc<Program>,
    /// Whether `value` is the loop's last answer — false for the editor's
    /// first (`PassBudget::Layered`), kept as the newest loop until the
    /// last replaces it.
    last: bool,
}

/// One program's loop while it is computed: the first caller fills the
/// cell, every later one for the same program waits on it.
#[derive(Debug)]
struct InFlight {
    program: Arc<Program>,
    cell: std::sync::OnceLock<Arc<crate::looped::Looped>>,
}

// =============================================================================
// Inputs
// =============================================================================

/// A database dropped while the editor's loop runs on stops it
/// (`crate::layered`): nothing will read its answers.
impl Drop for Db {
    fn drop(&mut self) {
        self.retire_flights();
    }
}

impl Db {
    /// A database over a fresh VFS.
    pub fn new(settings: Settings) -> Db {
        Db::with_vfs(Arc::new(Vfs::new()), settings)
    }

    /// A database over an existing VFS — the language-server shape, where a
    /// watcher thread and the analysis share one file store.
    pub fn with_vfs(vfs: Arc<Vfs>, settings: Settings) -> Db {
        Db {
            vfs,
            settings: Arc::new(settings),
            interner: Arc::new(Interner::new()),
            revision: 1,
            settings_revision: 1,
            world_revision: 1,
            file_revisions: FxHashMap::default(),
            pass_budget: PassBudget::default(),
            existence: FxHashMap::default(),
            memo: Memo::default(),
            stats: Arc::default(),
            cancel: kg_base::cancel::Cancel::new(),
        }
    }

    /// The token that stops a running query at its next checkpoint: raise
    /// it from any thread, without the database's lock. The query unwinds
    /// with `kg_base::cancel::Cancelled` (catch it with
    /// `kg_base::cancel::catch`) and writes no memo of its own; queries it
    /// had finished keep theirs. The next input change lowers it.
    pub fn cancel_token(&self) -> kg_base::cancel::Cancel {
        self.cancel.clone()
    }

    /// Every memo's hits and misses and each phase's exclusive time so far
    /// ([`crate::stats`]); subtract two snapshots for what one request cost.
    pub fn stats(&self) -> crate::stats::Snapshot {
        self.stats.snapshot()
    }

    /// Bounds the elaboration's rounds, or layers its answers — the
    /// language server's [`PassBudget::EDITOR`]. A batch compile leaves the
    /// default, [`PassBudget::Fixpoint`], which is the reference's
    /// behaviour.
    ///
    /// Moves the settings revision: every elaboration is keyed on program
    /// identity, and a program is rebuilt when the settings move, so nothing
    /// computed under the old budget survives. Meant to be called once, when
    /// the database is made.
    pub fn set_pass_budget(&mut self, budget: PassBudget) {
        if budget != self.pass_budget {
            self.retire_flights();
            self.memo.keeper.clear();
            self.pass_budget = budget;
            self.revision += 1;
            self.settings_revision = self.revision;
        }
    }

    pub fn pass_budget(&self) -> PassBudget {
        self.pass_budget
    }

    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The one interner every [`kg_base::Symbol`] in this database was made
    /// with. A `Symbol` is meaningless across interners, so anything comparing
    /// names must go through this rather than starting its own.
    pub fn interner(&self) -> &Interner {
        &self.interner
    }

    /// [`Db::interner`], shared: for what outlives the call that made it —
    /// a harvest's deferred fingerprints (`kg_hir::sort::Fingerprint`).
    pub(crate) fn shared_interner(&self) -> &Arc<Interner> {
        &self.interner
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// The revision at which some file's existence last changed — the key
    /// every whole-program memo checks, exposed so a test can say "that edit
    /// did not move it".
    pub fn world_revision(&self) -> Revision {
        self.world_revision
    }

    /// Interns `path`, without asserting anything about whether it exists.
    pub fn file_id(&self, path: &Utf8Path) -> FileId {
        self.vfs.intern(path)
    }

    pub fn path(&self, file: FileId) -> Option<kg_base::Utf8PathBuf> {
        self.vfs.path(file)
    }

    /// Replaces the project settings — import dirs, target device, defines.
    ///
    /// Invalidates every program and both synthetic modules, because the search
    /// path and the generated `.cmdargs` text both derive from here
    /// (`Parser/IO.hs:51-65,110`). Per-file parses survive.
    pub fn set_settings(&mut self, settings: Settings) {
        self.retire_flights();
        self.memo.keeper.clear();
        self.cancel.reset();
        self.revision += 1;
        self.settings_revision = self.revision;
        self.settings = Arc::new(settings);
    }

    /// Sets the in-memory contents of `path` — `textDocument/didOpen` and
    /// `didChange`.
    ///
    /// Cheap to call with unchanged text: the reparse happens, but
    /// [`Db::parse`] recognises the identical green tree and everything
    /// downstream keeps its memo.
    pub fn set_file_text(&mut self, path: &Utf8Path, text: String) -> FileId {
        let file = self.vfs.intern(path);
        self.set_overlay(file, text);
        file
    }

    /// [`Db::set_file_text`] for a file already interned.
    pub fn set_overlay(&mut self, file: FileId, text: String) {
        // Probing is exact here, unlike in `file_changed`: this database is
        // the writer, so the Vfs still holds the "before".
        let existed = self.vfs.read(file).is_some();
        self.vfs.set_overlay(file, text);
        self.bump_file(file);
        self.existence.insert(file, true);
        if !existed {
            // A path that previously probed as absent now resolves, so every
            // import search may answer differently.
            self.bump_world();
        }
    }

    /// Drops an overlay — `textDocument/didClose`. The file reverts to disk.
    pub fn remove_overlay(&mut self, file: FileId) {
        if !self.vfs.has_overlay(file) {
            return;
        }
        self.vfs.remove_overlay(file);
        self.bump_file(file);
        let exists = self.vfs.read(file).is_some();
        self.existence.insert(file, exists);
        if !exists {
            self.bump_world();
        }
    }

    /// Forgets what was read from disk for `file` — a watcher event.
    ///
    /// Moves the world only if the file's existence differs from what this
    /// database last saw of it. An edit to a file that exists before and after
    /// cannot change what any import search finds, and bumping the world for
    /// it would rebuild every whole-program memo on every keystroke.
    pub fn invalidate(&mut self, file: FileId) {
        self.vfs.invalidate(file);
        let exists = self.vfs.read(file).is_some();
        self.file_changed(file, exists);
    }

    /// Someone else already changed `file` in the shared [`Vfs`]; `exists` is
    /// whether it existed immediately after that write, as the writer saw it.
    ///
    /// The language server's door. It writes overlays into the `Vfs` on its
    /// main thread and hands the database the news later, on a worker, so the
    /// database must not probe for itself: by then the `Vfs` may be several
    /// writes further on, and a file that vanished and came back in between
    /// would probe "present" twice and never move the world — while a query
    /// running in that window saw it absent and memoized the failed import.
    /// Passing each write's own after-state, in order, makes every transition
    /// visible here even when the `Vfs` has since moved past it.
    ///
    /// Does not touch the `Vfs`'s disk cache; the writer owns that too.
    pub fn file_changed(&mut self, file: FileId, exists: bool) {
        self.bump_file(file);
        if self.existence.insert(file, exists) != Some(exists) {
            // `None` — never told about this file — also lands here. Whatever
            // an import search probed for it before now is unknown, so the
            // answer is to assume it changed.
            self.bump_world();
        }
    }

    /// Forgets every disk read. Overlays survive.
    pub fn invalidate_all(&mut self) {
        self.vfs.invalidate_all();
        self.retire_flights();
        self.cancel.reset();
        self.revision += 1;
        self.world_revision = self.revision;
        let stamp = self.revision;
        for (_, rev) in self.file_revisions.iter_mut() {
            *rev = stamp;
        }
        // Every remembered existence may be wrong now that the disk will be
        // read afresh, and a stale "present" would let the next change to that
        // file skip the world bump it needs.
        self.existence.clear();
        // A file read but never stamped must also be invalidated; the world
        // revision covers programs, and this covers per-file memos.
        self.memo.parses.write().unwrap().clear();
    }

    fn bump_file(&mut self, file: FileId) {
        self.retire_flights();
        self.cancel.reset();
        self.revision += 1;
        self.file_revisions.insert(file, self.revision);
    }

    fn bump_world(&mut self) {
        self.retire_flights();
        self.cancel.reset();
        self.revision += 1;
        self.world_revision = self.revision;
    }

    /// Stops every loop running under a layered budget (`crate::layered`):
    /// an input changed, and whatever it computes may be stale. Each answers
    /// to a token of its own, never lowered — the database's is lowered
    /// here, and a run nobody holds the database for may not have seen it
    /// raised. A loop whose program the change left alone is stopped too,
    /// as the database's token stops every query.
    fn retire_flights(&mut self) {
        for (_, flight) in self.memo.flights.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner).drain() {
            flight.cancel.raise();
        }
    }

    /// The revision a module's text was last known to change at. The synthetic
    /// modules are generated from the settings, so they move with those.
    fn stamp(&self, module: ModuleSource) -> Revision {
        match module {
            ModuleSource::File(file) => {
                self.file_revisions.get(&file).copied().unwrap_or_default()
            }
            ModuleSource::Cmdargs | ModuleSource::Options => self.settings_revision,
        }
    }
}

// =============================================================================
// Derived queries
// =============================================================================

impl Db {
    /// The parse tree of one file.
    pub fn parse(&self, file: FileId) -> Arc<Parse> {
        self.parse_module(ModuleSource::File(file))
    }

    /// The parse tree of any module, real or synthetic.
    ///
    /// The two synthetic modules are **derived inputs**: their text is generated
    /// from the settings (`Parser/IO.hs:37-65`) and then fed through the exact
    /// same parse / item-tree / interface chain as a real file, which is what
    /// keeps one code path for both (`Parser/IO.hs:100-102`).
    pub fn parse_module(&self, module: ModuleSource) -> Arc<Parse> {
        let stamp = self.stamp(module);
        let parse_docs = self.settings.parse_docs;
        let cached = self.memo.parses.read().unwrap().get(&module).cloned();
        if let Some(memo) = &cached
            && memo.stamp == stamp
            && memo.parse_docs == parse_docs
        {
            self.stats.hit(Query::Parse);
            return memo.value.clone();
        }
        let _miss = self.stats.missed(Query::Parse);

        let text = self.module_text(module);
        // `--parse-docs` (`Options/CmdArgs.hs:66`) reaches the parser here and
        // nowhere else: this is the only path from `Settings` to
        // `kg_syntax`, so the flag was plumbed through both CLIs and the
        // conformance harness and read by nothing for as long as it existed.
        // It changes the error list only — doc comments stay trivia in both
        // modes — so the tree, losslessness and the formatter are unaffected.
        let parsed = kg_syntax::parse_file_with(
            &text,
            kg_syntax::ParseOptions { parse_docs },
        );
        // The firewall: an edit that produces an identical tree keeps the old
        // `Arc`, so `item_tree`, `program` and `global_symbols` all stay valid.
        let value = match cached {
            Some(memo) if *memo.value == parsed => memo.value,
            _ => Arc::new(parsed),
        };
        let mut parses = self.memo.parses.write().unwrap();
        // Another thread that missed with us may have stored its parse of
        // the same text first: adopt it, so every reader of this revision
        // holds one `Arc` and the queries keyed on its identity — the item
        // tree, the program, the loop — are computed once (`Db::program`).
        if let Some(memo) = parses.get(&module)
            && memo.stamp == stamp
            && memo.parse_docs == parse_docs
        {
            return memo.value.clone();
        }
        parses.insert(module, ParseMemo { value: value.clone(), stamp, parse_docs });
        value
    }

    /// The last sorted lowering of `module`, for `crate::sorted` to validate
    /// against its key.
    pub(crate) fn sorted_memo(&self, module: ModuleSource) -> Option<Arc<crate::sorted::SortedMemo>> {
        self.memo.sorted.read().unwrap().get(&module).cloned()
    }

    pub(crate) fn set_sorted_memo(&self, module: ModuleSource, memo: Arc<crate::sorted::SortedMemo>) {
        self.memo.sorted.write().unwrap().insert(module, memo);
    }

    /// The declarations, scopes and imports of one module — `getSymbols`' input
    /// and the unit that must stay stable under an edit to a function body
    /// (`docs/spec/resolve.md` §11.2).
    pub fn item_tree(&self, module: ModuleSource) -> Arc<ItemTree> {
        let parse = self.parse_module(module);
        let cached = self.memo.item_trees.read().unwrap().get(&module).cloned();
        if let Some(memo) = cached
            && Arc::ptr_eq(&memo.parse, &parse)
        {
            self.stats.hit(Query::ItemTree);
            return memo.value;
        }
        let _miss = self.stats.missed(Query::ItemTree);

        let name = match module {
            ModuleSource::File(file) => self.path(file).map(|p| p.to_string()),
            _ => None,
        };
        let value = Arc::new(kg_resolve::lower_file_named(
            module,
            &parse.source_file(),
            &self.interner,
            name.as_deref(),
        ));
        let mut trees = self.memo.item_trees.write().unwrap();
        // One tree per parse however many lowered it at once (`Db::program`).
        if let Some(memo) = trees.get(&module)
            && Arc::ptr_eq(&memo.parse, &parse)
        {
            return memo.value.clone();
        }
        trees.insert(module, ItemTreeMemo { value: value.clone(), parse });
        value
    }

    /// The whole compilation rooted at `roots`: the base library, every
    /// transitive import, the module graph, and the loading diagnostics.
    ///
    /// `roots` is a *set of entry files*, not one, because the reference merges
    /// every command-line file into a single program (`Main.hs:70,76`) — so a
    /// declaration in one is visible to another in the same invocation. Order
    /// matters: it is the order the DFS visits them in, and therefore part of
    /// the file order duplicate-key precedence reads.
    pub fn program(&self, roots: &[FileId]) -> Arc<Program> {
        let key = roots.to_vec();
        let cached = self.memo.programs.read().unwrap().get(&key).cloned();
        if let Some(memo) = cached
            && memo.settings_revision == self.settings_revision
            && memo.world_revision == self.world_revision
            && memo.trees.iter().all(|(m, t)| Arc::ptr_eq(&self.item_tree(*m), t))
        {
            self.stats.hit(Query::Program);
            return memo.value;
        }
        let _miss = self.stats.missed(Query::Program);

        let value = Arc::new(program::load(self, roots));
        let trees: Vec<(ModuleSource, Arc<ItemTree>)> = value.files.iter().map(|&m| (m, self.item_tree(m))).collect();
        let mut programs = self.memo.programs.write().unwrap();
        // **One program per input state, however many load it at once.**
        // The language server's diagnostics and a request on the same edit
        // both miss here and both load; each minted its own `Arc`, and every
        // query keyed on program identity was then computed twice — the
        // loop's in-flight slot (`Db::looped`) matches by identity, so the
        // second caller ran a second loop beside the first. The first to
        // store wins and the others adopt it: the same revisions over the
        // same trees is the same program, and the parse and item-tree
        // memos adopt the same way, so racing loaders hold the same trees.
        if let Some(memo) = programs.get(&key)
            && memo.settings_revision == self.settings_revision
            && memo.world_revision == self.world_revision
            && memo.trees.len() == trees.len()
            && memo.trees.iter().zip(&trees).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
        {
            return memo.value.clone();
        }
        programs.insert(
            key,
            ProgramMemo {
                value: value.clone(),
                settings_revision: self.settings_revision,
                world_revision: self.world_revision,
                trees,
            },
        );
        value
    }

    /// Drops `roots`' whole-program memos — the program, its symbol maps,
    /// its loop — for a caller that asked about the root set once and will
    /// not again: a workspace search, which resolves each file it searches
    /// under the program that file roots (`kg-lsp`'s `references`), would
    /// otherwise leave up to [`MAX_ROOT_SETS`] programs behind it, each
    /// pinning its module graph and a symbol map over the whole program,
    /// and push the open buffers' own out of the bound on the way. The
    /// per-module memos stay: a module's parse and item tree are every
    /// program's that holds it.
    pub fn forget_roots(&self, roots: &[FileId]) {
        self.memo.programs.write().unwrap().remove_where(|k| k.as_slice() == roots);
        self.memo.symbols.write().unwrap().remove_where(|(k, _)| k.as_slice() == roots);
        self.memo.looped.write().unwrap().remove_where(|k| k.as_slice() == roots);
    }

    /// **The seam every unwired pass needs**: `getSymbols' config` over the whole
    /// program (`Symbols.hs:26,37-71`), built once and shared.
    ///
    /// Which config a pass wants is not a matter of taste — it is written in the
    /// Haskell. `withSymbols` is `getSymbols`, i.e.
    /// [`SymbolsConfig::AllSymbols`] (`Frontend.hs:40`); the inference fixpoint
    /// wants [`SymbolsConfig::TemplateDecl`] (`Inference.hs:37`);
    /// `addLambdaWrappers` wants [`SymbolsConfig::NoTemplate`]
    /// (`Template.hs:79`). The crate-level `unwired` table names the config for
    /// each pass.
    pub fn global_symbols(&self, roots: &[FileId], config: SymbolsConfig) -> Arc<SymbolMap> {
        let program = self.program(roots);
        let key = (roots.to_vec(), config);
        let cached = self.memo.symbols.read().unwrap().get(&key).cloned();
        if let Some(memo) = cached
            && Arc::ptr_eq(&memo.program, &program)
        {
            self.stats.hit(Query::Symbols);
            return memo.value;
        }
        let _miss = self.stats.missed(Query::Symbols);

        let value = Arc::new(symbols::build(&program, config, &self.interner));
        self.memo
            .symbols
            .write()
            .unwrap()
            .insert(key, SymbolsMemo { value: value.clone(), program });
        value
    }

    /// The program-wide loop on the IR (`kg_sem::elaborate`) over the
    /// program, with each module's engine-derived type table
    /// ([`crate::looped`]) — memoized on program identity. The backend walk
    /// reads its trees; the CST passes read its tables and carry its
    /// diagnostics.
    ///
    /// **Computed once per program however many ask at once.** A language
    /// server's diagnostics and a hover or an inlay hint on the same edit
    /// arrive on different threads, and both miss while the first is still
    /// computing: the second waits for the first rather than running the
    /// loop again (`Db::editor_loop`). The computation is detached from the
    /// request that started it (`kg_base::cancel::detached`) and answers to
    /// the database's token alone — an edit stops it, a `$/cancelRequest`
    /// for whichever request happened to start it does not, since another
    /// may be waiting on it. A computation an edit stops leaves the cell
    /// empty, and a waiter then runs it itself, into the same token.
    ///
    /// **Under the editor's layered budget** ([`PassBudget::Layered`]) the
    /// loop runs on a thread of its own and this waits for its last answer,
    /// the fixpoint's; [`Db::looped_first`] takes the first on the way
    /// (`crate::layered`).
    pub fn looped(&self, roots: &[FileId]) -> Arc<crate::looped::Looped> {
        if let Some(at) = self.pass_budget.first_answer() {
            return crate::layered::looped(self, roots, at, crate::layered::Want::Last).0;
        }
        let program = self.program(roots);
        let key = roots.to_vec();
        let cached = self.memo.looped.read().unwrap().get(&key).cloned();
        if let Some(memo) = cached
            && Arc::ptr_eq(&memo.program, &program)
            && memo.last
        {
            self.stats.hit(Query::Looped);
            return memo.value;
        }
        let flight = {
            let mut in_flight = self.memo.looped_in_flight.lock().unwrap();
            match in_flight.get(&key) {
                Some(f) if Arc::ptr_eq(&f.program, &program) => f.clone(),
                _ => {
                    let f = Arc::new(InFlight { program: program.clone(), cell: std::sync::OnceLock::new() });
                    in_flight.insert(key.clone(), f.clone());
                    f
                }
            }
        };
        let mut computed = false;
        let value = flight
            .cell
            .get_or_init(|| {
                computed = true;
                let _miss = self.stats.missed(Query::Looped);
                let looped = kg_base::cancel::detached(|| {
                    kg_base::cancel::scope(&self.cancel, || crate::looped::looped(self, roots))
                });
                Arc::new(looped)
            })
            .clone();
        if computed {
            self.memo.looped.write().unwrap().insert(key.clone(), LoopedMemo { value: value.clone(), program, last: true });
            let mut in_flight = self.memo.looped_in_flight.lock().unwrap();
            if in_flight.get(&key).is_some_and(|f| Arc::ptr_eq(f, &flight)) {
                in_flight.remove(&key);
            }
        } else {
            self.stats.hit(Query::Looped);
        }
        value
    }

    /// The newest loop the editor can read without waiting past the
    /// layered budget's bound ([`PassBudget::Layered`]): the last answer
    /// where it is ready, and otherwise the first, waiting for it if the
    /// run has not reached it — never for the fixpoint, never a second run
    /// (`crate::layered`). With whether it is the last. Under any other
    /// budget, [`Db::looped`]'s, the last.
    ///
    /// The diagnostics publish the first and then the last
    /// ([`Db::diagnostics_layered`]); every other reader takes this
    /// ([`Db::editor_loop`]), so a hover asked while the fixpoint runs on is
    /// answered from the first answer at once.
    pub fn looped_first(&self, roots: &[FileId]) -> (Arc<crate::looped::Looped>, bool) {
        match self.pass_budget.first_answer() {
            Some(at) => crate::layered::looped(self, roots, at, crate::layered::Want::First),
            None => (self.looped(roots), true),
        }
    }

    /// `roots`' loop in the memo, where it is `program`'s, with whether it
    /// is the last answer (`crate::layered`).
    pub(crate) fn looped_memo(&self, roots: &[FileId], program: &Arc<Program>) -> Option<(Arc<crate::looped::Looped>, bool)> {
        let memo = self.memo.looped.read().unwrap().get(&roots.to_vec()).cloned()?;
        Arc::ptr_eq(&memo.program, program).then_some((memo.value, memo.last))
    }

    /// Keeps `value`, `program`'s loop, as `roots`' newest — a first answer
    /// only where the memo holds no last answer of the same program: the
    /// two can be built in either order (`crate::layered`).
    pub(crate) fn remember_looped(&self, roots: &[FileId], program: &Arc<Program>, value: &Arc<crate::looped::Looped>, last: bool) {
        let key = roots.to_vec();
        let mut memo = self.memo.looped.write().unwrap();
        if !last && memo.get(&key).is_some_and(|m| m.last && Arc::ptr_eq(&m.program, program)) {
            return;
        }
        memo.insert(key, LoopedMemo { value: value.clone(), program: program.clone(), last });
    }

    /// The loops running under a layered budget, by root set.
    /// The editor's kept loops (`crate::snapshot`).
    pub(crate) fn keeper(&self) -> &Arc<crate::snapshot::Keeper> {
        &self.memo.keeper
    }

    pub(crate) fn flights(&self) -> &Mutex<FxHashMap<Vec<FileId>, Arc<crate::layered::Flight>>> {
        &self.memo.flights
    }

    /// The number of instantiation rounds, when the loop already run to its
    /// end for `roots` stopped at its [`PassBudget`]'s bound rather than at
    /// the fixpoint — `--template-iterations`, or the guard a loop that does
    /// not converge reaches — and `None` when it converged, or was never
    /// asked for. Reads the memo only: a caller reporting the bound must not
    /// start a loop [`Db::check`] had no reason to. The editor's first
    /// answer is never the loop's end, and never reported here.
    pub fn budget_exhausted(&self, roots: &[FileId]) -> Option<u32> {
        let key = roots.to_vec();
        let looped = self.memo.looped.read().unwrap().get(&key).cloned().filter(|m| m.last)?;
        let report = &looped.value.out.report;
        report.budget_exhausted.then_some(report.rounds)
    }

    /// The source text of one module: the VFS for a real file, generated text
    /// for the two synthetic ones.
    pub(crate) fn module_text(&self, module: ModuleSource) -> String {
        match module {
            ModuleSource::File(file) => {
                self.vfs.read(file).map(|t| t.text).unwrap_or_default()
            }
            // `buildCmdargsSpecialModule` (`Parser/IO.hs:49-55`).
            ModuleSource::Cmdargs => kg_resolve::module::cmdargs_source(
                &self.settings.defines,
                &self.settings.using,
            ),
            // `buildOptionsSpecialModule` (`Parser/IO.hs:57-65`): the five
            // constants come from the backend options in the settings, with
            // the compiler's command-line defaults.
            ModuleSource::Options => {
                let b = &self.settings.backend;
                kg_resolve::module::OptionsModule {
                    backend: if b.backend.eq_ignore_ascii_case("sv") {
                        kg_resolve::module::Backend::Sv
                    } else {
                        kg_resolve::module::Backend::None
                    },
                    stall: b.stall,
                    optimize: b.optimize,
                    max_threads_default: b.max_threads_default,
                    max_threads_limit: b.max_threads_limit,
                }
                .source()
            }
        }
    }
}

/// Sorts diagnostics into **program order**: by the file's position in the
/// post-order-DFS file list, then by offset within the file.
///
/// Not cosmetic. The reference merges every file's AST in that order
/// (`Main.hs:70,76`) and prints the resulting errors in it, so "the first error"
/// — the only thing `syntax-test.py:113-118` looks at — means the same thing on
/// both sides. It is also what makes the output stable: the memo tables are
/// hash maps, and without this the order would drift between runs.
fn in_program_order(program: &Program, mut diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let position: FxHashMap<FileId, usize> = program
        .files
        .iter()
        .enumerate()
        .map(|(i, m)| (m.diagnostic_file(), i))
        .collect();
    diagnostics.sort_by_key(|d| {
        (position.get(&d.range.file).copied().unwrap_or(usize::MAX), d.range.range.start())
    });
    diagnostics
}

// =============================================================================
// The rooted entry point
// =============================================================================

impl Db {
    /// Every diagnostic the frontend produces for the whole program rooted at
    /// `roots` — the batch-compile answer, and the one a rooted conformance run
    /// asks for.
    ///
    /// # The parse gate is reproduced
    ///
    /// If **any** file fails to parse — a syntax error, a redefinition
    /// (`SymbolTable.hs:200-201`), an unresolved or circular import
    /// (`Parser.hs:201`) — the reference exits before `frontend` is ever entered
    /// (`Main.hs:73-75`), so none of the nineteen passes runs and none of their
    /// diagnostics exists. Reproducing that is not pedantry: running the passes
    /// anyway would invent diagnostics on a tree the reference never desugars,
    /// which is the false-positive direction.
    pub fn check(&self, roots: &[FileId]) -> Vec<Diagnostic> {
        let program = self.program(roots);

        let mut out = self.parse_stage_of(roots, &program);
        if out.iter().any(Diagnostic::is_error) {
            return in_program_order(&program, out);
        }

        // The program's flag (`Db::program_erred_through_pass_9`) from the
        // runs themselves: each records whether its module erred through
        // pass 9, and a module holding a verdict that waits on the flag
        // runs after every other, so only its own kind are run twice. The
        // order is no output's: the diagnostics are sorted below.
        let known = KnownErred::default();
        let waits = |m: ModuleSource| self.looped(roots).text_for(m).iter().any(|v| v.undecided);
        let (last, first): (Vec<ModuleSource>, Vec<ModuleSource>) = program.files.iter().partition(|&&m| waits(m));
        for module in first.into_iter().chain(last) {
            self.cancel.check();
            let erred = || self.program_erred_through_pass_9(roots, module, true, &known, Elab::Last);
            let run = self.frontend_diagnostics(roots, module, Elab::Last, &erred);
            known.borrow_mut().insert(module, run.erred);
            out.extend(run.diagnostics);
        }
        // `--Wno-x` is applied here rather than by each caller, so the CLI, the
        // language server and the conformance harness cannot disagree about
        // which warnings exist. Errors are never filtered: `WarningSet::filter`
        // keeps every diagnostic whose `warning` is `None`.
        in_program_order(&program, self.settings.warnings.filter(out))
    }

    /// Every diagnostic for **one file** of the program rooted at `roots` — the
    /// language-server answer, one `textDocument/publishDiagnostics` payload.
    ///
    /// Unlike [`Db::check`] the parse gate is applied per file, not
    /// program-wide: a user editing `a.k` should keep seeing `b.k`'s errors, and
    /// an editor is not deciding conformance.
    ///
    /// A verdict that something was left undecided still waits on the
    /// whole program's flag (`Db::program_erred_through_pass_9`): another
    /// file's error can be its whole reason. The flag is asked only where
    /// such a verdict is at hand, so a clean file pays nothing for it.
    pub fn diagnostics(&self, roots: &[FileId], file: FileId) -> Vec<Diagnostic> {
        self.diagnostics_at(roots, file, Elab::Last)
    }

    /// [`Db::diagnostics`] as the editor publishes them under a layered
    /// budget ([`PassBudget::Layered`]): `first` is handed the answer at the
    /// loop's first bound — read off the run's own state there, which then
    /// goes on — and this returns the last, the fixpoint's, which are
    /// [`Db::diagnostics`]' and so `kg check`'s for the file. `first` is not
    /// called where there is no first answer: a buffer stopped before the
    /// loop, a loop that ended by the bound, a database with another budget.
    ///
    /// An edit cancels the run past the first answer like any query
    /// (`kg_base::cancel`): this unwinds, the first answer already handed
    /// out stands, and no last answer is memoized.
    pub fn diagnostics_layered(&self, roots: &[FileId], file: FileId, first: impl FnOnce(Vec<Diagnostic>)) -> Vec<Diagnostic> {
        if self.pass_budget.first_answer().is_some() && !self.before_the_passes(roots, file).iter().any(Diagnostic::is_error) {
            let (looped, last) = self.looped_first(roots);
            if !last {
                first(self.diagnostics_at(roots, file, Elab::At(&looped)));
            }
        }
        self.diagnostics(roots, file)
    }

    /// [`Db::diagnostics`] at the newest loop ready ([`Db::looped_first`]):
    /// under a layered budget, what the gutter shows while the fixpoint
    /// runs on — for a reader of the diagnostics that must not wait for it,
    /// a code action.
    pub fn diagnostics_first(&self, roots: &[FileId], file: FileId) -> Vec<Diagnostic> {
        if self.pass_budget.first_answer().is_none() || self.before_the_passes(roots, file).iter().any(Diagnostic::is_error) {
            return self.diagnostics(roots, file);
        }
        let (looped, _) = self.looped_first(roots);
        self.diagnostics_at(roots, file, Elab::At(&looped))
    }

    /// [`Db::diagnostics`], the passes reading the loop `elab` names.
    fn diagnostics_at(&self, roots: &[FileId], file: FileId, elab: Elab<'_>) -> Vec<Diagnostic> {
        let mut out = self.before_the_passes(roots, file);
        if out.iter().any(Diagnostic::is_error) {
            return out;
        }
        let module = ModuleSource::File(file);
        let known = KnownErred::default();
        let erred = || self.program_erred_through_pass_9(roots, module, false, &known, elab);
        out.extend(self.frontend_diagnostics(roots, module, elab, &erred).diagnostics);
        self.settings.warnings.filter(out)
    }

    /// Whether any module of the program rooted at `roots` erred through
    /// pass 9 — the reference's one flag as `cata typeErrors`, the first
    /// pass after `undefinedSymbols`, finds it: `checkPar` after the pass-1
    /// composite, `instantiateTemplates`' `hasErrors'` and pass 9's `or
    /// errs` set it, program-wide, and every pass after is then skipped
    /// (`Frontend.hs:44-52,85-86`, `Template.hs:55-56,84`). The verdicts
    /// that wait on it are the loop's undecided ones
    /// (`kg_sem::check::TextVerdict::undecided`).
    ///
    /// Folded, first error wins, from what decides it here: the parse stage
    /// of every module — the reference never enters `frontend` past one
    /// (`Main.hs:73-75`); skipped where the caller has seen it clean — the
    /// loop's own refusals and copies (`Looped::erred_through_pass_9`), and
    /// each module's CST run through pass 9 at its text. `asking` is the
    /// module whose run asks, which got past pass 9 to ask; `known` holds
    /// the runs already made ([`CstRun::erred`]), and any other is made
    /// now, through pass 9 only ([`Db::erred_through_pass_9_of`]). Nothing
    /// here is memoized beyond the one check: the CST runs never are
    /// (this module's docs), and a run made for the flag is no more than
    /// the run [`Db::check`] makes of the module anyway.
    fn program_erred_through_pass_9(
        &self,
        roots: &[FileId],
        asking: ModuleSource,
        parse_stage_clean: bool,
        known: &KnownErred,
        elab: Elab<'_>,
    ) -> bool {
        let program = self.program(roots);
        if !parse_stage_clean && self.parse_stage_of(roots, &program).iter().any(Diagnostic::is_error) {
            return true;
        }
        if elab.looped(self, roots).erred_through_pass_9() {
            return true;
        }
        let others = || program.files.iter().copied().filter(|&m| m != asking);
        if others().any(|m| known.borrow().get(&m) == Some(&true)) {
            return true;
        }
        let unknown: Vec<ModuleSource> = others().filter(|m| !known.borrow().contains_key(m)).collect();
        let found = self.erred_through_pass_9_of(roots, &unknown, elab);
        let mut known = known.borrow_mut();
        for (&module, erred) in unknown.iter().zip(&found) {
            if let Some(erred) = *erred {
                known.insert(module, erred);
            }
        }
        found.contains(&Some(true))
    }

    /// [`Db::erred_through_pass_9_at`] for each of `modules`, spread over
    /// threads, each with a program context of its own — the editor asks it
    /// of every module but its own buffer's (measured 2026-10-10: 20–28 ms
    /// over 32–35 modules in a row, one after another), and the runs read
    /// only memos. Stops once one errs, since the flag is their `or`: a
    /// module not run by then is `None`. A cancellation in a thread
    /// unwinds out of this call as it would have out of the run itself.
    fn erred_through_pass_9_of(&self, roots: &[FileId], modules: &[ModuleSource], elab: Elab<'_>) -> Vec<Option<bool>> {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        if modules.is_empty() {
            return Vec::new();
        }
        let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8).min(modules.len());
        let next = AtomicUsize::new(0);
        let erred = AtomicBool::new(false);
        let slots: Vec<Mutex<Option<bool>>> = modules.iter().map(|_| Mutex::new(None)).collect();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let cx = self.cst_program(roots, elab);
                        while !erred.load(Ordering::Relaxed) {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&module) = modules.get(i) else { break };
                            let found = self.erred_through_pass_9_at(&cx, module);
                            *slots[i].lock().unwrap() = Some(found);
                            if found {
                                erred.store(true, Ordering::Relaxed);
                            }
                        }
                    })
                })
                .collect();
            let mut unwound = None;
            for handle in handles {
                if let Err(payload) = handle.join() {
                    unwound.get_or_insert(payload);
                }
            }
            if let Some(payload) = unwound {
                std::panic::resume_unwind(payload);
            }
        });
        slots.into_iter().map(|s| s.into_inner().unwrap()).collect()
    }

    /// Whether `module`'s CST run errs through pass 9 at its own text —
    /// the run [`Db::frontend_diagnostics`] makes, stopped there and seeded
    /// with nothing: the loop's verdicts it is seeded with are the copies'
    /// passes after 9, which the reference's flag never reads, and the
    /// loop's part of the flag is asked apart.
    fn erred_through_pass_9_at(&self, cx: &CstProgram, module: ModuleSource) -> bool {
        kg_base::cancel::scope(&self.cancel, || {
            self.with_cst_module(cx, module, |root, whole, looped| {
                let from_loop = crate::passes::FromLoop::of(looped, module, &crate::passes::erred_always);
                crate::passes::undefined_symbols::run_through_9(root, whole, crate::passes::Carried::default(), &from_loop).erred()
            })
        })
    }

    /// What [`Db::diagnostics`] reports for `file` before any pass runs —
    /// loading, syntax, the parse-time symbol table, exposure — whose
    /// first error stops it there, before the program-wide loop.
    fn before_the_passes(&self, roots: &[FileId], file: FileId) -> Vec<Diagnostic> {
        let program = self.program(roots);
        let mut out: Vec<Diagnostic> = program
            .diagnostics
            .iter()
            .filter(|d| d.range.file == file)
            .cloned()
            .collect();
        out.extend(self.parse_stage_diagnostics(ModuleSource::File(file)));
        out.extend(self.exposure_ambiguities(roots, &program).into_iter().filter(|d| d.range.file == file));
        out
    }

    /// The program-wide loop the editor's type-reading features read at
    /// `file` — inlay hints, signature help, navigation — at the text the
    /// buffer holds ([`crate::looped::EditorLoop`]): on a buffer that
    /// parses, the loop its diagnostics run; on one that does not, the last
    /// loop computed for `roots`, mapped; `None` where there is none, and
    /// the features have no type to show (sprint W3.7: the CST engine that
    /// answered there is deleted).
    ///
    /// **The loop the diagnostics run, and no other.** The same root set,
    /// so the same memo, and the same gate: a buffer whose diagnostics stop
    /// before the loop — one that does not parse, mid-keystroke — runs none
    /// for a tooltip either. Measured on `risc-v.k` (2026-10-10, the
    /// journal's "The editor on a buffer that does not parse"): a keystroke
    /// that breaks the parse costs 23–26 ms of diagnostics, and the broken
    /// buffer's own loop another 234–273 ms under the bench's settings and
    /// 680–687 ms under the recorded ones. [`Db::looped`] computes it once
    /// however many ask, so a hint racing the diagnostics on one edit waits
    /// for their loop rather than running a second.
    ///
    /// **A buffer that does not parse reads the last loop.** The memo holds
    /// the loop of the last state of the program that ran one — for an
    /// editor, the last state of the buffer that parsed — and the text the
    /// edits since left alone is the text it typed, at another offset. Text
    /// they made is in no loop, and has no answer until a loop reads it.
    ///
    /// **Under the editor's layered budget** it is the newest answer ready
    /// ([`Db::looped_first`]): the first, at four rounds, while the run goes
    /// on to the fixpoint, and the last once it is there — never waiting
    /// for the fixpoint, never running a second loop (sprint W7.6). A hover
    /// asked while the diagnostics wait for the last answer is answered as
    /// the editor answered before the fixpoint was run at all.
    pub fn editor_loop(&self, roots: &[FileId], file: FileId) -> Option<crate::looped::EditorLoop> {
        if self.before_the_passes(roots, file).iter().any(Diagnostic::is_error) {
            return self.editor_loop_ready(roots);
        }
        Some(crate::looped::EditorLoop::new(self, roots, self.looped_first(roots).0))
    }

    /// [`Db::editor_loop`] for a reader that does not wait: the newest loop
    /// computed for `roots`, mapped onto the text as it stands — the edit's
    /// own once its diagnostics have run it (under a layered budget its
    /// first answer, then its last), the one before while it runs, on a
    /// buffer that does not parse the last one that ran — or `None`, and
    /// there is no type to show. Reads the memo only, as
    /// [`Db::budget_exhausted`] does.
    ///
    /// **Completion reads this.** It is asked on every keystroke, and
    /// waiting for the edit's loop, as hover does, made it the loop's
    /// latency: measured over stdio on `risc-v.k` (2026-10-10), a
    /// completion asked as a clean edit landed took ~2.2 s against ~100 ms
    /// from the CST engine it fell back on then, and ~10 ms once the
    /// diagnostics had run the loop. The loop before the edit, mapped, answers it at once; a hover
    /// or an inlay hint, shown until the next edit, waits for the edit's
    /// own instead (`Db::editor_loop`).
    pub fn editor_loop_ready(&self, roots: &[FileId]) -> Option<crate::looped::EditorLoop> {
        let memo = self.memo.looped.read().unwrap().get(&roots.to_vec()).cloned()?;
        Some(crate::looped::EditorLoop::new(self, roots, memo.value))
    }

    /// [`Db::editor_loop`], and on a buffer with no loop at all — opened
    /// broken, or evicted from the memo — the buffer's own, run once: the
    /// loop over its error-recovered trees (`Exp::Unsupported` where the
    /// recovery left a hole), for the answers alone. Its diagnostics are
    /// never read — [`Db::diagnostics`] stops at the parse error, as the
    /// reference does (`Main.hs:73-75`) — and every later state of the
    /// buffer reads it mapped, as it would the last parsing state's, until
    /// one that parses runs its own.
    ///
    /// **Hover reads this**, being asked for rather than sent on every
    /// keystroke as completion, inlay hints, signature help and document
    /// highlight are: the one loop costs that buffer what a clean keystroke
    /// costs (above), once.
    pub fn editor_loop_or_own(&self, roots: &[FileId], file: FileId) -> crate::looped::EditorLoop {
        self.editor_loop(roots, file)
            .unwrap_or_else(|| crate::looped::EditorLoop::new(self, roots, self.looped_first(roots).0))
    }

    /// The loop over `roots`' program run here, on the caller's thread, to
    /// the budget's last answer — `kg check`'s fixpoint under the editor's
    /// — and memoized nowhere: for a reader that asks about the program
    /// once, as a workspace search does of a file nobody has open
    /// (`kg-lsp`'s `references`). [`Db::looped`] would keep it, one of
    /// [`MAX_INSTANCE_ROOT_SETS`] each pinning every module's final tree,
    /// pushing an open buffer's out; under the layered budget it would
    /// also start the editor's run, which goes on to the fixpoint on a
    /// thread of its own after its reader has gone. What it does keep is
    /// per module, as every loop's is: each module's sorted lowering
    /// (`crate::sorted`). The request's token stops it at a checkpoint,
    /// as the database's does.
    pub fn loop_once(&self, roots: &[FileId]) -> crate::looped::EditorLoop {
        crate::looped::EditorLoop::new(self, roots, Arc::new(crate::looped::looped(self, roots)))
    }

    /// What the reference's `parseProgram` reports (`Parser/IO.hs:67-110`)
    /// for the program rooted at `roots`, in program order: [`Db::check`]'s
    /// first stage — loading, syntax, redefinitions, imports, exposure — its
    /// warnings unfiltered. An error here is one the reference prints
    /// before it enters `frontend` (`Main.hs:73-75`), which is all
    /// `kanagawa list-deps` runs (`Main.hs:111-125`) and what `--file-list`
    /// waits for (`Main.hs:76-79`).
    pub fn parse_stage(&self, roots: &[FileId]) -> Vec<Diagnostic> {
        let program = self.program(roots);
        let out = self.parse_stage_of(roots, &program);
        in_program_order(&program, out)
    }

    fn parse_stage_of(&self, roots: &[FileId], program: &Program) -> Vec<Diagnostic> {
        let mut out = program.diagnostics.clone();
        for &module in &program.files {
            out.extend(self.parse_stage_diagnostics(module));
        }
        out.extend(self.exposure_ambiguities(roots, program));
        out
    }

    /// Load `text` as the contents of `path` and check the program rooted at it.
    ///
    /// This is the whole rooted conformance protocol in one call: the harness
    /// writes each accumulated snippet to
    /// `<subfolder>/syntax_test_temp_file.k` and compiles it with `cwd` set to
    /// that subfolder and `--import-dir <helperdir>`
    /// (`test/syntax/syntax-test.py:44-107`) — so `path` is that temp file,
    /// [`Settings::import_dirs`](kg_config::Settings) is the helper directory,
    /// and the same `Db` is reused across snippets so the base library and the
    /// helper modules are parsed once for the whole run.
    ///
    /// The protocol is the whole run, so it ends where the reference's driver
    /// does: a program the frontend accepts prints its warnings, and under
    /// `--Werror` fails on them — `when (not (null ws) && warnings_as_errors)
    /// $ exitError "Error 1: Warnings treated as errors"` (`Main.hs:93-99`,
    /// `all-warnings-as-errors.k`). Each surviving warning is returned as an
    /// error then, its kind kept: the warnings are that `Error 1`'s reason,
    /// and the first of them is where the reference's output starts. A
    /// program with an error never reaches the warnings (`Main.hs:84-86`), so
    /// nothing changes there. The CLIs print the driver's own line
    /// themselves and call [`Db::check`], which never promotes.
    pub fn check_snippet(&mut self, path: &Utf8Path, text: &str) -> Vec<Diagnostic> {
        let file = self.set_file_text(path, text.to_owned());
        let mut out = self.check(&[file]);
        if self.settings.warnings_as_errors && !out.iter().any(Diagnostic::is_error) {
            for d in out.iter_mut().filter(|d| d.warning.is_some()) {
                d.severity = kg_diag::Severity::Error;
            }
        }
        out
    }

    /// `checkSymbols`' ambiguity (`Parser.hs:219-223`): an exported name
    /// `findSymbol` finds as two definitions is "symbol is ambiguous", and
    /// then — `findSymbol` answering `Nothing` — "undefined symbol", both at
    /// the name, at parse time (`SymbolTable.hs:228-237`;
    /// `library/device/agilex-7-f-023/hardware/dsp.k`, `kg-oracle --errors`
    /// 2026-09-27, where the backend was left to say "Duplicate function").
    ///
    /// The candidates are the module graph's
    /// ([`kg_resolve::ModuleGraph::exposed_ambiguities`]); whether two are
    /// two is `nub`'s question, which compares the declarations' trees with
    /// notes ignored (`Parser/Syntax.hs:594-595`): the device files that
    /// declare `fadd32` exactly as `mock`'s does are one definition and
    /// compile (`arria-10-gx-1150`). That is the sorted lowering's
    /// position-free [`kg_hir::sort::fingerprint`], as the sorter's own
    /// `nub` reads it. A definition with no fingerprint counts as the same
    /// as the rest — silence. The sorted program is lowered only when some
    /// export has more than one definition.
    fn exposure_ambiguities(&self, roots: &[FileId], program: &Program) -> Vec<Diagnostic> {
        let candidates: Vec<_> = program.files.iter().flat_map(|&m| program.graph.exposed_ambiguities(m)).collect();
        if candidates.is_empty() {
            return Vec::new();
        }
        let sorted = crate::sorted::sorted_program(self, roots);
        let mut harvests: FxHashMap<ModuleSource, FxHashMap<SymbolPath, kg_hir::sort::Harvested>> = FxHashMap::default();
        let mut fingerprint = |def: kg_resolve::DefId| -> Option<u64> {
            let tree = self.item_tree(def.module);
            let name = tree.items.get(def.item.index())?.qualified_name.clone();
            let harvest = match harvests.entry(def.module) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(kg_hir::sort::harvest_shapes(sorted.ir(def.module)?, &self.interner))
                }
            };
            harvest.get(&name).map(|h| h.fingerprint.get())
        };
        // `symbolLoc`: the declaring file as the parser was given it, and the
        // declaration's line and column.
        let loc = |def: kg_resolve::DefId| -> Option<String> {
            let tree = self.item_tree(def.module);
            let item = tree.items.get(def.item.index())?;
            let name = tree.display_name.clone()?;
            let text = self.parse_module(def.module).text();
            let pos = kg_base::LineIndex::new(&text).line_col(item.decl.range.start());
            Some(format!("{name}:{}:{}", pos.line + 1, pos.col + 1))
        };
        let mut out = Vec::new();
        for (at, name, hits) in candidates {
            let Some(prints) = hits.iter().map(|&d| fingerprint(d)).collect::<Option<Vec<u64>>>() else { continue };
            // `nub`, first occurrence kept.
            let mut distinct: Vec<kg_resolve::DefId> = Vec::new();
            let mut seen: Vec<u64> = Vec::new();
            for (d, f) in hits.iter().zip(prints) {
                if !seen.contains(&f) {
                    seen.push(f);
                    distinct.push(*d);
                }
            }
            if distinct.len() < 2 {
                continue;
            }
            let Some(locations) = distinct.iter().map(|&d| loc(d)).collect::<Option<Vec<String>>>() else { continue };
            let name = self.interner.resolve(name);
            out.push(Diagnostic::error(at, format!("`{name}`: undefined symbol")));
            out.push(Diagnostic::error(
                at,
                format!(
                    "`{name}`: symbol is ambiguous. It could refer to the definition at\n\t   {}",
                    locations.join("\n\tor ")
                ),
            ));
        }
        out
    }

    /// Syntax errors plus the parse-time symbol table's own hard errors, for one
    /// module.
    ///
    /// The second half is not a frontend pass: the reference builds the symbol
    /// table *during* parsing (`Parser.hs:108-241`) and reports a redefinition
    /// immediately (`SymbolTable.hs:200-201`), long before `frontend` is entered
    /// (`Main.hs:70-76`).
    fn parse_stage_diagnostics(&self, module: ModuleSource) -> Vec<Diagnostic> {
        let file = module.diagnostic_file();
        let mut out = self.parse_module(module).diagnostics(file);
        out.extend(self.item_tree(module).diagnostics.iter().cloned());
        out
    }

    /// The wired frontend passes over one module: the parser's own checks
    /// and pass 1 at its text, and the loop's verdicts admitted in every
    /// later pass's place (`crate::passes`). What any of them reads of a
    /// type or a constant is the loop's (`elab`); there is no other
    /// environment since the CST inference engine and its one-file
    /// constant environment were deleted (sprint W3.7).
    fn frontend_diagnostics(
        &self,
        roots: &[FileId],
        module: ModuleSource,
        elab: Elab<'_>,
        erred: &dyn Fn() -> bool,
    ) -> CstRun {
        let _miss = self.stats.missed(Query::CstPasses);
        kg_base::cancel::scope(&self.cancel, || self.frontend_diagnostics_in(roots, module, elab, erred))
    }

    /// What every module's CST run reads of the whole program, built once
    /// for a run of several ([`Db::program_erred_through_pass_9`]).
    fn cst_program(&self, roots: &[FileId], elab: Elab<'_>) -> CstProgram {
        // The rooted door runs one pass `check_source` structurally cannot:
        // pass 9 rejects a name absent from the whole-program symbol map, which
        // needs the imported modules to exist. `AllSymbols` because
        // `withSymbols` is `getSymbols` is `getSymbols' AllSymbols`
        // (`Frontend.hs:40,71`).
        let program = self.program(roots);
        // `getSymbols` folds the whole file list (`Symbols.hs:37-41`), so pass 4
        // may name a template any module declares — which is the only way a
        // corpus snippet's call into a `helper/` module can be deduced at all.
        let parses: Vec<_> = program.files.iter().map(|&m| self.parse_module(m)).collect();
        let roots_of: Vec<_> = parses.iter().map(|p| p.source_file()).collect();
        let templates = crate::passes::templates::Templates::of(
            program.files.iter().zip(&roots_of).map(|(&m, r)| (m.diagnostic_file(), r)),
            &self.interner,
        )
        .with_visible(visible_through_imports(&program));
        CstProgram {
            looped: elab.looped(self, roots),
            symbols: self.global_symbols(roots, SymbolsConfig::AllSymbols),
            program,
            roots_of,
            templates,
        }
    }

    /// What one module's CST run reads — its tree and the whole program's
    /// view ([`crate::passes::undefined_symbols::Whole`]) — handed to `run`,
    /// with the loop `elab` names.
    fn with_cst_inputs<R>(
        &self,
        roots: &[FileId],
        module: ModuleSource,
        elab: Elab<'_>,
        run: impl FnOnce(&kg_syntax::ast::SourceFile, crate::passes::undefined_symbols::Whole<'_>, &Arc<crate::looped::Looped>) -> R,
    ) -> R {
        let cx = self.cst_program(roots, elab);
        self.with_cst_module(&cx, module, run)
    }

    /// [`Db::with_cst_inputs`] over a program's context already built.
    fn with_cst_module<R>(
        &self,
        cx: &CstProgram,
        module: ModuleSource,
        run: impl FnOnce(&kg_syntax::ast::SourceFile, crate::passes::undefined_symbols::Whole<'_>, &Arc<crate::looped::Looped>) -> R,
    ) -> R {
        let parse = self.parse_module(module);
        let root = parse.source_file();
        let parsed = cx.looped.parsed(module).expect("the loop lowered every module of its program");
        let whole = crate::passes::undefined_symbols::Whole {
            module,
            file: module.diagnostic_file(),
            program: &cx.program,
            symbols: &cx.symbols,
            templates: &cx.templates,
            parsed,
            looped: &cx.looped,
            interner: &self.interner,
            roots: &cx.roots_of,
        };
        run(&root, whole, &cx.looped)
    }

    /// [`Db::frontend_diagnostics`], inside the database's token.
    fn frontend_diagnostics_in(
        &self,
        roots: &[FileId],
        module: ModuleSource,
        elab: Elab<'_>,
        erred: &dyn Fn() -> bool,
    ) -> CstRun {
        self.with_cst_inputs(roots, module, elab, |root, whole, looped| {
            // The other half of the seam: what the passes after instantiation
            // raised inside the copies — the loop's own (`kg_sem::check`, step
            // M4), reported at the template body with each instantiation as a
            // `while compiling` note — and the census's refusals. The run
            // at the text skips every template body (`facts::reached_only_after_elaboration`),
            // so nothing here is a second report of one of its own. Each
            // enters the run's sink at the pass it is the reference's
            // (`passes::Carried`): `errorMsg` leaves an `ErrorF` where it
            // judged (`Error.hs:147-148`), which the checkpoint after that pass
            // reads and pass 12's and pass 16's poisoning do, and none closes
            // a checkpoint before its own pass. What the run never reached it
            // does not show.
            let staged = looped.diagnostics_for(module).to_vec();
            let from_loop = crate::passes::FromLoop::of(looped, module, erred);
            let through =
                crate::passes::undefined_symbols::run_through_9(root, whole, crate::passes::Carried::of(&staged), &from_loop);
            let erred = through.erred();
            let (all, carried) = match through {
                crate::passes::undefined_symbols::Through9::Rejected(out) => (out, Vec::new()),
                crate::passes::undefined_symbols::Through9::Early(early) => {
                    let carried = early.carried.clone();
                    let all = crate::passes::run_from_10(*early, &from_loop);
                    (all, carried)
                }
            };
            let diagnostics = if carried.is_empty() {
                all
            } else {
                // A framed diagnostic replaces the base run's identical one — the
                // frame is what the base could not see; a seam diagnostic (no
                // `while compiling` frame) yields to whatever the base run said
                // at the same node, frames and all.
                let same =
                    |a: &Diagnostic, b: &Diagnostic| a.range == b.range && a.message == b.message && a.code == b.code;
                let mut out: Vec<Diagnostic> =
                    all.into_iter().filter(|d| !carried.iter().any(|c| !c.labels.is_empty() && same(c, d))).collect();
                for d in &carried {
                    let seam = d.labels.is_empty();
                    if seam && out.iter().any(|x| x.range.range.start() == d.range.range.start()) {
                        continue;
                    }
                    out.push(d.clone());
                }
                out.sort_by_key(|x| x.range.range.start());
                out.dedup_by(|a, b| a.range == b.range && a.message == b.message);
                out
            };
            CstRun { diagnostics, erred }
        })
    }
}

/// Which loop a module's CST run reads ([`Db::frontend_diagnostics`]).
#[derive(Clone, Copy)]
enum Elab<'a> {
    /// [`Db::looped`]'s, the loop's last answer.
    Last,
    /// This one: the editor's first answer ([`Db::diagnostics_layered`]).
    At(&'a Arc<crate::looped::Looped>),
}

impl Elab<'_> {
    /// The loop itself.
    fn looped(self, db: &Db, roots: &[FileId]) -> Arc<crate::looped::Looped> {
        match self {
            Elab::Last => db.looped(roots),
            Elab::At(looped) => looped.clone(),
        }
    }
}

/// What every module's CST run reads of the whole program
/// ([`Db::cst_program`]).
struct CstProgram {
    program: Arc<Program>,
    roots_of: Vec<kg_syntax::ast::SourceFile>,
    templates: crate::passes::templates::Templates,
    /// The program's loop: what the parser built of each module, and what
    /// the loop decided at its text and in its copies.
    looped: Arc<crate::looped::Looped>,
    symbols: Arc<SymbolMap>,
}

/// The runs a [`Db::check`] has made, by module: whether each erred through
/// pass 9 (`Db::program_erred_through_pass_9`).
type KnownErred = std::cell::RefCell<FxHashMap<ModuleSource, bool>>;

/// One module's CST run ([`Db::frontend_diagnostics`]).
struct CstRun {
    diagnostics: Vec<Diagnostic>,
    /// Whether the module erred through pass 9 — its own text, or the
    /// loop's verdicts in its copies through pass 9, which are part of the
    /// program's flag anyway (`Looped::erred_through_pass_9`).
    erred: bool,
}

/// Per file, the module-level names its unaliased imports make visible and
/// the file each is declared in — read off every import target's
/// `visibleSymbols` (`SymbolTable.hs:135-181`), keys `[namespace, name]`.
/// See `Templates::with_visible`.
pub(crate) fn visible_through_imports(
    program: &crate::program::Program,
) -> FxHashMap<FileId, FxHashMap<Symbol, FileId>> {
    let graph = &program.graph;
    let mut out: FxHashMap<FileId, FxHashMap<Symbol, FileId>> = FxHashMap::default();
    for &module in &program.files {
        let names = out.entry(module.diagnostic_file()).or_default();
        for (decl, target) in graph.resolved_imports(module) {
            if decl.alias.is_some() {
                continue;
            }
            let Some(interface) = graph.interface(target) else { continue };
            for symbols in interface.visible.values() {
                for (key, def) in symbols {
                    let segments = key.segments();
                    if segments.len() != 2 {
                        continue;
                    }
                    names.insert(segments[1], def.module.diagnostic_file());
                }
            }
        }
    }
    out
}
