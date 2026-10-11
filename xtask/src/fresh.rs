//! Which binary a gate grades, and which tree a ledger was written from.
//!
//! `backend` and `repoint` spawn our `kanagawa` compat binary. Until
//! 2026-09-22 they spawned whatever `target/release/kanagawa` happened to
//! exist (AUDIT D8), so a gate run after an edit and before a rebuild graded
//! the previous tree and said nothing — the memory notes' "rebuild before
//! gating" was the only guard. A timestamp comparison cannot be exact here:
//! `crates/` holds crates the binary does not link, so an edit to one of them
//! makes any binary look "older than the source" while cargo, rightly, has
//! nothing to rebuild. Cargo is the one party that knows what the binary
//! depends on, so the gate asks it: `cargo build --release -p kg-cli --bin
//! kanagawa`, which is a 0.1 s no-op on a fresh tree, and grades the
//! executable cargo *reports*, never a path assumed to be where cargo wrote
//! it (`CARGO_TARGET_DIR` moves it).
//!
//! The tree fingerprint stamps each ratchet ledger (`crate::ratchet`) with
//! the content of every input the gates' verdicts can depend on, so a ledger
//! left over from an older tree can be named stale rather than blessed.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::oracle_cache::Fnv;

/// The workspace root.
pub(crate) fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the workspace").to_path_buf()
}

/// Build `kanagawa` if cargo says it is out of date, and return the
/// executable cargo built. Loud either way: the line says which binary is
/// graded and whether it had to be rebuilt.
pub(crate) fn compat_binary() -> Result<PathBuf> {
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO"))
        .args(["build", "--release", "-p", "kg-cli", "--bin", "kanagawa", "--message-format=json-render-diagnostics"])
        .current_dir(workspace())
        .stderr(std::process::Stdio::inherit())
        .output()
        .context("spawning cargo to build the compat binary")?;
    if !output.status.success() {
        anyhow::bail!("`cargo build --release -p kg-cli --bin kanagawa` failed — refusing to grade a stale binary");
    }
    let mut found: Option<(PathBuf, bool)> = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["reason"] == "compiler-artifact"
            && v["target"]["name"] == "kanagawa"
            && let Some(exe) = v["executable"].as_str()
        {
            found = Some((PathBuf::from(exe), v["fresh"].as_bool().unwrap_or(false)));
        }
    }
    let Some((exe, fresh)) = found else {
        anyhow::bail!("cargo built kg-cli but reported no `kanagawa` executable");
    };
    if fresh {
        println!("binary: {} (up to date with the tree, checked by cargo)", exe.display());
    } else {
        println!(
            "binary: {} was STALE — rebuilt from the tree in {:.1}s before grading",
            exe.display(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(exe)
}

/// A binary someone else built, named on the command line: said out loud,
/// since nothing here can vouch for which tree it came from.
pub(crate) fn announce_external(binary: &Path) {
    println!("binary: {} — given by --binary, NOT checked against the tree", binary.display());
}

/// The inputs a gate's verdict can depend on, by relative path and content:
/// every file under `crates/` and `xtask/` except build output, the
/// workspace manifests, the reference pin and the oracle's driver.
/// `baseline.json` sits at the root and is not one of
/// them — blessing it must not make the ledgers it was blessed from stale.
pub(crate) fn tree_fingerprint() -> String {
    let root = workspace();
    let mut files = Vec::new();
    for dir in ["crates", "xtask"] {
        collect_files(&root.join(dir), &mut files);
    }
    // The pinned reference (`guard::LOCK`) and the oracle's driver are
    // inputs too: a moved pin changes every verdict, a changed driver what
    // `errors`, `ir` and `loop-left` read.
    for f in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "reference.lock", "oracle/driver/Main.hs", "oracle/oracle.cabal"] {
        files.push(root.join(f));
    }
    files.sort();
    let mut h = Fnv::new();
    for path in files {
        h.write_str(&path.strip_prefix(&root).unwrap_or(&path).to_string_lossy());
        h.write(&std::fs::read(&path).unwrap_or_default());
        h.write(&[0]);
    }
    h.finish()
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" || name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}
