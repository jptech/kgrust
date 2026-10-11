//! What a declaration's documentation says: the `//|` run before it and the
//! `//<` run after, as the reference's documentation generator reads them
//! (`kg_syntax::file_docs`). For the editor's hover and completion, and
//! `kg doc`.
//!
//! Read off the declaring file's parse, which is memoized; the walk that
//! finds the runs is linear in the file and runs per question, which is one
//! hover or one resolved completion at a time.

use kg_base::{FileId, FileRange};

use crate::db::Db;
use crate::navigate::Definition;

impl Db {
    /// The documentation written on the declaration at `range` — the name's
    /// range a [`Definition`] carries — as Markdown; `None` where it has
    /// none, or lives in a generated module (`.cmdargs`, `.options`), which
    /// has no text.
    pub fn documentation_at(&self, range: FileRange) -> Option<String> {
        if range.file == FileId::SYNTHETIC {
            return None;
        }
        let parse = self.parse(range.file);
        kg_syntax::file_docs(&parse.syntax_node()).around(range.range).map(kg_syntax::Doc::text)
    }

    /// [`Db::documentation_at`] the definition's name.
    pub fn documentation(&self, def: &Definition) -> Option<String> {
        self.documentation_at(def.range)
    }

    /// A module's own documentation, its header's: `/*| … */ module a.b`.
    pub fn module_documentation(&self, file: FileId) -> Option<String> {
        if file == FileId::SYNTHETIC {
            return None;
        }
        let parse = self.parse(file);
        kg_syntax::file_docs(&parse.syntax_node()).module().map(kg_syntax::Doc::text)
    }
}
