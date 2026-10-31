//! The language a file is written in: its edition and the features it opts
//! into (`docs/editions/README.md`).
//!
//! Edition `1.0` is the reference — upstream Kanagawa at `reference.lock`'s
//! pin — and every file is in it unless its own package's manifest says
//! otherwise. A later edition adds features; a feature only ever gives a
//! meaning to a program the reference rejects (`docs/plans/editions.md`
//! §1), so with [`Features::NONE`] no new branch is reachable anywhere.
//!
//! Here, rather than in `kg-config`, because the parser reads it and the
//! parser depends on nothing above this crate.

use std::fmt;

/// One extension of the language beyond the reference.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Feature {
    /// `f(x: 1, y: 2)`: arguments named by their parameters, in any order
    /// (`docs/editions/1.1.md`).
    NamedArguments,
    /// `f(a, b,)`: a comma before the closing bracket of an argument,
    /// parameter, template or initializer list (`docs/editions/1.1.md`).
    TrailingCommas,
    /// `uint8 f(uint8 x, uint8 shift = 0)`: a parameter's default, which a
    /// call may then leave out (`docs/editions/1.1.md`).
    DefaultArguments,
}

impl Feature {
    /// Every feature, in the order they were added.
    pub const ALL: [Feature; 3] = [Feature::NamedArguments, Feature::TrailingCommas, Feature::DefaultArguments];

    /// The spelling a manifest's `[language] features` and `--feature` use.
    pub fn name(self) -> &'static str {
        match self {
            Feature::NamedArguments => "named-arguments",
            Feature::TrailingCommas => "trailing-commas",
            Feature::DefaultArguments => "default-arguments",
        }
    }

    pub fn from_name(name: &str) -> Option<Feature> {
        Feature::ALL.into_iter().find(|f| f.name() == name)
    }

    fn bit(self) -> u32 {
        1 << self as u32
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A set of [`Feature`]s: what a file's parse and lowering may accept.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Features(u32);

impl Features {
    /// The reference: nothing beyond it.
    pub const NONE: Features = Features(0);

    pub fn has(self, feature: Feature) -> bool {
        self.0 & feature.bit() != 0
    }

    #[must_use]
    pub fn with(self, feature: Feature) -> Features {
        Features(self.0 | feature.bit())
    }

    #[must_use]
    pub fn union(self, other: Features) -> Features {
        Features(self.0 | other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Feature> {
        Feature::ALL.into_iter().filter(move |&f| self.has(f))
    }

    /// Every feature this build knows: what the `editions` gate turns on to
    /// prove that turning them on changes no program the reference accepts.
    pub fn all() -> Features {
        Feature::ALL.into_iter().fold(Features::NONE, Features::with)
    }
}

impl FromIterator<Feature> for Features {
    fn from_iter<I: IntoIterator<Item = Feature>>(iter: I) -> Features {
        iter.into_iter().fold(Features::NONE, Features::with)
    }
}

impl fmt::Debug for Features {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// A frozen bundle of features, versioned: `1.0` is the reference. A minor
/// version only adds meaning to programs the edition before it rejects; a
/// major version may also reject what an earlier one accepts — stricter
/// checks — and is the only kind that may (`docs/editions/README.md`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Edition {
    /// Upstream Kanagawa at `reference.lock`'s pin.
    #[default]
    V1_0,
    /// `1.0` and call sites that say what they mean: named arguments,
    /// trailing commas and default arguments (`docs/editions/1.1.md`). A preview until a release freezes it.
    V1_1,
}

impl Edition {
    /// Every edition, oldest first.
    pub const ALL: [Edition; 2] = [Edition::V1_0, Edition::V1_1];

    /// The newest edition this build knows.
    pub const LATEST: Edition = Edition::V1_1;

    pub fn name(self) -> &'static str {
        match self {
            Edition::V1_0 => "1.0",
            Edition::V1_1 => "1.1",
        }
    }

    pub fn from_name(name: &str) -> Option<Edition> {
        Edition::ALL.into_iter().find(|e| e.name() == name)
    }

    /// What the edition turns on, its predecessors' features included.
    pub fn features(self) -> Features {
        match self {
            Edition::V1_0 => Features::NONE,
            Edition::V1_1 => [Feature::NamedArguments, Feature::TrailingCommas, Feature::DefaultArguments]
            .into_iter()
            .collect(),
        }
    }

    /// Whether the edition may still change: one no release has frozen.
    pub fn is_preview(self) -> bool {
        matches!(self, Edition::V1_1)
    }
}

impl fmt::Display for Edition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A file's language: its edition, and the features named beside it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Language {
    pub edition: Edition,
    /// Features named on their own (`[language] features`, `--feature`),
    /// beyond the edition's.
    pub extra: Features,
}

impl Language {
    /// The reference: edition `1.0`, nothing beyond it.
    pub const REFERENCE: Language = Language { edition: Edition::V1_0, extra: Features::NONE };

    /// Everything the file may use.
    pub fn features(self) -> Features {
        self.edition.features().union(self.extra)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_turns_nothing_on() {
        assert!(Language::REFERENCE.features().is_empty());
        assert!(Edition::default().features().is_empty());
        assert_eq!(Language::default(), Language::REFERENCE);
    }

    #[test]
    fn names_round_trip() {
        for e in Edition::ALL {
            assert_eq!(Edition::from_name(e.name()), Some(e));
        }
        for f in Feature::ALL {
            assert_eq!(Feature::from_name(f.name()), Some(f));
        }
        assert_eq!(Edition::from_name("2.0"), None);
        assert_eq!(Feature::from_name("named_arguments"), None);
    }

    #[test]
    fn each_edition_holds_its_predecessors() {
        for pair in Edition::ALL.windows(2) {
            let (old, new) = (pair[0].features(), pair[1].features());
            assert_eq!(old.union(new), new, "{} drops a feature of {}", pair[1], pair[0]);
        }
    }

    #[test]
    fn a_feature_named_beside_an_edition_is_added() {
        let l = Language { edition: Edition::V1_0, extra: Features::NONE.with(Feature::NamedArguments) };
        assert!(l.features().has(Feature::NamedArguments));
        assert_eq!(Features::all().iter().count(), Feature::ALL.len());
    }
}
