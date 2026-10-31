//! The server, end to end, over an in-memory connection.
//!
//! Drives the *real* [`kg_lsp::run_server_on`] loop — the same code path stdio
//! uses — through whole sessions: initialize, didOpen, requests, shutdown,
//! exit. Unit tests prove each handler computes the right answer; this proves
//! the wiring exists at all, that capabilities are advertised, that diagnostics
//! arrive unprompted, and that shutdown produces exit code 0.
//!
//! Every request the server advertises is driven from here, and that is a rule
//! rather than a coincidence: a handler nothing dispatches is not shipped, it is
//! dead code with passing tests.
//!
//! No processes, no pipes, no sleeping: `Connection::memory()` is two channels,
//! so the test is deterministic and takes microseconds.

use std::str::FromStr;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::Notification as _;
use lsp_types::{
    ClientCapabilities, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    DocumentSymbolParams, DocumentSymbolResponse, FoldingRange, FoldingRangeParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams, InlayHint,
    InlayHintLabel, InlayHintParams, InitializeParams, InitializedParams, Location,
    PartialResultParams, Position, PrepareRenameResponse, PublishDiagnosticsParams, Range,
    ReferenceContext, ReferenceParams, RenameParams, SemanticTokens, SemanticTokensParams,
    ServerCapabilities, SignatureHelp, SignatureHelpParams, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, VersionedTextDocumentIdentifier, WorkDoneProgressParams,
    WorkspaceEdit, notification as notif, request as req,
};

const SRC: &str = "\
//| A tiny accumulator, with a non-ASCII comment: héllo 🦀
import base.util

class Adder {
    uint32 total;
    inline void add(uint32 x) {
        total = total + x;
    }
}
";

struct Client {
    conn: Connection,
    next_id: i32,
    server: Option<std::thread::JoinHandle<kg_lsp::ExitCode>>,
}

impl Client {
    fn start() -> Client {
        let (client, server) = Connection::memory();
        let handle = std::thread::Builder::new()
            .name("kg-lsp-under-test".to_owned())
            .spawn(move || kg_lsp::run_server_on(server).expect("the server loop"))
            .expect("spawning the server");
        Client { conn: client, next_id: 0, server: Some(handle) }
    }

    fn request<R: lsp_types::request::Request>(&mut self, params: R::Params) -> RequestId {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.conn
            .sender
            .send(Message::Request(Request::new(id.clone(), R::METHOD.to_owned(), params)))
            .unwrap();
        id
    }

    fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        self.conn
            .sender
            .send(Message::Notification(Notification::new(N::METHOD.to_owned(), params)))
            .unwrap();
    }

    /// Reads until the response to `id` arrives, collecting notifications on
    /// the way — which is the whole point of the protocol being two streams.
    fn response(&mut self, id: &RequestId, seen: &mut Vec<Notification>) -> Response {
        loop {
            match self.recv() {
                Message::Response(r) if r.id == *id => return r,
                Message::Notification(n) => seen.push(n),
                other => panic!("unexpected message while awaiting {id}: {other:?}"),
            }
        }
    }

    /// Reads until diagnostics for `uri` arrive, keeping every other
    /// notification in `seen`.
    fn diagnostics_for(&self, uri: &Uri, seen: &mut Vec<Notification>) -> PublishDiagnosticsParams {
        loop {
            match self.recv() {
                Message::Notification(n) if n.method == notif::PublishDiagnostics::METHOD => {
                    let params: PublishDiagnosticsParams =
                        serde_json::from_value(n.params.clone()).unwrap();
                    seen.push(n);
                    if params.uri == *uri {
                        return params;
                    }
                }
                Message::Notification(n) => seen.push(n),
                other => panic!("unexpected message while awaiting diagnostics: {other:?}"),
            }
        }
    }

    fn recv(&self) -> Message {
        // A liveness backstop, not an assertion. Nothing here is *timed*: the
        // test asserts what the server answers, never how fast, and this exists
        // only so a server that deadlocks fails the suite instead of hanging it
        // forever. So the budget is set for the worst machine, not the expected
        // one — it was 10s, which is ~10^6 times an in-memory round trip and
        // still lost roughly one full-workspace run in five when `cargo test`
        // saturated every core alongside another build. A flaky gate is worse
        // than a slow one: it teaches you to re-run instead of to look.
        self.conn
            .receiver
            .recv_timeout(std::time::Duration::from_secs(120))
            .expect("the server went quiet")
    }

    /// Joins the server thread, then drains everything it sent that nobody was
    /// listening for.
    ///
    /// `response` collects notifications only *incidentally* — whatever happens
    /// to arrive while it is blocked awaiting a particular id. Diagnostics are
    /// published on the server's own schedule, so whether they landed inside one
    /// of those windows is a race, and `a_whole_session_initialize_through_exit`
    /// lost it about one full-workspace run in five. Once the thread is joined
    /// the channel holds everything the server will ever send, so draining here
    /// is the difference between asserting "the server published diagnostics"
    /// and asserting "the test happened to be listening when it did".
    fn finish(mut self, seen: &mut Vec<Notification>) -> kg_lsp::ExitCode {
        let code =
            self.server.take().expect("started once").join().expect("the server thread panicked");
        while let Ok(message) = self.conn.receiver.try_recv() {
            if let Message::Notification(n) = message {
                seen.push(n);
            }
        }
        code
    }

    /// [`Client::finish`], keeping responses as well as notifications.
    fn finish_all(mut self) -> (kg_lsp::ExitCode, Vec<Message>) {
        let code =
            self.server.take().expect("started once").join().expect("the server thread panicked");
        (code, self.conn.receiver.try_iter().collect())
    }
}

fn result<T: serde::de::DeserializeOwned>(response: Response) -> T {
    let value = response.response_result.expect("a successful response");
    serde_json::from_value(value).expect("a well-typed result")
}

fn is_error(response: &Response) -> bool {
    response.response_result.is_err()
}

fn ok_value(response: &Response) -> Option<&serde_json::Value> {
    response.response_result.as_ref().ok()
}

fn uri() -> Uri {
    Uri::from_str("file:///workspace/adder.k").unwrap()
}

#[test]
fn a_whole_session_initialize_through_exit() {
    let mut client = Client::start();
    let mut notifications: Vec<Notification> = Vec::new();

    // ---------------------------------------------------------- initialize
    let id = client.request::<req::Initialize>(InitializeParams {
        capabilities: ClientCapabilities::default(),
        ..InitializeParams::default()
    });
    let init: serde_json::Value = result(client.response(&id, &mut notifications));
    let caps: ServerCapabilities =
        serde_json::from_value(init["capabilities"].clone()).expect("capabilities");

    assert_eq!(init["serverInfo"]["name"], "kg-lsp");
    // Advertised exactly because they are implemented.
    assert!(caps.document_symbol_provider.is_some());
    assert!(caps.folding_range_provider.is_some());
    assert!(caps.semantic_tokens_provider.is_some());
    assert!(caps.selection_range_provider.is_some());
    assert!(caps.document_highlight_provider.is_some());
    assert!(caps.definition_provider.is_some());
    assert!(caps.hover_provider.is_some());
    assert!(caps.references_provider.is_some());
    assert!(caps.rename_provider.is_some());
    assert!(caps.signature_help_provider.is_some());
    assert!(caps.inlay_hint_provider.is_some());
    // Completion joined on 2026-09-06, with `.` as its trigger — advertised
    // the day `handlers::completion` answered it, not before — and `:` on
    // 2026-10-11, the day it answered a path's `::`.
    let completion = caps.completion_provider.as_ref().expect("completion is answered");
    assert_eq!(completion.trigger_characters.as_deref(), Some(&[".".to_owned(), ":".to_owned()][..]));
    // The client offered nothing, so the mandatory default was chosen.
    assert_eq!(caps.position_encoding, Some(lsp_types::PositionEncodingKind::UTF16));

    client.notify::<notif::Initialized>(InitializedParams {});

    // ------------------------------------------------------------- didOpen
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: SRC.to_owned(),
        },
    });

    // ------------------------------------------------------ documentSymbol
    let id = client.request::<req::DocumentSymbolRequest>(DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri: uri() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let symbols: Option<DocumentSymbolResponse> = result(client.response(&id, &mut notifications));
    // Hierarchical, never the flat `SymbolInformation` form.
    match &symbols {
        // An outline the parser could not build is empty, never garbage.
        None => {}
        Some(DocumentSymbolResponse::Nested(syms)) if syms.is_empty() => {}
        Some(DocumentSymbolResponse::Nested(syms)) => {
            let class = syms.iter().find(|s| s.name == "Adder").expect("the class in the outline");
            assert_eq!(class.kind, lsp_types::SymbolKind::CLASS);
            // `selectionRange ⊆ range` is a protocol requirement, and it is
            // the name that must be selected.
            assert!(class.selection_range.start.character > class.range.start.character);
            let members = class.children.as_ref().expect("class members");
            assert!(members.iter().any(|m| m.name == "total"), "{members:?}");
            assert!(members.iter().any(|m| m.name == "add"), "{members:?}");
        }
        Some(other) => panic!("document symbols must be hierarchical: {other:?}"),
    }

    // ------------------------------------------------ semanticTokens/full
    let id = client.request::<req::SemanticTokensFullRequest>(SemanticTokensParams {
        text_document: TextDocumentIdentifier { uri: uri() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let tokens: Option<SemanticTokens> = result(client.response(&id, &mut notifications));
    let tokens = tokens.expect("semantic tokens");
    assert!(!tokens.data.is_empty(), "an open document must produce tokens");
    // No token may span a line break, and the deltas must be well formed.
    for token in &tokens.data {
        assert!(token.length > 0);
    }

    // ---------------------------------------------------------- foldingRange
    let id = client.request::<req::FoldingRangeRequest>(FoldingRangeParams {
        text_document: TextDocumentIdentifier { uri: uri() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let folds: Option<Vec<FoldingRange>> = result(client.response(&id, &mut notifications));
    let folds = folds.expect("folding ranges");
    assert!(folds.len() >= 2, "the class body and the function body both fold: {folds:?}");

    // --------------------------------------------------------------- exit
    let id = client.request::<req::Shutdown>(());
    let response = client.response(&id, &mut notifications);
    assert!(!is_error(&response));
    client.notify::<notif::Exit>(());

    assert_eq!(client.finish(&mut notifications), kg_lsp::ExitCode::Ok);

    // Diagnostics were published without anyone asking for them, which is the
    // whole reason this server is built on a message loop.
    let published: Vec<PublishDiagnosticsParams> = notifications
        .iter()
        .filter(|n| n.method == notif::PublishDiagnostics::METHOD)
        .map(|n| serde_json::from_value(n.params.clone()).unwrap())
        .collect();
    assert!(!published.is_empty(), "no diagnostics were published");
    assert_eq!(published[0].uri, uri());
    assert_eq!(published[0].version, Some(1));
}

/// A class field, a use of it, and one `auto` — enough for all four semantic
/// requests to have a right answer that is *not* the empty one.
const SEMANTIC_SRC: &str = "\
class Adder {
    uint32 total;
    inline void add(uint32 x) {
        total = total + x;
        auto width = 10u8;
    }
}
";

/// Every semantic request, driven the way a client drives it.
///
/// The unit tests in `handlers::*` prove each of these computes the right
/// answer; not one of them proves the *server* will ever call it. That gap has
/// been shipped before in this project: a handler with green tests, no `mod`
/// line, no dispatch arm, and an editor where the feature silently does nothing.
/// So the assertions here are deliberately shallow — one fact per request — and
/// what they actually pin is the chain `capabilities` → `dispatch_request` →
/// handler → wire.
#[test]
fn every_semantic_request_is_reachable_from_a_real_client() {
    let mut client = Client::start();
    let mut seen = Vec::new();

    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: SEMANTIC_SRC.to_owned(),
        },
    });

    // The caret is on the second `total` of `total = total + x` — a *use*, two
    // scopes below the field it names.
    let at = |position| TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri() },
        position,
    };
    let use_site = Position::new(3, 16);
    // `    uint32 total;` — the field's name token, which is what
    // `Item::range` stores (`symbolLoc`, `SymbolTable.hs:199`).
    let declaration = Range::new(Position::new(1, 11), Position::new(1, 16));

    // ------------------------------------------------------ goto-definition
    let id = client.request::<req::GotoDefinition>(GotoDefinitionParams {
        text_document_position_params: at(use_site),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let jump: Option<GotoDefinitionResponse> = result(client.response(&id, &mut seen));
    match jump.expect("the use must resolve to the field") {
        // `Scalar`, never `Link`: the server does not retain client
        // capabilities, so it cannot know `linkSupport` was advertised.
        GotoDefinitionResponse::Scalar(found) => {
            assert_eq!(found.uri, uri());
            assert_eq!(found.range, declaration);
        }
        other => panic!("expected one bare Location, got {other:?}"),
    }

    // ------------------------------------------------------------- hover
    let id = client.request::<req::HoverRequest>(HoverParams {
        text_document_position_params: at(use_site),
        work_done_progress_params: WorkDoneProgressParams::default(),
    });
    let hover: Option<Hover> = result(client.response(&id, &mut seen));
    let hover = hover.expect("the use must have something to describe");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("the body must be markdown, not a legacy string form")
    };
    assert!(markup.value.contains("**class field** `Adder::total`"), "{}", markup.value);
    assert!(markup.value.contains("uint32 total"), "{}", markup.value);

    // -------------------------------------------------------- references
    let id = client.request::<req::References>(ReferenceParams {
        text_document_position: at(use_site),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext { include_declaration: true },
    });
    let found: Option<Vec<Location>> = result(client.response(&id, &mut seen));
    let found = found.expect("references");
    let mut positions: Vec<Range> = found.iter().map(|l| l.range).collect();
    positions.sort_by_key(|r| (r.start.line, r.start.character));
    assert_eq!(
        positions,
        vec![
            declaration,
            Range::new(Position::new(3, 8), Position::new(3, 13)),
            Range::new(Position::new(3, 16), Position::new(3, 21)),
        ],
    );
    assert!(found.iter().all(|l| l.uri == uri()));

    // ------------------------------------------------------------ rename
    // `prepareRename` first, as a client does: the token under the caret and
    // its current spelling, for the inline box.
    let id = client.request::<req::PrepareRenameRequest>(at(use_site));
    let prepared: Option<PrepareRenameResponse> = result(client.response(&id, &mut seen));
    assert_eq!(
        prepared,
        Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: Range::new(Position::new(3, 16), Position::new(3, 21)),
            placeholder: "total".to_owned(),
        })
    );
    let rename_to = |new_name: &str| RenameParams {
        text_document_position: at(use_site),
        new_name: new_name.to_owned(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let id = client.request::<req::Rename>(rename_to("sum"));
    let edit: Option<WorkspaceEdit> = result(client.response(&id, &mut seen));
    #[allow(clippy::mutable_key_type)]
    let changes = edit.expect("an edit").changes.expect("changes keyed by document");
    let edits = &changes[&uri()];
    let mut ranges: Vec<Range> = edits.iter().map(|e| e.range).collect();
    ranges.sort_by_key(|r| (r.start.line, r.start.character));
    // The same three positions references reported, each replaced.
    assert_eq!(ranges, positions);
    assert!(edits.iter().all(|e| e.new_text == "sum"));
    // A refused rename is a request *error* carrying the reason — the client
    // shows the sentence — never a `null` that silently does nothing.
    let id = client.request::<req::Rename>(rename_to("class"));
    let refused = client.response(&id, &mut seen);
    assert!(is_error(&refused));
    match &refused.response_result {
        Err(e) => assert!(e.message.contains("not an identifier"), "{}", e.message),
        Ok(_) => unreachable!(),
    }

    // ---------------------------------------------------- signature help
    // A second document, with a call in it: `add(1, 2)` against a two-parameter
    // declaration, caret on the second argument.
    let sig_uri = Uri::from_str("file:///workspace/sig.k").unwrap();
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: sig_uri.clone(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: "inline uint32 add(uint32 x, uint32 y)\n{\n    return x + y;\n}\n\ninline void main()\n{\n    uint32 z = add(1, 2);\n}\n".to_owned(),
        },
    });
    let id = client.request::<req::SignatureHelpRequest>(SignatureHelpParams {
        context: None,
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: sig_uri.clone() },
            position: Position::new(7, 22),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    });
    let help: Option<SignatureHelp> = result(client.response(&id, &mut seen));
    let help = help.expect("a signature");
    assert_eq!(help.signatures.len(), 1);
    assert_eq!(help.signatures[0].label, "add(uint32 x, uint32 y) -> uint32");
    assert_eq!(help.active_parameter, Some(1));

    // ------------------------------------------------------- inlay hints
    let id = client.request::<req::InlayHintRequest>(InlayHintParams {
        work_done_progress_params: WorkDoneProgressParams::default(),
        text_document: TextDocumentIdentifier { uri: uri() },
        range: Range::new(Position::new(0, 0), Position::new(6, 0)),
    });
    let hints: Option<Vec<InlayHint>> = result(client.response(&id, &mut seen));
    let hints = hints.expect("inlay hints");
    assert_eq!(hints.len(), 1, "{hints:?}");
    // `        auto width = 10u8;` — the hint sits at the end of the name, so
    // the line reads back as `auto width: uint8 = 10u8;`.
    assert_eq!(hints[0].position, Position::new(4, 18));
    let InlayHintLabel::String(label) = &hints[0].label else { panic!("plain labels only") };
    assert_eq!(label, ": uint8");

    // ------------------------------------------------- document highlight
    let id = client.request::<req::DocumentHighlightRequest>(
        lsp_types::DocumentHighlightParams {
            text_document_position_params: at(use_site),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
    );
    let highlights: Option<Vec<lsp_types::DocumentHighlight>> =
        result(client.response(&id, &mut seen));
    // The declaration and its two uses — the same three the resolver answers
    // find-references with, restricted to this file.
    assert_eq!(highlights.expect("highlights").len(), 3);

    // --------------------------------------------------------- completion
    let id = client.request::<req::Completion>(lsp_types::CompletionParams {
        text_document_position: at(use_site),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: None,
    });
    let offered: Option<lsp_types::CompletionResponse> = result(client.response(&id, &mut seen));
    let Some(lsp_types::CompletionResponse::Array(offered)) = offered else {
        panic!("completion must answer with an array");
    };
    let labels: Vec<&str> = offered.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"total"), "{labels:?}");
    assert!(labels.contains(&"width"), "{labels:?}");

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Ok);
}

/// A semantic request for a document nobody opened is `null`, not an error and
/// not a fabricated empty answer — and it must not make the server load a
/// program for a URI it has never seen.
#[test]
fn goto_definition_on_an_unopened_document_answers_null() {
    let mut client = Client::start();
    let mut seen = Vec::new();

    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    let id = client.request::<req::GotoDefinition>(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: Uri::from_str("file:///workspace/never-opened.k").unwrap(),
            },
            position: Position::new(0, 0),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let response = client.response(&id, &mut seen);
    assert!(!is_error(&response), "an unknown document is not a protocol error");
    assert_eq!(ok_value(&response), Some(&serde_json::Value::Null));

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Ok);
}

#[test]
fn an_incremental_edit_reaches_the_next_request() {
    let mut client = Client::start();
    let mut seen = Vec::new();

    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            // A non-ASCII first line, so the edit below can only land in the
            // right place if UTF-16 columns are converted.
            text: "// héllo 🦀\nclass A {}\n".to_owned(),
        },
    });

    // Replace `A` with `Bee`. In UTF-16 the identifier is at line 1, column 6.
    client.notify::<notif::DidChangeTextDocument>(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier { uri: uri(), version: 2 },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(1, 6), Position::new(1, 7))),
            range_length: None,
            text: "Bee".to_owned(),
        }],
    });

    let id = client.request::<req::DocumentHighlightRequest>(lsp_types::DocumentHighlightParams {
        text_document_position_params: lsp_types::TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri() },
            position: Position::new(1, 7),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let highlights: Option<Vec<lsp_types::DocumentHighlight>> =
        result(client.response(&id, &mut seen));
    let highlights = highlights.expect("highlights");
    assert_eq!(highlights.len(), 1);
    assert_eq!(highlights[0].range, Range::new(Position::new(1, 6), Position::new(1, 9)));

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Ok);
}

#[test]
fn exiting_without_shutting_down_is_an_abnormal_exit() {
    let mut client = Client::start();
    let mut seen = Vec::new();

    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Abnormal);
}

#[test]
fn a_request_after_shutdown_is_refused() {
    let mut client = Client::start();
    let mut seen = Vec::new();

    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);

    let id = client.request::<req::DocumentSymbolRequest>(DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri: uri() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let response = client.response(&id, &mut seen);
    assert!(is_error(&response), "a request after shutdown must be an error");

    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Ok);
}

#[test]
fn a_request_for_an_unopened_document_answers_null() {
    let mut client = Client::start();
    let mut seen = Vec::new();

    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    let id = client.request::<req::FoldingRangeRequest>(FoldingRangeParams {
        text_document: TextDocumentIdentifier {
            uri: Uri::from_str("file:///workspace/never-opened.k").unwrap(),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let response = client.response(&id, &mut seen);
    assert!(!is_error(&response), "an unknown document is not a protocol error");
    assert_eq!(ok_value(&response), Some(&serde_json::Value::Null));

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Ok);
}

/// A class template whose body fails on every instance, and one instance.
/// The error belongs to the body (`Template.hs:127`, `Error.hs:182`); the
/// instantiation site reaches the editor as related information — the
/// `while compiling` note the reference prints (`Error.hs:192-194`) — so the
/// user can click from the body to the `Holder<uint32>` that compiled it.
const TWIN_SRC: &str = "\
template <typename T>
class Holder
{
public:
    T Get()
    {
        uint4 v = 0;
        bool b = v;
        return v;
    }
}

class C
{
    Holder<uint32> h;
public:
    uint32 Run()
    {
        return h.Get();
    }
}
";

#[test]
fn a_template_body_error_carries_its_site_as_related_information() {
    let mut client = Client::start();
    let mut notifications: Vec<Notification> = Vec::new();
    let twin_uri = Uri::from_str("file:///workspace/twin.k").unwrap();

    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut notifications));
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: twin_uri.clone(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: TWIN_SRC.to_owned(),
        },
    });

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut notifications);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut notifications), kg_lsp::ExitCode::Ok);

    let published: Vec<PublishDiagnosticsParams> = notifications
        .iter()
        .filter(|n| n.method == notif::PublishDiagnostics::METHOD)
        .map(|n| serde_json::from_value(n.params.clone()).unwrap())
        .filter(|p: &PublishDiagnosticsParams| p.uri == twin_uri)
        .collect();
    let last = published.last().expect("diagnostics for twin.k");
    assert_eq!(last.diagnostics.len(), 1, "{:?}", last.diagnostics);
    let d = &last.diagnostics[0];
    assert_eq!(d.message, "Invalid conversion from `uint4` to `bool`");
    assert_eq!(d.range.start, Position::new(7, 17), "`v` of `bool b = v;`");
    let related = d.related_information.as_ref().expect("the site as related information");
    assert_eq!(related.len(), 1);
    // The frame names the instance it compiles (`kg_sem::instance`); the
    // legacy output keeps the reference's bare note.
    assert_eq!(related[0].message, "while compiling `Holder<uint32>`");
    assert_eq!(related[0].location.uri, twin_uri);
    assert_eq!(related[0].location.range.start, Position::new(14, 4), "`Holder<uint32>`");
}

fn errors(published: &PublishDiagnosticsParams) -> Vec<&str> {
    published
        .diagnostics
        .iter()
        .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
        .map(|d| d.message.as_str())
        .collect()
}

/// AUDIT D2, the way a user reaches it: edit `import-dirs` in `kanagawa.toml`,
/// save, and the open file's diagnostics follow — without a restart. The
/// client's file watcher is what tells the server; the database that answers
/// has to be one built from the new manifest.
#[test]
fn a_manifest_edit_reaches_the_open_files_diagnostics() {
    let dir = std::env::temp_dir().join(format!("kg-lsp-e2e-manifest-{}", std::process::id()));
    let root = camino::Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
    std::fs::create_dir_all(root.join("one")).unwrap();
    std::fs::write(root.join("one/helper.k"), "module helper { }\n").unwrap();
    let manifest = root.join("kanagawa.toml");
    // No base library in this fixture, so the implicit base is `.cmdargs.k`.
    let write_manifest = |dirs: &str| {
        let text = format!("[project]\nimport-dirs = [{dirs}]\nimplicit-base = false\n");
        std::fs::write(&manifest, text).unwrap();
    };
    write_manifest("\"one\"");

    let mut client = Client::start();
    let mut seen = Vec::new();
    let folder = lsp_types::WorkspaceFolder {
        uri: kg_lsp::uri::path_to_uri(&root).unwrap(),
        name: "manifest".to_owned(),
    };
    let id = client.request::<req::Initialize>(InitializeParams {
        workspace_folders: Some(vec![folder]),
        ..InitializeParams::default()
    });
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    let main = kg_lsp::uri::path_to_uri(&root.join("main.k")).unwrap();
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: main.clone(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: "import helper\n".to_owned(),
        },
    });
    let before = client.diagnostics_for(&main, &mut seen);
    assert!(errors(&before).is_empty(), "`helper` is under `one/`: {:?}", errors(&before));

    write_manifest("\"two\"");
    client.notify::<notif::DidChangeWatchedFiles>(lsp_types::DidChangeWatchedFilesParams {
        changes: vec![lsp_types::FileEvent {
            uri: kg_lsp::uri::path_to_uri(&manifest).unwrap(),
            typ: lsp_types::FileChangeType::CHANGED,
        }],
    });
    let after = client.diagnostics_for(&main, &mut seen);
    assert!(
        !errors(&after).is_empty(),
        "`one/` left the search path and `import helper` still resolved"
    );

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Ok);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A manifest the server refuses is shown the user, not only logged: a
/// diagnostic on `kanagawa.toml` at the value refused, and a message once —
/// on the reload that broke it, not the one after. Fixed, its mark goes.
/// A refused `edition` leaves the files in 1.0, where a 1.1 program's
/// errors would otherwise be all the user saw.
#[test]
fn a_refused_manifest_is_marked_where_it_is_wrong() {
    let dir = std::env::temp_dir().join(format!("kg-lsp-e2e-refused-{}", std::process::id()));
    let root = camino::Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let manifest = root.join("kanagawa.toml");
    let write_manifest = |edition: &str| {
        let text = format!("[package]\nedition = \"{edition}\"\n\n[project]\nimplicit-base = false\n");
        std::fs::write(&manifest, text).unwrap();
    };
    let manifest_uri = kg_lsp::uri::path_to_uri(&manifest).unwrap();
    let changed = |client: &Client| {
        client.notify::<notif::DidChangeWatchedFiles>(lsp_types::DidChangeWatchedFilesParams {
            changes: vec![lsp_types::FileEvent { uri: manifest_uri.clone(), typ: lsp_types::FileChangeType::CHANGED }],
        });
    };
    let said = |seen: &[Notification]| seen.iter().filter(|n| n.method == notif::ShowMessage::METHOD).count();
    write_manifest("1.1");

    let mut client = Client::start();
    let mut seen = Vec::new();
    let folder = lsp_types::WorkspaceFolder { uri: kg_lsp::uri::path_to_uri(&root).unwrap(), name: "refused".to_owned() };
    let id = client.request::<req::Initialize>(InitializeParams {
        workspace_folders: Some(vec![folder]),
        ..InitializeParams::default()
    });
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    let main = kg_lsp::uri::path_to_uri(&root.join("main.k")).unwrap();
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: main.clone(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: "class A {}\n".to_owned(),
        },
    });
    let _ = client.diagnostics_for(&main, &mut seen);
    assert_eq!(said(&seen), 0, "a manifest that loads says nothing");

    write_manifest("9.9");
    changed(&client);
    let marked = client.diagnostics_for(&manifest_uri, &mut seen);
    assert_eq!(errors(&marked), ["unknown edition `9.9`: this kg knows 1.0, 1.1"]);
    assert_eq!(marked.diagnostics[0].range, Range::new(Position::new(1, 10), Position::new(1, 15)), "at `\"9.9\"`");
    // The message follows the mark from the main loop, so it is the next
    // of the main loop's own; a file's diagnostics, from a worker, may come
    // between.
    let message = loop {
        match client.recv() {
            Message::Notification(n) if n.method == notif::ShowMessage::METHOD => break n,
            Message::Notification(n) => seen.push(n),
            other => panic!("unexpected message while awaiting the message: {other:?}"),
        }
    };
    let message: lsp_types::ShowMessageParams = serde_json::from_value(message.params).unwrap();
    assert!(message.message.contains("unknown edition `9.9`") && message.message.contains("in edition 1.0"), "{}", message.message);

    // Read again and still broken the same way: marked, not said again. The
    // fix's reload is behind it on the main loop, so once its mark is taken
    // off, anything this reload said has arrived.
    changed(&client);
    // A request is dispatched after every notification before it, so its
    // answer means the reload has read the manifest still broken.
    let id = client.request::<req::DocumentSymbolRequest>(DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri: main.clone() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let _ = client.response(&id, &mut seen);
    write_manifest("1.0");
    changed(&client);
    let cleared = client.diagnostics_for(&manifest_uri, &mut seen);
    assert!(cleared.diagnostics.is_empty(), "fixed, its mark goes: {:?}", errors(&cleared));
    assert_eq!(said(&seen), 0, "said once, not on every reload");

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Ok);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A source changed on disk under an open file — a branch switched, a pull
/// — reaches its diagnostics without a restart: `helper.k`, never opened,
/// stops exposing `twice`, the client's watcher says so, and `main.k`'s
/// call is reported. Red with the event ignored (the stale disk read
/// answers) or the open files not republished.
#[test]
fn a_source_changed_on_disk_reaches_the_open_files_diagnostics() {
    let dir = std::env::temp_dir().join(format!("kg-lsp-e2e-disk-{}", std::process::id()));
    let root = camino::Utf8PathBuf::from_path_buf(dir.clone()).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("kanagawa.toml"), "[project]\nimport-dirs = [\"src\"]\nimplicit-base = false\n").unwrap();
    let helper = root.join("src/helper.k");
    std::fs::write(&helper, "module helper\n    { twice\n    }\n\ninline uint32 twice(uint32 x)\n{\n    return x + x;\n}\n").unwrap();

    let mut client = Client::start();
    let mut seen = Vec::new();
    let folder = lsp_types::WorkspaceFolder { uri: kg_lsp::uri::path_to_uri(&root).unwrap(), name: "disk".to_owned() };
    let id = client.request::<req::Initialize>(InitializeParams { workspace_folders: Some(vec![folder]), ..InitializeParams::default() });
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});
    let main = kg_lsp::uri::path_to_uri(&root.join("src/main.k")).unwrap();
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: main.clone(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: "import helper\n\ninline void main()\n{\n    uint32 y = twice(1);\n}\n".to_owned(),
        },
    });
    let before = client.diagnostics_for(&main, &mut seen);
    assert!(errors(&before).is_empty(), "{:?}", errors(&before));

    std::fs::write(&helper, "module helper\n    { thrice\n    }\n\ninline uint32 thrice(uint32 x)\n{\n    return x + x + x;\n}\n").unwrap();
    client.notify::<notif::DidChangeWatchedFiles>(lsp_types::DidChangeWatchedFilesParams {
        changes: vec![lsp_types::FileEvent { uri: kg_lsp::uri::path_to_uri(&helper).unwrap(), typ: lsp_types::FileChangeType::CHANGED }],
    });
    let after = client.diagnostics_for(&main, &mut seen);
    assert!(!errors(&after).is_empty(), "`twice` left `helper.k` and the call still resolved");

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut Vec::new()), kg_lsp::ExitCode::Ok);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Typing faster than a check runs: thirty edits in a burst. Whatever the
/// server skips along the way, what reaches the client is in version order —
/// never an older version's errors over a newer one's — and ends at the text
/// the user is looking at.
///
/// How many of the thirty get a check of their own depends on the machine, so
/// that is not asserted; `main_loop`'s unit tests pin the coalescing itself.
#[test]
fn a_burst_of_edits_publishes_in_version_order_and_ends_at_the_last() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});

    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: SEMANTIC_SRC.to_owned(),
        },
    });
    let last = 31;
    for version in 2..=last {
        // Alternate between a clean file and one with an error, so a stale
        // publication would be visible in the content as well as the version.
        let text = if version % 2 == 0 {
            SEMANTIC_SRC.to_owned()
        } else {
            format!("{SEMANTIC_SRC}void broken(\n")
        };
        client.notify::<notif::DidChangeTextDocument>(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier { uri: uri(), version },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text,
            }],
        });
    }

    let mut versions = Vec::new();
    let final_report = loop {
        let published = client.diagnostics_for(&uri(), &mut seen);
        versions.push(published.version.expect("an open document's version"));
        if published.version == Some(last) {
            break published;
        }
    };
    assert!(
        // Non-strict: a job that starts after an edit's text lands but before
        // its own job is queued reports that same version, correctly.
        versions.windows(2).all(|w| w[0] <= w[1]),
        "diagnostics were published out of version order: {versions:?}"
    );
    assert!(!errors(&final_report).is_empty(), "version {last} has a parse error");

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    let mut rest = Vec::new();
    assert_eq!(client.finish(&mut rest), kg_lsp::ExitCode::Ok);
    let late = rest
        .iter()
        .filter(|n| n.method == notif::PublishDiagnostics::METHOD)
        .map(|n| serde_json::from_value::<PublishDiagnosticsParams>(n.params.clone()).unwrap())
        .filter(|p| p.uri == uri() && p.version != Some(last))
        .collect::<Vec<_>>();
    // The last version may publish twice — its syntax errors early, then the
    // full check — but nothing older may follow it.
    assert!(late.is_empty(), "an older version was published after the last: {late:?}");
}

/// `$/cancelRequest` reaches the pool: a burst of requests, each cancelled
/// straight away, is answered exactly once per request — by its handler if a
/// worker had already started it, as `RequestCancelled` if not. Which of the
/// two each one gets is the scheduler's business; that one withdrawn before
/// it started is never run is pinned by `main_loop`'s unit test.
#[test]
fn every_cancelled_request_is_answered_exactly_once() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: SEMANTIC_SRC.to_owned(),
        },
    });

    let mut ids = Vec::new();
    for _ in 0..40 {
        let id = client.request::<req::HoverRequest>(HoverParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri() },
                position: Position::new(3, 9),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        });
        client.notify::<notif::Cancel>(lsp_types::CancelParams {
            id: lsp_types::NumberOrString::Number(client.next_id),
        });
        ids.push(id);
    }
    let id = client.request::<req::Shutdown>(());
    client.notify::<notif::Exit>(());
    // Joining the server joins its workers, so every response that will ever
    // be sent has been sent.
    let (code, messages) = client.finish_all();
    assert_eq!(code, kg_lsp::ExitCode::Ok);

    let mut answered: Vec<RequestId> = Vec::new();
    for message in messages {
        let Message::Response(r) = message else { continue };
        if r.id == id {
            continue;
        }
        if let Err(e) = &r.response_result {
            assert_eq!(e.code, lsp_server::ErrorCode::RequestCanceled as i32, "{e:?}");
        }
        answered.push(r.id);
    }

    answered.sort_by_key(|id| id.to_string());
    let mut expected = ids.clone();
    expected.sort_by_key(|id| id.to_string());
    assert_eq!(answered, expected, "some request was answered twice, or never");
}

const LOOP_SRC: &str = "\
template <typename T>
inline void take(T v)
{
    auto w = v + 1;
}

template <auto N>
class Box
{
    uint32[N] items;

    inline auto first()
    {
        return items[0] + N;
    }
}

inline void main()
{
    take<uint8>(1);
    take<uint16>(2);
    static Box<4> a;
    auto r = a.first();
}
";

/// The program-wide loop's types reach a client: inside a template's body
/// each instance's (`w` is `uint9` in `take<uint8>` and `uint17` in
/// `take<uint16>`), in a plain function what an instantiated call returns
/// (`r` is `Box<4>::first()`'s `uint33`), and at a call the instance it
/// compiles — none of which the CST engine the editor read before types.
/// The diagnostics are awaited first, as a client sees them land before it
/// asks: the requests then read the loop they ran (`Db::editor_loop`).
#[test]
fn the_loop_s_types_reach_hover_inlay_hints_and_signature_help() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let loop_uri = Uri::from_str("file:///workspace/loop.k").unwrap();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: loop_uri.clone(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: LOOP_SRC.to_owned(),
        },
    });
    let published = client.diagnostics_for(&loop_uri, &mut seen);
    assert_eq!(errors(&published), Vec::<&str>::new());

    let at = |line, character| TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: loop_uri.clone() },
        position: Position::new(line, character),
    };
    let hover = |client: &mut Client, seen: &mut Vec<Notification>, line, character| -> String {
        let id = client.request::<req::HoverRequest>(HoverParams {
            text_document_position_params: at(line, character),
            work_done_progress_params: WorkDoneProgressParams::default(),
        });
        let hover: Option<Hover> = result(client.response(&id, seen));
        match hover.expect("a hover").contents {
            HoverContents::Markup(m) => m.value,
            other => panic!("expected markdown, got {other:?}"),
        }
    };
    assert_eq!(
        hover(&mut client, &mut seen, 3, 9),
        "```kanagawa\nuint9 w   // take<uint8>\nuint17 w  // take<uint16>\n```\n\n\
         **local** `take::$::w`\n\nTyped in 2 instances: a template's body has no type of its own."
    );
    assert_eq!(hover(&mut client, &mut seen, 22, 9), "```kanagawa\nuint33 r\n```\n\n**local** `main::$::r`");

    let id = client.request::<req::InlayHintRequest>(InlayHintParams {
        work_done_progress_params: WorkDoneProgressParams::default(),
        text_document: TextDocumentIdentifier { uri: loop_uri.clone() },
        range: Range::new(Position::new(0, 0), Position::new(24, 0)),
    });
    let hints: Option<Vec<InlayHint>> = result(client.response(&id, &mut seen));
    let shown: Vec<(Position, String)> = hints
        .expect("inlay hints")
        .into_iter()
        .map(|h| match h.label {
            InlayHintLabel::String(s) => (h.position, s),
            other => panic!("plain labels only, got {other:?}"),
        })
        .collect();
    assert_eq!(
        shown,
        [(Position::new(3, 10), ": uint9 | uint17".to_owned()), (Position::new(22, 10), ": uint33".to_owned())]
    );

    let id = client.request::<req::SignatureHelpRequest>(SignatureHelpParams {
        context: None,
        text_document_position_params: at(20, 17),
        work_done_progress_params: WorkDoneProgressParams::default(),
    });
    let help: Option<SignatureHelp> = result(client.response(&id, &mut seen));
    let signature = help.expect("a signature").signatures.into_iter().next().expect("one");
    assert_eq!(signature.label, "take(T v) -> void");
    match signature.documentation.expect("the instance") {
        lsp_types::Documentation::MarkupContent(m) => {
            assert!(m.value.starts_with("```kanagawa\n(uint16) -> void take  // take<uint16>\n```"), "{}", m.value)
        }
        other => panic!("expected markdown, got {other:?}"),
    }

    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut seen), kg_lsp::ExitCode::Ok);
}

const MEMBERS_SRC: &str = "\
struct Pair
{
    uint8 lo;
    uint16 hi;
}

template <auto N>
class Box
{
    uint32[N] items;

    inline auto first()
    {
        return items[0] + N;
    }
}

template <typename T>
inline Pair make(T v)
{
    Pair p = {};
    return p;
}

template <typename T>
inline void take(T v)
{
    static Box<bitsizeof(T)> b;
    auto w = v + 1;
    auto h = b.items[0];
}

inline void main()
{
    take<uint8>(1);
    take<uint16>(2);
    static Box<4> a;
    auto r = a.first();
    auto p = make<uint8>(1);
    auto q = p.lo;
}
";

/// One completion candidate as a client shows it: label, detail,
/// documentation.
type Offered = (String, Option<String>, Option<String>);

/// What a completion at `(line, character)` of `uri` offers.
fn completed(client: &mut Client, seen: &mut Vec<Notification>, uri: &Uri, line: u32, character: u32) -> Vec<Offered> {
    let id = client.request::<req::Completion>(lsp_types::CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position::new(line, character),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: None,
    });
    let offered: Option<lsp_types::CompletionResponse> = result(client.response(&id, seen));
    let Some(lsp_types::CompletionResponse::Array(offered)) = offered else {
        panic!("completion must answer with an array");
    };
    offered
        .into_iter()
        .map(|i| {
            let documentation = i.documentation.map(|d| match d {
                lsp_types::Documentation::MarkupContent(m) => m.value,
                lsp_types::Documentation::String(s) => s,
            });
            (i.label, i.detail, documentation)
        })
        .collect()
}

/// Opens `MEMBERS_SRC` and awaits its diagnostics — as a client sees them
/// land before it asks — so the requests read the loop they ran
/// (`Db::editor_loop`).
fn open_members(client: &mut Client, seen: &mut Vec<Notification>) -> Uri {
    let uri = Uri::from_str("file:///workspace/members.k").unwrap();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, seen));
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: MEMBERS_SRC.to_owned(),
        },
    });
    let published = client.diagnostics_for(&uri, seen);
    assert_eq!(errors(&published), Vec::<&str>::new());
    uri
}

fn shut_down(mut client: Client, mut seen: Vec<Notification>) {
    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut seen), kg_lsp::ExitCode::Ok);
}

fn entry(label: &str, detail: &str) -> Offered {
    (label.to_owned(), Some(detail.to_owned()), None)
}

/// A member list read off the loop's type of the base, in a plain
/// function: `a` is `Box<4>`, so `a.` offers the instance's copy's members
/// with its widths — `uint32[4]`, and `first`'s `items[0] + 4`, `uint33` —
/// and `p` is what `make<uint8>` returns, so `p.` offers `Pair`'s fields.
/// The CST engine types neither base — an instance's name and an
/// instantiated call's result are the loop's to know — and offered nothing
/// at either.
#[test]
fn a_member_list_reads_the_loop_s_type_of_the_base() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = open_members(&mut client, &mut seen);
    assert_eq!(
        completed(&mut client, &mut seen, &uri, 37, 15),
        [entry("items", "uint32[4]"), entry("first", "() -> uint33")]
    );
    assert_eq!(completed(&mut client, &mut seen, &uri, 39, 15), [entry("lo", "uint8"), entry("hi", "uint16")]);
    shut_down(client, seen);
}

/// Inside a template's body the base has a type per instance — `b` is
/// `Box<8>` in `take<uint8>` and `Box<16>` in `take<uint16>` — and so does
/// each member: `items` is `uint32[8]` in one and `uint32[16]` in the
/// other, listed as both with the instance that gives each, where `first`
/// is `() -> uint33` in both.
#[test]
fn a_member_list_inside_a_template_lists_each_instance_s_type() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = open_members(&mut client, &mut seen);
    let offered = completed(&mut client, &mut seen, &uri, 29, 15);
    let shown: Vec<(&str, Option<&str>)> = offered.iter().map(|(l, d, _)| (l.as_str(), d.as_deref())).collect();
    assert_eq!(shown, [("items", Some("uint32[8] | uint32[16]")), ("first", Some("() -> uint33"))]);
    assert_eq!(
        offered[0].2.as_deref(),
        Some(
            "```kanagawa\nuint32[8]   // take<uint8>\nuint32[16]  // take<uint16>\n```\n\n\
             Typed in 2 instances: a template's body has no type of its own."
        )
    );
    shut_down(client, seen);
}

/// A bare name's detail is the loop's type where it is declared, as hover
/// shows it: inside a template's body each instance's (`w` is `v + 1`,
/// `uint9` in `take<uint8>` and `uint17` in `take<uint16>`), and in a
/// plain function what the loop decided (`r` is `Box<4>::first()`'s
/// `uint33`) — neither of which the CST engine types.
#[test]
fn a_candidate_s_detail_is_the_loop_s_type() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = open_members(&mut client, &mut seen);
    let in_body = completed(&mut client, &mut seen, &uri, 29, 13);
    let w = in_body.iter().find(|(l, _, _)| l == "w").expect("`w` is offered in its block");
    assert_eq!(w.1.as_deref(), Some("uint9 | uint17"));
    assert!(w.2.as_deref().is_some_and(|d| d.contains("uint9   // take<uint8>")), "{w:?}");
    let in_main = completed(&mut client, &mut seen, &uri, 39, 13);
    let r = in_main.iter().find(|(l, _, _)| l == "r").expect("`r` is offered in its block");
    assert_eq!(*r, entry("r", "uint33"));
    shut_down(client, seen);
}

/// Go-to-definition on a field reads the same type: `p.lo`, `p` what
/// `make<uint8>` returns, jumps to `Pair`'s `lo` — where the CST engine,
/// with no type for `p`, declined.
#[test]
fn a_field_jumps_through_the_loop_s_type_of_the_base() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = open_members(&mut client, &mut seen);
    let id = client.request::<req::GotoDefinition>(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position::new(39, 16),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let jump: Option<GotoDefinitionResponse> = result(client.response(&id, &mut seen));
    match jump.expect("a jump") {
        GotoDefinitionResponse::Scalar(found) => {
            assert_eq!(found.uri, uri);
            assert_eq!(found.range, Range::new(Position::new(2, 10), Position::new(2, 12)));
        }
        other => panic!("one location, got {other:?}"),
    }
    shut_down(client, seen);
}

/// Methods and members reached through a receiver, each shape the editor
/// meets: a plain class's method, a class template instance's (`Box<4>`,
/// and the same instance through a `using`), a member template (`Run<3>`,
/// and `As<uint3>` on an instance), and a member each branch of a `static
/// if` declares — reached from a template's body whose instances take both
/// branches, and from `main` on one instance.
const METHODS_SRC: &str = "\
class Counter
{
    uint32 n;

public:
    inline void Go()
    {
        n = n + 1;
    }

    inline void Bump()
    {
        auto f = [](uint32 a) -> uint32 { return a + 1; };
        n = f(n);
    }
}

template <auto N>
class Box
{
    uint32[N] items;

public:
    inline uint32 Get(uint32 i)
    {
        return items[i];
    }

    inline uint32[N] Items()
    {
        return items;
    }

    template <typename T>
    inline T As(uint32 i)
    {
        return cast<T>(items[i]);
    }
}

using Box4 = Box<4>;

class Fixture
{
public:
    template <auto K>
    inline uint32 Run(uint32 t)
    {
        return t + K;
    }
}

template <bool Fast>
class Engine
{
public:
    static if (Fast)
    {
        inline uint32 Go()
        {
            return 1;
        }
    }
    else
    {
        inline uint8 Go()
        {
            return 2;
        }
    }
}

template <bool F>
inline uint32 run()
{
    static Engine<F> e;
    return e.Go();
}

inline void main()
{
    static Counter c;
    c.Go();
    c.Bump();
    static Box<4> a;
    static Box<9> z;
    static Box4 b;
    auto x = a.Get(0);
    auto y = b.Get(1);
    auto i = a.Items();
    auto j = z.Items();
    static Fixture fixture;
    auto r = fixture.Run<3>(1);
    auto q = fixture.Run<5>(1);
    auto s = a.As<uint3>(2);
    static Engine<true> t;
    auto u = t.Go();
    auto v = run<true>();
    auto w = run<false>();
}
";

/// Where `ident` starts in the first `before` + `ident` of `text` — a
/// fixture's caret or a declaration's name, read off the text rather than
/// counted. The fixtures are ASCII, so a byte column is a UTF-16 one.
#[track_caller]
fn after(text: &str, before: &str, ident: &str) -> Position {
    let whole = format!("{before}{ident}");
    let at = text.find(&whole).unwrap_or_else(|| panic!("`{whole}` is not in the fixture")) + before.len();
    let line = text[..at].matches('\n').count() as u32;
    let character = (at - text[..at].rfind('\n').map_or(0, |n| n + 1)) as u32;
    Position::new(line, character)
}

/// The range of the name `ident` just after `before`.
#[track_caller]
fn named(text: &str, before: &str, ident: &str) -> Range {
    let start = after(text, before, ident);
    Range::new(start, Position::new(start.line, start.character + ident.len() as u32))
}

/// Opens `text` at `uri` and awaits its diagnostics, which must be clean —
/// so the requests that follow read the loop they ran (`Db::editor_loop`).
fn open_clean(client: &mut Client, seen: &mut Vec<Notification>, uri: &Uri, text: &str) {
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, seen));
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "kanagawa".to_owned(),
            version: 1,
            text: text.to_owned(),
        },
    });
    let published = client.diagnostics_for(uri, seen);
    assert_eq!(errors(&published), Vec::<&str>::new());
}

/// What go-to-definition at `position` in `uri` answers.
fn jumps(client: &mut Client, seen: &mut Vec<Notification>, uri: &Uri, position: Position) -> Option<GotoDefinitionResponse> {
    let id = client.request::<req::GotoDefinition>(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    result(client.response(&id, seen))
}

/// The one place a jump lands, in `uri`; panics on a list or on nothing.
#[track_caller]
fn lands(client: &mut Client, seen: &mut Vec<Notification>, uri: &Uri, position: Position) -> Range {
    match jumps(client, seen, uri, position) {
        Some(GotoDefinitionResponse::Scalar(found)) => {
            assert_eq!(found.uri, *uri);
            found.range
        }
        other => panic!("one location at {position:?}, got {other:?}"),
    }
}

/// The markdown a hover at `position` shows.
#[track_caller]
fn hovered(client: &mut Client, seen: &mut Vec<Notification>, uri: &Uri, position: Position) -> String {
    let id = client.request::<req::HoverRequest>(HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    });
    let hover: Option<Hover> = result(client.response(&id, seen));
    match hover.unwrap_or_else(|| panic!("a hover at {position:?}")).contents {
        HoverContents::Markup(m) => m.value,
        other => panic!("expected markdown, got {other:?}"),
    }
}

fn methods_uri() -> Uri {
    Uri::from_str("file:///workspace/methods.k").unwrap()
}

/// A plain class's method: `c.Go()` names `Go` through `c`'s type
/// (`method`, `Type.hs:1358-1362`), so the jump lands on `Counter`'s `Go`
/// — not `Engine`'s, spelled the same — and the hover shows its
/// signature. Neither engine answered a method before. `Bump` holds a
/// lambda, which `desugarLambdas` rebuilds the method round: the jump,
/// and the member list after `c.`, still reach it.
#[test]
fn a_method_jumps_to_its_declaration_through_the_receiver() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = methods_uri();
    open_clean(&mut client, &mut seen, &uri, METHODS_SRC);
    let caret = after(METHODS_SRC, "c.", "Go");
    assert_eq!(lands(&mut client, &mut seen, &uri, caret), named(METHODS_SRC, "inline void ", "Go"));
    assert_eq!(hovered(&mut client, &mut seen, &uri, caret), "```kanagawa\n() -> void Go\n```\n\n**function** `Counter::Go`");
    let bump = after(METHODS_SRC, "c.", "Bump");
    assert_eq!(lands(&mut client, &mut seen, &uri, bump), named(METHODS_SRC, "inline void ", "Bump"));
    let offered = completed(&mut client, &mut seen, &uri, bump.line, bump.character);
    let labels: Vec<&str> = offered.iter().map(|(l, _, _)| l.as_str()).collect();
    assert_eq!(labels, ["n", "Go", "Bump"]);
    shut_down(client, seen);
}

/// A class template instance's method lands on the template's member — the
/// text the user wrote, which every instance's copy was made from — through
/// the instance's name and through a `using` of it alike. The hover is the
/// instance's own: `a.Items()` on a `Box<4>` is `() -> uint32[4]`, where
/// the declaration's own answer lists `Box<9>`'s `uint32[9]` beside it.
#[test]
fn a_template_instance_s_method_jumps_to_the_template_s_member() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = methods_uri();
    open_clean(&mut client, &mut seen, &uri, METHODS_SRC);
    let get = named(METHODS_SRC, "inline uint32 ", "Get");
    assert_eq!(lands(&mut client, &mut seen, &uri, after(METHODS_SRC, "a.", "Get")), get);
    assert_eq!(lands(&mut client, &mut seen, &uri, after(METHODS_SRC, "b.", "Get")), get, "through `using Box4`");
    assert_eq!(
        hovered(&mut client, &mut seen, &uri, after(METHODS_SRC, "a.", "Items")),
        "```kanagawa\n() -> uint32[4] Items\n```\n\n**function** `Box::Items`"
    );
    assert_eq!(
        hovered(&mut client, &mut seen, &uri, after(METHODS_SRC, "z.", "Items")),
        "```kanagawa\n() -> uint32[9] Items\n```\n\n**function** `Box::Items`"
    );
    shut_down(client, seen);
}

/// A member template lands on its `template <…>`'s function — on a plain
/// class (`fixture.Run<3>`) and on an instance's copy (`a.As<uint3>`) —
/// and the hover shows the instance the call compiles, as signature help
/// does: `Run<3>`, not `Run<5>` beside it, which the template's own answer
/// would list.
#[test]
fn a_member_template_jumps_to_its_template() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = methods_uri();
    open_clean(&mut client, &mut seen, &uri, METHODS_SRC);
    let run = after(METHODS_SRC, "fixture.", "Run");
    assert_eq!(lands(&mut client, &mut seen, &uri, run), named(METHODS_SRC, "inline uint32 ", "Run"));
    let r#as = after(METHODS_SRC, "a.", "As");
    assert_eq!(lands(&mut client, &mut seen, &uri, r#as), named(METHODS_SRC, "inline T ", "As"));
    assert_eq!(
        hovered(&mut client, &mut seen, &uri, run),
        "```kanagawa\n(uint32) -> uint32 Run  // Run<3>\n```\n\n**template** `Fixture::Run`\n\nThe instance this call compiles."
    );
    assert_eq!(
        hovered(&mut client, &mut seen, &uri, r#as),
        "```kanagawa\n(uint32) -> uint3 As  // As<uint3>\n```\n\n**template** `Box::As`\n\nThe instance this call compiles."
    );
    shut_down(client, seen);
}

/// `Engine` declares `Go` in each branch of a `static if`, so inside
/// `run<F>`'s body `e.Go()` is the first branch's `Go` in `run<true>` and
/// the second's in `run<false>`: both are offered, as the protocol's list,
/// in the text's order, and the hover types each and names both. From
/// `main`, on the one instance `Engine<true>`, the jump is that branch's
/// alone.
#[test]
fn a_member_two_instances_declare_apart_offers_both() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = methods_uri();
    open_clean(&mut client, &mut seen, &uri, METHODS_SRC);
    let first = named(METHODS_SRC, "inline uint32 ", "Go");
    let second = named(METHODS_SRC, "inline uint8 ", "Go");
    let caret = after(METHODS_SRC, "e.", "Go");
    match jumps(&mut client, &mut seen, &uri, caret) {
        Some(GotoDefinitionResponse::Array(found)) => {
            assert!(found.iter().all(|l| l.uri == uri));
            assert_eq!(found.iter().map(|l| l.range).collect::<Vec<_>>(), [first, second]);
        }
        other => panic!("both declarations, got {other:?}"),
    }
    assert_eq!(lands(&mut client, &mut seen, &uri, after(METHODS_SRC, "t.", "Go")), first);
    assert_eq!(
        hovered(&mut client, &mut seen, &uri, caret),
        "```kanagawa\n() -> uint8 Go   // run<false>\n() -> uint32 Go  // run<true>\n```\n\n\
         **function** `Engine::$quasi0::Go`\n\n**function** `Engine::$quasi1::Go`\n\n\
         The instances reach 2 declarations of `Go`.\n\n\
         Typed in 2 instances: a template's body has no type of its own."
    );
    shut_down(client, seen);
}

/// References and rename reach a method through its receivers: `Counter`'s
/// `Go` is declared once and called as `c.Go()`, and neither `Engine`'s
/// `Go`s nor their calls are its uses. A rename of `Engine`'s first `Go` is
/// refused, naming the line: `e.Go()` reaches it in `run<true>` and the
/// other `Go` in `run<false>`, so no edit keeps both instances compiling.
#[test]
fn references_and_rename_reach_a_method_through_its_receivers() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = methods_uri();
    open_clean(&mut client, &mut seen, &uri, METHODS_SRC);
    let declaration = after(METHODS_SRC, "inline void ", "Go");
    let id = client.request::<req::References>(ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: declaration,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext { include_declaration: true },
    });
    let found: Option<Vec<Location>> = result(client.response(&id, &mut seen));
    let ranges: Vec<Range> = found.expect("references").into_iter().map(|l| l.range).collect();
    assert_eq!(ranges, [named(METHODS_SRC, "inline void ", "Go"), named(METHODS_SRC, "c.", "Go")]);

    let rename = |client: &mut Client, seen: &mut Vec<Notification>, position| {
        let id = client.request::<req::Rename>(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            new_name: "Step".to_owned(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        });
        client.response(&id, seen)
    };
    let edit: Option<WorkspaceEdit> = result(rename(&mut client, &mut seen, declaration));
    #[allow(clippy::mutable_key_type)]
    let changes = edit.expect("an edit").changes.expect("changes keyed by document");
    let mut ranges: Vec<Range> = changes[&uri].iter().map(|e| e.range).collect();
    ranges.sort_by_key(|r| (r.start.line, r.start.character));
    assert_eq!(ranges, [named(METHODS_SRC, "inline void ", "Go"), named(METHODS_SRC, "c.", "Go")]);

    let refused = rename(&mut client, &mut seen, after(METHODS_SRC, "inline uint32 ", "Go"));
    let message = refused.response_result.expect_err("refused").message;
    let line = after(METHODS_SRC, "e.", "Go").line + 1;
    assert!(message.contains(&format!("`.Go` at methods.k:{line}")), "{message}");
    shut_down(client, seen);
}

/// A class template whose method is the editor's commonest broken buffer
/// once `a.` is typed at the start of its body: `a` is a member, `Box<N>`,
/// so its type — and every type in `run` — is each instance's (`Runner<4>`
/// here), which only the loop knows.
const RUNNER_SRC: &str = "\
template <auto N>
class Box
{
    uint32[N] items;

public:
    inline uint32 first()
    {
        return items[0];
    }
}

template <auto N>
class Runner
{
    Box<N> a;

public:
    inline uint32 run()
    {
        auto f = a.first();
        auto g = f + 1;
        return g;
    }
}

inline void main()
{
    static Runner<4> r;
    auto v = r.run();
}
";

/// `RUNNER_SRC` with `a.` typed at the start of `run`'s body, which no
/// longer parses: `a.` before `auto` is a member access with no member.
fn runner_typed() -> String {
    RUNNER_SRC.replace("    {\n        auto f", "    {\n        a.\n        auto f")
}

fn runner_uri() -> Uri {
    Uri::from_str("file:///workspace/runner.k").unwrap()
}

/// Replaces the whole of `uri`'s text, as a client that sends full
/// documents does, and awaits the diagnostics of that version.
fn change(client: &mut Client, seen: &mut Vec<Notification>, uri: &Uri, version: i32, text: &str) -> PublishDiagnosticsParams {
    client.notify::<notif::DidChangeTextDocument>(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version },
        content_changes: vec![TextDocumentContentChangeEvent { range: None, range_length: None, text: text.to_owned() }],
    });
    loop {
        let published = client.diagnostics_for(uri, seen);
        if published.version == Some(version) {
            return published;
        }
    }
}

/// The editor's commonest member list, sprint W3.4's reason: `a.` typed at
/// the start of a template instance's method, which breaks the parse. The
/// buffer's diagnostics are its syntax errors alone, as the reference stops
/// there (`Main.hs:73-75`) — nothing the loop says reaches the gutter — and
/// every question reads the loop the last parsing state ran, mapped onto
/// the text: the member list is `Box<4>`'s, each member as `Runner<4>`
/// has it; `g`, a line lower than the loop knew it, hovers as `uint33`;
/// and `a.first()`, likewise moved, jumps to `Box`'s `first`. The CST
/// engine answered none of these — a template's body is no type of its
/// own, and it never typed a method.
#[test]
fn typing_a_dot_in_a_template_instance_s_method_reads_the_last_loop() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = runner_uri();
    open_clean(&mut client, &mut seen, &uri, RUNNER_SRC);
    let typed = runner_typed();
    let published = change(&mut client, &mut seen, &uri, 2, &typed);
    assert_eq!(errors(&published), ["expected an identifier", "expected `;`"]);

    let caret = after(&typed, "        a", ".");
    let offered = completed(&mut client, &mut seen, &uri, caret.line, caret.character + 1);
    let shown: Vec<(&str, Option<&str>)> = offered.iter().map(|(l, d, _)| (l.as_str(), d.as_deref())).collect();
    assert_eq!(shown, [("items", Some("uint32[4]")), ("first", Some("() -> uint32"))]);
    assert!(offered[0].2.as_deref().is_some_and(|d| d.contains("uint32[4]  // Runner<4>")), "{offered:?}");

    let g = hovered(&mut client, &mut seen, &uri, after(&typed, "auto ", "g"));
    assert!(g.starts_with("```kanagawa\nuint33 g  // Runner<4>\n```"), "{g}");
    assert_eq!(
        lands(&mut client, &mut seen, &uri, after(&typed, "a.", "first")),
        named(&typed, "inline uint32 ", "first")
    );
    shut_down(client, seen);
}

/// A buffer opened broken has no loop to map, and its first hover runs its
/// own, once — the loop over its error-recovered trees, for the answers
/// alone: the diagnostics stay the syntax errors. Until then completion,
/// which never runs a loop, has the CST engine's answer, which inside a
/// template's body is nothing; after it, the loop's.
#[test]
fn a_buffer_opened_broken_runs_its_own_loop_for_a_hover() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = runner_uri();
    let typed = runner_typed();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, &mut seen));
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem { uri: uri.clone(), language_id: "kanagawa".to_owned(), version: 1, text: typed.clone() },
    });
    let published = client.diagnostics_for(&uri, &mut seen);
    assert_eq!(errors(&published), ["expected an identifier", "expected `;`"]);

    let caret = after(&typed, "        a", ".");
    assert_eq!(completed(&mut client, &mut seen, &uri, caret.line, caret.character + 1), []);
    let g = hovered(&mut client, &mut seen, &uri, after(&typed, "auto ", "g"));
    assert!(g.starts_with("```kanagawa\nuint33 g  // Runner<4>\n```"), "{g}");
    let offered = completed(&mut client, &mut seen, &uri, caret.line, caret.character + 1);
    let shown: Vec<&str> = offered.iter().map(|(l, _, _)| l.as_str()).collect();
    assert_eq!(shown, ["items", "first"]);
    shut_down(client, seen);
}

/// The fix a diagnostic carries reaches a real client as a quick fix: the
/// server advertises `codeActionProvider`, publishes the narrowing error, and
/// answers `textDocument/codeAction` on its range with the cast as a
/// workspace edit (`handlers::code_action`). Red with the request left
/// undispatched (`MethodNotFound`) or the capability unadvertised.
#[test]
fn a_diagnostic_s_fix_is_offered_as_a_quick_fix() {
    const NARROWED: &str = "struct S\n{\n    uint8 a;\n}\n\ninline void main()\n{\n    uint32 x = 3;\n    S s = {.a = x};\n}\n";
    let mut client = Client::start();
    let mut seen: Vec<Notification> = Vec::new();
    let uri = Uri::from_str("file:///workspace/narrowed.k").unwrap();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let init: lsp_types::InitializeResult = result(client.response(&id, &mut seen));
    assert!(init.capabilities.code_action_provider.is_some(), "{:?}", init.capabilities);
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem { uri: uri.clone(), language_id: "kanagawa".to_owned(), version: 1, text: NARROWED.to_owned() },
    });
    let published = client.diagnostics_for(&uri, &mut seen);
    assert_eq!(errors(&published), ["Invalid conversion from `uint32` to `uint8`\nInsert an explicit cast to silence this issue `cast<uint8>()`"]);

    let id = client.request::<req::CodeActionRequest>(lsp_types::CodeActionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        range: published.diagnostics[0].range,
        context: lsp_types::CodeActionContext { diagnostics: published.diagnostics.clone(), only: None, trigger_kind: None },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let actions: lsp_types::CodeActionResponse = result(client.response(&id, &mut seen));
    let [lsp_types::CodeActionOrCommand::CodeAction(action)] = actions.as_slice() else { panic!("one action: {actions:?}") };
    assert_eq!(action.kind, Some(lsp_types::CodeActionKind::QUICKFIX));
    let edits = &action.edit.as_ref().and_then(|e| e.changes.as_ref()).expect("a workspace edit")[&uri];
    let texts: Vec<(Position, &str)> = edits.iter().map(|e| (e.range.start, e.new_text.as_str())).collect();
    assert_eq!(texts, [(Position::new(8, 16), "cast<uint8>("), (Position::new(8, 17), ")")]);
    shut_down(client, seen);
}

/// `workspace/symbol` reaches a real client: advertised, dispatched, and
/// answered with the open program's declarations by name — a class's member
/// under the class (`handlers::workspace_symbol`). Red with the request
/// left undispatched or the capability unadvertised.
#[test]
fn a_workspace_symbol_search_finds_a_member_under_its_class() {
    let mut client = Client::start();
    let mut seen: Vec<Notification> = Vec::new();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let init: lsp_types::InitializeResult = result(client.response(&id, &mut seen));
    assert!(init.capabilities.workspace_symbol_provider.is_some(), "{:?}", init.capabilities);
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem { uri: uri(), language_id: "kanagawa".to_owned(), version: 1, text: SRC.to_owned() },
    });
    let _ = client.diagnostics_for(&uri(), &mut seen);
    let id = client.request::<req::WorkspaceSymbolRequest>(lsp_types::WorkspaceSymbolParams {
        query: "add".to_owned(),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    // Read as JSON: the response type is untagged, and a `WorkspaceSymbol`
    // with a `Location` deserializes as the flat form's just as well.
    let found: Vec<serde_json::Value> = result(client.response(&id, &mut seen));
    let names: Vec<(&str, Option<&str>)> =
        found.iter().map(|s| (s["name"].as_str().unwrap(), s["containerName"].as_str())).collect();
    assert_eq!(names, [("add", Some("Adder")), ("Adder", None)]);
    shut_down(client, seen);
}

/// `Down<n>` calls `Down<n - 1>` down to `Down<0>`, whose `bool[0] x` is
/// the program's one error — reached `n + 1` rounds in, past the editor's
/// first bound of four — each copy carrying `body` statements, so the run
/// from there to the fixpoint takes long enough for a client to act in.
fn down(n: usize, body: usize) -> String {
    let body: String = (0..body).map(|i| format!("    const auto v{i} = cast<uint32>(a + {i}) * b;\n")).collect();
    format!(
        "template <auto N>\ninline uint32 Down(uint32 a, uint32 b)\n{{\n    bool[N] x;\n{body}    static if (N > 0)\n    {{\n        return Down<N - 1>(a, b);\n    }}\n    else\n    {{\n        return a;\n    }}\n}}\n\ninline uint32 main(uint32 a)\n{{\n    return Down<{n}>(a, 3);\n}}\n"
    )
}

/// Opens `text` as `down.k`, version 1.
fn open_down(client: &mut Client, seen: &mut Vec<Notification>, text: String) -> Uri {
    let uri = Uri::from_str("file:///workspace/down.k").unwrap();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let _: serde_json::Value = result(client.response(&id, seen));
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem { uri: uri.clone(), language_id: "kanagawa".to_owned(), version: 1, text },
    });
    uri
}

/// Every `publishDiagnostics` for `uri` among `seen`, as the client read
/// them.
fn published_for(seen: &[Notification], uri: &Uri) -> Vec<PublishDiagnosticsParams> {
    seen.iter()
        .filter(|n| n.method == notif::PublishDiagnostics::METHOD)
        .map(|n| serde_json::from_value::<PublishDiagnosticsParams>(n.params.clone()).unwrap())
        .filter(|p| p.uri == *uri)
        .collect()
}

const DOWN_ERROR: &str = "Array dimension must be a positive integer";

/// The editor's diagnostics land twice for one version where the loop runs
/// past four rounds (sprint W7.6, the owner's answer of 2026-10-10): first
/// at the fourth round, which leaves `Down<0>` unmade and so says nothing
/// — a truncated loop invents no error — then at the fixpoint, `kg
/// check`'s answer, with the error. Red with the first tier unpublished,
/// or the last answer never published after it.
#[test]
fn the_diagnostics_land_at_four_rounds_then_at_the_fixpoint() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = open_down(&mut client, &mut seen, down(120, 150));
    let first = client.diagnostics_for(&uri, &mut seen);
    assert_eq!((first.version, errors(&first)), (Some(1), Vec::<&str>::new()));
    let last = client.diagnostics_for(&uri, &mut seen);
    assert_eq!((last.version, errors(&last)), (Some(1), vec![DOWN_ERROR]));
    let mut rest = Vec::new();
    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut rest);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut rest), kg_lsp::ExitCode::Ok);
    assert_eq!(published_for(&rest, &uri), Vec::new(), "nothing after the last answer");
}

/// An edit while the fixpoint runs on cancels it: version 1's first
/// answer stands until version 2's replaces it, and version 1's last
/// answer — its error — never reaches the client. Version 2 ends by the
/// fourth round, so its one answer is its last. Red with the run deaf to
/// the edit's token (version 1's error is published, late or before
/// version 2's), or with the edit queued behind the run.
#[test]
fn an_edit_while_the_fixpoint_runs_cancels_it() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = open_down(&mut client, &mut seen, down(120, 300));
    let first = client.diagnostics_for(&uri, &mut seen);
    assert_eq!((first.version, errors(&first)), (Some(1), Vec::<&str>::new()));
    client.notify::<notif::DidChangeTextDocument>(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version: 2 },
        content_changes: vec![TextDocumentContentChangeEvent { range: None, range_length: None, text: down(3, 1) }],
    });
    let next = client.diagnostics_for(&uri, &mut seen);
    assert_eq!((next.version, errors(&next)), (Some(2), vec![DOWN_ERROR]), "version 1's last answer came first");
    let id = client.request::<req::Shutdown>(());
    let _ = client.response(&id, &mut seen);
    client.notify::<notif::Exit>(());
    assert_eq!(client.finish(&mut seen), kg_lsp::ExitCode::Ok);
    let versions: Vec<(Option<i32>, usize)> = published_for(&seen, &uri).iter().map(|p| (p.version, errors(p).len())).collect();
    assert_eq!(versions, [(Some(1), 0), (Some(2), 1)], "version 1's run was not cancelled");
}

/// A request asked while the fixpoint runs on is answered from the first
/// answer at once — not queued behind the run, nor a run of its own: a
/// hover on `x`, inside `Down`'s body, lists the instances four rounds
/// made and says the list may be short, and arrives before the last
/// answer is published. Once it is, the same hover says nothing of the
/// sort. Red with `Db::editor_loop` waiting for the last answer.
#[test]
fn a_hover_while_the_fixpoint_runs_is_answered_from_the_first_answer() {
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = open_down(&mut client, &mut seen, down(120, 300));
    let first = client.diagnostics_for(&uri, &mut seen);
    assert_eq!((first.version, errors(&first)), (Some(1), Vec::<&str>::new()));
    let hover = |client: &mut Client, seen: &mut Vec<Notification>| -> String {
        let id = client.request::<req::HoverRequest>(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: Position::new(3, 12),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        });
        let hover: Option<Hover> = result(client.response(&id, seen));
        match hover.expect("a hover").contents {
            HoverContents::Markup(m) => m.value,
            other => panic!("expected markdown, got {other:?}"),
        }
    };
    let mut during = Vec::new();
    let early = hover(&mut client, &mut during);
    assert!(early.contains("had not reached its fixpoint"), "{early}");
    assert!(published_for(&during, &uri).is_empty(), "the hover waited for the last answer: {during:?}");
    seen.extend(during);
    let last = client.diagnostics_for(&uri, &mut seen);
    assert_eq!((last.version, errors(&last)), (Some(1), vec![DOWN_ERROR]));
    let late = hover(&mut client, &mut seen);
    assert!(!late.contains("had not reached its fixpoint"), "{late}");
    shut_down(client, seen);
}

/// A declaration's documentation — its `//|` and `//<` comments — reaches a
/// real client twice: under the type in a hover on a use, and in the
/// completion item the client resolves when it is focused, read from the
/// declaration's location the item carries (`completionItem/resolve`,
/// advertised). Red with the documentation left out of either, or the
/// resolve request undispatched.
#[test]
fn a_declaration_s_documentation_reaches_hover_and_resolved_completion() {
    const DOCUMENTED: &str = "//| Doubles `x`.\n// Twice, exactly.\ninline uint32 twice(uint32 x)\n{\n    return x + x;\n}\n//< Pure.\n\ninline void main()\n{\n    uint32 y = twice(1);\n    uint32 z = y;\n}\n";
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = Uri::from_str("file:///workspace/documented.k").unwrap();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let init: lsp_types::InitializeResult = result(client.response(&id, &mut seen));
    let resolves = init.capabilities.completion_provider.and_then(|c| c.resolve_provider);
    assert_eq!(resolves, Some(true), "completion must advertise resolve");
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem { uri: uri.clone(), language_id: "kanagawa".to_owned(), version: 1, text: DOCUMENTED.to_owned() },
    });
    let _ = client.diagnostics_for(&uri, &mut seen);

    let hover = hovered(&mut client, &mut seen, &uri, after(DOCUMENTED, "uint32 y = ", "twice"));
    assert!(hover.contains("\n\n---\n\nDoubles `x`.\nTwice, exactly.\n\nPure."), "{hover}");

    let caret = after(DOCUMENTED, "uint32 z = ", "y");
    let id = client.request::<req::Completion>(lsp_types::CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: caret,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: None,
    });
    let offered: Option<lsp_types::CompletionResponse> = result(client.response(&id, &mut seen));
    let Some(lsp_types::CompletionResponse::Array(offered)) = offered else { panic!("completion must answer with an array") };
    let item = offered.into_iter().find(|i| i.label == "twice").expect("`twice` is offered");
    assert!(item.data.is_some(), "the item carries where its declaration is");
    let id = client.request::<req::ResolveCompletionItem>(item);
    let resolved: lsp_types::CompletionItem = result(client.response(&id, &mut seen));
    let documentation = match resolved.documentation {
        Some(lsp_types::Documentation::MarkupContent(m)) => m.value,
        other => panic!("resolved without documentation: {other:?}"),
    };
    assert!(documentation.contains("Doubles `x`.\nTwice, exactly.\n\nPure."), "{documentation}");
    shut_down(client, seen);
}

/// `mode::` completes the enum's constants, as VS Code asks for them — `:`
/// triggers the request — each with its value beside its type and, resolved,
/// its `//<` comment; hover says the value too. A `:` typed alone, as in
/// `public:`, gets no list. Red with the qualified context gone (the scopes'
/// names), with the value left out of the detail, or with a single `:`
/// answered.
#[test]
fn an_enum_qualifier_completes_its_constants_with_their_values() {
    // Comma first, as upstream writes a documented enum (`isa.k`): a `//<`
    // documents the element just before it, and after `pass,` that is no
    // constant.
    const ENUM: &str = "enum mode : uint2\n{ pass   //< Forward it.\n, smooth //< Average it.\n, hold = 3\n}\n\nclass C\n{\npublic:\n    bool f(mode m)\n    {\n        return m == mode::smooth;\n    }\n}\n\nexport C;\n";
    let mut client = Client::start();
    let mut seen = Vec::new();
    let uri = Uri::from_str("file:///workspace/modes.k").unwrap();
    let id = client.request::<req::Initialize>(InitializeParams::default());
    let init: lsp_types::InitializeResult = result(client.response(&id, &mut seen));
    let triggers = init.capabilities.completion_provider.and_then(|c| c.trigger_characters).unwrap_or_default();
    assert!(triggers.iter().any(|t| t == ":"), "`:` must trigger completion: {triggers:?}");
    client.notify::<notif::Initialized>(InitializedParams {});
    client.notify::<notif::DidOpenTextDocument>(DidOpenTextDocumentParams {
        text_document: TextDocumentItem { uri: uri.clone(), language_id: "kanagawa".to_owned(), version: 1, text: ENUM.to_owned() },
    });
    let _ = client.diagnostics_for(&uri, &mut seen);

    let hover = hovered(&mut client, &mut seen, &uri, after(ENUM, "m == mode::", "smooth"));
    assert!(hover.contains("smooth = 1"), "{hover}");

    let typed = ENUM.replace("mode::smooth;", "mode::;");
    client.notify::<notif::DidChangeTextDocument>(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version: 2 },
        content_changes: vec![TextDocumentContentChangeEvent { range: None, range_length: None, text: typed.clone() }],
    });
    let complete = |client: &mut Client, seen: &mut Vec<Notification>, position: Position| {
        let id = client.request::<req::Completion>(lsp_types::CompletionParams {
            text_document_position: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: uri.clone() }, position },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: Some(lsp_types::CompletionContext {
                trigger_kind: lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER,
                trigger_character: Some(":".to_owned()),
            }),
        });
        let offered: Option<lsp_types::CompletionResponse> = result(client.response(&id, seen));
        offered
    };
    let Some(lsp_types::CompletionResponse::Array(offered)) = complete(&mut client, &mut seen, after(&typed, "m == mode::", ";")) else {
        panic!("`mode::` must answer with a list")
    };
    let listed: Vec<(String, String)> =
        offered.iter().map(|i| (i.label.clone(), i.detail.clone().unwrap_or_default())).collect();
    assert_eq!(
        listed.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(),
        ["pass", "smooth", "hold"],
        "{listed:?}"
    );
    for ((_, detail), value) in listed.iter().zip(["= 0", "= 1", "= 3"]) {
        assert!(detail.ends_with(value), "{listed:?}");
    }
    let pass = offered.into_iter().next().expect("`pass`");
    let id = client.request::<req::ResolveCompletionItem>(pass);
    let resolved: lsp_types::CompletionItem = result(client.response(&id, &mut seen));
    let documentation = match resolved.documentation {
        Some(lsp_types::Documentation::MarkupContent(m)) => m.value,
        other => panic!("resolved without documentation: {other:?}"),
    };
    assert!(documentation.contains("Forward it."), "{documentation}");

    let single = complete(&mut client, &mut seen, after(&typed, "public:", "\n"));
    assert!(single.is_none(), "a single `:` gets no list: {single:?}");
    shut_down(client, seen);
}
