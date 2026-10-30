//! The Kanagawa lexer and parser.
//!
//! Deliberately free of any syntax-tree dependency: the parser emits a flat
//! event stream that a consumer (`kg-syntax`) turns into a tree. That keeps this
//! crate fuzzable and benchmarkable on its own, and keeps the tree
//! representation swappable.
//!
//! ```text
//! text ──LexedStr──▶ tokens ──Input──▶ Parser ──▶ Vec<Event> ──build_tree──▶ Step stream
//! ```

mod event;
mod grammar;
mod input;
mod lexer;
mod output;
mod parser;
mod syntax_kind;
mod token_set;

pub use event::Event;
pub use input::Input;
pub use lexer::{LexError, LexedStr};
pub use output::{Step, StepOwned, build_tree, to_steps};
pub use parser::{CompletedMarker, Marker, Parser, STALLED};
pub use syntax_kind::SyntaxKind;
pub use token_set::TokenSet;

/// Parses a whole source file.
///
/// Returns the event stream; pair it with the same [`LexedStr`] in
/// [`build_tree`] to get a lossless tree. Never fails — a file of pure garbage
/// produces `ERROR` nodes and diagnostics, because a language server must be
/// able to describe a file that is halfway through being typed.
pub fn parse_source_file(lexed: &LexedStr<'_>) -> Vec<Event> {
    parse_input(&Input::from_lexed(lexed))
}

/// Parses a prepared [`Input`] — one [`Input::from_lexed`] built and the
/// caller then annotated ([`Input::mark_comparison`]).
pub fn parse_input(input: &Input<'_>) -> Vec<Event> {
    let mut p = Parser::new(input);
    grammar::source_file(&mut p);
    p.finish()
}
