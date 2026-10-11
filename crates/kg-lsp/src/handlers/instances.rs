//! How a type the template's copies give is shown — shared by [`super::hover`],
//! [`super::inlay_hints`], [`super::signature_help`] and
//! [`super::completion`], so the four say it the same way.
//!
//! # Why a list, and not one type
//!
//! A template's body has no type of its own: `inferType` visits a template
//! for its condition only (`Inference.hs:63`), and the program-wide loop
//! types each instance's copy instead (`kg_db::looped::IrAnswer::Copies`).
//! Kanagawa's widths follow the arguments — `auto w = v + 1` is `uint9` in
//! `take<uint8>` and `uint17` in `take<uint16>` — so no one copy's type is the
//! body's, and picking one would state a fact about a single instance as if
//! it held for the text. Every distinct type is shown, each with the
//! instances that give it, in the order the loop decided them, and the
//! reader is told when the list may be short: an instance whose copy has no
//! type yet, or a loop short of its fixpoint — the editor's first answer
//! while the fixpoint runs on (`kg_db::PassBudget::Layered`).
//!
//! The instance is spelled as the reference's messages spell one
//! (`unmangleInstanceName`, `Type.hs:176-177`) — `take<uint8>`, the text a
//! `while compiling` note points at — and each type with `show_pretty`, as
//! everywhere else in the editor.

use kg_base::Interner;
use kg_db::looped::{IrAnswer, Looped};
use kg_ty::Ty;
use kg_ty::subtype::show_pretty;

/// Instance names past this many in one line are counted, not listed: a
/// hot template's body has dozens of instances, and a tooltip is not a
/// report.
const LISTED: usize = 4;

/// The distinct types past this many in one label are elided: the tooltip
/// lists them all, and a label wider than the line it annotates is noise.
const LABELLED: usize = 3;

/// The one-line form, where there is room for no more than the types: each
/// distinct one, `uint9 | uint17` — the binding is one of these, depending
/// on the instance, as a union reads — the inlay hint's label and a
/// completion's detail.
pub(crate) fn label(types: &[(Ty, Vec<String>)], names: &Interner) -> String {
    let mut out = types.iter().take(LABELLED).map(|(ty, _)| show_pretty(ty, Some(names))).collect::<Vec<_>>().join(" | ");
    if types.len() > LABELLED {
        out.push_str(" | …");
    }
    out
}

/// The long form beside [`label`]: a fence of each type with the instances
/// that give it, and the [`caveats`] — the inlay hint's tooltip and a
/// completion's documentation.
pub(crate) fn tooltip(types: &[(Ty, Vec<String>)], untyped: &[String], budget_exhausted: bool, names: &Interner) -> String {
    let lines: Vec<(String, String)> =
        types.iter().map(|(ty, instances)| (show_pretty(ty, Some(names)), listed(instances))).collect();
    let typed = types.iter().map(|(_, i)| i.len()).sum();
    format!("{}\n\n{}", fence(&lines), caveats(typed, untyped, budget_exhausted))
}

/// `a, b, c and 3 more`.
pub(crate) fn listed(instances: &[String]) -> String {
    let named: Vec<&str> = instances.iter().map(String::as_str).filter(|n| !n.is_empty()).collect();
    let unnamed = instances.len() - named.len();
    let mut out = named.iter().take(LISTED).copied().collect::<Vec<_>>().join(", ");
    let rest = named.len().saturating_sub(LISTED) + unnamed;
    if rest > 0 {
        if !out.is_empty() {
            out.push_str(" and ");
        }
        out.push_str(&format!("{rest} {}", if out.is_empty() { "unnamed" } else { "more" }));
    }
    out
}

/// A `kanagawa` code fence of `code  // comment` lines, the comments
/// aligned, as one would write them by hand.
pub(crate) fn fence(lines: &[(String, String)]) -> String {
    let width = lines.iter().map(|(code, _)| code.chars().count()).max().unwrap_or(0);
    let mut out = String::from("```kanagawa\n");
    for (code, comment) in lines {
        out.push_str(code);
        if !comment.is_empty() {
            out.push_str(&" ".repeat(width - code.chars().count() + 2));
            out.push_str("// ");
            out.push_str(comment);
        }
        out.push('\n');
    }
    out.push_str("```");
    out
}

/// The instance a call compiles (`kg_db::looped::Looped::callee_at`), as
/// [`super::signature_help`]'s documentation and [`super::hover`]'s fence
/// show it: the function `name` as the instance has it, the instance named
/// beside it — inside a template's body, with the enclosing instances that
/// call it — and the line said under it.
pub(crate) fn called(looped: &Looped, answer: &IrAnswer, name: &str, names: &Interner) -> (Vec<(String, String)>, String) {
    let line = |ty: &Ty| format!("{} {name}", show_pretty(ty, Some(names)));
    let instance = |ty: &Ty| looped.instance_name(ty, names).unwrap_or_default();
    match answer {
        IrAnswer::Written(ty) => (vec![(line(ty), instance(ty))], "The instance this call compiles.".to_owned()),
        IrAnswer::Copies { types, untyped, budget_exhausted } => {
            let lines = types
                .iter()
                .map(|(ty, callers)| (line(ty), format!("{} in {}", instance(ty), listed(callers))))
                .collect();
            let typed = types.iter().map(|(_, c)| c.len()).sum();
            (lines, caveats(typed, untyped, *budget_exhausted))
        }
    }
}

/// What the reader is told under the list: why there is one, and when it
/// may be incomplete.
pub(crate) fn caveats(instances: usize, untyped: &[String], budget_exhausted: bool) -> String {
    let mut out = format!(
        "Typed in {instances} instance{}: a template's body has no type of its own.",
        if instances == 1 { "" } else { "s" }
    );
    if !untyped.is_empty() {
        out.push_str(&format!(" Not typed in {}.", listed(untyped)));
    }
    if budget_exhausted {
        out.push_str(
            " Instantiation had not reached its fixpoint when this was answered, so there may be \
             more instances.",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comments line up, and a line with nothing to say has none.
    #[test]
    fn a_fence_aligns_its_comments() {
        let lines = [
            ("uint9 w".to_owned(), "take<uint8>".to_owned()),
            ("uint17 w".to_owned(), "take<uint16>".to_owned()),
            ("bool b".to_owned(), String::new()),
        ];
        assert_eq!(
            fence(&lines),
            "```kanagawa\nuint9 w   // take<uint8>\nuint17 w  // take<uint16>\nbool b\n```"
        );
    }

    /// Past four names the rest are counted; an unnamed copy is counted,
    /// never shown as an empty name.
    #[test]
    fn a_long_list_is_cut_and_counted() {
        let names: Vec<String> = (1..=6).map(|n| format!("f<{n}>")).collect();
        assert_eq!(listed(&names[..2]), "f<1>, f<2>");
        assert_eq!(listed(&names), "f<1>, f<2>, f<3>, f<4> and 2 more");
        assert_eq!(listed(&[String::new()]), "1 unnamed");
        assert_eq!(listed(&["f<1>".to_owned(), String::new()]), "f<1> and 1 more");
    }
}
