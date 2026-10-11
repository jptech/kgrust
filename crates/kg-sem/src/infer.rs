//! `inferType` (`Type/Inference.hs:25-69`) over the program's IR: the
//! reference's own fixpoint on the tree it was written for.
//!
//! Each round builds the symbol map over the tree as the round found it
//! (`getSymbols' TemplateDecl p`, `:35`), then walks every module bottom-up
//! (`inferExpTree`, `:47-69`), applying at each node
//!
//! ```haskell
//! staticControl . unfix . evaluateConstexpr symbols . unfix . inferExpType symbols
//! ```
//!
//! and stops when the count of unresolved nodes stops changing (`:36-44`).
//! `evaluateConstexpr` and `staticControl` **rewrite the node in place under
//! its own note** — a folded `-128` keeps the type its negation was given —
//! which the engine reproduces by writing the node's expression in its own
//! arena (a fork of the lowering, so nothing shared is touched) and leaving
//! its [`TypeMap`] entry alone.
//!
//! Declarations are read from the round's snapshot, as `lookupSymbol` reads
//! the map built at the round's start: a declaration typed later in the same
//! round is seen the round after, exactly as the reference sees it.

use kg_base::{FxHashMap, Interner, Symbol, SymbolPath};
use kg_const::{BinOp, ConstInt, ConstValue, UnOp};
use kg_hir::{Exp, ExpArena, ExpId, ModuleIr};
use kg_ty::{FunctionKind, TAttr, TAttrSet, TFuncParam, Ty, TyRef};

use crate::reads::Asked;
use crate::symbols::{Decl, Symbols, declared_name};
use crate::types::TypeMap;

/// One module of the program: its tree, rewritten as inference goes, and the
/// type of every node.
#[derive(Debug)]
pub struct Module {
    pub ir: ModuleIr,
    pub types: TypeMap,
}

/// What [`Program::infer`] did.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub rounds: u32,
    /// The unresolved count per module at the stable round (`sig`).
    pub unresolved: Vec<u64>,
}

/// The whole program, as `inferType` sees it.
#[derive(Debug)]
pub struct Program<'i> {
    pub modules: Vec<Module>,
    interner: &'i Interner,
    /// Where an unrolled `static for`'s iterations are framed
    /// (`staticControl`, `Constexpr.hs:48-56`); none for a caller that
    /// types a finished tree.
    expansions: Option<&'i kg_base::ExpansionInterner>,
    /// Walk every module whole on every iteration — the fixpoint as first
    /// written, kept as the incremental one's reference
    /// (`docs/plans/performance.md` §3.6). Off unless the caller sets it —
    /// `kg_config::Verdicts::infer_full` (`KG_INFER_FULL`) through
    /// `kg_db`, or an identity check asking for both walks.
    pub full: bool,
    /// Each fixpoint's `sig`, summed, iteration by iteration.
    pub trace: Vec<Vec<u64>>,
    /// The incremental walk's units, kept from one fixpoint to the next: the
    /// passes between them only append nodes and re-root, so a unit whose
    /// subtree they left alone, and whose lookups the new symbol map
    /// answers as the old one did, is where it was.
    units: Option<Units>,
    /// The symbol map the units' remembered lookups were answered from.
    map: Option<Symbols>,
    /// Per module, how far `intrinsics` has looked: a node it typed stays
    /// typed (a resolved node is never inferred again, and the passes
    /// between fixpoints type only what they mint), so each fixpoint looks
    /// only at what is new.
    intrinsics_from: Vec<usize>,
    /// Checked between iterations and between modules (sprint W7.2).
    cancel: kg_base::cancel::Cancel,
    /// What every subtree files in each symbol map, kept between the maps
    /// the loop builds ([`Program::symbols`]).
    filed: crate::symbols::Filed,
    /// The last snapshot's storage, refreshed in place by the next
    /// ([`Program::snapshot`]). A fresh copy of every arena twice a round,
    /// freed after its pass, was most of the loop's memory: macOS's
    /// allocator keeps freed large blocks for reuse (`MallocLargeCache`),
    /// and each round's copies outgrew the last round's, so none was —
    /// 1.2 of `array.k`'s 1.4 GB peak was blocks freed and kept
    /// (2026-10-10, `vmmap`: "Malloc Large (empty)").
    snapshots: Vec<Snapshot>,
    /// Where a recorded run's passes show what each module reads across
    /// its boundary ([`crate::reads`]); none otherwise.
    pub(crate) door: Option<&'i crate::reads::Door>,
}

/// A program cancelled mid-run (`kg_base::cancel`) is freed on another
/// thread: `risc-v.k`'s holds millions of nodes, and freeing them on the
/// unwind was most of the time from the cancel to the caller's return.
impl Drop for Program<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() && !self.modules.is_empty() {
            let doomed =
                (std::mem::take(&mut self.modules), self.units.take(), self.map.take(), std::mem::take(&mut self.filed));
            std::thread::spawn(move || drop(doomed));
        }
    }
}

/// The guard on the fixpoint: the reference loops until `sig` repeats, which
/// its monotonicity guarantees; this bounds a port bug to a stop, not a hang.
const ROUND_GUARD: u32 = 64;

impl<'i> Program<'i> {
    /// The program over `irs`, in program order (imports first). Each tree is
    /// forked: inference rewrites its own copy.
    pub fn new(irs: impl IntoIterator<Item = ModuleIr>, interner: &'i Interner) -> Program<'i> {
        Program {
            modules: irs.into_iter().map(|ir| Module { ir, types: TypeMap::new() }).collect(),
            interner,
            expansions: None,
            full: false,
            trace: Vec::new(),
            units: None,
            map: None,
            intrinsics_from: Vec::new(),
            cancel: kg_base::cancel::Cancel::never(),
            filed: crate::symbols::Filed::default(),
            snapshots: Vec::new(),
            door: None,
        }
    }

    /// `getSymbols' config` over every module as it stands — kept per
    /// subtree between builds ([`crate::symbols::Filed`]), and on the whole
    /// walk ([`Program::full`]) built whole, as the reference the kept one
    /// is graded against. `KG_SYMBOLS_CHECK=1` builds both and compares.
    pub fn symbols(&mut self, config: crate::symbols::Config) -> Symbols {
        if self.full {
            return Symbols::with(self.modules.iter().map(|m| &m.ir), self.interner, config);
        }
        let symbols = self.filed.symbols(self.modules.iter().map(|m| &m.ir), self.interner, config);
        if crate::env_flag!("KG_SYMBOLS_CHECK") {
            let whole = Symbols::with(self.modules.iter().map(|m| &m.ir), self.interner, config);
            assert!(symbols == whole, "the kept {config:?} symbol map differs from the whole walk's");
        }
        symbols
    }

    /// The token the passes check per module (`kg_base::cancel`).
    pub fn cancel_token(&self) -> kg_base::cancel::Cancel {
        self.cancel.clone()
    }

    /// [`Program::new`], stopping at the next checkpoint once `cancel` is
    /// raised (`kg_base::cancel`).
    pub fn with_cancel(mut self, cancel: kg_base::cancel::Cancel) -> Program<'i> {
        self.cancel = cancel;
        self
    }

    /// [`Program::new`], unrolling `static for`s under `expansions`' frames.
    pub fn with_expansions(mut self, expansions: &'i kg_base::ExpansionInterner) -> Program<'i> {
        self.expansions = Some(expansions);
        self
    }

    /// [`Program::new`], its passes showing `door` what each module reads
    /// across its boundary.
    pub fn with_door(mut self, door: Option<&'i crate::reads::Door>) -> Program<'i> {
        self.door = door;
        self
    }

    /// Every module as it stands, to read declarations from while a pass
    /// rewrites.
    pub fn snapshot(&mut self) -> Vec<Snapshot> {
        let mut snapshot = std::mem::take(&mut self.snapshots);
        snapshot.truncate(self.modules.len());
        snapshot.resize_with(self.modules.len(), || Snapshot { arena: ExpArena::new(), types: TypeMap::new() });
        for (s, m) in snapshot.iter_mut().zip(&self.modules) {
            m.ir.arena.fork_into(&mut s.arena);
            m.types.clone_into(&mut s.types);
        }
        snapshot
    }

    /// A snapshot done with, its storage kept for the next.
    pub fn done_with(&mut self, snapshot: Vec<Snapshot>) {
        self.snapshots = snapshot;
    }

    pub fn interner(&self) -> &'i Interner {
        self.interner
    }

    pub fn expansions(&self) -> Option<&'i kg_base::ExpansionInterner> {
        self.expansions
    }

    /// `intrinsics` (`Type.hs:1065-1081`, frontend pass 5), then `inferType`.
    pub fn infer(&mut self) -> Outcome {
        let started = std::time::Instant::now();
        for m in 0..self.modules.len() {
            self.intrinsics(m);
        }
        let mut previous: Option<Vec<u64>> = None;
        let mut rounds = 0;
        let mut kept: Option<Symbols> = if self.full { None } else { self.map.take() };
        let mut rebuild = true;
        let mut units = if self.full { None } else { self.units.take().or_else(|| Some(Units::new(self.modules.len()))) };
        // What the passes since the last fixpoint wrote over (the epoch
        // opened as it ended): the new nodes no walk owns yet, so only a
        // write to a node that was already there moves a unit.
        if let Some(units) = &mut units {
            let changed: Vec<Vec<usize>> = self.modules.iter_mut().map(|m| m.types.end_epoch()).collect();
            let lengths = std::mem::take(&mut units.lengths);
            if !lengths.is_empty() {
                units.settle(&changed, &lengths, false);
            }
        }
        // `KG_INFER_CHECK=1`: every iteration of the incremental walk
        // against one of the whole walk from the same state, node by node.
        let check = !self.full && std::env::var_os("KG_INFER_CHECK").is_some();
        self.trace.push(Vec::new());
        loop {
            self.cancel.check();
            rounds += 1;
            let shadow = check.then(|| {
                let mut shadow = Program {
                    modules: self.modules.iter().map(|m| Module { ir: m.ir.fork(), types: m.types.clone() }).collect(),
                    interner: self.interner,
                    expansions: self.expansions,
                    full: true,
                    trace: Vec::new(),
                    units: None,
                    map: None,
                    intrinsics_from: Vec::new(),
                    cancel: kg_base::cancel::Cancel::never(),
                    filed: crate::symbols::Filed::default(),
                    snapshots: Vec::new(),
                    // The check's walk is not the run's: it reads nothing in.
                    door: None,
                };
                let lengths: Vec<usize> = self.modules.iter().map(|m| m.ir.arena.len()).collect();
                shadow.iterate(rounds, &mut None, &mut None, true);
                (shadow, lengths)
            });
            let (sig, filed_differently) = self.iterate(rounds, &mut kept, &mut units, rebuild);
            rebuild = filed_differently;
            if let Some(t) = self.trace.last_mut() {
                t.push(sig.iter().sum());
            }
            if let Some((shadow, lengths)) = shadow {
                self.compare(&shadow, &lengths, units.as_ref(), rounds);
            }
            if previous.as_ref() == Some(&sig) || rounds >= ROUND_GUARD {
                if let Some(mut units) = units {
                    units.lengths = self.modules.iter().map(|m| m.ir.arena.len()).collect();
                    for m in &mut self.modules {
                        m.types.begin_epoch();
                    }
                    self.units = Some(units);
                    self.map = kept;
                }
                crate::profile::whole("inferType", started);
                return Outcome { rounds, unresolved: sig };
            }
            previous = Some(sig);
        }
    }

    /// `KG_INFER_CHECK`: where this iteration left the program otherwise
    /// than the whole walk left `shadow`, over the nodes both started with.
    fn compare(&self, shadow: &Program<'_>, lengths: &[usize], units: Option<&Units>, rounds: u32) {
        let mut reported = 0;
        for (m, (mine, theirs)) in self.modules.iter().zip(&shadow.modules).enumerate() {
            for ((a, na), (b, nb)) in mine.ir.arena.iter().zip(theirs.ir.arena.iter()).take(lengths[m]) {
                let same_type = mine.types.get(a) == theirs.types.get(b);
                let same_exp = std::mem::discriminant(&na.exp) == std::mem::discriminant(&nb.exp);
                if same_type && same_exp || reported >= 5 {
                    continue;
                }
                reported += 1;
                let owners: Vec<String> = units
                    .map(|u| {
                        u.owner[m]
                            .of(a.index())
                            .map(|o| {
                                let root = u.at[m].iter().find(|(_, v)| **v == o).map(|(k, _)| *k);
                                format!("unit {o} (root {root:?}, needs {})", u.units[o as usize].needs)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let shape = format!("{:?}", na.exp);
                // The path from the root, each step with its unit.
                if let Some(u) = units {
                    let mut path = Vec::new();
                    fn find(arena: &ExpArena, at: ExpId, want: usize, path: &mut Vec<ExpId>) -> bool {
                        path.push(at);
                        if at.index() == want {
                            return true;
                        }
                        let mut kids = Vec::new();
                        arena[at].exp.children(&mut kids);
                        for k in kids {
                            if find(arena, k, want, path) {
                                return true;
                            }
                        }
                        path.pop();
                        false
                    }
                    find(&mine.ir.arena, mine.ir.root, a.index(), &mut path);
                    for step in path.iter().rev().take(8) {
                        let unit = u.at[m].get(&(step.index() as u32)).copied();
                        let exp = format!("{:?}", mine.ir.arena[*step].exp);
                        eprintln!(
                            "    at {} unit {unit:?} needs {:?} owners {:?}: {}",
                            step.index(),
                            unit.map(|x| u.units[x as usize].needs),
                            u.owner[m].of(step.index()).collect::<Vec<_>>(),
                            exp.chars().take(100).collect::<String>()
                        );
                    }
                }
                eprintln!(
                    "infer check: iteration {rounds}: module {m} node {} ({}): incremental {:?} / whole {:?}; owners {owners:?}",
                    a.index(),
                    shape.chars().take(160).collect::<String>(),
                    mine.types.get(a),
                    theirs.types.get(b)
                );
            }
        }
    }

    /// One iteration of the fixpoint: its `sig`, and whether a rewrite
    /// changed what the symbol map files (`rebuild` it next time).
    fn iterate(&mut self, rounds: u32, kept: &mut Option<Symbols>, units: &mut Option<Units>, rebuild: bool) -> (Vec<u64>, bool) {
        let timing = std::env::var_os("KG_INFER_TIME").is_some();
        let t0 = std::time::Instant::now();
        let changes0: u64 = self.modules.iter().map(|m| m.types.changes()).sum();
        let (mut visited, mut replaced) = (0u64, 0u64);
        // The iteration reads declarations as it found them: copies of
        // every module on the whole walk, the live modules with what
        // this iteration overwrote set aside otherwise.
        let snapshot = if self.full { self.snapshot() } else { Vec::new() };
        let mut overlays: Vec<FxHashMap<ExpId, Exp>> = Vec::new();
        let lengths: Vec<usize> = self.modules.iter().map(|m| m.ir.arena.len()).collect();
        if !self.full {
            overlays.resize_with(self.modules.len(), FxHashMap::default);
            for m in &mut self.modules {
                m.types.begin_epoch();
            }
        }
        let t1 = std::time::Instant::now();
        // Rebuilt only when the last iteration may have changed it: on
        // the whole walk always; otherwise when a rewrite changed what
        // its subtree files ([`crate::symbols::contributions`]).
        let symbols = match kept.take() {
            Some(s) if !self.full && !rebuild => s,
            old => {
                let symbols = self.symbols(crate::symbols::Config::TemplateDecl);
                // A map built afresh may answer some lookup otherwise: the
                // units that asked it are walked again.
                if let Some(units) = units.as_mut() {
                    units.ask_again(&symbols, old.as_ref());
                }
                symbols
            }
        };
        let mut filed_differently = false;
        let t2 = std::time::Instant::now();
        if let Some(units) = units.as_mut() {
            units.pass += 1;
            // The spines are walked every pass: reached from the start.
            for m in 0..self.modules.len() {
                units.units[m].seen = units.pass;
            }
        }
        for m in 0..self.modules.len() {
            self.cancel.check();
            let _visit = crate::profile::visit("inferType walk", m, self.modules[m].ir.arena.len());
            let (left, rest) = self.modules.split_at_mut(m);
            let (module, right) = rest.split_first_mut().expect("m is in range");
            let (before, overlay) = if self.full {
                (Before::Copies(&snapshot), None)
            } else {
                let (o_left, o_rest) = overlays.split_at_mut(m);
                let (mine, o_right) = o_rest.split_first_mut().expect("one overlay per module");
                fn view<'v>((md, o): (&'v Module, &'v FxHashMap<ExpId, Exp>)) -> Option<LiveModule<'v>> {
                    Some(LiveModule { arena: &md.ir.arena, types: &md.types, replaced: o })
                }
                let others: Vec<Option<LiveModule<'_>>> = left
                    .iter()
                    .zip(o_left.iter())
                    .map(view)
                    .chain(std::iter::once(None))
                    .chain(right.iter().zip(o_right.iter()).map(view))
                    .collect();
                (Before::Live(others), Some(mine))
            };
            let root = module.ir.root;
            let mut cx = Cx {
                interner: self.interner,
                symbols: &symbols,
                before,
                overlay,
                units: units.as_mut(),
                unit: m as u32,
                reads: std::cell::RefCell::new(Vec::new()),
                kids: Vec::new(),
                asked: std::cell::RefCell::new(Vec::new()),
                door: self.door,
                shown: std::cell::RefCell::default(),
                notified: 0,
                arena: &mut module.ir.arena,
                types: &mut module.types,
                names: Names::new(self.interner),
                expansions: self.expansions,
                module: m,
                visited: 0,
                replaced: 0,
                filed_differently: false,
                vetted: 0,
            };
            cx.tree(root, Alg::Infer);
            cx.show_asked();
            visited += cx.visited;
            replaced += cx.replaced;
            filed_differently |= cx.filed_differently;
            self.filed.vetted(m, cx.vetted);
            let reads = cx.reads.into_inner();
            let asked = cx.asked.into_inner();
            if let Some(units) = units.as_mut() {
                for (u, module, index) in reads {
                    units.readers[module as usize].add(index, u);
                }
                for (u, query, answer) in asked {
                    if !units.queries.contains_key(&query) {
                        let last = match &query {
                            Query::ByName(path) => path.last(),
                            Query::InScope(_, name) => name.last().copied(),
                        };
                        units.by_last.entry(last).or_default().push(query.clone());
                    }
                    let (_, askers) = units.queries.entry(query).or_insert_with(|| (answer, Vec::new()));
                    if !askers.contains(&u) {
                        askers.push(u);
                    }
                }
            }
        }
        if self.full {
            self.done_with(snapshot);
        } else {
            *kept = Some(symbols);
        }
        let mut changed: Vec<Vec<usize>> = Vec::new();
        for (m, module) in self.modules.iter_mut().enumerate() {
            let mut nodes = module.types.end_epoch();
            if let Some(o) = overlays.get(m) {
                nodes.extend(o.iter().filter(|(id, old)| module.ir.arena[**id].exp != **old).map(|(id, _)| id.index()));
            }
            changed.push(nodes);
        }
        let t3 = std::time::Instant::now();
        let sig: Vec<u64> = match units.as_mut() {
            None => self.modules.iter().map(|m| unresolved(&m.ir, &m.types)).collect(),
            Some(units) => {
                units.settle(&changed, &lengths, false);
                let sig: Vec<u64> = self.modules.iter().enumerate().map(|(m, md)| units.count(m, &md.ir, &md.types, md.ir.root, NO_UNIT)).collect();
                if std::env::var_os("KG_INFER_CHECK").is_some() {
                    let full: Vec<u64> = self.modules.iter().map(|m| unresolved(&m.ir, &m.types)).collect();
                    if full != sig {
                        eprintln!("infer check: iteration {rounds}: counted {sig:?}, whole {full:?}");
                    }
                }
                sig
            }
        };
        if timing {
            let changes: u64 = self.modules.iter().map(|m| m.types.changes()).sum::<u64>() - changes0;
            let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1e3;
            eprintln!(
                "infer it {rounds:2}: snapshot {:7.1} symbols {:7.1} walk {:7.1} sig {:6.1} ms; visited {visited} types changed {changes} replaced {replaced} unresolved {}",
                ms(t0, t1),
                ms(t1, t2),
                ms(t2, t3),
                ms(t3, std::time::Instant::now()),
                sig.iter().sum::<u64>()
            );
        }
        (sig, filed_differently)
    }

    /// `intrinsics` (`Type.hs:1065-1081`): the five built-in callees typed
    /// before inference, by the qualified name they are written with.
    fn intrinsics(&mut self, m: usize) {
        let interner = self.interner;
        if self.intrinsics_from.len() <= m {
            self.intrinsics_from.resize(m + 1, 0);
        }
        let from = if self.full { 0 } else { self.intrinsics_from[m] };
        let module = &mut self.modules[m];
        self.intrinsics_from[m] = module.ir.arena.len();
        // A name can be one of the five only if its symbol was interned
        // when it was lowered, and a path is one of them only if it ends in
        // one: most name nodes are passed on their own leaf, without the
        // path (`spine_single` walked every new name's whole spine, 4% of
        // the loop on `risc-v.k`, 2026-10-10).
        let names: Vec<Symbol> = ["__print", "assert", "__cycles", "__str_cnt", "__assert_str_eq"]
            .into_iter()
            .filter_map(|n| interner.get(n))
            .collect();
        if names.is_empty() {
            return;
        }
        let ids: Vec<ExpId> = module.ir.arena.iter().skip(from).map(|(id, _)| id).collect();
        for id in ids {
            if !matches!(module.ir.arena[id].exp, Exp::QualifiedIdentifier(_) | Exp::ScopedName { .. }) {
                continue;
            }
            if kg_hir::sort::spine_last(&module.ir.arena, id).is_some_and(|last| !names.contains(&last)) {
                continue;
            }
            let Some(only) = kg_hir::sort::spine_single(&module.ir.arena, id) else { continue };
            let f = |ret: Ty, params: Vec<Ty>| Ty::Function {
                kind: FunctionKind::FreeFunction,
                attrs: TAttrSet::new(),
                ret: TyRef::new(ret),
                params: params.into_iter().map(|t| TFuncParam { attrs: TAttrSet::new(), ty: TyRef::new(t), name: None }).collect(),
            };
            let ty = match &*interner.resolve(only) {
                "__print" => f(Ty::Void, vec![Ty::Str]),
                "assert" => f(Ty::Void, vec![Ty::Boolean]),
                "__cycles" | "__str_cnt" => f(Ty::Unsigned(64), vec![]),
                "__assert_str_eq" => f(Ty::Void, vec![Ty::Str, Ty::Str]),
                _ => continue,
            };
            module.types.resolve(id, ty);
        }
    }
}

/// The count of unresolved nodes in a tree (`sig`'s summand,
/// `Type/Inference.hs:38-44`): every node reachable from the root.
fn unresolved(ir: &ModuleIr, types: &TypeMap) -> u64 {
    let mut n = 0;
    let mut stack = vec![ir.root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        if !types.resolved(id) {
            n += 1;
        }
        kids.clear();
        ir.arena[id].exp.children(&mut kids);
        stack.extend(kids.iter().copied());
    }
    n
}

/// A module as the round found it: what the round's symbol map answers
/// with (`getSymbols p` is taken before the pass runs).
#[derive(Debug)]
pub struct Snapshot {
    pub arena: ExpArena,
    pub types: TypeMap,
}

/// A node the incremental walk treats as a unit, whose subtree it skips
/// while nothing it holds or read has changed: a declaration, and a
/// statement or call — a pass that rewrites one call rebuilds every node
/// above it, so a rebuilt function is a new unit, and the statements it
/// kept are what can still be skipped. Any subtree would do: its walk reads
/// only itself, the iteration's view and the symbol map, all recorded.
fn is_unit(exp: &Exp) -> bool {
    matches!(
        exp,
        Exp::Function { .. }
            | Exp::FunctionDecl { .. }
            | Exp::Class { .. }
            | Exp::Struct { .. }
            | Exp::Union { .. }
            | Exp::Enum { .. }
            | Exp::Alias { .. }
            | Exp::Template { .. }
            | Exp::Variable { .. }
            | Exp::Return(_)
            | Exp::DoWhile { .. }
            | Exp::UnrolledFor { .. }
            | Exp::RangeFor { .. }
            | Exp::If { .. }
            | Exp::Switch { .. }
            | Exp::SwitchCase { .. }
            | Exp::SwitchDefault(_)
            | Exp::Assign { .. }
            | Exp::NestedScope { .. }
            | Exp::Reorder(_)
            | Exp::StaticAssert(_)
            | Exp::FunctionCall { .. }
    )
}

const NO_UNIT: u32 = u32::MAX;

/// A unit of the incremental walk (§3.3): a declaration's subtree, walked
/// again only when something in it changed, or something it read did.
struct Unit {
    parent: u32,
    alg: Alg,
    /// The pass ([`Units::pass`]) that last reached it, walked or skipped.
    seen: u32,
    /// Walk it this iteration: it, or what it read, changed.
    needs: bool,
    /// Its subtree's unresolved nodes, while nothing counted in it has
    /// changed, and the units whose counts include it.
    count: Option<u64>,
    count_parents: Vec<u32>,
    module: usize,
}

impl Unit {
    fn new(module: usize, alg: Alg) -> Unit {
        Unit { parent: NO_UNIT, alg, seen: 0, needs: true, count: None, count_parents: Vec::new(), module }
    }
}

/// Per module, each node's units — its first in a dense vector, any more
/// beside it. The tree is a DAG: a folded constant shares its
/// initializer's children (`fold_named_value`), so a node can lie in more
/// than one unit, and a change to it is every one's change.
#[derive(Default)]
struct Owners {
    first: Vec<u32>,
    more: FxHashMap<u32, Vec<u32>>,
}

impl Owners {
    fn add(&mut self, index: u32, unit: u32) {
        let i = index as usize;
        if self.first.len() <= i {
            self.first.resize(i + 1, NO_UNIT);
        }
        match self.first[i] {
            NO_UNIT => self.first[i] = unit,
            u if u == unit => {}
            _ => {
                let more = self.more.entry(index).or_default();
                if !more.contains(&unit) {
                    more.push(unit);
                }
            }
        }
    }

    fn of(&self, index: usize) -> impl Iterator<Item = u32> + '_ {
        let first = self.first.get(index).copied().filter(|&u| u != NO_UNIT);
        let more = first.and_then(|_| self.more.get(&(index as u32)));
        first.into_iter().chain(more.into_iter().flatten().copied())
    }
}

/// Incremental inference's memory between the iterations of one fixpoint.
struct Units {
    units: Vec<Unit>,
    /// Per module, a unit's root node to the unit.
    at: Vec<FxHashMap<u32, u32>>,
    /// Per module: the units whose walks visited each node or read it as a
    /// child, and the units whose kept counts counted it.
    owner: Vec<Owners>,
    counted: Vec<Owners>,
    /// Per module, each node the iteration's view was read at to the units
    /// that read it — whether or not any walk owns the node yet (a
    /// declaration in a body not yet walked is read before it is typed).
    readers: Vec<Owners>,
    /// Every lookup a unit made, with the answer it got and who asked, and
    /// the lookups by the last segment of the name they look up.
    queries: FxHashMap<Query, (Option<Decl>, Vec<u32>)>,
    by_last: FxHashMap<Option<Symbol>, Vec<Query>>,
    /// Each module's length when the last fixpoint ended.
    lengths: Vec<usize>,
    /// Counts the passes of every fixpoint, for [`Unit::seen`].
    pass: u32,
    kids: Vec<ExpId>,
    /// [`Units::count`]'s child lists, one stack for the whole recursion:
    /// a list per node counted was a twentieth of a whole-program check.
    counting: Vec<ExpId>,
}

impl std::fmt::Debug for Units {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Units({} units, {} lookups)", self.units.len(), self.queries.len())
    }
}

/// A lookup in the symbol map, as the incremental walk remembers it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Query {
    ByName(SymbolPath),
    InScope(Vec<Symbol>, Vec<Symbol>),
}

impl Units {
    /// One spine unit per module, numbered as the module: what no
    /// declaration holds, walked every iteration.
    fn new(modules: usize) -> Units {
        Units {
            units: (0..modules).map(|m| Unit::new(m, Alg::Infer)).collect(),
            at: vec![FxHashMap::default(); modules],
            owner: (0..modules).map(|_| Owners::default()).collect(),
            counted: (0..modules).map(|_| Owners::default()).collect(),
            readers: (0..modules).map(|_| Owners::default()).collect(),
            queries: FxHashMap::default(),
            by_last: FxHashMap::default(),
            lengths: Vec::new(),
            pass: 0,
            kids: Vec::new(),
            counting: Vec::new(),
        }
    }

    /// `unit`'s walk visits the node at `id`: it owns the node, and the
    /// node's children, whose expressions and types the rules at `id` read.
    fn own(&mut self, module: usize, arena: &ExpArena, id: ExpId, unit: u32) {
        self.owner[module].add(id.index() as u32, unit);
        let mut kids = std::mem::take(&mut self.kids);
        kids.clear();
        arena[id].exp.children(&mut kids);
        for &k in &kids {
            self.owner[module].add(k.index() as u32, unit);
        }
        self.kids = kids;
    }

    /// A node was written: every unit that owns it is walked, the rest of
    /// this iteration or the next.
    ///
    /// Only a unit this pass has not reached yet: one the walk is inside,
    /// or has passed, reads what it read, as the whole walk's would, and
    /// the pass's end ([`Units::settle`]) weighs what changed for the next.
    fn touch(&mut self, module: usize, index: usize) {
        let owners: Vec<u32> = self.owner[module].of(index).collect();
        for u in owners {
            let mut at = u;
            while at != NO_UNIT && self.units[at as usize].seen != self.pass && !self.units[at as usize].needs {
                self.units[at as usize].needs = true;
                at = self.units[at as usize].parent;
            }
        }
    }

    /// The remembered lookups a new map could answer otherwise, asked of
    /// it: a unit that would now get another answer is walked again. With
    /// the map the answers came from, only a lookup that probes a key the
    /// two maps disagree on is asked — every key a lookup probes ends with
    /// its name — and without it, every one.
    fn ask_again(&mut self, symbols: &Symbols, old: Option<&Symbols>) {
        let asked: Vec<Query> = match old {
            None => self.queries.keys().cloned().collect(),
            Some(old) => {
                let mut asked = Vec::new();
                for key in symbols.changed_keys(old) {
                    let Some(candidates) = self.by_last.get(&key.last()) else { continue };
                    for q in candidates {
                        let probes = match q {
                            Query::ByName(path) => path.segments() == key.segments(),
                            Query::InScope(scope, name) => {
                                key.len() >= name.len()
                                    && key.ends_with(name)
                                    && scope.starts_with(&key[..key.len() - name.len()])
                            }
                        };
                        if probes {
                            asked.push(q.clone());
                        }
                    }
                }
                asked
            }
        };
        let mut moved = Vec::new();
        for query in asked {
            let Some((answer, askers)) = self.queries.get_mut(&query) else { continue };
            let now = match &query {
                Query::ByName(path) => symbols.by_name(path),
                Query::InScope(scope, name) => symbols.in_scope(scope, name),
            };
            if now != *answer {
                *answer = now;
                moved.extend(askers.iter().copied());
            }
        }
        for u in moved {
            let mut at = u;
            while at != NO_UNIT {
                self.units[at as usize].needs = true;
                at = self.units[at as usize].parent;
            }
        }
    }

    /// A kept count is gone, and so is every count that includes it.
    fn uncount(&mut self, unit: u32) {
        let mut stack = vec![unit];
        while let Some(u) = stack.pop() {
            if self.units[u as usize].count.take().is_some() {
                stack.extend(std::mem::take(&mut self.units[u as usize].count_parents));
            }
        }
    }

    /// After an iteration: which units the next one walks. A unit is walked
    /// when a node it owns changed, or a node it read did; its ancestors
    /// are walked on the way down to it. A change no walk owns — a node
    /// that existed before the iteration and no walk has visited — or a
    /// symbol map rebuilt, and every unit is walked.
    fn settle(&mut self, changed: &[Vec<usize>], lengths: &[usize], rebuilt: bool) {
        let mut everything = rebuilt;
        let mut changed_units = vec![false; self.units.len()];
        let mut uncounted = Vec::new();
        let mut read_changed = Vec::new();
        for (m, nodes) in changed.iter().enumerate() {
            for &i in nodes {
                let mut owned = false;
                for u in self.owner[m].of(i) {
                    changed_units[u as usize] = true;
                    owned = true;
                }
                if !owned && i < lengths[m] {
                    everything = true;
                }
                uncounted.extend(self.counted[m].of(i));
                read_changed.extend(self.readers[m].of(i));
            }
        }
        for u in uncounted {
            self.uncount(u);
        }
        if everything {
            for unit in &mut self.units {
                unit.needs = true;
                unit.count = None;
                unit.count_parents.clear();
            }
            return;
        }
        let mut needs = changed_units;
        for u in read_changed {
            needs[u as usize] = true;
        }
        for (u, _) in needs.iter().enumerate().filter(|(_, n)| **n) {
            let mut at = u as u32;
            while at != NO_UNIT {
                self.units[at as usize].needs = true;
                at = self.units[at as usize].parent;
            }
        }
    }

    /// `sig`'s summand ([`unresolved`]) with each unit's count kept while
    /// no node it counted has changed. `within` is the innermost unit whose
    /// count this node's goes into (`NO_UNIT`: the spine's, never kept).
    fn count(&mut self, m: usize, ir: &ModuleIr, types: &TypeMap, id: ExpId, within: u32) -> u64 {
        let unit = self.at[m].get(&(id.index() as u32)).copied().filter(|&u| self.units[u as usize].module == m);
        if let Some(u) = unit {
            if within != NO_UNIT && !self.units[u as usize].count_parents.contains(&within) {
                self.units[u as usize].count_parents.push(within);
            }
            if let Some(n) = self.units[u as usize].count {
                return n;
            }
        }
        let me = unit.unwrap_or(within);
        if me != NO_UNIT {
            self.counted[m].add(id.index() as u32, me);
        }
        let mut n = u64::from(!types.resolved(id));
        let base = self.counting.len();
        ir.arena[id].exp.children(&mut self.counting);
        for i in base..self.counting.len() {
            let k = self.counting[i];
            n += self.count(m, ir, types, k, me);
        }
        self.counting.truncate(base);
        if let Some(u) = unit {
            self.units[u as usize].count = Some(n);
        }
        n
    }
}

/// Every module as the iteration found it (`getSymbols p` before the pass).
enum Before<'a> {
    /// Copies — the whole walk, [`Program::full`].
    Copies(&'a [Snapshot]),
    /// The live modules, this one's slot empty (it is the `Cx`'s own): an
    /// expression this iteration replaced is read from `replaced`, a type
    /// from its map's epoch journal ([`TypeMap::before`]).
    Live(Vec<Option<LiveModule<'a>>>),
}

struct LiveModule<'a> {
    arena: &'a ExpArena,
    types: &'a TypeMap,
    replaced: &'a FxHashMap<ExpId, Exp>,
}

/// The two algebras `inferExpTree'` is run with (`Type/Inference.hs:51,68`).
#[derive(Copy, Clone, PartialEq, Eq)]
enum Alg {
    /// `staticControl . evaluateConstexpr . inferExpType`.
    Infer,
    /// `staticControl` alone, inside a local class.
    StaticOnly,
}

struct Names {
    this: Symbol,
    errors: Symbol,
    empty: Symbol,
}

impl Names {
    fn new(interner: &Interner) -> Names {
        Names { this: interner.intern("$this"), errors: interner.intern("$errors"), empty: interner.intern("") }
    }
}

struct Cx<'a> {
    interner: &'a Interner,
    symbols: &'a Symbols,
    before: Before<'a>,
    /// This module's replaced expressions as they were, when `before` is
    /// live.
    overlay: Option<&'a mut FxHashMap<ExpId, Exp>>,
    /// The incremental walk's units, when `before` is live, and the one
    /// being walked (a module's spine is the unit numbered as the module).
    units: Option<&'a mut Units>,
    unit: u32,
    /// `(unit, module, node)` for every read of the iteration's view.
    reads: std::cell::RefCell<Vec<(u32, u32, u32)>>,
    /// The walk's child lists, one stack for the whole recursion.
    kids: Vec<ExpId>,
    /// Every lookup in the symbol map, with who asked and its answer.
    asked: std::cell::RefCell<Vec<(u32, Query, Option<Decl>)>>,
    /// A recorded run's read door ([`crate::reads`]), and the nodes of
    /// another module this walk has shown it — `(module, node, what)`, an
    /// expression 0, a type 1, a source 2: the iteration's view does not
    /// change under the walk, so a read is shown once.
    door: Option<&'a crate::reads::Door>,
    shown: std::cell::RefCell<kg_base::FxHashSet<(u32, u32, u8)>>,
    /// How much of this module's written-entries log [`Cx::notify`] has
    /// passed on.
    notified: usize,
    arena: &'a mut ExpArena,
    types: &'a mut TypeMap,
    names: Names,
    expansions: Option<&'a kg_base::ExpansionInterner>,
    /// This module's index in the program, for [`Cx::import`].
    module: usize,
    /// Nodes walked and nodes rewritten, for `KG_INFER_TIME`.
    visited: u64,
    replaced: u64,
    /// A rewrite changed what its subtree files in the symbol map: the
    /// next iteration's map is built afresh.
    filed_differently: bool,
    /// Rewrites weighed [`crate::symbols::harmless`] to every symbol map's
    /// kept entries ([`crate::symbols::Filed::vetted`]).
    vetted: u64,
}

impl Cx<'_> {
    fn exp(&self, id: ExpId) -> &Exp {
        &self.arena[id].exp
    }

    fn ty(&self, id: ExpId) -> Ty {
        self.types.get(id)
    }

    fn resolved(&self, id: ExpId) -> bool {
        self.types.resolved(id)
    }

    /// `lookupSymbolByName`, remembered for the incremental walk.
    fn by_name(&self, path: &SymbolPath) -> Option<Decl> {
        let answer = self.symbols.by_name(path);
        if self.units.is_some() || self.door.is_some() {
            self.asked.borrow_mut().push((self.unit, Query::ByName(path.clone()), answer));
        }
        answer
    }

    /// `lookupInScope`, remembered for the incremental walk.
    fn in_scope(&self, scope: &[Symbol], name: &[Symbol]) -> Option<Decl> {
        let answer = self.symbols.in_scope(scope, name);
        if self.units.is_some() || self.door.is_some() {
            self.asked.borrow_mut().push((self.unit, Query::InScope(scope.to_vec(), name.to_vec()), answer));
        }
        answer
    }

    /// The walk's lookups, each once, shown to a recorded run's door
    /// ([`crate::reads`]) as the walk saw their answers: the iteration's
    /// map and view do not change under it.
    fn show_asked(&self) {
        let Some(door) = self.door else { return };
        let asked = self.asked.borrow();
        let mut shown: kg_base::FxHashSet<(&Query, Option<Decl>)> = kg_base::FxHashSet::default();
        let view = |d: Decl| (self.view_arena(d.module), self.view_exp(d.module, d.id));
        for (_, query, answer) in asked.iter() {
            if !shown.insert((query, *answer)) {
                continue;
            }
            match query {
                Query::ByName(path) => door.lookup(self.module, Asked::ByName(path.segments()), *answer, view),
                Query::InScope(scope, name) => door.lookup(self.module, Asked::InScope(scope, name), *answer, view),
            }
        }
    }

    fn read(&self, module: usize, id: ExpId) {
        if self.units.is_some() {
            self.reads.borrow_mut().push((self.unit, module as u32, id.index() as u32));
        }
    }

    /// Whether a recorded run's door is yet to be shown this read of
    /// another module's node (`what`: its expression, type or source).
    fn to_show(&self, module: usize, id: ExpId, what: u8) -> Option<&crate::reads::Door> {
        let door = self.door.filter(|_| module != self.module)?;
        self.shown.borrow_mut().insert((module as u32, id.index() as u32, what)).then_some(door)
    }

    /// The arena of any module, as the iteration found it — its sources
    /// and kinds, which no rewrite in place changes.
    fn view_arena(&self, module: usize) -> &ExpArena {
        match &self.before {
            Before::Copies(s) => &s[module].arena,
            Before::Live(_) if module == self.module => self.arena,
            Before::Live(others) => others[module].as_ref().expect("another module's slot is filled").arena,
        }
    }

    /// [`Cx::snap_exp`], unrecorded.
    fn view_exp(&self, module: usize, id: ExpId) -> &Exp {
        match &self.before {
            Before::Copies(s) => &s[module].arena[id].exp,
            Before::Live(_) if module == self.module => {
                self.overlay.as_deref().and_then(|o| o.get(&id)).unwrap_or(&self.arena[id].exp)
            }
            Before::Live(others) => {
                let other = others[module].as_ref().expect("another module's slot is filled");
                other.replaced.get(&id).unwrap_or(&other.arena[id].exp)
            }
        }
    }

    /// A node of any module, as the iteration found it.
    fn snap_exp(&self, module: usize, id: ExpId) -> &Exp {
        self.read(module, id);
        let exp = self.view_exp(module, id);
        if let Some(door) = self.to_show(module, id, 0) {
            door.node(self.module, module, self.view_arena(module), id, exp);
        }
        exp
    }

    /// Another module's type entry as the iteration found it, for the door.
    fn show_ty(&self, module: usize, id: ExpId) {
        let Some(door) = self.to_show(module, id, 1) else { return };
        // Another module's, never this one's ([`Cx::to_show`]).
        let entry = match &self.before {
            Before::Copies(s) => s[module].types.entry(id.index()),
            Before::Live(others) => others[module].as_ref().expect("another module's slot is filled").types.before_entry(id),
        };
        door.ty(self.module, module, self.view_arena(module), id, entry);
    }

    /// A node's type in any module, as the iteration found it.
    fn snap_ty(&self, module: usize, id: ExpId) -> Ty {
        self.read(module, id);
        self.show_ty(module, id);
        match &self.before {
            Before::Copies(s) => s[module].types.get(id),
            Before::Live(_) if module == self.module => self.types.before(id),
            Before::Live(others) => others[module].as_ref().expect("another module's slot is filled").types.before(id),
        }
    }

    fn snap_is_set(&self, module: usize, id: ExpId) -> bool {
        self.read(module, id);
        self.show_ty(module, id);
        match &self.before {
            Before::Copies(s) => s[module].types.is_set(id),
            Before::Live(_) if module == self.module => self.types.before_is_set(id),
            Before::Live(others) => others[module].as_ref().expect("another module's slot is filled").types.before_is_set(id),
        }
    }

    /// A node's source and kind, which no rewrite in place changes.
    fn snap_src(&self, module: usize, id: ExpId) -> (kg_base::Src, kg_syntax::SyntaxKind) {
        let node = self.view_arena(module).node(id);
        if let Some(door) = self.to_show(module, id, 2) {
            door.src(self.module, module, self.view_arena(module), id);
        }
        (node.src, node.kind)
    }

    /// A declaration's node, as the round found it.
    fn decl_exp(&self, d: Decl) -> &Exp {
        self.snap_exp(d.module, d.id)
    }

    fn decl_ty(&self, d: Decl) -> Ty {
        self.snap_ty(d.module, d.id)
    }

    /// `showUnmangled = show . unmangle symbols` (`Constexpr.hs:131`) over
    /// this walk's map — its lookups recorded, as every other.
    fn show_unmangled(&self, t: &Ty) -> String {
        kg_ty::subtype::show_unmangled_with(t, Some(self.interner), &|name| {
            self.by_name(&SymbolPath::new(name.iter().copied())).map(|d| self.decl_ty(d))
        })
    }

    // --- the traversal ------------------------------------------------------

    /// `inferExpTree' alg = alg . go` (`Type/Inference.hs:51-69`), skipping
    /// a unit nothing has touched since a walk of it changed nothing
    /// (`docs/plans/performance.md` §3.3).
    fn tree(&mut self, id: ExpId, alg: Alg) {
        let Some(units) = self.units.as_deref_mut() else { return self.walk(id, alg) };
        let index = id.index() as u32;
        let outer = self.unit;
        if is_unit(&self.arena[id].exp) {
            let unit = match units.at[self.module].get(&index) {
                Some(&u) if !units.units[u as usize].needs && units.units[u as usize].alg == alg => {
                    // Where it hangs now: a unit moved under another is
                    // reached through that one's walk from here on.
                    units.units[u as usize].parent = outer;
                    units.units[u as usize].seen = units.pass;
                    return;
                }
                Some(&u) => u,
                None => {
                    units.units.push(Unit::new(self.module, alg));
                    let u = (units.units.len() - 1) as u32;
                    units.at[self.module].insert(index, u);
                    u
                }
            };
            let pass = units.pass;
            let entry = &mut units.units[unit as usize];
            entry.parent = outer;
            entry.alg = alg;
            entry.needs = false;
            entry.seen = pass;
            self.unit = unit;
        }
        units.own(self.module, self.arena, id, self.unit);
        self.walk(id, alg);
        self.unit = outer;
        self.notify();
    }

    /// Pass the types written since the last call to their owners: a unit
    /// later in this walk that shares a written node reads it live, as the
    /// whole walk's would, so it is walked this iteration.
    fn notify(&mut self) {
        let Some(units) = self.units.as_deref_mut() else { return };
        let written = self.types.written_since(self.notified);
        self.notified += written.len();
        for &i in written {
            units.touch(self.module, i);
        }
    }

    fn walk(&mut self, id: ExpId, alg: Alg) {
        self.visited += 1;
        kg_base::cancel::checkpoint_strided();
        // What the arms below need, read off the node: no clone of it.
        enum Shape {
            Lambda,
            Function { local: bool, params: ExpId, ret: ExpId },
            Template,
            StaticIf(ExpId),
            LocalClass,
            Other,
        }
        let shape = match self.exp(id) {
            Exp::Lambda { .. } => Shape::Lambda,
            Exp::Function { scope, params, ret, .. } => {
                Shape::Function { local: kg_hir::rewrite::is_local_scope(self.interner, scope), params: *params, ret: *ret }
            }
            Exp::Template { .. } => Shape::Template,
            Exp::StaticIf { cond, .. } => Shape::StaticIf(*cond),
            Exp::Class { scope, .. } if kg_hir::rewrite::is_local_scope(self.interner, scope) => Shape::LocalClass,
            _ => Shape::Other,
        };
        match shape {
            // `go (NotedExp n e@LambdaF{}) = Note n e`.
            Shape::Lambda => {}
            Shape::Function { local, params, ret } => {
                // `funcParams'` is computed, then kept or thrown away.
                let at = self.types.begin();
                self.tree(params, alg);
                let items = self.seq(params);
                if items.iter().any(|&p| self.ty(p).is_function()) {
                    self.types.rollback(at);
                    return self.apply(id, alg);
                }
                self.types.commit(at);
                if !items.iter().all(|&p| self.resolved(p)) {
                    return self.apply(id, alg);
                }
                if local {
                    self.tree(ret, alg);
                    return self.apply(id, alg);
                }
                self.children_except(id, &[params], alg);
            }
            Shape::Template => {
                let Exp::Template { params, .. } = &self.arena[id].exp else { unreachable!("shape read off this node") };
                let base = self.kids.len();
                self.kids.extend(params.iter().copied());
                self.walk_kids(base, &[], alg);
            }
            Shape::StaticIf(cond) => self.tree(cond, alg),
            // `go e@ClassF{} | isLocalScope e = inferExpTree' staticControl <$> unfix e`.
            Shape::LocalClass => self.children_except(id, &[], Alg::StaticOnly),
            Shape::Other => self.children_except(id, &[], alg),
        }
        self.apply(id, alg);
    }

    fn children_except(&mut self, id: ExpId, done: &[ExpId], alg: Alg) {
        let base = self.kids.len();
        self.arena[id].exp.children(&mut self.kids);
        self.walk_kids(base, done, alg);
    }

    /// Walk the children stacked from `base` on, then drop them.
    fn walk_kids(&mut self, base: usize, done: &[ExpId], alg: Alg) {
        for i in base..self.kids.len() {
            let k = self.kids[i];
            if !done.contains(&k) {
                self.tree(k, alg);
            }
        }
        self.kids.truncate(base);
    }

    fn apply(&mut self, id: ExpId, alg: Alg) {
        if alg == Alg::Infer {
            self.infer_exp_type(id);
            self.evaluate_constexpr(id);
        }
        self.static_control(id);
    }

    fn seq(&self, id: ExpId) -> Vec<ExpId> {
        match self.exp(id) {
            Exp::Seq(items) => items.clone(),
            _ => Vec::new(),
        }
    }

    // --- inferExpType (Type.hs:1105-1171) ------------------------------------

    fn infer_exp_type(&mut self, id: ExpId) {
        // The fields the arms below read, off the node in place: this runs
        // at every node walked, and a clone of each was a fiftieth of a
        // whole-program check.
        enum Arm {
            RangeFor { var: ExpId, limit: ExpId },
            StaticFor { var: ExpId, limit: ExpId },
            Variable { init: Option<ExpId> },
            Assign { lvalue: ExpId, value: ExpId },
            FunctionCall { function: ExpId, args: ExpId },
            Function { ret: ExpId, body: ExpId },
            Other,
        }
        let arm = match self.exp(id) {
            &Exp::RangeFor { var, limit, .. } => Arm::RangeFor { var, limit },
            &Exp::StaticFor { var, limit, .. } | &Exp::UnrolledFor { var, limit, .. } => Arm::StaticFor { var, limit },
            &Exp::Variable { init, .. } => Arm::Variable { init },
            &Exp::Assign { lvalue, value } => Arm::Assign { lvalue, value },
            &Exp::FunctionCall { function, args, .. } => Arm::FunctionCall { function, args },
            &Exp::Function { ret, body, .. } => Arm::Function { ret, body },
            _ => Arm::Other,
        };
        match arm {
            Arm::RangeFor { var, limit } => {
                if self.resolved(var) {
                    return;
                }
                if self.is_const_auto_var(var) {
                    // `inductionVarType`: a literal limit's index type, else
                    // the limit's own width.
                    let t = if self.is_int_literal(limit) {
                        self.const_index_type(limit)
                    } else {
                        self.size_of(limit).map_or(Ty::Unresolved, |w| Ty::Unsigned(w as u32).to_const())
                    };
                    self.types.set_type_of(var, t);
                    return;
                }
            }
            Arm::StaticFor { var, limit } => {
                if self.resolved(var) {
                    return;
                }
                if self.is_const_auto_var(var) {
                    let t = self.const_index_type(limit);
                    self.types.set_type_of(var, t);
                    return;
                }
            }
            Arm::Variable { init } if self.resolved(id) => {
                if let Some(init) = init {
                    let t = self.ty(id);
                    self.set_type_of_template_call(&t, init);
                }
                return;
            }
            Arm::Assign { lvalue, value } if self.resolved(lvalue) => {
                let t = self.ty(lvalue);
                self.set_type_of_template_call(&t, value);
                return;
            }
            Arm::FunctionCall { function, args } if self.resolved(id) => {
                let callee = self.ty(function);
                if callee.is_function() || callee.is_closure(&Ty::closure_template_path(self.interner)) {
                    let params: Vec<Ty> = callee
                        .function_params()
                        .map(|ps| ps.iter().map(|p| (*p.ty).clone()).collect())
                        .unwrap_or_default();
                    let items = self.seq(args);
                    if params.len() == items.len() {
                        for (t, a) in params.iter().zip(items) {
                            self.set_type_of_template_call(t, a);
                        }
                    }
                    return;
                }
            }
            Arm::Function { ret, body } if self.resolved(id) => {
                let t = self.ty(ret).the_type();
                for stmt in self.seq(body) {
                    if let &Exp::Return(Some(e)) = self.exp(stmt) {
                        self.set_type_of_template_call(&t, e);
                    }
                }
                return;
            }
            Arm::Variable { .. } | Arm::Assign { .. } | Arm::FunctionCall { .. } | Arm::Function { .. } | Arm::Other => {}
        }
        if self.resolved(id) {
            return;
        }
        let t = self.infer(id);
        self.types.resolve(id, t);
    }

    /// `setTypeOfTemplateCall` (`Type.hs:1120-1129`): a call whose callee is
    /// a template instance not yet typed has it typed as a function
    /// returning `t` — deduction's third source.
    fn set_type_of_template_call(&mut self, t: &Ty, x: ExpId) {
        if matches!(t, Ty::Auto) {
            return;
        }
        let Exp::FunctionCall { function, .. } = self.exp(x).clone() else { return };
        let typ = Ty::Function {
            kind: FunctionKind::FreeFunction,
            attrs: TAttrSet::new(),
            ret: TyRef::new(t.clone()),
            params: Vec::new(),
        };
        let mut callee = function;
        loop {
            match self.exp(callee).clone() {
                Exp::FunctionSpecifier { function, .. } => callee = function,
                Exp::QualifiedIdentifier(a) => {
                    if matches!(self.exp(a), Exp::TemplateInstance { .. }) && !self.resolved(a) {
                        self.types.set_type_of(a, typ);
                    }
                    return;
                }
                _ => return,
            }
        }
    }

    fn is_const_auto_var(&self, var: ExpId) -> bool {
        let Exp::Variable { ty, .. } = self.exp(var) else { return false };
        matches!(self.exp(*ty), Exp::Const(inner) if matches!(self.exp(*inner), Exp::Auto))
    }

    /// `constIndexType` (`Type.hs:1112-1118`).
    fn const_index_type(&self, limit: ExpId) -> Ty {
        let Some(x) = self.int_value(limit) else { return Ty::Unresolved };
        let t = if x.is_negative() {
            Ty::Error(kg_base::SmolStr::new_static("The iteration count of `static for` can't be negative"))
        } else if x.to_i128().is_some_and(|v| v < 3) {
            Ty::Unsigned(1)
        } else {
            match x.checked_clog2() {
                Some(w) => Ty::Unsigned(w),
                None => return Ty::Unresolved,
            }
        };
        t.to_const()
    }

    // --- infer (Type.hs:1174-1340) -------------------------------------------

    #[allow(clippy::too_many_lines)]
    fn infer(&self, id: ExpId) -> Ty {
        let tt = |t: Ty| Ty::Type(TyRef::new(t));
        match self.exp(id) {
            Exp::Auto => Ty::Unresolved,
            Exp::Alias { body, .. } => {
                // A `Closure` copy's note is its closure (`Type.hs:1175-1177`).
                if let Ty::Instance { args, .. } = self.ty(id)
                    && let [(_, kg_ty::TArg::Type(a)), (_, kg_ty::TArg::Name(_, b)), (_, kg_ty::TArg::Type(c))] =
                        args.as_slice()
                    && self.ty(id).is_closure(&Ty::closure_template_path(self.interner))
                {
                    let alias = self.qualified_name_of_decl(id).unwrap_or_else(SymbolPath::empty);
                    return tt(Ty::Closure { func: b.clone(), func_ty: a.clone(), capture: c.clone(), alias });
                }
                self.ty(*body)
            }
            Exp::Array { attr, elem, dims } => {
                let Some(attrs) = self.attributes(*attr) else { return Ty::Unresolved };
                let dims: Option<Vec<ConstInt>> = self.seq(*dims).iter().map(|&d| self.int_value(d)).collect();
                match dims {
                    Some(dims) => tt(Ty::Array { attrs, elem: TyRef::new(self.ty(*elem).the_type()), dims }),
                    None => Ty::Unresolved,
                }
            }
            Exp::ArrayAccess { base, .. } => self.ty(*base).element(Some(self.interner)),
            Exp::ArrayAccessLValue { base, .. } => self.ty(*base).element_lvalue(Some(self.interner)),
            Exp::Binary(op, a, b) => {
                let constant = |x: ExpId| {
                    let t = self.ty(x);
                    self.is_int_literal(x) || (t.is_const() && t.is_enum())
                };
                if constant(*a) && constant(*b) {
                    return Ty::Unresolved;
                }
                if !self.resolved(*a) || !self.resolved(*b) {
                    return Ty::Unresolved;
                }
                self.infer_binary_op(*op, *a, *b)
            }
            Exp::BitOffset { .. } | Exp::ByteOffset { .. } => Ty::Unresolved,
            Exp::BoolLiteral(_) => Ty::Boolean,
            Exp::Boolean => tt(Ty::Boolean),
            Exp::Cast { ty, .. } => self.ty(*ty).the_type(),
            Exp::Capture { .. } => self.type_of_symbol(id),
            Exp::Class { scope, body, .. } => {
                if kg_hir::rewrite::is_local_scope(self.interner, scope) {
                    return Ty::Unresolved;
                }
                let qn = self.qualified_name_of_decl(id).unwrap_or_else(SymbolPath::empty);
                tt(Ty::Class(qn, self.fields(*body)))
            }
            Exp::Decltype { expr, .. } => tt(self.ty(*expr)),
            Exp::Concat(items) => {
                let mut total: u64 = 0;
                for i in self.seq(*items) {
                    match self.size_of(i) {
                        Some(w) => total += w,
                        None => return Ty::Unresolved,
                    }
                }
                Ty::Unsigned(total as u32)
            }
            Exp::Const(a) => tt(self.ty(*a).the_type().to_const()),
            Exp::Designator { name, value } => match self.name_of(*name) {
                Some(n) => Ty::Designator(n, TyRef::new(self.ty(*value).un_const())),
                None => Ty::Unresolved,
            },
            Exp::Enum { ty, .. } => {
                let qn = self.qualified_name_of_decl(id).unwrap_or_else(SymbolPath::empty);
                tt(Ty::Enum(qn, TyRef::new(self.ty(*ty).the_type())))
            }
            // `try (tConst . theType) $ lookupSymbolByName (getQualifier x)`:
            // the enum the constant belongs to.
            Exp::EnumConstant { scope, .. } => match self.by_name(scope) {
                Some(d) => self.decl_ty(d).the_type().to_const(),
                None => Ty::Unresolved,
            },
            Exp::EnumValue { ty, .. } => {
                let mut q = kg_hir::sort::spine_qualname(self.arena, *ty);
                q.pop();
                match self.by_name(&SymbolPath::new(q)) {
                    Some(d) => self.decl_ty(d).the_type().to_const(),
                    None => Ty::Unresolved,
                }
            }
            Exp::FanOut { count, value } => match self.int_value(*count) {
                Some(n) => Ty::Array { attrs: TAttrSet::new(), elem: TyRef::new(self.ty(*value)), dims: vec![n] },
                None => Ty::Unresolved,
            },
            Exp::Float => tt(Ty::Float),
            Exp::FloatLiteral(_) => Ty::Float,
            Exp::FuncParam { ty, .. } => self.ty(*ty).the_type(),
            Exp::Function { kind, attr, ret, params, body, .. } => {
                let r = if matches!(self.exp(*ret), Exp::Auto) {
                    // `returnType Auto`: the first `return` or `static if` of
                    // the body, `TVoid` without one.
                    self.seq(*body)
                        .into_iter()
                        .find(|&s| matches!(self.exp(s), Exp::StaticIf { .. } | Exp::Return(_)))
                        .map_or(Ty::Void, |s| self.ty(s))
                } else {
                    self.ty(*ret).the_type()
                };
                self.func_type(*kind, *attr, r, *params)
            }
            Exp::FunctionCall { function, args, .. } => self.elaborate_type(*args, self.ty(*function).return_ty()),
            Exp::FunctionDecl { attr, ret, params, .. } => {
                self.func_type(FunctionKind::FreeFunction, *attr, self.ty(*ret).the_type(), *params)
            }
            Exp::FunctionSpecifier { receiver: None, function } => self.ty(*function),
            Exp::FunctionSpecifier { receiver: Some(a), function } => match self.name_of(*function) {
                Some(n) => self.method(&self.ty(*a), n),
                None => Ty::Unresolved,
            },
            Exp::FunctionType { attr, params, ret } => {
                let r = if matches!(self.exp(*ret), Exp::Auto) { Ty::Auto } else { self.ty(*ret).the_type() };
                match self.func_type(FunctionKind::FreeFunction, *attr, r, *params) {
                    Ty::Unresolved => Ty::Unresolved,
                    f => tt(f),
                }
            }
            Exp::FunctionTypeParam { ty, .. } => {
                if matches!(self.exp(*ty), Exp::Auto) {
                    Ty::Auto
                } else {
                    self.ty(*ty).the_type()
                }
            }
            Exp::InitializerList(items) => {
                Ty::Initializer(self.seq(*items).into_iter().map(|i| TyRef::new(self.ty(i))).collect())
            }
            Exp::Integer(w) => tt(Ty::Signed(*w)),
            Exp::IntLiteral { ty, value } => {
                kg_ty::width::infer_int_literal_type(&ConstValue::Int { ty: *ty, value: value.clone() })
            }
            Exp::InterpolatedString(_) | Exp::Interpolated(_) | Exp::StringLiteral(_) => Ty::Str,
            Exp::Lambda { .. } => Ty::Unresolved,
            Exp::MemberAccess { base, member } => {
                let Some(field) = self.name_of(*member) else { return Ty::Unresolved };
                match self.ty(*base) {
                    Ty::Reference(s) => match self.by_name(&s) {
                        Some(d) => match self.decl_exp(d) {
                            Exp::Class { body, .. } => {
                                self.decl_fields(d.module, *body)
                                    .into_iter()
                                    .find(|(n, _)| *n == field)
                                    .map_or(Ty::Undefined, |(_, t)| (*t).clone())
                            }
                            _ => Ty::Unresolved,
                        },
                        None => Ty::Unresolved,
                    },
                    t => t.field(field, Some(self.interner)),
                }
            }
            Exp::Mux { selector, cases } => {
                let arms = self.seq(*cases);
                let pick = match self.exp(*selector) {
                    Exp::IntLiteral { value, .. } => Some(value.to_usize()),
                    Exp::BoolLiteral(b) => Some(Some(usize::from(*b))),
                    _ => None,
                };
                match pick {
                    Some(i) => i.and_then(|i| arms.get(i)).map_or(Ty::Unresolved, |&a| self.ty(a).un_const()),
                    None => {
                        let tys: Vec<Ty> = arms.iter().map(|&a| self.ty(a).un_const()).collect();
                        kg_ty::subtype::t_mux_pretty(tys.iter(), Some(self.interner)).unwrap_or(Ty::Unresolved)
                    }
                }
            }
            Exp::NamedValue(a) => self.ty(*a),
            Exp::QualifiedIdentifier(a) | Exp::TypeIdentifier(_, a) => {
                if matches!(self.exp(*a), Exp::TemplateInstance { .. }) || self.ty(*a) == Ty::Unresolved {
                    return Ty::Unresolved;
                }
                self.type_of_symbol(id)
            }
            Exp::ParamInt(x) => self.natural(*x).map_or(Ty::Unresolved, |w| tt(Ty::Signed(w))),
            Exp::ParamUint(x) => self.natural(*x).map_or(Ty::Unresolved, |w| tt(Ty::Unsigned(w))),
            Exp::Positional { index, value } => Ty::Positional(*index, TyRef::new(self.ty(*value).un_const())),
            Exp::Reference(a) => tt(Ty::Reference(SymbolPath::new(kg_hir::sort::spine_qualname(self.arena, *a)))),
            Exp::Return(None) => Ty::Void,
            Exp::Return(Some(a)) => self.ty(*a).un_const(),
            Exp::ScopedName { prefix, ident, .. } => {
                let Some(leaf) = self.name_of(*ident) else { return Ty::Untyped };
                let text = self.interner.resolve(leaf);
                if (prefix.is_none() && text.starts_with('@')) || text.starts_with('$') {
                    return Ty::Untyped;
                }
                match self.lookup_symbol(id) {
                    Some(d) if matches!(self.decl_exp(d), Exp::Alias { .. }) => non_error(self.decl_ty(d)),
                    Some(_) => Ty::Untyped,
                    None => {
                        if prefix.is_some_and(|p| self.ty(p) == Ty::Unresolved) {
                            Ty::Unresolved
                        } else {
                            Ty::Undefined
                        }
                    }
                }
            }
            Exp::Static(a) => self.ty(*a),
            Exp::StaticIf { .. } => Ty::Unresolved,
            Exp::Struct { body, .. } => {
                let qn = self.qualified_name_of_decl(id).unwrap_or_else(SymbolPath::empty);
                tt(Ty::Struct(qn, self.fields(*body)))
            }
            Exp::Union { body, .. } => {
                let qn = self.qualified_name_of_decl(id).unwrap_or_else(SymbolPath::empty);
                tt(Ty::Union(qn, self.fields(*body)))
            }
            Exp::Str => tt(Ty::Str),
            Exp::Template { .. } => Ty::Template,
            Exp::TemplateInstance { .. } | Exp::TypeParam { .. } => Ty::Unresolved,
            Exp::This { .. } => match self.lookup_symbol(id) {
                Some(d) => match self.decl_exp(d) {
                    Exp::Class { .. } => {
                        match crate::symbols::declared_name_by(&|x| self.snap_exp(d.module, x), d.id) {
                            Some((scope, name)) => {
                                let mut q = scope.segments().to_vec();
                                q.push(name);
                                Ty::Reference(SymbolPath::new(q))
                            }
                            None => Ty::Unresolved,
                        }
                    }
                    _ => self.decl_ty(d),
                },
                None => Ty::Unresolved,
            },
            Exp::Unary(op, a) => {
                if !self.resolved(*a) {
                    return Ty::Unresolved;
                }
                let literal = self.int_literal(*a);
                kg_ty::width::infer_unary_op(*op, &self.ty(*a), literal.as_ref(), Some(self.interner))
            }
            Exp::Unsigned(w) => tt(Ty::Unsigned(*w)),
            Exp::Warning { node, .. } => self.ty(*node),
            Exp::Variable { ty, init, .. } => match self.exp(*ty) {
                Exp::Const(inner) if matches!(self.exp(*inner), Exp::Auto) => {
                    init.map_or(Ty::Unresolved, |i| self.ty(i).to_const())
                }
                Exp::Auto => init.map_or(Ty::Unresolved, |i| self.ty(i).un_const()),
                _ => self.ty(*ty).the_type(),
            },
            Exp::Void => tt(Ty::Void),
            // A name the sorter could not sort, and a construct the lowering
            // could not take: nothing in the reference's tree, so unknown —
            // never `TUntyped`, which reads as resolved and would type a call
            // through it as "Undefined function".
            Exp::Path(_) | Exp::Unsupported => Ty::Unresolved,
            _ => Ty::Untyped,
        }
    }

    /// `inferBinaryOp` (`Type.hs:756-775`): the literal-specialised rules
    /// picked by the operand *nodes*, then `tBinaryOp`.
    fn infer_binary_op(&self, op: BinOp, a: ExpId, b: ExpId) -> Ty {
        let names = Some(self.interner);
        let (ta, tb) = (self.ty(a), self.ty(b));
        let untyped = |x: ExpId| self.int_literal(x).is_some_and(|v| v.is_untyped_int());
        let literal = |x: ExpId| self.int_value(x);
        let rule = match op {
            BinOp::BitwiseAnd | BinOp::Mul if untyped(b) => literal(b).map(|v| (&ta, v)),
            BinOp::BitwiseAnd | BinOp::Mul if untyped(a) => literal(a).map(|v| (&tb, v)),
            BinOp::Div | BinOp::Mod | BinOp::ShiftLeft | BinOp::ShiftRight => literal(b).map(|v| (&ta, v)),
            _ => None,
        };
        if let Some((operand, value)) = rule
            && let Some(t) = kg_ty::width::t_binary_op_literal(op, operand, &value, names)
        {
            return t;
        }
        kg_ty::width::t_binary_op(op, kg_ty::width::type_of_operand(&ta), kg_ty::width::type_of_operand(&tb), names)
    }

    /// `elaborateType` (`Type.hs:1375-1378`): a `[[pipelined]]` callee's
    /// dependent return is an array sized by the call's first argument.
    fn elaborate_type(&self, args: ExpId, t: Ty) -> Ty {
        match t {
            Ty::Dependent(y) if matches!(*y, Ty::Void) => Ty::Void,
            Ty::Dependent(y) => {
                let dims = match self.seq(args).first() {
                    None => Vec::new(),
                    Some(&x) => self.int_value(x).into_iter().collect(),
                };
                Ty::Array { attrs: TAttrSet::new(), elem: y, dims }
            }
            t => t,
        }
    }

    /// `funcType` (`Type.hs:1363-1373`).
    fn func_type(&self, kind: FunctionKind, attr: ExpId, ret: Ty, params: ExpId) -> Ty {
        let Some(attrs) = self.attributes(attr) else { return Ty::Unresolved };
        let pipelined = self.seq(attr).iter().any(|&a| matches!(self.exp(a), Exp::FlagAttr(kg_ty::Flag::Pipelined)));
        let ret = if pipelined { Ty::Dependent(TyRef::new(ret)) } else { ret };
        Ty::Function { kind, attrs, ret: TyRef::new(ret), params: self.params(params) }
    }

    /// `attributes` (`Type.hs:1328-1333`): `Nothing` if an integer attribute
    /// has not folded.
    fn attributes(&self, attr: ExpId) -> Option<TAttrSet> {
        let mut out = TAttrSet::new();
        for a in self.seq(attr) {
            let t = match self.exp(a) {
                Exp::IntAttr(x, y) => TAttr::Int(*x, self.int_value(*y)?),
                Exp::Attr(x, y) => {
                    TAttr::Named(*x, SymbolPath::new(kg_hir::sort::spine_qualname(self.arena, *y)), TyRef::new(self.ty(*y)))
                }
                Exp::FlagAttr(f) => TAttr::Flag(*f),
                _ => continue,
            };
            out.insert(t);
        }
        Some(out)
    }

    /// `flags` (`Type.hs:1335-1340`).
    fn flags(&self, attr: ExpId) -> TAttrSet {
        let mut out = TAttrSet::new();
        for a in self.seq(attr) {
            let mut e = a;
            loop {
                match self.exp(e) {
                    Exp::FlagAttr(f) => {
                        out.insert(TAttr::Flag(*f));
                        break;
                    }
                    Exp::Warning { node, .. } => e = *node,
                    _ => break,
                }
            }
        }
        out
    }

    /// `params` (`Type.hs:1350-1356`).
    fn params(&self, params: ExpId) -> Vec<TFuncParam> {
        self.seq(params)
            .into_iter()
            .filter_map(|p| match self.exp(p) {
                Exp::FunctionTypeParam { attr, name, .. } => {
                    let t = self.ty(p);
                    if t == Ty::Void {
                        return None;
                    }
                    Some(TFuncParam { attrs: self.flags(*attr), ty: TyRef::new(t), name: name.and_then(|n| self.name_of(n)) })
                }
                Exp::FuncParam { attr, name, .. } => {
                    Some(TFuncParam { attrs: self.flags(*attr), ty: TyRef::new(self.ty(p)), name: self.name_of(*name) })
                }
                _ => None,
            })
            .collect()
    }

    /// `fields` (`Type.hs:1343-1348`) of a body in this module's tree.
    fn fields(&self, body: ExpId) -> Vec<(Symbol, TyRef)> {
        self.seq(body)
            .into_iter()
            .filter_map(|m| match self.exp(m) {
                Exp::Variable { name, .. } => self.name_of(*name).map(|n| (n, TyRef::new(self.ty(m)))),
                Exp::StaticIf { .. } => Some((self.names.empty, TyRef::new(Ty::Unresolved))),
                _ => None,
            })
            .collect()
    }

    /// [`Cx::fields`] of a body in the snapshot of any module.
    fn decl_fields(&self, module: usize, body: ExpId) -> Vec<(Symbol, TyRef)> {
        let Exp::Seq(items) = self.snap_exp(module, body) else { return Vec::new() };
        items
            .iter()
            .filter_map(|&m| match self.snap_exp(module, m) {
                Exp::Variable { name, .. } => match *self.snap_exp(module, *name) {
                    Exp::Identifier(n) => Some((n, TyRef::new(self.snap_ty(module, m)))),
                    _ => None,
                },
                Exp::StaticIf { .. } => Some((self.names.empty, TyRef::new(Ty::Unresolved))),
                _ => None,
            })
            .collect()
    }

    /// `method` (`Type.hs:1358-1362`).
    fn method(&self, t: &Ty, name: Symbol) -> Ty {
        match t {
            Ty::Class(x, _) => {
                let mut key = x.segments().to_vec();
                key.push(name);
                self.by_name(&SymbolPath::new(key)).map_or(Ty::Unresolved, |d| self.decl_ty(d))
            }
            Ty::Reference(x) => {
                let t = self.by_name(x).map_or(Ty::Unresolved, |d| self.decl_ty(d).the_type());
                self.method(&t, name)
            }
            Ty::Instance { concrete, .. } => self.method(concrete, name),
            _ => Ty::Unresolved,
        }
    }

    // --- names --------------------------------------------------------------

    /// `getName`: an identifier's symbol, and a qualified name's leaf —
    /// `FunctionSpecifierF (Just a) b`'s `b` is a `QualifiedIdentifierF`.
    fn name_of(&self, id: ExpId) -> Option<Symbol> {
        match self.exp(id) {
            Exp::Identifier(s) => Some(*s),
            Exp::QualifiedIdentifier(_) | Exp::ScopedName { .. } | Exp::NamedValue(_) | Exp::TypeIdentifier(..) => {
                kg_hir::sort::spine_qualname(self.arena, id).last().copied()
            }
            _ => None,
        }
    }

    /// `getQualifiedName` of a declaration: its scope and its name.
    fn qualified_name_of_decl(&self, id: ExpId) -> Option<SymbolPath> {
        let (scope, name) = declared_name(self.arena, id)?;
        let mut q = scope.segments().to_vec();
        q.push(name);
        Some(SymbolPath::new(q))
    }

    /// `lookupSymbol x` (`Symbols.hs:79-80`): `lookupInScope (getScope x) x`.
    fn lookup_symbol(&self, id: ExpId) -> Option<Decl> {
        let (scope, name): (SymbolPath, Vec<Symbol>) = match self.exp(id) {
            Exp::ScopedName { scope, .. } => (scope.clone(), kg_hir::sort::spine_qualname(self.arena, id)),
            Exp::QualifiedIdentifier(a) => match self.exp(*a) {
                Exp::ScopedName { scope, .. } => (scope.clone(), kg_hir::sort::spine_qualname(self.arena, *a)),
                _ => return None,
            },
            Exp::TypeIdentifier(scope, a) => (scope.clone(), kg_hir::sort::spine_qualname(self.arena, *a)),
            Exp::This { scope } => (scope.clone(), vec![self.names.this]),
            Exp::Capture { scope, name } => (scope.clone(), vec![self.name_of(*name)?]),
            _ => return None,
        };
        if name.is_empty() {
            return None;
        }
        self.in_scope(scope.segments(), &name)
    }

    /// `typeOfSymbol` (`Type.hs:1342`).
    ///
    /// A use of a *function* template is `TUnresolved`: the reference's
    /// parser wraps a name `findSymbol` knows as a template in an empty
    /// `TemplateInstanceF` (`decomposeInstance`), which infers `TUnresolved`,
    /// and so does the sorter for a template it can see — but an imported
    /// one its view does not flag reaches here bare (`test/library/data/
    /// array.k`'s `bind2nd`, `kg-oracle --typed --passes 6`). This holds the
    /// answer the wrap would have given until the sorter's import view
    /// carries template shapes.
    ///
    /// Only a name the source wrote: the parser's wrap is the parser's, and
    /// a name a rewrite minted — `desugarLambdas`' `Closure` argument naming
    /// the generic lambda it made a template — is typed as its declaration
    /// is, `TTemplate`.
    fn type_of_symbol(&self, id: ExpId) -> Ty {
        let written = self.arena[id].src.written == kg_base::Written::Own;
        match self.lookup_symbol(id) {
            Some(d) if written && matches!(self.decl_exp(d), Exp::Template { decl, .. }
                if matches!(self.snap_exp(d.module, *decl), Exp::Function { .. } | Exp::FunctionDecl { .. })) =>
            {
                Ty::Unresolved
            }
            Some(d) => non_error(self.decl_ty(d)),
            None => Ty::Undefined,
        }
    }

    // --- literals -------------------------------------------------------------

    /// `isIntLiteral` (`Type.hs:1082-1085`), seeing through `EnumValue`.
    fn is_int_literal(&self, id: ExpId) -> bool {
        self.int_literal(id).is_some()
    }

    /// The literal an `IntLiteral` (or an `EnumValue` of one) is.
    fn int_literal(&self, id: ExpId) -> Option<ConstValue> {
        match self.exp(id) {
            Exp::IntLiteral { ty, value } => Some(ConstValue::Int { ty: *ty, value: value.clone() }),
            Exp::EnumValue { value, .. } => self.int_literal(*value),
            _ => None,
        }
    }

    /// `fromIntegralConstexpr` (`Type.hs:1092-1095`).
    fn int_value(&self, id: ExpId) -> Option<ConstInt> {
        self.int_literal(id).and_then(|v| v.as_int().cloned())
    }

    /// `fromNaturalConstexpr` (`Type.hs:1097-1098`) as a width.
    fn natural(&self, id: ExpId) -> Option<u32> {
        let v = self.int_value(id)?;
        if v.is_negative() || v.to_i128() == Some(0) {
            return None;
        }
        v.to_u32()
    }

    /// `sizeOf = tWidth . typeOf` (`Type.hs:467-468`).
    /// `evalBinaryInt` and `evalBinaryBool` (`Constexpr.hs:230-246`): the
    /// folds that need only one side's value — `x & 0` where `x` is no wider
    /// than the zero's type, `x | y` where `y` sets every bit of `x`,
    /// `x || true`, `x && false`. `y_node` is the literal's node, whose
    /// *type* `sizeOf y` reads.
    fn one_literal(&self, op: BinOp, x: ExpId, y_node: ExpId, y: &ConstValue) -> Option<ConstValue> {
        match y {
            ConstValue::Int { value, .. } => {
                if !self.ty(x).is_int() {
                    return None;
                }
                let wx = self.size_of(x)?;
                match op {
                    BinOp::BitwiseAnd if value.is_zero() => {
                        // `sizeOf x <= sizeOf y`: the zero's node's type —
                        // a written `0` is `uint1`, but a constant substituted
                        // there wears the variable's note (`NotedExp n $ unNote
                        // $ unfix v`, `Constexpr.hs:256-258`): `i & x` over
                        // `const uint32 x = 0` folds for a `uint32 i`
                        // (`constexpr.k::143`), as the CST folder always had it.
                        let wy = self.size_of(y_node)?;
                        (wx <= wy).then(|| ConstValue::Int { ty: None, value: value.clone() })
                    }
                    BinOp::BitwiseOr if (0..wx).all(|bit| value.test_bit(bit)) => {
                        Some(ConstValue::Int { ty: None, value: value.clone() })
                    }
                    _ => None,
                }
            }
            ConstValue::Bool(v) => {
                if !self.ty(x).un_const().is_bool() {
                    return None;
                }
                match (op, v) {
                    (BinOp::Or, true) => Some(ConstValue::Bool(true)),
                    (BinOp::And, false) => Some(ConstValue::Bool(false)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn size_of(&self, id: ExpId) -> Option<u64> {
        kg_ty::width::t_width(&self.ty(id))
    }

    // --- evaluateConstexpr (Constexpr.hs:117-356), in place --------------------

    fn replace(&mut self, id: ExpId, exp: Exp) {
        if *self.exp(id) != exp {
            self.replaced += 1;
            // What the subtree files under every config, before and after:
            // inference's own map is rebuilt when its list moved, and a
            // write that moves none is reported harmless to the maps' kept
            // entries (`symbols::Filed`), which any other write discards.
            let filed = |cx: &Self| crate::symbols::filed_everywhere(cx.arena, id, cx.names.this, cx.names.errors);
            let before = self.overlay.is_some().then(|| filed(self));
            if let Some(o) = self.overlay.as_deref_mut() {
                o.entry(id).or_insert_with(|| self.arena[id].exp.clone());
            }
            if let Some(units) = self.units.as_deref_mut() {
                units.touch(self.module, id.index());
            }
            let old = std::mem::replace(&mut self.arena.node_mut(id).exp, exp);
            if let Some(before) = before {
                let after = filed(self);
                let inference = crate::symbols::Config::TemplateDecl.slot();
                self.filed_differently |= before[inference] != after[inference];
                if crate::symbols::harmless(&old, &self.arena[id].exp, &before, &after) {
                    self.vetted += 1;
                }
            }
        }
    }

    fn literal_exp(v: &ConstValue) -> Option<Exp> {
        Some(match v {
            ConstValue::Int { ty, value } => Exp::IntLiteral { ty: *ty, value: value.clone() },
            ConstValue::Bool(b) => Exp::BoolLiteral(*b),
            ConstValue::Float(f) => Exp::FloatLiteral(*f),
            _ => return None,
        })
    }

    /// The value a folded operand node already is.
    fn node_value(&self, id: ExpId) -> Option<ConstValue> {
        match self.exp(id) {
            Exp::IntLiteral { ty, value } => Some(ConstValue::Int { ty: *ty, value: value.clone() }),
            Exp::BoolLiteral(b) => Some(ConstValue::Bool(*b)),
            Exp::FloatLiteral(f) => Some(ConstValue::Float(*f)),
            Exp::InterpolatedString(tok) | Exp::StringLiteral(tok) => {
                kg_hir::exp::plain_string_value(tok).map(|s| ConstValue::Str(kg_base::SmolStr::new(s)))
            }
            Exp::EnumValue { value, .. } => self.node_value(*value),
            _ => None,
        }
    }

    /// `intLiteral | otherwise = typedIntLiteral $ inferBinaryOp op a b`
    /// (`Constexpr.hs:200-202`): a fold with a typed side is typed by the
    /// operator's rule over the two *nodes* — their notes, which a folded
    /// operand keeps from before it folded — not over the literals' own
    /// types. `-1 << Threshold` over `const uint16 Threshold = 8` is
    /// `(-1 : int2) << 8u16`, the `-1` wearing its negation's `tSuper uint1
    /// int1` (`Type.hs:745-746`): `-256` typed `int10`, where the literal's
    /// own `int1` would give `int9` (`optimization_test_cases.k`,
    /// `TestCompareShift`, the recorded walk). A type with no width declines
    /// the fold (`fromJust $ tWidth t` would crash the reference).
    fn typed_result(&self, op: BinOp, a: ExpId, b: ExpId, v: ConstValue) -> Option<ConstValue> {
        match v {
            ConstValue::Int { ty: Some(_), value } => {
                let t = self.infer_binary_op(op, a, b);
                let width = u32::try_from(kg_ty::width::t_width(&t)?).ok()?;
                // `signedness = if tIsSigned t then SignedLiteral else
                // UnsignedLiteral` (`Constexpr.hs:324-327`).
                let ty = if t.is_signed() {
                    kg_const::LiteralType::signed(width)
                } else {
                    kg_const::LiteralType::unsigned(width)
                };
                Some(ConstValue::Int { ty: Some(ty), value })
            }
            other => Some(other),
        }
    }

    fn evaluate_constexpr(&mut self, id: ExpId) {
        let exp = self.exp(id).clone();
        match exp {
            Exp::TemplateInstance { args, .. } => {
                self.template_args(args);
                return;
            }
            // `eval (ScopedNameF a (Just b) c) | tIsClass (theType b) ||
            // tIsEnum (theType b) = ScopedNameF a (scopedQualifier a c $
            // tQualifiedName $ theType b) c` (`Constexpr.hs:159-160`): a name
            // reached through a class — an alias of a copy, `Bbool::A` — is
            // qualified by the class's own name, so a lookup finds its member.
            Exp::ScopedName { scope, prefix: Some(b), ident } => {
                let t = self.ty(b).the_type();
                if !(t.is_class() || t.is_enum()) {
                    return;
                }
                let Some(q) = t.qualified_name().cloned() else { return };
                if kg_hir::sort::spine_qualname(self.arena, b) == q.segments() {
                    return;
                }
                let mut rw = kg_hir::rewrite::Rewriter::new(self.arena, self.interner);
                let mut m = crate::from_type::Mint { rw: &mut rw, types: self.types };
                let Some(prefix) = m.scoped_qualifier(&scope, ident, q.segments()) else { return };
                self.replace(id, Exp::ScopedName { scope, prefix: Some(prefix), ident });
                return;
            }
            // `eval (BitOffsetF x (Identifier y)) | typeResolved x = …`
            // (`Constexpr.hs:162-166,347-351`).
            Exp::BitOffset { ty, field } | Exp::ByteOffset { ty, field } => {
                let Exp::Identifier(y) = *self.exp(field) else { return };
                if !self.resolved(ty) {
                    return;
                }
                let bits = matches!(exp, Exp::BitOffset { .. });
                let name = if bits { "bitoffsetof" } else { "byteoffsetof" };
                let illegal = || {
                    Exp::Error(kg_hir::ErrorNote::new(format!(
                        "Illegal operands for {name}: {}, {}",
                        self.show_unmangled(&self.ty(ty).the_type()),
                        self.interner.resolve(y)
                    )))
                };
                let lit = |v: u64| Exp::IntLiteral { ty: None, value: kg_const::ConstInt::from(v) };
                let out = if bits {
                    kg_ty::subtype::bit_offset_of(&self.ty(ty), y).map_or_else(illegal, lit)
                } else {
                    match kg_ty::subtype::byte_offset_of(&self.ty(ty), y) {
                        Some(Ok(v)) => lit(v),
                        Some(Err(msg)) => Exp::Error(kg_hir::ErrorNote::new(msg)),
                        None => illegal(),
                    }
                };
                self.replace(id, out);
                return;
            }
            Exp::Unary(UnOp::Clog2, x) => {
                if let Some(v) = self.node_value(x).filter(|v| v.as_natural().is_some())
                    && let Some(folded) = kg_hir::constexpr::unary(UnOp::Clog2, &v)
                    && let Some(e) = Self::literal_exp(&folded)
                {
                    self.replace(id, e);
                }
                return;
            }
            Exp::Binary(op, a, b) => {
                if self.resolved(a) && self.resolved(b) {
                    let folded = if let (Ty::Type(s), Ty::Type(t)) = (self.ty(a), self.ty(b))
                        && self.node_value(a).is_none()
                        && self.node_value(b).is_none()
                    {
                        kg_hir::constexpr::binary_types(op, &s, &t).map(ConstValue::Bool)
                    } else {
                        match (self.node_value(a), self.node_value(b)) {
                            (Some(x), Some(y)) => {
                                // `Div | y /= 0 … | otherwise -> desugarError
                                // "Division by zero is undefined"`, and
                                // `Mod`'s remainder (`Constexpr.hs:181-186`):
                                // the node becomes the error, pass 6's.
                                if let Err(message) = kg_const::fold_binary_values(op, &x, &y) {
                                    self.replace(id, Exp::Error(kg_hir::ErrorNote::new(message)));
                                    return;
                                }
                                kg_hir::constexpr::binary(op, &x, &y).and_then(|v| self.typed_result(op, a, b, v))
                            }
                            // One literal side (`Constexpr.hs:216-246`):
                            // `evalBinary x y@IntLiteral` before its mirror.
                            (None, Some(y)) => self.one_literal(op, a, b, &y),
                            (Some(x), None) => self.one_literal(op, b, a, &x),
                            (None, None) => None,
                        }
                    };
                    if let Some(e) = folded.as_ref().and_then(Self::literal_exp) {
                        self.replace(id, e);
                    }
                }
                return;
            }
            Exp::Unary(op, a) => {
                // `bytesFromBits "bytesizeof is undefined for a type with
                // bit size of " <$> sizeOf x` (`Constexpr.hs:274-275,
                // 349-351`): a width that is not whole bytes is the node's
                // error.
                if op == UnOp::ByteSizeof
                    && let Some(w) = self.size_of(a).filter(|w| !w.is_multiple_of(8))
                {
                    let message = format!("bytesizeof is undefined for a type with bit size of {w}");
                    self.replace(id, Exp::Error(kg_hir::ErrorNote::new(message)));
                    return;
                }
                let folded = match op {
                    UnOp::BitSizeof | UnOp::ByteSizeof => {
                        self.size_of(a).and_then(|w| kg_hir::constexpr::size(op, w))
                    }
                    UnOp::Invert if !self.resolved(id) => None,
                    // `tIsEnum $ typeOf x = invert (tEnumBase $ typeOf x)`
                    // (`Constexpr.hs:262`): wrapped to the enum's base, and
                    // `intLiteral` keeps an untyped value untyped, else
                    // stamps `tInvert (typeOfOperand x)` — the base
                    // (`Type.hs:732-744`). `~E5::Count` over a `uint5` enum
                    // is `30` (`constexpr.k::118`).
                    UnOp::Invert if self.ty(a).is_enum() => {
                        let base = self.ty(a).enum_base();
                        self.node_value(a).and_then(|v| {
                            let wrapped = match &base {
                                Ty::Unsigned(w) => v.as_int()?.not().truncate_uint(*w),
                                Ty::Signed(w) => v.as_int()?.not().truncate_int(*w),
                                _ => return None,
                            };
                            Some(match v {
                                ConstValue::Int { ty: None, .. } => ConstValue::untyped_int(wrapped),
                                _ => ConstValue::Int { ty: kg_hir::constexpr::literal_type(&base), value: wrapped },
                            })
                        })
                    }
                    _ => self.node_value(a).and_then(|v| kg_hir::constexpr::unary(op, &v)),
                };
                if let Some(e) = folded.as_ref().and_then(Self::literal_exp) {
                    self.replace(id, e);
                }
                return;
            }
            Exp::NamedValue(x) => {
                self.fold_named_value(id, x);
                return;
            }
            _ => {}
        }
        // `eval _ | not $ typeResolved n = e`.
        if !self.resolved(id) {
            return;
        }
        match exp {
            // `eval (DecltypeF _ x) = fromMaybe (illegalDecltype x) $
            // expFromType x $ typeOf x` (`Constexpr.hs:304-305,345`).
            Exp::Decltype { expr, .. } => {
                let t = self.ty(expr);
                let minted = {
                    let mut rw = kg_hir::rewrite::Rewriter::new(self.arena, self.interner);
                    let mut m = crate::from_type::Mint { rw: &mut rw, types: self.types };
                    m.exp_from_type(expr, &t)
                };
                let out = minted.unwrap_or_else(|| {
                    Exp::Error(kg_hir::ErrorNote::new(format!(
                        "Argument of type `{}` is illegal for `decltype`",
                        self.show_unmangled(&t)
                    )))
                });
                self.replace(id, out);
            }
            Exp::ParamInt(x) => {
                if let Some(w) = self.natural(x) {
                    self.replace(id, Exp::Integer(w));
                }
            }
            Exp::ParamUint(x) => {
                if let Some(w) = self.natural(x) {
                    self.replace(id, Exp::Unsigned(w));
                }
            }
            // `eval (CastF t x)` (`Constexpr.hs:306-308`): an integral
            // operand through `castIntegralConstexpr`, a float literal
            // through `castFloatConstexpr`.
            Exp::Cast { ty, value } => {
                let operand = match self.exp(value) {
                    Exp::FloatLiteral(f) => Some(ConstValue::Float(*f)),
                    _ => self.int_literal(value),
                };
                if let Some(v) = operand
                    // The type under an alias copy's instance (`count_t<5>`).
                    && let Some(folded) = kg_hir::constexpr::cast(self.ty(ty).the_type().concrete(), &v)
                    && let Some(e) = Self::literal_exp(&folded)
                {
                    self.replace(id, e);
                }
            }
            // `eval (MuxF (BoolLiteral x) y) = mux y $ fromEnum x`, `eval
            // (MuxF x y) = maybe e (mux y) $ fromIntegralConstexpr x`
            // (`Constexpr.hs:311-315`): the selector an `Int`, and a case it
            // does not select the node's error (`:353-355`). Reached only
            // once the mux is typed, which a literal selector out of range
            // never is (`Type.hs:1237-1238`): only one a later fold gave —
            // a name's, an enum constant's — errs (`mux(four, 0, 1, 2, 3)`
            // over `const uint3 four = 4`, `kg-oracle --errors`).
            Exp::Mux { selector, cases } => {
                let index: Option<i128> = match self.exp(selector) {
                    Exp::BoolLiteral(b) => Some(i128::from(*b)),
                    // `fromIntegral` into an `Int` wraps to 64 bits.
                    _ => self.int_value(selector).and_then(|v| v.truncate_int(64).to_i128()),
                };
                let Some(i) = index else { return };
                let items = self.seq(cases);
                match usize::try_from(i).ok().and_then(|i| items.get(i)) {
                    Some(&case) => {
                        let e = self.exp(case).clone();
                        self.replace(id, e);
                    }
                    None => {
                        let message = format!("Invalid selector `{i}` for the `mux` with {} elements", items.len());
                        self.replace(id, Exp::Error(kg_hir::ErrorNote::new(message)));
                    }
                }
            }
            Exp::Static(x) => {
                if self.node_value(x).is_some() {
                    let e = self.exp(x).clone();
                    self.replace(id, e);
                }
            }
            _ => {}
        }
    }

    /// `fromIntegralConstexpr` over another module's snapshot: an integer
    /// literal, or one under any number of `EnumValue`s (`Type.hs:1092-1095`).
    fn snap_int_literal(&self, module: usize, id: ExpId) -> Option<ConstValue> {
        match self.snap_exp(module, id) {
            Exp::IntLiteral { ty, value } => Some(ConstValue::Int { ty: *ty, value: value.clone() }),
            Exp::EnumValue { value, .. } => self.snap_int_literal(module, *value),
            _ => None,
        }
    }

    /// `eval (NamedValueF x) | tIsConst (typeOf x)` (`Constexpr.hs:280-290`):
    /// a constant with a literal initializer is that literal, cast to its
    /// declared type; an enum constant with a literal value is an
    /// `EnumValue` of it.
    fn fold_named_value(&mut self, id: ExpId, x: ExpId) {
        if !self.ty(x).is_const() {
            return;
        }
        let Some(d) = self.lookup_symbol(x) else { return };
        match self.snap_exp(d.module, d.id).clone() {
            Exp::Variable { ty, init: Some(v), .. } => {
                let (ty, v) = (&ty, &v);
                let declared = self.snap_ty(d.module, *ty).the_type();
                let v_exp = self.snap_exp(d.module, *v).clone();
                let v_literal = self.snap_int_literal(d.module, *v);
                let is_const_decl = matches!(self.snap_exp(d.module, *ty), Exp::Const(_));
                if !is_const_decl {
                    return;
                }
                // `tIsInt`/`castIntegralConstexpr` see through an alias
                // copy's instance (`count_t<N>` is `uint4` under it).
                let base = declared.un_const().concrete().un_const();
                if (base.is_int() || base.is_float()) && v_literal.is_some() {
                    if let Some(v) = v_literal
                        && let Some(folded) = kg_hir::constexpr::cast(&base, &v)
                        && let Some(e) = Self::literal_exp(&folded)
                    {
                        self.replace(id, e);
                    }
                    return;
                }
                // `isLiteral v || isEnum v || tIsFunction (typeOf v)`: the
                // initializer itself (`unNote $ unfix v`), its children
                // shared — carried over, types and all, from another module.
                let literal = match &v_exp {
                    Exp::IntLiteral { .. } | Exp::BoolLiteral(_) | Exp::FloatLiteral(_) | Exp::StringLiteral(_) => true,
                    Exp::InterpolatedString(tok) => kg_hir::plain_string_value(tok).is_some(),
                    Exp::EnumValue { .. } => true,
                    _ => false,
                };
                if literal || self.snap_ty(d.module, *v).is_function() {
                    let exp = if d.module == self.module { v_exp } else { self.import_children(d.module, v_exp) };
                    self.replace(id, exp);
                }
            }
            // `Just (EnumConstant _ _ (Just v)) | isIntLiteral v` — an
            // `EnumValue` of one too (`Type.hs:1087-1090`): `G = F` over `F =
            // Bar::B1` is `EnumValue G (EnumValue Bar::B1 0)`, the inner
            // value kept whole (`enum.k::8`, the recorded walk).
            Exp::EnumConstant { value: Some(v), .. } => {
                if self.snap_int_literal(d.module, v).is_some() {
                    let v_exp = self.snap_exp(d.module, v).clone();
                    let v_exp = if d.module == self.module { v_exp } else { self.import_children(d.module, v_exp) };
                    let src = self.arena[id].src;
                    let kind = self.arena[id].kind;
                    let lit = self.arena.alloc(v_exp, kg_base::Src::donated(src), kind);
                    // `NotedExp n $ unNote $ unfix v` (`Constexpr.hs:292`): the
                    // value wears the named value's note, its type included.
                    if self.types.is_set(id) {
                        let t = self.ty(id);
                        self.types.reset_type_of(lit, t);
                    }
                    self.replace(id, Exp::EnumValue { ty: x, value: lit });
                }
            }
            _ => {}
        }
    }

    /// `eval (TemplateInstanceF x y) = TemplateInstanceF x $ desugar'
    /// rewrite y` (`Constexpr.hs:133-152`): bottom up over the arguments,
    /// each typed node that is not a closure is resolved — a function named
    /// by value to the bare name, a constant aggregate to its initializer
    /// list, a type alias to the type it names. The node keeps its note.
    fn template_args(&mut self, id: ExpId) {
        let mut kids = Vec::new();
        self.exp(id).children(&mut kids);
        for k in kids {
            self.template_args(k);
        }
        let t = self.ty(id);
        if !t.resolved() || t.the_type().is_closure(&Ty::closure_template_path(self.interner)) {
            return;
        }
        match self.exp(id).clone() {
            Exp::NamedValue(a) if self.ty(a).is_function() => {
                let e = self.exp(a).clone();
                self.replace(id, e);
            }
            Exp::NamedValue(a) => {
                let ta = self.ty(a);
                let aggregate = ta.is_const() && (ta.is_struct() || ta.is_array() || ta.is_union());
                if !aggregate {
                    return;
                }
                let Some(d) = self.lookup_symbol(a) else { return };
                if let Exp::Variable { ty, init: Some(v), .. } = self.snap_exp(d.module, d.id)
                    && matches!(self.snap_exp(d.module, *ty), Exp::Const(_))
                    && matches!(self.snap_exp(d.module, *v), Exp::InitializerList(_))
                {
                    let e = self.snap_exp(d.module, *v).clone();
                    let e = if d.module == self.module { e } else { self.import_children(d.module, e) };
                    self.replace(id, e);
                }
            }
            Exp::TypeIdentifier(..) => {
                let Some(d) = self.lookup_symbol(id) else { return };
                if let Exp::Alias { body, .. } = self.snap_exp(d.module, d.id) {
                    // A copy even from this module: the reference's tree is
                    // persistent, so the next round's resolve of `word_t`
                    // inside the spliced `word_t[4]` rewrites the argument
                    // and never the alias declaring it; a shared child here
                    // is rewritten in place under the declaration too, which
                    // the walk then spells `uint8[4]`
                    // (`data/cache/write_only.k`'s `cache_line_t`).
                    let e = self.snap_exp(d.module, *body).clone();
                    let e = self.import_children(d.module, e);
                    self.replace(id, e);
                }
            }
            _ => {}
        }
    }

    /// `exp` with every child copied over from `module`'s snapshot, with its
    /// type — the node the reference's one tree would share.
    fn import_children(&mut self, module: usize, mut exp: Exp) -> Exp {
        for child in exp.children_mut() {
            *child = self.import(module, *child);
        }
        exp
    }

    fn import(&mut self, module: usize, id: ExpId) -> ExpId {
        let (src, kind) = self.snap_src(module, id);
        let exp = self.snap_exp(module, id).clone();
        let exp = self.import_children(module, exp);
        let new = self.arena.alloc(exp, src, kind);
        if self.snap_is_set(module, id) {
            let t = self.snap_ty(module, id);
            self.types.reset_type_of(new, t);
        }
        new
    }

    // --- staticControl (Constexpr.hs:41-56) ------------------------------------

    fn static_control(&mut self, id: ExpId) {
        if !matches!(self.exp(id), Exp::StaticIf { .. } | Exp::StaticFor { .. } | Exp::Seq(_)) {
            return;
        }
        let exp = self.exp(id).clone();
        let mut rw = kg_hir::rewrite::Rewriter::new(self.arena, self.interner);
        let out = kg_hir::rewrite::static_control_node(&mut rw, id, exp, self.expansions);
        // What the unrolling rebuilt keeps its note at once — an
        // iteration's `const auto` variable is `renote inductionVar a`
        // (`Constexpr.hs:88-98`), typed as the loop's variable was, which
        // the inference below would otherwise re-derive from its literal.
        for (new, original) in self.arena.take_lineage() {
            if !self.types.is_set(new) && self.types.is_set(original) {
                let t = self.types.get(original);
                self.types.reset_type_of(new, t);
            }
        }
        self.replace(id, out);
    }
}

/// `nonErrorTypeOf` (`Type.hs:1344-1347`).
fn non_error(t: Ty) -> Ty {
    match t {
        Ty::Error(_) | Ty::Undefined => Ty::Unresolved,
        t => t,
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    use super::*;

    /// Infer one file and return the type of each `const` declaration's
    /// initializer node, in order, as `show_pretty` spells it.
    fn init_types(text: &str) -> Vec<String> {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let inits: Vec<ExpId> = ir
            .arena
            .iter()
            .filter_map(|(_, n)| match &n.exp {
                Exp::Variable { init: Some(i), .. } => Some(*i),
                _ => None,
            })
            .collect();
        let mut program = Program::new([ir], &interner);
        program.infer();
        inits
            .into_iter()
            .map(|i| kg_ty::subtype::show_pretty(&program.modules[0].types.get(i), Some(&interner)))
            .collect()
    }

    /// `evalBinaryBool` and `evalBinaryInt` (`Constexpr.hs:230-246`): one
    /// literal side is enough for `x && false`, `x || true`, `x & 0` and an
    /// `x | y` whose `y` sets every bit of `x` — the `static(verbose) &&
    /// succeeded` of `test/unit.k`. Red with only two-literal folds: every
    /// return is still an operator.
    #[test]
    fn one_literal_side_folds_a_short_circuit_and_a_mask() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(
            "inline bool f(bool x)\n{\n    return x && false;\n}\n\ninline bool g(bool x)\n{\n    return x || true;\n}\n\ninline uint8 h(uint8 x)\n{\n    return x | 255;\n}\n",
        );
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let mut program = Program::new([ir], &interner);
        program.infer();
        let m = &program.modules[0];
        let returned: Vec<String> = crate::elaborate_tests_reachable(&m.ir)
            .into_iter()
            .filter_map(|id| match m.ir.arena[id].exp {
                Exp::Return(Some(v)) => Some(format!("{:?}", m.ir.arena[v].exp)),
                _ => None,
            })
            .collect();
        let mut returned = returned;
        returned.sort();
        assert_eq!(returned, ["BoolLiteral(false)", "BoolLiteral(true)", "IntLiteral { ty: None, value: 255 }"]);
    }

    /// `sizeOf y` is the zero's *node's* type (`Constexpr.hs:231-232`): a
    /// `const uint32 x = 0` substituted into `i & x` wears the variable's
    /// note, 32 bits, so the mask folds for a `uint32` or narrower `i` —
    /// as `static assert((i & x) == 0)` needs (`constexpr.k::143,144`). Red
    /// with the zero's minimal width (1 bit): the `&` stays an operator.
    #[test]
    fn a_mask_by_a_zero_constant_folds_at_the_constant_s_width() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(
            "inline uint32 f(uint32 i)\n{\n    const uint32 x = 0;\n    return i & x;\n}\n\ninline uint32 g(uint16 i)\n{\n    const uint32 x = 0;\n    return i & x;\n}\n",
        );
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let mut program = Program::new([ir], &interner);
        program.infer();
        let m = &program.modules[0];
        let returned: Vec<String> = crate::elaborate_tests_reachable(&m.ir)
            .into_iter()
            .filter_map(|id| match m.ir.arena[id].exp {
                Exp::Return(Some(v)) => Some(format!("{:?}", m.ir.arena[v].exp)),
                _ => None,
            })
            .collect();
        assert_eq!(returned, ["IntLiteral { ty: None, value: 0 }", "IntLiteral { ty: None, value: 0 }"]);
    }

    /// `eval (BitOffsetF x (Identifier y))` (`Constexpr.hs:162-166`): a
    /// typed operand folds to the field's offset. Red without the arm: the
    /// `static assert` over it never folds (`constexpr.k::99`).
    #[test]
    fn an_offset_of_a_typed_struct_folds() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file("struct S\n{\n    uint8 a;\n    uint8 b;\n}\n\nconst auto o = bitoffsetof(S, b);\nconst auto p = byteoffsetof(S, b);\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let mut program = Program::new([ir], &interner);
        program.infer();
        let m = &program.modules[0];
        let inits: Vec<String> = crate::elaborate_tests_reachable(&m.ir)
            .into_iter()
            .filter_map(|id| match m.ir.arena[id].exp {
                Exp::Variable { init: Some(i), .. } => Some(format!("{:?}", m.ir.arena[i].exp)),
                _ => None,
            })
            .collect();
        let mut inits = inits;
        inits.sort();
        assert_eq!(inits, ["IntLiteral { ty: None, value: 1 }", "IntLiteral { ty: None, value: 8 }"]);
    }

    /// `eval (DecltypeF _ x) = … expFromType x $ typeOf x`
    /// (`Constexpr.hs:304-305`): a typed `decltype` becomes the type it
    /// names. Red without the arm: the `Decltype` stays in the tree, where
    /// the mangle cannot spell it (`Closure<…, decltype(…)>` never made).
    #[test]
    fn a_typed_decltype_becomes_the_type_it_names() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file("inline void f()\n{\n    uint8 x;\n    decltype(x) y;\n}\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let mut program = Program::new([ir], &interner);
        program.infer();
        let m = &program.modules[0];
        let tys: Vec<String> = m
            .ir
            .arena
            .iter()
            .filter_map(|(_, n)| match &n.exp {
                Exp::Variable { ty, name, .. } if matches!(m.ir.arena[*name].exp, Exp::Identifier(s) if &*interner.resolve(s) == "y") => {
                    Some(format!("{:?}", m.ir.arena[*ty].exp))
                }
                _ => None,
            })
            .collect();
        assert_eq!(tys, ["Unsigned(8)"]);
    }

    /// A named enum constant folds to `EnumValueF x (NotedExp n …)`
    /// (`Constexpr.hs:291-292`): the literal wears the named value's note,
    /// so it is typed as the named constant is (`const E`) — the backend
    /// stamps it with the enum's base width (`uint2`), not the literal's own
    /// (`uint1`). Red with the literal left to inference: `uint1`.
    #[test]
    fn a_folded_enum_constants_literal_wears_the_named_values_note() {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file("enum E : uint2\n{\n    A = 0,\n    B = 1,\n    C = 2\n}\n\nconst E e = E::B;\n");
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let mut program = Program::new([ir], &interner);
        program.infer();
        let m = &program.modules[0];
        let literals: Vec<String> = m
            .ir
            .arena
            .iter()
            .filter_map(|(_, n)| match n.exp {
                Exp::EnumValue { value, .. } => Some(kg_ty::subtype::show_pretty(&m.types.get(value), Some(&interner))),
                _ => None,
            })
            .collect();
        assert_eq!(literals, ["const E"]);
    }

    /// A node the fold rewrites keeps the note it was typed with: `-128` is
    /// the negation of a `uint8`, `int9`, not the folded literal's `int8`
    /// (`Constexpr.hs:117-283` under `NotedExp n`); `1 + 2` over two
    /// literals is `TUnresolved` (`Type.hs:1182-1185`), folds, and is typed
    /// as the literal it became the round after; a comparison of two
    /// constants likewise becomes `bool`.
    #[test]
    fn a_folded_node_keeps_its_note_and_an_unresolved_one_is_typed_as_its_literal() {
        let v = init_types(
            "const auto a = -128;\nconst auto b = 1 + 2;\nconst uint8 k = 3;\nconst auto c = k != 0;\n",
        );
        assert_eq!(v, vec!["int9", "uint2", "uint2", "bool"]);
    }

    /// A method call is typed by its method: `getName b` of
    /// `FunctionSpecifierF (Just a) b` is the leaf of a qualified name, and
    /// `method` finds `C::Run` from the receiver's class (`Type.hs:1222`,
    /// `:1358-1362`). A local function template's use is the empty
    /// `TemplateInstance` the sorter wraps it in, `TUnresolved`; an
    /// imported one the sorter left bare is held to the same answer by
    /// `type_of_symbol`, which the `sem --oracle` gate pins (red with the
    /// rule removed: `test/library/data/array.k`'s `bind2nd`).
    #[test]
    fn a_method_call_takes_its_methods_return_and_a_function_template_use_is_unresolved() {
        let v = init_types(
            "class C\n{\npublic:\n    uint8 Run() { return 1; }\n}\n\ntemplate <typename T>\ninline T id(T x) { return x; }\n\ninline void go()\n{\n    static C c;\n    const auto a = c.Run();\n    const auto f = id;\n}\n",
        );
        assert_eq!(v, vec!["uint8", "unresolved"]);
    }

    /// A fold with a typed side is typed by `inferBinaryOp` over the two
    /// nodes (`Constexpr.hs:200-202`): `-1` keeps its negation's note,
    /// `int2`, so `-1 << k` over `const uint16 k = 8` is `int10` — the
    /// literal's own `int1` would make it `int9`
    /// (`optimization_test_cases.k`'s `TestCompareShift`, the recorded
    /// walk). Red with the literals' own types read.
    #[test]
    fn a_typed_fold_is_typed_by_its_operands_notes() {
        let v = init_types("const uint16 k = 8;\nconst auto s = -1 << k;\n");
        assert_eq!(v, vec!["uint4", "int10"]);
    }

    /// Declarations are read as the round found them, across the program:
    /// a name typed only by another declaration's inference resolves once
    /// that declaration has, and a type written by an alias is its target.
    #[test]
    fn a_name_takes_its_declarations_type() {
        let v = init_types("using T = uint4;\nconst T x = 1;\nconst auto y = x;\n");
        assert_eq!(v, vec!["uint1", "const uint4"]);
    }
}

/// `KG_SEM_TEXT='…' cargo test -p kg-sem inspect -- --nocapture` prints
/// every node of one program with the type the engine gave it — the
/// inspection aid beside `KG_SEM_DUMP=<path>:<offset>` (with
/// `KG_SEM_CTOR`) in `kg_db::sem`, which does the same inside a corpus run.
#[cfg(test)]
mod debug_tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    use super::*;

    #[test]
    fn inspect() {
        let text = std::env::var("KG_SEM_TEXT").unwrap_or_default();
        if text.is_empty() {
            return;
        }
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(&text);
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = kg_hir::lower_sorted(&file, FileId(0), &interner, &kg_hir::NoImports);
        let mut program = Program::new([ir], &interner);
        program.infer();
        let m = &program.modules[0];
        for (id, n) in m.ir.arena.iter() {
            eprintln!("{:>4} {:?} {:?} :: {}", id.index(), n.src.head.range, n.exp, kg_ty::subtype::show_pretty(&m.types.get(id), Some(&interner)));
        }
    }
}
