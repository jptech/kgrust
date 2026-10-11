//! `kanagawa.toml` — the workspace manifest.
//!
//! The reference toolchain has no manifest at all: import directories, the
//! target device, defines and the base library live in `CMakeLists.txt` and
//! reach the compiler only as command-line flags. That is fine for a batch
//! compiler and fatal for a language server, which has no build to observe.
//! Without `--import-dir` and `--target-device` the synthetic `.cmdargs` module
//! is undefined and roughly half of `library/` fails to resolve.
//!
//! So this is not a convenience feature. It is the prerequisite for every
//! editor-facing capability in the toolchain.

mod discover;
mod resolve;

pub use discover::{ManifestError, discover, load};
pub use resolve::{BackendOptions, DEFAULT_TARGET_DEVICE, Settings, Verdicts};

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

/// A parsed `kanagawa.toml`, before profile selection and CLI overrides.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Manifest {
    #[serde(default)]
    pub package: Package,
    #[serde(default)]
    pub project: Project,
    #[serde(default)]
    pub warnings: Warnings,
    #[serde(default)]
    pub fmt: Fmt,
    #[serde(default)]
    pub lsp: Lsp,
    /// The ~54 backend codegen knobs, kept as an open table so a new upstream
    /// flag does not require a schema change here.
    #[serde(default)]
    pub hw: BTreeMap<String, toml::Value>,
    /// Named overlays selected with `--profile`.
    #[serde(default)]
    pub profile: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Package {
    pub name: Option<String>,
    pub version: Option<String>,
    /// Reserved for a future language-version selector.
    pub edition: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Project {
    /// Entry points. Globs allowed.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    /// `--import-dir`, repeatable. **Order is significant**: the reference
    /// searches `<dir>/device/<target-device>` for every dir first, and only
    /// then the plain dirs (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:110`).
    #[serde(default)]
    pub import_dirs: Vec<Utf8PathBuf>,
    /// `--target-device`. Changes the import search path, so it is not merely
    /// a backend setting.
    pub target_device: Option<String>,
    /// `--base-library`.
    pub base_library: Option<Utf8PathBuf>,
    /// `false` is `--no-implicit-base`.
    pub implicit_base: Option<bool>,
    /// `--parse-docs`.
    #[serde(default)]
    pub parse_docs: bool,
    /// `--define K=V`. Values are raw Kanagawa expression text; each becomes
    /// `const auto K = V;` in the synthetic `.cmdargs` module.
    #[serde(default)]
    pub defines: BTreeMap<String, String>,
    /// `--using K=V`. Each becomes `using K = V;` in `.cmdargs`.
    #[serde(default)]
    pub using: BTreeMap<String, String>,
}

/// Warning configuration.
///
/// Keys are the six wire names of
/// `compiler/hs/lib/Language/Kanagawa/Warning.hs:29-35`.
// `deny_unknown_fields` cannot coexist with `flatten`, and the flattened map is
// what lets `[warnings]` carry one key per warning kind. Unknown keys here are
// caught by `WarningKind::from_name` returning `None` instead.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Warnings {
    /// `--Wall`.
    #[serde(default)]
    pub all: bool,
    /// `--Werror`.
    #[serde(default)]
    pub error: bool,
    #[serde(flatten)]
    pub kinds: BTreeMap<String, Level>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Allow,
    Warn,
    Deny,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Fmt {
    pub line_width: usize,
    pub indent: usize,
    pub use_tabs: bool,
    pub newline: Newline,
    pub final_newline: bool,
    /// Keep import statements as one block, separated from the rest.
    pub group_imports: bool,
    /// Off by default: import order affects which definition wins, so sorting
    /// is not a safe default.
    pub sort_imports: bool,
}

impl Default for Fmt {
    fn default() -> Self {
        // Defaults measured from the 480-file corpus, not invented. See
        // `docs/spec/style.md`.
        Fmt {
            line_width: 100,
            indent: 4,
            use_tabs: false,
            newline: Newline::Lf,
            final_newline: true,
            group_imports: true,
            sort_imports: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Newline {
    #[default]
    Lf,
    Crlf,
}

impl Newline {
    pub fn as_str(self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::Crlf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Lsp {
    pub diagnostics: DiagnosticTiming,
    pub max_file_size_kb: u64,
    /// Index the import directories so goto-definition can reach `library/`.
    pub index_import_dirs: bool,
}

impl Default for Lsp {
    fn default() -> Self {
        Lsp { diagnostics: DiagnosticTiming::OnType, max_file_size_kb: 4096, index_import_dirs: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticTiming {
    #[default]
    OnType,
    OnSave,
}

/// A named overlay, deep-merged over the base tables.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Profile {
    #[serde(default)]
    pub project: Option<toml::Value>,
    #[serde(default)]
    pub warnings: Option<toml::Value>,
    #[serde(default)]
    pub fmt: Option<toml::Value>,
    #[serde(default)]
    pub hw: Option<toml::Value>,
}

/// The manifest file name, as searched for by [`discover`].
pub const MANIFEST_NAME: &str = "kanagawa.toml";
