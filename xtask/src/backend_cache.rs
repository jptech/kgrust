//! The backend verdict cache as the gates use it (`kg_ptbridge::cache`;
//! `docs/plans/performance.md` §4).
//!
//! `backend`, `output` and `repoint` start our compiler once per program,
//! and each start used to run the C++ backend; now each passes
//! `KG_BACKEND_CACHE=target/backend-cache`, and a program whose walk has not
//! changed since the last run replays the backend's answer. The xtask's own
//! environment wins: `KG_BACKEND_CACHE=` (empty) turns the cache off, and
//! `KG_BACKEND_CACHE_VERIFY=N` (inherited by the compilers) re-runs one hit
//! in `N` and records any difference, which [`Watch::check`] turns into a
//! failed gate.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::Result;
use kg_ptbridge::cache::{BUDGET_MB_VAR, BUDGET_VAR, DIR_VAR, FILES_VAR, MISMATCHES};

pub(crate) fn dir() -> PathBuf {
    match std::env::var_os(DIR_VAR) {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/backend-cache"),
    }
}

/// Run `command` through the cache; `files` names the directory, relative
/// to its working directory, whose written files the gate reads.
pub(crate) fn apply(command: &mut Command, files: Option<&str>) {
    if std::env::var_os(DIR_VAR).is_none() {
        command.env(DIR_VAR, dir());
    }
    // Below `crate::bounded`'s 900 s kill, which would take the answer with
    // it: a run past this is a crash either way, and kept as one.
    if std::env::var_os(BUDGET_VAR).is_none() {
        command.env(BUDGET_VAR, "600");
    }
    // The group's own memory budget, which `kg` samples more often.
    if std::env::var_os(BUDGET_MB_VAR).is_none() {
        command.env(BUDGET_MB_VAR, crate::bounded::Limits::compile().rss_mb.to_string());
    }
    match files {
        Some(f) => command.env(FILES_VAR, f),
        None => command.env_remove(FILES_VAR),
    };
}

/// Above this the cache is pruned, least recently used first, to half of
/// it. One generation of entries — every program once — is ~400 MB.
const CAP_BYTES: u64 = 4 << 30;

/// A gate's run, watched for verify-mode mismatches written during it.
pub(crate) struct Watch(SystemTime);

impl Watch {
    /// Prune the cache if it has outgrown its cap, then start watching.
    pub(crate) fn start() -> Watch {
        prune(&dir(), CAP_BYTES);
        Watch(SystemTime::now())
    }

    /// Red when a verified hit disagreed with a fresh backend run during
    /// this gate: the cache's premise failed, and every hit is suspect. A
    /// disagreement only in source locations is the backend's own
    /// nondeterminism (`kg_ptbridge::locations`) and is reported, not red.
    pub(crate) fn check(&self) -> Result<()> {
        let Ok(entries) = std::fs::read_dir(dir().join(MISMATCHES)) else { return Ok(()) };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let fresh = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= self.0);
            if fresh {
                let what = std::fs::read_to_string(entry.path()).unwrap_or_default();
                let line = format!("  {}: {}", entry.path().display(), what.trim().replace('\n', "; "));
                if kg_ptbridge::cache::location_only(&what) {
                    println!("backend cache: a fresh run differed from the stored one in source locations only — the backend's own nondeterminism:\n{line}");
                } else {
                    found.push(line);
                }
            }
        }
        if found.is_empty() {
            return Ok(());
        }
        found.sort();
        anyhow::bail!(
            "the backend cache disagreed with a fresh backend run {} time(s) — its premise failed; \
             clear {} and investigate:\n{}",
            found.len(),
            dir().display(),
            found.join("\n")
        )
    }
}

/// Each entry's directory, its size, and when it was last used (its
/// `header`'s modified time, which a hit refreshes).
fn entries(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut out = Vec::new();
    let Ok(shards) = std::fs::read_dir(dir) else { return out };
    for shard in shards.flatten() {
        if shard.file_name() == MISMATCHES {
            continue;
        }
        let Ok(list) = std::fs::read_dir(shard.path()) else { continue };
        for entry in list.flatten() {
            let path = entry.path();
            let used = std::fs::metadata(path.join("header")).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((path.clone(), size(&path), used));
        }
    }
    out
}

fn size(path: &Path) -> u64 {
    let Ok(list) = std::fs::read_dir(path) else { return 0 };
    list.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Above `cap` bytes, remove least recently used entries until at most
/// half of it is left. A gate reading an entry as it goes misses and runs
/// the backend: pruning never changes an answer.
fn prune(dir: &Path, cap: u64) {
    let mut all = entries(dir);
    let mut total: u64 = all.iter().map(|e| e.1).sum();
    if total <= cap {
        return;
    }
    all.sort_by_key(|e| e.2);
    for (path, bytes, _) in all {
        if total <= cap / 2 {
            break;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            total -= bytes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over the cap, the least recently used go first, down to half the
    /// cap. Red with the sort reversed.
    #[test]
    fn pruning_keeps_the_most_recently_used() {
        let dir = std::env::temp_dir().join(format!("xtask-backend-cache-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = SystemTime::now();
        for (i, age) in [30u64, 10, 20].into_iter().enumerate() {
            let entry = dir.join("ab").join(format!("e{i}"));
            std::fs::create_dir_all(&entry).expect("entry");
            std::fs::write(entry.join("recording"), vec![0u8; 100]).expect("write");
            std::fs::write(entry.join("header"), "h").expect("write");
            let header = std::fs::File::options().write(true).open(entry.join("header")).expect("open");
            header.set_modified(now - std::time::Duration::from_secs(age)).expect("mtime");
        }
        prune(&dir, 250);
        let left: Vec<String> =
            entries(&dir).into_iter().map(|e| e.0.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["e1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
