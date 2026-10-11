//! `cargo xtask output` — what `kanagawa --backend=sv` writes, ours against
//! the reference's, file by file.
//!
//! Every other gate grades a verdict: accepted or not, `Error N`, the call
//! sequence handed to the backend. This one grades the product. A program
//! the reference compiles through `Codegen` (its `ptrace` run exited 0) is
//! compiled again by both compilers with `--backend=sv -o out/o`, and every
//! file each writes under `out/` is compared.
//!
//! **Parity is identical output modulo source locations** (the owner's
//! decision, 2026-09-23; `docs/history/audit-2026-09-22.md` §6.1). Locations are where this port's
//! spans and the reference's differ (divergence #25), and they reach two
//! files: the `// file.k:14:9` comment CIRCT appends to an `.sv` line, and the
//! path and range columns of the `PathLength.pdrpt` report. Two sets, both
//! ratcheted, so neither may shrink:
//!
//! - `equal`: every file identical once those locations are blanked;
//! - `exact`: every file byte-identical — the location ratchet.
//!
//! The two compilers run in the **same working directory with the same
//! arguments**, one after the other: the report holds the source file's
//! absolute path, and the `.sv` header holds the command line
//! (`legacy::cmd_args_string`), so anything else differs for a reason no
//! compiler controls. The reference's side is stored under
//! `target/output/theirs/`, keyed by both reference binaries, every `.k`
//! file the program can import, the program's own text and flags, and the
//! directory it ran in; a program the reference wrote nothing for is stored
//! as such, and only a program with output costs our compiler a run.
//!
//! What this gate cannot see: a program for which the reference writes no
//! file and ours writes some (ours never runs there), and anything under
//! `out/` that is not a file. A location comment in a shape the normaliser
//! does not know reads as a difference, never as agreement.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};

use crate::oracle_cache::Fnv;
use crate::ptrace::{Loaded, Program};

/// The output base name both compilers are given.
const BASE: &str = "out/o";

/// What one program came to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    /// The reference wrote nothing: nothing to compare.
    NoOutput,
    /// The reference exited non-zero once asked for RTL, though its recorded
    /// run reached `Codegen`.
    TheirsFailed(String),
    Exact,
    /// Identical modulo locations.
    Equal,
    Differ(String),
    Declined(String),
    Failed(String),
}

/// A verdict and the location comments it compared: `(agreeing, total)`.
type Grade = (Verdict, (usize, usize));

pub fn run(kanagawa_root: &Path, build_dir: &Path, filter: Option<&str>, verbose: bool) -> Result<()> {
    let kanagawa_root = &kanagawa_root
        .canonicalize()
        .with_context(|| format!("resolving {}", kanagawa_root.display()))?;
    let dist = build_dir.join("dist/bin").canonicalize().with_context(|| {
        format!("no reference build under {} — the reference compiler is needed; pass --build-dir", build_dir.display())
    })?;
    let theirs_exe = dist.join("kanagawa");
    let ours_exe = crate::fresh::compat_binary()?;
    let environment = environment(&dist, kanagawa_root)?;

    let mut work: Vec<Program> = Vec::new();
    let mut stale = 0usize;
    for p in crate::ptrace::programs(kanagawa_root)? {
        if !filter.is_none_or(|f| p.name.contains(f)) {
            continue;
        }
        match crate::ptrace::load(kanagawa_root, &p)? {
            Loaded::Fresh(run, _) if run.exit == Some(0) && run.codegen == Some(true) => work.push(p),
            Loaded::Fresh(..) | Loaded::Unrecorded => {}
            Loaded::Stale => stale += 1,
        }
    }
    if stale > 0 {
        anyhow::bail!("{stale} ptrace recordings are stale — `cargo xtask ptrace` re-records them");
    }

    let started = std::time::Instant::now();
    let watch = crate::backend_cache::Watch::start();
    // Half the other gates' eight: the reference's own process peaks at
    // 2.2 GB on the smallest exported class (`/usr/bin/time -l`,
    // 2026-09-23), and this runs beside nothing else in `gates.sh`.
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(4);
    let next = AtomicUsize::new(0);
    let spawned = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Result<Grade>>>> = work.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(p) = work.get(i) else { break };
                    let grade = grade(p, &theirs_exe, &ours_exe, &dist, &environment, &spawned);
                    *slots[i].lock().expect("no panics hold this lock") = Some(grade);
                }
            });
        }
    });

    let mut ledger = crate::ratchet::Ledger::new("output");
    ledger.set("equal");
    ledger.set("exact");
    let mut counts = BTreeMap::<&str, usize>::new();
    let mut located = (0usize, 0usize);
    let mut lines = Vec::new();
    let mut reasons = BTreeMap::<String, (usize, String)>::new();
    for (p, slot) in work.iter().zip(slots) {
        let (verdict, (agree, total)) = slot.into_inner().expect("threads joined").expect("every slot filled")?;
        let class = match &verdict {
            Verdict::NoOutput => "no-output",
            Verdict::TheirsFailed(_) => "theirs-failed",
            Verdict::Exact => "exact",
            Verdict::Equal => "equal",
            Verdict::Differ(_) => "differ",
            Verdict::Declined(_) => "declined",
            Verdict::Failed(_) => "failed",
        };
        *counts.entry(class).or_default() += 1;
        located = (located.0 + agree, located.1 + total);
        // Per program, so a moved total names where it moved (the ratchet
        // prints a changed entry; it grades none).
        if total > 0 {
            ledger.histogram_count("locations-agree", &p.name, agree);
        }
        match &verdict {
            Verdict::Exact => {
                ledger.add("exact", &p.name);
                ledger.add("equal", &p.name);
            }
            Verdict::Equal => ledger.add("equal", &p.name),
            Verdict::Differ(why) | Verdict::Declined(why) | Verdict::Failed(why) | Verdict::TheirsFailed(why) => {
                let key = format!("{class}: {}", why.chars().take(70).collect::<String>());
                let e = reasons.entry(key).or_insert((0, p.name.clone()));
                e.0 += 1;
                if verbose || !matches!(verdict, Verdict::Declined(_)) {
                    lines.push(format!("  {} {}: {why}", class.to_uppercase(), p.name));
                }
            }
            Verdict::NoOutput => {}
        }
    }
    let graded = work.len() - counts.get("no-output").copied().unwrap_or(0);
    println!(
        "output: {graded} programs with RTL of {} the reference compiles, in {:.1}s ({} compiler runs) — {}",
        work.len(),
        started.elapsed().as_secs_f64(),
        spawned.load(Ordering::Relaxed),
        counts.iter().filter(|(k, _)| **k != "no-output").map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
    );
    let equal = counts.get("exact").copied().unwrap_or(0) + counts.get("equal").copied().unwrap_or(0);
    println!("  identical modulo locations: {equal}/{graded}; byte-identical: {}", counts.get("exact").copied().unwrap_or(0));
    println!("  location comments and report rows agreeing, where the rest is equal: {}/{}", located.0, located.1);
    if !reasons.is_empty() {
        println!("  not equal, by reason:");
        let mut v: Vec<_> = reasons.iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for (r, (n, example)) in v.iter().take(15) {
            println!("    {n:5}  {r}   e.g. {example}");
        }
    }
    for l in &lines {
        println!("{l}");
    }
    if filter.is_none() {
        ledger.total("graded", graded);
        ledger.total("locations", located.1);
        ledger.total("locations-agree", located.0);
        for (class, n) in &counts {
            ledger.total(class, *n);
        }
        crate::ratchet::enforce(ledger)?;
    }
    watch.check()
}

/// Both reference binaries and every `.k` file a program can import.
fn environment(dist: &Path, kanagawa_root: &Path) -> Result<String> {
    let mut h = Fnv::new();
    h.write_str("output v1");
    h.write_str(BASE);
    for entry in std::fs::read_dir(dist)? {
        let path = entry?.path();
        if path.is_file() {
            h.write_str(&path.file_name().unwrap_or_default().to_string_lossy());
            h.write(&std::fs::read(&path)?);
        }
    }
    crate::oracle_cache::write_k_tree(&mut h, kanagawa_root);
    Ok(h.finish())
}

fn output_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/output")
}

type Files = BTreeMap<String, Vec<u8>>;

fn grade(
    p: &Program,
    theirs_exe: &Path,
    ours_exe: &Path,
    dist: &Path,
    environment: &str,
    spawned: &AtomicUsize,
) -> Result<Grade> {
    let rel = p.name.replace("::", "/");
    let cwd = output_dir().join("work").join(&rel);
    let store = output_dir().join("theirs").join(&rel);
    let key = {
        let mut h = Fnv::new();
        h.write_str(environment);
        h.write_str(&crate::ptrace::program_key(p));
        h.write_str(&cwd.to_string_lossy());
        h.finish()
    };
    let theirs = match load(&store, &key) {
        Some(stored) => stored,
        None => {
            spawned.fetch_add(1, Ordering::Relaxed);
            let (status, files) = compile(theirs_exe, p, &cwd, None)?;
            save(&store, &key, &status, &files)?;
            (status, files)
        }
    };
    let theirs = match theirs {
        (status, _) if status != "0" => return Ok((Verdict::TheirsFailed(format!("exit {status}")), (0, 0))),
        (_, files) => written(files),
    };
    if theirs.is_empty() {
        return Ok((Verdict::NoOutput, (0, 0)));
    }
    spawned.fetch_add(1, Ordering::Relaxed);
    let (status, ours) = compile(ours_exe, p, &cwd, Some(dist))?;
    if status != "0" {
        let why = |kind: fn(String) -> Verdict, s: &str| Ok((kind(s.to_owned()), (0, 0)));
        let declined = ours.get("stderr").and_then(|s| {
            let s = String::from_utf8_lossy(s);
            s.lines().find(|l| l.contains("cannot lower this program yet")).map(|l| {
                l.trim_start_matches("kanagawa: cannot lower this program yet: ").to_owned()
            })
        });
        if let Some(reason) = declined {
            return why(Verdict::Declined, &reason);
        }
        let first = ours
            .get("stdout")
            .map(|s| String::from_utf8_lossy(s).lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_owned())
            .unwrap_or_default();
        return why(Verdict::Failed, &format!("exit {status}: {}", first.chars().take(100).collect::<String>()));
    }
    Ok(compare(&theirs, &written(ours)))
}

/// The files a run wrote, without the output streams riding along.
fn written(files: Files) -> Files {
    files.into_iter().filter(|(k, _)| k.starts_with("out/")).collect()
}

/// Run one compiler in `cwd`, freshly emptied, and read back what it wrote
/// under `out/`, keyed `out/<name>`; its stdout and stderr ride along under
/// those two keys for the caller's reading of a failure. `status` is the
/// exit code as text, or why the run was stopped.
fn compile(exe: &Path, p: &Program, cwd: &Path, backend: Option<&Path>) -> Result<(String, Files)> {
    let _ = std::fs::remove_dir_all(cwd);
    std::fs::create_dir_all(cwd.join("out")).with_context(|| format!("creating {}", cwd.display()))?;
    if let Some(source) = &p.source {
        std::fs::write(cwd.join(&p.file_arg), source)?;
    }
    let mut command = Command::new(exe);
    command.args(&p.options).args(["--backend=sv", "-o", BASE]).arg(&p.file_arg).current_dir(cwd);
    if let Some(dir) = backend {
        command.env("KG_BACKEND_DIR", dir);
        crate::backend_cache::apply(&mut command, Some("out"));
    }
    let (status, output) = match crate::bounded::run(command, crate::bounded::Limits::compile())? {
        Ok(output) => (output.status.code().map_or_else(|| "signal".to_owned(), |c| c.to_string()), output),
        Err((over, output)) => (over.to_string(), output),
    };
    let mut files = Files::new();
    let mut names: Vec<PathBuf> =
        std::fs::read_dir(cwd.join("out"))?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
    names.sort();
    for path in names {
        let name = format!("out/{}", path.file_name().unwrap_or_default().to_string_lossy());
        files.insert(name, std::fs::read(&path)?);
    }
    files.insert("stdout".to_owned(), output.stdout);
    files.insert("stderr".to_owned(), output.stderr);
    Ok((status, files))
}

/// The stored reference side: `key`, `status`, and `files/` holding what it
/// wrote under `out/`.
fn load(store: &Path, key: &str) -> Option<(String, Files)> {
    if std::fs::read_to_string(store.join("key")).ok()? != key {
        return None;
    }
    let status = std::fs::read_to_string(store.join("status")).ok()?;
    let mut files = Files::new();
    for entry in std::fs::read_dir(store.join("files")).ok()? {
        let path = entry.ok()?.path();
        files.insert(format!("out/{}", path.file_name()?.to_string_lossy()), std::fs::read(&path).ok()?);
    }
    Some((status, files))
}

fn save(store: &Path, key: &str, status: &str, files: &Files) -> Result<()> {
    let _ = std::fs::remove_dir_all(store);
    std::fs::create_dir_all(store.join("files"))?;
    for (name, bytes) in files {
        if let Some(file) = name.strip_prefix("out/") {
            std::fs::write(store.join("files").join(file), bytes)?;
        }
    }
    std::fs::write(store.join("status"), status)?;
    // The key last: a store interrupted before it is written is a miss.
    std::fs::write(store.join("key"), key)?;
    Ok(())
}

use kg_ptbridge::locations::{locations, modulo_locations};
#[cfg(test)]
use kg_ptbridge::locations::split_location;

/// Every file of both sides, byte for byte and then modulo locations.
fn compare(theirs: &Files, ours: &Files) -> Grade {
    let (missing, extra): (Vec<&String>, Vec<&String>) =
        (theirs.keys().filter(|k| !ours.contains_key(*k)).collect(), ours.keys().filter(|k| !theirs.contains_key(*k)).collect());
    if !missing.is_empty() || !extra.is_empty() {
        return (Verdict::Differ(format!("files: missing {missing:?}, extra {extra:?}")), (0, 0));
    }
    let mut exact = true;
    let mut located = (0, 0);
    for (name, a) in theirs {
        let b = &ours[name];
        if a == b {
            let n = locations(name, a);
            located = (located.0 + n, located.1 + n);
            continue;
        }
        exact = false;
        match modulo_locations(name, a, b) {
            Ok((agree, total)) => located = (located.0 + agree, located.1 + total),
            Err(why) => return (Verdict::Differ(format!("{}: {why}", name.trim_start_matches("out/"))), located),
        }
    }
    (if exact { Verdict::Exact } else { Verdict::Equal }, located)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> Files {
        list.iter().map(|(k, v)| ((*k).to_owned(), v.as_bytes().to_vec())).collect()
    }

    /// The three shapes read off the reference's own `.sv`
    /// (`test/interface/simple_exported_class.k`, 2026-09-23).
    #[test]
    fn the_location_shapes_circt_writes_are_split_off() {
        for (line, loc) in [
            ("  logic done;\t// simple_exported_class.k:14:9", "simple_exported_class.k:14:9"),
            ("  x <= y;\t// simple_exported_class.k:14:{9,16}", "simple_exported_class.k:14:{9,16}"),
            ("  z = w;\t// simple_exported_class.k:12:5, :14:9", "simple_exported_class.k:12:5, :14:9"),
            (
                "module M(\t// {{a.k:9:5 <- 'fixture<|@Test1|>@?lambda0'(a.k:20:3)} <- 'main'(a.k:31:1)}",
                "{{a.k:9:5 <- 'fixture<|@Test1|>@?lambda0'(a.k:20:3)} <- 'main'(a.k:31:1)}",
            ),
        ] {
            assert_eq!(split_location(line).1, Some(loc), "{line}");
        }
        assert_eq!(split_location("// kanagawa --backend=sv -o out/o a.k"), ("// kanagawa --backend=sv -o out/o a.k", None));
        assert_eq!(split_location("  wire a;\t// a comment, not a place").1, None);
        assert_eq!(split_location("  wire a;\t// see a.k:3:1 for why").1, None, "prose that names a place");
        assert_eq!(split_location("  wire a;\t// {a.k:3:1 <- 'unclosed(a.k:4:1)}").1, None);
    }

    #[test]
    fn a_program_differing_only_in_locations_is_equal_and_counted() {
        let theirs = files(&[("out/o.sv", "// hdr\nwire a;\t// a.k:14:9\nwire b;\t// a.k:9:5\n")]);
        let ours = files(&[("out/o.sv", "// hdr\nwire a;\t// a.k:13:5\nwire b;\t// a.k:9:5\n")]);
        assert_eq!(compare(&theirs, &ours), (Verdict::Equal, (1, 2)));
        assert_eq!(compare(&theirs, &theirs), (Verdict::Exact, (2, 2)));
    }

    /// Red if code beside a location is ignored, if a line only one side
    /// locates is let through, or if a missing file is not a difference.
    #[test]
    fn code_extra_lines_and_files_differ() {
        let theirs = files(&[("out/o.sv", "wire a;\t// a.k:14:9\n")]);
        let code = files(&[("out/o.sv", "wire b;\t// a.k:14:9\n")]);
        assert!(matches!(compare(&theirs, &code).0, Verdict::Differ(w) if w.contains("line 1")));
        let unlocated = files(&[("out/o.sv", "wire a;\n")]);
        assert!(matches!(compare(&theirs, &unlocated).0, Verdict::Differ(_)));
        let longer = files(&[("out/o.sv", "wire a;\t// a.k:14:9\nwire c;\n")]);
        assert!(matches!(compare(&theirs, &longer).0, Verdict::Differ(w) if w.contains("more line")));
        let more = files(&[("out/o.sv", "wire a;\t// a.k:14:9\n"), ("out/o_types.sv", "")]);
        assert!(matches!(compare(&theirs, &more).0, Verdict::Differ(w) if w.contains("extra")));
    }

    /// The report's path and range columns are locations; every other
    /// column is compared. The header row names them.
    #[test]
    fn the_report_s_location_columns_are_blanked_and_nothing_else() {
        let report = |line: u32, depth: u32| {
            format!(
                r#"{{"data": [["path0", "relativePath", "rangeStartLine", "rangeStartCharacter", "rangeEndLine", "rangeEndCharacter", "Op_Depth"],
                    [[{{"name": "f"}}], "/w/a.k", {line}, 1, 13, 1, {depth}]]}}"#
            )
        };
        let theirs = files(&[("out/oPathLength.pdrpt", &report(12, 1))]);
        assert_eq!(compare(&theirs, &files(&[("out/oPathLength.pdrpt", &report(11, 1))])), (Verdict::Equal, (0, 1)));
        assert!(matches!(compare(&theirs, &files(&[("out/oPathLength.pdrpt", &report(12, 2))])).0, Verdict::Differ(_)));
    }

    /// The two phrases the backend writes a source line into an assertion's
    /// message with (`file/test/library/data/random/xoshiro.k`'s
    /// `o_test_runner_main.sv`, 2026-09-23); the function naming a frame is
    /// still compared.
    #[test]
    fn an_assertion_message_s_lines_are_locations_and_its_frames_are_not() {
        let line = |at: u32, frame: &str| {
            files(&[(
                "out/o.sv",
                &format!("assert(x) else $fatal(1, \"Assertion at: loop.k line: {at}\\n    {frame} in file loop.k at line {}\");\n", at + 5),
            )])
        };
        assert_eq!(compare(&line(134, "pipelined_for<uint10>"), &line(133, "pipelined_for<uint10>")).0, Verdict::Equal);
        assert!(matches!(compare(&line(134, "pipelined_for<uint10>"), &line(134, "pipelined_for<|$uint@10|>")).0, Verdict::Differ(_)));
    }

    #[test]
    fn a_file_with_no_locations_must_match_byte_for_byte() {
        let theirs = files(&[("out/oRtlMap.json", "{\"a\": 1}")]);
        let ours = files(&[("out/oRtlMap.json", "{\"a\": 2}")]);
        assert!(matches!(compare(&theirs, &ours).0, Verdict::Differ(w) if w.contains("holds no locations")));
    }
}
