//! Verilator: self-checking testbenches, and the host bridge.
//!
//! A testbench's top module is `tb`. It passes by printing a line that
//! starts `PASS`; `FAIL` and `ERROR` lines are shown as they come.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Design, Failure, Result, fresh_dir, require, run_logged};

/// Builds `srcs` (top `tb`) into `<out>/<name>/obj/tb`.
pub fn build(design: &Design, name: &str, srcs: &[Utf8PathBuf], defines: &[&str]) -> Result<Utf8PathBuf> {
    require("verilator")?;
    let dir = design.out.join(name);
    fresh_dir(&dir)?;
    // SIMULATION: upstream's mock HAL refuses to compile without it.
    run_logged(
        Command::new("verilator")
            .args(["--binary", "--timing", "-j", "0", "-DSIMULATION", "-Wno-fatal", "-Wno-lint", "-Wno-style"])
            .args(["--top-module", "tb", "-o", "tb"])
            .arg("-Mdir").arg(dir.join("obj"))
            .args(defines)
            .args(srcs),
        &dir.join("build.log"),
        &["%Error"],
    )?;
    Ok(dir.join("obj/tb"))
}

/// Builds and runs a testbench; passes on its `PASS` line.
pub fn check(design: &Design, name: &str, srcs: &[Utf8PathBuf], defines: &[&str], log: &mut dyn Write) -> Result<()> {
    let tb = build(design, name, srcs, defines)?;
    let run_log = design.out.join(name).join("run.log");
    let mut child = Command::new(&tb)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| Failure::Failed(format!("cannot run {tb}: {e}")))?;
    let mut kept = std::fs::File::create(&run_log).map_err(|e| Failure::Failed(format!("{run_log}: {e}")))?;
    let mut passed = false;
    for line in std::io::BufReader::new(child.stdout.take().expect("piped")).lines() {
        let Ok(line) = line else { break };
        let _ = writeln!(kept, "{line}");
        if verdict_line(&line) {
            let _ = writeln!(log, "{line}");
        }
        passed |= line.starts_with("PASS");
    }
    let status = child.wait().map_err(|e| Failure::Failed(format!("{tb}: {e}")))?;
    if passed && status.success() {
        Ok(())
    } else {
        Err(Failure::Failed(format!("{name}: the testbench did not pass ({run_log})")))
    }
}

fn verdict_line(line: &str) -> bool {
    ["PASS", "FAIL", "ERROR"].iter().any(|w| line.starts_with(w)) || line.contains("%Error")
}

/// `kg board serve`: the core under the board's bridge testbench, its I/O
/// on the host — for a serial port, a pseudo-terminal whose path the bridge
/// writes to the file named by `+pty_path_file`. The path is printed and
/// passed to `command` as `BOARD_PTY`, and a front panel's address, where
/// the bridge has one, as `BOARD_PANEL`; with a command, the simulation stops
/// when it does, and its status is the result. Without one, the simulation
/// runs, its output shown, until interrupted.
pub fn serve(design: &Design, command: &[String], log: &mut dyn Write) -> Result<i32> {
    let bridge: Vec<Utf8PathBuf> = design.board.spec.board.bridge.iter().map(|p| design.board.file(p)).collect();
    if bridge.is_empty() {
        return Err(Failure::Usage(format!("the board {} has no bridge for `serve`", design.board.name())));
    }
    let srcs = design.simulation_sources(&bridge)?;
    // The bridge's UART at the board's rate.
    let spec = &design.board.spec;
    let clks = spec.resources.values().find(|r| r.kind == "uart").zip(spec.clock.mhz).map(|(uart, mhz)| {
        let baud = u64::from(uart.baud.unwrap_or(115_200));
        format!("-GCLKS_PER_BIT={}", (mhz as u64 * 1_000_000 + baud / 2) / baud)
    });
    let defines: Vec<&str> = clks.iter().map(String::as_str).collect();
    let tb = build(design, "serve", &srcs, &defines)?;
    let dir = design.out.join("serve");
    let pty_file = dir.join("pty");
    let run_log = dir.join("run.log");
    let output = if command.is_empty() {
        Stdio::inherit()
    } else {
        Stdio::from(std::fs::File::create(&run_log).map_err(|e| Failure::Failed(format!("{run_log}: {e}")))?)
    };
    let panel_file = dir.join("panel");
    let page = design.board.platform.join("sim/vboard.html");
    let mut sim = Command::new(&tb)
        .arg(format!("+pty_path_file={pty_file}"))
        .arg(format!("+vboard_page={page}"))
        .arg(format!("+vboard_port_file={panel_file}"))
        .stdout(output)
        .spawn()
        .map_err(|e| Failure::Failed(format!("cannot run {tb}: {e}")))?;
    let Some(pty) = wait_for(&pty_file, Duration::from_secs(30)) else {
        let _ = sim.kill();
        let _ = sim.wait();
        return Err(Failure::Failed(format!("the simulation opened no pseudo-terminal ({run_log})")));
    };
    // A bridge with a front panel (platform/sim/vboard.cpp) opens it next.
    let panel = if bridge.iter().any(|p| p.file_name() == Some("vboard.cpp")) {
        wait_for(&panel_file, Duration::from_secs(10)).map(|port| format!("http://127.0.0.1:{port}/"))
    } else {
        None
    };
    if let Some(url) = &panel {
        let _ = writeln!(log, "front panel: {url}");
    }
    let Some((program, args)) = command.split_first() else {
        let _ = writeln!(log, "serving on {pty} (BOARD_PTY); ^C stops");
        let _ = log.flush();
        let status = sim.wait().map_err(|e| Failure::Failed(format!("{tb}: {e}")))?;
        return Ok(status.code().unwrap_or(1));
    };
    let _ = writeln!(log, "serving on {pty}: {}", command.join(" "));
    let _ = log.flush();
    let mut child = Command::new(program);
    child.args(args).env("BOARD_PTY", &pty);
    if let Some(url) = &panel {
        child.env("BOARD_PANEL", url);
    }
    let status = child.status();
    let _ = sim.kill();
    let _ = sim.wait();
    let status = status.map_err(|e| Failure::Usage(format!("cannot run {program}: {e}")))?;
    Ok(status.code().unwrap_or(1))
}

/// The first line of `file`, once something has written it.
fn wait_for(file: &Utf8Path, limit: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Ok(text) = std::fs::read_to_string(file)
            && let Some(line) = text.lines().next().filter(|l| !l.is_empty())
        {
            return Some(line.to_owned());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_and_verilator_errors_are_shown() {
        assert!(verdict_line("PASS: upduino monitor (261 reply bytes)"));
        assert!(verdict_line("ERROR reply byte 3: 41, want 42"));
        assert!(verdict_line("%Error: tb.sv:12: assertion failed"));
        assert!(!verdict_line("led 010"));
    }
}
