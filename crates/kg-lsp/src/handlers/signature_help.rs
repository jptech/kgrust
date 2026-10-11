//! `textDocument/signatureHelp` — the parameters of the function the caret's
//! argument list calls, with the one the caret is on marked.
//!
//! # Where the answer comes from
//!
//! The *declaration*, not the type. The caret sits inside some `ARG_LIST`; its
//! `CALL_EXPR`'s callee is resolved with [`kg_db::Db::definition_at`] on the
//! callee's last name token — the same resolution goto-definition performs at
//! that token, so the signature shown is the declaration a jump would land on
//! — and the label is read off that declaration's `PARAM_LIST` as written:
//! `uint32 x`, `[[last]] bool done`, `(uint32) -> uint32 fn`. Reading the
//! parameters from the written declaration rather than from its type
//! ([`kg_db::Db::ir_answer_of_definition`]) is what lets a **template** function
//! answer at all: `Fn_T(x)`'s declared type is not resolved before pass 8, but
//! its parameter list `T x` is text, and text is what a signature box shows.
//!
//! The active parameter is the number of commas the caret is past, clamped to
//! the last parameter, so a caret in an over-long call keeps the last one lit
//! rather than none.
//!
//! # The instance a call compiles
//!
//! A template's written parameters say `T x`; the call says which `T`. Where
//! the program-wide loop resolved the call to a template's instance
//! (`kg_db::looped::Looped::callee_at`, over [`kg_db::Db::editor_loop`]),
//! the signature's documentation is that instance's type, spelled as hover
//! spells a function and named as a message names the instance —
//!
//! ```text
//! (uint16) -> void take  // take<uint16>
//! ```
//!
//! — and inside a template's body, where each instance of the *enclosing*
//! template may call a different one, one line per distinct callee with the
//! enclosing instances that call it (`super::instances`). A call to a plain
//! function adds nothing: its written signature is already the whole answer.
//! A buffer the diagnostics stop on reads the last loop, mapped
//! (`kg_db::Db::editor_loop`): a call typed since it ran is in no loop, and
//! has no documentation.
//!
//! # What declines
//!
//! Everything [`kg_db::Db::definition_at`] declines at the callee — an
//! intrinsic (`clog2`, `cast<…>`), a name it cannot resolve, a call through a
//! function-typed *value* (`fn(x)` where `fn` is a parameter or a local: the
//! declaration a jump reaches is the parameter, which has no parameter list of
//! its own) — and a callee whose declaration is not a function or a function
//! prototype. A declined answer is `null`, which a client shows as nothing.

use kg_base::{FileId, TextSize};
use kg_db::ModuleSource;
use kg_db::looped::{IrAnswer, Looped};
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, ast};
use lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureHelpParams, SignatureInformation,
};

use crate::convert;
use crate::global_state::GlobalState;
use crate::handlers::{Answer, instances};

pub fn signature_help(
    state: &GlobalState,
    params: &SignatureHelpParams,
) -> Answer<Option<SignatureHelp>> {
    let at = &params.text_document_position_params;
    let snap = state.snapshot(&at.text_document.uri)?;
    let offset = convert::offset(snap.line_index(), state.encoding(), at.position);

    let Some((call, active)) = enclosing_call(&snap.parse.syntax_node(), offset) else {
        return Ok(None);
    };
    let Some(probe) = callee_probe(&call) else { return Ok(None) };

    let help = state
        .with_db(snap.file, |db| {
            let roots = [snap.file];
            let found = db.definition_at(&roots, snap.file, probe)?;
            if found.range.file == FileId::SYNTHETIC {
                return None;
            }
            let root = db.parse(found.range.file).syntax_node();
            let decl = declaration_at(&root, found.range.range)?;
            let signature = signature_of(&decl)?;
            let parameters = signature.params.len() as u32;
            let names = db.interner();
            let documentation = db.editor_loop(&roots, snap.file).and_then(|looped| {
                let answer = looped.callee_at(ModuleSource::File(snap.file), call.syntax().text_range(), names)?;
                Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: instance_documentation(looped.looped(), &answer, &signature.name, names),
                }))
            });
            Some(SignatureHelp {
                active_parameter: (parameters > 0).then(|| active.min(parameters - 1)),
                signatures: vec![SignatureInformation {
                    label: signature.label,
                    documentation,
                    parameters: Some(
                        signature
                            .params
                            .into_iter()
                            .map(|(start, end)| ParameterInformation {
                                label: ParameterLabel::LabelOffsets([start, end]),
                                documentation: None,
                            })
                            .collect(),
                    ),
                    active_parameter: None,
                }],
                active_signature: Some(0),
            })
        })
        .flatten();
    Ok(help)
}

/// The callee's type at this call, from the loop — see the module docs.
fn instance_documentation(looped: &Looped, answer: &IrAnswer, name: &str, names: &kg_base::Interner) -> String {
    let (lines, said) = instances::called(looped, answer, name, names);
    format!("{}\n\n{said}", instances::fence(&lines))
}

/// The innermost call whose argument list the caret is inside — strictly
/// after its `(`, and not past its `)` — with the argument index the caret is
/// on: one per comma the caret is past.
fn enclosing_call(root: &SyntaxNode, offset: TextSize) -> Option<(ast::CallExpr, u32)> {
    if offset > root.text_range().end() {
        return None;
    }
    // `token_at_offset` yields one or two tokens; either's ancestors reach the
    // same argument lists, so the leftmost is enough.
    let token = root.token_at_offset(offset).left_biased()?;
    for node in token.parent_ancestors() {
        if node.kind() != SyntaxKind::ARG_LIST {
            continue;
        }
        let list = ast::ArgList::cast(node.clone())?;
        let open = list.l_paren_token()?.text_range().end();
        let close = node
            .last_token()
            .filter(|t| t.kind() == SyntaxKind::R_PAREN)
            .map(|t| t.text_range().start());
        if offset < open || close.is_some_and(|c| offset > c) {
            continue;
        }
        let call = node.parent().and_then(ast::CallExpr::cast)?;
        let active = list.comma_tokens().filter(|c| c.text_range().end() <= offset).count() as u32;
        return Some((call, active));
    }
    None
}

/// An offset inside the callee's last name token — the token
/// [`kg_db::Db::definition_at`] resolves for `f(…)`, `a::f(…)` and `o.f(…)`
/// alike. Its midpoint, for the reason `handlers::references::probe` gives.
fn callee_probe(call: &ast::CallExpr) -> Option<TextSize> {
    let callee = call.callee()?;
    let name = callee
        .syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .last()?;
    Some(name.text_range().start() + TextSize::new(name.text().len() as u32 / 2))
}

/// The function declaration or prototype whose name token is `name` — the
/// `FN_DECL`/`FN_PROTO` two levels above the token (`NAME` between).
fn declaration_at(root: &SyntaxNode, name: kg_base::TextRange) -> Option<SyntaxNode> {
    root.token_at_offset(name.start())
        .find(|t| t.text_range() == name)
        .and_then(|t| t.parent())
        .and_then(|n| n.parent())
        .filter(|n| matches!(n.kind(), SyntaxKind::FN_DECL | SyntaxKind::FN_PROTO))
}

struct Signature {
    /// The declared name, as the label begins with it.
    name: String,
    label: String,
    /// `[start, end)` of each parameter inside `label`, in UTF-16 units as the
    /// protocol's `ParameterLabel::LabelOffsets` counts them.
    params: Vec<(u32, u32)>,
}

/// `name(p1, p2) -> ret`, each parameter spelled as written with its
/// whitespace collapsed, the return type likewise, and the parameter spans
/// recorded as the label is built.
fn signature_of(decl: &SyntaxNode) -> Option<Signature> {
    let (name, params, ret) = match decl.kind() {
        SyntaxKind::FN_DECL => {
            let f = ast::FnDecl::cast(decl.clone())?;
            (f.name()?, f.param_list()?, f.ret())
        }
        SyntaxKind::FN_PROTO => {
            let f = ast::FnProto::cast(decl.clone())?;
            (f.name()?, f.param_list()?, f.ret())
        }
        _ => return None,
    };
    let name = name.ident_token()?.text().to_owned();
    let mut label = name.clone();
    label.push('(');
    let mut spans = Vec::new();
    for (i, param) in params.params().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = utf16_len(&label);
        label.push_str(&collapse_whitespace(param.syntax()));
        spans.push((start, utf16_len(&label)));
    }
    label.push(')');
    if let Some(ret) = ret {
        label.push_str(" -> ");
        label.push_str(&collapse_whitespace(ret.syntax()));
    }
    Some(Signature { name, label, params: spans })
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// The node's text with every run of whitespace — and every comment — reduced
/// to one space, so a parameter declared across lines reads on one.
fn collapse_whitespace(node: &SyntaxNode) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for token in node.descendants_with_tokens().filter_map(|e| e.into_token()) {
        if token.kind().is_trivia() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push_str(token.text());
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_state::test_uri;
    use lsp_types::{
        Position, TextDocumentIdentifier, TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    };

    fn params(uri: &Uri, position: Position) -> SignatureHelpParams {
        SignatureHelpParams {
            context: None,
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        }
    }

    /// `(label, parameter spans, active parameter)`.
    type Shown = (String, Vec<(u32, u32)>, Option<u32>);

    fn help(text: &str, pos: Position) -> Option<Shown> {
        let state = GlobalState::new();
        let uri = test_uri("sig.k");
        state.did_open(uri.clone(), text.to_owned(), 1);
        let help = signature_help(&state, &params(&uri, pos)).expect("the document is open")?;
        let sig = help.signatures.into_iter().next().expect("one signature");
        let spans = sig
            .parameters
            .unwrap_or_default()
            .into_iter()
            .map(|p| match p.label {
                ParameterLabel::LabelOffsets([a, b]) => (a, b),
                ParameterLabel::Simple(_) => panic!("offsets are the contract"),
            })
            .collect();
        Some((sig.label, spans, help.active_parameter))
    }

    const SRC: &str = "\
inline uint32 add(uint32 x,
                  [[last]] bool done)
{
    return x;
}

inline void main()
{
    uint32 y = add(1, true);
    uint32 z = add(add(2, false), true);
}
";

    /// The label is the declaration as written, on one line, with each
    /// parameter's span; the active parameter follows the commas.
    #[test]
    fn the_label_is_the_declaration_and_the_active_parameter_follows_the_commas() {
        let label = "add(uint32 x, [[last]] bool done) -> uint32".to_owned();
        let spans = vec![(4, 12), (14, 32)];
        assert_eq!(help(SRC, Position::new(8, 19)), Some((label.clone(), spans.clone(), Some(0))));
        assert_eq!(help(SRC, Position::new(8, 22)), Some((label.clone(), spans.clone(), Some(1))));
        // Right after `(`, before any argument is typed.
        assert_eq!(help(SRC, Position::new(8, 19)).map(|h| h.2), Some(Some(0)));
    }

    /// Nested calls: the innermost argument list the caret is in wins, and
    /// leaving it returns to the outer one.
    #[test]
    fn the_innermost_argument_list_wins() {
        assert_eq!(help(SRC, Position::new(9, 24)).map(|h| h.2), Some(Some(0)), "inside add(2, …)");
        assert_eq!(help(SRC, Position::new(9, 27)).map(|h| h.2), Some(Some(1)), "inside add(2, false)");
        assert_eq!(help(SRC, Position::new(9, 35)).map(|h| h.2), Some(Some(1)), "back in the outer call");
    }

    /// Off any argument list — on the callee's name, on a declaration — the
    /// answer is nothing, not the nearest call's signature.
    #[test]
    fn outside_an_argument_list_is_nothing() {
        assert_eq!(help(SRC, Position::new(8, 16)), None, "on the callee");
        assert_eq!(help(SRC, Position::new(0, 20)), None, "on the declaration");
        assert_eq!(help(SRC, Position::new(3, 4)), None);
    }

    /// A template function has no resolved type before pass 8, and still has a
    /// written parameter list — which is what the box shows.
    #[test]
    fn a_template_function_answers_from_its_written_parameters() {
        let src = "\
template <typename T>
inline T twice(T x, T y)
{
    return x;
}

inline void main()
{
    uint32 y = twice(1, 2);
}
";
        assert_eq!(
            help(src, Position::new(8, 24)),
            Some(("twice(T x, T y) -> T".to_owned(), vec![(6, 9), (11, 14)], Some(1)))
        );
    }

    /// A call through a function-typed value resolves to the *value*'s
    /// declaration, which has no parameter list; an unresolvable callee has no
    /// declaration at all. Both are `null`.
    #[test]
    fn a_callee_that_is_not_a_declared_function_declines() {
        let src = "\
inline void run((uint32) -> uint32 fn)
{
    uint32 y = fn(1);
    uint32 z = nowhere(1);
}
";
        assert_eq!(help(src, Position::new(2, 18)), None);
        assert_eq!(help(src, Position::new(3, 23)), None);
    }

    /// Past the last parameter the last one stays active; a function with no
    /// parameters has no active parameter to report.
    #[test]
    fn the_active_parameter_is_clamped() {
        assert_eq!(help(SRC, Position::new(8, 19)).map(|h| h.2), Some(Some(0)));
        let src = "inline void f(uint32 x)\n{\n}\n\ninline void main()\n{\n    f(1, 2, 3);\n}\n";
        assert_eq!(help(src, Position::new(6, 12)).map(|h| h.2), Some(Some(0)));
        let none = "inline void f()\n{\n}\n\ninline void main()\n{\n    f();\n}\n";
        assert_eq!(help(none, Position::new(6, 6)), Some(("f() -> void".to_owned(), vec![], None)));
    }

    /// The signature's documentation, as markdown.
    fn documentation(text: &str, pos: Position) -> Option<String> {
        let state = GlobalState::new();
        let uri = test_uri("sig.k");
        state.did_open(uri.clone(), text.to_owned(), 1);
        let help = signature_help(&state, &params(&uri, pos)).expect("the document is open").expect("a signature");
        match help.signatures.into_iter().next().expect("one signature").documentation? {
            Documentation::MarkupContent(m) => Some(m.value),
            Documentation::String(s) => panic!("expected markdown, got {s:?}"),
        }
    }

    const TAKE: &str = "template <typename T>\ninline void take(T v)\n{\n}\n\n\
                        template <typename U>\ninline void outer(U u)\n{\n    take(u + 1);\n}\n\n\
                        inline void main()\n{\n    take<uint8>(1);\n    take(2u16);\n    outer(3u4);\n    outer(4u6);\n}\n";

    /// The written parameters say `T v`; the call says which `T` — the
    /// instance the loop resolved it to, written or deduced (`take(2u16)`
    /// compiles `take<uint16>`: `kg-oracle --no-base --passes 19`,
    /// 2026-10-10, prints `take<|$uint@16|>(2)`). Red with the
    /// documentation dropped, or read from the loop where the caret is
    /// rather than at the call.
    #[test]
    fn a_call_to_a_template_shows_the_instance_it_compiles() {
        let written = documentation(TAKE, Position::new(13, 16)).expect("an instance");
        assert_eq!(written, "```kanagawa\n(uint8) -> void take  // take<uint8>\n```\n\nThe instance this call compiles.");
        let deduced = documentation(TAKE, Position::new(14, 9)).expect("an instance");
        assert!(deduced.starts_with("```kanagawa\n(uint16) -> void take  // take<uint16>\n```"), "{deduced}");
        // The label is still the declaration as written.
        assert_eq!(help(TAKE, Position::new(14, 9)).map(|h| h.0), Some("take(T v) -> void".to_owned()));
    }

    /// Inside a template's body each instance of the enclosing template
    /// may call a different instance: `take(u + 1)` in `outer<uint4>`
    /// calls `take<uint5>`, in `outer<uint6>` `take<uint7>` (`kg-oracle
    /// --no-base --passes 19`, 2026-10-10: `take<|$uint@5|>((u + 1))` in
    /// `outer<|$uint@4|>`, `take<|$uint@7|>` in `outer<|$uint@6|>`). Red
    /// with the copies' half of `callee_at` dropped.
    #[test]
    fn a_call_inside_a_template_body_shows_each_instance_s_callee() {
        let out = documentation(TAKE, Position::new(8, 9)).expect("the copies' callees");
        assert_eq!(
            out,
            "```kanagawa\n(uint5) -> void take  // take<uint5> in outer<uint4>\n(uint7) -> void take  // take<uint7> in outer<uint6>\n```\n\n\
             Typed in 2 instances: a template's body has no type of its own."
        );
    }

    /// A plain function's written signature is the whole answer: nothing is
    /// added. Red with a written callee that is not an instance shown.
    #[test]
    fn a_call_to_a_plain_function_adds_nothing() {
        assert_eq!(documentation(SRC, Position::new(8, 19)), None);
    }

    #[test]
    fn an_unopened_document_is_no_file() {
        let state = GlobalState::new();
        let uri = test_uri("never.k");
        assert!(signature_help(&state, &params(&uri, Position::new(0, 0))).is_err());
    }
}
