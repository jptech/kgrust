//! The real backend: `kg-backend-sys`, loaded, behind the trait.

use kg_backend_sys::raw;

pub struct Real {
    pub(crate) api: kg_backend_sys::Backend,
}

impl std::fmt::Debug for Real {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Real").field("path", &self.api.path()).finish()
    }
}

impl Real {
    /// Load the library at `path` — located by the parent with
    /// [`kg_backend_sys::locate`] and handed to the worker, so the two
    /// cannot resolve `KG_BACKEND_DIR` differently.
    ///
    /// Safe by this crate's contract: what `locate` finds under the
    /// backend's file name is a build of the reference's backend, whose
    /// initialisers the reference executable runs the same way. A caller
    /// that wants a different library sets `KG_BACKEND_DIR`.
    pub fn load(path: &std::path::Path) -> Result<Real, kg_backend_sys::LoadError> {
        // SAFETY: the contract above.
        let api = unsafe { kg_backend_sys::Backend::load(path) }
            .map_err(|e| kg_backend_sys::LoadError::Load(path.to_path_buf(), e))?;
        Ok(Real { api })
    }

    pub fn path(&self) -> &std::path::Path {
        self.api.path()
    }
}

/// The C struct for a trace location; `_valid` is what the reference's
/// `SetLocation` forces anyway (`abi.md` §6.1).
pub(crate) fn raw_location(l: &kg_pttrace::Location) -> raw::Location {
    raw::Location {
        _beginLine: l.begin_line,
        _beginColumn: l.begin_column,
        _endLine: l.end_line,
        _endColumn: l.end_column,
        _fileIndex: l.file,
        _valid: l.valid,
    }
}
