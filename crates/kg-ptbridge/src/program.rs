//! Driving a whole program through a backend: `compile`
//! (`ParseTree.hs:31-41`) — `InitCompiler`, the walk of the merged tree,
//! `Codegen`.
//!
//! The merged tree is `foldr1 append` over the modules in the order they
//! finished parsing (`Main.hs:80,106`), which is `Program::files`; the
//! reference's `_fileNames` and `Location._fileIndex` index that same list
//! (`ParseTree.hs:60`), with the two synthetic modules spelled `.cmdargs.k`
//! and `.options.k` (`Parser/IO.hs:100-101`).

use kg_backend_sys::options::OptionValues;
use kg_base::{FileId, LineIndex};
use kg_db::Db;
use kg_resolve::ModuleSource;

use crate::Backend;
use crate::walk::{Declined, ModuleCtx, Walker};

/// What to hand `Codegen` (`ParseTree.hs:32-37`).
#[derive(Clone, Debug)]
pub struct CodegenArgs {
    /// `show backend`: `"Sv"` or `"None"`.
    pub target: String,
    /// `--output`, or the base name of the last input file when empty.
    pub output: String,
    pub resource_usage: String,
    pub dgml: String,
    pub dgml_detailed: String,
}

/// How far a program got.
#[derive(Debug)]
pub struct Outcome {
    /// `InitCompiler`'s answer.
    pub initialised: bool,
    /// The node kind that stopped the walk, if one did.
    pub declined: Option<Declined>,
    /// `Codegen`'s answer, when it was reached.
    pub codegen: Option<bool>,
    /// The program's file list, as `_fileNames` carried it.
    pub file_names: Vec<String>,
}

/// The file name the reference gives a module in `_fileNames`: the two
/// synthetic modules by their names, a file by its canonical path
/// (`canonicalizePath`, `Parser/IO.hs:105`) — which is also how the
/// backend spells it in a message.
pub fn file_name(db: &Db, module: ModuleSource) -> String {
    match module {
        ModuleSource::Cmdargs => ".cmdargs.k".to_owned(),
        ModuleSource::Options => ".options.k".to_owned(),
        ModuleSource::File(id) => db
            .path(id)
            .map(|p| std::fs::canonicalize(p.as_std_path()).map_or_else(|_| p.to_string(), |c| c.display().to_string()))
            .unwrap_or_default(),
    }
}

/// `compile`: initialise, walk every module in program order into one root
/// list, and run `Codegen` if asked. `options.file_names` is filled here.
pub fn run<B: Backend>(
    db: &Db,
    roots: &[FileId],
    mut options: OptionValues,
    codegen: Option<&CodegenArgs>,
    backend: &mut B,
) -> Outcome {
    let program = db.program(roots);
    let file_names: Vec<String> = program.files.iter().map(|&m| file_name(db, m)).collect();
    options.file_names = file_names.clone();
    let mut outcome = Outcome { initialised: false, declined: None, codegen: None, file_names };

    if !backend.InitCompiler(&options) {
        return outcome;
    }
    outcome.initialised = true;

    // The program-wide loop on the IR (`kg_sem::elaborate`,
    // `docs/history/ir-elaboration.md` §2.4): its final trees, typed by its
    // engine — the walk's only types and folds (sprint W3.6).
    let looped = db.looped(roots);
    let symbols = db.global_symbols(roots, kg_resolve::SymbolsConfig::AllSymbols);
    let verdicts = db.settings().verdicts;
    let ctxs: Vec<ModuleCtx> = program
        .files
        .iter()
        .enumerate()
        .filter_map(|(index, &module)| {
            let m = looped.module(module)?;
            let lines = LineIndex::new(&db.parse_module(module).text());
            let redundant = program.graph.redundant_imports(module).count();
            let sem = std::sync::Arc::new(kg_sem::Module { ir: m.ir.fork(), types: m.types.clone() });
            Some(
                ModuleCtx::new(module, std::sync::Arc::new(m.ir.fork()), sem, lines, index)
                    .with_redundant_imports(redundant)
                    .with_seed(verdicts.ptbridge_seed),
            )
        })
        .collect();
    walk_modules(backend, db, &symbols, &program, &ctxs, codegen, outcome)
}

/// The walk over every module's context, then `Codegen` if asked.
#[allow(clippy::too_many_arguments)]
fn walk_modules<B: Backend>(
    backend: &mut B,
    db: &Db,
    symbols: &kg_db::SymbolMap,
    program: &kg_db::program::Program,
    modules: &[ModuleCtx],
    codegen: Option<&CodegenArgs>,
    mut outcome: Outcome,
) -> Outcome {
    let mut items = Vec::new();
    let mut walker = Walker::new(backend, db.interner(), symbols, &program.graph, program.items(), modules);
    for ctx in modules {
        match walker.module_items(ctx.view()) {
            Ok(nodes) => items.extend(nodes),
            Err(declined) => {
                outcome.declined = Some(declined);
                return outcome;
            }
        }
    }
    let Some(last) = modules.last() else { return outcome };
    let root = walker.program_root(last.view(), &items);

    if let Some(c) = codegen {
        let ok = backend.Codegen(
            Some(&c.target),
            Some(&c.output),
            Some(&c.resource_usage),
            Some(&c.dgml),
            Some(&c.dgml_detailed),
            root,
        );
        outcome.codegen = Some(ok);
    }
    outcome
}

/// `outputBaseName` (`ParseTree.hs:48`): `--output`, else the base name of
/// the last input file.
pub fn output_base_name(output: &str, file_names: &[String]) -> String {
    if !output.is_empty() {
        return output.to_owned();
    }
    file_names
        .last()
        .map(|f| std::path::Path::new(f).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
        .unwrap_or_default()
}
