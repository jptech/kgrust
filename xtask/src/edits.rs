//! The four edits a keystroke is measured and checked by (sprint W0.5,
//! W7.1): inside a function body, to a signature, to an import, and inside
//! a body in a library module the root imports. Each is a text edit made by
//! position in the parse, deterministic, and undone by putting the old text
//! back. They are made in a `Db`'s overlay only; nothing is written to disk.
//!
//! The edits are not meant to keep a program correct — a signature edit
//! breaks its callers on purpose. What they must be is the *kind* of change
//! an editor sends: `edit-identity` checks a warm `Db` against a fresh one
//! after each, and the keystroke table prices each.

use kg_syntax::{AstNode, ast};

/// Where an edit lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A statement added at the top of the first function body.
    Body,
    /// A parameter added to the first function's parameter list.
    Signature,
    /// The first import removed.
    Import,
    /// [`Kind::Body`], in the first library module the root imports.
    LibraryBody,
}

impl Kind {
    pub(crate) const ALL: [Kind; 4] = [Kind::Body, Kind::Signature, Kind::Import, Kind::LibraryBody];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Body => "body",
            Kind::Signature => "signature",
            Kind::Import => "import",
            Kind::LibraryBody => "library body",
        }
    }
}

/// `text` with the edit of `kind` made, or `None` when it has nowhere to
/// land (no function with a body, no import). [`Kind::LibraryBody`] edits
/// the library module's text, so it is [`Kind::Body`] here.
pub(crate) fn apply(kind: Kind, text: &str) -> Option<String> {
    splice(kind, text).map(|s| s.apply(text))
}

/// One edit as a splice: `removed` bytes at `at` replaced by `inserted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Splice {
    pub(crate) at: usize,
    pub(crate) removed: usize,
    pub(crate) inserted: &'static str,
}

impl Splice {
    pub(crate) fn apply(&self, text: &str) -> String {
        format!("{}{}{}", &text[..self.at], self.inserted, &text[self.at + self.removed..])
    }

    /// The splice as the loop's snapshot carries positions through it
    /// (`kg_sem::snapshot::Splice`): one rule, the editor's and the
    /// measurements'.
    pub(crate) fn map(&self) -> kg_sem::snapshot::Splice {
        kg_sem::snapshot::Splice { at: self.at, removed: self.removed, inserted: self.inserted.len() }
    }

    /// A range of the old text in the new (`kg_sem::snapshot::Splice::range`).
    pub(crate) fn range(&self, start: usize, end: usize) -> Option<(usize, usize)> {
        self.map().range(start, end)
    }
}

/// [`apply`]'s edit as a [`Splice`] of `text`.
pub(crate) fn splice(kind: Kind, text: &str) -> Option<Splice> {
    let parse = kg_syntax::parse_file(text);
    let file = parse.source_file();
    let syntax = file.syntax();
    match kind {
        Kind::Body | Kind::LibraryBody => {
            let body = syntax.descendants().filter_map(ast::FnDecl::cast).find_map(|f| f.body())?;
            let at = usize::from(body.l_curly_token()?.text_range().end());
            Some(Splice { at, removed: 0, inserted: " uint32 kg_edit_probe = 0;" })
        }
        Kind::Signature => {
            let params = syntax.descendants().filter_map(ast::FnDecl::cast).find_map(|f| f.param_list())?;
            let close = params.syntax().last_token()?;
            if close.text() != ")" {
                return None;
            }
            let empty = params.syntax().children().next().is_none();
            let at = usize::from(close.text_range().start());
            Some(Splice { at, removed: 0, inserted: if empty { "uint1 kg_edit_probe" } else { ", uint1 kg_edit_probe" } })
        }
        Kind::Import => {
            let import = syntax.descendants().find_map(ast::ImportDecl::cast)?;
            let range = import.syntax().text_range();
            let (start, end) = (usize::from(range.start()), usize::from(range.end()));
            Some(Splice { at: start, removed: end - start, inserted: "" })
        }
    }
}

/// Where [`Kind::LibraryBody`] lands for the program rooted at `root`: the
/// first module the root imports by a written `import` — not the implicit
/// base, `base.k`, which declares no function to edit — that lives under
/// `library` and takes the edit. Its path, its text, and the edited text.
pub(crate) fn library_target(
    db: &kg_db::Db,
    root: kg_base::FileId,
    library: &std::path::Path,
) -> Option<(camino::Utf8PathBuf, String, String)> {
    library_splice(db, root, library).map(|(path, before, splice)| {
        let after = splice.apply(&before);
        (path, before, after)
    })
}

/// [`library_target`]'s edit as a [`Splice`] of the module's text.
pub(crate) fn library_splice(
    db: &kg_db::Db,
    root: kg_base::FileId,
    library: &std::path::Path,
) -> Option<(camino::Utf8PathBuf, String, Splice)> {
    let program = db.program(&[root]);
    program.graph.resolved_imports(kg_db::ModuleSource::File(root)).find_map(|(decl, target)| {
        let kg_db::ModuleSource::File(f) = target else { return None };
        if decl.name.is_empty() {
            return None;
        }
        let path = db.path(f).filter(|p| p.as_std_path().starts_with(library))?;
        let before = std::fs::read_to_string(&path).ok()?;
        let splice = splice(Kind::LibraryBody, &before)?;
        Some((path, before, splice))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROGRAM: &str = "import data.array\n\ninline uint32 add(uint32 a, uint32 b)\n{\n    return a + b;\n}\n\ninline void none()\n{\n}\n";

    /// Each edit lands where it says and nowhere else. Red with a body
    /// statement inserted before the brace, with the parameter appended
    /// after the parenthesis, or with the import kept.
    #[test]
    fn each_edit_lands_where_it_says() {
        let body = apply(Kind::Body, PROGRAM).expect("a body");
        assert!(body.contains("{ uint32 kg_edit_probe = 0;\n    return a + b;"), "{body}");
        let sig = apply(Kind::Signature, PROGRAM).expect("a signature");
        assert!(sig.contains("add(uint32 a, uint32 b, uint1 kg_edit_probe)"), "{sig}");
        let import = apply(Kind::Import, PROGRAM).expect("an import");
        assert!(!import.contains("import"), "{import}");
        assert!(import.starts_with("\n\ninline uint32 add"), "{import}");
        assert_eq!(apply(Kind::Import, "inline void f() {}\n"), None);
    }

    /// An offset before the splice stays, one after it moves by what the
    /// splice added, and one inside what it removed has no place. A range
    /// that ends where text is inserted ends before it; one that starts
    /// there moves. Red with `start`'s `<` made `<=`, or `end`'s `<=` made
    /// `<` (`kg_sem::snapshot::Splice`, which this is).
    #[test]
    fn a_splice_maps_offsets_around_itself() {
        let insert = splice(Kind::Body, PROGRAM).expect("a body").map();
        let at = insert.at;
        let grown = insert.inserted;
        assert_eq!(insert.start(at - 1), Some(at - 1));
        assert_eq!(insert.start(at), Some(at + grown));
        assert_eq!(insert.end(at), Some(at));
        assert_eq!(insert.end(at + 1), Some(at + 1 + grown));
        let after = apply(Kind::Body, PROGRAM).expect("a body");
        assert_eq!(&after[insert.start(at).unwrap()..], &PROGRAM[at..]);
        let remove = splice(Kind::Import, PROGRAM).expect("an import").map();
        assert_eq!((remove.at, remove.removed), (0, "import data.array".len()));
        assert_eq!(remove.start(3), None);
        assert_eq!(remove.end(3), None);
        assert_eq!(remove.end(remove.removed), Some(0));
        assert_eq!(remove.start(remove.removed + 2), Some(2));
    }

    #[test]
    fn an_empty_parameter_list_gets_no_comma() {
        let sig = apply(Kind::Signature, "inline void none()\n{\n}\n").expect("a signature");
        assert!(sig.contains("none(uint1 kg_edit_probe)"), "{sig}");
    }
}
