//! `textDocument/documentHighlight` — the other occurrences, in this file, of
//! the declaration under the cursor.
//!
//! # This used to match text, and text was wrong
//!
//! The first version of this module matched identifiers by *spelling*, and said
//! so: "name resolution does not exist yet". It does now
//! ([`kg_db::Db::definition_at`]), and the textual answer is not a rougher
//! version of the resolved one — it is a different answer, wrong in the two
//! shapes an editor meets constantly:
//!
//! * **shadowing.** Kanagawa's `findInScopes` takes the first enclosing scope
//!   that binds a name and stops (`SymbolTable.hs:256-259`), so in
//!   `{ uint32 hidden = 1; } uint32 hidden = 2;` the two `hidden`s are unrelated
//!   symbols. Lighting both up tells the reader they are one thing;
//! * **containers.** `class A { uint32 total; }` and
//!   `class B { uint32 total; }` share nothing but a word, and a textual match
//!   lights both from either caret.
//!
//! It also under-reported: a qualified use and a bare use of one declaration are
//! spelled differently and never highlighted together.
//!
//! So this is now the **single-file restriction of
//! [`crate::handlers::references`]** — literally that module's
//! `references::occurrences`, given a search plan of one file — rather than a
//! second occurrence finder that could disagree with find-references about what
//! the word under the cursor means.
//!
//! # What it costs, measured
//!
//! A client sends this on cursor movement, which is the highest request rate in
//! the protocol, so the cost is worth stating rather than assuming. One request
//! is: resolve the caret, then resolve every identically-spelled identifier *in
//! this file only* — the program is never walked, and every parse and symbol map
//! involved is memoized. Warm, on the reference corpus's three largest files
//! (temporary instrumentation, removed):
//!
//! | file | caret on | candidates | time |
//! |---|---|---|---|
//! | `test/syntax/basics.k` (5,003 lines) | `expected` | 82 | 0.6 ms |
//! | `library/numeric/float32.k` (3,905 lines) | `exponent` | 136 | 0.1 ms |
//! | `test/library/processor/risc-v.k` (4,084 lines) | `decoded` | 722 | 13.7 ms |
//!
//! So roughly 19 µs per candidate, and the worst case in the corpus is a name
//! appearing 722 times in one file. That is fine on the task pool — the main
//! loop is never blocked (`main_loop`) — and it is the number to watch if this
//! ever feels slow: it scales with *one name's* frequency in *one file*, not
//! with the project.
//!
//! # What it declines
//!
//! Everything [`kg_db::Db::definition_at`] declines — a struct or union field, an
//! import qualifier, an intrinsic, an ambiguous name, a caret on punctuation —
//! and, now, **keywords**: the old textual version lit up every `class` in the
//! file, which is a fact about the lexer rather than about the program.
//! Declining is a visible, correct answer, and a client that wants word-based
//! highlighting for those cases already has one.
//!
//! Every occurrence is still reported as [`DocumentHighlightKind::TEXT`].
//! `READ`/`WRITE` is a separate claim — it needs the assignment-target analysis
//! that nothing here performs — and guessing at it would be a new lie in place
//! of the one this rewrite removed.

use lsp_types::{DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams};

use crate::convert;
use crate::global_state::GlobalState;
use crate::handlers::{Answer, references};

pub fn document_highlight(
    state: &GlobalState,
    params: &DocumentHighlightParams,
) -> Answer<Vec<DocumentHighlight>> {
    let at = &params.text_document_position_params;
    let snap = state.snapshot(&at.text_document.uri)?;
    let enc = state.encoding();
    let offset = convert::offset(snap.line_index(), enc, at.position);

    let hits = state.with_db(snap.file, |db| {
        // The file is its own root, matching `frontend_diagnostics`,
        // `handlers::definition` and `handlers::references`: a highlight and a
        // jump at the same caret must name the same declaration.
        let Some(target) = db.definition_at(&[snap.file], snap.file, offset) else {
            return Vec::new();
        };
        let Some(name) = target.qualified_name.last() else { return Vec::new() };
        let name = db.interner().resolve(name);
        // A search plan of exactly one file, searched under its own program.
        // That restriction is the whole difference between this and
        // find-references: a highlight is a decoration of the visible buffer,
        // and reporting a position in a file the user cannot see is not one.
        references::occurrences(db, &[(snap.file, vec![snap.file])], &target, Some(name.as_str()))
    });

    Ok(hits
        .unwrap_or_default()
        .into_iter()
        // Every hit is in `snap.file` by construction, so the snapshot's own
        // line table is the right one — and it is the same table the caret was
        // converted through, so a position in and a range out describe one text.
        .map(|hit| DocumentHighlight {
            range: convert::range(snap.line_index(), enc, hit.range),
            kind: Some(DocumentHighlightKind::TEXT),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_state::{NoFile, test_uri};
    use lsp_types::{
        PartialResultParams, Position, TextDocumentIdentifier, TextDocumentPositionParams, Uri,
        WorkDoneProgressParams,
    };

    fn params(uri: &Uri, position: Position) -> DocumentHighlightParams {
        DocumentHighlightParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        }
    }

    /// The highlights as `(line, column)`, which is all these fixtures are about.
    #[track_caller]
    fn highlights(text: &str, pos: Position) -> Vec<(u32, u32)> {
        let state = GlobalState::new();
        let uri = test_uri("highlight.k");
        state.did_open(uri.clone(), text.to_owned(), 1);
        document_highlight(&state, &params(&uri, pos))
            .expect("the document is open")
            .into_iter()
            .inspect(|h| assert_eq!(h.kind, Some(DocumentHighlightKind::TEXT)))
            .map(|h| (h.range.start.line, h.range.start.character))
            .collect()
    }

    /// A global and its two uses are one symbol, so all three light up.
    #[test]
    fn every_occurrence_of_the_identifier_lights_up() {
        let src = "uint32 total;\ninline void f() { total = total + 1; }\n";
        let out = highlights(src, Position::new(0, 8));
        assert_eq!(out, vec![(0, 7), (1, 18), (1, 26)]);
    }

    /// The caret is *between* the last character of the word and whatever
    /// follows, which is where it sits after a double-click or an arrow key.
    ///
    /// The fixture declares the name rather than using two bare `total;`
    /// statements as it once did: a highlight is now a statement about a
    /// declaration, and a file that declares nothing has nothing to say. The
    /// behaviour being pinned — a caret immediately after a word addresses that
    /// word (`kg_db::navigate`'s `identifier_at`) — is unchanged.
    #[test]
    fn a_caret_just_past_the_word_still_finds_it() {
        let src = "uint32 total;\ninline void f() { total = 1; }\n";
        // Column 12 is the `;` immediately after `total` on line 0.
        assert_eq!(highlights(src, Position::new(0, 12)), vec![(0, 7), (1, 18)]);
    }

    /// The lexical guarantee a textual matcher already had, and which resolution
    /// keeps: neither a comment nor a string literal contains an identifier.
    #[test]
    fn a_matching_word_inside_a_comment_or_string_is_not_an_occurrence() {
        let src = "// total\nuint32 total;\nstring s = \"total\";\n";
        let out = highlights(src, Position::new(1, 8));
        assert_eq!(out, vec![(1, 7)]);
    }

    /// **The bug this rewrite exists to fix.** Eight `x`es, two symbols
    /// (`SymbolTable.hs:256-259`), and neither query may report the other's.
    ///
    /// The textual version returned all eight for either caret, which is not a
    /// rough answer but a wrong one: it tells the reader that assigning to the
    /// inner `x` changes the outer one.
    #[test]
    fn a_binding_and_the_one_it_shadows_do_not_light_up_together() {
        let src = "\
inline void main()
{
    uint32 x = 1;
    x = x + 1;
    {
        bool x;
        x = x + 1;
    }
    x = x + 2;
}
";
        let outer = highlights(src, Position::new(2, 11));
        let inner = highlights(src, Position::new(5, 13));
        assert_eq!(outer, vec![(2, 11), (3, 4), (3, 8), (8, 4), (8, 8)]);
        assert_eq!(inner, vec![(5, 13), (6, 8), (6, 12)]);
        // Disjoint, and between them they account for every `x`: neither query
        // lost an occurrence to the other.
        assert!(outer.iter().all(|hit| !inner.contains(hit)));
        assert_eq!(outer.len() + inner.len(), src.matches('x').count());
    }

    /// The other false positive, with no nesting involved: two containers with
    /// a field of the same name share a word and nothing else.
    #[test]
    fn a_field_of_another_class_with_the_same_name_does_not_light_up() {
        let src = "\
class A
{
    uint32 total;
    inline void bump() { total = total + 1; }
}

class B
{
    uint32 total;
}
";
        assert_eq!(highlights(src, Position::new(3, 25)), vec![(2, 11), (3, 25), (3, 33)]);
        assert_eq!(highlights(src, Position::new(8, 11)), vec![(8, 11)]);
    }

    /// The fixture `handlers::mod` used to drive through the dispatcher: a class
    /// field reached from its member function is one symbol with its two uses.
    #[test]
    fn a_class_field_and_its_uses_in_a_member_function_are_one_symbol() {
        let src = "\
class Adder {
    uint32 total;
    inline void add(uint32 x) {
        total = total + x;
    }
}
";
        assert_eq!(highlights(src, Position::new(1, 12)).len(), 3);
    }

    /// A caret on punctuation has no name to resolve, and lighting up every `+`
    /// in the file would be noise rather than information.
    #[test]
    fn punctuation_highlights_nothing() {
        assert!(highlights("x + y;\nz + w;\n", Position::new(0, 2)).is_empty());
    }

    /// A keyword is not a declaration. The textual version lit every `class` in
    /// the file — a fact about the lexer, not about the program — and this
    /// records that dropping it was a decision.
    #[test]
    fn a_keyword_is_not_a_declaration_and_lights_up_nothing() {
        let src = "class A\n{\n}\n\nclass B\n{\n}\n";
        assert!(highlights(src, Position::new(0, 2)).is_empty());
    }

    /// A struct field is in neither symbol map (`Symbols.hs:48-49`,
    /// `Parser.hs:465`), so a textual answer would light up the `const auto x`
    /// above the struct — the one thing this must never do. Since 2026-09-06
    /// a field is a navigation item resolved through the base expression's
    /// *type* (`Db::definition_at`), so the declaration and `p.x` light up
    /// together and the look-alike stays dark. Before that the whole request
    /// declined.
    #[test]
    fn a_field_lights_its_uses_and_never_a_look_alike() {
        let src = "\
const auto x = 1;

struct Point
{
    uint32 x;
}

inline void main()
{
    Point p;
    auto v = p.x + x;
}
";
        assert_eq!(highlights(src, Position::new(4, 11)), vec![(4, 11), (10, 15)]);
        assert_eq!(highlights(src, Position::new(10, 15)), vec![(4, 11), (10, 15)]);
    }

    #[test]
    fn an_empty_document_answers_nothing() {
        assert!(highlights("", Position::new(0, 0)).is_empty());
        // Past the end of the buffer, which a client may legitimately send
        // after an edit the server has not seen yet.
        assert!(highlights("uint32 total;\n", Position::new(999, 0)).is_empty());
    }

    /// A document the server never saw is `NoFile`, not an empty answer — the
    /// same distinction every other handler keeps.
    #[test]
    fn an_unknown_document_is_declined_not_faked() {
        let state = GlobalState::new();
        let uri = test_uri("never-opened.k");
        let err = document_highlight(&state, &params(&uri, Position::new(0, 0))).unwrap_err();
        assert_eq!(err, NoFile::NotOpen);
    }
}

