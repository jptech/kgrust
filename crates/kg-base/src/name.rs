//! Qualified names.
//!
//! Owner-topic: the one spelling of `QualifiedName` shared by `kg-resolve`
//! (which produces them) and `kg-ty` (which stores them inside `Ty::Struct`,
//! `Ty::Class`, `Ty::Enum`, `Ty::Reference`, `Ty::Closure` and `Ty::Instance`).
//! Neither of those crates depends on the other, so the type has to live here.
//!
//! Spec: `docs/spec/types.md` §14.2 (`pub type QualifiedName = kg_base::SymbolPath`),
//! `docs/spec/resolve.md` §3.1 (the global symbol map is keyed by one of these) and
//! §10 (the keying table).
//!
//! Reference: `type QualifiedName = [Name]`
//! (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:69`), where
//! `newtype Name = Name (Hashed String)` (`Parser/Syntax.hs:51`).
//!
//! # The one place this diverges from Haskell, and it matters
//!
//! `Hashed`'s `Ord` compares the *underlying string*, so Haskell's `Ord Name` is
//! lexicographic over characters and `Ord QualifiedName` is lexicographic over
//! those. A [`Symbol`] is an interner id, so [`SymbolPath`]'s `Ord` is
//! **interner-insertion order**, not string order. `Eq`, `Hash` and every
//! `HashMap`/`FxHashMap` keyed on a path are unaffected. Anything whose *output*
//! depends on iteration order of an ordered container keyed by a name — template
//! instance emission order, and therefore mangled-name suffixes
//! (`Template.hs:88-108`) — must sort by the resolved string, not by `Symbol`.

use std::sync::Arc;

use crate::Symbol;

/// A `::`-separated name: the Rust image of Haskell `QualifiedName = [Name]`.
///
/// Cheap to clone (one `Arc` bump) and cheap to compare when two paths share an
/// allocation. Module namespaces are a *single* mangled component (`@a@b@c`,
/// `Parser/Syntax.hs:273-274`), never several — see `docs/spec/resolve.md` §10.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolPath(Arc<[Symbol]>);

impl SymbolPath {
    /// The empty path — the qualifier of a file with no `module` declaration
    /// (`docs/spec/resolve.md` §10).
    pub fn empty() -> SymbolPath {
        SymbolPath(Arc::from([] as [Symbol; 0]))
    }

    pub fn single(name: Symbol) -> SymbolPath {
        SymbolPath(Arc::from([name]))
    }

    pub fn new(segments: impl IntoIterator<Item = Symbol>) -> SymbolPath {
        SymbolPath(segments.into_iter().collect::<Vec<_>>().into())
    }

    #[inline]
    pub fn segments(&self) -> &[Symbol] {
        &self.0
    }

    /// `scope ++ [name]` — the key shape `getSymbols` builds (`Symbols.hs:70-71`).
    pub fn child(&self, name: Symbol) -> SymbolPath {
        let mut v = Vec::with_capacity(self.0.len() + 1);
        v.extend_from_slice(&self.0);
        v.push(name);
        SymbolPath(v.into())
    }

    /// `a ++ b`.
    pub fn concat(&self, other: &SymbolPath) -> SymbolPath {
        let mut v = Vec::with_capacity(self.0.len() + other.0.len());
        v.extend_from_slice(&self.0);
        v.extend_from_slice(&other.0);
        SymbolPath(v.into())
    }

    /// Haskell `init` — everything but the last component. Empty paths stay empty
    /// (Haskell's `init []` throws; `lookupInScope` guards with `null s` first,
    /// `Symbols.hs:79-87`).
    pub fn init(&self) -> SymbolPath {
        match self.0.len() {
            0 => self.clone(),
            n => SymbolPath(self.0[..n - 1].into()),
        }
    }

    /// Haskell `tail` — everything but the first component.
    pub fn tail(&self) -> SymbolPath {
        match self.0.len() {
            0 => self.clone(),
            _ => SymbolPath(self.0[1..].into()),
        }
    }

    pub fn first(&self) -> Option<Symbol> {
        self.0.first().copied()
    }

    pub fn last(&self) -> Option<Symbol> {
        self.0.last().copied()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn starts_with(&self, prefix: &[Symbol]) -> bool {
        self.0.starts_with(prefix)
    }

    /// Haskell `stripPrefix`. Note `stripPrefix [] xs == Just xs`, which is what
    /// makes an *unaliased* import match every name (`SymbolTable.hs:276-278`).
    pub fn strip_prefix(&self, prefix: &[Symbol]) -> Option<SymbolPath> {
        self.0.strip_prefix(prefix).map(|rest| SymbolPath(rest.into()))
    }
}

/// A map keyed by paths is probed with a slice, no path built: the derived
/// `Hash` and `Eq` are the `Arc<[Symbol]>`'s, which are the slice's.
impl std::borrow::Borrow<[Symbol]> for SymbolPath {
    fn borrow(&self) -> &[Symbol] {
        &self.0
    }
}

impl std::ops::Deref for SymbolPath {
    type Target = [Symbol];

    #[inline]
    fn deref(&self) -> &[Symbol] {
        &self.0
    }
}

impl FromIterator<Symbol> for SymbolPath {
    fn from_iter<I: IntoIterator<Item = Symbol>>(iter: I) -> SymbolPath {
        SymbolPath::new(iter)
    }
}

impl From<Vec<Symbol>> for SymbolPath {
    fn from(v: Vec<Symbol>) -> SymbolPath {
        SymbolPath(v.into())
    }
}

impl From<&[Symbol]> for SymbolPath {
    fn from(v: &[Symbol]) -> SymbolPath {
        SymbolPath(v.into())
    }
}

impl Default for SymbolPath {
    fn default() -> SymbolPath {
        SymbolPath::empty()
    }
}

impl std::fmt::Debug for SymbolPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Interner;

    #[test]
    fn child_and_init_round_trip() {
        let i = Interner::new();
        let scope = SymbolPath::new([i.intern("@a@b"), i.intern("C")]);
        let key = scope.child(i.intern("m"));
        assert_eq!(key.len(), 3);
        assert_eq!(key.init(), scope);
        assert_eq!(key.last(), Some(i.intern("m")));
    }

    #[test]
    fn strip_empty_prefix_is_identity() {
        let i = Interner::new();
        let p = SymbolPath::single(i.intern("x"));
        assert_eq!(p.strip_prefix(&[]), Some(p.clone()));
    }

    #[test]
    fn init_of_empty_is_empty() {
        assert!(SymbolPath::empty().init().is_empty());
    }
}

/// `moduleNamespace` (`Parser/Syntax.hs:273-274`): a dotted module name as
/// the single mangled segment it is filed under — `a.b` is `@a@b`.
pub fn mangle_namespace(dotted: &str) -> String {
    let mut out = String::with_capacity(dotted.len() + 1);
    for part in dotted.split('.') {
        out.push('@');
        out.push_str(part);
    }
    out
}

/// The inverse of [`mangle_namespace`] — `@a@b` is `a.b` — and any segment
/// that is not a namespace (no leading `@`) as it is.
pub fn unmangle_namespace(segment: &str) -> String {
    match segment.strip_prefix('@') {
        Some(rest) => rest.replace('@', "."),
        None => segment.to_string(),
    }
}

#[cfg(test)]
mod namespace_tests {
    use super::*;

    /// The two spellings of a module namespace, and a plain segment left as
    /// it is — the conversion every consumer used to inline.
    #[test]
    fn a_namespace_mangles_and_unmangles() {
        assert_eq!(mangle_namespace("helper.function_template_param"), "@helper@function_template_param");
        assert_eq!(unmangle_namespace("@helper@function_template_param"), "helper.function_template_param");
        assert_eq!(unmangle_namespace("main"), "main");
        assert_eq!(unmangle_namespace(&mangle_namespace("a")), "a");
    }
}

/// Whether a name is an instance's — its template's name with the mangle
/// suffixed, `Foo<|$uint@32|>` (`suffixName`, `Mangle.hs:29`) — and not one
/// that goes on past it (`Do<|$uint@16|>@?lambda0`, a lambda hoisted out of
/// a copy).
pub fn is_instance_name(text: &str) -> bool {
    text.contains("<|") && text.ends_with("|>")
}

/// Whether a name mentions an instance anywhere — an instance's own, or one
/// hoisted out of a copy.
pub fn mentions_instance(text: &str) -> bool {
    text.contains("<|")
}

/// `restrictedDamerauLevenshteinDistance defaultEditCosts`
/// (`Text.EditDistance`) — optimal string alignment, i.e. Levenshtein plus
/// adjacent transposition, with no substring reused across two transpositions.
/// All four costs are 1 in `defaultEditCosts`.
pub fn restricted_damerau_levenshtein(a: &str, b: &str) -> u32 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m as u32;
    }
    if m == 0 {
        return n as u32;
    }
    let mut prev2: Vec<u32> = vec![0; m + 1];
    let mut prev: Vec<u32> = (0..=m as u32).collect();
    let mut cur: Vec<u32> = vec![0; m + 1];
    for i in 1..=n {
        cur[0] = i as u32;
        for j in 1..=m {
            let cost = u32::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                cur[j] = cur[j].min(prev2[j - 2] + 1);
            }
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

#[cfg(test)]
mod distance_tests {
    use super::restricted_damerau_levenshtein;

    #[test]
    fn optimal_string_alignment_distance() {
        assert_eq!(restricted_damerau_levenshtein("", ""), 0);
        assert_eq!(restricted_damerau_levenshtein("abc", "abc"), 0);
        assert_eq!(restricted_damerau_levenshtein("abc", "acb"), 1);
        assert_eq!(restricted_damerau_levenshtein("uint32", "unit32"), 1);
        assert_eq!(restricted_damerau_levenshtein("kitten", "sitting"), 3);
        assert_eq!(restricted_damerau_levenshtein("a", ""), 1);
        // Restricted: a substring may not be edited twice, so `CA` -> `ABC` is
        // 3 and not the unrestricted 2.
        assert_eq!(restricted_damerau_levenshtein("CA", "ABC"), 3);
    }
}
