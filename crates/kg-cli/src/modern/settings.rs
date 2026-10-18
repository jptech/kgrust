//! Where a setting comes from.
//!
//! ```text
//! built-in defaults  <  kanagawa.toml  <  [profile.<name>]  <  environment  <  flags
//! ```
//!
//! Scalars are replaced by each higher layer; **lists append** in that same
//! order, matching the legacy binary, where a repeated `--import-dir`
//! accumulates and a repeated `--reset-cycles` does not
//! (`docs/spec/cli.md` §8.5).
//!
//! The profile overlay is applied to the parsed TOML *document*, not to the
//! deserialized [`Manifest`], so a profile can set any subset of any table
//! without every field having to become an `Option`.

use camino::{Utf8Path, Utf8PathBuf};
use kg_config::{Manifest, Settings};
use kg_diag::WarningKind;

/// The `KG_*` environment layer.
///
/// Passed in rather than read from the process inside the resolver, so that
/// tests are order-independent and parallel-safe.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// `KG_PROFILE` — the profile to select when `--profile` is absent.
    pub profile: Option<String>,
    /// `KG_IMPORT_DIRS` — additional import directories, separated by `:`
    /// (`;` on Windows). Appended after the manifest's.
    pub import_dirs: Option<String>,
    /// `KG_TARGET_DEVICE`.
    pub target_device: Option<String>,
    /// `KG_BASE_LIBRARY`.
    pub base_library: Option<String>,
    /// `KG_COLOR` — `auto`, `always` or `never`; `--color` still wins.
    pub color: Option<String>,
    /// `KG_PLATFORM` — where a manifest's `[board]` is looked for when it
    /// names no `platform` (`kg_config::board`).
    pub platform: Option<String>,
    /// `KG_LEFTOVERS`, `KG_INFER_FULL`, `KG_PTBRIDGE_SEED` —
    /// the port's verdict switches ([`kg_config::Verdicts`]), read here and
    /// nowhere inside the compiler.
    pub verdicts: kg_config::Verdicts,
}

impl Env {
    pub fn from_process() -> Env {
        let get = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
        Env {
            profile: get("KG_PROFILE"),
            import_dirs: get("KG_IMPORT_DIRS"),
            target_device: get("KG_TARGET_DEVICE"),
            base_library: get("KG_BASE_LIBRARY"),
            color: get("KG_COLOR"),
            platform: get("KG_PLATFORM"),
            verdicts: crate::verdicts_from_env(),
        }
    }

    fn import_dir_list(&self) -> Vec<String> {
        let separator = if cfg!(windows) { ';' } else { ':' };
        self.import_dirs
            .iter()
            .flat_map(|v| v.split(separator))
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

/// Command-line overrides, in the order the user wrote them where that matters.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub import_dirs: Vec<String>,
    /// `NAME=VALUE` or `NAME#VALUE`, split like the legacy `--define`.
    pub defines: Vec<String>,
    pub using: Vec<String>,
    pub base_library: Option<String>,
    pub no_implicit_base: bool,
    pub target_device: Option<String>,
    pub parse_docs: bool,
    /// `-W <name>` / `-W no-<name>` / `-W all` / `-W error`, in order.
    pub warnings: Vec<String>,
}

/// The resolved configuration, plus enough provenance to explain it.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub settings: Settings,
    pub manifest_path: Option<Utf8PathBuf>,
    pub profile: Option<String>,
    /// `project.sources`, resolved against the manifest directory. Used when a
    /// command is given no paths.
    pub sources: Vec<Utf8PathBuf>,
    /// The manifest's `[board]`, found and applied to `settings`.
    pub board: Option<kg_config::Board>,
    /// The backend options, lowest layer first: the board's `[hw]`, then the
    /// manifest's (profile applied). `-Z hw.<key>=<v>` goes over both.
    pub hw: Vec<std::collections::BTreeMap<String, toml::Value>>,
}

/// Layers every source into one [`Settings`].
///
/// `start` is where manifest *discovery* begins, which is the directory holding
/// the first input file rather than the process's working directory. The
/// language server "discovers the manifest for every file it is asked about,
/// exactly as `kg build` would" (`kg-lsp`'s `ConfigStore`), and that claim was
/// false for `kg check` and `kg fmt` until this parameter existed: the editor
/// read the manifest beside the file while the CLI read the one beside the
/// shell, so the same file was checked under two configurations depending on
/// where the user happened to be standing. `cwd` still roots
/// [`Settings::fallback`], which is about where *relative* paths resolve and is
/// a different question.
pub fn resolve(
    manifest_path: Option<&str>,
    no_manifest: bool,
    profile: Option<&str>,
    env: &Env,
    overrides: &Overrides,
    cwd: &Utf8Path,
    start: &Utf8Path,
) -> Result<Resolved, String> {
    let profile = profile.map(str::to_owned).or_else(|| env.profile.clone());

    let found = if no_manifest {
        None
    } else {
        match manifest_path {
            // `-C` may name the file or the directory holding it, which is what
            // people type.
            Some(path) => {
                let path = Utf8PathBuf::from(path);
                let path = if path.is_dir() { path.join(kg_config::MANIFEST_NAME) } else { path };
                if !path.is_file() {
                    return Err(format!("no manifest at {path}"));
                }
                Some(path)
            }
            None => kg_config::discover(start),
        }
    };

    let (mut settings, sources, manifest) = match &found {
        Some(path) => {
            let manifest = load_with_profile(path, profile.as_deref())?;
            let root = path.parent().unwrap_or(Utf8Path::new(".")).to_owned();
            let sources =
                manifest.project.sources.iter().map(|s| join(&root, s)).collect::<Vec<_>>();
            (Settings::from_manifest(&manifest, path), sources, Some(manifest))
        }
        None => (Settings::fallback(cwd), Vec::new(), None),
    };

    apply_env(&mut settings, env);
    apply_overrides(&mut settings, overrides)?;
    let mut settings = settings.or_bundled_library();

    // After the bundled library, so the platform's follows every library
    // the project could otherwise see.
    let mut hw = Vec::new();
    let mut board = None;
    if let Some(manifest) = manifest {
        if let (Some(table), Some(path)) = (&manifest.board, &found) {
            let root = path.parent().unwrap_or(Utf8Path::new("."));
            let platform = env.platform.as_deref().map(Utf8Path::new);
            let found = kg_config::board::resolve(table, root, platform).map_err(|e| format!("{path}: {e}"))?;
            kg_config::board::apply(&mut settings, &found).map_err(|e| format!("{path}: {e}"))?;
            hw.push(found.hw());
            board = Some(found);
        }
        hw.push(manifest.hw);
    }

    Ok(Resolved { settings, manifest_path: found, profile, sources, board, hw })
}

/// Reads a manifest and deep-merges `[profile.<name>]` over the base tables.
fn load_with_profile(path: &Utf8Path, profile: Option<&str>) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    // `toml::from_str`, not `str::parse`: in toml 1.0 the `FromStr` impl on
    // `Value` parses a single value expression, while the serde entry point
    // parses a document.
    let mut document: toml::Value =
        toml::from_str(&text).map_err(|e| format!("{path}:\n{e}"))?;

    if let Some(name) = profile {
        let overlay = document
            .get("profile")
            .and_then(|p| p.get(name))
            .cloned()
            .ok_or_else(|| format!("{path}: no [profile.{name}]"))?;
        merge(&mut document, &overlay);
    }

    document.try_into().map_err(|e| format!("{path}:\n{e}"))
}

/// Deep merge: tables recurse, everything else is replaced wholesale.
fn merge(base: &mut toml::Value, overlay: &toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

/// Relative paths in a manifest resolve against the directory holding it, not
/// against the process CWD — a `kg check` from a subdirectory must mean the same
/// thing as one from the root.
fn join(root: &Utf8Path, path: &str) -> Utf8PathBuf {
    let path = Utf8PathBuf::from(path);
    if path.is_absolute() { path } else { root.join(path) }
}

fn apply_env(settings: &mut Settings, env: &Env) {
    settings.verdicts = env.verdicts;
    settings.import_dirs.extend(env.import_dir_list().into_iter().map(Utf8PathBuf::from));
    if let Some(device) = &env.target_device {
        settings.target_device = device.clone();
    }
    if let Some(base) = &env.base_library {
        settings.base_library = Some(Utf8PathBuf::from(base));
    }
}

fn apply_overrides(settings: &mut Settings, overrides: &Overrides) -> Result<(), String> {
    settings.import_dirs.extend(overrides.import_dirs.iter().map(Utf8PathBuf::from));
    for define in &overrides.defines {
        let (name, value) = crate::legacy::options::split_name_value(define);
        settings.defines.push((name.to_owned(), value.to_owned()));
    }
    for using in &overrides.using {
        let (name, value) = crate::legacy::options::split_name_value(using);
        settings.using.push((name.to_owned(), value.to_owned()));
    }
    if let Some(base) = &overrides.base_library {
        settings.base_library = Some(Utf8PathBuf::from(base));
    }
    if overrides.no_implicit_base {
        settings.implicit_base = false;
    }
    if let Some(device) = &overrides.target_device {
        settings.target_device = device.clone();
    }
    if overrides.parse_docs {
        settings.parse_docs = true;
    }

    // `-W` switches fold left over whatever the manifest resolved to, exactly
    // like the legacy `--W…` list (`Warning.hs:46-50`).
    for switch in &overrides.warnings {
        match switch.as_str() {
            "all" => {
                settings.warnings.enable(WarningKind::Conversion);
                settings.warnings.enable(WarningKind::TransactionSize);
            }
            "error" => settings.warnings_as_errors = true,
            name => {
                let (name, enable) = match name.strip_prefix("no-") {
                    Some(rest) if WarningKind::from_name(rest).is_some() => (rest, false),
                    _ => (name, true),
                };
                let kind = WarningKind::from_name(name).ok_or_else(|| {
                    let names: Vec<&str> = WarningKind::ALL.iter().map(|k| k.name()).collect();
                    format!("unknown warning `{name}`; known: all, error, {}", names.join(", "))
                })?;
                if enable {
                    settings.warnings.enable(kind);
                } else {
                    settings.warnings.disable(kind);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(Utf8PathBuf);

    impl Dir {
        fn new(name: &str, manifest: &str) -> Dir {
            let path = Utf8PathBuf::from_path_buf(std::env::temp_dir())
                .expect("a UTF-8 temp dir")
                .join(format!("kg-cli-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("kanagawa.toml"), manifest).unwrap();
            Dir(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const MANIFEST: &str = r#"
[project]
import-dirs = ["library"]
target-device = "mock"
parse-docs = false

[project.defines]
verbose = "true"

[warnings]
shadow = "warn"

[profile.test]
project = { target-device = "stratix-10-gx-2800" }
warnings = { all = true, shadow = "allow" }
"#;

    fn resolved(dir: &Dir, profile: Option<&str>, env: Env, overrides: Overrides) -> Resolved {
        resolve(None, false, profile, &env, &overrides, &dir.0, &dir.0).unwrap()
    }

    /// `[board]`: the board's device, the platform's library after the
    /// project's, and two `[hw]` layers — the board's (its clock as the
    /// frequency), then the manifest's under the profile. KG_PLATFORM finds
    /// a board the manifest names without a platform.
    #[test]
    fn a_board_sets_the_device_the_library_and_the_hw_layers() {
        let dir = Dir::new(
            "board",
            "[project]\nimport-dirs = [\"library\"]\n[board]\nname = \"demo\"\n[hw]\nregister-ratio = 4\n\
             [profile.fast]\nhw = { register-ratio = 2 }\n",
        );
        let platform = dir.0.join("platform");
        std::fs::create_dir_all(platform.join("library")).unwrap();
        std::fs::create_dir_all(platform.join("boards/demo")).unwrap();
        std::fs::write(
            platform.join("boards/demo/board.toml"),
            "[fpga]\ndevice = \"ice40-up5k\"\npart = \"up5k\"\nhal = \"ice40\"\ntool = \"icestorm\"\n\
             [clock]\nmhz = 24\n[hw]\nregister-ratio = 8\n",
        )
        .unwrap();
        let env = Env { platform: Some(platform.to_string()), ..Env::default() };
        let r = resolved(&dir, Some("fast"), env, Overrides::default());
        assert_eq!(r.settings.target_device, "ice40-up5k");
        assert_eq!(r.settings.import_dirs, [dir.0.join("library"), platform.join("library")]);
        assert_eq!(r.hw.len(), 2);
        assert_eq!(r.hw[0]["register-ratio"].as_integer(), Some(8));
        assert_eq!(r.hw[0]["frequency"].as_integer(), Some(24));
        assert_eq!(r.hw[1]["register-ratio"].as_integer(), Some(2));
        assert_eq!(r.board.as_ref().map(kg_config::Board::name), Some("demo"));

        let err = resolve(None, false, None, &Env::default(), &Overrides::default(), &dir.0, &dir.0).unwrap_err();
        assert!(err.contains("no platform to find `demo`"), "{err}");
        let overrides = Overrides { target_device: Some("artix7-xc7a50t".to_owned()), ..Overrides::default() };
        let env = Env { platform: Some(platform.to_string()), ..Env::default() };
        let err = resolve(None, false, None, &env, &overrides, &dir.0, &dir.0).unwrap_err();
        assert!(err.contains("is a ice40-up5k"), "{err}");
    }

    /// No `[board]`, no change: one `[hw]` layer, the manifest's.
    #[test]
    fn without_a_board_nothing_changes() {
        let dir = Dir::new("noboard", MANIFEST);
        let r = resolved(&dir, None, Env::default(), Overrides::default());
        assert!(r.board.is_none());
        assert_eq!(r.hw.len(), 1);
        assert!(r.hw[0].is_empty());
    }

    #[test]
    fn the_manifest_is_the_base_layer() {
        let dir = Dir::new("base", MANIFEST);
        let r = resolved(&dir, None, Env::default(), Overrides::default());
        assert_eq!(r.settings.target_device, "mock");
        assert_eq!(r.settings.import_dirs, [dir.0.join("library")]);
        assert!(r.settings.warnings.is_enabled(WarningKind::Shadow));
        assert!(!r.settings.warnings.is_enabled(WarningKind::Conversion));
    }

    /// The language server discovers a manifest for every file it is asked
    /// about, and its `ConfigStore` says it does so "exactly as `kg build`
    /// would". That was false: `kg check` and `kg fmt` discovered from the
    /// process's working directory, so an editor and a terminal disagreed about
    /// which manifest governs a file — and therefore about the import path, the
    /// target device and `[fmt]`. The bug was found by a measurement that ran
    /// the real server over the library with no manifest in reach and quietly
    /// produced numbers for a program whose imports had not resolved.
    #[test]
    fn discovery_starts_beside_the_file_not_beside_the_shell() {
        let dir = Dir::new("discover-file", MANIFEST);
        let elsewhere = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join(format!("kg-cli-discover-empty-{}", std::process::id()));
        std::fs::create_dir_all(&elsewhere).unwrap();
        // No manifest here, and none above it that names `mock`.
        let _ = std::fs::remove_file(elsewhere.join("kanagawa.toml"));

        let found = resolve(None, false, None, &Env::default(), &Overrides::default(), &elsewhere, &dir.0)
            .unwrap();
        assert_eq!(found.manifest_path.as_deref(), Some(dir.0.join("kanagawa.toml").as_path()));
        assert_eq!(found.settings.target_device, "mock");

        // And the converse, so the test pins the *direction*: starting from the
        // manifest-less directory must not find it.
        let missed =
            resolve(None, false, None, &Env::default(), &Overrides::default(), &elsewhere, &elsewhere)
                .unwrap();
        assert_ne!(missed.manifest_path.as_deref(), Some(dir.0.join("kanagawa.toml").as_path()));
    }

    #[test]
    fn a_profile_deep_merges_over_the_manifest() {
        let dir = Dir::new("profile", MANIFEST);
        let r = resolved(&dir, Some("test"), Env::default(), Overrides::default());
        assert_eq!(r.settings.target_device, "stratix-10-gx-2800");
        // The profile touched only `target-device`; the rest of [project] survives.
        assert_eq!(r.settings.import_dirs, [dir.0.join("library")]);
        assert_eq!(r.settings.defines, [("verbose".to_owned(), "true".to_owned())]);
        // `all = true` plus an explicit allow.
        assert!(r.settings.warnings.is_enabled(WarningKind::Conversion));
        assert!(!r.settings.warnings.is_enabled(WarningKind::Shadow));
    }

    #[test]
    fn an_unknown_profile_is_an_error_not_a_silent_default() {
        let dir = Dir::new("unknown-profile", MANIFEST);
        let err = resolve(None, false, Some("nope"), &Env::default(), &Overrides::default(), &dir.0, &dir.0)
            .unwrap_err();
        assert!(err.contains("no [profile.nope]"), "{err}");
    }

    #[test]
    fn the_environment_overrides_the_profile_and_flags_override_the_environment() {
        let dir = Dir::new("env", MANIFEST);
        let env = Env {
            target_device: Some("agilex-7-f-023".to_owned()),
            import_dirs: Some("/env/one:/env/two".to_owned()),
            ..Env::default()
        };
        let r = resolved(&dir, Some("test"), env.clone(), Overrides::default());
        assert_eq!(r.settings.target_device, "agilex-7-f-023");

        let overrides = Overrides {
            target_device: Some("ultrascaleplus-xcu250".to_owned()),
            import_dirs: vec!["/flag".to_owned()],
            ..Overrides::default()
        };
        let r = resolved(&dir, Some("test"), env, overrides);
        assert_eq!(r.settings.target_device, "ultrascaleplus-xcu250");
        // Lists append in layer order; order is load-bearing for import search.
        assert_eq!(
            r.settings.import_dirs,
            [dir.0.join("library"), "/env/one".into(), "/env/two".into(), "/flag".into()]
        );
    }

    #[test]
    fn no_manifest_falls_back_to_built_in_defaults() {
        let dir = Dir::new("no-manifest", MANIFEST);
        let r = resolve(None, true, None, &Env::default(), &Overrides::default(), &dir.0, &dir.0).unwrap();
        assert!(r.manifest_path.is_none());
        assert!(r.settings.import_dirs.is_empty());
        assert!(r.settings.warnings.is_enabled(WarningKind::Shadow), "reference defaults apply");
    }

    #[test]
    fn defines_accept_both_separators_on_the_command_line() {
        let dir = Dir::new("defines", MANIFEST);
        let overrides = Overrides {
            defines: vec!["A#1".to_owned(), "B=2".to_owned()],
            using: vec!["T#uint8".to_owned()],
            ..Overrides::default()
        };
        let r = resolved(&dir, None, Env::default(), overrides);
        assert_eq!(
            r.settings.defines,
            [
                ("verbose".to_owned(), "true".to_owned()),
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "2".to_owned())
            ]
        );
        assert_eq!(r.settings.using, [("T".to_owned(), "uint8".to_owned())]);
    }

    #[test]
    fn warning_switches_fold_left_in_command_line_order() {
        let dir = Dir::new("warnings", MANIFEST);
        let overrides = Overrides {
            warnings: vec!["all".to_owned(), "no-shadow".to_owned(), "error".to_owned()],
            ..Overrides::default()
        };
        let r = resolved(&dir, None, Env::default(), overrides);
        assert!(r.settings.warnings.is_enabled(WarningKind::Conversion));
        assert!(!r.settings.warnings.is_enabled(WarningKind::Shadow));
        assert!(r.settings.warnings_as_errors);
    }

    #[test]
    fn an_unknown_warning_name_is_refused() {
        let dir = Dir::new("bad-warning", MANIFEST);
        let overrides =
            Overrides { warnings: vec!["no-such-warning".to_owned()], ..Overrides::default() };
        let err = resolve(None, false, None, &Env::default(), &overrides, &dir.0, &dir.0).unwrap_err();
        assert!(err.contains("unknown warning"), "{err}");
    }
}
