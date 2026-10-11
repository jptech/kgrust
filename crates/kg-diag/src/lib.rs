//! Diagnostics.
//!
//! Two consumers with incompatible needs share one model:
//!
//! * the `kanagawa` compat binary, whose stdout is matched by
//!   `test/syntax/syntax-test.py` with the regexes `Error (\d+)` and
//!   `warning:.*\[--Wno-([^\]]*)\]$`, and
//! * the language server, which wants ranges, related information and fix-its.
//!
//! Reproducing the first is a hard constraint; every frontend error the
//! reference reports is `Error 1` (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114`
//! and `compiler/hs/app/Main.hs:99`), so the *wording* is ours to improve but the
//! *shape* is not.

mod render;

pub use render::{Located, SourceView, render_legacy, render_legacy_all, render_legacy_located};

use kg_base::{FileId, FileRange, TextRange};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

/// The number that appears after `Error ` in the reference's output.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Code {
    /// Every frontend error. There is exactly one frontend code, which is why
    /// frontend conformance is an *accept/reject* question, not a message-text
    /// question.
    Frontend,
    /// A code from `compiler/cpp/errors.h`, produced by the C++ middle end.
    ///
    /// The Rust frontend must **not** start reporting these earlier than the
    /// reference does: 241 of the 2,843 corpus assertions expect a specific C++
    /// code, and a "better" frontend that catches those first fails them.
    Backend(u16),
}

impl Code {
    pub fn number(self) -> u16 {
        match self {
            Code::Frontend => 1,
            Code::Backend(n) => n,
        }
    }
}

/// The six warning kinds of `compiler/hs/lib/Language/Kanagawa/Warning.hs:20-35`.
///
/// The set is closed: `--W<name>` / `--Wno-<name>` accept exactly these, and the
/// test suite matches on the `[--Wno-<name>]` suffix.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum WarningKind {
    Conversion,
    Deprecated,
    InitializerOverrides,
    RedundantImport,
    Shadow,
    TransactionSize,
}

impl WarningKind {
    pub const ALL: [WarningKind; 6] = [
        WarningKind::Conversion,
        WarningKind::Deprecated,
        WarningKind::InitializerOverrides,
        WarningKind::RedundantImport,
        WarningKind::Shadow,
        WarningKind::TransactionSize,
    ];

    /// The spelling used in `--W<name>` and in the `[--Wno-<name>]` suffix.
    pub fn name(self) -> &'static str {
        match self {
            WarningKind::Conversion => "conversion",
            WarningKind::Deprecated => "deprecated",
            WarningKind::InitializerOverrides => "initializer-overrides",
            WarningKind::RedundantImport => "redundant-import",
            WarningKind::Shadow => "shadow",
            // Note the mismatch with the constructor name
            // (`TransactionSizeMissing`): the *switch* is `transaction-size`
            // (`Warning.hs:34`).
            WarningKind::TransactionSize => "transaction-size",
        }
    }

    pub fn from_name(name: &str) -> Option<WarningKind> {
        WarningKind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// Which of the reference's messages a diagnostic is — for code that must
/// decide by it, so that no pass tests a message's text (sprint W8.1). One
/// variant per message family a pass here asks about, each cited where the
/// reference makes it; a diagnostic nothing asks about has none
/// ([`Diagnostic::kind`] is `None`). The text stays what the user reads and
/// what `syntax-test.py` never reads: the legacy renderer prints no kind.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum DiagnosticKind {
    /// "Could not deduce all arguments for template `…`" — the `LT` arm of
    /// `unresolvedTemplates` (`Frontend.hs:421`). Asked by the pass-9
    /// checkpoint, which it must not close (`kg-db` `passes/mod.rs`), and
    /// by `deduceAutoTypes`' return-type arm (`passes/deduce_auto.rs`).
    CouldNotDeduceAll,
    /// "static if predicate did not evaluate to a boolean constant" —
    /// `unresolved StaticIfF{}` (`Frontend.hs:406`). Asked by
    /// `deduceAutoTypes`' return-type arm.
    StaticIfUndecided,
    /// "static assertion expression did not evaluate to a boolean constant"
    /// — `staticAssert`'s last arm (`Frontend.hs:396-400`). Asked by pass
    /// 16 at the text, which drops pass 10's verdicts inside the operand it
    /// replaced (`kg_sem::check::earlier_text`).
    StaticAssertUndecided,
    /// "int width did not evaluate to a positive integer constant", and its
    /// `uint` twin — `unresolved ParamIntF{}` (`Frontend.hs:404-405`). Asked
    /// by pass 19, which never sees inside the width it replaced.
    WidthUndecided,
    /// "Conflicting template argument deduction results for parameter `…`.
    /// Could be `…` or `…`." — `reconcile`'s `TError` (`Type.hs:1502-1506`).
    /// Asked by `kg_sem::check`, which raises it as the round's error
    /// wherever it stands; carried on the IR's error node
    /// (`kg_hir::ErrorNote`) from the deduction that made it.
    ConflictingDeduction,
}

impl DiagnosticKind {
    /// A stable spelling, for machine-readable output.
    pub fn name(self) -> &'static str {
        match self {
            DiagnosticKind::CouldNotDeduceAll => "could-not-deduce-all",
            DiagnosticKind::StaticIfUndecided => "static-if-undecided",
            DiagnosticKind::StaticAssertUndecided => "static-assert-undecided",
            DiagnosticKind::WidthUndecided => "width-undecided",
            DiagnosticKind::ConflictingDeduction => "conflicting-deduction",
        }
    }
}

/// A secondary span that explains the primary one.
#[derive(Clone, Debug)]
pub struct Label {
    pub range: FileRange,
    pub message: String,
    /// For a `while compiling` note, the instance that frame compiles,
    /// spelled as a message spells one — `Box<4>::get<uint3>`. The legacy
    /// renderer prints the reference's bare note; `kg check` and the editor
    /// name the instance.
    pub instance: Option<String>,
}

impl Label {
    /// The note as `kg check` and the editor say it: the message, and for
    /// a frame the instance it compiles — "while compiling `Box<4>`". The
    /// legacy renderer ([`crate::render`]) prints [`Label::message`] alone,
    /// as the reference does.
    pub fn said(&self) -> String {
        match &self.instance {
            Some(instance) => format!("{} `{instance}`", self.message),
            None => self.message.clone(),
        }
    }
}

/// A machine-applicable edit offered with a diagnostic.
///
/// Two already exist as prose in the reference — the `cast<uint32>()` conversion
/// hint and the missing `[[transaction_size(N)]]` warning — and become real code
/// actions here.
#[derive(Clone, Debug)]
pub struct Fix {
    pub message: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Clone, Debug)]
pub struct TextEdit {
    pub file: FileId,
    pub range: TextRange,
    pub new_text: String,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    /// Set for warnings; drives the `[--Wno-<name>]` suffix and `--Wno-` filtering.
    pub warning: Option<WarningKind>,
    /// Which message this is, where a pass decides by it
    /// ([`DiagnosticKind`]); set where the diagnostic is made.
    pub kind: Option<DiagnosticKind>,
    pub message: String,
    pub range: FileRange,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    /// A frontend error: everything the reference reports as `Error 1`.
    pub fn error(range: FileRange, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            code: Code::Frontend,
            severity: Severity::Error,
            warning: None,
            kind: None,
            message: message.into(),
            range,
            labels: Vec::new(),
            notes: Vec::new(),
            fixes: Vec::new(),
        }
    }

    pub fn warning(range: FileRange, kind: WarningKind, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            code: Code::Frontend,
            severity: Severity::Warning,
            warning: Some(kind),
            kind: None,
            message: message.into(),
            range,
            labels: Vec::new(),
            notes: Vec::new(),
            fixes: Vec::new(),
        }
    }

    /// This diagnostic as the message family `kind` ([`DiagnosticKind`]).
    pub fn with_kind(mut self, kind: impl Into<Option<DiagnosticKind>>) -> Diagnostic {
        self.kind = kind.into();
        self
    }

    /// Whether this is the message family `kind`.
    pub fn is(&self, kind: DiagnosticKind) -> bool {
        self.kind == Some(kind)
    }

    pub fn with_label(mut self, range: FileRange, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label { range, message: message.into(), instance: None });
        self
    }

    /// [`Diagnostic::with_label`] for a `while compiling` frame, with the
    /// instance it compiles where one is named.
    pub fn with_frame(mut self, range: FileRange, instance: Option<String>) -> Diagnostic {
        self.labels.push(Label { range, message: "while compiling".to_owned(), instance: instance.filter(|i| !i.is_empty()) });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn with_fix(mut self, fix: Fix) -> Diagnostic {
        self.fixes.push(fix);
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Which warnings are enabled, as built up by `--W<name>` / `--Wno-<name>`.
///
/// `enabledWarnings` folds the switches left-to-right with insert/delete
/// (`Warning.hs:46-50`), so a later switch always wins and the starting set is
/// empty — `--Wall` is expanded by the CLI into one `--W<name>` per kind.
#[derive(Clone, Debug, Default)]
pub struct WarningSet {
    enabled: [bool; 6],
}

impl WarningSet {
    pub fn none() -> WarningSet {
        WarningSet::default()
    }

    pub fn all() -> WarningSet {
        WarningSet { enabled: [true; 6] }
    }

    pub fn enable(&mut self, kind: WarningKind) {
        self.enabled[kind as usize] = true;
    }

    pub fn disable(&mut self, kind: WarningKind) {
        self.enabled[kind as usize] = false;
    }

    pub fn is_enabled(&self, kind: WarningKind) -> bool {
        self.enabled[kind as usize]
    }

    /// Drops diagnostics for disabled warnings.
    pub fn filter(&self, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        diagnostics
            .into_iter()
            .filter(|d| d.warning.is_none_or(|k| self.is_enabled(k)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_names_round_trip() {
        for kind in WarningKind::ALL {
            assert_eq!(WarningKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(WarningKind::from_name("transaction-size-missing"), None);
    }

    #[test]
    fn later_switches_win() {
        let mut set = WarningSet::all();
        set.disable(WarningKind::Shadow);
        assert!(!set.is_enabled(WarningKind::Shadow));
        assert!(set.is_enabled(WarningKind::Conversion));
        set.enable(WarningKind::Shadow);
        assert!(set.is_enabled(WarningKind::Shadow));
    }
}
