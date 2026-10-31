//! `textDocument/inlayHint` — the width the compiler inferred, shown where the
//! source says `auto`; and, at a call short of arguments, the defaults it is
//! given (edition 1.1, `docs/editions/1.1/default-arguments.md`).
//!
//! # Why this feature and not another
//!
//! Kanagawa infers integer *widths*, and a width is not visible in the text.
//! `auto a = x + y` is a different number of wires depending on what `x` and `y`
//! are (`tAdd` widens, `tSub` widens *and* signs — `Type.hs:861-862`), and
//!
//! ```kanagawa
//! for (const auto i : 256)
//! ```
//!
//! gives `i` the type `const uint8`, not `uint9` and not `uint32`: the induction
//! variable is `constIndexType`, which is `clog2` of the iteration count
//! (`Type.hs:1113-1118`, `inductionVarType` `Type.hs:1131-1137`) because the
//! loop counts `0..255`. Nobody guesses that from reading the line, and a
//! reader who assumes `uint32` writes a comparison that is always true. That is
//! the number this feature exists to put on the screen.
//!
//! # Nothing here decides a type
//!
//! The types are the program-wide loop's (`kg_sem::elaborate`, read through
//! [`kg_db::Db::editor_loop`] and `Looped::answer_at` at the binding's
//! `VAR_DECL`) — the engine that compiles the program, whose table pass 10
//! and pass 19 read (`Frontend.hs:56,65`; `kg_db::looped`) — and, where the
//! loop has no type to show or the buffer's diagnostics never reach it,
//! pass 6's own table ([`kg_db::Db::type_table`], `Type/Inference.hs:25-69`).
//! A hint computed any other way would be a second type inference, free to
//! drift from the one that decides the `conversion` warning on the next
//! line; the user would see an editor annotating `i` as `uint16` while the
//! gutter complains that `uint16` does not fit a `uint8` parameter. This
//! module chooses *which* declarations to annotate and *how* to render them,
//! and asks the database everything else.
//!
//! # Inside a template's body: each instance's width
//!
//! The written body has no type — `inferType` visits a template for its
//! condition only (`Inference.hs:63`) — and the loop types each instance's
//! copy, whose widths follow the arguments. A binding there is annotated
//! with its type when every copy agrees, and otherwise with each distinct
//! type, `auto w: uint9 | uint17 = v + 1`, the tooltip naming the instance
//! behind each and saying when the list may be short (`super::instances`).
//! No instance yet, no hint: an uninstantiated body is still not typed.
//!
//! # Resolved, or no hint at all
//!
//! `showable` declines wherever the frontend has not actually resolved a type,
//! and that is the point rather than a gap.
//! The frontend is incomplete. Before the loop was read here, everything
//! under a `template <…>` and everything an instantiation produces was
//! unannotated: over the reference library **184 of 1030** `auto` bindings
//! (measured 2026-09, the whole corpus driven through this handler over
//! stdio, on the CST engine's table alone). The loop's types annotate the
//! copies' bodies and what an instantiated call returns, and are the only
//! types read since sprint W3.7; that count has not been re-measured.
//!
//! A `?` or an empty annotation for those would turn a fact about the *port*
//! into a claim about the *program*. The two failure modes are not symmetric: a
//! missing hint reads as "not inferred", which is true, while a wrong or blank
//! one reads as a fact about the code. So this shows a type only when the
//! frontend actually has one.
//!
//! # The defaults a call is given
//!
//! A call that leaves out a parameter with a default is compiled with the
//! default in its place (`kg_sem::named`), and nothing on the line says
//! which value that is. The hint says it where the argument would be
//! written, `scale(p⟨, shift: 1⟩)`, spelled as the declaration spells the
//! default — the callee is resolved as signature help resolves it, and its
//! parameter list read as written. Only where the program has a file that
//! may declare a default: a reference program pays one look at its files'
//! languages per request.

use kg_base::{LineIndex, TextRange, TextSize};
use kg_db::looped::IrAnswer;
use kg_resolve::ModuleSource;
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, ast};
use kg_ty::Ty;
use lsp_types::{
    InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, InlayHintTooltip, MarkupContent, MarkupKind,
};

use crate::convert::{self, PositionEncoding};
use crate::global_state::GlobalState;
use crate::handlers::{Answer, instances};

/// `textDocument/inlayHint`.
///
/// # The range is load-bearing, not advisory
///
/// A client re-requests hints for the visible viewport on every scroll, and
/// [`kg_db::Db::type_table`] is a whole-file fixpoint. The cheap half runs
/// first: `auto_bindings` walks only the subtrees that overlap the requested
/// range and **returns before any database is touched** when that range holds no
/// `auto` and no call at all. A range of calls alone asks the database what
/// their callees declare — in a program with a file that may declare a
/// default, and no further than its files' languages in one without — and
/// runs no inference.
///
/// The pruning is a real prune, not a filter over `descendants()`: a subtree
/// that cannot contain a hint position is never entered.
pub fn inlay_hints(state: &GlobalState, params: &InlayHintParams) -> Answer<Vec<InlayHint>> {
    let snap = state.snapshot(&params.text_document.uri)?;
    let index = snap.line_index();
    let enc = state.encoding();
    let range = convert::text_range(index, enc, params.range);

    let root = snap.parse.syntax_node();
    let sites = auto_bindings(&root, range);
    let calls = calls_in(&root, range);
    if sites.is_empty() && calls.is_empty() {
        return Ok(Vec::new());
    }

    // `with_db` answers `None` only for a document the server does not have
    // open, which `snapshot` above has already ruled out. An empty hint list is
    // the right degradation either way: unlike diagnostics, no hints does not
    // assert anything about the file.
    let hints = state.with_db(snap.file, |db| {
        let roots = [snap.file];
        let module = ModuleSource::File(snap.file);
        // A viewport of calls alone asks for no loop.
        let looped = if sites.is_empty() { None } else { db.editor_loop(&roots, snap.file) };
        let names = db.interner();
        let mut hints: Vec<InlayHint> = sites
            .iter()
            .filter_map(|site| {
                let range = site.decl.text_range();
                let ir = looped.as_ref().and_then(|l| l.answer_at(module, range, site.decl.kind(), names, &showable));
                match ir {
                    Some(IrAnswer::Written(ty)) => Some(type_hint(site.at, &show(&ty, names), None, index, enc)),
                    Some(IrAnswer::Copies { types, untyped, budget_exhausted }) => {
                        Some(copies_hint(site.at, &types, &untyped, budget_exhausted, names, index, enc))
                    }
                    // Nothing the loop shows — or no loop, on a buffer
                    // opened broken before a hover ran its own — no hint.
                    None => None,
                }
            })
            .collect();
        hints.extend(default_hints(db, snap.file, &calls, index, enc));
        hints.sort_by_key(|h| (h.position.line, h.position.character));
        hints
    });
    Ok(hints.unwrap_or_default())
}

fn show(ty: &Ty, names: &kg_base::Interner) -> String {
    kg_ty::subtype::show_pretty(ty, Some(names))
}

/// The hint for a binding inside a template's body (`super::instances`):
/// its type when every instance's copy agrees, and otherwise each distinct
/// type, `: uint9 | uint17`, with a tooltip naming which instance gives
/// which.
fn copies_hint(
    at: TextSize,
    types: &[(Ty, Vec<String>)],
    untyped: &[String],
    budget_exhausted: bool,
    names: &kg_base::Interner,
    index: &LineIndex,
    enc: PositionEncoding,
) -> InlayHint {
    let label = instances::label(types, names);
    let tooltip = instances::tooltip(types, untyped, budget_exhausted, names);
    type_hint(at, &label, Some(tooltip), index, enc)
}

/// Every call whose closing `)` lands inside `range`, pruned as
/// [`auto_bindings`] prunes.
fn calls_in(root: &SyntaxNode, range: TextRange) -> Vec<ast::CallExpr> {
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        if let Some(call) = ast::CallExpr::cast(node.clone())
            && close_of(&call).is_some_and(|at| range.contains_inclusive(at))
        {
            out.push(call);
        }
        stack.extend(node.children().filter(|child| {
            let span = child.text_range();
            span.end() >= range.start() && span.start() <= range.end()
        }));
    }
    out
}

/// Where a call's `)` starts: where an argument left out would be written.
fn close_of(call: &ast::CallExpr) -> Option<TextSize> {
    let close = call.arg_list()?.syntax().last_token().filter(|t| t.kind() == SyntaxKind::R_PAREN)?;
    Some(close.text_range().start())
}

/// The defaults each of `calls` is given, one hint a call — see the module
/// docs. None where no file of the program may declare one.
fn default_hints(
    db: &kg_db::Db,
    file: kg_base::FileId,
    calls: &[ast::CallExpr],
    index: &LineIndex,
    enc: PositionEncoding,
) -> Vec<InlayHint> {
    use kg_base::language::Feature;
    let roots = [file];
    if calls.is_empty() || !db.program(&roots).files.iter().any(|&m| db.features_of(m).has(Feature::DefaultArguments)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for call in calls {
        let (Some(probe), Some(at), Some(list)) = (super::signature_help::callee_probe(call), close_of(call), call.arg_list()) else {
            continue;
        };
        let Some(found) = db.definition_at(&roots, file, probe) else { continue };
        if found.range.file == kg_base::FileId::SYNTHETIC {
            continue;
        }
        let root = db.parse(found.range.file).syntax_node();
        let Some(decl) = super::signature_help::declaration_at(&root, found.range.range) else { continue };
        let params = ast::FnDecl::cast(decl.clone())
            .and_then(|f| f.param_list())
            .or_else(|| ast::FnProto::cast(decl).and_then(|f| f.param_list()));
        let Some(params) = params else { continue };
        let params: Vec<ast::Param> = params.params().collect();
        if let Some(hint) = default_hint(&list, &params, at, index, enc) {
            out.push(hint);
        }
    }
    out
}

/// The hint for one call: each parameter no argument fills, with its
/// default as declared — or none, where one it leaves out has no default
/// (an error, which the diagnostics say) or it leaves out none.
fn default_hint(list: &ast::ArgList, params: &[ast::Param], at: TextSize, index: &LineIndex, enc: PositionEncoding) -> Option<InlayHint> {
    let name_of = |p: &ast::Param| p.name().and_then(|n| n.ident_token()).map(|t| t.text().to_owned());
    let mut filled = vec![false; params.len()];
    let mut label = None;
    let mut position = 0;
    for element in list.syntax().children() {
        if let Some(l) = ast::ArgLabel::cast(element.clone()) {
            label = l.ident_token().map(|t| t.text().to_owned());
            continue;
        }
        let slot = match label.take() {
            Some(l) => params.iter().position(|p| name_of(p).as_deref() == Some(&l)),
            None => Some(position),
        };
        position += 1;
        if let Some(f) = slot.and_then(|i| filled.get_mut(i)) {
            *f = true;
        }
    }
    let mut given = Vec::new();
    for (param, _) in params.iter().zip(&filled).filter(|(_, f)| !**f) {
        param.eq_token()?;
        let default = super::signature_help::collapse_whitespace(param.expr()?.syntax());
        given.push(format!("{}: {default}", name_of(param)?));
    }
    if given.is_empty() {
        return None;
    }
    // After an argument, the comma that would come before the next; after a
    // trailing comma, or in an empty list, none.
    let after_argument = list
        .syntax()
        .children_with_tokens()
        .filter(|e| !e.kind().is_trivia() && e.kind() != SyntaxKind::R_PAREN)
        .last()
        .is_some_and(|e| !matches!(e.kind(), SyntaxKind::COMMA | SyntaxKind::L_PAREN));
    let given = given.join(", ");
    Some(InlayHint {
        position: convert::position(index, enc, at),
        label: InlayHintLabel::String(if after_argument { format!(", {given}") } else { given }),
        kind: Some(InlayHintKind::PARAMETER),
        // No `text_edits`: writing the defaults in is a choice the reader
        // makes, and the hint is what the call means either way.
        text_edits: None,
        tooltip: Some(InlayHintTooltip::String("the defaults this call is given, as declared".to_owned())),
        padding_left: Some(false),
        padding_right: Some(false),
        data: None,
    })
}

/// One declaration that spells its type `auto`, and where its hint goes.
#[derive(Debug)]
struct Binding {
    /// The `VAR_DECL` — the node the type table is keyed on. Both cases this
    /// module handles are one: `inferExpType'`'s `RangeForF` clause writes the
    /// induction variable's type onto the loop's own `VAR_DECL`
    /// (`Type.hs:1131-1145`), exactly where `infer VariableF{..}` writes an
    /// ordinary binding's (`Type.hs:1285-1287`).
    decl: SyntaxNode,
    /// End of the declared name's token: the hint is read as though it were
    /// written there, `auto a: uint8 = …`.
    at: TextSize,
}

/// Every `auto` / `const auto` declaration whose hint would land inside `range`,
/// in document order.
///
/// The walk is an explicit worklist rather than recursion: nesting is bounded by
/// the source, not by this function, and a loop body inside a member function
/// inside a class is exactly the shape that nests.
fn auto_bindings(root: &SyntaxNode, range: TextRange) -> Vec<Binding> {
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(node) = stack.pop() {
        if let Some(binding) = binding_at(&node, range) {
            out.push(binding);
        }
        // A subtree ending before the viewport starts, or beginning after it
        // ends, cannot hold a hint position. Touching counts as overlap: a hint
        // is a *point*, so a declaration whose name ends exactly at
        // `range.start()` is still inside a viewport that starts there.
        stack.extend(node.children().filter(|child| {
            let span = child.text_range();
            span.end() >= range.start() && span.start() <= range.end()
        }));
    }
    // The worklist visits siblings back to front; a client is free to ignore
    // order, but a stable document order is what makes a failing test readable.
    out.sort_by_key(|binding| binding.at);
    out
}

fn binding_at(node: &SyntaxNode, range: TextRange) -> Option<Binding> {
    if node.kind() != SyntaxKind::VAR_DECL {
        return None;
    }
    let decl = ast::VarDecl::cast(node.clone())?;
    if !is_auto(&decl.ty()?) {
        return None;
    }
    let at = decl.name()?.ident_token()?.text_range().end();
    // The prune above keeps whole subtrees that *overlap* the viewport; this is
    // the exact test, on the one offset the client will render at.
    range.contains_inclusive(at).then_some(Binding { decl: node.clone(), at })
}

/// `Auto` or `Const Auto` — the two shapes `infer VariableF{..}` deduces from an
/// initializer (`Type.hs:1285-1287`), and `isConstAuto` (`Type.hs:1034-1035`).
///
/// Anything else is a written type. Annotating those would be repeating the line
/// back to the reader, which is how an inlay-hint feature becomes noise people
/// switch off.
fn is_auto(ty: &ast::Type) -> bool {
    match ty {
        ast::Type::AutoType(_) => true,
        ast::Type::ConstType(inner) => matches!(inner.ty(), Some(ast::Type::AutoType(_))),
        _ => false,
    }
}

/// Whether a type is one a reader can be *shown*.
///
/// `tResolved` (`Type.hs:995-1031`) is the first half and not the whole story:
/// three of the types it calls resolved print as something that is not a type.
///
/// * **`TError`** counts as resolved (`Type.hs:1015`) — trap 26, and deliberate,
///   because an error type has to terminate the fixpoint rather than loop. It
///   prints its own *message* (`Type.hs:400`), so `for (const auto i : -1)`
///   would annotate `i` with the sentence "The iteration count of `static for`
///   can't be negative". That belongs in the gutter, where pass 19 puts it.
/// * **`TAuto`** counts as resolved too (`Type.hs:1001`). It prints `auto`,
///   which is the word already on the screen.
/// * **`TUntyped`** prints the empty string (`Type.hs:380`). A blank annotation
///   is the worst of the three: it reads as a rendering bug.
///
/// All three are checked *through* `tInner` (`Type.hs:598-601`) — `TConst` and
/// `TInstance` — because `const auto x = e` is `tConst (typeOf e)`
/// (`Type.hs:1285`) and wraps whatever `e` gave it.
fn showable(ty: &Ty) -> bool {
    if !ty.resolved() {
        return false;
    }
    let mut core = ty;
    while let Some(inner) = core.inner() {
        core = inner;
    }
    !matches!(core, Ty::Error(_) | Ty::Auto | Ty::Untyped)
}

/// The hint, spelled so that reading the line with it substituted in gives the
/// source someone would have written by hand.
///
/// The label carries its own `": "`, and both paddings are **off**: the hint
/// sits immediately after the name token, and the source already supplies
/// whatever space follows it. `auto a = 10u8` reads `auto a: uint8 = 10u8`;
/// padding on the right would double the space before the `=`.
fn type_hint(at: TextSize, ty: &str, tooltip: Option<String>, index: &LineIndex, enc: PositionEncoding) -> InlayHint {
    InlayHint {
        position: convert::position(index, enc, at),
        label: InlayHintLabel::String(format!(": {ty}")),
        kind: Some(InlayHintKind::TYPE),
        // No `text_edits`: "accept this hint" would have to *replace* `auto`,
        // and `auto`/`const auto` differ from the deduced type in constness
        // (`tUnConst` versus `tConst`, `Type.hs:1285-1286`). An edit that
        // silently changed which of the two a declaration is would be a
        // refactoring, not an annotation.
        text_edits: None,
        tooltip: tooltip.map(|value| {
            InlayHintTooltip::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value })
        }),
        padding_left: Some(false),
        padding_right: Some(false),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_state::{NoFile, test_uri};
    use kg_ty::TyRef;
    use lsp_types::{Position, Range, TextDocumentIdentifier, Uri, WorkDoneProgressParams};

    fn open(src: &str) -> (GlobalState, Uri) {
        let state = GlobalState::new();
        let uri = test_uri("hints.k");
        state.did_open(uri.clone(), src.to_owned(), 1);
        (state, uri)
    }

    fn params(uri: &Uri, range: Range) -> InlayHintParams {
        InlayHintParams {
            work_done_progress_params: WorkDoneProgressParams::default(),
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range,
        }
    }

    /// The whole buffer: the line saturates to the last one, which in every
    /// fixture here is the empty line after the trailing newline. This is the
    /// "no range restriction" request every test but the range ones wants.
    ///
    /// The column is `0` rather than `u32::MAX` on purpose: `convert::offset`
    /// computes `line_start + character` before clamping, which overflows for a
    /// column near `u32::MAX`. That is worth knowing about — some clients spell
    /// "end of line" that way — but it is not this module's to fix.
    fn whole_file() -> Range {
        Range::new(Position::new(0, 0), Position::new(u32::MAX, 0))
    }

    #[track_caller]
    fn hints_in(src: &str, range: Range) -> Vec<InlayHint> {
        let (state, uri) = open(src);
        inlay_hints(&state, &params(&uri, range)).unwrap()
    }

    #[track_caller]
    fn hints(src: &str) -> Vec<InlayHint> {
        hints_in(src, whole_file())
    }

    #[track_caller]
    fn labels(src: &str) -> Vec<String> {
        hints(src)
            .into_iter()
            .map(|hint| match hint.label {
                InlayHintLabel::String(s) => s,
                other => panic!("expected a plain label, got {other:?}"),
            })
            .collect()
    }

    /// `infer (VariableF _ _ Auto _ a _) = try (tUnConst . typeOf) a`
    /// (`Type.hs:1286`) — the width of the literal, which the line does not say.
    #[test]
    fn an_auto_binding_is_annotated_with_the_type_it_deduces() {
        assert_eq!(labels("inline void main()\n{\n    auto a = 10u8;\n}\n"), [": uint8"]);
    }

    /// …and `const auto` keeps its `const` (`Type.hs:1285`), which is the
    /// difference that decides whether the folder can use the binding as a
    /// constant at all.
    #[test]
    fn a_const_auto_binding_is_annotated_with_its_constness() {
        assert_eq!(labels("inline void main()\n{\n    const auto a = 10u8;\n}\n"), [": const uint8"]);
    }

    /// The headline: `constIndexType` is `clog2` of the iteration count
    /// (`Type.hs:1113-1118`), so a loop to 256 counts in **8** bits, not 9 and
    /// not 32. Ground truth: `test/syntax/auto-placeholder.k:10-25` asserts
    /// `bitsizeof(x) == 3` for `: 8` and `== 4` for `: 10`.
    #[test]
    fn an_induction_variable_is_annotated_with_the_clog2_of_its_limit() {
        for (limit, want) in
            [("2", "const uint1"), ("8", "const uint3"), ("10", "const uint4"), ("256", "const uint8")]
        {
            let src = format!("inline void main()\n{{\n    for (const auto i : {limit})\n    {{\n    }}\n}}\n");
            assert_eq!(labels(&src), [format!(": {want}")], "limit {limit}");
        }
    }

    /// `static for` and `unrolled for` take `constIndexType` unconditionally
    /// (`Type.hs:1139-1145`), so they are annotated on the same rule.
    #[test]
    fn a_static_for_induction_variable_is_annotated_too() {
        assert_eq!(
            labels("inline void main()\n{\n    static for (const auto i : 2)\n    {\n    }\n}\n"),
            [": const uint1"]
        );
    }

    /// A *named* limit folds to the same width as the literal it was declared
    /// with: `evaluateConstexpr` has already replaced it by the time
    /// `inferExpType` runs (`Inference.hs:47`, `Constexpr.hs:284-288`).
    ///
    /// `K` is a `const uint16` and `i` is a `const uint8`. A hint that showed
    /// the limit's *type* instead of its *value* would say `uint16` here — and
    /// be believed, because it looks so reasonable.
    #[test]
    fn a_named_constant_limit_folds_the_same_way_a_literal_does() {
        let src = "const uint16 K = 256;\n\
                   inline void main()\n{\n    for (const auto i : K)\n    {\n    }\n}\n";
        assert_eq!(labels(src), [": const uint8"]);
    }

    /// `| otherwise = try (tConst . TUnsigned) $ sizeOf x` (`Type.hs:1135-1137`)
    /// — a limit that is not a constant at all gives the induction variable the
    /// limit's own width, and that is a fact worth showing too. Ground truth:
    /// `test/syntax/auto-placeholder.k:28-34` asserts
    /// `bitsizeof(x) == bitsizeof(y)` for `for (const auto x : y)` with a
    /// `uint32 y`.
    #[test]
    fn a_limit_that_is_not_constant_shows_the_limits_own_width() {
        let src = "inline void main(uint32 y)\n{\n    for (const auto i : y)\n    {\n    }\n}\n";
        assert_eq!(labels(src), [": const uint32"]);
    }

    /// An `auto` with nothing to deduce from is `TUnresolved`
    /// (`Type.hs:1286`, `try`) — and an unresolved type is shown as **nothing**,
    /// not as `?`. This is the rule the whole module is built around: the
    /// frontend is incomplete, and a placeholder would read as a claim about the
    /// program rather than about the port.
    #[test]
    fn an_unresolved_auto_gets_no_hint_rather_than_a_placeholder() {
        assert!(hints("inline void main()\n{\n    auto a;\n}\n").is_empty());
        // The commonest real case: the initializer names something this
        // environment cannot see, so `typeOfSymbol` is `TUndefined`
        // (`Type.hs:1336`) and `tResolved TUndefined` is `False` (`Type.hs:1003`).
        assert!(hints("inline void main()\n{\n    auto a = nowhere;\n}\n").is_empty());
    }

    /// `for (auto i : 8)` — **not** `const auto` — is not an induction variable
    /// the reference types: `isConstAutoVar` fails (`Type.hs:1110-1111`), the
    /// clause falls through, and the declaration has no initializer to deduce
    /// from. So there is no hint, rather than a guess at what the user meant.
    #[test]
    fn a_non_const_auto_induction_variable_gets_no_hint() {
        assert!(hints("inline void main()\n{\n    for (auto i : 8)\n    {\n    }\n}\n").is_empty());
    }

    /// A written type is left alone. Repeating `uint32` back at a line that says
    /// `uint32` is how this feature turns into noise people switch off.
    #[test]
    fn a_declaration_with_a_written_type_is_not_annotated() {
        assert!(hints("inline void main()\n{\n    uint32 x = 1;\n}\n").is_empty());
        assert!(hints("class C\n{\n    uint8 field;\n}\n").is_empty());
    }

    /// `TError` is *resolved* (`Type.hs:1015`) and prints its message
    /// (`Type.hs:400`). Without the exclusion in [`showable`] this annotates a
    /// variable with an English sentence.
    #[test]
    fn an_error_type_is_never_offered_as_a_type() {
        let negative = Ty::Error(kg_base::SmolStr::new_static(
            "The iteration count of `static for` can't be negative",
        ));
        assert!(negative.resolved(), "the trap being guarded is that it *is* resolved");
        assert!(!showable(&negative));
        // …and through `tConst`, which is the shape `constIndexType` returns it
        // in (`Type.hs:1115-1116`, `tConst` passes `TError` through).
        assert!(!showable(&Ty::Const(TyRef::new(negative))));
        // End to end, on the source that produces it.
        assert!(hints("inline void main()\n{\n    for (const auto i : -1)\n    {\n    }\n}\n").is_empty());
    }

    /// The other two resolved-but-unshowable types, and the negative control
    /// that proves the predicate is not simply always `false`.
    #[test]
    fn auto_and_untyped_are_resolved_and_still_not_shown() {
        assert!(Ty::Auto.resolved() && Ty::Untyped.resolved());
        assert!(!showable(&Ty::Auto));
        assert!(!showable(&Ty::Untyped));
        assert!(!showable(&Ty::Const(TyRef::new(Ty::Auto))));
        assert!(!showable(&Ty::Unresolved) && !showable(&Ty::Undefined));
        assert!(showable(&Ty::Unsigned(8)));
        assert!(showable(&Ty::Const(TyRef::new(Ty::Unsigned(8)))));
    }

    /// The protocol contract: a type hint, at the end of the name, with the
    /// paddings that make it read as source.
    #[test]
    fn the_hint_is_a_type_hint_positioned_just_after_the_name() {
        let src = "inline void main()\n{\n    auto a = 10u8;\n}\n";
        let out = hints(src);
        assert_eq!(out.len(), 1);
        let hint = &out[0];
        assert_eq!(hint.kind, Some(InlayHintKind::TYPE));
        // `    auto a` — line 2, and the name ends at column 10.
        assert_eq!(hint.position, Position::new(2, 10));
        assert_eq!(hint.padding_left, Some(false));
        assert_eq!(hint.padding_right, Some(false));
        assert!(hint.text_edits.is_none(), "accepting a hint must not rewrite `auto`");
    }

    /// …and the paddings are right because the line reads as source with the
    /// label spliced in at the hint's own position.
    #[test]
    fn a_rendered_hint_reads_as_the_source_someone_would_have_written() {
        let src = "inline void main()\n{\n    auto a = 10u8;\n    const auto b = a;\n}\n";
        let out = hints(src);
        let mut rendered = src.to_owned();
        // Back to front, so an earlier splice does not move a later position.
        for hint in out.iter().rev() {
            let InlayHintLabel::String(label) = &hint.label else { panic!("plain labels only") };
            let index = kg_base::LineIndex::new(src);
            let at = convert::offset(&index, PositionEncoding::Utf16, hint.position);
            rendered.insert_str(usize::from(at), label);
        }
        assert_eq!(
            rendered,
            "inline void main()\n{\n    auto a: uint8 = 10u8;\n    const auto b: const uint8 = a;\n}\n"
        );
    }

    /// The requested range is the viewport, and answering outside it is the
    /// standard way this feature becomes slow: a client re-requests on every
    /// scroll.
    #[test]
    fn only_declarations_inside_the_requested_range_are_answered() {
        let src = "inline void main()\n{\n    auto a = 10u8;\n    auto b = 20u16;\n    auto c = 30u32;\n}\n";
        assert_eq!(labels(src), [": uint8", ": uint16", ": uint32"], "the unrestricted answer");

        // Only line 3 (`auto b`).
        let only_b = hints_in(src, Range::new(Position::new(3, 0), Position::new(3, 19)));
        assert_eq!(only_b.len(), 1);
        assert_eq!(only_b[0].position, Position::new(3, 10));
    }

    /// A hint whose position is exactly at either end of the requested range is
    /// *inside* it — an off-by-one here drops the first or last hint of every
    /// viewport, which is the bug that would be blamed on the compiler.
    #[test]
    fn a_hint_exactly_at_the_edge_of_the_range_is_kept() {
        let src = "inline void main()\n{\n    auto a = 10u8;\n}\n";
        // The name of `a` ends at line 2, column 10.
        let starts_there = hints_in(src, Range::new(Position::new(2, 10), Position::new(3, 0)));
        assert_eq!(starts_there.len(), 1, "a hint at the range's start was dropped");
        let ends_there = hints_in(src, Range::new(Position::new(0, 0), Position::new(2, 10)));
        assert_eq!(ends_there.len(), 1, "a hint at the range's end was dropped");

        // …and the case that makes the *prune* keep a merely touching subtree:
        // an induction variable's `VAR_DECL` is `const auto i`, with no
        // semicolon, so the declaration ends at exactly the offset its hint sits
        // at. A prune that required strict overlap would never enter it.
        let loop_src = "inline void main()\n{\n    for (const auto i : 8)\n    {\n    }\n}\n";
        assert_eq!(
            hints_in(loop_src, Range::new(Position::new(2, 21), Position::new(3, 0))).len(),
            1,
            "the induction variable's declaration ends where its hint begins"
        );
    }

    /// The prune keeps any subtree that *overlaps* the viewport, but a hint is a
    /// single point and that point can lie outside a viewport the declaration
    /// crosses. Without the exact test on the position, a client asking about
    /// the line holding a long declaration's initializer gets an annotation
    /// drawn on the line above — one it never asked about, and will not clear
    /// when it scrolls.
    #[test]
    fn a_declaration_straddling_the_viewport_answers_only_where_its_name_is() {
        let src = "inline void main()\n{\n    auto wide\n        = 10u8;\n}\n";
        assert_eq!(labels(src), [": uint8"], "the unrestricted answer");
        // Line 3 is `        = 10u8;`: inside the declaration, past its name.
        let below = hints_in(src, Range::new(Position::new(3, 0), Position::new(3, 15)));
        assert!(below.is_empty(), "a hint was placed on a line nobody asked about: {below:?}");
    }

    /// Nesting: the pruning walk descends, rather than stopping at the first
    /// level of a subtree that overlaps the range. A loop body is two blocks
    /// down from the function.
    #[test]
    fn a_binding_nested_inside_another_declarations_scope_is_reached() {
        let src = "inline void main()\n{\n    for (const auto i : 8)\n    {\n        auto inner = 7u8;\n    }\n}\n";
        assert_eq!(labels(src), [": const uint3", ": uint8"]);
    }

    /// **A limitation, asserted so it cannot rot.** Nothing inside a lambda is
    /// annotated, and neither is the binding a lambda initialises: the inference
    /// traversal hands a `LambdaF` to the algebra untouched
    /// (`Inference.hs:55`, `VisitPolicy::Skip`) and `infer LambdaF{} =
    /// TUnresolved` (`Type.hs:1231`). The type arrives at pass 8
    /// (`addLambdaWrappers`, `Template.hs:79`), which is unwired.
    ///
    /// So this is a clean decline, not a wrong answer — and the day pass 8
    /// lands, this test fails and tells its reader the hint is now available.
    #[test]
    fn a_lambda_is_not_annotated_until_pass_8_lands() {
        let src = "inline void main()\n{\n    auto f = [](uint8 p) -> uint8 { auto inner = 7u8; return inner; };\n}\n";
        assert!(hints(src).is_empty(), "{:?}", labels(src));
    }

    const BOX: &str = "template <auto N>\nclass Box\n{\n    uint32[N] items;\n\n    inline auto first()\n    {\n        auto head = items[0] + N;\n        auto wide = items[0] * N;\n        return head;\n    }\n}\n\n\
                       inline void main()\n{\n    static Box<4> a;\n    static Box<9> b;\n    auto r = a.first();\n}\n";

    fn tooltip(hint: &InlayHint) -> Option<&str> {
        match &hint.tooltip {
            Some(InlayHintTooltip::MarkupContent(m)) => Some(&m.value),
            Some(InlayHintTooltip::String(s)) => Some(s),
            None => None,
        }
    }

    /// The widths of a template's body are its instances': `head` is
    /// `items[0] + N`, `uint33` in both `Box<4>` and `Box<9>`, so one type;
    /// `wide` is `items[0] * N`, `uint34` and `uint36` — `N` is a literal
    /// in each copy, and a product with one widens by `clog2` of it
    /// (`tMulLiteral`, `Type.hs:763-764,831-835`): `kg-oracle --no-base
    /// --passes 19`, 2026-10-10, prints `uint34 wide` in `Box<|@4|>` and
    /// `uint36 wide` in `Box<|@9|>` — so both, each instance named in the
    /// tooltip. And in
    /// the plain function, `r` is what the instantiated call returns.
    /// Red with the hints reading the CST engine (only the plain
    /// function's would be missing too — none of the three is there).
    #[test]
    fn a_template_body_is_annotated_with_each_instance_s_width() {
        let out = hints(BOX);
        let labels: Vec<String> = out
            .iter()
            .map(|h| match &h.label {
                InlayHintLabel::String(s) => s.clone(),
                other => panic!("expected a plain label, got {other:?}"),
            })
            .collect();
        assert_eq!(labels, [": uint33", ": uint34 | uint36", ": uint33"]);
        assert_eq!(
            tooltip(&out[0]),
            Some("```kanagawa\nuint33  // Box<4>, Box<9>\n```\n\nTyped in 2 instances: a template's body has no type of its own.")
        );
        assert_eq!(
            tooltip(&out[1]),
            Some("```kanagawa\nuint34  // Box<4>\nuint36  // Box<9>\n```\n\nTyped in 2 instances: a template's body has no type of its own.")
        );
        // Typed where it is written: a plain hint, as before.
        assert_eq!(tooltip(&out[2]), None);
    }

    /// A buffer that does not parse, and that no loop has run for — opened
    /// broken — has none to read (`Db::editor_loop`; a hint runs none), and
    /// is not annotated: the CST engine annotated its plain bindings until
    /// sprint W3.7 (the sprint plan's W3 entry weighs what that costs).
    /// Red with the hints asking `Db::looped` directly (the loop runs and
    /// annotates `c`).
    #[test]
    fn a_buffer_opened_broken_is_not_annotated() {
        let src = format!("{BOX}\ninline void broken()\n{{\n    auto c = 7u8;\n    @@@\n}}\n");
        assert_eq!(labels(&src), Vec::<String>::new());
    }

    /// A document nobody opened is declined, not answered with an empty list
    /// that a client would cache as "this file has no hints".
    #[test]
    fn an_unknown_document_is_declined_not_faked() {
        let state = GlobalState::new();
        let uri = test_uri("never-opened.k");
        let err = inlay_hints(&state, &params(&uri, whole_file())).unwrap_err();
        assert_eq!(err, NoFile::NotOpen);
    }

    /// A buffer mid-edit still annotates what parsed. `Db::check` refuses to run
    /// the passes at all when a file fails to parse (`Main.hs:73-75`) because an
    /// invented *diagnostic* is a false positive — but a hint is not a
    /// diagnostic, and blanking every annotation on the keystroke that
    /// transiently breaks the parse is the flicker that makes people turn the
    /// feature off. The hint is the last loop's, read through the edit
    /// (sprint W3.4); until sprint W3.7 the test opened the buffer broken,
    /// which the CST engine annotated with no loop.
    #[test]
    fn a_file_with_a_syntax_error_still_annotates_what_parsed() {
        let src = "inline void main()\n{\n    auto a = 10u8;\n    @@@\n}\n";
        let (state, uri) = open(&src.replace("    @@@\n", ""));
        let file = state.snapshot(&uri).unwrap().file;
        state.with_db(file, |db| db.looped(&[file])).expect("open");
        state.did_change_full(&uri, src.to_owned(), 2);
        let snap = state.snapshot(&uri).unwrap();
        assert!(
            !snap.parse.diagnostics(snap.file).is_empty(),
            "the fixture must fail to parse, or it proves nothing"
        );
        let labels: Vec<String> = inlay_hints(&state, &params(&uri, whole_file()))
            .expect("open")
            .into_iter()
            .map(|hint| match hint.label {
                InlayHintLabel::String(s) => s,
                other => panic!("expected a plain label, got {other:?}"),
            })
            .collect();
        assert_eq!(labels, [": uint8"]);
    }

    /// A call short of arguments shows the defaults it is given, as declared,
    /// where the next argument would be written — after the positional
    /// arguments, around the named ones, with no comma after a trailing one or
    /// in an empty list — and a call given every argument, or one short of a
    /// parameter with no default, shows nothing. Red with the filled slots
    /// miscounted, a label's parameter not taken as filled, or the comma
    /// always or never written.
    #[test]
    fn a_short_call_shows_the_defaults_it_is_given() {
        use crate::handlers::references::tests::{lay_out_1_1, open_one};
        let text = "\
inline uint8 scale(uint8 value = 0, uint8 shift = 1, bool round = false)
{
    return round ? value >> shift : value << shift;
}

inline uint8 mix(uint8 value, uint8 by = 2)
{
    return value + by;
}

inline uint8 main(uint8 p)
{
    uint8 a = scale(p);
    uint8 b = scale(p, round: true);
    uint8 c = scale(shift: 3, );
    uint8 d = scale();
    uint8 e = scale(p, 2, true);
    uint8 f = mix(by: 1);
    return a + b + c + d + e + f;
}
";
        let files = [("hints.k", text)];
        let (dir, state, src) = lay_out_1_1("inlay-defaults", &files);
        let uri = open_one(&state, &src, &files, "hints.k");
        let hints = inlay_hints(&state, &params(&uri, whole_file())).expect("open");
        std::fs::remove_dir_all(&dir).unwrap();
        let shown: Vec<(u32, u32, String)> = hints
            .into_iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| match h.label {
                InlayHintLabel::String(s) => (h.position.line, h.position.character, s),
                other => panic!("expected a plain label, got {other:?}"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                (12, 21, ", shift: 1, round: false".to_owned()),
                (13, 34, ", shift: 1".to_owned()),
                (14, 30, "value: 0, round: false".to_owned()),
                (15, 20, "value: 0, shift: 1, round: false".to_owned()),
            ]
        );
    }
}
