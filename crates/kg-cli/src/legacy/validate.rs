//! The option validation the C++ backend performs on startup.
//!
//! `SetupCodeGenConfig` (`compiler/cpp/config.cpp:162-207`) throws
//! `std::runtime_error` for ten option combinations; `parse_tree.cpp:6283-6298`
//! catches those and prints `e.what()` to **stdout** with no `Error N:` prefix,
//! then exits 1 (`docs/spec/cli.md` §2.4, §7).
//!
//! It is reproduced here even though there is no backend, because rejecting a
//! bad option set is behaviour a caller can depend on and it costs nothing.
//! Order of the checks is the order of the C++, since only the first is
//! reported.

use super::options::LegacyOptions;

/// Returns the message the reference would print, or `Ok` if the options are
/// acceptable to the backend.
pub fn validate(opts: &LegacyOptions) -> Result<(), &'static str> {
    if opts.reset_fan_out_cycles > opts.reset_cycles {
        return Err("--reset-fan-out-cycles must be no greater than --reset-cycles");
    }
    if opts.register_ratio < 1 {
        return Err("Logic to register ratio must be at least 1");
    }
    if opts.files.is_empty() {
        return Err("No input files specified");
    }
    // `PipelineClockGatingEnabled() = _clockGatingLevel > 0` (`config.cpp:70`).
    if opts.clock_gating > 0 {
        return Err("Invalid -- clock gating level must be equal to 0");
    }
    if opts.stallable_pipelines {
        return Err("Invalid -- stallable pipeline feature must be disabled");
    }
    // Note the help text says [0,3] and the check says [0,4]; the check wins.
    if opts.stall > 4 {
        return Err("Invalid --stall value not in [0,4]");
    }
    if opts.max_mux_sources < 2 {
        return Err("--max-mux-sources must be at least 2");
    }
    if !is_pow2(opts.max_mux_sources) {
        return Err("--max-mux-sources must be a power of 2");
    }
    if !is_pow2(opts.max_threads_default) {
        return Err("--max-threads-default must be a power of 2");
    }
    if !is_pow2(opts.max_threads_limit) {
        return Err("--max-threads-limit must be a power of 2");
    }
    Ok(())
}

fn is_pow2(n: i64) -> bool {
    n > 0 && n & (n - 1) == 0
}

/// `--place-iterations` is silently forced to 1 below `-O2`
/// (`config.cpp:188-192`). Exposed as a function rather than applied to the
/// record so that the value the user asked for is still visible.
pub fn effective_place_iterations(opts: &LegacyOptions) -> i64 {
    if opts.optimize < 2 { 1 } else { opts.place_iterations }
}

/// `MaximumWorkListSize()` (`config.cpp:95-118`): 0 means "pick a default from
/// the optimization level", the result is `1 << log`, and 64 or more is an error.
pub fn maximum_work_list_size(opts: &LegacyOptions) -> Result<u64, &'static str> {
    let log = if opts.work_list_size == 0 {
        if opts.optimize < 2 { 16 } else { 20 }
    } else {
        opts.work_list_size
    };
    if !(0..64).contains(&log) {
        return Err("Work list size is too large");
    }
    Ok(1u64 << log)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> LegacyOptions {
        LegacyOptions { files: vec!["a.k".to_owned()], ..LegacyOptions::default() }
    }

    #[test]
    fn the_defaults_are_valid() {
        assert_eq!(validate(&opts()), Ok(()));
    }

    #[test]
    fn every_check_fires_with_the_reference_message() {
        type Case = (Box<dyn Fn(&mut LegacyOptions)>, &'static str);
        let cases: Vec<Case> = vec![
            (Box::new(|o: &mut LegacyOptions| o.reset_fan_out_cycles = 9), "--reset-fan-out-cycles must be no greater than --reset-cycles"),
            (Box::new(|o: &mut LegacyOptions| o.register_ratio = 0), "Logic to register ratio must be at least 1"),
            (Box::new(|o: &mut LegacyOptions| o.files.clear()), "No input files specified"),
            (Box::new(|o: &mut LegacyOptions| o.clock_gating = 1), "Invalid -- clock gating level must be equal to 0"),
            (Box::new(|o: &mut LegacyOptions| o.stallable_pipelines = true), "Invalid -- stallable pipeline feature must be disabled"),
            (Box::new(|o: &mut LegacyOptions| o.stall = 5), "Invalid --stall value not in [0,4]"),
            (Box::new(|o: &mut LegacyOptions| o.max_mux_sources = 1), "--max-mux-sources must be at least 2"),
            (Box::new(|o: &mut LegacyOptions| o.max_mux_sources = 3), "--max-mux-sources must be a power of 2"),
            (Box::new(|o: &mut LegacyOptions| o.max_threads_default = 3), "--max-threads-default must be a power of 2"),
            (Box::new(|o: &mut LegacyOptions| o.max_threads_limit = 3), "--max-threads-limit must be a power of 2"),
        ];
        for (mutate, message) in cases {
            let mut o = opts();
            mutate(&mut o);
            assert_eq!(validate(&o), Err(message));
        }
    }

    #[test]
    fn stall_of_three_and_four_are_accepted_despite_the_help_text() {
        let mut o = opts();
        o.stall = 4;
        assert_eq!(validate(&o), Ok(()));
    }

    #[test]
    fn low_optimization_levels_override_place_iterations() {
        let mut o = opts();
        o.place_iterations = 3000;
        assert_eq!(effective_place_iterations(&o), 3000);
        o.optimize = 1;
        assert_eq!(effective_place_iterations(&o), 1);
    }

    #[test]
    fn work_list_size_defaults_depend_on_the_optimization_level() {
        let mut o = opts();
        assert_eq!(maximum_work_list_size(&o), Ok(1 << 20));
        o.optimize = 0;
        assert_eq!(maximum_work_list_size(&o), Ok(1 << 16));
        o.work_list_size = 64;
        assert_eq!(maximum_work_list_size(&o), Err("Work list size is too large"));
    }
}
