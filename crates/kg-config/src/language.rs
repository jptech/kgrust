//! A file's language — its edition and features (`kg_base::language`) — as
//! a manifest names it, and which manifest names it for which file
//! (`docs/editions/README.md`, "Which language a file is in").

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use camino::{Utf8Path, Utf8PathBuf};
use kg_base::language::{Edition, Feature, Language};
#[cfg(test)]
use kg_base::language::Features;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{MANIFEST_NAME, Manifest};

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
/// an older tool must not compile a newer program as something else.
pub(crate) fn edition<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Edition>, D::Error> {
    let Some(name) = Option::<String>::deserialize(d)? else { return Ok(None) };
    parse_edition(&name).map(Some).map_err(serde::de::Error::custom)
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
#[derive(Clone, Debug, Default)]
pub struct LanguageMap {
    root: Option<Utf8PathBuf>,
    language: Language,
    /// Each directory's answer, once walked.
    walked: Arc<Mutex<HashMap<Utf8PathBuf, Language>>>,
}

impl LanguageMap {
    /// The project rooted at `root`, in `language`.
    pub fn new(root: &Utf8Path, language: Language) -> LanguageMap {
        LanguageMap { root: Some(normal(root)), language, walked: Arc::default() }
    }

    /// The project's own language.
    pub fn project(&self) -> Language {
        self.language
    }

    /// The same project in `language` — what `--edition` and `--feature`
    /// make of the manifest's.
    #[must_use]
    pub fn with_project(&self, language: Language) -> LanguageMap {
        LanguageMap { root: self.root.clone(), language, walked: Arc::default() }
    }

    /// The language of the file at `path`.
    pub fn language_of(&self, path: &Utf8Path) -> Language {
        let path = normal(path);
        let Some(dir) = path.parent() else { return Language::REFERENCE };
        self.language_in(dir)
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
            // read as the reference, which is what it was before editions.
            return crate::load(&manifest).map(|m| m.language()).unwrap_or(Language::REFERENCE);
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
