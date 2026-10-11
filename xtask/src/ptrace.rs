//! `cargo xtask ptrace` — record what the **reference** front end tells the
//! backend, for every program in the corpus.
//!
//! The recorder is `kg-ptrace`, a shim `libkanagawa-backend` that logs every
//! ParseTree call and forwards it (`docs/spec/abi.md` §14). This driver lays
//! out `target/ptrace/bin/` — the reference executable, the real library
//! renamed `-real`, the shim under the real one's name — so the executable
//! loads the shim by rpath, and runs it once per program:
//!
//! * every registered syntax assertion, as `syntax-test.py` would: the
//!   snippet written to `syntax_test_temp_file.k` in a subfolder — one per
//!   assertion, where the runner has one per test, because eight workers
//!   share the file name — the effective flag list of that test
//!   (`conformance::Assertion::options`), the subfolder as working
//!   directory;
//! * every whole file under `library/` and `test/` that is not a snippet
//!   file, with `--import-dir <root>/library` and the file's own directory
//!   as working directory.
//!
//! What comes back per program is the trace — empty when the front end
//! rejected the program before `InitCompiler`, which is the reference's own
//! verdict — and the process's exit code and output, which for a program
//! that reached `Codegen` carry the backend's error code. Both are kept in
//! `target/ptrace/traces/<name>/`, gzipped, keyed by the program's text and
//! flags, under a stamp of the three binaries (`STAMP`) and one of every
//! `.k` file the program could import (`INPUTS`): a rebuilt reference or
//! shim, or an edited reference tree, clears the store, and a reader
//! ([`load`]) checks both keys before it serves a run. Nothing is committed; the store is a cache the way
//! `target/oracle-cache/` is, and `xtask ptdiff` reads it.
//!
//! A gate is a driver over threads (`CLAUDE.md`): programs are handed to
//! workers that each spawn one process at a time, and results land in slots
//! indexed by program.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};

use crate::conformance::{Assertion, Expectation};
use crate::oracle_cache::Fnv;

/// What one run of the reference left behind.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Run {
    pub name: String,
    /// The reference's exit code, `None` if killed by a signal.
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Lines in the trace: 0 means the front end never called `InitCompiler`.
    pub lines: usize,
    /// `Some(true)` if `Codegen` was reached and returned 1.
    pub codegen: Option<bool>,
}

/// One program to record.
#[derive(Debug, Clone)]
pub(crate) struct Program {
    /// `syntax::<test>::<n>` or `file::<relative path>`.
    pub name: String,
    /// Directory the reference runs in and writes to — always under
    /// `target/ptrace/work/`.
    pub cwd: PathBuf,
    /// The argument the reference is given for the file.
    pub file_arg: String,
    /// Written into `cwd/file_arg` before the run, or `None` for a file that
    /// already exists there.
    pub source: Option<String>,
    pub options: Vec<String>,
    /// What the corpus expects, for the report.
    pub expectation: Option<Expectation>,
}

/// `target/ptrace/` under the workspace.
pub(crate) fn ptrace_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/ptrace")
}

/// The store: one directory per program.
pub(crate) fn traces_dir() -> PathBuf {
    ptrace_dir().join("traces")
}

/// A program's directory in the store: `syntax::basics::17` → `syntax/basics/17`.
pub(crate) fn trace_path(name: &str) -> PathBuf {
    traces_dir().join(name.replace("::", "/"))
}

/// What the store holds for one program, as far as a gate may use it.
pub(crate) enum Loaded {
    /// Recorded from exactly this program and the current reference inputs.
    Fresh(Run, String),
    /// Recorded, but from a different program text, flag list or reference
    /// tree: the reference's answer to a question no longer being asked.
    Stale,
    Unrecorded,
}

/// The recorded run and its trace text, if it was recorded from this program
/// as it stands and from the reference tree as it stands (AUDIT D7).
///
/// Until 2026-09-22 this read the store by name alone, and the key the
/// recorder wrote beside each run was never read back here: a snippet edited
/// in the corpus, or a library file every program imports, and `ptdiff` and
/// `backend` graded against the reference's verdict on the old text. Two
/// checks now: the program's own key ([`program_key`]: its text and flags),
/// and the store's `INPUTS` stamp — every `.k` file the program can import,
/// hashed as `oracle_cache` hashes them.
pub(crate) fn load(kanagawa_root: &Path, p: &Program) -> Result<Loaded> {
    load_from(&traces_dir(), kanagawa_root, p)
}

/// [`load`] without the trace: the recorded run alone, checked the same way
/// (an empty text in `Loaded::Fresh`).
pub(crate) fn load_run(kanagawa_root: &Path, p: &Program) -> Result<Loaded> {
    let dir = traces_dir().join(p.name.replace("::", "/"));
    Ok(match checked_run(&traces_dir(), &dir, kanagawa_root, p)? {
        Ok(run) => Loaded::Fresh(run, String::new()),
        Err(other) => other,
    })
}

fn checked_run(store: &Path, dir: &Path, kanagawa_root: &Path, p: &Program) -> Result<std::result::Result<Run, Loaded>> {
    let run_path = dir.join("run.json");
    if !run_path.is_file() {
        return Ok(Err(Loaded::Unrecorded));
    }
    if !store_is_current(store, kanagawa_root)
        || std::fs::read_to_string(dir.join("key")).ok().as_deref() != Some(program_key(p).as_str())
    {
        return Ok(Err(Loaded::Stale));
    }
    Ok(Ok(serde_json::from_str(&std::fs::read_to_string(&run_path)?)?))
}

fn load_from(store: &Path, kanagawa_root: &Path, p: &Program) -> Result<Loaded> {
    let dir = store.join(p.name.replace("::", "/"));
    let run = match checked_run(store, &dir, kanagawa_root, p)? {
        Ok(run) => run,
        Err(other) => return Ok(other),
    };
    let gz = dir.join("trace.gz");
    let text = if gz.is_file() {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(std::fs::File::open(&gz)?), &mut out)?;
        out
    } else {
        String::new()
    };
    Ok(Loaded::Fresh(run, text))
}

/// The reference's importable inputs, hashed: a program's text is in its own
/// key, but what it imports is not, and one `library/` edit changes the
/// verdict on every program that reaches it. The whole tree rather than each
/// program's import closure, as in `oracle_cache` — the tree is read-only,
/// so the coarse key costs nothing in practice and cannot miss an edge.
fn inputs_key(kanagawa_root: &Path) -> String {
    let mut h = Fnv::new();
    h.write_str("ptrace inputs v1");
    crate::oracle_cache::write_k_tree(&mut h, kanagawa_root);
    h.finish()
}

fn inputs_path(store: &Path) -> PathBuf {
    store.join("INPUTS")
}

/// Whether the store as a whole was recorded against the reference tree as
/// it stands. Computed once per process; a reader never writes the stamp.
fn store_is_current(store: &Path, kanagawa_root: &Path) -> bool {
    type Memo = Vec<((PathBuf, PathBuf), bool)>;
    static CURRENT: Mutex<Memo> = Mutex::new(Vec::new());
    let mut memo = CURRENT.lock().expect("no panics hold this lock");
    let key = (store.to_path_buf(), kanagawa_root.to_path_buf());
    if let Some((_, current)) = memo.iter().find(|(k, _)| *k == key) {
        return *current;
    }
    let current = match std::fs::read_to_string(inputs_path(store)) {
        Ok(stamp) => stamp == inputs_key(kanagawa_root),
        Err(_) => {
            let proven = older_than_store(store, kanagawa_root);
            if proven {
                println!(
                    "ptrace store: no INPUTS stamp (recorded before 2026-09-22); every reference input predates its STAMP, \
                     so it is graded — `cargo xtask ptrace` stamps it"
                );
            }
            proven
        }
    };
    if !current {
        println!("ptrace store: STALE — recorded against a different reference tree; re-run `cargo xtask ptrace`");
    }
    memo.push((key, current));
    current
}

/// The one way to adopt a store recorded before `INPUTS` existed without
/// re-recording it: `STAMP` is written when the store is created, before any
/// run, so if no input has been modified since then, every run in it saw the
/// inputs as they are now. An mtime can only err toward "newer" (a checkout
/// touches files), which here means re-recording, never a stale answer.
fn older_than_store(store: &Path, kanagawa_root: &Path) -> bool {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(created) = mtime(&store.join("STAMP")) else { return false };
    crate::oracle_cache::k_tree(kanagawa_root).iter().all(|f| mtime(f).is_some_and(|t| t < created))
}

/// Build the shim and lay out `target/ptrace/bin/`. Returns the executable.
fn install(build_dir: &Path) -> Result<PathBuf> {
    let dist = build_dir.join("dist/bin");
    let exe = dist.join("kanagawa");
    let lib_name = kg_backend_sys_library_file();
    let real = dist.join(lib_name);
    if !exe.is_file() || !real.is_file() {
        anyhow::bail!(
            "no reference build at {} — expected `kanagawa` and `{lib_name}`; pass --build-dir",
            dist.display()
        );
    }
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "-p", "kg-ptrace"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
        .status()
        .context("building kg-ptrace")?;
    if !status.success() {
        anyhow::bail!("cargo build -p kg-ptrace failed");
    }
    let shim = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/release").join(shim_file());
    let bin = ptrace_dir().join("bin");
    std::fs::create_dir_all(&bin)?;
    let (stem, ext) = lib_name.rsplit_once('.').expect("library file has an extension");
    copy_if_changed(&exe, &bin.join("kanagawa"))?;
    copy_if_changed(&real, &bin.join(format!("{stem}-real.{ext}")))?;
    copy_if_changed(&shim, &bin.join(lib_name))?;
    Ok(bin.join("kanagawa"))
}

fn kg_backend_sys_library_file() -> &'static str {
    if cfg!(target_os = "macos") {
        "libkanagawa-backend.dylib"
    } else if cfg!(target_os = "windows") {
        "kanagawa-backend.dll"
    } else {
        "libkanagawa-backend.so"
    }
}

fn shim_file() -> &'static str {
    if cfg!(target_os = "macos") {
        "libkg_ptrace.dylib"
    } else if cfg!(target_os = "windows") {
        "kg_ptrace.dll"
    } else {
        "libkg_ptrace.so"
    }
}

fn copy_if_changed(from: &Path, to: &Path) -> Result<()> {
    let same = match (std::fs::metadata(from), std::fs::metadata(to)) {
        (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
        _ => false,
    };
    if !same {
        std::fs::copy(from, to).with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
        if let Ok(t) = std::fs::metadata(from).and_then(|m| m.modified()) {
            let _ = std::fs::File::options().write(true).open(to).and_then(|f| f.set_modified(t));
        }
    }
    Ok(())
}

/// The three binaries, hashed: a different reference or shim is a different
/// store.
fn stamp(bin: &Path) -> Result<String> {
    let mut h = Fnv::new();
    for entry in std::fs::read_dir(bin)? {
        let path = entry?.path();
        h.write_str(&path.file_name().unwrap_or_default().to_string_lossy());
        h.write(&std::fs::read(&path)?);
    }
    Ok(h.finish())
}

/// Every program in the corpus, syntax assertions first, in corpus order.
pub(crate) fn programs(kanagawa_root: &Path) -> Result<Vec<Program>> {
    let mut out = Vec::new();
    let work = ptrace_dir().join("work");
    for a in crate::conformance::load_all(kanagawa_root)? {
        let Assertion { name, file, source, expectation, options, .. } = a;
        // One directory per *assertion*, not per test as the runner has it:
        // eight workers write `syntax_test_temp_file.k` at once, and a
        // shared directory is a race the runner never had.
        let test = file.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let index = name.rsplit("::").next().unwrap_or("0").to_owned();
        out.push(Program {
            name,
            cwd: work.join(&test).join(index),
            file_arg: "syntax_test_temp_file.k".to_owned(),
            source: Some(source),
            options: options.to_vec(),
            expectation: Some(expectation),
        });
    }
    let mut files = Vec::new();
    for dir in ["library", "test"] {
        crate::corpus::collect(&kanagawa_root.join(dir), &mut files);
    }
    files.sort();
    let library = kanagawa_root.join("library").display().to_string();
    for path in files {
        if crate::corpus::is_snippet_file(&path) {
            continue;
        }
        let rel = path.strip_prefix(kanagawa_root).unwrap_or(&path).to_path_buf();
        out.push(Program {
            name: format!("file::{}", rel.display()),
            // A directory of our own, never the file's: a compile that
            // reaches `Codegen` writes its reports into the working
            // directory (`<base>PathLength.pdrpt`, `kanagawa.cpp:145-158`),
            // and `../kanagawa/` is read-only. Until 2026-09-19 the file's
            // own directory was used, and 163 reports landed in the
            // reference's `test/`. The file goes by its absolute path.
            cwd: work.join("file").join(rel.parent().unwrap_or(Path::new(""))),
            file_arg: path.display().to_string(),
            source: None,
            options: vec!["--place-iterations=1".to_owned(), "--import-dir".to_owned(), library.clone()],
            expectation: None,
        });
    }
    Ok(out)
}

/// The program's own inputs: name, flags, and the text of the file it is
/// given. What that file imports is the store's `INPUTS` stamp, not this —
/// one stamp for the tree rather than the tree hashed into every key, so the
/// keys recorded before the stamp existed stay valid ([`older_than_store`]).
pub(crate) fn program_key(p: &Program) -> String {
    let mut h = Fnv::new();
    h.write_str(&p.name);
    h.write_str(&p.file_arg);
    for o in &p.options {
        h.write_str(o);
    }
    match &p.source {
        Some(s) => h.write_str(s),
        None => h.write(&std::fs::read(p.cwd.join(&p.file_arg)).unwrap_or_default()),
    }
    h.finish()
}

/// A run and whether the store already had it.
type Recorded = Result<(Run, bool)>;

/// Record one program, or replay its stored run. `Ok(true)` on a hit.
fn record(exe: &Path, p: &Program, force: bool) -> Recorded {
    let dir = trace_path(&p.name);
    let key = program_key(p);
    let key_path = dir.join("key");
    // `run` has already cleared the store if its `INPUTS` stamp differs, so
    // the program's own key is the whole of what is left to check.
    if !force
        && std::fs::read_to_string(&key_path).ok().as_deref() == Some(key.as_str())
        && dir.join("run.json").is_file()
    {
        let run: Run = serde_json::from_str(&std::fs::read_to_string(dir.join("run.json"))?)?;
        return Ok((run, true));
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::create_dir_all(&p.cwd)?;
    if let Some(source) = &p.source {
        std::fs::write(p.cwd.join(&p.file_arg), source)?;
    }
    let trace_tmp = dir.join("trace.tmp");
    let _ = std::fs::remove_file(&trace_tmp);
    let mut command = Command::new(exe);
    command
        .args(&p.options)
        .arg(&p.file_arg)
        .current_dir(&p.cwd)
        .env("KG_PTRACE_OUT", &trace_tmp)
        .env_remove("PD_WAIT_FOR_DEBUG");
    // The reference compiles `array.k` in 23 s; a recording it cannot
    // finish inside our budget is not one to grade against (`crate::bounded`).
    let output = match crate::bounded::run(command, crate::bounded::Limits::compile())
        .with_context(|| format!("spawning {} for {}", exe.display(), p.name))?
    {
        Ok(output) => output,
        Err((over, _)) => anyhow::bail!("recording {}: the reference compiler was {over}", p.name),
    };
    let text = std::fs::read_to_string(&trace_tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&trace_tmp);
    let lines = text.lines().count();
    let codegen = text
        .lines()
        .rev()
        .find(|l| l.contains(" Codegen "))
        .map(|l| l.ends_with("-> 1"));
    let run = Run {
        name: p.name.clone(),
        exit: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        lines,
        codegen,
    };
    let gz = dir.join("trace.gz");
    if lines == 0 {
        let _ = std::fs::remove_file(&gz);
    } else {
        let file = std::fs::File::create(&gz)?;
        let mut enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        std::io::Write::write_all(&mut enc, text.as_bytes())?;
        enc.finish()?;
    }
    std::fs::write(dir.join("run.json"), serde_json::to_string_pretty(&run)?)?;
    std::fs::write(&key_path, &key)?;
    Ok((run, false))
}

pub fn run(kanagawa_root: &Path, build_dir: &Path, filter: Option<&str>, force: bool, verbose: bool) -> Result<()> {
    let kanagawa_root = &kanagawa_root
        .canonicalize()
        .with_context(|| format!("resolving {}", kanagawa_root.display()))?;
    let exe = install(build_dir)?;
    let bin = exe.parent().expect("the executable has a directory");
    let stamp = stamp(bin)?;
    let stamp_path = traces_dir().join("STAMP");
    let inputs = inputs_key(kanagawa_root);
    let recorded_inputs = std::fs::read_to_string(inputs_path(&traces_dir())).ok();
    let binaries_changed = std::fs::read_to_string(&stamp_path).ok().as_deref() != Some(stamp.as_str());
    let inputs_changed = match &recorded_inputs {
        Some(s) => *s != inputs,
        None => !older_than_store(&traces_dir(), kanagawa_root),
    };
    if binaries_changed || inputs_changed {
        if traces_dir().is_dir() {
            let why = if binaries_changed { "binaries changed" } else { "reference inputs changed" };
            println!("{why}: clearing {}", traces_dir().display());
            std::fs::remove_dir_all(traces_dir())?;
        }
        std::fs::create_dir_all(traces_dir())?;
        std::fs::write(&stamp_path, &stamp)?;
        std::fs::write(inputs_path(&traces_dir()), &inputs)?;
    } else if recorded_inputs.is_none() {
        println!("stamping {} with the reference inputs it was recorded from (every one predates its STAMP)", traces_dir().display());
        std::fs::write(inputs_path(&traces_dir()), &inputs)?;
    }

    let programs: Vec<Program> = programs(kanagawa_root)?
        .into_iter()
        .filter(|p| filter.is_none_or(|f| p.name.contains(f)))
        .collect();
    if programs.is_empty() {
        anyhow::bail!("no program matches the filter");
    }
    let started = std::time::Instant::now();

    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Recorded>>> = programs.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= programs.len() {
                        break;
                    }
                    *slots[i].lock().expect("no panics hold this lock") = Some(record(&exe, &programs[i], force));
                }
            });
        }
    });

    let mut hits = 0usize;
    let mut walked = 0usize;
    let mut rejected = 0usize;
    let mut codegen_ok = 0usize;
    let mut codegen_failed = Vec::new();
    let mut no_codegen = Vec::new();
    let mut backend_expected = 0usize;
    let mut backend_confirmed = 0usize;
    let mut backend_missed = Vec::new();
    let mut errors = Vec::new();
    for (p, slot) in programs.iter().zip(slots) {
        match slot.into_inner().expect("threads joined").expect("every slot filled") {
            Err(e) => errors.push(format!("{}: {e:#}", p.name)),
            Ok((run, hit)) => {
                hits += usize::from(hit);
                if run.lines == 0 {
                    rejected += 1;
                    continue;
                }
                walked += 1;
                // An assertion expecting a backend code is confirmed when the
                // reference's own run printed that code: the store is then
                // the ground truth `conformance` books as undecidable.
                if let Some(Expectation::Code(code)) = p.expectation
                    && code != 0
                {
                    backend_expected += 1;
                    if run.stdout.contains(&format!("Error {code}")) {
                        backend_confirmed += 1;
                    } else {
                        backend_missed.push((format!("{} (expected:{code})", p.name), first_error_line(&run.stdout)));
                    }
                }
                match run.codegen {
                    Some(true) => codegen_ok += 1,
                    Some(false) => codegen_failed.push((p.name.clone(), first_error_line(&run.stdout))),
                    None => no_codegen.push(p.name.clone()),
                }
            }
        }
    }
    println!(
        "ptrace: {} programs, {} replayed, {} recorded, in {:.1}s",
        programs.len(),
        hits,
        programs.len() - hits,
        started.elapsed().as_secs_f64()
    );
    println!("  rejected before InitCompiler: {rejected}");
    println!("  walked: {walked} — Codegen ok {codegen_ok}, Codegen failed {}, no Codegen {}", codegen_failed.len(), no_codegen.len());
    println!("  backend codes the corpus expects: {backend_expected}, confirmed by the reference: {backend_confirmed}");
    if verbose {
        for (name, msg) in &backend_missed {
            println!("  BACKEND CODE MISSED {name}: {msg}");
        }
        for (name, msg) in &codegen_failed {
            println!("  CODEGEN FAILED {name}: {msg}");
        }
        for name in &no_codegen {
            println!("  NO CODEGEN {name}");
        }
    } else if !codegen_failed.is_empty() {
        println!("  (--verbose lists the Codegen failures)");
    }
    for e in &errors {
        println!("  ERROR {e}");
    }
    if !errors.is_empty() {
        anyhow::bail!("{} programs could not be recorded", errors.len());
    }
    Ok(())
}

fn first_error_line(stdout: &str) -> String {
    stdout.lines().find(|l| l.contains("Error")).unwrap_or("").chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference tree of one library file and a store holding one recorded
    /// program, in a scratch directory of their own.
    fn fixture(tag: &str) -> (PathBuf, PathBuf, Program) {
        let base = std::env::temp_dir().join(format!("kg-ptrace-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, store) = (base.join("kanagawa"), base.join("traces"));
        std::fs::create_dir_all(root.join("library")).unwrap();
        std::fs::write(root.join("library/lib.k"), "inline uint32 f() { return 1; }\n").unwrap();
        let p = Program {
            name: "syntax::t::0".to_owned(),
            cwd: base.join("work"),
            file_arg: "syntax_test_temp_file.k".to_owned(),
            source: Some("import lib\n".to_owned()),
            options: vec!["--import-dir".to_owned(), root.join("library").display().to_string()],
            expectation: None,
        };
        let dir = store.join("syntax/t/0");
        std::fs::create_dir_all(&dir).unwrap();
        let run = Run { name: p.name.clone(), exit: Some(0), stdout: String::new(), stderr: String::new(), lines: 0, codegen: None };
        std::fs::write(dir.join("run.json"), serde_json::to_string(&run).unwrap()).unwrap();
        std::fs::write(dir.join("key"), program_key(&p)).unwrap();
        std::fs::write(store.join("INPUTS"), inputs_key(&root)).unwrap();
        (root, store, p)
    }

    fn fresh(store: &Path, root: &Path, p: &Program) -> bool {
        matches!(load_from(store, root, p).unwrap(), Loaded::Fresh(..))
    }

    #[test]
    fn a_recording_is_served_only_for_the_program_it_was_recorded_from() {
        let (root, store, mut p) = fixture("key");
        assert!(fresh(&store, &root, &p));
        p.source = Some("import lib\nfoo\n".to_owned());
        assert!(!fresh(&store, &root, &p), "an edited snippet must not replay the old run");
    }

    #[test]
    fn a_recording_is_stale_once_an_importable_file_changes() {
        let (root, store, p) = fixture("inputs");
        assert!(fresh(&store, &root, &p));
        // A second root whose library differs: the same program text, the
        // same store, and a different answer key underneath.
        let edited = root.with_file_name("kanagawa-edited");
        std::fs::create_dir_all(edited.join("library")).unwrap();
        std::fs::write(edited.join("library/lib.k"), "inline uint32 f() { return 2; }\n").unwrap();
        assert!(!fresh(&store, &edited, &p), "a library edit must not replay the old run");
    }
}
