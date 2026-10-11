//! The toolchain's two front doors.
//!
//! * [`modern`] is `kg`: subcommands, a manifest, layered configuration and
//!   rustc-shaped diagnostics.
//! * [`legacy`] is `kanagawa`: the reference compiler's exact argv surface —
//!   128 flag spellings over three modes (`legacy::flags`), stdout-only
//!   diagnostics, exit codes 0 and 1 — so that `build/cmake/test_helper.cmake`,
//!   CI and downstream repositories keep working without edits.
//!
//! Both are libraries first and binaries second: `run(argv, out, err) -> i32`
//! in each, with the streams injected, so behaviour is testable without
//! spawning a process.
//!
//! ## What this build does
//!
//! * `kg check` runs the whole frontend — the per-module passes and the
//!   program-wide loop — and reports every diagnostic with a source excerpt;
//! * `kg build` and the compat `kanagawa` also walk the elaborated program
//!   through the reference's C++ backend, in a worker process, to RTL
//!   (`docs/known-divergences.md` #33 on a backend crash; the compat
//!   binary's flags that are parsed and read by nothing are listed under
//!   "Not yet implemented" there);
//! * `kg fmt` formats, `kg lsp` is the language server, and `kg doc` writes
//!   a static API-documentation site from the sources' doc comments;
//! * `kg dump tokens|cst|ast|ir|instances` and `kg explain` are the
//!   debugging and reference tools.

pub mod explain;
pub mod legacy;
pub mod modern;

/// The port's verdict switches from the environment — read here, at the
/// front doors (`kg`'s settings layer, `kanagawa`'s compile, and `kg-lsp`'s
/// own), and never inside the compiler (sprint W8.2):
///
/// * `KG_LEFTOVERS=0` — the loop's leftover verdicts on its copies (pass
///   12's inferential arms, a `static if` standing in a copy), on by
///   default since sprint W1.4, fall silent;
/// * `KG_INFER_FULL` — set: inference walks the whole program every
///   iteration, the incremental walk's reference;
/// * `KG_PTBRIDGE_SEED=0` — the backend walk leaves out the parser's
///   redundant-import seed.
///
/// Each is a field of [`kg_config::Verdicts`], which says what it decides.
pub fn verdicts_from_env() -> kg_config::Verdicts {
    kg_config::Verdicts::read(|name| std::env::var_os(name))
}

/// The backend worker's entry, when `argv[1]` asks for it: `kg build` and
/// `kanagawa` re-run their own executable to replay a recorded walk into
/// the C++ backend (`kg_ptbridge::isolate`), so both binaries answer to it
/// before any other parsing — it is not a subcommand anyone types.
pub fn backend_worker(argv: &[String]) -> Option<i32> {
    (argv.get(1).map(String::as_str) == Some(kg_ptbridge::isolate::WORKER_ARG))
        .then(|| kg_ptbridge::isolate::worker_main(&argv[2..]))
}

/// `argv[0]`-based dispatch, so one binary installed (or symlinked) as
/// `kanagawa` behaves as the compat front end.
///
/// `docs/spec/cli.md` §9.4 asks for exactly this: a drop-in replacement is
/// installed under the old name, and the recorded `_cmdArgs` string then starts
/// with that name, which is what the generated RTL header needs.
pub fn dispatch_on_argv0(argv: &[String]) -> bool {
    argv.first()
        .map(|argv0| legacy::prog_name(argv0))
        .is_some_and(|name| name.starts_with("kanagawa"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn argv0_selects_the_front_end() {
        assert!(dispatch_on_argv0(&argv(&["/usr/local/bin/kanagawa", "a.k"])));
        assert!(dispatch_on_argv0(&argv(&["kanagawa.exe"])));
        assert!(!dispatch_on_argv0(&argv(&["/usr/local/bin/kg", "check"])));
        assert!(!dispatch_on_argv0(&[]));
    }
}
