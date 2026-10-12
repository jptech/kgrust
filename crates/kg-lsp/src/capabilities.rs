//! What the server tells the client it can do.
//!
//! The rule here is one line long and worth stating: **advertise exactly what
//! is implemented.** A capability answered with a stub is worse than an absent
//! one — a greyed-out "Go to Definition" is a fact, while one that silently
//! does nothing is a bug report. The rule is about what is *answered*, not what
//! is *possible*.
//!
//! `definitionProvider`, `hoverProvider`, `referencesProvider` and
//! `inlayHintProvider` are here because [`crate::handlers`] now answers those
//! four requests off [`kg_db::Db::definition_at`] and [`kg_db::Db::type_table`].
//! They were absent for as long as the rule required — the entry appeared with
//! the handler, not with the query it is built on. `completionProvider`
//! followed the same rule on 2026-09-06, with [`crate::handlers::completion`]
//! over [`kg_db::Db::completions_at`], and `renameProvider` on 2026-09-07 with
//! [`crate::handlers::rename`] over the references search, and
//! `signatureHelpProvider` the same day with [`crate::handlers::signature_help`].
//!
//! `codeActionProvider` followed on 2026-10-10 with
//! [`crate::handlers::code_action`], offering the fixes the diagnostics carry,
//! which until then only `kg check` printed, and `workspaceSymbolProvider`
//! the same day with [`crate::handlers::workspace_symbol`], and
//! `typeDefinitionProvider` with [`crate::handlers::definition::type_definition`].
//!
//! `documentFormattingProvider` is the one conditional entry, gated on
//! [`crate::fmt::available`]. It stayed dark until `kg-fmt` was linked in and
//! lit up on its own when it was.

use lsp_types::{
    ClientCapabilities, CodeActionKind, TypeDefinitionProviderCapability, CodeActionOptions, CodeActionProviderCapability, CompletionOptions, FoldingRangeProviderCapability, HoverProviderCapability, OneOf,
    RenameOptions, SemanticTokensFullOptions, SemanticTokensOptions, SignatureHelpOptions,
    SemanticTokensServerCapabilities, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions,
    WorkDoneProgressOptions,
};

use crate::convert::PositionEncoding;
use crate::fmt;
use crate::handlers::semantic_tokens;

pub fn server_capabilities(enc: PositionEncoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(enc.kind()),

        // Incremental sync, with `didSave` carrying no text: the server already
        // has the buffer, and `[lsp] diagnostics = "on-save"` only needs the
        // notification, not the contents.
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::INCREMENTAL),
            will_save: None,
            will_save_wait_until: None,
            save: Some(TextDocumentSyncSaveOptions::Supported(true)),
        })),

        document_symbol_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(true.into()),
        document_highlight_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        // `.` re-asks for the member list; every other keystroke filters the
        // complete list the last answer gave.
        completion_provider: Some(CompletionOptions {
            // `:` for a path's `::` (`mode::`); a single `:` asks too, and
            // gets no list (`completion`).
            trigger_characters: Some(vec![".".to_owned(), ":".to_owned()]),
            // The documentation, read when an item is focused.
            resolve_provider: Some(true),
            ..CompletionOptions::default()
        }),
        // `prepareProvider`: the client asks first, and is told `null` off a
        // name and given the refusal's sentence on one that cannot be renamed,
        // before the user types anything. See [`crate::handlers::rename`].
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        // `(` opens an argument list and `,` moves along it; both re-ask, so the
        // active parameter follows the caret.
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
            retrigger_characters: Some(vec![",".to_owned()]),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        // `OneOf::Left(true)` rather than `InlayHintServerCapabilities::Options
        // { resolve_provider }`: `inlayHint/resolve` is not implemented, and
        // this file's rule is to advertise exactly what is answered.
        inlay_hint_provider: Some(OneOf::Left(true)),
        // Quick fixes only: the kinds are listed so a client asking for
        // `source.*` on save is not sent a request this server answers empty.
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
            work_done_progress_options: WorkDoneProgressOptions::default(),
            resolve_provider: None,
        })),
        workspace_symbol_provider: Some(OneOf::Left(true)),

        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                work_done_progress_options: WorkDoneProgressOptions::default(),
                legend: semantic_tokens::legend(),
                // No delta support: the server does not keep previous results,
                // and claiming deltas it cannot compute would make the client
                // discard perfectly good full results.
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: Some(true),
            },
        )),

        document_formatting_provider: fmt::available().then_some(OneOf::Left(true)),
        document_range_formatting_provider: fmt::available().then_some(OneOf::Left(true)),

        ..ServerCapabilities::default()
    }
}

/// The position encodings the client offered, if any.
pub fn offered_encodings(caps: &ClientCapabilities) -> PositionEncoding {
    let offered = caps.general.as_ref().and_then(|g| g.position_encodings.as_deref());
    PositionEncoding::negotiate(offered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{GeneralClientCapabilities, PositionEncodingKind};

    #[test]
    fn only_implemented_capabilities_are_advertised() {
        let caps = server_capabilities(PositionEncoding::Utf16);
        assert!(caps.document_symbol_provider.is_some());
        assert!(caps.folding_range_provider.is_some());
        assert!(caps.selection_range_provider.is_some());
        assert!(caps.document_highlight_provider.is_some());
        assert!(caps.semantic_tokens_provider.is_some());
        assert!(caps.text_document_sync.is_some());
        // Answered off `Db::definition_at` and `Db::type_table`. These four were
        // `is_none()` here for as long as no handler answered them; the day one
        // did, this expectation was the thing that had outgrown the code.
        assert!(caps.definition_provider.is_some());
        assert!(caps.hover_provider.is_some());
        assert!(caps.references_provider.is_some());
        assert!(caps.inlay_hint_provider.is_some());

        assert!(caps.completion_provider.is_some(), "completion is answered since 2026-09-06");
        // Answered since 2026-09-07, with `prepareRename`: a rename rewrites
        // every buffer references reports, and refuses — with a sentence —
        // whenever the search cannot promise to have seen every use.
        assert!(matches!(
            caps.rename_provider,
            Some(OneOf::Right(RenameOptions { prepare_provider: Some(true), .. }))
        ));
        assert!(caps.signature_help_provider.is_some(), "signature help is answered since 2026-09-07");
        // The diagnostics' fixes, as quick fixes, since 2026-10-10.
        assert!(caps.code_action_provider.is_some());
        // Every open document's program, by name, since 2026-10-10.
        assert!(caps.workspace_symbol_provider.is_some());
        assert!(caps.type_definition_provider.is_some(), "go to type definition, since 2026-10-10");
    }

    #[test]
    fn formatting_is_advertised_exactly_when_a_backend_exists() {
        let caps = server_capabilities(PositionEncoding::Utf16);
        assert_eq!(caps.document_formatting_provider.is_some(), fmt::available());
        assert_eq!(caps.document_range_formatting_provider.is_some(), fmt::available());
    }

    #[test]
    fn the_semantic_token_legend_is_published() {
        let caps = server_capabilities(PositionEncoding::Utf16);
        let Some(SemanticTokensServerCapabilities::SemanticTokensOptions(opts)) =
            caps.semantic_tokens_provider
        else {
            panic!("semantic tokens must be advertised with a legend");
        };
        assert!(!opts.legend.token_types.is_empty());
        assert_eq!(opts.range, Some(true));
        assert_eq!(opts.full, Some(SemanticTokensFullOptions::Bool(true)));
    }

    #[test]
    fn the_encoding_is_negotiated_from_the_client() {
        let caps = ClientCapabilities {
            general: Some(GeneralClientCapabilities {
                position_encodings: Some(vec![
                    PositionEncodingKind::UTF16,
                    PositionEncodingKind::UTF8,
                ]),
                ..GeneralClientCapabilities::default()
            }),
            ..ClientCapabilities::default()
        };
        assert_eq!(offered_encodings(&caps), PositionEncoding::Utf8);
        assert_eq!(
            offered_encodings(&ClientCapabilities::default()),
            PositionEncoding::Utf16
        );
        assert_eq!(
            server_capabilities(PositionEncoding::Utf8).position_encoding,
            Some(PositionEncodingKind::UTF8)
        );
    }
}
