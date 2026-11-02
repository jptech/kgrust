//! The version a build from a checkout reports: `Cargo.toml`'s, `-dev`, and the
//! commit, `0.2.0-dev+abc1234`, so that what someone runs can be told from a
//! release and from another commit. A release and a nightly name themselves
//! instead, through `KG_VERSION` (`modern::VERSION`).
//!
//! Only this crate rebuilds when the commit moves: about 3 s in `release`,
//! under 1 s in `quick` (2026-11-01, `touch` and `cargo build`, load 15). Only
//! `kg --version`, the language server's `serverInfo` and `kanagawa`'s second
//! version line read it, so no compiled output depends on the commit.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let package = std::env::var("CARGO_PKG_VERSION").expect("set by cargo");
    let commit = if in_own_checkout(&manifest) { git(&manifest, &["rev-parse", "--short=7", "HEAD"]) } else { None };
    match commit {
        Some(commit) => {
            watch_head(&manifest);
            println!("cargo:rustc-env=KG_DEV_VERSION={package}-dev+{commit}");
        }
        // A source archive has no commit to name, and git would answer for
        // whichever repository it happens to be unpacked inside.
        None => println!("cargo:rustc-env=KG_DEV_VERSION={package}-dev"),
    }
}

/// Whether git's repository here is this workspace's own (`crates/kg-cli`).
fn in_own_checkout(manifest: &Path) -> bool {
    let Some(top) = git(manifest, &["rev-parse", "--show-toplevel"]) else { return false };
    let (Ok(top), Ok(workspace)) = (Path::new(&top).canonicalize(), manifest.join("../..").canonicalize()) else {
        return false;
    };
    top == workspace
}

/// Runs this script again when the commit moves: `HEAD` for a checkout or a
/// detached commit, the branch's ref for a commit on it. A ref with no loose
/// file is packed, and its next commit creates the file, so then the directory
/// is what changes. Paths come from git because a linked worktree keeps `HEAD`
/// and the refs in different places.
fn watch_head(manifest: &Path) {
    let path = |name: &str| git(manifest, &["rev-parse", "--git-path", name]).map(|p| manifest.join(p));
    let watch = |p: PathBuf| println!("cargo:rerun-if-changed={}", p.display());
    path("HEAD").into_iter().for_each(watch);
    if let Some(reference) = git(manifest, &["symbolic-ref", "-q", "HEAD"]) {
        match path(&reference) {
            Some(loose) if loose.exists() => watch(loose),
            _ => path("refs/heads").into_iter().for_each(watch),
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}
