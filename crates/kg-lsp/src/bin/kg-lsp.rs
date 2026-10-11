//! A standalone `kg-lsp` binary.
//!
//! The shipping entry point is `kg lsp`, which calls [`kg_lsp::run_server`]
//! directly. This binary exists so the server is runnable and debuggable before
//! `kg-cli` lands, and so an editor can be pointed straight at it — the VS Code
//! client's `kanagawa.server.path` takes either.
//!
//! stdout is the protocol stream. Nothing may be printed to it.

/// `kg lsp`'s allocator, tuned as `kg lsp` tunes it: this binary ran on
/// the system's until 2026-10-10, whose footprint grows ~90 MB a
/// keystroke on `risc-v.k` ([`kg_alloc::Allocator`]).
#[global_allocator]
static ALLOCATOR: kg_alloc::Allocator = kg_alloc::Allocator;

fn main() -> std::process::ExitCode {
    kg_alloc::purge_eagerly();
    match kg_lsp::run_server() {
        Ok(exit) => std::process::ExitCode::from(exit.code() as u8),
        Err(err) => {
            eprintln!("kg-lsp: {err}");
            std::process::ExitCode::from(1)
        }
    }
}
