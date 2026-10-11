//! `cargo xtask ratchet --rebase <commit>`: the ledgers' assertion names
//! carried across a move of the reference pin (sprint W6).
//!
//! An assertion is named by its position — `syntax::basics::17`, the 18th
//! marker of `basics.k` (`conformance::expand`) — so a snippet added
//! upstream renames every one after it, in every ratchet that names
//! assertions at once, and the next run reads as hundreds of regressions
//! and as many improvements. This maps each name the reference at `<commit>`
//! gave to the name the current checkout gives the same snippet — the same
//! test file, the same expectation, the same text — and rewrites
//! `baseline.json`'s items through it. A snippet with no unique twin (edited
//! or deleted upstream) keeps its old name and is listed: what it graded is
//! a question for the next run, not something to carry over by guessing.
//!
//! The old corpus is read with `git archive` from the reference checkout,
//! which is only read — nothing under it is written.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::conformance::Assertion;

/// Old name → new name, for every old assertion: `None` where no snippet of
/// the new corpus is its unique twin. Twins are matched by file, expectation
/// and text; identical snippets in one file are matched in order, the k-th
/// old to the k-th new, where both sides have as many.
pub(crate) fn rename_map(old: &[Assertion], new: &[Assertion]) -> BTreeMap<String, Option<String>> {
    type Key = (String, String, String);
    let key = |a: &Assertion| -> Key {
        let file = a.name.rsplit_once("::").map_or(a.name.as_str(), |(f, _)| f).to_owned();
        (file, format!("{:?}", a.expectation), a.source.clone())
    };
    let mut by_key: BTreeMap<Key, (Vec<&str>, Vec<&str>)> = BTreeMap::new();
    for a in old {
        by_key.entry(key(a)).or_default().0.push(&a.name);
    }
    for a in new {
        by_key.entry(key(a)).or_default().1.push(&a.name);
    }
    let mut out = BTreeMap::new();
    for (olds, news) in by_key.values() {
        let paired = olds.len() == news.len();
        for (i, o) in olds.iter().enumerate() {
            out.insert((*o).to_owned(), paired.then(|| news[i].to_owned()));
        }
    }
    out
}

/// The reference's `test/syntax` at `commit`, unpacked under `into`, which
/// then stands for a reference root to `conformance::load_all`.
fn unpack_old_corpus(kanagawa_root: &Path, commit: &str, into: &Path) -> Result<()> {
    if into.exists() {
        std::fs::remove_dir_all(into)?;
    }
    std::fs::create_dir_all(into)?;
    let archive = Command::new("git")
        .args(["-C"])
        .arg(kanagawa_root)
        .args(["archive", "--format=tar", commit, "test/syntax"])
        .output()
        .context("running git archive in the reference checkout")?;
    anyhow::ensure!(
        archive.status.success(),
        "git archive {commit} test/syntax: {}",
        String::from_utf8_lossy(&archive.stderr).trim()
    );
    let mut tar = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(into)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("running tar")?;
    std::io::Write::write_all(tar.stdin.as_mut().expect("piped"), &archive.stdout)?;
    anyhow::ensure!(tar.wait()?.success(), "tar could not unpack the old corpus");
    Ok(())
}

/// Rewrites every ledger item of `baseline.json` that names an assertion of
/// the reference at `commit` to the name the current reference gives the
/// same snippet, and reports what moved and what could not be carried.
pub fn run(kanagawa_root: &Path, commit: &str) -> Result<()> {
    let kanagawa_root = &kanagawa_root.canonicalize().with_context(|| format!("resolving {}", kanagawa_root.display()))?;
    let scratch = crate::fresh::workspace().join("target/rebase-corpus");
    unpack_old_corpus(kanagawa_root, commit, &scratch)?;
    let old = crate::conformance::load_all(&scratch)?;
    let new = crate::conformance::load_all(kanagawa_root)?;
    let map = rename_map(&old, &new);
    let (renamed, unmapped) = crate::ratchet::rename_items(&map)?;
    let moved = map.iter().filter(|(o, n)| n.as_deref().is_some_and(|n| n != o.as_str())).count();
    let twinless = map.values().filter(|n| n.is_none()).count();
    println!(
        "rebase: {} assertions at {commit}, {} now — {moved} renamed by the move, {twinless} with no twin; \
         {renamed} ledger item(s) renamed in baseline.json",
        old.len(),
        new.len()
    );
    if !unmapped.is_empty() {
        println!("  {} item(s) name a snippet with no unique twin now — kept as they were; the next run grades them:", unmapped.len());
        for (gate, item) in &unmapped {
            println!("    {gate}: {item}");
        }
    }
    println!("  read `git diff baseline.json` before committing it");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conformance::Expectation;

    fn a(name: &str, source: &str, code: u32) -> Assertion {
        Assertion {
            name: name.to_owned(),
            file: Default::default(),
            marker_line: 0,
            source: source.to_owned(),
            expectation: Expectation::Code(code),
            registered: true,
            options: Arc::from(Vec::<String>::new()),
        }
    }

    /// A snippet inserted upstream shifts every later name, and each maps
    /// to its own text's new place; an edited snippet has no twin; two
    /// identical snippets map in order. Red with the map keyed by name, or
    /// by text alone (across files).
    #[test]
    fn names_follow_their_snippets_across_a_shift() {
        let old = [
            a("syntax::basics::0", "uint8 a;\n", 0),
            a("syntax::basics::1", "uint8 b;\n", 1),
            a("syntax::basics::2", "same;\n", 1),
            a("syntax::basics::3", "same;\n", 1),
            a("syntax::basics::4", "was;\n", 0),
            a("syntax::other::0", "uint8 a;\n", 0),
        ];
        // Another file's identical snippet first: a key by text alone would
        // pair `basics::0` with it.
        let new = [
            a("syntax::other::0", "uint8 a;\n", 0),
            a("syntax::basics::0", "inserted;\n", 0),
            a("syntax::basics::1", "uint8 a;\n", 0),
            a("syntax::basics::2", "uint8 b;\n", 1),
            a("syntax::basics::3", "same;\n", 1),
            a("syntax::basics::4", "same;\n", 1),
            a("syntax::basics::5", "edited;\n", 0),
        ];
        let map = rename_map(&old, &new);
        let get = |k: &str| map[k].as_deref();
        assert_eq!(get("syntax::basics::0"), Some("syntax::basics::1"));
        assert_eq!(get("syntax::basics::1"), Some("syntax::basics::2"));
        assert_eq!(get("syntax::basics::2"), Some("syntax::basics::3"));
        assert_eq!(get("syntax::basics::3"), Some("syntax::basics::4"));
        assert_eq!(get("syntax::basics::4"), None, "edited upstream: no twin");
        assert_eq!(get("syntax::other::0"), Some("syntax::other::0"), "another file's same text stays its own");
    }

    /// A snippet whose expectation changed upstream is not the same
    /// assertion, whatever its text.
    #[test]
    fn an_expectation_changed_upstream_is_no_twin() {
        let map = rename_map(&[a("syntax::f::0", "x;\n", 0)], &[a("syntax::f::0", "x;\n", 1)]);
        assert_eq!(map["syntax::f::0"], None);
    }
}
