//! The node arena, and why an index here is not a key.
//!
//! `InstanceIr` and `ModuleIr` own their nodes outright. [`ExpId`] is an arena
//! index and **an arena index is an address, never a key**: a retained memo
//! holding bare `u32`s into a dropped arena is not a crash, it is a silently
//! wrong node (`docs/journal.md`, "Provenance is a copy…"). So every id carries
//! the stamp of the arena that minted it, and resolving an id against any other
//! arena is a panic — loud, at the misuse site — rather than whatever node
//! happens to live at that index. Cross-body references, when they exist, go
//! through a `DefId`-shaped handle instead, the way rust-analyzer's
//! `Body`/`ExprId` pair does.

use std::sync::atomic::{AtomicU32, Ordering};

use kg_base::Src;
use kg_syntax::SyntaxKind;

use crate::exp::Exp;

/// Distinguishes every arena ever created in this process. Never reused, so a
/// stale [`ExpId`] can only miss, never alias.
static NEXT_STAMP: AtomicU32 = AtomicU32::new(1);

/// One node's address within its arena. `Copy`, 8 bytes, and useless without
/// the [`ExpArena`] that minted it — deliberately: the stamp check below is
/// what turns "silently wrong node" into a panic.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct ExpId {
    stamp: u32,
    index: u32,
}

impl ExpId {
    /// The bare index, for renderers that number nodes. Meaningless across
    /// arenas; never store it.
    pub fn index(self) -> usize {
        self.index as usize
    }

    /// No node of any arena (no arena is stamped 0): what stands in for a
    /// child where a node is hashed without its children
    /// ([`crate::Exp::hash_shape`]).
    pub(crate) const PLACEHOLDER: ExpId = ExpId { stamp: 0, index: 0 };
}

/// One lowered node: the expression, where it came from, and the CST kind it
/// was lowered from.
///
/// `kind` is load-bearing, not decoration: `TypeKey` is
/// `{ file, offset, end, kind, via }` (`kg-ty/src/table.rs`) and divergence
/// #21 records what dropping the kind costs — a cross-module lookup becomes a
/// collision rather than a miss. `(src.head, kind, src.via)` is a legal
/// `TypeKey` iff `src.written == Own`.
#[derive(Clone, Debug)]
pub struct NodeData {
    pub exp: Exp,
    pub src: Src,
    pub kind: SyntaxKind,
}

impl NodeData {
    /// `(src.head, kind, src.via)` as a type-table key — legal iff
    /// `src.written == Own` (see the type-level note).
    pub fn type_key(&self) -> kg_ty::TypeKey {
        kg_ty::TypeKey::at(self.src.head.file, self.src.head.range, self.kind, self.src.via)
    }
}

/// An append-only arena of [`NodeData`]. See the module comment for why ids
/// are stamped.
#[derive(Debug)]
pub struct ExpArena {
    stamp: u32,
    nodes: Vec<NodeData>,
    /// The `(stamp, len)` of every arena this one was [`ExpArena::fork`]ed
    /// from, outermost first. Empty for an arena built by the lowering.
    ancestry: Vec<(u32, u32)>,
    /// Opt-in ([`ExpArena::track_lineage`]): `(new, original)` for every
    /// node allocated to stand in for another — a rewrite's replacement, a
    /// structural copy's node. The reference's rewrites keep a node's note,
    /// its type included (`desugar`, `Desugar.hs:92-94`); an append-only
    /// arena mints a fresh node instead, and a caller that keeps types
    /// beside the arena (`kg_sem::TypeMap`) reads this to carry them over.
    /// A fork of a tracking arena tracks too, from empty.
    lineage: Option<Vec<(u32, u32)>>,
    /// Every node [`ExpArena::node_mut`] has handed out, in order: what a
    /// cache of a subtree's contents (`kg_sem::symbols::Filed`) checks the
    /// writes it was told about against, and what a fork refreshed in
    /// place copies again ([`ExpArena::fork_into`]).
    written: Vec<u32>,
    /// Opt-in ([`ExpArena::track_facts`]): what each subtree holds
    /// ([`crate::facts`]), forgotten along every write in place. A fork of a
    /// tracking arena tracks too, from nothing known.
    facts: Option<Box<crate::facts::Facts>>,
    /// For a fork made by [`ExpArena::fork_into`]: the arena it was taken
    /// of, as it stood — its stamp, length and writes.
    taken: Option<(u32, usize, usize)>,
    /// Opt-in ([`ExpArena::begin_undo`]): each node [`ExpArena::node_mut`]
    /// hands out, as it was before — for a node below the floor, the
    /// arena's length when the log was opened; one appended since did not
    /// exist then and needs no undoing. What the loop's per-round snapshot
    /// restores an earlier round from (`kg_sem::snapshot`, sprint W7.5d).
    undo: Option<(u32, Vec<(u32, NodeData)>)>,
}

impl Default for ExpArena {
    fn default() -> ExpArena {
        ExpArena::new()
    }
}

impl ExpArena {
    pub fn new() -> ExpArena {
        ExpArena {
            stamp: NEXT_STAMP.fetch_add(1, Ordering::Relaxed),
            nodes: Vec::new(),
            ancestry: Vec::new(),
            lineage: None,
            written: Vec::new(),
            facts: None,
            taken: None,
            undo: None,
        }
    }

    /// A copy to rewrite into: the same nodes, a **fresh stamp**, and a record
    /// of where the copy came from.
    ///
    /// Pass 8 runs nine sub-passes over one module and five of them rewrite
    /// (`Template.hs:71-80`). Giving each its own arena rebuilds the whole file
    /// nine times per round; appending into one fork lets an unchanged subtree
    /// keep its id, which is what makes a rewrite cost the path it touched
    /// rather than the tree it walked.
    ///
    /// Why not `#[derive(Clone)]`: a derived clone copies the stamp, and two
    /// live arenas sharing a stamp is exactly the "silently wrong node" the
    /// module comment exists to prevent — an id from one would resolve in the
    /// other. Why not a fresh stamp alone: every id anyone already holds, the
    /// [`crate::ModuleIr::root`] included, would stop resolving. So the fork
    /// mints a new stamp *and* remembers `(parent stamp, parent len)`, which
    /// makes the accept rule exact rather than lenient:
    ///
    /// * an id this arena minted resolves;
    /// * an id an ancestor minted, at an index that predates the fork,
    ///   resolves — the prefix is identical, and it stays identical because a
    ///   forked-from arena is never appended to again (`kg_db::sorted` hands
    ///   out `Arc<ModuleIr>`);
    /// * an id from a *sibling* fork does not, which a shared stamp could not
    ///   have caught.
    pub fn fork(&self) -> ExpArena {
        let mut ancestry = self.ancestry.clone();
        ancestry.push((self.stamp, self.nodes.len() as u32));
        debug_assert!(ancestry.windows(2).all(|w| w[0].0 < w[1].0), "ancestry sorted by stamp, as `resolves` searches it");
        ExpArena {
            stamp: NEXT_STAMP.fetch_add(1, Ordering::Relaxed),
            nodes: self.nodes.clone(),
            ancestry,
            lineage: self.lineage.as_ref().map(|_| Vec::new()),
            written: Vec::new(),
            facts: self.facts.as_ref().map(|_| Box::default()),
            taken: None,
            undo: None,
        }
    }

    /// [`ExpArena::fork`] into `into`, whose storage it reuses: the same
    /// arena as a fork, made without allocating one. Where `into` is this
    /// arena's own earlier fork, untouched since, only what changed since
    /// is copied — the nodes appended, and the nodes written in place,
    /// which are the only others that can differ.
    pub fn fork_into(&self, into: &mut ExpArena) {
        let since = into.taken.filter(|&(stamp, len, writes)| {
            stamp == self.stamp
                && len <= self.nodes.len()
                && writes <= self.written.len()
                && into.nodes.len() == len
                && into.written.is_empty()
        });
        match since {
            Some((_, len, writes)) => {
                for &i in &self.written[writes..] {
                    let i = i as usize;
                    if i < len {
                        into.nodes[i].clone_from(&self.nodes[i]);
                    }
                }
                into.nodes.extend_from_slice(&self.nodes[len..]);
            }
            None => into.nodes.clone_from(&self.nodes),
        }
        into.taken = Some((self.stamp, self.nodes.len(), self.written.len()));
        into.stamp = NEXT_STAMP.fetch_add(1, Ordering::Relaxed);
        into.ancestry.clone_from(&self.ancestry);
        into.ancestry.push((self.stamp, self.nodes.len() as u32));
        debug_assert!(into.ancestry.windows(2).all(|w| w[0].0 < w[1].0), "ancestry sorted by stamp, as `resolves` searches it");
        into.lineage = self.lineage.as_ref().map(|_| Vec::new());
        into.written.clear();
        into.facts = self.facts.as_ref().map(|_| Box::default());
    }

    /// Keep, from now on, what each subtree holds ([`crate::facts`]).
    pub fn track_facts(&mut self) {
        self.facts.get_or_insert_with(Box::default);
    }

    /// What the subtree at `id` holds ([`crate::facts`]) — every shape,
    /// where this arena keeps no facts.
    pub fn holds(&mut self, id: ExpId) -> crate::facts::Flags {
        assert!(self.resolves(id), "ExpId used against an arena that did not mint it");
        match self.facts.as_deref_mut() {
            Some(facts) => facts.holds(&self.nodes, id.index as usize),
            None => crate::facts::ALL,
        }
    }

    /// Whether the subtree at `id` holds none of `shapes`.
    pub fn lacks(&mut self, id: ExpId, shapes: crate::facts::Flags) -> bool {
        self.holds(id) & shapes == 0
    }

    /// [`ExpArena::lacks`] for a reader that cannot derive what is not yet
    /// known: false unless it is known.
    pub fn known_to_lack(&self, id: ExpId, shapes: crate::facts::Flags) -> bool {
        assert!(self.resolves(id), "ExpId used against an arena that did not mint it");
        self.facts.as_deref().and_then(|f| f.known(id.index as usize)).is_some_and(|held| held & shapes == 0)
    }

    /// This arena's stamp, which no other arena — a fork of it included —
    /// has.
    pub fn stamp(&self) -> u32 {
        self.stamp
    }

    /// The in-place writes so far ([`ExpArena::node_mut`]): the only way a
    /// node, and so the subtree under it, changes once it is made.
    pub fn writes(&self) -> u64 {
        self.written.len() as u64
    }

    /// The nodes written in place since there were `from` writes
    /// ([`ExpArena::writes`]), in order, a node once per write.
    pub fn written_since(&self, from: u64) -> &[u32] {
        self.written.get(from as usize..).unwrap_or(&[])
    }

    /// Log, from now on, each node written in place as it was before
    /// ([`ExpArena::take_undo`]) — those that exist now; a node appended
    /// later is cut, not undone.
    pub fn begin_undo(&mut self) {
        self.undo = Some((self.nodes.len() as u32, Vec::new()));
    }

    /// The log since it was opened or last taken, oldest first, and a new
    /// one opened at the arena's length now. Empty where none was open.
    pub fn take_undo(&mut self) -> Vec<(u32, NodeData)> {
        let floor = self.nodes.len() as u32;
        match &mut self.undo {
            Some((at, log)) => {
                *at = floor;
                std::mem::take(log)
            }
            None => Vec::new(),
        }
    }

    /// Stop logging.
    pub fn end_undo(&mut self) {
        self.undo = None;
    }

    /// Record, from now on, which node each replacement or copy stands in
    /// for (see the field).
    pub fn track_lineage(&mut self) {
        self.lineage.get_or_insert_with(Vec::new);
    }

    /// The `(new, original)` pairs recorded since tracking began or the
    /// last take, in allocation order.
    pub fn take_lineage(&mut self) -> Vec<(ExpId, ExpId)> {
        let stamp = self.stamp;
        let id = |index| ExpId { stamp, index };
        self.lineage.as_mut().map(std::mem::take).unwrap_or_default().into_iter().map(|(n, o)| (id(n), id(o))).collect()
    }

    /// [`ExpArena::alloc`] for a node standing in for `original`, recorded
    /// when the arena tracks lineage.
    pub fn alloc_for(&mut self, original: ExpId, exp: Exp, src: Src, kind: SyntaxKind) -> ExpId {
        assert!(self.resolves(original), "ExpId used against an arena that did not mint it");
        let id = self.alloc(exp, src, kind);
        if let Some(l) = &mut self.lineage {
            l.push((id.index, original.index));
        }
        id
    }

    /// Whether `id` names a node of this arena — see [`ExpArena::fork`].
    ///
    /// Every index goes through this, and the ancestry grows by one per
    /// fork — a program-wide loop's module is forked every pass of every
    /// round — so it is searched, not scanned: ancestors are pushed in fork
    /// order and stamps only grow, so the list is sorted by stamp.
    fn resolves(&self, id: ExpId) -> bool {
        id.stamp == self.stamp
            || self
                .ancestry
                .binary_search_by_key(&id.stamp, |&(s, _)| s)
                .is_ok_and(|i| id.index < self.ancestry[i].1)
    }

    pub fn alloc(&mut self, exp: Exp, src: Src, kind: SyntaxKind) -> ExpId {
        let id = ExpId { stamp: self.stamp, index: self.nodes.len() as u32 };
        self.nodes.push(NodeData { exp, src, kind });
        id
    }

    /// Whether `other` came about as this arena did — forked from the same
    /// arenas at the same lengths, its own stamp aside — and stands at
    /// `len` nodes as this one does: a later run of the same lowering. Its
    /// ids then translate into this arena's ([`ExpArena::translate`]).
    pub fn parallels(&self, other: &ExpArena) -> bool {
        self.ancestry.len() == other.ancestry.len()
            && self.ancestry.iter().zip(&other.ancestry).all(|(a, b)| a.1 == b.1)
    }

    /// The id of `parallel`'s — an arena [`ExpArena::parallels`] — that
    /// `id` names there, as this arena names the same node: minted by
    /// `parallel` itself, by this arena; minted by an ancestor, by this
    /// arena's ancestor at the same place, an arena the two may share (a
    /// memoized lowering) or not. `None` for an id `parallel` does not
    /// resolve, or one of its own at or past `len`: what this arena will
    /// hold once nodes are taken in up to there ([`ExpArena::adopt`]).
    pub fn translate(&self, parallel: &ExpArena, id: ExpId, len: usize) -> Option<ExpId> {
        if id.stamp == parallel.stamp {
            return ((id.index as usize) < len).then_some(ExpId { stamp: self.stamp, index: id.index });
        }
        let at = parallel.ancestry.binary_search_by_key(&id.stamp, |&(s, _)| s).ok()?;
        let (stamp, len) = *self.ancestry.get(at)?;
        (id.index < len && id.index < parallel.ancestry[at].1).then_some(ExpId { stamp, index: id.index })
    }

    /// Whether this arena minted `id` — not an ancestor it was forked from.
    pub fn mints(&self, id: ExpId) -> bool {
        id.stamp == self.stamp
    }

    /// The id of the node at `index`, as this arena mints it.
    pub fn id_at(&self, index: usize) -> ExpId {
        assert!(index < self.nodes.len(), "no node at {index}");
        ExpId { stamp: self.stamp, index: index as u32 }
    }

    /// Takes in a node as this arena's next — each child already
    /// translated ([`ExpArena::translate`]) and so resolving here — without
    /// a lineage entry or a write: what a kept run's pass appended, where
    /// a later run of the same program reuses it (`kg_sem::reuse`).
    pub fn adopt(&mut self, node: NodeData) -> ExpId {
        let mut kids = Vec::new();
        node.exp.children(&mut kids);
        assert!(kids.iter().all(|&k| self.resolves(k)), "an adopted node's children resolve here");
        let id = ExpId { stamp: self.stamp, index: self.nodes.len() as u32 };
        self.nodes.push(node);
        id
    }

    /// Resolves an id **minted by this arena**. An id from any other arena —
    /// including a dropped one whose index happens to be in range — panics.
    pub fn node(&self, id: ExpId) -> &NodeData {
        assert!(self.resolves(id), "ExpId used against an arena that did not mint it");
        &self.nodes[id.index as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// The node at `index`, for a reader holding a bare index this arena's
    /// own log handed out ([`ExpArena::written_since`], [`ExpArena::take_undo`]).
    pub fn node_at(&self, index: usize) -> &NodeData {
        &self.nodes[index]
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every node, in allocation order (which is lowering walk order).
    pub fn iter(&self) -> impl Iterator<Item = (ExpId, &NodeData)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (ExpId { stamp: self.stamp, index: i as u32 }, n))
    }

    /// Mutable access, for the sorter's `setQualifier` (`Desugar.hs:193-201`),
    /// which grafts a materialised prefix chain into an already-built spine.
    pub fn node_mut(&mut self, id: ExpId) -> &mut NodeData {
        assert!(self.resolves(id), "ExpId used against an arena that did not mint it");
        self.written.push(id.index);
        if let Some((floor, log)) = &mut self.undo
            && id.index < *floor
        {
            log.push((id.index, self.nodes[id.index as usize].clone()));
        }
        if let Some(facts) = self.facts.as_deref_mut() {
            facts.forget(id.index as usize);
        }
        &mut self.nodes[id.index as usize]
    }

    /// A structural copy of one subtree. The reference *shares* the subtree —
    /// `copyNote (TypeParamF n a b c) s` reuses the declaration's kind `a` at
    /// every use site (`Parser.hs:660`) — but sharing in an arena makes a DAG
    /// that every tree-shaped walk here would visit twice, so each use gets
    /// its own copy. Copies keep the original ranges (they point at real CST
    /// text) but are `Donated`: the copy is not the lowering of that node.
    pub fn deep_copy(&mut self, id: ExpId) -> ExpId {
        let mut data = self.node(id).clone();
        data.src.written = kg_base::Written::Donated;
        let mut exp = data.exp.clone();
        for child in exp.children_mut() {
            *child = self.deep_copy(*child);
        }
        data.exp = exp;
        let new = ExpId { stamp: self.stamp, index: self.nodes.len() as u32 };
        self.nodes.push(data);
        if let Some(l) = &mut self.lineage {
            l.push((new.index, id.index));
        }
        new
    }

    /// [`ExpArena::deep_copy`] of a subtree of **another** arena — another
    /// module's declaration, which the reference would share by reference
    /// (`resolvedFuncParams`' alias, `Function.hs:133-141`). No lineage: the
    /// original is not this arena's.
    pub fn deep_copy_from(&mut self, from: &ExpArena, id: ExpId) -> ExpId {
        let mut data = from.node(id).clone();
        data.src.written = kg_base::Written::Donated;
        let mut exp = data.exp.clone();
        for child in exp.children_mut() {
            *child = self.deep_copy_from(from, *child);
        }
        data.exp = exp;
        let new = ExpId { stamp: self.stamp, index: self.nodes.len() as u32 };
        self.nodes.push(data);
        new
    }
}

impl std::ops::Index<ExpId> for ExpArena {
    type Output = NodeData;

    fn index(&self, id: ExpId) -> &NodeData {
        self.node(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_base::{FileId, FileRange, TextRange};

    fn src() -> Src {
        Src::own(FileRange::new(FileId(0), TextRange::new(0.into(), 1.into())))
    }

    #[test]
    fn an_id_resolves_in_the_arena_that_minted_it() {
        let mut arena = ExpArena::new();
        let id = arena.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        assert!(matches!(arena[id].exp, Exp::Barrier));
    }

    #[test]
    fn a_fork_resolves_the_ids_minted_before_it() {
        let mut base = ExpArena::new();
        let id = base.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        let mut fork = base.fork();
        // The prefix is identical, so the same id names the same node in both.
        assert!(matches!(fork[id].exp, Exp::Barrier));
        let appended = fork.alloc(Exp::Public, src(), SyntaxKind::VISIBILITY);
        assert!(matches!(fork[appended].exp, Exp::Public));
        assert!(matches!(base[id].exp, Exp::Barrier), "the base is untouched");
    }

    /// A fork into an old fork's storage is the fork itself: the base's
    /// nodes as they stand now, ids minted before it resolving, a stamp of
    /// its own — and the storage reused, not made again. Red when the
    /// ancestry is not extended (the base's ids stop resolving), when the
    /// nodes are not refreshed, or when the storage is allocated afresh.
    #[test]
    fn a_fork_into_old_storage_is_a_fork() {
        let mut base = ExpArena::new();
        let first = base.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        let mut into = base.fork();
        let later: Vec<ExpId> = (0..64).map(|_| base.alloc(Exp::Public, src(), SyntaxKind::VISIBILITY)).collect();
        base.fork_into(&mut into);
        assert!(matches!(into[later[63]].exp, Exp::Public));
        let (storage, old) = (into.nodes.as_ptr(), into.stamp());
        base.node_mut(first).exp = Exp::Private;
        let fork = base.fork();
        base.fork_into(&mut into);
        // An id the old fork minted must not resolve in the new one.
        assert_ne!(into.stamp(), old);
        assert_eq!(into.nodes.as_ptr(), storage, "storage reused");
        assert_eq!(into.len(), fork.len());
        assert!(matches!(into[first].exp, Exp::Private));
        assert_ne!(into.stamp(), base.stamp());
        assert_ne!(into.stamp(), fork.stamp());
        assert_eq!(into.ancestry, fork.ancestry);
    }

    /// A fork refreshed from what changed is node for node the fork made
    /// whole — over appends, writes in place to old nodes and to new ones,
    /// and a refresh whose target was itself written meanwhile. Red when
    /// the writes are not copied again, the appends are not copied, or a
    /// written target is refreshed from what changed.
    #[test]
    fn a_fork_refreshed_from_what_changed_is_the_whole_fork() {
        let exps = [Exp::Barrier, Exp::Public, Exp::Private, Exp::Auto, Exp::Void];
        let mut base = ExpArena::new();
        let mut ids: Vec<ExpId> = exps.iter().map(|e| base.alloc(e.clone(), src(), SyntaxKind::BARRIER_STMT)).collect();
        let mut into = ExpArena::new();
        let same = |into: &ExpArena, base: &ExpArena| {
            let fork = base.fork();
            assert_eq!(into.len(), fork.len());
            for ((_, a), (_, b)) in into.iter().zip(fork.iter()) {
                assert_eq!(a.exp, b.exp);
            }
        };
        base.fork_into(&mut into);
        same(&into, &base);
        for round in 0..4 {
            base.node_mut(ids[round]).exp = exps[(round + 2) % exps.len()].clone();
            ids.push(base.alloc(exps[round].clone(), src(), SyntaxKind::BARRIER_STMT));
            let last = *ids.last().expect("pushed");
            base.node_mut(last).exp = Exp::Typename;
            base.fork_into(&mut into);
            same(&into, &base);
        }
        // The target written in place: refreshed whole, not from the log —
        // and, as a fork, with no writes of its own.
        into.node_mut(ids[0]).exp = Exp::Str;
        base.fork_into(&mut into);
        same(&into, &base);
        assert_eq!(into.writes(), 0);
    }

    #[test]
    #[should_panic(expected = "did not mint it")]
    fn one_forks_id_does_not_resolve_in_a_sibling_fork() {
        // The hazard a shared stamp could not have caught: two rewrites
        // branching off one lowering, each appending its own nodes.
        let mut base = ExpArena::new();
        let _ = base.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        let mut a = base.fork();
        let mut b = base.fork();
        let in_a = a.alloc(Exp::Public, src(), SyntaxKind::VISIBILITY);
        let _ = b.alloc(Exp::Private, src(), SyntaxKind::VISIBILITY);
        let _ = b.node(in_a);
    }

    #[test]
    #[should_panic(expected = "did not mint it")]
    fn a_forks_own_id_does_not_resolve_in_its_base() {
        let mut base = ExpArena::new();
        let _ = base.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        let mut fork = base.fork();
        let in_fork = fork.alloc(Exp::Public, src(), SyntaxKind::VISIBILITY);
        // The base grows too, so `in_fork`'s index is *in range* here — without
        // that the `Vec` would panic on its own and the test would pass under a
        // shared stamp, which is the hazard it exists to pin.
        let _ = base.alloc(Exp::Private, src(), SyntaxKind::VISIBILITY);
        let _ = base.node(in_fork);
    }

    /// The bound on the ancestry clause, which nothing else reaches: an id the
    /// *base* mints **after** a fork is taken has an index the fork already
    /// used for something else. Without `id.index < n` the fork answers with
    /// its own node — the silently-wrong-node failure this module is built to
    /// prevent, reached through the lenient half of the accept rule.
    ///
    /// The fork doc comment argues this cannot happen because a forked-from
    /// arena is never appended to again. That is a discipline, not a type:
    /// `alloc` takes `&mut self` and nothing stops a caller holding both. This
    /// is the code that turns the discipline violation into a panic.
    #[test]
    #[should_panic(expected = "did not mint it")]
    fn an_ancestors_id_minted_after_the_fork_does_not_resolve_in_the_fork() {
        let mut base = ExpArena::new();
        let _ = base.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        let mut fork = base.fork();
        let _in_fork = fork.alloc(Exp::Public, src(), SyntaxKind::VISIBILITY);
        let after = base.alloc(Exp::Private, src(), SyntaxKind::VISIBILITY);
        let _ = fork.node(after);
    }

    /// A tracking arena records which node each replacement and copy stands
    /// in for, across a fork; an untracked one records nothing. Red with the
    /// fork dropping the flag: a pass's own fork loses every pair, and the
    /// types the caller keeps beside the arena do not follow a rewrite.
    #[test]
    fn a_tracking_arenas_forks_record_what_each_replacement_stands_for() {
        let mut base = ExpArena::new();
        let a = base.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        base.track_lineage();
        let mut fork = base.fork();
        let b = fork.alloc_for(a, Exp::Public, src(), SyntaxKind::VISIBILITY);
        let c = fork.deep_copy(a);
        let pairs: Vec<(usize, usize)> = fork.take_lineage().into_iter().map(|(n, o)| (n.index(), o.index())).collect();
        assert_eq!(pairs, [(b.index(), a.index()), (c.index(), a.index())]);
        assert!(fork.take_lineage().is_empty(), "taken");
        let mut untracked = ExpArena::new();
        let x = untracked.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        let _ = untracked.alloc_for(x, Exp::Public, src(), SyntaxKind::VISIBILITY);
        assert!(untracked.take_lineage().is_empty());
    }

    #[test]
    #[should_panic(expected = "did not mint it")]
    fn an_id_used_against_another_arena_panics_instead_of_aliasing() {
        let mut a = ExpArena::new();
        let mut b = ExpArena::new();
        let id = a.alloc(Exp::Barrier, src(), SyntaxKind::BARRIER_STMT);
        // `b` has a node at the same index; without the stamp this would
        // silently answer with it.
        let _ = b.alloc(Exp::Public, src(), SyntaxKind::VISIBILITY);
        let _ = b.node(id);
    }
}
