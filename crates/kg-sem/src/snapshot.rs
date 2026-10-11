//! The loop's state per module and per round, kept, and the shadow that
//! checks it against the next run (sprint W7.5d, D1 step 1;
//! `docs/plans/loop-across-edits.md` §6). Nothing is reused here: the
//! shadow says whether a kept round, restored, would have been the round
//! the next run ran, and `crate::reuse` takes a kept round's prefix pass in
//! where every key before it agreed (step 3) — for which each round's
//! prefix boundary is kept too ([`Prefix`], the node log split there).
//!
//! **What is kept** ([`Rounds`], one per run, in [`Elaborated::snapshot`]):
//! the final program — every module's arena and types, and the expansions,
//! which the run's [`Elaborated`] holds anyway — and per module and round
//! the arena's length and root at the round's end ([`End`]), the round's
//! undo log ([`Undo`]: each node [`kg_hir::ExpArena::node_mut`] handed out
//! and each type a write changed, as they were before), and the round's
//! key ([`Key`]). The state after round *r* is the final arena cut to its
//! length then, with the later rounds' logs undone newest first: nothing is
//! copied per round. Round 0's log is not kept — no state before round 0's
//! end is ever restored.
//!
//! **The key** of a module's round is §3's input (2): the copies the census
//! decided into the module that round, each spelled run-independently as
//! 5d-0 spelled it ([`Speller::copies`]: template, key, stamps, and the
//! occurrence's module, position, expansion and argument subtree), hashed
//! with its positions held aside so that a later run's key is compared with
//! them carried through the edit ([`Relocation`]). A position inside the
//! changed text has no place, and its key fails. Beside it, per module and
//! round, the key of (3) — what the round read across the module boundary,
//! the read door's (step 2, [`crate::reads`]) — its positions held aside
//! alike ([`ModuleRounds::reads`]).
//!
//! **Relocation** carries a position of the old text into the new through
//! one [`Splice`] per edited file — `xtask::edits::Splice`'s rule, 5d-0's:
//! a range ending where text is inserted stays, one starting there moves,
//! an empty one moves as its start, one inside what was removed has no
//! place. It assumes the passes compare positions only for equality and
//! for order within a file, which a monotone map keeps. A guess at the
//! splice's edges costs reuse, never exactness ([`Splice::start`]).
//!
//! **The shadow** ([`Shadow`]): the next run of the same program, still
//! from nothing, compares at every round's end each module's state — every
//! node, its provenance and its type, its root and its length — with the
//! kept run's restored to the same round and relocated, for every module
//! whose input to the loop is the kept run's and every round through which
//! both its keys have agreed. What it reports ([`ShadowReport`]) is the
//! number to hold D1 to before any round is reused: the rounds whose keys
//! agreed, and of those how many restored states equal the actual; and
//! where the reads alone parted a module, whether its state there was the
//! kept one's all the same — what the door's caution costs.

use std::hash::Hasher;
use std::sync::Arc;

use kg_base::{Expansion, ExpansionInterner, FileId, FileRange, FxHashMap, Interner, TextRange};
use kg_hir::{Exp, ExpId, NodeData};
use kg_ty::TyRef;

use crate::census::{Args, Census};
use crate::elaborate::Elaborated;
use crate::infer::Module;

// =============================================================================
// Relocation
// =============================================================================

/// One splice of a text: `removed` bytes at `at` replaced by `inserted`
/// bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splice {
    pub at: usize,
    pub removed: usize,
    pub inserted: usize,
}

impl Splice {
    /// Where an offset of the old text that begins a range is in the new
    /// one — `None` inside what was removed. The byte at `at` moves with
    /// the insertion.
    ///
    /// At the splice's edges this is a guess: text inserted where a node
    /// begins may be the node's own — `/* c */ ` typed before `using C = …`
    /// is the declaration's, whose node now starts at the comment — and
    /// only the new parse knows. A module whose own text changed is never
    /// followed (`kg_db::snapshot`); another holds a position in an edited
    /// file through the copies decided into it, whose key spells every such
    /// position, so a guess the parse belies parts the key — or through a
    /// read across the module boundary, §3's (3): `resolvedFuncParams`
    /// copies a parameter list another module declares into the reader
    /// (`Function.hs:133-141`, `kg_hir::ExpArena::deep_copy_from`), which
    /// the shadow found parting a state whose key agreed. The read door
    /// keys such a read, its positions held aside as the copies' are
    /// ([`crate::reads`]).
    pub fn start(&self, offset: usize) -> Option<usize> {
        if offset < self.at { Some(offset) } else { self.moved(offset) }
    }

    /// [`Splice::start`] for an offset that ends a range: a range ending at
    /// `at` ends before the insertion and stays.
    pub fn end(&self, offset: usize) -> Option<usize> {
        if offset <= self.at { Some(offset) } else { self.moved(offset) }
    }

    fn moved(&self, offset: usize) -> Option<usize> {
        (offset >= self.at + self.removed).then(|| offset - self.removed + self.inserted)
    }

    /// A range of the old text in the new: an empty range moves as its
    /// start.
    pub fn range(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        if start == end { self.start(start).map(|s| (s, s)) } else { self.start(start).zip(self.end(end)) }
    }

    /// The one splice that takes `old` to `new`, `None` where they are
    /// equal: what lies between their common prefix and suffix. Where the
    /// two overlap — text inserted beside a copy of itself, `xy` to `xyy` —
    /// the edit could be at either place, and a position between them would
    /// map to one of two equal texts; so the splice covers both places, and
    /// what lies between has no place.
    pub fn between(old: &str, new: &str) -> Option<Splice> {
        if old == new {
            return None;
        }
        let (a, b) = (old.as_bytes(), new.as_bytes());
        let min = a.len().min(b.len());
        let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
        let suffix = a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count();
        // The edit with the most prefix, and the one with the most suffix.
        let tail = suffix.min(min - prefix.min(min));
        let head = prefix.min(min - suffix.min(min));
        Some(Splice { at: head, removed: a.len() - tail - head, inserted: b.len() - tail - head })
    }
}

/// Every edited file's [`Splice`]: positions in any other file stay.
#[derive(Clone, Debug, Default)]
pub struct Relocation {
    splices: FxHashMap<FileId, Splice>,
}

impl Relocation {
    pub fn new() -> Relocation {
        Relocation::default()
    }

    /// `file` was edited by `splice`.
    pub fn edit(&mut self, file: FileId, splice: Splice) {
        self.splices.insert(file, splice);
    }

    /// Whether no file was edited.
    pub fn is_empty(&self) -> bool {
        self.splices.is_empty()
    }

    /// `start..end` in `file`, carried through its edit.
    pub fn pos(&self, file: FileId, start: u32, end: u32) -> Option<(u32, u32)> {
        match self.splices.get(&file) {
            None => Some((start, end)),
            Some(s) => s.range(start as usize, end as usize).map(|(a, b)| (a as u32, b as u32)),
        }
    }

    /// [`Relocation::pos`] of a range.
    pub fn range(&self, r: FileRange) -> Option<FileRange> {
        let (a, b) = self.pos(r.file, r.range.start().into(), r.range.end().into())?;
        Some(FileRange::new(r.file, TextRange::new(a.into(), b.into())))
    }
}

// =============================================================================
// Spelling: 5d-0's, shared with `cargo xtask loop-rounds`
// =============================================================================

/// A position inside a spelled input: `OPEN file SEP start SEP end CLOSE`.
pub const OPEN: char = '\u{1}';
pub const SEP: char = '\u{1f}';
pub const CLOSE: char = '\u{2}';

/// Spells what a round's census hands `instantiate` and `resolve` with
/// nothing a run interns: an expansion as its frames, a file as its path
/// (where `files` names it) or `#n`, and — where it names files, for a
/// reader — a symbol as its text. Without paths, as a [`Key`] spells, a
/// symbol stays numbered: both runs intern into the database's one
/// interner, and resolving every symbol of every type was half the key's
/// cost (2026-10-10, `sample`).
#[derive(Debug)]
pub struct Speller<'a> {
    interner: &'a Interner,
    expansions: &'a ExpansionInterner,
    files: Option<&'a FxHashMap<FileId, String>>,
    /// Each expansion spelled so far: one is spelled for every node it
    /// holds.
    vias: std::cell::RefCell<FxHashMap<Expansion, std::rc::Rc<str>>>,
    /// Each type spelled so far, by value: an argument's nodes repeat a
    /// handful of types, and a type's `Debug` rendering was most of what
    /// the key cost once symbols stayed numbered.
    types: std::cell::RefCell<FxHashMap<Strict, std::rc::Rc<str>>>,
}

/// A type compared strictly — by `Ord`, a parameter's name included, which
/// `==` ignores (`Type.hs:126-129`) — and hashed as `Hash` hashes it, which
/// two strictly equal types agree on.
#[derive(Debug, Clone)]
struct Strict(TyRef);

impl PartialEq for Strict {
    fn eq(&self, other: &Strict) -> bool {
        std::ptr::eq(self.0.get(), other.0.get()) || self.0.cmp(&other.0) == std::cmp::Ordering::Equal
    }
}

impl Eq for Strict {}

impl std::hash::Hash for Strict {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<'a> Speller<'a> {
    pub fn new(interner: &'a Interner, expansions: &'a ExpansionInterner, files: Option<&'a FxHashMap<FileId, String>>) -> Speller<'a> {
        Speller { interner, expansions, files, vias: std::cell::RefCell::default(), types: std::cell::RefCell::default() }
    }

    fn ty(&self, ty: &TyRef) -> std::rc::Rc<str> {
        let key = Strict(ty.clone());
        if let Some(s) = self.types.borrow().get(&key) {
            return std::rc::Rc::clone(s);
        }
        let s: std::rc::Rc<str> = self.symbols(&format!("{ty:?}")).into();
        self.types.borrow_mut().insert(key, std::rc::Rc::clone(&s));
        s
    }

    /// Per module: the copies decided into it — template, argument key,
    /// stamps, and the occurrence the copy is made from — in the census's
    /// order. `names` names each module run-independently.
    pub fn copies(&self, census: &Census, modules: &[Module], names: &[String]) -> Vec<Vec<String>> {
        let mut copies = vec![Vec::new(); modules.len()];
        for d in &census.decided {
            let first = match &d.first {
                Args::Written(m, at) => {
                    let module = &modules[*m];
                    let node = module.ir.arena.node(*at);
                    let args = match &node.exp {
                        Exp::TemplateInstance { args, .. } => self.tree(module, *args),
                        _ => "no arguments".to_owned(),
                    };
                    // The site the copy's frames carry, where it is not the
                    // occurrence's own (its last name, past a qualifier).
                    let site = crate::elaborate::instance_site(&module.ir.arena, *at);
                    let site = if site == node.src.head { String::new() } else { format!(" site {}", self.at(site)) };
                    format!("written in {} at {}{site} via {}: {args}", names[*m], self.at(node.src.head), self.via(node.src.via))
                }
                Args::Minted(values, m, at) => {
                    let node = modules[*m].ir.arena.node(*at);
                    let values = self.symbols(&format!("{values:?}"));
                    format!("minted {values} in {} at {} via {}", names[*m], self.at(node.src.head), self.via(node.src.via))
                }
            };
            copies[d.home.module].push(format!("{} from {first}", self.head(d)));
        }
        copies
    }

    /// Per module: the occurrences the census met in it (what `resolve`
    /// renames there).
    pub fn mentions(&self, census: &Census, modules: &[Module]) -> Vec<Vec<String>> {
        let mut mentions = vec![Vec::new(); modules.len()];
        for d in &census.decided {
            let head = self.head(d);
            for &(m, at) in &d.sites {
                let node = modules[m].ir.arena.node(at);
                mentions[m].push(format!("{head} at {} via {}", self.at(node.src.head), self.via(node.src.via)));
            }
        }
        mentions
    }

    fn head(&self, d: &crate::census::Decided) -> String {
        let template: Vec<String> = d.template.iter().map(|t| self.interner.resolve(*t).to_string()).collect();
        format!("{} {} {}", template.join("::"), d.key, self.symbols(&format!("{:?}", d.stamps)))
    }

    /// A subtree: its structure, then each node's position, provenance,
    /// kind, expansion and type, in pre-order.
    pub fn tree(&self, module: &Module, id: ExpId) -> String {
        let arena = &module.ir.arena;
        let mut out = kg_hir::structure::render_exp(arena, self.interner, id);
        let mut stack = vec![id];
        let mut kids = Vec::new();
        while let Some(at) = stack.pop() {
            let node = arena.node(at);
            let ty = module.types.entry(at.index()).map_or_else(|| "-".into(), |t| self.ty(t));
            out.push_str(&format!(
                "\n{} {:?} {:?} via {}: {ty}",
                self.at(node.src.head),
                node.src.written,
                node.kind,
                self.via(node.src.via)
            ));
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().rev().copied());
        }
        out
    }

    pub fn at(&self, range: FileRange) -> String {
        let file = self.files.and_then(|f| f.get(&range.file).cloned()).unwrap_or_else(|| format!("#{}", range.file.0));
        position(&file, u32::from(range.range.start()) as usize, u32::from(range.range.end()) as usize)
    }

    /// An expansion as its frames, innermost first.
    pub fn via(&self, via: Expansion) -> std::rc::Rc<str> {
        if via == Expansion::ROOT {
            return "-".into();
        }
        if let Some(s) = self.vias.borrow().get(&via) {
            return std::rc::Rc::clone(s);
        }
        let frames: Vec<String> = self
            .expansions
            .frames(via)
            .iter()
            .map(|f| format!("{:016x}@{}r{}{}", f.instance.0, self.at(f.site), f.round, if f.iteration { "i" } else { "" }))
            .collect();
        let s: std::rc::Rc<str> = frames.join("<").into();
        self.vias.borrow_mut().insert(via, std::rc::Rc::clone(&s));
        s
    }

    /// Every `Symbol(N)` a `Debug` rendering holds, as its text — where
    /// this speller spells for a reader (see the type).
    pub fn symbols(&self, text: &str) -> String {
        if self.files.is_none() {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find("Symbol(") {
            out.push_str(&rest[..at]);
            let after = &rest[at + "Symbol(".len()..];
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            match after[..digits].parse::<usize>().ok().and_then(|n| self.interner.resolve_index(n)) {
                Some(s) if after[digits..].starts_with(')') => {
                    out.push_str(&format!("{s:?}"));
                    rest = &after[digits + 1..];
                }
                _ => {
                    out.push_str("Symbol(");
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// `module`'s state as it stands: every node of its arena, in order —
    /// an arena is per module, so two runs that did the same to it number
    /// it alike — with its provenance and type, hashed with the positions
    /// in the `movable` files held aside. Symbols are left numbered: both
    /// runs intern into one interner.
    pub fn final_state(&self, module: &Module, movable: &[String]) -> Final {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut positions = Vec::new();
        for (id, node) in module.ir.arena.iter() {
            let via = self.via(node.src.via);
            let ty = if module.types.is_set(id) { format!("{:?}", module.types.get(id)) } else { "-".to_owned() };
            let line = format!(
                "{} {:?} {:?} {} {via} {ty}",
                unstamped(&format!("{:?}", node.exp)),
                node.kind,
                node.src.written,
                self.at(node.src.head)
            );
            h.write(extract(&line, movable, &mut positions).as_bytes());
            h.write_u8(0);
        }
        Final { hash: h.finish(), positions }
    }
}

/// A module's state — [`Speller::final_state`] — as a hash of all but the
/// positions in the files an edit may move, and those positions in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Final {
    pub hash: u64,
    /// `(index into the movable files, start, end)`.
    pub positions: Vec<(u8, u32, u32)>,
}

pub fn position(file: &str, start: usize, end: usize) -> String {
    format!("{OPEN}{file}{SEP}{start}{SEP}{end}{CLOSE}")
}

/// `text` (a `Debug` rendering) without the arenas' stamps, which every
/// run mints anew: `ExpId { index: 7 }`.
pub fn unstamped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("stamp: ") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "stamp: ".len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        rest = after[digits..].strip_prefix(", ").unwrap_or(&after[digits..]);
    }
    out.push_str(rest);
    out
}

/// `text` with each position in a `movable` file taken out into
/// `positions`, a placeholder naming the file left in its place.
pub fn extract(text: &str, movable: &[String], positions: &mut Vec<(u8, u32, u32)>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(OPEN) {
        out.push_str(&rest[..open]);
        let inside = &rest[open + OPEN.len_utf8()..];
        let Some(close) = inside.find(CLOSE) else { break };
        let spelled = &inside[..close];
        rest = &inside[close + CLOSE.len_utf8()..];
        let mut parts = spelled.split(SEP);
        let (path, start, end) = (parts.next(), parts.next().and_then(|s| s.parse::<u32>().ok()), parts.next().and_then(|s| s.parse::<u32>().ok()));
        match (path.and_then(|p| movable.iter().position(|m| m == p)), start, end) {
            (Some(i), Some(s), Some(e)) => {
                let i = u8::try_from(i).expect("two movable files");
                positions.push((i, s, e));
                out.push_str(&format!("§{i}"));
            }
            _ => {
                out.push(OPEN);
                out.push_str(spelled);
                out.push(CLOSE);
            }
        }
    }
    out.push_str(rest);
    out
}

// =============================================================================
// The key
// =============================================================================

/// A module's input to one round, as [`Speller::copies`] spells it with
/// files as `#n`, hashed with every position held aside.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub hash: u64,
    pub positions: Vec<(FileId, u32, u32)>,
}

impl Key {
    /// The key of `text`, every `#n` position taken out.
    pub fn of(text: &str) -> Key {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut positions = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find(OPEN) {
            h.write(&rest.as_bytes()[..open]);
            let inside = &rest[open + OPEN.len_utf8()..];
            let Some(close) = inside.find(CLOSE) else { break };
            let spelled = &inside[..close];
            rest = &inside[close + CLOSE.len_utf8()..];
            let mut parts = spelled.split(SEP);
            let file = parts.next().and_then(|f| f.strip_prefix('#')).and_then(|n| n.parse::<u32>().ok());
            match (file, parts.next().and_then(|s| s.parse::<u32>().ok()), parts.next().and_then(|s| s.parse::<u32>().ok())) {
                (Some(f), Some(s), Some(e)) => {
                    positions.push((FileId(f), s, e));
                    h.write_u8(1);
                }
                _ => h.write(spelled.as_bytes()),
            }
        }
        h.write(rest.as_bytes());
        Key { hash: h.finish(), positions }
    }

    /// Whether `new`, a later run's key, is this one with its positions
    /// carried through `relocation`.
    pub fn agrees(&self, new: &Key, relocation: &Relocation) -> bool {
        self.hash == new.hash
            && self.positions.len() == new.positions.len()
            && self.positions.iter().zip(&new.positions).all(|(&(f, s, e), &(g, t, u))| f == g && relocation.pos(f, s, e) == Some((t, u)))
    }
}

// =============================================================================
// What a run keeps
// =============================================================================

/// One module at one round's end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct End {
    /// The arena's length.
    pub len: u32,
    /// The root's index.
    pub root: u32,
}

/// What one round wrote over in one module, as it was before, oldest
/// first: the nodes written from the round's start to its prefix pass's
/// end, of those that existed when the round began; the nodes written from
/// there to the round's end, of those that existed at the prefix's end
/// (round 0's log opens there); and the types written, of the nodes that
/// existed when the round began.
#[derive(Debug, Default)]
pub struct Undo {
    pub prefix_nodes: Vec<(u32, NodeData)>,
    pub nodes: Vec<(u32, NodeData)>,
    pub types: Vec<(u32, Option<TyRef>)>,
}

/// One module's prefix pass in one round — `kg_hir::rewrite`'s
/// `round_prefix_in_place`, round 0's `pipeline_tail_in_place` — which
/// reads its module alone and only appends (D1 step 3, `crate::reuse`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefix {
    /// The arena's length and root when it began, and when it ended.
    pub from: u32,
    pub root_from: u32,
    pub len: u32,
    pub root: u32,
    /// Whether it wrote a node in place, which none of its rewrites does:
    /// a pass that did is not taken for what it appended.
    pub wrote: bool,
}

/// One module's rounds (see the module comment).
#[derive(Debug, Default)]
pub struct ModuleRounds {
    pub ends: Vec<End>,
    pub keys: Vec<Key>,
    /// Per round, what the module read across its boundary
    /// ([`crate::reads`]): §3's (3), beside the copies' (2) in `keys`.
    pub reads: Vec<Key>,
    /// Per round; round 0's holds only what was written after its prefix.
    pub undo: Vec<Undo>,
    /// Per round, what the module read across its boundary before its
    /// prefix pass, the pass, and the `(new, original)` pairs that carried
    /// a type onto what it appended (`carry_types`): what a later run
    /// reuses the pass by (`crate::reuse`).
    pub reads_before_prefix: Vec<Key>,
    pub prefixes: Vec<Prefix>,
    pub prefix_lineage: Vec<Vec<(u32, u32)>>,
}

/// A module's state at one round's end, restored: its nodes in order,
/// each one's type, its root.
#[derive(Clone, Debug)]
pub struct Restored {
    pub nodes: Vec<NodeData>,
    pub types: Vec<Option<TyRef>>,
    pub root: u32,
}

impl ModuleRounds {
    /// The state after round `round` of `module`, whose final state the
    /// run left it in: the final arena cut to the round's length, and the
    /// later rounds' logs undone, newest first. (The shadow reads the same
    /// state a node at a time, [`Shadow`].)
    pub fn restore(&self, module: &Module, round: usize) -> Restored {
        let len = self.ends[round].len as usize;
        let mut nodes: Vec<NodeData> = (0..len).map(|i| module.ir.arena.node_at(i).clone()).collect();
        let mut types: Vec<Option<TyRef>> = (0..len).map(|i| module.types.entry(i).cloned()).collect();
        for undo in self.undo[round + 1..].iter().rev() {
            for (i, node) in undo.nodes.iter().rev().chain(undo.prefix_nodes.iter().rev()) {
                if let Some(slot) = nodes.get_mut(*i as usize) {
                    slot.clone_from(node);
                }
            }
            for (i, ty) in undo.types.iter().rev() {
                if let Some(slot) = types.get_mut(*i as usize) {
                    slot.clone_from(ty);
                }
            }
        }
        Restored { nodes, types, root: self.ends[round].root }
    }
}

/// A run's snapshot: per module in program order — and what the run
/// reused of the snapshot before it (`crate::reuse`).
#[derive(Debug, Default)]
pub struct Rounds {
    pub modules: Vec<ModuleRounds>,
    pub reused: Option<crate::reuse::ReuseReport>,
}

/// What a snapshot holds beyond the final program it is the history of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    /// Nodes in the final program.
    pub nodes: u64,
    /// Nodes and types in the undo logs.
    pub logged_nodes: u64,
    pub logged_types: u64,
    /// Positions held aside by the keys.
    pub key_positions: u64,
    /// Positions held aside by the reads' keys.
    pub read_positions: u64,
    /// The prefix passes' `(new, original)` pairs.
    pub prefix_lineage: u64,
    /// An estimate of the bytes the logs and keys hold, beyond the program.
    pub bytes: u64,
}

impl Rounds {
    /// The number of rounds: one per census.
    pub fn rounds(&self) -> usize {
        self.modules.first().map_or(0, |m| m.ends.len())
    }

    /// The snapshot's size, beside `final_nodes`, the program's.
    pub fn size(&self, final_nodes: u64) -> Size {
        let mut s = Size { nodes: final_nodes, ..Size::default() };
        for m in &self.modules {
            for u in &m.undo {
                s.logged_nodes += (u.nodes.len() + u.prefix_nodes.len()) as u64;
                s.logged_types += u.types.len() as u64;
            }
            for k in &m.keys {
                s.key_positions += k.positions.len() as u64;
            }
            for k in m.reads.iter().chain(&m.reads_before_prefix) {
                s.read_positions += k.positions.len() as u64;
            }
            for l in &m.prefix_lineage {
                s.prefix_lineage += l.len() as u64;
            }
            s.bytes += (m.prefixes.len() * std::mem::size_of::<Prefix>() + m.reads_before_prefix.len() * std::mem::size_of::<Key>()) as u64;
            s.bytes += (m.ends.len() * std::mem::size_of::<End>() + (m.keys.len() + m.reads.len()) * std::mem::size_of::<Key>()) as u64;
        }
        // A logged node is its data and its index; a logged type a pointer
        // and its index, the type itself shared with the map that held it.
        s.bytes += s.logged_nodes * (std::mem::size_of::<NodeData>() as u64 + 8)
            + s.logged_types * (std::mem::size_of::<Option<TyRef>>() as u64 + 8)
            + (s.key_positions + s.read_positions) * std::mem::size_of::<(FileId, u32, u32)>() as u64
            + s.prefix_lineage * std::mem::size_of::<(u32, u32)>() as u64;
        s
    }
}

/// Takes a run's snapshot as the loop runs (`crate::elaborate`), and shows
/// each round's end to an observer — [`Shadow`] — if one is given.
pub struct Recorder<'a> {
    names: Vec<String>,
    rounds: Rounds,
    /// Per module, the arena's writes at the last round's end.
    writes: Vec<u64>,
    observer: Option<&'a mut dyn FnMut(&Ended<'_>)>,
    /// Per module, this round's prefix pass as it began — its arena's
    /// length, root and writes — and the nodes the round wrote over before
    /// the pass ended, till the round's end.
    prefix: Vec<(u32, u32, u64)>,
    prefix_logs: Vec<Vec<(u32, NodeData)>>,
    prefix_lineage: Vec<Vec<(u32, u32)>>,
    /// The kept run this one reuses ([`crate::reuse`]), if any.
    reuse: Option<crate::reuse::Reuse>,
}

impl std::fmt::Debug for Recorder<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder").field("rounds", &self.rounds.rounds()).finish_non_exhaustive()
    }
}

/// One round's end as [`Recorder`] shows it.
#[derive(Debug)]
pub struct Ended<'a> {
    pub round: u32,
    /// Whether the loop stops here.
    pub last: bool,
    pub modules: &'a [Module],
    pub expansions: &'a ExpansionInterner,
    /// The run's snapshot so far, this round's ends, keys and logs included.
    pub rounds: &'a Rounds,
    /// Per module, the arena's writes at the previous round's end: what
    /// it wrote in place this round is `written_since` them.
    pub since: &'a [u64],
    /// Each module's name, as the recorder was given them.
    pub names: &'a [String],
}

impl<'a> Recorder<'a> {
    /// A recorder for modules named, run-independently, by `names`.
    pub fn new(names: Vec<String>) -> Recorder<'a> {
        Recorder {
            names,
            rounds: Rounds::default(),
            writes: Vec::new(),
            observer: None,
            prefix: Vec::new(),
            prefix_logs: Vec::new(),
            prefix_lineage: Vec::new(),
            reuse: None,
        }
    }

    /// This recorder, reusing what `reuse` holds of a kept run.
    pub fn reusing(mut self, reuse: Option<crate::reuse::Reuse>) -> Recorder<'a> {
        self.reuse = reuse;
        self
    }

    /// Round `round`'s prefix pass begins: each module as it stands, and
    /// what it read before — and which modules reuse the kept run's pass.
    pub(crate) fn prefix_begins(&mut self, round: u32, modules: &[Module], door: &crate::reads::Door) {
        if self.rounds.modules.len() != modules.len() {
            self.rounds.modules.resize_with(modules.len(), ModuleRounds::default);
        }
        self.prefix = modules.iter().map(|m| (m.ir.arena.len() as u32, m.ir.root.index() as u32, m.ir.arena.writes())).collect();
        self.prefix_lineage = vec![Vec::new(); modules.len()];
        for (r, key) in self.rounds.modules.iter_mut().zip(door.keys_so_far(modules.len())) {
            r.reads_before_prefix.push(key);
        }
        if let Some(reuse) = self.reuse.as_mut() {
            reuse.plan(round, modules, &self.rounds);
        }
    }

    /// Module `m`'s prefix pass in round `round` taken from the kept run,
    /// where the plan allows: `true` where it was.
    pub(crate) fn reuse_prefix(&mut self, m: usize, module: &mut Module, round: u32, interner: &Interner, expansions: &ExpansionInterner) -> bool {
        let Some(lineage) = self.reuse.as_mut().and_then(|reuse| reuse.prefix(m, module, round, interner, expansions)) else {
            return false;
        };
        if let Some(slot) = self.prefix_lineage.get_mut(m) {
            *slot = lineage;
        }
        true
    }

    /// Module `m`'s prefix pass carried its types by `lineage`.
    pub(crate) fn prefix_carried(&mut self, m: usize, lineage: &[(kg_hir::ExpId, kg_hir::ExpId)]) {
        if let Some(slot) = self.prefix_lineage.get_mut(m) {
            *slot = lineage.iter().map(|(n, o)| (n.index() as u32, o.index() as u32)).collect();
        }
    }

    /// Round `round`'s prefix pass ended: each module's pass, and the log
    /// of what the round wrote over so far — round 0 opens the node log
    /// here.
    pub(crate) fn prefix_ends(&mut self, round: u32, modules: &mut [Module]) {
        self.prefix_logs.resize_with(modules.len(), Vec::new);
        for (m, module) in modules.iter_mut().enumerate() {
            let (from, root_from, writes) = self.prefix.get(m).copied().unwrap_or_default();
            let len = module.ir.arena.len() as u32;
            let r = &mut self.rounds.modules[m];
            r.prefixes.push(Prefix { from, root_from, len, root: module.ir.root.index() as u32, wrote: module.ir.arena.writes() != writes });
            r.prefix_lineage.push(self.prefix_lineage.get_mut(m).map(std::mem::take).unwrap_or_default());
            if round == 0 {
                module.ir.arena.begin_undo();
            } else {
                self.prefix_logs[m] = module.ir.arena.take_undo();
            }
        }
    }

    /// [`Recorder::new`], showing each round's end to `observer`.
    pub fn observed(names: Vec<String>, observer: &'a mut dyn FnMut(&Ended<'_>)) -> Recorder<'a> {
        Recorder { observer: Some(observer), ..Recorder::new(names) }
    }

    /// The modules' names, as the recorder was given them — or, where it
    /// was given none for `modules` of them, by their place.
    pub fn names_for(&mut self, modules: usize) -> &[String] {
        if self.names.len() != modules {
            self.names = (0..modules).map(|m| format!("module {m}")).collect();
        }
        &self.names
    }

    /// Round `round`'s census: each module's key.
    pub(crate) fn census(&mut self, modules: &[Module], census: &Census, interner: &Interner, expansions: &ExpansionInterner) {
        if self.rounds.modules.len() != modules.len() {
            self.rounds.modules.resize_with(modules.len(), ModuleRounds::default);
        }
        if self.names.len() != modules.len() {
            self.names = (0..modules.len()).map(|m| format!("module {m}")).collect();
        }
        let started = std::time::Instant::now();
        let speller = Speller::new(interner, expansions, None);
        let copies = speller.copies(census, modules, &self.names);
        let mentions = speller.mentions(census, modules);
        for ((r, copies), mentions) in self.rounds.modules.iter_mut().zip(copies).zip(mentions) {
            let mut text = copies.join("\n");
            for m in mentions {
                text.push_str("\nmention ");
                text.push_str(&m);
            }
            r.keys.push(Key::of(&text));
        }
        crate::profile::whole("snapshot keys", started);
    }

    /// Round `round`'s end: each module's length and root, the log of
    /// what the round wrote over — round 0 opens the logs, the last closes
    /// them — and what it read across its boundary, `door`'s.
    pub(crate) fn end(
        &mut self,
        round: u32,
        last: bool,
        modules: &mut [Module],
        expansions: &ExpansionInterner,
        door: &crate::reads::Door,
    ) {
        let started = std::time::Instant::now();
        if self.rounds.modules.len() != modules.len() {
            self.rounds.modules.resize_with(modules.len(), ModuleRounds::default);
        }
        let reads = door.end_round(modules.len());
        self.prefix_logs.resize_with(modules.len(), Vec::new);
        for (((r, m), read), prefix_nodes) in
            self.rounds.modules.iter_mut().zip(modules.iter_mut()).zip(reads).zip(self.prefix_logs.iter_mut())
        {
            r.reads.push(read);
            let len = m.ir.arena.len();
            r.ends.push(End { len: len as u32, root: m.ir.root.index() as u32 });
            let prefix_nodes = std::mem::take(prefix_nodes);
            if round == 0 {
                m.types.begin_undo(len);
                r.undo.push(Undo { prefix_nodes, nodes: m.ir.arena.take_undo(), types: Vec::new() });
            } else {
                r.undo.push(Undo { prefix_nodes, nodes: m.ir.arena.take_undo(), types: m.types.take_undo(len) });
            }
            if last {
                m.ir.arena.end_undo();
                m.types.end_undo();
            }
        }
        if self.writes.len() != modules.len() {
            self.writes = vec![0; modules.len()];
        }
        crate::profile::whole("snapshot ends", started);
        if let Some(reuse) = self.reuse.as_mut() {
            reuse.round_ended(round, last, &self.rounds);
        }
        if let Some(observe) = self.observer.as_mut() {
            let names = &self.names;
            observe(&Ended { round, last, modules, expansions, rounds: &self.rounds, since: &self.writes, names });
        }
        self.writes = modules.iter().map(|m| m.ir.arena.writes()).collect();
    }

    pub fn finish(self) -> Rounds {
        let mut rounds = self.rounds;
        rounds.reused = self.reuse.map(crate::reuse::Reuse::finish);
        rounds
    }
}

// =============================================================================
// The shadow
// =============================================================================

/// What the shadow found over one run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShadowReport {
    /// Rounds — censuses — of the kept run and of this one.
    pub old_rounds: u32,
    pub new_rounds: u32,
    pub modules: usize,
    /// Modules whose input to the loop is the kept run's: the only ones a
    /// kept round could stand for.
    pub comparable: usize,
    /// Their rounds in this run.
    pub rounds: u64,
    /// Of those, the rounds through which the key agreed: each compared.
    pub agreed: u64,
    /// Of those, the rounds whose restored state equals the actual.
    pub equal: u64,
    /// Rounds at which the copies decided into the module agreed and what
    /// it read across its boundary did not: where the read door stopped
    /// following it — and of those, the rounds whose restored state was
    /// the actual all the same, compared though not followed: what the
    /// door's caution costs in reuse.
    pub read_parted: u64,
    pub read_parted_equal: u64,
    /// Comparable modules whose key agreed through every round of both
    /// runs, which ended at the same round.
    pub whole: usize,
    /// The first unequal round of each module that had one.
    pub mismatches: Vec<Mismatch>,
}

/// A module whose restored state is not the one this run reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// Its index in this run, and its name.
    pub module: usize,
    pub name: String,
    pub round: u32,
    pub what: String,
}

/// One comparable module as the shadow follows it.
#[derive(Clone, Copy, Debug, Default)]
struct Following {
    /// Its index in the kept run.
    old: usize,
    /// Whether its key has agreed through the last round compared.
    agreed: bool,
    /// Whether its state was equal at the last round compared.
    equal: bool,
    /// Whether it has been unequal at any round.
    parted: bool,
}

/// Where each round's undo log changed an index: `(round, position)` in
/// order, per index.
pub(crate) type History = FxHashMap<u32, Vec<(u32, u32)>>;

impl ModuleRounds {
    /// Where each log segment changed a node, in order, per index: round
    /// *r*'s nodes before its prefix's end as segment 2*r*, after it as
    /// 2*r* + 1 ([`ModuleRounds::logged_node`]).
    pub(crate) fn node_history(&self) -> History {
        let mut h = History::default();
        for (round, u) in self.undo.iter().enumerate() {
            for (seg, log) in [(2 * round, &u.prefix_nodes), (2 * round + 1, &u.nodes)] {
                for (pos, (i, _)) in log.iter().enumerate() {
                    h.entry(*i).or_default().push((seg as u32, pos as u32));
                }
            }
        }
        h
    }

    /// The node a [`ModuleRounds::node_history`] entry logged.
    pub(crate) fn logged_node(&self, seg: u32, pos: u32) -> &NodeData {
        let u = &self.undo[seg as usize / 2];
        let log = if seg.is_multiple_of(2) { &u.prefix_nodes } else { &u.nodes };
        &log[pos as usize].1
    }
}

/// The kept run's snapshot, checked round by round against the run in
/// hand ([`Recorder::observed`]; see the module comment).
pub struct Shadow {
    old: Arc<Elaborated>,
    relocation: Relocation,
    following: Vec<Option<Following>>,
    nodes: Vec<History>,
    types: Vec<History>,
    frames: Frames,
    report: ShadowReport,
    touched: Vec<u32>,
    seen: Vec<bool>,
}

impl std::fmt::Debug for Shadow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shadow").field("report", &self.report).finish_non_exhaustive()
    }
}

impl Shadow {
    /// The shadow of `old`, a run that kept its snapshot: `map` gives each
    /// module of the run in hand its module in `old` where its input to the
    /// loop is `old`'s, and `relocation` carries `old`'s positions into
    /// this run's texts. `None` where `old` kept none.
    pub fn new(old: Arc<Elaborated>, map: &[Option<usize>], relocation: Relocation) -> Option<Shadow> {
        let rounds = old.snapshot.as_ref()?;
        let history = |pick: &dyn Fn(&Undo) -> Vec<u32>| -> Vec<History> {
            rounds
                .modules
                .iter()
                .map(|m| {
                    let mut h = History::default();
                    for (round, u) in m.undo.iter().enumerate() {
                        for (pos, i) in pick(u).into_iter().enumerate() {
                            h.entry(i).or_default().push((round as u32, pos as u32));
                        }
                    }
                    h
                })
                .collect()
        };
        let nodes = rounds.modules.iter().map(ModuleRounds::node_history).collect();
        let types = history(&|u| u.types.iter().map(|(i, _)| *i).collect());
        let following: Vec<Option<Following>> = map
            .iter()
            .map(|o| o.filter(|&o| o < rounds.modules.len()).map(|old| Following { old, agreed: true, equal: true, parted: false }))
            .collect();
        let report = ShadowReport {
            old_rounds: rounds.rounds() as u32,
            modules: map.len(),
            comparable: following.iter().flatten().count(),
            ..ShadowReport::default()
        };
        Some(Shadow { old, relocation, following, nodes, types, frames: Frames::default(), report, touched: Vec::new(), seen: Vec::new() })
    }

    /// Compare round `e.round`'s end with the kept run's.
    pub fn observe(&mut self, e: &Ended<'_>) {
        let r = e.round as usize;
        let old = Arc::clone(&self.old);
        let kept = old.snapshot.as_ref().expect("checked in `new`");
        let old_count = kept.rounds();
        for m in 0..self.following.len().min(e.modules.len()) {
            let Some(mut f) = self.following[m] else { continue };
            self.report.rounds += 1;
            if !f.agreed {
                continue;
            }
            let o = f.old;
            let copies = r < old_count
                && (r + 1 == old_count) == e.last
                && kept.modules[o].keys[r].agrees(&e.rounds.modules[m].keys[r], &self.relocation);
            let read = copies && kept.modules[o].reads[r].agrees(&e.rounds.modules[m].reads[r], &self.relocation);
            if copies && !read {
                self.report.read_parted += 1;
                let full = r == 0 || !f.equal || e.last;
                if self.compare(&old, kept, o, e, m, r, full).is_none() {
                    self.report.read_parted_equal += 1;
                }
            }
            if !(copies && read) {
                f.agreed = false;
                self.following[m] = Some(f);
                continue;
            }
            self.report.agreed += 1;
            // Round 0, a round after an unequal one, and the end of both
            // runs are compared whole: the last with no log on either side.
            let full = r == 0 || !f.equal || e.last;
            let difference = self.compare(&old, kept, o, e, m, r, full);
            f.equal = difference.is_none();
            match difference {
                None => self.report.equal += 1,
                Some(what) => {
                    if !f.parted {
                        let name = e.names.get(m).cloned().unwrap_or_default();
                        self.report.mismatches.push(Mismatch { module: m, name, round: e.round, what });
                    }
                    f.parted = true;
                }
            }
            if e.last && f.agreed && r + 1 == old_count {
                self.report.whole += 1;
            }
            self.following[m] = Some(f);
        }
        if e.last {
            self.report.new_rounds = e.round + 1;
        }
    }

    pub fn finish(self) -> ShadowReport {
        self.report
    }

    /// Module `m` of the run in hand against module `o` of the kept run,
    /// restored to round `r`: `None` where they are the same.
    #[allow(clippy::too_many_arguments)]
    fn compare(&mut self, old: &Elaborated, kept: &Rounds, o: usize, e: &Ended<'_>, m: usize, r: usize, full: bool) -> Option<String> {
        let new = &e.modules[m];
        let end = kept.modules[o].ends[r];
        let shape = if new.ir.arena.len() != end.len as usize {
            Some(format!("{} nodes against the kept {}", new.ir.arena.len(), end.len))
        } else if new.ir.root.index() != end.root as usize {
            Some(format!("rooted at {} against the kept {}", new.ir.root.index(), end.root))
        } else {
            None
        };
        // Unequal in shape: the first node that differs too, to say why.
        let full = full || shape.is_some();
        let len = (end.len as usize).min(new.ir.arena.len());
        let parted = |what: String| Some(shape.as_ref().map_or(what.clone(), |s| format!("{s}; {what}")));
        self.touched.clear();
        if full {
            self.touched.extend(0..len as u32);
        } else {
            // What either run changed this round: the state before it was
            // equal (the last round compared), so nothing else can differ.
            let prior = kept.modules[o].ends[r - 1].len;
            self.seen.clear();
            self.seen.resize(len, false);
            let mut add = |i: u32, touched: &mut Vec<u32>| {
                if (i as usize) < len && !std::mem::replace(&mut self.seen[i as usize], true) {
                    touched.push(i);
                }
            };
            let (oldlog, newlog) = (&kept.modules[o].undo[r], &e.rounds.modules[m].undo[r]);
            for &i in new.ir.arena.written_since(e.since[m]) {
                add(i, &mut self.touched);
            }
            for &(i, _) in newlog.types.iter().chain(&oldlog.types) {
                add(i, &mut self.touched);
            }
            for &(i, _) in oldlog.nodes.iter().chain(&oldlog.prefix_nodes) {
                add(i, &mut self.touched);
            }
            for i in prior..len as u32 {
                add(i, &mut self.touched);
            }
        }
        let om = &old.modules[o];
        for k in 0..self.touched.len() {
            let i = self.touched[k];
            let before = self.node_at(kept, om, o, i, r);
            let now = new.ir.arena.node_at(i as usize);
            let same_node = before.kind == now.kind
                && before.src.written == now.src.written
                && self.relocation.range(before.src.head) == Some(now.src.head)
                && self.frames.of_kept(before.src.via, &old.expansions, &self.relocation) == self.frames.of_run(now.src.via, e.expansions)
                && before.exp.same_but_stamps(&now.exp);
            if !same_node {
                return parted(format!("round {r}, node {i}: kept `{}` against `{}`", cut(before), cut(now)));
            }
            let (t0, t1) = (self.type_at(kept, om, o, i, r), new.types.entry(i as usize));
            let same_type = match (t0, t1) {
                (None, None) => true,
                (Some(a), Some(b)) => a.cmp(b) == std::cmp::Ordering::Equal,
                _ => false,
            };
            if !same_type {
                return parted(format!("round {r}, node {i}'s type: kept `{t0:?}` against `{t1:?}` at `{}`", cut(now)));
            }
        }
        shape
    }

    /// Node `i` of the kept module `o` as it stood at round `r`'s end: as a
    /// later round's log found it before writing it, or as the run left it.
    fn node_at<'b>(&self, kept: &'b Rounds, om: &'b Module, o: usize, i: u32, r: usize) -> &'b NodeData {
        // The first segment after round `r`'s end that logged it.
        match self.nodes[o].get(&i).and_then(|h| h.iter().find(|&&(seg, _)| seg as usize > 2 * r + 1)) {
            Some(&(seg, pos)) => kept.modules[o].logged_node(seg, pos),
            None => om.ir.arena.node_at(i as usize),
        }
    }

    /// [`Shadow::node_at`] for the node's type.
    fn type_at<'b>(&self, kept: &'b Rounds, om: &'b Module, o: usize, i: u32, r: usize) -> Option<&'b TyRef> {
        match self.types[o].get(&i).and_then(|h| h.iter().find(|&&(round, _)| round as usize > r)) {
            Some(&(round, pos)) => kept.modules[o].undo[round as usize].types[pos as usize].1.as_ref(),
            None => om.types.entry(i as usize),
        }
    }
}

fn cut(node: &NodeData) -> String {
    format!("{:?} {:?} {:?}", node.exp, node.src, node.kind).chars().take(200).collect()
}

/// One expansion frame, run-independently: its instance, its site — `None`
/// where relocation gave it no place — its round, and whether it is a
/// `static for` iteration.
type Frame = (u64, Option<(u32, u32, u32)>, u32, bool);

/// Expansions of two runs named alike where their frames are alike.
#[derive(Default)]
struct Frames {
    table: FxHashMap<Vec<Frame>, u32>,
    old: FxHashMap<Expansion, u32>,
    new: FxHashMap<Expansion, u32>,
}

impl Frames {
    fn name(table: &mut FxHashMap<Vec<Frame>, u32>, frames: Vec<Frame>) -> u32 {
        let next = table.len() as u32 + 1;
        *table.entry(frames).or_insert(next)
    }

    fn spell(via: Expansion, expansions: &ExpansionInterner, site: impl Fn(FileRange) -> Option<FileRange>) -> Vec<Frame> {
        expansions
            .frames(via)
            .iter()
            .map(|f| {
                let at = site(f.site).map(|s| (s.file.0, u32::from(s.range.start()), u32::from(s.range.end())));
                (f.instance.0, at, f.round, f.iteration)
            })
            .collect()
    }

    fn of_kept(&mut self, via: Expansion, expansions: &ExpansionInterner, relocation: &Relocation) -> u32 {
        if via == Expansion::ROOT {
            return 0;
        }
        if let Some(&n) = self.old.get(&via) {
            return n;
        }
        let n = Frames::name(&mut self.table, Frames::spell(via, expansions, |s| relocation.range(s)));
        self.old.insert(via, n);
        n
    }

    fn of_run(&mut self, via: Expansion, expansions: &ExpansionInterner) -> u32 {
        if via == Expansion::ROOT {
            return 0;
        }
        if let Some(&n) = self.new.get(&via) {
            return n;
        }
        let n = Frames::name(&mut self.table, Frames::spell(via, expansions, Some));
        self.new.insert(via, n);
        n
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    use super::*;
    use crate::elaborate::{elaborate_recorded, ROUND_GUARD};

    fn lowered(text: &str, interner: &Interner) -> kg_hir::ModuleIr {
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        kg_hir::lower_sorted(&file, FileId(0), interner, &kg_hir::NoImports)
    }

    fn recorded(text: &str, interner: &Interner) -> Elaborated {
        let never = kg_base::cancel::Cancel::never();
        elaborate_recorded(vec![lowered(text, interner)], interner, ROUND_GUARD, false, &never, Recorder::new(vec!["main".into()]))
    }

    /// `text`'s loop in the shadow of `old`'s, its one module taken for
    /// `old`'s and its positions carried through `relocation`.
    fn shadowed(old: Elaborated, text: &str, interner: &Interner, relocation: Relocation) -> (Elaborated, ShadowReport) {
        let never = kg_base::cancel::Cancel::never();
        let mut shadow = Shadow::new(Arc::new(old), &[Some(0)], relocation).expect("a snapshot");
        let mut observe = |e: &Ended<'_>| shadow.observe(e);
        let out = elaborate_recorded(
            vec![lowered(text, interner)],
            interner,
            ROUND_GUARD,
            false,
            &never,
            Recorder::observed(vec!["main".into()], &mut observe),
        );
        (out, shadow.finish())
    }

    /// Every state the loop passes through at a round's end, as the run
    /// holds it: per round, its nodes, their types, its root.
    fn states(text: &str, interner: &Interner) -> (Elaborated, Vec<Restored>) {
        let never = kg_base::cancel::Cancel::never();
        let mut seen = Vec::new();
        let mut observe = |e: &Ended<'_>| {
            let m = &e.modules[0];
            seen.push(Restored {
                nodes: (0..m.ir.arena.len()).map(|i| m.ir.arena.node_at(i).clone()).collect(),
                types: (0..m.ir.arena.len()).map(|i| m.types.entry(i).cloned()).collect(),
                root: m.ir.root.index() as u32,
            });
        };
        let out = elaborate_recorded(
            vec![lowered(text, interner)],
            interner,
            ROUND_GUARD,
            false,
            &never,
            Recorder::observed(vec!["main".into()], &mut observe),
        );
        (out, seen)
    }

    fn same(a: &Restored, b: &Restored) -> bool {
        a.root == b.root
            && a.nodes.len() == b.nodes.len()
            && a.nodes.iter().zip(&b.nodes).all(|(x, y)| x.exp.same_but_stamps(&y.exp) && x.src == y.src && x.kind == y.kind)
            && a.types.len() == b.types.len()
            && a.types.iter().zip(&b.types).all(|(x, y)| match (x, y) {
                (None, None) => true,
                (Some(x), Some(y)) => x.cmp(y) == std::cmp::Ordering::Equal,
                _ => false,
            })
    }

    /// `Box<3>` is made in round 0, `B`'s `Box<4>` once `bitsizeof(C)`
    /// folds, in round 1 — and with it `w`'s initializer, a node the
    /// lowering made, folds in place; `Down<4>` down to `Down<0>` takes
    /// rounds 0 to 4, each copy typing the call in the one before.
    const DOWN: &str = "template <auto N>\nstruct Box\n{\n    uint<N> x;\n}\n\n\
                        template <auto N>\ninline uint32 Down(uint32 a)\n{\n    Box<N + 1> b;\n    static if (N > 0)\n    {\n        return Down<N - 1>(a) + b.x;\n    }\n    else\n    {\n        return a;\n    }\n}\n\n\
                        using C = Box<3>;\n\nusing B = Box<bitsizeof(C) + 1>;\n\ninline uint32 main(uint32 a)\n{\n    const auto w = bitsizeof(B);\n    return Down<4>(a) + w;\n}\n";

    /// The state after each round, restored from the final one and the
    /// logs, is the state the run held there — over a node written in place
    /// after round 0 and types rewritten every round — and recording leaves
    /// the loop's output as it was. Red with a write left out of either log,
    /// a round's log undone that should not be (one round early), or the
    /// arena not cut to the round's length.
    #[test]
    fn each_round_restored_is_the_round_the_run_held() {
        let interner = Interner::new();
        let (out, seen) = states(DOWN, &interner);
        let kept = out.snapshot.as_deref().expect("recorded");
        assert_eq!(kept.rounds(), seen.len());
        assert_eq!(kept.rounds(), out.report.rounds as usize + 1, "one round per census");
        let size = kept.size(0);
        assert!(size.logged_nodes > 0 && size.logged_types > 0, "the logs are exercised: {size:?}");
        for (r, state) in seen.iter().enumerate() {
            let restored = kept.modules[0].restore(&out.modules[0], r);
            assert!(same(&restored, state), "round {r} restored is not the round the run held");
        }
        let plain = crate::elaborate::elaborate(vec![lowered(DOWN, &interner)], &interner);
        let fingerprint = |e: &Elaborated| crate::elaborate::fingerprint(e, &interner);
        assert!(fingerprint(&plain) == fingerprint(&out), "recording changed the loop");
    }

    /// The same program run again in the shadow of the first: every round's
    /// key agrees and every round restored, a node at a time, is the round
    /// run — and the end of both is compared whole. Red with the shadow's
    /// own restoring a round early or a log left out.
    #[test]
    fn the_same_program_again_agrees_in_every_round() {
        let interner = Interner::new();
        let (_, report) = shadowed(recorded(DOWN, &interner), DOWN, &interner, Relocation::new());
        assert_eq!(report.mismatches, []);
        assert_eq!((report.old_rounds, report.new_rounds), (7, 7));
        assert_eq!((report.rounds, report.agreed, report.equal, report.whole), (7, 7, 7, 1), "{report:?}");
    }

    /// A tab typed into `main`'s indentation moves every position after it
    /// — `Down<4>`'s site among them, and so the copies' frames: carried
    /// through the edit, every key agrees and every state restored is the
    /// state run; not carried, round 0's key fails and nothing is compared.
    /// The request respelled, `Down<4 + 0>`, parts the key. Red with
    /// relocation skipped in the key or in the state (a node's head, an
    /// expansion frame's site).
    #[test]
    fn a_kept_round_is_compared_with_its_positions_carried_through_the_edit() {
        let interner = Interner::new();
        let at = DOWN.find("    return Down").expect("the request") + 2;
        let moved = format!("{}\t{}", &DOWN[..at], &DOWN[at..]);
        let splice = Splice::between(DOWN, &moved).expect("an edit");
        assert_eq!(splice, Splice { at, removed: 0, inserted: 1 });
        let mut relocation = Relocation::new();
        relocation.edit(FileId(0), splice);
        let (_, carried) = shadowed(recorded(DOWN, &interner), &moved, &interner, relocation);
        assert_eq!(carried.mismatches, []);
        assert_eq!((carried.agreed, carried.equal, carried.whole), (7, 7, 1), "{carried:?}");
        let (_, unmoved) = shadowed(recorded(DOWN, &interner), &moved, &interner, Relocation::new());
        assert_eq!((unmoved.agreed, unmoved.whole), (0, 0), "{unmoved:?}");
        let at = DOWN.find("Down<4>").expect("the request") + "Down<4".len();
        let inside = format!("{} + 0{}", &DOWN[..at], &DOWN[at..]);
        let mut relocation = Relocation::new();
        relocation.edit(FileId(0), Splice::between(DOWN, &inside).expect("an edit"));
        let (_, parted) = shadowed(recorded(DOWN, &interner), &inside, &interner, relocation);
        assert_eq!((parted.agreed, parted.whole), (0, 0), "{parted:?}");
    }

    /// One splice between two texts: what lies between their common prefix
    /// and suffix — and where an insertion repeats the text beside it, both
    /// places it could have been made, with what lies between left no
    /// place. Red with the overlap taken from one side only.
    #[test]
    fn the_splice_between_two_texts_covers_every_place_the_edit_could_be() {
        assert_eq!(Splice::between("abc", "abc"), None);
        assert_eq!(Splice::between("ab{}cd", "ab{ x; }cd"), Some(Splice { at: 3, removed: 0, inserted: 4 }));
        assert_eq!(Splice::between("import a\nf", "\nf"), Some(Splice { at: 0, removed: 8, inserted: 0 }));
        // `xy` to `xyy`: the `y` inserted at 1 or at 2.
        let s = Splice::between("xy", "xyy").expect("an edit");
        assert_eq!(s, Splice { at: 1, removed: 1, inserted: 2 });
        assert_eq!(s.range(1, 2), None, "the old `y` is either new one");
        assert_eq!(s.range(0, 1), Some((0, 1)));
        assert_eq!(s.range(2, 2), Some((3, 3)));
        // `(size)` to `(size, e)`: the `e` repeats.
        let s = Splice::between("f(size) {}", "f(size, e) {}").expect("an edit");
        assert_eq!(s, Splice { at: 5, removed: 1, inserted: 4 });
        assert_eq!(s.range(5, 6), None, "the old `e` is either new one");
        assert_eq!(s.range(2, 5), Some((2, 5)));
        assert_eq!(s.range(6, 7), Some((9, 10)));
        // A deletion beside its own copy.
        assert_eq!(Splice::between("yy", "y"), Some(Splice { at: 0, removed: 2, inserted: 1 }));
    }

    /// A key holds its positions aside: the same text with its positions
    /// moved agrees once they are carried through the edit, not before; a
    /// position the edit removed agrees with nothing.
    #[test]
    fn a_key_agrees_with_its_positions_carried() {
        let at = |s: usize, e: usize| position("#3", s, e);
        let old = Key::of(&format!("Box 8 from written in main at {} via -", at(10, 16)));
        let new = Key::of(&format!("Box 8 from written in main at {} via -", at(14, 20)));
        assert_ne!(old, new);
        assert_eq!(old.hash, new.hash);
        let mut moved = Relocation::new();
        moved.edit(FileId(3), Splice { at: 2, removed: 0, inserted: 4 });
        assert!(old.agrees(&new, &moved));
        assert!(!old.agrees(&new, &Relocation::new()));
        let mut elsewhere = Relocation::new();
        elsewhere.edit(FileId(4), Splice { at: 2, removed: 0, inserted: 4 });
        assert!(!old.agrees(&new, &elsewhere));
        let mut removed = Relocation::new();
        removed.edit(FileId(3), Splice { at: 12, removed: 2, inserted: 6 });
        assert!(!old.agrees(&new, &removed));
        let other = Key::of(&format!("Box 9 from written in main at {} via -", at(14, 20)));
        assert!(!old.agrees(&other, &moved));
    }
}

