//! Memory and time bounds for the gates, so that one runaway cannot take the
//! machine down with it.
//!
//! On 2026-09-23 a session's gates and side runs ran a 64 GB machine out of
//! memory. Measured afterwards (`scratchpad/memwatch.py`, the whole process
//! tree sampled every half second): a green `scripts/gates.sh` peaks at
//! 17.3 GB with `ptdiff` 7.5 GB, `ir` 5.7 GB and the `query` test binary
//! 4.2 GB among its processes; the heaviest compiler run the gates spawn,
//! `test/library/data/array.k`, is 1.8 GB and 84 s of CPU, its backend worker
//! 0.6 GB. Nothing bounded any of it: a program whose elaboration never
//! converged, eight at a time, would have taken whatever there was.
//!
//! Two bounds, each with a headroom of several times the largest normal
//! reading and each overridable from the environment:
//!
//! - [`run`]: a child process — our compiler, the reference's, the oracle,
//!   upstream's runner — in a process group of its own, so the backend
//!   worker it spawns is measured and killed with it. Over its budget it is
//!   killed, and the caller grades that as the child's failure, named.
//! - [`watch_self`]: the gate's own process, whose threads each own a `Db`
//!   and cannot be killed one by one. Over its budget the gate exits with
//!   a message naming the bound, before the kernel picks something else.
//!
//! RSS is read with `ps`, which every Unix the gates run on has; xtask links
//! no `libc`. A sample every half second costs nothing next to a compile.

use std::io::Read as _;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// A child's budget.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub wall: Duration,
    pub rss_mb: u64,
}

impl Limits {
    /// One compile, the worker included: 1.8 GB and 84 s CPU at the most
    /// (`array.k`, alone on a quiet machine); eight run at once under the
    /// gates' other stages. `KG_CHILD_MEM_MB` and `KG_CHILD_SECS` override.
    pub(crate) fn compile() -> Limits {
        Limits { wall: Duration::from_secs(env_u64("KG_CHILD_SECS", 900)), rss_mb: env_u64("KG_CHILD_MEM_MB", 8192) }
    }
}

/// Why a child was stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Over {
    Time(Duration),
    Memory(u64),
}

impl std::fmt::Display for Over {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Over::Time(d) => write!(f, "killed after {} s, over its time budget (KG_CHILD_SECS)", d.as_secs()),
            Over::Memory(mb) => write!(f, "killed at {mb} MB resident, over its memory budget (KG_CHILD_MEM_MB)"),
        }
    }
}

/// `command.output()`, bounded. The pipes are drained on their own threads
/// while this one polls, since a child blocked on a full pipe never exits.
/// `Err(over)` carries what the child wrote before it was stopped.
pub(crate) fn run(mut command: Command, limits: Limits) -> std::io::Result<Result<Output, (Over, Output)>> {
    use std::os::unix::process::CommandExt as _;
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = command.spawn()?;
    let group = child.id();
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let started = Instant::now();
    let mut sampled = started;
    let mut over = None;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let now = Instant::now();
        if now - started >= limits.wall {
            over = Some(Over::Time(limits.wall));
        } else if now - sampled >= Duration::from_millis(500) {
            sampled = now;
            let mb = group_rss_kb(group) / 1024;
            if mb > limits.rss_mb {
                over = Some(Over::Memory(mb));
            }
        }
        if over.is_some() {
            kill_group(group);
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // A grandchild still holding a pipe would block the drain; the group is
    // gone by now, so this returns.
    let output = Output { status, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() };
    Ok(match over {
        None => Ok(output),
        Some(over) => Err((over, output)),
    })
}

/// Stop this process, with a message, once its own resident set passes
/// `KG_XTASK_MEM_MB` (default 16384: twice `ptdiff`'s 7.5 GB, the largest
/// gate measured). Exit code 99, which no gate uses for a verdict.
pub(crate) fn watch_self(gate: String) {
    // A probe server answers one mutant at a time for the differential
    // fuzzer, and one mutant can run the frontend away (a 6000-mutant run
    // grew one server to 10 GB, 2026-09-24): its budget is its own, and
    // the worker replaces a server that dies.
    let limit = if gate == "probe-server" {
        env_u64("KG_PROBE_MEM_MB", 4096)
    } else {
        env_u64("KG_XTASK_MEM_MB", 16384)
    };
    let pid = std::process::id();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let mb = rss_kb(pid) / 1024;
            if mb > limit {
                eprintln!(
                    "xtask {gate}: stopped at {mb} MB resident, over its {limit} MB budget (KG_XTASK_MEM_MB) — \
                     a runaway here would have taken the machine; see xtask/src/bounded.rs"
                );
                std::process::exit(99);
            }
        }
    });
}

/// This process's resident set, in MB.
pub(crate) fn self_rss_mb() -> u64 {
    rss_kb(std::process::id()) / 1024
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn rss_kb(pid: u32) -> u64 {
    Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

/// The summed resident set of every process in `group`.
fn group_rss_kb(group: u32) -> u64 {
    let Ok(out) = Command::new("ps").args(["-Ao", "pgid=,rss="]).output() else { return 0 };
    sum_group(&String::from_utf8_lossy(&out.stdout), group)
}

fn sum_group(ps: &str, group: u32) -> u64 {
    ps.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pgid: u32 = it.next()?.parse().ok()?;
            let rss: u64 = it.next()?.parse().ok()?;
            (pgid == group).then_some(rss)
        })
        .sum()
}

/// `--` before the group: procps's `kill` (Linux) reads `-KILL -<pgid>` as
/// two options, exits 0 and signals nothing, so only the leader died and a
/// grandchild — a backend worker — outlived its budget. macOS's took it.
/// Found when CI first ran this crate's tests on Linux (2026-10-11).
fn kill_group(group: u32) {
    let _ = Command::new("kill").args(["-s", "KILL", "--", &format!("-{group}")]).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_group_sums_only_its_own_members() {
        let ps = "  100  2048\n  100   1024\n  200 999999\n garbage\n";
        assert_eq!(sum_group(ps, 100), 3072);
        assert_eq!(sum_group(ps, 300), 0);
    }

    /// The bound is load-bearing on a real process tree: a shell that
    /// spawns a grandchild allocating past the budget is stopped by memory,
    /// and the grandchild goes with it. Red with the sampling removed (the
    /// time bound fires instead) or with only the leader killed (the drain
    /// waits on the grandchild's pipe until it finishes on its own).
    #[test]
    fn a_child_tree_over_its_memory_is_stopped_with_its_grandchildren() {
        let mut cmd = Command::new("sh");
        // ~600 MB resident, held for 30 s, in a grandchild.
        cmd.args(["-c", "python3 -c 'import time; b = bytearray(600 << 20); b[::4096] = b\"x\" * len(b[::4096]); time.sleep(30)'; true"]);
        let started = Instant::now();
        let result = run(cmd, Limits { wall: Duration::from_secs(20), rss_mb: 200 }).expect("spawns");
        let Err((Over::Memory(mb), _)) = result else { panic!("expected a memory stop, got {result:?}") };
        assert!(mb > 200, "{mb}");
        assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
    }

    #[test]
    fn a_child_over_its_time_is_stopped() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo started; sleep 30"]);
        let result = run(cmd, Limits { wall: Duration::from_secs(1), rss_mb: 1 << 20 }).expect("spawns");
        let Err((Over::Time(_), output)) = result else { panic!("expected a time stop, got {result:?}") };
        assert_eq!(String::from_utf8_lossy(&output.stdout), "started\n");
    }

    #[test]
    fn a_child_within_its_budget_is_its_output() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo ok; exit 3"]);
        let output = run(cmd, Limits { wall: Duration::from_secs(10), rss_mb: 1 << 20 }).expect("spawns").expect("in budget");
        assert_eq!((String::from_utf8_lossy(&output.stdout).as_ref(), output.status.code()), ("ok\n", Some(3)));
    }
}
