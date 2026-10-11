//! A set of [`SyntaxKind`]s, as a bitset.
//!
//! Recovery sets are consulted on every error path and follow sets on every
//! list element, so membership must be a couple of instructions, and the sets
//! must be constructible in `const` context to live as `static`s in the grammar.

use crate::SyntaxKind;

const WORDS: usize = (SyntaxKind::__LAST as usize).div_ceil(64);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TokenSet([u64; WORDS]);

impl TokenSet {
    pub const EMPTY: TokenSet = TokenSet([0; WORDS]);

    pub const fn new(kinds: &[SyntaxKind]) -> TokenSet {
        let mut words = [0u64; WORDS];
        let mut i = 0;
        while i < kinds.len() {
            let bit = kinds[i] as usize;
            words[bit / 64] |= 1 << (bit % 64);
            i += 1;
        }
        TokenSet(words)
    }

    pub const fn union(self, other: TokenSet) -> TokenSet {
        let mut words = [0u64; WORDS];
        let mut i = 0;
        while i < WORDS {
            words[i] = self.0[i] | other.0[i];
            i += 1;
        }
        TokenSet(words)
    }

    #[inline]
    pub const fn contains(&self, kind: SyntaxKind) -> bool {
        let bit = kind as usize;
        self.0[bit / 64] & (1 << (bit % 64)) != 0
    }
}

impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenSet(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SyntaxKind::*;

    #[test]
    fn membership() {
        const SET: TokenSet = TokenSet::new(&[SEMICOLON, R_CURLY, CLASS_KW]);
        assert!(SET.contains(SEMICOLON));
        assert!(SET.contains(CLASS_KW));
        assert!(!SET.contains(COMMA));
        assert!(!TokenSet::EMPTY.contains(SEMICOLON));
    }

    #[test]
    fn covers_every_kind() {
        // Guards against a future kind falling off the end of the bitset.
        for raw in 0..SyntaxKind::__LAST as u16 {
            let kind = SyntaxKind::try_from(raw).unwrap();
            assert!(TokenSet::new(&[kind]).contains(kind), "{kind:?} not representable");
        }
    }
}
