//! One program, both compilers: our first error against `kg-oracle --errors`,
//! graded the way `xtask errors` grades a must-reject snippet — the probe the
//! differential fuzzer (`fuzz --differential`) and the reducer (`reduce`)
//! share, so "the same disagreement" means the same thing to both.
//!
//! `xtask errors` only ever sees snippets the reference rejects. A program
//! written by a mutator can go either way, so the grading adds the two cases
//! that gate folds into `oracle silent` and `ours silent`, and they are not
//! equally bad:
//!
//! * **false alarm** — we reject what the reference accepts. An invented
//!   error on a correct program; `AGENTS.md` "fail toward silence" is about
//!   exactly this, and it is the worst thing either instrument can report.
//! * **ours silent** — we accept what the reference rejects. The scoreboard
//!   books this undecidable; it is honest, and it is work, not a lie.
//!
//! Where both reject, the verdict is `errors::judge`'s, unchanged: our first
//! error by position must be one the reference reports anywhere, a parse-time
//! rejection is compared on position only (divergence #8), and a grammar
//! error there against a later pass here is booked to divergence #7.
//!
//! The oracle's `--errors` mode runs the reference's parser and frontend
//! passes and nothing after (`oracle/driver/Main.hs`). A check the reference
//! makes only in its backend therefore reads as a false alarm here even when
//! the real compiler would agree with us; the fuzzer separates those by
//! grading the unmutated snippet too, and an unmutated snippet that already
//! disagrees the same way is inherited, not found.

use std::io::{BufRead as _, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::conformance::{Assertion, Expectation, settings_for};
use crate::errors::{FirstError, OracleSide, Reported, Verdict, first_error, judge, nearest, oracle_args, read_oracle};

/// How a program's two readings relate. Ordered by how bad a finding of the
/// kind is, which is the order a report lists its buckets in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Kind {
    /// We reject, the reference accepts.
    FalseAlarm,
    /// Our frontend panicked, or its process died.
    OursCrashed,
    /// Our frontend gave no answer within the limit.
    OursHung,
    /// Both reject at parse time, at different places.
    ParsePosition,
    /// Both reject: same message, different place.
    Position,
    /// Both reject: same place, different words.
    Message,
    /// Both reject, and nothing matches.
    Both,
    /// We accept, the reference rejects.
    OursSilent,
    /// The oracle timed out or could not be run; nothing is known.
    OracleFailed,
    /// The reference's parser rejects and a later pass here does (#7).
    Booked,
    /// Both accept, or both reject with the same first error.
    Agree,
}

impl Kind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Kind::FalseAlarm => "false-alarm",
            Kind::OursCrashed => "ours-crashed",
            Kind::OursHung => "ours-hung",
            Kind::ParsePosition => "parse-position",
            Kind::Position => "position",
            Kind::Message => "message",
            Kind::Both => "both-differ",
            Kind::OursSilent => "ours-silent",
            Kind::OracleFailed => "oracle-failed",
            Kind::Booked => "booked-7",
            Kind::Agree => "agree",
        }
    }

    /// A disagreement worth a bucket: everything but agreement, the booked
    /// divergence, and a run that told us nothing.
    pub(crate) fn is_finding(self) -> bool {
        !matches!(self, Kind::Agree | Kind::Booked | Kind::OracleFailed)
    }

    /// Both sides reject — a disagreement about *which* error, not whether.
    pub(crate) fn both_reject(self) -> bool {
        matches!(self, Kind::ParsePosition | Kind::Position | Kind::Message | Kind::Both | Kind::Booked)
    }
}

/// What makes two disagreements "the same": the kind and the first line of
/// each side's message, positions left out — a reduction moves every
/// position and must not move the bug.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct Signature {
    pub(crate) kind: Kind,
    pub(crate) ours: String,
    pub(crate) theirs: String,
}

/// Why our side gave no answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum OursFailed {
    /// A panic, caught in the probe server; its first line.
    Panicked(String),
    /// No answer within the limit; the server was killed.
    Hung(Duration),
    /// The server died without answering: an abort, a stack overflow.
    Died,
}

/// Both readings of one program.
#[derive(Clone, Debug)]
pub(crate) struct Probe {
    /// Our first error, or why there is none.
    pub(crate) ours: Result<FirstError, OursFailed>,
    pub(crate) theirs: OracleSide,
    /// What the reference printed when it died rather than answered — a
    /// Haskell `error` in a pass, which the real compiler would report as
    /// an internal error. Nothing is known about the program then.
    pub(crate) oracle_crash: Option<String>,
}

impl Probe {
    fn their_errors(&self) -> Option<&[Reported]> {
        match &self.theirs {
            OracleSide::Parse(t) | OracleSide::Frontend(t) => Some(t),
            OracleSide::Skipped(_) => None,
        }
    }

    pub(crate) fn kind(&self) -> Kind {
        let mine = match &self.ours {
            Err(OursFailed::Hung(_)) => return Kind::OursHung,
            Err(_) => return Kind::OursCrashed,
            Ok(mine) => mine,
        };
        let Some(theirs) = self.their_errors() else { return Kind::OracleFailed };
        if self.oracle_crash.is_some() {
            return Kind::OracleFailed;
        }
        // A `PARSE-ERROR` the bundle reader found nothing in is a rejection
        // we cannot read, not an acceptance.
        if theirs.is_empty() && matches!(self.theirs, OracleSide::Parse(_)) {
            return Kind::OracleFailed;
        }
        match (mine, theirs.is_empty()) {
            (None, true) => return Kind::Agree,
            (Some(_), true) => return Kind::FalseAlarm,
            _ => {}
        }
        match judge(mine.as_ref(), &self.theirs) {
            Verdict::Agree | Verdict::ParseAgree => Kind::Agree,
            Verdict::ParseTimeTable => Kind::Booked,
            Verdict::ParsePosition => Kind::ParsePosition,
            Verdict::Position => Kind::Position,
            Verdict::Message => Kind::Message,
            Verdict::Both => Kind::Both,
            Verdict::OursSilent => Kind::OursSilent,
            // Unreachable after the two early returns; graded as nothing known.
            Verdict::OracleSilent | Verdict::Skipped => Kind::OracleFailed,
        }
    }

    /// Our first error, as `errors` prints it.
    pub(crate) fn ours_error(&self) -> Option<&Reported> {
        self.ours.as_ref().ok().and_then(|m| m.as_ref().map(|(r, _)| r))
    }

    /// The reference's error a disagreement is read against: the nearest to
    /// ours (`errors::nearest`), or its first by position when we are silent.
    pub(crate) fn their_error(&self) -> Option<&Reported> {
        let theirs = self.their_errors()?;
        match self.ours_error() {
            Some(m) => nearest(theirs, m),
            None => theirs.iter().min_by(|a, b| a.position().cmp(&b.position())),
        }
    }

    pub(crate) fn signature(&self) -> Signature {
        let first = |r: Option<&Reported>| r.map_or_else(String::new, |r| r.message.lines().next().unwrap_or("").to_owned());
        let kind = self.kind();
        let ours = match &self.ours {
            Err(OursFailed::Panicked(panic)) => format!("panic: {panic}"),
            Err(OursFailed::Hung(_)) => "hung".to_owned(),
            Err(OursFailed::Died) => "died".to_owned(),
            Ok(_) => first(self.ours_error()),
        };
        Signature { kind, ours, theirs: first(self.their_error()) }
    }

    pub(crate) fn render_ours(&self) -> String {
        match &self.ours {
            Err(OursFailed::Panicked(panic)) => format!("panicked: {panic}"),
            Err(OursFailed::Hung(limit)) => format!("no answer in {} s; killed", limit.as_secs()),
            Err(OursFailed::Died) => "the probe server died without answering".to_owned(),
            Ok(None) => "accepted".to_owned(),
            Ok(Some((r, parser))) => format!("{}{}", r.render(), if *parser { "  [parser]" } else { "" }),
        }
    }

    pub(crate) fn render_theirs(&self) -> String {
        if let Some(crash) = &self.oracle_crash {
            return format!("crashed: {crash}");
        }
        match &self.theirs {
            OracleSide::Skipped(why) => format!("not run: {why}"),
            OracleSide::Parse(t) | OracleSide::Frontend(t) if t.is_empty() => "accepted".to_owned(),
            side => {
                let parse = if matches!(side, OracleSide::Parse(_)) { "  [parser]" } else { "" };
                self.their_error().map_or_else(String::new, |r| format!("{}{parse}", r.render()))
            }
        }
    }
}

/// What every probe shares: the reference tree, the oracle, a scratch
/// directory for the oracle's copy of each program, and its time limit.
pub(crate) struct Context {
    pub(crate) root: PathBuf,
    pub(crate) oracle: PathBuf,
    pub(crate) scratch: PathBuf,
    /// The oracle's time limit.
    pub(crate) limit: Duration,
    /// Ours. The frontend answers a snippet in milliseconds; ten seconds
    /// is a hang, not a slow program.
    pub(crate) ours_limit: Duration,
}

impl Context {
    /// `scratch` names a directory under this workspace's `target/`, never
    /// under the reference tree.
    pub(crate) fn new(kanagawa_root: &Path, scratch: &str) -> anyhow::Result<Context> {
        let Some(oracle) = crate::ir::find_oracle() else {
            anyhow::bail!("kg-oracle is not built — run oracle/build.sh first");
        };
        let root = kanagawa_root
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("no reference checkout at {}: {e}", kanagawa_root.display()))?;
        let scratch = workspace_target().join(scratch);
        Ok(Context { root, oracle, scratch, limit: Duration::from_secs(20), ours_limit: Duration::from_secs(10) })
    }
}

/// This workspace's `target/`, where every file these instruments write goes.
pub(crate) fn workspace_target() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the workspace").join("target")
}

/// One thread's two compilers: a probe server for ours, the oracle binary
/// for theirs.
///
/// Our side runs in a child process (`xtask probe-server`, [`serve`]) that
/// owns the `Db`, not on the thread itself. A mutant found in the first
/// real run sends `static for` unrolling into a loop that held one thread
/// at several gigabytes for minutes, and a thread cannot be stopped from
/// outside; a process can. The server keeps its `Db` across requests — the
/// `errors` gate's `our_side`, one program at a time, re-configured only
/// when the flags change — and is killed and replaced when it hangs or dies.
pub(crate) struct Worker {
    server: Option<Server>,
}

/// A running probe server, and its answers as they arrive.
struct Server {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    answers: mpsc::Receiver<String>,
}

/// Marks the server's answer lines, so anything else the frontend might
/// print to stdout cannot be read as one.
const ANSWER: &str = "\u{1}probe ";

impl Server {
    fn spawn(root: &Path) -> std::io::Result<Server> {
        use std::process::Stdio;
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .arg("probe-server")
            .arg("--kanagawa-root")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (tx, answers) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(answer) = line.strip_prefix(ANSWER)
                    && tx.send(answer.to_owned()).is_err()
                {
                    break;
                }
            }
        });
        Ok(Server { child, stdin, answers })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Worker {
    pub(crate) fn new() -> Worker {
        Worker { server: None }
    }

    /// Both readings of `text`, compiled as the snippet `like` names would
    /// be — its flags, its import directories, its temp-file path.
    pub(crate) fn probe(&mut self, cx: &Context, like: &Assertion, text: &str) -> Probe {
        let a = Assertion { source: text.to_owned(), ..like.clone() };
        let ours = self.ours(cx, &a);
        let (theirs, oracle_crash) = oracle_probe(cx, &a);
        Probe { ours, theirs, oracle_crash }
    }

    fn ours(&mut self, cx: &Context, a: &Assertion) -> Result<FirstError, OursFailed> {
        if self.server.is_none() {
            self.server = Some(Server::spawn(&cx.root).map_err(|_| OursFailed::Died)?);
        }
        let server = self.server.as_mut().expect("spawned above");
        let request = serde_json::json!({ "options": &*a.options, "file": a.file, "source": a.source });
        if writeln!(server.stdin, "{request}").and_then(|()| server.stdin.flush()).is_err() {
            self.server = None;
            return Err(OursFailed::Died);
        }
        let answer = match server.answers.recv_timeout(cx.ours_limit) {
            Ok(answer) => answer,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.server = None;
                return Err(OursFailed::Hung(cx.ours_limit));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.server = None;
                return Err(OursFailed::Died);
            }
        };
        let v: serde_json::Value = serde_json::from_str(&answer).map_err(|_| OursFailed::Died)?;
        if let Some(panic) = v["panic"].as_str() {
            return Err(OursFailed::Panicked(panic.to_owned()));
        }
        let first = &v["first"];
        if first.is_null() {
            return Ok(None);
        }
        let reported = Reported {
            path: first["path"].as_str().unwrap_or_default().to_owned(),
            line: first["line"].as_u64().unwrap_or_default() as u32,
            col: first["col"].as_u64().unwrap_or_default() as u32,
            message: first["message"].as_str().unwrap_or_default().to_owned(),
        };
        Ok(Some((reported, first["parser"].as_bool().unwrap_or_default())))
    }
}

/// `xtask probe-server`: our first error for each program a [`Worker`]
/// writes to stdin, one JSON object per line, answered one per line. The
/// server owns one `Db` for its life and re-configures it when the flags
/// change; a panic is caught and answered, and the `Db` it unwound through
/// is replaced.
pub fn serve(kanagawa_root: &Path) -> anyhow::Result<()> {
    let root = kanagawa_root.canonicalize()?;
    std::panic::set_hook(Box::new(|_| {}));
    let mut db: Option<kg_db::Db> = None;
    let mut current: Option<Arc<[String]>> = None;
    let mut out = std::io::stdout().lock();
    let mut served = 0u64;
    for line in std::io::stdin().lock().lines() {
        // Every mutant is a new program text, and a `Db` keeps what it
        // computed for each: one server grew to 9 GB over a 6000-mutant run
        // (2026-09-24) before the run's own watchdog killed it. Past
        // `KG_PROBE_DB_MB` (2048) resident, checked every 20 probes, the
        // `Db` is dropped and the next probe builds a fresh one.
        served += 1;
        if served.is_multiple_of(20) && crate::bounded::self_rss_mb() > probe_db_limit_mb() {
            db = None;
        }
        let v: serde_json::Value = serde_json::from_str(&line?)?;
        let options: Arc<[String]> = v["options"]
            .as_array()
            .map(|a| a.iter().filter_map(|o| o.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let a = Assertion {
            name: String::new(),
            file: PathBuf::from(v["file"].as_str().unwrap_or_default()),
            marker_line: 0,
            source: v["source"].as_str().unwrap_or_default().to_owned(),
            expectation: Expectation::Code(1),
            registered: true,
            options,
        };
        if db.is_none() || current.as_deref() != Some(&*a.options) {
            match &mut db {
                Some(db) => db.set_settings(settings_for(&root, &a.options)),
                None => db = Some(kg_db::Db::new(settings_for(&root, &a.options))),
            }
            current = Some(Arc::clone(&a.options));
        }
        let d = db.as_mut().expect("set above");
        let answer = match catch_unwind(AssertUnwindSafe(|| first_error(&root, d, &a))) {
            Ok(None) => serde_json::json!({ "first": null }),
            Ok(Some((r, parser))) => serde_json::json!({ "first": {
                "path": r.path, "line": r.line, "col": r.col, "message": r.message, "parser": parser,
            } }),
            Err(payload) => {
                // A `Db` a panic unwound through may hold a half-written memo.
                db = None;
                serde_json::json!({ "panic": crate::fuzz::panic_message(payload.as_ref()) })
            }
        };
        writeln!(out, "{ANSWER}{answer}")?;
        out.flush()?;
    }
    Ok(())
}

fn probe_db_limit_mb() -> u64 {
    std::env::var("KG_PROBE_DB_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(2048)
}

/// The oracle's side, in a directory named by what it holds: the importing
/// file's directory is searched first (`Parser/IO.hs:107`), so it must hold
/// nothing else, and a name that is a function of the content keeps the
/// oracle cache's key (which includes the path) stable from run to run.
fn oracle_probe(cx: &Context, a: &Assertion) -> (OracleSide, Option<String>) {
    let args = match oracle_args(&cx.root, &a.options) {
        Ok(args) => args,
        Err(why) => return (OracleSide::Skipped(why), None),
    };
    let mut h = crate::oracle_cache::Fnv::new();
    for arg in &args {
        h.write_str(arg);
    }
    h.write_str(&a.source);
    let dir = cx.scratch.join(h.finish());
    let file = dir.join("syntax_test_temp_file.k");
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&file, &a.source).is_err() {
        return (OracleSide::Skipped("could not write the scratch snippet"), None);
    }
    let mut all = args;
    all.push("--errors".to_owned());
    all.push(file.to_string_lossy().into_owned());
    let output = crate::oracle_cache::run_limited(&cx.oracle, &cx.root, &all, Some(&dir), &a.source, Some(cx.limit));
    let output = match output {
        Ok(Some(output)) => output,
        Ok(None) => return (OracleSide::Skipped("the oracle ran past its time limit"), None),
        Err(_) => return (OracleSide::Skipped("the oracle could not be spawned"), None),
    };
    // The driver writes nothing to stderr but the `PARSE-ERROR` marker and
    // its bundles (`oracle/driver/Main.hs`, `run`); anything else there is
    // the reference dying, and an empty stdout beside it is not an acceptance.
    let crash = (!output.stderr.trim().is_empty() && !output.stderr.contains("PARSE-ERROR\n"))
        .then(|| output.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("").to_owned());
    (read_oracle(&cx.root, &output.stdout, &output.stderr), crash)
}

/// Probes `items` over `workers`, one thread each, in the order `order`
/// gives, each result in its item's own slot. Nothing is started after
/// `deadline`; a slot left unstarted is `None`.
pub(crate) fn probe_all(
    cx: &Context,
    workers: &mut [Worker],
    items: &[(&Assertion, &str)],
    order: &[usize],
    deadline: Option<Instant>,
) -> Vec<Option<Probe>> {
    let next = AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<Option<Probe>>> = items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for worker in workers.iter_mut() {
            let (next, out) = (&next, &out);
            scope.spawn(move || loop {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    break;
                }
                let k = next.fetch_add(1, Ordering::SeqCst);
                let Some(&i) = order.get(k) else { break };
                let (like, text) = items[i];
                let probe = worker.probe(cx, like, text);
                *out[i].lock().expect("no panics hold this lock") = Some(probe);
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().expect("threads joined")).collect()
}

/// `--jobs`, or the gates' default: the machine's parallelism, at most 8.
pub(crate) fn jobs(requested: Option<usize>) -> usize {
    requested
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8))
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reported(line: u32, message: &str) -> Reported {
        Reported { path: "<snippet>".into(), line, col: 1, message: message.into() }
    }

    #[test]
    fn the_two_one_sided_kinds_are_told_apart() {
        let accept = OracleSide::Frontend(vec![]);
        let reject = OracleSide::Frontend(vec![reported(2, "m")]);
        let ours = |m: Option<Reported>| Ok(m.map(|r| (r, false)));
        let kind = |ours, theirs| Probe { ours, theirs, oracle_crash: None }.kind();
        assert_eq!(kind(ours(None), accept.clone()), Kind::Agree);
        assert_eq!(kind(ours(Some(reported(2, "m"))), accept), Kind::FalseAlarm);
        assert_eq!(kind(ours(None), reject.clone()), Kind::OursSilent);
        assert_eq!(kind(ours(Some(reported(2, "m"))), reject.clone()), Kind::Agree);
        assert_eq!(kind(ours(Some(reported(3, "m"))), reject.clone()), Kind::Position);
        assert_eq!(kind(ours(Some(reported(2, "n"))), reject.clone()), Kind::Message);
        assert_eq!(kind(Err(OursFailed::Panicked("boom".into())), reject.clone()), Kind::OursCrashed);
        assert_eq!(kind(Err(OursFailed::Hung(Duration::from_secs(1))), reject), Kind::OursHung);
        assert_eq!(kind(ours(None), OracleSide::Parse(vec![])), Kind::OracleFailed);
    }
}
