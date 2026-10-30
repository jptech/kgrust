//! `cargo xtask editions` — the language beyond the reference, graded
//! (`docs/editions/README.md`; the design, `docs/plans/editions.md` §6).
//!
//! Pass/fail, four checks; an extension is either exact or wrong, so none of
//! them is a ratchet:
//!
//! 1. **Neutral when on.** Every `.k` file of the reference's tree parses to
//!    the same tree, with the same errors, with every feature this build
//!    knows turned on as with none. Every feature so far acts through the
//!    parse alone — a later pass meets its construct only where the parser
//!    built one — so an identical parse is an identical program: rule 2
//!    (a feature claims only programs the reference rejects), measured.
//! 2. **The claims.** Every fixture that uses a feature is rejected by
//!    `kg-oracle --errors`, and by `kg` in edition 1.0: the tripwire for the
//!    day upstream gives a claimed syntax a meaning of its own.
//! 3. **The twins.** Each `accept/x.k` checks clean in its feature, its
//!    hand-written `x.ref.k` is accepted by `kg-oracle` and by `kg` in
//!    edition 1.0, and the two walks through the trace backend agree call
//!    for call, locations aside: the backend is given exactly what the
//!    reference would have given it (rule 3).
//! 4. **The rejections.** Each `reject/x.k`'s `// expect-error L:C: message`
//!    lines are exactly the errors `kg check` gives it, in order.
//!
//! Fixtures live in `tests/editions/<feature>/`, the directory named as the
//! manifest names the feature; a fixture is read in edition 1.0 with that
//! one feature on.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use kg_base::language::{Edition, Feature, Features, Language};

/// Every `.k` file under `dir`, sorted.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

pub fn run(kanagawa_root: &Path, verbose: bool) -> Result<()> {
    let root = kanagawa_root
        .canonicalize()
        .with_context(|| format!("no reference checkout at {}", kanagawa_root.display()))?;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).parent().context("the workspace")?.join("tests/editions");
    let mut failures: Vec<String> = Vec::new();

    let neutral = neutral(&root, &mut failures)?;
    println!("neutral: {neutral} reference files parsed with every feature on and with none");

    let oracle = crate::ir::find_oracle();
    if oracle.is_none() {
        failures.push("kg-oracle is not built — run oracle/build.sh first; the claims and twins need it".to_owned());
    }
    let (mut claims, mut twins, mut rejections) = (0, 0, 0);
    for feature in Feature::ALL {
        let dir = fixtures.join(feature.name());
        let mut accept = Vec::new();
        collect(&dir.join("accept"), &mut accept);
        let mut reject = Vec::new();
        collect(&dir.join("reject"), &mut reject);
        if accept.is_empty() || reject.is_empty() {
            failures.push(format!("{}: a feature needs fixtures to accept and to reject", rel(&dir, &fixtures)));
        }
        let language = Language { edition: Edition::V1_0, extra: Features::NONE.with(feature) };
        let is_twin = |p: &PathBuf| p.to_string_lossy().ends_with(".ref.k");
        for path in accept.iter().chain(&reject).filter(|p| !is_twin(p)) {
            let name = rel(path, &fixtures);
            claims += 1;
            if let Some(oracle) = &oracle
                && oracle_accepts(oracle, &root, path)?
            {
                failures.push(format!("{name}: claimed, but the reference accepts it"));
            }
            if check(&root, Language::REFERENCE, path)?.is_empty() {
                failures.push(format!("{name}: claimed, but edition 1.0 accepts it"));
            }
        }
        for path in accept.iter().filter(|p| !is_twin(p)) {
            let name = rel(path, &fixtures);
            twins += 1;
            let twin = path.with_extension("ref.k");
            if !twin.exists() {
                failures.push(format!("{name}: no twin `{}`", rel(&twin, &fixtures)));
                continue;
            }
            let errors = check(&root, language, path)?;
            if !errors.is_empty() {
                failures.push(format!("{name}: {}", errors.join("; ")));
                continue;
            }
            if let Some(oracle) = &oracle
                && !oracle_accepts(oracle, &root, &twin)?
            {
                failures.push(format!("{}: the reference rejects the twin", rel(&twin, &fixtures)));
            }
            let errors = check(&root, Language::REFERENCE, &twin)?;
            if !errors.is_empty() {
                failures.push(format!("{}: {}", rel(&twin, &fixtures), errors.join("; ")));
                continue;
            }
            let (ours, theirs) = (walk(&root, language, path)?, walk(&root, Language::REFERENCE, &twin)?);
            if let Some(at) = (0..ours.len().max(theirs.len())).find(|&i| ours.get(i) != theirs.get(i)) {
                let call = |w: &[String]| w.get(at).cloned().unwrap_or_else(|| "(the end)".to_owned());
                failures.push(format!("{name}: the walks part at call {at}: {} | twin: {}", call(&ours), call(&theirs)));
            } else if verbose {
                println!("  twin {name}: {} calls", ours.len());
            }
        }
        for path in reject.iter().filter(|p| !is_twin(p)) {
            let name = rel(path, &fixtures);
            rejections += 1;
            let text = std::fs::read_to_string(path)?;
            let expected: Vec<String> =
                text.lines().filter_map(|l| l.trim().strip_prefix("// expect-error ")).map(str::to_owned).collect();
            if expected.is_empty() {
                failures.push(format!("{name}: no `// expect-error` line"));
            }
            let got = check(&root, language, path)?;
            if got != expected {
                failures.push(format!("{name}: expected\n      {}\n    got\n      {}", expected.join("\n      "), got.join("\n      ")));
            }
        }
    }
    println!("claims: {claims} fixtures the reference rejects");
    println!("twins: {twins} programs walk as their reference twins");
    println!("rejections: {rejections} fixtures give exactly their expected errors");
    for f in &failures {
        println!("FAIL {f}");
    }
    if failures.is_empty() {
        println!("editions: pass");
        Ok(())
    } else {
        anyhow::bail!("editions: {} failures", failures.len())
    }
}

/// Check 1: the reference's tree, parsed with every feature and with none.
fn neutral(root: &Path, failures: &mut Vec<String>) -> Result<usize> {
    let mut files = Vec::new();
    for dir in ["library", "test", "doc", "tools"] {
        collect(&root.join(dir), &mut files);
    }
    if files.is_empty() {
        anyhow::bail!("no .k files under {}", root.display());
    }
    let all = kg_syntax::ParseOptions { features: Features::all(), ..Default::default() };
    let mut n = 0;
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let (off, on) = (kg_syntax::parse_file(&text), kg_syntax::parse_file_with(&text, all));
        n += 1;
        if off.green() != on.green() || off.errors() != on.errors() {
            failures.push(format!("{}: parses differently with every feature on", rel(path, root)));
        }
    }
    Ok(n)
}

/// The settings `kg` checks a fixture under: the reference's library on the
/// search path, the fixture's directory a project in `language`.
fn settings(root: &Path, language: Language, path: &Path) -> kg_config::Settings {
    let mut settings = crate::conformance::settings_for(root, &["--import-dir".to_owned(), "library".to_owned()]);
    let dir = camino::Utf8PathBuf::from_path_buf(path.parent().expect("a fixture has a directory").to_path_buf())
        .expect("the fixtures' path is UTF-8");
    settings.language = kg_config::LanguageMap::new(&dir, language);
    settings
}

/// The `Db` holding the fixture at `path` — under the twin's name too, so
/// that a fixture and its twin are one file to the walk, read in `language`.
fn db(root: &Path, language: Language, path: &Path) -> Result<(kg_db::Db, kg_base::FileId)> {
    let text = std::fs::read_to_string(path)?;
    let mut db = kg_db::Db::new(settings(root, language, path));
    let name = path.to_string_lossy().replace(".ref.k", ".k");
    let file = db.set_file_text(camino::Utf8Path::new(&name), text);
    Ok((db, file))
}

/// Every error `kg check` gives the fixture, as `L:C: message`.
fn check(root: &Path, language: Language, path: &Path) -> Result<Vec<String>> {
    let (db, file) = db(root, language, path)?;
    let text = std::fs::read_to_string(path)?;
    let index = kg_base::LineIndex::new(&text);
    Ok(db
        .check(&[file])
        .into_iter()
        .filter(kg_diag::Diagnostic::is_error)
        .map(|d| {
            let at = index.line_col(d.range.range.start());
            format!("{}:{}: {}", at.line + 1, at.col + 1, d.message)
        })
        .collect())
}

/// `kg-oracle --errors` on the file: whether the reference accepts it.
fn oracle_accepts(oracle: &Path, root: &Path, path: &Path) -> Result<bool> {
    let text = std::fs::read_to_string(path)?;
    let args = vec![
        "-I".to_owned(),
        root.join("library").to_string_lossy().into_owned(),
        "--errors".to_owned(),
        path.to_string_lossy().into_owned(),
    ];
    let output = crate::oracle_cache::run(oracle, root, &args, path.parent(), &text)?;
    Ok(matches!(crate::errors::read_oracle(root, &output.stdout, &output.stderr), crate::errors::OracleSide::Frontend(e) if e.is_empty()))
}

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("no panics hold this lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The fixture's walk through the trace backend: its calls, without sequence
/// numbers and locations — a named argument is longer than its value, so the
/// twin's columns differ (`ptdiff`'s structural reading).
fn walk(root: &Path, language: Language, path: &Path) -> Result<Vec<String>> {
    let (db, file) = db(root, language, path)?;
    let buf = SharedBuf::default();
    let mut trace = kg_ptbridge::trace::Trace::new(Box::new(buf.clone()));
    let mut options = kg_backend_sys::options::OptionValues::harness_defaults();
    options.target_device = db.settings().target_device.clone();
    let outcome = kg_ptbridge::program::run(&db, &[file], options, None, &mut trace);
    trace.flush();
    if let Some(declined) = outcome.declined {
        anyhow::bail!("{}: the walk declined: {}", path.display(), declined.reason);
    }
    let text = String::from_utf8_lossy(&buf.0.lock().expect("no panics hold this lock")).into_owned();
    Ok(text
        .lines()
        .filter_map(|l| l.splitn(3, ' ').nth(2))
        .filter(|c| !c.starts_with("SetLocation2") && !c.starts_with("UnknownLocation"))
        .map(str::to_owned)
        .collect())
}
