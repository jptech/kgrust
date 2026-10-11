//! `Src` — where a lowered node came from, transcribed from the reference.
//!
//! The reference annotates every AST node with `Src` (`Parser/Syntax.hs:610-621`),
//! a non-empty stack of real source locations: the head is where a diagnostic is
//! reported and the tail is the instantiation chain rendered as `while compiling`
//! notes (`Error.hs:176-195`). Two facts fixed this design (see
//! `docs/journal.md`, "Provenance is a copy, not a mapping"):
//!
//! * **The head never moves.** `instance Semigroup Src` is a stack push that
//!   keeps the left operand on top (`Parser/Syntax.hs:623-626`), and every
//!   consumer reports the head (`Error.hs:182,197,201`). An error inside an
//!   instantiated template body points at the *body*, in the template's file.
//! * **Synthesis is a copy, never an invention.** `newNote` hands a new node a
//!   donor's `Src` verbatim (`Type.hs:983-984`); no pass ever fabricates a
//!   range. So `head` is always a real range in a real file.
//!
//! This lives in `kg-base`, not `kg-hir`: `kg-ty` and `kg-const` construct
//! `Diagnostic`s directly (`kg-ty/src/convert.rs:268,331,359`,
//! `kg-const/src/eval.rs:340`) and are forbidden from depending on `kg-db`
//! (`kg-db/src/lib.rs:8-10`), so the type every such constructor will take has
//! to sit below all of them.

use std::sync::Mutex;

use crate::{FileRange, FxHashMap};

/// `Src` (`Parser/Syntax.hs:610-621`), transcribed. A non-empty stack of real
/// source ranges: `head` is where a diagnostic is reported
/// (`Error.hs:182-184,197,201`), `via` is the instantiation chain
/// `expandErrorStack` renders as `while compiling` (`Error.hs:186-195`).
///
/// `SrcUnknown` is deliberately not carried over: the reference's consumers all
/// treat it as "report nowhere", and here a node that has no range of its own
/// borrows a donor's instead ([`Src::donated`]) — which is also what the
/// reference's rewrites do at every synthesis site.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Src {
    /// The head of `SrcStack`. **Always a real range in a real file.** A
    /// synthetic node copies a donor's (`newNote`, `Type.hs:983-984`), so no
    /// node ever invents one.
    pub head: FileRange,
    /// Whether `head` is this node's own text or a range it was *given*.
    /// **Not in the reference**, which cannot tell them apart and does not need
    /// to. Here it is the difference between an index an editor may answer a
    /// position question from and one it may not.
    pub written: Written,
    /// `SrcStack`'s tail, interned. [`Expansion::ROOT`] is a bare `Src`.
    pub via: Expansion,
}

impl Src {
    /// A bare `Src` for a node lowered from its own CST node — the analogue of
    /// the parser's `noted` (`Parser.hs:58-64`). `(head, kind)` is a legal
    /// `TypeKey` for exactly the nodes built this way.
    pub fn own(head: FileRange) -> Src {
        Src { head, written: Written::Own, via: Expansion::ROOT }
    }

    /// `newNote` (`Type.hs:983-984`): a synthesized node wears a donor's range
    /// verbatim, and no position question may be answered from it.
    ///
    /// The donor's `via` is kept: a node synthesized inside an instantiated
    /// body still belongs to that body's `while compiling` chain.
    pub fn donated(donor: Src) -> Src {
        Src { head: donor.head, written: Written::Donated, via: donor.via }
    }

    /// `desugarWithNote n` (`Template.hs:127`, `Desugar.hs:92-94`), specialised
    /// to the one shape `instantiateTemplates` uses: the copied node keeps its
    /// own head **and its own `written`** — an instantiated copy of a body node
    /// *is* the user's own text — and the instantiation site is pushed onto the
    /// tail. Getting this wrong (marking the copy `Donated`) silently empties
    /// every `written == Own` index an editor answers from; the journal entry
    /// records a draft that did exactly that.
    pub fn expanded(
        node: Src,
        site: FileRange,
        instance: InstanceId,
        round: u32,
        expansions: &ExpansionInterner,
    ) -> Src {
        let via = expansions.intern(ExpansionData { site, instance, parent: node.via, iteration: false, round });
        Src { head: node.head, written: node.written, via }
    }
}

/// See [`Src::written`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Written {
    /// `head` is this node's own text, and `(head, kind)` is a valid `TypeKey`.
    Own,
    /// `head` came from a donor. No position question may be answered from it.
    Donated,
    /// `head` **is** this node's own text — every position question may be
    /// answered from it — but an [`Own`](Written::Own) node already claims
    /// `(head, kind)`, so no key question may.
    ///
    /// One producer: `kg_hir::lower`'s `reread`. `g[idx]++` is
    /// `Assign(g[idx], g[idx] + 1)` and the lvalue's text is lowered a second
    /// time for the right-hand side; both readings stand at the same range and
    /// kind, and only one can own the `TypeKey`. Marking the copy `Donated`
    /// said too much — the reference stamps *both* readings with the same type
    /// and gives both the same position (`static-if.k::17`'s `x++`, the
    /// recorded walk) — while reading the table for every donated node says
    /// far too little: a donated attribute list picks up the `uint1` of the
    /// `Own` node sharing its key, and `ptdiff` collapses from 1547 to 0
    /// (measured 2026-09-19).
    Reread,
}

/// An instance of a template — the key slot of an `ExpansionData` frame.
///
/// Defined here rather than beside the instance table (a later slice of pass 8,
/// in `kg-db`) because [`ExpansionData`] needs the *name* now: a `while
/// compiling` note renders the instance it was compiling
/// (`addErrorContext`, `Error.hs:192-194`). Only the table that will mint these
/// can resolve one.
///
/// Minted **content-addressed** from the pair the instance table already
/// computes — `(template qualified name, argument key)`
/// (`kg-db/src/instances.rs`, `InstanceSet::templates`) — so the id is a fact
/// about the instance's *text*, never about an offset or a decision order: an
/// edit above the deciding site changes no id (the measured alternative
/// churned 134,178 of 134,178 expansion keys on a one-blank-line edit —
/// `docs/journal.md`, "The fifth field works…"), and two modules
/// instantiating `Foo<uint8>` mint the *same* id, which is what allows the
/// instance's body inference to ever be shared.
///
/// 64 bits, not 32: two instances sharing an id share every expansion key,
/// which the provenance gate turns red (verified by the survey's
/// falsification) — at the reference's 23,931 distinct instances a 32-bit
/// birthday collision is ~6%, a 64-bit one ~2e-11.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct InstanceId(pub u64);

impl InstanceId {
    /// FNV-1a over `template`, a `0` byte no rendering emits, then `key`.
    /// The hash is spelled out rather than borrowed from `FxHasher` so the id
    /// a build mints today is the id every build mints.
    pub fn of(template: &str, argument_key: &str) -> InstanceId {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut h = OFFSET;
        for &b in template.as_bytes().iter().chain([0u8].iter()).chain(argument_key.as_bytes()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(PRIME);
        }
        InstanceId(h)
    }
}

/// An interned `SrcStack` tail. [`Expansion::ROOT`] is the empty stack — a bare
/// `Src` (`Parser/Syntax.hs:610-616`); anything else resolves through the
/// [`ExpansionInterner`] to one [`ExpansionData`] frame.
///
/// Interned because `Src` rides on every IR node and is `Copy`: the stack is
/// shared structure, the way the reference shares it as an ordinary immutable
/// list.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Expansion(u32);

impl Expansion {
    /// The empty stack: a `Src` that is not inside any instantiation.
    pub const ROOT: Expansion = Expansion(0);
}

/// One link of `SrcStack` (`Parser/Syntax.hs:618`). Pushed by instantiation
/// (`desugarWithNote n`, `Template.hs:127`) and by an unrolled `static for`'s
/// iterations ([`ExpansionData::iteration`]); the other rewrites copy a
/// donor's `Src` unchanged, because that is what `newNote` does.
///
/// The frame carries two different kinds of fact, and only one of them is
/// identity:
///
/// * `instance` and `parent` are the **interned key** — which instance's copy
///   this is, inside which enclosing copy. Both are offset-free.
/// * `site` is **rendering data only**, kept so `while compiling` can point at
///   the deciding `Foo<…>` (`Error.hs:186-195`). It is a `FileRange`, so if it
///   were part of the key, any edit above an instantiation would re-intern
///   every frame beneath it — measured at 134,178 of 134,178 expansion keys
///   churned by one blank line (`docs/journal.md`, "The fifth field works…").
///   The first-interned site wins, which is the reference's own tie-break:
///   `Map.union` is left-biased, so a template used at three sites reports at
///   the first in program order (`Template.hs:88`).
///
/// One stated truncation: the reference's push is `n' <> n` with the deciding
/// site's *entire* `Src` — head and tail — appended below (`Desugar.hs:92-94`,
/// `Parser/Syntax.hs:623-626`), so a site that is itself inside an instance
/// carries its own chain into the note. This frame keeps only the site's head;
/// the site's `Expansion` joins the rendering half on the day a `while
/// compiling` renderer exists to read it — it has no caller today, and this
/// repo has shipped enough callerless fields.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct ExpansionData {
    /// Where the instance was decided — the *first* `Foo<...>` in program order
    /// (`Map.union` is left-biased, `Template.hs:88`). **Not part of the
    /// interned identity** — see the type-level comment.
    pub site: FileRange,
    pub instance: InstanceId,
    /// The rest of the stack, [`Expansion::ROOT`] when this is the outermost
    /// frame.
    pub parent: Expansion,
    /// One iteration of an unrolled `static for` (`Constexpr.hs:88-99`), not
    /// an instance: each iteration is its own copy of the loop body — a
    /// decision about a mention inside it is a decision about that
    /// iteration's mention — but the reference notes no frame for it, so no
    /// `while compiling` is rendered ([`ExpansionInterner::copy_frames`]).
    pub iteration: bool,
    /// The reference's round that decided the instance — the iteration of
    /// `Template.hs:58-90` whose `decidedInstances` held it — which a
    /// mention inside the copy is decided a round after at the earliest.
    /// Ordering data, like `site`: **not part of the interned identity**,
    /// and the first intern's wins. 0 for a `static for` iteration.
    pub round: u32,
}

/// The append-only intern table for [`Expansion`].
///
/// Same shape and locking rationale as [`crate::Interner`]: ids are never
/// reused, so an `Expansion` obtained at any point stays resolvable for the
/// lifetime of the interner, and frames are pushed only by instantiation —
/// never on a per-token hot path.
#[derive(Debug, Default)]
pub struct ExpansionInterner {
    inner: Mutex<ExpansionTable>,
}

#[derive(Debug, Default)]
struct ExpansionTable {
    /// Index `n` holds the frame for `Expansion(n + 1)`; `Expansion(0)` is
    /// [`Expansion::ROOT`] and has no frame.
    frames: Vec<ExpansionData>,
    /// Keyed on `(instance, parent)` **only** — never on the frame's `site`,
    /// which is a range and would churn under any edit above it. See
    /// [`ExpansionData`].
    map: FxHashMap<(InstanceId, Expansion), Expansion>,
}

impl ExpansionInterner {
    pub fn new() -> ExpansionInterner {
        ExpansionInterner::default()
    }

    pub fn intern(&self, data: ExpansionData) -> Expansion {
        let mut inner = self.inner.lock().unwrap();
        if let Some(&id) = inner.map.get(&(data.instance, data.parent)) {
            // The stored frame keeps its `site`: first intern wins, the way
            // `Map.union`'s left bias keeps the first occurrence in program
            // order (`Template.hs:88`).
            return id;
        }
        let id = Expansion(inner.frames.len() as u32 + 1);
        inner.frames.push(data);
        inner.map.insert((data.instance, data.parent), id);
        id
    }

    /// The expansion [`ExpansionInterner::intern`] gave `instance` under
    /// `parent`, if it gave one — interning nothing.
    pub fn find(&self, instance: InstanceId, parent: Expansion) -> Option<Expansion> {
        self.inner.lock().unwrap().map.get(&(instance, parent)).copied()
    }

    /// `None` exactly for [`Expansion::ROOT`].
    pub fn frame(&self, id: Expansion) -> Option<ExpansionData> {
        if id == Expansion::ROOT {
            return None;
        }
        let inner = self.inner.lock().unwrap();
        Some(*inner.frames.get(id.0 as usize - 1).expect("expansion from a different interner"))
    }

    /// The stack outermost-last, the order `expandErrorStack` renders it
    /// (`Error.hs:186-195`).
    pub fn frames(&self, mut id: Expansion) -> Vec<ExpansionData> {
        let mut out = Vec::new();
        while let Some(frame) = self.frame(id) {
            out.push(frame);
            id = frame.parent;
        }
        out
    }

    /// [`ExpansionInterner::frames`] without the `static for` iterations —
    /// the instances a node was copied through, which are the `while
    /// compiling` notes a diagnostic inside it renders.
    pub fn copy_frames(&self, id: Expansion) -> Vec<ExpansionData> {
        self.frames(id).into_iter().filter(|f| !f.iteration).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileId, TextRange};

    fn range(file: u32, start: u32, end: u32) -> FileRange {
        FileRange::new(FileId(file), TextRange::new(start.into(), end.into()))
    }

    #[test]
    fn an_instantiated_copy_keeps_its_own_head_and_stays_own() {
        let interner = ExpansionInterner::new();
        let body = Src::own(range(1, 10, 20));
        let site = range(2, 100, 130);
        let copy = Src::expanded(body, site, InstanceId(0), 1, &interner);
        // `n' <> n` keeps the copied node's own note on top
        // (`Desugar.hs:92-94`, `Parser/Syntax.hs:623-626`).
        assert_eq!(copy.head, body.head);
        assert_eq!(copy.written, Written::Own);
        let frames = interner.frames(copy.via);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].site, site);
    }

    #[test]
    fn a_donated_src_is_never_own() {
        let donor = Src::own(range(1, 10, 20));
        let copy = Src::donated(donor);
        assert_eq!(copy.head, donor.head);
        assert_eq!(copy.written, Written::Donated);
        assert_eq!(copy.via, Expansion::ROOT);
    }

    #[test]
    fn donation_inside_an_instance_keeps_the_chain() {
        let interner = ExpansionInterner::new();
        let body = Src::own(range(1, 10, 20));
        let copy = Src::expanded(body, range(2, 0, 5), InstanceId(3), 1, &interner);
        let synthesized = Src::donated(copy);
        assert_eq!(synthesized.via, copy.via, "a donor's tail travels with its head");
    }

    #[test]
    fn nested_expansion_builds_the_stack_outermost_last() {
        let interner = ExpansionInterner::new();
        let body = Src::own(range(1, 10, 20));
        let inner = Src::expanded(body, range(2, 0, 5), InstanceId(0), 1, &interner);
        let outer = Src::expanded(inner, range(3, 50, 60), InstanceId(1), 2, &interner);
        let frames = interner.frames(outer.via);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].site, range(3, 50, 60));
        assert_eq!(frames[1].site, range(2, 0, 5));
    }

    #[test]
    fn equal_frames_intern_to_equal_expansions() {
        let interner = ExpansionInterner::new();
        let data = ExpansionData {
            site: range(1, 0, 4),
            instance: InstanceId(7),
            parent: Expansion::ROOT,
            iteration: false,
            round: 1,
        };
        assert_eq!(interner.intern(data), interner.intern(data));
        assert_ne!(
            interner.intern(data),
            interner.intern(ExpansionData { instance: InstanceId(8), ..data })
        );
    }

    /// The split's pin: `site` is rendering data, not identity — a frame
    /// re-interned after an edit moved its site is the *same* expansion, and
    /// the stored site stays the first one (`Map.union`'s left bias,
    /// `Template.hs:88`). Keying the map on the whole frame instead turns
    /// this red, which is exactly the 134,178-key churn the split removes.
    #[test]
    fn an_edit_above_the_site_does_not_change_the_expansion() {
        let interner = ExpansionInterner::new();
        let before = ExpansionData {
            site: range(2, 100, 130),
            instance: InstanceId(7),
            parent: Expansion::ROOT,
            iteration: false,
            round: 1,
        };
        let after = ExpansionData { site: range(2, 101, 131), ..before };
        let id = interner.intern(before);
        assert_eq!(interner.intern(after), id);
        assert_eq!(interner.frame(id).unwrap().site, before.site, "first site wins");
    }

    /// Content-addressing: the id is a function of `(template, argument key)`
    /// and nothing else — same pair, same id, in any order, from any process.
    #[test]
    fn instance_ids_are_content_addressed() {
        let a = InstanceId::of("Holder", "<uint8>");
        assert_eq!(a, InstanceId::of("Holder", "<uint8>"));
        assert_ne!(a, InstanceId::of("Holder", "<uint16>"));
        assert_ne!(a, InstanceId::of("Box", "<uint8>"));
        // The separator byte keeps the pair unambiguous: ("ab", "c") is not
        // ("a", "bc").
        assert_ne!(InstanceId::of("ab", "c"), InstanceId::of("a", "bc"));
    }
}
