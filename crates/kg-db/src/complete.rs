//! `textDocument/completion` — what a name at the caret could be.
//!
//! Two contexts, decided by the token to the left of the caret:
//!
//! * **After a `.`**, or inside the selector of a `FIELD_EXPR`: the members of
//!   the base expression's type — the same route [`Db::definition_at`] takes for
//!   a `.field`, because `a.b` names `b` *inside the type of `a`*
//!   (`tLookupField`, `Type.hs:961-971`) and never a symbol. A struct offers
//!   its fields, a class its fields and methods.
//! * **Anywhere else**: the names a bare identifier here could resolve to, in
//!   the order `findSymbol` would try them (`SymbolTable.hs:242-270`): the
//!   enclosing scopes innermost first ([`crate::navigate::introduces_scope`]),
//!   then this file's top-level declarations, then every name the file's
//!   imports expose. The first spelling wins, so a local shadows a global in the
//!   list exactly as it does in resolution.
//!
//! What is *not* offered, on purpose: keywords and intrinsics (an editor's
//! snippet list is the better home, and offering `bitsizeof` as a symbol would
//! misdescribe it), names a `static if` branch declares that elaboration would
//! delete, and anything from a module the file does not import — the reference
//! resolves nothing it does not import, and completion that offers more than
//! resolution accepts teaches the wrong program.
//!
//! The parser's recovery is what makes the member context work on half-typed
//! text: `s.` followed by a newline parses to a `FIELD_EXPR` holding the base
//! and the `DOT` and no selector, so the base is there to ask about.
//!
//! # Which engine reads the base's type
//!
//! The editor's list ([`Db::completions_with`]) reads the program-wide loop's
//! type of the base where the loop has one — the newest loop computed for
//! the program, mapped onto the text ([`Db::editor_loop_ready`]), so a buffer
//! that does not parse, which is most of them mid-keystroke, reads the last
//! one that ran; a base typed since, `a.` mid-statement, is typed by its
//! name's declaration there ([`Db::base_answer`]). The loop knows what the
//! CST inference engine, the editor's other reader until sprint W3.7, could
//! not: the instance a template's name denotes (`static Box<4> a;` — `a.` lists
//! `Box<4>`'s copy, `items` a `uint32[4]`), and what an instantiated call
//! returns (`auto p = make<uint8>(1);` — `p.` lists the struct's fields).
//! Inside a template's body the base has a type per instance, and each
//! member is listed with each instance's type
//! ([`crate::looped::IrAnswer::Copies`]).
//! [`Db::completions_at`] is the list with no loop to read: the scopes'
//! names, and no member list.

use kg_base::{FileId, FxHashSet, SymbolPath, TextSize};
use kg_resolve::{DefId, DefKind, ItemId, ModuleSource};
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, SyntaxToken, ast};
use kg_ty::Ty;

use crate::Db;
use crate::looped::{EditorLoop, IrAnswer};
use crate::navigate::{Definition, binder, introduces_scope};

/// Each distinct type and the instances that give it, as
/// [`IrAnswer::Copies`] holds them.
type ByType = Vec<(Ty, Vec<String>)>;

/// One candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub name: String,
    pub kind: DefKind,
    /// The declaration, for a caller that wants its type or location.
    pub def: DefId,
    /// Where the candidate came from, for ordering: `0` the enclosing scopes,
    /// `1` this file's top level, `2` an import, `3` a member of a type.
    pub rank: u8,
    /// The loop's type for a member it listed off the base's type: the
    /// instance's own member — a template's member has a type per
    /// instance, so its declaration alone cannot say which — or, for a
    /// base inside a template's body, each instance's. `None` for every
    /// other candidate, whose type is its declaration's
    /// ([`Db::ir_answer_of_completion`]).
    pub ty: Option<IrAnswer>,
}

impl Db {
    /// The candidates at `offset` in `file` with no loop to read, deduplicated
    /// by spelling with the nearest binding kept, in the order a client should
    /// show them: the scopes' names; a member list, read off a type, is empty.
    pub fn completions_at(&self, roots: &[FileId], file: FileId, offset: TextSize) -> Vec<Completion> {
        self.completions_with(roots, file, offset, None)
    }

    /// The candidates the editor offers at `offset`: the scopes' names, or
    /// after a `.` the members of `looped`'s type of the base
    /// ([`Db::loop_members_at`]) — none where it has no type for it. The
    /// editor's `looped` is [`Db::editor_loop_ready`]'s. The CST engine's
    /// type of the base answered there until sprint W3.7.
    pub fn completions_with(
        &self,
        roots: &[FileId],
        file: FileId,
        offset: TextSize,
        looped: Option<&EditorLoop>,
    ) -> Vec<Completion> {
        let module = ModuleSource::File(file);
        let root = self.parse(file).syntax_node();
        let Some(anchor) = token_left_of(&root, offset) else { return Vec::new() };

        if let Some(base) = member_base(&anchor) {
            return looped.and_then(|l| self.loop_members(l, module, &base)).unwrap_or_default();
        }
        self.names_in_scope(roots, module, &anchor)
    }

    /// The member list `looped` gives at `offset`: `None` where the caret
    /// is in no member access, or the loop has no type for its base, which
    /// the resilience gate counts apart from a type with no members.
    pub fn loop_members_at(&self, file: FileId, offset: TextSize, looped: &EditorLoop) -> Option<Vec<Completion>> {
        let root = self.parse(file).syntax_node();
        let base = token_left_of(&root, offset).and_then(|anchor| member_base(&anchor))?;
        self.loop_members(looped, ModuleSource::File(file), &base)
    }

    /// A candidate's type as the loop gives it, to a reader that prints a
    /// type where `shown` says it can: a listed member's own
    /// ([`Completion::ty`]), or what the loop says at the candidate's
    /// declaration ([`Db::ir_answer_of_definition`], the query hover
    /// reads). `None` where the loop has nothing to show.
    pub fn ir_answer_of_completion(
        &self,
        roots: &[FileId],
        looped: &EditorLoop,
        c: &Completion,
        shown: &dyn Fn(&Ty) -> bool,
    ) -> Option<IrAnswer> {
        match &c.ty {
            Some(answer) => answer.clone().shown(shown),
            None => self.ir_answer_of_definition(looped, &self.completion_definition(roots, c)?, shown),
        }
    }

    /// The members `looped` reaches on `base`'s type, each a candidate typed
    /// as that instance has it; `None` where the loop has no type for the
    /// base, or names an aggregate it does not declare. A type it decided
    /// that is no aggregate has no members: an empty list. (Until sprint
    /// W3.7 a `None` here asked the CST engine, and an empty list kept it
    /// from a second opinion the loop had already overruled.)
    fn loop_members(&self, looped: &EditorLoop, module: ModuleSource, base: &SyntaxNode) -> Option<Vec<Completion>> {
        let interner = self.interner();
        let answer = self.base_answer(looped, module, base)?;
        match answer {
            IrAnswer::Written(ty) => {
                let Some(members) = looped.members(&ty, interner) else {
                    return (!is_aggregate(&ty)).then(Vec::new);
                };
                Some(
                    members
                        .iter()
                        .filter_map(|m| {
                            let mut c = self.member_completion(m)?;
                            c.ty = Some(IrAnswer::Written(m.ty.clone()));
                            Some(c)
                        })
                        .collect(),
                )
            }
            IrAnswer::Copies { types, untyped, budget_exhausted } => {
                // Each instance's aggregate — usually one template's copies,
                // which list the same members — merged by name, in the order
                // the members are first met; each member's type grouped as
                // `IrAnswer::Copies` groups a declaration's.
                let mut out: Vec<(Completion, ByType)> = Vec::new();
                for (ty, instances) in &types {
                    for m in looped.members(ty, interner).unwrap_or_default() {
                        let at = match out.iter().position(|(c, _)| c.name == *interner.resolve(m.name)) {
                            Some(at) => at,
                            None => {
                                let Some(c) = self.member_completion(&m) else { continue };
                                out.push((c, Vec::new()));
                                out.len() - 1
                            }
                        };
                        let grouped = &mut out[at].1;
                        match grouped.iter_mut().find(|(t, _)| *t == m.ty) {
                            Some((_, given)) => given.extend(instances.iter().cloned()),
                            None => grouped.push((m.ty.clone(), instances.clone())),
                        }
                    }
                }
                Some(
                    out.into_iter()
                        .map(|(mut c, types)| {
                            c.ty = Some(IrAnswer::Copies { types, untyped: untyped.clone(), budget_exhausted });
                            c
                        })
                        .collect(),
                )
            }
        }
    }

    /// One member the loop listed, as a candidate: offered by the
    /// declaration it was written as (`Db::definition_of_declaration`), so
    /// its name, its kind, its jump and its fallback type are the text's —
    /// and only where that declaration is one [`Db::members_of`] offers.
    fn member_completion(&self, m: &crate::looped::Member) -> Option<Completion> {
        let def = self.definition_of_declaration(m.module, m.range, m.kind)?;
        if !matches!(def.kind, DefKind::Variable(_) | DefKind::Function | DefKind::FunctionDecl) {
            return None;
        }
        Some(Completion {
            name: self.interner().resolve(def.qualified_name.last()?).to_string(),
            kind: def.kind,
            def: def.def,
            rank: 3,
            ty: None,
        })
    }

    /// Whether a completion at `offset` is a member list — the one context
    /// whose candidates are read off a type (the base's), not off the
    /// scopes. The resilience gate (`xtask resilience`) asks it to know
    /// which lists the IR engine could not yet have produced.
    pub fn is_member_completion(&self, file: FileId, offset: TextSize) -> bool {
        let root = self.parse(file).syntax_node();
        token_left_of(&root, offset).and_then(|anchor| member_base(&anchor)).is_some()
    }

    /// Everything a bare name written at `anchor` could resolve to.
    fn names_in_scope(&self, roots: &[FileId], module: ModuleSource, anchor: &SyntaxToken) -> Vec<Completion> {
        let tree = self.item_tree(module);
        let interner = self.interner();
        let mut seen: FxHashSet<String> = FxHashSet::default();
        let mut out = Vec::new();
        let mut offer = |name: String, kind: DefKind, def: DefId, rank: u8| {
            if seen.insert(name.clone()) {
                out.push(Completion { name, kind, def, rank, ty: None });
            }
        };

        // 1. `findInScopes` — the enclosing scopes, innermost first. A
        //    declaration belongs to the scope `binder` names, which is what
        //    keeps a sibling block's locals out.
        let declared: Vec<(ItemId, SyntaxNode)> = tree
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| !matches!(item.kind, DefKind::Error | DefKind::ExportType))
            .filter(|(_, item)| !item.is_field())
            .filter_map(|(i, item)| {
                let range = item.range.range;
                let token = anchor
                    .parent()?
                    .ancestors()
                    .last()?
                    .token_at_offset(range.start())
                    .find(|t| t.text_range() == range)?;
                let declaration = token.parent()?.parent()?;
                Some((ItemId(i as u32), declaration))
            })
            .collect();
        for scope in anchor.parent().into_iter().flat_map(|p| p.ancestors()).filter(|s| introduces_scope(s.kind())) {
            for (id, declaration) in &declared {
                if binder(declaration).as_ref() != Some(&scope) {
                    continue;
                }
                let item = tree.get(*id);
                if let Some(name) = item.qualified_name.last() {
                    let rank = if scope.kind() == SyntaxKind::SOURCE_FILE { 1 } else { 0 };
                    offer(interner.resolve(name).to_string(), item.kind, DefId::new(module, *id), rank);
                }
            }
        }

        // 2. `findInImports` — every name an import exposes, under the
        //    namespace it is filed in. An aliased import hides its names from
        //    unqualified use (`SymbolTable.hs:276-278`), so it offers nothing
        //    bare.
        let program = self.program(roots);
        for import in program.graph.imports(module) {
            if import.alias.is_some() {
                continue;
            }
            let Some(target) = program.graph.import_target(module, import.id) else { continue };
            let Some(interface) = program.graph.interface(target) else { continue };
            let mut names: Vec<(SymbolPath, DefId)> = interface
                .visible
                .values()
                .flat_map(|syms| syms.iter().map(|(k, d)| (k.clone(), *d)))
                .collect();
            // By spelling, not by `SymbolPath`'s own order: that compares
            // interned ids, which number names in the order this database
            // first met them — so the list's order (the client's
            // `sort_text`) depended on what else the session had opened.
            // Found by `xtask resilience`, whose `--jobs 1` and default
            // readings parted on it (2026-09-27).
            names.sort_by_cached_key(|(key, def)| {
                (key.segments().iter().map(|&s| interner.resolve(s)).collect::<Vec<_>>(), *def)
            });
            for (key, def) in names {
                // `[@ns, X]` is the exposed spelling; anything deeper is a
                // qualified member a bare name cannot reach.
                if key.segments().len() != 2 {
                    continue;
                }
                let Some(item) = program.items().item(def) else { continue };
                if let Some(name) = key.last() {
                    offer(interner.resolve(name).to_string(), item.kind, def, 2);
                }
            }
        }
        out
    }

    /// The [`Definition`] a completion names, for its type and location.
    pub fn completion_definition(&self, roots: &[FileId], c: &Completion) -> Option<Definition> {
        let program = self.program(roots);
        program.items().item(c.def).map(|item| Definition::of(c.def, item))
    }
}

/// Whether the loop has decided `ty` — the types a tooltip prints
/// (`kg-lsp`'s `hover::shown`): `tResolved` (`Type.hs:1001-1015`) less
/// `TError`, `Untyped` and an undeduced `Auto`. Anything else at a member
/// access's base is no answer, and no list is offered.
pub(crate) fn decided(ty: &Ty) -> bool {
    !matches!(ty, Ty::Untyped | Ty::Auto) && ty.resolved() && !ty.is_error()
}

/// Whether `ty` names an aggregate a `.` reaches into (`tLookupField`,
/// `Type.hs:961-971`; `this`, `Type.hs:1232-1235`).
pub(crate) fn is_aggregate(ty: &Ty) -> bool {
    match ty {
        Ty::Const(inner) | Ty::Instance { concrete: inner, .. } => is_aggregate(inner),
        Ty::Struct(..) | Ty::Union(..) | Ty::Class(..) | Ty::Reference(_) => true,
        _ => false,
    }
}

/// The non-trivia token immediately left of the caret, or the identifier the
/// caret is inside.
fn token_left_of(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    if offset > root.text_range().end() {
        return None;
    }
    let at = root.token_at_offset(offset);
    let candidates: Vec<SyntaxToken> = at.clone().collect();
    // Inside or at the end of an identifier: that identifier.
    if let Some(t) = candidates.iter().find(|t| t.kind() == SyntaxKind::IDENT) {
        return Some(t.clone());
    }
    // Otherwise the token ending at the caret, skipping trivia leftwards.
    let mut tok = at.left_biased();
    while let Some(t) = tok {
        if !t.kind().is_trivia() {
            return Some(t);
        }
        tok = t.prev_token();
    }
    None
}

/// The base expression when `anchor` is the `.` of a member access or the
/// selector after one.
fn member_base(anchor: &SyntaxToken) -> Option<SyntaxNode> {
    let field_expr = match anchor.kind() {
        SyntaxKind::DOT => anchor.parent()?,
        SyntaxKind::IDENT => {
            let name_ref = anchor.parent().filter(|p| p.kind() == SyntaxKind::NAME_REF)?;
            let up = match name_ref.parent() {
                Some(p) if p.kind() == SyntaxKind::PATH_SEGMENT => p,
                _ => name_ref,
            };
            up.parent()?
        }
        _ => return None,
    };
    if field_expr.kind() != SyntaxKind::FIELD_EXPR {
        return None;
    }
    let field = ast::FieldExpr::cast(field_expr)?;
    // `a.b.c` with the caret on `b`: the selector is `b` and the base `a`, but
    // the caret's `FIELD_EXPR` is the *inner* one only when `b` is its last
    // name child; a caret on `a` is not a member context at all.
    if anchor.kind() == SyntaxKind::IDENT {
        let last = field
            .syntax()
            .children()
            .filter(|c| matches!(c.kind(), SyntaxKind::NAME_REF | SyntaxKind::PATH_SEGMENT))
            .last()?;
        let mine = anchor.parent()?;
        let mine = match mine.parent() {
            Some(p) if p.kind() == SyntaxKind::PATH_SEGMENT => p,
            _ => mine,
        };
        if last != mine {
            return None;
        }
    }
    Some(field.base()?.syntax().clone())
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, TextSize, Utf8PathBuf};
    use kg_config::Settings;
    use kg_resolve::{DeclareFlag, DefKind};

    use crate::Db;

    fn db() -> Db {
        let mut settings = Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        Db::new(settings)
    }

    fn write(db: &mut Db, name: &str, text: &str) -> FileId {
        db.set_file_text(&Utf8PathBuf::from(format!("/proj/{name}")), text.to_owned())
    }

    /// `$0` marks the caret; the names the editor offers there, in order: its
    /// diagnostics run first — the loop, where the buffer parses — and the
    /// list reads the newest loop (`Db::editor_loop_ready`), as
    /// `kg-lsp`'s completion does. A member list on a buffer that does
    /// not parse is the last parsing state's: [`names_after`].
    fn names(files: &[(&str, &str)]) -> Vec<String> {
        let mut db = db();
        let mut main = None;
        let mut caret = None;
        for (name, text) in files {
            let (clean, at) = match text.find("$0") {
                Some(i) => (format!("{}{}", &text[..i], &text[i + 2..]), Some(TextSize::new(i as u32))),
                None => ((*text).to_owned(), None),
            };
            let id = write(&mut db, name, &clean);
            if let Some(at) = at {
                main = Some(id);
                caret = Some(at);
            }
        }
        let (file, offset) = (main.expect("a `$0`"), caret.expect("a `$0`"));
        db.diagnostics(&[file], file);
        let looped = db.editor_loop_ready(&[file]);
        db.completions_with(&[file], file, offset, looped.as_ref()).into_iter().map(|c| c.name).collect()
    }

    /// [`names`] for `main.k`, as an editor gets there: `before` — which
    /// parses — loaded and its loop run, then edited into `after`, `$0` the
    /// caret. Until sprint W3.7 the member tests loaded `after` alone, which
    /// the CST engine typed with no loop.
    fn names_after(before: &str, after: &str) -> Vec<String> {
        let mut db = db();
        let file = write(&mut db, "main.k", before);
        db.looped(&[file]);
        let at = after.find("$0").expect("a `$0`");
        let file = write(&mut db, "main.k", &format!("{}{}", &after[..at], &after[at + 2..]));
        let looped = db.editor_loop_ready(&[file]);
        db.completions_with(&[file], file, TextSize::new(at as u32), looped.as_ref()).into_iter().map(|c| c.name).collect()
    }

    #[test]
    fn a_dot_offers_the_fields_of_the_base_type() {
        let after = "struct S\n{\n    uint32 x;\n    bool y;\n}\n\ninline void main()\n{\n    S s = {};\n    s.$0\n}\n";
        let out = names_after(&after.replace("    s.$0\n", "    s;\n"), after);
        assert_eq!(out, ["x", "y"]);
    }

    #[test]
    fn a_partial_selector_still_offers_every_member() {
        let out = names(&[(
            "main.k",
            "struct S\n{\n    uint32 xa;\n    uint32 xb;\n}\n\ninline void main()\n{\n    S s = {};\n    uint32 v = s.x$0;\n}\n",
        )]);
        assert_eq!(out, ["xa", "xb"]);
    }

    #[test]
    fn a_class_offers_fields_and_methods() {
        let mut db = db();
        let text = "class C\n{\npublic:\n    uint32 n;\n    inline void Go() {}\n}\n\ninline void main()\n{\n    C c;\n    c.\n}\n";
        let file = write(&mut db, "main.k", &text.replace("    c.\n", "    c;\n"));
        db.looped(&[file]);
        let file = write(&mut db, "main.k", text);
        let at = TextSize::new(text.find("c.\n").unwrap() as u32 + 2);
        let looped = db.editor_loop_ready(&[file]);
        let out = db.completions_with(&[file], file, at, looped.as_ref());
        let kinds: Vec<(String, DefKind)> = out.into_iter().map(|c| (c.name, c.kind)).collect();
        assert_eq!(
            kinds,
            [("n".to_owned(), DefKind::Variable(DeclareFlag::DeclareClass)), ("Go".to_owned(), DefKind::Function)]
        );
    }

    /// Innermost first, a sibling block's locals absent, and the file's own
    /// declarations after the scopes — the order `findSymbol` tries.
    #[test]
    fn bare_names_come_from_the_enclosing_scopes_then_the_file() {
        let out = names(&[(
            "main.k",
            "uint32 g;\n\ninline void other()\n{\n    uint32 hidden;\n}\n\ninline void main(uint32 p)\n{\n    uint32 a;\n    {\n        bool b;\n        $0\n    }\n}\n",
        )]);
        assert_eq!(out, ["b", "a", "p", "g", "other", "main"]);
    }

    /// A field is never a bare name (`Symbols.hs:48-49`).
    #[test]
    fn a_struct_field_is_not_offered_as_a_bare_name() {
        let out = names(&[("main.k", "struct S\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    $0\n}\n")]);
        assert_eq!(out, ["S", "main"]);
    }

    /// `findInImports`: what an import exposes is offered; what it does not, is
    /// not; and an aliased import exposes nothing bare (`SymbolTable.hs:276-278`).
    #[test]
    fn imports_offer_their_exposed_names() {
        let out = names(&[
            ("a.k", "module a\n    { Aa\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n\nstruct Hidden\n{\n    uint32 y;\n}\n"),
            ("main.k", "import a\n\ninline void main()\n{\n    $0\n}\n"),
        ]);
        assert_eq!(out, ["main", "Aa"]);
        let out = names(&[
            ("a.k", "module a\n    { Aa\n    }\n\nstruct Aa\n{\n    uint32 x;\n}\n"),
            ("main.k", "import a as m\n\ninline void main()\n{\n    $0\n}\n"),
        ]);
        assert_eq!(out, ["main"]);
    }

    /// Imported names come in spelling order whatever the database met
    /// first. Red with the sort by `SymbolPath` — interned ids — back: a
    /// session that had interned `zeta` before `alpha` listed it first.
    #[test]
    fn imported_names_are_ordered_by_spelling_not_by_session_history() {
        let files = [
            ("a.k", "module a\n    { alpha\n    , zeta\n    }\n\nstruct zeta\n{\n    uint32 x;\n}\n\nstruct alpha\n{\n    uint32 y;\n}\n"),
            ("main.k", "import a\n\ninline void main()\n{\n    \n}\n"),
        ];
        let listed = |warm: &[&str]| {
            let mut db = db();
            for name in warm {
                db.interner().intern(name);
            }
            let mut main = None;
            for (name, text) in files {
                main = Some(write(&mut db, name, text));
            }
            let file = main.expect("main.k");
            let at = TextSize::new(files[1].1.find("\n    \n").expect("the caret line") as u32 + 5);
            db.completions_at(&[file], file, at).into_iter().map(|c| c.name).collect::<Vec<_>>()
        };
        let fresh = listed(&[]);
        assert_eq!(fresh, ["main", "alpha", "zeta"]);
        assert_eq!(listed(&["zeta", "alpha"]), fresh);
        assert_eq!(listed(&["alpha", "zeta"]), fresh);
    }

    /// The loop's member list is the text's members, each once and nothing
    /// an instantiation made: `twice` takes an
    /// `auto`, so `abbreviatedFunctionTemplate` makes it a template
    /// (`Function.hs:288-311`) whose instance `twice<uint8>` is a copy in
    /// the class's body, ahead of the template; `get` is written as a
    /// template, which neither list offers. Red with the copies' expansion
    /// not checked (`twice` twice, the instance first), with the
    /// template's function not read through (no `twice`), or with the
    /// item tree's kinds not consulted (`get` listed).
    #[test]
    fn a_loop_member_list_offers_what_the_text_declares() {
        let text = "class C\n{\npublic:\n    uint32 n;\n\n\
                    inline uint32 twice(auto x)\n    {\n        return x + x;\n    }\n\n\
                    template <typename T>\n    inline T get()\n    {\n        return cast<T>(n);\n    }\n\n\
                    inline void run()\n    {\n        auto d = twice(3u8);\n        auto e = get<uint8>();\n    }\n}\n\n\
                    inline void main()\n{\n    static C c;\n    c.run();\n}\n";
        let mut db = db();
        let file = write(&mut db, "main.k", text);
        let errors: Vec<String> =
            db.diagnostics(&[file], file).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect();
        assert!(errors.is_empty(), "{errors:?}");
        let looped = db.editor_loop(&[file], file).expect("the buffer parses");
        let at = TextSize::new(text.find("c.run").expect("in the fixture") as u32 + 2);
        let offered = db.loop_members_at(file, at, &looped).expect("the loop types `c`");
        let names: Vec<&str> = offered.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["n", "twice", "run"]);
        // What the editor serves is that list (the CST engine's, of the same
        // names, was compared here until sprint W3.7).
        let served: Vec<String> = db.completions_with(&[file], file, at, Some(&looped)).into_iter().map(|c| c.name).collect();
        assert_eq!(served, names);
    }

    const BOX: &str = "template <auto N>\nclass Box\n{\n    uint32[N] items;\n\npublic:\n    inline uint32 first()\n    {\n        return items[0];\n    }\n}\n\n\
                       inline void main()\n{\n    static Box<4> a;\n    auto r = a.items[0];\n}\n";

    /// The editor's member list at `at` in `text`, a buffer that does not
    /// parse, after `BOX` — whose loop the diagnostics ran — was edited
    /// into it: the members' names, each with the line its declaration is
    /// on in `text` and its type as the loop gives it.
    fn after_editing_box(text: &str, at: TextSize) -> Option<Vec<(String, u32, String)>> {
        let mut db = db();
        let file = write(&mut db, "main.k", BOX);
        assert!(db.diagnostics(&[file], file).iter().all(|d| !d.is_error()));
        db.editor_loop(&[file], file).expect("the buffer parses");
        let file = write(&mut db, "main.k", text);
        assert!(db.diagnostics(&[file], file).iter().any(|d| d.is_error()), "the edit must break the parse");
        let before = db.stats().misses(crate::stats::Query::Looped);
        let looped = db.editor_loop(&[file], file).expect("the last loop");
        let line = |at: TextSize| text[..usize::from(at)].matches('\n').count() as u32;
        let names = db.interner();
        let out = db.loop_members_at(file, at, &looped).map(|members| {
            members
                .iter()
                .map(|c| {
                    let def = db.completion_definition(&[file], c).expect("a declaration");
                    let ty = match db.ir_answer_of_completion(&[file], &looped, c, &super::decided) {
                        Some(crate::looped::IrAnswer::Written(t)) => kg_ty::subtype::show_pretty(&t, Some(names)),
                        other => format!("{other:?}"),
                    };
                    (c.name.clone(), line(def.range.range.start()), ty)
                })
                .collect()
        });
        assert_eq!(db.stats().misses(crate::stats::Query::Looped), before, "a loop ran for a broken buffer");
        out
    }

    /// `a.` typed fresh is text the loop never saw, so its base has no node
    /// there; it is a name, and the loop's type at the declaration it
    /// resolves to is its type (`typeOfSymbol`, `Type.hs:1243-1245,1336`):
    /// `Box<4>`, whose members the loop lists — where the CST engine, with
    /// no type for an instance's name, lists nothing. Red with the
    /// declaration route dropped from `Db::base_answer`.
    #[test]
    fn a_base_typed_since_the_loop_ran_is_typed_by_its_declaration() {
        let text = BOX.replace("    auto r = a.items[0];\n", "    auto r = a.items[0];\n    a.\n");
        let at = TextSize::new(text.find("    a.\n").expect("the edit") as u32 + 6);
        let offered = after_editing_box(&text, at).expect("the loop types `a` by its declaration");
        assert_eq!(offered, [("items".to_owned(), 3, "uint32[4]".to_owned()), ("first".to_owned(), 6, "() -> uint32".to_owned())]);
    }

    /// An edit above the aggregate moves every member: the loop knows each
    /// by its old range, and each is offered at its declaration in the
    /// text as it stands. Red with `EditorLoop::members` leaving the
    /// ranges in the loop's text (no member found at the old range).
    #[test]
    fn a_member_list_reads_through_an_edit_that_moved_it() {
        let text = format!("@@@\n\n{BOX}");
        let at = TextSize::new(text.find("a.items").expect("in the fixture") as u32 + 2);
        let offered = after_editing_box(&text, at).expect("the loop types `a`");
        assert_eq!(offered, [("items".to_owned(), 5, "uint32[4]".to_owned()), ("first".to_owned(), 8, "() -> uint32".to_owned())]);
    }

    /// An edit that reshaped a member — here `first`'s body no longer
    /// closes — leaves the loop a member it cannot place in the text, and
    /// a list missing it would say `Box` has fewer than it does: the loop
    /// declines the whole list, and none is offered. Red with
    /// `EditorLoop::members` dropping the members it cannot map.
    #[test]
    fn a_member_list_an_edit_reshaped_is_not_the_loop_s() {
        let text = BOX.replace("        return items[0];\n    }\n", "        return items[0];\n");
        let at = TextSize::new(text.find("a.items").expect("in the fixture") as u32 + 2);
        assert_eq!(after_editing_box(&text, at), None);
    }
}
