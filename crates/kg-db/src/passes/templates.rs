//! The program's template declarations as the CST passes still ask them —
//! the parser's own template checks (`early_normalization`'s pass 0, the
//! parse-time table's in [`super::undefined_symbols`]) and the CST reading of
//! `templateResolved` that `cargo xtask ir`'s differential grades the second
//! census against ([`super::template_type_check::resolved_instance_args`]).
//!
//! What pass 4 `deduceTemplateArgs` reported here — a conflicting deduction,
//! a callback's closure type, pass 12's LT verdict — is the loop's since
//! sprint W3 step 1 (`kg_sem::check::TextPass::TemplateTypeCheck`,
//! `TextPass::UnresolvedTemplates`), and its CST driver is gone; the index it
//! was built for stays, with the scope walk that keeps it honest.

use kg_base::{FileId, FxHashMap, Interner, Symbol};
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, ast};

/// `lookupSymbol templateInstance symbols` (`Type.hs:1435`, `Symbols.hs:73-87`),
/// restricted to what an unqualified call can name.
///
/// # Why the key is the simple name, and why only top-level declarations
///
/// `lookupInScope` tries `scope ++ name`, then drops the last scope component
/// and retries, down to the empty scope (`Symbols.hs:82-87`). Every key it tries
/// *before* module scope is `<some enclosing scope of the call> ++ [name]`; only
/// the last one or two are top-level. So a simple-name index over top-level
/// templates answers the tail of that walk — and it answers it for every module
/// in the program, which is what makes a template from a `helper/` module
/// reachable at all.
///
/// **The index is only an answer once the earlier steps of the walk are known to
/// miss**, and that is [`shadowed_before_module_scope`]'s job, not this type's.
/// Without it the index is applied at scopes it says nothing about: a member
/// template, a member function or a local binding of the same name is what
/// `lookupInScope` would have found first, and reaching past it to a top-level
/// template deduces against the wrong signature. That was an *invented*
/// diagnostic — a conflict reported on a program the reference deduces cleanly —
/// which is the one failure direction this port does not accept.
///
/// Restricting the index itself to top level is what keeps it honest.
/// `helper/template_argument_deduction.k` declares `Fn_T_T` twice: once at module
/// scope and once as a member of `class Class`. Their keys are `…::Fn_T_T` and
/// `…::Class::Fn_T_T`; indexing both by simple name would see a duplicate and
/// lose them both, and indexing the member under a bare `Fn_T_T` would make it
/// reachable from outside `Class`, which `lookupInScope` never does.
///
/// A name **two modules** both declare at top level is dropped rather than
/// resolved to whichever came first. This is a deliberate divergence toward
/// silence: `getSymbols''` is `fromList . reverse`, so the reference resolves
/// such a collision to the *first* declaration (`Symbols.hs:41,44`) rather than
/// dropping it — it can afford to, because its key carries the scope and the
/// collision is therefore between two genuinely identical qualified names. Here
/// the collision is an artefact of erasing the scope, so the two candidates are
/// not interchangeable and picking one would be a guess.
///
/// # The member index
pub(crate) struct Templates {
    by_name: FxHashMap<Symbol, Option<(FileId, ast::TemplateDecl)>>,
    /// Per file, the module-level names its **unaliased** imports make
    /// visible, each with the file declaring it — the parse-time table's
    /// import half (`findInImport`, `SymbolTable.hs:276-278`; an aliased
    /// import hides its names from unqualified use). Empty unless
    /// [`Templates::with_visible`] filled it, in which case a parse-time
    /// verdict may name a template another module declares.
    visible: FxHashMap<FileId, FxHashMap<Symbol, FileId>>,
}

impl Templates {
    /// Over every module of the program — `withSymbols` is `getSymbols`, which
    /// folds the whole file list (`Frontend.hs:40,71`, `Symbols.hs:37-41`).
    pub(crate) fn of<'a>(
        roots: impl IntoIterator<Item = (FileId, &'a ast::SourceFile)>,
        interner: &Interner,
    ) -> Templates {
        let mut by_name: FxHashMap<Symbol, Option<(FileId, ast::TemplateDecl)>> =
            FxHashMap::default();
        for (file, root) in roots {
            for item in root.items() {
                let ast::Item::TemplateDecl(decl) = item else { continue };
                let Some(name) = template_name(&decl, interner) else { continue };
                by_name.entry(name).and_modify(|slot| *slot = None).or_insert(Some((file, decl)));
            }
        }
        Templates { by_name, visible: FxHashMap::default() }
    }

    /// See [`Templates::visible`].
    pub(crate) fn with_visible(mut self, visible: FxHashMap<FileId, FxHashMap<Symbol, FileId>>) -> Templates {
        self.visible = visible;
        self
    }

    /// The file declaring `name` as seen from `file` through an unaliased
    /// import, if any.
    pub(crate) fn visible_through_import(&self, file: FileId, name: Symbol) -> Option<FileId> {
        self.visible.get(&file)?.get(&name).copied()
    }

    /// The template `name` names, when exactly one module declares it, and
    /// the file it is in.
    pub(crate) fn located(&self, name: Symbol) -> Option<(FileId, &ast::TemplateDecl)> {
        let (file, decl) = self.by_name.get(&name)?.as_ref()?;
        Some((*file, decl))
    }

}

/// The name of the declaration a `template <...>` wraps — `Template params a`'s
/// `a` (`Parser.hs:373-375`), which is what an instance spells.
fn template_name(decl: &ast::TemplateDecl, interner: &Interner) -> Option<Symbol> {
    let name = match decl.decl()? {
        ast::Item::FnDecl(f) => f.name()?,
        ast::Item::ClassDecl(c) => c.name()?,
        ast::Item::StructDecl(s) => s.name()?,
        ast::Item::UnionDecl(u) => u.name()?,
        ast::Item::EnumDecl(e) => e.name()?,
        ast::Item::AliasDecl(a) => a.name()?,
        _ => return None,
    };
    Some(interner.intern(name.ident_token()?.text()))
}

/// Whether some scope between `call` and module scope declares `name`.
///
/// [`Templates`] answers only the *last* step of `lookupInScope`
/// (`Symbols.hs:82-87`). This decides whether the walk gets that far. Every
/// earlier step probes `s ++ [name]` for `s` a prefix of the call's own scope,
/// and a declaration with such a key is exactly one whose binding scope is an
/// ancestor of the call and is not the file — so the question is answerable from
/// the call's ancestor chain alone, with no scope path to compute and no symbol
/// map to consult.
///
/// A hit is silence, not a different answer, because what `lookupInScope` found
/// is then something this pass cannot deduce against: a member template it does
/// not index, or — for a plain function, a local or a parameter — a symbol that
/// is not a `Template` at all, which is `deduceInstance' _ e = e`
/// (`Type.hs:1448`), no deduction whatsoever.
///
/// Any `NAME` counts, whether or not it is a declaration the reference would
/// `symbol` into the map (`Symbols.hs:52-68`). Over-declining is the safe
/// direction and the imprecision is small: a `NAME` that binds nothing is rare,
/// and one that shares a spelling with a template being called is rarer.
pub(crate) fn shadowed_before_module_scope(
    binders: &Binders,
    call: &SyntaxNode,
    name: &str,
) -> bool {
    let at = call.text_range().start();
    call.ancestors()
        .filter(|scope| {
            crate::navigate::introduces_scope(scope.kind())
                && scope.kind() != SyntaxKind::SOURCE_FILE
        })
        .any(|scope| binders.binds(&scope, name, at))
}

/// Every (binding scope, name) pair in the file, built in one walk.
///
/// The obvious spelling of [`shadowed_before_module_scope`] asks each enclosing
/// scope "do you bind this name?" and answers by walking that scope's subtree.
/// That is quadratic in the worst way: the outermost ancestor's subtree is most
/// of the file, and it gets re-walked for every call inside it. It cost
/// `test/library/processor/risc-v.k` **1.9 seconds** for a single check — a
/// twentyfold regression on the whole-corpus worst case, which is a keystroke
/// the editor waits on.
///
/// The relation is the same one, computed the other way round: walk the file
/// once, and for each `NAME` record the scope that binds it. The lookup is then
/// one hash probe per ancestor, and the pass is linear again.
///
/// A scope is keyed by `(range, kind)` rather than by the node: rowan's
/// `SyntaxNode` hashes by identity through a cursor that is allocated per
/// traversal, so two cursors onto the same node are different keys. `kind` is in
/// the key because a node and its only child can share a range.
///
/// A local variable binds only once its declaration is parsed whole:
/// `parseSymbol` registers the name after the declaration's own parser runs
/// (`SymbolTable.hs:194-207`), so `uint2 Fn_T_T = Fn_T_T(10);` calls the
/// imported template in its own initializer, and the reference deduces it
/// (difffuzz 2026-09-24, "Undefined function" here). Each binding is kept
/// with where its declaration ends — `None` for every other binder, which
/// the parse-order rule is not applied to here.
#[derive(Default)]
pub(crate) struct Binders {
    pairs: FxHashMap<(kg_base::TextRange, SyntaxKind, kg_base::SmolStr), Vec<Option<kg_base::TextSize>>>,
}

impl Binders {
    pub(crate) fn of(root: &ast::SourceFile) -> Binders {
        let mut out = Binders::default();
        for n in root.syntax().descendants() {
            if n.kind() != SyntaxKind::NAME {
                continue;
            }
            let Some(text) = ast::Name::cast(n.clone()).and_then(|n| n.ident_token()) else {
                continue;
            };
            // The same "directly" as `navigate::bound_in`, and for the same
            // reason: a local of a sibling function is a descendant of the
            // enclosing class but is bound by that function's body, and counting
            // it against the class would decline a call that cannot reach it.
            let Some(decl) = n.parent() else { continue };
            let Some(scope) = crate::navigate::binder(&decl) else {
                continue;
            };
            let ends = (decl.kind() == SyntaxKind::VAR_DECL).then(|| decl.text_range().end());
            out.pairs
                .entry((scope.text_range(), scope.kind(), kg_base::SmolStr::new(text.text())))
                .or_default()
                .push(ends);
        }
        out
    }

    /// Whether `scope` binds `name` for a reference at `at`.
    fn binds(&self, scope: &SyntaxNode, name: &str, at: kg_base::TextSize) -> bool {
        self.pairs
            .get(&(scope.text_range(), scope.kind(), kg_base::SmolStr::new(name)))
            .is_some_and(|ends| ends.iter().any(|end| end.is_none_or(|end| end <= at)))
    }
}

#[cfg(test)]
mod tests {
    /// A local variable is not yet in the parser's table inside its own
    /// initializer (`parseSymbol`, `SymbolTable.hs:194-207`), so in `uint2
    /// Fn_T_T = Fn_T_T(10);` the call names the imported template and is
    /// deduced — the variable shadowing it was "Undefined function" on a
    /// program the reference compiles (difffuzz, 2026-09-24). Declared
    /// before the call, it shadows. Red with `Binders::binds` ignoring where
    /// the declaration ends.
    #[test]
    fn a_variable_shadows_a_template_only_once_declared() {
        use kg_syntax::AstNode;
        let shadowed = |text: &str| {
            let parse = kg_syntax::parse_file(text);
            let root = parse.source_file();
            let binders = super::Binders::of(&root);
            let call = root.syntax().descendants().find(|n| n.kind() == kg_syntax::SyntaxKind::CALL_EXPR).expect("a call");
            super::shadowed_before_module_scope(&binders, &call, "Fn_T_T")
        };
        assert!(!shadowed("inline void main()\n{\n    uint2 Fn_T_T = Fn_T_T(10);\n}\n"));
        assert!(shadowed("inline void main()\n{\n    uint2 Fn_T_T;\n    uint2 y = Fn_T_T(10);\n}\n"));
    }
}
