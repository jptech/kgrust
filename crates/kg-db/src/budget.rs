//! How far instantiation may iterate: the reference's loop is unbounded,
//! and the language server answers twice on the way.

/// How many instantiation rounds the program-wide loop may run
/// (`kg_sem::elaborate::elaborate_opts`), and whether an answer is taken
/// on the way.
///
/// The reference's instantiation loop is unbounded by default:
/// `template_iterations = maxBound` (`Options/CmdArgs.hs:105`), iterating
/// `instantiateTemplates` until no instance is decided (`Template.hs:50-62`).
/// A batch compile does the same here ([`PassBudget::Fixpoint`], the default).
/// The language server runs to the same fixpoint, and answers once on the
/// way ([`PassBudget::Layered`]): each round re-infers the whole program,
/// and a keystroke on `risc-v.k` waits 0.9 s for four rounds against 2.3 s
/// for the fixpoint (2026-10-10).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum PassBudget {
    /// Until nothing is decided, as the reference runs —
    /// `kg_sem::elaborate::ROUND_GUARD` rounds at most, which reaching
    /// means the loop does not converge, a bug here, reported as exhaustion
    /// (`Db::budget_exhausted`).
    #[default]
    Fixpoint,
    /// At most this many rounds of instantiation — `kanagawa
    /// --template-iterations` (`Template.hs:53-64`), where `0` is the pass
    /// list once and nothing instantiated. A reading of the language: what
    /// the bounded loop leaves is judged as the reference judges it.
    AtMost(u32),
    /// The language server's (sprint W7.6, the owner's answer of
    /// 2026-10-10): [`PassBudget::Fixpoint`], with a first answer taken as
    /// the run passes this many rounds and the run then carried on to the
    /// fixpoint — the rounds before the bound computed once
    /// (`kg_sem::elaborate::elaborate_layered`). The first answer is no
    /// reading of the language: a round it stops before may make an
    /// instance it left standing, so what judges the loop's leftovers says
    /// nothing there (`kg_sem::check::checked`'s `truncated`) and the gutter
    /// may miss a diagnostic, never gain one; the last is `kg check`'s.
    /// [`crate::Db::diagnostics_layered`] answers with both,
    /// [`crate::Db::editor_loop`] with the newest ready.
    Layered(u32),
}

impl PassBudget {
    /// The language server's: a first answer at four rounds, then the
    /// fixpoint. Four was the twin route's measured knee of walk agreements
    /// against keystroke latency (2026-09-20), and the editor's whole bound
    /// until 2026-10-10.
    pub const EDITOR: PassBudget = PassBudget::Layered(4);

    /// The loop's round bound under this budget.
    pub(crate) fn rounds(self) -> u32 {
        match self {
            PassBudget::Fixpoint | PassBudget::Layered(_) => kg_sem::elaborate::ROUND_GUARD,
            PassBudget::AtMost(n) => n,
        }
    }

    /// The round a first answer is taken at, under a layered budget.
    pub fn first_answer(self) -> Option<u32> {
        match self {
            PassBudget::Layered(n) => Some(n),
            PassBudget::Fixpoint | PassBudget::AtMost(_) => None,
        }
    }
}
