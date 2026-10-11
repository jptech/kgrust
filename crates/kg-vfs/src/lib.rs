//! A virtual file system.
//!
//! Every path the compiler reads goes through here, for one reason: an editor
//! asks about text that is not on disk. In batch mode this is a thin pass-through
//! to `std::fs`; in the language server the overlay map wins.
//!
//! Files are interned to [`FileId`] on first sight and never forgotten, so a
//! `FileId` obtained by any query stays valid.

use std::sync::{Mutex, RwLock};

use camino::{Utf8Path, Utf8PathBuf};
use kg_base::{FileId, FxHashMap, LineIndex};

/// Text of one file plus its precomputed line table.
#[derive(Debug, Clone)]
pub struct FileText {
    pub text: String,
    pub line_index: std::sync::Arc<LineIndex>,
}

impl FileText {
    pub fn new(text: String) -> FileText {
        let line_index = std::sync::Arc::new(LineIndex::new(&text));
        FileText { text, line_index }
    }
}

#[derive(Debug, Default)]
struct Paths {
    to_id: FxHashMap<Utf8PathBuf, FileId>,
    to_path: Vec<Utf8PathBuf>,
}

/// Why a file that **exists** on disk did not read as text.
///
/// [`Vfs::read`] answers `None` for these as for an absent file, because every
/// caller but one only needs the text. The one is the import search, which the
/// reference stops at the first file that *exists* (`findFile`,
/// `Parser/IO.hs:103`) and whose read failure it reports as that file's
/// exception rather than as "Can't find file" (`Parser/IO.hs:106-107`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFailure {
    /// Not UTF-8. `byte` is the first byte of the first invalid or truncated
    /// sequence — what GHC's decoder names in "cannot decode byte sequence
    /// starting from N" (`readFileUtf8`, `Internal.hs:24-32`).
    Undecodable { byte: u8 },
    /// A regular file that could not be opened for reading.
    PermissionDenied,
}

/// One disk read's outcome, cached until [`Vfs::invalidate`].
#[derive(Debug, Clone)]
enum Disk {
    Text(FileText),
    /// Not there, or not a regular file: `doesFileExist` is false for a
    /// directory, so the import search moves past one (`Parser/IO.hs:103`).
    Absent,
    Unreadable(ReadFailure),
}

#[derive(Debug, Default)]
pub struct Vfs {
    paths: Mutex<Paths>,
    /// Unsaved editor buffers. Present here means "ignore the disk".
    overlays: RwLock<FxHashMap<FileId, FileText>>,
    /// Contents read from disk, keyed by file. Invalidated by [`Vfs::invalidate`].
    disk: RwLock<FxHashMap<FileId, Disk>>,
}

impl Vfs {
    pub fn new() -> Vfs {
        Vfs::default()
    }

    /// Interns `path` under a lexically normalized key, so that two spellings
    /// of the same file share a [`FileId`] — which matters because the import
    /// search path routinely reaches one library file by several routes.
    pub fn intern(&self, path: &Utf8Path) -> FileId {
        let key = canonical(path);
        let mut paths = self.paths.lock().unwrap();
        if let Some(&id) = paths.to_id.get(&key) {
            return id;
        }
        let id = FileId(paths.to_path.len() as u32);
        paths.to_path.push(key.clone());
        paths.to_id.insert(key, id);
        id
    }

    pub fn path(&self, file: FileId) -> Option<Utf8PathBuf> {
        self.paths.lock().unwrap().to_path.get(file.index()).cloned()
    }

    /// Sets an in-memory overlay, as `textDocument/didOpen` and `didChange` do.
    pub fn set_overlay(&self, file: FileId, text: String) {
        self.overlays.write().unwrap().insert(file, FileText::new(text));
    }

    /// Drops an overlay, as `textDocument/didClose` does, so the file reverts to
    /// whatever is on disk.
    pub fn remove_overlay(&self, file: FileId) {
        self.overlays.write().unwrap().remove(&file);
    }

    pub fn has_overlay(&self, file: FileId) -> bool {
        self.overlays.read().unwrap().contains_key(&file)
    }

    /// Forgets the cached disk contents of `file`, e.g. on a watcher event.
    pub fn invalidate(&self, file: FileId) {
        self.disk.write().unwrap().remove(&file);
    }

    pub fn invalidate_all(&self) {
        self.disk.write().unwrap().clear();
    }

    /// Contents of `file`: the overlay if there is one, otherwise the disk.
    ///
    /// A file that does not exist reads as `None` rather than an error — a
    /// failed import is a diagnostic, not a crash.
    pub fn read(&self, file: FileId) -> Option<FileText> {
        if let Some(text) = self.overlays.read().unwrap().get(&file) {
            return Some(text.clone());
        }
        match self.disk(file)? {
            Disk::Text(text) => Some(text),
            Disk::Absent | Disk::Unreadable(_) => None,
        }
    }

    /// Why [`Vfs::read`] answers `None` for a file that exists; `None` when
    /// it answers text, or the file is not there at all.
    pub fn read_failure(&self, file: FileId) -> Option<ReadFailure> {
        if self.has_overlay(file) {
            return None;
        }
        match self.disk(file)? {
            Disk::Unreadable(failure) => Some(failure),
            Disk::Text(_) | Disk::Absent => None,
        }
    }

    fn disk(&self, file: FileId) -> Option<Disk> {
        if let Some(cached) = self.disk.read().unwrap().get(&file) {
            return Some(cached.clone());
        }
        let path = self.path(file)?;
        let loaded = load(&path);
        self.disk.write().unwrap().insert(file, loaded.clone());
        Some(loaded)
    }

    pub fn read_path(&self, path: &Utf8Path) -> Option<FileText> {
        self.read(self.intern(path))
    }

    /// Number of interned files. Only for diagnostics and tests.
    pub fn len(&self) -> usize {
        self.paths.lock().unwrap().to_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn load(path: &Utf8Path) -> Disk {
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            // `hSetEncoding h utf8_bom` (`Internal.hs:27`): a byte-order mark
            // at the head of the file is the decoder's, and the program starts
            // after it — `kg-oracle --errors` compiles a file that opens with
            // one and counts its columns from past it (2026-10-10). Only the
            // head's; one anywhere else is a character like any other.
            Ok(mut text) => {
                if text.starts_with('\u{feff}') {
                    text.drain(..'\u{feff}'.len_utf8());
                }
                Disk::Text(FileText::new(text))
            }
            // GHC's decoder stops at the start of the sequence it cannot
            // decode, a sequence truncated by the end of the file included
            // (`readTextDevice'` hands the leftover bytes to `recover`) — which
            // is where `valid_up_to` points. Both reject surrogates, overlong
            // forms and anything past U+10FFFF, so the two name the same byte.
            Err(e) => {
                let byte = e.as_bytes()[e.utf8_error().valid_up_to()];
                Disk::Unreadable(ReadFailure::Undecodable { byte })
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && path.is_file() => {
            Disk::Unreadable(ReadFailure::PermissionDenied)
        }
        Err(_) => Disk::Absent,
    }
}

/// Normalizes a path *lexically* — no filesystem access.
///
/// Deliberately not `std::fs::canonicalize`: a path that does not exist yet (an
/// editor's unsaved buffer, an import that is about to fail, a file the user is
/// mid-rename on) still needs a key, and that key must not change when the file
/// appears or disappears. Resolving symlinks would buy de-duplication of two
/// routes to one file at the cost of unstable ids, which is a bad trade for a
/// long-lived server process.
fn canonical(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component.as_str() {
            "." => {}
            ".." => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_wins_over_disk() {
        let vfs = Vfs::new();
        let file = vfs.intern(Utf8Path::new("/nowhere/does-not-exist.k"));
        assert!(vfs.read(file).is_none());

        vfs.set_overlay(file, "inline void main() {}\n".to_owned());
        assert_eq!(vfs.read(file).unwrap().text, "inline void main() {}\n");
        assert!(vfs.has_overlay(file));

        vfs.remove_overlay(file);
        assert!(vfs.read(file).is_none());
    }

    #[test]
    fn the_same_path_interns_once() {
        let vfs = Vfs::new();
        let a = vfs.intern(Utf8Path::new("/a/./b.k"));
        let b = vfs.intern(Utf8Path::new("/a/b.k"));
        assert_eq!(a, b);
        assert_eq!(vfs.len(), 1);
    }

    #[test]
    fn reads_a_real_file_and_indexes_its_lines() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("kg-vfs-test-{}.k", std::process::id()));
        std::fs::write(&path, "a\nbb\nccc\n").unwrap();
        let path = Utf8PathBuf::from_path_buf(path).unwrap();

        let vfs = Vfs::new();
        let text = vfs.read_path(&path).unwrap();
        assert_eq!(text.text, "a\nbb\nccc\n");
        assert_eq!(text.line_index.line_count(), 4);

        std::fs::remove_file(&path).unwrap();
        // Still cached until explicitly invalidated, and the id is unchanged
        // even though the file is gone.
        let id = vfs.intern(&path);
        assert!(vfs.read(id).is_some());
        vfs.invalidate(id);
        assert!(vfs.read(id).is_none());
    }

    /// `test/syntax/imports/binary.k` is the four bytes `c0 de de ad`; the
    /// reference names 192 (`kg-oracle --errors`). A sequence the end of the
    /// file truncates names its lead byte, 226 for `e2 82` (probed the same
    /// way, 2026-10-10), and a directory is not a file that failed to read.
    #[test]
    fn a_file_that_is_not_utf8_exists_and_names_its_first_bad_byte() {
        let dir = std::env::temp_dir().join(format!("kg-vfs-utf8-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder.k")).unwrap();
        std::fs::write(dir.join("binary.k"), b"\xc0\xde\xde\xad").unwrap();
        std::fs::write(dir.join("truncated.k"), b"// ok \xc3\xa9 \xe2\x82").unwrap();
        let dir = Utf8PathBuf::from_path_buf(dir).unwrap();

        let vfs = Vfs::new();
        let binary = vfs.intern(&dir.join("binary.k"));
        assert!(vfs.read(binary).is_none());
        assert_eq!(vfs.read_failure(binary), Some(ReadFailure::Undecodable { byte: 192 }));
        let truncated = vfs.intern(&dir.join("truncated.k"));
        assert_eq!(vfs.read_failure(truncated), Some(ReadFailure::Undecodable { byte: 226 }));
        let folder = vfs.intern(&dir.join("folder.k"));
        assert!(vfs.read(folder).is_none());
        assert_eq!(vfs.read_failure(folder), None);
        let absent = vfs.intern(&dir.join("absent.k"));
        assert_eq!(vfs.read_failure(absent), None);
        // An editor's buffer is text whatever the disk holds.
        vfs.set_overlay(binary, "module binary {}\n".to_owned());
        assert_eq!(vfs.read_failure(binary), None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `utf8_bom` (`Internal.hs:27`) drops a leading byte-order mark and
    /// no other: the reference compiles `\u{feff}inline void f() {}` and
    /// reports an error after one at the column past it (`kg-oracle
    /// --errors`, 2026-10-10), where this read kept it and the lexer
    /// rejected the file.
    #[test]
    fn a_leading_byte_order_mark_is_the_decoders() {
        let dir = std::env::temp_dir().join(format!("kg-vfs-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("head.k"), "\u{feff}const uint8 x = y;\n").unwrap();
        std::fs::write(dir.join("body.k"), "a\u{feff}\n").unwrap();
        let dir = Utf8PathBuf::from_path_buf(dir).unwrap();

        let vfs = Vfs::new();
        assert_eq!(vfs.read_path(&dir.join("head.k")).unwrap().text, "const uint8 x = y;\n");
        assert_eq!(vfs.read_path(&dir.join("body.k")).unwrap().text, "a\u{feff}\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
