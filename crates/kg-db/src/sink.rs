//! The per-file diagnostic accumulator every pass writes to.

use kg_base::{FileId, FileRange, TextRange};
use kg_diag::Diagnostic;

/// Accumulates diagnostics for one file.
///
/// Every error here is `Error 1` — the reference frontend has exactly one code
/// (`Parser/IO.hs:114`), which is why frontend conformance is an accept/reject
/// question and never a message-text one.
pub(crate) struct Sink {
    pub(crate) file: FileId,
    pub(crate) out: Vec<Diagnostic>,
}

impl Sink {
    pub(crate) fn error(&mut self, range: TextRange, message: impl Into<String>) {
        self.out.push(Diagnostic::error(FileRange::new(self.file, range), message));
    }
}
