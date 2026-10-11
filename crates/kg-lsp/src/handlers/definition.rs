//! `textDocument/definition` — jump to the declaration a name denotes.
//!
//! The resolution is [`kg_db::Db::definition_at`] and nothing here re-decides
//! any part of it. That is deliberate: goto-definition and the
//! `"name not in scope"` diagnostic are two readings of one algorithm
//! (`kg_db::navigate`), and a resolver written in the language server would be
//! free to drift from the one that decides the diagnostic. This module is the
//! protocol adapter over that query — position → offset, `FileId` → `Uri`, byte
//! range → `Range` — and is the only place that knows an editor is asking.
//!
//! # Declining is an answer
//!
//! [`kg_db::Db::definitions_at`] answers nothing for every case it cannot
//! decide: an import qualifier, a member whose receiver no engine types, an
//! intrinsic, a name two imports both expose, a declaration under an
//! unelaborated `static if`, a path segment nothing keys. Each is enumerated
//! with its reference citation on [`kg_db::Db::definition_at`]. All of them
//! arrive here as `null`, which a client renders as "no definition found" —
//! true, and better than a jump to a plausible wrong `x`, which silently
//! teaches the reader something untrue.
//!
//! An import is answered by [`kg_db::Db::import_at`] instead: its module path
//! or alias leads to the module's file, the one the program loaded for it.
//!
//! A member after a `.` — `c.go()`, `fixture.Run<3>(t)`, `a.items` — is
//! answered through the receiver's type as the program-wide loop has it,
//! and lands on the member as the class's text declares it: an instance's
//! copy on the template's member, a member template on its `template <…>`.
//!
//! One decline is not the query's but the protocol's: a declaration in the
//! generated `.cmdargs` / `.options` modules is a well-formed [`Definition`]
//! with no on-disk home, so there is nothing for the client to open. That
//! decision — along with reading the target file's own line table, and
//! reporting the target's own URI — belongs to `crate::handlers::location`,
//! which `references` answers through too: two features that disagreed about
//! which file a declaration is in would be two answers to one question.

use kg_db::Definition;
use lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, Location};

use crate::convert;
use crate::global_state::GlobalState;
use crate::handlers::Answer;

/// Answers with one [`Location`], or — for a member whose receiver's
/// instances reach several declarations — each of them.
///
/// `Scalar`, never `Link`: a [`lsp_types::LocationLink`] may only be sent to a
/// client that advertised `textDocument.definition.linkSupport`, and the server
/// does not retain the client's capabilities. A plain `Location` is what every
/// client understands.
///
/// # Several places, when the text means several
///
/// Inside a template's body a receiver has a type per instance, and the
/// member it names may be written apart in each: a class declaring `go` in
/// both branches of a `static if (B)`, reached from a body instantiated with
/// `B` true and false, is two declarations. Both are the answer — the
/// protocol's `Location[]`, which a client shows as a list to pick from — in
/// the text's order ([`kg_db::Db::definitions_at`]). A jump to one would be
/// a guess about which instance the reader means; the queries that need one
/// declaration (references, rename, hover's kind line) decline or list
/// instead.
///
/// `Ok(None)` and `Err(NoFile)` are different facts — "I resolved nothing" and
/// "I have no such document" — but the wire cannot tell them apart: the
/// dispatcher turns both into `null` (`main_loop::on!`). They are kept distinct
/// here because a test can tell them apart, and because a handler that returned
/// `Ok(None)` for an unopened document would be claiming to have looked.
pub fn definition(
    state: &GlobalState,
    params: &GotoDefinitionParams,
) -> Answer<Option<GotoDefinitionResponse>> {
    let at = &params.text_document_position_params;
    let snap = state.snapshot(&at.text_document.uri)?;
    let offset = convert::offset(snap.line_index(), state.encoding(), at.position);

    // The file is its own root, matching `GlobalState::frontend_diagnostics`:
    // an editor is looking at one buffer, and the program a jump is answered
    // against is the one that buffer entails.
    let (module, found) = state
        .with_db(snap.file, |db| match db.import_at(&[snap.file], snap.file, offset) {
            Some(module) => (Some(module), Vec::new()),
            None => (None, db.definitions_at(&[snap.file], snap.file, offset)),
        })
        .unwrap_or_default();
    // The module's first line: an import names the file, not a place in it.
    if let Some(module) = module {
        let start = kg_base::TextRange::empty(kg_base::TextSize::new(0));
        return Ok(crate::handlers::location(state, state.encoding(), kg_base::FileRange::new(module, start))
            .map(GotoDefinitionResponse::Scalar));
    }

    let mut locations: Vec<Location> = found.iter().filter_map(|found| location(state, found)).collect();
    Ok(match locations.len() {
        0 => None,
        1 => locations.pop().map(GotoDefinitionResponse::Scalar),
        _ => Some(GotoDefinitionResponse::Array(locations)),
    })
}

/// `textDocument/typeDefinition` — the declaration of the type the
/// program-wide loop gives the name under the caret
/// ([`kg_db::Db::type_definitions_at`]): a variable's struct or class, an
/// instance's at its template's text; inside a template's body, each
/// instance's. Declines, like [`definition`], where the query does.
pub fn type_definition(
    state: &GlobalState,
    params: &lsp_types::request::GotoTypeDefinitionParams,
) -> Answer<Option<GotoDefinitionResponse>> {
    let at = &params.text_document_position_params;
    let snap = state.snapshot(&at.text_document.uri)?;
    let offset = convert::offset(snap.line_index(), state.encoding(), at.position);
    let found = state.with_db(snap.file, |db| db.type_definitions_at(&[snap.file], snap.file, offset)).unwrap_or_default();
    let mut locations: Vec<Location> = found.iter().filter_map(|found| location(state, found)).collect();
    Ok(match locations.len() {
        0 => None,
        1 => locations.pop().map(GotoDefinitionResponse::Scalar),
        _ => Some(GotoDefinitionResponse::Array(locations)),
    })
}

/// The `Location` a [`Definition`] denotes, or `None` when it cannot be opened.
///
/// Ten of the 7,884 resolutions measured over the reference library are in a
/// generated module and have no location at all, so that decline — made by
/// [`crate::handlers::location`], with its citation — is not a theoretical case.
fn location(state: &GlobalState, found: &Definition) -> Option<Location> {
    crate::handlers::location(state, state.encoding(), found.range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::PositionEncoding;
    use crate::global_state::{NoFile, test_uri};
    use kg_base::FileId;
    use lsp_types::{
        PartialResultParams, Position, Range, TextDocumentIdentifier, TextDocumentPositionParams,
        Uri, WorkDoneProgressParams,
    };

    fn params(uri: &Uri, position: Position) -> GotoDefinitionParams {
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        }
    }

    /// The single location a jump answers with, or `None`. Panics on `Array` or
    /// `Link`, which this handler must never produce.
    #[track_caller]
    fn jump(state: &GlobalState, uri: &Uri, position: Position) -> Option<Location> {
        match definition(state, &params(uri, position)).expect("the document is open") {
            None => None,
            Some(GotoDefinitionResponse::Scalar(location)) => Some(location),
            Some(other) => panic!("expected one bare Location, got {other:?}"),
        }
    }

    const SRC: &str = "\
class Adder {
    uint32 total;
    inline void add(uint32 x) {
        total = total + x;
    }
}
";

    fn state() -> (GlobalState, Uri) {
        let state = GlobalState::new();
        let uri = test_uri("adder.k");
        state.did_open(uri.clone(), SRC.to_owned(), 1);
        (state, uri)
    }

    /// The caret is on the second `total` of `total = total + x`, and the answer
    /// is the field's *name token* — `Item::range` is `symbolLoc`
    /// (`SymbolTable.hs:199`), not the whole declaration — so the editor lands
    /// on the identifier rather than on `uint32`.
    #[test]
    fn a_use_jumps_to_the_name_token_of_its_declaration() {
        let (state, uri) = state();
        let found = jump(&state, &uri, Position::new(3, 18)).expect("`total` must resolve");
        assert_eq!(found.uri, uri);
        assert_eq!(found.range, Range::new(Position::new(1, 11), Position::new(1, 16)));
    }

    /// A declaration in another file must produce *that* file's URI. The
    /// imported module is only an overlay — never written to disk — because the
    /// module search reads through [`kg_vfs::Vfs`], which is exactly what lets a
    /// jump land in a buffer the user has edited but not saved.
    #[test]
    fn a_definition_in_another_file_answers_that_files_uri() {
        let state = GlobalState::new();
        let library = test_uri("a.k");
        let main = test_uri("main.k");
        let library_src = "module a\n    { module a\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n";
        let main_src = "import a\n\ninline void main()\n{\n    Aa v;\n}\n";
        state.did_open(library.clone(), library_src.to_owned(), 1);
        state.did_open(main.clone(), main_src.to_owned(), 1);

        // The caret is on `Aa` in `main.k`; `struct Aa` is on line 4 of `a.k`.
        let found = jump(&state, &main, Position::new(4, 5)).expect("the import must resolve");
        assert_eq!(found.uri, library, "the jump reported the requesting file's URI");
        assert_eq!(found.range, Range::new(Position::new(4, 7), Position::new(4, 9)));
    }

    /// The module path of an import leads to the module's file, at its
    /// first line — the file the program loaded for it, an open buffer
    /// here. Red with the handler asking `definitions_at` alone, which
    /// declines on an import.
    #[test]
    fn an_import_leads_to_its_module_file() {
        let state = GlobalState::new();
        let library = test_uri("a.k");
        let main = test_uri("main.k");
        state.did_open(library.clone(), "module a\n    { module a\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n".to_owned(), 1);
        state.did_open(main.clone(), "import a\n\ninline void main()\n{\n    Aa v;\n}\n".to_owned(), 1);
        let found = jump(&state, &main, Position::new(0, 7)).expect("the import must lead to its module");
        assert_eq!(found.uri, library);
        assert_eq!(found.range, Range::new(Position::new(0, 0), Position::new(0, 0)));
    }

    /// The type of a variable leads to its struct's declaration, in the file
    /// that declares it. Red with the request answered by `definitions_at`
    /// (the variable itself).
    #[test]
    fn a_variable_s_type_definition_is_its_struct() {
        let state = GlobalState::new();
        let library = test_uri("a.k");
        let main = test_uri("main.k");
        state.did_open(library.clone(), "module a\n    { module a\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n".to_owned(), 1);
        state.did_open(main.clone(), "import a\n\ninline void main()\n{\n    Aa v;\n    v.x = 1;\n}\n".to_owned(), 1);
        let params = lsp_types::request::GotoTypeDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: main.clone() },
                position: Position::new(5, 4),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let Some(GotoDefinitionResponse::Scalar(found)) = type_definition(&state, &params).expect("open") else {
            panic!("one location")
        };
        assert_eq!(found.uri, library);
        assert_eq!(found.range, Range::new(Position::new(4, 7), Position::new(4, 9)), "`Aa` of `struct Aa`");
    }

    /// The column is in the *negotiated* encoding, not in bytes.
    ///
    /// The two columns below address the same identifier and differ by six,
    /// because the three crabs cost four bytes each and two UTF-16 units each.
    /// A handler that skipped [`crate::convert`] would read one of them as the
    /// other and land inside the comment.
    #[test]
    fn the_caret_column_is_read_in_the_negotiated_encoding() {
        const WIDE: &str = "\
inline void main()
{
    uint32 total = 1;
    /* 🦀🦀🦀 */ total = total + 1;
}
";
        let line = WIDE.lines().nth(3).unwrap();
        let utf16 = line.find("total").unwrap() as u32 - 6;
        let bytes = line.find("total").unwrap() as u32;
        assert_eq!(bytes - utf16, 6, "the fixture must make the encodings disagree");

        let declaration = Range::new(Position::new(2, 11), Position::new(2, 16));
        for (enc, column) in
            [(PositionEncoding::Utf16, utf16), (PositionEncoding::Utf8, bytes)]
        {
            let state = GlobalState::new();
            state.set_encoding(enc);
            let uri = test_uri("wide.k");
            state.did_open(uri.clone(), WIDE.to_owned(), 1);

            let found = jump(&state, &uri, Position::new(3, column))
                .unwrap_or_else(|| panic!("`total` must resolve under {enc:?}"));
            assert_eq!(found.range, declaration, "under {enc:?}");
        }
    }

    /// A caret on punctuation has no name to resolve, and offering the nearest
    /// identifier would be a guess about what was pointed at.
    #[test]
    fn a_caret_that_is_not_on_an_identifier_answers_nothing() {
        let (state, uri) = state();
        // The `+` of `total + x`.
        assert_eq!(&SRC.lines().nth(3).unwrap()[22..23], "+");
        assert!(jump(&state, &uri, Position::new(3, 22)).is_none());
        // Past the end of the buffer, which a client may legitimately send
        // after an edit the server has not seen yet.
        assert!(jump(&state, &uri, Position::new(999, 0)).is_none());
    }

    /// A name that resolves to nothing declines rather than erroring: the
    /// request succeeded, the answer is "no definition".
    #[test]
    fn an_unresolvable_name_declines_without_failing_the_request() {
        let state = GlobalState::new();
        let uri = test_uri("unknown.k");
        let src = "inline void main()\n{\n    uint32 v = nosuch;\n}\n";
        state.did_open(uri.clone(), src.to_owned(), 1);
        assert_eq!(definition(&state, &params(&uri, Position::new(2, 15))).unwrap(), None);
    }

    /// A document the server never saw is `NoFile`, not an empty answer. The
    /// distinction is invisible on the wire and load-bearing in the handler: an
    /// unopened URI must not cause a program to be loaded for it.
    #[test]
    fn an_unknown_document_is_declined_not_faked() {
        let state = GlobalState::new();
        let uri = test_uri("never-opened.k");
        let err = definition(&state, &params(&uri, Position::new(0, 0))).unwrap_err();
        assert_eq!(err, NoFile::NotOpen);
    }

    /// The hazard [`location`] exists to handle: a `--define`d name resolves to
    /// a real declaration in the synthetic `.cmdargs` module, which has no file
    /// on disk (`Parser/IO.hs:101-102`).
    ///
    /// Both halves are asserted, so this cannot go green by the name simply
    /// failing to resolve — the query must find the declaration *and* the
    /// handler must decline to build a `Location` from it.
    #[test]
    fn a_declaration_in_a_generated_module_has_no_location_to_offer() {
        let dir = std::env::temp_dir().join(format!("kg-lsp-def-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // No `import-dirs`, so the implicit base *is* `.cmdargs`
        // (`Parser/IO.hs:71-75`) and every open file imports it.
        std::fs::write(
            dir.join("kanagawa.toml"),
            "[project]\n[project.defines]\nWIDTH = \"8\"\n",
        )
        .unwrap();

        let root = camino::Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
        let state = GlobalState::new();
        state.set_roots(vec![root.clone()]);
        let uri = crate::uri::path_to_uri(&root.join("top.k")).unwrap();
        let file = state.did_open(uri.clone(), "static assert(WIDTH == 8);\n".to_owned(), 1);

        let found = state
            .with_db(file, |db| db.definition_at(&[file], file, kg_base::TextSize::new(14)))
            .flatten()
            .expect("a --define'd name must resolve to its .cmdargs declaration");
        assert_eq!(found.range.file, FileId::SYNTHETIC);

        assert_eq!(
            jump(&state, &uri, Position::new(0, 14)),
            None,
            "a synthetic module has no URI, so there is nothing to open"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
