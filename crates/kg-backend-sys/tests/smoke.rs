//! The spike: load the real library, hand it an `Options` we filled, walk a
//! handful of nodes by hand, run `Codegen`. Proves linking, loading, the
//! struct layout on the wire and the lifecycle (`abi.md` §12) before any
//! walker exists. Skips, loudly, when the library is not on this machine;
//! a library that is there and refused is a failure, not a skip — a load
//! check that refused the real backend would otherwise pass every test here.

use std::ffi::CString;

use kg_backend_sys::options::{MarshalledOptions, OptionValues, StringArray, location};
use kg_backend_sys::{Backend, LoadError, raw};

fn harness_values(files: &[&str]) -> OptionValues {
    let mut v = OptionValues::harness_defaults();
    v.cmd_args = "kanagawa --place-iterations=1".into();
    v.file_names = files.iter().map(|s| s.to_string()).collect();
    v
}

fn load() -> Option<Backend> {
    // SAFETY: the located file is the reference's own build of the backend.
    match unsafe { Backend::locate_and_load() } {
        Ok(b) => Some(b),
        Err(e @ LoadError::NotFound) => {
            eprintln!("SKIPPED: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn init_walk_codegen() {
    let Some(b) = load() else { return };
    let opts = MarshalledOptions::new(&harness_values(&["one.k"]));
    assert_eq!(std::mem::size_of::<raw::Options>(), 520, "abi.md §3.3");
    // SAFETY: a fully initialised Options, alive across the call; the
    // protocol of §12, once per process.
    unsafe {
        assert_eq!(b.InitCompiler(opts.as_ptr()), 1, "InitCompiler accepted the harness options");

        // §6.4's `uint32 x = 5;`, minus the declaration itself (a global
        // is Error 1 in the frontend, so the reference never sends one):
        // locations, three leaf nodes, a typed literal, a list.
        let loc = location((1, 1), (1, 14), 0);
        b.SetLocation2(&raw const loc);
        let list = b.ParseBaseList(std::ptr::null_mut());
        assert!(!list.is_null());
        let ty = b.ParseUintType(32);
        assert!(!ty.is_null());
        let name = CString::new("x").unwrap();
        let ident = b.ParseIdentifier(name.as_ptr());
        assert!(!ident.is_null());
        let five = CString::new("5").unwrap();
        let lit = b.ParseDecimalLiteral(five.as_ptr());
        b.SetNodeType(lit, b.ParseUintType(32));
        let scope = StringArray::new(["main"]);
        let decl = b.ParseDeclare(
            b.ParseBaseList(std::ptr::null_mut()),
            ty,
            ident,
            lit,
            raw::DECLARE_FLAG_INITIALIZE,
            scope.as_ptr(),
        );
        assert!(!decl.is_null(), "ParseDeclare returned a NodeList (§6.4)");
        b.UnknownLocation();

        // An empty program root. Without the device library, `Codegen`
        // fails in `ExtractDeviceConfig` (Error 84) and returns 0 — that is
        // the whole-program sequence of §12.3 running to step 6, which is
        // what this spike is for. It prints to stdout on its own.
        let target = CString::new("None").unwrap();
        let output = CString::new("one").unwrap();
        let empty = CString::new("").unwrap();
        let ok = b.Codegen(
            target.as_ptr(),
            output.as_ptr(),
            empty.as_ptr(),
            empty.as_ptr(),
            empty.as_ptr(),
            list,
        );
        assert_eq!(ok, 0, "an empty program has no device config, so Codegen reports Error 84");
    }
}

#[test]
fn the_undefined_symbol_is_absent_not_fatal() {
    let Some(b) = load() else { return };
    assert!(b.ParseStages.is_err(), "ParseStages is declared but never defined (abi.md §1)");
    assert!(b.ParseDeclare.is_ok());
    assert!(b.InternalTests.is_ok());
    // Exactly the absence the load check tolerates: the reference's build
    // defines every other function the headers declare.
    assert_eq!(b.unresolved(), raw::UNDEFINED_SYMBOLS);
}
