//! Boards: a manifest's `[board]` table and the `board.toml` it names.
//!
//! Opt-in. A manifest without `[board]` resolves exactly as before; one with
//! it targets the board's device, puts the platform layer's library on the
//! import path after every other directory, so it adds modules and never
//! shadows one (`docs/plans/platform-layer.md` §3.1), and gives `kg board`
//! what it needs to simulate, synthesise and program the design
//! (`docs/plans/board-tooling.md`).
//!
//! A *platform* is a directory in `platform/`'s layout: `library/` (an
//! import root), `rtl/hal/<hal>/`, `boards/<board>/board.toml` and
//! `vendor/<tool>/`. A board is named in one of the platform's `boards/`, or
//! by its directory, for a board of a project's own.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::{DEFAULT_TARGET_DEVICE, Settings};

/// A manifest's `[board]`: which board, and the project's part of the
/// design beneath the board's shell.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BoardTable {
    /// A board in the platform's `boards/`.
    pub name: Option<String>,
    /// Or a board directory, holding `board.toml`, relative to the manifest.
    pub path: Option<Utf8PathBuf>,
    /// The platform: library, HALs, vendor flows, boards. Default:
    /// `KG_PLATFORM`, then the platform holding a `path` board, then the
    /// installation's (`share/kanagawa/platform`).
    pub platform: Option<Utf8PathBuf>,
    /// The project's RTL: the core module the board's shell instantiates,
    /// wrapping the exported classes.
    #[serde(default)]
    pub rtl: Vec<Utf8PathBuf>,
    /// The board's top-level module, if not the board's own shell.
    pub top_module: Option<String>,
    /// The module beneath the shell (`netsim` synthesises it alone).
    pub core_module: Option<String>,
    /// A self-checking testbench, top module `tb`, that prints `PASS`.
    pub testbench: Option<Utf8PathBuf>,
    /// Bindings for the generated core: an exported function's name to the
    /// board resource it serves, `rx = "uart.rx"`, `step = "led"`, or
    /// `"bridge"` for the host bridge. A function not named here binds by
    /// its own name (`docs/plans/board-tooling.md` §6).
    #[serde(default)]
    pub connect: BTreeMap<String, String>,
    /// cocotb test modules for `kg board test`; default: every `test_*.py`
    /// beside the manifest.
    #[serde(default)]
    pub tests: Vec<Utf8PathBuf>,
}

/// A board's `board.toml`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BoardSpec {
    #[serde(default)]
    pub board: BoardInfo,
    pub fpga: Fpga,
    #[serde(default)]
    pub clock: Clock,
    /// Backend options for every design on the board, keyed as the
    /// manifest's `[hw]`, which overrides them.
    #[serde(default)]
    pub hw: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub program: Program,
    #[serde(default)]
    pub constraints: Constraints,
    /// What the board offers a design, by name: its LEDs, buttons, serial
    /// port, buses and screen, each on ports of the core module. A design
    /// whose project supplies no core gets one generated from these.
    #[serde(default)]
    pub resources: BTreeMap<String, Resource>,
}

/// One of a board's resources: a kind, the core module's ports it is on,
/// and the kind's parameters. Which keys a kind reads, and the contract a
/// function binding to it must meet, are `kg_board::wrapper`'s.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Resource {
    /// `led`, `button`, `uart`, `spi`, `i2c` or `vga`.
    pub kind: String,
    /// One line for `kg board info`.
    pub doc: Option<String>,
    /// The core module's port for each of the kind's pins.
    #[serde(default)]
    pub pins: BTreeMap<String, String>,
    /// LEDs and buttons: how many.
    pub width: Option<u32>,
    /// A serial port's rate.
    pub baud: Option<u32>,
    /// A bus's clock, SPI's SCLK or I2C's SCL.
    pub hz: Option<u32>,
    /// SPI's mode, 0 to 3.
    pub mode: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BoardInfo {
    pub name: Option<String>,
    pub vendor: Option<String>,
    pub revision: Option<String>,
    pub url: Option<String>,
    /// The board's own RTL, relative to its directory: the shell, and parts
    /// any design on it may use.
    #[serde(default)]
    pub rtl: Vec<Utf8PathBuf>,
    pub top_module: Option<String>,
    pub core_module: Option<String>,
    /// `kg board serve`: a testbench (top `tb`) that runs the core with its
    /// I/O bridged to the host, and its C++.
    #[serde(default)]
    pub bridge: Vec<Utf8PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Fpga {
    /// `--target-device`: `<library>/device/<device>`.
    pub device: String,
    /// The vendor tool's name for the part.
    pub part: String,
    #[serde(default)]
    pub package: String,
    /// The HAL the generated RTL is built against: the platform's
    /// `rtl/hal/<hal>`, or the reference runtime's.
    pub hal: String,
    /// The HAL for simulation, where the vendor's needs its tools.
    pub sim_hal: Option<String>,
    /// The flow: `icestorm` or `vivado`.
    pub tool: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Clock {
    pub mhz: Option<i64>,
    /// The shell's parameter that sets the clock.
    pub parameter: Option<String>,
    /// The clock's pin, for a board whose clock is an input.
    pub port: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Program {
    /// The programmer's command line; the bitstream is appended.
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Constraints {
    #[serde(default)]
    pub files: Vec<Utf8PathBuf>,
}

/// A board, found: its directory and platform, its `board.toml`, and the
/// project's part, every path absolute.
#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    pub dir: Utf8PathBuf,
    pub platform: Utf8PathBuf,
    pub spec: BoardSpec,
    pub project: BoardTable,
}

/// The installation's platform: `../share/kanagawa/platform` beside the
/// running executable, as a release lays it out. `None` in a build tree.
pub fn bundled_platform() -> Option<Utf8PathBuf> {
    let exe = Utf8PathBuf::from_path_buf(std::env::current_exe().ok()?.canonicalize().ok()?).ok()?;
    let platform = exe.parent()?.join("../share/kanagawa/platform");
    platform.join("boards").is_dir().then(|| platform.canonicalize_utf8().unwrap_or(platform))
}

/// Whether `dir` is laid out as a platform.
pub fn is_platform(dir: &Utf8Path) -> bool {
    dir.join("boards").is_dir() && dir.join("library").is_dir()
}

/// The boards in a platform, by name.
pub fn list(platform: &Utf8Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(platform.join("boards"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("board.toml").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Reads a `board.toml`.
pub fn load_spec(path: &Utf8Path) -> Result<BoardSpec, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    toml::from_str(&text).map_err(|e| format!("{path}:\n{e}"))
}

/// Finds the board a manifest's `[board]` names. `root` is the manifest's
/// directory; `platform_env` is `KG_PLATFORM`, read at the front door.
pub fn resolve(table: &BoardTable, root: &Utf8Path, platform_env: Option<&Utf8Path>) -> Result<Board, String> {
    let named_platform = table.platform.as_ref().map(|p| join(root, p)).or_else(|| platform_env.map(|p| join(root, p)));
    let (dir, platform) = match (&table.name, &table.path) {
        (Some(_), Some(_)) => return Err("[board]: `name` and `path` both given; name one board".to_owned()),
        (None, None) => return Err("[board]: no board; give `name` (a platform's board) or `path` (a board directory)".to_owned()),
        (Some(name), None) => {
            let platform = named_platform.or_else(bundled_platform).ok_or_else(|| {
                format!("[board]: no platform to find `{name}` in; set `platform` in [board] or KG_PLATFORM")
            })?;
            (platform.join("boards").join(name), platform)
        }
        (None, Some(path)) => {
            let dir = normalize(&join(root, path));
            // A board inside a platform's `boards/` belongs to it.
            let holder = dir.parent().filter(|b| b.file_name() == Some("boards")).and_then(Utf8Path::parent);
            let platform = named_platform
                .or_else(|| holder.filter(|p| is_platform(p)).map(Utf8Path::to_owned))
                .or_else(bundled_platform)
                .ok_or_else(|| format!("[board]: no platform for the board at {dir}; set `platform` in [board] or KG_PLATFORM"))?;
            (dir, platform)
        }
    };
    let dir = normalize(&dir);
    let platform = normalize(&platform);
    if !is_platform(&platform) {
        return Err(format!("[board]: {platform} is not a platform (it needs boards/ and library/)"));
    }
    let manifest = dir.join("board.toml");
    if !manifest.is_file() {
        let known = list(&platform);
        let hint = if known.is_empty() { String::new() } else { format!("; {platform} has {}", known.join(", ")) };
        return Err(format!("[board]: no board at {dir} (no board.toml){hint}"));
    }
    let spec = load_spec(&manifest)?;
    let project = BoardTable {
        rtl: table.rtl.iter().map(|p| join(root, p)).collect(),
        testbench: table.testbench.as_ref().map(|p| join(root, p)),
        tests: table.tests.iter().map(|p| join(root, p)).collect(),
        ..table.clone()
    };
    Ok(Board { dir, platform, spec, project })
}

impl Board {
    /// The board's name: its `board.toml`'s, or its directory's.
    pub fn name(&self) -> &str {
        self.spec.board.name.as_deref().or(self.dir.file_name()).unwrap_or("board")
    }

    /// The platform's import root.
    pub fn library(&self) -> Utf8PathBuf {
        self.platform.join("library")
    }

    pub fn top_module(&self) -> Option<&str> {
        self.project.top_module.as_deref().or(self.spec.board.top_module.as_deref())
    }

    pub fn core_module(&self) -> Option<&str> {
        self.project.core_module.as_deref().or(self.spec.board.core_module.as_deref())
    }

    pub fn sim_hal(&self) -> &str {
        self.spec.fpga.sim_hal.as_deref().unwrap_or(&self.spec.fpga.hal)
    }

    /// A path in the board's directory.
    pub fn file(&self, path: &Utf8Path) -> Utf8PathBuf {
        join(&self.dir, path)
    }

    /// The board's backend options: its `[hw]`, with its clock as
    /// `frequency` unless `[hw]` names one. The manifest's `[hw]` goes over
    /// these.
    pub fn hw(&self) -> BTreeMap<String, toml::Value> {
        let mut hw = self.spec.hw.clone();
        if let Some(mhz) = self.spec.clock.mhz
            && !hw.keys().any(|k| k.replace('_', "-") == "frequency")
        {
            hw.insert("frequency".to_owned(), toml::Value::Integer(mhz));
        }
        hw
    }
}

/// Targets `board`: its device, unless another was asked for, which is an
/// error; and the platform's library, searched after every other import
/// directory. Called after [`Settings::or_bundled_library`], so the
/// installation's standard library precedes it too.
pub fn apply(settings: &mut Settings, board: &Board) -> Result<(), String> {
    let device = &board.spec.fpga.device;
    if settings.target_device != *device && settings.target_device != DEFAULT_TARGET_DEVICE {
        return Err(format!(
            "the board {} is a {device}, and the target device is {}; leave `target-device` unset",
            board.name(),
            settings.target_device
        ));
    }
    settings.target_device = device.clone();
    let library = board.library();
    if !settings.import_dirs.contains(&library) {
        settings.import_dirs.push(library);
    }
    Ok(())
}

fn join(root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() { path.to_owned() } else { root.join(path) }
}

/// `a/b/../c` as `a/c`: a board found through `../..` is named by the path
/// a reader expects. Lexical, as the paths need not exist yet.
fn normalize(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for part in path.components() {
        match part {
            camino::Utf8Component::CurDir => {}
            camino::Utf8Component::ParentDir if out.file_name().is_some() => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch platform with one board, `demo`, under a fresh directory.
    fn platform(tag: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap().join(format!(
            "kg-config-board-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("library")).unwrap();
        std::fs::create_dir_all(dir.join("boards/demo")).unwrap();
        std::fs::write(
            dir.join("boards/demo/board.toml"),
            r#"
[board]
name = "demo"
rtl = ["rtl/top.sv"]
top-module = "demo_top"
core-module = "demo_core"

[fpga]
device = "ice40-up5k"
part = "up5k"
package = "sg48"
hal = "ice40"
tool = "icestorm"

[clock]
mhz = 24

[hw]
reset-cycles = 1
"#,
        )
        .unwrap();
        dir
    }

    fn table(text: &str) -> BoardTable {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_named_board_is_found_in_the_platform() {
        let p = platform("named");
        let board = resolve(&table("name = \"demo\"\nrtl = [\"core.sv\"]\n"), Utf8Path::new("/proj"), Some(&p)).unwrap();
        assert_eq!(board.dir, p.join("boards/demo"));
        assert_eq!(board.name(), "demo");
        assert_eq!(board.top_module(), Some("demo_top"));
        assert_eq!(board.project.rtl, [Utf8PathBuf::from("/proj/core.sv")]);
        assert_eq!(board.library(), p.join("library"));
    }

    /// `[board] platform` is the manifest's own answer, before KG_PLATFORM.
    #[test]
    fn the_manifests_platform_wins_over_the_environment() {
        let p = platform("precedence");
        let board =
            resolve(&table(&format!("name = \"demo\"\nplatform = \"{p}\"\n")), Utf8Path::new("/proj"), Some(Utf8Path::new("/nowhere")))
                .unwrap();
        assert_eq!(board.platform, p);
    }

    #[test]
    fn a_board_by_path_belongs_to_the_platform_holding_it() {
        let p = platform("path");
        let example = p.join("boards/demo/examples/blinky");
        let board = resolve(&table("path = \"../..\"\ncore-module = \"mine\"\n"), &example, None).unwrap();
        assert_eq!(board.dir, p.join("boards/demo"));
        assert_eq!(board.platform, p);
        assert_eq!(board.core_module(), Some("mine"));
    }

    #[test]
    fn an_unknown_board_names_the_ones_there_are() {
        let p = platform("unknown");
        let err = resolve(&table("name = \"nope\"\n"), Utf8Path::new("/proj"), Some(&p)).unwrap_err();
        assert!(err.contains("no board at") && err.contains("has demo"), "{err}");
    }

    #[test]
    fn name_and_path_together_are_refused() {
        let err = resolve(&table("name = \"a\"\npath = \"b\"\n"), Utf8Path::new("/proj"), None).unwrap_err();
        assert!(err.contains("both"), "{err}");
    }

    #[test]
    fn the_boards_clock_is_its_frequency_unless_hw_names_one() {
        let p = platform("clock");
        let mut board = resolve(&table("name = \"demo\"\n"), Utf8Path::new("/proj"), Some(&p)).unwrap();
        assert_eq!(board.hw()["frequency"].as_integer(), Some(24));
        assert_eq!(board.hw()["reset-cycles"].as_integer(), Some(1));
        board.spec.hw.insert("frequency".to_owned(), toml::Value::Integer(12));
        assert_eq!(board.hw()["frequency"].as_integer(), Some(12));
    }

    /// The layer's library goes after every other import directory, the
    /// bundled standard library included, and the board sets the device.
    #[test]
    fn applying_a_board_sets_the_device_and_appends_the_library() {
        let p = platform("apply");
        let board = resolve(&table("name = \"demo\"\n"), Utf8Path::new("/proj"), Some(&p)).unwrap();
        let mut settings = Settings::fallback("/proj");
        settings.import_dirs = vec!["/upstream/library".into()];
        apply(&mut settings, &board).unwrap();
        assert_eq!(settings.target_device, "ice40-up5k");
        assert_eq!(settings.import_dirs, [Utf8PathBuf::from("/upstream/library"), p.join("library")]);
    }

    #[test]
    fn another_device_than_the_boards_is_refused() {
        let p = platform("device");
        let board = resolve(&table("name = \"demo\"\n"), Utf8Path::new("/proj"), Some(&p)).unwrap();
        let mut settings = Settings::fallback("/proj");
        settings.target_device = "artix7-xc7a50t".to_owned();
        let err = apply(&mut settings, &board).unwrap_err();
        assert!(err.contains("ice40-up5k") && err.contains("artix7-xc7a50t"), "{err}");
    }

    #[test]
    fn a_typo_in_board_toml_names_the_key() {
        let err = toml::from_str::<BoardSpec>("[fpga]\ndevice = \"d\"\npart = \"p\"\nhal = \"h\"\ntool = \"t\"\npackge = \"x\"\n")
            .unwrap_err();
        assert!(err.to_string().contains("packge"), "{err}");
    }

    /// A board's resources, and a project's bindings and cocotb tests, its
    /// paths taken from the manifest's directory.
    #[test]
    fn resources_connect_and_tests_are_read() {
        let dir = platform("resources");
        let spec = dir.join("boards/demo/board.toml");
        let text = std::fs::read_to_string(&spec).unwrap()
            + "\n[resources.uart]\nkind = \"uart\"\nbaud = 115200\npins = { rx = \"uart_rxd\", tx = \"uart_txd\" }\n";
        std::fs::write(&spec, text).unwrap();
        let t = table("name = \"demo\"\nconnect = { rx = \"uart.rx\" }\ntests = [\"tests/test_a.py\"]\n");
        let board = resolve(&t, Utf8Path::new("/proj"), Some(&dir)).unwrap();
        let uart = &board.spec.resources["uart"];
        assert_eq!((uart.kind.as_str(), uart.baud), ("uart", Some(115_200)));
        assert_eq!(uart.pins["tx"], "uart_txd");
        assert_eq!(board.project.connect["rx"], "uart.rx");
        assert_eq!(board.project.tests, [Utf8PathBuf::from("/proj/tests/test_a.py")]);

        let err = toml::from_str::<BoardSpec>("[fpga]\ndevice = \"d\"\npart = \"p\"\nhal = \"h\"\ntool = \"t\"\n[resources.x]\nkind = \"led\"\nwidht = 3\n")
            .unwrap_err();
        assert!(err.to_string().contains("widht"), "{err}");
    }
}