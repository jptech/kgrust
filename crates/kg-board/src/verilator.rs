//! Verilator: self-checking testbenches, and the host bridge.
//!
//! A testbench's top module is `tb`. It passes by printing a line that
//! starts `PASS`; `FAIL` and `ERROR` lines are shown as they come.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Design, Failure, Result, fresh_dir, require, run_logged};

/// How a testbench is clocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// It clocks itself with `#` delays, under `--timing`.
    Own,
    /// `platform/sim/main.cpp` drives its `clk` port: no `--timing`.
    Port,
}

/// Builds `srcs` (top `tb`) into `<out>/<name>/obj/tb`.
pub fn build(design: &Design, name: &str, srcs: &[Utf8PathBuf], defines: &[&str], clock: Clock) -> Result<Utf8PathBuf> {
    require("verilator")?;
    let dir = design.out.join(name);
    fresh_dir(&dir)?;
    let mode: &[&str] = match clock {
        Clock::Own => &["--binary", "--timing"],
        Clock::Port => &["--cc", "--exe", "--build"],
    };
    // SIMULATION: upstream's mock HAL refuses to compile without it.
    run_logged(
        Command::new("verilator")
            .args(mode)
            .args(["-j", "0", "-DSIMULATION", "-Wno-fatal", "-Wno-lint", "-Wno-style"])
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
    let tb = build(design, name, srcs, defines, Clock::Own)?;
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

/// `kg board serve`: the core with its I/O on the host, under the board's
/// own bridge testbench where `board.toml` names one, else the testbench
/// generated from its resources ([`crate::bench`]). A serial port is a
/// pseudo-terminal whose path the testbench writes to the file named by
/// `+pty_path_file`; it is printed and passed to `command` as `BOARD_PTY`.
/// The front panel's address, where there is one, is printed and passed as
/// `BOARD_PANEL`. With a command, the simulation stops when it does, and
/// its status is the result; without one, the simulation runs, its output
/// shown, until interrupted.
pub fn serve(design: &Design, command: &[String], log: &mut dyn Write) -> Result<i32> {
    let board = design.board;
    let own: Vec<Utf8PathBuf> = board.spec.board.bridge.iter().map(|p| board.file(p)).collect();
    let mut description = None;
    let mut has_uart = !own.is_empty();
    let (bridge, defines, clock) = if own.is_empty() {
        let bench = crate::bench::write(design)?;
        description = Some(bench.description);
        has_uart = bench.pty;
        (bench.files, Vec::new(), Clock::Port)
    } else {
        // A board's own bridge takes its UART's rate as a parameter.
        let spec = &board.spec;
        let clks = spec.resources.values().find(|r| r.kind == "uart").zip(spec.clock.mhz).map(|(uart, mhz)| {
            let baud = u64::from(uart.baud.unwrap_or(115_200));
            format!("-GCLKS_PER_BIT={}", (mhz as u64 * 1_000_000 + baud / 2) / baud)
        });
        (own, clks.into_iter().collect(), Clock::Own)
    };
    let srcs = design.simulation_sources(&bridge)?;
    let defines: Vec<&str> = defines.iter().map(String::as_str).collect();
    let tb = build(design, "serve", &srcs, &defines, clock)?;
    let dir = design.out.join("serve");
    let pty_file = dir.join("pty");
    let panel_file = dir.join("panel");
    let page = board.platform.join("sim/vboard.html");
    let run_log = dir.join("run.log");
    let has_panel = bridge.iter().any(|p| p.file_name() == Some("vboard.cpp"));
    let interactive = command.is_empty();
    // Starts the simulation, and waits for its pseudo-terminal and panel;
    // `port` keeps the panel's address across a restart.
    let start = |port: Option<&str>| -> Result<(std::process::Child, Option<String>, Option<String>)> {
        let _ = std::fs::remove_file(&pty_file);
        let _ = std::fs::remove_file(&panel_file);
        let output = if interactive {
            Stdio::inherit()
        } else {
            Stdio::from(std::fs::File::create(&run_log).map_err(|e| Failure::Failed(format!("{run_log}: {e}")))?)
        };
        let mut sim = Command::new(&tb);
        sim.arg(format!("+pty_path_file={pty_file}")).arg(format!("+vboard_page={page}")).arg(format!("+vboard_port_file={panel_file}"));
        if let Some(description) = &description {
            sim.arg(format!("+vboard_description={description}"));
        }
        if let Some(port) = port {
            sim.arg(format!("+vboard_port={port}"));
        }
        // Only `serve` alone restarts: a command's BOARD_PTY would go stale.
        if interactive {
            sim.arg("+vboard_restart");
        }
        let mut sim = sim.stdout(output).spawn().map_err(|e| Failure::Failed(format!("cannot run {tb}: {e}")))?;
        let pty = if has_uart { wait_for(&pty_file, Duration::from_secs(30)) } else { None };
        let port = if has_panel { wait_for(&panel_file, Duration::from_secs(30)) } else { None };
        if (has_uart && pty.is_none()) || (has_panel && port.is_none()) {
            let _ = sim.kill();
            let _ = sim.wait();
            return Err(Failure::Failed(format!("the simulation opened neither its pseudo-terminal nor its front panel ({run_log})")));
        }
        Ok((sim, pty, port))
    };
    let (mut sim, mut pty, port) = start(None)?;
    let panel = port.as_ref().map(|port| format!("http://127.0.0.1:{port}/"));
    if let Some(url) = &panel {
        let _ = writeln!(log, "front panel: {url}");
    }
    let Some((program, args)) = command.split_first() else {
        match &pty {
            Some(pty) => {
                let _ = writeln!(log, "serving on {pty} (BOARD_PTY); ^C stops");
            }
            None => {
                let _ = writeln!(log, "serving; ^C stops");
            }
        }
        let _ = log.flush();
        loop {
            let status = sim.wait().map_err(|e| Failure::Failed(format!("{tb}: {e}")))?;
            // The page asked to start again from power-on.
            if status.code() == Some(RESTART)
                && let Some(port) = &port
            {
                let was = pty.clone();
                (sim, pty, _) = start(Some(port))?;
                let moved = pty.as_ref().filter(|p| was.as_ref() != Some(*p)).map(|p| format!("; the serial port is now {p}")).unwrap_or_default();
                let _ = writeln!(log, "restarted from power-on{moved}");
                let _ = log.flush();
                continue;
            }
            return Ok(status.code().unwrap_or(1));
        }
    };
    let _ = writeln!(log, "serving on {}: {}", pty.as_deref().unwrap_or("no serial port"), command.join(" "));
    let _ = log.flush();
    let mut child = Command::new(program);
    child.args(args);
    if let Some(pty) = &pty {
        child.env("BOARD_PTY", pty);
    }
    if let Some(url) = &panel {
        child.env("BOARD_PANEL", url);
    }
    let status = child.status();
    let _ = sim.kill();
    let _ = sim.wait();
    let status = status.map_err(|e| Failure::Usage(format!("cannot run {program}: {e}")))?;
    Ok(status.code().unwrap_or(1))
}

/// The simulation's status when the front panel asks to restart from
/// power-on (`platform/sim/vboard.cpp`).
const RESTART: i32 = 75;

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
