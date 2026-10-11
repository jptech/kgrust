//! `textDocument/codeAction` — the quick fixes the diagnostics carry.
//!
//! A diagnostic may carry a machine-applicable fix ([`kg_diag::Fix`]): today
//! the cast `TypeCheck.hs:103`'s hint suggests on a narrowing conversion,
//! which `kg check` prints as `help:`. Until this handler the editor showed
//! the warning and had no way to apply the fix — a fix with no caller. Each
//! fix of a diagnostic whose range meets the requested one is offered as a
//! `quickfix`, its `diagnostics` the diagnostic exactly as it was published
//! (`super::diagnostics::diagnostics`), so a client pairs the two.
//!
//! The diagnostics are the file's own, at the newest loop ready
//! (`Db::diagnostics_first`): what the gutter shows — the first answer
//! while the fixpoint runs on, the last once it is there — since a client
//! asks for code actions on every cursor move, and one that waited for the
//! fixpoint would hold its worker as long. Not
//! `GlobalState::frontend_diagnostics`, which publishes. A fix with an edit
//! in a file the server cannot name is left out, never offered
//! half-applied.

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse, TextEdit,
    Uri, WorkspaceEdit,
};

use crate::convert;
use crate::global_state::GlobalState;
use crate::handlers::{Answer, diagnostics, uri_of};

pub fn code_action(state: &GlobalState, params: &CodeActionParams) -> Answer<Option<CodeActionResponse>> {
    let uri = &params.text_document.uri;
    let snap = state.snapshot(uri)?;
    // A client asking only for kinds this server never produces — a
    // `source.organizeImports` on save — gets nothing, without a check.
    if let Some(only) = &params.context.only
        && !only.iter().any(|k| CodeActionKind::QUICKFIX.as_str().starts_with(k.as_str()))
    {
        return Ok(None);
    }
    let enc = state.encoding();
    let index = snap.line_index();
    let wanted = kg_base::TextRange::new(
        convert::offset(index, enc, params.range.start),
        convert::offset(index, enc, params.range.end),
    );
    let Some(reported) = state.with_db(snap.file, |db| db.diagnostics_first(&[snap.file], snap.file)) else {
        return Ok(None);
    };
    let root = snap.parse.syntax_node();
    let locate = |range: kg_base::FileRange| {
        let uri = uri_of(state, range.file)?;
        let text = state.vfs().read(range.file)?;
        Some(lsp_types::Location { uri, range: convert::range(&text.line_index, enc, range.range) })
    };
    let mut actions = Vec::new();
    for d in reported {
        // Touching counts: a caret just past the expression it fixes is a
        // request for that fix.
        if d.range.file != snap.file
            || d.fixes.is_empty()
            || d.range.range.start() > wanted.end()
            || wanted.start() > d.range.range.end()
        {
            continue;
        }
        let fixes = d.fixes.clone();
        let published = diagnostics::diagnostics(vec![d], &root, snap.file, index, enc, &locate);
        for fix in &fixes {
            let Some(changes) = edits(state, fix) else { continue };
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: fix.message.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(published.clone()),
                edit: Some(WorkspaceEdit { changes: Some(changes), ..WorkspaceEdit::default() }),
                is_preferred: Some(fixes.len() == 1),
                ..CodeAction::default()
            }));
        }
    }
    Ok((!actions.is_empty()).then_some(actions))
}

/// A fix's edits, by the document each applies to — `None` when one of them
/// is in a file the server cannot name or read.
///
/// `WorkspaceEdit::changes` is `HashMap<Uri, Vec<TextEdit>>` by the
/// protocol's own definition; the key's interior mutability is
/// `fluent_uri`'s, and nothing here mutates a key.
#[allow(clippy::mutable_key_type)]
fn edits(state: &GlobalState, fix: &kg_diag::Fix) -> Option<HashMap<Uri, Vec<TextEdit>>> {
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for edit in &fix.edits {
        let uri = uri_of(state, edit.file)?;
        let text = state.vfs().read(edit.file)?;
        changes.entry(uri).or_default().push(TextEdit {
            range: convert::range(&text.line_index, state.encoding(), edit.range),
            new_text: edit.new_text.clone(),
        });
    }
    Some(changes)
}

#[cfg(test)]
mod tests {
    use lsp_types::{
        CodeActionContext, PartialResultParams, Position, Range, TextDocumentIdentifier, WorkDoneProgressParams,
    };

    use super::*;
    use crate::global_state::test_uri;

    /// A designated initializer takes the narrowing *error*, with
    /// `TypeCheck.hs:103`'s cast as two edits (`TypeCheck.hs:130`,
    /// `designated-initializer.k::1`) — no `--Wall` needed, where the
    /// narrowing warning is off by default.
    const NARROWING: &str = "struct S\n{\n    uint8 a;\n}\n\ninline void main()\n{\n    uint32 x = 3;\n    S s = {.a = x};\n}\n";

    fn params(uri: &Uri, at: Position, only: Option<Vec<CodeActionKind>>) -> CodeActionParams {
        CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range { start: at, end: at },
            context: CodeActionContext { diagnostics: Vec::new(), only, trigger_kind: None },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        }
    }

    fn opened() -> (GlobalState, Uri) {
        let state = GlobalState::new();
        let uri = test_uri("narrow.k");
        state.did_open(uri.clone(), NARROWING.to_owned(), 1);
        (state, uri)
    }

    /// The caret on the value is offered the cast, its edits bracketing
    /// `x` — applied, `{.a = cast<uint8>(x)}` — and paired with the error
    /// as published. Red with the handler unregistered from the fixes (no
    /// action) or the edits' positions converted from the wrong offsets.
    #[test]
    fn the_caret_on_a_narrowed_value_is_offered_the_cast() {
        let (state, uri) = opened();
        let actions = code_action(&state, &params(&uri, Position::new(8, 16), None))
            .expect("the document is open")
            .expect("the cast must be offered");
        assert_eq!(actions.len(), 1, "{actions:?}");
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else { panic!("a command: {actions:?}") };
        assert_eq!(action.title, "Insert an explicit cast to silence this issue `cast<uint8>()`");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        let paired = action.diagnostics.as_ref().expect("paired with its error");
        assert_eq!(paired.len(), 1);
        assert!(paired[0].message.starts_with("Invalid conversion from `uint32` to `uint8`"), "{paired:?}");
        #[allow(clippy::mutable_key_type)]
        let changes = action.edit.as_ref().and_then(|e| e.changes.as_ref()).expect("a workspace edit");
        let edits = &changes[&uri];
        assert_eq!(
            edits.iter().map(|e| (e.range.start, e.range.end, e.new_text.as_str())).collect::<Vec<_>>(),
            [
                (Position::new(8, 16), Position::new(8, 16), "cast<uint8>("),
                (Position::new(8, 17), Position::new(8, 17), ")"),
            ]
        );
    }

    /// A caret away from every diagnostic, or a request for kinds this
    /// server never produces, is answered with nothing; a request for
    /// quick fixes is answered.
    #[test]
    fn nothing_is_offered_away_from_the_warning_or_for_other_kinds() {
        let (state, uri) = opened();
        assert_eq!(code_action(&state, &params(&uri, Position::new(0, 3), None)).expect("open"), None);
        let organize = Some(vec![CodeActionKind::SOURCE_ORGANIZE_IMPORTS]);
        assert_eq!(code_action(&state, &params(&uri, Position::new(8, 16), organize)).expect("open"), None);
        let quickfix = Some(vec![CodeActionKind::QUICKFIX]);
        assert!(code_action(&state, &params(&uri, Position::new(8, 16), quickfix)).expect("open").is_some());
    }
}
