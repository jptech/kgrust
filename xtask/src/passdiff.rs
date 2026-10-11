//! `cargo xtask passdiff --pass <name> [--oracle]`: a CST pass against the
//! loop's version of it on each module's own text (`kg_db::passdiff`), over
//! every corpus root and every conformance snippet — the instrument each
//! step of sprint W3 step 1 lands behind: a pass's CST version stops running
//! only once this reads it identical to the loop's, or every difference is
//! settled by the reference in the loop's favour.
//!
//! Roots are the library and test modules outside `test/syntax` under the
//! settings the other corpus walks use (`bench::rooted_settings`); snippets
//! are the conformance harness's, each under its own command line
//! (`conformance::settings_for`). A `Db` per thread, results in indexed
//! slots, the report in corpus order. Red — exit non-zero — on any module
//! where the two sides differ, unless `--oracle` puts each difference to
//! `kg-oracle --errors` (cached, as the `errors` gate asks it) and the
//! reference gives every verdict only the loop gives and none only the CST
//! pass gives — position, message and each `while compiling` note's
//! position, in order. The reference is asked of a snippet only; a root
//! that differs stays red. Two verdicts alike in all the reference prints
//! and apart only beyond it — a range's end, a fix — are booked: listed on
//! every run, and no difference, since nothing can settle them. What it
//! cannot see: a module whose run stops before the pass, where neither
//! side is shown.
//!
//! The parser's own checks are probed the same way (`kg_db::passdiff::
//! Probe::Parser`, sprint W3.7): the CST engine's table against what the
//! parser built, as the oracle for one check. The reference's verdicts on
//! those are its parser's, a `PARSE-ERROR`, which is then the bundle list
//! read — and a snippet the reference's parser accepts has none.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::conformance::Assertion;
use crate::errors::Reported;
use kg_db::passdiff::Probe;

/// A verdict as the reference prints one: its position and message, as the
/// `errors` gate reads them, and each `while compiling` note's position.
#[derive(PartialEq, Eq, Debug)]
struct Bundle {
    reported: Reported,
    notes: Vec<(String, u32, u32)>,
}

/// One verdict only one side gives, read as the reference's are, and as
/// printed.
struct Apart {
    bundle: Bundle,
    shown: String,
    /// A warning, which the reference prints only under `--Werror` and
    /// only where it raises no error (`app/Main.hs:93-96`).
    warning: bool,
}

/// One program's reading: how many module runs reached the pass, and each
/// verdict only one side gives.
#[derive(Default)]
struct Reading {
    reached: usize,
    /// Every verdict the CST side gave, shared or not.
    given: usize,
    cst_only: Vec<Apart>,
    ir_only: Vec<Apart>,
    /// A CST verdict and a loop verdict alike in everything the reference
    /// prints, apart beyond it: each side as printed whole.
    unprinted: Vec<(String, String)>,
}

pub fn run(kanagawa_root: &Path, pass: &str, filter: Option<&str>, verbose: bool, oracle: bool) -> Result<()> {
    let Some(pass) = Probe::all().into_iter().find(|p| p.name().eq_ignore_ascii_case(pass)) else {
        let names: Vec<&str> = Probe::all().iter().map(|p| p.name()).collect();
        anyhow::bail!("passdiff: no pass `{pass}` — one of {}", names.join(", "));
    };
    // Absent is red (sprint §2): a pass with no CST side left reads
    // identical everywhere, having compared nothing.
    if !kg_db::passdiff::PROBED.contains(&pass) {
        anyhow::bail!("passdiff: {} has no CST version left to compare — the loop's alone since its move", pass.name());
    }
    let oracle = match oracle {
        // Absent is red (sprint §2): nothing settled reads as nothing to settle.
        true => Some(crate::ir::find_oracle().ok_or_else(|| anyhow::anyhow!("passdiff --oracle: kg-oracle is not built — run oracle/build.sh"))?),
        false => None,
    };
    let kanagawa_root = &kanagawa_root.canonicalize()?;
    let started = std::time::Instant::now();

    let mut files = Vec::new();
    for dir in ["library", "test"] {
        crate::corpus::collect(&kanagawa_root.join(dir), &mut files);
    }
    files.retain(|f| !f.components().any(|c| c.as_os_str() == "syntax"));
    files.sort();
    let mut snippets = crate::conformance::load_all(kanagawa_root)?;
    if let Some(f) = filter {
        files.retain(|p| p.to_string_lossy().contains(f));
        snippets.retain(|a| a.name.contains(f));
    }
    if files.is_empty() && snippets.is_empty() {
        anyhow::bail!("passdiff: nothing to compare (filter {filter:?})");
    }

    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let roots = fan_out(workers, 1, &files, || kg_db::Db::new(crate::bench::rooted_settings(kanagawa_root)), |db, path| {
        let text = std::fs::read_to_string(path).ok()?;
        let utf8 = camino::Utf8PathBuf::from_path_buf(path.clone()).ok()?;
        let root = db.set_file_text(&utf8, text);
        Some(read(db, &[root], pass, kanagawa_root))
    });
    // Consecutive chunks, as `conformance` hands them out: a snippet's
    // command line is its file's, and a change of settings starts the
    // thread's memos over.
    let chunk = snippets.len().div_ceil(workers * 4).max(1);
    let init = || (kg_db::Db::new(crate::conformance::settings_for(kanagawa_root, &[])), None::<Arc<[String]>>);
    let snippets_read = fan_out(workers, chunk, &snippets, init, |(db, options), a| {
        if options.as_deref() != Some(&*a.options) {
            db.set_settings(crate::conformance::settings_for(kanagawa_root, &a.options));
            *options = Some(Arc::clone(&a.options));
        }
        let file = db.set_file_text(&crate::conformance::snippet_path(kanagawa_root, a), a.source.clone());
        Some(read(db, &[file], pass, kanagawa_root))
    });

    let named = files
        .iter()
        .map(|p| (format!("file::{}", p.strip_prefix(kanagawa_root).unwrap_or(p).display()), None))
        .zip(roots)
        .chain(snippets.iter().enumerate().map(|(i, a)| (a.name.clone(), Some((i, a)))).zip(snippets_read));
    let (mut programs, mut reached, mut modules, mut differ) = (0usize, 0usize, 0usize, Vec::new());
    let (mut given, mut cst_only, mut ir_only) = (0usize, 0usize, 0usize);
    let mut booked: Vec<(String, (String, String))> = Vec::new();
    for ((name, snippet), reading) in named {
        let Some(r) = reading else { continue };
        programs += 1;
        if r.reached == 0 {
            continue;
        }
        reached += 1;
        modules += r.reached;
        given += r.given;
        cst_only += r.cst_only.len();
        ir_only += r.ir_only.len();
        booked.extend(r.unprinted.iter().map(|pair| (name.clone(), pair.clone())));
        if !r.cst_only.is_empty() || !r.ir_only.is_empty() {
            differ.push((name, snippet, r));
        }
    }
    println!(
        "passdiff {}: {} roots, {} snippets; the pass reached in {modules} module runs of {reached} of {programs} programs, \
         the CST pass giving {given} verdicts — {} programs identical, {} differ (cst-only {cst_only}, ir-only {ir_only}); \
         {} verdicts apart only beyond what the reference prints  [{:.1}s]",
        pass.name(),
        files.len(),
        snippets.len(),
        reached - differ.len(),
        differ.len(),
        booked.len(),
        started.elapsed().as_secs_f64()
    );
    for (name, (cst, ir)) in &booked {
        println!("  BOOKED {name}: cst {cst} / ir {ir}");
    }

    // Each difference put to the reference: `Some(true)` settled for the
    // loop, `Some(false)` against it, `None` not asked.
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask in the workspace").join("target/xtask-passdiff");
    let settled: Vec<Option<bool>> = differ
        .iter()
        .map(|(_, snippet, r)| {
            let (Some(oracle), Some((i, a))) = (&oracle, snippet) else { return None };
            Some(settled_for_the_loop(oracle, kanagawa_root, &scratch, *i, a, r, pass.parse_time()))
        })
        .collect();
    let unsettled = settled.iter().filter(|s| **s != Some(true)).count();
    if oracle.is_some() {
        println!("  the reference settles {} for the loop; {unsettled} unsettled", differ.len() - unsettled);
    }
    let mut shown = 0usize;
    for ((name, _, r), s) in differ.iter().zip(&settled) {
        if !verbose && (shown >= 20 || *s == Some(true)) {
            continue;
        }
        shown += 1;
        let tag = match s {
            Some(true) => " — the reference's: the loop's",
            Some(false) => " — the reference's: the CST pass's, or neither",
            None => "",
        };
        println!("  DIFF {name}{tag}");
        for d in &r.cst_only {
            println!("    cst only: {}", d.shown);
        }
        for d in &r.ir_only {
            println!("    ir only:  {}", d.shown);
        }
    }
    if unsettled > shown {
        println!("  ... and {} more (--verbose lists them)", unsettled - shown);
    }
    if unsettled > 0 {
        anyhow::bail!("passdiff {}: {unsettled} programs differ and are not settled for the loop", pass.name());
    }
    Ok(())
}

/// Whether the reference gives every verdict only the loop gives, and none
/// of those only the CST pass gives — each compared whole: position,
/// message, and its notes' positions in order.
///
/// A warning apart is asked of the reference a second time, every kind
/// enabled after the snippet's own switches and `--Werror` set: its
/// warnings are printed then, and only where it raises no error — so a
/// warning beside an error stays unsettled.
fn settled_for_the_loop(oracle: &Path, root: &Path, scratch: &Path, i: usize, a: &Assertion, r: &Reading, parse_time: bool) -> bool {
    let Some(mut theirs) = oracle_bundles(oracle, root, scratch, i, a, &[], parse_time) else { return false };
    if r.ir_only.iter().chain(&r.cst_only).any(|d| d.warning) {
        let every = WARNING_KINDS.iter().map(|k| format!("--W{k}")).chain(["--Werror".to_owned()]).collect::<Vec<_>>();
        theirs.extend(oracle_bundles(oracle, root, scratch, i, a, &every, parse_time).unwrap_or_default());
    }
    r.ir_only.iter().all(|d| theirs.contains(&d.bundle)) && r.cst_only.iter().all(|d| !theirs.contains(&d.bundle))
}

/// Every `WarningKind`'s switch name (`Warning.hs:29-35`).
const WARNING_KINDS: [&str; 6] = ["conversion", "deprecated", "initializer-overrides", "redundant-import", "shadow", "transaction-size"];

/// `kg-oracle --errors` on one snippet, under its command line, as the
/// `errors` gate runs it (`errors::run_oracle`, the same cache), read with
/// each bundle's notes; `None` where it cannot be asked. Where the
/// reference's parser rejects it, its verdicts are the parser's: the
/// bundles of a `parse_time` probe — one of the parser's own checks — and
/// `None` for any other, whose pass the reference never reached.
fn oracle_bundles(oracle: &Path, root: &Path, scratch: &Path, i: usize, a: &Assertion, extra: &[String], parse_time: bool) -> Option<Vec<Bundle>> {
    let mut args = crate::errors::oracle_args(root, &a.options).ok()?;
    args.extend(extra.iter().cloned());
    let stem = a.file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let dir = scratch.join(format!("{stem}-{i}"));
    let file = dir.join("syntax_test_temp_file.k");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&file, &a.source).ok()?;
    args.push("--errors".to_owned());
    args.push(file.to_string_lossy().into_owned());
    let output = crate::oracle_cache::run(oracle, root, &args, Some(&dir), &a.source);
    let _ = std::fs::remove_dir_all(&dir);
    let output = output.ok()?;
    if let Some((_, parsed)) = output.stderr.split_once("PARSE-ERROR\n") {
        return parse_time.then(|| bundles(root, parsed));
    }
    if parse_time {
        // The parser accepted it: none of its checks rejects anything.
        return Some(Vec::new());
    }
    Some(bundles(root, &output.stdout))
}

/// `errorBundlePretty`'s output, bundle by bundle, as `errors::parse_bundles`
/// reads it, each with the `note: <path>:<line>:<col>:` frames below it.
fn bundles(root: &Path, text: &str) -> Vec<Bundle> {
    use crate::errors::{header, is_gutter, normalize_message, normalize_path};
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((path, line, col)) = header(lines[i]).filter(|_| !lines[i].starts_with("note: ")) else {
            i += 1;
            continue;
        };
        i += 1;
        while i < lines.len() && is_gutter(lines[i]) {
            i += 1;
        }
        let mut message = String::new();
        while i < lines.len()
            && !lines[i].trim().is_empty()
            && header(lines[i]).is_none()
            && !lines[i].starts_with("note: ")
            && lines[i] != crate::errors::WERROR_TAIL
        {
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str(lines[i].trim_end());
            i += 1;
        }
        let message = crate::errors::bare_warning(message);
        let mut notes = Vec::new();
        while let Some((p, l, c)) = lines.get(i).and_then(|x| x.strip_prefix("note: ")).and_then(header) {
            notes.push((normalize_path(root, Path::new(p)), l, c));
            i += 1;
            while i < lines.len() && (is_gutter(lines[i]) || lines[i] == "while compiling") {
                i += 1;
            }
            while i < lines.len() && lines[i].trim().is_empty() && lines.get(i + 1).is_some_and(|x| x.starts_with("note: ")) {
                i += 1;
            }
        }
        let reported = Reported { path: normalize_path(root, Path::new(path)), line, col, message: normalize_message(&message) };
        out.push(Bundle { reported, notes });
    }
    out
}

/// The differential over one program, each verdict apart read as the
/// `errors` gate reads one and rendered with its file, line and column,
/// message and labels.
fn read(db: &kg_db::Db, roots: &[kg_base::FileId], pass: Probe, kanagawa_root: &Path) -> Reading {
    let diffs = kg_db::passdiff::differential(db, roots, pass);
    let at = |r: kg_base::FileRange| {
        let path = db.path(r.file).map(|p| p.to_string()).unwrap_or_default();
        let pos = kg_base::LineIndex::new(&db.parse(r.file).text()).line_col(r.range.start());
        (crate::errors::normalize_path(kanagawa_root, Path::new(&path)), pos.line + 1, pos.col + 1)
    };
    let apart = |d: &kg_diag::Diagnostic| {
        let (path, line, col) = at(d.range);
        let notes: Vec<(String, u32, u32)> = d.labels.iter().map(|l| at(l.range)).collect();
        let labels: Vec<String> = notes.iter().map(|(p, l, c)| format!(" [while compiling {p}:{l}:{c}]")).collect();
        let shown = format!("{path}:{line}:{col} {:?}{}", d.message, labels.concat());
        let reported = Reported { path, line, col, message: crate::errors::normalize_message(&d.message) };
        Apart { bundle: Bundle { reported, notes }, shown, warning: !d.is_error() }
    };
    let whole = |d: &kg_diag::Diagnostic| {
        let (path, line, col) = at(d.range);
        let end = kg_base::LineIndex::new(&db.parse(d.range.file).text()).line_col(d.range.range.end());
        format!("{path}:{line}:{col}-{}:{} {:?}", end.line + 1, end.col + 1, d)
    };
    let mut out = Reading { reached: diffs.len(), ..Reading::default() };
    for m in &diffs {
        out.given += m.cst.len();
        let (cst, ir) = m.apart();
        let mut ir: Vec<(&kg_diag::Diagnostic, Apart)> = ir.into_iter().map(|d| (d, apart(d))).collect();
        for d in cst {
            let a = apart(d);
            match ir.iter().position(|(_, x)| x.bundle == a.bundle) {
                Some(i) => {
                    let (twin, _) = ir.swap_remove(i);
                    out.unprinted.push((whole(d), whole(twin)));
                }
                None => out.cst_only.push(a),
            }
        }
        out.ir_only.extend(ir.into_iter().map(|(_, a)| a));
    }
    out
}

/// `items` over `workers` threads, `chunk` consecutive items at a time,
/// each thread with its own state from `init`; results in indexed slots,
/// in `items`' order.
fn fan_out<I: Sync, S, T: Send>(
    workers: usize,
    chunk: usize,
    items: &[I],
    init: impl Fn() -> S + Sync,
    each: impl Fn(&mut S, &I) -> Option<T> + Sync,
) -> Vec<Option<T>> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<T>>> = items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut state = init();
                loop {
                    let start = next.fetch_add(chunk, std::sync::atomic::Ordering::SeqCst);
                    if start >= items.len() {
                        break;
                    }
                    for (i, item) in items.iter().enumerate().skip(start).take(chunk) {
                        let result = each(&mut state, item);
                        *slots[i].lock().expect("no panics hold this lock") = result;
                    }
                }
            });
        }
    });
    slots.into_iter().map(|s| s.into_inner().expect("threads joined")).collect()
}
