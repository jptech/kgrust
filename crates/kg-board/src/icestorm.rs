//! The open iCE40 flow: sv2v, Yosys, nextpnr-ice40, icepack.
//!
//! Results land in `target/board/icestorm/`.

use std::io::Write;
use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Design, Failure, Result, fresh_dir, repair, require, run_logged, write};

/// Yosys's iCE40 cell models (`SB_SPRAM256KA`, `SB_LUT4`, …), which
/// Verilator runs: for primitives a design instantiates, and for netlists.
pub fn cell_models() -> Result<Utf8PathBuf> {
    require("yosys-config")?;
    let out = Command::new("yosys-config")
        .arg("--datdir")
        .output()
        .map_err(|e| Failure::Usage(format!("cannot run yosys-config: {e}")))?;
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let models = Utf8PathBuf::from(dir).join("ice40/cells_sim.v");
    if models.is_file() { Ok(models) } else { Err(Failure::Usage(format!("no iCE40 cell models at {models}"))) }
}

/// The design's sources, translate-off regions cut, as one Verilog file for
/// Yosys: sv2v's reading of the SystemVerilog Yosys reads only in part.
fn convert(design: &Design, dir: &Utf8Path, top: &str, tag: &str) -> Result<Utf8PathBuf> {
    for tool in ["sv2v", "yosys"] {
        require(tool)?;
    }
    let strip = dir.join("strip");
    fresh_dir(&strip)?;
    let mut stripped = Vec::new();
    // Numbered in order, which keeps two sources of one name apart.
    for (n, src) in design.synthesis_sources()?.iter().enumerate() {
        let text = std::fs::read_to_string(src).map_err(|e| Failure::Usage(format!("{src}: {e}")))?;
        let kept = repair::strip_translate_off(&text).map_err(|e| Failure::Failed(format!("{src}: {e}")))?;
        let path = strip.join(format!("{n:03}_{}", src.file_name().unwrap_or("source.sv")));
        write(&path, &kept)?;
        stripped.push(path);
    }
    let verilog = dir.join(format!("{tag}.v"));
    // SYNTHESIS: the runtime's `ifndef SYNTHESIS` simulation paths stay out.
    run_logged(
        Command::new("sv2v").arg("-DSYNTHESIS").arg(format!("--top={top}")).args(&stripped).arg("-w").arg(&verilog),
        &dir.join(format!("{tag}.sv2v.log")),
        &["rror"],
    )?;
    Ok(verilog)
}

/// Yosys's synthesis of `top`. The backend decides where every memory
/// lives, and the iCE40 HAL marks its memories accordingly; any memory left
/// unmarked (the runtime's internal register FIFOs) is logic, so Yosys never
/// spends an EBR the backend did not budget. No `-dsp`: every product the
/// backend makes is the HAL's KanagawaLutMul, which instantiates SB_MAC16
/// itself, and Yosys 0.69's ice40_dsp crashes on such a block followed by
/// two registers, the backend's own pipeline (`hal_lutmul.sv`). A `*` in
/// hand-written RTL is therefore logic. `extra` follows the synthesis.
fn synthesize(design: &Design, dir: &Utf8Path, verilog: &Utf8Path, top: &str, chparam: &str, extra: &str, tag: &str) -> Result<Utf8PathBuf> {
    let stat = dir.join(format!("{tag}.stat.txt"));
    let script = format!(
        "read_verilog -DSYNTHESIS {verilog}\n\
         hierarchy -top {top} {chparam}\n\
         setattr -set ram_style \"logic\" m:* a:ram_style %d\n\
         synth_ice40 -top {top}\n\
         {extra}\n\
         tee -q -o {stat} stat\n"
    );
    // Memory initialisation files are named relative to the generated RTL.
    run_logged(
        Command::new("yosys").arg("-q").arg("-l").arg(dir.join(format!("{tag}.yosys.log"))).arg("-p").arg(&script).current_dir(Design::gen_dir(&design.out)),
        &dir.join(format!("{tag}.yosys.out")),
        &["ERROR"],
    )
    .map_err(|_| Failure::Failed(format!("yosys failed:\n{}", quote(&dir.join(format!("{tag}.yosys.log")), "ERROR"))))?;
    Ok(stat)
}

fn quote(log: &Utf8Path, pattern: &str) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| l.contains(pattern)).take(5).collect();
    format!("{}\n({log})", lines.join("\n"))
}

/// The gate-level netlist of `top` (the core): the bitstream's synthesis,
/// so a simulation of it grades what the board gets.
pub fn netlist(design: &Design, top: &str, log: &mut dyn Write) -> Result<Utf8PathBuf> {
    let dir = design.out.join("icestorm");
    std::fs::create_dir_all(&dir).map_err(|e| Failure::Failed(format!("{dir}: {e}")))?;
    let verilog = convert(design, &dir, top, "netlist")?;
    let netlist = dir.join("netlist.v");
    synthesize(design, &dir, &verilog, top, "", &format!("write_verilog -noattr {netlist}"), "netlist")?;
    let _ = writeln!(log, "netlist: {netlist} (top {top})");
    Ok(netlist)
}

/// The bitstream: the board's top, its clock parameter set from `[clock]`,
/// placed and routed for the board's part and package and constraints.
pub fn bitstream(design: &Design, log: &mut dyn Write) -> Result<Utf8PathBuf> {
    require("nextpnr-ice40")?;
    require("icepack")?;
    let fpga = &design.board.spec.fpga;
    let top = design.top()?.to_owned();
    let dir = design.out.join("icestorm");
    std::fs::create_dir_all(&dir).map_err(|e| Failure::Failed(format!("{dir}: {e}")))?;
    let clock = &design.board.spec.clock;
    let chparam = match (&clock.parameter, clock.mhz) {
        (Some(param), Some(mhz)) => format!("-chparam {param} {mhz}"),
        _ => String::new(),
    };
    let verilog = convert(design, &dir, &top, "design")?;
    let json = dir.join("design.json");
    synthesize(design, &dir, &verilog, &top, &chparam, &format!("write_json {json}"), "design")?;

    let pcf = design.board.spec.constraints.files.first().map(|p| design.board.file(p)).ok_or_else(|| {
        Failure::Usage(format!("the board {} lists no constraints", design.board.name()))
    })?;
    let (asc, report, bin) = (dir.join("design.asc"), dir.join("report.json"), dir.join("design.bin"));
    let mhz = clock.mhz.unwrap_or(12);
    run_logged(
        Command::new("nextpnr-ice40")
            .arg(format!("--{}", fpga.part))
            .args(["--package", &fpga.package])
            .arg("--json").arg(&json)
            .arg("--pcf").arg(&pcf)
            .arg("--asc").arg(&asc)
            .args(["--freq", &mhz.to_string()])
            .arg("--report").arg(&report)
            .arg("--log").arg(dir.join("nextpnr.log"))
            .arg("--quiet"),
        &dir.join("nextpnr.out"),
        &["ERROR", "FAIL"],
    )?;
    run_logged(Command::new("icepack").arg(&asc).arg(&bin), &dir.join("icepack.log"), &["rror"])?;
    let text = std::fs::read_to_string(&report).map_err(|e| Failure::Failed(format!("{report}: {e}")))?;
    let _ = write!(log, "{}", summary(&text, mhz as f64));
    let _ = writeln!(log, "bitstream: {bin}");
    Ok(bin)
}

/// nextpnr's report as the lines a reader wants: the parts used of each
/// resource, and each clock's Fmax against the board's.
fn summary(report: &str, target: f64) -> String {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(report) else { return String::new() };
    let mut out = String::new();
    for kind in ["ICESTORM_LC", "ICESTORM_RAM", "ICESTORM_SPRAM", "ICESTORM_DSP", "SB_IO", "SB_GB"] {
        let u = &json["utilization"][kind];
        let (Some(used), Some(available)) = (u["used"].as_u64(), u["available"].as_u64()) else { continue };
        if used > 0 && available > 0 {
            out.push_str(&format!(
                "  {kind:<15} {used:>5} / {available:<5} ({:.1}%)\n",
                100.0 * used as f64 / available as f64
            ));
        }
    }
    if let Some(fmax) = json["fmax"].as_object() {
        for (clock, f) in fmax {
            let Some(achieved) = f["achieved"].as_f64() else { continue };
            let verdict = if achieved >= target { "meets" } else { "MISSES" };
            out.push_str(&format!("  fmax {clock}: {achieved:.1} MHz, {verdict} {target} MHz\n"));
        }
    }
    out
}

/// The cells of `top` after synthesis, in one line.
pub fn area(design: &Design, top: &str, log: &mut dyn Write) -> Result<String> {
    let dir = design.out.join("icestorm");
    std::fs::create_dir_all(&dir).map_err(|e| Failure::Failed(format!("{dir}: {e}")))?;
    let verilog = convert(design, &dir, top, "area")?;
    let stat = synthesize(design, &dir, &verilog, top, "", "", "area")?;
    let text = std::fs::read_to_string(&stat).map_err(|e| Failure::Failed(format!("{stat}: {e}")))?;
    let line = format!("{top}: {}", cells(&text));
    let _ = writeln!(log, "{line}");
    Ok(line)
}

/// Yosys's `stat` as the cells that cost: LUT4s, flip-flops of every kind,
/// carries, and the hard blocks.
fn cells(stat: &str) -> String {
    let mut counts = std::collections::BTreeMap::<&str, u64>::new();
    for line in stat.lines() {
        let mut words = line.split_whitespace();
        if let (Some(n), Some(cell)) = (words.next(), words.next())
            && cell.starts_with("SB_")
            && let Ok(n) = n.parse::<u64>()
        {
            *counts.entry(cell).or_default() += n;
        }
    }
    let ff: u64 = counts.iter().filter(|(c, _)| c.starts_with("SB_DFF")).map(|(_, n)| n).sum();
    let get = |cell: &str| counts.get(cell).copied().unwrap_or(0);
    format!(
        "LUT4 {}  FF {ff}  carry {}  EBR {}  SPRAM {}  DSP {}",
        get("SB_LUT4"),
        get("SB_CARRY"),
        get("SB_RAM40_4K"),
        get("SB_SPRAM256KA"),
        get("SB_MAC16")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of `nextpnr-ice40 --report` (0.7): a resource unused is left
    /// out, and Fmax is judged against the board's clock.
    #[test]
    fn the_report_summary_names_what_is_used_and_judges_fmax() {
        let report = r#"{"utilization": {"ICESTORM_LC": {"used": 380, "available": 5280},
            "ICESTORM_RAM": {"used": 0, "available": 30}, "ICESTORM_SPRAM": {"used": 2, "available": 4}},
            "fmax": {"clk": {"achieved": 42.8, "constraint": 24.0}, "slow": {"achieved": 20.0, "constraint": 24.0}}}"#;
        let text = summary(report, 24.0);
        assert!(text.contains("ICESTORM_LC       380 / 5280  (7.2%)"), "{text}");
        assert!(text.contains("ICESTORM_SPRAM      2 / 4     (50.0%)"), "{text}");
        assert!(!text.contains("ICESTORM_RAM "), "{text}");
        assert!(text.contains("fmax clk: 42.8 MHz, meets 24 MHz"), "{text}");
        assert!(text.contains("fmax slow: 20.0 MHz, MISSES 24 MHz"), "{text}");
    }

    /// Yosys 0.69's `stat`, count before cell: every flip-flop kind counts.
    #[test]
    fn cells_count_flip_flops_of_every_kind() {
        let stat = "      327 cells\n       12   SB_CARRY\n       20   SB_DFF\n      100   SB_DFFE\n       25   SB_DFFESR\n      147   SB_LUT4\n        2   SB_SPRAM256KA\n";
        assert_eq!(cells(stat), "LUT4 147  FF 145  carry 12  EBR 0  SPRAM 2  DSP 0");
    }
}
