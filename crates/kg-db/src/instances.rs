//! The template instances the program-wide loop made (`kg_sem::elaborate`,
//! `decidedInstances` round after round, `Template.hs:56-108`) — what `kg
//! dump instances` lists.
//!
//! Each copy `instantiate` made is recorded with the instance it is, the
//! `TInstance` `setTypeOfInstance` notes on it (`Template.hs:125`;
//! `kg_sem::Elaborated::copies`), and its expansion frame keeps where the
//! instance was first decided and in which round
//! (`kg_base::ExpansionData`). Nothing is read back off the tree, and
//! nothing here decides anything: the loop's census is the one census.
//!
//! Until sprint W3.7 this module was a second census, `decidedInstances`
//! re-read from the lowered IR and the CST engine's type table, graded
//! against a CST reading of `templateResolved` by `cargo xtask ir` — the
//! independent second reading `docs/plans/single-source-of-truth.md`'s
//! register kept until M6, the loop being the first.

use std::collections::BTreeMap;

use kg_base::{FileId, FileRange};

use crate::db::Db;
use crate::looped::Looped;

/// One instance the loop made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    /// As a `while compiling` note spells it: `Vec<uint8, 4>`, and
    /// `Box<4>::get<uint3>` for one made inside another's copy
    /// (`unmangleInstanceName`, `Type.hs:176-177`).
    pub spelled: String,
    /// The reference's round that decided it (`Template.hs:58-90`): 1 for an
    /// instance written in the program, more for one decided inside a copy.
    pub round: u32,
    /// Where it was first decided — the first `Foo<…>` in program order.
    pub site: FileRange,
}

/// The instances of the program rooted at `roots`, by template.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instances {
    /// Whether the program carries an error: the reference instantiates
    /// nothing then (`hasErrors'`, `Template.hs:55,84`), and the list is
    /// empty, though the loop — which the editor reads — made its copies.
    pub erred: bool,
    /// Each template by its declaration's dotted qualified name (`a.Vec`, a
    /// class template's member `a.Box.get`), its instances in the order the
    /// rounds decided them.
    pub templates: BTreeMap<String, Vec<Instance>>,
    /// The loop's instantiation rounds.
    pub rounds: u32,
}

impl Instances {
    /// How many distinct instances, every template's.
    pub fn total(&self) -> usize {
        self.templates.values().map(Vec::len).sum()
    }

    /// The last round that decided one.
    pub fn max_round(&self) -> u32 {
        self.templates.values().flatten().map(|i| i.round).max().unwrap_or(0)
    }
}

/// What the program-wide loop over `roots` instantiated
/// (`crate::Db::looped`), behind `crate::Db::check`'s errors.
pub fn instances(db: &Db, roots: &[FileId]) -> Instances {
    if db.check(roots).iter().any(kg_diag::Diagnostic::is_error) {
        return Instances { erred: true, ..Instances::default() };
    }
    let looped = db.looped(roots);
    let out = &looped.out;
    let mut templates: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
    for (&via, ty) in &out.copies {
        let Some(frame) = out.expansions.frame(via) else { continue };
        if frame.iteration {
            continue;
        }
        let kg_ty::Ty::Instance { template, .. } = ty else { continue };
        let spelled = looped.instance_chain(via, db.interner());
        if spelled.is_empty() {
            continue;
        }
        let list = templates.entry(declared(&looped, template.segments(), db.interner())).or_default();
        // One instance, however many copies of it nest under other frames.
        if !list.iter().any(|i| i.spelled == spelled) {
            list.push(Instance { spelled, round: frame.round, site: frame.site });
        }
    }
    for list in templates.values_mut() {
        list.sort_by(|a, b| (a.round, &a.spelled).cmp(&(b.round, &b.spelled)));
    }
    Instances { erred: false, templates, rounds: out.report.rounds }
}

/// The dotted name of the declaration `template` names. A member template
/// of a class template's instance is filed under the instance
/// (`Box<|$uint@32|>.get`), whose declaration is noted with its own
/// `TInstance` (a type's, `Type.hs:125`) — `Box<uint32>` of `Box` — so the member's declaration is
/// the class template's, `Box.get`: `unmangle`'s own lookup
/// (`lookupSymbolByName`, `Type.hs:198-205`), never the mangled name read
/// as text (`xtask lint`'s `mangled-text`).
fn declared(looped: &Looped, template: &[kg_base::Symbol], interner: &kg_base::Interner) -> String {
    if let Some((&last, scope)) = template.split_last()
        && let Some(noted) = looped.instance_named(scope, interner)
        && let kg_ty::Ty::Instance { template: outer, .. } = match noted {
            kg_ty::Ty::Type(inner) => (*inner).clone(),
            other => other,
        }
    {
        return format!("{}.{}", declared(looped, outer.segments(), interner), interner.resolve(last));
    }
    template.iter().map(|&s| kg_base::unmangle_namespace(&interner.resolve(s))).collect::<Vec<_>>().join(".")
}
