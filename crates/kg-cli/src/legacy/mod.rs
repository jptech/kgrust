//! The `kanagawa` compatibility front end.
//!
//! Its job is that `build/cmake/test_helper.cmake`, CI and every downstream
//! repository keep working untouched: the same argv, the same streams, the same
//! exit codes. What it must never do is *pretend*. There is no backend behind
//! it yet, so a run that would have produced RTL fails loudly with exit 1 and a
//! message that says exactly which stage is missing.
//!
//! Layout:
//!
//! | Module | Contents |
//! |---|---|
//! | [`options`] | the option record, defaults, `.cmdargs` synthesis, warning folding |
//! | [`flags`] | the flag inventory — 128 spellings, each citing `CmdArgs.hs` |
//! | [`parse`] | the `cmdargs`-shaped argv parser |
//! | [`validate`] | `SetupCodeGenConfig`'s ten option checks |
//! | [`help`] | `--help` / `--version`, rendered as `cmdargs` renders them |
//! | [`run`] | dispatch, diagnostics, exit codes |

pub mod flags;
pub mod help;
pub mod options;
pub mod parse;
pub mod run;
pub mod validate;

pub use options::{LegacyOptions, Mode, Verbosity};
pub use parse::{Outcome, parse};
pub use run::run;

/// `getProgName`: the basename of `argv[0]`.
///
/// GHC drops everything up to and including the last path separator — `/` on
/// POSIX, `/` or `\` on Windows — and does **not** strip `.exe` or resolve
/// symlinks (`docs/spec/cli.md` §6.1). The separator set is host-dependent
/// there, so it is host-dependent here.
pub fn prog_name(argv0: &str) -> &str {
    let mut name = argv0.rsplit('/').next().unwrap_or(argv0);
    if cfg!(windows) {
        name = name.rsplit('\\').next().unwrap_or(name);
    }
    name
}

/// `unwords (getProgName : getArgs)` — the string that ends up in
/// `Options::_cmdArgs` and, from there, verbatim into the second line of every
/// generated `.sv` file (`compiler/cpp/verilog.cpp:7356-7362`).
///
/// Space-joined with **no quoting and no escaping**, so an argument containing a
/// space is flattened irreversibly; reproducing that is the point. Any
/// difference here makes 100% of generated RTL differ.
pub fn cmd_args_string(argv: &[String]) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(argv.len());
    match argv.split_first() {
        Some((argv0, rest)) => {
            parts.push(prog_name(argv0));
            parts.extend(rest.iter().map(String::as_str));
        }
        None => return String::new(),
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn prog_name_is_the_basename_and_keeps_its_suffix() {
        assert_eq!(prog_name("/usr/local/bin/kanagawa"), "kanagawa");
        assert_eq!(prog_name("./kanagawa"), "kanagawa");
        assert_eq!(prog_name("kanagawa"), "kanagawa");
        // No `.exe` stripping, no symlink resolution, no version stripping.
        assert_eq!(prog_name("/opt/kanagawa-1.2.3"), "kanagawa-1.2.3");
        assert_eq!(prog_name("/opt/kanagawa.exe"), "kanagawa.exe");
        assert_eq!(prog_name(""), "");
    }

    #[test]
    fn cmd_args_is_the_basename_joined_with_the_verbatim_argv() {
        assert_eq!(
            cmd_args_string(&argv(&["/usr/bin/kanagawa", "--backend=sv", "source.k"])),
            "kanagawa --backend=sv source.k"
        );
        // The mode word is part of the recorded string when it is present.
        assert_eq!(
            cmd_args_string(&argv(&["kanagawa", "compile", "foo.k"])),
            "kanagawa compile foo.k"
        );
        // Installed under another name: the recorded string follows argv[0].
        assert_eq!(cmd_args_string(&argv(&["/opt/kanagawa-1.2.3", "a.k"])), "kanagawa-1.2.3 a.k");
    }

    #[test]
    fn arguments_containing_spaces_are_flattened_not_quoted() {
        assert_eq!(
            cmd_args_string(&argv(&["kanagawa", "--import-dir=/My Libs", "a.k"])),
            "kanagawa --import-dir=/My Libs a.k"
        );
    }

    #[test]
    fn no_arguments_records_just_the_program_name() {
        assert_eq!(cmd_args_string(&argv(&["kanagawa"])), "kanagawa");
        assert_eq!(cmd_args_string(&[]), "");
    }
}
