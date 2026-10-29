//! Where the backend library is looked for, in order:
//!
//! 1. `$KG_BACKEND_DIR/<lib>` — the explicit answer, and then the only one
//!    (`none` or empty: no backend at all);
//! 2. beside the running executable, which is where the reference keeps it
//!    (`@loader_path` / `$ORIGIN`, `abi.md` §0) and where an installed `kg`
//!    would. The executable's path is canonicalised first: `cargo xtask
//!    install` links `kg` into a bin directory from the tree that holds the
//!    library, as `kg-config` finds the platform and the standard library.
//!
//! There was a third, `<this crate>/../../../kanagawa-build/dist/bin` — the
//! developer's build of the reference, as a path compiled into every
//! binary. It never existed beside a worktree, an installed `kg` carried
//! the build machine's path, and the gates and scripts name the library
//! anyway (`KG_BACKEND_DIR`, `scripts/lib.sh`'s `kg_env`); it went with
//! sprint W8.4. A developer's run names it too:
//! `KG_BACKEND_DIR=target/backend/dist/bin` (`scripts/build-backend.sh`).
//!
//! The file name is the platform's: `libkanagawa-backend.dylib` on macOS,
//! `libkanagawa-backend.so` on Linux, `kanagawa-backend.dll` on Windows
//! (`compiler/cpp/CMakeLists.txt:56,228`).

use std::path::{Path, PathBuf};

/// The environment variable that names the directory holding the library.
pub const ENV_VAR: &str = "KG_BACKEND_DIR";

/// The library's file name on this platform.
pub const LIBRARY_FILE: &str = if cfg!(target_os = "macos") {
    "libkanagawa-backend.dylib"
} else if cfg!(target_os = "windows") {
    "kanagawa-backend.dll"
} else {
    "libkanagawa-backend.so"
};

/// Every path that would be tried, existing or not, in order. `KG_BACKEND_DIR`
/// set to `none` or empty means "no backend, whatever is on this machine" —
/// the frontend-only run a test pins.
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(1);
    if let Some(dir) = std::env::var_os(ENV_VAR) {
        if dir.is_empty() || dir == "none" {
            return out;
        }
        // Only the named directory: a gate names the library it grades
        // against, and a fallback found elsewhere would grade another one.
        out.push(PathBuf::from(dir).join(LIBRARY_FILE));
        return out;
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| exe_dir(&p)) {
        out.push(dir.join(LIBRARY_FILE));
    }
    out
}

/// The directory an executable really lives in, symlinks followed; its
/// lexical parent when it cannot be resolved.
fn exe_dir(exe: &Path) -> Option<PathBuf> {
    let real = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    real.parent().map(Path::to_path_buf)
}

/// The first candidate that exists.
pub fn backend_library() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

/// The directory `KG_BACKEND_DIR` names, when it names one (not `none`, not
/// empty) — a caller that finds no library there was told where it is and
/// must say it is missing rather than run without it.
pub fn named_dir() -> Option<PathBuf> {
    std::env::var_os(ENV_VAR).filter(|d| !d.is_empty() && d != "none").map(PathBuf::from)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_linked_executable_looks_beside_its_target() {
        let root = std::env::temp_dir().join(format!("kg-locate-{}", std::process::id()));
        let (real, bin) = (root.join("tree/bin"), root.join("bin"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(real.join("kg"), "").unwrap();
        std::os::unix::fs::symlink(real.join("kg"), bin.join("kg")).unwrap();
        let found = exe_dir(&bin.join("kg")).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(found.ends_with("tree/bin"), "{found:?}");
    }
}
