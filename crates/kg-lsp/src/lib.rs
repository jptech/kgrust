//! The Kanagawa language server.
//!
//! The reference toolchain has none, and cannot have one: its parser calls
//! `findSymbol` while parsing (`Parser.hs:668-680`), so it cannot recover from
//! an error, and a language server spends its whole life looking at files that
//! do not parse. `kgrust` inverts that — `kg-parser` has no symbol table and
//! `kg-syntax` produces a lossless tree for *any* input — and this crate is the
//! first thing to cash that in.
//!
//! ```text
//!   stdin ──▶ lsp-server::Connection ──▶ main loop ──▶ GlobalState (Vfs + Settings + parse cache)
//!                     ▲                     │                │
//!                     └── responses ────────┴── task pool ───┘   handlers: pure fns of (&state, params)
//! ```
//!
//! Three shapes are load-bearing:
//!
//! * **`lsp-server`, not `tower-lsp`.** LSP is a pair of byte streams, not a
//!   request/response protocol. Diagnostics are *pushed*; nobody asks for them.
//! * **The VFS overlay map is the document lifecycle.** `didOpen` is
//!   `set_overlay`, `didClose` is `remove_overlay`, and every query below reads
//!   through the same [`kg_vfs::Vfs`] the batch compiler does — so an unsaved
//!   buffer and a file on disk are the same thing to everything downstream.
//! * **Handlers never touch the connection.** They take `&GlobalState` and
//!   return a value, which is why every feature in this crate has a unit test
//!   with no server running.
//!
//! There are now two tiers of feature here, and they degrade differently:
//!
//! * **syntactic** — outline, folding, selection, semantic tokens, document
//!   and range formatting. Decidable from the CST alone, so they answer for a
//!   file that does not parse and for a project with no `kanagawa.toml`;
//! * **semantic** — diagnostics, go to definition and type definition, hover
//!   (with the declaration's documentation), find-references, document
//!   highlight, inlay hints, completion, signature help, rename, quick-fix
//!   code actions and workspace symbols. Answered by [`kg_db::Db`] through the
//!   single `GlobalState::with_db` seam, so a jump, a tooltip and the gutter
//!   are readings of one program rather than three opinions about it. Each
//!   declines — `null`, no hint, no highlight — wherever the frontend is
//!   incomplete, because a plausible wrong answer in an editor is worse than a
//!   visible absent one.
//!
//! Nothing is advertised unless it is implemented; [`capabilities`] states that
//! rule and is where it is enforced.

pub mod angles;
pub mod capabilities;
pub mod convert;
pub mod fmt;
pub mod global_state;
pub mod handlers;
mod main_loop;
mod sync;
pub mod uri;

use std::sync::{Mutex, OnceLock};

use crossbeam_channel::Sender;
use lsp_server::{Message, Notification};
use lsp_types::MessageType;

pub use convert::PositionEncoding;
pub use global_state::{Document, FileSnapshot, GlobalState, NoFile};
pub use main_loop::{ExitCode, run_server, run_server_on};

/// Where `window/logMessage` notifications go, once a connection exists.
///
/// A global because logging must work from anywhere — including from
/// [`fmt::format`], which is several layers below anything that has a
/// connection handle — and because there is exactly one connection per process.
static LOG_SINK: OnceLock<Mutex<Option<Sender<Message>>>> = OnceLock::new();

fn log_sink() -> &'static Mutex<Option<Sender<Message>>> {
    LOG_SINK.get_or_init(|| Mutex::new(None))
}

pub(crate) fn set_log_sink(sender: Option<Sender<Message>>) {
    *sync::lock(log_sink()) = sender;
}

/// Sends a `window/logMessage`, falling back to stderr.
///
/// stderr is the correct fallback and not a lazy one: stdout is the protocol
/// stream, so anything written there corrupts the session, while an editor
/// captures the server's stderr into its own output pane.
pub(crate) fn log(ty: MessageType, message: impl Into<String>) {
    let message = message.into();
    let sender = sync::lock(log_sink()).clone();
    match sender {
        Some(sender) => {
            let params = lsp_types::LogMessageParams { typ: ty, message };
            let _ = sender.send(Message::Notification(Notification::new(
                "window/logMessage".to_owned(),
                params,
            )));
        }
        None => eprintln!("[kg-lsp] {message}"),
    }
}
