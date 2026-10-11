//! Foundational types shared by every `kgrust` crate.
//!
//! Nothing here knows about Kanagawa. The point is that `kg-parser`, `kg-syntax`,
//! `kg-diag` and the semantic crates can all talk about *positions in files* and
//! *interned names* without depending on each other.

pub mod cancel;
mod interner;
mod line_index;
mod name;
mod src;

pub use interner::{Interner, Symbol};
pub use line_index::{LineCol, LineIndex, WideEncoding, WideLineCol};
pub use name::{
    SymbolPath, is_instance_name, mangle_namespace, mentions_instance, restricted_damerau_levenshtein, unmangle_namespace,
};
pub use src::{Expansion, ExpansionData, ExpansionInterner, InstanceId, Src, Written};

pub use text_size::{TextLen, TextRange, TextSize};

pub type FxHashMap<K, V> = rustc_hash::FxHashMap<K, V>;
pub type FxHashSet<T> = rustc_hash::FxHashSet<T>;
pub type FxIndexMap<K, V> = indexmap::IndexMap<K, V, rustc_hash::FxBuildHasher>;
pub type FxIndexSet<T> = indexmap::IndexSet<T, rustc_hash::FxBuildHasher>;

pub use camino::{Utf8Path, Utf8PathBuf};
pub use smol_str::SmolStr;

/// The reference checkout every corpus test reads: `KANAGAWA_ROOT` if set,
/// else this workspace's `third_party/kanagawa` submodule. The gate scripts
/// export `KANAGAWA_ROOT` (`scripts/lib.sh`), so a worktree reads the main
/// checkout's submodule rather than its own, uninitialised one.
#[doc(hidden)]
pub fn reference_root() -> std::path::PathBuf {
    match std::env::var_os("KANAGAWA_ROOT") {
        Some(root) => root.into(),
        None => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/kanagawa"),
    }
}

/// For a test that needs the reference corpus ([`reference_root`]) or the
/// reference's build and did not find it. Under the
/// gates — `KG_REQUIRE_CORPUS=1`, which `scripts/lib.sh` sets — that is a
/// failure: a test graded against nothing is not a pass (sprint §2, "an
/// absent instrument is red"). Elsewhere, a checkout without the reference,
/// the test says what it skipped and returns.
#[doc(hidden)]
pub fn corpus_absent(what: &str) {
    if std::env::var_os("KG_REQUIRE_CORPUS").is_some_and(|v| !v.is_empty() && v != "0") {
        panic!("{what} — KG_REQUIRE_CORPUS is set, and a test that graded nothing does not pass");
    }
    eprintln!("{what}; skipped (KG_REQUIRE_CORPUS is unset)");
}

/// Identifies a source file within a [`crate`-external] file store.
///
/// Deliberately opaque and cheap: it is the key that flows through every query,
/// so it must be `Copy` and hashable without touching the filesystem.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FileId(pub u32);

impl FileId {
    /// The file used for text that has no on-disk home — command-line-generated
    /// synthetic modules (`.cmdargs`, `.options`) and LSP scratch buffers.
    pub const SYNTHETIC: FileId = FileId(u32::MAX);

    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A range of text tagged with the file it lives in.
///
/// Diagnostics and go-to-definition both need this; a bare [`TextRange`] is
/// meaningless once more than one file is in play.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileRange {
    pub file: FileId,
    pub range: TextRange,
}

impl FileRange {
    #[inline]
    pub fn new(file: FileId, range: TextRange) -> Self {
        FileRange { file, range }
    }
}
