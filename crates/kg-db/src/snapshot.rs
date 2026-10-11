//! The editor's kept loop (sprint W7.5d, D1 step 1;
//! `docs/plans/loop-across-edits.md` §6): under the layered budget
//! ([`PassBudget::Layered`](crate::PassBudget)) each run that reaches its
//! fixpoint keeps its snapshot — the final program and its history per
//! module and round (`kg_sem::snapshot`) — for its root set, until the next
//! run of the same root set reaches its own. A batch compile records none
//! and keeps none: its loop is `crate::looped::looped`'s, unrecorded.
//!
//! **What is held.** The run's [`Elaborated`], shared with the `Looped`
//! built from it (no second copy while that answer is memoized), with each
//! module's parse and the sorted lowering the run was given: the texts a
//! kept position is a position of, and what says a module's input to the
//! loop is the kept run's. An edit moves the `Looped` memo on at the next
//! run's first answer; the kept run stays until the next run ends — so
//! two programs are held while a run that may yet be cancelled is under
//! way, and one otherwise. The next run of the root set reads it: the
//! shadow, where turned on, and reuse (`kg_sem::reuse`), every prefix pass
//! it can take from it, on unless turned off ([`Db::set_reuse`]).
//!
//! **The shadow** ([`Db::set_snapshot_shadow`]): each run of a root set
//! that kept a snapshot, still from nothing, checks every kept round whose
//! keys agree — the copies decided into the module, and what it read across
//! its boundary (`kg_sem::reads`, step 2) — against the round it ran
//! (`kg_sem::snapshot::Shadow`), and
//! leaves its report for [`Db::take_shadow_report`]. Off by default: it
//! walks what every round wrote. `cargo xtask edit-identity` turns it on.
//!
//! **Relocation.** A position in a module whose text changed is carried
//! through the one splice between the kept text and the new
//! (`kg_sem::snapshot::Splice::between`): the editor's change, for the
//! single edit a keystroke makes; several edits in one change are taken as
//! the one splice covering them, what lies between having no place.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use kg_base::{FileId, FxHashMap};
use kg_resolve::ModuleSource;
use kg_sem::elaborate::Elaborated;
use kg_sem::reuse::{Reuse, ReuseReport};
use kg_sem::snapshot::{Relocation, Shadow, ShadowReport, Size, Splice};

use crate::db::Db;
use crate::looped::Inputs;

/// How many root sets keep a snapshot: the `Looped` memo's bound, whose
/// programs the snapshots share while it holds them.
const KEPT_ROOT_SETS: usize = 8;

/// One run's snapshot, and what it was run on.
pub(crate) struct Kept {
    pub(crate) out: Arc<Elaborated>,
    /// The modules the run was given, each one's parse and sorted lowering
    /// — the lowering by pointer, the lowering memo's identity
    /// (`crate::sorted::SortedMemo`).
    pub(crate) inputs: Inputs,
}

/// The snapshots kept, by root set, oldest first in `order`.
#[derive(Debug, Default)]
struct Held {
    map: FxHashMap<Vec<FileId>, Arc<Kept>>,
    order: VecDeque<Vec<FileId>>,
}

impl std::fmt::Debug for Kept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kept").field("modules", &self.inputs.modules).finish_non_exhaustive()
    }
}

/// The kept snapshots and the shadow's reports, by root set — shared with
/// the loop's thread, which writes them as its run ends.
#[derive(Debug, Default)]
pub(crate) struct Keeper {
    kept: Mutex<Held>,
    reports: Mutex<FxHashMap<Vec<FileId>, ShadowReport>>,
    shadow: AtomicBool,
    /// Whether a run reuses its root set's kept run (on unless turned
    /// off), whether it checks what it reuses, and what the last run of
    /// each root set reused.
    no_reuse: AtomicBool,
    check_reuse: AtomicBool,
    reused: Mutex<FxHashMap<Vec<FileId>, ReuseReport>>,
}

impl Keeper {
    pub(crate) fn kept(&self, roots: &[FileId]) -> Option<Arc<Kept>> {
        self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner).map.get(roots).cloned()
    }

    /// Keeps `kept` as `roots`' snapshot, the oldest root set's let go
    /// past the bound — and hands back what it replaced, for the caller to
    /// free where no one waits on it: a program of `risc-v.k`'s size takes
    /// its last answer 0.1 s longer to free first.
    #[must_use]
    pub(crate) fn keep(&self, roots: Vec<FileId>, kept: Kept) -> Vec<Arc<Kept>> {
        let mut guard = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Held { map, order } = &mut *guard;
        let mut gone: Vec<Arc<Kept>> = map.insert(roots.clone(), Arc::new(kept)).into_iter().collect();
        if gone.is_empty() {
            order.push_back(roots);
            while order.len() > KEPT_ROOT_SETS {
                if let Some(oldest) = order.pop_front() {
                    gone.extend(map.remove(&oldest));
                }
            }
        }
        gone
    }

    pub(crate) fn report(&self, roots: Vec<FileId>, report: ShadowReport) {
        self.reports.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(roots, report);
    }

    /// Forgets every snapshot: the settings or the budget changed, and a
    /// kept run is no longer the program a run would be.
    pub(crate) fn clear(&self) {
        let mut guard = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.map.clear();
        guard.order.clear();
        self.reports.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        self.reused.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }

    pub(crate) fn shadowing(&self) -> bool {
        self.shadow.load(Ordering::Relaxed)
    }

    /// Whether a run reuses its root set's kept run, and checks it.
    pub(crate) fn reusing(&self) -> Option<bool> {
        (!self.no_reuse.load(Ordering::Relaxed)).then(|| self.check_reuse.load(Ordering::Relaxed))
    }

    pub(crate) fn report_reuse(&self, roots: Vec<FileId>, report: ReuseReport) {
        self.reused.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(roots, report);
    }
}

impl Kept {
    /// The shadow of this run over a run given `new`: a module is followed
    /// where its lowering is this run's, and a position in a module whose
    /// text changed is carried through the change.
    pub(crate) fn shadow(self: &Arc<Kept>, new: &Inputs) -> Option<Shadow> {
        let (map, relocation) = self.follow(new);
        Shadow::new(Arc::clone(&self.out), &map, relocation)
    }

    /// What a run given `new` may reuse of this one (`kg_sem::reuse`), the
    /// modules followed as the shadow follows them; with `check`, every
    /// pass reused is run as well and compared.
    pub(crate) fn reuse(self: &Arc<Kept>, new: &Inputs, check: bool) -> Option<Reuse> {
        let (map, relocation) = self.follow(new);
        let names = new.modules.iter().map(|m| format!("{m:?}")).collect();
        Reuse::new(Arc::clone(&self.out), &map, names, relocation, check)
    }

    /// Each module of a run given `new` that this one's stands for — its
    /// lowering this run's — and the edit that carries this run's
    /// positions into `new`'s texts.
    fn follow(&self, new: &Inputs) -> (Vec<Option<usize>>, Relocation) {
        let kept = &self.inputs;
        let old = |m: &ModuleSource| kept.modules.iter().position(|o| o == m);
        let map: Vec<Option<usize>> =
            new.modules.iter().zip(&new.sorted).map(|(m, ir)| old(m).filter(|&o| Arc::ptr_eq(&kept.sorted[o], ir))).collect();
        let mut relocation = Relocation::new();
        for (m, parse) in new.modules.iter().zip(&new.parses) {
            let (ModuleSource::File(file), Some(o)) = (m, old(m)) else { continue };
            if Arc::ptr_eq(&kept.parses[o], parse) {
                continue;
            }
            if let Some(splice) = Splice::between(&kept.parses[o].text(), &parse.text()) {
                relocation.edit(*file, splice);
            }
        }
        (map, relocation)
    }
}

impl Db {
    /// Whether each editor's run checks the kept snapshot of its root set
    /// against the rounds it runs (module comment), leaving a report for
    /// [`Db::take_shadow_report`].
    pub fn set_snapshot_shadow(&self, on: bool) {
        self.keeper().shadow.store(on, Ordering::Relaxed);
    }

    /// Whether each editor's run reuses what it can of its root set's kept
    /// run (`kg_sem::reuse`; on unless turned off), and with `check`,
    /// runs what it reuses as well and compares the two.
    pub fn set_reuse(&self, on: bool, check: bool) {
        self.keeper().no_reuse.store(!on, Ordering::Relaxed);
        self.keeper().check_reuse.store(check, Ordering::Relaxed);
    }

    /// What the last run of `roots` that had a kept run reused of it,
    /// taken.
    pub fn take_reuse_report(&self, roots: &[FileId]) -> Option<ReuseReport> {
        self.keeper().reused.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(roots)
    }

    /// What the shadow found in the last run of `roots` that had a kept
    /// snapshot to check, taken.
    pub fn take_shadow_report(&self, roots: &[FileId]) -> Option<ShadowReport> {
        self.keeper().reports.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(roots)
    }

    /// The snapshot kept for `roots`, as sizes: what it holds beyond the
    /// program, and the program's nodes.
    pub fn snapshot_size(&self, roots: &[FileId]) -> Option<Size> {
        let kept = self.keeper().kept(roots)?;
        let nodes = kept.out.modules.iter().map(|m| m.ir.arena.len() as u64).sum();
        Some(kept.out.snapshot.as_ref()?.size(nodes))
    }
}

#[cfg(test)]
mod tests {
    use kg_base::Utf8PathBuf;

    use crate::{Db, PassBudget};

    const LIB: &str = "module lib { Box, id }\n\ntemplate <typename T>\nstruct Box\n{\n    T x;\n}\n\n\
                       template <typename T>\ninline T id(T x)\n{\n    return x;\n}\n";
    const MAIN: &str = "import lib\n\ninline void g()\n{\n}\n\n\
                        inline uint8 f(uint8 y)\n{\n    Box<uint8> b;\n    return id<uint8>(y);\n}\n";

    fn db(budget: PassBudget) -> (Db, kg_base::FileId) {
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        let mut db = Db::new(settings);
        db.set_pass_budget(budget);
        db.set_file_text(&Utf8PathBuf::from("/proj/lib.k"), LIB.to_owned());
        let main = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), MAIN.to_owned());
        (db, main)
    }

    /// The editor keeps its run's snapshot; a batch compile keeps none.
    /// Red with the batch loop recorded, or the editor's not kept.
    #[test]
    fn the_editor_keeps_its_snapshot_and_a_batch_compile_does_not() {
        let (editor, main) = db(PassBudget::EDITOR);
        let _ = editor.check(&[main]);
        let size = editor.snapshot_size(&[main]).expect("the editor keeps a snapshot");
        assert!(size.nodes > 0 && size.key_positions > 0, "{size:?}");
        assert!(editor.looped(&[main]).out.snapshot.is_some());
        let (batch, main) = db(PassBudget::Fixpoint);
        let _ = batch.check(&[main]);
        assert!(batch.snapshot_size(&[main]).is_none());
        assert!(batch.looped(&[main]).out.snapshot.is_none(), "a batch compile records nothing");
    }

    /// A body edit above the requests, in the shadow: the library module's
    /// lowering is the kept run's, its keys agree in every round once the
    /// requests' positions are carried through the edit, and every round
    /// restored is the round run; the edited module is not followed. Red
    /// with the shadow given no relocation (no round agrees), or the
    /// lowering's identity not checked (the edited module followed and
    /// unequal).
    #[test]
    fn a_body_edit_keeps_every_library_round_in_the_shadow() {
        let (mut editor, main) = db(PassBudget::EDITOR);
        editor.set_snapshot_shadow(true);
        let _ = editor.check(&[main]);
        assert!(editor.take_shadow_report(&[main]).is_none(), "the first run has nothing kept to check");
        let at = MAIN.find("    Box").expect("a request");
        let edited = format!("{}    uint8 z = 0;\n{}", &MAIN[..at], &MAIN[at..]);
        editor.set_file_text(&Utf8PathBuf::from("/proj/main.k"), edited);
        let _ = editor.check(&[main]);
        let report = editor.take_shadow_report(&[main]).expect("the second run checks the first's snapshot");
        assert_eq!(report.mismatches, []);
        assert_eq!((report.modules, report.comparable), (3, 2), "the synthetic module and `lib`: {report:#?}");
        assert!(report.rounds > 1, "{report:?}");
        assert_eq!((report.agreed, report.equal, report.whole), (report.rounds, report.rounds, 2), "{report:?}");
    }

    /// The editor reuses what it can of its kept run (D1 step 3): after a
    /// body edit above the requests, every followed module's prefix pass of
    /// every round is taken from the kept run — run as well and found the
    /// same — and what the editor answers is a fresh database's. The
    /// edited module runs its own. Red with the kept run not handed to the
    /// loop, or reuse off by default.
    #[test]
    fn a_body_edit_takes_the_followed_modules_prefix_passes_from_the_kept_run() {
        let (mut editor, main) = db(PassBudget::EDITOR);
        editor.set_reuse(true, true);
        let _ = editor.check(&[main]);
        assert!(editor.take_reuse_report(&[main]).is_none(), "the first run has nothing kept to reuse");
        let at = MAIN.find("    Box").expect("a request");
        let edited = format!("{}    uint8 z = 0;\n{}", &MAIN[..at], &MAIN[at..]);
        editor.set_file_text(&Utf8PathBuf::from("/proj/main.k"), edited.clone());
        let warm = editor.check(&[main]);
        let report = editor.take_reuse_report(&[main]).expect("the second run reuses the first");
        assert_eq!(report.unequal, Vec::<String>::new());
        let rounds = editor.looped(&[main]).out.snapshot.as_ref().map_or(0, |s| s.rounds()) as u64;
        assert_eq!((report.offered, report.reused, report.checked), (2 * rounds, 2 * rounds, 2 * rounds), "{report:?}");
        let (mut fresh, main) = db(PassBudget::EDITOR);
        fresh.set_file_text(&Utf8PathBuf::from("/proj/main.k"), edited);
        let cold = fresh.check(&[main]);
        assert_eq!(format!("{warm:?}"), format!("{cold:?}"));
        let interner = editor.interner();
        assert!(
            kg_sem::elaborate::fingerprint(&editor.looped(&[main]).out, interner)
                == kg_sem::elaborate::fingerprint(&fresh.looped(&[main]).out, fresh.interner()),
            "reuse changed the loop"
        );
        let (default, _) = db(PassBudget::EDITOR);
        assert_eq!(default.keeper().reusing(), Some(false), "reuse is on, unchecked, by default");
    }

    /// A library template whose copy calls a method of the class it is
    /// instantiated with: the copy is decided into `lib`, its key the
    /// argument `Foo`, whose type holds no method — so what the copy reads
    /// of `run` is a read across the module boundary, (3), not in the key.
    const READER_LIB: &str = "module lib { runner }\n\ntemplate <typename C>\ninline uint8 runner(uint8 y)\n{\n    static C c;\n    return c.run(y);\n}\n";
    const READER_MAIN: &str = "import lib\n\ninline void g()\n{\n}\n\nclass Foo\n{\npublic:\n    uint8 run(uint8 a)\n    {\n        return a;\n    }\n}\n\n\
                               inline uint8 f(uint8 y)\n{\n    return runner<Foo>(y);\n}\n";

    /// `main.k`'s text edited by `edit`, in the shadow of its last run.
    fn shadowed(edit: impl Fn(&str) -> String) -> crate::snapshot::ShadowReport {
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        let mut editor = Db::new(settings);
        editor.set_pass_budget(PassBudget::EDITOR);
        editor.set_snapshot_shadow(true);
        editor.set_file_text(&Utf8PathBuf::from("/proj/lib.k"), READER_LIB.to_owned());
        let main = editor.set_file_text(&Utf8PathBuf::from("/proj/main.k"), READER_MAIN.to_owned());
        let _ = editor.check(&[main]);
        editor.set_file_text(&Utf8PathBuf::from("/proj/main.k"), edit(READER_MAIN));
        let _ = editor.check(&[main]);
        editor.take_shadow_report(&[main]).expect("the second run checks the first's snapshot")
    }

    /// The read door (§3's (3)): `run`'s parameter retyped, the copy in
    /// `lib` reads another signature while its key — the copies decided
    /// into `lib` — agrees, and the module is no longer followed from that
    /// round; its state there is not the kept one's, which following it
    /// would have reported. A body edit above `Foo` moves every position
    /// the copy read there and leaves every read the same once they are
    /// carried through the edit: `lib` is followed through every round. Red with the reads left out of what
    /// a round agrees on (a mismatch), with an inference read of another
    /// module not shown the door, or with the reads' positions not carried
    /// through the edit, or taken as they are rather than by rank (the
    /// body edit parts `lib`).
    #[test]
    fn a_read_across_the_boundary_parts_a_module_its_copies_do_not() {
        let retyped = shadowed(|t| t.replace("uint8 run(uint8 a)", "uint8 run(uint16 a)"));
        assert_eq!(retyped.mismatches, [], "{retyped:#?}");
        assert_eq!(retyped.comparable, 2, "the synthetic module and `lib`: {retyped:#?}");
        assert_eq!((retyped.read_parted, retyped.read_parted_equal), (1, 0), "{retyped:#?}");
        assert_eq!(retyped.whole, 1, "the synthetic module alone: {retyped:#?}");
        let moved = shadowed(|t| t.replace("inline void g()\n{\n}", "inline void g()\n{\n    uint8 z = 0;\n}"));
        assert_eq!(moved.mismatches, [], "{moved:#?}");
        assert_eq!((moved.read_parted, moved.whole), (0, 2), "{moved:#?}");
        assert_eq!((moved.agreed, moved.equal), (moved.rounds, moved.rounds), "{moved:#?}");
    }
}
