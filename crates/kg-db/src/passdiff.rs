//! Sprint W3 step 1's instrument: a CST pass against the loop's version of
//! it, on each module's own text (`cargo xtask passdiff`).
//!
//! The CST side is the pass as the CST run calls it — in its place, over
//! the sink as it stands there (an earlier verdict keeps a later one quiet,
//! `null (errors a)`, `TypeCheck.hs:162`), and only in a module whose run
//! gets past the checkpoints to it (`Frontend.hs:38-52`) — run on a copy of
//! the sink, so the run's own verdicts are what they are unrecorded. The
//! loop's side is what the run admits in the pass's place once the pass is
//! switched, recorded at the same place: what
//! [`crate::looped::Looped::text_for`] files for that module, an undecided
//! verdict only where the program's flag lets it through
//! (`kg_sem::check::TextVerdict::undecided`). So the two identical in every module the run reaches the pass
//! in is the switch changing no diagnostic anywhere; a module the run
//! stops short of is not compared, as neither side is admitted there.
//!
//! The parser's own checks are compared the same way ([`Probe::Parser`]):
//! a check whose oracle moves off the CST engine's type table onto what
//! the reference's parser built ([`crate::parsed`]) is run in its place
//! under both, on copies of the sink, and the run admits one. The
//! reference's verdict on such a check is a parse error.
//!
//! The recording is per thread: the CST run of one program happens on the
//! thread that asks for its diagnostics ([`crate::Db::check`]).

use std::cell::RefCell;

use kg_base::FileId;
use kg_diag::Diagnostic;

use crate::db::Db;

pub use kg_sem::check::TextPass;

/// What a differential compares: a pass the loop judges at a module's
/// text, or one of the parser's own checks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Probe {
    Text(TextPass),
    Parser(ParserCheck),
}

/// The parser's checks that asked the CST engine's type table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParserCheck {
    /// `expression`'s "Invalid use of a type as an expression"
    /// (`Parser.hs:823-828`) and `namedLValue`'s constant receiver
    /// (`:795-800`) — `early_normalization::pass_0_types_and_constants_as_values`:
    /// whether a name the text writes where a value goes is a type.
    TypesAsValues,
    /// `methodInstanceSpecifier`'s "undefined template" (`Parser.hs:766-770`)
    /// — `undefined_symbols::parse_time_find_symbol`: whether a method
    /// written with `<…>` names a template of the receiver's type, the CST
    /// engine's type of the receiver against the loop's.
    MemberTemplates,
}

impl Probe {
    /// Every probe, in the order a listing names them.
    pub fn all() -> Vec<Probe> {
        let parser = [ParserCheck::TypesAsValues, ParserCheck::MemberTemplates].map(Probe::Parser);
        TextPass::ALL.into_iter().map(Probe::Text).chain(parser).collect()
    }

    /// The name `cargo xtask passdiff --pass` takes.
    pub fn name(self) -> &'static str {
        match self {
            Probe::Text(pass) => pass.name(),
            Probe::Parser(ParserCheck::TypesAsValues) => "typesAsValues",
            Probe::Parser(ParserCheck::MemberTemplates) => "memberTemplates",
        }
    }

    /// Whether the reference's verdicts on it are its parser's
    /// (`PARSE-ERROR`, `Main.hs:73-75`).
    pub fn parse_time(self) -> bool {
        matches!(self, Probe::Parser(_))
    }
}

/// The probes whose CST code is still there, so that the CST run calls it
/// in their place through a judge that records both sides ([`record`]):
/// the only ones there is anything to compare for. A probe whose CST code
/// is gone records nothing, and a reading of it would be green for want of
/// a CST side. Empty: passes 2 to 19 lost their CST code by 2026-10-10,
/// passes 4 and 8 last, and the parser's two checks that asked the CST
/// engine's type table theirs in sprint W3.7 — what the parser built and
/// the loop's types answer them. A move brings a judge back.
pub const PROBED: &[Probe] = &[];

thread_local! {
    static RECORDING: RefCell<Option<Recording>> = const { RefCell::new(None) };
}

/// What one run recorded: each module that reached `pass`, with the CST
/// pass's verdicts there and the loop's the run admits.
struct Recording {
    probe: Probe,
    reached: Vec<ModuleDiff>,
}

/// What a judge in the CST run calls in `probe`'s place: while this thread
/// records `probe`, what the CST side adds there and the other side's
/// verdicts the run admits.
pub fn record(probe: Probe, file: FileId, cst: &[Diagnostic], ir: &[Diagnostic]) {
    RECORDING.with(|r| {
        if let Some(recording) = r.borrow_mut().as_mut().filter(|x| x.probe == probe) {
            recording.reached.push(ModuleDiff { file, cst: cst.to_vec(), ir: ir.to_vec() });
        }
    });
}

/// Restores the thread's recording as it was, however the run ends.
struct Restore(Option<Option<Recording>>);

impl Drop for Restore {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            RECORDING.with(|r| *r.borrow_mut() = previous);
        }
    }
}

/// One module whose CST run reached the pass: what each side says at its
/// text.
#[derive(Debug)]
pub struct ModuleDiff {
    pub file: FileId,
    pub cst: Vec<Diagnostic>,
    pub ir: Vec<Diagnostic>,
}

impl ModuleDiff {
    /// The CST side's verdicts the loop's lacks, and the loop's the CST
    /// side's lacks — each diagnostic compared whole (range, message,
    /// labels, kind, fixes), as many times as it is given.
    pub fn apart(&self) -> (Vec<&Diagnostic>, Vec<&Diagnostic>) {
        let key = |d: &Diagnostic| format!("{d:?}");
        let mut ir: Vec<(String, &Diagnostic)> = self.ir.iter().map(|d| (key(d), d)).collect();
        let mut cst_only = Vec::new();
        for d in &self.cst {
            let k = key(d);
            match ir.iter().position(|(x, _)| *x == k) {
                Some(i) => {
                    ir.swap_remove(i);
                }
                None => cst_only.push(d),
            }
        }
        let mut ir_only: Vec<&Diagnostic> = ir.into_iter().map(|(_, d)| d).collect();
        ir_only.sort_by_key(|d| (d.range.file, d.range.range.start()));
        (cst_only, ir_only)
    }
}

/// `probe` over the program rooted at `roots`, in every module its CST run
/// reaches it in: the CST side's verdicts there against the other's.
/// Empty where no run reaches it — a program that does not parse, say.
pub fn differential(db: &Db, roots: &[FileId], probe: Probe) -> Vec<ModuleDiff> {
    let restore = Restore(Some(RECORDING.with(|r| r.replace(Some(Recording { probe, reached: Vec::new() })))));
    let _ = db.check(roots);
    let recording = RECORDING.with(|r| r.borrow_mut().take()).expect("recording since the run began");
    drop(restore);
    recording.reached
}

#[cfg(test)]
mod tests {
    use super::{PROBED, Probe, differential};

    /// [`PROBED`] is every probe the CST run still calls in its place, and
    /// no other: on a clean program each probed one is reached, one whose
    /// CST code is gone is not. Red with a probe left off the list, and
    /// with one on it whose CST code is gone.
    #[test]
    fn probed_is_every_pass_the_cst_run_still_calls() {
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.implicit_base = false;
        let mut db = crate::Db::new(settings);
        let main = db.set_file_text(&kg_base::Utf8PathBuf::from("/proj/main.k"), "inline void main()\n{\n}\n".to_owned());
        for probe in Probe::all() {
            assert_eq!(!differential(&db, &[main], probe).is_empty(), PROBED.contains(&probe), "{probe:?}");
        }
    }
}
