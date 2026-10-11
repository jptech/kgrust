//! `kg explain <code>` — the long-form error reference.
//!
//! Two code spaces exist upstream and both are reproduced here:
//!
//! * **`Error 1`** is the *entire* frontend. Lexing, parsing, imports,
//!   desugaring, template instantiation, constant evaluation, name resolution,
//!   type checking and `--Werror` promotion all report it — the literal string
//!   is hard-coded at `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114` and
//!   `compiler/hs/app/Main.hs:99`. That is why frontend conformance is an
//!   accept/reject question and not a message-text question.
//! * **`Error 2`–`Error 84`** are `enum class CompileError`
//!   (`compiler/cpp/errors.h:6-84`), emitted by the C++ middle end as
//!   `Error <N> at: <file> (<line>): <msg>`.
//!
//! The table below is the whole `CompileError` enum, extracted mechanically
//! from `errors.h`, so `kg explain 27` can at least name a code the day a user
//! meets it in reference output. Prose is written per code as the corresponding
//! check lands in this toolchain; today only code 1 has any, and `kg explain`
//! says so rather than inventing text.

use kg_diag::WarningKind;

/// `(code, enum name, line in compiler/cpp/errors.h)`.
pub static COMPILE_ERRORS: &[(u16, &str, u32)] = &[
    (1, "SyntaxError", 8),
    (2, "DuplicateFunction", 9),
    (3, "DuplicateType", 10),
    (4, "UnknownType", 11),
    (5, "UnknownFunction", 12),
    (6, "ExportTypeRestriction", 13),
    (7, "InvalidGlobalAssignment", 14),
    (8, "MultipleMemoryWrites", 15),
    (9, "InvalidArrayIndexing", 16),
    (10, "PartialElementWrite", 17),
    (11, "InvalidLiteral", 18),
    (12, "UnorderedOperation", 19),
    (13, "InvalidSwitch", 20),
    (14, "InvalidConversion", 21),
    (16, "InvalidType", 22),
    (17, "InvalidArraySize", 23),
    (18, "InvalidFunctionModifier", 24),
    (19, "InvalidNesting", 25),
    (20, "InvalidFunctionReturnType", 26),
    (21, "InvalidReturn", 27),
    (22, "InvalidIterationCount", 28),
    (24, "NonValueType", 29),
    (25, "InvalidLoop", 30),
    (26, "InvalidMux", 31),
    (27, "InvalidCall", 32),
    (28, "InvalidContainer", 33),
    (29, "ProtectionModifier", 34),
    (30, "InvalidMember", 35),
    (31, "InvalidExtern", 36),
    (32, "UnknownIdentifier", 37),
    (33, "DuplicateIdentifier", 38),
    (34, "InvalidAtomic", 39),
    (35, "CallRateNotPredicated", 40),
    (36, "ExpressionNotStatic", 41),
    (37, "RecursionNotSupported", 42),
    (38, "InvalidWaitForCondition", 43),
    (40, "UnmatchedComment", 44),
    (41, "InvalidReset", 45),
    (43, "StaticAssert", 46),
    (44, "TypeToWide", 47),
    (45, "TooManyReadPorts", 48),
    (47, "InvalidPerThreadUsage", 49),
    (49, "ReservedName", 50),
    (50, "InvalidInspectionTarget", 51),
    (51, "TooManyInspectableVariables", 52),
    (52, "InspectableTooDeep", 53),
    (53, "InvalidString", 54),
    (54, "InvalidConst", 55),
    (56, "InvalidEndTransaction", 56),
    (57, "InvalidUpdateRate", 57),
    (58, "InvalidClockSpecifier", 58),
    (59, "InvalidDebugView", 59),
    (60, "DivideByZero", 60),
    (61, "AssignmentToConst", 61),
    (62, "InvalidInitializerList", 62),
    (63, "InvalidStaticLocal", 63),
    (64, "BasicBlockNotSchedulable", 64),
    (65, "InvalidAttribute", 65),
    (66, "InvalidExportClass", 66),
    (67, "InvalidQuadPortMemory", 67),
    (68, "InvalidRAMInitialization", 68),
    (69, "InvalidEccFunction", 69),
    (70, "EccUnsupported", 70),
    (71, "AmbiguousDebugSymbols", 71),
    (72, "InvalidMemoryReadDelay", 72),
    (73, "MethodReturningReference", 73),
    (74, "InvalidExternFunction", 74),
    (75, "InvalidCallback", 75),
    (76, "DeprecatedFeature", 76),
    (77, "InvalidAssignment", 77),
    (78, "InvalidIntrinsicCall", 78),
    (79, "InvalidTemplateArgument", 79),
    (81, "MissingDeviceConfigProperty", 80),
    (82, "InvalidDeviceConfigProperty", 81),
    (83, "QuadPortUnsupported", 82),
    (84, "MissingDeviceConfigSchema", 83),
];

/// Values `CompileError` never assigns, listed so a user who mistypes a code
/// gets told it does not exist rather than "unknown".
pub const UNASSIGNED: &[u16] = &[15, 23, 39, 42, 46, 48, 55, 80];

/// `enum class CompileWarning` (`compiler/cpp/errors.h:86-92`).
pub static COMPILE_WARNINGS: &[(u16, &str, &str)] = &[
    (8, "TooManyRams", "always on; no `--W` switch controls it"),
    (10, "LastWithoutTransactionSize", "gated on `-W transaction-size` / `-W all`"),
    (12, "NonConstGlobal", "gated on `-W deprecated`, which is on by default"),
    (13, "FreeNonInlineFunction", "declared but never emitted anywhere in the reference"),
];

/// What `kg explain` was asked about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Query {
    Error(u16),
    Warning(u16),
    Kind(WarningKind),
}

/// Parses the argument of `kg explain`.
///
/// Accepts what a user is likely to copy out of compiler output or type from
/// memory: `1`, `E1`, `Error 1`, `KGB0027`, `W12`, `Warning 12`, or a warning
/// name such as `shadow` / `no-shadow` / `--Wno-shadow`.
pub fn parse_query(input: &str) -> Option<Query> {
    let text = input.trim();
    let lower = text.to_ascii_lowercase();

    if let Some(kind) = warning_kind(&lower) {
        return Some(Query::Kind(kind));
    }

    let digits = |rest: &str| rest.trim().trim_start_matches('0').parse::<u16>().ok().or_else(|| rest.trim().parse::<u16>().ok());

    for prefix in ["warning ", "warning", "w"] {
        if let Some(rest) = lower.strip_prefix(prefix)
            && let Some(number) = digits(rest)
        {
            return Some(Query::Warning(number));
        }
    }
    for prefix in ["error ", "error", "kgb", "kgf", "e"] {
        if let Some(rest) = lower.strip_prefix(prefix)
            && let Some(number) = digits(rest)
        {
            return Some(Query::Error(number));
        }
    }
    digits(&lower).map(Query::Error)
}

fn warning_kind(lower: &str) -> Option<WarningKind> {
    let name = lower
        .trim_start_matches("--")
        .trim_start_matches("wno-")
        .trim_start_matches("no-")
        .trim_start_matches('w');
    WarningKind::from_name(name).or_else(|| WarningKind::from_name(lower))
}

/// The long-form text for one query.
pub fn explain(query: &Query) -> String {
    match query {
        Query::Error(1) => FRONTEND_ERROR.to_owned(),
        Query::Error(code) => match COMPILE_ERRORS.iter().find(|(n, _, _)| n == code) {
            Some((code, name, line)) => format!(
                "Error {code} — {name}\n\
                 \n\
                 Source      compiler/cpp/errors.h:{line} (`enum class CompileError`)\n\
                 Produced by the C++ middle end, printed as\n\
                 `Error {code} at: <file> (<line>): <message>` on stdout, exit code 1.\n\
                 \n\
                 No long-form explanation is written for this code yet. kgrust implements\n\
                 the frontend, which reports every problem it finds as Error 1; codes above\n\
                 1 come from the reference's C++ backend, and each one gets prose here as\n\
                 the equivalent check lands. `kg explain 1` describes the frontend code.\n"
            ),
            None if UNASSIGNED.contains(code) => format!(
                "Error {code} is not assigned.\n\n\
                 `CompileError` skips {}, so no compiler ever emits these.\n\
                 Run `kg explain --list` for the codes that do exist.\n",
                UNASSIGNED
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!(
                "Error {code} is not a Kanagawa error code.\n\n\
                 Codes run from 1 to 84 with gaps; run `kg explain --list` to see them all.\n"
            ),
        },
        Query::Warning(code) => match COMPILE_WARNINGS.iter().find(|(n, _, _)| n == code) {
            Some((code, name, gate)) => format!(
                "Warning {code} — {name}\n\
                 \n\
                 Source      compiler/cpp/errors.h:86-92 (`enum class CompileWarning`)\n\
                 Gate        {gate}\n\
                 \n\
                 Backend warnings print as `Warning {code} at: <file> (<line>): <message>`\n\
                 and carry no `[--Wno-…]` suffix, which is why the legacy syntax harness\n\
                 never matches them. kgrust does not emit backend warnings yet.\n"
            ),
            None => format!(
                "Warning {code} is not a Kanagawa backend warning code.\n\n\
                 The four that exist are 8, 10, 12 and 13; run `kg explain --list`.\n"
            ),
        },
        Query::Kind(kind) => format!(
            "warning: {name}\n\
             \n\
             Enable      -W {name}          (legacy: --W{name})\n\
             Disable     -W no-{name}       (legacy: --Wno-{name})\n\
             Manifest    [warnings] {name} = \"warn\" | \"allow\" | \"deny\"\n\
             Default     {default}\n\
             \n\
             {description}\n\
             \n\
             Frontend warnings are reported as `warning: <message> [--Wno-{name}]`; the\n\
             suffix is load-bearing for `test/syntax/syntax-test.py`, which anchors its\n\
             regex on it. `-W error` (legacy `--Werror`) turns any warning into a failed\n\
             build with the single line `Error 1: Warnings treated as errors`.\n",
            name = kind.name(),
            default = if default_on(*kind) { "on" } else { "off — enable it with `-W all` or `-W <name>`" },
            description = describe(*kind),
        ),
    }
}

/// The four warnings the reference enables before any switch is seen
/// (`Options/CmdArgs.hs:112-117`).
fn default_on(kind: WarningKind) -> bool {
    matches!(
        kind,
        WarningKind::RedundantImport
            | WarningKind::Deprecated
            | WarningKind::InitializerOverrides
            | WarningKind::Shadow
    )
}

fn describe(kind: WarningKind) -> &'static str {
    match kind {
        WarningKind::Conversion => {
            "An implicit conversion may alter the value being converted — a narrowing\n\
             assignment, or a signed/unsigned change. `--check-narrowing-assignment` is the\n\
             related backend flag: it turns the same situation into a runtime assertion in\n\
             generated RTL instead of a compile-time note."
        }
        WarningKind::Deprecated => {
            "A language feature that still works but is scheduled for removal. The backend\n\
             also reports one deprecation of its own, `Warning 12 NonConstGlobal`, under\n\
             this same switch."
        }
        WarningKind::InitializerOverrides => {
            "A designated initializer overwrites a field that an earlier initializer in the\n\
             same list already set. Legal, and almost always a mistake."
        }
        WarningKind::RedundantImport => {
            "The file imports a module it already has, directly or through another import.\n\
             Harmless, but import order is semantically load-bearing in Kanagawa, so a\n\
             redundant import is worth seeing."
        }
        WarningKind::Shadow => {
            "A declaration hides another of the same name from an enclosing scope."
        }
        WarningKind::TransactionSize => {
            "A call to a function marked `[[last]]` has no `[[transaction_size(N)]]`\n\
             attribute, so the compiler cannot size the transaction and falls back to a\n\
             conservative choice."
        }
    }
}

/// The one code the frontend produces, and the only one with prose today.
const FRONTEND_ERROR: &str = "\
Error 1 — the frontend rejected the program

Source      compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114 (`\"Error 1: \"`)
            compiler/hs/app/Main.hs:99 (`Error 1: Warnings treated as errors`)
Produced by lexing, parsing, import resolution, desugaring, template
            instantiation, constant evaluation, name resolution, type checking,
            and `--Werror` promotion — all of it, one code.

There is exactly one frontend error code. `grep -rn 'CompileError::SyntaxError'`
over the reference's C++ returns nothing, so `Error 1` is unambiguously \"the
frontend said no\", and every other code means the program got past the frontend
and was rejected later by the middle end.

What that means in practice:

  * The number carries no information beyond accept/reject. The *message* is
    where the diagnosis lives, which is why `kg check` prints a source excerpt
    and a caret instead of leading with a code.
  * 834 of the 2,843 assertions in `test/syntax/` expect exactly this code, and
    they are decidable by a frontend alone — which is what this toolchain is.
  * Under `--Werror` a single warning becomes `Error 1: Warnings treated as
    errors`, printed after the warnings themselves, before any backend runs.

In kgrust today `Error 1` covers lexing and parsing only. Name resolution and
type checking are not written yet, so a program that parses is reported as
parsed — never as compiled.
";

/// One line per code, for `kg explain --list`.
pub fn list() -> String {
    let mut out = String::from("Frontend:\n  1  the frontend rejected the program (`kg explain 1`)\n\nBackend (compiler/cpp/errors.h):\n");
    for (code, name, _) in COMPILE_ERRORS.iter().filter(|(n, _, _)| *n != 1) {
        out.push_str(&format!("  {code:>2}  {name}\n"));
    }
    out.push_str("\nBackend warnings:\n");
    for (code, name, gate) in COMPILE_WARNINGS {
        out.push_str(&format!("  {code:>2}  {name} — {gate}\n"));
    }
    out.push_str("\nWarning kinds (`kg explain shadow`):\n");
    for kind in WarningKind::ALL {
        let state = if default_on(kind) { "on by default" } else { "off by default" };
        out.push_str(&format!("      {} — {state}\n", kind.name()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_whole_enum() {
        // 76 enumerators, max 84, eight gaps (docs/spec/diagnostics.md §7.6).
        assert_eq!(COMPILE_ERRORS.len(), 76);
        assert_eq!(COMPILE_ERRORS.last().unwrap().0, 84);
        for code in UNASSIGNED {
            assert!(!COMPILE_ERRORS.iter().any(|(n, _, _)| n == code), "{code} should be a gap");
        }
        let mut codes: Vec<u16> = COMPILE_ERRORS.iter().map(|(n, _, _)| *n).collect();
        let before = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(before, codes.len(), "duplicate code in the table");
    }

    #[test]
    fn queries_parse_the_way_people_type_them() {
        assert_eq!(parse_query("1"), Some(Query::Error(1)));
        assert_eq!(parse_query("E1"), Some(Query::Error(1)));
        assert_eq!(parse_query("Error 27"), Some(Query::Error(27)));
        assert_eq!(parse_query("KGB0027"), Some(Query::Error(27)));
        assert_eq!(parse_query("W12"), Some(Query::Warning(12)));
        assert_eq!(parse_query("Warning 8"), Some(Query::Warning(8)));
        assert_eq!(parse_query("shadow"), Some(Query::Kind(WarningKind::Shadow)));
        assert_eq!(parse_query("--Wno-shadow"), Some(Query::Kind(WarningKind::Shadow)));
        assert_eq!(parse_query("no-transaction-size"), Some(Query::Kind(WarningKind::TransactionSize)));
        assert_eq!(parse_query("nonsense"), None);
    }

    #[test]
    fn code_one_has_real_prose() {
        let text = explain(&Query::Error(1));
        assert!(text.contains("exactly one frontend error code"), "{text}");
        assert!(text.len() > 500);
    }

    #[test]
    fn a_backend_code_is_named_and_honest_about_the_missing_prose() {
        let text = explain(&Query::Error(27));
        assert!(text.starts_with("Error 27 — InvalidCall"), "{text}");
        assert!(text.contains("errors.h:32"), "{text}");
        assert!(text.contains("No long-form explanation"), "{text}");
    }

    #[test]
    fn a_gap_is_reported_as_a_gap() {
        assert!(explain(&Query::Error(15)).contains("not assigned"));
        assert!(explain(&Query::Error(900)).contains("not a Kanagawa error code"));
    }

    #[test]
    fn every_warning_kind_explains_both_switch_spellings() {
        for kind in WarningKind::ALL {
            let text = explain(&Query::Kind(kind));
            assert!(text.contains(&format!("-W no-{}", kind.name())), "{text}");
            assert!(text.contains(&format!("--Wno-{}", kind.name())), "{text}");
        }
    }

    #[test]
    fn the_listing_covers_every_code() {
        let listing = list();
        for (code, name, _) in COMPILE_ERRORS.iter().filter(|(n, _, _)| *n != 1) {
            assert!(listing.contains(name), "missing {code} {name}");
        }
    }
}
