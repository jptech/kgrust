//! The proof every expansion carries before it is given: the program's
//! project read in edition 1.0, each file its expansion, must parse and check
//! clean, and walk into the backend call for call as the program does —
//! locations aside, which the edits move (`xtask editions`' twin comparison;
//! `ptdiff`'s structural reading).
//!
//! The spelling of what the edits wrote is only as good as the scope it
//! lands in: a call's default names an enum's constant it may not see, or sees
//! under another meaning. So a spelling is not trusted, it is read back — and
//! an expansion the frontend reads differently is refused, never printed.

use std::sync::{Arc, Mutex};

use kg_base::language::Language;
use kg_base::{FileId, FileRange, TextRange};
use kg_config::Settings;
use kg_db::Db;
use kg_diag::Diagnostic;

use crate::Expanded;
use crate::edit::Rendered;

pub(crate) fn verify(db: &Db, roots: &[FileId], out: &[Expanded], rendered: &[Rendered]) -> Result<(), Vec<Diagnostic>> {
    let settings = db.settings();
    let reference = Settings { language: settings.language.with_project(Language::REFERENCE), ..settings.clone() };
    let mut read = Db::new(reference);
    let files: Vec<FileId> = out.iter().map(|e| read.set_file_text(&e.path, e.text.clone())).collect();
    let read_roots: Vec<FileId> = roots.iter().filter_map(|&r| db.path(r)).map(|p| read.file_id(&p)).collect();

    // Back from where the reading found a fault to the file it stands for.
    let back = |at: FileRange, what: String| {
        let i = files.iter().position(|&f| f == at.file);
        let (file, rendered) = match i {
            Some(i) => (out[i].file, Some(&rendered[i])),
            None => (first_edit(out, rendered).file, None),
        };
        let (range, wrote) = rendered.map_or((TextRange::default(), None), |r| r.origin(at.range.start()));
        let message = match wrote {
            Some(wrote) => format!("cannot write {wrote} in edition 1.0: {what}"),
            None => format!("cannot write this program in edition 1.0: its text does not read back: {what}"),
        };
        Diagnostic::error(FileRange { file, range }, message)
    };

    let faults: Vec<Diagnostic> =
        read.check(&read_roots).into_iter().filter(Diagnostic::is_error).map(|d| back(d.range, d.message.clone())).collect();
    if !faults.is_empty() {
        return Err(faults);
    }

    let at = first_edit(out, rendered);
    let refused = |message: String| vec![Diagnostic::error(at, message)];
    let theirs = walk(db, roots).map_err(|why| refused(format!("cannot prove the program's text in edition 1.0: the program's walk into the backend stopped: {why}")))?;
    let ours = walk(&read, &read_roots).map_err(|why| refused(format!("cannot prove the program's text in edition 1.0: its walk into the backend stopped: {why}")))?;
    if let Some(call) = parted(&theirs, &ours) {
        let show = |w: &[String]| w.get(call).cloned().unwrap_or_else(|| "(the end)".to_owned());
        return Err(vec![
            Diagnostic::error(at, "cannot write this program in edition 1.0: its text is another program, as the walks into the backend show")
                .with_note(format!("at backend call {call}, the program has `{}` and the text `{}`", show(&theirs), show(&ours))),
        ]);
    }
    Ok(())
}

/// The first call the two walks differ at.
pub(crate) fn parted(a: &[String], b: &[String]) -> Option<usize> {
    (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i))
}

/// Where a fault that belongs to no one place is shown: the first edit that
/// wrote text, or that deleted some.
fn first_edit(out: &[Expanded], rendered: &[Rendered]) -> FileRange {
    for (e, r) in out.iter().zip(rendered) {
        if let Some(span) = r.spans.iter().find(|s| s.what.is_some()) {
            return FileRange { file: e.file, range: span.from };
        }
        if let Some(range) = r.first_edit {
            return FileRange { file: e.file, range };
        }
    }
    let file = out.first().map_or(FileId::SYNTHETIC, |e| e.file);
    FileRange { file, range: TextRange::default() }
}

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("no panics hold this lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The program's walk through the trace backend: its calls, without sequence
/// numbers and locations.
pub(crate) fn walk(db: &Db, roots: &[FileId]) -> Result<Vec<String>, String> {
    let buf = SharedBuf::default();
    let mut trace = kg_ptbridge::trace::Trace::new(Box::new(buf.clone()));
    let mut options = kg_backend_sys::options::OptionValues::harness_defaults();
    options.target_device = db.settings().target_device.clone();
    let outcome = kg_ptbridge::program::run(db, roots, options, None, &mut trace);
    trace.flush();
    if let Some(declined) = outcome.declined {
        return Err(declined.reason.to_string());
    }
    let text = String::from_utf8_lossy(&buf.0.lock().expect("no panics hold this lock")).into_owned();
    Ok(text
        .lines()
        .filter_map(|l| l.splitn(3, ' ').nth(2))
        .filter(|c| !c.starts_with("SetLocation2") && !c.starts_with("UnknownLocation"))
        .map(str::to_owned)
        .collect())
}
