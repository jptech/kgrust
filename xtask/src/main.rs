//! The kgrust scoreboard driver.
//!
//! Two subcommands today: `codegen` regenerates the derived files of the syntax
//! layer, and `conformance` scores the Rust frontend against the reference
//! corpus's 2,843 embedded assertions.

mod codegen;
mod dist;
mod guard;
mod bench;
mod bounded;
mod cli;
mod conformance;
mod errors;
mod differential;
mod edit_identity;
mod edits;
mod difffuzz;
mod corpus;
mod fmtgate;
mod fuzz;
mod ir;
mod lint;
mod options_agree;
mod loop_left;
mod rebase;
mod resilience;
mod loop_modules;
mod loop_rounds;
mod mem;
mod oracle_cache;
mod output;
mod backend;
mod backend_cache;
mod fresh;
mod ptdiff;
mod ptrace;
mod passdiff;
mod ratchet;
mod reduce;
mod sem;
mod status_md;
mod repoint;
mod strict;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// The binaries' allocator ([`kg_alloc::Allocator`]): the bench's
/// `keystroke` stage and every gate run the frontend here, and should
/// measure and grade what `kg` runs.
#[global_allocator]
static ALLOCATOR: kg_alloc::Allocator = kg_alloc::Allocator;

#[derive(Parser, Debug)]
#[command(name = "xtask", about = "kgrust developer tasks", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run our compat binary through the real backend over every recorded program and compare verdicts.
    Backend {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// The reference's build tree, holding the backend library.
        #[arg(long, env = "KANAGAWA_BUILD", default_value = "target/reference")]
        build_dir: std::path::PathBuf,
        /// Only programs whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// List every disagreeing program.
        #[arg(long)]
        verbose: bool,
        /// Fail when more than this many programs disagree.
        #[arg(long)]
        max_failures: Option<usize>,
        /// Run this one program (its exact name) and print the command line,
        /// our whole output and the reference's recorded one, then stop.
        #[arg(long)]
        show: Option<String>,
    },
    /// Compile every program the reference compiles with `--backend=sv`, both
    /// ways, and compare the files written — identical modulo source locations.
    Output {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// The reference's build tree, holding its compiler and backend library.
        #[arg(long, env = "KANAGAWA_BUILD", default_value = "target/reference")]
        build_dir: std::path::PathBuf,
        /// Only programs whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// List declined programs too.
        #[arg(long)]
        verbose: bool,
    },
    /// Inference's incremental fixpoint against the whole-walk one, every
    /// corpus root, by the elaboration's fingerprint
    /// (`docs/plans/performance.md` §3.6).
    InferIdentity {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Only roots whose path contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Instead: write each root's fingerprint, as this build elaborates
        /// it, under this directory — to diff against another build's.
        #[arg(long)]
        dump: Option<std::path::PathBuf>,
    },
    /// A warm `Db` after each kind of edit, and its undo, against a fresh
    /// one over the same texts — every corpus root (sprint W7.1).
    EditIdentity {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Only roots whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Only every Nth root (1: all).
        #[arg(long, default_value_t = 1)]
        every: usize,
    },
    /// Two roots' elaborations, module by module: is a library module's
    /// state independent of the root, copies aside? (sprint W7.5c)
    LoopModules {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Two roots, relative to the reference checkout.
        roots: Vec<String>,
    },
    /// Over `edit-identity`'s edits, the first round in which each library
    /// module's copies differ between the loop before the edit and after
    /// it, and the share of the library's work before it (sprint W7.5d,
    /// 5d-0). Per edit in `target/loop-rounds.tsv`.
    LoopRounds {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Only roots whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Only every Nth root (1: all).
        #[arg(long, default_value_t = 1)]
        every: usize,
        /// Worker threads (default: the cores, at most six).
        #[arg(long)]
        jobs: Option<usize>,
        /// Name, per edit, the first library module to part and how.
        #[arg(long)]
        verbose: bool,
    },
    /// Retiring spellings in production code, counted per rule and
    /// ratcheted: a count may fall, not rise (sprint W0.4).
    Lint,
    /// The reference commit and inputs `reference.lock` pins, against the
    /// checkout's; `--write` moves the pin (after a deliberate sync only).
    PinReference {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Write `reference.lock` from the checkout as it stands.
        #[arg(long)]
        write: bool,
    },
    /// What the program-wide loop leaves undecided and how many rounds it
    /// takes, per root, against the reference's own loop (`kg-oracle
    /// --loop`) — sprint W0.1/W0.2. Ratcheted.
    LoopLeft {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Only roots whose name contains this substring (no ledger).
        #[arg(long)]
        filter: Option<String>,
        /// Every root, and every leftover only one side holds.
        #[arg(long)]
        verbose: bool,
    },
    /// Hover and completion on broken buffers — the error-recovered corpus
    /// files and fuzzed mutants — as the editor answers them from the loop,
    /// per sampled node; every answer ratcheted (sprint W3.7).
    Resilience {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Fuzzed broken buffers, on top of the corpus's error-recovered files.
        #[arg(long, default_value_t = 64)]
        mutants: usize,
        /// Seed of the mutants; mutant `i` is a function of `(seed, i)`.
        #[arg(long, default_value_t = 0x5EED)]
        seed: u64,
        /// Hover questions per buffer, at most (spread evenly).
        #[arg(long, default_value_t = 48)]
        hover: usize,
        /// Completion questions per buffer, at most (spread evenly).
        #[arg(long, default_value_t = 8)]
        completion: usize,
        /// Only buffers whose label contains this substring (no ledger).
        #[arg(long)]
        filter: Option<String>,
        /// Every silent hover and every answer the copies give.
        #[arg(long)]
        verbose: bool,
        /// Threads, each with its own `Db`; `1` is the one-`Db` walk.
        #[arg(long, short = 'j', default_value_t = 8)]
        jobs: usize,
    },
    /// A CST pass against the loop's version of it on each module's own
    /// text, over every corpus root and conformance snippet (sprint W3
    /// step 1); red on any module where they differ.
    Passdiff {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// The pass, by the reference's name (`validateExtern`, …).
        #[arg(long)]
        pass: String,
        /// Only roots whose path, and snippets whose name, contains this.
        #[arg(long)]
        filter: Option<String>,
        /// List every program that differs.
        #[arg(long)]
        verbose: bool,
        /// Put each difference in a snippet to `kg-oracle --errors`, and
        /// pass where the reference gives the loop's verdicts and not the
        /// CST pass's.
        #[arg(long)]
        oracle: bool,
    },
    /// Compare our ParseTree walk with the reference's recorded one, per program.
    Ptdiff {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Only programs whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// List every divergent and rejected program.
        #[arg(long)]
        verbose: bool,
        /// Fail when more than this many programs are not exact.
        #[arg(long)]
        max_failures: Option<usize>,
        /// Write both traces of this one program under target/ptrace/diff/ and stop.
        #[arg(long)]
        dump: Option<String>,
        /// Print every program's verdict, one per line.
        #[arg(long)]
        list: bool,
    },
    /// Record the reference front end's ParseTree calls for every corpus program.
    Ptrace {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// The reference's build tree, holding `dist/bin/kanagawa` and the backend library.
        #[arg(long, env = "KANAGAWA_BUILD", default_value = "target/reference")]
        build_dir: std::path::PathBuf,
        /// Only programs whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Re-record even when the store already has this program.
        #[arg(long)]
        force: bool,
        /// List every program whose Codegen failed.
        #[arg(long)]
        verbose: bool,
    },
    /// Package a release archive for this platform: `kg`, `kanagawa`, the
    /// backend library beside them, the standard library, and the licenses.
    Dist {
        /// The reference checkout the standard library is taken from.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// The backend library's directory (`scripts/build-backend.sh`'s).
        #[arg(long, default_value = "target/backend/dist/bin")]
        backend_dir: std::path::PathBuf,
        /// Where the staged directory and the archive are written.
        #[arg(long, default_value = "target/dist")]
        out: std::path::PathBuf,
        /// Package without the backend: an archive that checks, formats,
        /// edits and documents, but cannot build.
        #[arg(long)]
        no_backend: bool,
    },
    /// Regenerate `syntax_kind_table.rs`, the typed AST layer, the editor's grammar, and the ABI's bindings.
    Codegen {
        /// Fail instead of writing when a generated file is out of date.
        #[arg(long)]
        check: bool,
        /// The reference checkout whose headers the ABI's bindings are generated from.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
    },
    /// Parse every corpus file and report losslessness and acceptance.
    Corpus {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// List every failing file rather than the worst 25.
        #[arg(long)]
        verbose: bool,
    },
    /// Time each compiler stage over the corpus.
    Bench {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Only run stages whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Emit JSON instead of a table, for recording a baseline.
        #[arg(long)]
        json: bool,
        /// Keystrokes the `keystroke-phases` table times per file, after
        /// its warm-up — `0` for the warm-up alone, whose instruction count
        /// subtracted from a run's is what the keystrokes cost.
        #[arg(long, default_value_t = 7)]
        keystrokes: usize,
        /// The `keystroke-phases` table under each file's recorded compile
        /// settings (the full base library) rather than the stage's.
        #[arg(long)]
        recorded: bool,
    },
    /// Peak memory per use case — `kg check`, the editor opening a file and
    /// the editor after edits — over five programs, against
    /// `memory-budgets.json` (`xtask/src/mem.rs`).
    Mem {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Edits the `editor-edits` case runs to the fixpoint.
        #[arg(long, default_value_t = 5)]
        keystrokes: usize,
        /// Only programs whose path contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Reset each measured budget to its peak plus a quarter.
        #[arg(long)]
        bless: bool,
        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Internal: run one use case on this file in this process, for
        /// the parent to measure.
        #[arg(long, hide = true)]
        child: Option<std::path::PathBuf>,
        #[arg(long, hide = true, default_value = "check")]
        case: String,
        /// The child's compile options, after `--`.
        #[arg(last = true, hide = true)]
        options: Vec<String>,
    },
    /// Mutate corpus files and check the lexer, parser and formatter survive;
    /// with `--differential`, mutate syntax snippets and grade each mutant
    /// against `kg-oracle --errors`.
    Fuzz {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// How many mutated inputs to try.
        #[arg(long, default_value_t = 2000)]
        iterations: usize,
        /// Seed, so a failure reproduces exactly.
        #[arg(long, default_value_t = 0x5EED)]
        seed: u64,
        /// Only fuzz one stage: lex, parse or format.
        #[arg(long, conflicts_with = "differential")]
        stage: Option<String>,
        /// Print every failure rather than the first ten; with
        /// `--differential`, every member of every bucket.
        #[arg(long)]
        verbose: bool,
        /// Grade each mutant's accept/reject and first error against the
        /// reference instead of checking crash invariants; buckets written
        /// to target/difffuzz/.
        #[arg(long)]
        differential: bool,
        /// Differential: threads, each with its own `Db` (default: at most 8).
        #[arg(long, short = 'j', requires = "differential")]
        jobs: Option<usize>,
        /// Differential: start no new mutant after this many seconds.
        #[arg(long, requires = "differential")]
        seconds: Option<u64>,
        /// Differential: reduce each bucket's smallest mutant, worst bucket
        /// first, starting none after this many seconds of reducing.
        #[arg(long, requires = "differential", value_name = "SECONDS", num_args = 0..=1, default_missing_value = "300")]
        reduce: Option<u64>,
        /// Differential: probe budget per reduction.
        #[arg(long, default_value_t = 1500)]
        max_tests: usize,
    },
    /// Our side of the differential probe, one program per stdin line.
    #[command(hide = true)]
    ProbeServer {
        #[arg(long)]
        kanagawa_root: std::path::PathBuf,
    },
    /// Delta-debug a `.k` file down while our first error and the
    /// reference's still disagree the same way.
    Reduce {
        /// The program to reduce.
        file: std::path::PathBuf,
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// `errors`: keep the kind and both first messages. `accept`: keep
        /// only which side accepts.
        #[arg(long, default_value = "errors")]
        keep: String,
        /// Compile under this syntax assertion's flags and import dirs
        /// (default: the flags most snippets share).
        #[arg(long)]
        like: Option<String>,
        /// Where to write the result (default: target/reduce/<stem>.reduced.k).
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Threads, each with its own `Db` (default: at most 8).
        #[arg(long, short = 'j')]
        jobs: Option<usize>,
        /// Stop after this many probes.
        #[arg(long, default_value_t = 4000)]
        max_tests: usize,
    },
    /// Prove the formatter is idempotent and layout-only over the whole corpus.
    FmtCheck {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Format at this line width instead of the configured default.
        #[arg(long)]
        width: Option<usize>,
        /// List every declined file and every failure rather than the first 25.
        #[arg(long)]
        verbose: bool,
    },
    /// Lower every corpus file to the IR and check totality, the pretty →
    /// reparse → lower round trip, and the provenance invariant.
    Ir {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// List every failing file rather than the worst 10.
        #[arg(long)]
        verbose: bool,
    },
    /// Grade our first error's position and message against `kg-oracle --errors`
    /// over every `expected:1` snippet — the class of bug conformance cannot see.
    Errors {
        /// Path to the reference checkout holding `test/syntax/`.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Only run assertions whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Succeed while disagreements are at or below this count — a ratchet.
        #[arg(long)]
        max_failures: Option<usize>,
        /// Print every disagreement, and the reference's message for every snippet we compile cleanly.
        #[arg(long)]
        verbose: bool,
    },
    /// Score the frontend against the reference corpus's embedded assertions.
    Conformance {
        /// Path to the reference checkout holding `test/syntax/`.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// Only run assertions whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// List the assertion inventory and exit without running anything.
        #[arg(long)]
        list: bool,
        /// Treat the frontend as feature-complete, so an assertion the parser
        /// cannot decide counts as a failure rather than as undecidable — what
        /// the plain run does too since `FRONTEND_COMPLETE` was set.
        #[arg(long)]
        strict: bool,
        /// Succeed while failures are at or below this count — a ratchet.
        ///
        /// Written for the era `--strict` was expected to fail; with
        /// `FRONTEND_COMPLETE` set (2026-10-10) the plain run grades the same
        /// and must fail nothing. Pinning a count still makes a gap something
        /// a change can only close.
        #[arg(long, value_name = "N")]
        max_failures: Option<usize>,
        /// Threads to spread the snippets over, each with its own `Db`.
        /// Defaults to the machine's parallelism, at most 8; `1` is the
        /// single-`Db` loop in corpus order.
        #[arg(long, short = 'j', value_name = "N")]
        jobs: Option<usize>,
    },
    /// Conformance and `--strict`, per assertion, ratcheted against
    /// `baseline.json` — the full failing set `conformance --strict` does not print.
    StrictLedger {
        /// Path to the reference checkout holding `test/syntax/`.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// A `conformance` or `conformance --strict` log whose headline and
        /// FAIL lines this ledger must reproduce; repeatable.
        #[arg(long, value_name = "LOG")]
        against: Vec<std::path::PathBuf>,
        /// Threads, as for `conformance`.
        #[arg(long, short = 'j', value_name = "N")]
        jobs: Option<usize>,
    },
    /// Compare the latest ledgers in `target/gates/` with `baseline.json`, print
    /// one screen and write `target/gates/status.json`; or bless them into it.
    Ratchet {
        /// Gates to read or bless (strict, errors, ptdiff, backend, repoint); all when empty.
        gates: Vec<String>,
        /// Write these gates' latest ledgers into `baseline.json` — the one
        /// deliberate way to move a ratchet, in either direction.
        #[arg(long)]
        bless: bool,
        /// A pipeline stage's outcome for the summary, `name=ok` or
        /// `name=FAIL[:note]`; repeatable (`scripts/gates.sh` passes these).
        #[arg(long, value_name = "NAME=STATUS")]
        stage: Vec<String>,
        /// Instead: carry `baseline.json`'s assertion names from the
        /// reference at this commit to the checkout's — after moving the
        /// pin, before the first run (`xtask/src/rebase.rs`).
        #[arg(long, value_name = "COMMIT")]
        rebase: Option<String>,
        /// Path to the reference checkout (for `--rebase`).
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
    },
    /// Render `docs/status.md`'s headline from the last green gate run's
    /// `target/gates/status.json`.
    StatusMd {
        /// Fail if the headline differs from the last run instead of writing it.
        #[arg(long)]
        check: bool,
    },
    /// Upstream's own CLI tests (`test/compiler/cli`, as the reference
    /// build's `CTestTestfile.cmake` runs them) against the `kanagawa` compat
    /// binary, and every mode's `--help` against the reference binary's.
    /// Ratcheted.
    Cli {
        /// Path to the reference checkout.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// The reference's build tree: its configured tests, its compiler and
        /// backend library.
        #[arg(long, env = "KANAGAWA_BUILD", default_value = "target/reference")]
        build_dir: std::path::PathBuf,
        /// A `kanagawa`-compatible binary to grade. By default, this tree's
        /// own, built first if cargo says it is stale; the reference's own
        /// is the check on the driver.
        #[arg(long)]
        binary: Option<std::path::PathBuf>,
        /// The whole output of every failing test.
        #[arg(long, short)]
        verbose: bool,
    },
    /// Run the **reference's own** `syntax-test.py` against the `kanagawa`
    /// compat binary — the cross-check `conformance` cannot perform on itself.
    Repoint {
        /// Path to the reference checkout holding `test/syntax/`.
        #[arg(long, env = "KANAGAWA_ROOT", default_value = "third_party/kanagawa")]
        kanagawa_root: std::path::PathBuf,
        /// A `kanagawa`-compatible binary for upstream's runner to drive. By
        /// default, this tree's own, built first if cargo says it is stale.
        #[arg(long)]
        binary: Option<std::path::PathBuf>,
        /// The reference's build tree, holding the backend library.
        #[arg(long, env = "KANAGAWA_BUILD", default_value = "target/reference")]
        build_dir: std::path::PathBuf,
        /// Only run registered tests whose name contains this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Succeed while failures are at or below this count — a ratchet.
        #[arg(long, value_name = "N")]
        max_failures: Option<usize>,
        /// Print the runner's own output for every failing test.
        #[arg(long, short)]
        verbose: bool,
    },
}

fn main() -> Result<()> {
    let command = Cli::parse().command;
    // Each gate's threads own a `Db` apiece and share one budget
    // ([`bounded::watch_self`]).
    bounded::watch_self(std::env::args().nth(1).unwrap_or_default());
    verdict_switches_note();
    // The reference tree is read-only, and a gate that spawns the compiler
    // can leave a report wherever its cwd is ([`guard`]).
    let root = reference_root(&command);
    // Every gate grades against the pinned reference (`guard::check_pin`);
    // `pin-reference` is how the pin is read and moved.
    if let Some(root) = &root
        && !matches!(command, Cmd::PinReference { .. })
    {
        guard::check_pin(root, &fresh::workspace().join(guard::LOCK))?;
    }
    let before = root.as_deref().map(guard::snapshot);
    let outcome = dispatch(command);
    if let (Some(root), Some(before)) = (root, before) {
        guard::check(&root, &before)?;
    }
    outcome
}

/// The port's verdict switches are settings read at the compiler's front
/// doors (`kg_config::Verdicts`, sprint W8.2): a compiler a gate spawns —
/// `backend`, `output`, `repoint` — reads them from its environment, and so
/// do the databases `conformance::settings_for` builds (`conformance`,
/// `errors`, `strict-ledger`). Said once, so that a run under one is not
/// read as a default run — nor blessed as one.
fn verdict_switches_note() {
    let defaults = kg_config::Verdicts::default();
    if kg_config::Verdicts::read(|name| std::env::var_os(name)) != defaults {
        eprintln!(
            "xtask: a verdict switch is set (KG_LEFTOVERS, KG_INFER_FULL or KG_PTBRIDGE_SEED): \
             this run grades the compiler under it — do not bless its ledgers"
        );
    }
}

/// The reference checkout a command was pointed at — every one that runs a
/// compiler over it. `codegen` reads only the ABI's headers, never runs
/// one, and checks the pin itself, only when the headers are there.
fn reference_root(command: &Cmd) -> Option<std::path::PathBuf> {
    match command {
        // A probe server's parent brackets the run.
        Cmd::Codegen { .. } | Cmd::Lint | Cmd::Ratchet { .. } | Cmd::StatusMd { .. } | Cmd::ProbeServer { .. } => None,
        // A measured child: its parent brackets the run, and a tree walk
        // here would be counted in the peak.
        Cmd::Mem { child: Some(_), .. } => None,
        Cmd::Mem { kanagawa_root, .. } => Some(kanagawa_root.clone()),
        // The library packaged is the pinned one, unmodified.
        Cmd::Dist { kanagawa_root, .. } => Some(kanagawa_root.clone()),
        Cmd::StrictLedger { kanagawa_root, .. }
        | Cmd::Backend { kanagawa_root, .. }
        | Cmd::Output { kanagawa_root, .. }
        | Cmd::Ptdiff { kanagawa_root, .. }
        | Cmd::Passdiff { kanagawa_root, .. }
        | Cmd::InferIdentity { kanagawa_root, .. }
        | Cmd::LoopLeft { kanagawa_root, .. }
        | Cmd::Resilience { kanagawa_root, .. }
        | Cmd::EditIdentity { kanagawa_root, .. }
        | Cmd::LoopModules { kanagawa_root, .. }
        | Cmd::LoopRounds { kanagawa_root, .. }
        | Cmd::PinReference { kanagawa_root, .. }
        | Cmd::Ptrace { kanagawa_root, .. }
        | Cmd::Corpus { kanagawa_root, .. }
        | Cmd::Ir { kanagawa_root, .. }
        | Cmd::Fuzz { kanagawa_root, .. }
        | Cmd::Reduce { kanagawa_root, .. }
        | Cmd::Bench { kanagawa_root, .. }
        | Cmd::FmtCheck { kanagawa_root, .. }
        | Cmd::Errors { kanagawa_root, .. }
        | Cmd::Conformance { kanagawa_root, .. }
        | Cmd::Cli { kanagawa_root, .. }
        | Cmd::Repoint { kanagawa_root, .. } => Some(kanagawa_root.clone()),
    }
}

fn dispatch(command: Cmd) -> Result<()> {
    match command {
        Cmd::Codegen { check, kanagawa_root } => codegen::run(check, &kanagawa_root),
        Cmd::Dist { kanagawa_root, backend_dir, out, no_backend } => dist::run(&kanagawa_root, &backend_dir, &out, no_backend),
        Cmd::Backend { kanagawa_root, build_dir, show: Some(name), .. } => backend::show(&kanagawa_root, &build_dir, &name),
        Cmd::Backend { kanagawa_root, build_dir, filter, verbose, max_failures, show: None } => {
            backend::run(&kanagawa_root, &build_dir, filter.as_deref(), verbose, max_failures)
        }
        Cmd::Output { kanagawa_root, build_dir, filter, verbose } => {
            output::run(&kanagawa_root, &build_dir, filter.as_deref(), verbose)
        }
        Cmd::InferIdentity { kanagawa_root, filter, dump } => sem::identity(&kanagawa_root, filter.as_deref(), dump.as_deref()),
        Cmd::Lint => lint::run(),
        Cmd::LoopModules { kanagawa_root, roots } => loop_modules::run(&kanagawa_root, &roots),
        Cmd::LoopRounds { kanagawa_root, filter, every, jobs, verbose } => {
            loop_rounds::run(&kanagawa_root, filter.as_deref(), every, jobs, verbose)
        }
        Cmd::EditIdentity { kanagawa_root, filter, every } => edit_identity::run(&kanagawa_root, filter.as_deref(), every),
        Cmd::PinReference { kanagawa_root, write } => guard::pin_reference(&kanagawa_root, write),
        Cmd::LoopLeft { kanagawa_root, filter, verbose } => loop_left::run(&kanagawa_root, filter.as_deref(), verbose),
        Cmd::Resilience { kanagawa_root, mutants, seed, hover, completion, filter, verbose, jobs } => {
            resilience::run(&kanagawa_root, &resilience::Options { seed, mutants, hover, completion, filter, verbose, jobs })
        }
        Cmd::Passdiff { kanagawa_root, pass, filter, verbose, oracle } => {
            passdiff::run(&kanagawa_root, &pass, filter.as_deref(), verbose, oracle)
        }
        Cmd::Ptdiff { kanagawa_root, filter, verbose, max_failures, dump, list } => match dump {
            Some(name) => ptdiff::dump(&kanagawa_root, &name),
            None => ptdiff::run(&kanagawa_root, filter.as_deref(), verbose, max_failures, list),
        },
        Cmd::Ptrace { kanagawa_root, build_dir, filter, force, verbose } => {
            ptrace::run(&kanagawa_root, &build_dir, filter.as_deref(), force, verbose)
        }
        Cmd::Corpus { kanagawa_root, verbose } => corpus::run(&kanagawa_root, verbose),
        Cmd::Ir { kanagawa_root, verbose } => ir::run(&kanagawa_root, verbose),
        Cmd::Fuzz { kanagawa_root, iterations, seed, verbose, differential: true, jobs, seconds, reduce, max_tests, .. } => {
            let opts = difffuzz::Options { iterations, seed, jobs, seconds, reduce, max_tests, verbose };
            difffuzz::run(&kanagawa_root, &opts)
        }
        Cmd::Fuzz { kanagawa_root, iterations, seed, stage, verbose, .. } => {
            fuzz::run(&kanagawa_root, iterations, seed, stage.as_deref(), verbose)
        }
        Cmd::ProbeServer { kanagawa_root } => differential::serve(&kanagawa_root),
        Cmd::Reduce { file, kanagawa_root, keep, like, out, jobs, max_tests } => {
            reduce::run(&kanagawa_root, &file, &keep, like.as_deref(), out.as_deref(), jobs, max_tests)
        }
        Cmd::Mem { kanagawa_root, child: Some(file), case, keystrokes, options, .. } => {
            mem::child(&kanagawa_root, &file, &case, keystrokes, &options)
        }
        Cmd::Mem { kanagawa_root, keystrokes, filter, bless, json, child: None, .. } => {
            mem::run(&kanagawa_root, keystrokes, filter.as_deref(), bless, json)
        }
        Cmd::Bench { kanagawa_root, filter, json, keystrokes, recorded } => {
            bench::run(&kanagawa_root, filter.as_deref(), json, keystrokes, recorded)
        }
        Cmd::FmtCheck { kanagawa_root, width, verbose } => {
            fmtgate::run(&kanagawa_root, width, verbose)
        }
        Cmd::Errors { kanagawa_root, filter, max_failures, verbose } => {
            errors::run(&kanagawa_root, filter.as_deref(), max_failures, verbose)
        }
        Cmd::Conformance { kanagawa_root, filter, list, strict, max_failures, jobs } => {
            conformance::run(&kanagawa_root, filter.as_deref(), list, strict, max_failures, jobs)
        }
        Cmd::Cli { kanagawa_root, build_dir, binary, verbose } => {
            cli::run(&kanagawa_root, &build_dir, binary.as_deref(), verbose)
        }
        Cmd::Repoint { kanagawa_root, binary, build_dir, filter, max_failures, verbose } => {
            repoint::run(&kanagawa_root, binary.as_deref(), &build_dir, filter.as_deref(), max_failures, verbose)
        }
        Cmd::StrictLedger { kanagawa_root, against, jobs } => strict::run(&kanagawa_root, &against, jobs),
        Cmd::Ratchet { rebase: Some(commit), kanagawa_root, .. } => rebase::run(&kanagawa_root, &commit),
        Cmd::Ratchet { gates, bless, stage, rebase: None, .. } => ratchet::run(&gates, bless, &stage),
        Cmd::StatusMd { check } => status_md::run(check),
    }
}
