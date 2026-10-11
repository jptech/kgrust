//! Sorted IR over a loaded program — the cross-file half of path sorting.
//!
//! `kg_hir::lower_sorted` resolves names against an incremental table plus an
//! [`ImportSymbols`] door; this module is that door for a real program. It
//! lowers every module of a [`crate::program::Program`] **in import
//! topological order** — `Program::files` is already a post-order DFS with
//! the base library first, the same order the reference parses in
//! (`parseImports` finishes every import before the importing file's body,
//! `Parser.hs:194-207`) — and harvests each sorted IR's exportable facts
//! ([`kg_hir::sort::harvest_shapes`]) for the modules that import it.
//!
//! Its callers are the loop (`crate::looped`, which forks each module's IR
//! and so leaves the memoized one untouched), `kg dump ir` on a file, and
//! `cargo xtask ir`'s oracle differential.

use std::cell::RefCell;
use std::sync::Arc;

use kg_base::{FileId, FxHashMap, SymbolPath};
use kg_hir::sort::{ExternalHit, Fingerprint, Harvested, ImportSymbols, SymShape};
use kg_hir::ModuleIr;
use kg_resolve::{DefKind, ItemId, ModuleGraph, ModuleSource};
use kg_syntax::Parse;

use crate::db::Db;
use crate::program::Program;

/// Every module of the program, lowered with paths sorted.
#[derive(Debug)]
pub struct SortedProgram {
    /// `Program::files` — import topological order, base first.
    pub files: Vec<ModuleSource>,
    pub irs: FxHashMap<ModuleSource, Arc<ModuleIr>>,
    /// The parse each module was lowered from: what its ranges are ranges
    /// of (`crate::looped::EditorLoop` maps them onto a later text).
    pub parses: FxHashMap<ModuleSource, Arc<Parse>>,
}

impl SortedProgram {
    pub fn ir(&self, module: ModuleSource) -> Option<&Arc<ModuleIr>> {
        self.irs.get(&module)
    }
}

/// Lower the whole program rooted at `roots` with paths sorted — each module
/// through its memo ([`SortedMemo`]), in `Program::files`' order, so every
/// module a lowering can ask about has been harvested before it is asked.
pub fn sorted_program(db: &Db, roots: &[FileId]) -> SortedProgram {
    let program = db.program(roots);
    let _program = db.stats.missed(crate::stats::Query::Sorted);
    let mut shapes: FxHashMap<ModuleSource, Arc<FxHashMap<SymbolPath, Harvested>>> =
        FxHashMap::default();
    let mut irs: FxHashMap<ModuleSource, Arc<ModuleIr>> = FxHashMap::default();
    let mut parses: FxHashMap<ModuleSource, Arc<Parse>> = FxHashMap::default();

    for &module in &program.files {
        db.cancel_token().check();
        let parse = db.parse_module(module);
        let view = ProgramImports {
            graph: &program.graph,
            origin: module,
            program: &program,
            shapes: &shapes,
        };
        let memo = match db.sorted_memo(module) {
            Some(memo) if memo.answers(&parse, &view) => {
                db.stats.hit(crate::stats::Query::SortedModule);
                memo
            }
            _ => {
                let memo = Arc::new(lower_module(db, module, parse, &view));
                db.set_sorted_memo(module, memo.clone());
                memo
            }
        };
        shapes.insert(module, memo.harvest.clone());
        irs.insert(module, memo.ir.clone());
        parses.insert(module, memo.parse.clone());
    }

    SortedProgram { files: program.files.clone(), irs, parses }
}

/// One module's sorted lowering, its harvest, and everything the lowering
/// read — the per-module memo of sprint W7.3.
///
/// **The key is what `kg_hir::lower_sorted` reads, by value.** Its inputs
/// are the module's parse, its file id (fixed by the module), the interner
/// (append-only, so no symbol it holds changes id), and whatever the import
/// door answers: nothing else crosses into the lowering. The door's answer
/// is the module graph's `find_in_imports` (alias stripping, re-exports),
/// and of each declaration it names the item tree's qualified name, scope
/// and kind, the harvested shape and [`kg_hir::sort::fingerprint`], and
/// the `ItemId` as `nub`'s token. Rather than list those per module the
/// graph can reach, the memo keeps every distinct question the lowering put
/// to the door with the answer it got, and is valid when the parse is the
/// same and the door, asked each question again, answers each the same: a
/// deterministic walk given the same answers asks the same questions next.
///
/// **What of an answer was read.** Its length and each hit's shape and
/// declaring scope always. Its tokens and fingerprints only where `nub`
/// weighed it against another hit (`SymbolTable.hs:229-237`, `SymTab::one`,
/// which the lowering reports through `ImportSymbols::compared`) — and
/// those two are what move with an edit that is not the importer's
/// business: the fingerprint hashes the whole declared tree, body included
/// (`nub` compares trees, `Parser/Syntax.hs:594-599`), and the token is an
/// `ItemId`, an index that a declaration added above it — a local in a body,
/// even — moves. So an answer no `nub` compared is checked without them,
/// and a library body edit re-lowers an importer only when the importer
/// weighed the edited declaration against another, or its shape changed.
///
/// The parse is compared by pointer, which is its value here: the parse
/// memo hands back the old `Arc` when a reparse is equal (`Db::parse_module`).
#[derive(Debug)]
pub(crate) struct SortedMemo {
    parse: Arc<Parse>,
    asked: Vec<(SymbolPath, Answer)>,
    ir: Arc<ModuleIr>,
    harvest: Arc<FxHashMap<SymbolPath, Harvested>>,
}

/// One question's answer as the lowering got it, and whether `nub` read its
/// tokens and fingerprints.
#[derive(Debug)]
struct Answer {
    hits: Vec<ExternalHit>,
    compared: bool,
}

impl Answer {
    /// Whether `now` reads, to the lowering, as this answer did.
    fn reads_as(&self, now: &[ExternalHit]) -> bool {
        if self.compared {
            return self.hits == now;
        }
        self.hits.len() == now.len()
            && self.hits.iter().zip(now).all(|(a, b)| a.shape == b.shape && a.decl_scope == b.decl_scope)
    }
}

impl SortedMemo {
    /// Whether this lowering is the one `parse` and `door` would make now.
    fn answers(&self, parse: &Arc<Parse>, door: &dyn ImportSymbols) -> bool {
        Arc::ptr_eq(&self.parse, parse) && self.asked.iter().all(|(key, answer)| answer.reads_as(&door.find(key)))
    }
}

fn lower_module(db: &Db, module: ModuleSource, parse: Arc<Parse>, door: &dyn ImportSymbols) -> SortedMemo {
    let _miss = db.stats.missed(crate::stats::Query::SortedModule);
    let file_id = match module {
        ModuleSource::File(id) => id,
        ModuleSource::Cmdargs | ModuleSource::Options => FileId::SYNTHETIC,
    };
    let recorder = Recorder { door, asked: RefCell::default() };
    let ir = Arc::new(kg_hir::lower_sorted(&parse.source_file(), file_id, db.interner(), &recorder));
    let harvest = Arc::new(kg_hir::sort::harvest_shapes(&ir, db.shared_interner()));
    // What [`Answer::reads_as`] will read: the fingerprints of an answer
    // `nub` compared, as values — a deferred one would keep the imported
    // module's IR of this moment alive for as long as the memo — and of
    // the rest nothing.
    let asked = recorder
        .asked
        .into_inner()
        .into_iter()
        .map(|(key, mut answer)| {
            for hit in &mut answer.hits {
                hit.fingerprint = if answer.compared { hit.fingerprint.as_ref().map(Fingerprint::read) } else { None };
            }
            (key, answer)
        })
        .collect();
    SortedMemo { parse, asked, ir, harvest }
}

/// The import door, each distinct question's answer written down as the
/// lowering asks it. The door cannot change during one lowering, so a
/// question asked again gets the answer it got the first time.
struct Recorder<'a> {
    door: &'a dyn ImportSymbols,
    asked: RefCell<FxHashMap<SymbolPath, Answer>>,
}

impl ImportSymbols for Recorder<'_> {
    fn find(&self, key: &SymbolPath) -> Vec<ExternalHit> {
        if let Some(answer) = self.asked.borrow().get(key) {
            return answer.hits.clone();
        }
        let hits = self.door.find(key);
        self.asked.borrow_mut().insert(key.clone(), Answer { hits: hits.clone(), compared: false });
        hits
    }

    fn compared(&self, key: &SymbolPath) {
        if let Some(answer) = self.asked.borrow_mut().get_mut(key) {
            answer.compared = true;
        }
    }
}

/// `findInImports` (`SymbolTable.hs:263-270`) for one module, answered by the
/// module graph — which owns alias stripping and the reverse iteration — and
/// fleshed out from the harvested shape of the defining module's sorted IR.
struct ProgramImports<'a> {
    graph: &'a ModuleGraph,
    origin: ModuleSource,
    program: &'a Program,
    /// Each module harvested so far, by module.
    shapes: &'a FxHashMap<ModuleSource, Arc<FxHashMap<SymbolPath, Harvested>>>,
}

impl ProgramImports<'_> {
    fn hit(&self, module: ModuleSource, item: ItemId) -> Option<ExternalHit> {
        let tree = self.program.tree(module)?;
        let data = tree.item(item)?;
        let harvested = self.shapes.get(&module).and_then(|m| m.get(&data.qualified_name));
        let (shape, fingerprint) = match harvested {
            Some(h) => (h.shape.clone(), Some(h.fingerprint.clone())),
            None => (fallback_shape(data.kind), None),
        };
        Some(ExternalHit {
            token: token(module, item),
            decl_scope: data.scope_name(),
            shape,
            fingerprint,
        })
    }
}

impl ImportSymbols for ProgramImports<'_> {
    fn find(&self, key: &SymbolPath) -> Vec<ExternalHit> {
        self.graph
            .find_in_imports(self.origin, key)
            .into_iter()
            .filter_map(|def| self.hit(def.module, def.item))
            .collect()
    }
}

/// A stable identity for `nub` (`SymbolTable.hs:231`).
fn token(module: ModuleSource, item: ItemId) -> u64 {
    let m: u64 = match module {
        ModuleSource::File(id) => u64::from(id.0),
        ModuleSource::Cmdargs => u64::from(u32::MAX - 1),
        ModuleSource::Options => u64::from(u32::MAX),
    };
    (m << 32) | u64::from(item.0)
}

/// The shape of a definition whose module was not harvested (unreachable in
/// practice: `Program::files` covers every import) or whose key the harvest
/// missed — the `DefKind`-level approximation.
fn fallback_shape(kind: DefKind) -> SymShape {
    match kind {
        DefKind::Struct | DefKind::Union | DefKind::Class | DefKind::Enum => SymShape::Type,
        DefKind::Alias => SymShape::Alias { target: None },
        DefKind::Function | DefKind::FunctionDecl => SymShape::Function,
        DefKind::Variable(_) => {
            SymShape::Variable { const_ty: false, type_name: None, fn_init: None }
        }
        // A `Template`'s wrapped kind is unknowable here; the harvest carries
        // the real answer, so this only fires for a key the harvest missed.
        DefKind::Template => SymShape::Template { function: false, type_like: false },
        _ => SymShape::OtherValue,
    }
}
