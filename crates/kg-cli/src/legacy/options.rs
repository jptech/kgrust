//! The option record the legacy front end fills in.
//!
//! One struct for every mode, mirroring `Options` in
//! `compiler/hs/app/Options.hs:18-148`, which is one Haskell data type whose
//! mode constructors (`Compile`, `PrettyPrint`, `ListDeps`) share a field
//! namespace. Field names, types and defaults are taken from the record
//! literal at `compiler/hs/app/Options/CmdArgs.hs:18-165`; every default is
//! cross-checked against the flag table by
//! `flags::tests::declared_defaults_match_the_struct`. The backend's share is
//! marshalled by [`option_values`], the frontend's by [`settings_from`].

use kg_diag::{WarningKind, WarningSet};

/// Which of the three `cmdargs` modes is running
/// (`modes [compile &= auto, pretty, listDeps]`, `Options/CmdArgs.hs:167`).
///
/// `compile` is `&= auto`, so it is the default and `kanagawa foo.k` means
/// `kanagawa compile foo.k`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Compile,
    Pretty,
    /// Parse and import resolution only; prints the program's files
    /// (`Main.hs:111-125`).
    ListDeps,
}

impl Mode {
    /// In `cmdargs`' order, which is the order `--help` lists them in.
    pub const ALL: [Mode; 3] = [Mode::Compile, Mode::Pretty, Mode::ListDeps];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Compile => "compile",
            Mode::Pretty => "pretty",
            Mode::ListDeps => "list-deps",
        }
    }

    /// The mode's bit in [`super::flags::Modes`].
    pub(crate) fn bit(self) -> u8 {
        match self {
            Mode::Compile => 1,
            Mode::Pretty => 2,
            Mode::ListDeps => 4,
        }
    }

    /// `help` (`CmdArgs.hs:139,153,158`).
    pub fn help(self) -> &'static str {
        match self {
            Mode::Compile => "Compile Kanagawa program",
            Mode::Pretty => "Parse and pretty print source file(s)",
            Mode::ListDeps => {
                "List the transitive .k source files needed to compile (parse only; no frontend or codegen)"
            }
        }
    }

    /// `details` — `modeHelpSuffix` (`CmdArgs.hs:140-143,159-165`).
    pub fn details(self) -> &'static [&'static str] {
        match self {
            Mode::Compile => &[
                "Usage example:",
                "    Compile source.k and generate SystemVerilog RTL:",
                "      kanagawa --backend=sv source.k",
            ],
            Mode::Pretty => &[],
            Mode::ListDeps => &[
                "Usage example:",
                "    Print the dependency list for source.k to stdout:",
                "      kanagawa list-deps source.k",
                "    Write the dependency list to a file:",
                "      kanagawa list-deps --file-list=deps.txt source.k",
            ],
        }
    }
}

/// `--backend` / `--target`. `show` gives `"Sv"` / `"None"`, which is the exact
/// string `Codegen` receives (`compiler/hs/app/ParseTree.hs:33`) and the value
/// of `.options::backend` (`Parser/IO.hs:59`).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Backend {
    #[default]
    None,
    Sv,
}

impl Backend {
    /// `show backend` — capitalised, because the C++ side compares it
    /// case-insensitively against `"sv"` (`compiler/cpp/kanagawa.cpp:30-33`).
    pub fn show(self) -> &'static str {
        match self {
            Backend::None => "None",
            Backend::Sv => "Sv",
        }
    }

    /// `cmdargs` matches enum values case-insensitively (§11 Q9, UNVERIFIED);
    /// `--backend=sv` is the only spelling used in-tree.
    pub fn parse(text: &str) -> Option<Backend> {
        match text.to_ascii_lowercase().as_str() {
            "sv" => Some(Backend::Sv),
            "none" => Some(Backend::None),
            _ => None,
        }
    }
}

/// `pretty --layout`. `Pretty` selects `layoutPretty`, `Smart` `layoutSmart`
/// (`compiler/hs/app/Main.hs:52-55`).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Layout {
    #[default]
    Pretty,
    Smart,
}

impl Layout {
    pub fn show(self) -> &'static str {
        match self {
            Layout::Pretty => "Pretty",
            Layout::Smart => "Smart",
        }
    }

    pub fn parse(text: &str) -> Option<Layout> {
        match text.to_ascii_lowercase().as_str() {
            "pretty" => Some(Layout::Pretty),
            "smart" => Some(Layout::Smart),
            _ => None,
        }
    }
}

/// One entry of `warnings_state :: [WarningState WarningKind]`
/// (`compiler/hs/app/Options.hs:125`, `Warning.hs:37`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WarningState {
    Enable(WarningKind),
    Disable(WarningKind),
}

/// The four warnings the reference enables before any flag is seen
/// (`Options/CmdArgs.hs:112-117`). Command-line switches **append** to this
/// list; they do not replace it.
pub const DEFAULT_WARNING_STATE: &[WarningState] = &[
    WarningState::Enable(WarningKind::RedundantImport),
    WarningState::Enable(WarningKind::Deprecated),
    WarningState::Enable(WarningKind::InitializerOverrides),
    WarningState::Enable(WarningKind::Shadow),
];

/// `enabledWarnings = foldl (flip update) mempty` (`Warning.hs:46-50`): a left
/// fold from the empty set, so the last switch mentioning a kind wins.
pub fn enabled_warnings(states: &[WarningState]) -> WarningSet {
    let mut set = WarningSet::none();
    for state in states {
        match *state {
            WarningState::Enable(kind) => set.enable(kind),
            WarningState::Disable(kind) => set.disable(kind),
        }
    }
    set
}

/// `--verbose` / `--quiet`, the pair `cmdargs` adds via `verbosity`
/// (`Options/CmdArgs.hs:158`). Marshalled to `CodeGenOptions::_verbosity`
/// (`Options/FFI.hsc:36`).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Verbosity {
    Quiet,
    #[default]
    Normal,
    Loud,
}

/// Every legacy option, in the declaration order of `Options/CmdArgs.hs`.
///
/// `#[allow(clippy::struct_excessive_bools)]` is unnecessary — the lint is
/// pedantic — but the shape is deliberate: 90 flat fields is what the reference
/// has, and collapsing them into groups would break the one-to-one mapping that
/// makes this auditable against the spec table.
#[derive(Clone, PartialEq, Debug)]
pub struct LegacyOptions {
    pub mode: Mode,
    pub verbosity: Verbosity,
    /// Positional `FILE…` (`CmdArgs.hs:20`).
    pub files: Vec<String>,

    // ------------------------------------------------------------------ Logic
    pub additional_latency: i64,
    pub auto_pipeline_scale: i64,
    pub auto_pipeline_cross_region: i64,
    pub carry_chain_width: i64,
    pub carry_select: bool,
    pub rmw_memory_write_delay: i64,
    pub rmw_memory_read_delay: i64,
    pub rmw_hardened_write_delay: i64,
    pub rmw_hardened_max_width: i64,
    pub rmw_hardened_max_height: i64,
    pub rmw_memory_favor_area: bool,
    pub strict_memory_read_write: bool,
    pub io_register_delay: i64,
    pub mem_to_reg: f64,
    pub fifo_to_dsp: i64,
    pub fifo_write_delay: i64,
    pub fifo_merge_distance: i64,
    pub frequency: i64,
    pub max_block_ram: i64,
    pub block_ram_util_threshold: i64,
    pub max_deep_ram: i64,
    pub deep_ram_util_threshold: i64,
    pub max_mux_sources: i64,
    pub max_mul_src_width: i64,
    pub max_threads_default: i64,
    pub max_threads_limit: i64,
    pub schedule_limit: i64,
    pub register_ratio: i64,
    pub max_register_ratio: i64,
    pub no_reset_fanout: bool,
    pub reset_cycles: i64,
    pub reset_fan_out_cycles: i64,
    pub reset_duration_cycles: i64,
    pub return_stages: i64,
    pub stallable_pipelines: bool,
    pub clock_gating: i64,
    pub clock_gating_threshold: i64,
    pub conditional_memory_reads: bool,
    pub semaphore_delay: i64,
    pub sparse_reg_opt: bool,

    // --------------------------------------------------------------- Compiler
    /// `Maybe Int`: unset means "one job per processor" (`Main.hs:62-68`).
    pub jobs: Option<i64>,
    /// Repeatable and **order significant** (`Parser/IO.hs:107`).
    pub import_dir: Vec<String>,
    /// Raw `name#val` strings, split by [`split_name_value`].
    pub define: Vec<String>,
    /// Raw `name#type` strings, split by [`split_name_value`].
    pub using: Vec<String>,
    pub no_implicit_base: bool,
    pub base_library: String,
    pub parse_docs: bool,

    // ----------------------------------------------------------------- Output
    pub output: String,
    pub dgml: String,
    pub dgml_detailed: String,
    pub resource_usage: String,
    pub file_list: String,
    pub identifier_length: i64,
    pub skip_circt_lowering: bool,

    // -------------------------------------------------------- Code generation
    pub backend: Backend,
    pub debug: bool,
    pub dump_memory_metadata: bool,
    pub strip_debug_symbols: bool,
    pub force_mem_to_array: i64,
    pub max_stack_depth: i64,
    pub no_fifo_stutter: bool,
    pub no_debug_view: bool,
    pub no_verilog_header: bool,
    pub code_coverage: bool,
    pub code_coverage_mux_threshold: i64,
    pub optimize: i64,
    pub target_device: String,
    pub stall: i64,
    pub work_list_size: i64,

    // -------------------------------------------------------------- Placement
    pub place_display: bool,
    pub place_iterations: i64,
    pub place_pull: f64,
    pub place_push: f64,
    pub place_seed: i64,
    pub place_update_rate: f64,
    pub place_wall: f64,

    // ------------------------------------------------------------ Diagnostics
    pub check_narrowing_assignment: bool,
    /// Haskell field `dump_IR`; the derived flag spelling is `--dump-IR`
    /// (§11 Q3 — `cmdargs` maps `_` to `-` and does not lowercase).
    pub dump_ir: bool,
    pub dump_opt: bool,
    pub dump_parse: bool,
    pub dump_program: bool,
    pub dump_types: bool,
    pub dump_source: bool,
    /// `--frontend-passes` (Haskell field `passes`). Default `maxBound::Int`.
    pub passes: i64,
    /// `--template-iterations`. Default `maxBound::Int`.
    pub template_iterations: i64,
    /// `--template-passes`. Default `maxBound::Int`. A no-op unless
    /// `--template-iterations` is also given (`Template.hs:63`).
    pub template_passes: i64,
    pub write_ir_post_opt: bool,
    pub write_circt_ir: bool,
    pub no_asserts: bool,

    // --------------------------------------------------------------- Warnings
    /// `--Werror`.
    pub warnings_as_errors: bool,
    /// The accumulated switch list, defaults first.
    pub warnings_state: Vec<WarningState>,

    // ----------------------------------------------------------------- pretty
    pub line_width: i64,
    pub indent: i64,
    pub ribbon: f64,
    pub layout: Layout,
}

impl Default for LegacyOptions {
    fn default() -> LegacyOptions {
        LegacyOptions {
            mode: Mode::Compile,
            verbosity: Verbosity::Normal,
            files: Vec::new(),

            additional_latency: 1,
            auto_pipeline_scale: 0,
            auto_pipeline_cross_region: 0,
            carry_chain_width: 40,
            carry_select: false,
            rmw_memory_write_delay: 1,
            rmw_memory_read_delay: 0,
            rmw_hardened_write_delay: 1,
            rmw_hardened_max_width: i64::MAX,
            rmw_hardened_max_height: 0,
            rmw_memory_favor_area: false,
            strict_memory_read_write: false,
            io_register_delay: 0,
            mem_to_reg: 0.0,
            fifo_to_dsp: 0,
            fifo_write_delay: 1,
            fifo_merge_distance: 4,
            frequency: 0,
            max_block_ram: -1,
            block_ram_util_threshold: 20,
            max_deep_ram: -1,
            deep_ram_util_threshold: 20,
            max_mux_sources: 16,
            max_mul_src_width: 32,
            max_threads_default: 512,
            max_threads_limit: 512,
            schedule_limit: 4096,
            register_ratio: 2,
            max_register_ratio: 0,
            no_reset_fanout: false,
            reset_cycles: 3,
            reset_fan_out_cycles: 1,
            reset_duration_cycles: 0,
            return_stages: 3,
            stallable_pipelines: false,
            clock_gating: 0,
            clock_gating_threshold: 5,
            conditional_memory_reads: false,
            semaphore_delay: 3,
            sparse_reg_opt: false,

            jobs: None,
            import_dir: Vec::new(),
            define: Vec::new(),
            using: Vec::new(),
            no_implicit_base: false,
            base_library: "base.k".to_owned(),
            parse_docs: false,

            output: String::new(),
            dgml: String::new(),
            dgml_detailed: String::new(),
            resource_usage: String::new(),
            file_list: String::new(),
            identifier_length: 100,
            skip_circt_lowering: false,

            backend: Backend::None,
            debug: false,
            dump_memory_metadata: false,
            strip_debug_symbols: false,
            force_mem_to_array: 0,
            max_stack_depth: 64,
            no_fifo_stutter: false,
            no_debug_view: false,
            no_verilog_header: false,
            code_coverage: false,
            code_coverage_mux_threshold: 0,
            optimize: 2,
            target_device: "mock".to_owned(),
            stall: 0,
            work_list_size: 0,

            place_display: false,
            place_iterations: 3000,
            place_pull: 100.0,
            place_push: 0.001,
            place_seed: 1,
            place_update_rate: 0.005,
            place_wall: 0.001,

            check_narrowing_assignment: false,
            dump_ir: false,
            dump_opt: false,
            dump_parse: false,
            dump_program: false,
            dump_types: false,
            dump_source: false,
            passes: i64::MAX,
            template_iterations: i64::MAX,
            template_passes: i64::MAX,
            write_ir_post_opt: false,
            write_circt_ir: false,
            no_asserts: false,

            warnings_as_errors: false,
            warnings_state: DEFAULT_WARNING_STATE.to_vec(),

            line_width: 80,
            indent: 4,
            ribbon: 1.0,
            layout: Layout::Pretty,
        }
    }
}

impl LegacyOptions {
    /// The warning set the frontend would use, after folding the switch list.
    pub fn enabled_warnings(&self) -> WarningSet {
        enabled_warnings(&self.warnings_state)
    }

    /// `--define` split into pairs, in command-line order.
    pub fn defines(&self) -> Vec<(&str, &str)> {
        self.define.iter().map(|d| split_name_value(d)).collect()
    }

    /// `--using` split into pairs, in command-line order.
    pub fn usings(&self) -> Vec<(&str, &str)> {
        self.using.iter().map(|u| split_name_value(u)).collect()
    }

    /// The text of the synthetic `.cmdargs` module (`Parser/IO.hs:37-55`):
    /// `const auto <name> = <val>;` per `--define`, then `using <name> = <type>;`
    /// per `--using`, both in command-line order.
    pub fn cmdargs_module_source(&self) -> String {
        let mut out = String::from("module .cmdargs{module .cmdargs}\n");
        for (name, value) in self.defines() {
            out.push_str(&format!("const auto {name} = {value};\n"));
        }
        for (name, ty) in self.usings() {
            out.push_str(&format!("using {name} = {ty};\n"));
        }
        out
    }

    /// The search path for `import`, for a file living in `dir`
    /// (`Parser/IO.hs:103,107`): the importing file's own directory, then every
    /// `<import-dir>/device/<target-device>`, then every plain `<import-dir>`.
    pub fn import_search_path(&self, dir: &str) -> Vec<String> {
        let mut path = Vec::with_capacity(1 + self.import_dir.len() * 2);
        path.push(dir.to_owned());
        path.extend(self.import_dirs());
        path
    }

    /// `importDirs` (`Parser/IO.hs:110`): every `<import-dir>/device/<target-device>`,
    /// then every plain `<import-dir>`, joined by `</>` as spelled — which
    /// is how "Can't find file" lists them (`:108`).
    pub fn import_dirs(&self) -> Vec<String> {
        let device = |d: &str| combine(&combine(d, "device"), &self.target_device);
        self.import_dir.iter().map(|d| device(d)).chain(self.import_dir.iter().cloned()).collect()
    }
}

/// `System.FilePath`'s `</>` on POSIX: an absolute right side wins, an empty
/// left side vanishes, and a left side that already ends in `/` gets no
/// second one.
pub fn combine(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_owned()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `canonicalizePath` (`Parser/IO.hs:104`): absolute, symlinks resolved, and
/// as given when the path does not exist — the reference only ever
/// canonicalizes a path it has found.
pub fn canonical_path(path: &str) -> String {
    std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| path.to_owned())
}

/// [`LegacyOptions`] as the `Options` the backend takes (`Options/FFI.hsc`),
/// field for field, the three negated flags negated back.
pub fn option_values(opts: &LegacyOptions, cmd_args: &str) -> kg_backend_sys::options::OptionValues {
    kg_backend_sys::options::OptionValues {
        frequency: opts.frequency,
        target_device: opts.target_device.clone(),
        cmd_args: cmd_args.to_owned(),
        file_names: Vec::new(),
        additional_latency: opts.additional_latency,
        reset_duration_cycles: opts.reset_duration_cycles,
        auto_pipeline_scale: opts.auto_pipeline_scale,
        auto_pipeline_cross_region: opts.auto_pipeline_cross_region,
        max_block_ram: opts.max_block_ram,
        block_ram_util_threshold: opts.block_ram_util_threshold,
        max_deep_ram: opts.max_deep_ram,
        deep_ram_util_threshold: opts.deep_ram_util_threshold,
        max_mux_sources: opts.max_mux_sources,
        max_mul_src_width: opts.max_mul_src_width,
        max_threads_default: opts.max_threads_default,
        max_threads_limit: opts.max_threads_limit,
        schedule_limit: opts.schedule_limit,
        carry_chain_width: opts.carry_chain_width,
        rmw_memory_write_delay: opts.rmw_memory_write_delay,
        rmw_memory_read_delay: opts.rmw_memory_read_delay,
        rmw_hardened_write_delay: opts.rmw_hardened_write_delay,
        rmw_hardened_max_width: opts.rmw_hardened_max_width,
        rmw_hardened_max_height: opts.rmw_hardened_max_height,
        rmw_memory_favor_area: opts.rmw_memory_favor_area,
        strict_memory_read_write: opts.strict_memory_read_write,
        io_register_delay: opts.io_register_delay,
        mem_to_reg: opts.mem_to_reg as f32,
        fifo_to_dsp: opts.fifo_to_dsp,
        check_narrowing_assignment: opts.check_narrowing_assignment,
        dump_ir: opts.dump_ir,
        dump_opt: opts.dump_opt,
        write_ir_post_opt: opts.write_ir_post_opt,
        write_circt_ir: opts.write_circt_ir,
        skip_circt_lowering: opts.skip_circt_lowering,
        fifo_write_delay: opts.fifo_write_delay,
        fifo_merge_distance: opts.fifo_merge_distance,
        no_verilog_header: opts.no_verilog_header,
        optimize: opts.optimize,
        register_ratio: opts.register_ratio,
        max_register_ratio: opts.max_register_ratio,
        reset_cycles: opts.reset_cycles,
        reset_fan_out_cycles: opts.reset_fan_out_cycles,
        clock_gating: opts.clock_gating,
        clock_gating_threshold: opts.clock_gating_threshold,
        allow_fifo_stutter: !opts.no_fifo_stutter,
        stallable_pipelines: opts.stallable_pipelines,
        conditional_memory_reads: opts.conditional_memory_reads,
        semaphore_delay: opts.semaphore_delay,
        release_assert: !opts.no_asserts,
        no_debug_view: opts.no_debug_view,
        code_coverage: opts.code_coverage,
        code_coverage_mux_threshold: opts.code_coverage_mux_threshold,
        dump_memory_metadata: opts.dump_memory_metadata,
        identifier_length: opts.identifier_length,
        work_list_size: opts.work_list_size,
        return_stages: opts.return_stages,
        debug: opts.debug,
        strip_debug_symbols: opts.strip_debug_symbols,
        stall: opts.stall,
        force_mem_to_array: opts.force_mem_to_array,
        sparse_reg_opt: opts.sparse_reg_opt,
        carry_select: opts.carry_select,
        max_stack_depth: opts.max_stack_depth,
        reset_fanout: !opts.no_reset_fanout,
        enable_transaction_size_warning: opts.enabled_warnings().is_enabled(WarningKind::TransactionSize),
        enable_deprecation_warnings: opts.enabled_warnings().is_enabled(WarningKind::Deprecated),
        warnings_as_errors: opts.warnings_as_errors,
        verbosity: match opts.verbosity {
            Verbosity::Quiet => 0,
            Verbosity::Normal => 1,
            Verbosity::Loud => 2,
        },
        place_iterations: opts.place_iterations,
        place_seed: opts.place_seed,
        place_pull: opts.place_pull as f32,
        place_push: opts.place_push as f32,
        place_wall: opts.place_wall as f32,
        place_update_rate: opts.place_update_rate as f32,
        place_display: opts.place_display,
    }
}

/// [`LegacyOptions`] as the [`kg_config::Settings`] the query layer takes.
///
/// The reference has no manifest — every one of these is a command-line flag —
/// so this is the whole of the mapping, and each line is a flag the frontend
/// would otherwise silently ignore.
///
/// `root` is the **process working directory**, not a project root: a
/// command-line file resolves against the CWD first (`Parser/IO.hs:80-82`), and
/// `Settings::root` is this toolchain's stand-in for that. `--target-device` and
/// `--base-library` have reference defaults already applied by
/// [`LegacyOptions::default`], so they are copied unconditionally.
pub fn settings_from(opts: &LegacyOptions, cwd: &str) -> kg_config::Settings {
    let mut settings = kg_config::Settings::fallback(kg_base::Utf8PathBuf::from(cwd));
    // Canonical, as every file the reference finds is (`Parser/IO.hs:104`):
    // an import resolved under a directory is then the same path as the
    // command-line root that names the same file.
    settings.import_dirs =
        opts.import_dir.iter().map(|d| kg_base::Utf8PathBuf::from(canonical_path(d))).collect();
    settings.target_device = opts.target_device.clone();
    settings.base_library = Some(kg_base::Utf8PathBuf::from(&opts.base_library));
    settings.implicit_base = !opts.no_implicit_base;
    settings.parse_docs = opts.parse_docs;
    settings.defines = opts
        .define
        .iter()
        .map(|d| {
            let (n, v) = split_name_value(d);
            (n.to_owned(), v.to_owned())
        })
        .collect();
    settings.using = opts
        .using
        .iter()
        .map(|u| {
            let (n, v) = split_name_value(u);
            (n.to_owned(), v.to_owned())
        })
        .collect();
    settings.warnings = opts.enabled_warnings();
    settings.warnings_as_errors = opts.warnings_as_errors;
    settings.backend = kg_config::BackendOptions {
        backend: opts.backend.show().to_owned(),
        stall: opts.stall,
        optimize: opts.optimize,
        max_threads_default: opts.max_threads_default,
        max_threads_limit: opts.max_threads_limit,
    };
    settings
}

/// Splits a `--define` / `--using` argument at its **first** `#` **or** `=`.
///
/// ```haskell
/// nameValue (x:xs')
///    | x == '#' || x == '=' = ([], xs')
///    | otherwise            = let (ys, zs) = nameValue xs' in (x:ys, zs)
/// ```
/// — `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:52-55`.
///
/// Both separators are used in-tree: `--define=verbose#true`
/// (`build/cmake/test_helper.cmake:391`) and `--define=MemoryLength=32`
/// (`tools/chkdoc/chkdoc.cpp:41`). An argument with no separator yields an empty
/// value, which the reference then emits as `const auto NAME = ;` — a parse
/// error, faithfully reproduced by leaving the value empty here.
pub fn split_name_value(arg: &str) -> (&str, &str) {
    match arg.find(['#', '=']) {
        Some(i) => (&arg[..i], &arg[i + 1..]),
        None => (arg, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_separator_is_hash_or_equals_whichever_comes_first() {
        assert_eq!(split_name_value("ELEVEN#11"), ("ELEVEN", "11"));
        assert_eq!(split_name_value("MemoryLength=32"), ("MemoryLength", "32"));
        // First separator wins, and the rest of the string is the value verbatim.
        assert_eq!(split_name_value("a#b=c"), ("a", "b=c"));
        assert_eq!(split_name_value("a=b#c"), ("a", "b#c"));
        // No separator: the whole argument is the name and the value is empty.
        assert_eq!(split_name_value("BARE"), ("BARE", ""));
        assert_eq!(split_name_value(""), ("", ""));
        // A leading separator yields an empty name.
        assert_eq!(split_name_value("#v"), ("", "v"));
    }

    #[test]
    fn warning_defaults_are_the_reference_four() {
        let set = enabled_warnings(DEFAULT_WARNING_STATE);
        assert!(set.is_enabled(WarningKind::RedundantImport));
        assert!(set.is_enabled(WarningKind::Deprecated));
        assert!(set.is_enabled(WarningKind::InitializerOverrides));
        assert!(set.is_enabled(WarningKind::Shadow));
        assert!(!set.is_enabled(WarningKind::Conversion));
        assert!(!set.is_enabled(WarningKind::TransactionSize));
    }

    #[test]
    fn cmdargs_module_lists_defines_before_usings_in_order() {
        let opts = LegacyOptions {
            define: vec!["verbose#true".to_owned(), "tag#0".to_owned()],
            using: vec!["COMPILE_PARAM_1#uint8".to_owned()],
            ..LegacyOptions::default()
        };
        assert_eq!(
            opts.cmdargs_module_source(),
            "module .cmdargs{module .cmdargs}\nconst auto verbose = true;\nconst auto tag = 0;\nusing COMPILE_PARAM_1 = uint8;\n"
        );
    }

    #[test]
    fn device_directories_precede_every_plain_import_dir() {
        let opts = LegacyOptions {
            import_dir: vec!["/root/library".to_owned(), "/root/test/logic".to_owned()],
            target_device: "mock".to_owned(),
            ..LegacyOptions::default()
        };
        assert_eq!(
            opts.import_search_path("/src"),
            vec![
                "/src",
                "/root/library/device/mock",
                "/root/test/logic/device/mock",
                "/root/library",
                "/root/test/logic",
            ]
        );
    }
}
