//! `Options` (`abi.md` §3) from plain Rust values, and the two array shapes
//! the walk passes: a NUL-terminated `const char**` for scopes and file
//! names, and a `Location` by pointer.
//!
//! The widths are pinned here, field by field, because the reference's
//! `#{poke}` writes whatever width the *Haskell* type has (§3.4): `Int` is
//! 8 bytes into a `size_t`, `Bool` is 4 into a `BOOL`, `Float` is 4. A
//! negative `Int` such as `--max-block-ram`'s default `-1` lands as
//! two's-complement `size_t`, which `as usize` reproduces. The whole struct
//! is zeroed first; the reference leaves three fields and every padding run
//! indeterminate (§3.2), and `SetupCodeGenConfig` overwrites the three.

use std::ffi::{CString, c_char};

use crate::raw;

/// Every value the reference pokes into `Options`, named as its
/// `Options.hs` field is (§3, "Haskell expression" column). No defaults live
/// here: the CLI that parsed the flags owns them.
///
/// Serializable because the backend runs in a worker process
/// (`kg-ptbridge::isolate`) and these travel to it; the `f32`s travel as
/// their bits, since a decimal round trip is not guaranteed to land on the
/// same float and JSON has no infinity.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OptionValues {
    // `Options`
    pub frequency: i64,
    /// `_deviceName`, from `--target-device`.
    pub target_device: String,
    /// `_cmdArgs`: the whole command line, `unwords (prog : args)`
    /// (`Main.hs:40`).
    pub cmd_args: String,
    /// `_fileNames`: every source file of the program, in the order the
    /// `Location._fileIndex` of the walk indexes them (`ParseTree.hs:60`).
    pub file_names: Vec<String>,

    // `CodeGenOptions`, in `FFI.hsc` poke order
    pub additional_latency: i64,
    pub reset_duration_cycles: i64,
    pub auto_pipeline_scale: i64,
    pub auto_pipeline_cross_region: i64,
    pub max_block_ram: i64,
    pub block_ram_util_threshold: i64,
    pub max_deep_ram: i64,
    pub deep_ram_util_threshold: i64,
    pub max_mux_sources: i64,
    pub max_mul_src_width: i64,
    pub max_threads_default: i64,
    pub max_threads_limit: i64,
    pub schedule_limit: i64,
    pub carry_chain_width: i64,
    pub rmw_memory_write_delay: i64,
    pub rmw_memory_read_delay: i64,
    pub rmw_hardened_write_delay: i64,
    pub rmw_hardened_max_width: i64,
    pub rmw_hardened_max_height: i64,
    pub rmw_memory_favor_area: bool,
    pub strict_memory_read_write: bool,
    pub io_register_delay: i64,
    #[serde(with = "f32_bits")]
    pub mem_to_reg: f32,
    pub fifo_to_dsp: i64,
    pub check_narrowing_assignment: bool,
    pub dump_ir: bool,
    pub dump_opt: bool,
    pub write_ir_post_opt: bool,
    pub write_circt_ir: bool,
    pub skip_circt_lowering: bool,
    pub fifo_write_delay: i64,
    pub fifo_merge_distance: i64,
    pub no_verilog_header: bool,
    pub optimize: i64,
    pub register_ratio: i64,
    pub max_register_ratio: i64,
    pub reset_cycles: i64,
    pub reset_fan_out_cycles: i64,
    pub clock_gating: i64,
    pub clock_gating_threshold: i64,
    /// Poked as `not no_fifo_stutter`: this is the *positive* value.
    pub allow_fifo_stutter: bool,
    pub stallable_pipelines: bool,
    pub conditional_memory_reads: bool,
    pub semaphore_delay: i64,
    /// Poked as `not no_asserts`: the positive value.
    pub release_assert: bool,
    pub no_debug_view: bool,
    pub code_coverage: bool,
    pub code_coverage_mux_threshold: i64,
    pub dump_memory_metadata: bool,
    pub identifier_length: i64,
    pub work_list_size: i64,
    pub return_stages: i64,
    pub debug: bool,
    pub strip_debug_symbols: bool,
    pub stall: i64,
    pub force_mem_to_array: i64,
    pub sparse_reg_opt: bool,
    pub carry_select: bool,
    pub max_stack_depth: i64,
    /// Poked as `not no_reset_fanout`: the positive value.
    pub reset_fanout: bool,
    pub enable_transaction_size_warning: bool,
    pub enable_deprecation_warnings: bool,
    pub warnings_as_errors: bool,
    /// `_verbosity`, poked in `withOptions` rather than `poke`
    /// (`FFI.hsc:36`): `fromEnum` of `Quiet | Normal | Verbose`.
    pub verbosity: i64,

    // `PlacementOptions`
    pub place_iterations: i64,
    pub place_seed: i64,
    #[serde(with = "f32_bits")]
    pub place_pull: f32,
    #[serde(with = "f32_bits")]
    pub place_push: f32,
    #[serde(with = "f32_bits")]
    pub place_wall: f32,
    #[serde(with = "f32_bits")]
    pub place_update_rate: f32,
    pub place_display: bool,
}

impl OptionValues {
    /// The reference's defaults (`Options/CmdArgs.hs`, mirrored field for
    /// field by `kg-cli`'s `LegacyOptions::default`, which a test there
    /// cross-checks against this), plus what the upstream syntax runner
    /// always passes — `--place-iterations=1` (`syntax-test.py:106`) — and
    /// the mock device. `file_names` and `cmd_args` are the caller's.
    pub fn harness_defaults() -> OptionValues {
        OptionValues {
            frequency: 0,
            target_device: "mock".to_owned(),
            cmd_args: String::new(),
            file_names: Vec::new(),
            additional_latency: 1,
            reset_duration_cycles: 0,
            auto_pipeline_scale: 0,
            auto_pipeline_cross_region: 0,
            max_block_ram: -1,
            block_ram_util_threshold: 20,
            max_deep_ram: -1,
            deep_ram_util_threshold: 20,
            max_mux_sources: 16,
            max_mul_src_width: 32,
            max_threads_default: 512,
            max_threads_limit: 512,
            schedule_limit: 4096,
            carry_chain_width: 40,
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
            check_narrowing_assignment: false,
            dump_ir: false,
            dump_opt: false,
            write_ir_post_opt: false,
            write_circt_ir: false,
            skip_circt_lowering: false,
            fifo_write_delay: 1,
            fifo_merge_distance: 4,
            no_verilog_header: false,
            optimize: 2,
            register_ratio: 2,
            max_register_ratio: 0,
            reset_cycles: 3,
            reset_fan_out_cycles: 1,
            clock_gating: 0,
            clock_gating_threshold: 5,
            allow_fifo_stutter: true,
            stallable_pipelines: false,
            conditional_memory_reads: false,
            semaphore_delay: 3,
            release_assert: true,
            no_debug_view: false,
            code_coverage: false,
            code_coverage_mux_threshold: 0,
            dump_memory_metadata: false,
            identifier_length: 100,
            work_list_size: 0,
            return_stages: 3,
            debug: false,
            strip_debug_symbols: false,
            stall: 0,
            force_mem_to_array: 0,
            sparse_reg_opt: false,
            carry_select: false,
            max_stack_depth: 64,
            reset_fanout: true,
            enable_transaction_size_warning: true,
            enable_deprecation_warnings: true,
            warnings_as_errors: false,
            verbosity: 1,
            place_iterations: 1,
            place_seed: 1,
            place_pull: 100.0,
            place_push: 0.001,
            place_wall: 0.001,
            place_update_rate: 0.005,
            place_display: false,
        }
    }
}

/// `f32` as its bit pattern, for [`OptionValues`]' serialization.
mod f32_bits {
    pub fn serialize<S: serde::Serializer>(x: &f32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(x.to_bits())
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
        <u32 as serde::Deserialize>::deserialize(d).map(f32::from_bits)
    }
}

/// A filled `Options` plus the C strings it points into. Alive for exactly
/// as long as the reference's `withOptions` keeps its `alloca`s: the
/// `InitCompiler` call, which copies everything it needs (§3.3, "Lifetime").
#[derive(Debug)]
pub struct MarshalledOptions {
    options: raw::Options,
    _device: CString,
    _cmd_args: CString,
    _files: StringArray,
}

impl MarshalledOptions {
    /// Fill the struct. Strings are marshalled as `withCAString` does
    /// ([`crate::ca_string`]).
    pub fn new(v: &OptionValues) -> MarshalledOptions {
        let device = crate::ca_string(&v.target_device);
        let cmd_args = crate::ca_string(&v.cmd_args);
        let files = StringArray::new(v.file_names.iter().map(String::as_str));
        let b = |x: bool| -> raw::BOOL { raw::BOOL::from(x) };
        let z = |x: i64| -> usize { x as usize };
        let codegen = raw::CodeGenOptions {
            _debug: b(v.debug),
            _stripDebugSymbols: b(v.strip_debug_symbols),
            _optimize: z(v.optimize),
            _assertNarrowingConversion: b(v.check_narrowing_assignment),
            _dumpIR: b(v.dump_ir),
            _dumpOpt: b(v.dump_opt),
            _serializeIRPostOpt: b(v.write_ir_post_opt),
            _serializeCIRCTIR: b(v.write_circt_ir),
            _skipCirctLowering: b(v.skip_circt_lowering),
            _detectRaces: 0,
            _controlInspection: 0,
            _inspection: 0,
            _releaseAssert: b(v.release_assert),
            _allowFifoStutter: b(v.allow_fifo_stutter),
            _stallablePipelines: b(v.stallable_pipelines),
            _clockGatingLevel: z(v.clock_gating),
            _clockGatingThreshold: z(v.clock_gating_threshold),
            _conditionalMemoryReads: b(v.conditional_memory_reads),
            _semaphoreDelay: z(v.semaphore_delay),
            _codeCoverage: b(v.code_coverage),
            _codeCoverageMuxThreshold: z(v.code_coverage_mux_threshold),
            _dumpMemoryMetadata: b(v.dump_memory_metadata),
            _maxBlockRam: z(v.max_block_ram),
            _maxDeepRam: z(v.max_deep_ram),
            _minBlockRamUtilization: z(v.block_ram_util_threshold),
            _minDeepRamUtilization: z(v.deep_ram_util_threshold),
            _maxSelectInputs: z(v.max_mux_sources),
            _maxMulSrcWidth: z(v.max_mul_src_width),
            _maxThreadsDefault: z(v.max_threads_default),
            _maxThreadsLimit: z(v.max_threads_limit),
            _scheduleLimit: z(v.schedule_limit),
            _additionalLatency: z(v.additional_latency),
            _additionalResetCycles: z(v.reset_duration_cycles),
            _autoPipelineScale: z(v.auto_pipeline_scale),
            _autoPipelineCrossRegion: z(v.auto_pipeline_cross_region),
            _logicRegisterRatio: z(v.register_ratio),
            _maxLogicRegisterRatio: z(v.max_register_ratio),
            _resetCycles: z(v.reset_cycles),
            _resetFanOutCycles: z(v.reset_fan_out_cycles),
            _fifoWriteDelay: z(v.fifo_write_delay),
            _fifoMergeDistance: z(v.fifo_merge_distance),
            _carryChainWidthPerLogicLevel: z(v.carry_chain_width),
            _rmwMemoryWriteDelay: z(v.rmw_memory_write_delay),
            _rmwMemoryReadDelay: z(v.rmw_memory_read_delay),
            _rmwHardenedMemoryWriteDelay: z(v.rmw_hardened_write_delay),
            _rmwHardenedMemoryMaxWidth: z(v.rmw_hardened_max_width),
            _rmwHardenedMemoryMaxHeight: z(v.rmw_hardened_max_height),
            _rmwMemoryFavorArea: b(v.rmw_memory_favor_area),
            _strictMemoryReadWrite: b(v.strict_memory_read_write),
            _inputOutputRegisterDelay: z(v.io_register_delay),
            _memToRegThreshold: v.mem_to_reg,
            _fifoToDspThreshold: z(v.fifo_to_dsp),
            _verbosity: z(v.verbosity),
            _maxStringLength: z(v.identifier_length),
            _stall: z(v.stall),
            _forceMemToArray: z(v.force_mem_to_array),
            _suppressDebugView: b(v.no_debug_view),
            _logWorkListSize: z(v.work_list_size),
            _returnStages: z(v.return_stages),
            _maxStackDepth: z(v.max_stack_depth),
            _resetFanout: b(v.reset_fanout),
            _sparseRegOpt: b(v.sparse_reg_opt),
            _carrySelect: b(v.carry_select),
            _noVerilogHeader: b(v.no_verilog_header),
            _enableTransactionSizeWarning: b(v.enable_transaction_size_warning),
            _enableDeprecationWarnings: b(v.enable_deprecation_warnings),
            _warningsAsErrors: b(v.warnings_as_errors),
        };
        let placement = raw::PlacementOptions {
            _numIterations: z(v.place_iterations),
            _seed: z(v.place_seed),
            _pullStrength: v.place_pull,
            _pushStrength: v.place_push,
            _wallStrength: v.place_wall,
            _updateRate: v.place_update_rate,
            _display: b(v.place_display),
        };
        let options = raw::Options {
            _frequency: z(v.frequency),
            _deviceName: device.as_ptr(),
            _cmdArgs: cmd_args.as_ptr(),
            _codegenOptions: codegen,
            _placementOptions: placement,
            _fileNames: files.as_ptr().cast_mut(),
        };
        MarshalledOptions { options, _device: device, _cmd_args: cmd_args, _files: files }
    }

    /// The pointer `InitCompiler` takes, valid while `self` is.
    pub fn as_ptr(&self) -> *const raw::Options {
        &raw const self.options
    }

    /// The struct itself, for a reader that renders or hashes the fields.
    pub fn options(&self) -> &raw::Options {
        &self.options
    }
}

/// A NUL-terminated array of C strings — `withStringArray`
/// (`Options/FFI.hsc:44-58`) — for `_fileNames`, a `ParseNamespaceScopePtr`
/// and a `ParseQualifiedNamePtr` (§8: the elements are written outer→inner
/// and the backend reverses them).
#[derive(Debug)]
pub struct StringArray {
    _strings: Vec<CString>,
    pointers: Vec<*const c_char>,
}

impl StringArray {
    /// Build from the elements in array order, each marshalled as
    /// `allocaStrings`' `withCAString` does ([`crate::ca_string`]).
    pub fn new<'a>(items: impl IntoIterator<Item = &'a str>) -> StringArray {
        let strings: Vec<CString> = items.into_iter().map(crate::ca_string).collect();
        let mut pointers: Vec<*const c_char> = strings.iter().map(|s| s.as_ptr()).collect();
        pointers.push(std::ptr::null());
        StringArray { _strings: strings, pointers }
    }

    /// The `const char* const*` the ABI takes, valid while `self` is.
    pub fn as_ptr(&self) -> *const *const c_char {
        self.pointers.as_ptr()
    }

    /// How many strings, not counting the terminator.
    pub fn len(&self) -> usize {
        self.pointers.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A `Location` as `SetLocation2` takes it (§2.2): 1-based lines and
/// columns, an index into `_fileNames`, and `_valid`, which `SetLocation`
/// forces to `true` regardless (§6.1).
pub fn location(begin: (usize, usize), end: (usize, usize), file_index: usize) -> raw::Location {
    raw::Location {
        _beginLine: begin.0,
        _beginColumn: begin.1,
        _endLine: end.0,
        _endColumn: end.1,
        _fileIndex: file_index,
        _valid: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_array_is_nul_terminated_in_order() {
        let a = StringArray::new(["a", "bc"]);
        assert_eq!(a.len(), 2);
        // SAFETY: two live strings and a terminator, built just above.
        unsafe {
            let p = a.as_ptr();
            assert_eq!(std::ffi::CStr::from_ptr(*p).to_str().unwrap(), "a");
            assert_eq!(std::ffi::CStr::from_ptr(*p.add(1)).to_str().unwrap(), "bc");
            assert!((*p.add(2)).is_null());
        }
    }
}
