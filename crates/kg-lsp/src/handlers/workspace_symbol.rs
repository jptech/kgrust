//! `workspace/symbol` — a declaration anywhere in the programs the open
//! documents root, found by name.
//!
//! The search is every module of every open document's program
//! (`Db::program`): the open buffers and everything they import, a module
//! nobody has opened included, the way [`super::references`] searches. Each
//! module contributes its outline ([`super::document_symbol`]), flattened,
//! the container a symbol sits in kept as its `containerName` — so the two
//! features cannot disagree about what a file declares. A module of no open
//! document's program is not searched: the server would have to guess at the
//! settings it is compiled under — except under `[lsp] index-import-dirs`
//! (on by default), where every `.k` under the import directories is
//! searched too, under the settings of the project that names them, so a
//! declaration the open programs do not import yet is found by name.
//!
//! A name matches when the query's characters occur in it in order, case
//! ignored — the fuzzy matching every editor's symbol picker applies — and
//! the closest come first: an exact name, then a prefix, then a substring,
//! then the rest, the shorter name first within each.

use kg_base::FileId;
use kg_resolve::ModuleSource;
use lsp_types::{DocumentSymbol, Location, OneOf, WorkspaceSymbol, WorkspaceSymbolParams, WorkspaceSymbolResponse};

use crate::global_state::GlobalState;
use crate::handlers::{Answer, document_symbol, sources, uri_of};

/// Past this many matches the list is cut: a picker shows a screenful, and
/// an empty query over the base library names thousands.
pub const MAX_SYMBOLS: usize = 256;

pub fn workspace_symbol(state: &GlobalState, params: &WorkspaceSymbolParams) -> Answer<Option<WorkspaceSymbolResponse>> {
    let query = params.query.to_lowercase();
    let enc = state.encoding();
    let mut searched: kg_base::FxHashSet<FileId> = kg_base::FxHashSet::default();
    // (rank, name, symbol): ranked as they are found, sorted once.
    let mut found: Vec<(u8, WorkspaceSymbol)> = Vec::new();
    for document in state.open_documents() {
        let outlines = state.with_db(document.file, |db| {
            let program = db.program(&[document.file]);
            let mut files: Vec<FileId> =
                program.files.iter().filter_map(|m| if let ModuleSource::File(f) = m { Some(*f) } else { None }).collect();
            if db.settings().lsp.index_import_dirs {
                for dir in &db.settings().import_dirs {
                    sources::sources_under(dir, false, &mut |path| files.push(db.file_id(&path)));
                }
            }
            let mut out = Vec::new();
            for file in files {
                if !searched.insert(file) {
                    continue;
                }
                let Some(text) = state.vfs().read(file) else { continue };
                let parse = db.parse(file);
                out.push((file, document_symbol::document_symbols(&parse.syntax_node(), &text.line_index, enc)));
            }
            out
        });
        for (file, outline) in outlines.into_iter().flatten() {
            let Some(uri) = uri_of(state, file) else { continue };
            flatten(&outline, None, &mut |symbol, container| {
                let Some(rank) = rank(&query, &symbol.name) else { return };
                #[allow(deprecated)]
                found.push((
                    rank,
                    WorkspaceSymbol {
                        name: symbol.name.clone(),
                        kind: symbol.kind,
                        tags: None,
                        container_name: container.map(str::to_owned),
                        location: OneOf::Left(Location { uri: uri.clone(), range: symbol.selection_range }),
                        data: None,
                    },
                ));
            });
        }
    }
    found.sort_by(|(a, x), (b, y)| a.cmp(b).then(x.name.len().cmp(&y.name.len())).then_with(|| x.name.cmp(&y.name)));
    found.truncate(MAX_SYMBOLS);
    Ok(Some(WorkspaceSymbolResponse::Nested(found.into_iter().map(|(_, s)| s).collect())))
}

/// Every symbol of an outline with the name of the one it sits in, depth
/// first, in the outline's order.
fn flatten(symbols: &[DocumentSymbol], container: Option<&str>, each: &mut dyn FnMut(&DocumentSymbol, Option<&str>)) {
    for symbol in symbols {
        each(symbol, container);
        if let Some(children) = &symbol.children {
            flatten(children, Some(&symbol.name), each);
        }
    }
}

/// How closely `name` matches `query` (already lowercased), lower first —
/// `None` where the query's characters do not occur in it in order.
fn rank(query: &str, name: &str) -> Option<u8> {
    let name = name.to_lowercase();
    if query.is_empty() || name == query {
        return Some(0);
    }
    if name.starts_with(query) {
        return Some(1);
    }
    if name.contains(query) {
        return Some(2);
    }
    let mut rest = name.chars();
    query.chars().all(|q| rest.any(|c| c == q)).then_some(3)
}

#[cfg(test)]
mod tests {
    use lsp_types::{PartialResultParams, WorkDoneProgressParams};

    use super::*;

    const HELPER: &str = "module helper\n    { Accumulator\n    }\n\nclass Accumulator\n{\n    uint32 total;\n    inline void add(uint32 x)\n    {\n        total = total + x;\n    }\n}\n";
    const MAIN: &str = "import helper\n\ninline void main()\n{\n    static Accumulator acc;\n    acc.add(1);\n}\n";

    /// A project on disk, as `references`' tests lay one down: with no
    /// manifest `import helper` would not resolve, and the imported module
    /// would be missing for the wrong reason.
    fn project(tag: &str) -> (std::path::PathBuf, GlobalState, lsp_types::Uri) {
        let dir = std::env::temp_dir().join(format!("kg-lsp-wsym-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("kanagawa.toml"),
            "[project]\nimport-dirs = [\"src\"]\ntarget-device = \"mock\"\nimplicit-base = false\n",
        )
        .unwrap();
        std::fs::write(dir.join("src").join("helper.k"), HELPER).unwrap();
        std::fs::write(dir.join("src").join("main.k"), MAIN).unwrap();
        std::fs::write(dir.join("src").join("unused.k"), "module unused\n    { Spare\n    }\n\nstruct Spare\n{\n    bool b;\n}\n").unwrap();
        let root = kg_base::Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
        let state = GlobalState::new();
        state.set_roots(vec![root.clone()]);
        let main = crate::uri::path_to_uri(&root.join("src").join("main.k")).unwrap();
        state.did_open(main.clone(), MAIN.to_owned(), 1);
        (dir, state, main)
    }

    fn search(state: &GlobalState, query: &str) -> Vec<(String, Option<String>, String)> {
        let params = WorkspaceSymbolParams {
            query: query.to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let Some(WorkspaceSymbolResponse::Nested(found)) = workspace_symbol(state, &params).expect("answered") else {
            panic!("the nested form")
        };
        found
            .into_iter()
            .map(|s| {
                let OneOf::Left(at) = s.location else { panic!("a location") };
                let file = at.uri.as_str().rsplit('/').next().unwrap().to_owned();
                (s.name, s.container_name, file)
            })
            .collect()
    }

    /// `Accumulator` is declared in `helper.k`, which nobody opened — it is
    /// in `main.k`'s program by its import — and its member is reported
    /// under it. Red with the search limited to the open documents.
    #[test]
    fn a_declaration_in_an_imported_module_is_found() {
        let (dir, state, _main) = project("imported");
        let found = search(&state, "accum");
        assert_eq!(found, [("Accumulator".to_owned(), None, "helper.k".to_owned())]);
        let found = search(&state, "add");
        assert_eq!(found, [("add".to_owned(), Some("Accumulator".to_owned()), "helper.k".to_owned())]);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Under `[lsp] index-import-dirs` (the default) a module nothing
    /// imports is searched too, being under an import directory; with it
    /// off, only the open document's program is. Red with the setting
    /// read by nothing.
    #[test]
    fn the_import_directories_are_searched_when_indexed() {
        let (dir, state, _main) = project("indexed");
        assert_eq!(search(&state, "spare"), [("Spare".to_owned(), None, "unused.k".to_owned())]);
        std::fs::remove_dir_all(dir).ok();
        let (dir, state, main) = project("unindexed");
        std::fs::write(
            dir.join("kanagawa.toml"),
            "[project]\nimport-dirs = [\"src\"]\ntarget-device = \"mock\"\nimplicit-base = false\n\n[lsp]\nindex-import-dirs = false\n",
        )
        .unwrap();
        state.reload_config();
        state.did_open(main, MAIN.to_owned(), 2);
        assert_eq!(search(&state, "spare"), Vec::<(String, Option<String>, String)>::new());
        std::fs::remove_dir_all(dir).ok();
    }

    /// The closest names first: an exact match, a prefix, a substring, then
    /// the characters in order.
    #[test]
    fn matches_are_ranked_by_closeness() {
        assert_eq!(rank("add", "add"), Some(0));
        assert_eq!(rank("add", "Address"), Some(1));
        assert_eq!(rank("add", "readd"), Some(2));
        assert_eq!(rank("acm", "Accumulator"), Some(3));
        assert_eq!(rank("xyz", "Accumulator"), None);
        let (dir, state, _main) = project("ranked");
        let names: Vec<String> = search(&state, "a").into_iter().map(|(n, _, _)| n).collect();
        // Two prefixes, the shorter first, then three substrings — the
        // unimported module's `Spare` among them, its directory indexed —
        // ties of length in name order.
        assert_eq!(names, ["add", "Accumulator", "main", "Spare", "total"]);
        std::fs::remove_dir_all(dir).ok();
    }
}
