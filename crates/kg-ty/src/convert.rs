//! Owner-topic: **implicit conversions and the `conversion` warning** — the
//! pieces of `checkConversion` / `checkConversion'` pass 19 builds its
//! verdicts from: the three narrowing continuations, the source
//! normalisation, the rendered texts and the fix-it. The decision itself is
//! pass 19's, on the loop (`kg_sem::check`; `single-source-of-truth.md`
//! D2.6).
//!
//! Spec: `docs/spec/types.md` §6 (§6.1 the call sites, §6.2 the algorithm,
//! §6.3 the exact rendered text, §6.5 the other `validateProgram` checks),
//! §15 traps 19-21.
//! Reference: `TypeCheck.hs:33-216`, and `Error.hs:142-145` for the rendering.
//!
//! # The single sentence that defines this file
//!
//! The `conversion` warning fires **exactly when** `not (s <: t)` **and**
//! `s tIsConvertibleTo t` **and** the site uses `warningNarrowingConversions`
//! (`docs/spec/types.md` §6.2). Everything else here is bookkeeping around that.
//!
//! # Traps that live in this file
//!
//! * Conversion of an integer **literal** is checked against its *minimal* type,
//!   not its suffixed type (`TypeCheck.hs:154`) — trap 19. This is why constant
//!   folding silences narrowing warnings
//!   (`test/syntax/conversion-warning.k:118-127`). See [`normalize_source`].
//! * `return` statements **never** emit the warning (`TypeCheck.hs:84`,
//!   `TypeCheck.hs:102`) — trap 20.
//! * `conversion` is **off by default**; only `--Wall` or `--Wconversion`
//!   enables it (`CmdArgs.hs:112-122`) — trap 21. `kg_config` already encodes
//!   the defaults.
//! * `checkArguments` skips the **first** argument entirely for `[[pipelined]]`
//!   functions (`TypeCheck.hs:92-93`) — trap 17.
//!
//! # The fix-it is structured, not prose
//!
//! `errorNarrowingConversions` appends
//! ``"\nInsert an explicit cast to silence this issue `cast<T>()`"`` to the
//! message (`TypeCheck.hs:103`). The exact text is reproduced byte for byte by
//! [`invalid_conversion_error`] — the legacy renderer prints the message and
//! nothing else, so anything less would change what the corpus sees — and the
//! *same* suggestion is additionally attached as a [`kg_diag::Fix`] by
//! [`cast_fix`]. The fix is two zero-width edits (`cast<T>(` before the
//! expression, `)` after it), so it needs no source text and is directly
//! applicable by an editor. `kg_diag::render_legacy` ignores `fixes`, so
//! attaching one cannot perturb the conformance harness — which is also why the
//! *warning* form carries the fix even though the reference has no hint there.

use kg_base::{FileRange, TextRange};
use kg_const::ConstValue;
use kg_diag::{Diagnostic, Fix, TextEdit, WarningKind};

use crate::ty::Ty;
use crate::width::minimal_int_type;

/// The narrowing continuation a call site passes in — `TypeCheck.hs:101-103`.
///
/// This *is* the difference between the six call sites; nothing else about them
/// varies.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Narrowing {
    /// `warningNarrowingConversions` — ``"Narrowing conversion from `s` to `t`"``
    /// as a [`WarningKind::Conversion`] warning (`TypeCheck.hs:101`).
    ///
    /// Used by: function-call arguments (`TypeCheck.hs:97`) and the normal
    /// variable-initialiser case (`TypeCheck.hs:131`).
    Warn,
    /// `allowNarrowingConversions` — silence (`TypeCheck.hs:102`).
    ///
    /// Used by: `return` statements only (`TypeCheck.hs:84`).
    Allow,
    /// `errorNarrowingConversions` — ``"Invalid conversion from `s` to `t`"``
    /// plus the ``cast<T>()`` hint on its own line (`TypeCheck.hs:103`).
    ///
    /// Used by: designated-initializer-list variable initialisers
    /// (`TypeCheck.hs:130`), function-typed variables whose initialiser is not a
    /// subtype (`TypeCheck.hs:137`), and the value of a `.field = e` designator
    /// at any depth (`TypeCheck.hs:157`).
    Error,
}

/// `checkConversion` steps 2 and 3 (`TypeCheck.hs:154-156`) — the *source*
/// normalisation, expressed over a folded constant rather than an expression.
///
/// This is trap 19 and the whole reason the corpus behaves the way it does:
///
/// * `folded` is an integer literal → the source type becomes
///   `inferIntLiteralType Nothing x`, the **minimal** type for the value, and any
///   `intN`/`uintN` suffix the literal carried is discarded
///   (`TypeCheck.hs:154`). `uint8 z = x + y` with `const uint32 x, y` therefore
///   does *not* warn, because the sum folds to a literal
///   (`test/syntax/conversion-warning.k:118-136`), while the same code with
///   non-`const` operands does (`:106-116`).
/// * `folded` is an enum value **and the target is an integer** → the enum
///   wrapper is discarded and the check recurses on the inner literal
///   (`TypeCheck.hs:155-156`), which is why `uint3 x = Foo::One` with
///   `enum Foo : uint16` does not warn (`:176-186`) but `auto x = Foo::One;
///   uint3 y = x;` does (`:204-216`) — in the second case `x` has type
///   `const Foo`, not a literal.
/// * anything else → the declared type, unchanged (`TypeCheck.hs:159`).
///
/// `Designator`/`Positional` sources (`TypeCheck.hs:157-158`) are *structural*
/// recursion, not a type rewrite; the caller walks into the element and switches
/// the continuation to [`Narrowing::Error`] for a designator itself.
pub fn normalize_source(declared: &Ty, target: &Ty, folded: Option<&ConstValue>) -> Ty {
    match folded {
        Some(ConstValue::Int { value, .. }) => minimal_int_type(value),
        // `checkConversion n t (EnumValue _ x) | tIsInt t = checkConversion n t x`
        // — guarded on the *target* being an integer.
        Some(ConstValue::Enum { value, .. }) if target.is_int() => minimal_int_type(value),
        _ => declared.clone(),
    }
}

/// The exact warning text of `docs/spec/types.md` §6.3.
///
/// ```text
/// warning: Narrowing conversion from `int32` to `uint32` [--Wno-conversion]
/// ```
///
/// The `[--Wno-conversion]` suffix is appended by the renderer
/// (`kg_diag::render_legacy`, from `Error.hs:142-145`), so the message here stops
/// before it. `from`/`to` must be rendered with the *unmangled* spelling
/// (`showUnmangled`, `TypeCheck.hs:36`, `Type.hs:137-176`), which turns a mangled
/// template instance back into `Name<args>`.
pub fn narrowing_warning(range: FileRange, from: &str, to: &str) -> Diagnostic {
    Diagnostic::warning(
        range,
        WarningKind::Conversion,
        format!("Narrowing conversion from `{from}` to `{to}`"),
    )
}

/// The error form, including the embedded newline and the fix-it hint
/// (`TypeCheck.hs:99`, `TypeCheck.hs:103`):
///
/// ```text
/// Invalid conversion from `S` to `T`
/// Insert an explicit cast to silence this issue `cast<T>()`
/// ```
pub fn invalid_conversion_error(range: FileRange, from: &str, to: &str, hint: bool) -> Diagnostic {
    let mut message = format!("Invalid conversion from `{from}` to `{to}`");
    if hint {
        message.push_str(&format!(
            "\nInsert an explicit cast to silence this issue `cast<{to}>()`"
        ));
    }
    Diagnostic::error(range, message)
}

/// The machine-applicable form of `TypeCheck.hs:103`'s hint.
///
/// Two zero-width edits wrap the offending expression in `cast<T>(…)`, so no
/// source text is needed to build it. `range` is the expression the conversion
/// was checked on.
pub fn cast_fix(range: FileRange, to: &str) -> Fix {
    Fix {
        message: format!("Insert an explicit cast to silence this issue `cast<{to}>()`"),
        edits: vec![
            TextEdit {
                file: range.file,
                range: TextRange::empty(range.range.start()),
                new_text: format!("cast<{to}>("),
            },
            TextEdit {
                file: range.file,
                range: TextRange::empty(range.range.end()),
                new_text: ")".to_string(),
            },
        ],
    }
}

/// `"Invalid number of arguments, expected N"` (`TypeCheck.hs:91`).
pub fn invalid_argument_count_error(range: FileRange, expected: usize) -> Diagnostic {
    Diagnostic::error(range, format!("Invalid number of arguments, expected {expected}"))
}

/// Which arguments `checkArguments` actually converts (`TypeCheck.hs:90-94`).
///
/// Trap 17: a `[[pipelined]]` function's **first** argument is skipped entirely —
/// it is the pipeline token, whose type is elaborated at the call site
/// (`Type.hs:1368-1378`). The arity check at `TypeCheck.hs:91` still counts it.
/// Returns the index of the first argument to check.
pub fn first_checked_argument(pipelined: bool, arg_count: usize) -> usize {
    if pipelined && arg_count != 0 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::TyRef;
    use kg_base::{FileId, SymbolPath, TextSize};

    fn r(t: Ty) -> TyRef {
        TyRef::new(t)
    }

    fn range() -> FileRange {
        FileRange::new(FileId(0), TextRange::new(TextSize::new(10), TextSize::new(14)))
    }

    #[test]
    fn diagnostic_text_matches_the_reference() {
        let w = narrowing_warning(range(), "int32", "uint32");
        assert_eq!(w.message, "Narrowing conversion from `int32` to `uint32`");
        assert_eq!(w.warning, Some(WarningKind::Conversion));

        let e = invalid_conversion_error(range(), "S", "T", true);
        assert_eq!(
            e.message,
            "Invalid conversion from `S` to `T`\nInsert an explicit cast to silence this issue `cast<T>()`"
        );
        // `conversionError s t ""` (`TypeCheck.hs:178`): no hint.
        let e = invalid_conversion_error(range(), "bool", "uint1", false);
        assert_eq!(e.message, "Invalid conversion from `bool` to `uint1`");
    }

    #[test]
    fn corpus_constant_folding_silences_the_warning() {
        // Trap 19 — TypeCheck.hs:154.
        //   :106-116  `uint32 x, y; uint8 z = x + y;`        → warning:conversion
        //   :118-136  `const uint32 x, y; uint8 z = x + y;`  → warning:0
        // The second folds to the literal 12, whose *minimal* type is uint4.
        let target = Ty::Unsigned(8);
        assert_eq!(normalize_source(&Ty::Unsigned(33), &target, None), Ty::Unsigned(33));
        let folded = normalize_source(
            &Ty::Unsigned(33),
            &target,
            Some(&ConstValue::untyped_int(kg_const::ConstInt::from_i128(12))),
        );
        assert_eq!(folded, Ty::Unsigned(4), "inferIntLiteralType Nothing 12");
    }

    #[test]
    fn corpus_a_suffixed_literal_is_still_retyped_to_its_minimum() {
        // `checkConversion n t a@(IntLiteral _ x)` ignores the suffix entirely
        // (TypeCheck.hs:154) — the `_` is the `Maybe LiteralType`.
        use kg_const::{LiteralSign, LiteralType};
        let suffixed = ConstValue::typed_int(
            LiteralType { sign: LiteralSign::Unsigned, width: 32 },
            kg_const::ConstInt::from_i128(1),
        );
        assert_eq!(
            normalize_source(&Ty::Unsigned(32), &Ty::Unsigned(3), Some(&suffixed)),
            Ty::Unsigned(1)
        );
    }

    #[test]
    fn corpus_enum_value_unwraps_only_for_an_integer_target() {
        // conversion-warning.k:176-186 → `uint3 x = Foo::One` with
        // `enum Foo : uint16` is warning:0, because step 3 unwraps the
        // `EnumValue` down to the literal 1 (minimal type uint1).
        let foo = Ty::Enum(SymbolPath::empty(), r(Ty::Unsigned(16)));
        let value = ConstValue::Enum { name: SymbolPath::empty(), value: kg_const::ConstInt::from_i128(1) };
        assert_eq!(normalize_source(&foo, &Ty::Unsigned(3), Some(&value)), Ty::Unsigned(1));
        // ...and the guard really is on the target: a non-integer target keeps
        // the enum type (TypeCheck.hs:156).
        assert_eq!(normalize_source(&foo, &Ty::Boolean, Some(&value)), foo);
    }

    #[test]
    fn the_cast_fix_wraps_the_expression() {
        // The structured form of TypeCheck.hs:103.
        let fix = cast_fix(range(), "uint32");
        assert_eq!(fix.message, "Insert an explicit cast to silence this issue `cast<uint32>()`");
        assert_eq!(fix.edits.len(), 2);
        assert_eq!(fix.edits[0].new_text, "cast<uint32>(");
        assert_eq!(fix.edits[0].range, TextRange::empty(TextSize::new(10)));
        assert_eq!(fix.edits[1].new_text, ")");
        assert_eq!(fix.edits[1].range, TextRange::empty(TextSize::new(14)));
    }

    #[test]
    fn pipelined_functions_skip_their_first_argument() {
        // Trap 17 — TypeCheck.hs:92-93.
        assert_eq!(first_checked_argument(true, 3), 1);
        assert_eq!(first_checked_argument(false, 3), 0);
        // `&& not (null args)` — an empty argument list takes the `otherwise`
        // branch, which zips nothing.
        assert_eq!(first_checked_argument(true, 0), 0);
    }

    #[test]
    fn arity_error_text() {
        // TypeCheck.hs:91.
        assert_eq!(
            invalid_argument_count_error(range(), 2).message,
            "Invalid number of arguments, expected 2"
        );
    }
}
