//! The flows behind `kg board`: what happens to a design's generated RTL
//! once `kg build` has written it for a board (`docs/plans/board-tooling.md`).
//!
//! - [`sim`]: the project's testbench under Verilator, against upstream's
//!   portable runtime, the board's simulation HAL and the board's RTL.
//! - [`netsim`]: the same testbench against the gate-level netlist the
//!   vendor flow makes of the design's core.
//! - [`bitstream`] and [`program`]: the vendor flow, then the programmer.
//! - [`serve`]: the core in simulation with its I/O bridged to the host (a
//!   pseudo-terminal for a serial port), for host programs to talk to.
//! - [`area`]: the core's cells after synthesis.
//!
//! Every external tool runs as a child process with its output in a log
//! beside its results, under the project's `target/board/`. A missing tool
//! is said by name before anything runs.

pub mod bench;
pub mod icestorm;
pub mod interface;
pub mod locate;
pub mod python;
pub mod repair;
pub mod scaffold;
pub mod verilator;
pub mod vivado;
pub mod wrapper;

use std::io::Write;
use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use kg_config::Board;
pub use locate::which;

/// Why a flow stopped. Each maps to `kg`'s exit codes.
#[derive(Debug)]
pub enum Failure {
    /// The design failed: a testbench, a tool's verdict on it. Exit 1.
    Failed(String),
    /// The request cannot run: no testbench, no tool. Exit 2.
    Usage(String),
}

impl Failure {
    pub fn exit_code(&self) -> i32 {
        match self {
            Failure::Failed(_) => 1,
            Failure::Usage(_) => 2,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Failure::Failed(m) | Failure::Usage(m) => m,
        }
    }
}

pub type Result<T> = std::result::Result<T, Failure>;

/// A design built for a board: what the flows read.
#[derive(Debug)]
pub struct Design<'a> {
    pub board: &'a Board,
    /// Upstream's `runtime/rtl`: the portable runtime and its HALs.
    pub runtime: Utf8PathBuf,
    /// The project's `target/board`.
    pub out: Utf8PathBuf,
    /// The generated RTL, `*_types.sv` first, repaired ([`repair`]).
    pub generated: Vec<Utf8PathBuf>,
    /// The exported class's interface, or why it could not be described.
    pub interface: std::result::Result<interface::Interface, String>,
    /// The generated core ([`wrapper`]), when the project supplies none.
    pub core: Option<wrapper::Core>,
}

impl Design<'_> {
    /// The directory `kg build` writes into.
    pub fn gen_dir(out: &Utf8Path) -> Utf8PathBuf {
        out.join("gen")
    }

    /// The generated RTL in a build directory, packages first, as the HAL's
    /// own `files.f` orders them.
    pub fn collect_generated(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let mut files: Vec<Utf8PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
            .filter(|p| p.extension() == Some("sv"))
            .collect();
        files.sort_by_key(|p| (!p.as_str().ends_with("_types.sv"), p.clone()));
        files
    }

    /// A HAL: the platform's, else the reference runtime's.
    pub fn hal_dir(&self, hal: &str) -> Utf8PathBuf {
        let own = self.board.platform.join("rtl/hal").join(hal);
        if own.is_dir() { own } else { self.runtime.join("hal").join(hal) }
    }

    /// The board's RTL, the platform's peripheral engines, and the
    /// generated core where there is one.
    fn board_rtl(&self) -> Result<Vec<Utf8PathBuf>> {
        let mut rtl: Vec<Utf8PathBuf> = self.board.spec.board.rtl.iter().map(|p| self.board.file(p)).collect();
        let periph = self.board.platform.join("rtl/periph");
        if periph.join("files.f").is_file() {
            rtl.extend(files_f(&periph)?);
        }
        if self.core.is_some() {
            rtl.push(self.core_file());
        }
        Ok(rtl)
    }

    /// Where the generated core is written.
    pub fn core_file(&self) -> Utf8PathBuf {
        let name = self.core.as_ref().map_or("core", |c| c.module.as_str());
        self.out.join("core").join(format!("{name}.sv"))
    }

    /// The interface, for a flow that needs it.
    pub fn interface(&self) -> Result<&interface::Interface> {
        self.interface.as_ref().map_err(|e| Failure::Failed(e.clone()))
    }

    /// What synthesis reads: the runtime, the HAL, the generated RTL, the
    /// board's RTL and the project's, each file once.
    pub fn synthesis_sources(&self) -> Result<Vec<Utf8PathBuf>> {
        let mut srcs = files_f(&self.runtime)?;
        srcs.extend(files_f(&self.hal_dir(&self.board.spec.fpga.hal))?);
        srcs.extend(self.generated.iter().cloned());
        srcs.extend(self.board_rtl()?);
        srcs.extend(self.board.project.rtl.iter().cloned());
        Ok(dedup(srcs))
    }

    /// What a simulation of the RTL reads: the vendor's cell models, the
    /// runtime with its simulation support, the simulation HAL, the
    /// generated RTL, the board's and the project's RTL, then `extra` (a
    /// testbench, a bridge).
    pub fn simulation_sources(&self, extra: &[Utf8PathBuf]) -> Result<Vec<Utf8PathBuf>> {
        let mut srcs = self.cell_models()?;
        srcs.extend(files_f(&self.runtime)?);
        srcs.extend(files_f(&self.hal_dir(self.board.sim_hal()))?);
        srcs.extend(files_f(&self.runtime.join("sim"))?);
        srcs.extend(self.generated.iter().cloned());
        srcs.extend(self.board_rtl()?);
        srcs.extend(self.board.project.rtl.iter().cloned());
        srcs.extend(extra.iter().cloned());
        Ok(dedup(srcs))
    }

    /// The vendor's simulation models of its primitives: Yosys's for a
    /// board on the iCE40 HAL, which instantiates them (the virtual board
    /// too, which has no flow of its own); none needed for the others yet.
    pub fn cell_models(&self) -> Result<Vec<Utf8PathBuf>> {
        let fpga = &self.board.spec.fpga;
        if fpga.tool == "icestorm" || fpga.hal == "ice40" || self.board.sim_hal() == "ice40" {
            Ok(vec![icestorm::cell_models()?])
        } else {
            Ok(Vec::new())
        }
    }

    /// The vendor flow's inputs beside the generated RTL: `sources.f`,
    /// `constraints.f`, `board.env` and `board.tcl`, for a flow run by hand
    /// or on another machine.
    pub fn bundle(&self) -> Result<()> {
        let fpga = &self.board.spec.fpga;
        let top = self.top()?;
        let lines = |paths: &[Utf8PathBuf]| paths.iter().map(|p| format!("{p}\n")).collect::<String>();
        let constraints: Vec<Utf8PathBuf> = self.board.spec.constraints.files.iter().map(|p| self.board.file(p)).collect();
        let mhz = self.board.spec.clock.mhz.map(|m| m.to_string()).unwrap_or_default();
        let param = self.board.spec.clock.parameter.clone().unwrap_or_default();
        write(&self.out.join("sources.f"), &lines(&self.synthesis_sources()?))?;
        write(&self.out.join("constraints.f"), &lines(&constraints))?;
        write(
            &self.out.join("board.env"),
            &format!("part={}\npackage={}\ntop={top}\nmhz={mhz}\nclock_param={param}\n", fpga.part, fpga.package),
        )?;
        write(&self.out.join("board.tcl"), &format!("set part {}\nset top {top}\n", fpga.part))
    }

    pub fn top(&self) -> Result<&str> {
        self.board
            .top_module()
            .ok_or_else(|| Failure::Usage(format!("the board {} names no top-module; set one in [board]", self.board.name())))
    }

    pub fn core(&self) -> Result<&str> {
        self.board.core_module().ok_or_else(|| {
            Failure::Usage(format!("the board {} names no core-module; set one in [board]", self.board.name()))
        })
    }

    pub fn testbench(&self) -> Result<Utf8PathBuf> {
        self.board
            .project
            .testbench
            .clone()
            .ok_or_else(|| {
                Failure::Usage("no testbench; name one in [board] testbench, or test with cocotb: `kg board test` runs test_*.py".to_owned())
            })
    }
}

/// What a build leaves for the flows beyond its RTL: the interface
/// description, written to `interface.json`, and the generated core when
/// the board has resources and the project names no core of its own. The
/// bindings are logged. A description that fails is kept as the reason, and
/// is an error only for what needs it.
pub fn describe(
    board: &Board,
    out: &Utf8Path,
    generated: &[Utf8PathBuf],
    sources: &[Utf8PathBuf],
    log: &mut dyn Write,
) -> Result<(std::result::Result<interface::Interface, String>, Option<wrapper::Core>)> {
    let interface = match interface::module_name(generated) {
        None => Err("the build wrote no `<Module>_types.sv`; is a class exported?".to_owned()),
        Some(module) => {
            let path = Design::gen_dir(out).join(format!("{module}.sv"));
            let text = std::fs::read_to_string(&path).map_err(|e| Failure::Failed(format!("{path}: {e}")))?;
            let sources = interface::read_sources(sources)?;
            interface::describe(&module, &text, &sources).map_err(|f| f.message().to_owned())
        }
    };
    if let Ok(interface) = &interface {
        interface::write(interface, out)?;
    }
    let wanted = board.core_module().filter(|core| !board.spec.resources.is_empty() && !defines(&board.project.rtl, core));
    let Some(core_module) = wanted else {
        if let Ok(interface) = &interface {
            python::write(interface, None, board.spec.clock.mhz, &board.platform, out)?;
        }
        return Ok((interface, None));
    };
    let described = interface.as_ref().map_err(|e| {
        Failure::Failed(format!("{e}\n(the project names no `{core_module}` in [board] rtl, so kg board generates one from the interface)"))
    })?;
    let core = wrapper::generate(board, described)?;
    let path = out.join("core").join(format!("{core_module}.sv"));
    std::fs::create_dir_all(out.join("core")).map_err(|e| Failure::Failed(format!("{path}: {e}")))?;
    write(&path, &core.text)?;
    let _ = writeln!(log, "core: {path}, generated for {}:\n{}", described.module, core.summary().trim_end());
    let py = python::write(described, core.bridge.as_ref(), board.spec.clock.mhz, &board.platform, out)?;
    if core.bridge.is_some() {
        let _ = writeln!(log, "python: {py}");
    }
    Ok((interface, Some(core)))
}

/// Whether any of `files` declares `module <name>`.
fn defines(files: &[Utf8PathBuf], name: &str) -> bool {
    files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).any(|text| {
        text.lines().any(|line| {
            let mut words = line.split(|c: char| c.is_whitespace() || c == '(' || c == '#' || c == ';').filter(|w| !w.is_empty());
            words.next() == Some("module") && words.next() == Some(name)
        })
    })
}

/// `kg board sim`: the testbench against the RTL. Passes on its `PASS` line.
pub fn sim(design: &Design, log: &mut dyn Write) -> Result<()> {
    let tb = design.testbench()?;
    let srcs = design.simulation_sources(&[tb])?;
    verilator::check(design, "sim", &srcs, &[], log)
}

/// `kg board netsim`: the testbench against the core's gate-level netlist,
/// with `NETSIM` defined for a testbench that must reach into it.
pub fn netsim(design: &Design, log: &mut dyn Write) -> Result<()> {
    let tb = design.testbench()?;
    let core = design.core()?.to_owned();
    let netlist = match design.board.spec.fpga.tool.as_str() {
        "icestorm" => icestorm::netlist(design, &core, log)?,
        "virtual" => return Err(simulated_only(design, "netsim")),
        tool => return Err(Failure::Usage(format!("netsim needs an open flow; {tool} has none here"))),
    };
    let mut srcs = design.cell_models()?;
    srcs.extend([netlist, tb]);
    verilator::check(design, "netsim", &srcs, &["-DNETSIM"], log)
}

/// `kg board bitstream`.
pub fn bitstream(design: &Design, log: &mut dyn Write) -> Result<Utf8PathBuf> {
    if design.board.spec.fpga.tool == "virtual" {
        return Err(simulated_only(design, "bitstream"));
    }
    design.bundle()?;
    match design.board.spec.fpga.tool.as_str() {
        "icestorm" => icestorm::bitstream(design, log),
        "vivado" => vivado::bitstream(design, log),
        "virtual" => Err(simulated_only(design, "bitstream")),
        tool => Err(Failure::Usage(format!("no flow for the tool `{tool}`"))),
    }
}

/// `kg board flash`: the last bitstream into the board.
pub fn program(design: &Design, log: &mut dyn Write) -> Result<()> {
    match design.board.spec.fpga.tool.as_str() {
        "icestorm" => {
            let bin = design.out.join("icestorm/design.bin");
            let command = &design.board.spec.program.command;
            let Some((program, args)) = command.split_first() else {
                return Err(Failure::Usage(format!("the board {} has no [program] command", design.board.name())));
            };
            if !bin.is_file() {
                return Err(Failure::Usage("no bitstream; run `kg board bitstream` first".to_owned()));
            }
            require(program)?;
            let _ = writeln!(log, "flash: {} {bin}", command.join(" "));
            let status = Command::new(program).args(args).arg(&bin).status().map_err(|e| spawn_failure(program, e))?;
            if status.success() { Ok(()) } else { Err(Failure::Failed(format!("{program} failed ({status})"))) }
        }
        "vivado" => vivado::program(design, log),
        "virtual" => Err(simulated_only(design, "flash")),
        tool => Err(Failure::Usage(format!("no programmer for the tool `{tool}`"))),
    }
}

/// `kg board test`: the project's cocotb test modules against the generated
/// module itself, under Verilator, every function reachable whatever it is
/// bound to on the board. `python` must have cocotb 2; the test modules
/// import the generated Python module, which drives the ports
/// (`platform/python/kgbridge.py`, `Cocotb`).
pub fn test(design: &Design, python: &str, log: &mut dyn Write) -> Result<()> {
    let interface = design.interface()?;
    let root = design.out.parent().and_then(Utf8Path::parent).map(Utf8Path::to_owned).unwrap_or_default();
    let tests = if design.board.project.tests.is_empty() {
        let mut found: Vec<Utf8PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
            .filter(|p| p.file_name().is_some_and(|n| n.starts_with("test_") && n.ends_with(".py")))
            .collect();
        found.sort();
        found
    } else {
        design.board.project.tests.clone()
    };
    let Some(first) = tests.first() else {
        return Err(Failure::Usage(format!("no cocotb tests: no test_*.py in {root}, and [board] tests names none")));
    };
    let test_dir = first.parent().map(Utf8Path::to_owned).unwrap_or_else(|| root.clone());
    if let Some(stray) = tests.iter().find(|t| t.parent() != Some(test_dir.as_path())) {
        return Err(Failure::Usage(format!("cocotb runs test modules from one directory; {stray} is not in {test_dir}")));
    }
    require(python)?;
    let probe = Command::new(python).args(["-c", "import cocotb, cocotb_tools.runner"]).output().map_err(|e| spawn_failure(python, e))?;
    if !probe.status.success() {
        return Err(Failure::Usage(format!(
            "`{python}` has no cocotb 2: `{python} -m pip install cocotb` (or `uv pip install cocotb` in a virtual environment, then --python .venv/bin/python)"
        )));
    }
    // The design alone: no board RTL, no testbench.
    let mut srcs = design.cell_models()?;
    srcs.extend(files_f(&design.runtime)?);
    srcs.extend(files_f(&design.hal_dir(design.board.sim_hal()))?);
    srcs.extend(files_f(&design.runtime.join("sim"))?);
    srcs.extend(design.generated.iter().cloned());
    let dir = design.out.join("cocotb");
    fresh_dir(&dir)?;
    let list = dir.join("sources.f");
    write(&list, &dedup(srcs).iter().map(|p| format!("{p}\n")).collect::<String>())?;
    let mut command = Command::new(python);
    command
        .arg(design.out.join("py/kgbridge.py"))
        .arg("cocotb")
        .args(["--toplevel", &interface.module])
        .arg("--sources").arg(&list)
        .arg("--build-dir").arg(dir.join("build"))
        .arg("--test-dir").arg(&test_dir)
        .args(["--define", "SIMULATION"]);
    for t in &tests {
        command.arg("--module").arg(t.file_stem().unwrap_or_default());
    }
    let _ = writeln!(log, "cocotb: {} on {} ({dir}/run.log)", tests.iter().filter_map(|t| t.file_name()).collect::<Vec<_>>().join(", "), interface.module);
    let _ = log.flush();
    let run_log = dir.join("run.log");
    let file = std::fs::File::create(&run_log).map_err(|e| Failure::Failed(format!("{run_log}: {e}")))?;
    let err = file.try_clone().map_err(|e| Failure::Failed(format!("{run_log}: {e}")))?;
    let status = command.stdout(file).stderr(err).status().map_err(|e| spawn_failure(python, e))?;
    let text = std::fs::read_to_string(&run_log).unwrap_or_default();
    // cocotb's table of results, and the first lines that say why.
    for line in text.lines().filter(|l| l.contains("** ") && (l.contains(" PASS ") || l.contains(" FAIL ") || l.contains("TESTS="))) {
        let _ = writeln!(log, "  {}", line.trim().trim_matches('*').trim());
    }
    if status.success() {
        return Ok(());
    }
    let why: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("Error") || l.contains("assert") || l.contains("%Error"))
        .map(str::trim)
        .take(8)
        .collect();
    Err(Failure::Failed(format!("cocotb: the tests did not pass ({run_log}){}{}", if why.is_empty() { "" } else { ":\n" }, why.join("\n"))))
}

/// `kg board serve`: see [`verilator::serve`].
pub fn serve(design: &Design, command: &[String], log: &mut dyn Write) -> Result<i32> {
    verilator::serve(design, command, log)
}

/// `kg board area`: the cells of `top` (default: the core) after the vendor
/// flow's synthesis.
pub fn area(design: &Design, top: Option<&str>, log: &mut dyn Write) -> Result<String> {
    let top = match top {
        Some(top) => top.to_owned(),
        None => design.core()?.to_owned(),
    };
    match design.board.spec.fpga.tool.as_str() {
        "icestorm" => icestorm::area(design, &top, log),
        "virtual" => Err(simulated_only(design, "area")),
        tool => Err(Failure::Usage(format!("area needs an open flow; {tool} has none here"))),
    }
}

/// A board with no hardware refuses what needs some.
fn simulated_only(design: &Design, command: &str) -> Failure {
    Failure::Usage(format!(
        "the {} board is simulated only: `{command}` needs hardware; `sim`, `serve` and `test` run here (name a real board in [board] to build for one)",
        design.board.name()
    ))
}

/// The tools a board's flows run, and which are on `PATH`.
pub fn tools(board: &Board) -> Vec<(&'static str, String, bool)> {
    let mut list = vec![("sim, netsim, serve", "verilator".to_owned())];
    match board.spec.fpga.tool.as_str() {
        "icestorm" => {
            for tool in ["sv2v", "yosys", "nextpnr-ice40", "icepack"] {
                list.push(("bitstream, netsim, area", tool.to_owned()));
            }
        }
        "vivado" => list.push(("bitstream, flash", "vivado".to_owned())),
        "virtual" => list[0].0 = "sim, serve",
        _ => {}
    }
    if let Some(program) = board.spec.program.command.first() {
        list.push(("flash", program.clone()));
    }
    list.push(("test (cocotb 2)", "python3".to_owned()));
    list.into_iter().map(|(why, tool)| {
        let found = which(&tool).is_some();
        (why, tool, found)
    }).collect()
}

/// Refuses, by name, to go on without `program`.
pub fn require(program: &str) -> Result<()> {
    match which(program) {
        Some(_) => Ok(()),
        None => Err(Failure::Usage(format!("needs `{program}` on PATH ({})", install_hint(program)))),
    }
}

fn install_hint(program: &str) -> &'static str {
    match program {
        "verilator" => "Verilator 5: Homebrew `verilator`, or OSS CAD Suite",
        "vivado" => "AMD Vivado",
        "sv2v" | "yosys" | "nextpnr-ice40" | "icepack" | "iceprog" => {
            "Homebrew `sv2v yosys nextpnr-ice40 icestorm`, or OSS CAD Suite"
        }
        _ => "install it, or put it on PATH",
    }
}

fn spawn_failure(program: &str, e: std::io::Error) -> Failure {
    Failure::Usage(format!("cannot run {program}: {e}"))
}

/// Runs `command` with its output in `log`; a failure quotes the log's lines
/// that match `pattern` (or its tail).
pub fn run_logged(command: &mut Command, log: &Utf8Path, pattern: &[&str]) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let file = std::fs::File::create(log).map_err(|e| Failure::Failed(format!("{log}: {e}")))?;
    let err = file.try_clone().map_err(|e| Failure::Failed(format!("{log}: {e}")))?;
    let status = command.stdout(file).stderr(err).status().map_err(|e| spawn_failure(&program, e))?;
    if status.success() {
        return Ok(());
    }
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut quoted: Vec<&str> = text.lines().filter(|l| pattern.iter().any(|p| l.contains(p))).take(10).collect();
    if quoted.is_empty() {
        let lines: Vec<&str> = text.lines().collect();
        quoted = lines[lines.len().saturating_sub(10)..].to_vec();
    }
    Err(Failure::Failed(format!("{program} failed ({log}):\n{}", quoted.join("\n"))))
}

/// A directory's `files.f`, as paths in its order: one per line, `#`
/// comments and blank lines skipped. Upstream's runtime lists its packages
/// first, so the order is the compile order.
pub fn files_f(dir: &Utf8Path) -> Result<Vec<Utf8PathBuf>> {
    let list = dir.join("files.f");
    let text = std::fs::read_to_string(&list).map_err(|e| Failure::Usage(format!("{list}: {e}")))?;
    Ok(parse_files_f(dir, &text))
}

fn parse_files_f(dir: &Utf8Path, text: &str) -> Vec<Utf8PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| dir.join(l))
        .collect()
}

/// Each path once, at its first place.
fn dedup(paths: Vec<Utf8PathBuf>) -> Vec<Utf8PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

fn write(path: &Utf8Path, text: &str) -> Result<()> {
    std::fs::write(path, text).map_err(|e| Failure::Failed(format!("{path}: {e}")))
}

/// A fresh directory.
fn fresh_dir(dir: &Utf8Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| Failure::Failed(format!("{dir}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_f_keeps_its_order_and_skips_comments() {
        let files = parse_files_f(Utf8Path::new("/rt"), "# packages first\nkanagawa_pkg.sv\n\n  fifo.sv \n#x.sv\n");
        assert_eq!(files, [Utf8PathBuf::from("/rt/kanagawa_pkg.sv"), Utf8PathBuf::from("/rt/fifo.sv")]);
    }

    /// A board's shared RTL may also be named by the project: once, at the
    /// first place.
    #[test]
    fn a_source_named_twice_is_kept_once_where_first_named() {
        let paths = ["/a.sv", "/b.sv", "/a.sv", "/c.sv"].map(Utf8PathBuf::from).to_vec();
        assert_eq!(dedup(paths), ["/a.sv", "/b.sv", "/c.sv"].map(Utf8PathBuf::from));
    }

    #[test]
    fn generated_packages_come_first() {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap().join(format!("kg-board-gen-{}", std::process::id()));
        fresh_dir(&dir).unwrap();
        for f in ["Blinky.sv", "Blinky_types.sv", "BlinkyRtlMap.json", "Alpha.sv"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let names: Vec<String> =
            Design::collect_generated(&dir).iter().map(|p| p.file_name().unwrap().to_owned()).collect();
        assert_eq!(names, ["Blinky_types.sv", "Alpha.sv", "Blinky.sv"]);
    }
}
