//! Reuse of a kept run's rounds (sprint W7.5d, D1 step 3;
//! `docs/plans/loop-across-edits.md` §6): a pass of a module the next run
//! need not run, its output taken from the kept run instead — one pass at
//! a time, the prefix first.
//!
//! **The prefix pass** (`kg_hir::rewrite`'s `round_prefix_in_place`, round
//! 0's `pipeline_tail_in_place`) reads its module alone and only appends:
//! what it does to a module is a function of the module as the pass finds
//! it. A module's state then is its state at the round before's end — the
//! kept run's wherever every round so far was keyed alike, its copies
//! (`crate::snapshot::Key`) and its reads (`crate::reads`) — moved on by
//! what the round did before the pass, `addLambdaWrappers` (and before
//! round 0, the passes ahead of the loop), whose reads are keyed to the
//! pass's start ([`crate::snapshot::ModuleRounds::reads_before_prefix`]).
//! Where all of that agrees, carried through the edit, the pass's output
//! is the kept run's: the nodes it appended as the pass left them (the
//! final arena's, with the round's later writes undone), the root, and
//! its types carried as `carry_types` carried them, by the kept pass's
//! `(new, original)` pairs ([`crate::snapshot::ModuleRounds::prefix_lineage`])
//! from the module as it stands — which is the kept module as it stood.
//!
//! **Taken in** ([`Reuse::prefix`]): each node's head carried through the
//! edit, its expansion as the run in hand interned the same frames (none
//! is interned anew: a node the pass appends wears its donor's), and each
//! child as the run in hand's arena names the same node — the same place
//! in the same chain of forks ([`kg_hir::ExpArena::translate`]), so an id
//! a rewrite compares is the id it would have built. Anything that does
//! not carry — a position inside the change, a frame not interned — and the
//! module runs the pass itself.
//!
//! **Checked** ([`Reuse::new`]'s `check`, `cargo xtask edit-identity`):
//! every pass taken is also run, on a fork, and the two compared node by
//! node; what differs is reported, and is the gate's to fail on.

use std::sync::Arc;

use kg_base::{Expansion, ExpansionInterner, FxHashMap, Interner, Src};
use kg_hir::{Exp, ExpArena, NodeData};

use crate::elaborate::Elaborated;
use crate::infer::Module;
use crate::snapshot::{History, Relocation, Rounds};

/// What a run reused of the kept run before it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReuseReport {
    /// Module-rounds whose prefix pass the kept run holds, of modules
    /// followed through the round before.
    pub offered: u64,
    /// Of those, the passes taken from the kept run, and the nodes they
    /// appended.
    pub reused: u64,
    pub nodes: u64,
    /// The passes checked against the pass run on a fork, and each one
    /// that differed: `module@round: what`.
    pub checked: u64,
    pub unequal: Vec<String>,
}

/// A kept run, and how far the run in hand may reuse it (module comment).
pub struct Reuse {
    old: Arc<Elaborated>,
    /// Each module of the run in hand: its module in `old`, where its input
    /// to the loop is `old`'s.
    map: Vec<Option<usize>>,
    names: Vec<String>,
    relocation: Relocation,
    check: bool,
    /// Per module, whether every round so far was keyed as `old`'s, and
    /// whether this round's prefix pass is taken from it.
    following: Vec<bool>,
    plan: Vec<bool>,
    /// Per module of `old`, where its logs changed each node.
    histories: FxHashMap<usize, History>,
    /// Each expansion of `old`'s as the run in hand interned it.
    vias: FxHashMap<Expansion, Expansion>,
    report: ReuseReport,
}

impl std::fmt::Debug for Reuse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reuse").field("report", &self.report).finish_non_exhaustive()
    }
}

impl Reuse {
    /// Reuse of `old`, a run that kept its snapshot: `map` gives each
    /// module of the run in hand its module in `old` where its input to the
    /// loop is `old`'s, `names` names them, and `relocation` carries `old`'s
    /// positions into this run's texts. With `check`, every pass taken is
    /// also run and compared. `None` where `old` kept none.
    pub fn new(old: Arc<Elaborated>, map: &[Option<usize>], names: Vec<String>, relocation: Relocation, check: bool) -> Option<Reuse> {
        let kept = old.snapshot.as_ref()?;
        let map: Vec<Option<usize>> = map.iter().map(|o| o.filter(|&o| o < kept.modules.len())).collect();
        let following = map.iter().map(Option::is_some).collect();
        Some(Reuse {
            old,
            map,
            names,
            relocation,
            check,
            following,
            plan: Vec::new(),
            histories: FxHashMap::default(),
            vias: FxHashMap::default(),
            report: ReuseReport::default(),
        })
    }

    /// Round `round` ended, its keys taken: a module is followed on only
    /// while they are `old`'s, carried through the edit — as the shadow
    /// follows it (`crate::snapshot::Shadow`).
    pub(crate) fn round_ended(&mut self, round: u32, last: bool, new: &Rounds) {
        let r = round as usize;
        let old = Arc::clone(&self.old);
        let kept = old.snapshot.as_ref().expect("checked in `new`");
        let count = kept.rounds();
        for m in 0..self.following.len() {
            let (true, Some(o), Some(n)) = (self.following[m], self.map[m], new.modules.get(m)) else { continue };
            let k = &kept.modules[o];
            self.following[m] = r < count
                && (r + 1 == count) == last
                && k.keys[r].agrees(&n.keys[r], &self.relocation)
                && k.reads[r].agrees(&n.reads[r], &self.relocation);
        }
    }

    /// Round `round`'s prefix pass begins: which modules take it from
    /// `old` — followed through the round before, keyed alike up to the
    /// pass, and standing where `old`'s module stood when its pass began.
    pub(crate) fn plan(&mut self, round: u32, modules: &[Module], new: &Rounds) {
        let r = round as usize;
        self.plan.clear();
        self.plan.resize(modules.len(), false);
        let old = Arc::clone(&self.old);
        let kept = old.snapshot.as_ref().expect("checked in `new`");
        for (m, module) in modules.iter().enumerate() {
            let (Some(&true), Some(&Some(o))) = (self.following.get(m), self.map.get(m)) else { continue };
            let k = &kept.modules[o];
            let (Some(p), Some(before), Some(now)) =
                (k.prefixes.get(r), k.reads_before_prefix.get(r), new.modules.get(m).and_then(|n| n.reads_before_prefix.get(r)))
            else {
                continue;
            };
            self.report.offered += 1;
            self.plan[m] = !p.wrote
                && before.agrees(now, &self.relocation)
                && module.ir.arena.len() == p.from as usize
                && module.ir.root.index() == p.root_from as usize
                // A root the pass changed is a node it appended.
                && (p.root == p.root_from || p.root >= p.from);
        }
    }

    /// Module `m`'s prefix pass in round `round`, taken from `old` where
    /// the plan allows and everything it appended carries: the module then
    /// as the pass would have left it, its types carried as `carry_types`
    /// carries them, by the kept pass's `(new, original)` pairs — which are
    /// handed back, for the run in hand's own snapshot.
    pub(crate) fn prefix(
        &mut self,
        m: usize,
        module: &mut Module,
        round: u32,
        interner: &Interner,
        expansions: &ExpansionInterner,
    ) -> Option<Vec<(u32, u32)>> {
        if !self.plan.get(m).copied().unwrap_or(false) {
            return None;
        }
        let r = round as usize;
        let old = Arc::clone(&self.old);
        let o = self.map[m].expect("planned");
        let k = &old.snapshot.as_ref().expect("checked in `new`").modules[o];
        let p = k.prefixes[r];
        let nodes = self.appended(&old, o, r, &module.ir.arena, expansions)?;
        let from = module.ir.arena.len();
        let probe = self.check.then(|| run(module, round, interner));
        self.report.reused += 1;
        self.report.nodes += nodes.len() as u64;
        for node in nodes {
            module.ir.arena.adopt(node);
        }
        if p.root != p.root_from {
            module.ir.root = module.ir.arena.id_at(p.root as usize);
        }
        let lineage = k.prefix_lineage[r].clone();
        let pairs: Vec<_> =
            lineage.iter().map(|&(n, o)| (module.ir.arena.id_at(n as usize), module.ir.arena.id_at(o as usize))).collect();
        crate::elaborate::carry_lineage(module, &pairs);
        if let Some(probe) = probe {
            self.report.checked += 1;
            if let Some(what) = check(&probe, module, from) {
                let name = self.names.get(m).cloned().unwrap_or_default();
                self.report.unequal.push(format!("{name}@{round}: {what}"));
            }
        }
        Some(lineage)
    }

    pub fn finish(self) -> ReuseReport {
        self.report
    }

    /// What `old`'s module `o` appended in round `r`'s prefix pass, as the
    /// pass left it and as `arena` — the run in hand's, standing where
    /// `o`'s stood when the pass began — would name it: `None` where any
    /// of it does not carry.
    fn appended(&mut self, old: &Elaborated, o: usize, r: usize, arena: &ExpArena, expansions: &ExpansionInterner) -> Option<Vec<NodeData>> {
        let k = &old.snapshot.as_ref()?.modules[o];
        let from = &old.modules[o].ir.arena;
        if !arena.parallels(from) {
            return None;
        }
        let p = k.prefixes[r];
        let history = self.histories.entry(o).or_insert_with(|| k.node_history());
        let (vias, relocation) = (&mut self.vias, &self.relocation);
        // A node as the pass left it: as the first segment after the pass's
        // end that logged it found it — round `r`'s after its prefix is
        // segment 2r + 1 — or as the run left it.
        let at = |i: u32| match history.get(&i).and_then(|h| h.iter().find(|&&(seg, _)| seg as usize > 2 * r)) {
            Some(&(seg, pos)) => k.logged_node(seg, pos).clone(),
            None => from.node_at(i as usize).clone(),
        };
        let mut out = Vec::with_capacity((p.len - p.from) as usize);
        for i in p.from..p.len {
            let NodeData { mut exp, src, kind } = at(i);
            let head = relocation.range(src.head)?;
            let via = via(vias, relocation, src.via, &old.expansions, expansions)?;
            for child in exp.children_mut() {
                *child = arena.translate(from, *child, p.len as usize)?;
            }
            out.push(NodeData { exp, src: Src { head, written: src.written, via }, kind });
        }
        Some(out)
    }

}

/// The kept run's expansion `via` as the run in hand interned the same
/// frames — each instance under the same parent, at the same site carried
/// through the edit, in the same round: `None` where it did not. Those
/// found are remembered in `vias`.
fn via(
    vias: &mut FxHashMap<Expansion, Expansion>,
    relocation: &Relocation,
    via: Expansion,
    old: &ExpansionInterner,
    new: &ExpansionInterner,
) -> Option<Expansion> {
    if via == Expansion::ROOT {
        return Some(Expansion::ROOT);
    }
    if let Some(&v) = vias.get(&via) {
        return Some(v);
    }
    let mut parent = Expansion::ROOT;
    for f in old.frames(via).iter().rev() {
        let id = new.find(f.instance, parent)?;
        let g = new.frame(id)?;
        if relocation.range(f.site) != Some(g.site) || g.round != f.round || g.iteration != f.iteration {
            return None;
        }
        parent = id;
    }
    vias.insert(via, parent);
    Some(parent)
}

/// The prefix pass run on a fork of `module`, its types carried: what the
/// module would be had it run the pass itself.
fn run(module: &Module, round: u32, interner: &Interner) -> Module {
    let mut probe = Module { ir: module.ir.fork(), types: module.types.clone() };
    if round == 0 {
        kg_hir::rewrite::pipeline_tail_in_place(&mut probe.ir, interner);
    } else {
        kg_hir::rewrite::round_prefix_in_place(&mut probe.ir, interner);
    }
    crate::elaborate::carry_types(&mut probe);
    probe
}

/// `probe` — the pass run ([`run`]) — against `module`, the pass taken in,
/// both from `from` nodes: `None` where they are the same — every node
/// appended, its children naming the same nodes as the same arenas mint
/// them, its source, kind and type — and otherwise what differs first.
fn check(probe: &Module, module: &Module, from: usize) -> Option<String> {
    let arena = &probe.ir.arena;
    if arena.len() != module.ir.arena.len() {
        return Some(format!("the pass appended {} nodes, the kept run {}", arena.len() - from, module.ir.arena.len() - from));
    }
    if probe.ir.root.index() != module.ir.root.index() {
        return Some(format!("rooted at {}, the kept run at {}", probe.ir.root.index(), module.ir.root.index()));
    }
    let mut ours = Vec::new();
    let mut theirs = Vec::new();
    for i in from..arena.len() {
        let node = module.ir.arena.node_at(i);
        let run = arena.node_at(i);
        let same_children = {
            ours.clear();
            theirs.clear();
            run.exp.children(&mut ours);
            node.exp.children(&mut theirs);
            // The fork's own nodes are the module's own; any other is the
            // same id in both.
            ours.len() == theirs.len()
                && ours.iter().zip(&theirs).all(|(&a, &b)| {
                    a.index() == b.index() && if arena.mints(a) { module.ir.arena.mints(b) } else { a == b }
                })
        };
        if !(same_children && run.exp.same_but_stamps(&node.exp) && run.src == node.src && run.kind == node.kind) {
            return Some(format!("node {i}: the pass made `{}`, the kept run `{}`", cut(&run.exp), cut(&node.exp)));
        }
    }
    for i in from..arena.len() {
        let (a, b) = (probe.types.entry(i), module.types.entry(i));
        let same = match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.cmp(b) == std::cmp::Ordering::Equal,
            _ => false,
        };
        if !same {
            return Some(format!("node {i}'s type: the pass gave `{a:?}`, the kept run `{b:?}`"));
        }
    }
    None
}

fn cut(exp: &Exp) -> String {
    format!("{exp:?}").chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use kg_base::FileId;
    use kg_syntax::{AstNode, ast};

    use super::*;
    use crate::elaborate::{elaborate_recorded, fingerprint, ROUND_GUARD};
    use crate::snapshot::{Recorder, Splice};

    fn lowered(text: &str, interner: &Interner) -> kg_hir::ModuleIr {
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        kg_hir::lower_sorted(&file, FileId(0), interner, &kg_hir::NoImports)
    }

    fn run(text: &str, interner: &Interner, reuse: Option<Reuse>) -> Elaborated {
        let never = kg_base::cancel::Cancel::never();
        let record = Recorder::new(vec!["main".into()]).reusing(reuse);
        elaborate_recorded(vec![lowered(text, interner)], interner, ROUND_GUARD, false, &never, record)
    }

    /// A template whose copies the prefix rewrites every round — its
    /// lambda, desugared inside each copy, under the copy's frames — and
    /// whose copies chain over four rounds.
    const DOWN: &str = "template <auto N>\ninline uint32 Down(uint32 a)\n{\n    auto add = [](uint32 x) -> uint32\n    {\n        return x + N;\n    };\n\
                        static if (N > 0)\n    {\n        return Down<N - 1>(add(a));\n    }\n    else\n    {\n        return add(a);\n    }\n}\n\n\
                        inline uint32 main(uint32 a)\n{\n    return Down<3>(a);\n}\n";

    /// Every prefix pass of a run, taken from the run before with its
    /// positions carried through the edit, is the pass the run would make
    /// — the check runs it on a fork and finds it so — and the run's output
    /// is a fresh run's. The one module stands in for a followed one (its
    /// own text moves here, which a database never follows). Not carried
    /// through the edit, the pass taken is not the run's, and the check
    /// says so. Red with a head not carried, an expansion or a child named
    /// otherwise than the run names it, or the types not taken in.
    #[test]
    fn a_kept_prefix_pass_is_the_pass_the_run_would_make() {
        let interner = Interner::new();
        let old = Arc::new(run(DOWN, &interner, None));
        let at = DOWN.find("    return Down<3>").expect("the request") + 2;
        let moved = format!("{}\t{}", &DOWN[..at], &DOWN[at..]);
        let mut relocation = Relocation::new();
        relocation.edit(FileId(0), Splice::between(DOWN, &moved).expect("an edit"));
        let reuse = Reuse::new(Arc::clone(&old), &[Some(0)], vec!["main".into()], relocation, true);
        let reused = run(&moved, &interner, reuse);
        let report = reused.snapshot.as_ref().and_then(|s| s.reused.clone()).expect("a report");
        assert_eq!(report.unequal, Vec::<String>::new());
        let rounds = old.snapshot.as_ref().expect("recorded").rounds() as u64;
        assert_eq!((report.offered, report.reused, report.checked), (rounds, rounds, rounds), "{report:?}");
        assert!(report.nodes > 0, "{report:?}");
        let fresh = run(&moved, &interner, None);
        assert!(fingerprint(&fresh, &interner) == fingerprint(&reused, &interner), "reuse changed the loop");
        let unmoved = Reuse::new(old, &[Some(0)], vec!["main".into()], Relocation::new(), true);
        let report = run(&moved, &interner, unmoved).snapshot.as_ref().and_then(|s| s.reused.clone()).expect("a report");
        assert!(report.reused > 0 && !report.unequal.is_empty(), "{report:?}");
    }
}
