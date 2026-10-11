//! A disk cache of `kg-oracle` runs, keyed by everything a run reads.
//!
//! The reference binary is the slow half of two gates: `errors` spawns it
//! once per must-reject snippet (about 830 runs) and `ir` seven times over
//! every file of the reference's `library/` and `test/` trees — together
//! about four minutes of a full gate run, spent re-deriving outputs that
//! have not changed since the last run. The oracle is a pure function of
//! its binary, its arguments, the file it is given and the files that file
//! can import, so a run whose inputs are unchanged is a run whose output is
//! unchanged, and the key covers all of it:
//!
//! * the oracle binary's bytes — a rebuilt oracle misses everything;
//! * every `.k` file under the reference's `library/` and `test/`, by
//!   relative path and content — an edit anywhere in the reference tree
//!   misses everything, which is the only honest reading of a snippet that
//!   can import any of it;
//! * the arguments, the working directory, and the text of the file the
//!   oracle is given.
//!
//! Nothing this port computes is in the key, because nothing this port
//! computes is in the output: the cache stores the oracle's raw `stdout`
//! and `stderr`, and every reading of them — the position parsers, the
//! tree readers — happens after, on every run. A change to a gate's
//! reading of the oracle is graded fresh; only the spawn is skipped.
//!
//! The store is `target/oracle-cache/`, one JSON file per key, written
//! through a temporary name so eight worker threads never read a half
//! file; it can be deleted at any time. `KG_ORACLE_CACHE=0` bypasses it,
//! and each gate reports its hits and misses so a run that graded nothing
//! fresh says so.
//!
//! A run is found by its key, so a stored run can only be served for the
//! inputs it was recorded under — the key *is* the verification, and there
//! is no second copy of it to disagree. (`ptrace`'s store is addressed by
//! program name instead, and had to learn to check; AUDIT D7.)
//!
//! The hash is FNV-1a over 128 bits, written here rather than pulled in: the
//! inputs are local and trusted, a collision needs two different inputs to
//! meet in 2^128, and no crate in the workspace provides a stronger digest.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// What a run produced, whether spawned or replayed.
pub(crate) struct Cached {
    pub stdout: String,
    pub stderr: String,
}

static HITS: AtomicUsize = AtomicUsize::new(0);
static MISSES: AtomicUsize = AtomicUsize::new(0);
/// The environment key, computed once per reference root per process.
static ENVIRONMENT: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// Run the oracle — or replay it. `root` is the reference tree the file can
/// import from (`kanagawa/`), `args` the full argument list including the
/// file, `cwd` the working directory the gate spawns in, `text` the file's
/// content. The spawn's own failure is the only error.
pub(crate) fn run(
    oracle: &Path,
    root: &Path,
    args: &[String],
    cwd: Option<&Path>,
    text: &str,
) -> std::io::Result<Cached> {
    Ok(run_limited(oracle, root, args, cwd, text, None)?.expect("an unlimited run always finishes"))
}

/// [`run`], killing a spawn that outlives `limit` and answering `None` for
/// it. A killed run is never stored: what it would have printed is unknown,
/// and a replayed timeout would outlive the machine load that caused it.
/// The corpus gates pass no limit; the differential fuzzer and the reducer
/// feed the reference programs no one wrote, and one that loops must cost a
/// slot, not the run.
pub(crate) fn run_limited(
    oracle: &Path,
    root: &Path,
    args: &[String],
    cwd: Option<&Path>,
    text: &str,
    limit: Option<std::time::Duration>,
) -> std::io::Result<Option<Cached>> {
    let enabled = std::env::var_os("KG_ORACLE_CACHE").is_none_or(|v| v != "0");
    let slot = enabled.then(|| {
        let mut h = Fnv::new();
        h.write_str(&environment(oracle, root));
        for a in args {
            h.write_str(a);
        }
        h.write_str(&cwd.map(|c| c.to_string_lossy().into_owned()).unwrap_or_default());
        h.write_str(text);
        store().join(h.finish())
    });
    if let Some(path) = &slot
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let (Some(stdout), Some(stderr)) = (v["stdout"].as_str(), v["stderr"].as_str())
    {
        HITS.fetch_add(1, Ordering::Relaxed);
        return Ok(Some(Cached { stdout: stdout.to_owned(), stderr: stderr.to_owned() }));
    }
    let mut command = std::process::Command::new(oracle);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    // Memory is bounded whether or not time is (`crate::bounded`): a run
    // killed for memory is an error, not a slot — no gate here grades it.
    let limits = crate::bounded::Limits {
        wall: limit.unwrap_or(std::time::Duration::from_secs(24 * 3600)),
        ..crate::bounded::Limits::compile()
    };
    let output = match crate::bounded::run(command, limits)? {
        Ok(output) => output,
        Err((crate::bounded::Over::Time(_), _)) => return Ok(None),
        Err((over @ crate::bounded::Over::Memory(_), _)) => {
            return Err(std::io::Error::other(format!("kg-oracle {}: {over}", args.join(" "))));
        }
    };
    MISSES.fetch_add(1, Ordering::Relaxed);
    let cached = Cached {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    if let Some(path) = slot {
        let value = serde_json::json!({ "stdout": cached.stdout, "stderr": cached.stderr });
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::create_dir_all(store()).is_ok()
            && std::fs::write(&tmp, value.to_string()).is_ok()
            && std::fs::rename(&tmp, &path).is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    Ok(Some(cached))
}

/// `(hits, misses)` so far in this process, and a line for a gate's report.
pub(crate) fn report() -> String {
    let (h, m) = (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed));
    if std::env::var_os("KG_ORACLE_CACHE").is_some_and(|v| v == "0") {
        format!("oracle cache: off, {m} run(s) spawned")
    } else {
        format!("oracle cache: {h} replayed, {m} spawned (target/oracle-cache, KG_ORACLE_CACHE=0 bypasses)")
    }
}

fn store() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the workspace").join("target/oracle-cache")
}

/// The hash of the oracle binary and of every `.k` file under `library/`
/// and `test/` of the reference tree — what any run may read besides its
/// own arguments. About 480 files; computed once per process.
fn environment(oracle: &Path, root: &Path) -> String {
    let mut guard = ENVIRONMENT.lock().expect("no panics hold this lock");
    if let Some((r, key)) = guard.as_ref()
        && r == root
    {
        return key.clone();
    }
    let mut h = Fnv::new();
    h.write(&std::fs::read(oracle).unwrap_or_default());
    write_k_tree(&mut h, root);
    let key = h.finish();
    *guard = Some((root.to_path_buf(), key.clone()));
    key
}

/// Every `.k` file under `library/` and `test/` of the reference tree, by
/// relative path and content, into `h` — what any program there can import.
/// `ptrace`'s store is keyed by the same walk (AUDIT D7), so the two caches
/// cannot disagree about what "the inputs changed" means.
pub(crate) fn write_k_tree(h: &mut Fnv, root: &Path) {
    for path in k_tree(root) {
        let rel = path.strip_prefix(root).unwrap_or(&path);
        h.write_str(&rel.to_string_lossy());
        h.write(&std::fs::read(&path).unwrap_or_default());
        h.write(&[0]);
    }
}

/// The files [`write_k_tree`] hashes, sorted.
pub(crate) fn k_tree(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in ["library", "test"] {
        collect_k(&root.join(dir), &mut files);
    }
    files.sort();
    files
}

fn collect_k(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_k(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

/// FNV-1a, 128-bit (offset basis and prime from the FNV specification).
pub(crate) struct Fnv(u128);

impl Fnv {
    pub(crate) fn new() -> Fnv {
        Fnv(0x6c62_272e_07bb_0142_62b8_2175_6295_c58d)
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u128::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013b);
        }
    }

    /// A string, NUL-terminated so `"ab","c"` and `"a","bc"` differ.
    pub(crate) fn write_str(&mut self, s: &str) {
        self.write(s.as_bytes());
        self.write(&[0]);
    }

    pub(crate) fn finish(&self) -> String {
        format!("{:032x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Fnv;

    #[test]
    fn the_hash_separates_strings_and_is_stable() {
        let key = |parts: &[&str]| {
            let mut h = Fnv::new();
            for p in parts {
                h.write_str(p);
            }
            h.finish()
        };
        assert_ne!(key(&["ab", "c"]), key(&["a", "bc"]));
        assert_eq!(key(&["x"]), key(&["x"]));
        // The FNV-1a 128 test vector for the empty input is the offset basis.
        assert_eq!(Fnv::new().finish(), "6c62272e07bb014262b821756295c58d");
    }
}
