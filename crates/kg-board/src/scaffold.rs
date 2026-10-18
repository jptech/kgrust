//! `kg board new`: a project started from an example.
//!
//! An example is a project already, a directory with its `kanagawa.toml`:
//! a board's own (`boards/<board>/examples/`, naming the board by path) or
//! the platform's (`examples/`, for any board with the resources it binds).
//! The copy names the board chosen by name, with every path the manifest
//! holds re-expressed from where the copy lives.

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Failure, Result};

/// Copies `example` of `board` in `platform` to `dest`. `library` is the
/// standard library the copy should import when the example's own path to
/// it does not reach from `dest`; `None` when the installation's serves.
/// Returns the files written.
pub fn new_project(
    platform: &Utf8Path,
    board: &str,
    example: &str,
    dest: &Utf8Path,
    library: Option<&Utf8Path>,
) -> Result<Vec<Utf8PathBuf>> {
    let boards = platform.join("boards");
    if !boards.join(board).join("board.toml").is_file() {
        return Err(Failure::Usage(format!("no board `{board}` in {boards}; it has {}", kg_config::board::list(platform).join(", "))));
    }
    let own = boards.join(board).join("examples").join(example);
    let shared = platform.join("examples").join(example);
    let Some(source) = [own, shared].into_iter().find(|d| d.join("kanagawa.toml").is_file()) else {
        let mut known = examples(&boards.join(board));
        known.extend(examples(platform));
        return Err(Failure::Usage(if known.is_empty() {
            format!("the board {board} has no examples")
        } else {
            format!("no example `{example}` for {board}; there are {}", known.join(", "))
        }));
    };
    if dest.exists() && std::fs::read_dir(dest).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(Failure::Usage(format!("{dest} exists and is not empty")));
    }
    std::fs::create_dir_all(dest).map_err(|e| Failure::Usage(format!("{dest}: {e}")))?;
    let dest = absolute(dest);
    let source = absolute(&source);
    let platform = absolute(platform);

    let mut written = Vec::new();
    copy_tree(&source, &dest, &mut written)?;

    let text = std::fs::read_to_string(source.join("kanagawa.toml")).map_err(|e| Failure::Usage(e.to_string()))?;
    let manifest: toml::Table = toml::from_str(&text).map_err(|e| Failure::Usage(format!("{source}/kanagawa.toml: {e}")))?;
    let name = dest.file_name().unwrap_or("design").to_owned();
    let quote = |s: &str| toml::Value::String(s.to_owned()).to_string();

    // Every path re-expressed from the copy, or left out where it reaches
    // nothing from there and the installation serves.
    let mut dirs: Vec<String> = manifest
        .get("project")
        .and_then(|p| p.get("import-dirs"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(|d| source.join(d))
        .filter(|d| d.is_dir())
        .map(|d| quote(relative(&dest, &absolute(&d)).as_str()))
        .collect();
    if dirs.is_empty()
        && let Some(library) = library
    {
        dirs.push(quote(relative(&dest, &absolute(library)).as_str()));
    }
    let platform_value = (kg_config::board::bundled_platform().as_deref() != Some(platform.as_path()))
        .then(|| quote(relative(&dest, &platform).as_str()));

    // Edited as text, so the example's comments come along.
    let mut edited = Manifest::new(&text);
    edited.set("package", "name", Some(&quote(&name)));
    edited.set("project", "import-dirs", (!dirs.is_empty()).then(|| format!("[{}]", dirs.join(", "))).as_deref());
    edited.set("board", "path", None);
    edited.set("board", "name", Some(&quote(board)));
    edited.set("board", "platform", platform_value.as_deref());
    let header = format!("# Started from the `{example}` example for the {board} board (`kg board new`).\n\n");
    let path = dest.join("kanagawa.toml");
    std::fs::write(&path, header + edited.text.trim_end() + "\n").map_err(|e| Failure::Failed(format!("{path}: {e}")))?;
    Ok(written)
}

/// A manifest's text, edited key by key, every comment kept.
struct Manifest {
    text: String,
}

impl Manifest {
    fn new(text: &str) -> Manifest {
        Manifest { text: text.to_owned() }
    }

    /// Sets `key` in `[section]` to `value` (TOML text), or removes it with
    /// `None`. A missing key goes at the end of its section, and a missing
    /// section at the end of the file, `[package]` at its start.
    fn set(&mut self, section: &str, key: &str, value: Option<&str>) {
        let lines: Vec<&str> = self.text.split_inclusive('\n').collect();
        let header = |l: &str| l.trim_start().starts_with('[') && !l.trim_start().starts_with("[[");
        let start = lines.iter().position(|l| header(l) && l.trim().trim_matches(|c| c == '[' || c == ']').trim() == section);
        let Some(start) = start else {
            let Some(value) = value else { return };
            let table = format!("[{section}]\n{key} = {value}\n");
            self.text = if section == "package" {
                format!("{table}\n{}", self.text)
            } else {
                format!("{}\n{table}", self.text.trim_end_matches('\n').to_owned() + "\n")
            };
            return;
        };
        let end = lines[start + 1..].iter().position(|l| header(l)).map_or(lines.len(), |i| start + 1 + i);
        let found = (start + 1..end).find(|&i| {
            let l = lines[i].trim_start();
            l.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))
        });
        let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
        match (found, value) {
            (Some(i), _) => {
                // The value may span lines: through its closing bracket.
                let mut last = i;
                let mut depth: i32 = 0;
                for (j, line) in lines.iter().enumerate().take(end).skip(i) {
                    let code = line.split('#').next().unwrap_or("");
                    depth += code.matches('[').count() as i32 - code.matches(']').count() as i32;
                    last = j;
                    if depth <= 0 {
                        break;
                    }
                }
                let replacement = value.map(|v| format!("{key} = {v}\n")).unwrap_or_default();
                out.splice(i..=last, std::iter::once(replacement));
            }
            (None, Some(v)) => {
                // After the section's last key, before its trailing blank lines.
                let mut at = end;
                while at > start + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                out.insert(at, format!("{key} = {v}\n"));
            }
            (None, None) => {}
        }
        self.text = out.concat();
    }
}

/// The examples in `dir/examples` (a board's, or the platform's), by name.
pub fn examples(dir: &Utf8Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join("examples"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("kanagawa.toml").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Every file but the build's, which `kg board` writes under `target/`.
fn copy_tree(from: &Utf8Path, to: &Utf8Path, written: &mut Vec<Utf8PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(from).map_err(|e| Failure::Usage(format!("{from}: {e}")))?;
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let Ok(name) = entry.file_name().into_string() else { continue };
        let (src, dst) = (from.join(&name), to.join(&name));
        if src.is_dir() {
            if name == "target" {
                continue;
            }
            std::fs::create_dir_all(&dst).map_err(|e| Failure::Failed(format!("{dst}: {e}")))?;
            copy_tree(&src, &dst, written)?;
        } else if name != "kanagawa.toml" {
            std::fs::copy(&src, &dst).map_err(|e| Failure::Failed(format!("{dst}: {e}")))?;
            written.push(dst);
        }
    }
    Ok(())
}

fn absolute(path: &Utf8Path) -> Utf8PathBuf {
    path.canonicalize_utf8().unwrap_or_else(|_| path.to_owned())
}

/// `to` as seen from `from`, both absolute; `to` itself where they share
/// only the root, which no relative path would make clearer.
fn relative(from: &Utf8Path, to: &Utf8Path) -> Utf8PathBuf {
    let from: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to_parts).take_while(|(a, b)| a == b).count();
    if common <= 1 {
        return to.to_owned();
    }
    let to = to_parts;
    let mut path = Utf8PathBuf::new();
    for _ in common..from.len() {
        path.push("..");
    }
    for part in &to[common..] {
        path.push(part.as_str());
    }
    if path.as_str().is_empty() { Utf8PathBuf::from(".") } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_is_edited_key_by_key() {
        let mut m = Manifest::new("# top\n[project]\n# dirs\nimport-dirs = [\n  \"a\",\n]\nsources = [\"x.k\"]\n\n[board]\npath = \"../..\" # here\n");
        m.set("project", "import-dirs", Some("[\"lib\"]"));
        m.set("board", "path", None);
        m.set("board", "name", Some("\"demo\""));
        m.set("package", "name", Some("\"mine\""));
        assert_eq!(
            m.text,
            "[package]\nname = \"mine\"\n\n# top\n[project]\n# dirs\nimport-dirs = [\"lib\"]\nsources = [\"x.k\"]\n\n[board]\nname = \"demo\"\n"
        );
    }

    #[test]
    fn relative_paths_climb_to_the_common_root() {
        assert_eq!(relative(Utf8Path::new("/home/me/blink"), Utf8Path::new("/home/me/kgrust/platform")), "../kgrust/platform");
        assert_eq!(relative(Utf8Path::new("/a/b"), Utf8Path::new("/a/b")), ".");
        assert_eq!(relative(Utf8Path::new("/a/b"), Utf8Path::new("/a/b/c")), "c");
        assert_eq!(relative(Utf8Path::new("/tmp/mine"), Utf8Path::new("/Users/me/platform")), "/Users/me/platform");
    }

    /// The copy names the board, the platform and the library from where it
    /// lives, and leaves the example's own manifest alone.
    #[test]
    fn a_new_project_names_its_board_from_where_it_lives() {
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap().join(format!("kg-board-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let example = root.join("platform/boards/demo/examples/blinky");
        std::fs::create_dir_all(&example).unwrap();
        std::fs::write(root.join("platform/boards/demo/board.toml"), "").unwrap();
        std::fs::create_dir_all(root.join("platform/library")).unwrap();
        std::fs::create_dir_all(root.join("upstream/library")).unwrap();
        std::fs::create_dir_all(example.join("target/gen")).unwrap();
        std::fs::write(example.join("blinky.k"), "class Blinky {}\n").unwrap();
        std::fs::write(example.join("target/gen/Blinky.sv"), "").unwrap();
        let manifest = "[project]\nsources = [\"blinky.k\"]\nimport-dirs = [\"../../../../../nowhere\"]\n\n[board]\npath = \"../..\"\n# the shell's core\nrtl = [\"core.sv\"]\n";
        std::fs::write(example.join("kanagawa.toml"), manifest).unwrap();

        let dest = root.join("work/mine");
        let written = new_project(&root.join("platform"), "demo", "blinky", &dest, Some(&root.join("upstream/library"))).unwrap();
        assert_eq!(written, [absolute(&dest).join("blinky.k")]);
        let text = std::fs::read_to_string(dest.join("kanagawa.toml")).unwrap();
        let (package, project, board) = (text.find("[package]"), text.find("[project]"), text.find("[board]"));
        assert!(package < project && project < board, "{text}");
        let copied: toml::Table = toml::from_str(&text).unwrap();
        assert_eq!(copied["package"]["name"].as_str(), Some("mine"));
        assert_eq!(copied["board"]["name"].as_str(), Some("demo"));
        assert!(copied["board"].get("path").is_none());
        assert_eq!(copied["board"]["platform"].as_str(), Some("../../platform"));
        assert_eq!(copied["board"]["rtl"][0].as_str(), Some("core.sv"));
        assert_eq!(copied["project"]["import-dirs"][0].as_str(), Some("../../upstream/library"));
        assert_eq!(std::fs::read_to_string(example.join("kanagawa.toml")).unwrap(), manifest);
        assert!(text.contains("# the shell's core\n"), "the example's comments are kept:\n{text}");

        let err = new_project(&root.join("platform"), "demo", "blinky", &dest, None).unwrap_err();
        assert!(err.message().contains("not empty"), "{}", err.message());
        let err = new_project(&root.join("platform"), "demo", "nope", &root.join("work/other"), None).unwrap_err();
        assert!(err.message().contains("there are blinky"), "{}", err.message());

        // The platform's own examples serve any board.
        let shared = root.join("platform/examples/accel");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("accel.k"), "class Accel {}\n").unwrap();
        std::fs::write(shared.join("kanagawa.toml"), "[project]\nsources = [\"accel.k\"]\n\n[board]\nname = \"other\"\nplatform = \"../..\"\n").unwrap();
        new_project(&root.join("platform"), "demo", "accel", &root.join("work/accel"), None).unwrap();
        let copied: toml::Table = toml::from_str(&std::fs::read_to_string(root.join("work/accel/kanagawa.toml")).unwrap()).unwrap();
        assert_eq!(copied["board"]["name"].as_str(), Some("demo"));
        assert_eq!(copied["board"]["platform"].as_str(), Some("../../platform"));
        let err = new_project(&root.join("platform"), "nowhere", "accel", &root.join("work/x"), None).unwrap_err();
        assert!(err.message().contains("no board `nowhere`"), "{}", err.message());
    }
}
