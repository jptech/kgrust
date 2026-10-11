//! The legacy flag inventory — one row per accepted spelling.
//!
//! This table *is* the compatibility contract. It is transcribed from the
//! `cmdargs` record literal at `compiler/hs/app/Options/CmdArgs.hs:18-163`, and
//! every row carries the source line it came from so a reader can check it
//! against the reference in one jump. `docs/spec/cli.md` §1 counts the surface:
//!
//! | Bucket | Count | Source |
//! |---|---|---|
//! | `compile` one-flag fields | 90 | `CmdArgs.hs:21-110` |
//! | `warnings_state` switches | 13 | `CmdArgs.hs:111-136` |
//! | `pretty` declared | 4 | `CmdArgs.hs:147-150` |
//! | `pretty` inherited (unassigned record fields) | 8 | `Options.hs:24-36` |
//! | `list-deps` inherited (`ListDeps{}`, every field) | 8 | `Options.hs:132-141` |
//! | `cmdargs` built-ins | 5 | `CmdArgs.hs:158` + library |
//! | **total user-visible flags** | **128** | |
//!
//! `cli.md` predates `list-deps` (upstream 2026-08-27, `CmdArgs.hs:155-165`)
//! and counts 120. The inherited `pretty` and `list-deps` flags are the same
//! rows as their `compile` counterparts, marked with every mode that declares
//! the field ([`Modes`]); the total counts them once per mode, exactly as
//! §1.0 does, so 112 distinct long names cover 128 flags.

use super::options::{LegacyOptions, WarningState};
use kg_diag::WarningKind::{
    Conversion, Deprecated, InitializerOverrides, RedundantImport, Shadow, TransactionSize,
};

use WarningState::{Disable, Enable};

/// Where a parsed value is written.
///
/// Field accessors rather than setters, so the table stays a one-liner per flag
/// and the compiler checks the field type against the value kind.
#[derive(Copy, Clone, Debug)]
pub enum Sink {
    Int(fn(&mut LegacyOptions) -> &mut i64),
    Float(fn(&mut LegacyOptions) -> &mut f64),
    /// A switch. Bare means `true`; `=false` is proved in-tree
    /// (`test/syntax/CMakeLists.txt:76`). Never consumes the next argument.
    Bool(fn(&mut LegacyOptions) -> &mut bool),
    Str(fn(&mut LegacyOptions) -> &mut String),
    /// Repeatable; values append in command-line order.
    List(fn(&mut LegacyOptions) -> &mut Vec<String>),
    /// `Maybe Int` — `--jobs`.
    MaybeInt(fn(&mut LegacyOptions) -> &mut Option<i64>),
    /// `&= opt (maxBound::Int)`: the value may be omitted, in which case the
    /// field becomes `maxBound`. Never consumes the next argument, so
    /// `--frontend-passes foo.k` leaves `foo.k` a source file.
    OptInt(fn(&mut LegacyOptions) -> &mut i64),
    Backend,
    Layout,
    /// A `warnings_state` alternative: appends its contribution to the list.
    Warnings(&'static [WarningState]),
    /// Handled by the parser itself, not written to the option record.
    Builtin(Builtin),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Builtin {
    Help,
    Version,
    NumericVersion,
    Verbose,
    Quiet,
}

/// The declared default, kept beside the flag for `--help` and cross-checked
/// against [`LegacyOptions::default`] by a test.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Def {
    Int(i64),
    /// `maxBound::Int`.
    MaxInt,
    Float(f64),
    Bool(bool),
    Text(&'static str),
    /// Empty list.
    List,
    /// `Nothing`.
    Unset,
    Enum(&'static str),
    /// Built-ins have no stored value.
    None,
}

impl Def {
    pub fn render(self) -> String {
        match self {
            Def::Int(n) => n.to_string(),
            Def::MaxInt => "all".to_owned(),
            Def::Float(x) => format!("{x}"),
            Def::Bool(b) => b.to_string(),
            Def::Text("") => "\"\"".to_owned(),
            Def::Text(t) => t.to_owned(),
            Def::List => "[]".to_owned(),
            Def::Unset => "unset".to_owned(),
            Def::Enum(e) => e.to_owned(),
            Def::None => String::new(),
        }
    }
}

/// `--help` sections, from `&= groupname` (`CmdArgs.hs:21-135`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Group {
    Logic,
    Compiler,
    Output,
    CodeGeneration,
    Placement,
    Diagnostics,
    Warnings,
    /// No `&= groupname`: `pretty`'s four own fields (`CmdArgs.hs:147-150`).
    /// `cmdargs` gives such a flag the group of the field before it in its
    /// mode (`assignGroups`, `Implicit/Global.hs:128-134`), which is why
    /// `--indent` is listed under "Compiler".
    Undeclared,
    Common,
}

impl Group {
    /// The section heading; `None` for [`Group::Undeclared`], which has
    /// none of its own.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Group::Logic => "Logic",
            Group::Compiler => "Compiler",
            Group::Output => "Output",
            Group::CodeGeneration => "Code generation",
            Group::Placement => "Placement",
            Group::Diagnostics => "Diagnostics",
            Group::Warnings => "Warnings",
            Group::Undeclared => return None,
            // `commonGroup` (`Implicit/Global.hs:147`).
            Group::Common => "Common flags",
        })
    }
}

/// Which mode(s) accept a flag: a bit per mode, as each of `Options.hs`'s
/// three records declares the field or not.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Modes(u8);

impl Modes {
    pub const COMPILE: Modes = Modes(1);
    pub const PRETTY: Modes = Modes(2);
    pub const LIST_DEPS: Modes = Modes(4);
    /// `dump_parse`, which `PrettyPrint` declares and `ListDeps` does not.
    pub const COMPILE_PRETTY: Modes = Modes(3);
    /// `file_list` (`Options.hs:68,135`).
    pub const COMPILE_LIST_DEPS: Modes = Modes(5);
    /// Where `cmdargs` gives the source-resolution flags a letter: the two
    /// small modes, where their first letters are unique.
    pub const PRETTY_LIST_DEPS: Modes = Modes(6);
    /// The source-resolution fields all three records declare, and the
    /// built-ins every mode gets (`extraFlags`, `Implicit/Global.hs:161-171`).
    pub const ALL: Modes = Modes(7);

    pub fn accepts(self, mode: super::options::Mode) -> bool {
        self.0 & mode.bit() != 0
    }

    /// How many of the 128 spellings this row accounts for.
    pub fn count(self) -> usize {
        self.0.count_ones() as usize
    }
}

/// One accepted flag.
#[derive(Copy, Clone, Debug)]
pub struct FlagSpec {
    /// The primary long name, without `--`.
    pub long: &'static str,
    /// Additional long names declared with `&= name` (`--target`).
    pub aliases: &'static [&'static str],
    /// A short alias — declared, or one `cmdargs` auto-assigned to a
    /// non-`explicit` field. The auto-assigned ones are read off the
    /// reference's own `--help` per mode (`kanagawa compile --help`,
    /// `kanagawa pretty --help`, 2026-09-23), since `cmdargs` assigns them
    /// mode by mode: `--base-library` is `-b` in `pretty` and `list-deps`
    /// and has no short form in `compile`. `tests::short_letters_are_cmdargs_own`
    /// re-derives them by `cmdargs`' rule.
    pub short: Option<char>,
    /// The modes [`FlagSpec::short`] is accepted in.
    pub short_modes: Modes,
    /// The value's name in `--help` (`flagType`): `typ`/`typFile`/`typDir`
    /// where declared, else the reader's (`INT`, `NUM`; an enum's type name,
    /// upper-cased). `[=INT]` marks an `&= opt` flag; a switch has none.
    pub metavar: &'static str,
    pub group: Group,
    pub modes: Modes,
    /// `&= explicit` — the reference derives no extra names for this flag.
    pub explicit: bool,
    /// Line in `compiler/hs/app/Options/CmdArgs.hs`.
    pub src: u32,
    pub sink: Sink,
    pub default: Def,
    pub help: &'static str,
}

impl FlagSpec {
    const fn new(
        long: &'static str,
        metavar: &'static str,
        group: Group,
        sink: Sink,
        default: Def,
        src: u32,
        help: &'static str,
    ) -> FlagSpec {
        FlagSpec {
            long,
            aliases: &[],
            short: None,
            short_modes: Modes::ALL,
            metavar,
            group,
            modes: Modes::COMPILE,
            explicit: false,
            src,
            sink,
            default,
            help,
        }
    }

    const fn explicit(mut self) -> FlagSpec {
        self.explicit = true;
        self
    }

    const fn short(mut self, c: char) -> FlagSpec {
        self.short = Some(c);
        self
    }

    const fn short_in(mut self, c: char, modes: Modes) -> FlagSpec {
        self.short = Some(c);
        self.short_modes = modes;
        self
    }

    const fn alias(mut self, names: &'static [&'static str]) -> FlagSpec {
        self.aliases = names;
        self
    }

    const fn modes(mut self, modes: Modes) -> FlagSpec {
        self.modes = modes;
        self
    }

    /// Whether a space-separated value binds to this flag.
    ///
    /// `--base-library helper/custom_base.k` and `--import-dir <dir>` prove the
    /// space form for value flags (`test/syntax/CMakeLists.txt:91`,
    /// `syntax-test.py:106`). `opt`-valued and boolean flags take only the
    /// attached `=value` form, so that `--frontend-passes foo.k` and
    /// `--parse-docs foo.k` both leave `foo.k` a source file (§11 Q8/Q10 — both
    /// UNVERIFIED upstream; this is the reading under which every in-tree
    /// invocation keeps working).
    pub fn takes_separate_value(&self) -> bool {
        matches!(
            self.sink,
            Sink::Int(_)
                | Sink::Float(_)
                | Sink::Str(_)
                | Sink::List(_)
                | Sink::MaybeInt(_)
                | Sink::Backend
                | Sink::Layout
        )
    }

    /// Whether `--flag=value` is accepted. Adds the switches (`=false`) and the
    /// optional-value flags to [`FlagSpec::takes_separate_value`].
    pub fn accepts_attached_value(&self) -> bool {
        self.takes_separate_value()
            || matches!(self.sink, Sink::Bool(_) | Sink::OptInt(_) | Sink::Builtin(Builtin::Help))
    }

    /// Every long spelling of this flag.
    pub fn long_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.long).chain(self.aliases.iter().copied())
    }
}

const INT: &str = "INT";
const NUM: &str = "NUM";
const BOOL: &str = "BOOL";
const FILE: &str = "FILE";
const DIR: &str = "DIR";
const NONE: &str = "";

/// Every flag of `compile` mode, `pretty` mode and the `cmdargs` built-ins.
///
/// Order is the declaration order of `CmdArgs.hs`, which is also the order
/// `--help` prints within a group.
pub static FLAGS: &[FlagSpec] = &[
    // ------------------------------------------------------------------ Logic
    FlagSpec::new("additional-latency", INT, Group::Logic, Sink::Int(|o| &mut o.additional_latency), Def::Int(1), 21,
        "Extra latency added to ease routing").explicit(),
    FlagSpec::new("auto-pipeline-ratio", INT, Group::Logic, Sink::Int(|o| &mut o.auto_pipeline_scale), Def::Int(0), 22,
        "The maximum number of extra pipeline registers to insert on function entry points. On platforms that support it, the actual pipeline depth is determined by the EDA tool.").explicit(),
    FlagSpec::new("auto-pipeline-cross-region", INT, Group::Logic, Sink::Int(|o| &mut o.auto_pipeline_cross_region), Def::Int(0), 23,
        "The maximum number of extra pipeline registers to insert on cross-region function entry points. On platforms that support it, the actual pipeline depth is determined by the EDA tool.").explicit(),
    FlagSpec::new("carry-chain-width", INT, Group::Logic, Sink::Int(|o| &mut o.carry_chain_width), Def::Int(40), 24,
        "The number of output bits in a carry-chain operation (add/sub) that is considered to be 1 level of logic").explicit(),
    FlagSpec::new("carry-select", BOOL, Group::Logic, Sink::Bool(|o| &mut o.carry_select), Def::Bool(false), 25,
        "Reduce pipeline length by implementing add/sub/compare operations with replicated, speculative computations"),
    FlagSpec::new("rmw-memory-write-delay", INT, Group::Logic, Sink::Int(|o| &mut o.rmw_memory_write_delay), Def::Int(1), 26,
        "Additional delay to add to memory writes for read-modify-write memory operations"),
    FlagSpec::new("rmw-memory-read-delay", INT, Group::Logic, Sink::Int(|o| &mut o.rmw_memory_read_delay), Def::Int(0), 27,
        "Additional delay to add to memory reads for read-modify-write memory operations"),
    FlagSpec::new("rmw-hardened-write-delay", INT, Group::Logic, Sink::Int(|o| &mut o.rmw_hardened_write_delay), Def::Int(1), 28,
        "Additional delay to add to memory writes for read-modify-write memory operations (when hardened forwarding logic is used)"),
    FlagSpec::new("rmw-hardened-max-width", INT, Group::Logic, Sink::Int(|o| &mut o.rmw_hardened_max_width), Def::MaxInt, 29,
        "Prevent hardened forwarding logic from being used when BRAM width is > rmw_hardened_max_width AND BRAM height is <= rmw_hardened_max_height"),
    FlagSpec::new("rmw-hardened-max-height", INT, Group::Logic, Sink::Int(|o| &mut o.rmw_hardened_max_height), Def::Int(0), 30,
        "Prevent hardened forwarding logic from being used when BRAM width is > rmw_hardened_max_width AND BRAM height is <= rmw_hardened_max_height"),
    FlagSpec::new("rmw-memory-favor-area", BOOL, Group::Logic, Sink::Bool(|o| &mut o.rmw_memory_favor_area), Def::Bool(false), 31,
        "Optimize read-modify-write memory operations for smaller area. By default optimize for Fmax."),
    FlagSpec::new("strict-memory-read-write", BOOL, Group::Logic, Sink::Bool(|o| &mut o.strict_memory_read_write), Def::Bool(false), 32,
        "Require concurrent memory read-write operations to the same address to result in the read seeing the old data (at a potential area cost)"),
    FlagSpec::new("io-register-delay", INT, Group::Logic, Sink::Int(|o| &mut o.io_register_delay), Def::Int(0), 33,
        "Delay to account for routing data to/from DSP/BRAM input/output registers (measured in levels of logic)"),
    FlagSpec::new("mem-to-reg", NUM, Group::Logic, Sink::Float(|o| &mut o.mem_to_reg), Def::Float(0.0), 34,
        "Upper threshold to move data from registers to memory (measured in memory bits per register bit).  Higher values lead to more memory consumption."),
    FlagSpec::new("fifo-to-dsp", INT, Group::Logic, Sink::Int(|o| &mut o.fifo_to_dsp), Def::Int(0), 35,
        "Local data propagation FIFOs for a number of pipeline stages less than or equal to this threshold will be converted to DSPs"),
    FlagSpec::new("fifo-write-delay", INT, Group::Logic, Sink::Int(|o| &mut o.fifo_write_delay), Def::Int(1), 36,
        "Number of cycles to add to fifo writes"),
    FlagSpec::new("fifo-merge-distance", INT, Group::Logic, Sink::Int(|o| &mut o.fifo_merge_distance), Def::Int(4), 37,
        "Maximum distance (pipeline stages) between merged fifo enqueue operations"),
    FlagSpec::new("frequency", "MHz", Group::Logic, Sink::Int(|o| &mut o.frequency), Def::Int(0), 38,
        "Clock 0 frequency").short('f'),
    FlagSpec::new("max-block-ram", INT, Group::Logic, Sink::Int(|o| &mut o.max_block_ram), Def::Int(-1), 39,
        "Maximum number of Block RAMs generated"),
    FlagSpec::new("block-ram-util-threshold", INT, Group::Logic, Sink::Int(|o| &mut o.block_ram_util_threshold), Def::Int(20), 40,
        "Percent minimum Block RAM utilization"),
    FlagSpec::new("max-deep-ram", INT, Group::Logic, Sink::Int(|o| &mut o.max_deep_ram), Def::Int(-1), 41,
        "Maximum number of Deep RAMs generated"),
    FlagSpec::new("deep-ram-util-threshold", INT, Group::Logic, Sink::Int(|o| &mut o.deep_ram_util_threshold), Def::Int(20), 42,
        "Percent minimum Deep RAM utilization"),
    FlagSpec::new("max-mux-sources", INT, Group::Logic, Sink::Int(|o| &mut o.max_mux_sources), Def::Int(16), 43,
        "Decompose multiplexers with more than this many dynamic inputs"),
    FlagSpec::new("max-mul-src-width", INT, Group::Logic, Sink::Int(|o| &mut o.max_mul_src_width), Def::Int(32), 44,
        "Decompose multipliers with inputs wider than this"),
    FlagSpec::new("max-threads-default", INT, Group::Logic, Sink::Int(|o| &mut o.max_threads_default), Def::Int(512), 45,
        "Default value for the maximum number of threads allowed in a function at one time"),
    FlagSpec::new("max-threads-limit", INT, Group::Logic, Sink::Int(|o| &mut o.max_threads_limit), Def::Int(512), 46,
        "Upper bound on the maximum number of threads allowed in a function at one time"),
    FlagSpec::new("schedule-limit", INT, Group::Logic, Sink::Int(|o| &mut o.schedule_limit), Def::Int(4096), 47,
        "Maximum number of iterations during instruction scheduling"),
    FlagSpec::new("register-ratio", INT, Group::Logic, Sink::Int(|o| &mut o.register_ratio), Def::Int(2), 48,
        "Insert pipeline registers after every N levels of combinatorial logic"),
    FlagSpec::new("max-register-ratio", INT, Group::Logic, Sink::Int(|o| &mut o.max_register_ratio), Def::Int(0), 49,
        "Atomic blocks can relax scheduling to insert pipeline register after every N levels of combinatorial logic"),
    FlagSpec::new("no-reset-fanout", BOOL, Group::Logic, Sink::Bool(|o| &mut o.no_reset_fanout), Def::Bool(false), 50,
        "Disable generation of reset fanout and rely on CAD tools to perform fanout"),
    FlagSpec::new("reset-cycles", INT, Group::Logic, Sink::Int(|o| &mut o.reset_cycles), Def::Int(3), 51,
        "Number of cycles to use to propagate the reset signal across the chip.").explicit(),
    FlagSpec::new("reset-fan-out-cycles", INT, Group::Logic, Sink::Int(|o| &mut o.reset_fan_out_cycles), Def::Int(1), 52,
        "Number of cycles used to fan-out the reset signal"),
    FlagSpec::new("reset-duration-cycles", INT, Group::Logic, Sink::Int(|o| &mut o.reset_duration_cycles), Def::Int(0), 53,
        "Number of extra cycles to hold the reset signal high"),
    FlagSpec::new("return-stages", INT, Group::Logic, Sink::Int(|o| &mut o.return_stages), Def::Int(3), 54,
        "Number of extra pipeline stages to add for returns from functions with multiple call sites"),
    FlagSpec::new("stallable-pipelines", BOOL, Group::Logic, Sink::Bool(|o| &mut o.stallable_pipelines), Def::Bool(false), 55,
        "Generate pipelines where individual stages can be stalled (reduces FIFO depth)"),
    FlagSpec::new("clock-gating", INT, Group::Logic, Sink::Int(|o| &mut o.clock_gating), Def::Int(0), 56,
        "Level of clock gating to apply (0 to disable)"),
    FlagSpec::new("clock-gating-threshold", INT, Group::Logic, Sink::Int(|o| &mut o.clock_gating_threshold), Def::Int(5), 57,
        "Instantiate a clock gate if it will apply to more than N register bits"),
    FlagSpec::new("conditional-memory-reads", BOOL, Group::Logic, Sink::Bool(|o| &mut o.conditional_memory_reads), Def::Bool(false), 58,
        "For memory reads that are conditional, try and make the memory read enable signal conditional as well. This can lower power consumption, but in some cases may negatively impact the maximum clock frequency"),
    FlagSpec::new("semaphore-delay", INT, Group::Logic, Sink::Int(|o| &mut o.semaphore_delay), Def::Int(3), 59,
        "Number of pipeline registers to add to the input signals that drive an internal semaphore module that is used to limit threads within a function"),
    FlagSpec::new("sparse-reg-opt", BOOL, Group::Logic, Sink::Bool(|o| &mut o.sparse_reg_opt), Def::Bool(false), 60,
        "Eliminate unused or constant bits in registers."),

    // --------------------------------------------------------------- Compiler
    FlagSpec::new("jobs", INT, Group::Compiler, Sink::MaybeInt(|o| &mut o.jobs), Def::Unset, 61,
        "Number of parallel jobs to use during compilation. By default equal to number of processors. Negative value can be used to specify how many fewer jobs than number of processors to run.").short_in('j', Modes::COMPILE),
    FlagSpec::new("import-dir", DIR, Group::Compiler, Sink::List(|o| &mut o.import_dir), Def::List, 62,
        "Directories to search for imported files").explicit().modes(Modes::ALL),
    FlagSpec::new("define", "name#val", Group::Compiler, Sink::List(|o| &mut o.define), Def::List, 63,
        "Define a global constant").explicit().short('d').modes(Modes::ALL),
    FlagSpec::new("using", "name#type", Group::Compiler, Sink::List(|o| &mut o.using), Def::List, 64,
        "Define a global type alias").explicit().short('u').modes(Modes::ALL),
    FlagSpec::new("no-implicit-base", BOOL, Group::Compiler, Sink::Bool(|o| &mut o.no_implicit_base), Def::Bool(false), 65,
        "Disable implicit import of base library").short_in('n', Modes::PRETTY_LIST_DEPS).modes(Modes::ALL),
    FlagSpec::new("base-library", FILE, Group::Compiler, Sink::Str(|o| &mut o.base_library), Def::Text("base.k"), 66,
        "Base library file path").short_in('b', Modes::PRETTY_LIST_DEPS).modes(Modes::ALL),
    FlagSpec::new("parse-docs", BOOL, Group::Compiler, Sink::Bool(|o| &mut o.parse_docs), Def::Bool(false), 67,
        "Parse documentation comments").short_in('p', Modes::PRETTY_LIST_DEPS).modes(Modes::ALL),

    // ----------------------------------------------------------------- Output
    FlagSpec::new("output", "STRING", Group::Output, Sink::Str(|o| &mut o.output), Def::Text(""), 68,
        "Base name of output file(s)").short_in('o', Modes::COMPILE),
    FlagSpec::new("dgml", FILE, Group::Output, Sink::Str(|o| &mut o.dgml), Def::Text(""), 69,
        "Output design in Directed Graph Markup format"),
    FlagSpec::new("dgml-detailed", DIR, Group::Output, Sink::Str(|o| &mut o.dgml_detailed), Def::Text(""), 70,
        "Output each basic block in Directed Graph Markup format"),
    FlagSpec::new("resource-usage", FILE, Group::Output, Sink::Str(|o| &mut o.resource_usage), Def::Text(""), 71,
        "Output resource utilization statistics"),
    FlagSpec::new("file-list", FILE, Group::Output, Sink::Str(|o| &mut o.file_list), Def::Text(""), 72,
        "Update specified file whenever list compiled files (including imported files) changes").short_in('f', Modes::LIST_DEPS).modes(Modes::COMPILE_LIST_DEPS),
    FlagSpec::new("identifier-length", INT, Group::Output, Sink::Int(|o| &mut o.identifier_length), Def::Int(100), 73,
        "Maximum length of identifiers in generated code"),
    FlagSpec::new("skip-circt-lowering", BOOL, Group::Output, Sink::Bool(|o| &mut o.skip_circt_lowering), Def::Bool(false), 74,
        "Skip CIRCT lowering passes; do not emit CIRCT-lowered SystemVerilog module bodies (other outputs such as RtlMap.json and *_types.sv may still be written). Implies --write-circt-ir"),

    // -------------------------------------------------------- Code generation
    FlagSpec::new("backend", "BACKEND", Group::CodeGeneration, Sink::Backend, Def::Enum("none"), 75,
        "Code generation backend: sv (SystemVerilog) or not specified (produce metadata only)").alias(&["target"]),
    FlagSpec::new("debug", BOOL, Group::CodeGeneration, Sink::Bool(|o| &mut o.debug), Def::Bool(false), 76,
        "Debug mode"),
    FlagSpec::new("dump-memory-metadata", BOOL, Group::CodeGeneration, Sink::Bool(|o| &mut o.dump_memory_metadata), Def::Bool(false), 77,
        "Output metadata files for memories and fifos"),
    FlagSpec::new("strip-debug-symbols", BOOL, Group::CodeGeneration, Sink::Bool(|o| &mut o.strip_debug_symbols), Def::Bool(false), 78,
        "Generate debug symbols without full file path names"),
    FlagSpec::new("force-mem-to-array", INT, Group::CodeGeneration, Sink::Int(|o| &mut o.force_mem_to_array), Def::Int(0), 79,
        "Regardless of cost, forces conversion of memories less than the specified size to arrays. For testing purposes only"),
    FlagSpec::new("max-stack-depth", INT, Group::CodeGeneration, Sink::Int(|o| &mut o.max_stack_depth), Def::Int(64), 80,
        "Maximum number of stack frames to inserted into the generated code"),
    FlagSpec::new("no-fifo-stutter", BOOL, Group::CodeGeneration, Sink::Bool(|o| &mut o.no_fifo_stutter), Def::Bool(false), 81,
        "Disable fifos stutter"),
    FlagSpec::new("no-debug-view", BOOL, Group::CodeGeneration, Sink::Bool(|o| &mut o.no_debug_view), Def::Bool(false), 82,
        "Disables debug views"),
    FlagSpec::new("no-verilog-header", BOOL, Group::CodeGeneration, Sink::Bool(|o| &mut o.no_verilog_header), Def::Bool(false), 83,
        "Don't output the informational header on top of generated Verilog files"),
    FlagSpec::new("code-coverage", BOOL, Group::CodeGeneration, Sink::Bool(|o| &mut o.code_coverage), Def::Bool(false), 84,
        "Enable generation of code coverage using SystemVerilog coverpoints"),
    FlagSpec::new("code-coverage-mux-threshold", INT, Group::CodeGeneration, Sink::Int(|o| &mut o.code_coverage_mux_threshold), Def::Int(0), 85,
        "Only generate code coverage code for muxes with this number of cases; the default value of 0 means to generate for all muxes"),
    FlagSpec::new("optimize", "LEVEL", Group::CodeGeneration, Sink::Int(|o| &mut o.optimize), Def::Int(2), 86,
        "Optimize generated code").short('O'),
    FlagSpec::new("target-device", "STRING", Group::CodeGeneration, Sink::Str(|o| &mut o.target_device), Def::Text("mock"), 87,
        "Target FPGA device name").short_in('t', Modes::PRETTY_LIST_DEPS).modes(Modes::ALL),
    FlagSpec::new("stall", INT, Group::CodeGeneration, Sink::Int(|o| &mut o.stall), Def::Int(0), 88,
        "Randomly force each basic block to stall (acceptable values [0,3]; default is 0 meaning no stalls)"),
    FlagSpec::new("work-list-size", INT, Group::CodeGeneration, Sink::Int(|o| &mut o.work_list_size), Def::Int(0), 89,
        "Optimization work list upper bound (log2)"),

    // -------------------------------------------------------------- Placement
    FlagSpec::new("place-display", BOOL, Group::Placement, Sink::Bool(|o| &mut o.place_display), Def::Bool(false), 90,
        "Display animation of the placement process"),
    FlagSpec::new("place-iterations", INT, Group::Placement, Sink::Int(|o| &mut o.place_iterations), Def::Int(3000), 91,
        "The number of iterations to run placement for"),
    FlagSpec::new("place-pull", NUM, Group::Placement, Sink::Float(|o| &mut o.place_pull), Def::Float(100.0), 92,
        "Coefficient of linear force that pulls connected nodes together"),
    FlagSpec::new("place-push", NUM, Group::Placement, Sink::Float(|o| &mut o.place_push), Def::Float(0.001), 93,
        "Coefficient of inverse square force that pushes nodes away from others"),
    FlagSpec::new("place-seed", INT, Group::Placement, Sink::Int(|o| &mut o.place_seed), Def::Int(1), 94,
        "Random number seed"),
    FlagSpec::new("place-update-rate", NUM, Group::Placement, Sink::Float(|o| &mut o.place_update_rate), Def::Float(0.005), 95,
        "Maximum length of gradient vectors"),
    FlagSpec::new("place-wall", NUM, Group::Placement, Sink::Float(|o| &mut o.place_wall), Def::Float(0.001), 96,
        "Coefficient of inverse cube force that pushes nodes away from walls"),

    // ------------------------------------------------------------ Diagnostics
    FlagSpec::new("check-narrowing-assignment", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.check_narrowing_assignment), Def::Bool(false), 97,
        "Assert that the input to a narrowing conversion can be represented by the result type"),
    // Field `dump_IR`: `asName` lower-cases a derived name
    // (`Implicit/Global.hs:232`), so the flag is `--dump-ir`, and the
    // reference refuses `--dump-IR` ("Unknown flag", probed 2026-10-09).
    FlagSpec::new("dump-ir", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.dump_ir), Def::Bool(false), 98,
        "Dump IR"),
    FlagSpec::new("dump-opt", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.dump_opt), Def::Bool(false), 99,
        "Dump optimizations"),
    FlagSpec::new("dump-parse", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.dump_parse), Def::Bool(false), 100,
        "Dump abstract syntax tree(s)").modes(Modes::COMPILE_PRETTY),
    FlagSpec::new("dump-program", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.dump_program), Def::Bool(false), 101,
        "Dump desugared abstract syntax tree"),
    FlagSpec::new("dump-types", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.dump_types), Def::Bool(false), 102,
        "Dump abstract syntax tree with type annotations"),
    FlagSpec::new("dump-source", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.dump_source), Def::Bool(false), 103,
        "Dump desugared source code of the program"),
    FlagSpec::new("frontend-passes", "[=INT]", Group::Diagnostics, Sink::OptInt(|o| &mut o.passes), Def::MaxInt, 104,
        "Subset of frontend passes to run, default all").explicit(),
    FlagSpec::new("template-iterations", "[=INT]", Group::Diagnostics, Sink::OptInt(|o| &mut o.template_iterations), Def::MaxInt, 105,
        "Number of iterations of template instantiation to run, default all").explicit(),
    FlagSpec::new("template-passes", "[=INT]", Group::Diagnostics, Sink::OptInt(|o| &mut o.template_passes), Def::MaxInt, 106,
        "Subset of desugaring passes to run after the final template instantiation iteration specified via --template-iterations, default all").explicit(),
    FlagSpec::new("write-ir-post-opt", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.write_ir_post_opt), Def::Bool(false), 107,
        "Serialize IR to json after optimizations"),
    FlagSpec::new("write-circt-ir", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.write_circt_ir), Def::Bool(false), 108,
        "Serialize to CIRCT IR before CIRCT passes"),
    FlagSpec::new("no-asserts", BOOL, Group::Diagnostics, Sink::Bool(|o| &mut o.no_asserts), Def::Bool(false), 109,
        "Disable asserts"),

    // --------------------------------------------------------------- Warnings
    FlagSpec::new("Werror", BOOL, Group::Warnings, Sink::Bool(|o| &mut o.warnings_as_errors), Def::Bool(false), 110,
        "Treat warnings as errors").explicit(),
    FlagSpec::new("Wall", NONE, Group::Warnings, Sink::Warnings(&[Enable(Conversion), Enable(TransactionSize)]), Def::None, 119,
        "Enable all warnings").explicit(),
    FlagSpec::new("Wconversion", NONE, Group::Warnings, Sink::Warnings(&[Enable(Conversion)]), Def::None, 123,
        "Warn for implicit conversions that may alter a value").explicit(),
    FlagSpec::new("Wdeprecated", NONE, Group::Warnings, Sink::Warnings(&[Enable(Deprecated)]), Def::None, 124,
        "Warn for deprecated features").explicit(),
    FlagSpec::new("Winitializer-overrides", NONE, Group::Warnings, Sink::Warnings(&[Enable(InitializerOverrides)]), Def::None, 125,
        "Warn for overriding designated initializers").explicit(),
    FlagSpec::new("Wredundant-import", NONE, Group::Warnings, Sink::Warnings(&[Enable(RedundantImport)]), Def::None, 126,
        "Warn for redundant imports").explicit(),
    FlagSpec::new("Wshadow", NONE, Group::Warnings, Sink::Warnings(&[Enable(Shadow)]), Def::None, 127,
        "Warn for shadowing declarations").explicit(),
    FlagSpec::new("Wtransaction-size", NONE, Group::Warnings, Sink::Warnings(&[Enable(TransactionSize)]), Def::None, 128,
        "Warn for missing [[transaction_size]] attribute at calls to functions with the [[last]] attribute").explicit(),
    FlagSpec::new("Wno-conversion", NONE, Group::Warnings, Sink::Warnings(&[Disable(Conversion)]), Def::None, 130,
        "Don't warn for implicit conversions that may alter a value").explicit(),
    FlagSpec::new("Wno-deprecated", NONE, Group::Warnings, Sink::Warnings(&[Disable(Deprecated)]), Def::None, 131,
        "Don't warn for deprecated features").explicit(),
    FlagSpec::new("Wno-initializer-overrides", NONE, Group::Warnings, Sink::Warnings(&[Disable(InitializerOverrides)]), Def::None, 132,
        "Don't warn for overriding designated initializers").explicit(),
    FlagSpec::new("Wno-redundant-import", NONE, Group::Warnings, Sink::Warnings(&[Disable(RedundantImport)]), Def::None, 133,
        "Don't warn for redundant imports").explicit(),
    FlagSpec::new("Wno-shadow", NONE, Group::Warnings, Sink::Warnings(&[Disable(Shadow)]), Def::None, 134,
        "Don't warn for shadowing declarations").explicit(),
    FlagSpec::new("Wno-transaction-size", NONE, Group::Warnings, Sink::Warnings(&[Disable(TransactionSize)]), Def::None, 135,
        "Don't warn for missing [[transaction_size]] attribute at calls to functions with the [[last]] attribute").explicit(),

    // ------------------------------------------------------------ pretty mode
    FlagSpec::new("line-width", INT, Group::Undeclared, Sink::Int(|o| &mut o.line_width), Def::Int(80), 147,
        "Width of the page in characters; default 80").short('w').modes(Modes::PRETTY),
    FlagSpec::new("indent", INT, Group::Undeclared, Sink::Int(|o| &mut o.indent), Def::Int(4), 148,
        "Number of spaces per indentation level; default 4").short('i').modes(Modes::PRETTY),
    FlagSpec::new("ribbon", NUM, Group::Undeclared, Sink::Float(|o| &mut o.ribbon), Def::Float(1.0), 149,
        "Fraction of line that should be filled before trying to break it up; default 1.0").short('r').modes(Modes::PRETTY),
    FlagSpec::new("layout", "LAYOUT", Group::Undeclared, Sink::Layout, Def::Enum("pretty"), 150,
        "Layout variant to be used: pretty or smart").short('l').modes(Modes::PRETTY),

    // ----------------------------------------------------------- cmdargs built-ins
    FlagSpec::new("help", "[=FORMAT]", Group::Common, Sink::Builtin(Builtin::Help), Def::None, 158,
        "Display help message").short('?').modes(Modes::ALL),
    FlagSpec::new("version", NONE, Group::Common, Sink::Builtin(Builtin::Version), Def::None, 158,
        "Print version information").short('V').modes(Modes::ALL),
    FlagSpec::new("numeric-version", NONE, Group::Common, Sink::Builtin(Builtin::NumericVersion), Def::None, 158,
        "Print just the version number").modes(Modes::ALL),
    FlagSpec::new("verbose", NONE, Group::Common, Sink::Builtin(Builtin::Verbose), Def::None, 158,
        "Loud verbosity").short('v').modes(Modes::ALL),
    FlagSpec::new("quiet", NONE, Group::Common, Sink::Builtin(Builtin::Quiet), Def::None, 158,
        "Quiet verbosity").short('q').modes(Modes::ALL),
];

/// Flags declared by `compile` mode (the 90 one-flag fields + 13 warning switches).
pub fn compile_flags() -> impl Iterator<Item = &'static FlagSpec> {
    FLAGS.iter().filter(|f| f.group != Group::Common && f.modes.accepts(super::options::Mode::Compile))
}

/// `compile`'s flags as `cmdargs` holds them: in the order of the
/// `Compile` record's fields (`Options.hs:39-130`), not of the record
/// literal in `CmdArgs.hs` — `mode_` reads the constructor's `fields`
/// (`Implicit/Local.hs:128-129`) — with `warnings_state`'s alternatives in
/// their `enum` order (`CmdArgs.hs:119-135`). `--help` lists them in this
/// order within each group.
pub static COMPILE_ORDER: &[&str] = &[
    "additional-latency", "auto-pipeline-cross-region", "auto-pipeline-ratio", "backend",
    "base-library", "block-ram-util-threshold", "carry-chain-width", "carry-select", "clock-gating",
    "clock-gating-threshold", "code-coverage", "code-coverage-mux-threshold",
    "conditional-memory-reads", "debug", "deep-ram-util-threshold", "define", "dgml",
    "dgml-detailed", "check-narrowing-assignment", "dump-ir", "dump-memory-metadata", "dump-opt",
    "dump-parse", "dump-program", "dump-source", "dump-types", "fifo-merge-distance", "fifo-to-dsp",
    "fifo-write-delay", "file-list", "force-mem-to-array", "frequency", "identifier-length",
    "import-dir", "io-register-delay", "jobs", "max-block-ram", "max-deep-ram",
    "max-register-ratio", "max-mul-src-width", "max-mux-sources", "max-stack-depth",
    "max-threads-default", "max-threads-limit", "mem-to-reg", "no-asserts", "no-debug-view",
    "no-fifo-stutter", "no-implicit-base", "no-reset-fanout", "no-verilog-header", "optimize",
    "output", "parse-docs", "frontend-passes", "place-display", "place-iterations", "place-pull",
    "place-push", "place-seed", "place-update-rate", "place-wall", "register-ratio",
    "reset-cycles", "reset-fan-out-cycles", "reset-duration-cycles", "resource-usage",
    "return-stages", "rmw-hardened-max-height", "rmw-hardened-max-width",
    "rmw-hardened-write-delay", "rmw-memory-favor-area", "rmw-memory-read-delay",
    "rmw-memory-write-delay", "schedule-limit", "semaphore-delay", "sparse-reg-opt", "stall",
    "stallable-pipelines", "strict-memory-read-write", "strip-debug-symbols", "target-device",
    "template-iterations", "template-passes", "using",
    "Wall", "Wconversion", "Wdeprecated", "Winitializer-overrides", "Wredundant-import",
    "Wshadow", "Wtransaction-size", "Wno-conversion", "Wno-deprecated",
    "Wno-initializer-overrides", "Wno-redundant-import", "Wno-shadow", "Wno-transaction-size",
    "Werror", "work-list-size", "write-circt-ir", "write-ir-post-opt", "skip-circt-lowering",
];

/// The `PrettyPrint` record's fields (`Options.hs:24-36`).
pub static PRETTY_ORDER: &[&str] = &[
    "base-library", "define", "dump-parse", "import-dir", "indent", "layout", "line-width",
    "no-implicit-base", "parse-docs", "ribbon", "target-device", "using",
];

/// The `ListDeps` record's fields (`Options.hs:133-141`).
pub static LIST_DEPS_ORDER: &[&str] = &[
    "base-library", "define", "file-list", "import-dir", "no-implicit-base", "parse-docs",
    "target-device", "using",
];

/// The built-ins, appended to every mode's own flags in this order
/// (`extraFlags`, `Implicit/Global.hs:161-171`).
pub static BUILTIN_ORDER: &[&str] = &["help", "version", "numeric-version", "verbose", "quiet"];

/// The flag spelled `long` (its primary name).
pub fn by_long(long: &str) -> Option<&'static FlagSpec> {
    FLAGS.iter().find(|f| f.long == long)
}

/// A mode's flags in `cmdargs`' order: its record's fields, then the
/// built-ins.
pub fn mode_flags(mode: super::options::Mode) -> Vec<&'static FlagSpec> {
    use super::options::Mode;
    let own = match mode {
        Mode::Compile => COMPILE_ORDER,
        Mode::Pretty => PRETTY_ORDER,
        Mode::ListDeps => LIST_DEPS_ORDER,
    };
    own.iter().chain(BUILTIN_ORDER).map(|l| by_long(l).expect("every ordered name is in FLAGS")).collect()
}

/// The number of user-visible flag spellings this table implements, counted the
/// way `docs/spec/cli.md` §1.0 counts them: `compile` (103) + `pretty` (12) +
/// `list-deps` (8) + built-ins (5) = 128.
///
/// A mode-shared row counts once per mode, because §1.0 counts the eight
/// inherited `pretty` flags separately from their `compile` twins, and
/// `list-deps`' eight the same way. The five built-ins are counted once in
/// total, as its "both modes" line does.
pub fn spelling_count() -> usize {
    FLAGS
        .iter()
        .map(|f| if f.group == Group::Common { 1 } else { f.modes.count() })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::options::Mode;

    #[test]
    fn the_inventory_matches_the_spec_counts() {
        let compile: Vec<_> = FLAGS.iter().filter(|f| f.modes.accepts(Mode::Compile)).collect();
        let pretty: Vec<_> = FLAGS.iter().filter(|f| f.modes.accepts(Mode::Pretty)).collect();
        let builtins = FLAGS.iter().filter(|f| f.group == Group::Common).count();

        // cli.md §1.0: 90 one-flag fields + 13 warnings_state switches.
        let compile_only = compile.len() - builtins;
        assert_eq!(compile_only, 103, "compile-mode flags");
        let warning_switches =
            FLAGS.iter().filter(|f| matches!(f.sink, Sink::Warnings(_))).count();
        assert_eq!(warning_switches, 13, "warnings_state switches");
        assert_eq!(compile_only - warning_switches, 90, "one-flag fields");

        // cli.md §1.2 + §1.3: 4 declared + 8 inherited.
        assert_eq!(pretty.len() - builtins, 12, "pretty-mode flags");
        // `ListDeps`' nine fields less `files` (`Options.hs:133-141`).
        let list_deps = FLAGS.iter().filter(|f| f.modes.accepts(Mode::ListDeps)).count();
        assert_eq!(list_deps - builtins, 8, "list-deps-mode flags");
        assert_eq!(builtins, 5, "cmdargs built-ins");
        assert_eq!(spelling_count(), 128, "total user-visible spellings");
    }

    /// Each mode's order names exactly the flags the mode accepts, once —
    /// so `--help`, which lists a mode from its order, can neither drop a
    /// flag the parser accepts nor show one it refuses.
    #[test]
    fn every_mode_order_names_exactly_the_flags_of_its_mode() {
        for mode in [Mode::Compile, Mode::Pretty, Mode::ListDeps] {
            let mut ordered: Vec<&str> = mode_flags(mode).iter().map(|f| f.long).collect();
            let mut accepted: Vec<&str> = FLAGS.iter().filter(|f| f.modes.accepts(mode)).map(|f| f.long).collect();
            ordered.sort_unstable();
            accepted.sort_unstable();
            let before = ordered.len();
            ordered.dedup();
            assert_eq!(before, ordered.len(), "{mode:?}: a flag ordered twice");
            assert_eq!(ordered, accepted, "{mode:?}");
        }
    }

    /// The short letters, re-derived per mode by `cmdargs`' own rule rather
    /// than read off its help: a non-`explicit` flag with no one-letter name
    /// wants the first letter of each of its names, and gets those no other
    /// flag of the mode holds or also wants (`assignNames`, `names`,
    /// `Implicit/Global.hs:214-246`). The declared letters are the
    /// `&= name "x"` of `CmdArgs.hs:38,63,64,86,147` and the built-ins'.
    #[test]
    fn short_letters_are_cmdargs_own() {
        let declared = |f: &FlagSpec| match f.long {
            "frequency" => Some('f'),
            "optimize" => Some('O'),
            "define" => Some('d'),
            "using" => Some('u'),
            "line-width" => Some('w'),
            "help" => Some('?'),
            "version" => Some('V'),
            "verbose" => Some('v'),
            "quiet" => Some('q'),
            _ => None,
        };
        for mode in [Mode::Compile, Mode::Pretty, Mode::ListDeps] {
            let flags = mode_flags(mode);
            let wants: Vec<Vec<char>> = flags
                .iter()
                .map(|f| {
                    if f.explicit || f.group == Group::Common || declared(f).is_some() {
                        return Vec::new();
                    }
                    let mut w: Vec<char> = f.long_names().filter_map(|n| n.chars().next()).collect();
                    w.sort_unstable();
                    w.dedup();
                    w
                })
                .collect();
            let held: Vec<char> = flags.iter().filter_map(|f| declared(f)).collect();
            let all_wants: Vec<char> = wants.iter().flatten().copied().collect();
            for (f, w) in flags.iter().zip(&wants) {
                let got: Vec<char> = w
                    .iter()
                    .copied()
                    .filter(|c| !held.contains(c) && all_wants.iter().filter(|x| *x == c).count() == 1)
                    .collect();
                assert!(got.len() <= 1, "--{} gets two letters in {mode:?}", f.long);
                let derived = declared(f).or(got.first().copied());
                let table = f.short.filter(|_| f.short_modes.accepts(mode));
                assert_eq!(table, derived, "--{}'s short letter in {mode:?}", f.long);
            }
        }
    }

    #[test]
    fn every_long_name_is_unique() {
        let mut names: Vec<&str> = FLAGS.iter().flat_map(|f| f.long_names()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate long flag name");
        // 103 compile + 4 pretty-only + 5 built-in long names, plus
        // `--target`; `list-deps` declares none of its own.
        assert_eq!(names.len(), 113);
    }

    #[test]
    fn declared_defaults_match_the_struct() {
        let mut o = LegacyOptions::default();
        for f in FLAGS {
            let actual = match (f.sink, f.default) {
                (Sink::Int(get), Def::Int(n)) => (*get(&mut o) == n, format!("{}", get(&mut o))),
                (Sink::Int(get), Def::MaxInt) | (Sink::OptInt(get), Def::MaxInt) => {
                    (*get(&mut o) == i64::MAX, format!("{}", get(&mut o)))
                }
                (Sink::Float(get), Def::Float(x)) => {
                    (*get(&mut o) == x, format!("{}", get(&mut o)))
                }
                (Sink::Bool(get), Def::Bool(b)) => (*get(&mut o) == b, format!("{}", get(&mut o))),
                (Sink::Str(get), Def::Text(t)) => (get(&mut o) == t, get(&mut o).clone()),
                (Sink::List(get), Def::List) => (get(&mut o).is_empty(), String::new()),
                (Sink::MaybeInt(get), Def::Unset) => (get(&mut o).is_none(), String::new()),
                (Sink::Backend, Def::Enum(e)) => {
                    (o.backend.show().eq_ignore_ascii_case(e), o.backend.show().to_owned())
                }
                (Sink::Layout, Def::Enum(e)) => {
                    (o.layout.show().eq_ignore_ascii_case(e), o.layout.show().to_owned())
                }
                (Sink::Warnings(_) | Sink::Builtin(_), Def::None) => (true, String::new()),
                (sink, def) => panic!("--{}: sink {sink:?} does not match default {def:?}", f.long),
            };
            assert!(actual.0, "--{} declares default {:?} but the struct has {}", f.long, f.default, actual.1);
        }
    }

    #[test]
    fn source_lines_are_in_declaration_order_within_compile_mode() {
        let lines: Vec<u32> = compile_flags().map(|f| f.src).collect();
        let mut sorted = lines.clone();
        sorted.sort_unstable();
        assert_eq!(lines, sorted, "table order should follow CmdArgs.hs");
    }
}
