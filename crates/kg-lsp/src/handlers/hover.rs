//! `textDocument/hover` — what the symbol under the cursor *is*.
//!
//! [`kg_db::Db::definition_at`] says which declaration the name denotes, and
//! the type is the program-wide loop's at that declaration
//! ([`kg_db::Db::ir_answer_of_definition`] over
//! [`kg_db::Db::editor_loop_or_own`]) — the engine that compiles the program
//! (sprint W3): the loop the buffer's diagnostics run, or on a buffer that
//! does not parse the last one computed, mapped onto the text, or — where
//! there is none at all, a buffer opened broken — the buffer's own, run once.
//! Where the loop has no type to show, a declaration typed since it ran
//! included, the hover is the declaration's kind and name, "not yet
//! inferred" — the CST inference engine answered there until sprint W3.7,
//! deleted since. Everything this module adds is presentation — a kind
//! label, a code fence, a `Range` for the client to underline.
//!
//! # Inside a template's body
//!
//! The written body has no type — `inferType` visits a template for its
//! condition only — and each instance's copy has one, which differs with the
//! arguments. The fence then holds one line per distinct type, each with the
//! instances that give it as a comment, and a line under the kind says so
//! and says when the list may be short (`super::instances`):
//!
//! ```text
//! uint9 w   // take<uint8>
//! uint17 w  // take<uint16>
//!
//! **local** `take::$::w`
//!
//! Typed in 2 instances: a template's body has no type of its own.
//! ```
//!
//! # A member after a `.`
//!
//! `a.get` names `get` inside `a`'s type, and its type is the one the
//! receiver's instance gives it ([`kg_db::Db::member_answer_at`]): on a
//! `static Box<4> a`, `Box<4>`'s `get`, not every instance of `Box`'s that
//! the declaration's own answer lists — inside a template's body, each
//! enclosing instance's, listed as above. A member template's call shows
//! the instance it compiles, as signature help does
//! ([`kg_db::Db::member_callee_at`]): `fixture.Run<3>(t)` reads
//! `(uint32) -> uint32 Run  // Run<3>`. Where the receiver's instances reach
//! two declarations — a class declaring `go` in each branch of a `static
//! if` — each is named under the fence, and counted.
//!
//! # Spelling types the way the errors spell them
//!
//! The rendered type is [`kg_ty::subtype::show_pretty`], which is `showPretty =
//! show . pretty` (`Type.hs:376-401`) — the same function the conversion
//! check (`kg_sem::check`) formats `"Narrowing conversion from \`const
//! uint16\`"` with. A second renderer here would let a hover say `u32` where the gutter
//! says `uint32`, and a user comparing the two would be right to distrust both.
//! That is also why a function reads `(uint32) -> void` rather than the C-style
//! `void f(uint32)` an editor might expect: the arrow form is the reference's
//! own spelling of a `TFunction`, and inventing a second one to look familiar
//! would break the property this module exists to keep.
//!
//! # Declining a type is not declining a hover
//!
//! The frontend is incomplete: a declaration under a `template <…>` that no
//! instance has copied yet is [`Ty::Unresolved`] to both engines. (A name
//! from *another module* no longer is — [`kg_db::Db::type_table`]'s environment
//! resolves through the import graph.) `showPretty` renders that as the word
//! `unresolved` (`Type.hs:235`) — a perfectly good rendering of a *compiler*
//! state and a lie in a tooltip, where it reads as a type named "unresolved".
//! So `shown` filters it out and the hover says, in words, that the type is
//! not inferred yet. The kind and the qualified name are still facts, and they
//! are still reported.
//!
//! Every case [`kg_db::Db::definitions_at`] answers nothing for — an import
//! qualifier, a member whose receiver no engine types, an intrinsic, an
//! ambiguous name, a caret on punctuation — is a hover of `null`. They are
//! enumerated with citations on [`kg_db::Db::definition_at`].

use kg_base::{Interner, SymbolPath};
use kg_db::Definition;
use kg_db::looped::IrAnswer;
use kg_resolve::{DeclareFlag, DefKind};
use kg_ty::Ty;
use lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};

use crate::convert;
use crate::global_state::GlobalState;
use crate::handlers::{Answer, instances};

/// Answers with a markdown [`Hover`], or `None` when nothing under the cursor
/// resolves to a declaration.
///
/// `Ok(None)` and `Err(NoFile)` are different facts — "I looked and found
/// nothing" and "I have no such document" — that the wire cannot tell apart,
/// since the dispatcher turns both into `null` (`main_loop::on!`). They are
/// kept distinct because a test can tell them apart, and because answering
/// `Ok(None)` for a document nobody opened would be claiming to have looked.
pub fn hover(state: &GlobalState, params: &HoverParams) -> Answer<Option<Hover>> {
    let at = &params.text_document_position_params;
    let snap = state.snapshot(&at.text_document.uri)?;
    let offset = convert::offset(snap.line_index(), state.encoding(), at.position);

    // Rendered inside the closure because the qualified name is a
    // `SymbolPath` of interned components and the `Interner` that resolves them
    // belongs to the database, which `with_db` will not let out.
    //
    // The file is its own root, matching `GlobalState::frontend_diagnostics`:
    // an editor is looking at one buffer, and the program this is answered
    // against is the one that buffer entails.
    let markup = state
        .with_db(snap.file, |db| {
            let roots = [snap.file];
            // An import names a module, not a declaration: the module, and
            // the file the program loaded for it (`Db::import_at`).
            if let Some(module) = db.import_at(&roots, snap.file, offset) {
                let shown = import_markdown(state, &snap.parse.syntax_node(), offset, module);
                return Some(with_docs(shown, db.module_documentation(module)));
            }
            let found = db.definitions_at(&roots, snap.file, offset);
            let first = found.first()?;
            // What the declaration's comments say (`//|` before, `//<` after):
            // the first declaration's that has any, where the name reaches
            // several.
            let docs = found.iter().find_map(|d| db.documentation(d));
            let names = db.interner();
            let name = first.qualified_name.last().map(|n| names.resolve(n).to_string()).unwrap_or_default();
            let declared = declared(db, snap.file, &found, &name);
            let printable = |t: &Ty| shown(t).is_some();
            // On a buffer that does not parse, the last loop, mapped — or,
            // where there is none, the buffer's own, run once.
            let looped = db.editor_loop_or_own(&roots, snap.file);
            // A member template's call: the instance it compiles, said as
            // signature help says it — the member's own declaration is a
            // template, which has no one signature.
            if let Some(answer) = db.member_callee_at(&looped, snap.file, offset) {
                let (lines, said) = instances::called(looped.looped(), &answer, &name, names);
                return Some(with_docs(format!("{}\n\n{declared}\n\n{said}", instances::fence(&lines)), docs));
            }
            // A member after a `.` is typed as the receiver's instance has
            // it; anything else, and a member the loop has nothing to show
            // for, at its declaration — which for several declarations is
            // no one answer. Where the loop has nothing to show, the
            // declaration's kind and name are the hover, "not yet
            // inferred": the CST engine answered there until sprint W3.7,
            // and what that costs is the plan's W3 entry.
            let ir = db
                .member_answer_at(&looped, snap.file, offset, &printable)
                .or_else(|| (found.len() == 1).then(|| db.ir_answer_of_definition(&looped, first, &printable)).flatten());
            let shown = match ir {
                Some(IrAnswer::Written(ty)) => markdown(&declared, &name, &ty, names),
                Some(IrAnswer::Copies { types, untyped, budget_exhausted }) => {
                    copies_markdown(&declared, &name, &types, &untyped, budget_exhausted, names)
                }
                None => markdown(&declared, &name, &Ty::Unresolved, names),
            };
            Some(with_docs(shown, docs))
        })
        .flatten();
    let Some(markup) = markup else { return Ok(None) };

    Ok(Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markup,
        }),
        // The token the client underlines is the token that resolved, not a
        // second guess at what the caret is on: `kg_db::navigate::identifier_at`
        // is the function `definition_at` itself used to pick it.
        range: kg_db::navigate::identifier_at(&snap.parse.syntax_node(), offset)
            .map(|token| convert::range(snap.line_index(), state.encoding(), token.text_range())),
    }))
}

/// The tooltip `shown`, and under a rule the documentation, where there is
/// any: the reference's Pandoc Markdown as CommonMark, headings as bold lines
/// so they do not outshout the declaration (`kg_syntax::to_commonmark`).
fn with_docs(shown: String, docs: Option<String>) -> String {
    match docs {
        Some(docs) => format!("{shown}\n\n---\n\n{}", kg_syntax::to_commonmark(&docs, kg_syntax::Headings::Bold)),
        None => shown,
    }
}

/// An import's tooltip: the module as the import spells it, and a link to
/// the file the program loaded for it.
fn import_markdown(state: &GlobalState, root: &kg_syntax::SyntaxNode, offset: kg_base::TextSize, module: kg_base::FileId) -> String {
    use kg_syntax::AstNode;
    let named = kg_db::navigate::identifier_at(root, offset)
        .and_then(|t| t.parent_ancestors().find_map(kg_syntax::ast::ImportDecl::cast))
        .and_then(|decl| decl.module_path())
        .map(|path| path.syntax().text().to_string().split_whitespace().collect::<String>())
        .unwrap_or_default();
    let file = state.vfs().path(module).map(|p| p.to_string()).unwrap_or_default();
    let loaded = match crate::handlers::uri_of(state, module) {
        Some(uri) => format!("[`{file}`]({})", uri.as_str()),
        None => format!("`{file}`"),
    };
    format!("```kanagawa\nimport {named}\n```\n\n**module** `{named}`, loaded from {loaded}")
}

/// The body of the tooltip: a fenced signature, then the kind and the fully
/// qualified name.
///
/// The fence holds `<type> <name>`, which is how a Kanagawa declaration is
/// written — except for a declaration whose own type is a `TType`, i.e. a
/// `struct`, `class`, `enum`, `union` or `using`. `showPretty` renders those as
/// `type Foo` / `type uint8` (`Type.hs:378`), a spelling of the *declaration*
/// rather than of a value, and appending the name to it would produce
/// `type Foo Foo`.
///
/// When there is no type to print there is no signature either, so the fence is
/// left out rather than filled with the bare name — the kind line already
/// carries the name, and an empty-looking code block reads as a rendering bug
/// rather than as the absence of an answer.
///
/// `declared` is the kind and name ([`declared`]), and `name` what the fence
/// names.
fn markdown(declared: &str, name: &str, ty: &Ty, names: &Interner) -> String {
    let printed = shown(ty);
    let mut out = String::new();

    if let Some(ty) = printed {
        out.push_str("```kanagawa\n");
        out.push_str(&kg_ty::subtype::show_pretty(ty, Some(names)));
        if !matches!(ty, Ty::Type(_)) {
            out.push(' ');
            out.push_str(name);
        }
        out.push_str("\n```\n\n");
    }

    out.push_str(declared);
    if printed.is_none() {
        out.push_str("\n\nType not yet inferred.");
    }
    out
}

/// [`markdown`] for a declaration inside a template's body, or a member
/// reached through a receiver typed per instance: a fence line per
/// distinct type the instances give, each spelled as `markdown` spells
/// one, with those instances as a comment, then the kind and name, then
/// what the list is (`super::instances`).
fn copies_markdown(
    declared: &str,
    name: &str,
    types: &[(Ty, Vec<String>)],
    untyped: &[String],
    budget_exhausted: bool,
    names: &Interner,
) -> String {
    let lines: Vec<(String, String)> = types
        .iter()
        .map(|(ty, instances)| {
            let mut code = kg_ty::subtype::show_pretty(ty, Some(names));
            if !matches!(ty, Ty::Type(_)) {
                code.push(' ');
                code.push_str(name);
            }
            (code, instances::listed(instances))
        })
        .collect();
    let typed = types.iter().map(|(_, i)| i.len()).sum();
    format!("{}\n\n{declared}\n\n{}", instances::fence(&lines), instances::caveats(typed, untyped, budget_exhausted))
}

/// The kind and the qualified name of each declaration the name reaches —
/// one, but for a member whose receiver's instances reach several
/// (`kg_db::Db::definitions_at`), which are listed and counted.
///
/// The qualified name is [`join`]ed the way the reference prints one, and is
/// otherwise unedited: it keeps the synthetic scope components `getSymbols`
/// builds keys from — `$` for a function body (`Desugar.hs:173-174`), `$quasiN`
/// for a braced block (`Parser.hs:135-140`) — so a local reads `main::$::x`,
/// and a member each branch of a `static if` declares `C::$quasi0::go` and
/// `C::$quasi1::go`. That is the key the symbol map actually holds, and
/// trimming it to look tidier would print a name that identifies nothing. The
/// one exception is a struct or union field, which is in no map at all and is
/// shown under its aggregate: `S::x`.
fn declared(db: &kg_db::Db, file: kg_base::FileId, found: &[Definition], name: &str) -> String {
    let names = db.interner();
    let mut lines: Vec<String> = Vec::new();
    for def in found {
        // A field's key is `$quasiN::x` — the struct body's quasi scope,
        // which no user ever spells and no map ever holds (`Symbols.hs:48-49`).
        let display = if def.kind == DefKind::Variable(DeclareFlag::DeclareMember) {
            let program = db.program(&[file]);
            let items = program.items();
            items
                .item(def.def)
                .and_then(|field| field.parent)
                .and_then(|parent| items.item(kg_resolve::DefId::new(def.def.module, parent)))
                .zip(def.qualified_name.last())
                .map(|(owner, name)| owner.qualified_name.child(name))
                .unwrap_or_else(|| def.qualified_name.clone())
        } else {
            def.qualified_name.clone()
        };
        let line = format!("**{}** `{}`", kind_label(def.kind), join(&display, names));
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    if found.len() > 1 {
        lines.push(format!("The instances reach {} declarations of `{name}`.", found.len()));
    }
    lines.join("\n\n")
}

/// The type to print, or `None` when printing one would overstate what the
/// frontend knows.
///
/// Most of it is `tResolved` (`Type.hs:1001-1015`), which is already `False`
/// for [`Ty::Unresolved`] (not known *this round*), [`Ty::Undefined`] (lookup
/// failed) and [`Ty::Template`] (a template *name*, not an instance).
///
/// `TError` it accepts — deliberately, so that an error terminates the fixpoint
/// rather than looping (`Type.hs:1015`) — and this does not: the payload is a
/// diagnostic message, which belongs in the gutter and not in a tooltip that
/// claims to be printing a type.
///
/// Two more `tResolved` accepts and this does not:
///
/// * [`Ty::Untyped`], which `pretty` renders as the empty string
///   (`Type.hs:380`). It is what a statement or an attribute has by
///   construction (`docs/spec/types.md` §1.5), and an empty code fence is not
///   an answer;
/// * [`Ty::Auto`], which is `True` at `Type.hs:1001` and is nonetheless the
///   *absence* of a deduced type until `deduceAuto` (pass 17) runs — and pass
///   17 is unwired. A resolved `auto` variable already carries its deduced type
///   here (`inferType` sets it, `Type.hs:1110-1111`), so a surviving `TAuto` is
///   precisely the case where nothing was deduced.
fn shown(ty: &Ty) -> Option<&Ty> {
    let printable = !matches!(ty, Ty::Untyped | Ty::Auto) && ty.resolved() && !ty.is_error();
    printable.then_some(ty)
}

/// How the declaration was written, in the words a user would use.
///
/// Exhaustive rather than defaulted, so that a new [`DefKind`] is a compile
/// error here instead of a silently mislabelled tooltip.
fn kind_label(kind: DefKind) -> &'static str {
    match kind {
        DefKind::Alias => "alias",
        DefKind::Capture => "lambda capture",
        DefKind::Struct => "struct",
        DefKind::Union => "union",
        DefKind::Class => "class",
        DefKind::Enum => "enum",
        DefKind::EnumConstant => "enum constant",
        DefKind::Function => "function",
        DefKind::FunctionDecl => "function declaration",
        DefKind::Variable(DeclareFlag::DeclareGlobal) => "global",
        DefKind::Variable(DeclareFlag::DeclareClass) => "class field",
        DefKind::Variable(DeclareFlag::DeclareLocal) => "local",
        DefKind::Variable(DeclareFlag::DeclareStatic) => "static local",
        // A struct/union member is an item for navigation and never a symbol
        // (`Symbols.hs:48-49`, `Parser.hs:465`); `definition_at` reaches it
        // through the base expression's type.
        DefKind::Variable(DeclareFlag::DeclareMember) => "field",
        // `uninitConst` (`Parser.hs:451`), which in this grammar is only ever
        // the induction variable of a `for` (`kg_resolve::lower`).
        DefKind::Variable(DeclareFlag::DeclareUninitConst) => "loop variable",
        DefKind::FuncParam => "parameter",
        DefKind::Template => "template",
        DefKind::TemplateParam => "template parameter",
        // Neither can be the answer to `definition_at`: `$exportN` and
        // `$errors` are synthetic keys no written name spells, and
        // `navigate::declarations_by_name_token` drops both.
        DefKind::ExportType => "export",
        DefKind::Error => "error",
    }
}

/// `prettyQualifiedName` (`Type.hs:404-416`): `::` between components, with
/// `unmangleModuleNamespace` applied to the **first** one and to no other, so
/// `@data@closure@core::Closure` prints as `data.closure.core::Closure`.
///
/// The same rule `show_pretty` applies to a nominal type's name, reached through
/// `kg_resolve`'s copy of the unmangler rather than a second one — a hover whose
/// two lines spelled the same module differently would be its own small lie.
fn join(path: &SymbolPath, names: &Interner) -> String {
    let mut out = String::new();
    for (i, &segment) in path.segments().iter().enumerate() {
        if i > 0 {
            out.push_str("::");
        }
        let text = names.resolve(segment);
        match i {
            0 => out.push_str(&kg_resolve::Namespace::unmangle(&text)),
            _ => out.push_str(&text),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_state::{NoFile, test_uri};
    use lsp_types::{
        Position, Range, TextDocumentIdentifier, TextDocumentPositionParams,
        WorkDoneProgressParams, Uri,
    };

    fn params(uri: &Uri, position: Position) -> HoverParams {
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        }
    }

    /// The markdown body, or `None`. Panics on the two legacy `HoverContents`
    /// shapes, which this handler must never produce.
    #[track_caller]
    fn body(state: &GlobalState, uri: &Uri, position: Position) -> Option<String> {
        let out = hover(state, &params(uri, position)).expect("the document is open")?;
        match out.contents {
            HoverContents::Markup(m) => {
                assert_eq!(m.kind, MarkupKind::Markdown, "the body must be markdown");
                Some(m.value)
            }
            other => panic!("expected MarkupContent, got {other:?}"),
        }
    }

    const SRC: &str = "\
class Adder {
    uint32 total;
    inline void add(uint32 x) {
        total = total + x;
    }
}
";

    fn state() -> (GlobalState, Uri) {
        let state = GlobalState::new();
        let uri = test_uri("adder.k");
        state.did_open(uri.clone(), SRC.to_owned(), 1);
        (state, uri)
    }

    /// The caret is on the second `total` of `total = total + x` — a *use*, and
    /// the answer is the *declaration's* type. A hover that read the type off
    /// the cursor's own node would describe the expression instead.
    #[test]
    fn a_use_reports_the_type_kind_and_qualified_name_of_its_declaration() {
        let (state, uri) = state();
        let out = body(&state, &uri, Position::new(3, 18)).expect("`total` must resolve");
        assert_eq!(out, "```kanagawa\nuint32 total\n```\n\n**class field** `Adder::total`");
    }

    /// A struct field hovers to its declared type and is shown under the struct
    /// — `S::x`, not the `$quasi0::x` key nothing but the item tree holds.
    #[test]
    fn a_struct_field_hovers_under_its_struct() {
        let state = GlobalState::new();
        let uri = test_uri("point.k");
        state.did_open(
            uri.clone(),
            "struct S\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    S s = {};\n    uint32 v = s.x;\n}\n".to_owned(),
            1,
        );
        let out = body(&state, &uri, Position::new(8, 17)).expect("`s.x` must resolve");
        assert_eq!(out, "```kanagawa\nuint32 x\n```\n\n**field** `S::x`");
    }

    /// The declaration site answers the same thing as the use site: `Item::range`
    /// is the name token, and `definition_at` matches the caret against it.
    #[test]
    fn the_declaration_itself_hovers_to_itself() {
        let (state, uri) = state();
        assert_eq!(
            body(&state, &uri, Position::new(1, 12)),
            body(&state, &uri, Position::new(3, 18)),
        );
    }

    /// A parameter is a real declaration with a real type, and the one an editor
    /// is asked about most.
    #[test]
    fn a_parameter_reports_its_declared_type() {
        let (state, uri) = state();
        assert_eq!(&SRC.lines().nth(3).unwrap()[24..25], "x");
        let out = body(&state, &uri, Position::new(3, 24)).expect("`x` must resolve");
        assert!(out.starts_with("```kanagawa\nuint32 x\n```"), "{out}");
        assert!(out.contains("**parameter** `Adder::add::x`"), "{out}");
    }

    /// A function's type is a `TFunction`, and `showPretty` spells one
    /// `(params) -> ret` (`Type.hs:392-395`). The arrow form is deliberate: see
    /// the module docs on why this does not re-spell it C-style.
    #[test]
    fn a_function_is_spelled_the_way_show_pretty_spells_a_function_type() {
        let (state, uri) = state();
        let out = body(&state, &uri, Position::new(2, 17)).expect("`add` must resolve");
        assert!(out.starts_with("```kanagawa\n(uint32) -> void add\n```"), "{out}");
        assert!(out.contains("**function** `Adder::add`"), "{out}");
    }

    /// A `class` declaration's own type is `TType (TClass …)`, which
    /// `showPretty` renders `type Adder` — already naming the declaration, so
    /// the name is not appended a second time.
    #[test]
    fn a_type_declaration_is_not_named_twice() {
        let (state, uri) = state();
        let out = body(&state, &uri, Position::new(0, 8)).expect("`Adder` must resolve");
        assert_eq!(out, "```kanagawa\ntype Adder\n```\n\n**class** `Adder`");
    }

    /// The heart of the honesty requirement. `showPretty TUnresolved` is the
    /// word `unresolved` (`Type.hs:235`), which in a tooltip reads as a type by
    /// that name. Both halves are asserted, so this cannot pass by the hover
    /// simply disappearing: the declaration is still described, only the type
    /// line is withheld.
    ///
    /// The fixture is a templated function: `inferType` visits a
    /// `template <…>`'s condition only, so nothing below one is typed, and the
    /// reference does not type it either until `instantiateTemplates` (pass 8),
    /// which is unwired.
    #[test]
    fn an_uninferred_type_is_said_to_be_uninferred_not_printed_as_unresolved() {
        let state = GlobalState::new();
        let uri = test_uri("tmpl.k");
        state.did_open(
            uri.clone(),
            "template <typename T>\ninline void take(T v)\n{\n}\n".to_owned(),
            1,
        );

        let out = body(&state, &uri, Position::new(1, 13)).expect("`take` must resolve");
        assert!(!out.contains("unresolved"), "a compiler state was printed as a type: {out}");
        // The facts that *are* known are still reported; only the type is not.
        assert_eq!(out, "**template** `take`\n\nType not yet inferred.");
    }

    const TAKE: &str = "template <typename T>\ninline void take(T v)\n{\n    auto w = v + 1;\n}\n\n\
                        inline void main()\n{\n    take<uint8>(1);\n    take<uint16>(2);\n}\n";

    /// Inside a template's body the type is each instance's: `w` is `v +
    /// 1`, `uint9` in `take<uint8>` and `uint17` in `take<uint16>` (`tAdd`
    /// widens, `Type.hs:854`; `kg-oracle --no-base --passes 19` on this
    /// file, 2026-10-10, prints `uint9 w` in `take<|$uint@8|>` and
    /// `uint17 w` in `take<|$uint@16|>`).
    /// One line per type, the instance as its comment, and the list said
    /// to be a list. Red with the hover reading the CST engine (`Type not
    /// yet inferred`) or with the copies' answer dropped.
    #[test]
    fn inside_a_template_body_each_instance_s_type_is_listed() {
        let state = GlobalState::new();
        let uri = test_uri("take.k");
        state.did_open(uri.clone(), TAKE.to_owned(), 1);
        let out = body(&state, &uri, Position::new(3, 9)).expect("`w` must resolve");
        assert_eq!(
            out,
            "```kanagawa\nuint9 w   // take<uint8>\nuint17 w  // take<uint16>\n```\n\n\
             **local** `take::$::w`\n\n\
             Typed in 2 instances: a template's body has no type of its own."
        );
        // The template itself: each instance's function type.
        let out = body(&state, &uri, Position::new(1, 13)).expect("`take` must resolve");
        assert!(
            out.starts_with("```kanagawa\n(uint8) -> void take   // take<uint8>\n(uint16) -> void take  // take<uint16>\n```"),
            "{out}"
        );
    }

    /// A copy is named by the instance the loop recorded as it made it
    /// (`kg_sem::Elaborated::copies`), not read back off the final tree: a
    /// function template's instance passed as a value — `unit::test<1>
    /// (TestTranspose<false, false>)`, `test/library/data/fifo/transpose.k`
    /// — leaves a node of its copy reachable from outside the copy, which
    /// the walk that named copies met first and took for the declaration,
    /// and the hover's instances read `in , `. Red with the names read back
    /// by the first node of each expansion a walk from the root meets.
    #[test]
    fn a_function_instance_passed_as_a_value_is_named() {
        let state = GlobalState::new();
        let uri = test_uri("value.k");
        let text = "template <auto N>\ninline void test((uint32) -> void fn)\n{\n    fn(N);\n}\n\n\
                    template <bool B>\ninline void each(uint32 tag)\n{\n    auto w = B;\n}\n\n\
                    inline void main()\n{\n    test<1>(each<false>);\n    test<2>(each<true>);\n}\n";
        state.did_open(uri.clone(), text.to_owned(), 1);
        let out = body(&state, &uri, Position::new(9, 9)).expect("`w` must resolve");
        assert!(out.starts_with("```kanagawa\nbool w  // each<false>, each<true>\n```"), "{out}");
    }

    /// An import's module path hovers as the module and the file the
    /// program loaded for it. Red with the handler asking `definitions_at`
    /// alone, which declines on an import.
    #[test]
    fn an_import_hovers_as_its_module_and_file() {
        let state = GlobalState::new();
        let library = test_uri("a.k");
        let main = test_uri("main.k");
        state.did_open(library.clone(), "module a\n    { module a\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n".to_owned(), 1);
        state.did_open(main.clone(), "import a\n\ninline void main()\n{\n    Aa v;\n}\n".to_owned(), 1);
        let out = body(&state, &main, Position::new(0, 7)).expect("the import must hover");
        assert_eq!(
            out,
            format!("```kanagawa\nimport a\n```\n\n**module** `a`, loaded from [`/kanagawa-lsp-test/a.k`]({})", library.as_str())
        );
    }

    /// In a plain function the type is the loop's too, which types what an
    /// instantiation produces: `a.first()` calls `Box<4>`'s member, whose
    /// `auto` return the CST engine never sees (it is in a template's
    /// body); `kg-oracle --no-base --passes 19` prints `uint33 r =
    /// a.first();` (2026-10-10). Red with the hover reading the CST engine.
    #[test]
    fn a_plain_function_reads_the_type_the_loop_gave_it() {
        let state = GlobalState::new();
        let uri = test_uri("box.k");
        let text = "template <auto N>\nclass Box\n{\n    uint32[N] items;\n\n    inline auto first()\n    {\n        return items[0] + N;\n    }\n}\n\n\
                    inline void main()\n{\n    static Box<4> a;\n    auto r = a.first();\n}\n";
        state.did_open(uri.clone(), text.to_owned(), 1);
        let out = body(&state, &uri, Position::new(14, 9)).expect("`r` must resolve");
        assert_eq!(out, "```kanagawa\nuint33 r\n```\n\n**local** `main::$::r`");
    }

    /// A buffer that does not parse reads the last loop computed for it,
    /// mapped (`Db::editor_loop`); one opened broken has none, and its
    /// first hover runs the buffer's own, once (`Db::editor_loop_or_own`) —
    /// every later keystroke's hover reads that one, mapped, and runs no
    /// other: `w`, inside `take`'s body, is each instance's type both
    /// times, where the CST engine had none. Red with the hover reading
    /// `Db::editor_loop` alone (no loop, "not yet inferred"), or asking
    /// `Db::looped` directly (a second loop for the second edit).
    #[test]
    fn a_buffer_opened_broken_runs_its_own_loop_once() {
        let state = GlobalState::new();
        let uri = test_uri("broken.k");
        state.did_open(uri.clone(), format!("{TAKE}\ninline void broken() {{ auto = ; }}\n"), 1);
        let file = state.snapshot(&uri).expect("open").file;
        let loops = || state.with_db(file, |db| db.stats().misses(kg_db::stats::Query::Looped)).expect("open");
        let before = loops();
        let out = body(&state, &uri, Position::new(3, 9)).expect("`w` must resolve");
        assert!(out.starts_with("```kanagawa\nuint9 w   // take<uint8>\nuint17 w  // take<uint16>\n```"), "{out}");
        assert_eq!(loops(), before + 1, "the buffer's own loop, once");
        state.did_change_full(&uri, format!("{TAKE}\ninline void broken() {{ auto = ; ; }}\n"), 2);
        let again = body(&state, &uri, Position::new(3, 9)).expect("`w` must resolve");
        assert_eq!(again, out);
        assert_eq!(loops(), before + 1, "a later state reads that loop, mapped");
    }

    /// A local's qualified name carries `$`, the function-body scope component
    /// (`Desugar.hs:173-174`). Printed as-is: it is the key the symbol map
    /// holds, and tidying it away would print a name that identifies nothing.
    #[test]
    fn a_local_reports_the_synthetic_scope_its_key_actually_carries() {
        let state = GlobalState::new();
        let uri = test_uri("local.k");
        state.did_open(
            uri.clone(),
            "inline void main()\n{\n    const uint8 limit = 3;\n    uint8 v = limit;\n}\n"
                .to_owned(),
            1,
        );

        let out = body(&state, &uri, Position::new(3, 15)).expect("`limit` must resolve");
        assert!(out.contains("`main::$::limit`"), "{out}");
        // `const` is part of the *type* (`TConst`, `Type.hs:257`), not a kind.
        assert!(out.starts_with("```kanagawa\nconst uint8 limit\n```"), "{out}");
        assert!(out.contains("**local**"), "{out}");
    }

    /// The hover's `range` is the identifier the caret is on, so the client
    /// underlines the word it was told about — and it is read in the negotiated
    /// encoding, not in bytes.
    #[test]
    fn the_highlighted_range_is_the_identifier_under_the_caret() {
        let (state, uri) = state();
        let out = hover(&state, &params(&uri, Position::new(3, 18))).unwrap().unwrap();
        assert_eq!(out.range, Some(Range::new(Position::new(3, 16), Position::new(3, 21))));
    }

    /// A caret on punctuation has nothing to describe, and offering the nearest
    /// identifier would be a guess about what was pointed at.
    #[test]
    fn a_caret_that_is_not_on_an_identifier_answers_nothing() {
        let (state, uri) = state();
        // The `+` of `total + x`.
        assert_eq!(&SRC.lines().nth(3).unwrap()[22..23], "+");
        assert_eq!(body(&state, &uri, Position::new(3, 22)), None);
        // Past the end of the buffer, which a client may legitimately send
        // after an edit the server has not seen yet.
        assert_eq!(body(&state, &uri, Position::new(999, 0)), None);
    }

    /// An intrinsic is typed from a table (`Type.hs:1074-1082`) and has no
    /// declaration, so there is nothing to describe. Declining is the answer.
    #[test]
    fn an_intrinsic_has_no_declaration_to_describe() {
        let state = GlobalState::new();
        let uri = test_uri("intrinsic.k");
        state.did_open(
            uri.clone(),
            "inline void main()\n{\n    __print(\"hi\");\n}\n".to_owned(),
            1,
        );
        assert_eq!(body(&state, &uri, Position::new(2, 6)), None);
    }

    /// An edit must not be answered from the previous text's type table.
    ///
    /// [`kg_db::Db::type_table`] is the first memo in that database keyed on an
    /// item tree's identity, and a memo whose key silently stops matching its
    /// query is a staleness bug that shows up as a tooltip nobody double-checks.
    /// So the invalidation path — `didChange` → `Db::invalidate` → a new
    /// `Arc<ItemTree>` → a rebuilt table — is pinned rather than trusted.
    #[test]
    fn an_edit_invalidates_the_type_a_hover_reports() {
        let state = GlobalState::new();
        let uri = test_uri("edited.k");
        state.did_open(uri.clone(), "inline void main()\n{\n    uint8 v = 1;\n}\n".to_owned(), 1);
        let before = body(&state, &uri, Position::new(2, 10)).expect("`v` must resolve");
        assert!(before.starts_with("```kanagawa\nuint8 v\n```"), "{before}");

        state.did_change_full(&uri, "inline void main()\n{\n    uint32 v = 1;\n}\n".to_owned(), 2);
        let after = body(&state, &uri, Position::new(2, 11)).expect("`v` must still resolve");
        assert!(after.starts_with("```kanagawa\nuint32 v\n```"), "{after}");
    }

    /// A document the server never saw is `NoFile`, not an empty answer. The
    /// distinction is invisible on the wire and load-bearing in the handler: an
    /// unopened URI must not cause a program to be loaded for it.
    #[test]
    fn an_unknown_document_is_declined_not_faked() {
        let state = GlobalState::new();
        let uri = test_uri("never-opened.k");
        let err = hover(&state, &params(&uri, Position::new(0, 0))).unwrap_err();
        assert_eq!(err, NoFile::NotOpen);
    }

    /// A declaration in another module is described from *that* module's type
    /// table. The per-file inference environment would call the name
    /// `TUndefined` when asked from the importing file, so reading the type at
    /// the cursor rather than at the declaration would lose every imported
    /// symbol — which is most of what a user hovers.
    ///
    /// **The two lines used to disagree**, and this assertion was written to
    /// record it: the symbol-map key carried the module namespace while
    /// the CST engine named a nominal type from a scope that started empty, so
    /// the key read `a::Aa` and the type read `Aa`. They agree now, and the
    /// reason is the one the old note asked for — inference is given the
    /// qualifier `moduleDecl` pushes (`scoped ident declarations`,
    /// `Parser.hs:185-189`, mangled by `moduleNamespace`,
    /// `Parser/Syntax.hs:273-274`), so a nominal type's `QualifiedName`
    /// (`TStruct`, `Type.hs:269`) is the declaration's own `getQualifiedName`
    /// on both sides. Two names for one declaration was the bug; one is the fix.
    #[test]
    fn a_declaration_in_another_file_is_typed_in_its_own_file() {
        let state = GlobalState::new();
        let library = test_uri("a.k");
        let main = test_uri("main.k");
        state.did_open(
            library.clone(),
            "module a\n    { module a\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n".to_owned(),
            1,
        );
        state.did_open(
            main.clone(),
            "import a\n\ninline void main()\n{\n    Aa v;\n}\n".to_owned(),
            1,
        );

        // The caret is on `Aa` in `main.k`; `struct Aa` is declared in `a.k`.
        let out = body(&state, &main, Position::new(4, 5)).expect("the import must resolve");
        assert_eq!(out, "```kanagawa\ntype a::Aa\n```\n\n**struct** `a::Aa`");
    }
}
