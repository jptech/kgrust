//! Text repairs on RTL before the open tools read it.

use camino::Utf8Path;

use crate::{Failure, Result};

/// Repairs generated SystemVerilog the reference backend emits malformed, in
/// place. Each repair is narrow, keyed on the exact text the backend writes,
/// and named in `docs/plans/platform-layer.md` §8.5 with the backend line
/// that causes it; a file needing none is left untouched.
///
/// 1. A single reset replica. `InstantiateResetControlCIRCT` declares
///    `reg_rst_delayed` as an integer of `resetReplicas` bits
///    (`compiler/cpp/verilog.cpp:2935`), which CIRCT writes as a scalar
///    `logic` when there is one; `GetResetReplica` still names it
///    `reg_rst_delayed[0]` (`verilog.cpp:6350`). A bit-select of a scalar is
///    illegal: Verilator refuses it, Yosys reads it as the bit itself.
///    Declaring the one bit as `[0:0]` is the meaning every use assumes.
pub fn generated(path: &Utf8Path) -> Result<()> {
    let text = std::fs::read_to_string(path).map_err(|e| Failure::Failed(format!("{path}: {e}")))?;
    let fixed = single_reset_replica(&text);
    if fixed != text {
        std::fs::write(path, fixed).map_err(|e| Failure::Failed(format!("{path}: {e}")))?;
    }
    Ok(())
}

fn single_reset_replica(text: &str) -> String {
    if !text.contains("reg_rst_delayed[0]") || text.contains("reg_rst_delayed[1]") {
        return text.to_owned();
    }
    text.split_inclusive('\n')
        .map(|line| {
            let body = line.trim_start();
            let indent = &line[..line.len() - body.len()];
            match body.strip_prefix("logic") {
                Some(rest) if rest.starts_with(char::is_whitespace) && rest.trim_start().starts_with("reg_rst_delayed;") => {
                    let gap = &rest[..rest.len() - rest.trim_start().len()];
                    format!("{indent}logic [0:0]{gap}{}", rest.trim_start())
                }
                _ => line.to_owned(),
            }
        })
        .collect()
}

/// A source with its simulation-only regions cut: generated RTL and
/// upstream's runtime fence what synthesis must not see (string tables,
/// dynamic assertions, `real` arithmetic) between `synopsys translate_off`
/// and `translate_on` comments. Synthesis tools honour the comments; sv2v
/// drops every comment first, so the regions are cut before it reads them.
pub fn strip_translate_off(text: &str) -> std::result::Result<String, &'static str> {
    let mut kept = String::with_capacity(text.len());
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        if pragma(line, "translate_off") {
            skipping = true;
        } else if pragma(line, "translate_on") {
            skipping = false;
        } else if !skipping {
            kept.push_str(line);
        }
    }
    if skipping { Err("translate_off without translate_on") } else { Ok(kept) }
}

/// `synopsys <word>` or `pragma <word>`, any space between.
fn pragma(line: &str, word: &str) -> bool {
    ["synopsys", "pragma"].iter().any(|lead| {
        line.match_indices(lead).any(|(i, _)| {
            let rest = &line[i + lead.len()..];
            let trimmed = rest.trim_start();
            trimmed.len() < rest.len() && trimmed.starts_with(word)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_replica_is_declared_as_one_bit() {
        let text = "  logic reg_rst_delayed;\n  assign x = reg_rst_delayed[0];\n";
        assert_eq!(single_reset_replica(text), "  logic [0:0] reg_rst_delayed;\n  assign x = reg_rst_delayed[0];\n");
    }

    /// Two replicas are a vector already; a scalar never indexed is legal.
    #[test]
    fn other_declarations_are_left_alone() {
        let two = "  logic reg_rst_delayed;\n  x = reg_rst_delayed[0] | reg_rst_delayed[1];\n";
        assert_eq!(single_reset_replica(two), two);
        let unindexed = "  logic reg_rst_delayed;\n  x = reg_rst_delayed;\n";
        assert_eq!(single_reset_replica(unindexed), unindexed);
        let other = "  logic reg_rst_delayed_q;\n  x = reg_rst_delayed[0];\n";
        assert_eq!(single_reset_replica(other), other);
    }

    #[test]
    fn simulation_regions_are_cut() {
        let text = "a\n// synopsys translate_off\nb\n// synopsys translate_on\nc\n/* pragma  translate_off */\nd\n// pragma translate_on\n";
        assert_eq!(strip_translate_off(text).unwrap(), "a\nc\n");
    }

    #[test]
    fn an_unclosed_region_is_refused() {
        assert!(strip_translate_off("a\n// synopsys translate_off\nb\n").is_err());
    }

    /// The word must follow `synopsys` or `pragma` across whitespace alone,
    /// as `(synopsys|pragma)\s+translate_off` has it.
    #[test]
    fn a_word_alone_is_not_a_pragma() {
        assert_eq!(strip_translate_off("// translate_off\nx\n").unwrap(), "// translate_off\nx\n");
        let prose = "// synopsys says translate_off is a comment\nx\n";
        assert_eq!(strip_translate_off(prose).unwrap(), prose);
    }
}
