//! One reading of the options (sprint W6): `conformance::settings_for`
//! grades each registered test under the flags `CMakeLists.txt` gives it,
//! and `kanagawa` — what upstream's runner drives — reads the same flags
//! through `kg_cli::legacy`. Two readers of one command line can disagree
//! (the scoreboard splits `--define` at `#` only, the reference at `#` or
//! `=`, `Parser/IO.hs:52-55`), and a gate that graded under settings no user
//! gets would grade nothing real. The scoreboard is not edited here
//! (`CLAUDE.md`); this holds the two readings equal over every registered
//! test's flags, and a disagreement is the owner's to rule on.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The settings that decide a verdict, spelled so two readings compare.
    fn projection(s: &kg_config::Settings) -> String {
        format!(
            "import_dirs={:?}\nbase_library={:?}\ntarget_device={}\nimplicit_base={}\nparse_docs={}\ndefines={:?}\nusing={:?}\nwarnings={:?}\nwerror={}\nbackend={:?}",
            s.import_dirs,
            s.base_library,
            s.target_device,
            s.implicit_base,
            s.parse_docs,
            s.defines,
            s.using,
            s.warnings,
            s.warnings_as_errors,
            s.backend,
        )
    }

    fn kanagawa_root() -> PathBuf {
        std::env::var_os("KANAGAWA_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../third_party/kanagawa"))
    }

    /// Disagreements found and left for the owner (the scoreboard is not
    /// ours to edit), each with the field the two readings part at.
    ///
    /// `all-warnings-as-errors`: its flags are `--Wno-redundant-import …
    /// --Wall`, and the gate reads `--Wall` as every warning, which turns
    /// `redundant-import` back on; the reference's `--Wall` enables
    /// `Conversion` and `TransactionSizeMissing` only
    /// (`Options/CmdArgs.hs`'s `warnings_state`), and its binary prints no
    /// redundant-import warning under those flags (probed 2026-10-10). The
    /// compat binary reads it as the reference does. No assertion of the
    /// file moves today: none has a redundant import.
    const BOOKED: &[(&str, &str)] = &[("all-warnings-as-errors", "warnings=")];

    /// Every registration's flags, read by the gate and by the compat
    /// binary from the reference root as the working directory (where
    /// `settings_for` resolves a relative path), give the same settings —
    /// but for [`BOOKED`], which must still disagree exactly there.
    #[test]
    fn the_gate_and_the_compat_binary_read_every_registered_command_line_alike() {
        let root = kanagawa_root();
        if !root.join("test/syntax/CMakeLists.txt").is_file() {
            kg_base::corpus_absent(&format!("no reference corpus at {}", root.display()));
            return;
        }
        let root = root.canonicalize().expect("the reference root resolves");
        let cwd = root.to_str().expect("UTF-8").to_owned();
        let registrations = crate::conformance::registrations(&root).expect("the registrations parse");
        assert!(registrations.len() > 50, "{}", registrations.len());
        let mut differ = Vec::new();
        for reg in &registrations {
            let gate = crate::conformance::settings_for(&root, &reg.options);
            let mut argv = reg.options.clone();
            argv.push("snippet.k".to_owned());
            let kg_cli::legacy::parse::Outcome::Parsed(opts) = kg_cli::legacy::parse::parse(&argv) else {
                differ.push(format!("{}: the compat binary refuses {:?}", reg.name, reg.options));
                continue;
            };
            let mut cli = kg_cli::legacy::options::settings_from(&opts, &cwd);
            // The verdict switches are read from the environment at both doors.
            cli.verdicts = gate.verdicts;
            let (a, b) = (projection(&gate), projection(&cli));
            let parted = a.lines().zip(b.lines()).find(|(x, y)| x != y);
            let booked = BOOKED.iter().find(|(name, _)| *name == reg.name).map(|(_, field)| *field);
            match (parted, booked) {
                (None, None) => {}
                (Some((x, _)), Some(field)) if x.starts_with(field) => {}
                (None, Some(field)) => differ.push(format!("{}: booked at `{field}` but the readings now agree — unbook it", reg.name)),
                (Some((x, y)), _) => differ.push(format!("{} {:?}:\n    gate {x}\n    cli  {y}", reg.name, reg.options)),
            }
        }
        assert!(differ.is_empty(), "{}", differ.join("\n"));
    }
}
