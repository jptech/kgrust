//! `textDocument/completion` — the protocol adapter over
//! [`kg_db::Db::completions_with`].
//!
//! Like [`super::definition`], nothing here re-decides resolution: which names
//! are offered is the query's answer, and this module only spells each one as
//! a [`CompletionItem`] — its kind from the declaration's [`DefKind`], its
//! detail from its type, and a sort key that keeps the query's order (scopes,
//! then file, then imports, then members) ahead of the client's alphabetical
//! one.
//!
//! # Which engine types a candidate
//!
//! The program-wide loop the buffer's diagnostics ran, by the query hover
//! reads: a member listed off the loop's type of the base carries that
//! instance's type, and any other candidate is typed where it is declared
//! ([`kg_db::Db::ir_answer_of_completion`]). A candidate declared inside a
//! template's body has a type per instance: its detail lists each, `uint9 |
//! uint17`, and its documentation says which instance gives which
//! (`super::instances`).
//!
//! Completion runs on every keystroke, so it reads the newest loop computed
//! for the program, mapped onto the text as it stands
//! ([`kg_db::Db::editor_loop_ready`]): the edit's own once the diagnostics
//! have run it, the one before while they do, and on a buffer that does not
//! parse — `a.` typed mid-statement is one — the last one that ran. It never
//! runs a loop and never waits for one, which on `risc-v.k` would make a
//! clean keystroke's completion the loop's ~2 s instead of ~100 ms, and a
//! broken one's a loop the diagnostics never run. A base typed since that
//! loop ran is typed by its name's declaration there.
//!
//! A candidate the loop has no type for — text no loop has seen, a
//! declaration typed since, every candidate where no loop has run at all —
//! has no detail, and a member list where no loop types the base is empty:
//! the CST inference engine answered both until sprint W3.7, deleted since.
//!
//! # Parameters to name
//!
//! In a file whose language has named arguments (edition 1.1:
//! `docs/editions/1.1.md`), an argument begun — after the `(` or a `,`, or
//! one word into it — is offered the callee's parameters no other argument
//! fills, each as `x:`, ahead of the names in scope
//! ([`kg_db::Db::label_completions`]).
//!
//! The list is complete, never incremental: `is_incomplete` is `false`, so a
//! client filters what it was given as the user keeps typing instead of asking
//! again per keystroke.

use kg_db::looped::IrAnswer;
use kg_resolve::{DeclareFlag, DefKind};
use kg_ty::Ty;
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse, Documentation, MarkupContent,
    MarkupKind,
};

use crate::convert;
use crate::global_state::GlobalState;
use crate::handlers::{Answer, instances};

pub fn completion(state: &GlobalState, params: &CompletionParams) -> Answer<Option<CompletionResponse>> {
    let at = &params.text_document_position;
    let snap = state.snapshot(&at.text_document.uri)?;
    let offset = convert::offset(snap.line_index(), state.encoding(), at.position);

    // `:` triggers completion for a path's `::`; typed alone — `public:`,
    // `case 1:`, a ternary's — it is no place for a list, and a client
    // would pop the scopes' names up at every one.
    let by_colon = params.context.as_ref().and_then(|c| c.trigger_character.as_deref()) == Some(":");
    if by_colon && !state.with_db(snap.file, |db| db.is_qualified_completion(snap.file, offset)).unwrap_or(false) {
        return Ok(None);
    }

    let items = state
        .with_db(snap.file, |db| {
            let roots = [snap.file];
            let looped = db.editor_loop_ready(&roots);
            let names = db.interner();
            // The parameters an argument begun here could name lead the
            // list: where one is offered, naming it is what the call wants.
            let labels = db.label_completions(&roots, snap.file, offset, looped.as_ref());
            let labelled = labels.len();
            labels
                .into_iter()
                .chain(db.completions_with(&roots, snap.file, offset, looped.as_ref()))
                .enumerate()
                .map(|(i, c)| {
                    let ir = looped.as_ref().and_then(|l| db.ir_answer_of_completion(&roots, l, &c, &shown));
                    let (detail, documentation) = match ir {
                        Some(IrAnswer::Written(ty)) => (Some(kg_ty::subtype::show_pretty(&ty, Some(names))), None),
                        Some(IrAnswer::Copies { types, untyped, budget_exhausted }) => (
                            Some(instances::label(&types, names)),
                            Some(Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: instances::tooltip(&types, &untyped, budget_exhausted, names),
                            })),
                        ),
                        // No loop yet, or nothing it can show: no detail.
                        None => (None, None),
                    };
                    let definition = db.completion_definition(&roots, &c);
                    // An enum constant's value beside its type, as the loop
                    // folded it: what picking between `mode::` constants
                    // needs to know.
                    let value = looped.as_ref().zip(definition.as_ref()).and_then(|(l, d)| db.enum_constant_value(l, d));
                    let detail = match (detail, value) {
                        (Some(ty), Some(v)) => Some(format!("{ty} = {v}")),
                        (None, Some(v)) => Some(format!("= {v}")),
                        (detail, None) => detail,
                    };
                    // Where the declaration is, for `completionItem/resolve` to
                    // read its documentation from when the item is focused —
                    // not for every candidate of a long list.
                    let data = definition.map(|def| {
                        serde_json::json!({
                            "root": snap.file.0,
                            "file": def.range.file.0,
                            "start": u32::from(def.range.range.start()),
                            "end": u32::from(def.range.range.end()),
                        })
                    });
                    let named = i < labelled;
                    CompletionItem {
                        label: if named { format!("{}:", c.name) } else { c.name.clone() },
                        insert_text: named.then(|| format!("{}: ", c.name)),
                        filter_text: named.then(|| c.name.clone()),
                        kind: Some(item_kind(c.kind)),
                        detail,
                        documentation,
                        // Rank first, then the query's own order within a rank.
                        sort_text: Some(format!("{}{:04}", c.rank, i)),
                        data,
                        ..CompletionItem::default()
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(Some(CompletionResponse::Array(items)))
}

/// `completionItem/resolve`: the focused candidate's documentation — its
/// declaration's `//|` and `//<` runs, as hover shows them — after whatever
/// the item already says (the instances' types of a template's member).
pub fn completion_resolve(state: &GlobalState, item: &CompletionItem) -> Answer<CompletionItem> {
    let mut item = item.clone();
    let field = |name: &str| item.data.as_ref()?.get(name)?.as_u64().and_then(|v| u32::try_from(v).ok());
    let (Some(root), Some(file), Some(start), Some(end)) = (field("root"), field("file"), field("start"), field("end"))
    else {
        return Ok(item);
    };
    let range = kg_base::FileRange {
        file: kg_base::FileId(file),
        range: kg_base::TextRange::new(start.into(), end.into()),
    };
    let docs = state.with_db(kg_base::FileId(root), |db| db.documentation_at(range)).flatten();
    if let Some(docs) = docs {
        let docs = kg_syntax::to_commonmark(&docs, kg_syntax::Headings::Bold);
        let value = match item.documentation.take() {
            Some(Documentation::MarkupContent(m)) => format!("{}\n\n---\n\n{docs}", m.value),
            Some(Documentation::String(s)) => format!("{s}\n\n---\n\n{docs}"),
            None => docs,
        };
        item.documentation = Some(Documentation::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value }));
    }
    Ok(item)
}

/// Whether a detail prints `ty` — hover's test (`super::hover`'s `shown`):
/// `tResolved` (`Type.hs:1001-1015`) less `TError`, `Untyped` and an
/// undeduced `Auto`, each of which would print a word that is no type.
fn shown(ty: &Ty) -> bool {
    !matches!(ty, Ty::Untyped | Ty::Auto) && ty.resolved() && !ty.is_error()
}

/// The client's icon for a declaration kind. Exhaustive so a new [`DefKind`]
/// is a compile error here rather than a mystery icon.
fn item_kind(kind: DefKind) -> CompletionItemKind {
    match kind {
        DefKind::Alias => CompletionItemKind::TYPE_PARAMETER,
        DefKind::Capture => CompletionItemKind::VARIABLE,
        DefKind::Struct | DefKind::Union => CompletionItemKind::STRUCT,
        DefKind::Class => CompletionItemKind::CLASS,
        DefKind::Enum => CompletionItemKind::ENUM,
        DefKind::EnumConstant => CompletionItemKind::ENUM_MEMBER,
        DefKind::Function | DefKind::FunctionDecl => CompletionItemKind::FUNCTION,
        DefKind::Variable(DeclareFlag::DeclareClass | DeclareFlag::DeclareMember) => {
            CompletionItemKind::FIELD
        }
        DefKind::Variable(_) => CompletionItemKind::VARIABLE,
        DefKind::FuncParam => CompletionItemKind::VARIABLE,
        DefKind::Template => CompletionItemKind::CLASS,
        DefKind::TemplateParam => CompletionItemKind::TYPE_PARAMETER,
        DefKind::ExportType | DefKind::Error => CompletionItemKind::TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_state::test_uri;
    use lsp_types::{
        PartialResultParams, Position, TextDocumentIdentifier, TextDocumentPositionParams, Uri,
        WorkDoneProgressParams,
    };

    fn params(uri: &Uri, position: Position) -> CompletionParams {
        CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        }
    }

    fn items(text: &str, pos: Position) -> Vec<CompletionItem> {
        let state = GlobalState::new();
        let uri = test_uri("complete.k");
        state.did_open(uri.clone(), text.to_owned(), 1);
        match completion(&state, &params(&uri, pos)).expect("the document is open") {
            Some(CompletionResponse::Array(items)) => items,
            other => panic!("expected an array, got {other:?}"),
        }
    }

    /// The member list after `s.`, with each field's declared type as its detail
    /// and the field icon — what a client shows in the popup. As an editor
    /// gets there: the buffer parsed and its diagnostics ran its loop, then
    /// the `.` was typed, which no longer parses — the list is the last
    /// loop's, the base typed by its name's declaration (`Db::base_answer`).
    /// Until sprint W3.7 the test opened the buffer with the `.` already in,
    /// which the CST engine answered with no loop.
    #[test]
    fn a_dot_lists_fields_with_their_types() {
        let src = "struct S\n{\n    uint32 x;\n    bool y;\n}\n\ninline void main()\n{\n    S s = {};\n    s.\n}\n";
        let state = GlobalState::new();
        let uri = test_uri("complete.k");
        state.did_open(uri.clone(), src.replace("    s.\n", "    s;\n"), 1);
        let file = state.snapshot(&uri).expect("open").file;
        state.with_db(file, |db| db.looped(&[file])).expect("open");
        state.did_change_full(&uri, src.to_owned(), 2);
        let Some(CompletionResponse::Array(out)) = completion(&state, &params(&uri, Position::new(9, 6))).expect("open")
        else {
            panic!("expected an array")
        };
        let shown: Vec<(String, Option<CompletionItemKind>, Option<String>)> =
            out.into_iter().map(|i| (i.label, i.kind, i.detail)).collect();
        assert_eq!(
            shown,
            [
                ("x".to_owned(), Some(CompletionItemKind::FIELD), Some("uint32".to_owned())),
                ("y".to_owned(), Some(CompletionItemKind::FIELD), Some("bool".to_owned())),
            ]
        );
    }

    /// A buffer that does not parse — `s.` mid-keystroke is one — runs no
    /// loop for its completion (`Db::editor_loop_ready`): completion is
    /// asked on every keystroke, and the one that broke the parse pays for
    /// no loop the diagnostics did not run. With no loop run before it —
    /// a buffer opened broken — no list: the CST engine's type of the base
    /// answered there until sprint W3.7, and what its deletion costs is the
    /// sprint plan's W3 entry. Red with the handler asking `Db::looped`
    /// directly (the loop runs, and lists `x` and `y`).
    #[test]
    fn a_buffer_that_does_not_parse_runs_no_loop() {
        let src = "struct S\n{\n    uint32 x;\n    bool y;\n}\n\ninline void main()\n{\n    S s = {};\n    s.\n}\n";
        let state = GlobalState::new();
        let uri = test_uri("complete.k");
        state.did_open(uri.clone(), src.to_owned(), 1);
        let file = state.snapshot(&uri).expect("open").file;
        let loops = || state.with_db(file, |db| db.stats().misses(kg_db::stats::Query::Looped)).expect("open");
        let before = loops();
        let Some(CompletionResponse::Array(out)) = completion(&state, &params(&uri, Position::new(9, 6))).expect("open")
        else {
            panic!("expected an array")
        };
        let shown: Vec<(String, Option<String>)> = out.into_iter().map(|i| (i.label, i.detail)).collect();
        assert_eq!(shown, []);
        assert_eq!(loops(), before, "a completion on a broken buffer ran the loop");
    }

    /// A clean buffer no loop has run for offers no member list — a
    /// keystroke's completion does not wait for the edit's loop — and, once
    /// the loop is in, the loop's: `a` is `Box<4>`. After an edit, before its diagnostics run
    /// the edit's loop, the one before it answers, read through the edit
    /// (`kg_db::looped::EditorLoop`), and still none runs. Red with the
    /// handler waiting for the loop (`Db::editor_loop`): the first
    /// completion runs it.
    #[test]
    fn a_completion_reads_the_loop_once_the_diagnostics_ran_it() {
        let src = "template <auto N>\nclass Box\n{\n    uint32[N] items;\n}\n\n\
                   inline void main()\n{\n    static Box<4> a;\n    auto r = a.items[0];\n}\n";
        let state = GlobalState::new();
        let uri = test_uri("complete.k");
        state.did_open(uri.clone(), src.to_owned(), 1);
        let file = state.snapshot(&uri).expect("open").file;
        let loops = || state.with_db(file, |db| db.stats().misses(kg_db::stats::Query::Looped)).expect("open");
        let offered = || match completion(&state, &params(&uri, Position::new(9, 15))).expect("open") {
            Some(CompletionResponse::Array(out)) => out.into_iter().map(|i| (i.label, i.detail)).collect::<Vec<_>>(),
            other => panic!("expected an array, got {other:?}"),
        };
        let before = loops();
        assert_eq!(offered(), [], "no loop has typed `a`");
        assert_eq!(loops(), before, "a completion ran the loop");
        state.with_db(file, |db| db.looped(&[file])).expect("open");
        assert_eq!(offered(), [("items".to_owned(), Some("uint32[4]".to_owned()))]);
        let ran = loops();
        state.did_change_full(&uri, format!("{src} "), 2);
        assert_eq!(offered(), [("items".to_owned(), Some("uint32[4]".to_owned()))], "the last loop, through the edit");
        assert_eq!(loops(), ran, "a completion ran the edit's loop");
    }

    /// Sort keys keep a local ahead of a global whatever the alphabet says.
    #[test]
    fn a_local_sorts_before_a_global() {
        let src = "uint32 aaa;\n\ninline void main()\n{\n    uint32 zzz;\n    \n}\n";
        let out = items(src, Position::new(5, 4));
        let mut by_key: Vec<(String, String)> =
            out.into_iter().map(|i| (i.sort_text.unwrap_or_default(), i.label)).collect();
        by_key.sort();
        let labels: Vec<&str> = by_key.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, ["zzz", "aaa", "main"]);
    }

    /// In edition 1.1 an argument begun is offered the parameters left to
    /// name, as `x:`: not one a positional or another named argument fills,
    /// not a pipelined function's thread index, none in the thread count's
    /// place, none on a label already written. A file of edition 1.0 is
    /// offered none — a label there is a parse error. Red with the filled
    /// parameters offered, the index offered, or the language not asked.
    #[test]
    fn an_argument_begun_is_offered_the_parameters_left_to_name() {
        use crate::handlers::references::tests::{lay_out_1_1, open_one};
        let text = "class Main\n{\n    [[pipelined]] void Work(uint32 tid, bool flag, uint8 v)\n    {\n    }\n\n    uint8 sub(uint8 x, uint8 y, uint8 z)\n    {\n        return x;\n    }\n\npublic:\n    uint8 main(bool f)\n    {\n        uint8 a = sub(1, z: 2, );\n        uint8 b = sub(y);\n        Work(4, );\n        Work();\n        Work(flag: f, );\n        return sub(x: 1, y: 2, z: 3);\n    }\n}\n";
        let files = [("main.k", text)];
        let (dir, state, src) = lay_out_1_1("complete-labels", &files);
        let uri = open_one(&state, &src, &files, "main.k");
        let labels = |line: u32, col: u32| -> Vec<(String, Option<String>, Option<String>)> {
            match completion(&state, &params(&uri, Position::new(line, col))).expect("open") {
                Some(CompletionResponse::Array(out)) => out
                    .into_iter()
                    .filter(|i| i.label.ends_with(':'))
                    .map(|i| (i.label, i.insert_text, i.sort_text))
                    .collect(),
                other => panic!("expected an array, got {other:?}"),
            }
        };
        let named = |names: &[&str]| -> Vec<String> { names.iter().map(|n| format!("{n}:")).collect() };
        let shown = |line, col| labels(line, col).into_iter().map(|(l, ..)| l).collect::<Vec<_>>();

        assert_eq!(labels(14, 31), [("y:".to_owned(), Some("y: ".to_owned()), Some("00000".to_owned()))]);
        assert_eq!(shown(15, 23), named(&["x", "y", "z"]), "one word into the argument");
        assert_eq!(shown(16, 16), named(&["flag", "v"]), "never `tid`");
        assert_eq!(shown(17, 13), named(&[]), "the thread count's place");
        assert_eq!(shown(18, 22), named(&["v"]), "never `tid`, the count unwritten");
        assert_eq!(shown(19, 20), named(&[]), "on a label already written");
        std::fs::remove_dir_all(&dir).unwrap();

        let plain = "inline uint8 sub(uint8 x, uint8 y)\n{\n    return x;\n}\n\ninline uint8 main()\n{\n    return sub(1, );\n}\n";
        let reference: Vec<String> =
            items(plain, Position::new(7, 18)).into_iter().map(|i| i.label).filter(|l| l.ends_with(':')).collect();
        assert_eq!(reference, named(&[]), "edition 1.0 has no named arguments");
    }

    /// An unopened document is `NoFile`, never an empty list pretending to
    /// have looked.
    #[test]
    fn an_unopened_document_is_no_file() {
        let state = GlobalState::new();
        let uri = test_uri("nowhere.k");
        assert!(completion(&state, &params(&uri, Position::new(0, 0))).is_err());
    }
}
