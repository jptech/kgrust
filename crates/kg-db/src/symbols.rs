//! The whole-program symbol map — `getSymbols` (`Symbols.hs:26,37-71`).
//!
//! This is the query the seven unwired passes are waiting for. The reference
//! rebuilds it from scratch **at least fifteen times per compile**
//! (`docs/spec/resolve.md` §3.2 counts them: nine `withSymbols` sites in
//! `Frontend.hs`, three in `Template.hs`, one per `inferType` fixpoint round,
//! one per function during `moveFunctions`), which is an artefact of purity and
//! not a semantic requirement. Here it is built **once per program per config**
//! and handed out as an [`Arc`](std::sync::Arc).

use kg_base::{FxHashMap, Interner, SymbolPath};
use kg_resolve::{DefId, SymbolsConfig};

use crate::program::Program;

/// One build of `getSymbols'` — a config, the map, and the two facts every
/// caller asks of it.
///
/// Keyed by `getScope e ++ [getName e]` (`Symbols.hs:70-71`) with **first in
/// pre-order wins** on a duplicate key: a parent beats its descendants, and the
/// earlier file in the program's post-order-DFS file list beats the later one
/// (`Symbols.hs:41,44,50`, `docs/spec/resolve.md` §3.1).
#[derive(Clone, Debug)]
pub struct SymbolMap {
    config: SymbolsConfig,
    map: FxHashMap<SymbolPath, DefId>,
    has_error_symbol: bool,
}

impl SymbolMap {
    /// Which `getSymbols'` config this is.
    pub fn config(&self) -> SymbolsConfig {
        self.config
    }

    /// `lookupSymbolByName = HashMap.lookup` (`Symbols.hs:73`) — an exact
    /// qualified-key probe, no scope walk.
    pub fn get(&self, key: &SymbolPath) -> Option<DefId> {
        self.map.get(key).copied()
    }

    /// `lookupInScope` (`Symbols.hs:79-87`) — try `scope ++ name`, then drop the
    /// last scope component and retry, down to the empty scope.
    ///
    /// This is the *post-parse* analogue of the parser's scope stack and is what
    /// makes forward references inside a module work
    /// (`docs/spec/resolve.md` §3.3, §7.2). `scope` is the referring
    /// declaration's `declScopeName`, i.e. [`kg_resolve::Item::scope_name`].
    pub fn lookup_in_scope(&self, scope: &SymbolPath, name: &SymbolPath) -> Option<DefId> {
        kg_resolve::lookup_in_scope(&self.map, scope, name).copied()
    }

    /// `hasErrorSymbol = HashMap.member ["$errors"]` (`Symbols.hs:31-35`) — the
    /// pipeline's "did anything fail" flag, and the guard `pass`/`checkPar` uses
    /// to stop the remaining passes (`Frontend.hs:44-52`).
    pub fn has_error_symbol(&self) -> bool {
        self.has_error_symbol
    }

    /// Every key, for the passes that scan rather than probe — `undefinedSymbols`
    /// walks the whole map to build its "Perhaps you meant" hints
    /// (`Frontend.hs:135-155`).
    pub fn iter(&self) -> impl Iterator<Item = (&SymbolPath, DefId)> + '_ {
        self.map.iter().map(|(k, v)| (k, *v))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// `getSymbols' config` over a whole program (`Symbols.hs:37-41`).
pub(crate) fn build(program: &Program, config: SymbolsConfig, interner: &Interner) -> SymbolMap {
    let map = kg_resolve::global_symbols(config, program.trees(), interner);
    let errors = SymbolPath::single(interner.intern(kg_resolve::names::ERRORS));
    SymbolMap { config, has_error_symbol: map.contains_key(&errors), map }
}

