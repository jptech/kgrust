//! `cargo xtask repoint` — run the **reference's own** test runner against this
//! toolchain's `kanagawa` binary.
//!
//! # Why this exists
//!
//! [`crate::conformance`] reimplements `test/syntax/syntax-test.py` natively.
//! That makes it fast and individually addressable, and it makes it *unable to
//! check itself*: a scoreboard written from a script cannot discover that it
//! misread the script. Every number it reports is conditional on a reading
//! nobody has audited.
//!
//! This does the opposite. It shells out to the actual `syntax-test.py`, with
//! the actual per-test flags from `CMakeLists.txt`, pointed at `target/…/kanagawa`
//! — which is what `set_target_properties(kanagawa::exe …)` would do at cutover.
//! Upstream's runner decides; nothing here interprets an assertion.
//!
//! # What a failure means, and what it does not
//!
//! `syntax-test.py` stops at the **first** mismatching assertion in a file, so a
//! result is per *file*, not per assertion — coarse on purpose, and not a
//! substitute for `xtask conformance`.
//!
//! Three causes of failure are expected today and are not defects here:
//!
//! * a file containing an assertion that expects a specific **C++ backend** code
//!   (`expected:34`, `expected:64`, …). `xtask conformance` books those as
//!   *undecidable*; upstream's runner cannot, and fails the file;
//! * a test whose flags request `--backend=sv`, which this build refuses;
//! * a genuine frontend gap — the same ones `--strict` counts.
//!
//! The invariant worth gating on is the **other** direction: no file may pass
//! here that `xtask conformance --strict` calls dirty. That would mean the
//! native harness invents failures upstream does not have.

use std::path::Path;

use anyhow::{Context, Result};

/// Where the runner is allowed to scribble. It creates one subdirectory per
/// test file and writes `syntax_test_temp_file.k` into it, so this must not be
/// the reference checkout — that tree is read-only.
fn work_dir() -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("kg-repoint-{}", std::process::id()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

pub fn run(
    kanagawa_root: &Path,
    binary: Option<&Path>,
    build_dir: &Path,
    filter: Option<&str>,
    max_failures: Option<usize>,
    verbose: bool,
) -> Result<()> {
    // **Absolute, always** — and before anything is derived from it. The runner
    // is spawned with its working directory set to a scratch tree, so every
    // relative path built from `kanagawa_root` breaks there: the script itself,
    // `-testfile`, `-helperdir`, and — least visibly — the `${CMAKE_SOURCE_DIR}`
    // expansions *inside* the per-test options, which is how the base library
    // and the import path get their values.
    //
    // The default `--kanagawa-root third_party/kanagawa` is relative, so this is the
    // normal case, not the exotic one. Getting it wrong reads as 0/72 with every
    // test "failing" for a reason the summary never shows.
    let kanagawa_root = &kanagawa_root
        .canonicalize()
        .with_context(|| format!("resolving {}", kanagawa_root.display()))?;

    // The backend library, named to every compile as `backend` and `output`
    // name it: without it the compat binary found its own compiled-in
    // fallback, which a worktree does not have beside it — a likely reason
    // a worktree's baseline once disagreed with main's on 21 tests
    // (sprint W0.4). Named and missing, the binary now fails.
    let backend_dir = build_dir.join("dist/bin").canonicalize().with_context(|| {
        format!("no reference build under {} — the backend library is needed; pass --build-dir", build_dir.display())
    })?;
    let dir = kanagawa_root.join("test/syntax");
    let script = dir.join("syntax-test.py");
    if !script.is_file() {
        anyhow::bail!("no runner at {} — pass --kanagawa-root", script.display());
    }
    // This tree's binary unless told otherwise, and never a stale one: until
    // 2026-09-22 the default was whatever `target/release/kanagawa` existed
    // (AUDIT D8).
    let binary = match binary {
        None => crate::fresh::compat_binary()?,
        Some(b) => {
            if !b.is_file() {
                anyhow::bail!("no compat binary at {}", b.display());
            }
            crate::fresh::announce_external(b);
            b.to_path_buf()
        }
    };
    // **Absolute, always.** The runner is spawned with its working directory set
    // to a scratch tree, so a relative `--binary` resolves against *that* and
    // is simply not found. `syntax-test.py` reports the resulting spawn failure
    // the same way it reports a compile failure, so every test "fails" and the
    // gate reads 0/72 while looking perfectly healthy.
    let binary = binary
        .canonicalize()
        .with_context(|| format!("resolving {}", binary.display()))?;
    // A gate that silently skips is indistinguishable from a gate that passes.
    let python = std::process::Command::new("python3").arg("--version").output();
    if python.is_err() {
        anyhow::bail!("python3 is required to run the reference's syntax-test.py");
    }

    let mut regs = crate::conformance::registrations(kanagawa_root)?;
    if let Some(f) = filter {
        regs.retain(|r| r.name.contains(f));
    }
    if regs.is_empty() {
        anyhow::bail!("no registered test matched --filter");
    }

    let work = work_dir()?;
    let mut passed = Vec::new();
    let mut failed = Vec::new();

    // One runner per registration, spread over threads. The runner writes
    // its temp snippet, `.done` and `.error` files under a subfolder named
    // for the test file, relative to its working directory — so every
    // registration gets a working directory of its own, and two that share
    // a test file under different options cannot trample each other. The
    // verdicts are printed in registration order once every runner is
    // done; one at a time this was 51 seconds, most of it python start-up
    // and one process per snippet.
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let watch = crate::backend_cache::Watch::start();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let outcomes: Vec<std::sync::Mutex<Option<Result<std::process::Output>>>> =
        regs.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some(reg) = regs.get(i) else { break };
                let own = work.join(format!("{i}"));
                let result = std::fs::create_dir_all(&own)
                    .with_context(|| format!("creating {}", own.display()))
                    .and_then(|()| {
                        let mut cmd = std::process::Command::new("python3");
                        cmd.arg(&script)
                            .arg("-kanagawa")
                            .arg(&binary)
                            .arg("-testfile")
                            .arg(dir.join(&reg.test_file))
                            .arg("-helperdir")
                            .arg(&dir)
                            .arg("-options")
                            .arg(reg.options.join(" "))
                            .current_dir(&own)
                            .env("KG_BACKEND_DIR", &backend_dir);
                        if let Some(helper) = &reg.helper {
                            cmd.arg("-helper").arg(helper);
                        }
                        crate::backend_cache::apply(&mut cmd, None);
                        // Upstream's runner and the compiler it runs, as one
                        // group; over budget, the test fails as the run's own
                        // failure would (`crate::bounded`).
                        crate::bounded::run(cmd, crate::bounded::Limits::compile())
                            .with_context(|| format!("running {}", reg.name))
                            .map(|r| r.unwrap_or_else(|(_, output)| output))
                    });
                *outcomes[i].lock().expect("no panics hold this lock") = Some(result);
            });
        }
    });
    for (reg, outcome) in regs.iter().zip(outcomes) {
        let status = outcome.into_inner().expect("threads joined").expect("every slot filled")?;
        // Worth having behind a flag rather than reading the runner's own
        // `.error` files: when this gate broke, it broke by reporting 0/72 with
        // every test failing for a reason — a path resolved against the wrong
        // directory — that no summary line could have shown.
        if verbose && !status.status.success() {
            eprintln!("--- {} opts={:?}", reg.name, reg.options);
            eprintln!("{}", String::from_utf8_lossy(&status.stdout));
            eprintln!("{}", String::from_utf8_lossy(&status.stderr));
        }
        if status.status.success() {
            passed.push(reg.name.clone());
        } else {
            failed.push(reg.name.clone());
        }
        println!("{} {}", if status.status.success() { "PASS" } else { "fail" }, reg.name);
    }

    let _ = std::fs::remove_dir_all(&work);
    watch.check()?;

    println!();
    println!("{}/{} upstream syntax tests pass against {}", passed.len(), regs.len(), binary.display());

    // The ratchet is the verdict for an unfiltered run with a baseline: which
    // registrations pass, not how many (`crate::ratchet`).
    if filter.is_none() {
        let mut ledger = crate::ratchet::Ledger::new("repoint");
        ledger.set("pass");
        for name in &passed {
            ledger.add("pass", name);
        }
        ledger.total("graded", regs.len());
        ledger.total("passed", passed.len());
        ledger.total("failed", failed.len());
        if crate::ratchet::enforce(ledger)? && max_failures.is_none() {
            return Ok(());
        }
    }

    match max_failures {
        Some(budget) if failed.len() <= budget => {
            if failed.len() < budget {
                println!("{} failing, under the budget of {budget} — lower it.", failed.len());
            }
            Ok(())
        }
        Some(budget) => {
            anyhow::bail!("{} upstream tests failed, over the budget of {budget}", failed.len())
        }
        None if !failed.is_empty() => {
            anyhow::bail!("{} upstream tests failed", failed.len())
        }
        None => Ok(()),
    }
}
