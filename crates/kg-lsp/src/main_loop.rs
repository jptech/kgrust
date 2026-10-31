//! The message loop.
//!
//! One thread reads from the [`Connection`] and owns the [`GlobalState`]
//! mutations; a small pool answers requests. The loop must never block on a
//! computation, because the client sends `$/cancelRequest`, `didChange` and
//! `shutdown` down the same stream it sent the slow request down — a server
//! that blocks on a request cannot even be told to stop.
//!
//! Shutdown is handled by hand rather than through
//! `lsp_server::Connection::handle_shutdown`, which blocks for up to thirty
//! seconds waiting for `exit`. The protocol's rule is what matters: after
//! `shutdown` every request is an error, `exit` terminates, and the process
//! exit code is 0 only if `shutdown` came first.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use kg_base::{FileId, FxHashMap, FxHashSet};
use lsp_server::{
    Connection, ErrorCode, ExtractError, Message, Notification, Request, RequestId, Response,
};
use lsp_types::notification::Notification as _;
use lsp_types::request::Request as _;
use lsp_types::{MessageType, Uri, notification as notif, request as req};

use crate::capabilities;
use crate::global_state::{GlobalState, ManifestProblem};
use crate::handlers;
use crate::sync::lock;

/// What the process should exit with.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ExitCode {
    /// `exit` arrived after `shutdown`, as the protocol requires.
    Ok,
    /// `exit` arrived without a preceding `shutdown`, or the stream died.
    Abnormal,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        match self {
            ExitCode::Ok => 0,
            ExitCode::Abnormal => 1,
        }
    }
}

/// Runs the server over stdio. This is what `kg lsp` calls.
pub fn run_server() -> anyhow::Result<ExitCode> {
    let (connection, io_threads) = Connection::stdio();
    let exit = run_server_on(connection);
    io_threads.join()?;
    exit
}

/// Runs the server over any connection, including an in-memory one.
///
/// Split out so the end-to-end test can drive the real loop with
/// `Connection::memory()` and no process, no pipes and no sleeping.
pub fn run_server_on(connection: Connection) -> anyhow::Result<ExitCode> {
    let (id, params) = connection.initialize_start()?;
    let params: lsp_types::InitializeParams = serde_json::from_value(params)?;

    let state = GlobalState::new();
    let encoding = capabilities::offered_encodings(&params.capabilities);
    state.set_encoding(encoding);
    state.set_roots(workspace_roots(&params));

    let result = serde_json::json!({
        "capabilities": capabilities::server_capabilities(encoding),
        "serverInfo": { "name": "kg-lsp", "version": env!("CARGO_PKG_VERSION") },
    });
    connection.initialize_finish(id, result)?;

    crate::set_log_sink(Some(connection.sender.clone()));
    let exit = main_loop(&connection, state);
    crate::set_log_sink(None);
    Ok(exit)
}

fn workspace_roots(params: &lsp_types::InitializeParams) -> Vec<camino::Utf8PathBuf> {
    let mut roots = Vec::new();
    if let Some(folders) = &params.workspace_folders {
        roots.extend(folders.iter().filter_map(|f| crate::uri::uri_to_path(&f.uri)));
    }
    #[allow(deprecated)]
    if roots.is_empty()
        && let Some(uri) = &params.root_uri
    {
        roots.extend(crate::uri::uri_to_path(uri));
    }
    roots
}

/// What every message handler needs, gathered so the dispatch functions take
/// one argument instead of four.
struct Server<'a> {
    connection: &'a Connection,
    state: GlobalState,
    /// Requests.
    pool: TaskPool,
    /// Whole-program diagnostics, one at a time, on a thread of their own.
    ///
    /// Sharing the request pool, a burst of edits started a check per worker
    /// before the next edit could supersede it — measured over stdio, four
    /// concurrent checks of successive versions, and a syntactic
    /// `documentSymbol` queued behind them for half a second. One lane runs
    /// at most one check, and every job queued behind it collapses to the
    /// newest ([`Published`]).
    diagnostics: TaskPool,
    published: Arc<Published>,
    /// The broken manifests the editor was last told of, and what it was
    /// told: a problem is marked and said once, and its mark is taken off
    /// when the manifest loads again ([`report_config_errors`]).
    manifests_shown: Mutex<FxHashMap<camino::Utf8PathBuf, ManifestProblem>>,
}

fn main_loop(connection: &Connection, state: GlobalState) -> ExitCode {
    let server = Server {
        connection,
        state,
        pool: TaskPool::new(connection.sender.clone(), default_threads()),
        diagnostics: TaskPool::new(connection.sender.clone(), 1),
        published: Arc::new(Published::new(connection.sender.clone())),
        manifests_shown: Mutex::new(FxHashMap::default()),
    };
    let mut shutdown_requested = false;

    loop {
        // A plain blocking receive: every slow computation is already on the
        // pool, so the only thing this thread ever waits for is the client.
        let Ok(message) = connection.receiver.recv() else { return ExitCode::Abnormal };

        match message {
            Message::Request(req) => {
                if req.method == req::Shutdown::METHOD {
                    shutdown_requested = true;
                    respond(connection, Response::new_ok(req.id, ()));
                    continue;
                }
                if shutdown_requested {
                    respond(
                        connection,
                        error_response(
                            req.id,
                            ErrorCode::InvalidRequest,
                            "the server has been asked to shut down",
                        ),
                    );
                    continue;
                }
                dispatch_request(&server, req);
            }
            Message::Notification(notification) => {
                if notification.method == notif::Exit::METHOD {
                    return if shutdown_requested { ExitCode::Ok } else { ExitCode::Abnormal };
                }
                handle_notification(&server, notification);
            }
            // The server issues no requests of its own, so any response is a
            // reply to something we did not send.
            Message::Response(_) => {}
        }
    }
}

fn respond(connection: &Connection, response: Response) {
    let _ = connection.sender.send(Message::Response(response));
}

fn error_response(id: RequestId, code: ErrorCode, message: &str) -> Response {
    Response::new_err(id, code as i32, message.to_owned())
}

/// Runs one request on the pool.
///
/// The macro exists to avoid writing the same eight lines per method: extract
/// typed params, hand the handler a state clone, serialize whatever comes back.
/// A [`crate::NoFile`] answer becomes `null`, which every client understands as
/// "nothing for that document".
macro_rules! on {
    ($server:expr, $req:expr, $kind:ty, $handler:path) => {{
        // `extract` consumes the request and only hands it back on a method
        // mismatch, so the id has to be saved before the call.
        let id = $req.id.clone();
        match $req.extract::<<$kind as lsp_types::request::Request>::Params>(<$kind>::METHOD) {
            Ok((id, params)) => {
                let state = $server.state.clone();
                $server.pool.spawn(id, <$kind>::METHOD, move |id| match $handler(&state, &params) {
                    Ok(result) => Response::new_ok(id, result),
                    // "I have nothing for that document" is a normal answer,
                    // not a protocol error.
                    Err(_) => Response::new_ok(id, serde_json::Value::Null),
                });
                return;
            }
            Err(ExtractError::MethodMismatch(back)) => $req = back,
            Err(ExtractError::JsonError { method, error }) => {
                let message = format!("{method}: {error}");
                respond($server.connection, error_response(id, ErrorCode::InvalidParams, &message));
                return;
            }
        }
    }};
}

/// [`on!`] for a handler whose failure may carry a sentence for the user —
/// [`crate::handlers::rename::Refusal::Because`] — which is answered as a
/// request error so the client shows it. A `NoFile` is `null` as everywhere.
macro_rules! on_verdict {
    ($server:expr, $req:expr, $kind:ty, $handler:path) => {{
        let id = $req.id.clone();
        match $req.extract::<<$kind as lsp_types::request::Request>::Params>(<$kind>::METHOD) {
            Ok((id, params)) => {
                let state = $server.state.clone();
                $server.pool.spawn(id, <$kind>::METHOD, move |id| match $handler(&state, &params) {
                    Ok(result) => Response::new_ok(id, result),
                    Err(handlers::rename::Refusal::NoFile(_)) => {
                        Response::new_ok(id, serde_json::Value::Null)
                    }
                    Err(handlers::rename::Refusal::Because(message)) => {
                        error_response(id, ErrorCode::RequestFailed, &message)
                    }
                });
                return;
            }
            Err(ExtractError::MethodMismatch(back)) => $req = back,
            Err(ExtractError::JsonError { method, error }) => {
                let message = format!("{method}: {error}");
                respond($server.connection, error_response(id, ErrorCode::InvalidParams, &message));
                return;
            }
        }
    }};
}

fn dispatch_request(server: &Server<'_>, mut req: Request) {
    on!(server, req, req::DocumentSymbolRequest, handlers::document_symbol);
    on!(server, req, req::FoldingRangeRequest, handlers::folding_range);
    on!(server, req, req::SelectionRangeRequest, handlers::selection_range);
    on!(server, req, req::SemanticTokensFullRequest, handlers::semantic_tokens_full);
    on!(server, req, req::SemanticTokensRangeRequest, handlers::semantic_tokens_range);
    on!(server, req, req::InlayHintRequest, handlers::inlay_hints::inlay_hints);
    on!(server, req, req::DocumentHighlightRequest, handlers::document_highlight::document_highlight);
    on!(server, req, req::GotoDefinition, handlers::definition::definition);
    on!(server, req, req::GotoTypeDefinition, handlers::definition::type_definition);
    on!(server, req, req::Completion, handlers::completion::completion);
    on!(server, req, req::ResolveCompletionItem, handlers::completion::completion_resolve);
    on!(server, req, req::HoverRequest, handlers::hover::hover);
    on!(server, req, req::References, handlers::references::references);
    on_verdict!(server, req, req::PrepareRenameRequest, handlers::rename::prepare_rename);
    on_verdict!(server, req, req::Rename, handlers::rename::rename);
    on!(server, req, req::SignatureHelpRequest, handlers::signature_help::signature_help);
    on!(server, req, req::CodeActionRequest, handlers::code_action::code_action);
    on!(server, req, req::WorkspaceSymbolRequest, handlers::workspace_symbol::workspace_symbol);
    on!(server, req, req::Formatting, handlers::formatting);
    on!(server, req, req::RangeFormatting, handlers::range_formatting);

    let message = format!("unhandled method {}", req.method);
    respond(server.connection, error_response(req.id, ErrorCode::MethodNotFound, &message));
}

fn handle_notification(server: &Server<'_>, mut notification: Notification) {
    let state = &server.state;
    macro_rules! on_notification {
        ($kind:ty, |$params:ident| $body:expr) => {
            match notification
                .extract::<<$kind as lsp_types::notification::Notification>::Params>(
                    <$kind>::METHOD,
                ) {
                Ok($params) => {
                    $body;
                    return;
                }
                Err(ExtractError::MethodMismatch(back)) => notification = back,
                Err(ExtractError::JsonError { method, error }) => {
                    crate::log(MessageType::ERROR, format!("{method}: {error}"));
                    return;
                }
            }
        };
    }

    on_notification!(notif::DidOpenTextDocument, |params| {
        let doc = params.text_document;
        state.did_open(doc.uri.clone(), doc.text, doc.version);
        // After `publish`, which resolves the file's settings: a manifest
        // that fails to load, or none at all, is known only then.
        publish(server, doc.uri, Trigger::Edit);
        report_config_errors(server);
    });

    on_notification!(notif::DidChangeTextDocument, |params| {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let Some(file) = state.file_id(&uri) else { return };
        for change in params.content_changes {
            let Some(text) = state.apply_edit(file, change.range, &change.text) else { return };
            state.did_change_full(&uri, text, version);
        }
        publish(server, uri, Trigger::Edit);
    });

    on_notification!(notif::DidSaveTextDocument, |params| {
        publish(server, params.text_document.uri, Trigger::Save);
    });

    on_notification!(notif::DidCloseTextDocument, |params| {
        let uri = params.text_document.uri;
        let file = state.file_id(&uri);
        state.did_close(&uri);
        // A closed file must not keep its diagnostics on screen — nor have
        // them put back by a job that was already running.
        server.published.clear(file, &uri);
    });

    on_notification!(notif::DidChangeConfiguration, |_params| {
        reload_and_republish(server);
    });

    // The client watches `kanagawa.toml` and `**/*.k` for us. Editing
    // `import-dirs` or `target-device` changes the import search path, and
    // therefore what every open file means; a source changed on disk — a
    // branch switched, a pull — changes what an import of it reads. Both
    // take effect without a restart.
    on_notification!(notif::DidChangeWatchedFiles, |params| {
        let mut touched_manifest = false;
        let mut touched_source = false;
        for change in &params.changes {
            let Some(path) = crate::uri::uri_to_path(&change.uri) else { continue };
            if path.file_name() == Some(kg_config::MANIFEST_NAME) {
                touched_manifest = true;
            } else if path.extension() == Some("k") {
                server.state.disk_changed(&change.uri);
                touched_source = true;
            }
        }
        if touched_manifest {
            reload_and_republish(server);
        } else if touched_source {
            for uri in server.state.open_documents().into_iter().map(|d| d.uri) {
                publish(server, uri, Trigger::Edit);
            }
        }
    });

    // A request no worker has started is answered here; one running is asked
    // to stop at its next checkpoint and its worker answers it (sprint W7.2).
    on_notification!(notif::Cancel, |params| {
        let id = match params.id {
            lsp_types::NumberOrString::Number(n) => RequestId::from(n),
            lsp_types::NumberOrString::String(s) => RequestId::from(s),
        };
        if server.pool.cancel(&id) {
            respond(
                server.connection,
                error_response(id, ErrorCode::RequestCanceled, "cancelled before it started"),
            );
        }
    });

    // `initialized` and `$/setTrace` land here, and need nothing.
    let _ = notification;
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Trigger {
    Edit,
    Save,
}

/// Drops every cached manifest and every database built from one, and reports
/// each open file again under the new settings.
fn reload_and_republish(server: &Server<'_>) {
    server.state.reload_config();
    for uri in server.state.open_documents().into_iter().map(|d| d.uri) {
        publish(server, uri, Trigger::Edit);
    }
    // After the files, as at `didOpen`: a manifest is read again only when a
    // file under it asks for its settings, so a broken one is known only now.
    report_config_errors(server);
}

/// Publishes diagnostics for `uri`, honouring `[lsp] diagnostics` from
/// `kanagawa.toml`: `on-type` (the default) republishes on every keystroke,
/// `on-save` only when the file is written.
///
/// Whether to publish is decided here, on the main loop, and not on the
/// worker: a job that is scheduled supersedes every older one for the same
/// document ([`Published`]), so a job that might then decide to publish
/// nothing — an `on-save` edit behind a save — would swallow the save's.
///
/// Two jobs share the ticket: the syntax errors on the request pool, which
/// take a parse, and the whole-program check on the diagnostics lane, which
/// takes the check. A syntax error the user just typed shows at once instead
/// of behind the check already running.
///
/// The check publishes twice where its loop runs past the editor's first
/// bound (sprint W7.6): at four rounds, as soon as the run gets there, and
/// at the fixpoint, `kg check`'s answer. An edit while the fixpoint runs on
/// cancels it — the database's token, raised as the edit is queued — and
/// the first answer stays on screen until the new ticket's first replaces
/// it.
fn publish(server: &Server<'_>, uri: Uri, trigger: Trigger) {
    let state = &server.state;
    let Some(file) = state.file_id(&uri) else { return };
    let wanted = match state.settings_for(file).lsp.diagnostics {
        kg_config::DiagnosticTiming::OnType => true,
        kg_config::DiagnosticTiming::OnSave => trigger == Trigger::Save,
    };
    if !wanted {
        return;
    }
    let ticket = server.published.schedule(file);
    {
        let state = state.clone();
        let published = Arc::clone(&server.published);
        let uri = uri.clone();
        server.pool.spawn_notification(move || {
            if !published.is_latest(file, ticket) {
                return;
            }
            if let Some(params) = handlers::parse_diagnostics(&state, &uri) {
                published.send_if_latest(file, ticket, Tier::Parse, params);
            }
        });
    }
    let state = state.clone();
    let published = Arc::clone(&server.published);
    server.diagnostics.spawn_notification(move || {
        // Coalescing: a newer job for this document is queued behind this
        // one, and will answer for newer text. Typing faster than a check
        // runs therefore costs one check per pause, not one per keystroke.
        if !published.is_latest(file, ticket) {
            return;
        }
        let first = |params| published.send_if_latest(file, ticket, Tier::First, params);
        let params = match handlers::publish_diagnostics(&state, &uri, first) {
            Ok(params) => params,
            // Too large, or closed while the task was queued: clear rather than
            // leave stale errors on screen.
            Err(_) => handlers::cleared_diagnostics(&uri),
        };
        published.send_if_latest(file, ticket, Tier::Full, params);
    });
}

/// Logs each manifest error once, and shows the user each broken manifest:
/// a diagnostic on the manifest, at the value `toml` refused, and a message
/// once. A log alone was not enough: a manifest refused whole leaves its
/// files on the defaults, so `edition = "9.9"` read as edition 1.0, and the
/// user saw only the parse errors of a 1.1 program with nothing naming why.
fn report_config_errors(server: &Server<'_>) {
    let state = &server.state;
    for message in state.take_config_errors() {
        crate::log(MessageType::WARNING, format!("kanagawa.toml: {message}"));
    }
    let problems: FxHashMap<_, _> = state.manifest_problems().into_iter().collect();
    let mut shown = lock(&server.manifests_shown);
    for path in shown.keys().filter(|p| !problems.contains_key(*p)) {
        if let Some(uri) = crate::uri::path_to_uri(path) {
            send_diagnostics(&server.connection.sender, handlers::cleared_diagnostics(&uri));
        }
    }
    for (path, problem) in &problems {
        if shown.get(path) == Some(problem) {
            continue;
        }
        let Some(uri) = crate::uri::path_to_uri(path) else { continue };
        let range = problem.span.as_ref().map_or_else(lsp_types::Range::default, |(span, text)| {
            let index = kg_base::LineIndex::new(text);
            let span = kg_base::TextRange::new(
                kg_base::TextSize::new(span.start as u32),
                kg_base::TextSize::new(span.end as u32),
            );
            crate::convert::range(&index, state.encoding(), span)
        });
        let diagnostic = lsp_types::Diagnostic {
            range,
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            source: Some("kg".to_owned()),
            message: problem.message.clone(),
            ..lsp_types::Diagnostic::default()
        };
        send_diagnostics(
            &server.connection.sender,
            lsp_types::PublishDiagnosticsParams { uri, diagnostics: vec![diagnostic], version: None },
        );
        let consequence = if problem.refused {
            " Until it is fixed, the files under it are read as if it were not there, in edition 1.0."
        } else {
            ""
        };
        let params = lsp_types::ShowMessageParams {
            typ: MessageType::WARNING,
            message: format!("{path}: {}.{consequence}", problem.message.trim_end_matches('.')),
        };
        let _ = server.connection.sender.send(Message::Notification(Notification::new(
            notif::ShowMessage::METHOD.to_owned(),
            params,
        )));
    }
    *shown = problems;
}

/// The order `publishDiagnostics` reaches the client in.
///
/// Each diagnostics job takes a ticket when the main loop schedules it, and a
/// document's newest ticket is the only one allowed to run or to publish. The
/// check and the send happen under one lock, and so does taking a ticket, so
/// once a newer job exists nothing older can reach the client after it — two
/// workers finishing out of order cannot put an old version's errors back on
/// screen over a new one's.
///
/// Within one ticket, a tier publishes only above the highest already sent,
/// so the syntax-only answer can precede the full one and never follow it.
#[derive(Debug)]
struct Published {
    sender: Sender<Message>,
    tickets: Mutex<Tickets>,
}

#[derive(Debug, Default)]
struct Tickets {
    next: u64,
    /// The newest ticket per open document, and the highest tier sent under
    /// it. A closed document has none, so every job still in flight for it
    /// is stale.
    latest: FxHashMap<FileId, (u64, Option<Tier>)>,
}

/// How much of the answer a publication carries. Ordered: a later tier
/// supersedes an earlier one for the same ticket.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Tier {
    /// Syntax errors only, sent only when there are some.
    Parse,
    /// The whole-program check at the loop's first bound, four rounds,
    /// sent only where the loop runs on past it (`kg_db::PassBudget::
    /// Layered`): it may miss a diagnostic the fixpoint shows, never show
    /// one the fixpoint does not.
    First,
    /// The whole-program check at the loop's fixpoint — `kg check`'s.
    Full,
}

impl Published {
    fn new(sender: Sender<Message>) -> Published {
        Published { sender, tickets: Mutex::new(Tickets::default()) }
    }

    fn schedule(&self, file: FileId) -> u64 {
        let mut tickets = lock(&self.tickets);
        tickets.next += 1;
        let ticket = tickets.next;
        tickets.latest.insert(file, (ticket, None));
        ticket
    }

    fn is_latest(&self, file: FileId, ticket: u64) -> bool {
        lock(&self.tickets).latest.get(&file).is_some_and(|&(t, _)| t == ticket)
    }

    fn send_if_latest(
        &self,
        file: FileId,
        ticket: u64,
        tier: Tier,
        params: lsp_types::PublishDiagnosticsParams,
    ) {
        let mut tickets = lock(&self.tickets);
        if let Some((latest, sent)) = tickets.latest.get_mut(&file)
            && *latest == ticket
            && *sent < Some(tier)
        {
            *sent = Some(tier);
            send_diagnostics(&self.sender, params);
        }
    }

    /// Clears a closed document's diagnostics, and retires every job still
    /// running for it.
    fn clear(&self, file: Option<FileId>, uri: &Uri) {
        let mut tickets = lock(&self.tickets);
        if let Some(file) = file {
            tickets.latest.remove(&file);
        }
        send_diagnostics(&self.sender, handlers::cleared_diagnostics(uri));
    }
}

fn send_diagnostics(sender: &Sender<Message>, params: lsp_types::PublishDiagnosticsParams) {
    let _ = sender.send(Message::Notification(Notification::new(
        lsp_types::notification::PublishDiagnostics::METHOD.to_owned(),
        params,
    )));
}

fn default_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(2, 4)
}

/// A fixed pool of worker threads.
///
/// Small on purpose: the work here is parsing and tree walking, and the point
/// of the pool is to keep the *main loop* free, not to parallelize one request.
///
/// A job that panics costs its own answer and nothing else: a request is
/// answered `InternalError`, a notification job is dropped, and the worker
/// takes the next job. Without that, one panicking handler kills a worker
/// thread per occurrence and the server goes silent once they are gone.
struct TaskPool {
    jobs: Sender<Job>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// Requests queued and not yet started. Whoever removes an id first —
    /// the worker starting it, or `$/cancelRequest` — decides whether it is
    /// answered by its handler or as cancelled, so it is answered exactly once.
    queued: Arc<Mutex<FxHashSet<RequestId>>>,
    /// Requests running, each with the token `$/cancelRequest` raises: its
    /// query stops at its next checkpoint and the worker answers it as
    /// cancelled (`kg_base::cancel`, sprint W7.2).
    running: Arc<Mutex<FxHashMap<RequestId, kg_base::cancel::Cancel>>>,
}

type Job = Box<dyn FnOnce() -> Option<Message> + Send + 'static>;

impl std::fmt::Debug for TaskPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskPool").field("workers", &self.workers.len()).finish()
    }
}

impl TaskPool {
    fn new(sender: Sender<Message>, threads: usize) -> TaskPool {
        let (jobs, rx) = crossbeam_channel::unbounded::<Job>();
        let rx = Arc::new(rx);

        let workers = (0..threads)
            .map(|i| {
                let rx: Arc<Receiver<Job>> = Arc::clone(&rx);
                let sender = sender.clone();
                std::thread::Builder::new()
                    .name(format!("kg-lsp-worker-{i}"))
                    .spawn(move || {
                        while let Ok(job) = rx.recv() {
                            let message = match catch_unwind(AssertUnwindSafe(job)) {
                                Ok(message) => message,
                                // A diagnostics job an edit superseded: the
                                // newer job answers for the newer text.
                                Err(payload) if payload.is::<kg_base::cancel::Cancelled>() => continue,
                                // The panic hook has already written the message
                                // and location to stderr, which the editor keeps.
                                Err(_) => {
                                    crate::log(MessageType::ERROR, "a background job panicked");
                                    continue;
                                }
                            };
                            if let Some(message) = message
                                && sender.send(message).is_err()
                            {
                                break;
                            }
                        }
                    })
                    .expect("spawning a worker thread")
            })
            .collect();

        TaskPool { jobs, workers, queued: Arc::default(), running: Arc::default() }
    }

    fn spawn(
        &self,
        id: RequestId,
        method: &'static str,
        job: impl FnOnce(RequestId) -> Response + Send + 'static,
    ) {
        lock(&self.queued).insert(id.clone());
        let queued = Arc::clone(&self.queued);
        let running = Arc::clone(&self.running);
        let _ = self.jobs.send(Box::new(move || {
            // Started under the queue's lock, so `cancel` finds the request
            // in exactly one of the two maps.
            let token = kg_base::cancel::Cancel::new();
            {
                let mut q = lock(&queued);
                if !q.remove(&id) {
                    // Cancelled while queued, and already answered as such.
                    return None;
                }
                lock(&running).insert(id.clone(), token.clone());
            }
            let outcome = catch_unwind(AssertUnwindSafe(|| kg_base::cancel::scope(&token, || job(id.clone()))));
            lock(&running).remove(&id);
            let response = match outcome {
                Ok(response) => response,
                // Its own `$/cancelRequest`, or an edit to what it asked about.
                Err(payload) if payload.is::<kg_base::cancel::Cancelled>() => {
                    if token.is_raised() {
                        error_response(id, ErrorCode::RequestCanceled, "cancelled")
                    } else {
                        error_response(id, ErrorCode::ContentModified, "the document changed while it was answered")
                    }
                }
                Err(payload) => {
                    let message = format!("{method} panicked: {}", panic_message(&*payload));
                    crate::log(MessageType::ERROR, message.clone());
                    error_response(id, ErrorCode::InternalError, &message)
                }
            };
            Some(Message::Response(response))
        }));
    }

    /// Withdraws a queued request. True if it had not started — and then the
    /// caller must answer it, because its worker now will not. One already
    /// running is asked to stop at its next checkpoint, and its worker
    /// answers it as cancelled.
    fn cancel(&self, id: &RequestId) -> bool {
        let queued = lock(&self.queued).remove(id);
        if !queued && let Some(token) = lock(&self.running).get(id) {
            token.raise();
        }
        queued
    }

    fn spawn_notification(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.jobs.send(Box::new(move || {
            job();
            None
        }));
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else {
        "a panic with no message"
    }
}

impl Drop for TaskPool {
    fn drop(&mut self) {
        // Closing the queue is what tells the workers to stop; joining them
        // guarantees no response is written after the loop has returned.
        let (dead, _) = crossbeam_channel::bounded(0);
        let _ = std::mem::replace(&mut self.jobs, dead);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_protocol() {
        assert_eq!(ExitCode::Ok.code(), 0);
        assert_eq!(ExitCode::Abnormal.code(), 1);
    }

    #[test]
    fn workspace_folders_win_over_the_deprecated_root_uri() {
        let folder = lsp_types::WorkspaceFolder {
            uri: crate::uri::path_to_uri(camino::Utf8Path::new("/work/design")).unwrap(),
            name: "design".to_owned(),
        };
        let params = lsp_types::InitializeParams {
            workspace_folders: Some(vec![folder]),
            ..lsp_types::InitializeParams::default()
        };
        assert_eq!(workspace_roots(&params), vec![camino::Utf8PathBuf::from("/work/design")]);
    }

    fn next_response(rx: &Receiver<Message>) -> Response {
        match rx.recv_timeout(std::time::Duration::from_secs(60)).expect("a response") {
            Message::Response(response) => response,
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn the_pool_answers_and_shuts_down_cleanly() {
        let (tx, rx) = crossbeam_channel::unbounded();
        {
            let pool = TaskPool::new(tx, 2);
            pool.spawn(RequestId::from(7), "test/done", |id| Response::new_ok(id, "done"));
        }
        let response = next_response(&rx);
        assert_eq!(response.id, RequestId::from(7));
    }

    /// One worker, so the request after the panic can only be answered if
    /// that same thread survived it.
    #[test]
    fn a_panicking_request_is_an_internal_error_and_the_worker_lives_on() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let pool = TaskPool::new(tx, 1);
        pool.spawn(RequestId::from(1), "test/panic", |_| panic!("a handler bug"));
        pool.spawn_notification(|| panic!("a diagnostics job bug"));
        pool.spawn(RequestId::from(2), "test/after", |id| Response::new_ok(id, "alive"));

        let failed = next_response(&rx);
        assert_eq!(failed.id, RequestId::from(1));
        let error = failed.response_result.expect_err("an error response");
        assert_eq!(error.code, ErrorCode::InternalError as i32);
        assert!(error.message.contains("test/panic"), "{}", error.message);
        assert!(error.message.contains("a handler bug"), "{}", error.message);

        // The panicking notification job said nothing; the next thing on the
        // wire is the answer to the request queued behind it.
        let next = next_response(&rx);
        assert_eq!(next.id, RequestId::from(2));
        assert_eq!(next.response_result.ok(), Some(serde_json::json!("alive")));
    }

    /// A request cancelled while it runs stops at its next checkpoint and is
    /// answered `RequestCanceled` by its worker; one whose query an edit
    /// cancelled is answered `ContentModified` (sprint W7.2). Red with the
    /// running token left unraised, or with the two causes answered alike.
    #[test]
    fn a_running_request_stops_at_its_next_checkpoint() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let pool = TaskPool::new(tx, 1);
        let (started, wait_started) = crossbeam_channel::bounded::<()>(0);
        pool.spawn(RequestId::from(1), "test/long", move |id| {
            let _ = started.send(());
            // A query's loop, polling the token this thread's request
            // carries — bounded, so a token never raised fails the test
            // rather than hanging the pool's join.
            for _ in 0..10_000 {
                kg_base::cancel::Cancel::never().check();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Response::new_ok(id, "ran to the end")
        });
        wait_started.recv().expect("the job started");
        assert!(!pool.cancel(&RequestId::from(1)), "a running request is answered by its worker");
        let cancelled = next_response(&rx);
        assert_eq!(cancelled.id, RequestId::from(1));
        assert_eq!(cancelled.response_result.expect_err("cancelled").code, ErrorCode::RequestCanceled as i32);

        let edited = kg_base::cancel::Cancel::new();
        edited.raise();
        pool.spawn(RequestId::from(2), "test/edited", move |_| {
            edited.check();
            unreachable!("the edit cancelled it")
        });
        let modified = next_response(&rx);
        assert_eq!(modified.response_result.expect_err("cancelled").code, ErrorCode::ContentModified as i32);
    }

    /// A request withdrawn before a worker reached it is not run, and is not
    /// answered by the pool — the main loop answers it, once.
    #[test]
    fn a_cancelled_request_that_had_not_started_is_not_run() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let pool = TaskPool::new(tx, 1);
        // Hold the only worker so the next two stay queued.
        let (release, wait) = crossbeam_channel::bounded::<()>(0);
        pool.spawn(RequestId::from(1), "test/block", move |id| {
            let _ = wait.recv();
            Response::new_ok(id, "first")
        });
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let witness = Arc::clone(&ran);
        pool.spawn(RequestId::from(2), "test/cancelled", move |id| {
            witness.store(true, std::sync::atomic::Ordering::SeqCst);
            Response::new_ok(id, "should not run")
        });
        pool.spawn(RequestId::from(3), "test/after", |id| Response::new_ok(id, "third"));

        assert!(pool.cancel(&RequestId::from(2)), "a queued request could not be withdrawn");
        assert!(!pool.cancel(&RequestId::from(2)), "withdrawn twice");
        release.send(()).unwrap();

        assert_eq!(next_response(&rx).id, RequestId::from(1));
        assert_eq!(next_response(&rx).id, RequestId::from(3));
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst), "the cancelled handler ran");
        // A request that has started can no longer be withdrawn.
        assert!(!pool.cancel(&RequestId::from(1)));
    }

    fn diagnostics(message: Message) -> lsp_types::PublishDiagnosticsParams {
        match message {
            Message::Notification(n) => serde_json::from_value(n.params).unwrap(),
            other => panic!("expected a notification, got {other:?}"),
        }
    }

    fn params(uri: &Uri, version: i32) -> lsp_types::PublishDiagnosticsParams {
        lsp_types::PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics: Vec::new(),
            version: Some(version),
        }
    }

    /// Two jobs for one document finishing in the wrong order: the older
    /// one's result must not reach the client at all.
    #[test]
    fn an_older_diagnostics_job_never_publishes_over_a_newer_one() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let published = Published::new(tx);
        let uri = crate::global_state::test_uri("top.k");
        let file = FileId(0);

        let old = published.schedule(file);
        let new = published.schedule(file);
        assert!(!published.is_latest(file, old), "a superseded job would still run");
        published.send_if_latest(file, new, Tier::Full, params(&uri, 2));
        published.send_if_latest(file, old, Tier::Full, params(&uri, 1));

        assert_eq!(diagnostics(rx.try_recv().unwrap()).version, Some(2));
        assert!(rx.try_recv().is_err(), "the older version was published after the newer");
    }

    /// The loop's first answer sits between the syntax errors and the
    /// fixpoint's: after either it is stale, and a newer ticket's first
    /// answer supersedes everything the older one sent (sprint W7.6). Red
    /// with `First` ordered after `Full`, or before `Parse`.
    #[test]
    fn the_first_answer_publishes_between_the_parse_tier_and_the_full_one() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let published = Published::new(tx);
        let uri = crate::global_state::test_uri("top.k");
        let file = FileId(0);

        let one = published.schedule(file);
        published.send_if_latest(file, one, Tier::Parse, params(&uri, 1));
        published.send_if_latest(file, one, Tier::First, params(&uri, 1));
        published.send_if_latest(file, one, Tier::Full, params(&uri, 1));
        assert_eq!(rx.try_iter().count(), 3, "parse, first, then full all publish");

        let two = published.schedule(file);
        published.send_if_latest(file, two, Tier::First, params(&uri, 2));
        published.send_if_latest(file, two, Tier::Parse, params(&uri, 2));
        assert_eq!(rx.try_iter().count(), 1, "a parse tier after the first answer was published");
        published.send_if_latest(file, two, Tier::Full, params(&uri, 2));
        published.send_if_latest(file, two, Tier::First, params(&uri, 2));
        assert_eq!(rx.try_iter().count(), 1, "the full answer, and nothing after it");

        // An edit while the fixpoint runs: the old ticket's last answer, had
        // it not been cancelled, is not sent over the new ticket's first.
        let three = published.schedule(file);
        let four = published.schedule(file);
        published.send_if_latest(file, four, Tier::First, params(&uri, 4));
        published.send_if_latest(file, three, Tier::Full, params(&uri, 3));
        let sent: Vec<Option<i32>> = rx.try_iter().map(|m| diagnostics(m).version).collect();
        assert_eq!(sent, [Some(4)]);
    }

    /// The syntax-only answer may precede the full one for its version and
    /// must never follow it: arriving second, it would erase the load errors
    /// the full answer added.
    #[test]
    fn the_parse_tier_never_publishes_after_the_full_answer() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let published = Published::new(tx);
        let uri = crate::global_state::test_uri("top.k");
        let file = FileId(0);

        let first = published.schedule(file);
        published.send_if_latest(file, first, Tier::Parse, params(&uri, 1));
        published.send_if_latest(file, first, Tier::Full, params(&uri, 1));
        assert_eq!(rx.try_iter().count(), 2, "parse then full both publish");

        let second = published.schedule(file);
        published.send_if_latest(file, second, Tier::Full, params(&uri, 2));
        published.send_if_latest(file, second, Tier::Parse, params(&uri, 2));
        assert_eq!(rx.try_iter().count(), 1, "a parse tier after the full answer was published");
    }

    /// Closing a document retires the job still computing for it, so its
    /// errors do not reappear after the clear.
    #[test]
    fn a_job_for_a_closed_document_publishes_nothing() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let published = Published::new(tx);
        let uri = crate::global_state::test_uri("top.k");
        let file = FileId(0);

        let running = published.schedule(file);
        published.clear(Some(file), &uri);
        published.send_if_latest(file, running, Tier::Full, params(&uri, 1));

        let cleared = diagnostics(rx.try_recv().unwrap());
        assert!(cleared.diagnostics.is_empty() && cleared.version.is_none());
        assert!(rx.try_recv().is_err(), "a closed document's diagnostics came back");
    }
}
