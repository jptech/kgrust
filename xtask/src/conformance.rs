//! `cargo xtask conformance` — the scoreboard.
//!
//! A native reimplementation of `test/syntax/syntax-test.py`'s semantics, so
//! every one of the 2,843 `expected:` assertions in the reference corpus becomes
//! an individually named test rather than one opaque pass/fail per file.
//!
//! The important property of this corpus is that **the oracle is embedded in the
//! test files**: `expected:0` means the frontend must accept the snippet,
//! `expected:1` means it must reject it, and a higher code means the C++ middle
//! end rejects it and the frontend must pass it through. So conformance is
//! measurable with no reference binary anywhere in sight.
//!
//! ## The harness's exact semantics
//!
//! From `test/syntax/syntax-test.py:60-140`, reproduced deliberately rather than
//! approximated:
//!
//! * Lines accumulate into a temp file. A line matching `expected:(\d*)` or
//!   `warning:([^\s]*)` is a **marker**: it is *not* appended, it triggers a
//!   compile of what has accumulated so far, and then the accumulator resets to
//!   just the header.
//! * The header is `import helper.<name>` when the test registers a helper.
//! * `expected:` with no digits means `0`.
//! * The error match wins: a line with both markers is treated as an error marker
//!   (the script uses `elif`).
//! * `warning:0` asserts that *no* warning was produced; a bare `warning:`
//!   asserts nothing about warnings.
//! * The first `Error (\d+)` in stdout is the observed code; `0` if none.
//! * The first line matching `warning:.*\[--Wno-([^\]]*)\]$` gives the observed
//!   warning name — the regex is anchored at end of line.
//! * A line that is exactly `Call Stack:` is an internal compiler error and
//!   fails the test regardless of anything else.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

/// One assertion: a snippet plus what the reference says happens to it.
#[derive(Debug, Clone)]
pub struct Assertion {
    /// Test name, e.g. `syntax::basics::17`.
    pub name: String,
    /// Source file the marker came from.
    pub file: PathBuf,
    /// 1-based line of the marker.
    pub marker_line: usize,
    /// The accumulated snippet, header included.
    pub source: String,
    pub expectation: Expectation,
    /// False when the assertion comes from a corpus file ctest never runs.
    pub registered: bool,
    /// The compiler flags this snippet's ctest case runs under — the default
    /// `set(OPTIONS …)` block plus whatever the `add_syntax_test` call appends
    /// (`CMakeLists.txt:39-48`). Shared rather than cloned: 2,922 assertions
    /// draw on 73 distinct lists.
    pub options: Arc<[String]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// `expected:<n>`.
    Code(u32),
    /// `warning:<name>`; `None` means `warning:0`, i.e. no warning at all.
    Warning(Option<String>),
    /// A bare `warning:` — compiles, asserts only that no error occurs.
    AnyWarning,
}

/// One registered ctest case, from `test/syntax/CMakeLists.txt`.
#[derive(Debug, Clone, Default)]
pub struct TestRegistration {
    pub name: String,
    pub test_file: String,
    pub helper: Option<String>,
    pub options: Vec<String>,
    /// False for a corpus file that exists but has no `add_syntax_test` call.
    ///
    /// There is exactly one today — `test/syntax/basics.k`, the *largest* test
    /// file in the corpus at 406 assertions, which upstream never runs. Its
    /// assertions are perfectly good; they are simply orphaned. Running them
    /// costs nothing and is 16% more signal than the live ctest suite provides.
    pub registered: bool,
}

/// The `${…}` substitutions the syntax-test CMake file uses in its option
/// values. Only two appear, and both are directories.
struct CMakeVars {
    /// `${CMAKE_SOURCE_DIR}` — the repository root.
    source_dir: String,
    /// `${CMAKE_CURRENT_SOURCE_DIR}` — `test/syntax`.
    current_source_dir: String,
}

impl CMakeVars {
    fn expand(&self, arg: &str) -> String {
        arg.replace("${CMAKE_CURRENT_SOURCE_DIR}", &self.current_source_dir)
            .replace("${CMAKE_SOURCE_DIR}", &self.source_dir)
    }
}

/// The `set(OPTIONS …)` block every syntax test starts from
/// (`CMakeLists.txt:39-44`), plus the two flags `syntax-test.py:106` appends
/// after it.
///
/// Parsed rather than hardcoded for the same reason the registrations are: the
/// defaults carry `--base-library` and `--import-dir`, so a change to them
/// silently re-points the whole corpus. `-helperdir` is not in the CMake block
/// at all — it reaches the compiler as `--import-dir <CMAKE_CURRENT_SOURCE_DIR>`
/// from the Python runner, and is what makes `import helper.foo` resolve.
fn default_options(text: &str, vars: &CMakeVars) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(idx) = text.find("set(OPTIONS") {
        let start = idx + "set(".len();
        if let Some(len) = balanced_len(&text[start..]) {
            out.extend(
                tokenize_cmake(&text[start..start + len])
                    .into_iter()
                    .skip(1) // the `OPTIONS` variable name
                    .map(|a| vars.expand(&a)),
            );
        }
    }
    // `cmd.extend(['--place-iterations=1', '--import-dir', args.helperdir, …])`
    // (`syntax-test.py:106`), appended *after* the CMake options, which is the
    // order `importDir` accumulates in.
    out.push("--place-iterations=1".to_owned());
    out.push("--import-dir".to_owned());
    out.push(vars.current_source_dir.clone());
    out
}

/// Parses the `add_syntax_test(...)` calls out of the reference CMake file.
///
/// Reading the registration rather than hardcoding it means an upstream sync
/// shows up as new assertions instead of silent drift.
///
/// Each registration's `options` is the **effective** command line — defaults
/// first, then the call's own `OPTIONS`. That ordering is load-bearing: a test
/// that overrides `--target-device` relies on the later flag winning, and
/// `--import-dir` accumulates rather than replaces.
fn read_registrations(
    text: &str,
    defaults: &[String],
    vars: &CMakeVars,
) -> Vec<TestRegistration> {
    let mut out = Vec::new();

    for (idx, _) in text.match_indices("add_syntax_test(") {
        let start = idx + "add_syntax_test(".len();
        let Some(len) = balanced_len(&text[start..]) else { continue };
        let args = tokenize_cmake(&text[start..start + len]);
        let mut reg = TestRegistration::default();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "HELPER" => {
                    reg.helper = args.get(i + 1).cloned();
                    i += 2;
                }
                "TEST_FILE" => {
                    reg.test_file = args.get(i + 1).cloned().unwrap_or_default();
                    i += 2;
                }
                "OPTIONS" => {
                    i += 1;
                    while i < args.len() && !matches!(args[i].as_str(), "HELPER" | "TEST_FILE") {
                        reg.options.push(vars.expand(&args[i]));
                        i += 1;
                    }
                }
                _ if reg.name.is_empty() => {
                    reg.name = args[i].clone();
                    i += 1;
                }
                _ => i += 1,
            }
        }
        if reg.test_file.is_empty() {
            reg.test_file = format!("{}.k", reg.name);
        }
        // The default block is *prepended*, so a per-test flag repeating one of
        // its keys wins — `memory`'s `--target-device=mock` and `custom-base`'s
        // `--base-library` both depend on that.
        let mut options = defaults.to_vec();
        options.append(&mut reg.options);
        reg.options = options;
        reg.registered = true;
        if !reg.name.is_empty() {
            out.push(reg);
        }
    }
    out
}

/// Length of the text up to the `)` matching an already-consumed `(`.
fn balanced_len(text: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits a CMake argument list, honoring double quotes.
fn tokenize_cmake(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in text.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Expands one test file into its individually named assertions.
///
/// `test_name` becomes the prefix, so a failure reads
/// `syntax::declaration-order::12` and points at a single snippet.
pub fn expand(reg: &TestRegistration, dir: &Path) -> Result<Vec<Assertion>> {
    let path = dir.join(&reg.test_file);
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;

    let header = match &reg.helper {
        Some(h) => format!("import helper.{h}\n"),
        None => String::new(),
    };
    let options: Arc<[String]> = reg.options.as_slice().into();

    let mut out = Vec::new();
    let mut acc = header.clone();
    let mut index = 0usize;

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let Some(expectation) = parse_marker(line) else {
            // Not a marker: it is snippet text. The script writes the *stripped*
            // line (`f.write(line + '\n')`, syntax-test.py:88), so leading
            // indentation is discarded — reproduce that, because it changes
            // reported columns.
            acc.push_str(line);
            acc.push('\n');
            continue;
        };

        out.push(Assertion {
            name: format!("syntax::{}::{index}", reg.name),
            file: path.clone(),
            marker_line: lineno + 1,
            source: std::mem::replace(&mut acc, header.clone()),
            expectation,
            registered: reg.registered,
            options: Arc::clone(&options),
        });
        index += 1;
    }
    Ok(out)
}

/// The marker regexes of `syntax-test.py:71-77`, hand-rolled to keep xtask
/// dependency-free.
fn parse_marker(line: &str) -> Option<Expectation> {
    if let Some(rest) = find_after(line, "expected:") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        // `expected:` with no digits means 0 (`int(match.group(1) or 0)`).
        return Some(Expectation::Code(digits.parse().unwrap_or(0)));
    }
    // `elif`: an error marker on the same line wins, so this is only reached
    // when there is no `expected:`.
    if let Some(rest) = find_after(line, "warning:") {
        let name: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        return Some(match name.as_str() {
            "" => Expectation::AnyWarning,
            "0" => Expectation::Warning(None),
            other => Expectation::Warning(Some(other.to_owned())),
        });
    }
    None
}

fn find_after<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    haystack.find(needle).map(|i| &haystack[i + needle.len()..])
}

/// What the reference says the *frontend* alone decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendVerdict {
    /// `expected:0` — the frontend must accept.
    MustAccept,
    /// `expected:1` — the frontend must reject. Every reference frontend error
    /// is code 1 (`Parser/IO.hs:114`), so this says nothing about the message.
    MustReject,
    /// A specific C++ code: the frontend must pass the snippet through, and the
    /// backend decides. Not checkable without the backend — and a frontend that
    /// "improves" by rejecting these *fails* them.
    PassThrough(u32),
    /// A warning assertion; needs the semantic passes.
    NeedsSemantics,
}

impl Expectation {
    pub fn frontend_verdict(&self) -> FrontendVerdict {
        match self {
            Expectation::Code(0) => FrontendVerdict::MustAccept,
            Expectation::Code(1) => FrontendVerdict::MustReject,
            Expectation::Code(n) => FrontendVerdict::PassThrough(*n),
            Expectation::Warning(_) | Expectation::AnyWarning => FrontendVerdict::NeedsSemantics,
        }
    }
}

/// Every `add_syntax_test` call, with its **effective** command line.
///
/// Public because [`crate::repoint`] drives the reference's own
/// `syntax-test.py` with exactly these flags. One parser, two consumers: if the
/// two disagreed about what upstream runs, the cross-check between them would be
/// worthless — which is the entire reason `repoint` exists.
pub fn registrations(kanagawa_root: &Path) -> Result<Vec<TestRegistration>> {
    let dir = kanagawa_root.join("test/syntax");
    let cmake = dir.join("CMakeLists.txt");
    let text = std::fs::read_to_string(&cmake)
        .with_context(|| format!("reading {}", cmake.display()))?;
    let vars = CMakeVars {
        source_dir: kanagawa_root.display().to_string(),
        current_source_dir: dir.display().to_string(),
    };
    let defaults = default_options(&text, &vars);
    Ok(read_registrations(&text, &defaults, &vars))
}

/// Loads every assertion in the reference corpus, registered or not.
pub fn load_all(kanagawa_root: &Path) -> Result<Vec<Assertion>> {
    let dir = kanagawa_root.join("test/syntax");
    let cmake = dir.join("CMakeLists.txt");
    let text = std::fs::read_to_string(&cmake)
        .with_context(|| format!("reading {}", cmake.display()))?;
    let vars = CMakeVars {
        source_dir: kanagawa_root.display().to_string(),
        current_source_dir: dir.display().to_string(),
    };

    let defaults = default_options(&text, &vars);
    let mut regs = read_registrations(&text, &defaults, &vars);
    regs.extend(unregistered(&dir, &regs, &defaults)?);

    let mut out = Vec::new();
    for reg in &regs {
        out.extend(expand(reg, &dir)?);
    }
    Ok(out)
}

/// Test files present in `test/syntax/` that no `add_syntax_test` call names.
///
/// Reported rather than silently included or silently dropped: a file becoming
/// orphaned upstream is exactly the kind of drift the sync ritual should
/// surface.
fn unregistered(
    dir: &Path,
    regs: &[TestRegistration],
    defaults: &[String],
) -> Result<Vec<TestRegistration>> {
    let known: Vec<&str> = regs.iter().map(|r| r.test_file.as_str()).collect();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)?.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "k") {
            continue;
        }
        let Some(file) = path.file_name().and_then(|f| f.to_str()) else { continue };
        if known.contains(&file) {
            continue;
        }
        out.push(TestRegistration {
            name: file.trim_end_matches(".k").to_owned(),
            test_file: file.to_owned(),
            helper: None,
            // What it *would* run with, had anyone registered it.
            options: defaults.to_vec(),
            registered: false,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// A one-screen summary of where the frontend stands.
pub fn summarize(assertions: &[Assertion]) -> String {
    let mut accept = 0;
    let mut reject = 0;
    let mut pass_through = 0;
    let mut semantics = 0;
    for a in assertions {
        match a.expectation.frontend_verdict() {
            FrontendVerdict::MustAccept => accept += 1,
            FrontendVerdict::MustReject => reject += 1,
            FrontendVerdict::PassThrough(_) => pass_through += 1,
            FrontendVerdict::NeedsSemantics => semantics += 1,
        }
    }
    let total = assertions.len();
    let orphaned = assertions.iter().filter(|a| !a.registered).count();
    let mut out = String::new();
    let _ = writeln!(out, "{total} assertions from {} test files", count_files(assertions));
    if orphaned > 0 {
        let _ = writeln!(
            out,
            "  ({orphaned} of them from files ctest never runs — see `unregistered`)"
        );
    }
    let _ = writeln!(out, "  {accept:>5}  must accept        (expected:0)");
    let _ = writeln!(out, "  {reject:>5}  must reject        (expected:1)");
    let _ = writeln!(out, "  {pass_through:>5}  pass through       (specific C++ code — needs the backend)");
    let _ = writeln!(out, "  {semantics:>5}  warning assertions (needs the semantic passes)");
    out
}

fn count_files(assertions: &[Assertion]) -> usize {
    let mut files: Vec<&Path> = assertions.iter().map(|a| a.file.as_path()).collect();
    files.sort_unstable();
    files.dedup();
    files.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_parsing_matches_the_python() {
        assert_eq!(parse_marker("expected:1"), Some(Expectation::Code(1)));
        assert_eq!(parse_marker("expected:"), Some(Expectation::Code(0)));
        assert_eq!(parse_marker("expected:34"), Some(Expectation::Code(34)));
        // The error branch wins over the warning branch (`elif`).
        assert_eq!(parse_marker("expected:0 warning:shadow"), Some(Expectation::Code(0)));
        assert_eq!(
            parse_marker("warning:shadow"),
            Some(Expectation::Warning(Some("shadow".to_owned())))
        );
        assert_eq!(parse_marker("warning:0"), Some(Expectation::Warning(None)));
        assert_eq!(parse_marker("warning:"), Some(Expectation::AnyWarning));
        assert_eq!(parse_marker("    uint32 x;"), None);
    }

    #[test]
    fn cmake_argument_tokenizing() {
        let args = tokenize_cmake(r#""base-test" OPTIONS --no-implicit-base=false --template-iterations=1"#);
        assert_eq!(args[0], "base-test");
        assert_eq!(args[1], "OPTIONS");
        assert_eq!(args.len(), 4);
    }
}

// ---------------------------------------------------------------------------
// Running the assertions
// ---------------------------------------------------------------------------

/// The outcome of checking one snippet with whatever frontend is wired in.
#[derive(Debug, Clone, Default)]
pub struct CheckResult {
    /// The first error code the frontend would print, or 0 for a clean compile.
    pub first_error_code: u32,
    /// The first error's message. Carried purely so a failing assertion can say
    /// *why* — "expected a clean compile, got Error 1" names the symptom and
    /// costs an afternoon; naming the diagnostic costs nothing and points at
    /// the cause.
    pub first_error: Option<String>,
    /// The first warning name, if any.
    pub first_warning: Option<String>,
    /// True if the frontend crashed — the reference prints `Call Stack:` and
    /// the harness treats it as an automatic failure whatever else happened.
    pub internal_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail(String),
    /// The frontend cannot decide this yet — a specific C++ code, or a warning
    /// that needs the semantic passes. Counted separately and never as a pass,
    /// so the scoreboard never flatters itself.
    Undecidable(&'static str),
}

/// Scores one assertion against a frontend result.
///
/// `frontend_complete` says whether the wired-in frontend implements the
/// semantic passes. Until it does, an `expected:1` snippet that parses cleanly
/// is *undecidable* rather than a failure — the error it should report may be a
/// type error, which no parser can see. An `expected:0` snippet that fails to
/// parse is always a real failure.
pub fn score(assertion: &Assertion, result: &CheckResult, frontend_complete: bool) -> Outcome {
    if result.internal_error {
        return Outcome::Fail("internal compiler error (`Call Stack:`)".to_owned());
    }
    match assertion.expectation.frontend_verdict() {
        FrontendVerdict::MustAccept => {
            if result.first_error_code == 0 {
                Outcome::Pass
            } else {
                Outcome::Fail(format!(
                    "expected a clean compile, got Error {}{}",
                    result.first_error_code,
                    result.first_error.as_deref().map_or(String::new(), |m| format!(": {m}"))
                ))
            }
        }
        FrontendVerdict::MustReject => match (result.first_error_code, frontend_complete) {
            (1, _) => Outcome::Pass,
            (0, false) => Outcome::Undecidable("rejection may need the semantic passes"),
            (0, true) => Outcome::Fail("expected Error 1, compiled cleanly".to_owned()),
            (n, _) => Outcome::Fail(format!("expected Error 1, got Error {n}")),
        },
        FrontendVerdict::PassThrough(code) => {
            // The frontend must NOT catch these. Reporting an error here is a
            // real failure even without the backend: it means the Rust frontend
            // diagnoses earlier than the reference one, which breaks 241
            // assertions. Staying silent is as far as we can get on our own.
            if result.first_error_code == 0 {
                Outcome::Undecidable("needs the C++ backend to produce the expected code")
            } else {
                Outcome::Fail(format!(
                    "frontend reported Error {} but the reference defers this to the backend as Error {code}",
                    result.first_error_code
                ))
            }
        }
        FrontendVerdict::NeedsSemantics => {
            if result.first_error_code != 0 {
                return Outcome::Fail(format!(
                    "expected a warning assertion to compile, got Error {}",
                    result.first_error_code
                ));
            }
            match (&assertion.expectation, frontend_complete) {
                (_, false) => Outcome::Undecidable("warnings need the semantic passes"),
                (Expectation::AnyWarning, true) => Outcome::Pass,
                (Expectation::Warning(None), true) => match &result.first_warning {
                    None => Outcome::Pass,
                    Some(w) => Outcome::Fail(format!("expected no warning, got `{w}`")),
                },
                (Expectation::Warning(Some(want)), true) => match &result.first_warning {
                    Some(got) if got == want => Outcome::Pass,
                    other => Outcome::Fail(format!("expected warning `{want}`, got {other:?}")),
                },
                (Expectation::Code(_), true) => unreachable!("not a warning assertion"),
            }
        }
    }
}

/// Aggregate scoreboard.
#[derive(Debug, Default, Clone)]
pub struct Scoreboard {
    pub passed: usize,
    pub failed: usize,
    pub undecidable: usize,
    pub failures: Vec<(String, String)>,
}

impl Scoreboard {
    pub fn record(&mut self, name: &str, outcome: Outcome) {
        match outcome {
            Outcome::Pass => self.passed += 1,
            Outcome::Undecidable(_) => self.undecidable += 1,
            Outcome::Fail(why) => {
                self.failed += 1;
                self.failures.push((name.to_owned(), why));
            }
        }
    }

    pub fn report(&self) -> String {
        let mut out = String::new();
        let decided = self.passed + self.failed;
        let _ = writeln!(
            out,
            "{}/{decided} decidable assertions pass  ({} undecidable without the backend)",
            self.passed, self.undecidable
        );
        for (name, why) in self.failures.iter().take(40) {
            let _ = writeln!(out, "  FAIL {name}: {why}");
        }
        if self.failures.len() > 40 {
            let _ = writeln!(out, "  ... and {} more", self.failures.len() - 40);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// The `cargo xtask conformance` entry point
// ---------------------------------------------------------------------------

/// Checks one snippet with the Rust frontend: every pass it runs, through
/// `Db::check_snippet`. With [`FRONTEND_COMPLETE`] set, the `expected:1`
/// assertions and the warnings are a gate, not undecidable.
fn check_snippet(db: &mut kg_db::Db, path: &camino::Utf8Path, source: &str) -> CheckResult {
    debug_assert_eq!(kg_syntax::parse_file(source).text(), source, "the tree lost text");

    // One seam: `Db::check_snippet` runs every frontend pass that exists. As
    // passes land there, this scoreboard moves without being touched — which is
    // the point, since a scoreboard the implementation can edit is not a
    // scoreboard.
    //
    // The `Db` is threaded through rather than built per snippet so that
    // `library/` and the helper modules are parsed once for the whole run
    // instead of ~2,900 times.
    let diagnostics = db.check_snippet(path, source);

    let first_warning = diagnostics
        .iter()
        .find_map(|d| d.warning.map(|w| w.name().to_owned()));

    let first_error = diagnostics.iter().find(|d| d.is_error());
    CheckResult {
        // Every frontend error the reference reports is code 1
        // (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114`).
        first_error_code: u32::from(first_error.is_some()),
        first_error: first_error.map(|d| d.message.clone()),
        first_warning,
        internal_error: false,
    }
}


/// Scores every assertion, spread over `jobs` threads that each own a `Db`.
///
/// **A driver, not a grade.** What decides an outcome is `check_snippet` and
/// `score`, called here exactly as the single loop called them; this only
/// decides which thread calls them. Outcomes come back indexed by assertion, so
/// the report reads in corpus order whatever the threads did.
///
/// One `Db` per thread rather than per snippet: the reference forks a process
/// per test file, which would mean re-parsing `library/` 73 times, and here
/// the flags are an input `set_settings` takes. Work is handed out in
/// **consecutive** chunks because assertions arrive grouped by test file, so a
/// chunk switches settings a few times at most and the parse memo behind each
/// switch is shared across the snippets that follow. `jobs == 1` is the old
/// loop, one `Db` in corpus order, and is the reading to fall back to if the
/// sharded one ever disagrees with it — a disagreement would mean a snippet's
/// verdict depends on what the `Db` checked before it, which is a bug in the
/// memo, not in this fan-out.
fn check_all(kanagawa_root: &Path, assertions: &[Assertion], complete: bool, jobs: usize) -> Vec<Outcome> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let chunk = assertions.len().div_ceil(jobs * 4).max(1);
    let next = AtomicUsize::new(0);
    let out: Vec<Mutex<Option<Outcome>>> = assertions.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                let mut db = kg_db::Db::new(settings_for(kanagawa_root, &[]));
                let mut current: Option<Arc<[String]>> = None;
                loop {
                    let start = next.fetch_add(chunk, Ordering::SeqCst);
                    if start >= assertions.len() {
                        break;
                    }
                    for (i, a) in assertions.iter().enumerate().skip(start).take(chunk) {
                        if current.as_deref() != Some(&*a.options) {
                            db.set_settings(settings_for(kanagawa_root, &a.options));
                            current = Some(Arc::clone(&a.options));
                        }
                        let result = check_snippet(&mut db, &snippet_path(kanagawa_root, a), &a.source);
                        *out[i].lock().expect("no panics hold this lock") = Some(score(a, &result, complete));
                    }
                }
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined").expect("every slot filled")).collect()
}

/// The reference's own invocation, reproduced from its own command line.
///
/// Every syntax test is a separate compiler process with its own flags. The
/// list arrives here already assembled by [`default_options`] and
/// [`read_registrations`] — the default `set(OPTIONS …)` block, then the
/// `add_syntax_test` call's own `OPTIONS`, then `syntax-test.py:106`'s
/// `--import-dir <helperdir>`.
///
/// **Reading the flags rather than assuming one global configuration is not a
/// nicety.** Thirteen of the seventy-three cases override something, and the
/// overrides are load-bearing: `imports` adds
/// `--import-dir=<test/syntax>/imports`, without which 19 assertions report a
/// missing `a1/b1/b1.k` the reference finds; `custom-base` swaps the base
/// library outright; `no-warning` disables `shadow`; `all-warnings-as-errors`
/// turns every warning into an error.
///
/// Flags with no frontend meaning here — `--jobs`, `--backend`, `--stall`,
/// `--place-iterations`, `--template-iterations` — are deliberately ignored
/// rather than rejected: they select or tune the C++ backend, which this
/// toolchain does not have, and `syntax-test.py` runs with no `--backend` for
/// 72 of the 73 cases anyway.
pub(crate) fn settings_for(kanagawa_root: &Path, options: &[String]) -> kg_config::Settings {
    let root = camino::Utf8PathBuf::from_path_buf(
        kanagawa_root.canonicalize().unwrap_or_else(|_| kanagawa_root.to_path_buf()),
    )
    .expect("the reference path is UTF-8");

    let mut settings = kg_config::Settings::fallback(root.clone());
    // The port's verdict switches (`KG_LEFTOVERS`, …; `kg_config::Verdicts`,
    // sprint W8.2) are read at every front door, and the gates are one: a
    // run under a switch grades the compiler as `kg` and `kanagawa` run it
    // under that switch — what W1.4's gate to switch is stated in. Unset,
    // they are the defaults, and every grade is what it was (owner's
    // decision, 2026-09-27).
    settings.verdicts = kg_config::Verdicts::read(|name| std::env::var_os(name));
    // `mock` is the *default* (`Parser/Options.hs:47`), passed on no command
    // line and therefore easy to miss. The search path is
    // `map (</> "device" </> targetDevice) importDir ++ importDir`
    // (`Parser/IO.hs:110`), so without it `library/device/mock/hardware/config.k`
    // is invisible, `mini-base` fails on a re-exposed module, and every one of
    // the 2,922 assertions reports the same false error.
    settings.target_device = "mock".to_owned();

    let mut args = options.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        // `--flag=value` and `--flag value` are both accepted by cmdargs, and
        // the corpus uses both spellings of `--base-library`.
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f, Some(v)),
            None => (arg, None),
        };
        let mut value = || inline.map(str::to_owned).or_else(|| args.next().map(str::to_owned));

        match flag {
            "--import-dir" => {
                if let Some(v) = value() {
                    settings.import_dirs.push(join_root(&root, &v));
                }
            }
            // A path, not a module name: it may be absolute (the default) or
            // relative to the import search (`custom-base`'s
            // `helper/custom_base.k`), which `resolve_import_file` handles.
            "--base-library" => settings.base_library = value().map(camino::Utf8PathBuf::from),
            "--target-device" => {
                if let Some(v) = value() {
                    settings.target_device = v;
                }
            }
            "--no-implicit-base" => settings.implicit_base = inline.is_some_and(is_false_flag),
            "--parse-docs" => settings.parse_docs = true,
            // The five backend options the `.options` module spells
            // (`Parser/IO.hs:57-65`); `cmdargs` accepts a prefix of a flag,
            // and `--backend` is also `--target`.
            "--backend" | "--target" => {
                if let Some(v) = value() {
                    settings.backend.backend = if v.eq_ignore_ascii_case("sv") { "Sv".into() } else { "None".into() };
                }
            }
            "--stall" => settings.backend.stall = value().and_then(|v| v.parse().ok()).unwrap_or(0),
            "-O" | "--optimize" => settings.backend.optimize = value().and_then(|v| v.parse().ok()).unwrap_or(2),
            "--max-threads-default" => {
                settings.backend.max_threads_default = value().and_then(|v| v.parse().ok()).unwrap_or(512);
            }
            "--max-threads-limit" => {
                settings.backend.max_threads_limit = value().and_then(|v| v.parse().ok()).unwrap_or(512);
            }
            // `--define=NAME#VALUE` (`Parser/IO.hs:51-58`).
            "--define" => {
                if let Some((name, v)) = value().as_deref().and_then(|v| v.split_once('#')) {
                    settings.defines.push((name.to_owned(), v.to_owned()));
                }
            }
            "--using" => {
                if let Some((name, v)) = value().as_deref().and_then(|v| v.split_once('#')) {
                    settings.using.push((name.to_owned(), v.to_owned()));
                }
            }
            // Order matters and is not alphabetical: `--Werror` and `--Wall`
            // must be matched before the generic `--W<name>` arm, or `--Werror`
            // enables a warning called `error`.
            "--Werror" => settings.warnings_as_errors = true,
            "--Wall" => settings.warnings = kg_diag::WarningSet::all(),
            _ => apply_warning_switch(&mut settings.warnings, flag),
        }
    }
    settings
}

/// cmdargs' boolean spelling: `--flag=false` turns it off, anything else on.
/// Only `--no-implicit-base=false` appears in the corpus, and it is a *no-op* —
/// the default of `no_implicit_base` is already `def`, i.e. `False`
/// (`Options/CmdArgs.hs:65`). Reproduced anyway so the flag is not silently
/// inverted the day a test writes `=true`.
fn is_false_flag(v: &str) -> bool {
    v.eq_ignore_ascii_case("false") || v == "0"
}

/// `--W<name>` enables, `--Wno-<name>` disables (`Warning.hs`, and the
/// `enableSwitch`/`disableSwitch` pair at `Options/CmdArgs.hs:123-130`).
/// An unknown `--W…` is left alone rather than guessed at.
fn apply_warning_switch(set: &mut kg_diag::WarningSet, flag: &str) {
    let Some(rest) = flag.strip_prefix("--W") else { return };
    let (enable, name) = match rest.strip_prefix("no-") {
        Some(name) => (false, name),
        None => (true, rest),
    };
    for kind in kg_diag::WarningKind::ALL {
        if kind.name() == name {
            if enable { set.enable(kind) } else { set.disable(kind) }
            return;
        }
    }
}

/// Resolves a command-line directory against the corpus root. CMake expands its
/// `${…}` to absolute paths, so this is a no-op today; it exists so a relative
/// `--import-dir` appearing upstream does not silently resolve against this
/// process's working directory.
fn join_root(root: &camino::Utf8Path, path: &str) -> camino::Utf8PathBuf {
    let path = camino::Utf8Path::new(path);
    if path.is_absolute() { path.to_owned() } else { root.join(path) }
}

/// Where the harness pretends the snippet lives.
///
/// `syntax-test.py:40-44` creates a subfolder named after the test file, writes
/// `syntax_test_temp_file.k` into it and runs with `cwd` set there. Two details
/// decide the answer, and both come from `add_test`'s
/// `WORKING_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR}` (`CMakeLists.txt:71`):
///
/// * the directory is in the **build** tree, not the source tree, so it holds
///   nothing but the temp file. That emptiness is the point — `Parser/IO.hs:107`
///   searches the importing file's own directory *first*, so a snippet dir
///   containing fixtures would resolve imports the reference cannot see. Using
///   the source-tree `test/syntax/imports/` would do exactly that to all 19
///   `a1.*` imports;
/// * nothing is ever created on disk. The path is interned and given an
///   overlay; the [`kg_vfs::Vfs`] answers from memory and the corpus stays
///   read-only.
pub(crate) fn snippet_path(kanagawa_root: &Path, a: &Assertion) -> camino::Utf8PathBuf {
    let base = a.file.file_stem().unwrap_or_default().to_string_lossy();
    let root = camino::Utf8PathBuf::from_path_buf(
        kanagawa_root.canonicalize().unwrap_or_else(|_| kanagawa_root.to_path_buf()),
    )
    .expect("the reference path is UTF-8");
    root.join("build/test/syntax").join(base.as_ref()).join("syntax_test_temp_file.k")
}

/// True once the frontend implements name resolution and type checking.
///
/// Kept as a named constant rather than a feature flag so the scoreboard never
/// silently overstates itself: while this is `false`, an `expected:1` snippet
/// that parses cleanly is reported as *undecidable*, not as a pass.
///
/// # Why this is `true`
///
/// Flipped 2026-10-10 by the owner's decision (`docs/plans/sprint-parity.md`
/// §8), on the condition this comment set when it was `false`: `--strict`,
/// which scores exactly as if it were `true`, fails nothing — 0 of 2,681
/// decidable assertions since 2026-09-28. So an `expected:1` snippet that
/// compiles cleanly, and a warning assertion the frontend does not raise, are
/// failures now, not undecidable. What stays undecidable is only what the
/// frontend must *not* decide: the 241 assertions that expect a C++ backend
/// code, which `cargo xtask backend` grades end to end. The history of the
/// `false` era is `docs/journal.md`, "Why `FRONTEND_COMPLETE` is still
/// `false`".
pub(crate) const FRONTEND_COMPLETE: bool = true;

pub fn run(
    kanagawa_root: &Path,
    filter: Option<&str>,
    list: bool,
    strict: bool,
    max_failures: Option<usize>,
    jobs: Option<usize>,
) -> Result<()> {
    if !kanagawa_root.join("test/syntax").is_dir() {
        anyhow::bail!(
            "no reference corpus at {} — pass --kanagawa-root",
            kanagawa_root.display()
        );
    }

    let mut assertions = load_all(kanagawa_root)?;
    if let Some(filter) = filter {
        assertions.retain(|a| a.name.contains(filter));
    }

    print!("{}", summarize(&assertions));
    if list {
        for a in &assertions {
            println!(
                "{}  {}:{}  {:?}",
                a.name,
                a.file.file_name().unwrap_or_default().to_string_lossy(),
                a.marker_line,
                a.expectation
            );
        }
        return Ok(());
    }

    let complete = strict || FRONTEND_COMPLETE;
    let jobs = jobs
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8))
        .max(1);
    let mut board = Scoreboard::default();
    for (a, outcome) in assertions.iter().zip(check_all(kanagawa_root, &assertions, complete, jobs)) {
        board.record(&a.name, outcome);
    }

    println!();
    print!("{}", board.report());

    match max_failures {
        // A ratchet, not a target. Below the budget is a pass *and* a nudge: the
        // number in CI is now stale in the good direction and wants lowering, so
        // say so rather than letting the slack quietly accumulate.
        Some(budget) if board.failed <= budget => {
            if board.failed < budget {
                println!(
                    "\n{} failures, under the budget of {budget} — lower it.",
                    board.failed
                );
            }
            Ok(())
        }
        Some(budget) => anyhow::bail!(
            "{} conformance assertions failed, over the budget of {budget}",
            board.failed
        ),
        None if board.failed > 0 => {
            anyhow::bail!("{} conformance assertions failed", board.failed)
        }
        None => Ok(()),
    }
}
