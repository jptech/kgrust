//! Finding and loading `kanagawa.toml`.

use camino::{Utf8Path, Utf8PathBuf};

use crate::{MANIFEST_NAME, Manifest};

#[derive(Debug)]
pub enum ManifestError {
    Io { path: Utf8PathBuf, source: std::io::Error },
    Parse { path: Utf8PathBuf, source: toml::de::Error },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io { path, source } => write!(f, "{path}: {source}"),
            // `toml`'s own rendering already carries line/column and a caret,
            // so a manifest typo reads like a compiler error rather than a
            // serde dump.
            ManifestError::Parse { path, source } => write!(f, "{path}:\n{source}"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl ManifestError {
    /// The byte range of the manifest's text at fault, where `toml` knows
    /// it — a syntax error, or a value its field refused, as `edition =
    /// "9.9"`: the language server marks it in the manifest itself.
    pub fn span(&self) -> Option<std::ops::Range<usize>> {
        match self {
            ManifestError::Io { .. } => None,
            ManifestError::Parse { source, .. } => source.span(),
        }
    }

    /// The problem alone, without the path or `toml`'s rendering of the
    /// line around it: for a diagnostic placed at [`span`](Self::span).
    pub fn message(&self) -> String {
        match self {
            ManifestError::Io { source, .. } => source.to_string(),
            ManifestError::Parse { source, .. } => source.message().to_owned(),
        }
    }
}

/// Walks up from `start` looking for the nearest `kanagawa.toml`.
///
/// This is how the language server answers "what are the import directories for
/// the file I was just asked about" with no build system in the loop.
pub fn discover(start: &Utf8Path) -> Option<Utf8PathBuf> {
    let mut dir = if start.is_dir() { Some(start) } else { start.parent() };
    while let Some(d) = dir {
        let candidate = d.join(MANIFEST_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Reads and parses a manifest.
pub fn load(path: &Utf8Path) -> Result<Manifest, ManifestError> {
    let text = std::fs::read_to_string(path)
        .map_err(|source| ManifestError::Io { path: path.to_owned(), source })?;
    parse(&text, path)
}

pub fn parse(text: &str, path: &Utf8Path) -> Result<Manifest, ManifestError> {
    toml::from_str(text).map_err(|source| ManifestError::Parse { path: path.to_owned(), source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_manifest() {
        let text = r#"
[package]
name = "my-design"

[project]
sources = ["src/top.k"]
import-dirs = ["library", "third_party/kanagawa-lib"]
target-device = "mock"
base-library = "library/base.k"
implicit-base = true
parse-docs = true

[project.defines]
verbose = "true"
mask = "0x1FFF"

[project.using]
COMPILE_PARAM_1 = "uint8"

[warnings]
all = true
error = true
shadow = "allow"

[fmt]
line-width = 120
indent = 4
use-tabs = false
newline = "lf"
final-newline = true
group-imports = true
sort-imports = false

[hw]
optimize = 2
frequency = 250

[profile.test]
hw = { optimize = 0 }
"#;
        let m = parse(text, Utf8Path::new("kanagawa.toml")).unwrap();
        assert_eq!(m.package.name.as_deref(), Some("my-design"));
        assert_eq!(m.project.import_dirs.len(), 2);
        assert_eq!(m.project.target_device.as_deref(), Some("mock"));
        assert_eq!(m.project.defines["mask"], "0x1FFF");
        assert!(m.warnings.all && m.warnings.error);
        assert_eq!(m.warnings.kinds["shadow"], crate::Level::Allow);
        assert_eq!(m.fmt.line_width, 120);
        assert_eq!(m.hw["frequency"].as_integer(), Some(250));
        assert!(m.profile.contains_key("test"));
    }

    /// A table that sets some keys takes the documented default for the
    /// rest (`docs/spec/cli.md` §10): `[fmt]` with only `line-width` was
    /// refused for want of `indent`, and `[lsp]` likewise.
    #[test]
    fn a_partial_table_takes_the_defaults_for_the_rest() {
        let text = "[fmt]\nline-width = 80\n\n[lsp]\ndiagnostics = \"on-save\"\n";
        let m = parse(text, Utf8Path::new("kanagawa.toml")).unwrap();
        assert_eq!((m.fmt.line_width, m.fmt.indent, m.fmt.final_newline), (80, 4, true));
        assert_eq!((m.lsp.diagnostics, m.lsp.max_file_size_kb), (crate::DiagnosticTiming::OnSave, 4096));
    }

    #[test]
    fn an_empty_manifest_is_all_defaults() {
        let m = parse("", Utf8Path::new("kanagawa.toml")).unwrap();
        assert!(m.project.import_dirs.is_empty());
        assert_eq!(m.fmt.indent, 4);
        assert!(!m.warnings.error);
    }

    #[test]
    fn a_typo_names_the_offending_key() {
        let err = parse("[projekt]\n", Utf8Path::new("kanagawa.toml")).unwrap_err();
        assert!(err.to_string().contains("projekt"), "{err}");
    }
}
