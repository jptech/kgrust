//! Execution-order pass 9's place in a module's CST run — `undefinedSymbols`
//! (`Frontend.hs:82-157`) — and the parse-time table's own rejections, which
//! come before every pass.
//!
//! **Pass 9 is the loop's** since sprint W3 step 1 (2026-10-10):
//! `kg_sem::check`'s walk judges every name the engine types `TUndefined` —
//! `typeOfSymbol`'s failed `lookupInScope` (`Type.hs:1243-1245,1336`,
//! `Symbols.hs:76-87`) — at each module's text as in its copies, and
//! [`run_through_9`] admits its verdicts in pass 9's place, behind this
//! run's checkpoints, unless the program is one the reference never runs
//! pass 9 on ([`stands_down`]). The CST pass that judged the text before
//! (`is_known`, `qualified_member`, `alias_qualified`, the scoped probe) is
//! deleted; its tests are `crate::tests::undefined_symbols`'s, and the
//! journal's "Pass 9 on the loop: ten causes" names every way the two
//! readings parted and which one the reference took.
//!
//! **The parse-time table** stays here, the CST's: `findSymbol`'s ambiguity
//! and import-qualifier rejections ([`parse_time_find_symbol`]), and the
//! parser's template checks — a `<` no parse-time template opens
//! ([`not_a_template_yet`]), a member template the receiver's class lacks, a
//! template template parameter's default that is no template, a function
//! template written as a type. Each is a `PARSE-ERROR` in the reference, so
//! a module holding one runs no pass at all.
//!
//! The rooted door is the only one that reaches any of this: an entry point
//! that cannot see the imported modules ([`crate::check_source`]) would
//! reject every base-library name in the corpus.

use kg_base::{FileId, Interner, Symbol, SymbolPath};
use kg_diag::Diagnostic;
use kg_resolve::{ItemTree, ModuleSource, Namespace};
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode, ast};

use crate::program::Program;
use crate::sink::Sink;
use crate::symbols::SymbolMap;

/// The whole-program view this pass needs, and the only thing that distinguishes
/// the rooted door from [`crate::check_source`].
///
/// Three queries, all memoized on [`crate::Db`]: the loaded [`Program`] (for the
/// module graph and this file's item tree), the `AllSymbols` build of
/// [`SymbolMap`] (`withSymbols` is `getSymbols` is `AllSymbols`,
/// `Frontend.hs:40`), and the interner both were built with.
#[derive(Copy, Clone)]
pub(crate) struct Whole<'a> {
    pub(crate) module: ModuleSource,
    /// The file `types` is keyed by and the sink stamps: the module's own,
    /// or — for the run over its twin (`crate::elaborate`, the passes after
    /// `instantiateTemplates`) — the twin's synthetic id, whose ranges the
    /// seam translates home. Everything keyed by *module* (the tree, the
    /// graph, the imports) is the same in both runs.
    pub(crate) file: FileId,
    pub(crate) program: &'a Program,
    pub(crate) symbols: &'a SymbolMap,
    /// Every top-level template of every module — pass 4's half of
    /// `withSymbols` (`Frontend.hs:71`). Built once per program and shared, not
    /// rebuilt per file.
    pub(crate) templates: &'a super::templates::Templates,
    /// What the reference's parser built of this module
    /// ([`crate::parsed::Parsed`]) — what its own checks ask.
    pub(crate) parsed: &'a crate::parsed::Parsed,
    /// The program-wide loop: its types at this module's text, which the
    /// parser's member-template check asks of a receiver.
    pub(crate) looped: &'a crate::looped::Looped,
    pub(crate) interner: &'a Interner,
    /// Every module's tree, in `Program::files` order — for pass 9's
    /// question whether any file declares a name ([`declared_anywhere`]).
    pub(crate) roots: &'a [ast::SourceFile],
}

impl<'a> Whole<'a> {
    /// `module`'s item tree.
    pub(crate) fn tree(&self, module: ModuleSource) -> Option<&'a ItemTree> {
        self.program.tree(module)
    }
}

/// What one module's run through pass 9 leaves ([`run_through_9`]).
pub(crate) enum Through9 {
    /// The passes from 10 to run ([`crate::passes::run_from_10`]), and
    /// whether a checkpoint closed them.
    Early(Box<super::Early>),
    /// The parse-time table's rejections, alone: the reference never
    /// enters `frontend` (`Main.hs:73-75`).
    Rejected(Vec<Diagnostic>),
}

impl Through9 {
    /// Whether this module erred through pass 9 — its part of the
    /// reference's flag as pass 10 finds it (`Frontend.hs:44-52,85-86`;
    /// `crate::Db::program_erred_through_pass_9` folds every module's).
    pub(crate) fn erred(&self) -> bool {
        match self {
            Through9::Early(early) => early.gated,
            Through9::Rejected(_) => true,
        }
    }
}

/// Every wired pass through 9 over one file of a loaded program:
/// [`crate::passes::run_through_8`] and pass 9, which needs the whole
/// program; [`crate::passes::run_from_10`] runs the rest. Pass 9's
/// verdicts at this text are `from_loop`'s, admitted in its place.
///
/// Kept here rather than in `passes::run` because `run`'s signature is the
/// contract [`crate::check_source`] is written against, and pass 9 must not
/// reach that door — it would reject every base-library name in the corpus.
pub(crate) fn run_through_9(
    root: &ast::SourceFile,
    whole: Whole<'_>,
    carried: super::Carried,
    from_loop: &super::FromLoop,
) -> Through9 {
    let file = whole.file;
    // The parse-time table's own two rejections come first and alone: a
    // parse error keeps the reference out of `frontend` altogether
    // (`Main.hs:73-75`), so nothing a pass would say is ever shown beside one.
    let mut out = Vec::new();
    {
        let mut d = Sink { file, out: std::mem::take(&mut out) };
        let before = d.out.len();
        parse_time_find_symbol(root, &whole, &mut d);
        if d.out.len() > before {
            d.out.sort_by_key(|x| x.range.range.start());
            d.out.dedup_by(|a, b| a.range == b.range && a.message == b.message);
            return Through9::Rejected(d.out);
        }
        out = d.out;
    }
    let mut early =
        super::run_through_8(file, root, whole.templates, whole.parsed, whole.interner, carried.at_8, from_loop, out);
    // `undefinedSymbols _ (p, True) = (p, True)` (`Frontend.hs:85`), and its
    // own verdict — in the copies as well — sets the flag for passes 10
    // onward (`:86`). At the text it is the loop's (`kg_sem::check::
    // TextPass::UndefinedSymbols`), admitted in its place.
    if !early.gated {
        if !stands_down(&whole) {
            from_loop.admit(super::TextPass::UndefinedSymbols, &mut early.d);
        }
        early.d.out.extend(carried.at_9.iter().cloned());
        early.carried.extend(carried.at_9);
        early.checkpoint();
    }
    // The passes after 9 in the copies, which no checkpoint follows: in
    // the sink before pass 10, where pass 12 and pass 16 read it.
    if !early.gated {
        early.d.out.extend(carried.after_9.iter().cloned());
        early.carried.extend(carried.after_9);
    }
    Through9::Early(Box::new(early))
}

/// Whether pass 9 says nothing in this program, whatever this module's own
/// run says: the reference never runs it on a program where any module
/// holds an error symbol after pass 1 — `checkPar` sets the flag
/// program-wide (`Frontend.hs:48-52,85`) — nor enters the frontend at all
/// where an import failed to load (`Main.hs:73-75`), and a module that
/// failed to load contributes no symbols, so every name it would have
/// supplied looks undefined. [`crate::Db::check`] stops at the parse stage
/// before either; [`crate::Db::diagnostics`] gates per file, so that an
/// editor keeps showing `a.k`'s errors while `b.k` is half-typed, which is
/// the case this is for. The CST pass stood down the same way; the switch
/// to the loop's verdicts lost it, and `passdiff` could not see that, since
/// its rooted runs stop at the parse stage.
fn stands_down(w: &Whole<'_>) -> bool {
    w.symbols.has_error_symbol()
        || !w.program.failures.is_empty()
        || w.program.diagnostics.iter().any(Diagnostic::is_error)
        || w.program.graph.interface(w.module).is_none()
}

/// `findSymbol`'s two rejections (`SymbolTable.hs:229-250`), the parse-time
/// table's own — reported before any pass, since the reference's parser
/// fails on them and `frontend` is never entered.
///
/// ```haskell
/// findSymbol e = do
///     sym <- findInSymbolTable e t
///     case nub sym of
///         [x] -> return $ Just x
///         []  -> return Nothing
///         xs  -> registerSymbolError e $ "symbol is ambiguous. It could refer to the definition at\n\t   " ++ …
///   where
///     findInSymbolTable s | null $ getQualifier s = findInScopes scopes >>= checkImportQualifiers
///     checkImportQualifiers [x] = … find ((Just (getName s) ==) . importedAs) $ imports module_
///         Just Import{..} -> registerFancyError importOffset $ "The import qualifier '" ++ …
///     findInScopes (Scope{..}:xs) = case … HashMap.lookup (getName s) symbolsInScope of
///         [x] -> (x :) <$> findInImports x
///         _   -> findInScopes xs
///     findInScopes _ = findInSymbols
/// ```
///
/// Every unqualified name — the first segment of every path, in a type or a
/// value position, template bodies included — is looked up from its innermost
/// enclosing scope outward. What that finds decides everything, and it was
/// probed against the oracle 2026-09-10 (`scratchpad/amb/*.k`):
///
/// * a declaration in a **nested** scope — a local, a parameter, a member, a
///   template parameter, a capture — is unique: `findInImport x` asks for the
///   found symbol's *qualified* name, which no import exposes. It is found
///   only once the table holds it ([`innermost_in_parse_table`]): a local
///   or a parameter mentioned in its own type, or before it, is not, and
///   the lookup goes on outward (difffuzz 2026-10-10 [01]);
/// * a declaration at **module scope in a file with no `module` header** is
///   keyed bare, so the same question reaches the imports, and any unaliased
///   import exposing the name makes the use ambiguous — the local definition
///   listed first, the imports' after, in the reference's reverse source
///   order (`kg-resolve`'s `find_in_imports`). A `module` header qualifies
///   the key and ends the ambiguity, as an `as` alias does for the import;
/// * no declaration in this file: two imports exposing the name are
///   ambiguous, one is the answer;
/// * and when exactly one symbol answers, an import of this file aliased to
///   that very name is the second error, at the import.
///
/// Module-scope declarations count only when written *before* the use: the
/// parse-time scope stack holds what has been parsed, and a forward
/// reference falls through to `findInSymbols` and the imports, where a
/// single import hit is unique (`docs/spec/resolve.md` §7.2). Class member
/// bodies are parsed after their class (`skipMemberBody`), which this
/// port's textual rule does not reproduce; that direction is silence.
pub(crate) fn parse_time_find_symbol(root: &ast::SourceFile, w: &Whole<'_>, d: &mut Sink) {
    if !w.program.failures.is_empty()
        || w.program.diagnostics.iter().any(Diagnostic::is_error)
        || w.program.graph.interface(w.module).is_none()
    {
        return;
    }
    let Some(tree) = w.tree(w.module) else { return };
    let has_header = tree.module_header.is_some();
    let imports = w.program.graph.imports(w.module);
    let file = w.file;

    // `c.Nope<uint32>(x)`, no `template` keyword: the parser decides whether
    // `<` opens a template argument list by finding `Nope` among the
    // receiver's class's member templates (`Parser.hs:668-680`), and where
    // the class has none of that name — or the receiver is a member the
    // class lacks, so there is no class to look in — it is "`Nope`:
    // undefined template" at the head of the callee, and the frontend is
    // never entered (`member-function-template.k::22`; `scratchpad/pt/
    // {p1,p6,q8}.k`, `kg-oracle --errors` 2026-09-17). An array or a
    // scalar receiver parses and is pass 12's. The source only: a copy is
    // never parsed by the reference.
    {
        // The sort's reading — no template under the receiver's type name
        // (`crate::parsed`) — confirmed by the loop's types at this text,
        // which show where that name has nothing to look in
        // ([`parse_time_receiver_fails`]). The CST engine's type of the
        // receiver decided it until sprint W3.7.
        let typed =
            |node: &SyntaxNode| w.looped.written_at(w.module, node.text_range(), node.kind()).unwrap_or(kg_ty::Ty::Untyped);
        // A template's body and a `static if`'s branches are parsed like any
        // text, but the loop types them only in copies: there the arms that
        // read no type decide alone.
        let untyped = |_: &SyntaxNode| kg_ty::Ty::Untyped;
        for node in root.syntax().descendants() {
            if node.kind() != SyntaxKind::FIELD_EXPR
                || node.children_with_tokens().any(|c| c.kind() == SyntaxKind::TEMPLATE_KW)
                || !node.children().any(|c| ast::TemplateArgList::can_cast(c.kind()))
            {
                continue;
            }
            let type_of: &dyn Fn(&SyntaxNode) -> kg_ty::Ty =
                if crate::facts::reached_only_after_elaboration(&node) { &untyped } else { &typed };
            let Some(field) = ast::FieldExpr::cast(node.clone()) else { continue };
            let Some(base) = field.base() else { continue };
            let Some(name) = node.children().filter(|c| c.kind() == SyntaxKind::NAME_REF).last().and_then(ast::NameRef::cast)
            else {
                continue;
            };
            let Some(token) = name.ident_token() else { continue };
            if w.parsed.opens_no_template(name.syntax().text_range())
                && parse_time_receiver_fails(w, type_of, base.syntax(), token.text())
            {
                d.error(node.text_range(), format!("`{}`: undefined template", token.text()));
            }
        }
        not_a_template_yet(root, w, d);
        if !d.out.is_empty() {
            return;
        }
    }

    for node in root.syntax().descendants() {
        if node.kind() != SyntaxKind::PATH {
            continue;
        }
        let Some(path) = ast::Path::cast(node.clone()) else { continue };
        let Some(segment) = path.path_segments().next() else { continue };
        let Some(token) = segment.name_ref().and_then(|n| n.ident_token()) else { continue };
        let name = w.interner.intern(token.text());
        let bare = SymbolPath::single(name);

        let mut hits: Vec<kg_base::FileRange> = Vec::new();
        let mut unique_by_scope = false;
        match innermost_in_parse_table(&node, token.text()) {
            Some((scope, decl)) if scope.kind() == SyntaxKind::SOURCE_FILE => {
                // An alias, a variable or a plain function is in the table
                // only once it has been parsed whole: `parseSymbol p` runs
                // `p`, then registers (`SymbolTable.hs:194-207`), so a
                // mention inside it — `using Bar = Bar<uint32>;`, `intConst`
                // inside `inline uint32 intConst()` — sees only what was
                // there before, the import (difffuzz [01], [08]). A function
                // *template*'s name is seen in its own body: `auto_Fn` inside
                // `template <auto Cond> inline auto auto_Fn()` is ambiguous
                // with an imported one, the reference says (`static-if.k::5,6`).
                //
                // A declaration the table does not hold yet — that one, or
                // one written further down — leaves `findInScopes` to
                // `findInSymbols`, the imports alone (`SymbolTable.hs:
                // 256-258`): two that expose the name are ambiguous at the
                // use (`c1<>` inside `inline void c1()`, difffuzz 2026-10-10
                // [42]). One the table holds is found in the module's scope;
                // under a `module` header its key is qualified, so no import
                // answers beside it, and an import aliased to its name is
                // the conflict at the import (`kg-oracle --errors`,
                // 2026-10-10).
                let visible_from = match decl.kind() {
                    SyntaxKind::ALIAS_DECL | SyntaxKind::VAR_DECL | SyntaxKind::FN_DECL => decl.text_range().end(),
                    _ => decl.text_range().start(),
                };
                if visible_from <= node.text_range().start() {
                    if has_header {
                        unique_by_scope = true;
                    } else {
                        hits.push(kg_base::FileRange::new(file, decl.text_range()));
                    }
                }
            }
            // A module-scope template's parameter is keyed under the
            // template's own `getScopeQualifier` — the module's, empty
            // without a header — so `findInImport` asks for its bare name,
            // and an import exposing it is the ambiguity, the parameter
            // listed first (`kg-oracle --errors`, 2026-10-10: `Fn_T_N` under
            // `template <auto Fn_T_N>` beside `helper.return_type_
            // polymorphism`'s, difffuzz [23]). Under a header, or for a
            // template nested anywhere, the key is qualified and unique.
            Some((scope, decl))
                if matches!(decl.kind(), SyntaxKind::TYPE_PARAM | SyntaxKind::VALUE_PARAM)
                    && scope.kind() == SyntaxKind::TEMPLATE_DECL
                    && scope.parent().is_some_and(|p| p.kind() == SyntaxKind::SOURCE_FILE)
                    && !has_header =>
            {
                hits.push(kg_base::FileRange::new(file, decl.text_range()));
            }
            Some(_) => unique_by_scope = true,
            None => {}
        }
        if !unique_by_scope {
            for def in w.program.graph.find_in_imports(w.module, &bare) {
                if let Some(item) = w.tree(def.module).and_then(|t| t.items.get(def.item.index())) {
                    hits.push(item.decl);
                }
            }
        }

        if hits.len() > 1 {
            let locations: Vec<String> = hits.iter().map(|r| symbol_loc(w, *r)).collect();
            d.error(
                token.text_range(),
                format!(
                    "`{}`: symbol is ambiguous. It could refer to the definition at\n\t   {}",
                    token.text(),
                    locations.join("\n\tor ")
                ),
            );
            continue;
        }
        let Some(import) = imports.iter().find(|i| i.alias == Some(name)) else { continue };
        let found = if unique_by_scope {
            innermost_in_parse_table(&node, token.text())
                .map(|(_, decl)| kg_base::FileRange::new(file, decl.text_range()))
        } else {
            hits.first().copied()
        };
        let Some(found) = found else { continue };
        d.out.push(
            Diagnostic::error(
                import.range,
                format!(
                    "The import qualifier '{}' conflicts with the name of symbol defined at {}",
                    token.text(),
                    symbol_loc(w, found)
                ),
            )
            .with_label(found, "the symbol"),
        );
    }
}

/// `Foo<…>` where the parser's table does not hold a template `Foo` at
/// that point of the parse. A name reads as a template instance only when
/// `findSymbol` finds a template (`Parser.hs:669-676`); otherwise the `<` is
/// the comparison operator and the parse fails further on. Three cases, each
/// the reference's rejection (`kg-oracle --errors`):
///
/// * a template declared at module scope *after* the mention — the table is
///   built as the file is parsed (`function-template.k::17`);
/// * a module-scope declaration before it that is not a template
///   (`function-template.k::8`: `Foo<uint2>()` over `inline void Foo()`);
/// * a template the file declares only inside a block the mention is not
///   in — under a top-level `static if` (`dependent-template.k::7`).
///
/// A single-segment name with no `template` keyword, found in no import,
/// with one argument that cannot be read as the comparison the reference
/// then parses — a type, a literal or a bare name: `Foo < uint32 > (10)` is
/// no expression and `Foo < N > (x)` is a non-associative chain
/// (`Parser.hs:915-916`), where `Do<a && b>(10)` over a variable `Do` is
/// `(Do < a) && (b > 10)` and compiles (`helper/dependent_template.k`).
/// Anything the table might hold by another road — a class member (member
/// bodies are parsed after their class), a template parameter, an import —
/// is left alone: the direction a miss fails in is silence. The one nested
/// road that is certain is [`nested_non_template_in_table`].
fn not_a_template_yet(root: &ast::SourceFile, w: &Whole<'_>, d: &mut Sink) {
    // Templates declared inside a braced block at module scope — a
    // top-level `static if { … }` — visible only inside it. An unbraced
    // `static if (c) template …` declares into the module itself.
    let block_templates: Vec<(String, SyntaxNode)> = root
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::TEMPLATE_DECL)
        .filter_map(|t| {
            let block = t.parent().filter(|p| p.kind() == SyntaxKind::BLOCK_STMT)?;
            let owner = block.parent().filter(|p| p.kind() == SyntaxKind::STATIC_IF_STMT || p.kind() == SyntaxKind::ELSE_BRANCH)?;
            let at_module = owner.ancestors().skip(1).all(|a| {
                matches!(a.kind(), SyntaxKind::SOURCE_FILE | SyntaxKind::STATIC_IF_STMT | SyntaxKind::ELSE_BRANCH | SyntaxKind::BLOCK_STMT)
            });
            if !at_module {
                return None;
            }
            let decl = t.children().find(|c| c.kind() != SyntaxKind::TEMPLATE_PARAM_LIST)?;
            let name = decl.children().find(|c| c.kind() == SyntaxKind::NAME)?.first_token()?;
            Some((name.text().to_string(), block))
        })
        .collect();
    // [`declared_anywhere`] walks every module's tree, and a file asks it
    // of the same few names over and over (`risc-v.k`: ~28 asks, all of
    // `index_t`, `optional` and three more, 2026-10-10).
    let mut asked: kg_base::FxHashMap<String, bool> = kg_base::FxHashMap::default();
    let mut declared = |name: &str| *asked.entry(name.to_owned()).or_insert_with(|| declared_anywhere(w, name));
    for node in root.syntax().descendants() {
        if node.kind() != SyntaxKind::PATH || node.parent().is_some_and(|p| p.kind() == SyntaxKind::FIELD_EXPR) {
            continue;
        }
        let Some(path) = ast::Path::cast(node.clone()) else { continue };
        let mut segments = path.path_segments();
        let (Some(segment), None) = (segments.next(), segments.next()) else { continue };
        if segment.template_kw_token().is_some() {
            continue;
        }
        let Some(list) = segment.template_arg_list() else { continue };
        let args: Vec<ast::Expr> = list.exprs().collect();
        // In a type position no reading of the `<` parses — `typeSpecifier`
        // takes no operator (`Parser.hs:712-726`) — so any list is the
        // rejection there; in an expression a comma can split the chain
        // into two arguments of an enclosing call, and only one argument
        // that cannot be read as a comparison's operand is certain — or
        // none: `x<>` is `x <` with no operand before the `>`, whatever
        // follows (`x<> ;` inside `inline void x()`, difffuzz 2026-10-10
        // [33]).
        let in_type = node.parent().is_some_and(|p| p.kind() == SyntaxKind::PATH_TYPE);
        if !in_type {
            match args.as_slice() {
                [] if list.syntax().children().next().is_none() => {}
                [ast::Expr::Type(_) | ast::Expr::Literal(_) | ast::Expr::Path(_)] => {}
                _ => continue,
            }
        }
        let Some(token) = segment.name_ref().and_then(|n| n.ident_token()) else { continue };
        let name = token.text();
        let bare = SymbolPath::single(w.interner.intern(name));
        let imported = || !w.program.graph.find_in_imports(w.module, &bare).is_empty();
        // A nested declaration the table does not hold yet is passed, as
        // `findInScopes` passes it: `static x<int32, -10> x;` inside
        // `inline void x()` reads the function, not the local being
        // declared (difffuzz 2026-10-10, the re-run's `type-check.k::22`
        // mutant).
        let rejected = match innermost_in_parse_table(&node, name) {
            Some((scope, decl)) if scope.kind() == SyntaxKind::SOURCE_FILE => {
                !imported()
                    && (decl.text_range().start() > node.text_range().start()
                        || decl.kind() != SyntaxKind::TEMPLATE_DECL)
            }
            // A nested declaration is found first and alone: `findInImport`
            // asks for its qualified name, which no import exposes
            // (`SymbolTable.hs:256-257,272-279`).
            Some((scope, decl)) => nested_non_template_in_table(&scope, &decl, &node),
            // A name no file of the program declares cannot be in the
            // table by any road, so `findSymbol` is `Nothing` and no list is
            // parsed (`Parser.hs:678-680`) — inside a template body too,
            // which pass 9 leaves to the copies and the reference rejects
            // whether or not it is ever instantiated (`kg-oracle --errors`,
            // 2026-10-10: `static Nope<T, 32> m;`, `return Nope<T>(x);`).
            None => {
                !imported()
                    && (block_templates
                        .iter()
                        .any(|(n, block)| n == name && !block.text_range().contains_range(node.text_range()))
                        || !declared(name))
            }
        };
        if rejected {
            // A name declared nowhere is most likely a misspelling: offer
            // what pass 9 would (`hintMsg`, `Frontend.hs:110-155`) — the
            // reference's own rejection is a bare parse error.
            let hint = match (innermost_declaring_scope(&node, name), w.tree(w.module)) {
                (None, Some(tree)) if !declared(name) => {
                    hints(w, w.interner.intern(name), in_type, &use_scope(w, tree, &node), &node, &[])
                }
                _ => String::new(),
            };
            d.error(segment.syntax().text_range(), format!("`{name}`: undefined template{hint}"));
        }
    }
    receiver_not_yet_a_class(root, w, d);
    template_template_default_not_a_template(root, w, d);
    function_template_as_a_type(root, w, d);
}

/// Whether any file of the program declares `name` — a `NAME` node of that
/// spelling anywhere, every kind of declaration and every scope.
fn declared_anywhere(w: &Whole<'_>, name: &str) -> bool {
    w.roots.iter().any(|root| {
        root.syntax().descendants().any(|n| {
            n.kind() == SyntaxKind::NAME && n.first_token().is_some_and(|t| t.text() == name)
        })
    })
}

/// Whether a nested declaration the parse-time table certainly holds at
/// `at`, and that is no template, shadows the name — so `Foo<10>` is `Foo <
/// 10 >` and the parse fails (`Parser.hs:669-686`, the `otherwise` arm):
///
/// * a class's variable, alias or type member, wherever in the class it is
///   written — `class_` parses the class once skipping member bodies and
///   seeds its scope with every member for which `isTemplate m || isType m
///   || isVariable m` (`Parser.hs:316-321,323-329`), so `using Foo =
///   Foo<10>;` inside a class names the member being declared
///   (`using.k::37`), and `Foo<10> x;` names a member written after it;
/// * a local declared before the mention in a block, or a function's
///   parameter — both `symbol`-ed as parsed (`SymbolTable.hs:194-207`,
///   `Parser.hs:377`).
///
/// Probed with `kg-oracle --errors` (2026-09-27, `target/w2/c1`, `c3`): every
/// one a `PARSE-ERROR`. Not claimed: a member function (not seeded, so a
/// later one is not in the table — the reference then reports "Undefined
/// template" from the frontend), a struct's field (`memberVariable` is not a
/// `symbol`), a declaration inside a nested `static if` block, a template
/// parameter, a capture, and a local declared after the mention (the
/// lookup then continues outward).
fn nested_non_template_in_table(scope: &SyntaxNode, decl: &SyntaxNode, at: &SyntaxNode) -> bool {
    let is_type_or_variable = matches!(
        decl.kind(),
        SyntaxKind::VAR_DECL
            | SyntaxKind::ALIAS_DECL
            | SyntaxKind::STRUCT_DECL
            | SyntaxKind::UNION_DECL
            | SyntaxKind::CLASS_DECL
            | SyntaxKind::ENUM_DECL
    );
    match scope.kind() {
        SyntaxKind::DECL_LIST if scope.parent().is_some_and(|p| p.kind() == SyntaxKind::CLASS_DECL) => is_type_or_variable,
        SyntaxKind::BLOCK_STMT => is_type_or_variable && decl.text_range().end() <= at.text_range().start(),
        SyntaxKind::FN_DECL => decl.kind() == SyntaxKind::PARAM,
        _ => false,
    }
}

/// A template template parameter's default names no template the parse-time
/// table holds: `typename TT = X` parses its default with `templateSpecifier`
/// (`Parser.hs:368`), which is `unexpectedSymbol s "template specifier"`
/// unless `findSymbol` returns a template (`Parser.hs:696-702`) — a
/// `PARSE-ERROR` at the name (`template-template-parameter.k::23`: `using X
/// = uint32;`). Claimed where the answer is certain: a module-scope
/// declaration before the parameter that is not a template, a template
/// declared only after it (the table has not reached it), or — nothing in
/// the file — imports whose every answer is not a template. A name nothing
/// answers is pass 9's "Type name not in scope" already; a nested
/// declaration, a qualified name and an instance are left alone.
fn template_template_default_not_a_template(root: &ast::SourceFile, w: &Whole<'_>, d: &mut Sink) {
    for param in root.syntax().descendants().filter(|n| n.kind() == SyntaxKind::TYPE_PARAM) {
        if !param.children_with_tokens().any(|c| c.kind() == SyntaxKind::TEMPLATE_KW) {
            continue;
        }
        let Some(ast::Type::PathType(default)) = param
            .children_with_tokens()
            .skip_while(|c| c.kind() != SyntaxKind::EQ)
            .filter_map(|c| c.into_node())
            .find_map(ast::Type::cast)
        else {
            continue;
        };
        let Some(path) = default.path() else { continue };
        let mut segments = path.path_segments();
        let (Some(segment), None) = (segments.next(), segments.next()) else { continue };
        if segment.template_arg_list().is_some() || segment.template_kw_token().is_some() {
            continue;
        }
        let Some(token) = segment.name_ref().and_then(|n| n.ident_token()) else { continue };
        let name = token.text();
        let not_a_template = match innermost_declaring_scope(path.syntax(), name) {
            Some((scope, decl)) if scope.kind() == SyntaxKind::SOURCE_FILE => {
                decl.kind() != SyntaxKind::TEMPLATE_DECL || decl.text_range().start() > path.syntax().text_range().start()
            }
            Some(_) => false,
            None => {
                let hits = w.program.graph.find_in_imports(w.module, &SymbolPath::single(w.interner.intern(name)));
                !hits.is_empty()
                    && hits.iter().all(|def| {
                        w.tree(def.module).and_then(|t| t.items.get(def.item.index())).is_some_and(|item| item.kind != kg_resolve::DefKind::Template)
                    })
            }
        };
        if not_a_template {
            d.error(token.text_range(), format!("`{name}` is not a template"));
        }
    }
}

/// A variable's, a parameter's or an alias's written type that names a
/// *function* template with arguments: `typeSpecifier` accepts a symbol
/// only when `isType t` (`Parser.hs:712-721`), and `isType (TemplateF _ a)
/// = isType a` is `False` for a function (`Parser/Syntax.hs:695`), so it is
/// `unexpectedSymbol s "type specifier"` — a `PARSE-ERROR`
/// (`function-template.k::18`: `Bar<uint32, 0> x;` over an imported
/// function template; `::19`: `using Bar32 = Bar<T, 0>;`, where the
/// alias-template alternative backtracks and the error lands on `using`).
/// Without arguments it is the "template arguments missing" arm, already
/// `pass_0_template_arguments_missing`'s. Claimed where the table's answer
/// is certain: a function template declared at module scope before the use
/// that no import also answers, or — nothing in the file — a single import
/// answer that is one. A type position inside a template argument list is
/// `templateSpecifier`'s, not this (`Parser.hs:691`), and is left alone.
fn function_template_as_a_type(root: &ast::SourceFile, w: &Whole<'_>, d: &mut Sink) {
    for node in root.syntax().descendants() {
        let Some(ty) = ast::PathType::cast(node.clone()) else { continue };
        // The parser reads every body, a template's included: no
        // `reached_only_after_elaboration` here.
        if !node.parent().is_some_and(|p| matches!(p.kind(), SyntaxKind::VAR_DECL | SyntaxKind::ALIAS_DECL | SyntaxKind::PARAM)) {
            continue;
        }
        let Some(path) = ty.path() else { continue };
        let mut segments = path.path_segments();
        let (Some(segment), None) = (segments.next(), segments.next()) else { continue };
        if segment.template_arg_list().is_none() {
            continue;
        }
        let Some(token) = segment.name_ref().and_then(|n| n.ident_token()) else { continue };
        let name = token.text();
        let hits = w.program.graph.find_in_imports(w.module, &SymbolPath::single(w.interner.intern(name)));
        let function_template = match innermost_declaring_scope(&node, name) {
            Some((scope, decl)) if scope.kind() == SyntaxKind::SOURCE_FILE => {
                hits.is_empty()
                    && decl.kind() == SyntaxKind::TEMPLATE_DECL
                    && decl.text_range().end() <= node.text_range().start()
                    && ast::TemplateDecl::cast(decl).and_then(|t| t.decl()).is_some_and(|inner| inner.syntax().kind() == SyntaxKind::FN_DECL)
            }
            Some(_) => false,
            None => match hits.as_slice() {
                [def] => imported_function_template(w, *def),
                _ => false,
            },
        };
        if function_template {
            d.error(token.text_range(), format!("`{name}`: a function template is not a type"));
        }
    }
}

/// Whether an import's answer is a `template <…>` whose declaration is a
/// function — the item it wraps (`Item::parent`, `is_template`).
fn imported_function_template(w: &Whole<'_>, def: kg_resolve::DefId) -> bool {
    let Some(tree) = w.tree(def.module) else { return false };
    if tree.items.get(def.item.index()).is_none_or(|i| i.kind != kg_resolve::DefKind::Template) {
        return false;
    }
    tree.items.iter().any(|i| {
        i.parent == Some(def.item)
            && i.is_template
            && matches!(i.kind, kg_resolve::DefKind::Function | kg_resolve::DefKind::FunctionDecl)
    })
}

/// `recv.M<…>` with no `template` keyword, where the parser cannot know
/// `recv`'s class at that point, so `M` is no member template of it and `<`
/// is the comparison (`Parser.hs:668-680`): the receiver's declared type is
/// a `typename` parameter — a dependent name, which is what `template` is
/// for (`dependent-template.k::4`) — or the receiver is declared only inside
/// a block the mention is not in, a `static if { … }` in a class or a
/// function (`::2,11`). The same single-argument shape as
/// [`not_a_template_yet`], and the same silence for anything else.
fn receiver_not_yet_a_class(root: &ast::SourceFile, w: &Whole<'_>, d: &mut Sink) {
    for node in root.syntax().descendants() {
        if node.kind() != SyntaxKind::FIELD_EXPR || node.children_with_tokens().any(|c| c.kind() == SyntaxKind::TEMPLATE_KW) {
            continue;
        }
        let Some(list) = node.children().find_map(ast::TemplateArgList::cast) else { continue };
        let args: Vec<ast::Expr> = list.exprs().collect();
        let [arg] = args.as_slice() else { continue };
        if !matches!(arg, ast::Expr::Type(_) | ast::Expr::Literal(_) | ast::Expr::Path(_)) {
            continue;
        }
        let Some(ast::Expr::Path(receiver)) = ast::FieldExpr::cast(node.clone()).and_then(|f| f.base()) else {
            continue;
        };
        let mut segments = receiver.path_segments();
        let (Some(segment), None) = (segments.next(), segments.next()) else { continue };
        let Some(token) = segment.name_ref().and_then(|n| n.ident_token()) else { continue };
        let name = token.text();
        let Some(member) = node
            .children()
            .filter(|c| c.kind() == SyntaxKind::NAME_REF)
            .last()
            .and_then(|n| n.first_token())
        else {
            continue;
        };
        let rejected = match innermost_declaring_scope(receiver.syntax(), name) {
            Some((_, decl)) => declared_type_is_a_typename(&decl),
            None => {
                let bare = SymbolPath::single(w.interner.intern(name));
                w.program.graph.find_in_imports(w.module, &bare).is_empty()
                    && declared_only_in_other_blocks(root, name, &node)
            }
        };
        if rejected {
            d.error(node.text_range(), format!("`{}`: undefined template", member.text()));
        }
    }
}

/// Whether a variable declaration's written type is a bare `typename`
/// parameter of an enclosing template.
fn declared_type_is_a_typename(decl: &SyntaxNode) -> bool {
    let Some(ast::Type::PathType(ty)) = decl.children().find_map(ast::Type::cast) else { return false };
    let Some(path) = ty.path() else { return false };
    let mut segments = path.path_segments();
    let (Some(segment), None) = (segments.next(), segments.next()) else { return false };
    if segment.template_arg_list().is_some() {
        return false;
    }
    let Some(name) = segment.name_ref().and_then(|n| n.ident_token()) else { return false };
    matches!(
        innermost_declaring_scope(ty.syntax(), name.text()),
        Some((_, param)) if param.kind() == SyntaxKind::TYPE_PARAM
            && param.children_with_tokens().any(|c| c.kind() == SyntaxKind::TYPENAME_KW)
            && !param.children().any(|c| c.kind() == SyntaxKind::TEMPLATE_PARAM_LIST)
    )
}

/// Whether every declaration of `name` in the file is a variable inside a
/// braced block that does not contain `at` — and there is one.
fn declared_only_in_other_blocks(root: &ast::SourceFile, name: &str, at: &SyntaxNode) -> bool {
    let mut found = false;
    for decl in root.syntax().descendants() {
        let declares = decl.children().any(|c| c.kind() == SyntaxKind::NAME && c.first_token().is_some_and(|t| t.text() == name));
        if !declares {
            continue;
        }
        if decl.kind() != SyntaxKind::VAR_DECL {
            return false;
        }
        let Some(block) = decl.ancestors().skip(1).find(|a| a.kind() == SyntaxKind::BLOCK_STMT) else { return false };
        if block.text_range().contains_range(at.text_range()) {
            return false;
        }
        found = true;
    }
    found
}

/// The innermost enclosing node that declares `name` directly — in its own
/// children, or in a parameter, template-parameter or capture list among
/// them — and the declaring node. The parse-time scope stack, read off the
/// tree: a `SOURCE_FILE` answer is a module-scope declaration.
fn innermost_declaring_scope(from: &SyntaxNode, name: &str) -> Option<(SyntaxNode, SyntaxNode)> {
    innermost_declaring_scope_where(from, name, |_, _| true)
}

/// [`innermost_declaring_scope`], skipping a nested declaration the
/// parse-time table does not hold yet when the parser reaches `from` — the
/// lookup then goes on outward, as `findInScopes` does past a scope whose
/// map lacks the name (`SymbolTable.hs:254-256`). Probed with `kg-oracle
/// --errors` over `import a1.a1 as Foo` and a `Foo::a1<10>` beside a
/// nested `Foo` (2026-10-10, difffuzz [01]):
///
/// * a local, a function's or a lambda's parameter, and a function — a
///   class's member function too — are `symbol`-ed once parsed whole
///   (`SymbolTable.hs:194-207`; `variable`, `funcParams`, `function`,
///   `Parser.hs:377,440-450`), so a mention inside one, or before it, is
///   not theirs: `Foo::a1<10> Foo;` in a block, `void f(Foo::a1<10> Foo)`;
/// * a struct's or a union's field is never in the table: `memberVariable`
///   is `uninitVar`, `noted` and not `symbol` (`Parser.hs:300,306,457`);
/// * a class's variable is, from the class's first line: `class_` seeds
///   the class's scope with every member that `isTemplate || isType ||
///   isVariable` (`Parser.hs:316-321`), and a member function is none.
///
/// Every other nested declaration counts wherever it is, as it did before
/// this was asked: a template parameter and an induction variable are
/// parsed before anything that can name them.
fn innermost_in_parse_table(from: &SyntaxNode, name: &str) -> Option<(SyntaxNode, SyntaxNode)> {
    let at = from.text_range().start();
    innermost_declaring_scope_where(from, name, |scope, decl| {
        let member_of = |kinds: &[SyntaxKind]| {
            scope.kind() == SyntaxKind::DECL_LIST && scope.parent().is_some_and(|p| kinds.contains(&p.kind()))
        };
        match decl.kind() {
            SyntaxKind::VAR_DECL if member_of(&[SyntaxKind::STRUCT_DECL, SyntaxKind::UNION_DECL]) => false,
            SyntaxKind::VAR_DECL if member_of(&[SyntaxKind::CLASS_DECL]) => true,
            SyntaxKind::VAR_DECL | SyntaxKind::PARAM | SyntaxKind::FN_DECL | SyntaxKind::FN_PROTO => {
                scope.kind() == SyntaxKind::SOURCE_FILE || decl.text_range().end() <= at
            }
            _ => true,
        }
    })
}

fn innermost_declaring_scope_where(
    from: &SyntaxNode,
    name: &str,
    held: impl Fn(&SyntaxNode, &SyntaxNode) -> bool,
) -> Option<(SyntaxNode, SyntaxNode)> {
    let declares = |n: &SyntaxNode| -> bool {
        let named = |child: SyntaxNode| {
            child.kind() == SyntaxKind::NAME
                && child.first_token().is_some_and(|t| t.text() == name)
        };
        match n.kind() {
            // `import a.b as m` binds a qualifier, not a symbol
            // (`SymbolTable.hs:276-278`); it is in no scope's table.
            SyntaxKind::IMPORT_DECL => false,
            SyntaxKind::TEMPLATE_DECL => n
                .children()
                .find(|c| c.kind() != SyntaxKind::TEMPLATE_PARAM_LIST)
                .is_some_and(|decl| decl.children().any(named)),
            SyntaxKind::CAPTURE => n.children().any(|c| {
                c.kind() == SyntaxKind::NAME_REF && c.first_token().is_some_and(|t| t.text() == name)
            }),
            _ => n.children().any(named),
        }
    };
    for ancestor in from.ancestors().skip(1) {
        for child in ancestor.children() {
            let candidates: Vec<SyntaxNode> = match child.kind() {
                SyntaxKind::PARAM_LIST
                | SyntaxKind::TEMPLATE_PARAM_LIST
                | SyntaxKind::CAPTURE_LIST => child.children().collect(),
                _ => vec![child.clone()],
            };
            for c in candidates {
                if declares(&c) {
                    // A template's wrapped declaration is `symbol`-ed into the
                    // scope the `template` keyword was written in
                    // (`Parser.hs:352-375`); only its parameters live inside.
                    // Its location is the template's — `symbolLoc` prints
                    // the `template` keyword's line for it.
                    let (scope, decl) = if ancestor.kind() == SyntaxKind::TEMPLATE_DECL
                        && !matches!(c.kind(), SyntaxKind::TYPE_PARAM | SyntaxKind::VALUE_PARAM)
                    {
                        (ancestor.parent().unwrap_or_else(|| ancestor.clone()), ancestor.clone())
                    } else {
                        (ancestor.clone(), c)
                    };
                    if held(&scope, &decl) {
                        return Some((scope, decl));
                    }
                }
            }
        }
    }
    None
}

/// `symbolLoc` (`Error.hs:108-112`): `name:line:col`, 1-based, for a
/// declaration in any module of the program; the range alone when the module
/// has no display name.
fn symbol_loc(w: &Whole<'_>, at: kg_base::FileRange) -> String {
    let module = w.program.files.iter().position(|m| m.file() == Some(at.file));
    let name = module
        .and_then(|i| w.tree(w.program.files[i]))
        .and_then(|t| t.display_name.clone());
    let text = module.and_then(|i| w.roots.get(i)).map(|r| r.syntax().text().to_string());
    match (name, text) {
        (Some(name), Some(text)) => {
            let pos = kg_base::LineIndex::new(&text).line_col(at.range.start());
            format!("{name}:{}:{}", pos.line + 1, pos.col + 1)
        }
        _ => format!("{:?}", at.range),
    }
}

/// Whether the class declared under `class` has no member template named
/// `member` — asked of the class's own declaration, prototype or body
/// (`template <typename T> void Do(T x);` in an `extern` class counts),
/// found through the symbol map and the declaring module's tree; a class
/// this cannot locate answers `false`, silence.
fn class_lacks_member_template(w: &Whole<'_>, class: &SymbolPath, member: &str) -> bool {
    let Some(def) = w.symbols.get(class).or_else(|| w.symbols.lookup_in_scope(&SymbolPath::empty(), class))
    else {
        return false;
    };
    let Some(index) = w.program.files.iter().position(|m| *m == def.module) else { return false };
    let Some(item) = w.tree(def.module).and_then(|t| t.item(def.item)) else { return false };
    let Some(root) = w.roots.get(index) else { return false };
    let Some(decl) = root
        .syntax()
        .descendants()
        .filter_map(ast::ClassDecl::cast)
        .find(|c| c.name().is_some_and(|n| n.syntax().text_range() == item.range.range))
    else {
        return false;
    };
    let templates = decl
        .syntax()
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::TEMPLATE_DECL)
        .filter(|t| t.ancestors().skip(1).find(|a| a.kind() == SyntaxKind::CLASS_DECL).as_ref() == Some(decl.syntax()));
    !templates.into_iter().any(|t| {
        t.children().any(|wrapped| {
            wrapped
                .children()
                .find(|c| c.kind() == SyntaxKind::NAME)
                .and_then(|n| n.first_token())
                .is_some_and(|tok| tok.text() == member)
        })
    })
}

/// Whether the parser's `resolveTypeName` (`Parser.hs:736-762`) finds no
/// template `member` under the type name of `base`, a method's receiver —
/// confirmed by the loop's types (`type_of`), for a method the sort has
/// already read as opening no template list (`crate::parsed`). Alone, the
/// sort's reading also rejects a method whose receiver's type the parser
/// never resolved — `fj.fork<0>(fast_task)` over a `fork_join<…>` no
/// import reaches (`test/library/control/async.k` and
/// `test/interface/export_class_test_cases_3.k` under `mini-base`) —
/// where the reference's parser has already failed at the type, which
/// this port reports apart (pass 12's "undefined template", divergence
/// #7); a second parse-time error at the method would close the module's
/// run before that one. Two ways the name has nothing to look in, the CST
/// check's two:
///
/// * the receiver is a class the loop types, declaring no member template
///   `member` (`c.Nope<uint32>(x)`);
/// * a member down the chain is one its aggregate lacks — the loop types
///   the access `Undefined`, or its base as an aggregate declaring no
///   member of that name — past which the table has no class to look into
///   (`c2s[1].c[0].Foo<uint32>(x)` over `helper`'s `C2`, which has no `c`:
///   `member-function-template.k::22`, `kg-oracle --errors`; the loop types
///   `c2s[1].c` unresolved, the CST engine typed it `Undefined`).
///
/// And two where the receiver is a bare name and the parser resolves no
/// type name for it at all, whatever its class declares (`kg-oracle
/// --errors`, 2026-10-10):
///
/// * the table holds the name as no `Variable` — a parameter, a template's
///   value parameter: `resolveTypeName` reads a type only off a `Variable`
///   (`Parser.hs:744-751`), so `c.Foo<uint32>(1)` over `inline uint32
///   f(C c)` is "`Foo`: undefined template" though `C` declares it, and so
///   is `d.Go<>` over `template <auto d>` (difffuzz [44]). The guard above
///   still holds: a parameter whose written type has arguments the parser
///   could have failed on counts only where the loop types it a class;
/// * a variable whose written type is a bare name the table does not hold
///   at the method — `typeName` asks `findSymbol` there (`:752-757`): `C
///   c; c.Do<>();` above the `class C` that declares `Do` (difffuzz [49]).
///   A bare name is never the parser's failure, and a name an import or a
///   nested scope may answer is held.
///
/// Anything else — a receiver the loop does not type as a class, a chain —
/// says nothing: the reference rejects some of these, but the sort's
/// reading alone could be one of its gaps.
fn parse_time_receiver_fails(w: &Whole<'_>, type_of: &dyn Fn(&SyntaxNode) -> kg_ty::Ty, base: &SyntaxNode, member: &str) -> bool {
    let class = match type_of(base) {
        kg_ty::Ty::Reference(q) => Some(q),
        other if other.is_class() => other.qualified_name().cloned(),
        _ => None,
    };
    if class.as_ref().is_some_and(|class| class_lacks_member_template(w, class, member)) {
        return true;
    }
    if let Some(receiver) = ast::Path::cast(base.clone())
        && let (Some(segment), None) = { let mut s = receiver.path_segments(); (s.next(), s.next()) }
        && segment.template_arg_list().is_none()
        && let Some(name) = segment.name_ref().and_then(|n| n.ident_token())
    {
        match innermost_in_parse_table(base, name.text()) {
            Some((_, decl)) if matches!(decl.kind(), SyntaxKind::PARAM | SyntaxKind::VALUE_PARAM) => {
                let written = decl.children().find_map(ast::Type::cast);
                let args = written.is_none_or(|t| t.syntax().descendants().any(|n| n.kind() == SyntaxKind::TEMPLATE_ARG_LIST));
                return !args || class.is_some();
            }
            Some((_, decl)) if decl.kind() == SyntaxKind::VAR_DECL => {
                if let Some(ast::Type::PathType(t)) = decl.children().find_map(ast::Type::cast)
                    && let Some(path) = t.path()
                    && let (Some(segment), None) = { let mut s = path.path_segments(); (s.next(), s.next()) }
                    && segment.template_arg_list().is_none()
                    && let Some(type_name) = segment.name_ref().and_then(|n| n.ident_token())
                {
                    return !type_name_held_at(w, base, type_name.text());
                }
            }
            _ => {}
        }
    }
    let mut node = base.clone();
    loop {
        match node.kind() {
            SyntaxKind::INDEX_EXPR => {
                let Some(inner) = ast::IndexExpr::cast(node.clone()).and_then(|i| i.base()) else { return false };
                node = inner.syntax().clone();
            }
            SyntaxKind::FIELD_EXPR => {
                if matches!(type_of(&node), kg_ty::Ty::Undefined) {
                    return true;
                }
                let Some(inner) = ast::FieldExpr::cast(node.clone()).and_then(|f| f.base()) else { return false };
                let name = node.children().filter(|c| c.kind() == SyntaxKind::NAME_REF).last().and_then(|n| n.first_token());
                if let Some(name) = name
                    && let Some(members) = w.looped.members(&type_of(inner.syntax()), w.interner)
                    && !members.iter().any(|m| w.interner.resolve(m.name) == name.text())
                {
                    return true;
                }
                node = inner.syntax().clone();
            }
            _ => return false,
        }
    }
}

/// Whether the parse-time table may hold `name` when the parser reaches
/// `at`: a nested declaration it holds there ([`innermost_in_parse_table`]),
/// a module-scope one registered before it (`parseSymbol`, as
/// [`parse_time_find_symbol`] reads it), or any import exposing the name —
/// an ambiguity included, which is a rejection of its own.
fn type_name_held_at(w: &Whole<'_>, at: &SyntaxNode, name: &str) -> bool {
    match innermost_in_parse_table(at, name) {
        Some((scope, decl)) if scope.kind() == SyntaxKind::SOURCE_FILE => {
            let visible_from = match decl.kind() {
                SyntaxKind::ALIAS_DECL | SyntaxKind::VAR_DECL | SyntaxKind::FN_DECL => decl.text_range().end(),
                _ => decl.text_range().start(),
            };
            if visible_from <= at.text_range().start() {
                return true;
            }
        }
        Some(_) => return true,
        None => {}
    }
    !w.program.graph.find_in_imports(w.module, &SymbolPath::single(w.interner.intern(name))).is_empty()
}

/// `getScope e` for the use at `at` — the `declScopeName` the parser gave
/// the node, which is the scope stack's qualifier at that offset. Replayed
/// off the item tree's scopes (`kg_resolve::scope_at`, `qualifier_at`),
/// since 2026-09-10; it used to be read off the innermost item's range plus
/// `$` for any use inside a function, which put a *signature* inside the
/// body and offered a body's alias as the hint for its own return type
/// (`using.k::29`).
fn use_scope(w: &Whole<'_>, tree: &ItemTree, at: &SyntaxNode) -> SymbolPath {
    let _ = w;
    let offset = at.text_range().start();
    let scope = kg_resolve::scope_at(&tree.scopes, offset);
    kg_resolve::qualifier_at(&tree.scopes, scope, offset)
}

/// `hintMsg (hints cond)` (`Frontend.hs:110-155`) — the "Perhaps you meant"
/// tail.
///
/// Faithful in its first disjunct and a **subset** in its second. `similar` is
///
/// ```haskell
/// similar (name, distance) =
///     (distance <= 1 && moduleDefined name) ||
///     (distance <= threshold && inScope name)
/// ```
///
/// and `inScope` needs the referring node's `declScopeName`, which is a
/// parse-time note this crate does not carry to a use site. Only the case that
/// is true for *every* scope is admitted: a key of one component, whose
/// `init` is `[]` and is therefore a prefix of any scope. Nothing observable
/// depends on the difference — every reference frontend error is `Error 1`
/// (`Parser/IO.hs:114`) and the harness reads only the code
/// (`syntax-test.py:113-118`) — so the narrower set costs nothing and cannot
/// invent a hint that names a symbol the reference would not have offered.
fn hints(
    w: &Whole<'_>,
    undefined: Symbol,
    want_type: bool,
    scope: &SymbolPath,
    at: &SyntaxNode,
    written_qualifier: &[Symbol],
) -> String {
    let target = w.interner.resolve(undefined);
    let threshold = 3.min(target.chars().count() as u32 / 2);
    let file = w.file;

    let mut scored: Vec<(u32, bool, String)> = Vec::new();
    for (key, def) in w.symbols.iter() {
        let Some(last) = key.last() else { continue };
        // `filter (cond . typeOf . snd)`: `tIsType` for a type-position miss,
        // `tIsTemplate t || (tResolved t && not (tIsType t))` otherwise
        // (`Frontend.hs:115-122`). `DefKind::is_type` is the same partition
        // drawn from the declaration instead of from its inferred type.
        let Some(item) = w.program.items().item(def) else { continue };
        if item.kind.is_type() != want_type {
            continue;
        }
        let candidate = w.interner.resolve(last);
        let distance = kg_base::restricted_damerau_levenshtein(&target, &candidate);
        // `moduleDefined x = '@' == head (toString $ head x) && 2 == length x`.
        let module_defined = key.len() == 2
            && key.first().is_some_and(|s| w.interner.resolve(s).starts_with('@'));
        // `inScope x = let qualifier = init x in getQualifier e \`isSuffixOf\`
        // qualifier && take (…) qualifier \`isPrefixOf\` getScope e`
        // (`Frontend.hs:125-127`): the candidate's declaring scope is a
        // prefix of the referring expression's own — a local of the same
        // function counts, a local of another does not, and a top-level
        // name always does (`type-identifier.k::16,17`: `values` → `value`).
        // `getQualifier e` is the use's qualifier *after* the parse-time
        // table resolved it — `Bar::alia` inside `Foo` is `[Foo, Bar, alia]`
        // — so a qualified miss offers the members of what it named: the
        // candidate's scope must end in that qualifier, and what is left of
        // it must prefix the referring scope (`type-identifier.k::22`:
        // `Bar::alia` → `dahlia`, and not the `alias` of a class declared
        // inside the method).
        let qualifier = &key.segments()[..key.len() - 1];
        let in_scope = qualifier.ends_with(written_qualifier)
            && scope.segments().starts_with(&qualifier[..qualifier.len() - written_qualifier.len()]);
        if !((distance <= 1 && module_defined) || (distance <= threshold && in_scope)) {
            continue;
        }
        // `filter (cond . typeOf . snd)`'s `tResolved`, in the two shapes the
        // corpus pins. A *variable* whose own initializer holds the undefined
        // name is unresolved because of it — `const auto fn = fn2;` offers
        // `fn1`, never `fn` (`function-constants.k::3`); the enclosing
        // *function* is resolved and is offered (`min` → `main`,
        // `type-check.k::56`). And a function declared inside another's body
        // is not typed by pass 9 — `moveFunctions` has not run — so it is
        // never offered (`type-identifier.k::17`: `valeus` is a local
        // function and the reference names only `value`).
        let is_variable = matches!(item.kind, kg_resolve::DefKind::Variable(_));
        if is_variable && item.decl.file == file && item.decl.range.contains_range(at.text_range()) {
            continue;
        }
        let dollar = w.interner.intern("$");
        if item.kind == kg_resolve::DefKind::Function && qualifier.contains(&dollar) {
            continue;
        }
        // `hint name | moduleDefined name && not (inScope name) = … "defined
        // in" …` (`Frontend.hs:144-147`): a module-scope name the use's own
        // scope reaches — its own module's — is spelled bare, `foo`, as is
        // any other in scope; only another module's says where it is.
        let text = if module_defined && !in_scope {
            let ns = w.interner.resolve(key.first().expect("checked len == 2"));
            format!("`{candidate}` defined in {}", Namespace::unmangle(&ns))
        } else {
            format!("`{candidate}`")
        };
        scored.push((distance, module_defined, text));
    }
    // `take 3 $ sortOn snd` — by distance, and stably, so a `HashMap.toList`
    // order this crate does not share cannot change which three are printed.
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
    scored.dedup_by(|a, b| a.2 == b.2);
    let chosen: Vec<String> = scored.into_iter().take(3).map(|(_, _, t)| t).collect();

    match chosen.len() {
        0 => String::new(),
        1 => format!("\nPerhaps you meant: {}", chosen[0]),
        _ => format!("\nPerhaps you meant one of these:\n\t{}", chosen.join("\n\t")),
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Utf8PathBuf};
    use kg_config::Settings;

    use crate::Db;

    /// A two-file program rooted at `main.k`, with `/proj` on the import path so
    /// a bare `import a` finds `/proj/a.k`, and no base library — the shape
    /// `tests/query.rs` uses, kept here so this pass's oracles travel with it.
    fn program(files: &[(&str, &str)]) -> (Db, FileId) {
        let mut settings = Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        let mut db = Db::new(settings);
        let mut root = FileId::SYNTHETIC;
        for (name, text) in files {
            let file = db.set_file_text(
                &Utf8PathBuf::from(format!("/proj/{name}")),
                (*text).to_owned(),
            );
            if *name == "main.k" {
                root = file;
            }
        }
        (db, root)
    }

    #[track_caller]
    fn errors(files: &[(&str, &str)]) -> Vec<String> {
        let (db, root) = program(files);
        db.check(&[root]).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect()
    }

    /// A method on a receiver whose type is itself an undefined template
    /// is the type's error, not the method's: `static Nope<uint32> b;
    /// b.get<1>();` — the reference's parser fails at the type
    /// (`kg-oracle --no-base --errors`: 3:16, "unexpected '<'"), and this
    /// port reports the type, "`Nope`: undefined template" (divergence #7's
    /// stand-in). The sort reads `get` as opening no template list — under
    /// an unresolved type name it finds nothing — and the loop's types
    /// confirm nothing there (no class), so the method says nothing; taken
    /// alone, the sort's reading would close the module's run on the
    /// method, after the type (`test/library/control/async.k`'s
    /// `fj.fork<0>` under `mini-base`, which reaches no `fork_join`). Red
    /// with the sort's reading taken alone.
    #[test]
    fn a_method_on_an_undefined_templates_instance_is_the_types_error() {
        let main = "inline void Do()\n{\n    static Nope<uint32> b;\n    b.get<1>();\n}\n";
        assert_eq!(errors(&[("main.k", main)]), ["`Nope`: undefined template"]);
    }

    /// `recv.M<…>` with no `template`: a receiver whose type is a
    /// `typename` parameter has no class the parser can look `M` up in
    /// (`dependent-template.k::4`), nor does one declared only inside a
    /// block the call is not in (`::11`); `template` says it is one, and a
    /// receiver of a known class is the member rule's. Red with the rule
    /// taken out.
    #[test]
    fn a_member_template_of_a_receiver_the_parser_cannot_see_is_not_one() {
        let dependent = "template <typename T>\nclass D\n{\npublic:\n    inline void Run()\n    {\n        obj.Go<uint32>();\n    }\n\n    T obj;\n}\n";
        assert_eq!(errors(&[("main.k", dependent)]), ["`Go`: undefined template"]);
        let keyword = dependent.replace("obj.Go<uint32>", "obj.template Go<uint32>");
        assert!(errors(&[("main.k", &keyword)]).is_empty());
        let block = "class B\n{\npublic:\n    template <typename T>\n    T Go()\n    {\n        return {};\n    }\n}\n\ninline void main()\n{\n    static if (true)\n    {\n        static B obj;\n    }\n    obj.Go<uint32>();\n}\n";
        assert_eq!(errors(&[("main.k", block)]), ["`Go`: undefined template"]);
    }

    /// `Foo<…>` is a template instance only where the parse-time table
    /// holds a template `Foo` (`Parser.hs:669-676`): not one declared later
    /// (`function-template.k::17`), not a plain function
    /// (`function-template.k::8`), not one inside a module-level `static if`
    /// block (`dependent-template.k::7`), all `kg-oracle --errors`
    /// rejections. `Do<a && b>(10)` over a variable `Do` is two comparisons
    /// and compiles. Red with the rule taken out, or with the argument's
    /// shape unchecked.
    #[test]
    fn a_name_the_parser_does_not_yet_know_as_a_template_is_not_one() {
        let later = "inline uint32 Do()\n{\n    return Foo<uint32>(10);\n}\n\ntemplate <typename T>\ninline T Foo(T x)\n{\n    return x;\n}\n";
        assert_eq!(errors(&[("main.k", later)]), ["`Foo`: undefined template"]);
        let plain = "inline void Foo()\n{\n}\n\ninline void Do()\n{\n    Foo<uint2>();\n}\n";
        assert_eq!(errors(&[("main.k", plain)]), ["`Foo`: undefined template"]);
        let block = "static if (true)\n{\n    template <typename T>\n    inline void Go()\n    {\n    }\n}\n\ninline void main()\n{\n    Go<bool>();\n}\n";
        assert_eq!(errors(&[("main.k", block)]), ["`Go`: undefined template"]);
        let unbraced = "static if (true) template <typename T> inline void Go() {}\n\ninline void main()\n{\n    Go<bool>();\n}\n";
        assert!(errors(&[("main.k", unbraced)]).is_empty());
        let before = "template <typename T>\ninline T Foo(T x)\n{\n    return x;\n}\n\ninline uint32 Do()\n{\n    return Foo<uint32>(10);\n}\n";
        assert!(errors(&[("main.k", before)]).is_empty());
    }

    /// `c.Nope<uint32>(x)` on a class with no member template `Nope`, and
    /// `c2.zz.Foo<uint32>(x)` through a member the class lacks: the
    /// parser's table rejects at the callee's head and the frontend never
    /// runs — no `zz` beside it (`scratchpad/pt/{p1,p6}.k`, `kg-oracle
    /// --errors` 2026-09-17).
    #[test]
    fn a_member_template_the_parser_cannot_find_is_its_rejection() {
        let c1 = "class C1\n{\npublic:\n    template <typename T>\n    T Foo(T x)\n    {\n        return x;\n    }\n}\n\n";
        let main = format!("{c1}inline uint32 Do(uint32 x)\n{{\n    static C1 c;\n    return c.Nope<uint32>(x);\n}}\n");
        let out: Vec<String> = crate::tests::program(&[("main.k", &main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(out, vec!["`Nope`: undefined template @ main.k:c.Nope<uint32>"]);
        let main = format!("{c1}class C2\n{{\npublic:\n    C1 c1;\n}}\n\ninline uint32 Do(uint32 x)\n{{\n    static C2 c2;\n    return c2.zz.Foo<uint32>(x);\n}}\n");
        let out: Vec<String> = crate::tests::program(&[("main.k", &main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(out, vec!["`Foo`: undefined template @ main.k:c2.zz.Foo<uint32>"]);
        let main = format!("{c1}class C2\n{{\npublic:\n    C1 c1;\n}}\n\ninline uint32 Do(uint32 x)\n{{\n    static C2 c2;\n    return c2.c1.Foo<uint32>(x);\n}}\n");
        assert_eq!(crate::tests::program(&[("main.k", &main)]), Vec::new());
        // Through an index: `c2s[1].c[0].Foo<uint32>(x)`, `c` missing
        // (`member-function-template.k::22`).
        let main = format!("{c1}class C2\n{{\npublic:\n    C1 c1;\n}}\n\ninline uint32 Do(uint32 x)\n{{\n    static C2[4] c2s;\n    return c2s[1].c[0].Foo<uint32>(x);\n}}\n");
        let out: Vec<String> = crate::tests::program(&[("main.k", &main)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect();
        assert_eq!(out, vec!["`Foo`: undefined template @ main.k:c2s[1].c[0].Foo<uint32>"]);
    }

    /// `resolveTypeName` (`Parser.hs:736-762`) reads a type name only off a
    /// `Variable`, and `typeName` asks `findSymbol` at the method: over a
    /// parameter, a template's value parameter, or a variable whose class
    /// is declared further down, `c.Foo<uint32>(1)` is "`Foo`: undefined
    /// template" though the class declares it — inside a template's body
    /// too (`kg-oracle --errors` 2026-10-10, each at the receiver; difffuzz
    /// [44], [49]). A variable whose class is declared before, a member,
    /// and the `template` keyword are fine. Red with the two arms removed.
    #[test]
    fn a_receiver_the_parser_resolves_no_type_name_for() {
        let c = "class C\n{\npublic:\n    template <typename T>\n    inline T Foo(T x)\n    {\n        return x;\n    }\n}\n\n";
        let first = |text: &str| -> Vec<String> {
            crate::tests::program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
        };
        let rejected = "`Foo`: undefined template @ main.k:c.Foo<uint32>";
        assert_eq!(first(&format!("{c}inline uint32 f(C c)\n{{\n    return c.Foo<uint32>(1);\n}}\n")), vec![rejected]);
        assert_eq!(first(&format!("{c}template <typename T>\ninline uint32 f(T c)\n{{\n    return c.Foo<uint32>(1);\n}}\n")), vec![rejected]);
        assert_eq!(
            first(&format!("{c}template <auto d>\ninline uint32 f()\n{{\n    return d.Foo<uint32>(1);\n}}\n")),
            vec!["`Foo`: undefined template @ main.k:d.Foo<uint32>"]
        );
        assert_eq!(first(&format!("inline uint32 f()\n{{\n    C c;\n    return c.Foo<uint32>(1);\n}}\n\n{c}")), vec![rejected]);
        assert_eq!(first(&format!("{c}inline uint32 f()\n{{\n    C c;\n    return c.Foo<uint32>(1);\n}}\n")), Vec::<String>::new());
        assert_eq!(first(&format!("{c}class D\n{{\n    C c;\n    inline uint32 f()\n    {{\n        return c.Foo<uint32>(1);\n    }}\n}}\n")), Vec::<String>::new());
        assert_eq!(first(&format!("{c}inline uint32 f(C c)\n{{\n    return c.template Foo<uint32>(1);\n}}\n")), Vec::<String>::new());
    }
}

#[cfg(test)]
mod find_symbol_tests {
    use crate::tests::program;

    const H1: &str = "module h1 { c1, Aa, f, K }\n\nstruct c1\n{\n    uint32 x;\n}\n\ntemplate <auto N>\nstruct Aa\n{\n    uint<N> x;\n}\n\ninline uint32 f()\n{\n    return 1;\n}\n\nconst uint32 K = 3;\n";
    const H2: &str = "module h2 { c1, N }\n\nstruct c1\n{\n    uint32 y;\n}\n\nstruct N\n{\n    uint32 y;\n}\n";

    fn first(files: &[(&str, &str)]) -> Vec<String> {
        program(files).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    /// An alias or a variable is not in the table inside itself
    /// (`parseSymbol`, `SymbolTable.hs:194-207`): `using Aa = Aa<4>;` beside
    /// the imported template is the import, not an ambiguity (difffuzz [01];
    /// the full reference compiler accepts). A later use *is* ambiguous, and
    /// so is a function *template*'s name inside its body (`static-if.k::5`).
    /// Red with the alias read from its start.
    #[test]
    fn an_alias_does_not_see_itself_but_a_later_use_does() {
        let own = "import h1\nusing Aa = Aa<4>;\n";
        assert_eq!(first(&[("main.k", own), ("h1.k", H1)]), Vec::<String>::new());
        let later = "import h1\nusing Aa = Aa<4>;\nAa x;\n";
        assert_eq!(first(&[("main.k", later), ("h1.k", H1)]).len(), 1);
        let template = "import h1\ntemplate <auto N>\ninline uint32 f()\n{\n    return f();\n}\n";
        assert_eq!(first(&[("main.k", template), ("h1.k", H1)]).len(), 1);
    }

    /// A variable's initializer does not see the variable: `uint32 f = f();`
    /// calls the imported `f`, which is what the reference's parser
    /// rewrote the name to (`parseSymbol` registers after the whole
    /// declaration). difffuzz [03] was "Undefined function" here. Red with
    /// `Round::past_own_declaration` answering `None`.
    /// A nested non-template the parse-time table holds makes `Foo<10>` a
    /// comparison (`Parser.hs:669-686`): a class's alias member, even the
    /// one being declared (`using.k::37`) or one written later
    /// (`class_`'s seeded scope, `Parser.hs:316-321`), a local before the
    /// mention, a parameter. A local declared after the mention and a
    /// struct's field are not in the table (`kg-oracle --errors`,
    /// 2026-09-27). Red with `nested_non_template_in_table` answering
    /// `false`.
    #[test]
    fn a_nested_non_template_shadows_the_template_at_parse_time() {
        let foo = "template <auto N>\nusing Foo = uint<N>;\n\n";
        let rejected = |body: &str| {
            let text = format!("{foo}{body}");
            let out = first(&[("main.k", &text)]);
            assert_eq!(out, ["`Foo`: undefined template @ main.k:Foo<10>"], "{body}");
        };
        rejected("class Bar\n{\n    using Foo = Foo<10>;\n}\n");
        rejected("class Bar\n{\n    Foo<10> x;\n    uint32 Foo;\n}\n");
        rejected("inline void f()\n{\n    uint32 Foo = 1;\n    Foo<10> x = 0;\n}\n");
        rejected("inline void f(uint32 Foo)\n{\n    Foo<10> x = 0;\n}\n");
        let later = format!("{foo}inline void f()\n{{\n    Foo<10> x = 0;\n    uint32 Foo = 1;\n}}\n");
        assert!(!first(&[("main.k", &later)]).iter().any(|m| m.contains("undefined template")));
        let field = format!("{foo}struct S\n{{\n    uint32 Foo;\n    Foo<10> x;\n}}\n");
        assert_eq!(first(&[("main.k", &field)]), Vec::<String>::new());
    }

    /// A name no file declares is in no table, so `Nope<…>` is never an
    /// instance (`Parser.hs:678-680`) — inside a template body too, which
    /// pass 9 leaves to the copies and the reference rejects at parse time
    /// whether or not it is instantiated (`kg-oracle --errors`,
    /// 2026-10-10). In a type position any list is the rejection; in an
    /// expression one argument that is no comparison's operand is, and two
    /// are not — `f(Nope<a, b>(c))` passes `Nope < a` and `b > (c)` — so
    /// that stays silent. Red with `declared_anywhere` answering `true`:
    /// the template bodies compile clean.
    #[test]
    fn a_name_declared_nowhere_takes_no_template_arguments() {
        let rejected = |text: &str, at: &str| {
            assert_eq!(first(&[("main.k", text)]), [format!("`Nope`: undefined template @ main.k:{at}")], "{text}");
        };
        rejected("template <typename T>\ninline void f()\n{\n    static Nope<T, 32> m;\n}\n", "Nope<T, 32>");
        rejected("template <typename T>\ninline T f(T x)\n{\n    return Nope<T>(x);\n}\n", "Nope<T>");
        rejected("inline void f()\n{\n    static Nope<uint8, 32> m;\n}\n", "Nope<uint8, 32>");
        let comma = "inline uint1 g(bool a, bool b)\n{\n    return 0;\n}\n\ntemplate <auto a, auto b>\ninline uint1 f(uint32 c)\n{\n    return g(Nope<a, b>(c));\n}\n";
        assert!(!first(&[("main.k", comma)]).iter().any(|m| m.contains("undefined template")));
    }

    /// Whether a name is declared anywhere is asked of the whole program
    /// once a run, and each name is answered for itself: `Inner`, declared
    /// in a class the use is not inside, is declared somewhere — the
    /// rejection is not certain, so silence — and `Nope`, asked after it, is
    /// declared nowhere. Red with every name answered as the first was.
    #[test]
    fn each_name_is_asked_of_the_program_for_itself() {
        let text = "class C\n{\n    template <auto N>\n    using Inner = uint<N>;\n}\n\n\
                    inline void f()\n{\n    static Inner<10> a;\n    static Nope<10> b;\n}\n";
        let out = first(&[("main.k", text)]);
        assert!(out.iter().any(|m| m == "`Nope`: undefined template @ main.k:Nope<10>"), "{out:?}");
        assert!(!out.iter().any(|m| m.starts_with("`Inner`: undefined template")), "{out:?}");
    }

    /// A template template parameter's default is a `templateSpecifier`
    /// (`Parser.hs:368,696-702`): an alias before it, an import's plain
    /// struct, a template declared only after it are each the parser's
    /// rejection (`template-template-parameter.k::23`; `kg-oracle --errors`,
    /// 2026-09-27); a template before it, or an import's, is the default.
    /// Red with `template_template_default_not_a_template` taken out.
    #[test]
    fn a_template_template_default_names_a_template() {
        let param = "template <template <typename> typename TT = X>\nclass Foo\n{\n}\n";
        let alias = format!("using X = uint32;\n{param}");
        assert_eq!(first(&[("main.k", &alias)]), ["`X` is not a template @ main.k:X"]);
        let later = format!("{param}template <typename T>\nstruct X\n{{\n    T x;\n}}\n");
        assert_eq!(first(&[("main.k", &later)]), ["`X` is not a template @ main.k:X"]);
        let before = format!("template <typename T>\nstruct X\n{{\n    T x;\n}}\n{param}");
        assert_eq!(first(&[("main.k", &before)]), Vec::<String>::new());
        let imported = |name: &str| format!("import h2\n{}", param.replace("= X", &format!("= {name}")));
        assert_eq!(first(&[("main.k", &imported("N")), ("h2.k", H2)]), ["`N` is not a template @ main.k:N"]);
        let h3 = "module h3 { S }\n\ntemplate <typename T>\nstruct S\n{\n    T x;\n}\n";
        let ok = "import h3\ntemplate <template <typename> typename TT = S>\nclass Foo\n{\n}\n";
        assert_eq!(first(&[("main.k", ok), ("h3.k", h3)]), Vec::<String>::new());
    }

    /// A function template instance written as a type is `typeSpecifier`'s
    /// `unexpectedSymbol` (`Parser.hs:712-721`, `isType` of a function
    /// template `False`, `Parser/Syntax.hs:695`): a variable's type and an
    /// alias template's (`function-template.k::18,19`), through an import or
    /// declared before. A class template is a type. Red with
    /// `function_template_as_a_type` taken out.
    #[test]
    fn a_function_template_instance_is_not_a_type() {
        let h = "module h { Bar, C }\n\ntemplate <typename T, T N>\ninline T Bar(T x)\n{\n    return x;\n}\n\ntemplate <typename T>\nclass C\n{\n}\n";
        let var = "import h\n\ninline void main()\n{\n    Bar<uint32, 0> x;\n}\n";
        assert_eq!(first(&[("main.k", var), ("h.k", h)]), ["`Bar`: a function template is not a type @ main.k:Bar"]);
        let alias = "import h\n\ntemplate <typename T>\nusing Bar32 = Bar<T, 0>;\n";
        assert_eq!(first(&[("main.k", alias), ("h.k", h)]), ["`Bar`: a function template is not a type @ main.k:Bar"]);
        let own = "template <typename T>\ninline T Baz(T x)\n{\n    return x;\n}\n\ninline void g(Baz<uint32> x)\n{\n}\n";
        assert_eq!(first(&[("main.k", own)]), ["`Baz`: a function template is not a type @ main.k:Baz"]);
        let class = "import h\n\ninline void main()\n{\n    static C<uint32> c;\n}\n";
        assert_eq!(first(&[("main.k", class), ("h.k", h)]), Vec::<String>::new());
    }

    #[test]
    fn a_variable_does_not_see_itself_in_its_initializer() {
        let own = "import h1\n\ninline void main()\n{\n    uint32 f = f();\n}\n";
        assert_eq!(first(&[("main.k", own), ("h1.k", H1)]), Vec::<String>::new());
    }

    /// A plain function's name is not in the table inside its own body; a
    /// function template's is (`kg-oracle --errors`, 2026-09-23). So `K`
    /// inside `inline uint32 K()` is the imported constant, and so is the
    /// `K` in a `K` variable's initializer inside that function (difffuzz
    /// [08], which needs both declarations passed over); inside a template
    /// `K` it is ambiguous, as `static-if.k::5` pins. Red with pass 9's
    /// `FN_DECL` arm removed, and with kg-ty's function skip removed.
    #[test]
    fn a_plain_function_does_not_see_itself_in_its_body() {
        let plain = "import h1\n\ninline uint32 K()\n{\n    return K;\n}\n";
        assert_eq!(first(&[("main.k", plain), ("h1.k", H1)]), Vec::<String>::new());
        let both = "import h1\n\ninline uint32 g(uint32 x)\n{\n    return x;\n}\n\ninline void K()\n{\n    const auto K = g(K);\n}\n";
        assert_eq!(first(&[("main.k", both), ("h1.k", H1)]), Vec::<String>::new());
        let template = "import h1\n\ntemplate <auto N>\ninline uint32 K()\n{\n    return K;\n}\n";
        assert_eq!(first(&[("main.k", template), ("h1.k", H1)]).len(), 1);
    }

    /// A string hole naming the file-level variable the string initializes
    /// is not that variable (`parseSymbol`): `const auto K = "{K:b}"` formats
    /// the imported `uint32`, which `b` accepts; read as the local `string`
    /// it was a false alarm (difffuzz, 2026-09-23). Red with pass 18's
    /// open-declaration check removed.
    #[test]
    fn a_string_hole_does_not_see_the_variable_it_initializes() {
        let own = "import h1\nconst auto K = \"{K:b}\";\n";
        assert_eq!(first(&[("main.k", own), ("h1.k", H1)]), Vec::<String>::new());
    }

    /// A local declared later in the block is not in the parser's table
    /// yet: `Foo x;` before `Foo Foo = …;` names the struct, and so does the
    /// later declaration's own type — the full reference compiler accepts.
    /// A regression guard: the first cut of the own-declaration rule (a
    /// per-reference ancestor walk, 2026-09-23) made each `Foo x;` "Expected
    /// a type", which the differential fuzzer caught; the session-start
    /// commit and the registration index both accept.
    #[test]
    fn a_local_declared_later_is_not_seen_before_it() {
        let text = "struct Foo\n{}\ninline void main()\n{\nFoo x;\nFoo y;\nbool pred ;\nFoo Foo = (pred) ? x : y;\n}\n";
        assert_eq!(first(&[("main.k", text)]), Vec::<String>::new());
    }

    /// The parser's template checks do not run over a twin: after
    /// `higherOrderFunctions` the twin reads `template <typename T, …,
    /// (__kg_m48) -> T T>`, a name beside one it shadows, where the source
    /// `inline void Go(() -> T T)` is fine (difffuzz, 2026-09-23). Red with
    /// the checks run over twins.
    #[test]
    fn the_parsers_checks_do_not_run_over_a_twin() {
        let text = "template <typename T>\ninline void Go(() -> T T)\n{}\n";
        assert_eq!(first(&[("main.k", text)]), Vec::<String>::new());
    }

    /// `return (Fn_T_N());` deduces from the return type exactly as
    /// `return Fn_T_N();` does: the reference's tree has no parentheses
    /// (`Parser.hs:845`), so `setTypeOfTemplateCall` sees the call
    /// (`Type.hs:1166`). difffuzz [02] was "Could not deduce all arguments".
    #[test]
    fn a_parenthesized_call_deduces_from_its_context() {
        let h = "module h { Fn }\n\ntemplate <typename T, auto N>\ninline T[N] Fn()\n{\n    T[N] r;\n    return r;\n}\n";
        for body in ["return Fn();", "return (Fn());", "return ((Fn()));"] {
            let main = format!("import h\n\ninline uint8[4] Run()\n{{\n    {body}\n}}\n");
            assert_eq!(first(&[("main.k", main.as_str()), ("h.k", h)]), Vec::<String>::new(), "{body}");
        }
    }

    /// `SymbolTable.hs:229-236`, `kg-oracle --errors` 2026-09-10 on
    /// `scratchpad/amb/a.k`, `b.k`, `g.k`: a module-scope declaration beside
    /// an import exposing the name, and two imports exposing it, are
    /// ambiguous at the use — the local definition listed first.
    #[test]
    fn a_name_two_definitions_answer_is_ambiguous_at_the_use() {
        let a = "import h1\nstruct c1\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    static c1 c;\n}\n";
        assert_eq!(
            first(&[("main.k", a), ("h1.k", H1), ("h2.k", H2)]),
            vec!["`c1`: symbol is ambiguous. It could refer to the definition at\n\t   /proj/main.k:2:1\n\tor /proj/h1.k:3:1 @ main.k:c1"]
        );
        let b = "import h1\nimport h2\n\ninline void main()\n{\n    static c1 c;\n}\n";
        assert_eq!(
            first(&[("main.k", b), ("h1.k", H1), ("h2.k", H2)]),
            vec!["`c1`: symbol is ambiguous. It could refer to the definition at\n\t   /proj/h2.k:3:1\n\tor /proj/h1.k:3:1 @ main.k:c1"]
        );
        let g = "import h1\n\ninline uint32 f()\n{\n    return 2;\n}\n\ninline uint32 main()\n{\n    return f();\n}\n";
        assert_eq!(first(&[("main.k", g), ("h1.k", H1)]).len(), 1);
        // A template's own name inside its body resolves at module scope
        // (`test/syntax/static-if.k::5,6`): `Aa` beside the import's `Aa`.
        let t = "import h1\n\ntemplate <auto N>\ninline auto Aa()\n{\n    static if (N)\n        return Aa<false>();\n    else\n        return 0;\n}\n";
        assert_eq!(first(&[("main.k", t), ("h1.k", H1)]).len(), 1);
    }

    /// `c.k`, `e.k`, `k.k`, `j.k` of the same probe: an aliased import, a
    /// local, a class member and a block-scoped local are each unique; and a
    /// module-scope declaration written after the use is a forward reference
    /// the parse-time stack does not hold.
    #[test]
    fn a_nested_or_aliased_or_later_definition_is_unique() {
        let c = "import h1 as A\nstruct c1\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    static c1 c;\n    static A::c1 d;\n}\n";
        assert_eq!(first(&[("main.k", c), ("h1.k", H1)]), Vec::<String>::new());
        let e = "import h1\n\ninline uint32 main()\n{\n    uint32 f = 2;\n    uint32 K = 4;\n    return f + K;\n}\n";
        // No ambiguity — and `h1`, whose `f` and `K` the locals shadow, is
        // never probed by name (`findInScopes`, `SymbolTable.hs:256-259`):
        // the reference compiler warns the import redundant even under its
        // default flags (`kanagawa`, 2026-09-25).
        assert_eq!(first(&[("main.k", e), ("h1.k", H1)]), vec!["The import is redundant @ main.k:import"]);
        let k = "import h1\n\nclass Foo\n{\n    uint32 K;\npublic:\n    uint32 Go()\n    {\n        return K;\n    }\n}\ninline void main()\n{\n    static Foo foo;\n    foo.Go();\n}\n";
        // Unique too, and the member answers from the class's scope, so the
        // import is never probed by name either: the reference's tree
        // wears the redundant-import seed (`kg-oracle --tree`, 2026-09-27).
        // Corrected from "no diagnostic", which pinned the bare-name probe
        // `crate::imports` made of a class member before 2026-09-27.
        assert_eq!(first(&[("main.k", k), ("h1.k", H1)]), vec!["The import is redundant @ main.k:import"]);
        let fwd = "import h1\n\ninline void main()\n{\n    static c1 c;\n}\n\nstruct c1\n{\n    uint32 x;\n}\n";
        assert_eq!(first(&[("main.k", fwd), ("h1.k", H1)]), Vec::<String>::new());
    }

    /// `SymbolTable.hs:243-250`, `d.k` of the probe: `N` resolves to `h2`'s
    /// struct while `import h1 as N` carries the same name — reported at the
    /// import, naming the symbol's definition.
    #[test]
    fn an_import_alias_that_a_symbol_also_answers_to_is_a_conflict_at_the_import() {
        let d = "import h1 as N\nimport h2\n\ninline void main()\n{\n    static N::Aa<1> x;\n}\n";
        assert_eq!(
            first(&[("main.k", d), ("h1.k", H1), ("h2.k", H2)]),
            vec!["The import qualifier 'N' conflicts with the name of symbol defined at /proj/h2.k:8:1 @ main.k:import"]
        );
        // The same alias with no such symbol anywhere is what an alias is for.
        let ok = "import h1 as N\n\ninline void main()\n{\n    static N::Aa<1> x;\n}\n";
        assert_eq!(first(&[("main.k", ok), ("h1.k", H1)]), Vec::<String>::new());
    }

    /// The conflict needs the nested symbol in the parse-time table when
    /// the qualifier is read (difffuzz 2026-10-10 [01]; every case
    /// `kg-oracle --errors`): a local or a parameter whose own type
    /// mentions the alias, a local declared after the mention, a struct's
    /// field and a class's later member function are not there, so the
    /// reference accepts; a class's variable written later is (the class's
    /// scope is seeded), and past a local not yet declared the lookup finds
    /// the one an outer block declared.
    #[test]
    fn an_import_alias_conflicts_only_with_a_symbol_the_table_holds_by_then() {
        let fine = [
            "import h1 as N\n\ninline void main()\n{\n    N::Aa<1> N;\n}\n",
            "import h1 as N\n\ninline void main()\n{\n    N::Aa<1> x;\n    uint32 N;\n}\n",
            "import h1 as N\n\ninline void main(N::Aa<1> N)\n{\n}\n",
            "import h1 as N\n\nstruct S\n{\n    uint32 N;\n    N::Aa<1> y;\n}\n",
            "import h1 as N\n\nclass C\n{\n    void f()\n    {\n        N::Aa<1> x;\n    }\n    void N()\n    {\n    }\n}\n",
        ];
        for text in fine {
            assert_eq!(first(&[("main.k", text), ("h1.k", H1)]), Vec::<String>::new(), "{text}");
        }
        let member = "import h1 as N\n\nclass C\n{\n    void f()\n    {\n        N::Aa<1> x;\n    }\n    uint32 N;\n}\n";
        assert_eq!(
            first(&[("main.k", member), ("h1.k", H1)]),
            vec!["The import qualifier 'N' conflicts with the name of symbol defined at /proj/main.k:9:5 @ main.k:import"]
        );
        let outer = "import h1 as N\n\ninline void main()\n{\n    uint32 N;\n    {\n        N::Aa<1> x;\n        uint32 N;\n    }\n}\n";
        assert_eq!(
            first(&[("main.k", outer), ("h1.k", H1)]),
            vec!["The import qualifier 'N' conflicts with the name of symbol defined at /proj/main.k:5:5 @ main.k:import"]
        );
    }

    /// A module-scope declaration the table does not hold yet leaves the
    /// lookup to the imports (`findInSymbols`, `SymbolTable.hs:256-258`):
    /// two that expose the name are ambiguous inside the declaration
    /// itself and before it (difffuzz 2026-10-10 [42]: `c1<>` inside
    /// `inline void c1()`), with a `module` header or without. Under a
    /// header, one the table holds is unique — its key is qualified — and
    /// an import aliased to its name conflicts. Each `kg-oracle --errors`,
    /// 2026-10-10.
    #[test]
    fn a_module_scope_name_not_yet_in_the_table_is_the_imports_alone() {
        let ambiguous = "`c1`: symbol is ambiguous. It could refer to the definition at\n\t   /proj/h2.k:3:1\n\tor /proj/h1.k:3:1 @ main.k:c1";
        let inside = "import h1\nimport h2\n\ninline void c1()\n{\n    static c1 c;\n}\n";
        assert_eq!(first(&[("main.k", inside), ("h1.k", H1), ("h2.k", H2)]), vec![ambiguous]);
        let before = "module m { main }\n\nimport h1\nimport h2\n\ninline void main()\n{\n    static c1 c;\n}\n\nstruct c1\n{\n    uint32 z;\n}\n";
        assert_eq!(first(&[("main.k", before), ("h1.k", H1), ("h2.k", H2)]), vec![ambiguous]);
        let held = "module m { main }\n\nimport h1\nimport h2\n\nstruct c1\n{\n    uint32 z;\n}\n\ninline void main()\n{\n    static c1 c;\n}\n";
        assert_eq!(first(&[("main.k", held), ("h1.k", H1), ("h2.k", H2)]), Vec::<String>::new());
        let alias = "module m { main }\n\nimport h1 as N\n\nstruct N\n{\n    uint32 z;\n}\n\ninline void main()\n{\n    static N::Aa<1> x;\n}\n";
        assert_eq!(
            first(&[("main.k", alias), ("h1.k", H1)]),
            vec!["The import qualifier 'N' conflicts with the name of symbol defined at /proj/main.k:5:1 @ main.k:import"]
        );
    }

    /// `x<>` over a name the table holds as no template is `x <` and then
    /// no operand (`Parser.hs:669-686`, the `otherwise` arm): a parse error
    /// in an expression as in a type (difffuzz 2026-10-10 [33]: `x<> ;`
    /// inside `inline void x()`, `kg-oracle --errors` "unexpected '<'").
    /// A module-scope template's parameter is keyed by its bare name
    /// without a `module` header, so an import exposing the name makes a
    /// use ambiguous, the parameter first (`findInScopes`' `(x :) <$>
    /// findInImports x`, `SymbolTable.hs:256`; `kg-oracle --errors`
    /// 2026-10-10, difffuzz [23]); a template nested in a class, or a
    /// header, qualifies it. Red with the parameter taken as unique.
    #[test]
    fn a_module_scope_templates_parameter_meets_the_imports() {
        let t = "import h1\n\ntemplate <auto K>\ninline uint32 T()\n{\n    return K;\n}\n";
        assert_eq!(
            first(&[("main.k", t), ("h1.k", H1)]),
            vec!["`K`: symbol is ambiguous. It could refer to the definition at\n\t   /proj/main.k:3:11\n\tor /proj/h1.k:19:1 @ main.k:K"]
        );
        let nested = "import h1\n\nclass C\n{\n    template <auto K>\n    uint32 T()\n    {\n        return K;\n    }\n}\n";
        assert!(!first(&[("main.k", nested), ("h1.k", H1)]).iter().any(|m| m.contains("ambiguous")));
        let header = "module m { T }\n\nimport h1\n\ntemplate <auto K>\ninline uint32 T()\n{\n    return K;\n}\n";
        assert!(!first(&[("main.k", header), ("h1.k", H1)]).iter().any(|m| m.contains("ambiguous")));
    }

    #[test]
    fn a_local_being_declared_is_not_what_its_own_type_names() {
        // `static x<int32, 10> x;` inside `inline void x()`: the local is
        // not in the table inside itself, so `x` reads the function, no
        // template, and the `<` fails the parse (difffuzz 2026-10-10, the
        // re-run's `type-check.k::22` mutant; `kg-oracle --errors`). Red
        // with the local taken as found.
        let own = "inline void x()\n{\n    static x<int32, 10> x;\n}\n";
        assert_eq!(first(&[("main.k", own)]), vec!["`x`: undefined template @ main.k:x<int32, 10>"]);
    }

    #[test]
    fn an_empty_argument_list_on_no_template_is_rejected() {
        let own = "inline void x()\n{\n    x<> ;\n}\n";
        assert_eq!(first(&[("main.k", own)]), vec!["`x`: undefined template @ main.k:x<>"]);
        let template = "template <typename T = uint8>\ninline void y()\n{\n}\n\ninline void x()\n{\n    y<>();\n}\n";
        assert_eq!(first(&[("main.k", template)]), Vec::<String>::new());
    }
}

#[cfg(test)]
mod member_and_export_tests {
    use crate::tests::{accepts, errors, program};

    fn first(files: &[(&str, &str)]) -> Vec<String> {
        program(files).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    /// `Parser.hs:219-223`: an export-list name nothing declares.
    #[test]
    fn an_exported_name_nothing_declares_is_undefined() {
        assert_eq!(
            first(&[("main.k", "module m { x, Foo }\n\nstruct Foo\n{\n    uint32 a;\n}\n")]),
            vec!["`x`: undefined symbol @ main.k:x"]
        );
        accepts("module m { Foo }\n\nstruct Foo\n{\n    uint32 a;\n}\n");
    }

    /// `findSymbol`'s two answers are `Nothing` too (`SymbolTable.hs:
    /// 231-237`): an exported name the module declares *and* an unaliased
    /// import exposes is undefined (`imports.k::63`), and ambiguous at the
    /// same name — both, in that order, as `kg-oracle --errors` prints them
    /// (2026-09-27; the ambiguity is `ModuleGraph::exposed_ambiguities`');
    /// an aliased import, or the import's alone, is one answer. Red with the
    /// count read as "any answer".
    #[test]
    fn an_exported_name_two_declarations_answer_is_undefined() {
        let h = "module h { memory }\n\nusing memory = uint8;\n";
        let both = "module m { memory }\n\nimport h\n\nusing memory = uint32;\n";
        assert_eq!(
            first(&[("main.k", both), ("h.k", h)]),
            vec![
                "`memory`: undefined symbol @ main.k:memory",
                "`memory`: symbol is ambiguous. It could refer to the definition at\n\t   /proj/main.k:5:1\n\tor /proj/h.k:3:1 @ main.k:memory",
            ]
        );
        let aliased = both.replace("import h", "import h as H");
        assert!(!first(&[("main.k", &aliased), ("h.k", h)]).iter().any(|m| m.contains("undefined symbol")));
        let reexported = "module m { memory }\n\nimport h\n";
        assert_eq!(first(&[("main.k", reexported), ("h.k", h)]), Vec::<String>::new());
        // A second file of the *same* module name declaring an equal
        // `memory` is one answer to `nub`, which ignores notes
        // (`Parser/Syntax.hs:593-595`; the library's device `dsp.k` files):
        // one namespace, counted once.
        let twin = "module m { memory }\n\nusing memory = uint32;\n";
        let same = "module m { memory }\n\nimport m\n\nusing memory = uint32;\n";
        assert_eq!(first(&[("main.k", same), ("m.k", twin)]), Vec::<String>::new());
    }

    /// `Parser.hs:551-556`: a function type's named parameters share one
    /// quasi scope, so a repeated name is a redefinition at the second.
    #[test]
    fn a_function_types_parameter_names_are_symbols() {
        let e = errors("inline void main()\n{\n    const (uint32 x, uint32 x) -> void f = main;\n}\n");
        assert!(e.iter().any(|m| m.starts_with("`x`: symbol redefinition")), "{e:?}");
        accepts("inline void Go(uint32 a)\n{\n}\n\ninline void Go2(uint32 a, uint32 b)\n{\n}\n\ninline void main()\n{\n    const (uint32 x, uint32 y) -> void f = Go2;\n    const (uint32 x) -> void g = Go;\n}\n");
    }
}
