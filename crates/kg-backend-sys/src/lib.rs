//! Raw bindings to the ParseTree C ABI of `libkanagawa-backend`
//! (`docs/spec/abi.md`), and nothing safe: every function here is `unsafe`
//! and takes what C takes. `kg-ptbridge` is the safe surface.
//!
//! The library is **loaded, not linked**. `abi.md` §0 documents how the
//! reference finds it by rpath; here [`Backend::load`] `dlopen`s a path, so
//! the workspace compiles without the C++ build and only a `kg build` that
//! actually runs needs the dylib. [`locate`] says where it is looked for.
//!
//! Three things live here besides the generated code:
//!
//! * [`options`] — the `Options` struct filled from plain Rust values, with
//!   the widths §3.4 warns about pinned and the never-written bytes zeroed.
//! * [`locate`] — where the dylib is looked for, in order.
//! * [`Backend`] — the loaded library and its function table.

pub mod locate;
pub mod options;

/// Everything bindgen generated: the types and constants of `parse_tree.h`
/// and `options.h`, and [`raw::KanagawaBackend`], the function table — the
/// checked-in `bindings.rs`, which `cargo xtask codegen` writes from the
/// pinned reference's headers (`kg_abi_gen::bindings`) and `--check`
/// compares, so that building this crate needs neither the headers nor
/// libclang.
///
/// Enums are modules of constants (`raw::ParseTreeBinaryOpType::…`) over a
/// plain integer alias, so a parameter typed by one is a number on the wire
/// exactly as §2 tabulates it.
///
/// Every function is typed `extern "C-unwind"`, not the header's `"C"`: the
/// backend throws C++ exceptions out of the ABI (`OnAssertionFailure`,
/// `compiler.cpp:1561`, from any `Parse*` call; only `InitCompiler` and
/// `Codegen` catch), and a foreign unwind out of an `extern "C"` import is
/// undefined behaviour — Rust may assume the call cannot unwind and omit the
/// landing pads. Under `"C-unwind"` it is defined: Rust's destructors run and
/// the unwind aborts the process at the first `catch_unwind`, which is what
/// lets the backend worker (`kg-ptbridge::isolate`) say which call threw
/// before it dies. Measured 2026-09-23 on macOS/arm64, the worker's guard
/// also ran with the bindings on `"C"` — undefined behaviour that happened
/// to do the defined thing — so no process test can tell the two apart and
/// the choice is pinned by type instead (`the_bindings_are_c_unwind`).
/// `kg-ptrace`'s interposer keeps `"C"`; it forwards.
#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    missing_debug_implementations,
    clippy::all
)]
pub mod raw {
    include!("bindings.rs");
}

/// The bytes the reference hands C for `s`: `withCAString`
/// (`ParseTree.hs:33-44,178,213`, `Options/FFI.hsc:52`) pokes each `Char`'s
/// **low byte** (`castCharToCChar`) and a terminating NUL, so C sees Latin-1
/// truncation, and sees the string end at the first zero byte — an escaped
/// `\0` in a string literal, or any character whose low byte is zero.
///
/// Every string bound for the backend goes through here, and the trace side
/// records the same bytes, so `ptdiff` compares what C actually received on
/// both sides. The result holds no NUL, which is what makes the
/// `CString` built from it infallible ([`ca_string`]).
pub fn ca_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).take_while(|&b| b != 0).collect()
}

/// [`ca_bytes`] as the `CString` a call site passes.
pub fn ca_string(s: &str) -> std::ffi::CString {
    std::ffi::CString::new(ca_bytes(s)).expect("ca_bytes stops at the first NUL")
}

#[cfg(test)]
mod tests {
    use super::ca_bytes;

    #[test]
    fn a_string_reaches_c_as_with_ca_string_pokes_it() {
        assert_eq!(ca_bytes("abc"), b"abc");
        // A NUL ends the C string: the panic `docs/history/audit-2026-09-22.md` D5 found in `kg build`.
        assert_eq!(ca_bytes("a\0b"), b"a");
        // One byte per character, the low one: not UTF-8.
        assert_eq!(ca_bytes("\u{e9}"), [0xe9]);
        assert_eq!(ca_bytes("x\u{20ac}"), [b'x', 0xac]);
        // A character whose low byte is zero ends it too.
        assert_eq!(ca_bytes("x\u{100}y"), b"x");
    }

    /// Any library loads — `dlopen` takes it, and the table holds an `Err`
    /// for each function it lacks — so before the check one that was not
    /// the backend was found out at its first call, by a panic. Now it is
    /// refused at load, naming what it lacks (sprint W8.5). The platform's
    /// C library stands in: on every machine, and not the backend.
    #[test]
    fn a_library_that_is_not_the_backend_is_refused_at_load() {
        const OTHER: &str = if cfg!(target_os = "macos") {
            "/usr/lib/libSystem.B.dylib"
        } else if cfg!(target_os = "windows") {
            "kernel32.dll"
        } else {
            "libc.so.6"
        };
        // SAFETY: a library this process has already loaded, so no
        // initialiser runs; and no function of the table is called.
        let refused = unsafe { crate::Backend::load(OTHER) }.expect_err("the C library is not the backend");
        let crate::Refused::Mismatch { missing } = &refused else { panic!("{refused}") };
        for f in ["InitCompiler", "ParseDeclare", "Codegen", "InternalTests"] {
            assert!(missing.contains(&f), "{f} not reported: {missing:?}");
        }
        assert!(!missing.contains(&"ParseStages"), "the absence every build has is not a mismatch");
        // §1: 85 functions in `parse_tree.h` and `InternalTests`.
        assert_eq!(missing.len() + crate::raw::UNDEFINED_SYMBOLS.len(), 86);
        let text = refused.to_string();
        assert!(text.contains(crate::raw::HEADERS_DIGEST) && text.contains("ParseDeclare"), "{text}");
    }

    /// The bindings let a C++ exception unwind ([`crate::raw`]): this compiles
    /// only while the function pointers are `extern "C-unwind"`, which no
    /// run can tell from `"C"` (the undefined behaviour of the second
    /// happens to unwind too, measured).
    #[test]
    fn the_bindings_are_c_unwind() {
        fn pinned(b: &crate::KanagawaBackend) {
            let _: &Result<unsafe extern "C-unwind" fn(*const crate::Options) -> crate::raw::BOOL, libloading::Error> =
                &b.InitCompiler;
        }
        let _ = pinned;
    }
}

pub use raw::{
    CodeGenOptions, KanagawaBackend, Location, Options, ParseNamespaceScopePtr,
    ParseQualifiedNamePtr, ParseTreeNodePtr, PlacementOptions,
};

/// The loaded backend: the dylib and one function pointer per ABI entry.
///
/// Dereference to [`KanagawaBackend`] to call: `backend.ParseIdentifier(p)`.
/// Every call is `unsafe`, because the protocol is stateful (§6) and the
/// pointers are the backend's.
pub struct Backend {
    path: std::path::PathBuf,
    api: KanagawaBackend,
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Backend {
    /// `dlopen` the library at `path`, resolve every symbol, and refuse it,
    /// before any call is made, unless it defines every function the headers
    /// declare. `ParseStages`, declared and never defined (§1), is the one
    /// absence tolerated: an `Err` in the table, an error only if called.
    ///
    /// That is the whole of the check a library allows (sprint W8.5): it
    /// carries no version — no symbol of the ABI reports one (§1), its
    /// `LC_ID_DYLIB` says 0.0.0, and the release version reaches only the
    /// reference's Haskell executable (`ReleaseVersion.hs`). So what is
    /// compared is the library's symbols against the headers' declarations.
    /// It catches a library built from headers that lack a function these
    /// declare, and a file that is not the backend at all; it cannot see a
    /// changed signature under the same name, a changed `Options` layout or
    /// enum numbering, or a function the library adds — C symbols carry no
    /// types. Those are held at generation instead: `cargo xtask codegen
    /// --check` regenerates the bindings from the pinned reference's
    /// headers, whose digest the refusal names ([`raw::HEADERS_DIGEST`]),
    /// and that the gates' library is built from that reference is the
    /// build's to keep true.
    ///
    /// # Safety
    /// Loading runs the library's initialisers; the caller vouches that the
    /// file is `libkanagawa-backend` built from the headers these bindings
    /// were generated from, which the check above can only partly confirm.
    pub unsafe fn load(path: impl AsRef<std::path::Path>) -> Result<Backend, Refused> {
        let path = path.as_ref().to_path_buf();
        // SAFETY: forwarded to the caller's contract above.
        let api = unsafe { KanagawaBackend::new(&path) }.map_err(Refused::Dlopen)?;
        let missing: Vec<&'static str> =
            api.unresolved().into_iter().filter(|name| !raw::UNDEFINED_SYMBOLS.contains(name)).collect();
        if !missing.is_empty() {
            return Err(Refused::Mismatch { missing });
        }
        Ok(Backend { path, api })
    }

    /// [`locate::backend_library`] then [`Backend::load`].
    ///
    /// # Safety
    /// As [`Backend::load`].
    pub unsafe fn locate_and_load() -> Result<Backend, LoadError> {
        let path = locate::backend_library().ok_or(LoadError::NotFound)?;
        // SAFETY: forwarded.
        unsafe { Backend::load(&path) }.map_err(|e| LoadError::Load(path, e))
    }

    /// Where the library was loaded from.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl std::ops::Deref for Backend {
    type Target = KanagawaBackend;
    fn deref(&self) -> &KanagawaBackend {
        &self.api
    }
}

/// Why [`Backend::load`] refused a file.
#[derive(Debug)]
pub enum Refused {
    /// `dlopen` refused it.
    Dlopen(libloading::Error),
    /// It loaded, and does not define these functions the headers declare,
    /// in header order: it was not built from the headers these bindings
    /// were generated from.
    Mismatch { missing: Vec<&'static str> },
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Dlopen(e) => write!(f, "{e}"),
            Refused::Mismatch { missing } => {
                const SHOWN: usize = 5;
                let mut names = missing.iter().take(SHOWN).copied().collect::<Vec<_>>().join(", ");
                if missing.len() > SHOWN {
                    names.push_str(&format!(" and {} more", missing.len() - SHOWN));
                }
                write!(
                    f,
                    "it does not define {names}, which the ABI headers this build was generated from \
                     declare (kanagawa commit {}, headers digest {}): it was built from other sources, \
                     or is not libkanagawa-backend",
                    raw::REFERENCE_COMMIT,
                    raw::HEADERS_DIGEST
                )
            }
        }
    }
}

impl std::error::Error for Refused {}

/// Why [`Backend::locate_and_load`] did not.
#[derive(Debug)]
pub enum LoadError {
    /// No candidate of [`locate::backend_library`] exists.
    NotFound,
    /// The file exists and [`Backend::load`] refused it.
    Load(std::path::PathBuf, Refused),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotFound => write!(
                f,
                "libkanagawa-backend not found; set {} to the directory holding it (tried: {})",
                locate::ENV_VAR,
                locate::candidates()
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            LoadError::Load(path, e) => write!(f, "could not load {}: {e}", path.display()),
        }
    }
}

impl std::error::Error for LoadError {}
