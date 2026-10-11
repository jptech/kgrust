//! `kanagawa` — the argv-compatible legacy front end.
//!
//! Same flags, same streams, same exit codes as the reference compiler
//! (`docs/spec/cli.md`). It compiles through the reference's C++ backend
//! where `kg_backend_sys::locate` finds the library (`KG_BACKEND_DIR`, or
//! beside this executable); without one, a run that would have produced RTL
//! fails with a message naming the missing stage rather than reporting a
//! success it did not earn, and so does a program the walk cannot lower yet.

use std::io::Write;

/// `kg`'s allocator, for `kg`'s reason ([`kg_alloc::Allocator`]). The
/// backend's C++ allocates with its own `malloc`; nothing crosses the two.
#[global_allocator]
static ALLOCATOR: kg_alloc::Allocator = kg_alloc::Allocator;

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    if let Some(code) = kg_cli::backend_worker(&argv) {
        return std::process::ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();

    let code = kg_cli::legacy::run(&argv, &mut out, &mut err);

    // The reference flushes stdout explicitly on both paths because its C++
    // callee cannot rely on the Haskell caller to do it
    // (`compiler/cpp/kanagawa.cpp:258,262-264`); the same care costs nothing here.
    let _ = out.flush();
    let _ = err.flush();
    std::process::ExitCode::from(u8::try_from(code).unwrap_or(1))
}
