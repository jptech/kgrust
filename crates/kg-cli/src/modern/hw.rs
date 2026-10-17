//! `[hw]` and `-Z hw.<key>=<value>`: the backend's options for `kg build`.
//!
//! Keys are the legacy long flags (`docs/spec/cli.md` §9.3), dashes or
//! underscores alike, and values are typed as the flag is: `reset-cycles = 1`,
//! `no-reset-fanout = true`. They are written into a [`LegacyOptions`]
//! through the compat binary's own flag table, so `kg build` and `kanagawa`
//! given the same options hand the backend the same record.

use std::collections::BTreeMap;

use crate::legacy::LegacyOptions;
use crate::legacy::flags::{self, Sink};

/// The backend options `[hw]` may set: §9.3's codegen tuning, less the two
/// that are `kg build --emit` (`resource-usage`, `dgml`), plus the placement
/// constants, `stall` and `force-mem-to-array`, which §9.3 leaves to `-Z`.
pub const KEYS: &[&str] = &[
    "additional-latency", "auto-pipeline-ratio", "auto-pipeline-cross-region", "carry-chain-width",
    "carry-select", "rmw-memory-write-delay", "rmw-memory-read-delay", "rmw-hardened-write-delay",
    "rmw-hardened-max-width", "rmw-hardened-max-height", "rmw-memory-favor-area",
    "strict-memory-read-write", "io-register-delay", "mem-to-reg", "fifo-to-dsp", "fifo-write-delay",
    "fifo-merge-distance", "frequency", "max-block-ram", "block-ram-util-threshold", "max-deep-ram",
    "deep-ram-util-threshold", "max-mux-sources", "max-mul-src-width", "max-threads-default",
    "max-threads-limit", "schedule-limit", "register-ratio", "max-register-ratio", "no-reset-fanout",
    "reset-cycles", "reset-fan-out-cycles", "reset-duration-cycles", "return-stages",
    "stallable-pipelines", "clock-gating", "clock-gating-threshold", "conditional-memory-reads",
    "semaphore-delay", "sparse-reg-opt", "identifier-length", "debug", "dump-memory-metadata",
    "strip-debug-symbols", "max-stack-depth", "no-fifo-stutter", "no-debug-view", "no-verilog-header",
    "code-coverage", "code-coverage-mux-threshold", "optimize", "work-list-size",
    "place-iterations", "place-pull", "place-push", "place-seed", "place-update-rate", "place-wall",
    "stall", "force-mem-to-array",
];

/// The reference's defaults with `layers` written over them in order, then
/// `-Z` (`unstable`): each `hw.<key>[=<value>]` sets a key, a bare switch
/// meaning `true`.
pub fn options(layers: &[BTreeMap<String, toml::Value>], unstable: &[String]) -> Result<LegacyOptions, String> {
    let mut opts = LegacyOptions::default();
    for layer in layers {
        for (key, value) in layer {
            set(&mut opts, key, &Value::Toml(value)).map_err(|e| format!("[hw] {e}"))?;
        }
    }
    for knob in unstable {
        let knob = knob.trim();
        let (key, value) = match knob.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (knob, None),
        };
        let Some(key) = key.strip_prefix("hw.") else {
            return Err(format!("unknown -Z option `{key}`; `-Z hw.<key>=<value>` sets a backend option"));
        };
        set(&mut opts, key, &Value::Text(value)).map_err(|e| format!("-Z hw.{e}"))?;
    }
    Ok(opts)
}

enum Value<'a> {
    Toml(&'a toml::Value),
    /// `-Z`'s text; `None` for a bare switch.
    Text(Option<&'a str>),
}

fn set(opts: &mut LegacyOptions, key: &str, value: &Value) -> Result<(), String> {
    let name = key.replace('_', "-");
    if !KEYS.contains(&name.as_str()) {
        return Err(format!("{key}: not a backend option (`kg explain` has none; docs/spec/cli.md §9.3 lists them)"));
    }
    let flag = flags::by_long(&name).expect("every key is a legacy flag");
    let bad = |want: &str| format!("{key}: wants {want}");
    match (flag.sink, value) {
        (Sink::Int(field) | Sink::OptInt(field), Value::Toml(toml::Value::Integer(n))) => *field(opts) = *n,
        (Sink::Int(field) | Sink::OptInt(field), Value::Text(Some(text))) => {
            *field(opts) = text.parse().map_err(|_| bad("an integer"))?
        }
        (Sink::Int(_) | Sink::OptInt(_), _) => return Err(bad("an integer")),
        (Sink::Float(field), Value::Toml(toml::Value::Float(x))) => *field(opts) = *x,
        (Sink::Float(field), Value::Toml(toml::Value::Integer(n))) => *field(opts) = *n as f64,
        (Sink::Float(field), Value::Text(Some(text))) => *field(opts) = text.parse().map_err(|_| bad("a number"))?,
        (Sink::Float(_), _) => return Err(bad("a number")),
        (Sink::Bool(field), Value::Toml(toml::Value::Boolean(b))) => *field(opts) = *b,
        (Sink::Bool(field), Value::Text(None)) => *field(opts) = true,
        (Sink::Bool(field), Value::Text(Some(text))) => {
            *field(opts) = match *text {
                "true" => true,
                "false" => false,
                _ => return Err(bad("true or false")),
            }
        }
        (Sink::Bool(_), _) => return Err(bad("true or false")),
        _ => unreachable!("no key of another kind is in KEYS"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> BTreeMap<String, toml::Value> {
        toml::from_str(text).unwrap()
    }

    fn z(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Every key is a flag of the table, of a kind `set` writes.
    #[test]
    fn every_key_is_a_settable_flag() {
        for key in KEYS {
            let flag = flags::by_long(key).unwrap_or_else(|| panic!("{key} is no legacy flag"));
            assert!(
                matches!(flag.sink, Sink::Int(_) | Sink::OptInt(_) | Sink::Float(_) | Sink::Bool(_)),
                "{key}: {:?}",
                flag.sink
            );
        }
    }

    #[test]
    fn nothing_given_is_the_references_defaults() {
        let opts = options(&[], &[]).unwrap();
        let defaults = LegacyOptions::default();
        assert_eq!((opts.reset_cycles, opts.register_ratio, opts.frequency), (defaults.reset_cycles, defaults.register_ratio, 0));
    }

    /// The UPduino's options, as its `board.toml` gives them.
    #[test]
    fn a_table_sets_typed_values_by_either_spelling() {
        let opts = options(
            &[table("reset-cycles = 1\nsemaphore_delay = 0\nno-reset-fanout = true\nfrequency = 24\nmem-to-reg = 2\n")],
            &[],
        )
        .unwrap();
        assert_eq!((opts.reset_cycles, opts.semaphore_delay, opts.frequency), (1, 0, 24));
        assert!(opts.no_reset_fanout);
        assert_eq!(opts.mem_to_reg, 2.0);
    }

    /// Board, then manifest, then `-Z`: each over the last.
    #[test]
    fn later_layers_and_z_win() {
        let opts = options(
            &[table("register-ratio = 8\nreset-cycles = 1\n"), table("register-ratio = 4\n")],
            &z(&["hw.reset-cycles=2", "hw.no-reset-fanout"]),
        )
        .unwrap();
        assert_eq!((opts.register_ratio, opts.reset_cycles), (4, 2));
        assert!(opts.no_reset_fanout);
    }

    #[test]
    fn an_unknown_key_or_a_wrong_type_is_refused() {
        let err = options(&[table("reset-cycle = 1\n")], &[]).unwrap_err();
        assert!(err.contains("reset-cycle:"), "{err}");
        let err = options(&[table("import-dir = \"x\"\n")], &[]).unwrap_err();
        assert!(err.contains("not a backend option"), "{err}");
        let err = options(&[table("reset-cycles = \"1\"\n")], &[]).unwrap_err();
        assert!(err.contains("wants an integer"), "{err}");
        let err = options(&[], &z(&["hw.no-reset-fanout=yes"])).unwrap_err();
        assert!(err.contains("true or false"), "{err}");
    }

    #[test]
    fn a_z_option_outside_hw_is_refused() {
        let err = options(&[], &z(&["dump-opt"])).unwrap_err();
        assert!(err.contains("unknown -Z option `dump-opt`"), "{err}");
    }
}
