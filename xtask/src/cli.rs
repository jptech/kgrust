//! `cargo xtask cli` — upstream's own command-line tests, and every mode's
//! `--help`, against the compat `kanagawa` binary (sprint W8.6).
//!
//! # Upstream's tests, as its `ctest` would run them
//!
//! `test/compiler/cli/CMakeLists.txt` registers its tests through two CMake
//! functions (`add_cli_test`, `add_golden_test`), and CMake writes the
//! commands they resolve to into the build tree's `CTestTestfile.cmake`.
//! This reads that file — every `add_test`, every fixture — and runs each
//! command as `ctest` would, with two substitutions, which are what pointing
//! `kanagawa::exe` at this binary would do at cutover: the reference's
//! compiler becomes the binary under test, and the build tree's test
//! directory becomes a scratch one, so nothing is written into the
//! reference's build. A test upstream adds is graded here the day its build
//! is reconfigured; a command this driver does not know is a failure, never
//! a skip.
//!
//! The golden a golden test compares against is the build's configured copy
//! of its template; it is configured here again from the reference's source
//! (`add_golden_test`, `build/cmake/test_helper.cmake:502-527`) — the
//! template's `@KANAGAWA_SOURCE_DIR@` is the reference checkout's real path —
//! so the file graded against is the checked-in one, not a build's.
//!
//! `cmake -E rm`, `make_directory` and `compare_files` are done here; `python3`
//! runs upstream's checking scripts, and its absence fails the gate, as the
//! build's does (sprint §2: an absent instrument is red).
//!
//! # Help and usage
//!
//! Every mode's `--help`, in the formats that change its text, and every
//! mode with no file: ours against the reference binary's, exit code, stdout
//! and stderr. The version in the summary line (`Kanagawa Compiler <v>,
//! (C) Microsoft`) is masked: the reference's build stamps its release, 1.2.0,
//! and this binary is not that release (`kg_cli::legacy::help::NUMERIC_VERSION`).
//! `pretty`'s help is graded with the rest: the mode is refused when run, by
//! decision, and its flags are still parsed, so its help still describes
//! this binary.
//!
//! # What it is blind to
//!
//! `cmdargs`' argv errors (`Unknown flag: …` and the rest), which this
//! binary words differently on stderr (`docs/known-divergences.md` #34);
//! `--version`, whose second line says what this binary is; and the
//! diagnostics' own rendering, which `errors` and `backend` grade.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// One `add_test`, its fixtures and working directory.
#[derive(Clone, Debug, Default, PartialEq)]
struct CTest {
    name: String,
    command: Vec<String>,
    setup: Vec<String>,
    required: Vec<String>,
    cwd: Option<String>,
}

/// The `[=[…]=]`-bracketed and `"…"`-quoted arguments of one CMake command.
fn cmake_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut arg = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(escaped) = chars.next() {
                                arg.push(escaped);
                            }
                        }
                        c => arg.push(c),
                    }
                }
                out.push(arg);
            }
            '[' if chars.peek() == Some(&'=') => {
                chars.next();
                let _ = chars.next(); // the second `[`
                let mut arg = String::new();
                while let Some(c) = chars.next() {
                    if c == ']' && chars.peek() == Some(&'=') {
                        chars.next();
                        let _ = chars.next();
                        break;
                    }
                    arg.push(c);
                }
                out.push(arg);
            }
            c if c.is_whitespace() => {}
            c => {
                let mut arg = String::from(c);
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    arg.push(c);
                    chars.next();
                }
                out.push(arg);
            }
        }
    }
    out
}

/// Every test of a `CTestTestfile.cmake`, in its order.
fn ctests(text: &str) -> Result<Vec<CTest>> {
    let mut tests: Vec<CTest> = Vec::new();
    for line in text.lines() {
        if let Some(body) = line.strip_prefix("add_test(").and_then(|b| b.strip_suffix(')')) {
            let mut args = cmake_args(body).into_iter();
            let name = args.next().context("an add_test with no name")?;
            tests.push(CTest { name, command: args.collect(), ..CTest::default() });
        } else if let Some(body) = line.strip_prefix("set_tests_properties(").and_then(|b| b.strip_suffix(')')) {
            let args = cmake_args(body);
            let name = args.first().context("set_tests_properties with no test")?;
            let test = tests
                .iter_mut()
                .find(|t| &t.name == name)
                .with_context(|| format!("properties for `{name}`, which no add_test before it declares"))?;
            let mut pairs = args[1..].iter().skip_while(|a| *a == "PROPERTIES");
            while let (Some(key), Some(value)) = (pairs.next(), pairs.next()) {
                let list = || value.split(';').filter(|v| !v.is_empty()).map(str::to_owned).collect::<Vec<_>>();
                match key.as_str() {
                    "FIXTURES_SETUP" => test.setup = list(),
                    "FIXTURES_REQUIRED" => test.required = list(),
                    "WORKING_DIRECTORY" => test.cwd = Some(value.clone()),
                    _ => {}
                }
            }
        }
    }
    Ok(tests)
}

/// `add_golden_test(<name> … GOLDEN <template> …)` calls of a CMakeLists:
/// each golden test's configured file name and its template.
fn golden_templates(cmakelists: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = cmakelists;
    while let Some(at) = rest.find("add_golden_test(") {
        let body = &rest[at + "add_golden_test(".len()..];
        let end = body.find(')').unwrap_or(body.len());
        let words: Vec<&str> = body[..end].split_whitespace().collect();
        if let (Some(name), Some(i)) = (words.first(), words.iter().position(|w| *w == "GOLDEN"))
            && let Some(template) = words.get(i + 1)
        {
            out.push((format!("{name}.golden"), (*template).to_owned()));
        }
        rest = &body[end..];
    }
    out
}

/// `string(CONFIGURE … @ONLY)` of a template whose one variable is
/// `KANAGAWA_SOURCE_DIR`; any other `@VAR@` is an error, not an empty string.
fn configure(template: &str, source_dir: &str) -> Result<String> {
    let text = template.replace("@KANAGAWA_SOURCE_DIR@", source_dir);
    let mut rest = text.as_str();
    while let Some(at) = rest.find('@') {
        let after = &rest[at + 1..];
        if let Some(end) = after.find('@')
            && !after[..end].is_empty()
            && after[..end].chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            anyhow::bail!("golden template names @{}@, which this driver does not configure", &after[..end]);
        }
        rest = after;
    }
    Ok(text)
}

/// `cmake -E compare_files [--ignore-eol] a b`.
fn compare_files(args: &[String]) -> Result<bool> {
    let (ignore_eol, files) = match args {
        [flag, a, b] if flag == "--ignore-eol" => (true, [a, b]),
        [a, b] => (false, [a, b]),
        _ => anyhow::bail!("compare_files {args:?}"),
    };
    let read = |p: &String| std::fs::read(p).with_context(|| format!("reading {p}"));
    let (a, b) = (read(files[0])?, read(files[1])?);
    if !ignore_eol {
        return Ok(a == b);
    }
    // `cmSystemTools::TextFilesDiffer`: line by line, each line's one
    // trailing CR dropped, a missing last newline no difference.
    let lines = |bytes: &[u8]| -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = bytes.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
        if out.last().is_some_and(Vec::is_empty) {
            out.pop();
        }
        for line in &mut out {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        out
    };
    Ok(lines(&a) == lines(&b))
}

/// What running one command came to.
enum Ran {
    Pass,
    Fail(String),
}

/// The binaries and directories one run grades with.
struct Setup<'a> {
    /// The reference compiler, as the `CTestTestfile` names it.
    reference: &'a Path,
    /// What runs in its place.
    binary: &'a Path,
    backend_dir: &'a Path,
    /// The build tree's test directory, and the scratch one that stands in.
    build_tests: String,
    work: &'a Path,
}

impl Setup<'_> {
    fn redirect(&self, arg: &str) -> String {
        arg.replace(&self.build_tests, self.work.to_str().expect("UTF-8 work dir"))
    }

    fn run(&self, test: &CTest) -> Result<Ran> {
        let args: Vec<String> = test.command.iter().map(|a| self.redirect(a)).collect();
        let Some((program, rest)) = args.split_first() else { anyhow::bail!("`{}` has no command", test.name) };
        let cwd = test.cwd.as_deref().map_or_else(|| self.work.to_path_buf(), |c| PathBuf::from(self.redirect(c)));
        if Path::new(program).file_name().is_some_and(|n| n == "cmake") {
            return cmake_e(rest, self.work);
        }
        let mut command = if Path::new(program) == self.reference {
            let mut c = Command::new(self.binary);
            c.env("KG_BACKEND_DIR", self.backend_dir);
            c
        } else if program == "python3" {
            Command::new("python3")
        } else {
            return Ok(Ran::Fail(format!("a command this driver does not know: {program}")));
        };
        command.args(rest).current_dir(&cwd);
        let output = match crate::bounded::run(command, crate::bounded::Limits::compile())
            .with_context(|| format!("running {}", test.name))?
        {
            Ok(output) => output,
            Err((over, _)) => return Ok(Ran::Fail(over.to_string())),
        };
        Ok(if output.status.success() {
            Ran::Pass
        } else {
            Ran::Fail(format!(
                "exit {:?}\n{}{}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ))
        })
    }
}

/// The `cmake -E` commands the CLI tests use. A path they would remove or
/// make outside the scratch directory is refused: one the redirection
/// missed points into the reference's build, or its checkout.
fn cmake_e(args: &[String], work: &Path) -> Result<Ran> {
    let outside = |path: &str| !Path::new(path).starts_with(work);
    Ok(match args {
        [e, verb, .., path] if e == "-E" && (verb == "rm" || verb == "make_directory") && outside(path) => {
            Ran::Fail(format!("cmake -E {verb} {path}: outside the scratch directory, refused"))
        }
        [e, rm, flag, path] if e == "-E" && rm == "rm" && flag == "-rf" => {
            match std::fs::remove_dir_all(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Ok(Ran::Fail(format!("rm -rf {path}: {e}"))),
            }
            Ran::Pass
        }
        [e, mk, path] if e == "-E" && mk == "make_directory" => match std::fs::create_dir_all(path) {
            Ok(()) => Ran::Pass,
            Err(e) => Ran::Fail(format!("make_directory {path}: {e}")),
        },
        [e, cmp, rest @ ..] if e == "-E" && cmp == "compare_files" => {
            if compare_files(rest)? {
                Ran::Pass
            } else {
                Ran::Fail(format!("{} and {} differ", rest[rest.len() - 2], rest[rest.len() - 1]))
            }
        }
        _ => Ran::Fail(format!("a cmake command this driver does not know: {args:?}")),
    })
}

/// The command lines whose help and usage are compared, by the argv after
/// the program name: every mode's `--help`, the formats that change its
/// text, the program's help reached by a file argument and by a mode
/// prefix, and each mode with no file.
const HELP_CASES: &[&[&str]] = &[
    &[],
    &["--help"],
    &["--help=all"],
    &["--help=one"],
    &["--help=60"],
    &["--help=html"],
    &["--help=all,html"],
    &["-?"],
    &["a.k", "--help"],
    &["compile", "--help"],
    &["pretty", "--help"],
    &["list-deps", "--help"],
    &["list-deps", "--help=all"],
    &["l", "--help"],
    &["compile"],
    &["pretty"],
    &["list-deps"],
];

/// The summary line's version, masked: `Kanagawa Compiler <v>, (C) Microsoft`.
fn mask_version(text: &str) -> String {
    const HEAD: &str = "Kanagawa Compiler ";
    const TAIL: &str = ", (C) Microsoft";
    match text.find(HEAD) {
        Some(at) => match text[at..].find(TAIL) {
            Some(len) if !text[at..at + len].contains('\n') => {
                format!("{}{HEAD}<version>{}", &text[..at], &text[at + len..])
            }
            _ => text.to_owned(),
        },
        None => text.to_owned(),
    }
}

/// One command line through both binaries: `None` when they agree.
fn help_case(ours: &Path, theirs: &Path, args: &[&str], cwd: &Path) -> Result<Option<String>> {
    let run = |binary: &Path| -> Result<(Option<i32>, String, String)> {
        let mut command = Command::new(binary);
        command.args(args).current_dir(cwd);
        let output = match crate::bounded::run(command, crate::bounded::Limits::compile())? {
            Ok(output) => output,
            Err((over, _)) => anyhow::bail!("{}: {over}", binary.display()),
        };
        Ok((
            output.status.code(),
            mask_version(&String::from_utf8_lossy(&output.stdout)),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    };
    let (a, b) = (run(ours)?, run(theirs)?);
    if a == b {
        return Ok(None);
    }
    let first = |x: &str, y: &str| {
        let line = x.lines().zip(y.lines()).position(|(p, q)| p != q).unwrap_or(x.lines().count().min(y.lines().count()));
        format!(
            "line {}: ours `{}` / reference `{}`",
            line + 1,
            x.lines().nth(line).unwrap_or("<end>"),
            y.lines().nth(line).unwrap_or("<end>")
        )
    };
    Ok(Some(if a.0 != b.0 {
        format!("exit {:?} / reference {:?}", a.0, b.0)
    } else if a.1 != b.1 {
        format!("stdout {}", first(&a.1, &b.1))
    } else {
        format!("stderr {}", first(&a.2, &b.2))
    }))
}

pub fn run(kanagawa_root: &Path, build_dir: &Path, binary: Option<&Path>, verbose: bool) -> Result<()> {
    let root = kanagawa_root.canonicalize().with_context(|| format!("resolving {}", kanagawa_root.display()))?;
    let build = build_dir.canonicalize().with_context(|| {
        format!("no reference build at {} — upstream's CLI tests are configured there; pass --build-dir", build_dir.display())
    })?;
    let reference = build.join("dist/bin/kanagawa");
    if !reference.is_file() {
        anyhow::bail!("no reference compiler at {} — the help is graded against it", reference.display());
    }
    let backend_dir = build.join("dist/bin");
    let source_tests = root.join("test/compiler/cli");
    let build_tests = build.join("test/compiler/cli");
    let testfile = build_tests.join("CTestTestfile.cmake");
    let text = std::fs::read_to_string(&testfile)
        .with_context(|| format!("reading {} — is the reference build configured with its tests?", testfile.display()))?;
    // Graded against this checkout's tests, or not at all.
    let configured_from = text
        .lines()
        .find_map(|l| l.strip_prefix("# Source directory: "))
        .context("CTestTestfile.cmake names no source directory")?;
    if Path::new(configured_from) != source_tests {
        anyhow::bail!(
            "{} was configured from {configured_from}, not {} — reconfigure the build from this checkout",
            testfile.display(),
            source_tests.display()
        );
    }
    let tests = ctests(&text)?;
    if tests.is_empty() {
        anyhow::bail!("{} registers no test", testfile.display());
    }
    // The upstream checks are Python; a gate that skipped them would pass
    // with them absent.
    if Command::new("python3").arg("--version").output().is_err() {
        anyhow::bail!("python3 is required to run upstream's CLI checks");
    }
    let external = binary.is_some();
    let binary = match binary {
        None => crate::fresh::compat_binary()?,
        Some(b) => {
            if !b.is_file() {
                anyhow::bail!("no binary at {}", b.display());
            }
            crate::fresh::announce_external(b);
            b.canonicalize()?
        }
    };

    let work = std::env::temp_dir().join(format!("kg-cli-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;
    let work = work.canonicalize()?;
    let cmakelists = std::fs::read_to_string(source_tests.join("CMakeLists.txt"))?;
    for (golden, template) in golden_templates(&cmakelists) {
        let text = std::fs::read_to_string(source_tests.join(&template))
            .with_context(|| format!("reading the golden template {template}"))?;
        std::fs::write(work.join(golden), configure(&text, root.to_str().expect("UTF-8 reference path"))?)?;
    }

    let setup = Setup {
        reference: &reference,
        binary: &binary,
        backend_dir: &backend_dir,
        build_tests: build_tests.to_str().expect("UTF-8 build path").to_owned(),
        work: &work,
    };
    let mut graded: Vec<(String, Option<String>)> = Vec::new();
    // `ctest` runs a fixture's setup before the tests that require it, and
    // does not run those whose setup failed.
    let mut fixtures: BTreeMap<String, bool> = BTreeMap::new();
    for test in &tests {
        let failed_setup = test.required.iter().find(|f| fixtures.get(*f) != Some(&true));
        let verdict = match failed_setup {
            Some(f) if fixtures.contains_key(f) => Some(format!("not run: fixture `{f}` failed")),
            Some(f) => Some(format!("not run: fixture `{f}` has no setup before it")),
            None => match setup.run(test)? {
                Ran::Pass => None,
                Ran::Fail(why) => Some(why),
            },
        };
        for f in &test.setup {
            let ok = verdict.is_none() && fixtures.get(f) != Some(&false);
            fixtures.insert(f.clone(), ok);
        }
        // The `cmake -E` preparation steps are the harness's, not tests of
        // the compiler; graded all the same, since a failed one fails the
        // tests that need it.
        graded.push((test.name.clone(), verdict));
    }
    for args in HELP_CASES {
        let name = format!("help: kanagawa {}", args.join(" ")).trim_end().to_owned();
        graded.push((name, help_case(&binary, &reference, args, &work)?));
    }
    let _ = std::fs::remove_dir_all(&work);

    let mut ledger = crate::ratchet::Ledger::new("cli");
    ledger.set("pass");
    let mut failed = 0usize;
    for (name, verdict) in &graded {
        match verdict {
            None => {
                ledger.add("pass", name);
                println!("PASS {name}");
            }
            Some(why) => {
                failed += 1;
                let shown = if verbose { why.as_str() } else { why.lines().next().unwrap_or("") };
                println!("fail {name} — {shown}");
            }
        }
    }
    println!();
    println!("cli: {}/{} pass against {}", graded.len() - failed, graded.len(), binary.display());
    ledger.total("graded", graded.len());
    ledger.total("passed", graded.len() - failed);
    ledger.total("failed", failed);
    // A ledger is stamped with this tree's fingerprint, so one graded from
    // another binary would be a false one.
    if external {
        println!("ratchet cli: no ledger — --binary graded another binary than this tree's");
    } else if crate::ratchet::enforce(ledger)? {
        return Ok(());
    }
    if failed > 0 {
        anyhow::bail!("{failed} CLI tests failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TESTFILE: &str = r#"# Source directory: /src/test/compiler/cli
add_test([=[cli.a.run]=] "/build/dist/bin/kanagawa" "list-deps" "--file-list=/build/test/compiler/cli/a.txt" "/src/a.k")
set_tests_properties([=[cli.a.run]=] PROPERTIES  FIXTURES_SETUP "cli.a" _BACKTRACE_TRIPLES "x;1;y")
add_test([=[cli.a.golden]=] "/opt/bin/cmake" "-E" "compare_files" "--ignore-eol" "/build/test/compiler/cli/a.txt" "/build/test/compiler/cli/cli.a.golden")
set_tests_properties([=[cli.a.golden]=] PROPERTIES  FIXTURES_REQUIRED "cli.a" _BACKTRACE_TRIPLES "x;2;y")
"#;

    #[test]
    fn a_ctest_file_reads_as_its_tests_and_fixtures() {
        let tests = ctests(TESTFILE).unwrap();
        assert_eq!(tests.len(), 2);
        assert_eq!(tests[0].name, "cli.a.run");
        assert_eq!(tests[0].command[1], "list-deps");
        assert_eq!(tests[0].setup, ["cli.a"]);
        assert_eq!(tests[1].required, ["cli.a"]);
        assert_eq!(tests[1].command[..4], ["/opt/bin/cmake", "-E", "compare_files", "--ignore-eol"]);
    }

    #[test]
    fn a_golden_is_configured_from_its_template_and_nothing_else() {
        let cmake = "add_golden_test(cli.list_deps\n  COMMAND x list-deps\n  ACTUAL y\n  GOLDEN list_deps.golden.in\n)\n";
        assert_eq!(golden_templates(cmake), [("cli.list_deps.golden".to_owned(), "list_deps.golden.in".to_owned())]);
        assert_eq!(configure("@KANAGAWA_SOURCE_DIR@/a.k\n", "/src").unwrap(), "/src/a.k\n");
        assert!(configure("@OTHER_DIR@/a.k\n", "/src").is_err());
        assert_eq!(configure("a@b.c and @ alone\n", "/src").unwrap(), "a@b.c and @ alone\n");
    }

    /// `--ignore-eol` forgives a CR ending a line and a missing last
    /// newline, and nothing else — a CR inside a line is a difference.
    #[test]
    fn compare_files_ignores_only_line_endings_when_asked() {
        let dir = std::env::temp_dir().join(format!("kg-cli-cmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = |p: &Path| p.to_str().unwrap().to_owned();
        let compare = |x: &str, y: &str, ignore_eol: bool| {
            let (a, b) = (dir.join("a"), dir.join("b"));
            std::fs::write(&a, x).unwrap();
            std::fs::write(&b, y).unwrap();
            let mut args = if ignore_eol { vec!["--ignore-eol".to_owned()] } else { Vec::new() };
            args.extend([s(&a), s(&b)]);
            compare_files(&args).unwrap()
        };
        let results = [
            compare("x\r\ny\n", "x\ny\n", true),
            compare("x\r\ny\n", "x\ny\n", false),
            compare("x\ny", "x\ny\n", true),
            compare("x\ry\n", "xy\n", true),
        ];
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(results, [true, false, true, false]);
    }

    #[test]
    fn only_the_version_is_masked() {
        assert_eq!(
            mask_version("Kanagawa Compiler 1.2.0 (local), (C) Microsoft\n\nkanagawa\n"),
            "Kanagawa Compiler <version>, (C) Microsoft\n\nkanagawa\n"
        );
        assert_eq!(
            mask_version("<tr><td colspan='3'>Kanagawa Compiler 0.0.0 (local), (C) Microsoft</td></tr>"),
            "<tr><td colspan='3'>Kanagawa Compiler <version>, (C) Microsoft</td></tr>"
        );
        assert_eq!(mask_version("Missing source filename(s)\n"), "Missing source filename(s)\n");
    }
}
