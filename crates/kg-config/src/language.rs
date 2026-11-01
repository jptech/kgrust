//! A file's language — its edition and features (`kg_base::language`) — as
//! a manifest names it, and which manifest names it for which file
//! (`docs/editions/README.md`, "Which language a file is in").

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use camino::{Utf8Path, Utf8PathBuf};
use kg_base::language::{Edition, Feature, Language};
#[cfg(test)]
use kg_base::language::Features;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{MANIFEST_NAME, Manifest, ManifestError};

/// `[language]`: features named on their own, beside the edition.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct LanguageTable {
    #[serde(default, deserialize_with = "features", serialize_with = "feature_names")]
    pub features: Vec<Feature>,
}

impl Manifest {
    /// The language this manifest's own package is written in.
    pub fn language(&self) -> Language {
        Language {
            edition: self.package.edition.unwrap_or_default(),
            extra: self.language.features.iter().copied().collect(),
        }
    }
}

/// An edition this `kg` does not know is refused, never read as another:
/// an older tool must not compile a newer program as something else. One
/// written as a number, `edition = 1.1`, is refused with the spelling that
/// would have been read.
pub(crate) fn edition<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Edition>, D::Error> {
    d.deserialize_any(EditionName).map(Some)
}

/// A visitor rather than a `String` read and then parsed, so that `toml`
/// places every refusal at the value, the number's included.
struct EditionName;

impl serde::de::Visitor<'_> for EditionName {
    type Value = Edition;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an edition, as a string such as \"1.1\"")
    }

    fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Edition, E> {
        parse_edition(name).map_err(E::custom)
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Edition, E> {
        Err(unquoted(&format!("{v:?}")))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Edition, E> {
        Err(unquoted(&v.to_string()))
    }
}

fn unquoted<E: serde::de::Error>(number: &str) -> E {
    E::custom(format!("an edition is a string, not a number: write `edition = \"{number}\"`"))
}

pub(crate) fn edition_name<S: Serializer>(e: &Option<Edition>, s: S) -> Result<S::Ok, S::Error> {
    e.map(Edition::name).serialize(s)
}

fn features<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Feature>, D::Error> {
    Vec::<String>::deserialize(d)?.iter().map(|n| parse_feature(n).map_err(serde::de::Error::custom)).collect()
}

fn feature_names<S: Serializer>(f: &[Feature], s: S) -> Result<S::Ok, S::Error> {
    f.iter().map(|f| f.name()).collect::<Vec<_>>().serialize(s)
}

/// An edition by name, or why not — what `--edition` and the manifest say.
pub fn parse_edition(name: &str) -> Result<Edition, String> {
    Edition::from_name(name).ok_or_else(|| {
        let known: Vec<_> = Edition::ALL.iter().map(|e| e.name()).collect();
        format!("unknown edition `{name}`: this kg knows {}", known.join(", "))
    })
}

/// A feature by name, or why not — what `--feature` and the manifest say.
pub fn parse_feature(name: &str) -> Result<Feature, String> {
    Feature::from_name(name).ok_or_else(|| {
        let known: Vec<_> = Feature::ALL.iter().map(|f| f.name()).collect();
        format!("unknown language feature `{name}`: this kg knows {}", known.join(", "))
    })
}

/// A language from the command line's `--edition` and `--feature` over
/// `base`: the edition replaced, the features added.
pub fn overridden(base: Language, edition: Option<&str>, features: &[String]) -> Result<Language, String> {
    let mut language = base;
    if let Some(e) = edition {
        language.edition = parse_edition(e)?;
    }
    for f in features {
        language.extra = language.extra.with(parse_feature(f)?);
    }
    Ok(language)
}

/// Which language each file is in: a file's own package decides, never the
/// package that imports it.
///
/// A file's package is the nearest one above it: walking up from the file,
/// the first `kanagawa.toml` names its language — or the project's root
/// does, in the language `--edition` and `--feature` give it — and with
/// neither, past the filesystem's root, it is the reference. So a
/// dependency keeps the edition its own manifest names, a project's `lib/`
/// is the project's, and the standard library — a release's, the
/// submodule's — has no manifest above it and is edition 1.0.
///
/// Where a library does sit under a manifest of a later edition, it reads
/// the same: a feature never changes what a program the reference accepts
/// means (`docs/plans/editions.md` §1). So the walk needs no boundary at an
/// import directory, which would miss the common layout of a dependency's
/// manifest beside the `src/` its users import.
///
/// A manifest the walk meets that does not load names no language — its
/// files are read as the reference — and is kept, for the front door to
/// report ([`LanguageMap::refused`]): an unknown edition is an error, never
/// ignored, a dependency's included.
#[derive(Clone, Debug, Default)]
pub struct LanguageMap {
    root: Option<Utf8PathBuf>,
    language: Language,
    /// Reads no manifest ([`LanguageMap::fixed`]).
    fixed: bool,
    /// Each directory's answer, once walked.
    walked: Arc<Mutex<HashMap<Utf8PathBuf, Language>>>,
    /// Each manifest met that did not load, by path.
    refused: Arc<Mutex<BTreeMap<Utf8PathBuf, Arc<ManifestError>>>>,
}

impl LanguageMap {
    /// The project rooted at `root`, in `language`.
    pub fn new(root: &Utf8Path, language: Language) -> LanguageMap {
        LanguageMap { root: Some(normal(root)), language, ..LanguageMap::default() }
    }

    /// The project rooted at `root`, in `language`, reading no manifest:
    /// every file under `root` is in `language`, every other the
    /// reference's. What `--no-manifest` asks for, which a nested package's
    /// manifest would otherwise answer for its own files.
    pub fn fixed(root: &Utf8Path, language: Language) -> LanguageMap {
        LanguageMap { fixed: true, ..LanguageMap::new(root, language) }
    }

    /// Every file in the reference, whatever manifest it sits under: the
    /// compat binary's, which is the reference's command line, and the
    /// reference reads no manifest (`docs/plans/editions.md` §1, rule 6).
    pub fn reference() -> LanguageMap {
        LanguageMap { fixed: true, ..LanguageMap::default() }
    }

    /// Each manifest whose language a file was asked for and which did not
    /// load, in path order — a dependency's, which nothing else reads.
    pub fn refused(&self) -> Vec<(Utf8PathBuf, Arc<ManifestError>)> {
        self.refused.lock().unwrap().iter().map(|(p, e)| (p.clone(), Arc::clone(e))).collect()
    }

    /// The project's own language.
    pub fn project(&self) -> Language {
        self.language
    }

    /// The same project in `language` — what `--edition` and `--feature`
    /// make of the manifest's.
    #[must_use]
    pub fn with_project(&self, language: Language) -> LanguageMap {
        LanguageMap { root: self.root.clone(), language, fixed: self.fixed, walked: Arc::default(), refused: Arc::default() }
    }

    /// The language of the file at `path`.
    pub fn language_of(&self, path: &Utf8Path) -> Language {
        let path = normal(path);
        let Some(dir) = path.parent() else { return Language::REFERENCE };
        if self.fixed {
            return match &self.root {
                Some(root) if dir.starts_with(root) => self.language,
                _ => Language::REFERENCE,
            };
        }
        self.language_in(dir)
    }

    /// Whether the file at `path` is one of the project's own: under its
    /// root and under no other package's manifest — read in the project's
    /// language because it is the project's, which `kg expand` rewrites,
    /// and not a dependency's, which keeps its own.
    pub fn is_project(&self, path: &Utf8Path) -> bool {
        let path = normal(path);
        let (Some(root), Some(dir)) = (&self.root, path.parent()) else { return false };
        if !dir.starts_with(root) {
            return false;
        }
        if self.fixed {
            return true;
        }
        let mut at = dir;
        while at != root.as_path() {
            if at.join(MANIFEST_NAME).is_file() {
                return false;
            }
            match at.parent() {
                Some(up) => at = up,
                None => return false,
            }
        }
        true
    }

    /// The project's root directory, if the map has one.
    pub fn root(&self) -> Option<&Utf8Path> {
        self.root.as_deref()
    }

    /// The same project with its root at `root` — its canonical spelling,
    /// where the caller keeps its files by theirs.
    #[must_use]
    pub fn rooted_at(&self, root: &Utf8Path) -> LanguageMap {
        LanguageMap { root: Some(normal(root)), language: self.language, fixed: self.fixed, walked: Arc::default(), refused: Arc::default() }
    }

    fn language_in(&self, dir: &Utf8Path) -> Language {
        if let Some(&l) = self.walked.lock().unwrap().get(dir) {
            return l;
        }
        let l = self.walk(dir);
        self.walked.lock().unwrap().insert(dir.to_owned(), l);
        l
    }

    fn walk(&self, dir: &Utf8Path) -> Language {
        if self.root.as_deref() == Some(dir) {
            return self.language;
        }
        let manifest = dir.join(MANIFEST_NAME);
        if manifest.is_file() {
            // A manifest that does not load names no language: the file is
            // read as the reference, which is what it was before editions,
            // and the manifest is reported.
            return match crate::load(&manifest) {
                Ok(m) => m.language(),
                Err(e) => {
                    self.refused.lock().unwrap().insert(manifest, Arc::new(e));
                    Language::REFERENCE
                }
            };
        }
        match dir.parent() {
            Some(up) => self.language_in(up),
            None => Language::REFERENCE,
        }
    }
}

/// `path` made absolute and stripped of `.` and `..` by text — never by the
/// filesystem, whose links would make a walk's answer depend on how a path
/// was spelled.
fn normal(path: &Utf8Path) -> Utf8PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        match std::env::current_dir().ok().and_then(|d| Utf8PathBuf::from_path_buf(d).ok()) {
            Some(cwd) => cwd.join(path),
            None => path.to_owned(),
        }
    };
    let mut out = Utf8PathBuf::new();
    for c in absolute.components() {
        match c {
            camino::Utf8Component::CurDir => {}
            camino::Utf8Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMED: Language = Language { edition: Edition::V1_1, extra: Features::NONE };

    fn tree(files: &[(&str, &str)]) -> (tempdir::Dir, Utf8PathBuf) {
        let dir = tempdir::Dir::new();
        let root = dir.path().to_owned();
        for (path, text) in files {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text).unwrap();
        }
        (dir, root)
    }

    #[test]
    fn the_project_is_in_its_own_language_and_the_library_in_the_reference() {
        let (_d, root) = tree(&[
            ("proj/kanagawa.toml", "[package]\nedition = \"1.1\"\n"),
            ("proj/src/a.k", ""),
            ("proj/lib/b.k", ""),
            ("library/base.k", ""),
            ("library/data/x.k", ""),
        ]);
        let proj = root.join("proj");
        let lib = root.join("library");
        let map = LanguageMap::new(&proj, NAMED);
        assert_eq!(map.language_of(&proj.join("src/a.k")), NAMED);
        assert_eq!(map.language_of(&proj.join("lib/b.k")), NAMED, "an import directory inside the project is the project's");
        assert_eq!(map.language_of(&lib.join("base.k")), Language::REFERENCE);
        assert_eq!(map.language_of(&lib.join("data/x.k")), Language::REFERENCE);
    }

    #[test]
    fn a_nested_manifest_is_another_package() {
        let (_d, root) = tree(&[
            ("proj/kanagawa.toml", "[package]\nedition = \"1.1\"\n"),
            ("proj/vendor/dep/kanagawa.toml", "[package]\nname = \"dep\"\n"),
            ("proj/vendor/dep/d.k", ""),
            ("dep2/kanagawa.toml", "[language]\nfeatures = [\"named-arguments\"]\n"),
            ("dep2/src/e.k", ""),
        ]);
        let proj = root.join("proj");
        let map = LanguageMap::new(&proj, NAMED);
        assert_eq!(map.language_of(&proj.join("vendor/dep/d.k")), Language::REFERENCE);
        let e = map.language_of(&root.join("dep2/src/e.k"));
        assert_eq!(e.edition, Edition::V1_0);
        assert!(e.features().has(Feature::NamedArguments), "a dependency keeps the language its manifest names");
    }

    /// A project's file is under its root and under no other package's
    /// manifest: what `kg expand` rewrites. A fixed map reads no manifest, and a
    /// map with no root has no project. Red with the manifest walk gone, a
    /// vendored dependency's text then rewritten in another language.
    #[test]
    fn a_nested_package_is_no_file_of_the_project() {
        let (_d, root) = tree(&[
            ("proj/kanagawa.toml", "[package]\nedition = \"1.1\"\n"),
            ("proj/a.k", ""),
            ("proj/lib/b.k", ""),
            ("proj/vendor/dep/kanagawa.toml", "[package]\nname = \"dep\"\n"),
            ("proj/vendor/dep/src/d.k", ""),
            ("other/e.k", ""),
        ]);
        let proj = root.join("proj");
        let map = LanguageMap::new(&proj, NAMED);
        assert!(map.is_project(&proj.join("a.k")));
        assert!(map.is_project(&proj.join("lib/b.k")));
        assert!(!map.is_project(&proj.join("vendor/dep/src/d.k")));
        assert!(!map.is_project(&root.join("other/e.k")));
        assert!(LanguageMap::fixed(&proj, NAMED).is_project(&proj.join("vendor/dep/src/d.k")));
        assert!(!LanguageMap::reference().is_project(&proj.join("a.k")));
    }

    #[test]
    fn a_file_outside_every_package_is_the_reference() {
        let (_d, root) = tree(&[("elsewhere/z.k", "")]);
        let map = LanguageMap::new(&root.join("proj"), NAMED);
        assert_eq!(map.language_of(&root.join("elsewhere/z.k")), Language::REFERENCE);
        assert_eq!(LanguageMap::default().language_of(&root.join("elsewhere/z.k")), Language::REFERENCE);
    }

    #[test]
    fn flags_change_the_project_and_nothing_else() {
        let (_d, root) = tree(&[("proj/a.k", ""), ("library/base.k", "")]);
        let proj = root.join("proj");
        let map = LanguageMap::new(&proj, Language::REFERENCE).with_project(NAMED);
        assert_eq!(map.language_of(&proj.join("a.k")), NAMED);
        assert_eq!(map.language_of(&root.join("library/base.k")), Language::REFERENCE);
    }

    #[test]
    fn an_unknown_edition_or_feature_is_refused_by_name() {
        let path = Utf8Path::new("kanagawa.toml");
        let err = crate::discover::parse("[package]\nedition = \"2.0\"\n", path).unwrap_err().to_string();
        assert!(err.contains("unknown edition `2.0`: this kg knows 1.0, 1.1"), "{err}");
        let err = crate::discover::parse("[language]\nfeatures = [\"named_arguments\"]\n", path).unwrap_err().to_string();
        assert!(err.contains("unknown language feature `named_arguments`"), "{err}");
        let m = crate::discover::parse("[package]\nedition = \"1.1\"\n", path).unwrap();
        assert_eq!(m.language(), NAMED);
        assert_eq!(crate::discover::parse("", path).unwrap().language(), Language::REFERENCE);
    }

    /// The language server marks a refused edition where it is written, so
    /// the error carries the value's place and its message bare.
    #[test]
    fn an_unknown_edition_is_placed_at_its_value() {
        let text = "[package]\nname = \"p\"\nedition = \"9.9\"\n";
        let err = crate::discover::parse(text, Utf8Path::new("kanagawa.toml")).unwrap_err();
        assert_eq!(err.span().map(|s| &text[s]), Some("\"9.9\""));
        assert_eq!(err.message(), "unknown edition `9.9`: this kg knows 1.0, 1.1");
    }

    /// `edition = 1.1`, a TOML float, is the likeliest slip: refused at the
    /// number, with the spelling that would have been read.
    #[test]
    fn an_edition_written_as_a_number_is_told_to_quote_it() {
        let text = "[package]\nedition = 1.1\n";
        let err = crate::discover::parse(text, Utf8Path::new("kanagawa.toml")).unwrap_err();
        assert_eq!(err.span().map(|s| &text[s]), Some("1.1"));
        assert_eq!(err.message(), "an edition is a string, not a number: write `edition = \"1.1\"`");
        let err = crate::discover::parse("[package]\nedition = 1\n", Utf8Path::new("kanagawa.toml")).unwrap_err();
        assert_eq!(err.message(), "an edition is a string, not a number: write `edition = \"1\"`");
    }

    /// A manifest the walk meets that does not load is read as the
    /// reference and kept, for the front door to report: a dependency's
    /// unknown edition is an error, never ignored.
    #[test]
    fn a_refused_manifest_is_the_reference_and_is_kept() {
        let (_d, root) = tree(&[
            ("proj/kanagawa.toml", "[package]\nedition = \"1.1\"\n"),
            ("dep/kanagawa.toml", "[package]\nedition = \"9.9\"\n"),
            ("dep/src/d.k", ""),
            ("ok/kanagawa.toml", "[package]\nedition = \"1.1\"\n"),
            ("ok/o.k", ""),
        ]);
        let map = LanguageMap::new(&root.join("proj"), NAMED);
        assert_eq!(map.language_of(&root.join("ok/o.k")), NAMED);
        assert!(map.refused().is_empty());
        assert_eq!(map.language_of(&root.join("dep/src/d.k")), Language::REFERENCE);
        let refused = map.refused();
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].0, root.join("dep/kanagawa.toml"));
        assert_eq!(refused[0].1.message(), "unknown edition `9.9`: this kg knows 1.0, 1.1");
        assert_eq!(map.clone().refused().len(), 1, "a clone shares what was met");
    }

    /// `--no-manifest` reads none, a nested package's included; the compat
    /// binary reads none, and is the reference everywhere.
    #[test]
    fn a_fixed_map_reads_no_manifest() {
        let (_d, root) = tree(&[
            ("proj/kanagawa.toml", "[package]\nedition = \"1.1\"\n"),
            ("proj/a.k", ""),
            ("proj/pkg/kanagawa.toml", "[package]\nedition = \"1.1\"\n"),
            ("proj/pkg/b.k", ""),
            ("dep/kanagawa.toml", "[package]\nedition = \"1.1\"\n"),
            ("dep/c.k", ""),
        ]);
        let proj = root.join("proj");
        let fixed = LanguageMap::fixed(&proj, Language::REFERENCE);
        assert_eq!(fixed.language_of(&proj.join("pkg/b.k")), Language::REFERENCE);
        assert_eq!(fixed.language_of(&root.join("dep/c.k")), Language::REFERENCE);
        let flagged = fixed.with_project(NAMED);
        assert_eq!(flagged.language_of(&proj.join("a.k")), NAMED);
        assert_eq!(flagged.language_of(&proj.join("pkg/b.k")), NAMED, "the flags reach the inputs' tree");
        assert_eq!(flagged.language_of(&root.join("dep/c.k")), Language::REFERENCE);
        let reference = LanguageMap::reference();
        for path in [proj.join("a.k"), proj.join("pkg/b.k"), root.join("dep/c.k")] {
            assert_eq!(reference.language_of(&path), Language::REFERENCE, "{path}");
        }
        assert!(fixed.refused().is_empty() && reference.refused().is_empty());
    }

    /// A directory removed when dropped; the crate has no temp-dir
    /// dependency, and these tests need only this much of one.
    mod tempdir {
        use camino::{Utf8Path, Utf8PathBuf};

        pub struct Dir(Utf8PathBuf);

        impl Dir {
            pub fn new() -> Dir {
                use std::sync::atomic::{AtomicU32, Ordering};
                static N: AtomicU32 = AtomicU32::new(0);
                let base = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
                let dir = base.join(format!("kg-language-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
                std::fs::create_dir_all(&dir).unwrap();
                Dir(Utf8PathBuf::from_path_buf(dir.canonicalize().unwrap()).unwrap())
            }

            pub fn path(&self) -> &Utf8Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
