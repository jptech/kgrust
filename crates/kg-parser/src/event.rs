//! The parser's output: a flat event stream.
//!
//! Keeping the parser's output flat — rather than having it build a tree
//! directly — is what lets this crate stay free of a tree dependency, and is
//! what makes `Marker::precede` possible: a node can decide *after* parsing its
//! contents that it wants a parent, which a direct tree builder cannot express.

use crate::SyntaxKind;

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// Opens a node.
    ///
    /// `forward_parent` chains this node into one that was created *later* and
    /// wants to become its ancestor (see [`crate::Marker::precede`]). The chain
    /// is resolved by [`process`] before a consumer ever sees it.
    Start { kind: SyntaxKind, forward_parent: Option<u32> },
    /// Closes the innermost open node.
    Finish,
    /// Consumes `n_raw_tokens` input tokens as a single tree token of `kind`.
    ///
    /// `n_raw_tokens > 1` only for the composites the lexer refuses to build:
    /// `<=`, `>=`, `<<`, `>>` (see [`crate::SyntaxKind::SHL`]).
    Token { kind: SyntaxKind, n_raw_tokens: u8 },
    /// A diagnostic attached at the current position.
    Error { msg: String },
    /// A slot whose event was moved elsewhere by [`process`]. Skipped.
    Tombstone,
}

impl Event {
    pub fn tombstone() -> Self {
        Event::Tombstone
    }
}

/// Resolves `forward_parent` chains so the stream can be consumed linearly.
///
/// After this, every `Start` appears in the order its node must be opened.
pub fn process(mut events: Vec<Event>) -> Vec<Event> {
    let mut out = Vec::with_capacity(events.len());
    let mut forward_parents = Vec::new();

    for i in 0..events.len() {
        match std::mem::replace(&mut events[i], Event::Tombstone) {
            Event::Start { kind, forward_parent } => {
                // Walk the forward-parent chain, collecting the ancestors that
                // were created after this node but must be opened before it.
                forward_parents.push(kind);
                let mut idx = i;
                let mut fp = forward_parent;
                while let Some(delta) = fp {
                    idx += delta as usize;
                    fp = match std::mem::replace(&mut events[idx], Event::Tombstone) {
                        Event::Start { kind, forward_parent } => {
                            forward_parents.push(kind);
                            forward_parent
                        }
                        other => unreachable!("forward parent pointed at {other:?}"),
                    };
                }
                // Outermost ancestor first.
                for kind in forward_parents.drain(..).rev() {
                    out.push(Event::Start { kind, forward_parent: None });
                }
            }
            Event::Tombstone => {}
            other => out.push(other),
        }
    }
    out
}
