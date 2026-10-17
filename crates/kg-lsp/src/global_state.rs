//! Everything the server knows, and the only thing that mutates.
//!
//! The shape is rust-analyzer's, shrunk: a [`Vfs`] whose overlays *are* the
//! `didOpen`/`didChange`/`didClose` lifecycle, a [`kg_config::Settings`] per
//! discovered `kanagawa.toml`, and a parse cache keyed by the text it parsed.
//!
//! Every field is behind an `Arc`, so a `GlobalState` is a cheap handle that a
//! worker thread can hold while the main loop keeps reading messages. Request
//! handlers take `&GlobalState` and never mutate it, which is what makes them
//! callable straight from a unit test with no server running.
//!
//! **The main loop never waits on a query.** Every lock it takes is held by
//! anyone only for a map lookup, an insert or a push. The one lock held for the
//! length of a query — a project's `RwLock<Db>` — is never taken by the main
//! loop: it queues its news for the database instead, and the next worker to
//! query applies the queue under a brief write lock first (`Project`).

use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

use camino::{Utf8Path, Utf8PathBuf};
use kg_base::{FileId, FxHashMap};
use kg_config::Settings;
use kg_db::Db;
use kg_syntax::{Parse, parse_file};
use kg_vfs::{FileText, Vfs};
use lsp_types::Uri;

use crate::convert::PositionEncoding;
use crate::sync::{lock, read, write};
use crate::uri::uri_to_key;

/// One open editor buffer.
#[derive(Clone, Debug)]
pub struct Document {
    pub file: FileId,
    pub uri: Uri,
    /// The client's version, echoed back on `publishDiagnostics` so a stale
    /// result is discardable by the client.
    pub version: i32,
}

/// A parse plus the text it was produced from, which is what every handler
/// actually needs.
#[derive(Clone, Debug)]
pub struct FileSnapshot {
    pub file: FileId,
    /// The client's version *of this text* — read together with it, so a
    /// diagnostic published from the snapshot names the version it describes.
    pub version: i32,
    pub text: FileText,
    pub parse: Arc<Parse>,
    pub settings: Arc<Settings>,
}

impl FileSnapshot {
    pub fn line_index(&self) -> &kg_base::LineIndex {
        &self.text.line_index
    }
}

/// Why a file has no snapshot.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum NoFile {
    /// The client never opened it (or already closed it).
    NotOpen,
    /// Bigger than `[lsp] max-file-size-kb`. Answering would be worse than
    /// declining: a multi-megabyte generated file freezes the editor.
    TooLarge,
}

#[derive(Debug)]
struct Inner {
    vfs: Arc<Vfs>,
    /// Open documents. Also the lock that makes a buffer's text and its
    /// version one fact: the main loop writes both under its write lock, and
    /// a snapshot reads both under its read lock. Written in two steps, a
    /// worker between them labelled one version's syntax errors with the
    /// previous version's number — seen over stdio, not argued.
    docs: RwLock<FxHashMap<FileId, Document>>,
    /// The last parse of each open document, with the line index of the text
    /// it was parsed from — which is the key. `FileText::new` mints a fresh
    /// `Arc<LineIndex>` per write, so pointer identity pins the parse to the
    /// exact text a snapshot read. A revision counter kept beside the overlay
    /// could not: the main loop writes the text and bumps the counter in two
    /// steps, and a worker reading between them would pair a new text with the
    /// old parse.
    parses: Mutex<FxHashMap<FileId, CachedParse>>,
    config: Mutex<ConfigStore>,
    encoding: RwLock<PositionEncoding>,
    /// One query database per project root, sharing this server's [`Vfs`] so an
    /// unsaved buffer is what the frontend sees.
    ///
    /// Keyed by `Settings::root` — the directory holding `kanagawa.toml` —
    /// because that is the unit a program is resolved against: two files under
    /// different manifests have different import paths and cannot share a memo.
    /// A single-root workspace, which is nearly all of them, has exactly one.
    ///
    /// The mutex guards the map only, and is held for a lookup or an insert;
    /// a query holds its own project's `Arc`, never this lock.
    dbs: Mutex<FxHashMap<Utf8PathBuf, Arc<Project>>>,
    /// The search the last `prepareRename` made, for the `rename` after it
    /// (`handlers::rename::Remembered`).
    remembered: Mutex<Option<crate::handlers::rename::Remembered>>,
}

/// A parse, and the line index of the text it was parsed from.
type CachedParse = (Arc<kg_base::LineIndex>, Arc<Parse>);

/// One project's database, and the news it has not yet been told.
#[derive(Debug)]
struct Project {
    /// The settings the database was built from, compared by identity on
    /// every use. `ConfigStore` hands out one `Arc` per manifest until a
    /// reload, so a different `Arc` means the manifest was re-read and this
    /// database answers for a search path that may no longer exist.
    settings: Arc<Settings>,
    /// Read-locked for the length of a query; `Db` is `Sync`, so any number
    /// run at once. Write-locked only to apply `pending`, which is brief.
    db: RwLock<Db>,
    /// `(file, exists)` per change, in the order the main loop made them —
    /// the order `Db::file_changed` needs. Pushed by the main loop, drained
    /// by a worker under `db`'s write lock.
    pending: Mutex<Vec<(FileId, bool)>>,
    /// The database's cancellation token (`Db::cancel_token`), raised by the
    /// main loop as it queues news: a query running against the text the
    /// news supersedes stops at its next checkpoint and lets the write in
    /// (sprint W7.2). Applying the news lowers it.
    cancel: kg_base::cancel::Cancel,
}

impl Project {
    /// The database, having seen every change queued before this call.
    ///
    /// Sound against a concurrent drain because the drain happens only under
    /// the write lock: finding the queue empty means either nothing was
    /// queued, or another worker took it and is applying it now — and then
    /// the `read` below waits for that worker to finish.
    fn caught_up(&self) -> RwLockReadGuard<'_, Db> {
        if !lock(&self.pending).is_empty() {
            let mut db = write(&self.db);
            let changes = std::mem::take(&mut *lock(&self.pending));
            for (file, exists) in changes {
                db.file_changed(file, exists);
            }
        }
        read(&self.db)
    }
}

/// Drops a project whose query panicked, so the next request builds a fresh
/// database instead of inheriting whatever the unwind left behind.
///
/// `Db`'s own memo locks unwrap their guards; a panic while one was held would
/// poison it, and every later query on that database would panic in turn. The
/// cost of eviction is one reload of the program; the cost of keeping it could
/// be a project that never answers again.
struct EvictOnPanic<'a> {
    dbs: &'a Mutex<FxHashMap<Utf8PathBuf, Arc<Project>>>,
    project: &'a Arc<Project>,
}

impl Drop for EvictOnPanic<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut dbs = lock(self.dbs);
        let root = &self.project.settings.root;
        // Only if it is still the one in the map: a reload may already have
        // replaced it, and the replacement did not panic.
        if dbs.get(root).is_some_and(|p| Arc::ptr_eq(p, self.project)) {
            dbs.remove(root);
        }
    }
}

/// The server's state. Cloning is a refcount bump.
#[derive(Clone, Debug)]
pub struct GlobalState {
    inner: Arc<Inner>,
}

impl Default for GlobalState {
    fn default() -> Self {
        GlobalState::new()
    }
}

impl GlobalState {
    pub fn new() -> GlobalState {
        GlobalState {
            inner: Arc::new(Inner {
                vfs: Arc::new(Vfs::new()),
                docs: RwLock::new(FxHashMap::default()),
                parses: Mutex::new(FxHashMap::default()),
                config: Mutex::new(ConfigStore::new()),
                encoding: RwLock::new(PositionEncoding::Utf16),
                dbs: Mutex::new(FxHashMap::default()),
                remembered: Mutex::new(None),
            }),
        }
    }

    /// The slot a `prepareRename` keeps its search in for the `rename` after
    /// it (`handlers::rename::Remembered`).
    pub(crate) fn remembered(&self) -> &Mutex<Option<crate::handlers::rename::Remembered>> {
        &self.inner.remembered
    }

    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.inner.vfs
    }

    pub fn encoding(&self) -> PositionEncoding {
        *read(&self.inner.encoding)
    }

    pub fn set_encoding(&self, enc: PositionEncoding) {
        *write(&self.inner.encoding) = enc;
    }

    /// Records the workspace folders `initialize` reported, so manifest
    /// discovery has somewhere to start for a buffer that is not on disk.
    pub fn set_roots(&self, roots: Vec<Utf8PathBuf>) {
        lock(&self.inner.config).set_roots(roots);
    }

    /// Forgets every loaded manifest, e.g. on `workspace/didChangeConfiguration`
    /// or when `kanagawa.toml` itself is saved — and every database built from
    /// one, and every disk read.
    ///
    /// The databases go because their settings are an input they were built
    /// with: a changed `import-dirs` changes what every import resolves to,
    /// and a database kept across the change answers for the old search path.
    /// Dropping them is also what makes forgetting the disk reads sound
    /// without telling a database: none is left holding a memo of them. A
    /// query still running holds its own `Arc<Project>` and finishes on the old
    /// database; nothing new is asked of it.
    ///
    /// The order matters. A worker may build a database between any two of
    /// these steps; one built before the map is emptied is dropped with the
    /// rest, and one built after it reads a disk already forgotten. A worker
    /// still holding settings from before the reload may re-insert a database
    /// built from them, which `GlobalState::project` replaces on the next use
    /// with the new ones.
    pub fn reload_config(&self) {
        lock(&self.inner.config).clear();
        self.inner.vfs.invalidate_all();
        // Freed outside the map's lock: a database's memos take a moment to
        // drop, and workers look projects up under that lock.
        let dropped = std::mem::take(&mut *lock(&self.inner.dbs));
        drop(dropped);
    }

    /// The resolved settings governing `file`.
    pub fn settings_for(&self, file: FileId) -> Arc<Settings> {
        let path = self.inner.vfs.path(file);
        lock(&self.inner.config).settings_for(path.as_deref())
    }

    /// Manifests that failed to load since the last call, for reporting to the
    /// user exactly once each.
    pub fn take_config_errors(&self) -> Vec<String> {
        lock(&self.inner.config).take_errors()
    }

    // ------------------------------------------------------------- lifecycle

    /// `textDocument/didOpen`.
    pub fn did_open(&self, uri: Uri, text: String, version: i32) -> FileId {
        let file = self.intern(&uri);
        {
            let mut docs = write(&self.inner.docs);
            self.inner.vfs.set_overlay(file, text);
            docs.insert(file, Document { file, uri, version });
        }
        // The buffer need not match the disk (a restored unsaved buffer), and
        // the file need not exist there at all (a new one) — either way a
        // database that already read this path has a stale memo of it.
        self.invalidate_everywhere(file);
        file
    }

    /// Replaces a document wholesale — full sync, and the fallback for an
    /// incremental change event with no range.
    pub fn did_change_full(&self, uri: &Uri, text: String, version: i32) -> FileId {
        let file = self.intern(uri);
        {
            let mut docs = write(&self.inner.docs);
            self.inner.vfs.set_overlay(file, text);
            let doc =
                docs.entry(file).or_insert_with(|| Document { file, uri: uri.clone(), version });
            doc.version = version;
        }
        // The databases still hold memoized answers computed from the previous
        // text. Telling them the file moved is what turns an edit into a
        // reparse of that file and a re-check of whatever depended on it — and
        // *only* that.
        self.invalidate_everywhere(file);
        file
    }

    /// The text of `file` with `edit` applied, or `None` if it is not open.
    ///
    /// Split out from [`GlobalState::did_change_full`] so that incremental
    /// splicing is testable without a server: it is the one place where a
    /// UTF-16 column becomes a byte index into a `String`.
    pub fn apply_edit(
        &self,
        file: FileId,
        range: Option<lsp_types::Range>,
        new_text: &str,
    ) -> Option<String> {
        let current = self.inner.vfs.read(file)?;
        let Some(range) = range else { return Some(new_text.to_owned()) };

        let enc = self.encoding();
        let span = crate::convert::text_range(&current.line_index, enc, range);
        let start = crate::convert::snap_to_boundary(&current.text, span.start());
        let end = crate::convert::snap_to_boundary(&current.text, span.end());

        let mut out = String::with_capacity(current.text.len() + new_text.len());
        out.push_str(&current.text[..usize::from(start)]);
        out.push_str(new_text);
        out.push_str(&current.text[usize::from(end)..]);
        Some(out)
    }

    /// `textDocument/didClose`: the buffer reverts to whatever is on disk.
    pub fn did_close(&self, uri: &Uri) -> Option<FileId> {
        let file = self.file_id(uri)?;
        {
            let mut docs = write(&self.inner.docs);
            self.inner.vfs.remove_overlay(file);
            docs.remove(&file);
        }
        lock(&self.inner.parses).remove(&file);
        // `didClose` reverts the file to whatever is on disk (`Vfs::remove_overlay`
        // via the caller), so every memo computed from the buffer is stale.
        self.invalidate_everywhere(file);
        Some(file)
    }

    /// A watcher's news that `uri` changed on disk — created, edited or
    /// deleted: a branch switched under the editor, a pull, another editor's
    /// save. Every database is told and the disk cache dropped, so whatever
    /// imports the file reads it again; an open buffer keeps its text, the
    /// overlay winning over the disk as always.
    pub fn disk_changed(&self, uri: &Uri) -> FileId {
        let file = self.intern(uri);
        self.invalidate_everywhere(file);
        file
    }

    /// Diagnostics for one open file, from the whole program rooted at it:
    /// the answer at the loop's fixpoint, `kg check`'s, with the one at its
    /// fourth round handed to `first` on the way where the two differ
    /// (`kg_db::Db::diagnostics_layered`, sprint W7.6).
    ///
    /// The file is its own root: an editor is looking at one buffer, and
    /// `Db::diagnostics` applies the parse gate per file rather than
    /// program-wide, so an error in `b.k` does not blank out `a.k`'s report.
    ///
    /// Returns `None` for a file the server does not have open, or one whose
    /// project has no database yet — never an empty vector, which would clear
    /// the editor's gutter as if the file were clean.
    pub fn frontend_diagnostics(
        &self,
        file: FileId,
        first: impl FnOnce(Vec<kg_diag::Diagnostic>),
    ) -> Option<Vec<kg_diag::Diagnostic>> {
        self.with_db(file, |db| {
            let diagnostics = db.diagnostics_layered(&[file], file, first);
            // The fixpoint is the reference's own; a loop that never reaches
            // it is a bug here, which `kg check` reports as well and the
            // gutter must not hide (`kg_db::PassBudget::Fixpoint`).
            if let Some(passes) = db.budget_exhausted(&[file]) {
                crate::log(
                    lsp_types::MessageType::WARNING,
                    format!(
                        "{}: template instantiation stopped after {passes} program passes without \
                         converging; instances it had not decided are missing from the diagnostics",
                        self.inner.vfs.path(file).map(|p| p.to_string()).unwrap_or_default(),
                    ),
                );
            }
            diagnostics
        })
    }

    /// Runs `query` against the database governing `file`, creating it on first
    /// use.
    ///
    /// The one way into [`Db`] from a handler, so that every semantic feature
    /// answers from the *same* database as the diagnostics do: a jump computed
    /// against a second, separately loaded program could disagree with the
    /// gutter about what is in scope.
    ///
    /// `None` when the server does not have `file` open — a request naming a
    /// document nobody opened must not silently load a program for it.
    ///
    /// The database has seen every change the main loop made before this call
    /// (`Project::caught_up`), so a request is never answered from text older
    /// than the request. It may see *newer* text: the `Vfs` is shared and
    /// the main loop does not wait, which is the direction that is harmless.
    ///
    /// Must not be re-entered from inside `query`: the read lock it holds
    /// would deadlock against a worker waiting to apply changes.
    ///
    /// A query cancelled at a checkpoint (`kg_base::cancel`) goes on
    /// unwinding with `Cancelled` to the pool, which answers it as
    /// cancelled — but not through the eviction guard: a cancellation is
    /// not a bug, no memo lock is held at any checkpoint, and the database
    /// it leaves is sound.
    pub(crate) fn with_db<T>(&self, file: FileId, query: impl FnOnce(&Db) -> T) -> Option<T> {
        self.document(file)?;
        let settings = self.settings_for(file);
        let project = self.project(&settings);
        let db = project.caught_up();
        let evict = EvictOnPanic { dbs: &self.inner.dbs, project: &project };
        let answer = kg_base::cancel::catch(|| query(&db));
        drop(evict);
        drop(db);
        match answer {
            Ok(value) => Some(value),
            Err(cancelled) => std::panic::resume_unwind(Box::new(cancelled)),
        }
    }

    /// The project answering for `settings`, created — or replaced, if it was
    /// built from other settings — on first use.
    fn project(&self, settings: &Arc<Settings>) -> Arc<Project> {
        let mut dbs = lock(&self.inner.dbs);
        if let Some(project) = dbs.get(&settings.root)
            && Arc::ptr_eq(&project.settings, settings)
        {
            return Arc::clone(project);
        }
        // Cheap: a database loads nothing until it is asked something.
        let mut db = Db::with_vfs(Arc::clone(&self.inner.vfs), (**settings).clone());
        // A keystroke pays for every program pass: the editor answers at a
        // measured bound, then at the fixpoint a batch compile reaches.
        db.set_pass_budget(kg_db::PassBudget::EDITOR);
        let project = Arc::new(Project {
            settings: Arc::clone(settings),
            cancel: db.cancel_token(),
            db: RwLock::new(db),
            pending: Mutex::new(Vec::new()),
        });
        dbs.insert(settings.root.clone(), Arc::clone(&project));
        project
    }

    /// Tells every database that `file` changed, without waiting for any.
    ///
    /// Every database, not just the one for this file's project: a header under
    /// `library/` is reachable from several roots at once, and a stale memo in
    /// the root nobody is editing is exactly the one nobody would notice.
    ///
    /// The existence is observed *here*, straight after the write, because
    /// the database applies the news later and by then the `Vfs` may have
    /// moved on (`Db::file_changed` says why that matters).
    fn invalidate_everywhere(&self, file: FileId) {
        let vfs = &self.inner.vfs;
        let exists = vfs.has_overlay(file) || {
            // No buffer: the disk decides, and a cached read of it may be
            // older than the event that brought us here.
            vfs.invalidate(file);
            vfs.read(file).is_some()
        };
        for project in lock(&self.inner.dbs).values() {
            lock(&project.pending).push((file, exists));
            project.cancel.raise();
        }
    }

    // ---------------------------------------------------------------- lookup

    pub fn intern(&self, uri: &Uri) -> FileId {
        self.inner.vfs.intern(&uri_to_key(uri))
    }

    /// The id of an *already interned* file. Deliberately does not intern, so
    /// that a request naming a document the server never saw is a clean
    /// "unknown file" instead of a silently empty one.
    pub fn file_id(&self, uri: &Uri) -> Option<FileId> {
        let key = uri_to_key(uri);
        let id = self.inner.vfs.intern(&key);
        read(&self.inner.docs).contains_key(&id).then_some(id)
    }

    pub fn document(&self, file: FileId) -> Option<Document> {
        read(&self.inner.docs).get(&file).cloned()
    }

    pub fn open_documents(&self) -> Vec<Document> {
        read(&self.inner.docs).values().cloned().collect()
    }

    pub fn uri_of(&self, file: FileId) -> Option<Uri> {
        read(&self.inner.docs).get(&file).map(|d| d.uri.clone())
    }

    /// The parsed form of an open document.
    ///
    /// Cached per text (`Inner::parses`): a request storm on one keystroke —
    /// diagnostics, semantic tokens, folding ranges and document symbols all
    /// arrive together — parses once.
    pub fn snapshot(&self, uri: &Uri) -> Result<FileSnapshot, NoFile> {
        let file = self.file_id(uri).ok_or(NoFile::NotOpen)?;
        self.snapshot_of(file)
    }

    pub fn snapshot_of(&self, file: FileId) -> Result<FileSnapshot, NoFile> {
        let (version, text) = {
            let docs = read(&self.inner.docs);
            let doc = docs.get(&file).ok_or(NoFile::NotOpen)?;
            (doc.version, self.inner.vfs.read(file).ok_or(NoFile::NotOpen)?)
        };
        let settings = self.settings_for(file);

        if text.text.len() as u64 > settings.lsp.max_file_size_kb.saturating_mul(1024) {
            return Err(NoFile::TooLarge);
        }

        let cached = lock(&self.inner.parses).get(&file).cloned();
        let parse = match cached {
            Some((index, parse)) if Arc::ptr_eq(&index, &text.line_index) => parse,
            // Parsed outside the lock: two workers racing on one new text
            // parse it twice, where holding the lock would make every other
            // document's snapshot wait for this one's parse.
            _ => {
                let parse = Arc::new(parse_file(&text.text));
                lock(&self.inner.parses)
                    .insert(file, (Arc::clone(&text.line_index), Arc::clone(&parse)));
                parse
            }
        };

        Ok(FileSnapshot { file, version, text, parse, settings })
    }
}

/// `kanagawa.toml` discovery, cached by manifest path.
///
/// Risk #13 of the project plan is an LSP with no config to read: without
/// `import-dirs` and `target-device` the import search path is empty and the
/// standard library does not resolve. So the server discovers the manifest for
/// every file it is asked about, exactly as `kg build` would.
#[derive(Debug)]
struct ConfigStore {
    roots: Vec<Utf8PathBuf>,
    by_manifest: FxHashMap<Utf8PathBuf, Arc<Settings>>,
    /// Manifests whose load failed, and the message, pending delivery to the
    /// user. Reported once each: a broken manifest that re-reports on every
    /// keystroke is worse than the original problem.
    errors: Vec<String>,
    reported: Vec<Utf8PathBuf>,
    fallback: Option<Arc<Settings>>,
    /// Whether a file with no manifest above it has been reported: its
    /// settings have no import directories, so the base library is not
    /// loaded and an `import` finds nothing, which the user is told once
    /// rather than left to infer from every name the gutter rejects.
    said_unmanaged: bool,
    /// The port's verdict switches (`kg_config::Verdicts`), read from the
    /// environment once, when the server starts — `KG_LEFTOVERS`,
    /// `KG_INFER_FULL`, `KG_PTBRIDGE_SEED`, as `kg` reads
    /// them — and set on every project's settings.
    verdicts: kg_config::Verdicts,
    /// `KG_PLATFORM`, read once, as `kg` reads it: where a manifest's
    /// `[board]` is looked for when it names no platform.
    platform: Option<Utf8PathBuf>,
}

impl ConfigStore {
    fn new() -> ConfigStore {
        ConfigStore {
            roots: Vec::new(),
            by_manifest: FxHashMap::default(),
            errors: Vec::new(),
            reported: Vec::new(),
            fallback: None,
            said_unmanaged: false,
            verdicts: kg_config::Verdicts::read(|name| std::env::var_os(name)),
            platform: std::env::var("KG_PLATFORM").ok().filter(|p| !p.is_empty()).map(Utf8PathBuf::from),
        }
    }

    fn set_roots(&mut self, roots: Vec<Utf8PathBuf>) {
        self.roots = roots;
        self.clear();
    }

    fn clear(&mut self) {
        self.by_manifest.clear();
        self.reported.clear();
        self.fallback = None;
    }

    fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    fn settings_for(&mut self, path: Option<&Utf8Path>) -> Arc<Settings> {
        let start = path
            .filter(|p| p.is_absolute())
            .map(|p| p.to_owned())
            .or_else(|| self.roots.first().cloned());

        let manifest = start.as_deref().and_then(kg_config::discover);
        let Some(manifest) = manifest else {
            if Settings::bundled_library().is_none() && !std::mem::replace(&mut self.said_unmanaged, true) {
                let above = start.as_deref().and_then(Utf8Path::parent).map(|d| d.to_string()).unwrap_or_default();
                self.errors.push(format!(
                    "none found above `{above}`, so it is checked with no import directories: the base library \
                     is not loaded and an `import` resolves nothing. A `kanagawa.toml` with \
                     `[project] import-dirs = [\"path/to/library\"]` in this directory or above it fixes that."
                ));
            }
            return self.fallback(start.as_deref());
        };

        if let Some(cached) = self.by_manifest.get(&manifest) {
            return Arc::clone(cached);
        }
        match kg_config::load(&manifest) {
            Ok(m) => {
                let mut settings = Settings { verdicts: self.verdicts, ..Settings::from_manifest(&m, &manifest).or_bundled_library() };
                // The board's device and the platform's library, as `kg`
                // applies them; a board not found is said, and the rest of
                // the manifest still holds.
                if let Some(table) = &m.board {
                    let root = manifest.parent().unwrap_or(Utf8Path::new("."));
                    let applied = kg_config::board::resolve(table, root, self.platform.as_deref())
                        .and_then(|board| kg_config::board::apply(&mut settings, &board));
                    if let Err(err) = applied
                        && !self.reported.contains(&manifest)
                    {
                        self.reported.push(manifest.clone());
                        self.errors.push(format!("{manifest}: {err}"));
                    }
                }
                let settings = Arc::new(settings);
                self.by_manifest.insert(manifest, Arc::clone(&settings));
                settings
            }
            Err(err) => {
                if !self.reported.contains(&manifest) {
                    self.reported.push(manifest.clone());
                    self.errors.push(format!("{err}"));
                }
                self.fallback(start.as_deref())
            }
        }
    }

    /// Defaults for a tree with no (usable) manifest, so a bare checkout still
    /// gets diagnostics and highlighting.
    fn fallback(&mut self, near: Option<&Utf8Path>) -> Arc<Settings> {
        if let Some(cached) = &self.fallback {
            return Arc::clone(cached);
        }
        let root = near
            .and_then(|p| p.parent())
            .map(|p| p.to_owned())
            .or_else(|| self.roots.first().cloned())
            .unwrap_or_else(|| Utf8PathBuf::from("."));
        let settings = Arc::new(Settings { verdicts: self.verdicts, ..Settings::fallback(root).or_bundled_library() });
        self.fallback = Some(Arc::clone(&settings));
        settings
    }
}

/// Interns a path-like name as a `file:` URI, for tests that need a document
/// identity but no file on disk.
#[doc(hidden)]
pub fn test_uri(name: &str) -> Uri {
    let path = if name.starts_with('/') {
        Utf8PathBuf::from(name)
    } else {
        Utf8PathBuf::from("/kanagawa-lsp-test").join(name)
    };
    crate::uri::path_to_uri(&path).expect("a valid test path")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    #[test]
    fn open_change_close_is_the_overlay_lifecycle() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");

        let file = state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);
        assert!(state.vfs().has_overlay(file));
        assert_eq!(state.snapshot(&uri).unwrap().text.text, "class A {}\n");

        state.did_change_full(&uri, "class B {}\n".to_owned(), 2);
        assert_eq!(state.snapshot(&uri).unwrap().text.text, "class B {}\n");
        assert_eq!(state.document(file).unwrap().version, 2);

        state.did_close(&uri);
        assert!(!state.vfs().has_overlay(file));
        assert_eq!(state.snapshot(&uri).unwrap_err(), NoFile::NotOpen);
    }

    /// The editor's databases run under the layered budget — a first answer
    /// at four rounds, then the fixpoint — a batch compile's under the
    /// fixpoint alone (`kg_db::PassBudget`). Red with the server's
    /// `set_pass_budget` removed.
    #[test]
    fn the_server_layers_the_pass_budget() {
        let state = GlobalState::new();
        let file = state.did_open(test_uri("top.k"), "class A {}\n".to_owned(), 1);
        let budget = state.with_db(file, Db::pass_budget).expect("open");
        assert_eq!(budget, kg_db::PassBudget::EDITOR);
        assert_eq!(budget.first_answer(), Some(4));
        assert_ne!(budget, kg_db::PassBudget::default());
    }

    #[test]
    fn the_parse_cache_is_keyed_on_the_revision() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);

        let a = state.snapshot(&uri).unwrap().parse;
        let b = state.snapshot(&uri).unwrap().parse;
        assert!(Arc::ptr_eq(&a, &b), "an unchanged document reparsed");

        state.did_change_full(&uri, "class B {}\n".to_owned(), 2);
        let c = state.snapshot(&uri).unwrap().parse;
        assert!(!Arc::ptr_eq(&a, &c), "a changed document served a stale parse");
    }

    #[test]
    fn an_incremental_edit_splices_at_utf16_columns() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        // '🦀' is two UTF-16 units, so the identifier starts at column 5.
        let file = state.did_open(uri.clone(), "// 🦀 name\n".to_owned(), 1);

        let edited = state
            .apply_edit(
                file,
                Some(Range::new(Position::new(0, 6), Position::new(0, 10))),
                "text",
            )
            .unwrap();
        assert_eq!(edited, "// 🦀 text\n");
    }

    #[test]
    fn an_edit_with_no_range_replaces_everything() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        let file = state.did_open(uri, "old".to_owned(), 1);
        assert_eq!(state.apply_edit(file, None, "new").unwrap(), "new");
    }

    #[test]
    fn a_file_with_no_manifest_still_gets_settings() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        let file = state.did_open(uri, "class A {}\n".to_owned(), 1);
        let settings = state.settings_for(file);
        assert!(settings.implicit_base, "the fallback must still describe a usable project");
        assert_eq!(settings.lsp.max_file_size_kb, 4096);
    }

    /// Risk #13 of the project plan: a language server with no config to read.
    /// Without `import-dirs` and `target-device` the import search path is
    /// empty and the standard library does not resolve, so the server has to
    /// discover the manifest the same way `kg build` does — by walking up from
    /// the file it was asked about.
    #[test]
    fn the_manifest_is_discovered_by_walking_up_from_the_file() {
        let dir = std::env::temp_dir().join(format!("kg-lsp-cfg-{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            dir.join("kanagawa.toml"),
            "[project]\nimport-dirs = [\"library\"]\ntarget-device = \"mock\"\n\
             [lsp]\ndiagnostics = \"on-save\"\nmax-file-size-kb = 1\nindex-import-dirs = true\n",
        )
        .unwrap();

        let root = Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
        let state = GlobalState::new();
        state.set_roots(vec![root.clone()]);

        let uri = crate::uri::path_to_uri(&root.join("src").join("top.k")).unwrap();
        let file = state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);

        let settings = state.settings_for(file);
        assert_eq!(settings.target_device, "mock");
        // Device directories precede plain ones, as the reference searches them.
        assert_eq!(
            settings.import_search_path(),
            vec![root.join("library/device/mock"), root.join("library")]
        );
        // The `[lsp]` table is honoured, not merely parsed.
        assert_eq!(settings.lsp.diagnostics, kg_config::DiagnosticTiming::OnSave);
        assert_eq!(settings.lsp.max_file_size_kb, 1);

        // ...and the size cap it sets is enforced.
        state.did_change_full(&uri, "x".repeat(2048), 2);
        assert_eq!(state.snapshot(&uri).unwrap_err(), NoFile::TooLarge);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A manifest's `[board]` reaches the editor as it reaches `kg`: the
    /// board's device, and the platform's library searched last, so a
    /// device library (`import lattice.spram`) resolves. A board not found
    /// is said once, and the rest of the manifest still holds.
    #[test]
    fn a_board_sets_the_device_and_the_platforms_library() {
        let dir = std::env::temp_dir().join(format!("kg-lsp-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
        std::fs::create_dir_all(root.join("platform/library")).unwrap();
        std::fs::create_dir_all(root.join("platform/boards/demo")).unwrap();
        std::fs::write(
            root.join("platform/boards/demo/board.toml"),
            "[fpga]\ndevice = \"ice40-up5k\"\npart = \"up5k\"\nhal = \"ice40\"\ntool = \"icestorm\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("good")).unwrap();
        std::fs::write(
            root.join("good/kanagawa.toml"),
            "[project]\nimport-dirs = [\"lib\"]\n[board]\nname = \"demo\"\nplatform = \"../platform\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("bad")).unwrap();
        std::fs::write(
            root.join("bad/kanagawa.toml"),
            "[project]\nimport-dirs = [\"lib\"]\n[board]\nname = \"nope\"\nplatform = \"../platform\"\n",
        )
        .unwrap();

        let state = GlobalState::new();
        state.set_roots(vec![root.clone()]);
        let good = state.did_open(crate::uri::path_to_uri(&root.join("good/top.k")).unwrap(), "class A {}\n".to_owned(), 1);
        let settings = state.settings_for(good);
        assert_eq!(settings.target_device, "ice40-up5k");
        assert_eq!(settings.import_dirs, [root.join("good/lib"), root.join("platform/library")]);
        assert!(state.take_config_errors().is_empty());

        let bad = state.did_open(crate::uri::path_to_uri(&root.join("bad/top.k")).unwrap(), "class A {}\n".to_owned(), 1);
        let settings = state.settings_for(bad);
        assert_eq!(settings.import_dirs, [root.join("bad/lib")]);
        let errors = state.take_config_errors();
        assert!(errors.len() == 1 && errors[0].contains("no board at"), "{errors:?}");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_manifest_is_reported_once_and_does_not_take_the_file_offline() {
        let dir = std::env::temp_dir().join(format!("kg-lsp-badcfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kanagawa.toml"), "[projekt]\n").unwrap();

        let root = Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
        let state = GlobalState::new();
        state.set_roots(vec![root.clone()]);

        let uri = crate::uri::path_to_uri(&root.join("top.k")).unwrap();
        state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);

        // The file still parses, on fallback settings.
        assert_eq!(state.snapshot(&uri).unwrap().text.text, "class A {}\n");

        let errors = state.take_config_errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("projekt"), "{errors:?}");
        // Reported once, not on every keystroke.
        let _ = state.snapshot(&uri);
        assert!(state.take_config_errors().is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A file with no manifest above it is said to have none, once: its
    /// settings load no base library and resolve no import, and the user
    /// is told why rather than left with a gutter of names not in scope.
    /// Red with the fallback taken silently.
    #[test]
    fn a_file_with_no_manifest_is_said_to_have_none_once() {
        let state = GlobalState::new();
        let uri = test_uri("bare/top.k");
        let file = state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);
        let _ = state.settings_for(file);
        let said = state.take_config_errors();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].starts_with("none found above `/kanagawa-lsp-test/bare`"), "{said:?}");
        let other = state.did_open(test_uri("bare/other.k"), "class B {}\n".to_owned(), 1);
        let _ = state.settings_for(other);
        assert!(state.take_config_errors().is_empty(), "said once, not per file");
    }

    #[test]
    fn an_oversized_file_is_declined_rather_than_parsed() {
        let state = GlobalState::new();
        let uri = test_uri("huge.k");
        // The fallback manifest caps at 4 MiB.
        let huge = "\n".repeat(5 * 1024 * 1024);
        state.did_open(uri.clone(), huge, 1);
        assert_eq!(state.snapshot(&uri).unwrap_err(), NoFile::TooLarge);
    }

    /// The main loop writes a buffer's text and then its document entry, in
    /// two steps; a worker snapshotting between them must still get the parse
    /// *of the text it read*. Keyed on a revision stored in the document entry,
    /// it got the previous text's parse — every offset in it wrong, and a
    /// slice by one of them a panic.
    #[test]
    fn a_snapshot_never_pairs_a_text_with_another_texts_parse() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        let file = state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);
        let _ = state.snapshot(&uri).unwrap();

        // The window: the overlay has moved, the document entry has not.
        state.vfs().set_overlay(file, "class Longer {}\n".to_owned());
        let snap = state.snapshot(&uri).unwrap();
        assert_eq!(snap.text.text, "class Longer {}\n");
        assert_eq!(snap.parse.syntax_node().to_string(), snap.text.text);
    }

    /// A snapshot's version is the version of its text, however the main loop
    /// and a worker interleave. A stress test: it cannot fail on a correct
    /// implementation, and it caught the two-step write within its budget on
    /// both runs against it.
    #[test]
    fn a_snapshot_names_the_version_of_the_text_it_holds() {
        let state = GlobalState::new();
        let uri = test_uri("racing.k");
        state.did_open(uri.clone(), "// 1\n".to_owned(), 1);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (state, uri, stop) = (state.clone(), uri.clone(), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut torn = Vec::new();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let snap = state.snapshot(&uri).unwrap();
                    let written = snap.text.text[3..snap.text.text.len() - 1].parse::<i32>().unwrap();
                    if written != snap.version {
                        torn.push((snap.version, written));
                    }
                }
                torn
            })
        };
        for version in 2..20_000 {
            state.did_change_full(&uri, format!("// {version}\n"), version);
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let torn = reader.join().unwrap();
        assert!(torn.is_empty(), "(version, text) pairs that disagree: {:?}", &torn[..torn.len().min(5)]);
    }

    fn text_seen_by_the_db(state: &GlobalState, file: FileId) -> String {
        state.with_db(file, |db| db.parse(file).syntax_node().to_string()).expect("open")
    }

    /// AUDIT D3. The main loop applies `didChange` while a worker is inside a
    /// query; if the two share a lock held for the query's length, a
    /// keystroke during a one-second check freezes the editor for a second.
    #[test]
    fn a_keystroke_does_not_wait_for_a_running_query() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        let file = state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);

        let (started, is_started) = crossbeam_channel::bounded::<()>(0);
        let (release, released) = crossbeam_channel::bounded::<()>(0);
        let query = {
            let state = state.clone();
            std::thread::spawn(move || {
                state.with_db(file, |db| {
                    // A memo at the old text, which only the queued change
                    // can invalidate.
                    db.parse(file);
                    started.send(()).unwrap();
                    released.recv().unwrap();
                })
            })
        };
        is_started.recv().unwrap();

        let (done, is_done) = crossbeam_channel::bounded::<()>(1);
        let keystroke = {
            let state = state.clone();
            let uri = uri.clone();
            std::thread::spawn(move || {
                state.did_change_full(&uri, "class B {}\n".to_owned(), 2);
                done.send(()).unwrap();
            })
        };
        // A liveness bound, not a timing: the change needs no lock the query
        // holds, so it finishes whatever the machine's load.
        let finished = is_done.recv_timeout(std::time::Duration::from_secs(30)).is_ok();
        release.send(()).unwrap();
        query.join().unwrap();
        keystroke.join().unwrap();
        assert!(finished, "didChange waited for a running query");

        // Queued, not lost: the next query sees the edit.
        assert_eq!(text_seen_by_the_db(&state, file), "class B {}\n");
    }

    /// AUDIT D4 through the server's own door: once a database has heard of a
    /// buffer, typing in it leaves the world revision — and with it every
    /// whole-program memo — alone.
    #[test]
    fn typing_in_an_open_buffer_does_not_move_the_world() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        let file = state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);
        let world = |state: &GlobalState| state.with_db(file, |db| db.world_revision()).unwrap();
        world(&state);
        // The first news of a file moves the world, conservatively.
        state.did_change_full(&uri, "class B {}\n".to_owned(), 2);
        let settled = world(&state);
        state.did_change_full(&uri, "class C {}\n".to_owned(), 3);
        state.did_change_full(&uri, "class D {}\n".to_owned(), 4);
        assert_eq!(world(&state), settled, "a keystroke moved the world");

        // Closing a buffer with nothing on disk behind it is an existence
        // change, and must still move it.
        state.did_close(&uri);
        state.did_open(uri, "class E {}\n".to_owned(), 5);
        assert_ne!(world(&state), settled, "a vanished file did not move the world");
    }

    /// A handler that panics inside a query must cost that request only. The
    /// old shape held a `Mutex` across the query, so the unwind poisoned it and
    /// the next keystroke's `lock().unwrap()` on the main loop ended the server.
    #[test]
    fn a_panicking_query_leaves_the_server_usable() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        let file = state.did_open(uri.clone(), "class A {}\n".to_owned(), 1);
        // Build the database, then tell it about the file once, so that an
        // identical edit afterwards moves nothing and the old database would
        // hand back the old program.
        let _ = text_seen_by_the_db(&state, file);
        state.did_change_full(&uri, "class A {}\n".to_owned(), 2);
        let before = state.with_db(file, |db| db.program(&[file])).unwrap();

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.with_db(file, |_| panic!("a query bug"))
        }));
        assert!(unwound.is_err());

        state.did_change_full(&uri, "class A {}\n".to_owned(), 3);
        let after = state.with_db(file, |db| db.program(&[file])).unwrap();
        // A fresh database, not the one the unwind went through: the same
        // inputs through the old one would have handed back `before` itself.
        assert!(!Arc::ptr_eq(&before, &after), "the panicked database was kept");
        assert_eq!(text_seen_by_the_db(&state, file), "class A {}\n");
    }

    /// A query cancelled at a checkpoint (sprint W7.2) unwinds out of
    /// `with_db` as `Cancelled`, and the project is kept: the next query is
    /// answered by the same database. Red with the eviction guard seeing the
    /// cancellation as a panic.
    #[test]
    fn a_cancelled_query_keeps_its_database() {
        let state = GlobalState::new();
        let uri = test_uri("top.k");
        let file = state.did_open(uri, "class A {}\n".to_owned(), 1);
        let before = state.with_db(file, |db| db.program(&[file])).unwrap();
        let cancelled = kg_base::cancel::catch(|| {
            state.with_db(file, |db| {
                db.cancel_token().raise();
                db.cancel_token().check();
            })
        });
        assert_eq!(cancelled, Err(kg_base::cancel::Cancelled));
        state.with_db(file, |db| db.cancel_token().reset());
        let after = state.with_db(file, |db| db.program(&[file])).unwrap();
        assert!(Arc::ptr_eq(&before, &after), "the cancelled database was dropped");
    }

    /// AUDIT D2. A manifest edit must reach the database answering for that
    /// project; building it once and keeping it forever meant `import-dirs`
    /// and `target-device` changes needed a server restart.
    #[test]
    fn a_reloaded_manifest_reaches_the_database() {
        let dir = std::env::temp_dir().join(format!("kg-lsp-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("kanagawa.toml");
        std::fs::write(&manifest, "[project]\ntarget-device = \"mock\"\n").unwrap();

        let root = Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
        let state = GlobalState::new();
        state.set_roots(vec![root.clone()]);
        let uri = crate::uri::path_to_uri(&root.join("top.k")).unwrap();
        let file = state.did_open(uri, "class A {}\n".to_owned(), 1);

        let device = |state: &GlobalState| {
            state.with_db(file, |db| db.settings().target_device.clone()).unwrap()
        };
        assert_eq!(device(&state), "mock");

        std::fs::write(&manifest, "[project]\ntarget-device = \"other\"\n").unwrap();
        state.reload_config();
        assert_eq!(device(&state), "other", "the database kept the old manifest's settings");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
