//! A shim `libkanagawa-backend`: every exported function of the ABI is
//! defined here, logs the call in `kg-pttrace`'s format, forwards it to the
//! *real* library, logs the result, and returns it. `docs/spec/abi.md` §14
//! is the design; `build.rs` generates the forwarders from the headers.
//!
//! Installed by rename-and-shim (§14.4): the reference executable finds
//! `libkanagawa-backend.dylib` by rpath, so a directory holding the
//! executable, this library under that name, and the real one renamed
//! `libkanagawa-backend-real.dylib` records every run. `cargo xtask ptrace`
//! lays that directory out.
//!
//! Environment:
//!
//! * `KG_PTRACE_OUT` — the trace file (created, truncated); stderr if unset.
//! * `KG_PTRACE_REAL` — the real library; default is
//!   `libkanagawa-backend-real.<ext>` beside the running executable.
//!
//! On macOS the two-level namespace binds the backend's *internal* calls to
//! its own definitions, so nothing here is reached at depth 1; the depth
//! counter is kept because ELF interposition does route them here (§5.4).

use std::cell::Cell;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Mutex, OnceLock};

use kg_pttrace::{Arg, Writer};

mod forwarders {
    #![allow(non_snake_case, clippy::missing_safety_doc)]
    use kg_backend_sys::raw::*;
    include!(concat!(env!("OUT_DIR"), "/forwarders.rs"));
}

pub use forwarders::{ABI_HASH, FORWARDERS};

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

static REAL: OnceLock<libloading::Library> = OnceLock::new();
static OUT: OnceLock<Mutex<Writer>> = OnceLock::new();

fn real_library_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("KG_PTRACE_REAL") {
        return p.into();
    }
    let file = kg_backend_sys::locate::LIBRARY_FILE;
    let (stem, ext) = file.rsplit_once('.').expect("library file name has an extension");
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
    dir.join(format!("{stem}-real.{ext}"))
}

fn library() -> &'static libloading::Library {
    REAL.get_or_init(|| {
        let path = real_library_path();
        // SAFETY: the file is the reference's backend, renamed; its
        // initialisers are the ones the executable would have run anyway.
        unsafe { libloading::Library::new(&path) }
            .unwrap_or_else(|e| panic!("kg-ptrace: cannot load the real backend at {}: {e}", path.display()))
    })
}

/// The real function behind `name`, as the caller's function-pointer type.
///
/// # Safety
/// `F` must be the exact `unsafe extern "C" fn` type of the symbol.
pub unsafe fn real<F: Copy>(name: &str) -> F {
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*const c_void>());
    let lib = library();
    // SAFETY: the symbol's type is the caller's contract.
    let sym: libloading::Symbol<'_, *const c_void> = unsafe { lib.get(name.as_bytes()) }
        .unwrap_or_else(|e| panic!("kg-ptrace: real backend lacks {name}: {e}"));
    let ptr: *const c_void = *sym;
    // SAFETY: same size, and F is a function pointer per the contract.
    unsafe { std::mem::transmute_copy(&ptr) }
}

fn writer() -> &'static Mutex<Writer> {
    OUT.get_or_init(|| {
        let out: Box<dyn std::io::Write + Send> = match std::env::var_os("KG_PTRACE_OUT") {
            Some(p) => Box::new(
                std::fs::File::create(&p)
                    .unwrap_or_else(|e| panic!("kg-ptrace: cannot create {}: {e}", p.to_string_lossy())),
            ),
            None => Box::new(std::io::stderr()),
        };
        let mut w = Writer::new(out);
        let _ = w.version(&real_library_path().display().to_string(), ABI_HASH);
        Mutex::new(w)
    })
}

/// One call in flight: its sequence number and depth, taken at entry.
#[derive(Debug)]
pub struct Call {
    name: &'static str,
    seq: u64,
    depth: u32,
}

impl Call {
    pub fn enter(name: &'static str) -> Call {
        let depth = DEPTH.with(|d| {
            let v = d.get();
            d.set(v + 1);
            v
        });
        let mut w = writer().lock().unwrap_or_else(|e| e.into_inner());
        let seq = w.next_seq();
        Call { name, seq, depth }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get() - 1));
    }
}

/// An argument before the writer's ids have been consulted.
#[derive(Debug)]
pub enum Raw {
    Node(usize),
    Str(Option<Vec<u8>>),
    Scope(Option<Vec<Vec<u8>>>),
    UInt(u64),
    Int(i64),
    F32(f32),
    Loc(Option<kg_pttrace::Location>),
    /// The rendered fields, for the `#options` line.
    Options(String),
    Void,
}

/// Constructors the generated forwarders call, one per column of §14.3.
/// The pointer-taking ones are `unsafe`: they read what the reference
/// passed, under the ABI's own contract.
#[allow(clippy::missing_safety_doc)]
pub mod arg {
    use super::*;
    use kg_backend_sys::raw::{Location, Options, ParseTreeNodePtr};

    pub fn node(p: ParseTreeNodePtr) -> Raw {
        Raw::Node(p as usize)
    }
    pub unsafe fn cstr(p: *const c_char) -> Raw {
        // SAFETY: the reference passes NUL-terminated strings or NULL.
        Raw::Str(if p.is_null() { None } else { Some(unsafe { CStr::from_ptr(p) }.to_bytes().to_vec()) })
    }
    pub unsafe fn scope(p: *const *const c_char) -> Raw {
        if p.is_null() {
            return Raw::Scope(None);
        }
        let mut items = Vec::new();
        let mut q = p;
        // SAFETY: a NUL-terminated array of NUL-terminated strings (§8).
        unsafe {
            while !(*q).is_null() {
                items.push(CStr::from_ptr(*q).to_bytes().to_vec());
                q = q.add(1);
            }
        }
        Raw::Scope(Some(items))
    }
    pub fn uint(n: u64) -> Raw {
        Raw::UInt(n)
    }
    pub fn int(n: i64) -> Raw {
        Raw::Int(n)
    }
    pub fn f32(x: f32) -> Raw {
        Raw::F32(x)
    }
    /// `_valid` is read as a **byte**, never as the `bool` the binding
    /// declares: the reference pokes five fields of an `alloca`'d
    /// `Location` (`Types.hsc:31-37`) and leaves that one indeterminate, and
    /// a Rust `bool` holding a byte other than 0 or 1 is undefined
    /// behaviour — the first recording read it, and 50 traces ended with an
    /// `Option<Location>` decoded as a different variant, then an abort
    /// freeing a `String` that never existed. `SetLocation` forces the flag
    /// true anyway (§6.1), so the wire value is noise a differ must skip.
    pub unsafe fn loc_ptr(p: *const Location) -> Raw {
        if p.is_null() {
            return Raw::Loc(None);
        }
        // SAFETY: a live `Location` per the ABI; `_valid` read as a byte
        // for the reason above.
        let (l, valid) = unsafe {
            (&*p, p.cast::<u8>().add(std::mem::offset_of!(Location, _valid)).read() != 0)
        };
        Raw::Loc(Some(kg_pttrace::Location {
            begin_line: l._beginLine,
            begin_column: l._beginColumn,
            end_line: l._endLine,
            end_column: l._endColumn,
            file: l._fileIndex,
            valid,
        }))
    }
    pub fn loc(l: Location) -> Raw {
        // SAFETY: a copy on this stack frame.
        unsafe { loc_ptr(&raw const l) }
    }
    pub unsafe fn options(p: *const Options) -> Raw {
        if p.is_null() {
            return Raw::Options("_".to_owned());
        }
        // SAFETY: a fully initialised Options for the duration of the call.
        let o = unsafe { &*p };
        let mut s = String::new();
        s.push_str("device=");
        match unsafe { cstr(o._deviceName) } {
            Raw::Str(Some(b)) => kg_pttrace::render_string(&b, &mut s),
            _ => s.push('_'),
        }
        s.push_str(" cmd=");
        match unsafe { cstr(o._cmdArgs) } {
            Raw::Str(Some(b)) => kg_pttrace::render_string(&b, &mut s),
            _ => s.push('_'),
        }
        s.push_str(" files=");
        match unsafe { scope(o._fileNames.cast_const()) } {
            Raw::Scope(items) => Arg::Scope(items).render(&mut s),
            _ => unreachable!(),
        }
        s.push_str(&format!(
            " frequency={} codegen={:?} placement={:?}",
            o._frequency, o._codegenOptions, o._placementOptions
        ));
        Raw::Options(s.replace('\n', " "))
    }
    pub fn void() -> Raw {
        Raw::Void
    }
}

/// Write the line for a finished call.
pub fn record(call: Call, args: &[Raw], ret: Raw) {
    let mut w = writer().lock().unwrap_or_else(|e| e.into_inner());
    let lift = |w: &mut Writer, r: &Raw| -> Arg {
        match r {
            Raw::Node(p) => Arg::Node(w.ids().node(*p)),
            Raw::Str(s) => Arg::Str(s.clone()),
            Raw::Scope(s) => Arg::Scope(s.clone()),
            Raw::UInt(n) => Arg::UInt(*n),
            Raw::Int(n) => Arg::Int(*n),
            Raw::F32(x) => Arg::F32(*x),
            Raw::Loc(l) => Arg::Loc(*l),
            Raw::Options(fields) => {
                let _ = w.options(fields);
                Arg::Options
            }
            Raw::Void => Arg::Void,
        }
    };
    let args: Vec<Arg> = args.iter().map(|r| lift(&mut w, r)).collect();
    let ret = lift(&mut w, &ret);
    let _ = w.call(call.seq, call.depth, call.name, &args, &ret);
    // The epoch boundary goes *after* the `Codegen` line, so its root
    // argument keeps the id the walk gave it; clearing first re-minted the
    // root as a node nobody had created (§14.2 says "on entry", and this is
    // the reading of it that keeps the region before `Codegen` self-contained).
    if call.name == "Codegen" {
        let _ = w.epoch();
    }
    if call.name == "Codegen" || call.name == "InitCompiler" {
        let _ = w.flush();
    }
}
