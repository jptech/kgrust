//! The `.k` files of a project, as the server finds them on disk: one walk,
//! for every feature that looks past the programs the open buffers root —
//! `workspace/symbol`'s index of the import directories, and the files
//! find-references and rename search beside the open buffers'.

use kg_base::Utf8PathBuf;
use kg_config::Settings;

/// Every `.k` file a search of the project covers, by path, sorted and
/// without repeats: everything under each import directory, which is what
/// the project compiles against (`Parser/IO.hs:103,110`); and, where the
/// project has a manifest, everything under its root — the project's own
/// files, which an import directory need not hold (a test bench beside the
/// library it imports). Under the root, a directory with a manifest of its
/// own is another project, and a hidden one (`.git`) is nobody's; both are
/// left out. Without a manifest the root is only the directory the first
/// file was opened from ([`Settings::fallback`]), and nothing says what
/// else is the project's, so it is not walked.
pub(crate) fn project_sources(settings: &Settings) -> Vec<Utf8PathBuf> {
    let mut out = Vec::new();
    for dir in &settings.import_dirs {
        sources_under(dir, false, &mut |path| out.push(path));
    }
    if settings.root.join(kg_config::MANIFEST_NAME).is_file() {
        sources_under(&settings.root, true, &mut |path| out.push(path));
    }
    out.sort();
    out.dedup();
    out
}

/// Every `.k` file under `dir`, at any depth, in the order the directory
/// lists them; a directory that cannot be read contributes nothing. A
/// symbolic link to a directory is not followed, so a link cycle cannot
/// make the walk endless. `own_project` leaves out the directories
/// [`project_sources`] says are not the project's.
pub(crate) fn sources_under(dir: &kg_base::Utf8Path, own_project: bool, each: &mut dyn FnMut(Utf8PathBuf)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else { continue };
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                let hidden = path.file_name().is_some_and(|n| n.starts_with('.'));
                let nested = own_project && path.join(kg_config::MANIFEST_NAME).is_file();
                if !(own_project && (hidden || nested)) {
                    sources_under(&path, own_project, each);
                }
            }
            _ if path.extension() == Some("k") && path.is_file() => each(path),
            _ => {}
        }
    }
}
