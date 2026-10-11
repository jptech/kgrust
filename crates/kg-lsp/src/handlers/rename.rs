//! `textDocument/prepareRename` and `textDocument/rename` — the reference
//! list, turned into an edit.
//!
//! # One algorithm, already written
//!
//! A rename is find-references with a replacement attached, and it is built
//! on exactly that: [`references::search`] — the open buffers' programs, then
//! every file of the project that spells the name and whose own program holds
//! the declaring module, each under that program — every hit resolved by
//! [`kg_db::Db::definition_at`] rather than matched by spelling. The trap that
//! module exists to avoid — two `x`es in nested scopes that are two symbols,
//! or a file that spells the name and never imports it — is the trap a rename
//! turns from a wrong answer into a corrupted program, so nothing here decides
//! anything by text.
//!
//! The edit reaches a file nobody has open: since 2026-10-10 the search reads
//! the project's files, so a rename of a declaration other files use edits
//! them, where it used to be refused until each was open. It also reaches the
//! declaring module's own exposure list (`module m { x }`), whose name stands
//! for the declaration (`checkSymbols`, `Parser.hs:216-223`) and would
//! otherwise be left naming one that is gone.
//!
//! # What a rename refuses, and why refusing is the feature
//!
//! A reference list may under-report and say so. An *edit* that under-reports
//! is a program that no longer compiles in a file the user did not have open.
//! So a rename is refused, with a message naming the reason, whenever the
//! search cannot promise to have seen every use:
//!
//! * **a search that stopped at its bound** ([`references::Bound::RENAME`]:
//!   512 files, or a minute) — the message counts the project's files that
//!   spell the name and were not read, and names one. The refusal is the
//!   safety net the whole design rests on: what the search did not read, the
//!   edit does not touch, and so the edit is not made;
//! * **a member with a use the search cannot place** — a `.f` spelled like
//!   it whose receiver no loop types outside a template's body (inside a
//!   lambda, say, or in a file whose loop could not type it), or whose
//!   instances reach this member and another (a `static if` declaring it in
//!   each branch), or which sits in a buffer edited since its loop ran, where
//!   a receiver is typed as the last state of the buffer that parsed had it
//!   (`kg_db::looped::EditorLoop`): the refusal names the file and line,
//!   since skipping the use and taking it would each leave some instance
//!   naming a member it does not have. A file nobody has open is typed by a
//!   loop of its own, run to the fixpoint for the search;
//! * **a declaration in the base library** is not the project's to rename;
//! * **a declaration in a generated module** (`.cmdargs`, `.options`) has no
//!   text to edit (`Parser/IO.hs:101-102`);
//! * **a new name that is not one identifier** — a keyword, a `uint32`, two
//!   words — is refused before any edit is built, as the protocol asks
//!   (`RenameParams::new_name`'s contract).
//!
//! A name inside an interpolated string's hole is no reason to refuse: the
//! hole is an ordinary expression in the tree, and its names are renamed like
//! any other (`a_use_inside_an_interpolated_string_is_renamed`).
//!
//! `prepareRename` makes the same search and applies the same refusals, so the
//! client declines *before* the user types a name, and answers `null` — not
//! an error — when the caret is simply not on a renameable name, which is
//! every case [`kg_db::Db::definition_at`] declines. It keeps the search for
//! the `rename` that follows ([`Remembered`]), which reads it while nothing
//! has changed: a member's search runs a loop per file, and the pair would
//! otherwise pay it twice.
//!
//! # What is not checked
//!
//! Whether the new name collides with one already in scope. The reference
//! reports that as a redefinition or a shadow the moment the edit lands, and
//! `publishDiagnostics` carries it to the same buffer; a second, weaker copy
//! of that check here would be a second opinion. And a file the search does
//! not cover at all ([`references`]' "What is *not* searched") — outside the
//! import directories and the manifest's root, or with no manifest — is no
//! more reached than before: a project without a manifest renames within the
//! open buffers' programs.

use std::collections::HashMap;
use std::sync::Arc;

use kg_base::{FileId, TextSize};
use kg_config::Settings;
use kg_db::{Db, Definition, Revision};
use kg_resolve::ModuleSource;
use kg_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use lsp_types::{
    PrepareRenameResponse, RenameParams, TextDocumentPositionParams, TextEdit, Uri, WorkspaceEdit,
};

use crate::convert;
use crate::global_state::{FileSnapshot, GlobalState, NoFile};
use crate::handlers::references::{self, Bound, Found};
use crate::sync::lock;
use crate::handlers::uri_of;

/// Why a rename did not happen. [`Refusal::NoFile`] is the ordinary "nothing
/// for that document" every handler has; [`Refusal::Because`] carries a
/// sentence for the user, and the dispatcher turns it into a request error so
/// the client shows it instead of silently doing nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NoFile(NoFile),
    Because(String),
}

impl From<NoFile> for Refusal {
    fn from(e: NoFile) -> Refusal {
        Refusal::NoFile(e)
    }
}

pub type Verdict<T> = Result<T, Refusal>;

/// The range and current spelling of the name the caret is on, when it can be
/// renamed; `None` when the caret is not on a name this server resolves.
pub fn prepare_rename(
    state: &GlobalState,
    params: &TextDocumentPositionParams,
) -> Verdict<Option<PrepareRenameResponse>> {
    prepare_within(state, params, Bound::RENAME)
}

/// [`prepare_rename`] under a bound of the caller's, for the tests that
/// reach it.
fn prepare_within(
    state: &GlobalState,
    params: &TextDocumentPositionParams,
    bound: Bound,
) -> Verdict<Option<PrepareRenameResponse>> {
    let snap = state.snapshot(&params.text_document.uri)?;
    let enc = state.encoding();
    let offset = convert::offset(snap.line_index(), enc, params.position);
    let buffers = references::buffers_of_this_project(state, &snap.settings.root, snap.file);

    let answer = state.with_db(snap.file, |db| {
        let Some((target, token)) = target(db, &snap, offset) else {
            return Ok(None);
        };
        refuse(db, &snap, &target)?;
        let key = Key { settings: Arc::clone(&snap.settings), revision: db.revision(), buffers, target, bound };
        let found = searched(state, db, &key)?;
        *lock(state.remembered()) = Some(Remembered { key, found });
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: convert::range(snap.line_index(), enc, token.text_range()),
            placeholder: token.text().to_owned(),
        }))
    });
    answer.unwrap_or(Ok(None))
}

/// Every occurrence the search resolves to the declaration under the caret,
/// rewritten to `new_name`, grouped by document.
pub fn rename(state: &GlobalState, params: &RenameParams) -> Verdict<Option<WorkspaceEdit>> {
    rename_within(state, params, Bound::RENAME)
}

/// [`rename`] under a bound of the caller's, for the tests that reach it.
fn rename_within(state: &GlobalState, params: &RenameParams, bound: Bound) -> Verdict<Option<WorkspaceEdit>> {
    let at = &params.text_document_position;
    let snap = state.snapshot(&at.text_document.uri)?;
    let enc = state.encoding();
    let offset = convert::offset(snap.line_index(), enc, at.position);
    let buffers = references::buffers_of_this_project(state, &snap.settings.root, snap.file);

    let answer = state.with_db(snap.file, |db| {
        let Some((target, _)) = target(db, &snap, offset) else {
            return Ok(None);
        };
        refuse(db, &snap, &target)?;
        one_identifier(&params.new_name)?;
        let key = Key { settings: Arc::clone(&snap.settings), revision: db.revision(), buffers, target, bound };
        let found = searched(state, db, &key)?;

        // `WorkspaceEdit::changes` is `HashMap<Uri, Vec<TextEdit>>` by the
        // protocol's own definition; the key's interior mutability is
        // `fluent_uri`'s, and nothing here mutates a key.
        #[allow(clippy::mutable_key_type)]
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        for hit in found.hits {
            // A hit in a file with no URI is a hit in a generated module,
            // which `refuse` already excluded for the declaration; a *use* in
            // one cannot happen, since nothing is written there by hand. Stated
            // as a skip rather than an unwrap for the day that changes.
            let Some(uri) = uri_of(state, hit.file) else { continue };
            // The target file's own line table, read from the VFS as
            // `handlers::location` reads it: the same text `Db::parse` saw,
            // unsaved overlay included.
            let Some(text) = state.vfs().read(hit.file) else { continue };
            changes.entry(uri).or_default().push(TextEdit {
                range: convert::range(&text.line_index, enc, hit.range),
                new_text: params.new_name.clone(),
            });
        }
        for edits in changes.values_mut() {
            edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));
        }
        Ok(Some(WorkspaceEdit { changes: Some(changes), ..WorkspaceEdit::default() }))
    });
    answer.unwrap_or(Ok(None))
}

/// The declaration under the caret and the identifier token the caret is on.
///
/// The file is its own root, exactly as `handlers::definition`,
/// `handlers::references` and `handlers::document_highlight` do it: the
/// declaration a rename rewrites must be the one a jump at the same caret
/// lands on, or the two features describe different programs.
fn target(db: &Db, snap: &FileSnapshot, offset: TextSize) -> Option<(Definition, SyntaxToken)> {
    let target = db.definition_at(&[snap.file], snap.file, offset)?;
    let token = ident_at(&snap.parse.syntax_node(), offset)?;
    Some((target, token))
}

/// The identifier the caret is inside or immediately after — the same rule
/// [`kg_db::Db::definition_at`] resolves by, so the range handed back for the
/// client to edit in place is the token that produced the answer.
fn ident_at(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    // `token_at_offset` panics outside the tree, and a client is entitled to
    // ask about any position.
    if offset > root.text_range().end() {
        return None;
    }
    root.token_at_offset(offset).find(|t| t.kind() == SyntaxKind::IDENT)
}

/// The spelling of `target`'s name, for the messages.
fn name_of(db: &Db, target: &Definition) -> String {
    target.qualified_name.last().map(|s| db.interner().resolve(s).to_string()).unwrap_or_default()
}

/// The two grounds on which a resolvable name is refused before anything is
/// searched: where it is declared.
fn refuse(db: &Db, snap: &FileSnapshot, target: &Definition) -> Verdict<()> {
    let name = name_of(db, target);
    if target.range.file == FileId::SYNTHETIC {
        return Err(Refusal::Because(format!(
            "`{name}` is declared in a generated module (`.cmdargs`/`.options`) and has no text to edit"
        )));
    }
    let program = db.program(&[snap.file]);
    if program.base.and_then(ModuleSource::file) == Some(target.range.file) {
        return Err(Refusal::Because(format!(
            "`{name}` is declared in the base library, which is not this project's to rename"
        )));
    }
    Ok(())
}

/// A search a `prepareRename` made, kept for the `rename` the client sends
/// after it about the same caret: past the open buffers a search can take
/// seconds — a member's runs a loop per file — and the two would otherwise
/// each pay it. One slot, taken by the `rename` that matches it.
#[derive(Debug)]
pub(crate) struct Remembered {
    key: Key,
    found: Found,
}

/// What a remembered search answers for: the same project's database at the
/// same revision — which every edit, every file changed on disk and every
/// buffer opened or closed moves — the same open buffers, the same
/// declaration and the same bound. The settings are held, so a reloaded
/// project's cannot share their address.
#[derive(Debug)]
struct Key {
    settings: Arc<Settings>,
    revision: Revision,
    buffers: Vec<FileId>,
    target: Definition,
    bound: Bound,
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        Arc::ptr_eq(&self.settings, &other.settings)
            && self.revision == other.revision
            && self.buffers == other.buffers
            && self.target == other.target
            && self.bound == other.bound
    }
}

/// The search a rename edits by ([`references::search`]) — the one a
/// `prepareRename` left for `key` where there is one — or the reason it
/// cannot promise to have seen every use: a `.name` it could not place, or
/// a file of the project that spells the name and that the bound left
/// unread.
fn searched(state: &GlobalState, db: &Db, key: &Key) -> Verdict<Found> {
    let name = name_of(db, &key.target);
    let kept = lock(state.remembered()).take_if(|r| r.key == *key).map(|r| r.found);
    let found = kept.unwrap_or_else(|| references::search(db, &key.buffers, &key.target, &name, key.bound));
    if let Some(unsure) = found.unsure {
        let at = state.vfs().read(unsure.file).map(|t| t.line_index.line_col(unsure.at).line + 1);
        let file = state.vfs().path(unsure.file).and_then(|p| p.file_name().map(str::to_owned)).unwrap_or_default();
        return Err(Refusal::Because(format!(
            "`.{name}` at {file}:{} {}, so the rename cannot tell whether it is a use",
            at.unwrap_or_default(),
            unsure.why
        )));
    }
    if let Some(&first) = found.unsearched.first() {
        let shown = shown_in_project(db, first);
        return Err(Refusal::Because(format!(
            "`{name}` is spelled in {} of the project's files that the search, stopping at {} files or {:?}, \
             did not read — `{shown}` among them; the rename cannot promise to reach every use",
            found.unsearched.len(),
            key.bound.files,
            key.bound.time
        )));
    }
    Ok(found)
}

/// `file`'s path relative to the import directory or project root it sits
/// under, for a message; the whole path where it sits under none.
fn shown_in_project(db: &Db, file: FileId) -> String {
    let Some(path) = db.path(file) else { return String::new() };
    let settings = db.settings();
    settings
        .import_dirs
        .iter()
        .chain(std::iter::once(&settings.root))
        .find_map(|dir| path.strip_prefix(dir).ok())
        .map_or_else(|| path.to_string(), |p| p.to_string())
}

/// `new_name` lexes as exactly one identifier — not a keyword, not `uint32`,
/// not two words, not empty. The lexer is the authority, so the rule is the
/// lexer's (`Parser/Lexer.hs:221-222`, Unicode-aware) rather than a copy of it.
fn one_identifier(new_name: &str) -> Verdict<()> {
    let lexed = kg_parser::LexedStr::new(new_name);
    let kinds: Vec<SyntaxKind> =
        (0..lexed.len()).map(|i| lexed.kind(i)).filter(|k| !k.is_trivia()).collect();
    if lexed.errors().is_empty() && kinds == [SyntaxKind::IDENT] {
        Ok(())
    } else {
        Err(Refusal::Because(format!("`{new_name}` is not an identifier")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_state::test_uri;
    use lsp_types::{Position, TextDocumentIdentifier, WorkDoneProgressParams};

    fn at(uri: &Uri, position: Position) -> TextDocumentPositionParams {
        TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: uri.clone() }, position }
    }

    fn params(uri: &Uri, position: Position, new_name: &str) -> RenameParams {
        RenameParams {
            text_document_position: at(uri, position),
            new_name: new_name.to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        }
    }

    /// The edit as `(file name, line, column, new text)`, sorted, which is what
    /// a fixture can be read against.
    fn edits(edit: WorkspaceEdit) -> Vec<(String, u32, u32, String)> {
        let mut out: Vec<_> = edit
            .changes
            .unwrap_or_default()
            .into_iter()
            .flat_map(|(uri, edits)| {
                let name = uri.as_str().rsplit('/').next().unwrap_or_default().to_owned();
                edits.into_iter().map(move |e| (name.clone(), e.range.start.line, e.range.start.character, e.new_text))
            })
            .collect();
        out.sort();
        out
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

    /// The same fixture `references` proves itself on: two `x`es, one
    /// shadowing the other.
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

    /// The corruption a textual rename would commit: renaming the outer `x`
    /// must leave the inner block's three `x`es alone, and vice versa.
    #[test]
    fn renaming_a_shadowed_binding_leaves_the_shadowing_one_alone() {
        let (state, uris) = open(&[("shadow.k", SHADOW)]);
        let f = "shadow.k".to_owned();
        let outer = rename(&state, &params(&uris[0], Position::new(2, 11), "count")).unwrap().unwrap();
        assert_eq!(
            edits(outer),
            vec![
                (f.clone(), 2, 11, "count".to_owned()),
                (f.clone(), 3, 4, "count".to_owned()),
                (f.clone(), 3, 8, "count".to_owned()),
                (f.clone(), 8, 4, "count".to_owned()),
                (f.clone(), 8, 8, "count".to_owned()),
            ]
        );
        let inner = rename(&state, &params(&uris[0], Position::new(6, 12), "flag")).unwrap().unwrap();
        assert_eq!(
            edits(inner),
            vec![
                (f.clone(), 5, 13, "flag".to_owned()),
                (f.clone(), 6, 8, "flag".to_owned()),
                (f.clone(), 6, 12, "flag".to_owned()),
            ]
        );
    }

    /// `prepareRename` hands back the token under the caret and its spelling,
    /// so the client's inline box starts on the right text; off a name it is
    /// `null`, never an error.
    #[test]
    fn prepare_answers_the_token_or_nothing() {
        let (state, uris) = open(&[("shadow.k", SHADOW)]);
        let on_a_use = prepare_rename(&state, &at(&uris[0], Position::new(3, 9))).unwrap();
        assert_eq!(
            on_a_use,
            Some(PrepareRenameResponse::RangeWithPlaceholder {
                range: lsp_types::Range::new(Position::new(3, 8), Position::new(3, 9)),
                placeholder: "x".to_owned(),
            })
        );
        assert_eq!(prepare_rename(&state, &at(&uris[0], Position::new(1, 0))).unwrap(), None);
        assert_eq!(prepare_rename(&state, &at(&uris[0], Position::new(999, 0))).unwrap(), None);
    }

    /// A name the lexer would not read as one identifier is refused with a
    /// message, before any edit exists: a keyword, a built-in type spelling,
    /// two words, nothing.
    #[test]
    fn a_new_name_that_is_not_an_identifier_is_refused() {
        let (state, uris) = open(&[("shadow.k", SHADOW)]);
        for bad in ["class", "uint32", "two words", "", "3x", "a-b"] {
            let verdict = rename(&state, &params(&uris[0], Position::new(2, 11), bad));
            assert_eq!(verdict, Err(Refusal::Because(format!("`{bad}` is not an identifier"))), "{bad:?}");
        }
        assert!(rename(&state, &params(&uris[0], Position::new(2, 11), "über_1")).is_ok());
    }

    /// A string's hole is an ordinary expression in the tree, so a name in
    /// it is a use like any other and the rename rewrites it — the reference
    /// resolves it in the scope the string sits in. An escaped `{{total}}`
    /// is text, and a hole naming something else is left alone.
    #[test]
    fn a_use_inside_an_interpolated_string_is_renamed() {
        let src = "\
inline void main()
{
    const auto total = 3;
    const auto other = 4;
    const auto s = \"{total:x} {other} {{total}} {total + other=}\";
}
";
        let (state, uris) = open(&[("interp.k", src)]);
        let edit = rename(&state, &params(&uris[0], Position::new(2, 15), "sum")).unwrap().unwrap();
        assert_eq!(
            edits(edit),
            vec![
                ("interp.k".to_owned(), 2, 15, "sum".to_owned()),
                ("interp.k".to_owned(), 4, 21, "sum".to_owned()),
                ("interp.k".to_owned(), 4, 49, "sum".to_owned()),
            ]
        );
    }

    #[test]
    fn an_unopened_document_is_no_file() {
        let state = GlobalState::new();
        let uri = test_uri("never.k");
        assert_eq!(
            rename(&state, &params(&uri, Position::new(0, 0), "y")),
            Err(Refusal::NoFile(NoFile::NotOpen))
        );
        assert_eq!(prepare_rename(&state, &at(&uri, Position::new(0, 0))), Err(Refusal::NoFile(NoFile::NotOpen)));
    }

    const HELPER: &str = "inline void twice()\n{\n}\n";
    const IMPORTER: &str = "import helper\n\ninline void main()\n{\n    twice();\n}\n";

    /// A real project on disk, as `references`' own fixture lays one down and
    /// for the same reason: the fallback settings have no import directories,
    /// so an in-memory `import helper` would resolve nothing.
    fn project(tag: &str) -> (std::path::PathBuf, GlobalState, Uri, Uri) {
        let dir = std::env::temp_dir().join(format!("kg-lsp-rename-{tag}-{}", std::process::id()));
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

    /// `main.k` imports `helper.k` and is on disk but not open: a rename of
    /// `twice` from `helper.k` edits it there too, the search reading the
    /// project's files and `main.k`'s own program reaching the declaration;
    /// `prepareRename` answers as the rename does. Corrected 2026-10-10:
    /// until the search read the project's files this was refused, naming
    /// `main.k`, until `main.k` was open. Red with the search kept to the
    /// open buffers' programs, which edits `helper.k` alone.
    #[test]
    fn a_declaration_an_unopened_file_imports_is_renamed_there_too() {
        let (dir, state, helper, main) = project("importer");
        state.did_open(helper.clone(), HELPER.to_owned(), 1);
        let both = vec![
            ("helper.k".to_owned(), 0, 12, "thrice".to_owned()),
            ("main.k".to_owned(), 4, 4, "thrice".to_owned()),
        ];

        assert!(prepare_rename(&state, &at(&helper, Position::new(0, 12))).unwrap().is_some());
        let edit = rename(&state, &params(&helper, Position::new(0, 12), "thrice")).unwrap().unwrap();
        assert_eq!(edits(edit), both);

        state.did_open(main.clone(), IMPORTER.to_owned(), 1);
        let edit = rename(&state, &params(&helper, Position::new(0, 12), "thrice")).unwrap().unwrap();
        assert_eq!(edits(edit), both, "open or not, the same edit");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// (b) A rename from a module with a header — the shape of every module
    /// another file imports — edits every file of the project whose program
    /// reaches the declaration, none of them open: `main.k`, which imports
    /// `helper`, and `user.k`, which imports only `facade`, which re-exposes
    /// `helper` (`SymbolTable.hs:158-166`); and `helper`'s own exposure list,
    /// which would otherwise name a function that is gone ("undefined
    /// symbol", `Parser.hs:216-223`). Red with a declaration inside
    /// `module helper { … }` taken for a file-local one (the edit is then
    /// `helper.k`'s alone), or with the exposure list's name unresolved.
    #[test]
    fn a_rename_edits_every_file_whose_program_reaches_the_declaration() {
        use crate::handlers::references::tests::{MODULES, lay_out, open_one};
        let (dir, state, src) = lay_out("rename-modules", MODULES);
        let helper = open_one(&state, &src, MODULES, "helper.k");
        let at_decl = Position::new(2, 12);
        assert!(prepare_rename(&state, &at(&helper, at_decl)).unwrap().is_some());
        let edit = rename(&state, &params(&helper, at_decl, "thrice")).unwrap().unwrap();
        assert_eq!(
            edits(edit),
            vec![
                ("helper.k".to_owned(), 0, 16, "thrice".to_owned()),
                ("helper.k".to_owned(), 2, 12, "thrice".to_owned()),
                ("main.k".to_owned(), 4, 4, "thrice".to_owned()),
                ("user.k".to_owned(), 4, 4, "thrice".to_owned()),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// (c) The over-resolution trap (`references`' `TRAP`): `other.k` spells
    /// `twice`, imports `extra` and not `helper.k`, and is left alone — as is
    /// `extra.k`'s own `twice`. Red with each file searched under the
    /// cursor's file and itself together, which reaches `helper.k`'s `twice`
    /// from `other.k` through the namespace they share.
    #[test]
    fn a_rename_leaves_a_file_that_does_not_import_the_declaration_alone() {
        use crate::handlers::references::tests::{TRAP, lay_out, open_one};
        let (dir, state, src) = lay_out("rename-trap", TRAP);
        let helper = open_one(&state, &src, TRAP, "helper.k");
        let edit = rename(&state, &params(&helper, Position::new(0, 12), "thrice")).unwrap().unwrap();
        assert_eq!(
            edits(edit),
            vec![
                ("helper.k".to_owned(), 0, 12, "thrice".to_owned()),
                ("main.k".to_owned(), 4, 4, "thrice".to_owned()),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// (d) A rename whose search stopped at its bound is refused, naming a
    /// file it did not read — never an edit of the files it did — and so is
    /// its `prepareRename`. Red with the bound never checked.
    #[test]
    fn a_rename_past_the_search_s_bound_is_refused() {
        use crate::handlers::references::tests::{MODULES, lay_out, open_one};
        let (dir, state, src) = lay_out("rename-bound", MODULES);
        let helper = open_one(&state, &src, MODULES, "helper.k");
        let bound = Bound { files: 1, ..Bound::RENAME };
        let at_decl = Position::new(2, 12);
        for verdict in [
            rename_within(&state, &params(&helper, at_decl, "thrice"), bound).map(|_| ()),
            prepare_within(&state, &at(&helper, at_decl), bound).map(|_| ()),
        ] {
            match verdict {
                Err(Refusal::Because(message)) => {
                    assert!(message.contains("1 of the project's files") && message.contains("user.k"), "{message}")
                }
                other => panic!("expected a refusal naming user.k, got {other:?}"),
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `prepareRename` keeps its search for the `rename` after it, which
    /// reads it rather than searching again — no program of the files it
    /// read loaded a second time — unless the database has moved since: a
    /// use added to `main.k` on disk in between is renamed too. Red with
    /// the kept search never taken, or taken across a change.
    #[test]
    fn a_rename_takes_the_search_its_prepare_made_while_nothing_changed() {
        use crate::handlers::references::tests::{MODULES, lay_out, open_one};
        let (dir, state, src) = lay_out("rename-kept", MODULES);
        let helper = open_one(&state, &src, MODULES, "helper.k");
        let file = state.file_id(&helper).unwrap();
        let at_decl = Position::new(2, 12);
        let program_loads = || state.with_db(file, |db| db.stats().misses(kg_db::stats::Query::Program)).unwrap();

        assert!(prepare_rename(&state, &at(&helper, at_decl)).unwrap().is_some());
        let before = program_loads();
        assert_eq!(edits(rename(&state, &params(&helper, at_decl, "thrice")).unwrap().unwrap()).len(), 4);
        assert_eq!(program_loads(), before, "the rename searched again");

        assert!(prepare_rename(&state, &at(&helper, at_decl)).unwrap().is_some());
        let main = src.join("main.k");
        std::fs::write(&main, "import helper\n\ninline void main()\n{\n    twice();\n    twice();\n}\n").unwrap();
        state.disk_changed(&crate::uri::path_to_uri(&main).unwrap());
        assert_eq!(
            edits(rename(&state, &params(&helper, at_decl, "thrice")).unwrap().unwrap()).len(),
            5,
            "the rename read a search the change had made stale"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The scan is by *module*, so a file that imports something else is not
    /// a reason to refuse — and a local of `helper.k`'s function is never
    /// scanned for at all, since nothing outside the file can name it.
    #[test]
    fn an_unrelated_file_and_a_local_do_not_refuse() {
        let (dir, state, helper, _main) = project("unrelated");
        std::fs::write(dir.join("src").join("other.k"), "import helpers\n").unwrap();
        std::fs::write(dir.join("src").join("main.k"), "inline void main()\n{\n}\n").unwrap();
        let local = "inline void twice()\n{\n    uint32 n = 2;\n    n = n + 1;\n}\n";
        state.did_open(helper.clone(), local.to_owned(), 1);

        assert!(rename(&state, &params(&helper, Position::new(0, 12), "thrice")).unwrap().is_some());
        let edit = rename(&state, &params(&helper, Position::new(2, 11), "m")).unwrap().unwrap();
        assert_eq!(edits(edit).len(), 3);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A method is not at file scope, and still importable: `main.k` holds
    /// a `Counter` and calls `c.Go()` through it, and the search, reading
    /// `main.k` under its own program through a loop of its own, places the
    /// call and renames it with the declaration. Corrected 2026-10-10: until
    /// the search read the project's files this was refused, naming `main.k`.
    /// Red with no loop for a file nobody has open, which leaves `c.Go()`
    /// unplaced and the rename refused.
    #[test]
    fn a_method_an_unopened_file_calls_is_renamed_through_its_receiver() {
        let (dir, state, helper, _main) = project("method");
        let class = "class Counter\n{\npublic:\n    inline void Go()\n    {\n    }\n}\n";
        std::fs::write(dir.join("src").join("helper.k"), class).unwrap();
        std::fs::write(
            dir.join("src").join("main.k"),
            "import helper\n\ninline void main()\n{\n    static Counter c;\n    c.Go();\n}\n",
        )
        .unwrap();
        state.did_open(helper.clone(), class.to_owned(), 1);

        let edit = rename(&state, &params(&helper, Position::new(3, 16), "Step")).unwrap().unwrap();
        assert_eq!(
            edits(edit),
            vec![("helper.k".to_owned(), 3, 16, "Step".to_owned()), ("main.k".to_owned(), 5, 6, "Step".to_owned())]
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `.Go` in a file nobody has open whose receiver no loop types —
    /// `c` is what an undeclared function returns — cannot be placed, and
    /// the rename is refused, naming the file and line, as it is in an
    /// open buffer. Red with an unplaced use let through.
    #[test]
    fn a_method_use_an_unopened_file_holds_and_no_loop_places_refuses_the_rename() {
        let (dir, state, helper, _main) = project("unplaced");
        let class = "class Counter\n{\npublic:\n    inline void Go()\n    {\n    }\n}\n";
        std::fs::write(dir.join("src").join("helper.k"), class).unwrap();
        std::fs::write(
            dir.join("src").join("main.k"),
            "import helper\n\ninline void main()\n{\n    auto c = nothing();\n    c.Go();\n}\n",
        )
        .unwrap();
        state.did_open(helper.clone(), class.to_owned(), 1);

        match rename(&state, &params(&helper, Position::new(3, 16), "Step")) {
            Err(Refusal::Because(message)) => assert!(message.contains("`.Go` at main.k:6"), "{message}"),
            other => panic!("expected a refusal naming main.k:6, got {other:?}"),
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A use the open buffer itself cannot place refuses the rename before
    /// any file of the project is read: `main.k`, which the search would
    /// otherwise load and type through a loop of its own, is never loaded.
    /// Red with the search going on past the first unplaced use.
    #[test]
    fn a_rename_refused_in_the_open_buffer_reads_no_other_file() {
        let (dir, state, helper, _main) = project("refused-early");
        let class = "class Counter\n{\npublic:\n    inline void Go()\n    {\n    }\n}\n\n\
                     inline void poke()\n{\n    auto c = nothing();\n    c.Go();\n}\n";
        std::fs::write(dir.join("src").join("helper.k"), class).unwrap();
        std::fs::write(
            dir.join("src").join("main.k"),
            "import helper\n\ninline void main()\n{\n    static Counter c;\n    c.Go();\n}\n",
        )
        .unwrap();
        state.did_open(helper.clone(), class.to_owned(), 1);
        let file = state.file_id(&helper).unwrap();
        let program_loads = || state.with_db(file, |db| db.stats().misses(kg_db::stats::Query::Program)).unwrap();

        assert!(prepare_rename(&state, &at(&helper, Position::new(3, 16))).is_err());
        let before = program_loads();
        match rename(&state, &params(&helper, Position::new(3, 16), "Step")) {
            Err(Refusal::Because(message)) => assert!(message.contains("`.Go` at helper.k:12"), "{message}"),
            other => panic!("expected a refusal naming helper.k:12, got {other:?}"),
        }
        assert_eq!(program_loads(), before, "the search read `main.k`");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A buffer opened broken has no loop to read (a rename runs none), so
    /// `c.Go()` cannot be placed: renaming `Go`
    /// would skip it and leave the call naming a method that no longer
    /// exists. Refused, naming the line. Red with an unresolved use let
    /// through.
    #[test]
    fn a_method_use_the_editor_cannot_place_refuses_the_rename() {
        let src = "class Counter\n{\npublic:\n    inline void Go()\n    {\n    }\n}\n\n\
                   inline void main()\n{\n    static Counter c;\n    c.Go();\n    @@@\n}\n";
        let (state, uris) = open(&[("broken.k", src)]);
        match rename(&state, &params(&uris[0], Position::new(3, 16), "Step")) {
            Err(Refusal::Because(message)) => assert!(message.contains("`.Go` at broken.k:12"), "{message}"),
            other => panic!("expected a refusal naming the call, got {other:?}"),
        }
    }

    /// A buffer edited since its loop ran reads that loop through the edit,
    /// and a receiver there is typed as the last parsing state had it — the
    /// edit below retypes `c`, and the loop still says `Counter`. A rename
    /// of `Counter::Go` would then rename `c.Go()`, now `Engine`'s: refused,
    /// naming the line, until the buffer parses. Red with the stale reading
    /// let through (the edit renames the call).
    #[test]
    fn a_member_use_read_through_an_edit_refuses_the_rename() {
        let src = "class Counter\n{\npublic:\n    inline void Go()\n    {\n    }\n}\n\n\
                   class Engine\n{\npublic:\n    inline void Go()\n    {\n    }\n}\n\n\
                   inline void main()\n{\n    static Counter c;\n    c.Go();\n}\n";
        let (state, uris) = open(&[("main.k", src)]);
        let file = state.snapshot(&uris[0]).expect("open").file;
        state.with_db(file, |db| db.editor_loop(&[file], file).map(|l| l.is_current())).expect("open").expect("parses");
        let edited = src.replace("static Counter c;", "static Engine c;").replace("c.Go();\n", "c.Go();\n    @@@\n");
        state.did_change_full(&uris[0], edited, 2);
        match rename(&state, &params(&uris[0], Position::new(3, 16), "Step")) {
            Err(Refusal::Because(message)) => assert!(message.contains("`.Go` at main.k:20"), "{message}"),
            other => panic!("expected a refusal naming the call, got {other:?}"),
        }
    }

    /// Inside a template no program instantiates, a receiver has no type
    /// anywhere, and the `.Go` there is text this program does not
    /// compile: not refused — the base library is full of such bodies, and
    /// refusing on each would refuse most renames of a common member name.
    /// The call the loop does place is renamed. Red with an unresolved use
    /// inside a template refused.
    #[test]
    fn a_use_in_an_uninstantiated_template_does_not_refuse() {
        let src = "class Counter\n{\npublic:\n    inline void Go()\n    {\n    }\n}\n\n\
                   template <typename T>\ninline void poke(T t)\n{\n    t.Go();\n}\n\n\
                   inline void main()\n{\n    static Counter c;\n    c.Go();\n}\n";
        let (state, uris) = open(&[("poke.k", src)]);
        let edit = rename(&state, &params(&uris[0], Position::new(3, 16), "Step")).unwrap().unwrap();
        assert_eq!(
            edits(edit),
            vec![("poke.k".to_owned(), 3, 16, "Step".to_owned()), ("poke.k".to_owned(), 17, 6, "Step".to_owned())]
        );
    }
}
