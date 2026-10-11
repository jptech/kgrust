//! Request handlers.
//!
//! Every one is a pure function of `(&GlobalState, params)`. Nothing here
//! touches the connection, spawns anything, or mutates state, which is what
//! makes each of them callable straight from a unit test — see the `tests`
//! module at the bottom of this file, and the one in every submodule, all of
//! which drive real handlers against documents that were never sent over a
//! wire.
//!
//! # Where a handler's body lives
//!
//! A *syntactic* feature — outline, folding, selection, semantic tokens,
//! formatting — is a pure function of a `SyntaxNode` and a `LineIndex`, so its
//! submodule exposes exactly that and the `(&GlobalState, params)` wrapper the
//! dispatcher calls lives here. That is what lets those algorithms be tested
//! with no `GlobalState` at all.
//!
//! A *semantic* feature — [`definition`], [`hover`], [`references`],
//! [`inlay_hints`], [`document_highlight`], [`rename`] and [`signature_help`] — has to reach
//! `GlobalState::with_db`, so there is nothing smaller than the handler to
//! expose and the handler itself lives in the submodule. There is no wrapper
//! for those: `main_loop` names them `handlers::<module>::<module>`, and a
//! second entry point here would be a second thing to keep in step.

pub mod code_action;
pub mod completion;
pub mod definition;
pub mod diagnostics;
pub mod document_highlight;
pub mod document_symbol;
pub mod folding_range;
pub mod hover;
pub mod inlay_hints;
mod instances;
pub mod references;
pub mod rename;
pub mod selection_range;
pub mod semantic_tokens;
pub mod signature_help;
mod sources;
pub mod workspace_symbol;

use kg_base::{FileId, FileRange};
use lsp_types::{
    DocumentFormattingParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, FoldingRange, FoldingRangeParams, Location, PublishDiagnosticsParams,
    SelectionRange, SelectionRangeParams, SemanticTokens, SemanticTokensParams,
    SemanticTokensRangeParams, SemanticTokensRangeResult, SemanticTokensResult, TextEdit, Uri,
};

use crate::convert::{self, PositionEncoding};
use crate::fmt;
use crate::global_state::{FileSnapshot, GlobalState, NoFile};

/// A handler could not answer. Reported to the client as an empty/`null`
/// result rather than as a protocol error: "I have nothing for that document"
/// is a normal state, not a failure.
pub type Answer<T> = Result<T, NoFile>;

/// The [`Location`] a [`kg_base::FileRange`] denotes, or `None` when there is
/// nothing for the client to open.
///
/// Shared by [`definition`] and [`references`], which would otherwise each own a
/// copy of the same three decisions:
///
/// * **the URI is the target's own**, never the requesting document's;
/// * **the line table is the target file's**, read from the VFS rather than
///   taken from the request's snapshot. The target is usually not the file that
///   was asked about and is often not open at all — and this is the same read
///   [`kg_db::Db::parse`] made, so the byte range and the line table describe
///   the same text, unsaved overlay included;
/// * **a synthetic file has no location.** A declaration in the generated
///   `.cmdargs` / `.options` modules is a well-formed [`kg_db::Definition`]
///   whose file is [`FileId::SYNTHETIC`]; those modules are built from the
///   manifest's `defines` / `using` and have no path (`Parser/IO.hs:101-102`),
///   so there is no position for a client to open.
pub(crate) fn location(
    state: &GlobalState,
    enc: PositionEncoding,
    at: FileRange,
) -> Option<Location> {
    // Stated rather than left to fall out: `FileId::SYNTHETIC` is `u32::MAX`
    // and no `Vfs` will ever intern it, so `uri_of` below would decline anyway
    // — but that is an invariant of another crate, and the reason a generated
    // module has no location is not that a lookup happens to miss.
    if at.file == FileId::SYNTHETIC {
        return None;
    }
    let uri = uri_of(state, at.file)?;
    let text = state.vfs().read(at.file)?;
    Some(Location { uri, range: convert::range(&text.line_index, enc, at.range) })
}

/// The URI to report for `file`.
///
/// An open document's own URI wins, because that is the spelling the client
/// used and the only one that survives a non-`file:` scheme: [`crate::uri`]
/// keys an `untitled:` buffer under its URI *text*, which is not a path and must
/// never be handed back through `path_to_uri` — doing so would answer
/// `file:///untitled:Untitled-1`. Everything else is a file the import search
/// found through [`kg_vfs::Vfs`], whose key is a real absolute path.
pub(crate) fn uri_of(state: &GlobalState, file: FileId) -> Option<Uri> {
    if let Some(uri) = state.uri_of(file) {
        return Some(uri);
    }
    let path = state.vfs().path(file)?;
    path.is_absolute().then(|| crate::uri::path_to_uri(&path)).flatten()
}

pub fn document_symbol(
    state: &GlobalState,
    params: &DocumentSymbolParams,
) -> Answer<DocumentSymbolResponse> {
    let snap = state.snapshot(&params.text_document.uri)?;
    Ok(DocumentSymbolResponse::Nested(document_symbol::document_symbols(
        &snap.parse.syntax_node(),
        snap.line_index(),
        state.encoding(),
    )))
}

pub fn folding_range(state: &GlobalState, params: &FoldingRangeParams) -> Answer<Vec<FoldingRange>> {
    let snap = state.snapshot(&params.text_document.uri)?;
    Ok(folding_range::folding_ranges(
        &snap.parse.syntax_node(),
        snap.line_index(),
        state.encoding(),
    ))
}

pub fn selection_range(
    state: &GlobalState,
    params: &SelectionRangeParams,
) -> Answer<Vec<SelectionRange>> {
    let snap = state.snapshot(&params.text_document.uri)?;
    Ok(selection_range::selection_ranges(
        &snap.parse.syntax_node(),
        snap.line_index(),
        state.encoding(),
        &params.positions,
    ))
}

pub fn semantic_tokens_full(
    state: &GlobalState,
    params: &SemanticTokensParams,
) -> Answer<SemanticTokensResult> {
    let snap = state.snapshot(&params.text_document.uri)?;
    Ok(SemanticTokensResult::Tokens(tokens(state, &snap, None)))
}

pub fn semantic_tokens_range(
    state: &GlobalState,
    params: &SemanticTokensRangeParams,
) -> Answer<SemanticTokensRangeResult> {
    let snap = state.snapshot(&params.text_document.uri)?;
    let range = convert::text_range(snap.line_index(), state.encoding(), params.range);
    Ok(SemanticTokensRangeResult::Tokens(tokens(state, &snap, Some(range))))
}

fn tokens(
    state: &GlobalState,
    snap: &FileSnapshot,
    range: Option<kg_base::TextRange>,
) -> SemanticTokens {
    semantic_tokens::semantic_tokens(
        &snap.parse.syntax_node(),
        snap.line_index(),
        state.encoding(),
        range,
    )
}

pub fn formatting(state: &GlobalState, params: &DocumentFormattingParams) -> Answer<Vec<TextEdit>> {
    let snap = state.snapshot(&params.text_document.uri)?;
    Ok(fmt::format(
        &snap.text.text,
        snap.line_index(),
        state.encoding(),
        &snap.settings.fmt,
        None,
    ))
}

pub fn range_formatting(
    state: &GlobalState,
    params: &DocumentRangeFormattingParams,
) -> Answer<Vec<TextEdit>> {
    let snap = state.snapshot(&params.text_document.uri)?;
    let range = convert::text_range(snap.line_index(), state.encoding(), params.range);
    Ok(fmt::format(
        &snap.text.text,
        snap.line_index(),
        state.encoding(),
        &snap.settings.fmt,
        Some(range),
    ))
}

/// The diagnostics to publish for one document: the loop's last answer,
/// with its first handed to `first` on the way where there is one between
/// (`GlobalState::frontend_diagnostics`, sprint W7.6) — both for the text
/// of one snapshot, under its version.
///
/// Not a request handler — nobody asks for these — but the same shape, and the
/// same testability.
pub fn publish_diagnostics(
    state: &GlobalState,
    uri: &Uri,
    first: impl FnOnce(PublishDiagnosticsParams),
) -> Answer<PublishDiagnosticsParams> {
    let snap = state.snapshot(uri)?;
    // The frontend answer when there is one, the bare parse otherwise. The
    // fallback is not decoration: `frontend_diagnostics` returns `None` for a
    // file the server has no database for, and publishing an empty list there
    // would clear the gutter as though the file were clean.
    let reported = state
        .frontend_diagnostics(snap.file, |early| first(to_params(state, uri, &snap, early)))
        .unwrap_or_else(|| snap.parse.diagnostics(snap.file));
    Ok(to_params(state, uri, &snap, reported))
}

/// The syntax errors alone, for publishing ahead of the whole-program check —
/// or `None` when there are none.
///
/// Sound as an early answer because it is a subset of the late one: a file
/// with a parse error gets its load and parse-stage errors from
/// `Db::diagnostics` and nothing else (`Main.hs:73-75`), and the snapshot's
/// parse is the default-options parse, whose errors the database's own parse
/// includes. A clean parse publishes nothing: an empty list would clear the
/// semantic errors on screen and claim a clean file until the check lands.
pub fn parse_diagnostics(state: &GlobalState, uri: &Uri) -> Option<PublishDiagnosticsParams> {
    let snap = state.snapshot(uri).ok()?;
    let reported = snap.parse.diagnostics(snap.file);
    if !reported.iter().any(kg_diag::Diagnostic::is_error) {
        return None;
    }
    Some(to_params(state, uri, &snap, reported))
}

fn to_params(
    state: &GlobalState,
    uri: &Uri,
    snap: &FileSnapshot,
    reported: Vec<kg_diag::Diagnostic>,
) -> PublishDiagnosticsParams {
    // A `while compiling` note names the instantiation site, which may be
    // in another module of the program; any file the server can name and
    // read becomes a clickable location.
    let locate = |range: kg_base::FileRange| {
        let uri = uri_of(state, range.file)?;
        let text = state.vfs().read(range.file)?;
        Some(lsp_types::Location {
            uri,
            range: crate::convert::range(&text.line_index, state.encoding(), range.range),
        })
    };
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: diagnostics::diagnostics(
            reported,
            &snap.parse.syntax_node(),
            snap.file,
            snap.line_index(),
            state.encoding(),
            &locate,
        ),
        version: Some(snap.version),
    }
}

/// The empty result to return when a document is unknown or too large.
///
/// Clearing diagnostics matters: a file that grows past the size cap must not
/// keep its last known errors on screen forever.
pub fn cleared_diagnostics(uri: &Uri) -> PublishDiagnosticsParams {
    PublishDiagnosticsParams { uri: uri.clone(), diagnostics: Vec::new(), version: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_state::test_uri;
    use lsp_types::{
        FormattingOptions, PartialResultParams, Position, Range, TextDocumentIdentifier,
        WorkDoneProgressParams,
    };

    const SRC: &str = "\
//| The accumulator.
import base.util

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

    fn doc(uri: &Uri) -> TextDocumentIdentifier {
        TextDocumentIdentifier { uri: uri.clone() }
    }

    #[test]
    fn document_symbol_answers() {
        let (state, uri) = state();
        let out = document_symbol(
            &state,
            &DocumentSymbolParams {
                text_document: doc(&uri),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .unwrap();
        // Hierarchical, never flat.
        assert!(matches!(out, DocumentSymbolResponse::Nested(_)));
    }

    #[test]
    fn folding_range_answers() {
        let (state, uri) = state();
        let out = folding_range(
            &state,
            &FoldingRangeParams {
                text_document: doc(&uri),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .unwrap();
        // The class body and the function body both fold, whatever the parser
        // makes of the rest.
        assert!(out.len() >= 2, "{out:?}");
    }

    #[test]
    fn selection_range_answers() {
        let (state, uri) = state();
        let out = selection_range(
            &state,
            &SelectionRangeParams {
                text_document: doc(&uri),
                positions: vec![Position::new(4, 12)],
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].parent.is_some(), "expand-selection must have somewhere to go");
    }

    #[test]
    fn semantic_tokens_full_and_range_answer() {
        let (state, uri) = state();
        let full = semantic_tokens_full(
            &state,
            &SemanticTokensParams {
                text_document: doc(&uri),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .unwrap();
        let SemanticTokensResult::Tokens(full) = full else { panic!("expected full tokens") };
        assert!(!full.data.is_empty());

        let ranged = semantic_tokens_range(
            &state,
            &SemanticTokensRangeParams {
                text_document: doc(&uri),
                range: Range::new(Position::new(4, 0), Position::new(5, 0)),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .unwrap();
        let SemanticTokensRangeResult::Tokens(ranged) = ranged else {
            panic!("expected ranged tokens")
        };
        assert!(!ranged.data.is_empty());
        assert!(ranged.data.len() < full.data.len(), "a range must be a subset");
    }

    /// Formatting used to answer with no edits because no backend was linked
    /// in; `kg-fmt` is wired in now, so the contract this pins is the round
    /// trip — the handler reaches the real formatter, and applying what it
    /// returns leaves the buffer already formatted.
    #[test]
    fn formatting_round_trips_through_the_real_formatter() {
        let (state, uri) = state();
        let out = formatting(
            &state,
            &DocumentFormattingParams {
                text_document: doc(&uri),
                options: FormattingOptions::default(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        )
        .unwrap();
        assert!(fmt::available(), "the capability must be on to match");

        // `SRC` may already be formatted, in which case no edits is correct —
        // what must not happen is the handler answering empty because it never
        // reached a formatter. Formatting the text directly settles which it is.
        let want = kg_fmt::format_file(SRC, &kg_config::Fmt::default()).unwrap();
        if want == SRC {
            assert!(out.is_empty(), "already formatted, so nothing to send: {out:?}");
        } else {
            assert_eq!(out.len(), 1, "{out:?}");
            assert_eq!(out[0].new_text, want);
        }
    }

    #[test]
    fn diagnostics_are_published_with_the_document_version() {
        let (state, uri) = state();
        let out = publish_diagnostics(&state, &uri, |_| {}).unwrap();
        assert_eq!(out.version, Some(1));
        assert_eq!(out.uri, uri);
    }

    /// The server publishes what the *frontend* says, not what the parser says.
    ///
    /// This file parses cleanly and is still wrong: `tNot` has no clause for an
    /// integer (`Type.hs:571`), which only pass 10 knows. Before the query layer
    /// was wired the gutter stayed empty here — and stayed empty for unresolved
    /// imports and type errors too, which is most of what a user wants an editor
    /// to catch.
    #[test]
    fn diagnostics_come_from_the_frontend_not_just_the_parser() {
        let state = GlobalState::new();
        let uri = test_uri("semantic.k");
        let src = "inline void main(uint8 a)\n{\n    bool b = !a;\n}\n";
        state.did_open(uri.clone(), src.to_owned(), 1);

        assert!(
            kg_syntax::parse_file(src).diagnostics(kg_base::FileId(0)).is_empty(),
            "the fixture must parse cleanly, or it proves nothing"
        );

        let out = publish_diagnostics(&state, &uri, |_| {}).unwrap();
        assert_eq!(out.diagnostics.len(), 1, "{:?}", out.diagnostics);
        assert!(
            out.diagnostics[0].message.contains("boolean operand"),
            "{:?}",
            out.diagnostics[0]
        );
    }

    /// An edit must not be answered from the memo of the previous text.
    #[test]
    fn an_edit_invalidates_the_frontend_answer() {
        let state = GlobalState::new();
        let uri = test_uri("edited.k");
        state.did_open(uri.clone(), "inline void main(uint8 a)\n{\n    bool b = !a;\n}\n".to_owned(), 1);
        assert_eq!(publish_diagnostics(&state, &uri, |_| {}).unwrap().diagnostics.len(), 1);

        state.did_change_full(&uri, "inline void main(bool a)\n{\n    bool b = !a;\n}\n".to_owned(), 2);
        let out = publish_diagnostics(&state, &uri, |_| {}).unwrap();
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert_eq!(out.version, Some(2));
    }

    /// The early tier speaks only when the parser has something to say, and
    /// then says exactly what the full check will also say.
    #[test]
    fn parse_diagnostics_are_published_early_only_when_there_are_some() {
        let state = GlobalState::new();
        let uri = test_uri("typing.k");
        let clean = "inline void main(uint8 a)\n{\n    bool b = !a;\n}\n";
        state.did_open(uri.clone(), clean.to_owned(), 1);
        assert!(
            parse_diagnostics(&state, &uri).is_none(),
            "an empty early list would hide the frontend's error until the check lands"
        );

        state.did_change_full(&uri, format!("{clean}void broken(\n"), 2);
        let early = parse_diagnostics(&state, &uri).expect("a syntax error to report");
        let late = publish_diagnostics(&state, &uri, |_| {}).unwrap();
        assert_eq!(early.version, Some(2));
        assert!(!early.diagnostics.is_empty());
        for d in &early.diagnostics {
            assert!(late.diagnostics.contains(d), "{d:?} is not in the full answer {:?}", late.diagnostics);
        }
    }

    /// The loop's first answer is published on the way to its last, for
    /// the same version: `Down<40>` reaches `Down<0>`'s zero-width array at
    /// round 41, past the editor's first bound of four, so the first answer
    /// says nothing — a truncated loop invents no error — and the last says
    /// what `kg check` says (sprint W7.6). Red with the first answer taken
    /// at the fixpoint (nothing between to publish), or with the last
    /// answer read off the first.
    #[test]
    fn the_first_answer_is_published_on_the_way_to_the_last() {
        let state = GlobalState::new();
        let uri = test_uri("down.k");
        state.did_open(uri.clone(), down(40, 50), 1);
        let mut first = None;
        let last = publish_diagnostics(&state, &uri, |p| first = Some(p)).unwrap();
        let first = first.expect("a first answer: the loop runs past four rounds");
        assert_eq!((first.version, last.version), (Some(1), Some(1)));
        assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
        let messages: Vec<&str> = last.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages, ["Array dimension must be a positive integer"]);
    }

    /// `Down<n>` calls `Down<n - 1>` down to `Down<0>`, whose `bool[0]` is
    /// the one error, `n + 1` rounds in; each copy carries `body`
    /// statements, so the run past four rounds is not over before the
    /// first answer is read.
    fn down(n: usize, body: usize) -> String {
        let body: String = (0..body).map(|i| format!("    const auto v{i} = cast<uint32>(a + {i}) * b;\n")).collect();
        format!(
            "template <auto N>\ninline uint32 Down(uint32 a, uint32 b)\n{{\n    bool[N] x;\n{body}    static if (N > 0)\n    {{\n        return Down<N - 1>(a, b);\n    }}\n    else\n    {{\n        return a;\n    }}\n}}\n\ninline uint32 main(uint32 a)\n{{\n    return Down<{n}>(a, 3);\n}}\n"
        )
    }

    #[test]
    fn an_unknown_document_is_declined_not_faked() {
        let state = GlobalState::new();
        let uri = test_uri("never-opened.k");
        let err = publish_diagnostics(&state, &uri, |_| {}).unwrap_err();
        assert_eq!(err, NoFile::NotOpen);
        assert!(cleared_diagnostics(&uri).diagnostics.is_empty());
    }
}
