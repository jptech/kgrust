//! The loop's read door (sprint W7.5d, D1 step 2;
//! `docs/plans/loop-across-edits.md` §6): every read a round makes across a
//! module boundary, recorded for the module that reads — §3's input (3),
//! which joins the copies decided into the module, (2)
//! ([`crate::snapshot::Key`]), in what its round is keyed by.
//!
//! **What crosses the boundary**, pass by pass — each through [`Door`]:
//! - a lookup in the symbol map, which every module files into (inference,
//!   `addLambdaWrappers`, `deduceTemplateArgs`, pass 1's
//!   `redundantNameScope` and `resolvedFuncParams`): the question and its
//!   answer ([`Door::lookup`]);
//! - a node of another module read, its type or its source — the
//!   declaration a lookup found, and what a pass copies of it into the
//!   reader (inference's `import`, a wrapper's parameters, a template's
//!   default, an alias's function type; [`Door::node`], [`Door::ty`],
//!   [`Door::src`], [`Door::subtree`]);
//! - a fact the whole program decides: whether any module declares an alias
//!   a parameter can resolve through (pass 2), which classes are `extern`
//!   (pass 7), and each match `resolve` made against the round's decisions
//!   in every module ([`Door::fact`]).
//!
//! A read of the reader's own module is not recorded: its state at the
//! round's start is the kept one's wherever its rounds are followed, and
//! what it does to itself follows from that and from what it read.
//! A declaration a pass reads whole — a template's signature, for a
//! deduction — is one item, spelled once for every reader and every pass
//! until its module is written again ([`Door::declaration`]).
//!
//! **Outcomes, where a rewrite reads.** A deduction and a wrapper are tried
//! with what they read held ([`Door::tentative`]), and keep it only where
//! they changed the module ([`Door::settle`]); `resolve` keeps only the
//! matches it makes. What a rewrite read and left the module alike after
//! moved nothing, and another run that left it alike ran the same whatever
//! it read: keeping it parted a module wherever a later round re-read a
//! template for a call already rewritten, or asked of a decision no module
//! made any more (`docs/plans/loop-across-edits.md` §6, step 2).
//!
//! **An item** is one read with its answer, spelled run-independently: a
//! module by its name, a node by where it was written — its module, head,
//! expansion frames, syntax kind and provenance — its expression without
//! its children's ids and with each child's identity instead, a type by its
//! strict hash (a parameter's name included, [`kg_ty::hash_strict`]), a
//! symbol by its number (both runs intern into one interner). Every
//! position is held aside, as the copies' are in their key, so that a
//! later run's reads compare with the positions carried through the edit.
//! Within an inference iteration a module reads the others as the
//! iteration found them, so an item is the same at every read of it there;
//! a read whose answer changes from one iteration to the next is two items.
//! Two nodes can be written alike — a rewrite's donations share their
//! donor's head and kind — and are then one identity: what tells them
//! apart to a reader is their expressions, which items hold.
//!
//! **The key** of a module's round is the *set* of its items, each tagged
//! with its pass and the pass's place in the round, hashed in order with
//! each position named by its rank among the round's distinct positions,
//! and those positions, in order ([`Door::end_round`]). Relocation keeps
//! order within a file and leaves other files alone, so a later run whose
//! items are these, each position carried through the edit, has the same
//! ranks and the same hash; one whose positions collapse or have no place
//! disagrees. A set, not a sequence: a module read again in an iteration
//! another module needed — the same answers — reads nothing new.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use kg_base::{Expansion, ExpansionInterner, FileId, FileRange, FxHashMap, Src, Symbol};
use kg_hir::{Exp, ExpArena, ExpId};
use kg_syntax::SyntaxKind;
use kg_ty::TyRef;

use crate::snapshot::Key;
use crate::symbols::Decl;
use crate::types::TypeMap;

/// A position held aside: a file and a range in it.
pub type Pos = (FileId, u32, u32);

/// Which pass reads.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pass {
    /// Pass 1's `redundantNameScope`, before the loop.
    NameScope,
    /// Passes 2 and 3 (`higherOrderFunctions`' `resolvedFuncParams`).
    Heads,
    /// `deduceTemplateArgs`.
    Deduce,
    /// `inferType`.
    Infer,
    /// `trimExternClasses`.
    Trim,
    /// `addLambdaWrappers`.
    Wrappers,
    /// `resolve`'s spread over the round's decisions, in `instantiate`.
    Resolve,
}

/// A lookup in the symbol map.
#[derive(Copy, Clone, Debug)]
pub enum Asked<'q> {
    /// `lookupSymbolByName`.
    ByName(&'q [Symbol]),
    /// `lookupInScope scope name`.
    InScope(&'q [Symbol], &'q [Symbol]),
}

/// One run's recorder of what each module read across the boundary (see
/// the module comment). Shared by the passes as `&Door`; a run that does
/// not record has none, and reads nothing into it.
pub struct Door {
    /// Each module's name, hashed.
    names: Vec<u64>,
    expansions: Arc<ExpansionInterner>,
    state: RefCell<State>,
}

impl std::fmt::Debug for Door {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Door").field("modules", &self.names.len()).finish_non_exhaustive()
    }
}

#[derive(Default)]
struct State {
    /// The pass reading, and its place in the round, and the two hashed:
    /// what every item begins with.
    pass: (Option<Pass>, u32),
    seed: u64,
    /// Per reading module, this round's items.
    modules: Vec<Items>,
    /// Each node's identity, numbered, by module and index: a node's
    /// source and kind never change in place, and the arenas only grow.
    numbered: FxHashMap<(u32, u32), u32>,
    idents: Vec<Ident>,
    /// Each expansion's frames, hashed, their sites held aside.
    vias: FxHashMap<Expansion, Ident>,
    /// The identities' and the frames' positions.
    pool: Vec<Pos>,
    /// Each type's strict hash, by its allocation, the type held so that
    /// the allocation is not another type's later.
    types: FxHashMap<usize, (TyRef, u64)>,
    /// The identities the item being spelled names, in order, and a node's
    /// children.
    scratch: Vec<u32>,
    kids: Vec<ExpId>,
    /// Where items go instead of a reader's while a declaration is spelled
    /// ([`Door::declaration`]) or a rewrite is tried ([`Door::tentative`]):
    /// one buffer, and where each hold under way began in it, innermost
    /// last — a hold settled is cut off the buffer's end, or left to the
    /// hold around it. And each declaration spelled so far, by its module
    /// and node.
    held: Spelled,
    holds: Vec<(u32, u32)>,
    declarations: FxHashMap<(u32, u32), Declaration>,
    /// [`Items::key`]'s scratch, by identity: each one's ranks hashed, and
    /// the key it was last named in.
    rank: Vec<u64>,
    mark: Vec<u32>,
    epoch: u32,
}

/// A declaration spelled ([`Door::declaration`]): the version of its
/// module it was spelled at, its one item's hash, and the identities that
/// item names.
struct Declaration {
    version: (u64, u64),
    hash: u64,
    names: Vec<u32>,
}

/// Items held ([`State::held`]).
#[derive(Debug, Default)]
struct Spelled {
    items: Vec<(u64, u32, u32)>,
    names: Vec<u32>,
}

/// A module's round's items, as spelled: one spelled twice is one in the
/// key ([`Items::key`]), and set aside here costs less than looked up.
#[derive(Default)]
struct Items {
    /// `(the item's hash, the start and length of what it names in
    /// names)`.
    items: Vec<(u64, u32, u32)>,
    names: Vec<u32>,
}

/// Where a node was written, or an expansion's frames: the rest hashed,
/// its positions in the pool.
#[derive(Copy, Clone)]
struct Ident {
    hash: u64,
    start: u32,
    len: u32,
}

fn pos(r: FileRange) -> Pos {
    (r.file, u32::from(r.range.start()), u32::from(r.range.end()))
}

fn hash_str(s: &str) -> u64 {
    let mut h = Mix::new();
    s.hash(&mut h);
    h.finish()
}

/// The door's hasher: each word folded into the state by a full 64×64
/// multiplication whose halves are xored (the mixing of foldhash and
/// wyhash), the seed fixed — run-independent, as a key must be — and a
/// last fold to finish. A key compares hashes of what two runs read, so it
/// needs collisions as rare as chance, which this gives on data no one
/// chose to collide — SipHash's resistance to chosen collisions is not
/// needed, and costs several times as much a word.
#[derive(Clone, Copy)]
struct Mix(u64);

const SEED: u64 = 0x243f_6a88_85a3_08d3;
const ARM: u64 = 0x1319_8a2e_0370_7344;
const END: u64 = 0xa409_3822_299f_31d0;

fn fold(a: u64, b: u64) -> u64 {
    let p = u128::from(a) * u128::from(b);
    (p as u64) ^ ((p >> 64) as u64)
}

impl Mix {
    fn new() -> Mix {
        Mix(SEED)
    }
}

impl Hasher for Mix {
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.write_u64(u64::from_le_bytes(c.try_into().expect("eight bytes")));
        }
        let rest = chunks.remainder();
        let mut last = [0u8; 8];
        last[..rest.len()].copy_from_slice(rest);
        self.write_u64(u64::from_le_bytes(last) ^ ((bytes.len() as u64) << 56));
    }

    fn write_u8(&mut self, x: u8) {
        self.write_u64(u64::from(x));
    }

    fn write_u16(&mut self, x: u16) {
        self.write_u64(u64::from(x));
    }

    fn write_u32(&mut self, x: u32) {
        self.write_u64(u64::from(x));
    }

    fn write_u64(&mut self, x: u64) {
        // Xored with the seed after the fold: a state the input cancels
        // to zero folds to zero, and would stay there.
        self.0 = fold(self.0 ^ x, ARM) ^ SEED;
    }

    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }

    fn finish(&self) -> u64 {
        fold(self.0, END)
    }
}

/// What each item begins with, after its pass.
#[derive(Copy, Clone, Hash)]
enum Tag {
    Lookup,
    Node,
    Type,
    Src,
    Fact,
    Declaration,
}

impl Door {
    /// A door for the modules `names` names, in program order;
    /// `expansions` frames their copies.
    pub fn new(names: &[String], expansions: Arc<ExpansionInterner>) -> Door {
        let mut state = State::default();
        state.set_pass((None, 0));
        Door { names: names.iter().map(|n| hash_str(n)).collect(), expansions, state: RefCell::new(state) }
    }

    /// From here on, `pass` reads.
    pub fn pass(&self, pass: Pass) {
        let mut st = self.state.borrow_mut();
        let next = st.pass.1 + 1;
        st.set_pass((Some(pass), next));
    }

    /// `reader` looked `asked` up and was answered `answer`, which `node`
    /// reads (its source, kind and expression as the reader saw them).
    pub fn lookup<'e>(&self, reader: usize, asked: Asked<'_>, answer: Option<Decl>, node: impl FnOnce(Decl) -> (&'e ExpArena, &'e Exp)) {
        let mut st = self.state.borrow_mut();
        let mut h = st.begin(Tag::Lookup);
        match asked {
            Asked::ByName(path) => {
                0u8.hash(&mut h);
                path.hash(&mut h);
            }
            Asked::InScope(scope, name) => {
                1u8.hash(&mut h);
                scope.hash(&mut h);
                name.hash(&mut h);
            }
        }
        match answer {
            None => 0u8.hash(&mut h),
            // The reader's own: where its rounds are followed, its nodes
            // are numbered as the kept run's.
            Some(d) if d.module == reader => {
                1u8.hash(&mut h);
                d.id.index().hash(&mut h);
            }
            Some(d) => {
                2u8.hash(&mut h);
                let (arena, exp) = node(d);
                st.put_ident(self, &mut h, d.module, arena, d.id);
                std::mem::discriminant(exp).hash(&mut h);
            }
        }
        st.finish(reader, h);
    }

    /// `reader` read node `id` of `module` — `exp`, as the reader saw it.
    pub fn node(&self, reader: usize, module: usize, arena: &ExpArena, id: ExpId, exp: &Exp) {
        if module == reader {
            return;
        }
        let mut st = self.state.borrow_mut();
        let mut h = st.begin(Tag::Node);
        st.put_ident(self, &mut h, module, arena, id);
        let mut kids = std::mem::take(&mut st.kids);
        kids.clear();
        exp.children(&mut kids);
        // A leaf is its own shape, with nothing to set aside.
        if kids.is_empty() {
            exp.hash(&mut h);
        } else {
            exp.hash_shape(&mut h);
        }
        for &k in &kids {
            st.put_ident(self, &mut h, module, arena, k);
        }
        st.kids = kids;
        st.finish(reader, h);
    }

    /// `reader` read node `id` of `module`'s type: `ty`, `None` where none
    /// was written.
    pub fn ty(&self, reader: usize, module: usize, arena: &ExpArena, id: ExpId, ty: Option<&TyRef>) {
        if module == reader {
            return;
        }
        let mut st = self.state.borrow_mut();
        let mut h = st.begin(Tag::Type);
        st.put_ident(self, &mut h, module, arena, id);
        match ty {
            None => 0u8.hash(&mut h),
            Some(t) => {
                1u8.hash(&mut h);
                st.strict(t).hash(&mut h);
            }
        }
        st.finish(reader, h);
    }

    /// `reader` read node `id` of `module`'s source and kind.
    pub fn src(&self, reader: usize, module: usize, arena: &ExpArena, id: ExpId) {
        if module == reader {
            return;
        }
        let mut st = self.state.borrow_mut();
        let mut h = st.begin(Tag::Src);
        st.put_ident(self, &mut h, module, arena, id);
        st.finish(reader, h);
    }

    /// `reader` read `module`'s subtree at `id` whole — each node, and its
    /// type where `types` holds the module's.
    pub fn subtree(&self, reader: usize, module: usize, arena: &ExpArena, types: Option<&TypeMap>, id: ExpId) {
        if module == reader {
            return;
        }
        let mut stack = vec![id];
        let mut kids = Vec::new();
        while let Some(at) = stack.pop() {
            let exp = &arena[at].exp;
            self.node(reader, module, arena, at, exp);
            if let Some(types) = types {
                self.ty(reader, module, arena, at, types.entry(at.index()));
            }
            kids.clear();
            exp.children(&mut kids);
            stack.extend(kids.iter().rev().copied());
        }
    }

    /// `reader` read a fact the whole program decides: `what`, answered
    /// `value` (hashed by the caller).
    pub fn fact(&self, reader: usize, what: &str, value: u64) {
        let mut st = self.state.borrow_mut();
        let mut h = st.begin(Tag::Fact);
        what.hash(&mut h);
        value.hash(&mut h);
        st.finish(reader, h);
    }

    /// The round's end: each module's key over what it read since the last
    /// (`modules` of them, in program order), and the next round begins.
    pub fn end_round(&self, modules: usize) -> Vec<Key> {
        let mut st = self.state.borrow_mut();
        st.set_pass((None, 0));
        st.keys(modules, true)
    }

    /// Each module's key over what it read so far this round, the round
    /// going on: what a later run compares before it reuses a pass of the
    /// round (`crate::reuse`).
    pub fn keys_so_far(&self, modules: usize) -> Vec<Key> {
        self.state.borrow_mut().keys(modules, false)
    }

    /// `reader` read declaration `at` of `module` as `spell` reads it —
    /// showing the door each node and type, with any reader but `module`
    /// itself — while `module` stood at `version`, which any write to it
    /// moves ([`kg_hir::ExpArena::writes`], [`TypeMap::writes`]). One item:
    /// what `spell` reads, spelled once for every reader and every pass the
    /// module has not been written since, outside any pass; the item is
    /// the pass's.
    pub fn declaration(&self, reader: usize, module: usize, at: ExpId, version: (u64, u64), spell: impl FnOnce()) {
        let key = (module as u32, at.index() as u32);
        let cached = self.state.borrow().declarations.get(&key).is_some_and(|d| d.version == version);
        if !cached {
            let pass = {
                let mut st = self.state.borrow_mut();
                let at = (st.held.items.len() as u32, st.held.names.len() as u32);
                st.holds.push(at);
                let pass = st.pass;
                st.set_pass((None, 0));
                pass
            };
            spell();
            let mut st = self.state.borrow_mut();
            st.set_pass(pass);
            let (items, names) = st.holds.pop().expect("held above");
            let mut h = Mix::new();
            for &(content, _, len) in &st.held.items[items as usize..] {
                content.hash(&mut h);
                len.hash(&mut h);
            }
            let spelled = st.held.names[names as usize..].to_vec();
            st.held.items.truncate(items as usize);
            st.held.names.truncate(names as usize);
            st.declarations.insert(key, Declaration { version, hash: h.finish(), names: spelled });
        }
        let mut st = self.state.borrow_mut();
        let mut h = st.begin(Tag::Declaration);
        let d = &st.declarations[&key];
        d.hash.hash(&mut h);
        let names = d.names.clone();
        st.scratch.extend_from_slice(&names);
        st.finish(reader, h);
    }

    /// From here on, what is read is held until [`Door::settle`] says
    /// whether the rewrite that read it changed anything: a rewrite tried
    /// and left the module as it was read nothing that moved it, and
    /// another run that left it alike, whatever it read, ran the same.
    /// Nested, an inner rewrite's reads settle into the outer's.
    pub fn tentative(&self) {
        let mut st = self.state.borrow_mut();
        let at = (st.held.items.len() as u32, st.held.names.len() as u32);
        st.holds.push(at);
    }

    /// The reads since the matching [`Door::tentative`]: kept, `reader`'s —
    /// or the enclosing rewrite's — where `changed`, else dropped.
    pub fn settle(&self, reader: usize, changed: bool) {
        let mut st = self.state.borrow_mut();
        let Some((items, names)) = st.holds.pop() else { return };
        if changed && !st.holds.is_empty() {
            // The enclosing hold's now: left where they are.
            return;
        }
        if changed {
            let st = &mut *st;
            for &(content, start, len) in &st.held.items[items as usize..] {
                let named = &st.held.names[start as usize..(start + len) as usize];
                State::keep_in(&mut st.modules, reader, content, named);
            }
        }
        st.held.items.truncate(items as usize);
        st.held.names.truncate(names as usize);
    }
}

impl State {
    /// Each of `modules` modules' key over its items, cleared after where
    /// `clear` — their storage kept for the next round's.
    fn keys(&mut self, modules: usize, clear: bool) -> Vec<Key> {
        let st = self;
        st.modules.resize_with(st.modules.len().max(modules), Items::default);
        st.rank.resize(st.idents.len(), 0);
        st.mark.resize(st.idents.len(), 0);
        let mut keys = Vec::with_capacity(modules);
        for items in &mut st.modules[..modules] {
            st.epoch = st.epoch.wrapping_add(1);
            keys.push(items.key(&st.idents, &st.pool, &mut st.rank, &mut st.mark, st.epoch));
            if clear {
                items.items.clear();
                items.names.clear();
            }
        }
        keys
    }

    fn set_pass(&mut self, pass: (Option<Pass>, u32)) {
        let mut h = Mix::new();
        pass.hash(&mut h);
        self.pass = pass;
        self.seed = h.finish();
    }

    fn begin(&mut self, tag: Tag) -> Mix {
        self.scratch.clear();
        let mut h = Mix::new();
        h.write_u64(self.seed);
        tag.hash(&mut h);
        h
    }

    fn finish(&mut self, reader: usize, h: Mix) {
        let content = h.finish();
        if !self.holds.is_empty() {
            let start = self.held.names.len() as u32;
            self.held.names.extend_from_slice(&self.scratch);
            self.held.items.push((content, start, self.scratch.len() as u32));
            return;
        }
        State::keep_in(&mut self.modules, reader, content, &self.scratch);
    }

    /// Keeps the item `content` naming `names` in `reader`'s round.
    fn keep_in(modules: &mut Vec<Items>, reader: usize, content: u64, names: &[u32]) {
        if modules.len() <= reader {
            modules.resize_with(reader + 1, Items::default);
        }
        let items = &mut modules[reader];
        let start = items.names.len() as u32;
        items.names.extend_from_slice(names);
        items.items.push((content, start, names.len() as u32));
    }

    /// Node `id` of `module`: where it was written, held aside, and the
    /// rest hashed into `h`.
    fn put_ident(&mut self, door: &Door, h: &mut Mix, module: usize, arena: &ExpArena, id: ExpId) {
        let key = (module as u32, id.index() as u32);
        let n = match self.numbered.get(&key) {
            Some(&n) => n,
            None => {
                let node = arena.node(id);
                let i = self.spell_ident(door, module, node.src, node.kind);
                let n = self.idents.len() as u32;
                self.idents.push(i);
                self.numbered.insert(key, n);
                n
            }
        };
        self.idents[n as usize].hash.hash(h);
        self.scratch.push(n);
    }

    fn spell_ident(&mut self, door: &Door, module: usize, src: Src, kind: SyntaxKind) -> Ident {
        let via = self.via(door, src.via);
        let mut h = Mix::new();
        door.names.get(module).copied().unwrap_or(module as u64).hash(&mut h);
        kind.hash(&mut h);
        src.written.hash(&mut h);
        via.hash.hash(&mut h);
        let start = self.pool.len() as u32;
        self.pool.push(pos(src.head));
        let (s, n) = (via.start as usize, via.len as usize);
        self.pool.extend_from_within(s..s + n);
        Ident { hash: h.finish(), start, len: via.len + 1 }
    }

    /// An expansion as its frames, innermost first: each instance, round
    /// and iteration hashed, each site held aside.
    fn via(&mut self, door: &Door, via: Expansion) -> Ident {
        if let Some(&v) = self.vias.get(&via) {
            return v;
        }
        let mut h = Mix::new();
        let start = self.pool.len() as u32;
        let frames = if via == Expansion::ROOT { Vec::new() } else { door.expansions.frames(via) };
        for f in &frames {
            f.instance.0.hash(&mut h);
            f.round.hash(&mut h);
            f.iteration.hash(&mut h);
            self.pool.push(pos(f.site));
        }
        let v = Ident { hash: h.finish(), start, len: frames.len() as u32 };
        self.vias.insert(via, v);
        v
    }

    fn strict(&mut self, t: &TyRef) -> u64 {
        let at = std::ptr::from_ref(t.get()) as usize;
        if let Some((_, hash)) = self.types.get(&at) {
            return *hash;
        }
        let mut h = Mix::new();
        kg_ty::hash_strict(t, &mut h);
        let hash = h.finish();
        self.types.insert(at, (t.clone(), hash));
        hash
    }
}

impl Items {
    /// The round's key: the set of items, each position an identity holds
    /// named by its rank among the round's distinct positions, and those
    /// positions in order.
    /// `rank` and `mark` are scratch by identity, `epoch` new to `mark`.
    fn key(&self, idents: &[Ident], pool: &[Pos], rank: &mut [u64], mark: &mut [u32], epoch: u32) -> Key {
        let mut named = Vec::new();
        for &n in &self.names {
            if std::mem::replace(&mut mark[n as usize], epoch) != epoch {
                named.push(n);
            }
        }
        let held = |n: u32| {
            let i = idents[n as usize];
            &pool[i.start as usize..(i.start + i.len) as usize]
        };
        let mut positions: Vec<Pos> = named.iter().flat_map(|&n| held(n).iter().copied()).collect();
        positions.sort_unstable();
        positions.dedup();
        for &n in &named {
            let mut h = Mix::new();
            for p in held(n) {
                positions.binary_search(p).expect("collected above").hash(&mut h);
            }
            rank[n as usize] = h.finish();
        }
        let mut hashes: Vec<u64> = self
            .items
            .iter()
            .map(|&(content, start, len)| {
                let mut h = Mix::new();
                content.hash(&mut h);
                for &n in &self.names[start as usize..(start + len) as usize] {
                    rank[n as usize].hash(&mut h);
                }
                h.finish()
            })
            .collect();
        hashes.sort_unstable();
        hashes.dedup();
        let mut h = Mix::new();
        hashes.hash(&mut h);
        Key { hash: h.finish(), positions }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each round's keys of two modules, after `read` shows the door what
    /// module 0 read in one pass.
    fn keys(read: &dyn Fn(&Door)) -> Vec<Key> {
        let door = Door::new(&["a".to_owned(), "b".to_owned()], Arc::new(ExpansionInterner::new()));
        door.pass(Pass::Deduce);
        read(&door);
        door.end_round(2)
    }

    /// What a tried rewrite read is kept where the rewrite changed the
    /// module — into the enclosing rewrite's where it is nested — and
    /// dropped where it changed nothing: a round keyed as though it read
    /// nothing, as a run that made no such rewrite is. Red with `settle`
    /// inverted, or a nested rewrite's reads kept past an outer one that
    /// changed nothing.
    #[test]
    fn a_rewrite_that_changed_nothing_read_nothing() {
        let none = keys(&|_| {});
        let direct = keys(&|d| d.fact(0, "x", 1));
        assert_ne!(direct, none);
        let tried = |outer: Option<bool>, inner: bool| {
            keys(&move |d| {
                if outer.is_some() {
                    d.tentative();
                }
                d.tentative();
                d.fact(0, "x", 1);
                d.settle(0, inner);
                if let Some(outer) = outer {
                    d.settle(0, outer);
                }
            })
        };
        assert_eq!(tried(None, false), none);
        assert_eq!(tried(None, true), direct);
        assert_eq!(tried(Some(true), true), direct);
        assert_eq!(tried(Some(false), true), none);
        assert_eq!(tried(Some(true), false), none);
    }
}
