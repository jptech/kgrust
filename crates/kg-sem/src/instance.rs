//! An instance as a message spells it — `take<uint8>`, `Box<4>::get<uint3>`
//! for one made inside another (`unmangleInstanceName`, `Type.hs:176-177`)
//! — from the `TInstance` the loop recorded as it made each copy
//! ([`Elaborated::copies`]), never from a mangled name. One owner for the
//! editor's hover lists (`kg_db::looped::Looped`) and the `while compiling`
//! notes [`crate::check`] labels a copy's diagnostic with.

use kg_base::{Expansion, ExpansionInterner, FxHashMap, Interner, Symbol};
use kg_ty::Ty;

use crate::elaborate::Elaborated;
use crate::symbols::Symbols;

/// `lookupSymbolByName symbols` as `unmangle` asks it (`Type.hs:198-205`):
/// the note at the declaration `name` names in the final program — a
/// copy's `TInstance` — or `None`.
pub fn noted(e: &Elaborated, symbols: &Symbols, name: &[Symbol]) -> Option<Ty> {
    symbols.noted(name, |d| {
        let m = e.modules.get(d.module)?;
        m.types.is_set(d.id).then(|| m.types.get(d.id))
    })
}

/// The instance a `TInstance` is, spelled as a message spells it:
/// `take<uint8>`.
pub fn spelled(e: &Elaborated, symbols: &Symbols, ty: &Ty, interner: &Interner) -> Option<String> {
    let declared = |name: &[Symbol]| noted(e, symbols, name);
    kg_ty::subtype::unmangle_instance_name_with(ty, Some(interner), &declared)
}

/// Every copy's instance spelled, by the expansion its nodes carry.
pub fn spelled_copies(e: &Elaborated, symbols: &Symbols, interner: &Interner) -> FxHashMap<Expansion, String> {
    e.copies.iter().filter_map(|(&via, ty)| Some((via, spelled(e, symbols, ty, interner)?))).collect()
}

/// The instance `via` compiles, outermost first — `Box<4>::get<uint3>` —
/// from `named` ([`spelled_copies`]): a `static for` iteration's frame
/// names nothing (`ExpansionInterner::copy_frames`), and a frame `named`
/// does not hold is left out rather than guessed. Empty where none is
/// named.
pub fn chain(expansions: &ExpansionInterner, named: &FxHashMap<Expansion, String>, via: Expansion) -> String {
    let mut out = Vec::new();
    let mut at = via;
    while let Some(frame) = expansions.frame(at) {
        if !frame.iteration
            && let Some(name) = named.get(&at)
        {
            out.push(name.as_str());
        }
        at = frame.parent;
    }
    out.reverse();
    out.join("::")
}
