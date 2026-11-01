//! Turning a [`Manifest`] into the settings the compiler front doors actually consume.

use camino::{Utf8Path, Utf8PathBuf};
use kg_diag::{WarningKind, WarningSet};

use crate::{Fmt, LanguageMap, Level, Lsp, Manifest};

/// Fully resolved project settings.
/// `targetDevice = "mock"` (`Parser/Options.hs:47`, `app/Options/CmdArgs.hs:87`).
///
/// Passed on no command line in the reference corpus, which is exactly what
/// makes it easy to miss.
pub const DEFAULT_TARGET_DEVICE: &str = "mock";

#[derive(Debug, Clone)]
pub struct Settings {
    /// Directory containing `kanagawa.toml`; all relative paths resolve against it.
    pub root: Utf8PathBuf,
    /// Import directories in declaration order, before device expansion.
    pub import_dirs: Vec<Utf8PathBuf>,
    /// Not `Option`: the reference has no "no device" state.
    /// `targetDevice :: String` with `= "mock"` (`Parser/Options.hs:29,47`), and
    /// the search path prepends `<dir>/device/<target>` unconditionally
    /// (`Parser/IO.hs:110`). Modelling the absent case cost 2,088 false
    /// conformance failures once already, and would silently hide
    /// `library/device/mock/hardware/config.k` from any project whose manifest
    /// simply does not mention a device.
    pub target_device: String,
    pub base_library: Option<Utf8PathBuf>,
    pub implicit_base: bool,
    pub parse_docs: bool,
    pub defines: Vec<(String, String)>,
    pub using: Vec<(String, String)>,
    pub warnings: WarningSet,
    pub warnings_as_errors: bool,
    pub fmt: Fmt,
    pub lsp: Lsp,
    /// The five backend options the synthetic `.options` module is built
    /// from (`buildOptionsSpecialModule`, `Parser/IO.hs:57-65`).
    pub backend: BackendOptions,
    /// The port's own verdict switches — [`Verdicts`].
    pub verdicts: Verdicts,
    /// Which language each file is in ([`LanguageMap`]): the reference
    /// everywhere unless a package's manifest, or `--edition`, says
    /// otherwise.
    pub language: LanguageMap,
}

/// Switches that change what this compiler decides or emits and are not the
/// language's: instruments and fall-backs, for measuring the port against
/// itself. Settings rather than environment reads so that every memo keyed
/// on the settings sees them — a change is a new settings revision, and the
/// program and everything elaborated from it is rebuilt (sprint §2, W8.2).
///
/// Each has an environment variable, read once, at a front door — `kg`,
/// `kanagawa`, `kg-lsp` ([`Verdicts::read`]); nothing inside the compiler
/// reads the environment for them. The default is the compiler as shipped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdicts {
    /// Cleared by `KG_LEFTOVERS=0`: the verdicts about what instantiation
    /// left undone, on the loop's copies (`kg_sem::check`) — pass 12's
    /// inferential arms and a `static if` left standing in a copy. Off until
    /// the loop's parity on whole programs, since a leftover of the loop's
    /// own is not one of the reference's; on since 2026-09-27 (sprint W1.4),
    /// when turning them on changed no verdict of `conformance`, `--strict`,
    /// `errors` or `backend`'s corpus roots. Cleared, the reading to fall
    /// back to if a leftover of the loop's own appears again.
    pub leftovers: bool,
    /// `KG_INFER_FULL` (set): inference's fixpoint walked whole every
    /// iteration, the incremental walk's reference
    /// (`docs/plans/performance.md` §3.6). The same results by design —
    /// `cargo xtask infer-identity` holds it — at the whole walk's cost.
    pub infer_full: bool,
    /// Cleared by `KG_PTBRIDGE_SEED=0`: the backend walk leaves out the
    /// parser's redundant-import seed (`checkRedundantImports`,
    /// `Parser.hs:234-241`) — the reading to fall back to if the loader's
    /// redundant imports ever disagree with the reference again (journal,
    /// 2026-09-17).
    pub ptbridge_seed: bool,
}

impl Default for Verdicts {
    fn default() -> Verdicts {
        Verdicts { leftovers: true, infer_full: false, ptbridge_seed: true }
    }
}

impl Verdicts {
    /// The switches as the environment `var` spells them — a front door
    /// passes `|name| std::env::var_os(name)`. Set means any value, empty
    /// included, as the reads it replaces had it; the seed and the
    /// leftovers, on by default, are cleared by exactly `0`.
    pub fn read(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Verdicts {
        Verdicts {
            leftovers: var("KG_LEFTOVERS").is_none_or(|v| v != "0"),
            infer_full: var("KG_INFER_FULL").is_some(),
            ptbridge_seed: var("KG_PTBRIDGE_SEED").is_none_or(|v| v != "0"),
        }
    }
}

/// `backend`, `stall`, `optimize`, `max_threads_default`, `max_threads_limit`
/// as the `.options` module spells them, with the compiler's command-line
/// defaults (`Options/CmdArgs.hs`): a program reads `options::optimize` as
/// `2` under `kanagawa foo.k`, not as the parser library's `-1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendOptions {
    /// `"None"` or `"Sv"` — `show backend`.
    pub backend: String,
    pub stall: i64,
    pub optimize: i64,
    pub max_threads_default: i64,
    pub max_threads_limit: i64,
}

impl Default for BackendOptions {
    fn default() -> BackendOptions {
        BackendOptions {
            backend: "None".to_owned(),
            stall: 0,
            optimize: 2,
            max_threads_default: 512,
            max_threads_limit: 512,
        }
    }
}

impl Settings {
    /// Resolves a manifest rooted at the directory containing it.
    pub fn from_manifest(manifest: &Manifest, manifest_path: &Utf8Path) -> Settings {
        let root = manifest_path.parent().unwrap_or(Utf8Path::new(".")).to_owned();
        let p = &manifest.project;
        Settings {
            language: LanguageMap::new(&root, manifest.language()),
            import_dirs: p.import_dirs.iter().map(|d| join(&root, d)).collect(),
            target_device: p.target_device.clone().unwrap_or_else(|| DEFAULT_TARGET_DEVICE.to_owned()),
            base_library: p.base_library.as_ref().map(|b| join(&root, b)),
            implicit_base: p.implicit_base.unwrap_or(true),
            parse_docs: p.parse_docs,
            defines: p.defines.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            using: p.using.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            warnings: resolve_warnings(manifest),
            warnings_as_errors: manifest.warnings.error,
            fmt: manifest.fmt.clone(),
            lsp: manifest.lsp.clone(),
            root,
            backend: BackendOptions::default(),
            verdicts: Verdicts::default(),
        }
    }

    /// This installation's standard library: `../share/kanagawa/library`
    /// beside the running executable, where a release lays it out
    /// (`cargo xtask dist`). `None` in a build tree, which has none — so
    /// nothing a test or a gate runs ever finds one.
    pub fn bundled_library() -> Option<Utf8PathBuf> {
        let exe = Utf8PathBuf::from_path_buf(std::env::current_exe().ok()?.canonicalize().ok()?).ok()?;
        let library = exe.parent()?.join("../share/kanagawa/library");
        library.join("base.k").is_file().then(|| library.canonicalize_utf8().unwrap_or(library))
    }

    /// These settings, with [`Settings::bundled_library`] searched last
    /// where no import directory provides the base library (`base.k`): an
    /// installed `kg` checks a file outside any project, or a project naming
    /// only its own directories, against the library it shipped with, while
    /// a project that vendors a library keeps its own.
    pub fn or_bundled_library(mut self) -> Settings {
        if !self.import_dirs.iter().any(|d| d.join("base.k").is_file())
            && let Some(library) = Settings::bundled_library()
        {
            self.import_dirs.push(library);
        }
        self
    }

    /// Defaults for a directory with no manifest, so the language server still
    /// does something useful in a bare checkout.
    pub fn fallback(root: impl Into<Utf8PathBuf>) -> Settings {
        let root = root.into();
        Settings {
            language: LanguageMap::new(&root, kg_base::language::Language::REFERENCE),
            root,
            import_dirs: Vec::new(),
            target_device: DEFAULT_TARGET_DEVICE.to_owned(),
            base_library: None,
            implicit_base: true,
            parse_docs: false,
            defines: Vec::new(),
            using: Vec::new(),
            warnings: default_warnings(),
            warnings_as_errors: false,
            fmt: Fmt::default(),
            lsp: Lsp::default(),
            backend: BackendOptions::default(),
            verdicts: Verdicts::default(),
        }
    }

    /// The language the file at `path` is written in ([`LanguageMap`]).
    pub fn language_of(&self, path: &Utf8Path) -> kg_base::language::Language {
        self.language.language_of(path)
    }

    /// The import search path, in the exact order the reference searches it.
    ///
    /// `importDirs = map (</> "device" </> targetDevice opt) (importDir opt) ++ importDir opt`
    /// (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:110`) — every
    /// device-specific directory comes before every plain one, not interleaved.
    /// Getting this backwards silently resolves the wrong `config.k`.
    pub fn import_search_path(&self) -> Vec<Utf8PathBuf> {
        let mut path = Vec::with_capacity(self.import_dirs.len() * 2);
        path.extend(
            self.import_dirs.iter().map(|d| d.join("device").join(&self.target_device)),
        );
        path.extend(self.import_dirs.iter().cloned());
        path
    }

    /// The source text of the synthetic `.cmdargs` module.
    ///
    /// Every file implicitly imports it, so a language server that does not
    /// synthesize it reports "unknown identifier" for every `--define`d name.
    pub fn cmdargs_source(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.using {
            out.push_str(&format!("using {k} = {v};\n"));
        }
        for (k, v) in &self.defines {
            out.push_str(&format!("const auto {k} = {v};\n"));
        }
        out
    }
}

fn join(root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() { path.to_owned() } else { root.join(path) }
}

/// The reference's built-in warning defaults.
///
/// `warnings_state`'s default value is
/// `[Enable RedundantImport, Enable Deprecated, Enable InitializerOverrides, Enable Shadow]`
/// (`compiler/hs/app/Options/CmdArgs.hs:111-117`), and command-line switches
/// *accumulate* on top of it — confirmed by `test/syntax/shadow-warning.k`,
/// which asserts ten `warning:shadow` results while passing only
/// `--Wno-redundant-import`.
fn default_warnings() -> WarningSet {
    let mut set = WarningSet::none();
    set.enable(WarningKind::RedundantImport);
    set.enable(WarningKind::Deprecated);
    set.enable(WarningKind::InitializerOverrides);
    set.enable(WarningKind::Shadow);
    set
}

fn resolve_warnings(manifest: &Manifest) -> WarningSet {
    let mut set = default_warnings();
    // `--Wall` adds exactly the two off-by-default kinds
    // (`Options/CmdArgs.hs:119-121`).
    if manifest.warnings.all {
        set.enable(WarningKind::Conversion);
        set.enable(WarningKind::TransactionSize);
    }
    for (name, level) in &manifest.warnings.kinds {
        let Some(kind) = WarningKind::from_name(name) else { continue };
        match level {
            Level::Allow => set.disable(kind),
            Level::Warn | Level::Deny => set.enable(kind),
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::parse;

    fn settings(text: &str) -> Settings {
        let m = parse(text, Utf8Path::new("/proj/kanagawa.toml")).unwrap();
        Settings::from_manifest(&m, Utf8Path::new("/proj/kanagawa.toml"))
    }

    /// The verdict switches read as the reads they replaced read them: set
    /// is any value, empty included; the seed and the leftovers, on by
    /// default, go only for exactly `0`.
    #[test]
    fn verdict_switches_read_as_they_always_did() {
        use std::ffi::OsString;
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v))
        };
        assert_eq!(Verdicts::read(env(&[])), Verdicts::default());
        assert_eq!(
            Verdicts::read(env(&[("KG_LEFTOVERS", "0"), ("KG_INFER_FULL", "0"), ("KG_PTBRIDGE_SEED", "0")])),
            Verdicts { leftovers: false, infer_full: true, ptbridge_seed: false }
        );
        assert!(Verdicts::read(env(&[("KG_LEFTOVERS", "1")])).leftovers);
        assert!(Verdicts::read(env(&[("KG_LEFTOVERS", "")])).leftovers);
        assert!(Verdicts::read(env(&[("KG_PTBRIDGE_SEED", "1")])).ptbridge_seed);
        assert!(Verdicts::read(env(&[("KG_PTBRIDGE_SEED", "")])).ptbridge_seed);
    }

    #[test]
    fn device_dirs_precede_every_plain_dir() {
        let s = settings(
            r#"
[project]
import-dirs = ["library", "vendor/lib"]
target-device = "mock"
"#,
        );
        let path: Vec<String> = s.import_search_path().iter().map(|p| p.to_string()).collect();
        assert_eq!(
            path,
            vec![
                "/proj/library/device/mock",
                "/proj/vendor/lib/device/mock",
                "/proj/library",
                "/proj/vendor/lib",
            ]
        );
    }

    /// An omitted `target-device` is `mock`, not "no device".
    ///
    /// This test used to assert the opposite. The reference's `targetDevice` is
    /// a `String` defaulting to `"mock"` (`Parser/Options.hs:29,47`) and
    /// `Parser/IO.hs:110` prepends `<dir>/device/<target>` unconditionally, so
    /// there is no state in which the device directories are absent — and a
    /// manifest that does not mention a device is the common case, not an
    /// exotic one.
    #[test]
    fn an_omitted_target_device_still_searches_the_default_device_dir() {
        let s = settings("[project]\nimport-dirs = [\"library\"]\n");
        assert_eq!(
            s.import_search_path(),
            vec![
                Utf8PathBuf::from("/proj/library/device/mock"),
                Utf8PathBuf::from("/proj/library"),
            ]
        );
    }

    #[test]
    fn cmdargs_module_emits_using_before_defines() {
        let s = settings(
            r#"
[project.defines]
mask = "0x1FFF"
[project.using]
WORD = "uint8"
"#,
        );
        assert_eq!(s.cmdargs_source(), "using WORD = uint8;\nconst auto mask = 0x1FFF;\n");
    }

    #[test]
    fn warning_defaults_match_the_reference() {
        let s = settings("");
        assert!(s.warnings.is_enabled(WarningKind::Shadow));
        assert!(s.warnings.is_enabled(WarningKind::Deprecated));
        assert!(s.warnings.is_enabled(WarningKind::RedundantImport));
        assert!(s.warnings.is_enabled(WarningKind::InitializerOverrides));
        assert!(!s.warnings.is_enabled(WarningKind::Conversion));
        assert!(!s.warnings.is_enabled(WarningKind::TransactionSize));
    }

    #[test]
    fn wall_adds_exactly_the_two_off_by_default_kinds() {
        let s = settings("[warnings]\nall = true\n");
        for kind in WarningKind::ALL {
            assert!(s.warnings.is_enabled(kind), "{kind:?} should be on under --Wall");
        }
    }

    #[test]
    fn an_explicit_allow_overrides_wall() {
        let s = settings("[warnings]\nall = true\nshadow = \"allow\"\n");
        assert!(!s.warnings.is_enabled(WarningKind::Shadow));
        assert!(s.warnings.is_enabled(WarningKind::Conversion));
    }
}
