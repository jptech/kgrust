//! The reference tree is read-only, and this is the check that says so.
//!
//! A gate that spawns the reference compiler leaves `<base>PathLength.pdrpt`
//! in its working directory (`kanagawa.cpp:145-158`), and until the cwds were
//! fixed on 2026-09-19 those landed in `../kanagawa/test/` — 163 of them,
//! found by reading the tree rather than by any gate saying so (journal, "Two
//! gates wrote into the reference"). Six more appeared on 2026-09-19 that no
//! gate reproduces, which is the argument for this file: a rule nobody can
//! *check* is a rule that gets broken quietly, and every gate here runs long
//! enough that nobody watches the tree while it does.
//!
//! So every subcommand is bracketed: the corpus directories are listed before
//! it runs and after, and a path that appeared is reported and fails the run.
//! Deletions are not policed — the reference is the user's checkout, and a
//! `git pull` there is not this driver's business.
//!
//! **It never removes what it finds.** `CLAUDE.md` forbids writing *and*
//! deleting under `../kanagawa`, cleanup included: a file that should not be
//! there is evidence, and the gate that made it is the thing to fix.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Every file under the two corpus directories of `root` — `library/` and
/// `test/`, which is what the gates read and the only place a spawned
/// compiler has ever written. 727 files, about 7 ms; the rest of a reference
/// checkout is its own build tree and sources, which no gate goes near.
pub(crate) fn snapshot(root: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for dir in ["library", "test"] {
        walk(&root.join(dir), &mut out);
    }
    out
}

fn walk(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out),
            Ok(_) => {
                out.insert(path);
            }
            Err(_) => {}
        }
    }
}

/// The paths `after` holds and `before` did not, in tree order.
pub(crate) fn gained(before: &BTreeSet<PathBuf>, after: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    after.difference(before).cloned().collect()
}

/// Compare the tree against `before` and fail the run if it grew, naming
/// every path. The command's own output has already been printed, so its
/// numbers stand and this is the last word.
pub(crate) fn check(root: &Path, before: &BTreeSet<PathBuf>) -> Result<()> {
    let grew = gained(before, &snapshot(root));
    if grew.is_empty() {
        return Ok(());
    }
    eprintln!("\nthe reference tree gained {} file(s) during this run:", grew.len());
    for path in &grew {
        eprintln!("  {}", path.display());
    }
    eprintln!(
        "`{}` is read-only. Whatever spawned a compiler ran it with its working \
         directory inside that tree; fix the cwd, and leave these where they are.",
        root.display()
    );
    anyhow::bail!("the reference tree gained {} file(s)", grew.len())
}

/// The pin: `reference.lock` at the workspace root names the reference
/// every gate grades against (sprint W0.4) — its commit, and a hash of every
/// file under `library/` and `test/`. A gate graded against another
/// reference grades another answer key, and nothing said so until the
/// reference's commit was written down: `kanagawa/` had moved six commits
/// past the one the CI workflow pinned (journal, "The foundation survey").
pub(crate) const LOCK: &str = "reference.lock";

/// What the lock holds.
#[derive(Debug, PartialEq)]
pub(crate) struct Pin {
    pub commit: String,
    pub inputs: String,
}

impl Pin {
    fn parse(text: &str) -> Option<Pin> {
        let mut commit = None;
        let mut inputs = None;
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            match line.split_once(' ') {
                Some(("commit", v)) => commit = Some(v.trim().to_owned()),
                Some(("inputs", v)) => inputs = Some(v.trim().to_owned()),
                _ => return None,
            }
        }
        Some(Pin { commit: commit?, inputs: inputs? })
    }

    fn render(&self) -> String {
        format!(
            "# The reference checkout every xtask gate grades against (xtask/src/guard.rs).\n\
             # Each command that reads it checks first: HEAD is this commit, nothing\n\
             # under library/ or test/ differs from it or is untracked, and every file\n\
             # there hashes to `inputs`. Move it only on purpose, after a reference sync:\n\
             # `cargo xtask pin-reference --write`, and every ratchet re-read.\n\
             commit {}\ninputs {}\n",
            self.commit, self.inputs
        )
    }
}

/// Every file under `library/` and `test/`, by relative path and content.
fn inputs_hash(root: &Path) -> String {
    let mut h = crate::oracle_cache::Fnv::new();
    h.write_str("reference inputs v1");
    for path in snapshot(root) {
        let rel = path.strip_prefix(root).unwrap_or(&path);
        h.write_str(&rel.to_string_lossy());
        h.write(&std::fs::read(&path).unwrap_or_default());
        h.write(&[0]);
    }
    h.finish()
}

/// `git` in the reference, read-only: `--no-optional-locks` keeps `status`
/// from refreshing the index, which would be a write. `None` when the
/// reference is not a git checkout.
pub(crate) fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("--no-optional-locks")
        .args(args)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The reference as it stands.
pub(crate) fn current_pin(root: &Path) -> Pin {
    Pin { commit: git(root, &["rev-parse", "HEAD"]).unwrap_or_else(|| "none".to_owned()), inputs: inputs_hash(root) }
}

/// Refuse a reference other than the pinned one, naming every difference.
pub(crate) fn check_pin(root: &Path, lock: &Path) -> Result<()> {
    let text = std::fs::read_to_string(lock)
        .map_err(|e| anyhow::anyhow!("{}: {e} — the reference is unpinned; `cargo xtask pin-reference --write`", lock.display()))?;
    let Some(pin) = Pin::parse(&text) else {
        anyhow::bail!("{} is not a pin (`commit <sha>` and `inputs <hash>` lines)", lock.display());
    };
    let mut problems = Vec::new();
    if let Some(head) = git(root, &["rev-parse", "HEAD"]) {
        if head != pin.commit {
            problems.push(format!("HEAD is {head}, the pin {}", pin.commit));
        }
        let dirty = git(root, &["status", "--porcelain", "--untracked-files=all", "--", "library", "test"]).unwrap_or_default();
        for line in dirty.lines().take(10) {
            problems.push(format!("not as committed: {line}"));
        }
    } else if pin.commit != "none" {
        problems.push(format!("not a git checkout, and the pin names commit {}", pin.commit));
    }
    let inputs = inputs_hash(root);
    if inputs != pin.inputs {
        problems.push(format!("library/ and test/ hash to {inputs}, the pin {}", pin.inputs));
    }
    if problems.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "the reference at {} is not the one {} pins:\n  {}\nEvery gate grades against the pinned reference. After a deliberate sync, \
         `cargo xtask pin-reference --write` moves the pin — and every ratchet's baseline must be re-read against it.",
        root.display(),
        lock.display(),
        problems.join("\n  ")
    )
}

/// `cargo xtask pin-reference`: print the pin the reference would have, and
/// with `write`, move `reference.lock` to it.
pub fn pin_reference(root: &Path, write: bool) -> Result<()> {
    let pin = current_pin(root);
    let lock = crate::fresh::workspace().join(LOCK);
    let old = std::fs::read_to_string(&lock).ok().and_then(|t| Pin::parse(&t));
    println!("reference {}: commit {} inputs {}", root.display(), pin.commit, pin.inputs);
    match &old {
        Some(o) if *o == pin => println!("the pin already names it"),
        Some(o) => println!("the pin names commit {} inputs {}", o.commit, o.inputs),
        None => println!("no pin yet"),
    }
    if write {
        std::fs::write(&lock, pin.render())?;
        println!("wrote {}", lock.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference that is not a git checkout is pinned by its hash alone:
    /// an edit to any file under `test/` — not only a `.k` file — or a file
    /// added there, is refused. Red with the hash left out of the check.
    #[test]
    fn the_pin_refuses_an_edited_or_grown_reference() {
        let root = std::env::temp_dir().join(format!("kg-pin-{}", std::process::id()));
        std::fs::create_dir_all(root.join("library")).expect("scratch tree");
        std::fs::create_dir_all(root.join("test")).expect("scratch tree");
        std::fs::write(root.join("test/CMakeLists.txt"), "add_syntax_test(a)\n").expect("write");
        let lock = root.join("reference.lock");
        let pin = Pin { commit: "none".to_owned(), inputs: inputs_hash(&root) };
        assert_eq!(Pin::parse(&pin.render()), Some(Pin { commit: "none".to_owned(), inputs: pin.inputs.clone() }));
        std::fs::write(&lock, pin.render()).expect("write");
        assert!(check_pin(&root, &lock).is_ok());
        std::fs::write(root.join("test/CMakeLists.txt"), "add_syntax_test(b)\n").expect("write");
        assert!(check_pin(&root, &lock).is_err(), "an edit is refused");
        std::fs::write(root.join("test/CMakeLists.txt"), "add_syntax_test(a)\n").expect("write");
        assert!(check_pin(&root, &lock).is_ok());
        std::fs::write(root.join("test/aMainPathLength.pdrpt"), "report\n").expect("write");
        assert!(check_pin(&root, &lock).is_err(), "a stray file is refused");
        std::fs::remove_dir_all(&root).expect("scratch tree");
    }

    /// A file that appears under `test/` between the two listings is reported;
    /// one that was already there is not, and a *removed* one is not either —
    /// the reference is the user's checkout, and only growth is this driver's
    /// doing. Red with the difference taken the other way round, and red with
    /// `library/`'s subdirectories unwalked.
    #[test]
    fn a_file_that_appears_under_the_corpus_is_reported() {
        let root = std::env::temp_dir().join(format!("kg-guard-{}", std::process::id()));
        let nested = root.join("library").join("data");
        std::fs::create_dir_all(&nested).expect("scratch tree");
        std::fs::create_dir_all(root.join("test")).expect("scratch tree");
        std::fs::write(nested.join("array.k"), "// a corpus file\n").expect("write");
        let before = snapshot(&root);
        assert_eq!(before.len(), 1, "the nested file is listed");

        std::fs::write(root.join("test").join("aMainPathLength.pdrpt"), "report\n").expect("write");
        let after = snapshot(&root);
        let grew = gained(&before, &after);
        assert_eq!(grew, vec![root.join("test").join("aMainPathLength.pdrpt")]);
        assert!(check(&root, &before).is_err(), "and the run fails");

        std::fs::remove_file(root.join("test").join("aMainPathLength.pdrpt")).expect("remove");
        std::fs::remove_file(nested.join("array.k")).expect("remove");
        assert!(gained(&before, &snapshot(&root)).is_empty(), "a removal is not growth");
        assert!(check(&root, &before).is_ok());
        std::fs::remove_dir_all(&root).expect("scratch tree");
    }
}
