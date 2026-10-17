//! `kg board`: a design on a board — simulated, synthesised, programmed.
//!
//! Opt-in: every subcommand but `list` and `new` reads the manifest's
//! `[board]` (`kg_config::board`), and nothing else in `kg` changes for a
//! project without one. The commands that need RTL build it first, exactly
//! as `kg build` would, into the project's `target/board/gen/`; the flows
//! after that are `kg_board`'s (`docs/plans/board-tooling.md`).

use std::io::Write;

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Args, Subcommand};

use super::settings::{Env, Resolved};
use super::style::Styles;
use super::{BuildArgs, GlobalArgs, SourceArgs};

#[derive(Args, Debug)]
pub struct BoardArgs {
    #[command(subcommand)]
    pub command: BoardCommand,
}

#[derive(Subcommand, Debug)]
pub enum BoardCommand {
    /// List the boards a platform has, and the examples to start from.
    List(PlatformArgs),
    /// Show the project's board, and which of its tools are on PATH.
    Info,
    /// Start a project from an example: the board's own, or the platform's.
    New(NewArgs),
    /// Build, then write the vendor flow's inputs (sources.f, constraints.f,
    /// board.env, board.tcl) for a flow run elsewhere.
    Bundle(FlowArgs),
    /// Build, then run the project's testbench under Verilator.
    Sim(FlowArgs),
    /// Build, synthesise the core, then run the testbench on its netlist.
    Netsim(FlowArgs),
    /// Build, then the vendor flow to a bitstream.
    Bitstream(FlowArgs),
    /// Program the board with the last bitstream.
    Flash,
    /// Build, then run the core in simulation with its I/O on the host (a
    /// serial port on a pseudo-terminal, exported as BOARD_PTY). With a
    /// command after `--`, run it against the simulation and exit with its
    /// status.
    Serve(ServeArgs),
    /// Build, synthesise, and count the cells of the core (or `--top`).
    Area(AreaArgs),
    /// Build, then run the project's cocotb tests (test_*.py) against the
    /// generated module, every function driven through its own ports.
    Test(TestArgs),
    /// Build, then show the exported class's interface and what the
    /// generated core binds each function to.
    Interface(InterfaceArgs),
}

#[derive(Args, Debug)]
pub struct TestArgs {
    #[command(flatten)]
    pub flow: FlowArgs,
    /// The Python with cocotb 2 (a virtual environment's, say).
    #[arg(long, value_name = "PATH", default_value = "python3")]
    pub python: String,
}

#[derive(Args, Debug)]
pub struct InterfaceArgs {
    #[command(flatten)]
    pub flow: FlowArgs,
    /// Print target/board/interface.json instead.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct PlatformArgs {
    /// The platform (default: the manifest's board's, KG_PLATFORM, then the
    /// installation's).
    #[arg(long, value_name = "DIR")]
    pub platform: Option<String>,
}

#[derive(Args, Debug)]
pub struct NewArgs {
    /// The project directory to create.
    #[arg(value_name = "DIR")]
    pub dir: String,
    /// The board.
    #[arg(long, value_name = "NAME")]
    pub board: String,
    /// The example to start from.
    #[arg(long, value_name = "NAME", default_value = "blinky")]
    pub example: String,
    /// The platform (default: KG_PLATFORM, then the installation's).
    #[arg(long, value_name = "DIR")]
    pub platform: Option<String>,
}

#[derive(Args, Debug, Default)]
pub struct FlowArgs {
    /// A backend option over the board's and the manifest's `[hw]`,
    /// `-Z hw.<key>=<value>`.
    #[arg(short = 'Z', value_name = "hw.KEY=VALUE")]
    pub unstable: Vec<String>,
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    #[command(flatten)]
    pub flow: FlowArgs,
    /// A command to run against the simulation.
    #[arg(last = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}

#[derive(Args, Debug)]
pub struct AreaArgs {
    #[command(flatten)]
    pub flow: FlowArgs,
    /// The module to synthesise (default: the board's core).
    #[arg(long, value_name = "MODULE")]
    pub top: Option<String>,
}

pub fn run(global: &GlobalArgs, args: &BoardArgs, styles: &Styles, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match &args.command {
        BoardCommand::List(a) => list(global, a, out, err),
        BoardCommand::New(a) => new(a, out, err),
        BoardCommand::Info => {
            let Some(resolved) = board_project(global, err) else { return 2 };
            info(&resolved, out)
        }
        BoardCommand::Flash => {
            let Some(resolved) = board_project(global, err) else { return 2 };
            let board = resolved.board.as_ref().expect("board_project");
            let design = kg_board::Design {
                board,
                runtime: Utf8PathBuf::new(),
                out: out_dir(&resolved),
                generated: Vec::new(),
                interface: Err("not built".to_owned()),
                core: None,
            };
            finish(kg_board::program(&design, out), styles, err)
        }
        BoardCommand::Bundle(flow) => {
            with_design(global, flow, styles, out, err, |design, out| {
                design.bundle()?;
                let _ = writeln!(out, "bundle: {}", design.out);
                Ok(0)
            })
        }
        BoardCommand::Sim(flow) => with_design(global, flow, styles, out, err, |d, out| kg_board::sim(d, out).map(|()| 0)),
        BoardCommand::Netsim(flow) => {
            with_design(global, flow, styles, out, err, |d, out| kg_board::netsim(d, out).map(|()| 0))
        }
        BoardCommand::Bitstream(flow) => {
            with_design(global, flow, styles, out, err, |d, out| kg_board::bitstream(d, out).map(|_| 0))
        }
        BoardCommand::Serve(a) => with_design(global, &a.flow, styles, out, err, |d, out| kg_board::serve(d, &a.command, out)),
        BoardCommand::Test(a) => with_design(global, &a.flow, styles, out, err, |d, out| kg_board::test(d, &a.python, out).map(|()| 0)),
        // With --json, stdout is the JSON alone; the build's log goes to stderr.
        BoardCommand::Interface(a) if a.json => {
            let mut log = Vec::new();
            let mut json = String::new();
            let code = with_design(global, &a.flow, styles, &mut log, err, |d, _| {
                json = d.interface()?.to_json();
                Ok(0)
            });
            let _ = err.write_all(&log);
            let _ = write!(out, "{json}");
            code
        }
        BoardCommand::Interface(a) => with_design(global, &a.flow, styles, out, err, |d, out| {
            let interface = d.interface()?;
            {
                let _ = write!(out, "{}", interface.summary());
                if let Some(core) = &d.core {
                    let _ = write!(out, "bindings (generated {}):\n{}", core.module, core.summary());
                }
            }
            Ok(0)
        }),
        BoardCommand::Area(a) => {
            with_design(global, &a.flow, styles, out, err, |d, out| kg_board::area(d, a.top.as_deref(), out).map(|_| 0))
        }
    }
}

/// The manifest's settings, which must name a board.
fn board_project(global: &GlobalArgs, err: &mut dyn Write) -> Option<Resolved> {
    let resolved = super::resolve(global, &SourceArgs::default(), &[], err)?;
    if resolved.board.is_none() {
        let where_ = resolved.manifest_path.as_ref().map_or_else(|| "no kanagawa.toml here".to_owned(), |p| format!("{p} has no [board]"));
        let _ = writeln!(err, "kg board: {where_}; `kg board new <dir> --board <name>` starts a project for one");
        return None;
    }
    Some(resolved)
}

fn out_dir(resolved: &Resolved) -> Utf8PathBuf {
    let root = resolved.manifest_path.as_ref().and_then(|m| m.parent()).map_or_else(super::cwd, Utf8Path::to_owned);
    root.join("target/board")
}

/// Builds the project for its board, then runs `flow` on the result.
fn with_design(
    global: &GlobalArgs,
    flow: &FlowArgs,
    styles: &Styles,
    out: &mut dyn Write,
    err: &mut dyn Write,
    then: impl FnOnce(&kg_board::Design, &mut dyn Write) -> kg_board::Result<i32>,
) -> i32 {
    let Some(resolved) = board_project(global, err) else { return 2 };
    let Some(runtime) = runtime_dir(&resolved) else {
        let _ = writeln!(
            err,
            "kg board: cannot find upstream's runtime RTL (runtime/rtl beside the standard library, or under KANAGAWA_ROOT)"
        );
        return 2;
    };
    let out_dir = out_dir(&resolved);
    let gen_dir = kg_board::Design::gen_dir(&out_dir);
    let _ = std::fs::remove_dir_all(&gen_dir);
    let build = BuildArgs {
        paths: Vec::new(),
        backend: "sv".to_owned(),
        output: Some(format!("{gen_dir}/")),
        optimize: None,
        emit: vec![format!("resource-usage={out_dir}/resources.txt")],
        debug: false,
        unstable: flow.unstable.clone(),
        source: SourceArgs::default(),
    };
    let board = resolved.board.clone().expect("board_project");
    let sources = match super::collect_inputs(&[], &resolved, err) {
        Ok(sources) => sources,
        Err(code) => return code,
    };
    // The backend reports its progress on stdout; it is the build's log.
    let code = super::compile(global, &build, resolved, styles, out, err);
    if code != 0 {
        return code;
    }
    let generated = kg_board::Design::collect_generated(&gen_dir);
    if generated.is_empty() {
        let _ = writeln!(err, "kg board: the build wrote no RTL to {gen_dir}; is a class exported?");
        return 1;
    }
    for file in &generated {
        if let Err(f) = kg_board::repair::generated(file) {
            return finish::<()>(Err(f), styles, err);
        }
    }
    let (interface, core) = match kg_board::describe(&board, &out_dir, &generated, &sources, out) {
        Ok(described) => described,
        Err(f) => return finish::<()>(Err(f), styles, err),
    };
    let design = kg_board::Design { board: &board, runtime, out: out_dir, generated, interface, core };
    match then(&design, out) {
        Ok(code) => code,
        Err(f) => finish::<()>(Err(f), styles, err),
    }
}

fn finish<T>(result: kg_board::Result<T>, styles: &Styles, err: &mut dyn Write) -> i32 {
    match result {
        Ok(_) => 0,
        Err(f) => {
            let _ = writeln!(err, "{}: {}", styles.error("error"), f.message());
            f.exit_code()
        }
    }
}

/// Upstream's `runtime/rtl`: beside the standard library the project
/// imports (upstream's layout, and the installation's), else under
/// `KANAGAWA_ROOT`.
fn runtime_dir(resolved: &Resolved) -> Option<Utf8PathBuf> {
    resolved
        .settings
        .import_dirs
        .iter()
        .filter(|d| d.join("base.k").is_file())
        .map(|d| d.join("../runtime/rtl"))
        .chain(std::env::var("KANAGAWA_ROOT").ok().map(|r| Utf8PathBuf::from(r).join("runtime/rtl")))
        .find(|d| d.join("files.f").is_file())
        .map(|d| d.canonicalize_utf8().unwrap_or(d))
}

fn info(resolved: &Resolved, out: &mut dyn Write) -> i32 {
    let board = resolved.board.as_ref().expect("board_project");
    let spec = &board.spec;
    let info = &spec.board;
    let _ = writeln!(out, "board     {}{}", board.name(), info.vendor.as_ref().map(|v| format!(" ({v})")).unwrap_or_default());
    if let Some(url) = &info.url {
        let _ = writeln!(out, "          {url}");
    }
    let _ = writeln!(out, "directory {}", board.dir);
    let _ = writeln!(out, "platform  {}", board.platform);
    let package = if spec.fpga.package.is_empty() { String::new() } else { format!(", package {}", spec.fpga.package) };
    let _ = writeln!(out, "device    {} (part {}{package})", spec.fpga.device, spec.fpga.part);
    if let Some(mhz) = spec.clock.mhz {
        let _ = writeln!(out, "clock     {mhz} MHz");
    }
    let _ = writeln!(out, "hal       {} (simulation: {})", spec.fpga.hal, board.sim_hal());
    let _ = writeln!(out, "top       {}", board.top_module().unwrap_or("-"));
    let _ = writeln!(out, "core      {}", board.core_module().unwrap_or("-"));
    match runtime_dir(resolved) {
        Some(dir) => {
            let _ = writeln!(out, "runtime   {dir}");
        }
        None => {
            let _ = writeln!(out, "runtime   NOT FOUND (runtime/rtl beside the standard library, or KANAGAWA_ROOT)");
        }
    }
    if !spec.resources.is_empty() {
        let _ = writeln!(out, "resources (a core is generated unless [board] rtl names `{}`):", board.core_module().unwrap_or("-"));
        for (name, r) in &spec.resources {
            let _ = writeln!(out, "  {name:<10} {:<7} {}", r.kind, r.doc.as_deref().unwrap_or(""));
        }
    }
    let _ = writeln!(out, "tools ({}):", spec.fpga.tool);
    for (why, tool, found) in kg_board::tools(board) {
        let _ = writeln!(out, "  {:<14} {:<9} for {why}", tool, if found { "found" } else { "MISSING" });
    }
    0
}

/// The platform a command not about the project's board means.
fn platform_dir(given: Option<&str>, global: &GlobalArgs, err: &mut dyn Write) -> Option<Utf8PathBuf> {
    let from_manifest = || {
        let resolved = super::resolve(global, &SourceArgs::default(), &[], &mut std::io::sink())?;
        resolved.board.map(|b| b.platform)
    };
    let found = given
        .map(Utf8PathBuf::from)
        .or_else(|| Env::from_process().platform.map(Utf8PathBuf::from))
        .or_else(from_manifest)
        .or_else(kg_config::board::bundled_platform);
    match found {
        Some(dir) if kg_config::board::is_platform(&dir) => Some(dir),
        Some(dir) => {
            let _ = writeln!(err, "kg board: {dir} is not a platform (it needs boards/ and library/)");
            None
        }
        None => {
            let _ = writeln!(err, "kg board: no platform; pass --platform or set KG_PLATFORM");
            None
        }
    }
}

fn list(global: &GlobalArgs, args: &PlatformArgs, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let Some(platform) = platform_dir(args.platform.as_deref(), global, err) else { return 2 };
    for name in kg_config::board::list(&platform) {
        let dir = platform.join("boards").join(&name);
        let device = kg_config::board::load_spec(&dir.join("board.toml")).map(|s| s.fpga.device).unwrap_or_else(|e| e);
        let examples = kg_board::scaffold::examples(&dir);
        let _ = writeln!(out, "{name:<14} {device:<16} examples: {}", if examples.is_empty() { "-".to_owned() } else { examples.join(", ") });
    }
    let shared = kg_board::scaffold::examples(&platform);
    if !shared.is_empty() {
        let _ = writeln!(out, "\nfor any board with the resources they use: {}", shared.join(", "));
    }
    0
}

fn new(args: &NewArgs, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let Some(platform) = platform_dir(args.platform.as_deref(), &GlobalArgs { no_manifest: true, ..GlobalArgs::default() }, err)
    else {
        return 2;
    };
    // A library to name where the installation has none and the example's
    // own path to one does not reach.
    let library = match kg_config::Settings::bundled_library() {
        Some(_) => None,
        None => std::env::var("KANAGAWA_ROOT").ok().map(|r| Utf8PathBuf::from(r).join("library")),
    };
    let dest = Utf8PathBuf::from(&args.dir);
    match kg_board::scaffold::new_project(&platform, &args.board, &args.example, &dest, library.as_deref()) {
        Ok(files) => {
            let _ = writeln!(out, "Created {dest} from the `{}` example for the {} board ({} files). Next:", args.example, args.board, files.len() + 1);
            let _ = writeln!(out, "  cd {dest}");
            let mut next = |command: String, why: &str| {
                let _ = writeln!(out, "  {command:<40} # {why}");
            };
            next("kg board interface".to_owned(), "what it exports, and what serves each function");
            let manifest = std::fs::read_to_string(dest.join("kanagawa.toml")).unwrap_or_default();
            if manifest.lines().any(|l| l.trim_start().starts_with("testbench")) {
                next("kg board sim".to_owned(), "its testbench, in Verilator");
            }
            let names: Vec<&str> = files.iter().filter_map(|f| f.file_name()).collect();
            if let Some(test) = names.iter().find(|n| n.starts_with("test_") && n.ends_with(".py")) {
                next("kg board test".to_owned(), "its cocotb tests (Python with cocotb 2; --python)");
                next(format!("kg board serve -- python3 {test}"), "the same tests, over the simulated board's serial port");
            } else if names.contains(&"host.py") {
                next("kg board serve -- python3 host.py test".to_owned(), "its host program, against the simulation");
            } else {
                next("kg board serve".to_owned(), "the design in simulation, with a front panel where the board has one");
            }
            if args.board != "virtual" {
                next("kg board bitstream && kg board flash".to_owned(), "onto the board");
            }
            0
        }
        Err(f) => {
            let _ = writeln!(err, "kg board new: {}", f.message());
            f.exit_code()
        }
    }
}
