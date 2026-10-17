//! Where the flows' tools are: a program on `PATH`, as a shell would find
//! it. Locating, not deciding — what a tool computes is not changed by
//! where it is found, so this is the one place `kg-board` reads the
//! environment (`xtask lint`'s `env-verdict`).

use camino::Utf8PathBuf;

/// A program on `PATH`.
pub fn which(program: &str) -> Option<Utf8PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter_map(|d| Utf8PathBuf::from_path_buf(d).ok())
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}
