//! The type of every node, keyed by the node (`docs/history/ir-elaboration.md`
//! §2.1).
//!
//! The reference keeps the type *in* the node's note (`Typed n`,
//! `Type.hs:465-471`), initialised to `TUnresolved` by `noteType`
//! (`Type.hs:970-972`). One `TypeMap` per arena is that note, as a dense side
//! vector indexed by [`ExpId::index`]: every node has one, minted and donated
//! nodes alike — which the CST inference engine's position-keyed type table
//! (deleted in sprint W3.7) could not offer a node with no text of its own.
//!
//! The write rules are that table's, moved over unchanged: `resolve` is
//! `resolveTypeOf` (`Type.hs:1037-1048`), `set_type_of` is `setTypeOf`
//! with its `TInstance` threading (`:1050-1057`), and an undo journal backs
//! the one place the reference *discards* a computed subtree
//! (`Inference.hs:55-56`).

use std::sync::atomic::{AtomicU32, Ordering};

use kg_base::FxHashMap;
use kg_hir::ExpId;
use kg_ty::{Ty, TyRef};

/// Entries as they were before a write changed them, oldest first
/// ([`TypeMap::take_undo`]).
pub type UndoLog = Vec<(u32, Option<TyRef>)>;

/// A point in [`TypeMap`]'s undo journal.
#[derive(Copy, Clone, Debug)]
pub struct Checkpoint(usize);

/// Distinguishes every map, a clone included: what [`TypeMap::clone_into`]
/// knows its earlier clone's source by.
static NEXT_MAP: AtomicU32 = AtomicU32::new(1);

#[derive(Debug)]
pub struct TypeMap {
    id: u32,
    types: Vec<Option<TyRef>>,
    /// `tResolved` of each entry, computed when it is written: asked of
    /// every node on every walk and count, and a type is a tree.
    resolved: Vec<bool>,
    journal: Vec<(usize, Option<TyRef>)>,
    recording: u32,
    /// Writes that changed a node's type, ever — rolled back or not.
    changes: u64,
    /// Since [`TypeMap::begin_epoch`]: each written entry's value before
    /// its first write — what [`TypeMap::before`] answers with.
    epoch: Option<FxHashMap<usize, Option<TyRef>>>,
    /// The entries the epoch's writes changed, in order, once per change.
    written: Vec<usize>,
    /// Every entry written since the first `logged` writes, in order: what
    /// [`TypeMap::clone_into`] copies again into a clone taken earlier.
    /// Dropped whenever it outgrows the map — a clone older than that is
    /// copied whole.
    log: Vec<u32>,
    logged: u64,
    /// For a clone made by [`TypeMap::clone_into`]: the map it was taken
    /// of — its identity, length and writes so far.
    taken: Option<(u32, usize, u64)>,
    /// Opt-in ([`TypeMap::begin_undo`]): each entry below the floor as it
    /// was before a write changed it — strictly, a parameter's name
    /// included, which `==` ignores (`Type.hs:126-129`). The loop's
    /// per-round snapshot (`crate::snapshot`), as `kg_hir`'s arena logs its
    /// nodes; a clone logs nothing.
    undo: Option<(u32, UndoLog)>,
}

impl Default for TypeMap {
    fn default() -> TypeMap {
        TypeMap {
            id: NEXT_MAP.fetch_add(1, Ordering::Relaxed),
            types: Vec::new(),
            resolved: Vec::new(),
            journal: Vec::new(),
            recording: 0,
            changes: 0,
            epoch: None,
            written: Vec::new(),
            log: Vec::new(),
            logged: 0,
            taken: None,
            undo: None,
        }
    }
}

/// A clone is another map, with an identity of its own.
impl Clone for TypeMap {
    fn clone(&self) -> TypeMap {
        TypeMap {
            types: self.types.clone(),
            resolved: self.resolved.clone(),
            journal: self.journal.clone(),
            recording: self.recording,
            changes: self.changes,
            epoch: self.epoch.clone(),
            written: self.written.clone(),
            ..TypeMap::default()
        }
    }
}

impl TypeMap {
    pub fn new() -> TypeMap {
        TypeMap::default()
    }

    fn writes_so_far(&self) -> u64 {
        self.logged + self.log.len() as u64
    }

    /// Every write so far, rollbacks included: an entry that reads
    /// otherwise than it did has been written since.
    pub fn writes(&self) -> u64 {
        self.writes_so_far()
    }

    fn log_write(&mut self, i: usize) {
        if self.log.len() > self.types.len() {
            self.logged += self.log.len() as u64;
            self.log.clear();
        }
        self.log.push(i as u32);
    }

    /// `typeOf`: the node's type, `TUnresolved` until something wrote one.
    pub fn get(&self, id: ExpId) -> Ty {
        self.types.get(id.index()).and_then(|t| t.as_ref()).map_or(Ty::Unresolved, |t| (**t).clone())
    }

    /// Whether the node has had a type written (resolved or not).
    pub fn is_set(&self, id: ExpId) -> bool {
        self.types.get(id.index()).is_some_and(Option::is_some)
    }

    /// `typeResolved` (`tResolved . typeOf`).
    pub fn resolved(&self, id: ExpId) -> bool {
        self.resolved.get(id.index()).copied().unwrap_or(false)
    }

    /// `resolveTypeOf` (`Type.hs:1037-1048`): install `ty` if the node is
    /// undefined, or `ty` is resolved or one of the five unresolved shapes
    /// the reference installs anyway.
    pub fn resolve(&mut self, id: ExpId, ty: Ty) {
        let installable = self.get(id).is_undefined()
            || matches!(&ty, Ty::Array { dims, .. } if dims.is_empty())
            || matches!(ty, Ty::Designator(..) | Ty::Positional(..) | Ty::Initializer(_) | Ty::Template)
            || ty.resolved()
            || ty.is_undefined();
        if installable {
            self.set_type_of(id, ty);
        }
    }

    /// `setTypeOf` (`Type.hs:1050-1057`): unconditional, except that an
    /// unresolved `TInstance` keeps its wrapper and takes the new type as its
    /// concrete type.
    pub fn set_type_of(&mut self, id: ExpId, ty: Ty) {
        let next = match self.get(id) {
            Ty::Type(inner) if inner.is_instance() => return,
            Ty::Instance { concrete, template, args } => {
                if concrete.resolved() {
                    return;
                }
                match ty {
                    Ty::Type(t) => Ty::Type(TyRef::new(Ty::Instance { concrete: t, template, args })),
                    t => Ty::Instance { concrete: TyRef::new(t), template, args },
                }
            }
            _ => ty,
        };
        self.write(id, TyRef::new(next));
    }

    /// `resetTypeOf` (`Type.hs:1059-1060`): overwrite, no threading.
    pub fn reset_type_of(&mut self, id: ExpId, ty: Ty) {
        self.write(id, TyRef::new(ty));
    }

    fn write(&mut self, id: ExpId, ty: TyRef) {
        let i = id.index();
        if self.types.len() <= i {
            self.types.resize(i + 1, None);
            self.resolved.resize(i + 1, false);
        }
        self.log_undo(i, Some(&ty));
        if let Some(epoch) = &mut self.epoch {
            epoch.entry(i).or_insert_with(|| self.types[i].clone());
            // Every write that changes the entry, however often: what a
            // reader of it later in the iteration would see move. A write
            // of the value already there moves nothing.
            if self.types[i].as_ref() != Some(&ty) {
                self.written.push(i);
            }
        }
        if self.recording > 0 {
            self.journal.push((i, self.types[i].clone()));
        }
        if self.types[i].as_ref() != Some(&ty) {
            self.changes += 1;
        }
        self.resolved[i] = ty.resolved();
        self.types[i] = Some(ty);
        self.log_write(i);
    }

    /// `self.clone()` into `into`, whose storage it reuses. Where `into` is
    /// this map's own earlier clone, untouched since, only the entries
    /// written since are copied again, and those added.
    pub fn clone_into(&self, into: &mut TypeMap) {
        let since = into.taken.filter(|&(id, len, at)| {
            id == self.id
                && len <= self.types.len()
                && at >= self.logged
                && at <= self.writes_so_far()
                && into.types.len() == len
                && into.writes_so_far() == 0
        });
        match since {
            Some((_, len, at)) => {
                for &i in &self.log[(at - self.logged) as usize..] {
                    let i = i as usize;
                    if i < len {
                        into.types[i].clone_from(&self.types[i]);
                        into.resolved[i] = self.resolved[i];
                    }
                }
                into.types.extend_from_slice(&self.types[len..]);
                into.resolved.extend_from_slice(&self.resolved[len..]);
            }
            None => {
                into.types.clone_from(&self.types);
                into.resolved.clone_from(&self.resolved);
            }
        }
        into.taken = Some((self.id, self.types.len(), self.writes_so_far()));
        into.log.clear();
        into.logged = 0;
        into.journal.clone_from(&self.journal);
        into.recording = self.recording;
        into.changes = self.changes;
        into.epoch.clone_from(&self.epoch);
        into.written.clone_from(&self.written);
    }

    /// Remember, from now on, what each entry was before it is written: an
    /// inference iteration's view of the map as the iteration found it,
    /// without a copy.
    pub fn begin_epoch(&mut self) {
        self.epoch = Some(FxHashMap::default());
        self.written.clear();
    }

    /// The entries written since the epoch began, from the `from`th on.
    pub fn written_since(&self, from: usize) -> &[usize] {
        self.written.get(from..).unwrap_or(&[])
    }

    /// Stop remembering, and say which entries the epoch changed — written
    /// and not written back, rollbacks included.
    pub fn end_epoch(&mut self) -> Vec<usize> {
        let Some(epoch) = self.epoch.take() else { return Vec::new() };
        epoch.into_iter().filter(|(i, old)| self.types.get(*i) != Some(old)).map(|(i, _)| i).collect()
    }

    /// [`TypeMap::get`] as it was at [`TypeMap::begin_epoch`].
    pub fn before(&self, id: ExpId) -> Ty {
        match self.epoch.as_ref().and_then(|e| e.get(&id.index())) {
            Some(old) => old.as_ref().map_or(Ty::Unresolved, |t| (**t).clone()),
            None => self.get(id),
        }
    }

    /// [`TypeMap::entry`] as it was at [`TypeMap::begin_epoch`].
    pub fn before_entry(&self, id: ExpId) -> Option<&TyRef> {
        match self.epoch.as_ref().and_then(|e| e.get(&id.index())) {
            Some(old) => old.as_ref(),
            None => self.entry(id.index()),
        }
    }

    /// [`TypeMap::is_set`] as it was at [`TypeMap::begin_epoch`].
    pub fn before_is_set(&self, id: ExpId) -> bool {
        match self.epoch.as_ref().and_then(|e| e.get(&id.index())) {
            Some(old) => old.is_some(),
            None => self.is_set(id),
        }
    }

    /// Log entry `i` as it stands, where the undo log is open, `i` is below
    /// its floor and `next` changes it — by pointer, or else strictly.
    fn log_undo(&mut self, i: usize, next: Option<&TyRef>) {
        let Some((floor, log)) = &mut self.undo else { return };
        if i >= *floor as usize {
            return;
        }
        let now = self.types[i].as_ref();
        let same = match (now, next) {
            (None, None) => true,
            (Some(a), Some(b)) => std::ptr::eq(a.get(), b.get()) || a.cmp(b) == std::cmp::Ordering::Equal,
            _ => false,
        };
        if !same {
            log.push((i as u32, now.cloned()));
        }
    }

    /// Log, from now on, each entry of the first `floor` nodes as it was
    /// before a write changed it ([`TypeMap::take_undo`]): a node at or past
    /// `floor` did not exist when the log was opened.
    pub fn begin_undo(&mut self, floor: usize) {
        self.undo = Some((floor as u32, Vec::new()));
    }

    /// The log since it was opened or last taken, oldest first, and a new
    /// one opened with `floor`. Empty where none was open.
    pub fn take_undo(&mut self, floor: usize) -> UndoLog {
        match &mut self.undo {
            Some((at, log)) => {
                *at = floor as u32;
                std::mem::take(log)
            }
            None => Vec::new(),
        }
    }

    /// Stop logging.
    pub fn end_undo(&mut self) {
        self.undo = None;
    }

    /// The entry at `index` as it is held — no clone of the type.
    pub fn entry(&self, index: usize) -> Option<&TyRef> {
        self.types.get(index).and_then(Option::as_ref)
    }

    /// How many writes have changed a node's type (see the field).
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// Start recording, so what follows can be undone.
    pub fn begin(&mut self) -> Checkpoint {
        self.recording += 1;
        Checkpoint(self.journal.len())
    }

    /// Keep what was written since `at`.
    pub fn commit(&mut self, _at: Checkpoint) {
        self.recording -= 1;
        if self.recording == 0 {
            self.journal.clear();
        }
    }

    /// Undo every write since `at`.
    pub fn rollback(&mut self, at: Checkpoint) {
        while self.journal.len() > at.0 {
            let (i, previous) = self.journal.pop().expect("length checked");
            self.log_undo(i, previous.as_ref());
            self.resolved[i] = previous.as_ref().is_some_and(|t| t.resolved());
            self.types[i] = previous;
            self.log_write(i);
        }
        self.recording -= 1;
    }
}

impl kg_hir::NodeTypes for TypeMap {
    fn type_at(&self, _arena: &kg_hir::ExpArena, id: ExpId) -> Ty {
        self.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<ExpId> {
        let mut arena = kg_hir::ExpArena::new();
        let src = kg_base::Src::own(kg_base::FileRange::new(kg_base::FileId(0), kg_base::TextRange::new(0.into(), 1.into())));
        (0..n).map(|_| arena.alloc(kg_hir::Exp::Barrier, src, kg_syntax::SyntaxKind::ERROR)).collect()
    }

    /// `resolveTypeOf` installs a resolved type over an unresolved one, and
    /// leaves a resolved node alone only because the caller never asks; an
    /// unresolved type goes only over an undefined node.
    #[test]
    fn resolve_installs_what_the_reference_installs() {
        let id = ids(1)[0];
        let mut m = TypeMap::new();
        assert_eq!(m.get(id), Ty::Unresolved);
        m.resolve(id, Ty::Unresolved);
        assert!(!m.is_set(id), "an unresolved type over an unresolved node installs nothing");
        m.resolve(id, Ty::Undefined);
        assert_eq!(m.get(id), Ty::Undefined);
        m.resolve(id, Ty::Unresolved);
        assert_eq!(m.get(id), Ty::Unresolved, "over an undefined node anything installs");
        m.resolve(id, Ty::Boolean);
        assert!(m.resolved(id));
    }

    /// `resolveTypeOf`'s other four shapes (`Type.hs:1041-1044`) —
    /// `TDesignator`, `TPositional`, `TInitializer`, `TTemplate` — install
    /// though they are not resolved. Moved from the CST engine's table with
    /// its rules (sprint W3.7). Red with `resolve` installing resolved types
    /// alone.
    #[test]
    fn resolve_installs_the_four_other_shapes_the_reference_lists() {
        let shapes = [
            Ty::Designator(kg_base::Interner::new().intern("f"), TyRef::new(Ty::Unresolved)),
            Ty::Positional(0, TyRef::new(Ty::Unresolved)),
            Ty::Initializer(vec![TyRef::new(Ty::Unresolved)]),
            Ty::Template,
        ];
        let at = ids(shapes.len());
        let mut m = TypeMap::new();
        for (i, shape) in shapes.into_iter().enumerate() {
            assert!(!shape.resolved(), "shape {i} is meant to be unresolved");
            m.resolve(at[i], shape.clone());
            assert_eq!(m.get(at[i]), shape);
        }
    }

    /// `setTypeOf` threads a type through an unresolved instance
    /// (`Type.hs:1052-1056`: `set (TInstance a b c) | (TType t') <- t =
    /// TType (TInstance t' b c)`). Moved from the CST engine's table with
    /// its rules (sprint W3.7). Red with the instance overwritten.
    #[test]
    fn set_type_of_threads_through_an_unresolved_instance() {
        let interner = kg_base::Interner::new();
        let tmpl = kg_base::SymbolPath::single(interner.intern("T"));
        let id = ids(1)[0];
        let mut m = TypeMap::new();
        m.reset_type_of(id, Ty::Instance { concrete: TyRef::new(Ty::Unresolved), template: tmpl.clone(), args: Vec::new() });
        m.set_type_of(id, Ty::Type(TyRef::new(Ty::Unsigned(8))));
        match m.get(id) {
            Ty::Type(inner) => match &*inner {
                Ty::Instance { concrete, template, .. } => {
                    assert_eq!(**concrete, Ty::Unsigned(8));
                    assert_eq!(*template, tmpl);
                }
                other => panic!("expected TType (TInstance …), got {other:?}"),
            },
            other => panic!("expected TType …, got {other:?}"),
        }
    }

    /// A clone into an old clone's storage answers every entry as the map
    /// does, what the epoch remembers included, and reuses the storage.
    /// Red when a field is left as the old clone had it.
    #[test]
    fn a_clone_into_old_storage_is_a_clone() {
        let v = ids(40);
        let mut m = TypeMap::new();
        m.resolve(v[0], Ty::Boolean);
        let mut into = m.clone();
        for &id in &v[1..] {
            m.resolve(id, Ty::Float);
        }
        m.clone_into(&mut into);
        let storage = into.types.as_ptr();
        m.begin_epoch();
        m.reset_type_of(v[0], Ty::Str);
        m.reset_type_of(v[1], Ty::Unresolved);
        m.clone_into(&mut into);
        assert_eq!(into.types.as_ptr(), storage, "storage reused");
        for &id in &v {
            assert_eq!(into.get(id), m.get(id));
            assert_eq!(into.resolved(id), m.resolved(id));
            assert_eq!(into.before(id), m.before(id));
        }
        assert_eq!(into.changes(), m.changes());
        assert_eq!(into.written_since(0), m.written_since(0));
        let (mut a, mut b) = (into.end_epoch(), m.end_epoch());
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
        assert_eq!(a, [0, 1]);
    }

    /// A clone refreshed from what changed answers as a clone made whole —
    /// over writes, rollbacks, a log outgrown and dropped, and a target
    /// itself written meanwhile. Red when the writes or the rollbacks are
    /// not logged, the log's entries are not copied again, the appended
    /// entries are not copied, or a dropped log or a written target is
    /// refreshed from what is left.
    #[test]
    fn a_clone_refreshed_from_what_changed_is_the_whole_clone() {
        let v = ids(64);
        let same = |into: &TypeMap, m: &TypeMap| {
            let whole = m.clone();
            for &id in &v {
                assert_eq!((into.get(id), into.is_set(id), into.resolved(id)), (whole.get(id), whole.is_set(id), whole.resolved(id)));
                assert_eq!(into.before(id), whole.before(id));
            }
            assert_eq!(into.changes(), whole.changes());
            assert_eq!(into.writes_so_far(), 0, "a clone has written nothing");
        };
        let mut m = TypeMap::new();
        let mut into = TypeMap::new();
        m.resolve(v[40], Ty::Boolean);
        m.clone_into(&mut into);
        same(&into, &m);
        for k in 0..4 {
            m.resolve(v[k + 1], Ty::Float);
            m.reset_type_of(v[k], Ty::Str);
            // Grown past what the clone was taken at.
            m.resolve(v[41 + k], Ty::Float);
            let at = m.begin();
            m.reset_type_of(v[k + 2], Ty::Boolean);
            m.reset_type_of(v[40], Ty::Undefined);
            m.rollback(at);
            m.clone_into(&mut into);
            same(&into, &m);
        }
        // A rollback after the clone was taken, of writes before it.
        let at = m.begin();
        m.reset_type_of(v[7], Ty::Str);
        m.clone_into(&mut into);
        same(&into, &m);
        m.rollback(at);
        m.clone_into(&mut into);
        same(&into, &m);
        // More writes than the map is long: the log is dropped.
        for k in 0..100 {
            m.reset_type_of(v[k % 3], if k % 2 == 0 { Ty::Float } else { Ty::Str });
        }
        m.reset_type_of(v[11], Ty::Boolean);
        m.clone_into(&mut into);
        same(&into, &m);
        // The target written meanwhile: refreshed whole.
        into.reset_type_of(v[5], Ty::Void);
        m.reset_type_of(v[6], Ty::Str);
        m.clone_into(&mut into);
        same(&into, &m);
        // Another map, as long and written as often: refreshed whole.
        let (mut a, mut b) = (TypeMap::new(), TypeMap::new());
        a.resolve(v[63], Ty::Boolean);
        a.clone_into(&mut into);
        same(&into, &a);
        b.resolve(v[63], Ty::Void);
        b.resolve(v[0], Ty::Boolean);
        b.clone_into(&mut into);
        same(&into, &b);
    }

    /// The journal undoes exactly what was written since the checkpoint.
    #[test]
    fn rollback_undoes_the_writes_since_the_checkpoint() {
        let v = ids(2);
        let mut m = TypeMap::new();
        m.resolve(v[0], Ty::Boolean);
        let at = m.begin();
        m.resolve(v[1], Ty::Float);
        m.reset_type_of(v[0], Ty::Str);
        m.rollback(at);
        assert_eq!(m.get(v[0]), Ty::Boolean);
        assert!(!m.is_set(v[1]));
    }
}
