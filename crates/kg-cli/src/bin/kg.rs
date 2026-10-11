//! `kg` — the Kanagawa toolchain.
//!
//! Installed (or symlinked) as `kanagawa`, it dispatches to the compat front
//! end instead, so a drop-in replacement needs no wrapper script and the
//! recorded command line still starts with the name the caller used.

use std::io::Write;

/// mimalloc, for the reasons [`kg_alloc::Allocator`] gives; `kg lsp`
/// also purges eagerly ([`kg_alloc::purge_eagerly`]).
#[global_allocator]
static ALLOCATOR: kg_alloc::Allocator = kg_alloc::Allocator;

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    if let Some(code) = kg_cli::backend_worker(&argv) {
        return std::process::ExitCode::from(u8::try_from(code).unwrap_or(2));
    }
    // Handles, **not** locks. `kg lsp` hands the process's stdio to
    // `lsp-server`, whose writer thread locks stdout itself — and `StdoutLock`
    // is reentrant per *thread*, so a lock held here across dispatch deadlocks
    // that thread instead of failing. The symptom is a language server that
    // answers nothing and never exits, which looks like a hung editor rather
    // than like a bug here.
    //
    // The cost is a lock acquisition per write instead of one for the process.
    // For a command that prints diagnostics that is not measurable; for `lsp`,
    // which writes nothing to stdout itself, it is free.
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();

    let code = if kg_cli::dispatch_on_argv0(&argv) {
        kg_cli::legacy::run(&argv, &mut out, &mut err)
    } else {
        kg_cli::modern::run(&argv, &mut out, &mut err)
    };

    let _ = out.flush();
    let _ = err.flush();
    std::process::ExitCode::from(u8::try_from(code).unwrap_or(2))
}
