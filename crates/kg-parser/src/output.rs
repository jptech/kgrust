//! Turning events back into a token/tree stream, with trivia put back.
//!
//! The parser never sees trivia, so something has to decide which node each
//! comment belongs to. That decision is here, in one place, because it is the
//! difference between a formatter that preserves comments and one that moves
//! them somewhere surprising.

use kg_base::TextSize;

use crate::{Event, LexedStr, SyntaxKind};
use SyntaxKind::*;

/// One instruction for a tree builder.
#[derive(Debug)]
pub enum Step<'a, 'e> {
    Enter { kind: SyntaxKind },
    Token { kind: SyntaxKind, text: &'a str },
    Exit,
    Error { msg: &'e str, offset: TextSize },
}

/// Replays `events` against `lexed`, re-inserting trivia, and drives `sink`.
///
/// Guarantees: the concatenation of every `Token` text equals `lexed.as_str()`,
/// and `Enter`/`Exit` are balanced.
pub fn build_tree<'a>(lexed: &'a LexedStr<'a>, events: Vec<Event>, sink: &mut dyn FnMut(Step<'a, '_>)) {
    let events = crate::event::process(events);
    let mut builder = Builder { lexed, sink, raw: 0, state: State::PendingEnter };

    // Errors carry an owned message so the borrow of `events` ends here; the
    // sink sees a `&str` valid for the callback only, which is all it needs.
    for event in events {
        match event {
            Event::Start { kind, .. } => builder.enter(kind),
            Event::Finish => builder.exit(),
            Event::Token { kind, n_raw_tokens } => builder.token(kind, n_raw_tokens as usize),
            Event::Error { msg } => {
                // An error raised before the next token is consumed lands on
                // *that token*, not on the trivia in front of it: `raw` may
                // still point at a blank line or a comment, and megaparsec's
                // offset is taken after `whitespace` has run, so the reference
                // reports `static uint32 g;` at its `static`, never at the
                // comment above it (`basics.k::25`).
                let mut i = builder.raw.min(builder.lexed.len());
                while i < builder.lexed.len() && builder.lexed.kind(i).is_trivia() {
                    i += 1;
                }
                let offset = builder.lexed.text_start(i);
                (builder.sink)(Step::Error { msg: &msg, offset });
            }
            Event::Tombstone => {}
        }
    }

    // Trailing trivia at end of file belongs to the root.
    match builder.state {
        State::PendingExit => {
            builder.eat_trivia(usize::MAX);
            (builder.sink)(Step::Exit);
        }
        State::Normal | State::PendingEnter => builder.eat_trivia(usize::MAX),
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum State {
    /// Nothing entered yet; the next `Enter` is the root and must precede all trivia.
    PendingEnter,
    Normal,
    /// An `Exit` is held back so trailing trivia can be routed before it closes.
    PendingExit,
}

struct Builder<'a, 'b> {
    lexed: &'a LexedStr<'a>,
    sink: &'b mut dyn FnMut(Step<'a, '_>),
    raw: usize,
    state: State,
}

impl<'a> Builder<'a, '_> {
    fn token(&mut self, kind: SyntaxKind, n_raw: usize) {
        match std::mem::replace(&mut self.state, State::Normal) {
            State::PendingEnter => unreachable!("a token before the root node"),
            State::PendingExit => (self.sink)(Step::Exit),
            State::Normal => (),
        }
        self.eat_trivia(usize::MAX);

        // A composite (`<<`, `>=`, ...) spans several raw tokens; emit their
        // combined text as one tree token so the CST still round-trips.
        let start = self.lexed.text_start(self.raw);
        let end = self.lexed.range(self.raw + n_raw - 1).end();
        let text = &self.lexed.as_str()[usize::from(start)..usize::from(end)];
        self.raw += n_raw;
        (self.sink)(Step::Token { kind, text });
    }

    fn enter(&mut self, kind: SyntaxKind) {
        match std::mem::replace(&mut self.state, State::Normal) {
            State::PendingEnter => {
                (self.sink)(Step::Enter { kind });
                return;
            }
            State::PendingExit => (self.sink)(Step::Exit),
            State::Normal => (),
        }

        // Decide how much of the upcoming trivia belongs *inside* this node:
        // the comments directly above a declaration document it, so they must
        // be its children, not its previous sibling's trailing junk.
        let n_trivia = self.pending_trivia();
        let attached = n_attached_trivia(kind, self.lexed, self.raw, n_trivia);
        self.eat_trivia(n_trivia - attached);
        (self.sink)(Step::Enter { kind });
        self.eat_trivia(attached);
    }

    fn exit(&mut self) {
        match std::mem::replace(&mut self.state, State::PendingExit) {
            State::PendingEnter => unreachable!("an exit before the root node"),
            State::PendingExit => (self.sink)(Step::Exit),
            State::Normal => (),
        }
    }

    /// Number of consecutive trivia tokens at the cursor.
    fn pending_trivia(&self) -> usize {
        (self.raw..self.lexed.len()).take_while(|&i| self.lexed.kind(i).is_trivia()).count()
    }

    fn eat_trivia(&mut self, limit: usize) {
        let mut taken = 0;
        while taken < limit && self.raw < self.lexed.len() {
            let kind = self.lexed.kind(self.raw);
            if !kind.is_trivia() {
                break;
            }
            (self.sink)(Step::Token { kind, text: self.lexed.text(self.raw) });
            self.raw += 1;
            taken += 1;
        }
    }
}

/// Kinds whose leading comments document them and so belong inside the node.
///
/// `//|` explicitly means "documents what follows"
/// (`Parser/Lexer.hs:63-65`), so for these kinds attachment is not a heuristic
/// but the language's own convention.
fn documents_following_item(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        CLASS_DECL
            | STRUCT_DECL
            | UNION_DECL
            | ENUM_DECL
            | ENUM_VARIANT
            | FN_DECL
            | FN_PROTO
            | VAR_DECL
            | DECL_STMT
            | ALIAS_DECL
            | TEMPLATE_DECL
            | EXPORT_DECL
            | EXTERN_DECL
            | IMPORT_DECL
            | MODULE_DECL
            | STATIC_ASSERT_DECL
            | PARAM
            | TYPE_PARAM
            | VALUE_PARAM
    )
}

/// How many of the `n` trivia tokens at `raw` should become children of a node
/// of `kind`.
///
/// Walking backwards from the node: comments are attached until a blank line
/// (a whitespace run with two or more newlines) breaks the association, which
/// is what separates a file's license header from its first declaration.
fn n_attached_trivia(kind: SyntaxKind, lexed: &LexedStr<'_>, raw: usize, n: usize) -> usize {
    if !documents_following_item(kind) || n == 0 {
        return 0;
    }
    let mut attached = 0;
    let mut pending_ws = 0;
    for i in (raw..raw + n).rev() {
        match lexed.kind(i) {
            WHITESPACE => {
                if lexed.text(i).bytes().filter(|&b| b == b'\n').count() >= 2 {
                    break;
                }
                pending_ws += 1;
            }
            k if k.is_comment() => {
                // A `//<` comment documents what *precedes* it, so it must stay
                // outside — it belongs to the previous sibling.
                if k.is_trailing_doc() {
                    break;
                }
                attached += pending_ws + 1;
                pending_ws = 0;
            }
            _ => break,
        }
    }
    attached
}

/// Convenience wrapper: collects the steps into a flat list. Used by tests and
/// by `kg dump ast`.
pub fn to_steps<'a>(lexed: &'a LexedStr<'a>, events: Vec<Event>) -> Vec<StepOwned> {
    let mut out = Vec::new();
    build_tree(lexed, events, &mut |step| {
        out.push(match step {
            Step::Enter { kind } => StepOwned::Enter(kind),
            Step::Token { kind, text } => StepOwned::Token(kind, text.to_owned()),
            Step::Exit => StepOwned::Exit,
            Step::Error { msg, offset } => StepOwned::Error(msg.to_owned(), offset),
        })
    });
    out
}

#[derive(Debug, PartialEq, Eq)]
pub enum StepOwned {
    Enter(SyntaxKind),
    Token(SyntaxKind, String),
    Exit,
    Error(String, TextSize),
}
