//! `cargo xtask backend` — our compat binary against the reference's
//! recorded runs, program by program, by what `syntax-test.py` reads: the
//! first `Error N` on stdout and the exit code (`syntax-test.py:113-160`).
//!
//! `cargo xtask ptrace` kept every reference run's output beside its trace;
//! this spawns `target/release/kanagawa` with the same flags in the same
//! directory, the backend loaded from `KG_BACKEND_DIR` or beside the
//! reference build, and compares. Where `ptdiff` grades the *calls*, this
//! grades what the backend *said* about them — the 241 backend-code
//! assertions, and every program the backend accepts — end to end, in a
//! separate process per program because `InitCompiler` is once per process
//! (`abi.md` §12.1).
//!
//! Classes: `agree`; `frontend` (one side's frontend rejected, the other's
//! did not, or both did with different codes — `conformance`'s and
//! `errors`' territory, counted here for completeness); `declined` (our
//! walk stopped at a node kind it does not emit — `ptdiff`'s territory);
//! `backend` (both reached the backend and it answered differently — the
//! walk emitted something the reference did not, or the reverse); `crash`
//! (our process died of a signal, or printed an internal compiler error's
//! call stack). Since the backend runs in a worker (`kg_ptbridge::isolate`),
//! a backend that throws or faults no longer kills the binary: it exits 3
//! with no `Error N`, which is graded like any other answer — as
//! `syntax-test.py` would grade it — and never agrees with a reference
//! that exits 0 or 1.

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};

use crate::ptrace::{Loaded, Program, Run, programs};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Class {
    Agree,
    Frontend,
    Declined,
    Backend,
    Crash,
}

/// A decline's reason cut to the bucket it is counted in — the reason and
/// the head of its path, so one site's programs group together. The
/// verbose `DECLINED` line keeps the whole `path:line:col`, which is what
/// finds a site many programs share.
fn declined_key(detail: &str, reference: &Path) -> String {
    // A path in the reason is absolute, so the key named the machine it ran
    // on: one into the reference tree is cut to the tree (`library/…`), and
    // a program's own, under a checkout's `target/`, to `target/…` — the
    // last `/target/`, so a snapshot's run and the main checkout's agree.
    let detail = detail.replace(&format!("{}/", reference.display()), "");
    let detail = match (detail.find(" /"), detail.rfind("/target/")) {
        (Some(start), Some(target)) if start < target => format!("{}{}", &detail[..=start], &detail[target + 1..]),
        _ => detail,
    };
    detail.chars().take(60).collect()
}

/// The first `Error N` on stdout, `0` if none — `first_compile_error_code`.
fn first_code(stdout: &str) -> u32 {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Error ")
            && let Some(n) = rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse().ok())
        {
            return n;
        }
    }
    0
}

/// The first `Error N at: <file> (<line>): <message>` line as it reads with
/// the file cut to its name: what the backend says and where, the
/// directory each side ran in left out. `None` without such a line.
fn error_message(stdout: &str) -> Option<String> {
    let line = stdout.lines().find(|l| first_code(l) > 0)?;
    let Some((head, rest)) = line.split_once(" at: ") else { return Some(line.to_owned()) };
    Some(match rest.split_once(" (") {
        Some((path, tail)) => format!("{head} at: {} ({tail}", path.rsplit('/').next().unwrap_or(path)),
        None => format!("{head} at: {}", rest.rsplit('/').next().unwrap_or(rest)),
    })
}

/// Both sides' first error lines, as [`error_message`] reads them.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Message {
    Same,
    /// Ours, then the reference's.
    Differ(String, String),
}

/// One program's grade: its class, the detail its line prints, and — for
/// a program both sides reject with the same backend code — whether the first
/// error line is the same too, file name, line and message, and both lines
/// where it is not (`None` where either side printed none): what §8's
/// "source locations" decision costs in the backend's messages (sprint W6).
type Judged = (Class, String, Option<Message>);

fn judge(binary: &Path, backend_dir: &Path, p: &Program, theirs: &Run) -> Judged {
    let mut command = Command::new(binary);
    command.args(&p.options).arg(&p.file_arg).current_dir(&p.cwd).env("KG_BACKEND_DIR", backend_dir);
    crate::backend_cache::apply(&mut command, None);
    // A run stopped by its budget is ours to answer for, like a crash: the
    // reference finished it (`crate::bounded`).
    let output = match crate::bounded::run(command, crate::bounded::Limits::compile()) {
        Ok(Ok(o)) => o,
        Ok(Err((over, _))) => return (Class::Crash, over.to_string(), None),
        Err(e) => return (Class::Crash, format!("spawn: {e}"), None),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let ours = (first_code(&stdout), output.status.code());
    let refs = (first_code(&theirs.stdout), theirs.exit);
    if output.status.code().is_none() || stdout.contains("Call Stack:") {
        return (Class::Crash, format!("signal or internal error: {}", stderr.lines().next().unwrap_or("")), None);
    }
    // The backend runs in a worker since D6: a C++ exception or signal there
    // is exit 3 (`kg_cli::legacy::run::BACKEND_CRASHED`) with the call it
    // came out of on stderr — still a crash, not a verdict to grade.
    if output.status.code() == Some(3) {
        return (Class::Crash, format!("backend worker: {}", stderr.lines().next().unwrap_or("")), None);
    }
    if stderr.contains("cannot lower this program yet") {
        let reason = stderr.lines().find(|l| l.contains("cannot lower")).unwrap_or("").to_owned();
        return (Class::Declined, reason.chars().skip("kanagawa: cannot lower this program yet: ".len()).collect(), None);
    }
    if ours == refs {
        // A backend code's line only: `Error 1` is the frontend's, a
        // position the `errors` gate grades with its bookings (#7, #30).
        let message = (ours.0 > 1)
            .then(|| error_message(&stdout).zip(error_message(&theirs.stdout)))
            .flatten()
            .map(|(ours, theirs)| if ours == theirs { Message::Same } else { Message::Differ(ours, theirs) });
        return (Class::Agree, String::new(), message);
    }
    let detail = format!(
        "theirs {}/exit {:?}, ours {}/exit {:?}: {}",
        refs.0,
        refs.1,
        ours.0,
        ours.1,
        stdout.lines().next().unwrap_or("").chars().take(90).collect::<String>()
    );
    if refs.0 <= 1 || ours.0 <= 1 {
        // A frontend verdict on at least one side: code 1, or a clean run
        // against a frontend rejection.
        if refs.0 > 1 || ours.0 > 1 {
            return (Class::Backend, detail, None);
        }
        return (Class::Frontend, detail, None);
    }
    (Class::Backend, detail, None)
}

/// One program, run exactly as [`run`] runs it, everything printed: the
/// instrument for a disagreement whose one-line detail is cut short.
pub fn show(kanagawa_root: &Path, build_dir: &Path, name: &str) -> Result<()> {
    let kanagawa_root = &kanagawa_root
        .canonicalize()
        .with_context(|| format!("resolving {}", kanagawa_root.display()))?;
    let backend_dir = build_dir.join("dist/bin").canonicalize().with_context(|| {
        format!("no reference build under {} — the backend library is needed; pass --build-dir", build_dir.display())
    })?;
    let binary = crate::fresh::compat_binary()?;
    let Some(p) = programs(kanagawa_root)?.into_iter().find(|p| p.name == name) else {
        anyhow::bail!("no program named {name}");
    };
    std::fs::create_dir_all(&p.cwd)?;
    if let Some(source) = &p.source {
        std::fs::write(p.cwd.join(&p.file_arg), source)?;
    }
    println!("cd {} && {} {} {}", p.cwd.display(), binary.display(), p.options.join(" "), p.file_arg);
    let mut command = Command::new(&binary);
    command.args(&p.options).arg(&p.file_arg).current_dir(&p.cwd).env("KG_BACKEND_DIR", &backend_dir);
    crate::backend_cache::apply(&mut command, None);
    let output = match crate::bounded::run(command, crate::bounded::Limits::compile())? {
        Ok(output) => output,
        Err((over, output)) => {
            println!("--- ours: {over}");
            output
        }
    };
    println!("--- ours: exit {:?}\n{}{}", output.status.code(), String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    match crate::ptrace::load(kanagawa_root, &p)? {
        Loaded::Fresh(theirs, _) => println!("--- theirs: exit {:?}\n{}{}", theirs.exit, theirs.stdout, theirs.stderr),
        Loaded::Stale => println!("--- theirs: STALE — recorded from other inputs; re-run `cargo xtask ptrace`"),
        Loaded::Unrecorded => println!("--- theirs: not recorded"),
    }
    Ok(())
}

pub fn run(kanagawa_root: &Path, build_dir: &Path, filter: Option<&str>, verbose: bool, max_failures: Option<usize>) -> Result<()> {
    let kanagawa_root = &kanagawa_root
        .canonicalize()
        .with_context(|| format!("resolving {}", kanagawa_root.display()))?;
    let backend_dir = build_dir.join("dist/bin").canonicalize().with_context(|| {
        format!("no reference build under {} — the backend library is needed; pass --build-dir", build_dir.display())
    })?;
    // Built by cargo if stale, never whatever happens to be on disk (AUDIT D8).
    let binary = crate::fresh::compat_binary()?;
    let mut work: Vec<(Program, Run)> = Vec::new();
    let mut unrecorded = 0usize;
    let mut stale = Vec::new();
    for p in programs(kanagawa_root)? {
        if !filter.is_none_or(|f| p.name.contains(f)) {
            continue;
        }
        match crate::ptrace::load(kanagawa_root, &p)? {
            Loaded::Fresh(run, _) => work.push((p, run)),
            Loaded::Stale => stale.push(p.name),
            Loaded::Unrecorded => unrecorded += 1,
        }
    }
    if work.is_empty() {
        anyhow::bail!(
            "nothing to compare ({unrecorded} unrecorded, {} stale) — run `cargo xtask ptrace` first",
            stale.len()
        );
    }
    let started = std::time::Instant::now();
    let watch = crate::backend_cache::Watch::start();
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Judged>>> = work.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= work.len() {
                        break;
                    }
                    let (p, theirs) = &work[i];
                    let _ = std::fs::create_dir_all(&p.cwd);
                    if let Some(source) = &p.source {
                        let _ = std::fs::write(p.cwd.join(&p.file_arg), source);
                    }
                    *slots[i].lock().expect("no panics hold this lock") = Some(judge(&binary, &backend_dir, p, theirs));
                }
            });
        }
    });

    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut backend_pairs = std::collections::BTreeMap::<String, (usize, String)>::new();
    let mut declined = std::collections::BTreeMap::<String, (usize, String)>::new();
    let mut lines = Vec::new();
    let mut walked_agree = 0usize;
    let mut walked = 0usize;
    let mut ledger = crate::ratchet::Ledger::new("backend");
    ledger.set("agree");
    ledger.set("crash");
    ledger.set("message");
    let (mut messages, mut messages_agree) = (0usize, 0usize);
    for ((p, theirs), slot) in work.iter().zip(slots) {
        let (class, detail, message) = slot.into_inner().expect("threads joined").expect("every slot filled");
        match message {
            Some(Message::Same) => {
                messages += 1;
                messages_agree += 1;
                ledger.add("message", &p.name);
            }
            Some(Message::Differ(ours, theirs)) => {
                messages += 1;
                if verbose {
                    lines.push(format!("  MESSAGE {}: ours `{ours}`, the reference's `{theirs}`", p.name));
                }
            }
            None => {}
        }
        *counts.entry(format!("{class:?}").to_lowercase()).or_default() += 1;
        match class {
            Class::Agree => ledger.add("agree", &p.name),
            Class::Crash => ledger.add("crash", &p.name),
            Class::Declined => ledger.histogram("declined", &declined_key(&detail, kanagawa_root)),
            Class::Backend => ledger.histogram("backend", detail.split(':').next().unwrap_or("")),
            Class::Frontend => {}
        }
        if theirs.lines > 0 {
            walked += 1;
            if class == Class::Agree {
                walked_agree += 1;
            }
        }
        match class {
            Class::Backend => {
                let key = detail.split(':').next().unwrap_or("").to_owned();
                let e = backend_pairs.entry(key).or_insert((0, String::new()));
                e.0 += 1;
                if e.1.is_empty() {
                    e.1 = format!("{}: {detail}", p.name);
                }
                if verbose {
                    lines.push(format!("  BACKEND {}: {detail}", p.name));
                }
            }
            Class::Declined => {
                let e = declined.entry(declined_key(&detail, kanagawa_root)).or_insert((0, String::new()));
                e.0 += 1;
                if e.1.is_empty() {
                    e.1 = p.name.clone();
                }
                if verbose {
                    lines.push(format!("  DECLINED {}: {detail}", p.name));
                }
            }
            Class::Crash => lines.push(format!("  CRASH {}: {detail}", p.name)),
            Class::Frontend if verbose => lines.push(format!("  FRONTEND {}: {detail}", p.name)),
            _ => {}
        }
    }
    println!(
        "backend: {} programs ({unrecorded} unrecorded, {} stale) in {:.1}s — {}",
        work.len(),
        stale.len(),
        started.elapsed().as_secs_f64(),
        counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
    );
    println!("  of the {walked} the reference walked, the backend says the same about {walked_agree}");
    println!("  of the {messages} the backend rejects alike, the first error line is the same, file name, line and message, in {messages_agree}");
    if !backend_pairs.is_empty() {
        println!("  backend disagreements, by (theirs, ours):");
        let mut v: Vec<_> = backend_pairs.iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for (k, (n, example)) in v.iter().take(15) {
            println!("    {n:5}  {k}   e.g. {example}");
        }
    }
    if !declined.is_empty() {
        println!("  declined, by reason:");
        let mut v: Vec<_> = declined.iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for (r, (n, example)) in v.iter().take(15) {
            println!("    {n:5}  {r}   e.g. {example}");
        }
    }
    for l in &lines {
        println!("{l}");
    }
    for name in stale.iter().take(10) {
        println!("  STALE {name}: recorded from other inputs — not graded");
    }
    let disagree = work.len() - counts.get("agree").copied().unwrap_or(0);
    watch.check()?;
    if filter.is_none() {
        ledger.total("graded", work.len());
        ledger.total("unrecorded", unrecorded);
        ledger.total("stale", stale.len());
        ledger.total("walked", walked);
        ledger.total("walked-agree", walked_agree);
        ledger.total("messages", messages);
        for (class, n) in &counts {
            ledger.total(class, *n);
        }
        crate::ratchet::enforce(ledger)?;
    }
    if !stale.is_empty() {
        anyhow::bail!("{} recordings are stale — `cargo xtask ptrace` re-records them", stale.len());
    }
    if let Some(max) = max_failures
        && disagree > max
    {
        anyhow::bail!("{disagree} programs disagree, above the ratchet of {max}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first `Error N` line decides, whatever follows — as
    /// `syntax-test.py:116-119` stops looking once it has a code.
    #[test]
    fn first_code_reads_the_first_error_line() {
        assert_eq!(first_code("warning: w\nError 12: first\nError 3: second\n"), 12);
    }

    /// No `Error N` line reads 0, `syntax-test.py:99`'s starting value —
    /// the code a clean run is expected to have.
    #[test]
    fn first_code_without_an_error_line_is_zero() {
        assert_eq!(first_code(""), 0);
        assert_eq!(first_code("compiled\n"), 0);
    }

    /// The code is the run of digits right after `Error `, whatever ends
    /// it — `syntax-test.py:117`'s `Error (\d+)`.
    #[test]
    fn first_code_is_the_digits_after_error() {
        assert_eq!(first_code("Error 42: message"), 42);
        assert_eq!(first_code("Error 7"), 7);
        assert_eq!(first_code("Error 12abc"), 12);
    }

    /// A line with no digits after `Error ` is passed over and the search
    /// goes on, as `Error (\d+)` does not match it (`syntax-test.py:117`);
    /// `Errors 5` is not `Error 5` to either.
    #[test]
    fn first_code_passes_over_an_error_line_without_digits() {
        assert_eq!(first_code("Error : oops\nError 9: x\n"), 9);
        assert_eq!(first_code("Errors 5\n"), 0);
    }

    /// Booked: differs from `syntax-test.py:117`, symmetric today. Upstream
    /// `re.search`es each line, so `Error 5` anywhere in it is read; here
    /// only a line that *starts* with `Error ` is. Both sides of the
    /// `backend` gate go through this function, so a program printing its
    /// code mid-line grades the same on both — but not as upstream's runner
    /// would grade it (sprint-parity.md §3 W6). The owner's to rule on.
    #[test]
    fn first_code_reads_only_a_line_starting_with_error() {
        assert_eq!(first_code("kanagawa: Error 5: x\n"), 0);
        assert_eq!(first_code("  Error 5\n"), 0);
    }

    /// Booked: differs from `syntax-test.py:116`, symmetric today. Upstream
    /// keeps searching while its code is still 0, so `Error 0` followed by
    /// `Error 5` reads 5 there; here the first line with digits returns,
    /// and reads 0. The owner's to rule on.
    #[test]
    fn first_code_stops_at_error_zero() {
        assert_eq!(first_code("Error 0\nError 5\n"), 0);
    }

    /// Booked: differs from `syntax-test.py:119`, symmetric today. A code
    /// past `u32` does not parse here and the line is passed over; Python's
    /// `int` reads it. The owner's to rule on.
    #[test]
    fn first_code_passes_over_a_code_past_u32() {
        assert_eq!(first_code("Error 99999999999\nError 5\n"), 5);
    }

    /// A program run in a scratch directory by `/bin/sh -c script`, the
    /// file argument landing in `$0` — so [`judge`] runs unchanged, its
    /// spawn included, with a script standing in for our compiler.
    fn judged(script: &str, exit: Option<i32>, stdout: &str) -> (Class, String) {
        judged_by(Path::new("/bin/sh"), script, exit, stdout)
    }

    fn judged_by(binary: &Path, script: &str, exit: Option<i32>, stdout: &str) -> (Class, String) {
        let (class, detail, _) = judged_with_message(binary, script, exit, stdout);
        (class, detail)
    }

    fn judged_with_message(binary: &Path, script: &str, exit: Option<i32>, stdout: &str) -> Judged {
        let p = Program {
            name: "syntax::t::0".to_owned(),
            cwd: std::env::temp_dir(),
            file_arg: "syntax_test_temp_file.k".to_owned(),
            source: None,
            options: vec!["-c".to_owned(), script.to_owned()],
            expectation: None,
        };
        let theirs = Run { name: p.name.clone(), exit, stdout: stdout.to_owned(), stderr: String::new(), lines: 1, codegen: None };
        judge(binary, Path::new("/nonexistent/backend"), &p, &theirs)
    }

    /// Beside agreement, the first error line is compared whole but for the
    /// directory each side ran in: the same file name, line and message is
    /// `Same`, another line `Differ` with both, and no `Error` line on
    /// either side `None`. Red with the path kept (the directories differ
    /// always) or the line number dropped.
    #[test]
    fn the_first_error_line_is_compared_but_for_the_directory() {
        let sh = Path::new("/bin/sh");
        let theirs = "Error 22 at: /reference/work/syntax_test_temp_file.k (5): Pipelined function requires unsigned integer count parameter\nError: Type checking failed\n";
        let same = "echo 'Error 22 at: /ours/elsewhere/syntax_test_temp_file.k (5): Pipelined function requires unsigned integer count parameter'; exit 1";
        let moved = "echo 'Error 22 at: /ours/elsewhere/syntax_test_temp_file.k (6): Pipelined function requires unsigned integer count parameter'; exit 1";
        // The frontend's code is a position the `errors` gate grades.
        let frontend = "Error 1: /reference/work/syntax_test_temp_file.k:4:1:\n";
        assert_eq!(judged_with_message(sh, "echo 'Error 1: /ours/syntax_test_temp_file.k:8:1:'; exit 1", Some(1), frontend).2, None);
        assert_eq!(judged_with_message(sh, same, Some(1), theirs), (Class::Agree, String::new(), Some(Message::Same)));
        let (_, _, differ) = judged_with_message(sh, moved, Some(1), theirs);
        assert!(matches!(&differ, Some(Message::Differ(ours, _)) if ours.contains("(6)")), "{differ:?}");
        assert_eq!(judged_with_message(sh, "exit 0", Some(0), ""), (Class::Agree, String::new(), None));
        assert_eq!(error_message("Error 3 at: a/b/c.k (7): m"), Some("Error 3 at: c.k (7): m".to_owned()));
    }

    /// Agreement is the first code and the exit code, never the message —
    /// the two things `syntax-test.py:137` and `:158` grade.
    #[test]
    fn agreement_is_the_code_and_the_exit_not_the_message() {
        assert_eq!(judged("echo 'Error 5: ours'; exit 1", Some(1), "Error 5: theirs, worded otherwise\n"), (Class::Agree, String::new()));
    }

    /// The exit code is graded on its own: no code on either side, but a
    /// different exit, is a disagreement (`syntax-test.py:158`).
    #[test]
    fn a_different_exit_with_the_same_code_disagrees() {
        assert_eq!(judged("exit 0", Some(1), ""), (Class::Frontend, "theirs 0/exit Some(1), ours 0/exit Some(0): ".to_owned()));
    }

    /// Code 1 against no code, either way round, is a frontend verdict —
    /// `conformance`'s and `errors`' territory, not the backend's.
    #[test]
    fn a_frontend_code_against_a_clean_run_is_frontend() {
        assert_eq!(judged("exit 0", Some(1), "Error 1: theirs\n").0, Class::Frontend);
        assert_eq!(judged("echo 'Error 1: ours'; exit 1", Some(0), "").0, Class::Frontend);
    }

    /// A code above 1 on either side makes the disagreement the backend's,
    /// both codes above 1 included. The detail's text before its first
    /// `:` — both sides' code and exit — is the key of the `backend`
    /// ledger's histogram (`run`), so its format is pinned whole.
    #[test]
    fn a_backend_code_on_either_side_is_backend() {
        assert_eq!(
            judged("echo 'Error 1: ours'; exit 1", Some(1), "Error 42: theirs\n"),
            (Class::Backend, "theirs 42/exit Some(1), ours 1/exit Some(1): Error 1: ours".to_owned())
        );
        assert_eq!(judged("echo 'Error 42: ours'; exit 1", Some(0), "").0, Class::Backend);
        assert_eq!(judged("echo 'Error 43: ours'; exit 1", Some(1), "Error 42: theirs\n").0, Class::Backend);
    }

    /// The detail quotes 90 characters of our first stdout line, no more.
    #[test]
    fn the_detail_quotes_ninety_characters_of_our_first_line() {
        let line = format!("Error 42: {}", "x".repeat(100));
        let (class, detail) = judged(&format!("echo '{line}'; exit 1"), Some(0), "");
        assert_eq!(class, Class::Backend);
        assert_eq!(detail, format!("theirs 0/exit Some(0), ours 42/exit Some(1): {}", &line[..90]));
    }

    /// Death by a signal is a crash, whatever the reference said; the
    /// detail carries our first stderr line.
    #[test]
    fn a_signal_is_a_crash() {
        assert_eq!(judged("echo boom >&2; kill -9 $$", Some(0), ""), (Class::Crash, "signal or internal error: boom".to_owned()));
    }

    /// `Call Stack:` on our stdout is an internal compiler error — a crash
    /// even where the codes agree, as `syntax-test.py:126,133` fails the
    /// test before it compares a code.
    #[test]
    fn a_call_stack_is_a_crash_even_where_the_codes_agree() {
        assert_eq!(
            judged("echo 'Error 5: x'; echo 'Call Stack:'; echo ice >&2; exit 1", Some(1), "Error 5: x\n"),
            (Class::Crash, "signal or internal error: ice".to_owned())
        );
    }

    /// Booked: differs from `syntax-test.py:126`, which counts a line whose
    /// whole stripped text is `Call Stack:`; here the text anywhere on
    /// stdout is a crash. Not symmetric: the reference's recorded stdout is
    /// never searched for it. The owner's to rule on.
    #[test]
    fn call_stack_mid_line_is_a_crash_here() {
        assert_eq!(judged("echo 'note: see Call Stack: above'; exit 0", Some(0), "").0, Class::Crash);
    }

    /// Exit 3 is the backend worker's crash (`kg_cli::legacy::run::BACKEND_CRASHED`),
    /// never a verdict to grade — even against a reference that also exited 3.
    #[test]
    fn a_backend_worker_crash_is_a_crash() {
        assert_eq!(judged("echo 'worker died' >&2; exit 3", Some(3), ""), (Class::Crash, "backend worker: worker died".to_owned()));
    }

    /// A decline's bucket names the program from `target/` on, whichever
    /// checkout — or machine — ran it: the baseline once held a key with
    /// `/Users/…/kgrust/` in it. Red with the path left absolute.
    #[test]
    fn a_declined_key_names_no_machine() {
        let reference = Path::new("/home/a/kgrust/third_party/kanagawa");
        let key = |root: &str| declined_key(&format!("fold:static assert at {root}/target/ptrace/work/s.k:3:1"), reference);
        assert_eq!(key("/home/a/kgrust"), "fold:static assert at target/ptrace/work/s.k:3:1");
        assert_eq!(key("/home/a/kgrust"), key("/Users/b/x/kgrust/target/snapshot/tree/xtask/.."));
        assert_eq!(
            declined_key("fold:int<N> at /home/a/kgrust/third_party/kanagawa/library/data/bits.k:9:2", reference),
            "fold:int<N> at library/data/bits.k:9:2"
        );
        assert_eq!(declined_key("fold:attribute", reference), "fold:attribute");
    }

    /// A walk that declined is `Declined`, even where its exit matches the
    /// reference's; the detail is the reason as `kg_cli::legacy::run`
    /// prints it, read from whichever stderr line carries it, with the
    /// fixed prefix cut.
    #[test]
    fn a_decline_is_declined_with_its_reason() {
        let script = "echo 'warning: first' >&2; \
                      echo 'kanagawa: cannot lower this program yet: lambda at a.k:1:2 (cargo xtask ptdiff counts these)' >&2; \
                      exit 1";
        assert_eq!(judged(script, Some(1), ""), (Class::Declined, "lambda at a.k:1:2 (cargo xtask ptdiff counts these)".to_owned()));
    }

    /// A compiler that cannot be started is ours to answer for: a crash.
    #[test]
    fn a_compiler_that_cannot_start_is_a_crash() {
        let (class, detail) = judged_by(Path::new("/nonexistent/kanagawa"), "", Some(0), "");
        assert_eq!(class, Class::Crash);
        assert!(detail.starts_with("spawn: "), "{detail}");
    }
}
