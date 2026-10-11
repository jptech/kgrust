//! The backend in a process of its own (AUDIT D6).
//!
//! `libkanagawa-backend` throws C++ exceptions out of the ABI — an
//! assertion anywhere in a `Parse*` call is a `RuntimeErrorWithTrace`
//! (`compiler.cpp:1561`), and only `InitCompiler` and `Codegen` catch
//! (`parse_tree.cpp:6285`, `kanagawa.cpp:50`) — and it can fault like any
//! C++. Run inside `kg build`, either took the whole frontend down with an
//! `abort` that said nothing about where. So [`compile`] splits the run in
//! two:
//!
//! 1. The walk goes into a [`Recording`] in this process. A program the walk
//!    declines is reported here and nothing reaches the library (§4.4: the
//!    backend never holds half a program).
//! 2. A worker — this same executable, re-run with [`WORKER_ARG`] — reads
//!    the recording on stdin and replays it into [`Real`]. It inherits
//!    stdout, stderr and the working directory, so what the backend prints
//!    and writes is byte for byte what it printed and wrote in-process; its
//!    exit code says how the replay ended, and anything else it can do — a
//!    signal, an unknown code — is a crash, reported as one.
//!
//! The worker's exit codes are deliberately not 0 or 1: a process that is
//! *not* the worker (a test harness standing in as `current_exe`, a wrong
//! binary) exits 0 or 1 for its own reasons, and must read as a crash, never
//! as a compile or a verdict.
//!
//! The bindings are `extern "C-unwind"` (`kg_backend_sys::raw`), so an
//! exception escaping a call unwinds through the replay as defined
//! behaviour; the worker's [`InFlight`] guard names the call on the way
//! out and aborts before the unwind reaches the runtime's
//! `catch_unwind`, which would abort anyway, but anonymously.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use kg_backend_sys::options::OptionValues;
use kg_base::FileId;
use kg_db::Db;

use crate::cache::{Budget, Cache, Captured};
use crate::program::CodegenArgs;
use crate::real::Real;
use crate::record::Recording;
use crate::walk::Declined;

/// The hidden first argument that makes `kg` or `kanagawa` the worker.
pub const WORKER_ARG: &str = "__kg-backend-worker";

const COMPILED: i32 = 40;
const REJECTED: i32 = 41;
const INIT_FAILED: i32 = 42;
const LOAD_FAILED: i32 = 43;
const BAD_RECORDING: i32 = 44;

/// How the backend's run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// `Codegen` returned true.
    Compiled,
    /// `Codegen` returned false: the backend rejected the program and has
    /// printed why.
    Rejected,
    /// `InitCompiler` returned false, having printed why.
    InitFailed,
    /// The library would not load; the worker printed why.
    LoadFailed,
    /// No verdict: the worker died, or ended in a way the protocol does not
    /// name. `how` says which, for a message.
    Crashed { how: String },
}

/// [`compile`]'s result.
#[derive(Debug)]
pub struct Isolated {
    /// The program's file list, as `_fileNames` carried it.
    pub file_names: Vec<String>,
    /// The node kind that stopped the walk; when set, the backend was never
    /// started.
    pub declined: Option<Declined>,
    /// The backend's run, when there was one.
    pub verdict: Option<Verdict>,
}

/// `compile` (`ParseTree.hs:31-41`) with the backend at arm's length: walk
/// into a recording, then replay it into the library at `library` in a
/// worker process.
pub fn compile(db: &Db, roots: &[FileId], options: OptionValues, codegen: &CodegenArgs, library: &Path) -> Isolated {
    let mut recording = Recording::new();
    let outcome = crate::program::run(db, roots, options, Some(codegen), &mut recording);
    let mut isolated = Isolated { file_names: outcome.file_names, declined: outcome.declined, verdict: None };
    if isolated.declined.is_none() {
        isolated.verdict = Some(Worker::this_executable().run(library, &recording));
    }
    isolated
}

/// The process that replays a recording: an executable whose `main` hands
/// [`WORKER_ARG`] to [`worker_main`].
#[derive(Clone, Debug)]
pub struct Worker {
    pub program: PathBuf,
}

impl Worker {
    /// The running executable — `kg` and `kanagawa` both dispatch the
    /// worker argument before anything else.
    pub fn this_executable() -> Worker {
        Worker { program: std::env::current_exe().unwrap_or_else(|_| PathBuf::from(std::env::args_os().next().unwrap_or_default())) }
    }

    /// Replay `recording` into the library at `library` and wait. Stdout
    /// and stderr are this process's own, so the caller flushes whatever it
    /// has buffered first. Under the gates' cache ([`Cache::from_env`]) the
    /// run may be a stored one, its streams and files replayed.
    pub fn run(&self, library: &Path, recording: &Recording) -> Verdict {
        match Cache::from_env() {
            Some(cache) => {
                let run = self.run_cached(&cache, library, &recording.encode());
                crate::cache::forward(&run);
                run.verdict
            }
            None => self.run_inherited(library, &recording.encode()),
        }
    }

    fn spawn(&self, library: &Path, streams: fn() -> Stdio) -> Result<std::process::Child, Verdict> {
        Command::new(&self.program)
            .arg(WORKER_ARG)
            .arg(library)
            .stdin(Stdio::piped())
            .stdout(streams())
            .stderr(streams())
            .spawn()
            .map_err(|e| Verdict::Crashed { how: format!("the worker {} could not be started: {e}", self.program.display()) })
    }

    fn run_inherited(&self, library: &Path, bytes: &[u8]) -> Verdict {
        let mut child = match self.spawn(library, Stdio::inherit) {
            Ok(c) => c,
            Err(crashed) => return crashed,
        };
        if let Some(mut stdin) = child.stdin.take() {
            // A worker that died before reading everything is a broken pipe
            // here and a status below; the status is the one to report.
            let _ = stdin.write_all(bytes);
        }
        match child.wait() {
            Ok(status) => verdict(status),
            Err(e) => Verdict::Crashed { how: format!("the worker could not be waited for: {e}") },
        }
    }

    /// The run with its streams captured, and the files it wrote under
    /// `files` read back; over `budget` the worker is killed and the run is
    /// [`Captured::over_budget`].
    pub fn run_captured(&self, library: &Path, bytes: &[u8], files: Option<&Path>, budget: Budget) -> Captured {
        let before = files.map(crate::cache::listing).unwrap_or_default();
        let captured = |verdict, stdout, stderr, over_budget| {
            let files = files.map(|root| crate::cache::written(root, &before)).unwrap_or_default();
            Captured { verdict, stdout, stderr, files, over_budget }
        };
        let mut child = match self.spawn(library, Stdio::piped) {
            Ok(c) => c,
            Err(crashed) => return captured(crashed, Vec::new(), Vec::new(), None),
        };
        let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let started = std::time::Instant::now();
        // The worker reads its whole recording before it writes, but a
        // worker that fails early writes first: feed it and drain it from
        // threads so neither side can wait on the other.
        let (status, out, err) = std::thread::scope(|scope| {
            scope.spawn(|| {
                if let Some(mut stdin) = stdin {
                    let _ = stdin.write_all(bytes);
                }
            });
            let drain = |pipe: Option<Box<dyn Read + Send>>| {
                scope.spawn(move || {
                    let mut v = Vec::new();
                    if let Some(mut p) = pipe {
                        let _ = p.read_to_end(&mut v);
                    }
                    v
                })
            };
            let out = drain(stdout.map(|p| Box::new(p) as Box<dyn Read + Send>));
            let err = drain(stderr.map(|p| Box::new(p) as Box<dyn Read + Send>));
            let status = if budget.is_none() {
                child.wait().map(|s| Some(Ok::<_, String>(s)))
            } else {
                let mut pause = std::time::Duration::from_millis(1);
                let mut sampled = started;
                let worker = child.id();
                loop {
                    let stop = match child.try_wait() {
                        Ok(None) => {
                            let elapsed = started.elapsed();
                            // Every 200 ms after the first second — more
                            // often than `xtask::bounded`'s 500 — so this
                            // stops the run before the gate stops both.
                            let mut over_size = || {
                                let limit = budget.mb?;
                                if elapsed.as_secs() < 1 || sampled.elapsed().as_millis() < 200 {
                                    return None;
                                }
                                sampled = std::time::Instant::now();
                                let mb = resident_mb(&[std::process::id(), worker]);
                                (mb > limit).then(|| crate::cache::over_memory(mb, limit))
                            };
                            match budget.secs.filter(|&s| elapsed.as_secs() >= s) {
                                Some(secs) => Some(crate::cache::over_time(secs)),
                                None => over_size(),
                            }
                        }
                        done => break done.map(|s| s.map(Ok)),
                    };
                    if let Some(how) = stop {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Ok(Some(Err(how)));
                    }
                    std::thread::sleep(pause);
                    pause = (pause * 2).min(std::time::Duration::from_millis(50));
                }
            };
            (status, out.join().unwrap_or_default(), err.join().unwrap_or_default())
        });
        match status {
            Ok(Some(Ok(status))) => captured(verdict(status), out, err, None),
            Ok(Some(Err(how))) => captured(Verdict::Crashed { how: how.clone() }, out, err, Some(how)),
            Ok(None) => captured(Verdict::Crashed { how: "the worker ended without a status".to_owned() }, out, err, None),
            Err(e) => captured(Verdict::Crashed { how: format!("the worker could not be waited for: {e}") }, out, err, None),
        }
    }

    /// A run through `cache`: the stored one if there is one (and, when
    /// verifying, a fresh one compared against it), otherwise a fresh one,
    /// kept.
    pub fn run_cached(&self, cache: &Cache, library: &Path, bytes: &[u8]) -> Captured {
        let files = cache.files.as_ref().map(|(root, _)| root.as_path());
        if let Some(stored) = cache.load(library, bytes) {
            // A run stopped over budget is not re-verified: which budget
            // stops it first depends on the machine's load, and checking it
            // costs the budget again. Its key still moves with its
            // recording and library.
            if !cache.verifies() || stored.over_budget.is_some() {
                if cache.restore(&stored).is_ok() {
                    return stored;
                }
            } else {
                let fresh = self.run_captured(library, bytes, files, cache.budget);
                if fresh != stored {
                    cache.mismatch(library, bytes, &stored, &fresh);
                    cache.store(library, bytes, &fresh);
                }
                return fresh;
            }
        }
        let fresh = self.run_captured(library, bytes, files, cache.budget);
        cache.store(library, bytes, &fresh);
        fresh
    }
}

/// The summed resident set of `pids`, in MB, as `ps` reports it — the
/// measure the gates' budget uses (`xtask/src/bounded.rs`).
fn resident_mb(pids: &[u32]) -> u64 {
    let list: Vec<String> = pids.iter().map(u32::to_string).collect();
    let Ok(out) = Command::new("ps").args(["-o", "rss=", "-p", &list.join(",")]).output() else { return 0 };
    let kb: u64 = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<u64>().ok()).sum();
    kb / 1024
}

/// An exit status read as the protocol reads it.
pub fn verdict(status: ExitStatus) -> Verdict {
    match status.code() {
        Some(COMPILED) => Verdict::Compiled,
        Some(REJECTED) => Verdict::Rejected,
        Some(INIT_FAILED) => Verdict::InitFailed,
        Some(LOAD_FAILED) => Verdict::LoadFailed,
        Some(BAD_RECORDING) => Verdict::Crashed { how: "the worker could not read the recording".to_owned() },
        Some(code) => Verdict::Crashed { how: format!("the worker exited with code {code}") },
        None => Verdict::Crashed { how: signalled(status) },
    }
}

#[cfg(unix)]
fn signalled(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    let Some(signal) = status.signal() else { return format!("the worker ended without a code: {status}") };
    // The numbers that differ between the two platforms are the ones not
    // named here, except SIGBUS.
    let name = match signal {
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        10 if cfg!(target_os = "macos") => "SIGBUS",
        7 if cfg!(target_os = "linux") => "SIGBUS",
        _ => "",
    };
    if name.is_empty() { format!("killed by signal {signal}") } else { format!("killed by signal {signal}, {name}") }
}

#[cfg(not(unix))]
fn signalled(status: ExitStatus) -> String {
    format!("the worker ended without a code: {status}")
}

/// Names the call a C++ exception (or a panic) unwound out of, then aborts:
/// the only frame a foreign unwind can run code in on its way out, and the
/// abort is what the runtime's `catch_unwind` would do next anyway, minus
/// the name.
struct InFlight {
    program: String,
    call: Option<(usize, &'static str)>,
    total: usize,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let Some((index, name)) = self.call else { return };
        let what = if std::thread::panicking() {
            "the backend worker panicked in"
        } else {
            "the backend threw a C++ exception out of"
        };
        let _ = writeln!(std::io::stderr(), "{}: {what} {name} (call {} of {})", self.program, index + 1, self.total);
        std::process::abort();
    }
}

/// The worker: `<exe> __kg-backend-worker <library>`, a recording on stdin.
/// Returns the exit code, which [`verdict`] reads.
pub fn worker_main(args: &[String]) -> i32 {
    let program = std::env::args()
        .next()
        .and_then(|a| Path::new(&a).file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "kg".to_owned());
    let Some(library) = args.first() else {
        eprintln!("{program}: {WORKER_ARG} takes the backend library's path");
        return BAD_RECORDING;
    };
    let mut bytes = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
        eprintln!("{program}: the backend worker could not read its recording: {e}");
        return BAD_RECORDING;
    }
    let recording = match Recording::decode(&bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{program}: the backend worker could not read its recording: {e}");
            return BAD_RECORDING;
        }
    };
    let mut real = match Real::load(Path::new(library)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{program}: {e}");
            return LOAD_FAILED;
        }
    };
    let mut guard = InFlight { program: program.clone(), call: None, total: recording.calls().len() };
    let replay = recording.replay(&mut real, |i, call| guard.call = Some((i, call.name())));
    guard.call = None;
    let _ = std::io::stdout().flush();
    match replay {
        Ok(r) if !r.initialised => INIT_FAILED,
        Ok(r) if r.codegen == Some(false) => REJECTED,
        Ok(_) => COMPILED,
        Err(e) => {
            eprintln!("{program}: the backend worker could not replay its recording: {e}");
            BAD_RECORDING
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    /// An executable that is not the worker exits 0 or 1 for reasons of its
    /// own — `cargo test`'s harness, run with the worker argument as a test
    /// filter, runs nothing and exits 0 — and that must read as no verdict,
    /// never as a compile or a rejection. Red with `COMPILED` at 0.
    #[test]
    fn an_executable_that_is_not_the_worker_gives_no_verdict() {
        for (program, code) in [("true", 0), ("false", 1)] {
            let worker = Worker { program: PathBuf::from(program) };
            let verdict = worker.run(Path::new("/no/library"), &Recording::new());
            assert_eq!(verdict, Verdict::Crashed { how: format!("the worker exited with code {code}") });
        }
    }

    /// A fresh directory under the system's temporary one.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kg-backend-cache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A stand-in worker: a shell script that swallows its recording, prints
    /// `said` on stdout and `e` on stderr, writes `said` to `<root>/sub/f`,
    /// and exits `code`.
    fn stand_in(dir: &Path, name: &str, root: &Path, said: &str, code: i32) -> Worker {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        let script = format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '{said}'\nprintf e >&2\nmkdir -p '{root}/sub'\nprintf '{said}' > '{root}/sub/f'\nexit {code}\n",
            root = root.display()
        );
        std::fs::write(&path, script).expect("script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        Worker { program: path }
    }

    fn cache_in(dir: &Path, root: &Path, verify: u32) -> Cache {
        Cache { dir: dir.join("cache"), files: Some((root.to_path_buf(), "out".to_owned())), verify, budget: Budget::default() }
    }

    /// A stored run comes back — verdict, both streams, and its files
    /// written again — with no worker started: the second stand-in would
    /// have rejected. Red with `load` always missing.
    #[test]
    fn a_stored_run_is_replayed_without_the_worker() {
        let dir = scratch("replay");
        let root = dir.join("out");
        let library = dir.join("lib");
        std::fs::write(&library, "one").expect("library");
        let cache = cache_in(&dir, &root, 0);
        let first = stand_in(&dir, "a", &root, "yes", COMPILED).run_cached(&cache, &library, b"recording");
        assert_eq!(first.verdict, Verdict::Compiled);
        assert_eq!((first.stdout.as_slice(), first.stderr.as_slice()), (&b"yes"[..], &b"e"[..]));
        assert_eq!(first.files.get("sub/f").map(Vec::as_slice), Some(&b"yes"[..]));
        std::fs::remove_dir_all(&root).expect("clear");
        let again = stand_in(&dir, "b", &root, "no", REJECTED).run_cached(&cache, &library, b"recording");
        assert_eq!(again, first);
        assert_eq!(std::fs::read(root.join("sub/f")).expect("restored"), b"yes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A different recording, a changed library, or a different captured
    /// directory is a different run. Red with the header left out of the
    /// key.
    #[test]
    fn another_input_is_another_run() {
        let dir = scratch("inputs");
        let root = dir.join("out");
        let library = dir.join("lib");
        std::fs::write(&library, "one").expect("library");
        let cache = cache_in(&dir, &root, 0);
        stand_in(&dir, "a", &root, "yes", COMPILED).run_cached(&cache, &library, b"recording");
        let other = stand_in(&dir, "b", &root, "no", REJECTED);
        assert_eq!(other.run_cached(&cache, &library, b"another").verdict, Verdict::Rejected);
        std::fs::write(&library, "rebuilt").expect("library");
        assert_eq!(other.run_cached(&cache, &library, b"recording").verdict, Verdict::Rejected);
        std::fs::write(&library, "one").expect("library");
        let elsewhere = Cache { files: None, ..cache.clone() };
        assert_eq!(other.run_cached(&elsewhere, &library, b"recording").verdict, Verdict::Rejected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A crash is not a function of the recording: it is never kept, and
    /// the next run starts the worker. Red with crashes stored as verdicts.
    #[test]
    fn a_crash_is_not_kept() {
        let dir = scratch("crash");
        let root = dir.join("out");
        let library = dir.join("lib");
        std::fs::write(&library, "one").expect("library");
        let cache = cache_in(&dir, &root, 0);
        let crashed = stand_in(&dir, "a", &root, "boom", 3).run_cached(&cache, &library, b"recording");
        assert!(matches!(crashed.verdict, Verdict::Crashed { .. }));
        let next = stand_in(&dir, "b", &root, "yes", COMPILED).run_cached(&cache, &library, b"recording");
        assert_eq!((next.verdict, next.stdout.as_slice()), (Verdict::Compiled, &b"yes"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Verifying runs the worker on a hit, answers with the fresh run,
    /// records the difference and replaces the entry. Red with verify
    /// ignored on a hit.
    #[test]
    fn verifying_reruns_and_records_a_difference() {
        let dir = scratch("verify");
        let root = dir.join("out");
        let library = dir.join("lib");
        std::fs::write(&library, "one").expect("library");
        let cache = cache_in(&dir, &root, 0);
        stand_in(&dir, "a", &root, "yes", COMPILED).run_cached(&cache, &library, b"recording");
        let verifying = cache_in(&dir, &root, 1);
        let fresh = stand_in(&dir, "b", &root, "no", REJECTED).run_cached(&verifying, &library, b"recording");
        assert_eq!(fresh.verdict, Verdict::Rejected);
        let mismatches: Vec<_> = std::fs::read_dir(cache.dir.join(crate::cache::MISMATCHES)).expect("recorded").collect();
        assert_eq!(mismatches.len(), 1);
        assert_eq!(cache.load(&library, b"recording").map(|r| r.verdict), Some(Verdict::Rejected));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worker past the budget is killed, and its crash is kept: the next
    /// run of the same recording answers at once, a crash naming the budget,
    /// though the worker now would compile. A different budget is a
    /// different run. Red with over-budget runs not stored.
    #[test]
    fn a_run_over_budget_is_kept_as_a_crash() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("budget");
        let root = dir.join("out");
        let library = dir.join("lib");
        std::fs::write(&library, "one").expect("library");
        let slow = dir.join("slow");
        std::fs::write(&slow, "#!/bin/sh\ncat >/dev/null\nexec sleep 30\n").expect("script");
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let cache = Cache { budget: Budget { secs: Some(1), mb: None }, ..cache_in(&dir, &root, 0) };
        let started = std::time::Instant::now();
        let first = Worker { program: slow }.run_cached(&cache, &library, b"recording");
        assert!(started.elapsed().as_secs() < 10, "killed at its budget");
        let over = Verdict::Crashed { how: crate::cache::over_time(1) };
        assert_eq!((first.verdict.clone(), first.over_budget), (over.clone(), Some(crate::cache::over_time(1))));
        let fast = stand_in(&dir, "b", &root, "yes", COMPILED);
        assert_eq!(fast.run_cached(&cache, &library, b"recording").verdict, over);
        let longer = Cache { budget: Budget { secs: Some(2), mb: None }, ..cache.clone() };
        assert_eq!(fast.run_cached(&longer, &library, b"recording").verdict, Verdict::Compiled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stored over-budget run is answered even when verifying: it is not a
    /// function of its recording alone, and re-running it costs the budget.
    /// Red with over-budget entries sampled like the rest.
    #[test]
    fn an_over_budget_run_is_not_verified() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("unverified");
        let root = dir.join("out");
        let library = dir.join("lib");
        std::fs::write(&library, "one").expect("library");
        let slow = dir.join("slow");
        std::fs::write(&slow, "#!/bin/sh\ncat >/dev/null\nexec sleep 30\n").expect("script");
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let cache = Cache { budget: Budget { secs: Some(1), mb: None }, ..cache_in(&dir, &root, 0) };
        Worker { program: slow }.run_cached(&cache, &library, b"recording");
        let verifying = Cache { verify: 1, ..cache.clone() };
        let fast = stand_in(&dir, "b", &root, "yes", COMPILED).run_cached(&verifying, &library, b"recording");
        assert_eq!(fast.verdict, Verdict::Crashed { how: crate::cache::over_time(1) });
        assert!(std::fs::read_dir(cache.dir.join(crate::cache::MISMATCHES)).is_err(), "nothing recorded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worker that, with this process, grows past the budget in megabytes
    /// is killed and its crash kept, naming the size. Red with the size
    /// never sampled.
    #[test]
    fn a_run_over_its_size_is_kept_as_a_crash() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("size");
        let root = dir.join("out");
        let library = dir.join("lib");
        std::fs::write(&library, "one").expect("library");
        let big = dir.join("big");
        let script = "#!/bin/sh\ncat >/dev/null\nexec python3 -c 'import time; x = bytearray(600 << 20); time.sleep(20)'\n";
        std::fs::write(&big, script).expect("script");
        std::fs::set_permissions(&big, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let limit = crate::isolate::resident_mb(&[std::process::id()]) + 300;
        let cache = Cache { budget: Budget { secs: Some(15), mb: Some(limit) }, ..cache_in(&dir, &root, 0) };
        let run = Worker { program: big }.run_cached(&cache, &library, b"recording");
        let Verdict::Crashed { how } = &run.verdict else { panic!("{:?}", run.verdict) };
        assert!(how.contains(&format!("over their budget of {limit} MB")), "{how}");
        assert_eq!(cache.load(&library, b"recording").map(|r| r.verdict), Some(run.verdict.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worker killed by a signal is a crash that names the signal.
    #[test]
    fn a_signal_is_a_crash_by_name() {
        assert_eq!(verdict(ExitStatus::from_raw(6)), Verdict::Crashed { how: "killed by signal 6, SIGABRT".to_owned() });
        assert_eq!(verdict(ExitStatus::from_raw(COMPILED << 8)), Verdict::Compiled);
        assert_eq!(verdict(ExitStatus::from_raw(REJECTED << 8)), Verdict::Rejected);
    }
}
