//! What find-references and rename cost across a project, measured in the
//! process that answers them: the reference's own library and its tests as
//! one project, a warm database, the handlers a client reaches.
//!
//! ```text
//! KG_REFERENCE=third_party/kanagawa cargo test -p kg-lsp --profile quick --test workspace_cost -- --ignored --nocapture
//! ```
//!
//! `KG_COST_CASE=<name>` runs one case. Ignored, and silent without
//! `KG_REFERENCE`: it reads a checkout this repository does not hold, and it
//! measures rather than asserts. The project is a scratch directory holding
//! a manifest and a link to each of the reference's `library` and
//! `test/library`, its import directories, under the `mock` device — the
//! shape `cargo xtask resilience` and the bench give the corpus; a manifest
//! governs only the files under it, and the reference tree is read-only.
//! Each case opens the declaring file and runs its diagnostics, as an editor
//! does on open, then asks references twice (the second reads the parses the
//! first left), `prepareRename` and `rename` (which takes the search the
//! first left it).
//!
//! Memory is the live heap, counted by [`kg_alloc::Counting`] — the server's
//! allocator, counted — around each request: what the request held at its
//! peak above what was live before it, and what it left live after. The
//! process's peak footprint is `/usr/bin/time -l` on this test binary.

use std::time::Instant;

use kg_alloc::Counting;
use kg_lsp::GlobalState;
use kg_lsp::handlers::{references, rename};
use lsp_types::{
    PartialResultParams, Position, ReferenceContext, ReferenceParams, RenameParams, TextDocumentIdentifier,
    TextDocumentPositionParams, Uri, WorkDoneProgressParams,
};

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MIB: f64 = 1024.0 * 1024.0;

/// The peak since `base` was read and the live heap now, in MiB above it.
fn heap(base: usize) -> (f64, f64) {
    let peak = Counting::take_peak();
    ((peak.saturating_sub(base)) as f64 / MIB, (Counting::live() as f64 - base as f64) / MIB)
}

/// What is live now, the next span's peak starting from it.
fn start_peak() -> usize {
    Counting::take_peak();
    Counting::live()
}

/// The case's declaration: the file under the reference, the text that
/// starts its line, and the name in it.
const CASES: &[(&str, &str, &str)] = &[
    // A file-scope template function, used through `test.unit`'s re-exposure
    // of `test.internal.assert` (`SymbolTable.hs:158-166`).
    ("library/test/internal/assert.k", "inline void assert_equal", "assert_equal"),
    // A method of a class template, used as `v.next(…)` through receivers.
    ("library/data/random/lfsr.k", "    value_t next(", "next"),
    // A method of a class, `.increment()`.
    ("library/data/counter.k", "    inline void increment()", "increment"),
    // A field of a struct template most of the library holds a value of.
    ("library/data/optional.k", "    bool is_valid;", "is_valid"),
    // A method the RISC-V tests call, each of whose programs is the
    // corpus's largest loop.
    ("library/processor/risc_v.k", "    inline void imem_write", "imem_write"),
];

#[test]
#[ignore = "measures against the reference checkout named by KG_REFERENCE"]
fn workspace_search_cost() {
    let Some(reference) = std::env::var_os("KG_REFERENCE") else {
        eprintln!("KG_REFERENCE is not set; nothing measured");
        return;
    };
    kg_alloc::purge_eagerly();
    let reference = std::fs::canonicalize(reference).expect("the reference checkout");
    let reference = camino::Utf8PathBuf::from_path_buf(reference).expect("a UTF-8 path");
    // A manifest governs the files under it (`kg_config::discover`), and the
    // reference tree is read-only, so the project is a scratch directory
    // holding the manifest and a link to each import directory: the files are
    // opened, and found, at their paths under it.
    let project = std::env::temp_dir().join(format!("kg-lsp-workspace-cost-{}", std::process::id()));
    std::fs::create_dir_all(&project).unwrap();
    std::os::unix::fs::symlink(reference.join("library"), project.join("library")).unwrap();
    std::os::unix::fs::symlink(reference.join("test/library"), project.join("tests")).unwrap();
    std::fs::write(
        project.join("kanagawa.toml"),
        "[project]\nimport-dirs = [\"library\", \"tests\"]\ntarget-device = \"mock\"\n",
    )
    .unwrap();
    let project = camino::Utf8PathBuf::from_path_buf(project).unwrap();

    let only = std::env::var("KG_COST_CASE").ok();
    for &(file, line, name) in CASES {
        if only.as_deref().is_some_and(|only| only != name) {
            continue;
        }
        let path = project.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let row = text.lines().position(|l| l.starts_with(line)).expect("the declaration's line");
        let column = text.lines().nth(row).unwrap().rfind(name).expect("the name") as u32;
        let at = Position::new(row as u32, column + 1);

        let state = GlobalState::new();
        state.set_roots(vec![project.clone()]);
        let uri = kg_lsp::uri::path_to_uri(&path).unwrap();
        state.did_open(uri.clone(), text, 1);
        let t = Instant::now();
        kg_lsp::handlers::publish_diagnostics(&state, &uri, |_| {}).expect("open");
        let opened = t.elapsed();

        eprintln!("\n== `{name}` ({file}); diagnostics on open {opened:.0?}");
        for round in ["references", "references again"] {
            let base = start_peak();
            let t = Instant::now();
            let found = references::references(&state, &refs(&uri, at)).expect("open");
            let took = t.elapsed();
            let (peak, left) = heap(base);
            let files: std::collections::BTreeSet<&str> = found.iter().map(|l| l.uri.as_str()).collect();
            eprintln!(
                "{round}: {} hits in {} files, {took:.0?}; heap peak +{peak:.1} MiB, left +{left:.1} MiB",
                found.len(),
                files.len()
            );
        }
        let base = start_peak();
        let t = Instant::now();
        let prepared = rename::prepare_rename(&state, &position(&uri, at));
        let took = t.elapsed();
        let (peak, left) = heap(base);
        eprintln!("prepareRename: {}, {took:.0?}; heap peak +{peak:.1} MiB, left +{left:.1} MiB", verdict(&prepared));
        let base = start_peak();
        let t = Instant::now();
        let renamed = rename::rename(
            &state,
            &RenameParams {
                text_document_position: position(&uri, at),
                new_name: format!("{name}_renamed"),
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        );
        let took = t.elapsed();
        let (peak, left) = heap(base);
        let edits = match &renamed {
            Ok(Some(edit)) => {
                let changes = edit.changes.as_ref().map_or(0, |c| c.len());
                let total: usize = edit.changes.iter().flat_map(|c| c.values()).map(Vec::len).sum();
                format!("{total} edits in {changes} files")
            }
            other => verdict(other),
        };
        eprintln!("rename: {edits}, {took:.0?}; heap peak +{peak:.1} MiB, left +{left:.1} MiB");
    }
    // The links first: `remove_dir_all` does not follow one, but the
    // reference is read-only, and that is not a promise to lean on.
    std::fs::remove_file(project.join("library")).unwrap();
    std::fs::remove_file(project.join("tests")).unwrap();
    std::fs::remove_dir_all(&project).unwrap();
}

fn verdict<T: std::fmt::Debug>(v: &Result<Option<T>, rename::Refusal>) -> String {
    match v {
        Ok(Some(_)) => "answered".to_owned(),
        Ok(None) => "null".to_owned(),
        Err(rename::Refusal::Because(why)) => format!("refused: {why}"),
        Err(other) => format!("{other:?}"),
    }
}

fn position(uri: &Uri, at: Position) -> TextDocumentPositionParams {
    TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: uri.clone() }, position: at }
}

fn refs(uri: &Uri, at: Position) -> ReferenceParams {
    ReferenceParams {
        text_document_position: position(uri, at),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext { include_declaration: true },
    }
}
