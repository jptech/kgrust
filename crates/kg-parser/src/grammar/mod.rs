//! The Kanagawa grammar.
//!
//! Recursive descent with error recovery, producing a flat event stream. No
//! symbol table: see [`lookahead`] for how the two symbol-table-dependent
//! decisions of the reference parser are replaced by bounded token scans.
//!
//! Module ownership:
//!
//! | module | owns |
//! |---|---|
//! | [`items`] | source file, imports, modules, classes, structs, unions, enums, functions, variables, templates |
//! | [`attrs`] | `[[...]]` groups and the closed set of attribute names |
//! | [`types`] | every type form |
//! | [`paths`] | names, `::` paths, template argument lists |
//! | [`exprs`] | the Pratt expression parser, in both full and template modes |
//! | [`stmts`] | statements and blocks |

mod attrs;
mod exprs;
mod items;
mod lookahead;
mod paths;
mod stmts;
mod types;

#[cfg(test)]
mod tests;

use crate::{Parser, SyntaxKind, TokenSet};
use SyntaxKind::*;

/// Parses a whole file. The only entry point.
pub(crate) fn source_file(p: &mut Parser<'_>) {
    depth::reset();
    lookahead::reset();
    let m = p.start();
    items::source_file_contents(p);
    p.finish_stalled();
    m.complete(p, SOURCE_FILE);
}

/// The recursion budget.
///
/// Expressions, types, statements and declarations are mutually recursive, and
/// this is a recursive-descent parser, so source nesting depth is native stack
/// depth. The reference is recursive here too — `decltype` (`Parser.hs:546`)
/// and `hidden (parens expr)` (`:845`) both recurse without a limit — but the
/// reference is a batch compiler, where a pathological file fails one compile.
/// This parser is hosted in a language server whose workers get the default
/// 2 MiB stack, where the same input aborts the process: a stack overflow is
/// `SIGABRT`, not a `catch_unwind`-able panic, so nothing downstream can
/// contain it.
///
/// So depth is counted instead. Every cycle in the grammar passes through at
/// least one of [`items::item`], [`stmts::stmt_or_decl`], [`exprs::expr`] and
/// [`types::type_`], and each of those takes a guard; past the limit the parser
/// reports and recovers exactly as it does for any other malformed input.
pub(crate) mod depth {
    use std::cell::Cell;

    /// The deepest whole program in the reference corpus reaches 39
    /// (`tests::the_recursion_budget_has_headroom`), and a debug build spends
    /// between one and two kilobytes of frames per unit — measured by running
    /// `tests::deep_nesting_does_not_overflow_the_stack` on shrinking stacks,
    /// where 512 KiB survives and 256 KiB does not. So this leaves real code
    /// six times the room it uses and still fits a 2 MiB worker four times
    /// over, in the build with the fattest frames.
    pub(crate) const LIMIT: u32 = 256;

    pub(crate) const TOO_DEEP: &str = "nested too deeply to parse";

    thread_local! {
        static DEPTH: Cell<u32> = const { Cell::new(0) };
        /// Deepest point reached by the parse in progress, so a test can show
        /// how much headroom real code leaves.
        static PEAK: Cell<u32> = const { Cell::new(0) };
    }

    /// Claims one level of nesting, or `None` when the budget is spent.
    pub(crate) fn enter() -> Option<Guard> {
        DEPTH.with(|d| {
            let n = d.get();
            if n >= LIMIT {
                return None;
            }
            d.set(n + 1);
            PEAK.with(|p| {
                if n + 1 > p.get() {
                    p.set(n + 1);
                }
            });
            Some(Guard)
        })
    }

    pub(crate) fn reset() {
        DEPTH.with(|d| d.set(0));
        PEAK.with(|p| p.set(0));
    }

    /// The depth the parse just finished actually reached.
    #[cfg(test)]
    pub(crate) fn peak() -> u32 {
        PEAK.with(Cell::get)
    }

    /// Releases the level, including while a panic unwinds.
    #[derive(Debug)]
    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }
}

/// Which operators the expression parser may use.
///
/// Inside `<...>` the reference removes `<`, `>`, `<<` and `>>` from the
/// operator table (`opsTemplate = ops' [] []`, `Parser.hs:913`), which is what
/// makes a template argument list terminable. A parenthesized sub-expression
/// restores the full table (`Parser.hs:845`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum ExprMode {
    Full,
    TemplateArg,
}

/// Tokens that can begin a declaration or statement, used to resynchronize
/// after an error rather than consuming the rest of the file.
pub(crate) const ITEM_RECOVERY: TokenSet = TokenSet::new(&[
    CLASS_KW,
    STRUCT_KW,
    UNION_KW,
    ENUM_KW,
    TEMPLATE_KW,
    USING_KW,
    IMPORT_KW,
    MODULE_KW,
    EXPORT_KW,
    EXTERN_KW,
    STATIC_ASSERT_KW,
    PUBLIC_KW,
    PRIVATE_KW,
    INLINE_KW,
    NOINLINE_KW,
    R_CURLY,
]);

pub(crate) const STMT_RECOVERY: TokenSet = TokenSet::new(&[
    SEMICOLON,
    R_CURLY,
    IF_KW,
    FOR_KW,
    WHILE_KW,
    DO_KW,
    SWITCH_KW,
    RETURN_KW,
    BREAK_KW,
    BARRIER_KW,
    REORDER_KW,
    ATOMIC_KW,
    STATIC_KW,
])
.union(ITEM_RECOVERY);
