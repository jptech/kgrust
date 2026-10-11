//! String interning.
//!
//! The reference compiler's `Name = Hashed String` exists purely to make
//! `HashMap<QualifiedName, _>` fast (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs`).
//! Interning gets the same win structurally: names become `Copy` 32-bit ids, so
//! symbol tables key on integers and comparison is a single `cmp`.

use std::sync::Mutex;

use crate::{FxIndexSet, SmolStr};

/// An interned string. Cheap to copy, compare, and hash.
///
/// A `Symbol` is only meaningful relative to the [`Interner`] that produced it.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol(u32);

impl Symbol {
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Debug for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Symbol({})", self.0)
    }
}

/// An append-only string interner.
///
/// Append-only is what makes it safe to hand out `Symbol`s freely: an id is
/// never reused, so a `Symbol` obtained at any point stays resolvable for the
/// lifetime of the interner.
#[derive(Debug, Default)]
pub struct Interner {
    // A `Mutex` rather than a lock-free structure because interning is not on a
    // hot path *per token* — the lexer interns identifiers only, and parsing a
    // file is already parallel at file granularity.
    inner: Mutex<FxIndexSet<SmolStr>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&self, text: &str) -> Symbol {
        let mut inner = self.inner.lock().unwrap();
        if let Some(idx) = inner.get_index_of(text) {
            return Symbol(idx as u32);
        }
        let (idx, _) = inner.insert_full(SmolStr::new(text));
        Symbol(idx as u32)
    }

    /// The symbol `text` is interned as, if it is — without interning it,
    /// so asking leaves every later symbol's number as it would have been.
    pub fn get(&self, text: &str) -> Option<Symbol> {
        self.inner.lock().unwrap().get_index_of(text).map(|idx| Symbol(idx as u32))
    }

    pub fn resolve(&self, sym: Symbol) -> SmolStr {
        let inner = self.inner.lock().unwrap();
        inner
            .get_index(sym.index())
            .expect("symbol from a different interner")
            .clone()
    }

    /// The text behind a symbol's number as `Debug` prints it
    /// (`Symbol(12)`) — for turning a dump that numbers symbols into one two
    /// interners agree on.
    pub fn resolve_index(&self, index: usize) -> Option<SmolStr> {
        self.inner.lock().unwrap().get_index(index).cloned()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interns_equal_strings_to_equal_symbols() {
        let i = Interner::new();
        assert_eq!(i.intern("uint32"), i.intern("uint32"));
        assert_ne!(i.intern("uint32"), i.intern("uint64"));
        assert_eq!(i.resolve(i.intern("uint32")), "uint32");
        assert_eq!(i.len(), 2);
    }

    /// `get` finds what was interned and interns nothing itself.
    #[test]
    fn get_finds_without_interning() {
        let i = Interner::new();
        let a = i.intern("assert");
        assert_eq!(i.get("assert"), Some(a));
        assert_eq!(i.get("__print"), None);
        assert_eq!(i.len(), 1);
    }
}
