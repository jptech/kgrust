//! The one place that knows where `parse_tree.h` lives, how bindgen reads
//! it, and what is generated from it, so that nothing crossing the ParseTree
//! ABI can disagree with anything else about it — `docs/spec/abi.md` §14.3.
//!
//! Two kinds of caller. `cargo xtask codegen` writes the user-facing crates'
//! generated sources from here — `kg-backend-sys`'s bindings
//! ([`bindings`]) and `kg-ptbridge`'s `Backend` trait ([`backend`]) — as
//! checked-in files, so building `kg` needs neither the reference checkout
//! nor libclang (sprint W8.4). `kg-ptrace`'s `build.rs` still generates its
//! forwarders through [`builder`]: the interposer records the reference's
//! own runs, so it is built only where the reference is.
//!
//! Header discovery for a build script: `KANAGAWA_ROOT` if set, else the
//! workspace's `third_party/kanagawa` submodule — the same default every
//! gate uses for the reference tree. `codegen` names the tree itself
//! (`--kanagawa-root`) and calls [`configure`].

use std::path::{Path, PathBuf};

mod backend;

pub use backend::backend;

/// For a caller that adds to [`configure`]'s choices without a bindgen
/// dependency of its own to drift.
pub use bindgen;

/// The reference checkout, `KANAGAWA_ROOT` or the workspace's
/// `third_party/kanagawa` submodule.
///
/// Resolved relative to the *calling* crate's manifest directory, which for a
/// crate under `crates/` is two levels below the workspace root. For build
/// scripts only: it prints a `rerun-if` line.
pub fn kanagawa_root() -> PathBuf {
    println!("cargo:rerun-if-env-changed=KANAGAWA_ROOT");
    if let Some(root) = std::env::var_os("KANAGAWA_ROOT") {
        return PathBuf::from(root);
    }
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    manifest.join("../../third_party/kanagawa")
}

/// `compiler/cpp` under the reference: the directory the three ABI headers
/// live in (`abi.md` §0).
pub fn cpp_dir() -> PathBuf {
    kanagawa_root().join(CPP)
}

/// Where the headers sit under a reference checkout.
pub const CPP: &str = "compiler/cpp";

/// The three headers that make up the boundary, as `abi.md` §0 verifies by
/// grepping for `extern "C"`.
pub const HEADERS: [&str; 3] = ["parse_tree.h", "options.h", "internal_tests.h"];

/// The wrapper bindgen parses. `options.h` uses `size_t` without including
/// `<stddef.h>` (the C++ build gets it transitively), so the wrapper includes
/// it first. `internal_tests.h` opens an *unguarded* `extern "C"` block and so
/// does not parse as C; its one declaration (`API int InternalTests();`,
/// `internal_tests.h:12`) is restated here instead, and the header is still
/// an input — of the digest and of a build script's `rerun-if-changed` — so a
/// change to it is at least noticed.
pub const WRAPPER: &str = "#include <stddef.h>\n#include \"parse_tree.h\"\nint InternalTests(void);\n";

/// [`configure`] over [`cpp_dir`], for a build script: emits the
/// `rerun-if-changed` lines for the headers, and fails the build with a
/// readable message if the reference tree is not where it should be.
pub fn builder() -> bindgen::Builder {
    let cpp = cpp_dir();
    for h in HEADERS {
        let path = cpp.join(h);
        assert!(
            path.is_file(),
            "reference header not found at {}; set KANAGAWA_ROOT to the kanagawa checkout",
            path.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
    }
    configure(&cpp)
}

/// A bindgen builder over the wrapper, with the choices `abi.md` §14.3 makes:
/// every declaration from the three headers and nothing else, enums as
/// module constants (so an enum-typed parameter is a plain `c_uint` and the
/// interposer renders it as a number), Rust 2024. bindgen's layout checks
/// come out as `const` assertions, so a drifted struct fails to compile;
/// the sizes and offsets §3 measured are also pinned by name in
/// `kg-backend-sys/tests/layout.rs`.
///
/// No formatter is chosen here: what is parsed with `syn` needs none, and
/// what is written out chooses its own ([`bindings`]).
pub fn configure(cpp: &Path) -> bindgen::Builder {
    bindgen::Builder::default()
        .header_contents("wrapper.h", WRAPPER)
        .clang_arg(format!("-I{}", cpp.display()))
        .clang_arg("-xc")
        .allowlist_file(r"(.*[/\\])?(parse_tree|options|wrapper)\.h")
        .default_enum_style(bindgen::EnumVariation::ModuleConsts)
        .layout_tests(true)
        .derive_debug(true)
        .derive_default(true)
        .derive_copy(true)
        .wrap_unsafe_ops(true)
        .rust_edition(bindgen::RustEdition::Edition2024)
        .rust_target(bindgen::RustTarget::stable(90, 0).expect("bindgen knows Rust 1.90"))
}

/// The name bindgen gives the dynamic-loading struct in `kg-backend-sys`.
pub const DYNAMIC_STRUCT: &str = "KanagawaBackend";

/// The declared-but-never-defined symbols (`abi.md` §1, §14.3): a loader that
/// resolves them eagerly fails, so the dynamic struct tolerates their absence
/// and the interposer does not forward them.
pub const UNDEFINED_SYMBOLS: &[&str] = &["ParseStages"];

/// FNV-1a, 64 bits, over each header's name and bytes in [`HEADERS`] order:
/// what the generated files record of their input, and what the loader's
/// refusal names (`kg_backend_sys::Backend::load`). Stable across builds and
/// machines, which `DefaultHasher` does not promise.
pub fn headers_digest(cpp: &Path) -> std::io::Result<String> {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for name in HEADERS {
        let text = std::fs::read(cpp.join(name))?;
        eat(name.as_bytes());
        eat(&[0]);
        eat(&text);
        eat(&[0]);
    }
    Ok(format!("{h:016x}"))
}

/// The first lines of a generated file: what made it and from what, so a
/// reader of the checked-in text knows not to edit it and where it came
/// from. Line comments, not `//!`: the file is `include!`d into a module.
fn banner(what: &str, commit: &str, digest: &str) -> String {
    let [a, b, c] = HEADERS;
    format!(
        r#"// @generated by cargo xtask codegen — do not edit.
//
// {what}.
// Inputs: the reference's ABI headers `{CPP}/{a}`,
// `{b}` and `{c}` at kanagawa commit
// {commit} (`reference.lock`),
// headers digest {digest} (`kg_abi_gen::headers_digest`), through
// bindgen as `kg_abi_gen::configure` sets it up. `cargo xtask codegen
// --check` regenerates this file from the pinned reference and compares.

"#
    )
}

/// `kg-backend-sys`'s `bindings.rs`: every type and constant of the ABI
/// headers, and [`DYNAMIC_STRUCT`], a table with one field per function,
/// filled by `dlopen` of a path rather than by linking — so the workspace
/// builds without the C++ backend and only a `kg build` that runs needs it.
/// After bindgen's text, what the loader checks a library against: the
/// headers' digest and commit, [`UNDEFINED_SYMBOLS`], and
/// `KanagawaBackend::unresolved`.
///
/// Every function is typed `extern "C-unwind"`, not the header's `"C"`:
/// the backend throws C++ exceptions out of the ABI, and a foreign unwind
/// out of an `extern "C"` import is undefined behaviour — the reasons, and
/// the test that pins it by type, are in `kg-backend-sys/src/lib.rs`.
/// `dynamic_link_require_all(false)` is what lets `ParseStages`, declared
/// and never defined, be an `Err` in the table rather than a load failure;
/// the loader decides which absences are fatal.
///
/// Formatted by `prettyplease`, which is a library and the same on every
/// machine; bindgen's default runs whatever `rustfmt` is on `PATH`, under
/// whatever `rustfmt.toml` its working directory finds — this tree's
/// disables formatting outright — and `codegen --check` would compare
/// formatters rather than bindings.
pub fn bindings(cpp: &Path, commit: &str) -> Result<String, String> {
    let digest = headers_digest(cpp).map_err(|e| format!("reading the headers under {}: {e}", cpp.display()))?;
    let text = configure(cpp)
        .dynamic_library_name(DYNAMIC_STRUCT)
        .dynamic_link_require_all(false)
        .override_abi(bindgen::Abi::CUnwind, ".*")
        .formatter(bindgen::Formatter::Prettyplease)
        .generate()
        .map_err(|e| format!("bindgen over {}: {e}", cpp.join(HEADERS[0]).display()))?
        .to_string();
    let file: syn::File = syn::parse_str(&text).map_err(|e| format!("bindgen's output does not parse: {e}"))?;
    let fields = dynamic_fields(&file)?;

    let undefined = UNDEFINED_SYMBOLS.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ");
    let resolved: String = fields.iter().map(|f| format!("            (\"{f}\", self.{f}.is_ok()),\n")).collect();
    let mut out = banner("The ParseTree ABI's types and its function table", commit, &digest);
    out.push_str(&text);
    out.push_str(&format!(
        r#"
// What the loader checks a library against (`kg_backend_sys::Backend::load`),
// added by `kg_abi_gen::bindings` after bindgen's output.

/// The reference commit whose headers this file was generated from.
pub const REFERENCE_COMMIT: &str = "{commit}";

/// `kg_abi_gen::headers_digest` of those headers.
pub const HEADERS_DIGEST: &str = "{digest}";

/// Declared by the headers and defined nowhere in the reference
/// (`abi.md` §1): absent from every library built from them.
pub const UNDEFINED_SYMBOLS: &[&str] = &[{undefined}];

impl {DYNAMIC_STRUCT} {{
    /// Every function the headers declare that the loaded library does
    /// not define, in header order.
    pub fn unresolved(&self) -> Vec<&'static str> {{
        let resolved = [
{resolved}        ];
        resolved.iter().filter(|(_, ok)| !ok).map(|(name, _)| *name).collect()
    }}
}}
"#
    ));
    Ok(out)
}

/// The function fields of the dynamic struct, in declaration order — every
/// named field but bindgen's private library handle.
fn dynamic_fields(file: &syn::File) -> Result<Vec<String>, String> {
    let table = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(s) if s.ident == DYNAMIC_STRUCT => Some(s),
            _ => None,
        })
        .ok_or_else(|| format!("bindgen emitted no `{DYNAMIC_STRUCT}`"))?;
    let fields: Vec<String> = table
        .fields
        .iter()
        .filter_map(|f| f.ident.as_ref().map(ToString::to_string))
        .filter(|name| !name.starts_with("__"))
        .collect();
    if fields.is_empty() {
        return Err(format!("`{DYNAMIC_STRUCT}` has no function fields"));
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header the banner names and the digest skipped would make the
    /// record a lie.
    #[test]
    fn the_digest_reads_every_header_by_name_and_content() {
        let dir = std::env::temp_dir().join(format!("kg-abi-gen-digest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for h in HEADERS {
            std::fs::write(dir.join(h), "int x;\n").unwrap();
        }
        let base = headers_digest(&dir).unwrap();
        assert_eq!(base.len(), 16);
        for h in HEADERS {
            std::fs::write(dir.join(h), "int y;\n").unwrap();
            assert_ne!(headers_digest(&dir).unwrap(), base, "{h} is not in the digest");
            std::fs::write(dir.join(h), "int x;\n").unwrap();
        }
        assert_eq!(headers_digest(&dir).unwrap(), base);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_banner_names_every_header_and_is_line_comments() {
        let b = banner("x", "abc", "0123");
        for h in HEADERS {
            assert!(b.contains(h), "{h} missing from\n{b}");
        }
        assert!(b.starts_with("// @generated by cargo xtask codegen"));
        // An inner doc comment cannot be `include!`d into a module.
        assert!(b.lines().all(|l| l.is_empty() || (l.starts_with("//") && !l.starts_with("//!"))), "{b}");
    }
}
