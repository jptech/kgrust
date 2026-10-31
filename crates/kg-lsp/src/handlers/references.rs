//! `textDocument/references` — every *use* of the declaration under the cursor.
//!
//! # The whole algorithm, and the trap it exists to avoid
//!
//! Resolve the cursor to a declaration with [`kg_db::Db::definition_at`], then
//! resolve **every identifier in the program with the same spelling** and keep
//! the ones that answer with that same declaration.
//!
//! The obvious cheaper implementation — collect the identifiers whose text
//! matches — is wrong, and wrong in the direction that is hardest to notice.
//! Kanagawa shadows: `findInScopes` takes the *first* enclosing scope that binds
//! a name and stops (`SymbolTable.hs:256-259`), so the `x` in an inner block and
//! the `x` outside it are two unrelated symbols that happen to be spelled the
//! same. A textual answer reports each one's uses under the other's name, and a
//! rename built on it would silently corrupt the program. Nothing here compares
//! text except as a *filter over candidates*: the decision is always a
//! resolution (see `occurrences`).
//!
//! # Which program each file is resolved under
//!
//! Its own: an open buffer, and every file its program holds, under the
//! program that buffer roots — exactly the one [`kg_db::Db::diagnostics`] and
//! `handlers::definition` answer against — and a file of the project nobody
//! has open, under the program *it* roots. Never one program merged over
//! several.
//!
//! Merging every root into a single root set is what `kg check a.k b.k` does
//! (`Main.hs:70,80`), and it is the wrong model for an editor and wrong for a
//! search: two unrelated designs each declare a top-level `main`, which merged
//! would collide in the one [`kg_db::SymbolMap`], and every file with no
//! `module` header shares one namespace, so merged each would resolve the
//! others' names without importing them — over-resolution, divergence #19's
//! shape (`Symbols.hs:37-41`), and a rename built on it would edit a file that
//! never named the declaration. A file's own program is also what makes an
//! answer *mean* something a user can check: a reported position is one where
//! goto-definition, invoked with that file focused, lands on the target.
//!
//! Comparing across programs is sound because `Db::item_tree` is memoized per
//! module and takes no root set: a `DefId` — and so the whole
//! [`kg_db::Definition`] built from it — denotes the same declaration in every
//! program that contains its module.
//!
//! # Which files: the project's, that spell the name and can see it
//!
//! A declaration another file can name ([`importable`]: at the top of its
//! file or its `module`, or a member of something that is) is searched for in
//! every file of the project ([`sources::project_sources`]: under the import
//! directories, and under the manifest's root) that spells its name and roots
//! a program holding the declaring module — the loader's own answer
//! (`Parser/IO.hs:85-110`), so a file that reaches the module only through
//! another that re-exposes it (`module facade { module helper }`,
//! `SymbolTable.hs:158-166`) is searched, and so is one that holds a value of
//! a member's aggregate without importing it at all: `v.f` names `f` through
//! `v`'s type. Both checks are exact filters, not guesses: every use is an
//! identifier token spelled as the name, and a name resolves within the
//! program its file roots.
//!
//! A file's program is forgotten once it is searched ([`Db::forget_roots`]),
//! and a member's receivers are typed by a loop of the file's own, run for the
//! search and dropped with it ([`Db::loop_once`]): the search holds one such
//! program at a time, and none after it. What stays is per module — the parse
//! of each file read, and the sorted lowering of each a loop ran over.
//!
//! **The cost**, measured on the reference's library and its tests as one
//! project (2026-10-10, `crates/kg-lsp/tests/workspace_cost.rs`): a name no
//! `.` reaches is ~7 ms a file, `assert_equal`'s 484 uses in 37 files 0.3 s;
//! a member is a loop a file, 26 ms to 2.8 s each, `optional`'s `is_valid`
//! 36 of them and 13 s, its heap at most one program's loop above the
//! editor's. So the search is bounded ([`Bound`]): past it a reference list is
//! partial and says so in the log, and a rename is refused.
//!
//! # What is *not* searched
//!
//! A file outside the import directories and the manifest's root, a file
//! under another manifest, and — with no manifest at all — every file but the
//! open buffers' programs: the fallback settings name no import directory and
//! only guess at a root. A use there is an under-report.
//!
//! Everything [`kg_db::Db::definition_at`] declines to resolve — an import
//! qualifier, an intrinsic, an ambiguous name, a declaration under an
//! unelaborated `static if` — has no references here either, for the same reason
//! it has no definition: the query that would have to identify them is the one
//! that declined.
//!
//! # Members, through their receivers
//!
//! A field or a method is used as `v.f`, which names `f` through `v`'s type,
//! and `definition_at` resolves it so — through the program-wide loop's type
//! of `v` where a loop has read it: an open buffer's program's, the editor's;
//! a file nobody has open, the one run for it. So the uses found are those
//! whose receiver the loop types: `a.get()` on a `static Box<4> a`,
//! `make<uint8>(1).lo`, and inside a template's body a receiver every
//! instance types alike. Missed, and so an under-report: a member's use in a
//! buffer that does not parse and that no loop has read — opened broken, or
//! typed since the last loop ran (sprint W3.4); a use inside a template's
//! body no instance in the program reaches; a use whose receiver the loop
//! does not type (inside a lambda, an abbreviated template's `auto`
//! parameter); and a use whose instances reach two declarations — a `static
//! if` declaring the member in each branch — which `definition_at` declines,
//! so it is the use of neither. In a buffer edited since its loop ran, a
//! receiver the edit retyped is read as it last parsed — the last good
//! answer, which a listing may show. A rename, which may neither under- nor
//! over-report, refuses on each of those outside a template's body
//! (`super::rename`).
//!
//! # A parameter, through its labels
//!
//! A named argument (`f(x: 1)`, edition 1.1: `docs/editions/1.1.md`) is a
//! use of `f`'s parameter `x`, and `definition_at` resolves the label so;
//! a rename of `x` that left it would break the call. So a parameter of a
//! function another file can name is searched for like the function
//! ([`labelled`]) — but only in the files whose language has named
//! arguments, the only ones a label can be written in. A template's
//! parameter likewise, through a named template argument (`Buffer<Size:
//! 8>`), in the files whose language has those. A project of edition 1.0
//! has none, and searches a parameter in its open buffers, as it did before
//! editions.

use std::cell::OnceCell;

use kg_base::language::Feature;
use kg_base::{FileId, FileRange, FxHashSet, TextSize, Utf8Path};
use kg_db::looped::EditorLoop;
use kg_db::{Db, Definition};
use kg_resolve::{DefKind, Item, ItemTree};
use kg_syntax::{SyntaxKind, SyntaxToken};
use lsp_types::{Location, MessageType, ReferenceParams};

use crate::convert;
use crate::global_state::GlobalState;
use crate::handlers::{Answer, location, sources};

/// How far one [`search`] goes past the open buffers' programs: at most
/// `files` of the project's files read, and no new one started once `time`
/// has gone by. Past either the answer is partial, which a reference list
/// says in the log and a rename refuses on.
///
/// **Why a clock as well as a count.** A file is read under a program of
/// its own, ~7 ms apiece on the reference's corpus, and a count bounds that
/// well. A member's use is placed through a loop of the file's own, and a
/// loop costs what its program costs: measured over the 36 files that hold
/// `optional`'s `is_valid` (2026-10-10, load 6-10,
/// `crates/kg-lsp/tests/workspace_cost.rs`), from 26 ms for a library
/// module's to 2.8 s for `risc-v.k`'s, a count of loops or of modules
/// tracking the time not at all. So the latency is bounded where it is
/// spent, and the price is a partial answer that can differ under load —
/// which a reference list says, and a rename turns into a refusal, never
/// into a smaller edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Bound {
    pub(crate) files: usize,
    pub(crate) time: std::time::Duration,
    /// Stop at the first use the search cannot place ([`Found::unsure`]):
    /// a rename is refused on it whatever else the search would find, and
    /// one the open buffers hold is found before any file is read.
    pub(crate) until_unsure: bool,
}

impl Bound {
    /// A reference list's: an answer while the user waits for it. Every
    /// case measured on the corpus but the commonest field's is inside it.
    pub(crate) const REFERENCES: Bound =
        Bound { files: 512, time: std::time::Duration::from_secs(10), until_unsure: false };

    /// A rename's, and its `prepareRename`'s: an edit the user asked for and
    /// waits on, which must be whole or not at all, so the clock is a
    /// minute — the bound is there for a project this server cannot search
    /// in reasonable time at all, not to trade completeness for latency.
    pub(crate) const RENAME: Bound =
        Bound { files: 512, time: std::time::Duration::from_secs(60), until_unsure: true };
}

/// Answers with every reference, in search order: the cursor's own file
/// first, then whatever the other buffers' programs reach, then the
/// project's other files by path.
///
/// An empty vector and `Err(NoFile)` are different facts — "this name has no
/// references I can see" and "I have no such document" — and the dispatcher
/// flattens both to a client-visible nothing. They are kept apart because a test
/// can tell them apart, and because answering `[]` for a document nobody opened
/// would be claiming to have looked.
pub fn references(state: &GlobalState, params: &ReferenceParams) -> Answer<Vec<Location>> {
    references_within(state, params, Bound::REFERENCES)
}

/// [`references`] under a bound of the caller's, for the tests that reach
/// it.
fn references_within(state: &GlobalState, params: &ReferenceParams, bound: Bound) -> Answer<Vec<Location>> {
    let at = &params.text_document_position;
    let snap = state.snapshot(&at.text_document.uri)?;
    let enc = state.encoding();
    let offset = convert::offset(snap.line_index(), enc, at.position);
    let buffers = buffers_of_this_project(state, &snap.settings.root, snap.file);
    let include_declaration = params.context.include_declaration;

    let found = state.with_db(snap.file, |db| {
        // The cursor's file is its own root, character for character what
        // `handlers::definition` does — the target of a find-references and the
        // target of a goto-definition at the same position must be the same
        // declaration, or the two features are describing different programs.
        let Some(target) = db.definition_at(&[snap.file], snap.file, offset) else {
            return Vec::new();
        };
        let Some(name) = target.qualified_name.last() else { return Vec::new() };
        let name = db.interner().resolve(name);

        let started = std::time::Instant::now();
        let found = search(db, &buffers, &target, name.as_str(), bound);
        if found.spelled > 0 {
            crate::log(
                MessageType::LOG,
                format!(
                    "references to `{name}`: {} of the project's files outside the open programs spell it, \
                     {} of them hold its module; {} loops, {:.0?}",
                    found.spelled,
                    found.held,
                    found.loops,
                    started.elapsed()
                ),
            );
        }
        if let Some(&first) = found.unsearched.first() {
            let shown = db.path(first).map(|p| p.to_string()).unwrap_or_default();
            crate::log(
                MessageType::WARNING,
                format!(
                    "references to `{name}`: {} of the project's files spell it and were not searched, \
                     the search stopping at {} files or {:?}; the first is `{shown}`",
                    found.unsearched.len(),
                    bound.files,
                    bound.time
                ),
            );
        }
        found
            .hits
            .into_iter()
            // `include_declaration = false` drops exactly the declaration's own
            // name token, which the search finds like any other occurrence:
            // `definition_at` on a declaration answers with that declaration.
            .filter(|hit| include_declaration || *hit != target.range)
            .filter_map(|hit| location(state, enc, hit))
            .collect()
    });

    Ok(found.unwrap_or_default())
}

/// The open buffers this answer covers: the cursor's file first, then every
/// other open document governed by the same `kanagawa.toml`.
///
/// Restricted to one project because [`GlobalState::with_db`] selects the
/// database by `Settings::root`, and a file under a different manifest has a
/// different import search path — resolving it against this database's settings
/// would answer about a program that does not exist.
///
/// The cursor's file leads so that its own program is the one its own file is
/// searched under (see [`search_plan`]), and so the results a user sees first
/// are the ones in the buffer they are looking at. The rest are sorted because
/// `GlobalState::open_documents` reads a hash map: an unstable order would be an
/// unstable root set, and the root set is `Db::program`'s memo key.
pub(crate) fn buffers_of_this_project(state: &GlobalState, root: &Utf8Path, cursor: FileId) -> Vec<FileId> {
    let mut rest: Vec<FileId> = state
        .open_documents()
        .into_iter()
        .map(|doc| doc.file)
        .filter(|&file| file != cursor && state.settings_for(file).root.as_path() == root)
        .collect();
    rest.sort_unstable();
    rest.dedup();

    let mut out = Vec::with_capacity(rest.len() + 1);
    out.push(cursor);
    out.extend(rest);
    out
}

/// Which root set each file is searched under: one entry per open buffer,
/// holding the files of that buffer's program that no earlier buffer already
/// claimed.
///
/// The claiming is what keeps a file — the base library above all — from being
/// walked once per open tab, and what keeps a position from being reported
/// twice. Which buffer claims a shared file is decided by
/// [`buffers_of_this_project`]'s order, so it is deterministic; it matters only
/// for a file that resolves differently depending on which root pulled it in,
/// which is a file whose own program disagrees with itself.
pub(crate) fn search_plan(db: &Db, buffers: &[FileId]) -> Vec<(FileId, Vec<FileId>)> {
    let mut claimed = FxHashSet::default();
    let mut plan = Vec::new();
    for &buffer in buffers {
        let files: Vec<FileId> = db
            .program(&[buffer])
            .files
            .iter()
            // A module with no file is `.cmdargs` / `.options`, generated from
            // the manifest and with no on-disk text (`Parser/IO.hs:101-102`).
            // There is no position in it for a client to open.
            .filter_map(|module| module.file())
            .filter(|file| claimed.insert(*file))
            .collect();
        if !files.is_empty() {
            plan.push((buffer, files));
        }
    }
    plan
}

/// What a [`search`] found.
#[derive(Debug, Default)]
pub(crate) struct Found {
    /// Every occurrence that resolves to the target, in search order.
    pub(crate) hits: Vec<FileRange>,
    /// The first `.name` spelled as a member target whose receiver does not
    /// say whether it is a use ([`Unsure`]): a reference list under-reports
    /// it, and a rename refuses on it (`super::rename`).
    pub(crate) unsure: Option<Unsure>,
    /// The project's files that spell the name, may hold a use, and were
    /// left unsearched by the bound, in the order they would have been
    /// searched.
    pub(crate) unsearched: Vec<FileId>,
    /// How many of the project's files outside the open buffers' programs
    /// spell the name; of those read, how many root a program that holds
    /// the declaring module; and how many loops those needed.
    pub(crate) spelled: usize,
    pub(crate) held: usize,
    pub(crate) loops: usize,
}

/// A `.name` the search could not place: where, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Unsure {
    pub(crate) file: FileId,
    pub(crate) at: TextSize,
    pub(crate) why: &'static str,
}

/// Every occurrence of `target` the project holds: the open buffers'
/// programs ([`search_plan`]) under the editor's loops, and then — for a
/// declaration another file can name ([`importable`]) — every file of the
/// project ([`sources::project_sources`]) that spells `needle`, is in no
/// open buffer's program, and roots a program that holds the declaring
/// module, each searched under that program of its own and through a loop
/// of its own ([`Db::loop_once`]), forgotten as the next file starts
/// ([`Db::forget_roots`]). The search stops where `bound` says, and what it
/// did not read is [`Found::unsearched`].
///
/// "Spells" is a substring of the text, read before anything is parsed:
/// every use is an identifier token spelled `needle` ([`occurrences`]), and
/// a token's text is part of the file's, so a file without it holds no use
/// and costs one read. "Holds the declaring module" is the loader's own
/// answer, `Db::program(&[file]).contains(..)` — the import search, re-
/// exposure and all (`Parser/IO.hs:85-110`, `SymbolTable.hs:158-166`) —
/// and it is exact: a name in `file` resolves within the program `file`
/// roots, so a program without the module has no use of it.
pub(crate) fn search(db: &Db, buffers: &[FileId], target: &Definition, needle: &str, bound: Bound) -> Found {
    let plan = search_plan(db, buffers);
    let member = selectable(db, target);
    let mut found = Found::default();
    for (root, files) in &plan {
        walk(db, &[*root], files, Reading::Editor, target, Some(needle), member, &mut found);
    }
    let labels_only = !importable(db, target);
    let labels = labelled(db, target);
    if labels_only && labels.is_none() {
        return found;
    }

    let started = std::time::Instant::now();
    let claimed: FxHashSet<FileId> = plan.iter().flat_map(|(_, files)| files.iter().copied()).collect();
    let spelled: Vec<FileId> = sources::project_sources(db.settings())
        .into_iter()
        .filter(|path| !labels_only || labels.is_some_and(|f| db.settings().language_of(path).features().has(f)))
        .map(|path| db.file_id(&path))
        .filter(|file| !claimed.contains(file))
        .filter(|&file| db.vfs().read(file).is_some_and(|text| text.text.contains(needle)))
        .collect();
    found.spelled = spelled.len();
    for (i, &file) in spelled.iter().enumerate() {
        if bound.until_unsure && found.unsure.is_some() {
            break;
        }
        if i == bound.files || started.elapsed() >= bound.time {
            found.unsearched = spelled[i..].to_vec();
            break;
        }
        kg_base::cancel::checkpoint();
        let roots = vec![file];
        let forget = Forget { db, roots };
        if db.program(&forget.roots).contains(target.def.module) {
            found.held += 1;
            walk(db, &forget.roots, &[file], Reading::Once, target, Some(needle), member, &mut found);
        }
    }
    found
}

/// Forgets a file's program as the search leaves it, unwinding included: a
/// cancelled search holds no program behind it either.
struct Forget<'a> {
    db: &'a Db,
    roots: Vec<FileId>,
}

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        self.db.forget_roots(&self.roots);
    }
}

/// Where a search entry reads a member's receiver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    /// An open buffer's program: the editor's loop, the one its
    /// diagnostics run and the memo keeps ([`Db::editor_loop`]).
    Editor,
    /// A file nobody has open: a loop run for this search and dropped with
    /// the entry ([`Db::loop_once`]) — the fixpoint, `kg check`'s answer.
    Once,
}

/// Every identifier in the planned files that **resolves to** `target`.
///
/// `pub(crate)` for [`crate::handlers::document_highlight`], which is this
/// function over a plan of one file; [`search`] is it over the project, and
/// [`crate::handlers::rename`] that with a replacement attached. Two
/// occurrence finders — one for the highlight and one for the reference
/// list — would be two answers to "what else is this name", and the cheap
/// one would be the wrong one.
///
/// `needle` is a pre-filter on the identifier's spelling, and only a
/// pre-filter — the answer is decided by [`kg_db::Db::definition_at`] on every
/// candidate that survives it. It is sound because every way `definition_at`
/// reaches a declaration goes through a key ending in the name under the cursor:
/// the cursor on the declaration's own name token matches by range, the scope
/// walk requires `qualified_name.last() == name`, and sources 2-4 probe the
/// written path, whose last segment is the token. With it goes a second: a
/// member selector (`v.f`'s `f`) is skipped when `target` is no member, since
/// `definition_at` answers a selector only with a member the receiver's type
/// declares — and so never asks for the loop that would type the receiver.
/// Passing `None` disables both and resolves every identifier in the program,
/// which is what `a_spelling_filter_hides_no_reference` compares the filtered
/// answer against.
pub(crate) fn occurrences(
    db: &Db,
    plan: &[(FileId, Vec<FileId>)],
    target: &Definition,
    needle: Option<&str>,
) -> Vec<FileRange> {
    let member = selectable(db, target);
    let mut found = Found::default();
    for (root, files) in plan {
        walk(db, &[*root], files, Reading::Editor, target, needle, member, &mut found);
    }
    found.hits
}

/// One search entry: `files` resolved under the program `roots` root, a
/// member's receiver read as `reading` says — the loop asked for at the first
/// selector that needs it, and kept for the entry.
///
/// Notes in `found` the first `.name` it cannot place, where `target` is a
/// member: a selector whose receiver no loop types, outside a template's body
/// — or which sits in a buffer edited since its loop ran, where a receiver is
/// typed as the last state of the buffer that parsed had it
/// (`kg_db::looped::EditorLoop`) — or whose instances reach `target` and
/// another declaration (a `static if` declaring it in each branch). An
/// unresolved `.name` inside a template's body is not noted: its receiver
/// has a type only in the instances, and none of this program's has it
/// there — the template is not instantiated, or not yet by the editor's
/// first answer (`kg_db::PassBudget::Layered`). Noting every such use would
/// refuse most renames of a common member name, the base library being full
/// of templates; the cost is a use the loop had not reached, which a rename
/// then misses and the next full check reports.
#[allow(clippy::too_many_arguments)]
fn walk(
    db: &Db,
    roots: &[FileId],
    files: &[FileId],
    reading: Reading,
    target: &Definition,
    needle: Option<&str>,
    member: bool,
    found: &mut Found,
) {
    let cell: OnceCell<Option<EditorLoop>> = OnceCell::new();
    let looped = || {
        cell.get_or_init(|| match reading {
            Reading::Editor => roots.iter().find_map(|&root| db.editor_loop(roots, root)),
            Reading::Once => Some(db.loop_once(roots)),
        })
        .clone()
    };
    for &file in files {
        kg_base::cancel::checkpoint();
        let tree = db.parse(file).syntax_node();
        for token in tree.descendants_with_tokens().filter_map(|e| e.into_token()) {
            if token.kind() != SyntaxKind::IDENT {
                continue;
            }
            let selector = kg_db::navigate::is_member_selector(&token);
            if let Some(needle) = needle
                && (token.text() != needle || (selector && !member))
            {
                continue;
            }
            let reached = db.definitions_at_through(&looped, roots, file, probe(&token));
            if matches!(reached.as_slice(), [one] if one == target) {
                found.hits.push(FileRange::new(file, token.text_range()));
            }
            if !(selector && member) || found.unsure.is_some() {
                continue;
            }
            let edited = cell.get().and_then(Option::as_ref).is_some_and(|l| !l.is_current());
            let in_template = || token.parent_ancestors().any(|n| n.kind() == SyntaxKind::TEMPLATE_DECL);
            let why = match reached.as_slice() {
                _ if edited => "is typed as the buffer was before an edit that broke its parse",
                [] if in_template() => continue,
                [] => "cannot be resolved through its receiver here",
                [_] => continue,
                several if several.contains(target) => "reaches this member in some instances and another in others",
                _ => continue,
            };
            found.unsure = Some(Unsure { file, at: token.text_range().start(), why });
        }
    }
    if reading == Reading::Once && cell.get().is_some() {
        found.loops += 1;
    }
}

/// Whether a `.` can name `target`: a member of an aggregate — a field, a
/// method, a member template — so that `v.f` names it through `v`'s type
/// (`tLookupField`, `Type.hs:961-971`; `method`, `Type.hs:1358-1362`). Read
/// through any `template` between the item and its aggregate; `true` where
/// the item tree has no such item, which only costs a search its shortcut.
pub(crate) fn selectable(db: &Db, target: &Definition) -> bool {
    let tree = db.item_tree(target.def.module);
    let Some(mut at) = tree.item(target.def.item) else { return true };
    while let Some(parent) = at.parent.and_then(|p| tree.item(p)) {
        match parent.kind {
            DefKind::Struct | DefKind::Union | DefKind::Class => return true,
            DefKind::Template => at = parent,
            _ => return false,
        }
    }
    false
}

/// Whether another file can name `target`: a declaration at the top level
/// of its file — inside the file's `module` declaration, whose body is a
/// scope of its own (`scoped ident declarations`, `Parser.hs:185-189`), or
/// in a top-level `static if` branch — or a member of something that is: a
/// field, a method, a member template, an enum's constant, a template's
/// target. `E::A` names a constant through its enum, and `v.f` names `f`
/// through `v`'s type, so a use of a member sits wherever a value of its
/// aggregate's type is held, which is any module whose program holds the
/// aggregate's — no import of it needed. A local, a parameter, a capture, a
/// template parameter, a member of a class declared inside a function: none
/// is reachable from outside its file, and none sends a search past the
/// open buffers. `true` where the item tree has no such item.
///
/// Read off the parent chain, which the lowering records for every
/// declaration inside another; a declaration with none is at the top level.
/// Until 2026-10-10 this asked for the file's own scope instead, which a
/// declaration inside `module m { … }` is never in — so no declaration of a
/// module with a header, every module another file can import, was
/// importable, and a rename edited the open buffers' programs alone.
pub(crate) fn importable(db: &Db, target: &Definition) -> bool {
    let tree = db.item_tree(target.def.module);
    let Some(at) = tree.item(target.def.item) else { return true };
    importable_item(&tree, at)
}

/// Whether a named argument in another file can name `target` — a
/// parameter of a function, or of a template, another file can name (the
/// module docs) — and the feature a file's language needs to name it.
pub(crate) fn labelled(db: &Db, target: &Definition) -> Option<Feature> {
    let tree = db.item_tree(target.def.module);
    let param = tree.item(target.def.item)?;
    let feature = match param.kind {
        DefKind::FuncParam => Feature::NamedArguments,
        DefKind::TemplateParam => Feature::NamedTemplateArguments,
        _ => return None,
    };
    let owner = param.parent.and_then(|p| tree.item(p))?;
    importable_item(&tree, owner).then_some(feature)
}

fn importable_item<'a>(tree: &'a ItemTree, mut at: &'a Item) -> bool {
    loop {
        if at.kind == DefKind::TemplateParam {
            return false;
        }
        let Some(parent) = at.parent.and_then(|p| tree.item(p)) else { return true };
        if !matches!(parent.kind, DefKind::Struct | DefKind::Union | DefKind::Class | DefKind::Enum | DefKind::Template) {
            return false;
        }
        at = parent;
    }
}

/// The offset to ask about `token` at: its midpoint, not its start.
///
/// `Db::definition_at` resolves "the identifier the caret is inside or
/// immediately after", so an offset on a token boundary is ambiguous by design.
/// The midpoint is strictly inside every token of two bytes or more; a
/// single-byte identifier falls back to its start, where the only token that
/// could steal the answer is an identifier ending exactly there — which the
/// lexer cannot produce, since two adjacent identifiers with no trivia between
/// them are one identifier.
fn probe(token: &SyntaxToken) -> TextSize {
    token.text_range().start() + TextSize::new(token.text().len() as u32 / 2)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::convert::PositionEncoding;
    use crate::global_state::{NoFile, test_uri};
    use lsp_types::{
        PartialResultParams, Position, ReferenceContext, TextDocumentIdentifier,
        TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    };

    fn params(uri: &Uri, position: Position, include_declaration: bool) -> ReferenceParams {
        ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext { include_declaration },
        }
    }

    /// The answer as `(file name, line, column)`, which is how these fixtures
    /// are readable at all — a `Location` is a URI and a `Range` and neither is
    /// worth spelling out in an assertion.
    #[track_caller]
    fn refs(
        state: &GlobalState,
        uri: &Uri,
        position: Position,
        include_declaration: bool,
    ) -> Vec<(String, u32, u32)> {
        references(state, &params(uri, position, include_declaration))
            .expect("the document is open")
            .into_iter()
            .map(|found| {
                let name = found.uri.as_str().rsplit('/').next().unwrap_or_default().to_owned();
                (name, found.range.start.line, found.range.start.character)
            })
            .collect()
    }

    /// The same answer with the file order taken out, for the cross-file cases:
    /// which file a hit lands in is a contract, but `Program::files`' order is
    /// the loader's business and not something this module should pin.
    fn sorted(mut found: Vec<(String, u32, u32)>) -> Vec<(String, u32, u32)> {
        found.sort();
        found
    }

    fn open(files: &[(&str, &str)]) -> (GlobalState, Vec<Uri>) {
        let state = GlobalState::new();
        let uris = files
            .iter()
            .map(|(name, text)| {
                let uri = test_uri(name);
                state.did_open(uri.clone(), (*text).to_owned(), 1);
                uri
            })
            .collect();
        (state, uris)
    }

    /// Two `x`es, one shadowing the other. Line 2 declares the outer, line 5 the
    /// inner; lines 3 and 8 use the outer and line 6 uses the inner.
    const SHADOW: &str = "\
inline void main()
{
    uint32 x = 1;
    x = x + 1;
    {
        bool x;
        x = x + 1;
    }
    x = x + 2;
}
";

    /// The correctness trap. Eight occurrences of `x` are spelled identically
    /// and belong to two different symbols, because `findInScopes` stops at the
    /// first scope that binds the name (`SymbolTable.hs:256-259`). A textual
    /// answer would report all eight for either query; each query here must see
    /// only its own.
    #[test]
    fn an_inner_binding_and_the_outer_one_it_shadows_are_reported_separately() {
        let (state, uris) = open(&[("shadow.k", SHADOW)]);
        let file = "shadow.k".to_owned();

        let outer = refs(&state, &uris[0], Position::new(2, 11), true);
        assert_eq!(
            outer,
            vec![
                (file.clone(), 2, 11),
                (file.clone(), 3, 4),
                (file.clone(), 3, 8),
                (file.clone(), 8, 4),
                (file.clone(), 8, 8),
            ],
        );

        let inner = refs(&state, &uris[0], Position::new(5, 13), true);
        assert_eq!(
            inner,
            vec![(file.clone(), 5, 13), (file.clone(), 6, 8), (file.clone(), 6, 12)],
        );

        // Disjoint, and between them they account for every `x` in the file —
        // so neither query lost an occurrence to the other.
        assert!(outer.iter().all(|hit| !inner.contains(hit)));
        assert_eq!(outer.len() + inner.len(), SHADOW.matches('x').count());
    }

    /// Asking from a *use* is the same question as asking from the declaration.
    /// The inner block's use is the interesting one: it is textually identical
    /// to the outer uses and must still answer with the inner symbol's set.
    #[test]
    fn a_query_from_a_use_answers_the_same_set_as_one_from_the_declaration() {
        let (state, uris) = open(&[("shadow.k", SHADOW)]);
        assert_eq!(
            refs(&state, &uris[0], Position::new(6, 12), true),
            refs(&state, &uris[0], Position::new(5, 13), true),
        );
        assert_eq!(
            refs(&state, &uris[0], Position::new(3, 8), true),
            refs(&state, &uris[0], Position::new(2, 11), true),
        );
    }

    /// `context.includeDeclaration` removes exactly one entry — the name token
    /// of the declaration — and nothing else.
    #[test]
    fn the_declaration_is_reported_only_when_the_context_asks_for_it() {
        let (state, uris) = open(&[("shadow.k", SHADOW)]);
        let with = refs(&state, &uris[0], Position::new(2, 11), true);
        let without = refs(&state, &uris[0], Position::new(2, 11), false);

        assert_eq!(without.len() + 1, with.len());
        assert!(!without.contains(&("shadow.k".to_owned(), 2, 11)));
        let dropped = with.iter().filter(|hit| !without.contains(*hit)).count();
        assert_eq!(dropped, 1, "only the declaration may be dropped");
    }

    /// Two functions, one spelling, no relationship. This is the case a textual
    /// search gets wrong without any nesting involved: `other`'s `v` is a
    /// descendant of the same file scope, and only `binder` keeps it out
    /// (`kg_db::navigate`).
    #[test]
    fn a_local_of_another_function_with_the_same_name_is_not_a_reference() {
        let src = "\
inline void one()
{
    uint32 v = 1;
    v = v + 1;
}

inline void two()
{
    uint32 v = 2;
    v = v + 2;
}
";
        let (state, uris) = open(&[("twice.k", src)]);
        let file = "twice.k".to_owned();
        assert_eq!(
            refs(&state, &uris[0], Position::new(2, 11), true),
            vec![(file.clone(), 2, 11), (file.clone(), 3, 4), (file.clone(), 3, 8)],
        );
        assert_eq!(
            refs(&state, &uris[0], Position::new(8, 11), true),
            vec![(file.clone(), 8, 11), (file.clone(), 9, 4), (file.clone(), 9, 8)],
        );
    }

    /// A class field is reached from its member function through the
    /// `CLASS_DECL` scope, so the field and both uses are one symbol — the
    /// shape `handlers::mod`'s own fixture uses.
    #[test]
    fn a_class_field_is_found_from_its_member_function() {
        let src = "\
class Adder {
    uint32 total;
    inline void add(uint32 x) {
        total = total + x;
    }
}
";
        let (state, uris) = open(&[("adder.k", src)]);
        let file = "adder.k".to_owned();
        assert_eq!(
            refs(&state, &uris[0], Position::new(3, 8), true),
            vec![(file.clone(), 1, 11), (file.clone(), 3, 8), (file.clone(), 3, 16)],
        );
    }

    /// The pre-filter in [`occurrences`] is an optimisation, and an optimisation
    /// that changes the answer is a bug. Resolving *every* identifier in the
    /// program — the answer with no filter at all — must produce the same list.
    ///
    /// This is the test that keeps the filter honest: if a use could ever reach
    /// a declaration spelled differently from the token that reaches it, the
    /// filtered search would drop it silently and only this comparison would
    /// notice.
    #[test]
    fn a_spelling_filter_hides_no_reference() {
        let (state, uris) = open(&[("shadow.k", SHADOW)]);
        let file = state.file_id(&uris[0]).unwrap();
        let snap = state.snapshot(&uris[0]).unwrap();
        let at = Position::new(2, 11);
        let offset = convert::offset(snap.line_index(), PositionEncoding::Utf16, at);

        state
            .with_db(file, |db| {
                let target = db.definition_at(&[file], file, offset).expect("the outer `x`");
                let plan = search_plan(db, &[file]);
                let filtered = occurrences(db, &plan, &target, Some("x"));
                let exhaustive = occurrences(db, &plan, &target, None);
                assert!(!filtered.is_empty());
                assert_eq!(filtered, exhaustive);
            })
            .expect("the document is open");
    }

    /// …and the member selectors it skips for a target no `.` can name: a
    /// file-scope `x` beside a field `x` used as `p.x`, each asked for with
    /// the filter and without. Red with a member taken for a non-member,
    /// whose `p.x` the filter would then skip.
    #[test]
    fn a_selector_filter_hides_no_reference() {
        let src = "const auto x = 1;\n\nstruct Point\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    Point p;\n    auto v = p.x + x;\n}\n";
        let (state, uris) = open(&[("field.k", src)]);
        let file = state.file_id(&uris[0]).unwrap();
        for (line, column, uses) in [(0, 11, 2), (4, 11, 2)] {
            let offset = convert::offset(state.snapshot(&uris[0]).unwrap().line_index(), PositionEncoding::Utf16, Position::new(line, column));
            state
                .with_db(file, |db| {
                    let target = db.definition_at(&[file], file, offset).expect("an `x`");
                    let plan = search_plan(db, &[file]);
                    let filtered = occurrences(db, &plan, &target, Some("x"));
                    assert_eq!(filtered.len(), uses, "{line}:{column}");
                    assert_eq!(filtered, occurrences(db, &plan, &target, None), "{line}:{column}");
                })
                .expect("the document is open");
        }
    }

    /// A cursor on something that is not an identifier resolves to nothing, and
    /// nothing is the answer — not the nearest identifier's references.
    #[test]
    fn a_cursor_that_is_not_on_a_name_has_no_references() {
        let (state, uris) = open(&[("shadow.k", SHADOW)]);
        assert!(refs(&state, &uris[0], Position::new(1, 0), true).is_empty());
        assert!(refs(&state, &uris[0], Position::new(999, 0), true).is_empty());
    }

    /// A struct field is in neither symbol map — `getSymbols''` replaces an
    /// aggregate body with `mempty` (`Symbols.hs:48-49`) and a field is
    /// `uninitVar`, never `symbol` (`Parser.hs:465`) — so the `const auto x`
    /// above the struct, or a textual match, is exactly the answer this module
    /// refuses to give. Since 2026-09-06 the field is a navigation item
    /// resolved through `p`'s type, so the answer is the field and its `.x`
    /// use, and nothing else.
    #[test]
    fn a_struct_field_answers_its_uses_and_never_a_look_alike() {
        let src = "\
const auto x = 1;

struct Point
{
    uint32 x;
}

inline void main()
{
    Point p;
    auto v = p.x + x;
}
";
        let (state, uris) = open(&[("field.k", src)]);
        assert_eq!(
            sorted(refs(&state, &uris[0], Position::new(4, 11), true)),
            vec![("field.k".to_owned(), 4, 11), ("field.k".to_owned(), 10, 15)]
        );
        assert_eq!(
            refs(&state, &uris[0], Position::new(10, 15), false),
            vec![("field.k".to_owned(), 10, 15)]
        );
    }

    #[test]
    fn an_unknown_document_is_declined_not_answered_empty() {
        let state = GlobalState::new();
        let uri = test_uri("never-opened.k");
        let err = references(&state, &params(&uri, Position::new(0, 0), true)).unwrap_err();
        assert_eq!(err, NoFile::NotOpen);
    }

    const HELPER: &str = "inline void twice()\n{\n}\n";
    const IMPORTER: &str = "import helper\n\ninline void main()\n{\n    twice();\n}\n";

    /// Lays down a real project — `kanagawa.toml`, `src/helper.k`,
    /// `src/main.k` — and returns its two URIs.
    ///
    /// On disk rather than in overlays because `Settings::fallback` has no
    /// import directories at all: with no manifest, `import helper` would not
    /// resolve and the cross-file tests would pass for the wrong reason. The
    /// sources live in `src/` so that the import directory is `<root>/src` with
    /// no `.` component — the VFS keys files by path *text*, and
    /// `<root>/./helper.k` would be a second `FileId` for the same file.
    fn project(tag: &str) -> (std::path::PathBuf, GlobalState, Uri, Uri) {
        let dir = std::env::temp_dir().join(format!("kg-lsp-refs-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("kanagawa.toml"),
            "[project]\nimport-dirs = [\"src\"]\ntarget-device = \"mock\"\nimplicit-base = false\n",
        )
        .unwrap();
        std::fs::write(dir.join("src").join("helper.k"), HELPER).unwrap();
        std::fs::write(dir.join("src").join("main.k"), IMPORTER).unwrap();

        let root = kg_base::Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
        let state = GlobalState::new();
        state.set_roots(vec![root.clone()]);
        let helper = crate::uri::path_to_uri(&root.join("src").join("helper.k")).unwrap();
        let main = crate::uri::path_to_uri(&root.join("src").join("main.k")).unwrap();
        (dir, state, helper, main)
    }

    /// Scope: the search is not the open buffer. `main.k` imports `helper.k`, so
    /// `helper.k` is in `main.k`'s program and both files are searched — and the
    /// declaration is reported against `helper.k`'s own URI, which the client
    /// can open even though the request named `main.k`.
    #[test]
    fn a_use_in_another_file_of_the_program_is_found() {
        let (dir, state, _helper, main) = project("cross");
        state.did_open(main.clone(), IMPORTER.to_owned(), 1);

        // The declaration is in `helper.k`, which is not open — it arrived
        // through the import — and is still reported, against its own URI.
        assert_eq!(
            sorted(refs(&state, &main, Position::new(4, 4), true)),
            vec![("helper.k".to_owned(), 0, 12), ("main.k".to_owned(), 4, 4)],
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// …and from the other end: with the declaring file focused, its user is
    /// found whether or not it is open — open, through its buffer's program;
    /// closed, as a file of the project whose own program holds `helper.k`.
    /// Corrected 2026-10-10: until the search read the project's files this
    /// test pinned the opposite, closing `main.k` costing the reference in it.
    #[test]
    fn a_use_in_an_importing_file_is_found_open_or_not() {
        let (dir, state, helper, main) = project("back");

        state.did_open(helper.clone(), HELPER.to_owned(), 1);
        assert_eq!(
            sorted(refs(&state, &helper, Position::new(0, 12), true)),
            vec![("helper.k".to_owned(), 0, 12), ("main.k".to_owned(), 4, 4)],
            "`main.k` is not open, and its own program reaches the declaration",
        );

        state.did_open(main.clone(), IMPORTER.to_owned(), 1);
        assert_eq!(
            sorted(refs(&state, &helper, Position::new(0, 12), true)),
            vec![("helper.k".to_owned(), 0, 12), ("main.k".to_owned(), 4, 4)],
            "with the importing buffer open, its program reaches the declaration",
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A module with a header, as every module another file imports has
    /// one (`Parser.hs:205`): `helper` exposes `twice`; `main.k` imports it;
    /// `facade` re-exposes `helper` whole (`module facade { module helper }`,
    /// `SymbolTable.hs:158-166`), and `user.k` imports only `facade`.
    pub(crate) const MODULES: &[(&str, &str)] = &[
        ("helper.k", "module helper { twice }\n\ninline void twice()\n{\n}\n"),
        ("main.k", "import helper\n\ninline void main()\n{\n    twice();\n}\n"),
        ("facade.k", "module facade { module helper }\n\nimport helper\n"),
        ("user.k", "import facade\n\ninline void go()\n{\n    twice();\n}\n"),
    ];

    /// Lays down a project — `kanagawa.toml` and each of `files` under
    /// `src/`, for the reason [`project`] gives — and returns its directory,
    /// a state rooted there, and its `src/`.
    pub(crate) fn lay_out(tag: &str, files: &[(&str, &str)]) -> (std::path::PathBuf, GlobalState, kg_base::Utf8PathBuf) {
        lay_out_in("", tag, files)
    }

    /// [`lay_out`], the manifest naming edition 1.1 (`docs/editions/1.1.md`).
    pub(crate) fn lay_out_1_1(tag: &str, files: &[(&str, &str)]) -> (std::path::PathBuf, GlobalState, kg_base::Utf8PathBuf) {
        lay_out_in("[package]\nedition = \"1.1\"\n\n", tag, files)
    }

    fn lay_out_in(
        package: &str,
        tag: &str,
        files: &[(&str, &str)],
    ) -> (std::path::PathBuf, GlobalState, kg_base::Utf8PathBuf) {
        let dir = std::env::temp_dir().join(format!("kg-lsp-search-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("kanagawa.toml"),
            format!("{package}[project]\nimport-dirs = [\"src\"]\ntarget-device = \"mock\"\nimplicit-base = false\n"),
        )
        .unwrap();
        for (name, text) in files {
            std::fs::write(dir.join("src").join(name), text).unwrap();
        }
        let root = kg_base::Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
        let state = GlobalState::new();
        state.set_roots(vec![root.clone()]);
        (dir, state, root.join("src"))
    }

    /// Opens `name` under `src` with its text from `files`.
    pub(crate) fn open_one(state: &GlobalState, src: &kg_base::Utf8Path, files: &[(&str, &str)], name: &str) -> Uri {
        let uri = crate::uri::path_to_uri(&src.join(name)).unwrap();
        let text = files.iter().find(|(n, _)| *n == name).expect("a file of the fixture").1;
        state.did_open(uri.clone(), text.to_owned(), 1);
        uri
    }

    /// (a) A use in a file nobody has open is found, in a file that imports
    /// the declaring module and in one that only reaches it through a module
    /// re-exposing it — each file searched under the program it roots, which
    /// is what decides whether the module is visible there. Red with the
    /// search kept to the open buffers' programs, with a declaration inside
    /// `module helper { … }` not taken for one another file can name, or with
    /// only the files that `import helper` themselves searched.
    #[test]
    fn a_use_in_a_file_nobody_opened_is_found_through_its_own_program() {
        let (dir, state, src) = lay_out("unopened", MODULES);
        let helper = open_one(&state, &src, MODULES, "helper.k");
        assert_eq!(
            sorted(refs(&state, &helper, Position::new(2, 12), true)),
            vec![
                ("helper.k".to_owned(), 0, 16),
                ("helper.k".to_owned(), 2, 12),
                ("main.k".to_owned(), 4, 4),
                ("user.k".to_owned(), 4, 4),
            ],
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The over-resolution trap, divergence #19's shape: `other.k` spells
    /// `twice` too and does not import `helper.k`, whose `twice` is in the
    /// namespace with no name that `other.k` is in. Searched under its own
    /// program — `extra`'s, where its `twice` is `extra`'s — it holds no use.
    /// Rooted with the declaring file, as one program over the workspace
    /// would root it, the name would reach `helper.k`'s through that shared
    /// namespace (`Main.hs:70,80`, `Symbols.hs:37-41`) and be reported. Red
    /// with each file searched under the cursor's file and itself together.
    pub(crate) const TRAP: &[(&str, &str)] = &[
        ("helper.k", HELPER),
        ("main.k", IMPORTER),
        ("extra.k", "module extra { twice }\n\ninline void twice()\n{\n}\n"),
        ("other.k", "import extra\n\ninline void elsewhere()\n{\n    twice();\n}\n"),
    ];

    /// (c) The trap's references: `helper.k`'s and `main.k`'s, never
    /// `other.k`'s or `extra.k`'s.
    #[test]
    fn a_file_that_does_not_import_the_declaration_holds_none_of_its_uses() {
        let (dir, state, src) = lay_out("trap", TRAP);
        let helper = open_one(&state, &src, TRAP, "helper.k");
        assert_eq!(
            sorted(refs(&state, &helper, Position::new(0, 12), true)),
            vec![("helper.k".to_owned(), 0, 12), ("main.k".to_owned(), 4, 4)],
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// (d) Past its bound the search stops, and says which of the project's
    /// files that spell the name it did not read — by count, after the
    /// first of them by path (`main.k`), and by the clock, before any. Red
    /// with the bound never checked.
    #[test]
    fn a_search_past_its_bound_names_what_it_did_not_read() {
        let (dir, state, src) = lay_out("bound", MODULES);
        let helper = open_one(&state, &src, MODULES, "helper.k");
        let names = |files: &[FileId]| -> Vec<String> {
            files.iter().map(|&f| state.vfs().path(f).unwrap().file_name().unwrap().to_owned()).collect()
        };
        let at = Position::new(2, 12);
        let mut unread = Vec::new();
        let (forever, never) = (std::time::Duration::MAX, std::time::Duration::ZERO);
        for bound in [
            Bound { files: 1, time: forever, until_unsure: false },
            Bound { files: 512, time: never, until_unsure: false },
        ] {
            let listed: Vec<(String, u32, u32)> = references_within(&state, &params(&helper, at, true), bound)
                .unwrap()
                .into_iter()
                .map(|l| (l.uri.as_str().rsplit('/').next().unwrap().to_owned(), l.range.start.line, l.range.start.character))
                .collect();
            let file = state.file_id(&helper).unwrap();
            let snap = state.snapshot(&helper).unwrap();
            let offset = convert::offset(snap.line_index(), PositionEncoding::Utf16, at);
            let found = state
                .with_db(file, |db| {
                    let target = db.definition_at(&[file], file, offset).unwrap();
                    search(db, &[file], &target, "twice", bound)
                })
                .unwrap();
            assert_eq!(found.hits.len(), listed.len());
            unread.push((names(&found.unsearched), listed.iter().any(|(f, ..)| f == "main.k")));
        }
        assert_eq!(unread, vec![(vec!["user.k".to_owned()], true), (vec!["main.k".to_owned(), "user.k".to_owned()], false)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A search holds no program of a file it read for itself once it is
    /// done: `main.k`'s, asked for again, is loaded again. Red with the
    /// search's programs kept, which would leave up to 64 of them behind it
    /// — each with its module graph and a symbol map over its program.
    #[test]
    fn a_search_leaves_no_program_of_a_file_it_read_behind() {
        let (dir, state, src) = lay_out("forget", MODULES);
        let helper = open_one(&state, &src, MODULES, "helper.k");
        assert_eq!(refs(&state, &helper, Position::new(2, 12), true).len(), 4);
        let file = state.file_id(&helper).unwrap();
        let reloaded = state
            .with_db(file, |db| {
                let main = db.file_id(&src.join("main.k"));
                let before = db.stats();
                db.program(&[main]);
                db.stats().since(&before).misses(kg_db::stats::Query::Program)
            })
            .unwrap();
        assert_eq!(reloaded, 1, "the search kept `main.k`'s program");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
