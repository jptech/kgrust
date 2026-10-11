//! `updateNeeded` and `checkRedundantImports` — the one import diagnostic that
//! is not decided by the loader.
//!
//! Three of the four import diagnostics are answered while the program is being
//! loaded, because the loader is the thing that knows: a module that is not on
//! the search path (`Parser/IO.hs:108`), an import cycle (`Parser/IO.hs:90`) and
//! a file whose `module` declaration disagrees with the path it was found at
//! (`Parser.hs:203-204`) are all failures of `parseFile'` itself, and
//! [`crate::program::load`] reports them where the reference does — at the
//! `import` keyword of the importing file (`parseFileFailure offset err`,
//! `Parser.hs:201`).
//!
//! The fourth is different in kind. `redundant-import` is not decided by
//! *resolving* the import; it is decided by whether anything in the file ever
//! *used* it. The reference gets that for free because `findSymbol` runs during
//! parsing and mutates the symbol table as a side effect: every lookup that a
//! given `import` satisfied flips that import's `importNeeded` to `Needed`
//! (`updateNeeded`, `SymbolTable.hs:263-270`), and whatever is still `Redundant`
//! when the file ends gets one warning (`Parser.hs:234-241`). Here resolution is
//! a pure query, so the side effect has to be replayed deliberately — that is
//! [`mark_used_imports`], and [`redundant_import_warnings`] is the fold that
//! reads the result.
//!
//! Spec: `docs/spec/resolve.md` §2.9 (which enumerates the four conditions and
//! the complete oracle set), §1.3, §1.4, §8.
//!
//! # One deliberate divergence
//!
//! The reference prints warnings only from `codegen`, i.e. **after** both the
//! parse gate and the desugar gate have passed (`Main.hs:73-75,84-93`), so a
//! program with any error prints no warnings at all. These go into
//! [`Program::diagnostics`](crate::Program) instead, which
//! [`Db::check`](crate::Db::check) returns even when it stops at the parse gate.
//! No conformance assertion can see the difference — `syntax-test.py:113-126`
//! fails a warning assertion on any non-zero error code before it looks at the
//! warning at all — and a language server that shows one file's unused import
//! while another file is mid-edit is the better behaviour.
//!
//! # Why this over-approximates *uses*, deliberately
//!
//! Marking one import `Needed` too often costs a warning that the reference
//! emits — a false negative, which the harness scores `Undecidable`. Marking one
//! too rarely invents a warning the reference does not emit — a false positive,
//! which breaks a `warning:0` assertion. Those are not symmetric (see the
//! crate docs), so where the exact `findSymbol` key is not reconstructible from
//! the tree alone this probes a *superset* of the reference's keys and accepts
//! the missed warning.
//!
//! Concretely, the reference probes the imports with `getQualifiedName` of the
//! reference being resolved (`SymbolTable.hs:263,272`), which is
//! `getQualifier ++ [getName]` — the **written `A::B::` prefix**, not the scope
//! qualifier (`Parser/Syntax.hs:746-764`). That is reconstructible: it is the
//! `PATH` node's segments. The one case that is not is `findInScopes`
//! (`SymbolTable.hs:256-259`), which probes the *found declaration's* qualified
//! name instead of the written one — reproducing it exactly would need a live
//! [`kg_resolve::ScopeStack`] replay, so both keys are probed instead.

use kg_base::{FxHashMap, Interner, Symbol, SymbolPath};
use kg_diag::Diagnostic;
use kg_resolve::module::ModuleGraph;
use kg_resolve::{ExportItem, ItemTree, ModuleSource, redundant_import_warning};
use kg_syntax::ast;
use kg_syntax::{AstNode, SyntaxKind, SyntaxNode};

/// Replays `updateNeeded` (`SymbolTable.hs:263-270`) for one module: every name
/// the file mentions is looked up in the module's imports, and every import that
/// supplied a *residual* hit is flipped to `Needed`.
///
/// "Residual" is the load-bearing word and it is already
/// [`ModuleGraph::import_hits`]'s job: the reference subtracts every
/// earlier-iterated import's hits while walking the import list in **reverse**
/// source order, so when two imports offer the same declaration only the one
/// written **last** is credited (`zipWith (foldl' (\\)) foundImported (inits
/// foundImported)`). That is the whole content of the oracle pair
/// `test/syntax/imports.k:503-512` (warns) versus `:514-523` (does not) — the
/// same two imports, swapped.
///
/// Must run after every participating interface is built, because
/// `find_in_import` reads `visibleSymbols` of the *imported* module.
pub(crate) fn mark_used_imports(
    graph: &mut ModuleGraph,
    module: ModuleSource,
    root: &ast::SourceFile,
    tree: &ItemTree,
    interner: &Interner,
    is_type: &dyn Fn(kg_resolve::DefId) -> bool,
) {
    // Nothing left to prove: `updateImports`' three seeds already justified
    // every import, so no lookup can change the answer and the CST walk is pure
    // cost. This is the common case in `library/`, where a module that re-exposes
    // what it imports seeds them all `Needed` (`SymbolTable.hs:104`).
    if graph.redundant_imports(module).next().is_none() {
        return;
    }
    let mut keys = probes(root, tree, interner);
    keys.extend(member_probes(graph, module, root, tree, interner, is_type));
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        let used = graph.import_hits(module, &key).used;
        if !used.is_empty() {
            graph.mark_needed(module, &used);
        }
    }
}

/// The keys a method call hands to `findInImports`: `obj.Go()` is
/// `methodSpecifier`, which resolves `obj`'s type name `T` and looks the
/// method up as `T::Go` — `prefixedScopedNameAndSymbol scope unresolved t`,
/// whose `findSymbol` key is `getQualifier t ++ [getName t, Go]`
/// (`Parser.hs:729-735,652-655`) — and each member of the receiver's chain
/// the same way, `a.b.Go()` probing `T(a)::b` (`resolveTypeName`,
/// `Parser.hs:739-742`). Never the bare member name. A field access that
/// is no method call's receiver looks nothing up at all: `access` parses
/// its member with `identifier` (`Parser.hs:811-819`).
///
/// `T` is the receiver's declared type, which this pure pass cannot name,
/// so every key an import exposes that ends in the member under a type — a
/// prefix the same map files as a type declaration — is probed: a superset
/// of the reference's hits (the module docs' direction), which no longer
/// credits an import for a top-level `Go` it exposes, nor for a function's
/// parameter `Do` (`Exec(() -> void Do)`, whose key is `Exec::Do`). That credit wrapped a
/// program's seed in one warning too few (`static-if.k::48`: `foo32.Go()`
/// beside `helper.static_if`'s free `Go`), which the walk emits.
fn member_probes(
    graph: &ModuleGraph,
    module: ModuleSource,
    root: &ast::SourceFile,
    tree: &ItemTree,
    interner: &Interner,
    is_type: &dyn Fn(kg_resolve::DefId) -> bool,
) -> Vec<SymbolPath> {
    let mut out = Vec::new();
    // The members whose receiver's type this file does not answer.
    let mut members: Vec<Symbol> = Vec::new();
    for field in root.syntax().descendants().filter(|n| n.kind() == SyntaxKind::FIELD_EXPR && in_method_chain(n)) {
        let Some(member) = field.children().filter(|c| c.kind() == SyntaxKind::NAME_REF).last().and_then(|n| ident(&n, interner)) else {
            continue;
        };
        match receiver_type_here(&field, tree, interner) {
            Some(owner) => out.push(owner.concat(&SymbolPath::single(member))),
            None => members.push(member),
        }
    }
    members.sort_unstable();
    members.dedup();
    if members.is_empty() {
        return out;
    }
    for (_, target) in graph.resolved_imports(module) {
        let Some(interface) = graph.interface(target) else { continue };
        for syms in interface.visible.values() {
            for key in syms.keys() {
                let segments = key.segments();
                let owner = SymbolPath::new(segments[..segments.len().saturating_sub(1)].iter().copied());
                if segments.len() >= 3
                    && segments.last().is_some_and(|l| members.binary_search(l).is_ok())
                    && syms.get(&owner).is_some_and(|d| is_type(*d))
                {
                    out.push(SymbolPath::new(segments[1..].iter().copied()));
                }
            }
        }
    }
    out
}

/// The qualified name of the type `resolveTypeName` finds for a method
/// call's receiver (`Parser.hs:736-757`), where this file alone answers: the
/// receiver is a bare name, indexed or not, declared by the innermost block,
/// parameter list, class body or file that declares it, as a variable of a
/// type named by one segment — `T`, `T<…>`, or either as `[N]`
/// (`typeName (Array _ x _)`) — which this file declares at top level, a
/// template's target included, and not as an alias (`typeName` follows
/// one). That
/// type is `makeQualified` by its declaration's scope (`Parser.hs:686-687`,
/// `Desugar.hs:190-191`), the item's own key. Anything else — a member of a
/// member, a qualified type, a name a lambda, a loop or a template declares
/// — is `None`, and the caller probes the superset.
fn receiver_type_here(field: &SyntaxNode, tree: &ItemTree, interner: &Interner) -> Option<SymbolPath> {
    let mut base = field.children().next()?;
    while base.kind() == SyntaxKind::INDEX_EXPR {
        base = base.children().next()?;
    }
    let path = ast::Path::cast(base)?;
    let mut segments = path.path_segments();
    let segment = segments.next()?;
    if segments.next().is_some() || segment.template_arg_list().is_some() {
        return None;
    }
    let name = segment.name_ref()?.ident_token()?;
    let decl = variable_declaring(path.syntax(), name.text())?;
    let mut ty = decl.children().find(|c| matches!(c.kind(), SyntaxKind::PATH_TYPE | SyntaxKind::ARRAY_TYPE))?;
    if ty.kind() == SyntaxKind::ARRAY_TYPE {
        ty = ty.children().find(|c| c.kind() == SyntaxKind::PATH_TYPE)?;
    }
    let ty_path = ty.children().find_map(ast::Path::cast)?;
    let mut ty_segments = ty_path.path_segments();
    let ty_segment = ty_segments.next()?;
    if ty_segments.next().is_some() {
        return None;
    }
    // An instance's name is its template's: `getName (TemplateInstanceF a
    // _) = getName a` (`Parser/Syntax.hs:731-732`), so `Foo<uint32>`
    // prefixes the key as `Foo` does (`extern-class.k::26`).
    let ty_name = interner.intern(ty_segment.name_ref()?.ident_token()?.text());
    let top_level = |i: &kg_resolve::Item| match i.parent {
        None => true,
        Some(p) => tree.item(p).is_some_and(|t| t.kind == kg_resolve::def::DefKind::Template && t.parent.is_none()),
    };
    let mut declared = tree.items.iter().filter(|i| {
        top_level(i) && i.kind.is_type() && i.qualified_name.last() == Some(ty_name) && i.qualified_name.segments().len() <= 2
    });
    let item = declared.next()?;
    if declared.next().is_some() || matches!(item.kind, kg_resolve::def::DefKind::Alias) {
        return None;
    }
    Some(item.qualified_name.clone())
}

/// The `VAR_DECL` or `PARAM` that declares the variable `name` a use at `at`
/// reads: the innermost block that declares it before the use, a function's
/// parameter list, a class or struct body, the file — `findInScopes`' order
/// (`SymbolTable.hs:255-258`). `None` where the innermost declaration is not
/// a variable, or where a lambda, a loop or a template in between might
/// declare it.
fn variable_declaring(at: &SyntaxNode, name: &str) -> Option<SyntaxNode> {
    let start = at.text_range().start();
    for ancestor in at.ancestors().skip(1) {
        let candidates: Vec<SyntaxNode> = match ancestor.kind() {
            SyntaxKind::BLOCK_STMT => {
                ancestor.children().take_while(|c| c.text_range().end() <= start).filter(|c| declares_any(c, name)).collect()
            }
            SyntaxKind::FN_DECL => ancestor
                .children()
                .filter(|c| c.kind() == SyntaxKind::PARAM_LIST)
                .flat_map(|l| l.children())
                .filter(|p| p.kind() == SyntaxKind::PARAM && names(p, name))
                .collect(),
            SyntaxKind::DECL_LIST | SyntaxKind::SOURCE_FILE => {
                ancestor.children().filter(|c| declares_any(c, name)).collect()
            }
            SyntaxKind::LAMBDA_EXPR
            | SyntaxKind::FOR_STMT
            | SyntaxKind::UNROLLED_FOR_STMT
            | SyntaxKind::STATIC_FOR_STMT
            | SyntaxKind::TEMPLATE_DECL => {
                if ancestor.descendants().any(|d| d.kind() == SyntaxKind::NAME && d.first_token().is_some_and(|t| t.text() == name)
                    && !d.ancestors().any(|a| a.kind() == SyntaxKind::BLOCK_STMT && a.text_range().contains_range(at.text_range())))
                {
                    return None;
                }
                continue;
            }
            _ => continue,
        };
        match candidates.as_slice() {
            [] => continue,
            [one] if matches!(one.kind(), SyntaxKind::VAR_DECL | SyntaxKind::PARAM) => return Some(one.clone()),
            _ => return None,
        }
    }
    None
}

/// [`declares`], variables included, and a declaration under a `static if`
/// read as unknown.
fn declares_any(node: &SyntaxNode, name: &str) -> bool {
    declares(node, name) || (node.kind() == SyntaxKind::STATIC_IF_STMT && node.descendants().any(|d| d.kind() == SyntaxKind::NAME && d.first_token().is_some_and(|t| t.text() == name)))
}

/// Whether a `FIELD_EXPR` is a method call's callee, or a link of the
/// receiver chain under one — through member accesses and indexing,
/// `resolveTypeName`'s `MemberAccess` and `ArrayAccessLValue` arms
/// (`Parser.hs:736-742`).
fn in_method_chain(field: &SyntaxNode) -> bool {
    let mut at = field.clone();
    loop {
        let Some(parent) = at.parent() else { return false };
        let first = parent.children().next();
        match parent.kind() {
            SyntaxKind::CALL_EXPR => return first.as_ref() == Some(&at),
            SyntaxKind::FIELD_EXPR | SyntaxKind::INDEX_EXPR if first.as_ref() == Some(&at) => at = parent,
            _ => return false,
        }
    }
}

/// `checkRedundantImports` (`Parser.hs:234-241`) — one warning per import still
/// `Redundant` at end of file.
///
/// The three ways an import is *seeded* `Needed` before any name is resolved —
/// the importing module re-exposes it as a whole module, it appears in an
/// exposed module difference, or the imported module itself contains an
/// `export T;` or a `[[reset]]` function (`SymbolTable.hs:104-106,149`) — are
/// applied by [`ModuleGraph::add_import`] when the graph is built, and so are
/// already reflected here. So is the implicit base import, which
/// `parseProgram` adds with `Needed` (`Parser/IO.hs:77`) — which is why `base`
/// and `.cmdargs` never warn.
pub(crate) fn redundant_import_warnings(
    graph: &ModuleGraph,
    module: ModuleSource,
) -> Vec<Diagnostic> {
    graph.redundant_imports(module).map(|i| redundant_import_warning(i.range)).collect()
}

/// Every key this file hands to `findInImports`, over-approximated — see the
/// module docs for which direction the approximation runs in and why.
fn probes(root: &ast::SourceFile, tree: &ItemTree, interner: &Interner) -> Vec<SymbolPath> {
    let declared = declared_by_last_component(tree);
    let mut scopes = ScopeNames::default();
    let mut out = Vec::new();

    // `checkSymbols` (`Parser.hs:216-223`) — each exposed symbol is looked up
    // before the file ends, so an `import` that supplies it is `Needed`. This is
    // the whole reason `test/syntax/imports/a1/a3.k` — which imports `a1.a1`,
    // re-exposes its `a1` and mentions it nowhere else — does *not* warn.
    if let Some(header) = &tree.module_header {
        for item in &header.exports {
            if let ExportItem::Symbol(name) = item {
                push_name(&mut out, &declared, *name);
            }
        }
    }

    for node in root.syntax().descendants() {
        // A string's holes are ordinary expressions in the tree, and the
        // reference resolves their names like any other: `"{opt::stall}"` is
        // what justifies `import .options as opt` in
        // `logic/context_saver_depth.k`.
        if let Some(path) = ast::Path::cast(node.clone()) {
            push_path(&mut out, &declared, &mut scopes, &path, interner);
        } else if node.kind() == SyntaxKind::NAME_REF
            && !in_path(&node)
            && !node.parent().is_some_and(|p| matches!(p.kind(), SyntaxKind::FIELD_EXPR | SyntaxKind::DESIGNATOR))
        {
            // A name that is not a `PATH` segment, a member (a member is
            // [`member_probes`]') nor a designator — `designator` parses its
            // name with `identifier` and looks nothing up (`Parser.hs:863`):
            // an explicit lambda capture, which goes through `parseSymbol`'s
            // lookup (`Parser.hs:441`), and anything else this does not
            // recognise. The bare name is probed — over-approximating, per
            // the module docs.
            if let Some(name) = ident(&node, interner) {
                push_name(&mut out, &declared, name);
            }
        }
    }

    out.sort_unstable();
    out.dedup();
    out
}

/// `prefixedScopedNameAndSymbol` (`Parser.hs:647-680`) resolves a `A::B::c`
/// chain one component at a time, calling `findSymbol` at each step with the
/// prefix accumulated so far, so all three of `[A]`, `[A,B]` and `[A,B,c]` reach
/// `findInImports`. The alias case is why the whole chain matters:
/// `import a1.a5 as A5` is credited only by the key `[A5, N]`, which
/// `find_in_import` strips the alias off before prepending the namespace
/// (`SymbolTable.hs:276-278`) — oracle `test/syntax/imports.k:828-840`.
fn push_path(
    out: &mut Vec<SymbolPath>,
    declared: &FxHashMap<Symbol, Vec<SymbolPath>>,
    scopes: &mut ScopeNames,
    path: &ast::Path,
    interner: &Interner,
) {
    let segments: Vec<Symbol> = path
        .path_segments()
        .filter_map(|s| s.name_ref())
        .filter_map(|n| n.ident_token())
        .map(|t| interner.intern(t.text()))
        .collect();

    // A bare name an enclosing block declared before this use is found in
    // `findInScopes`, which probes the imports with the *found* declaration's
    // qualified name — `["main", "$", "Foo"]`, which no import exposes — and
    // never with the written one (`SymbolTable.hs:256-259`). So `Foo()` over a
    // local `Foo` leaves `import helper.local_functions`, which also exports
    // one, redundant (`local-functions.k::21-26`, the recorded walks' warning).
    //
    // The same holds for the *first* component of a longer path: `Bar` of
    // `Bar::Zero` has no qualifier and goes through `findInScopes` too, and
    // only the later components' keys — `[Bar, Zero]`, the written prefix
    // (`prefixedScopedNameAndSymbol`, `Parser.hs:652-655`), which is no
    // qualifier-less lookup — reach `findInImports` as written. So a class's
    // own `enum Bar` named `Bar::Zero` does not credit an import exposing a
    // `Bar` (`enum.k::51`, the recorded walk's seed).
    let local = match (segments.first(), path.path_segments().next()) {
        (Some(first), Some(seg)) => declared_earlier_in_scope(seg.syntax(), &interner.resolve(*first), scopes),
        _ => false,
    };
    for k in (if local { 2 } else { 1 })..=segments.len() {
        out.push(SymbolPath::new(segments[..k].iter().copied()));
    }
    if let Some(&first) = segments.first() {
        out.extend(declared.get(&first).into_iter().flatten().cloned());
    }
}

/// Whether `name` is declared, before `at`, in a block enclosing it or as a
/// parameter of a function enclosing it, or earlier in an enclosing class
/// body — `findInScopes`' hit (`SymbolTable.hs:256`), whose
/// `symbolsInScope` gain a declaration as the parser meets it. Anything
/// this does not recognise stays an import probe, the over-approximation
/// of the module docs.
///
/// A class or struct body is a scope too: `c1 c;` after the member `struct
/// c1` finds the member, and the imports are probed with `Foo::c1`, which
/// none exposes (`imports.k::13`, beside `import a1.b2.c1.c1`). A member
/// declared after the use is left a probe — the direction the module docs
/// choose.
fn declared_earlier_in_scope(at: &SyntaxNode, name: &str, scopes: &mut ScopeNames) -> bool {
    let start = at.text_range().start();
    at.ancestors().any(|ancestor| {
        matches!(ancestor.kind(), SyntaxKind::BLOCK_STMT | SyntaxKind::FN_DECL | SyntaxKind::DECL_LIST)
            && scopes.of(&ancestor).get(name).is_some_and(|&end| end <= start)
    })
}

/// What each scope [`declared_earlier_in_scope`] reads declares, read once
/// per scope: of a block or a class body, each name with the end of the
/// first statement declaring it — the statements are in text order, so
/// "a statement ending before the use declares it" is "the first one
/// does" — and of a function, its parameters' names, which hold from its
/// start. Scanning a scope's earlier statements once per written path made
/// the probes quadratic in a scope's length: a twentieth of a keystroke on
/// `risc-v.k`, whose classes are long (2026-10-10).
#[derive(Default)]
struct ScopeNames(FxHashMap<SyntaxNode, FxHashMap<kg_base::SmolStr, kg_base::TextSize>>);

impl ScopeNames {
    fn of(&mut self, scope: &SyntaxNode) -> &FxHashMap<kg_base::SmolStr, kg_base::TextSize> {
        self.0.entry(scope.clone()).or_insert_with(|| {
            let mut names: FxHashMap<kg_base::SmolStr, kg_base::TextSize> = FxHashMap::default();
            if scope.kind() == SyntaxKind::FN_DECL {
                let lists = scope.children().filter(|c| c.kind() == SyntaxKind::PARAM_LIST);
                for param in lists.flat_map(|l| l.children()).filter(|p| p.kind() == SyntaxKind::PARAM) {
                    own_names(&param, &mut |n| {
                        names.entry(n.into()).or_insert(0.into());
                    });
                }
            } else {
                for child in scope.children() {
                    let end = child.text_range().end();
                    declared_names(&child, &mut |n| {
                        names.entry(n.into()).or_insert(end);
                    });
                }
            }
            names
        })
    }
}

/// Every name a block's statement declares: a function, variable or type,
/// or a template of one.
fn declared_names(node: &SyntaxNode, each: &mut dyn FnMut(&str)) {
    match node.kind() {
        SyntaxKind::FN_DECL
        | SyntaxKind::VAR_DECL
        | SyntaxKind::STRUCT_DECL
        | SyntaxKind::CLASS_DECL
        | SyntaxKind::UNION_DECL
        | SyntaxKind::ENUM_DECL => own_names(node, each),
        SyntaxKind::TEMPLATE_DECL => {
            for c in node.children() {
                declared_names(&c, each);
            }
        }
        _ => {}
    }
}

/// What each of `node`'s own `NAME` children spells.
fn own_names(node: &SyntaxNode, each: &mut dyn FnMut(&str)) {
    for n in node.children().filter(|c| c.kind() == SyntaxKind::NAME) {
        if let Some(t) = n.first_token() {
            each(t.text());
        }
    }
}

/// Whether a block's statement declares `name` ([`declared_names`]).
fn declares(node: &SyntaxNode, name: &str) -> bool {
    let mut found = false;
    declared_names(node, &mut |n| found |= n == name);
    found
}

/// Whether one of `node`'s own `NAME` children spells `name`.
fn names(node: &SyntaxNode, name: &str) -> bool {
    let mut found = false;
    own_names(node, &mut |n| found |= n == name);
    found
}

/// A bare name, plus the qualified key it would carry if this file declares it —
/// the `findInScopes` branch (`SymbolTable.hs:256-259`), which probes the found
/// declaration's `getQualifiedName` rather than what was written.
fn push_name(
    out: &mut Vec<SymbolPath>,
    declared: &FxHashMap<Symbol, Vec<SymbolPath>>,
    name: Symbol,
) {
    out.push(SymbolPath::single(name));
    out.extend(declared.get(&name).into_iter().flatten().cloned());
}

/// Every declaration in the file, indexed by the last component of its key — the
/// only component a written use site spells.
fn declared_by_last_component(tree: &ItemTree) -> FxHashMap<Symbol, Vec<SymbolPath>> {
    let mut map: FxHashMap<Symbol, Vec<SymbolPath>> = FxHashMap::default();
    for item in &tree.items {
        // A field is not in the reference's map (`Symbols.hs:48-49`), so a use
        // of its name cannot be attributed to it.
        if item.is_field() {
            continue;
        }
        // A one-component key is the bare name already, which `push_name` and
        // `push_path` push unconditionally.
        if item.qualified_name.segments().len() > 1
            && let Some(last) = item.qualified_name.last()
        {
            map.entry(last).or_default().push(item.qualified_name.clone());
        }
    }
    for keys in map.values_mut() {
        keys.sort_unstable();
        keys.dedup();
    }
    map
}

fn in_path(node: &SyntaxNode) -> bool {
    node.parent().is_some_and(|p| p.kind() == SyntaxKind::PATH_SEGMENT)
}

fn ident(node: &SyntaxNode, interner: &Interner) -> Option<Symbol> {
    ast::NameRef::cast(node.clone())
        .and_then(|n| n.ident_token())
        .map(|t| interner.intern(t.text()))
}

#[cfg(test)]
mod tests {
    use kg_base::{Utf8PathBuf, Utf8Path};
    use kg_config::Settings;
    use kg_diag::WarningKind;

    use crate::Db;

    /// A database with `redundant-import` on, rooted at `/proj`, no base library
    /// — the shape of `test/syntax/CMakeLists.txt:123`, which re-enables the
    /// warning for the `imports` test after the defaults switched it off.
    fn db() -> Db {
        let mut settings = Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        settings.warnings.enable(WarningKind::RedundantImport);
        Db::new(settings)
    }

    fn write(db: &mut Db, name: &str, text: &str) -> kg_base::FileId {
        db.set_file_text(&Utf8PathBuf::from(format!("/proj/{name}")), text.to_owned())
    }

    fn warnings(db: &Db, roots: &[kg_base::FileId]) -> Vec<String> {
        db.check(roots)
            .into_iter()
            .filter(|d| d.warning == Some(WarningKind::RedundantImport))
            .map(|d| format!("{}:{:?}", d.range.file.0, d.range.range.start()))
            .collect()
    }

    /// `test/syntax/imports.k:426-428` — an import nothing uses.
    #[test]
    fn an_unused_import_warns() {
        let mut db = db();
        write(&mut db, "a.k", "module a { x }\ntemplate <auto N>\nstruct x { uint<N> y; }\n");
        let main = write(&mut db, "main.k", "import a\ninline void main() {}\n");

        assert_eq!(warnings(&db, &[main]).len(), 1);
    }

    /// `test/syntax/imports.k:430-437` — the same import, used once.
    #[test]
    fn a_used_import_does_not_warn() {
        let mut db = db();
        write(&mut db, "a.k", "module a { x }\ntemplate <auto N>\nstruct x { uint<N> y; }\n");
        let main =
            write(&mut db, "main.k", "import a\ninline void main() { static x<10> v; }\n");

        assert!(warnings(&db, &[main]).is_empty());
    }

    /// `findInScopes` (`SymbolTable.hs:256-259`): a bare name a block declared
    /// before the use is found there, and the imports are probed with the
    /// local declaration's qualified name, which none exposes — so an import
    /// that also exports `Foo` stays redundant (`local-functions.k::22`, the
    /// recorded walk's `redundant-import` warning). The same call with no
    /// local `Foo`, or before it, uses the import. All three read off the
    /// reference compiler (`kanagawa --Wredundant-import`, 2026-09-25).
    #[test]
    fn a_name_a_block_declared_first_does_not_use_an_import() {
        let mut db = db();
        write(&mut db, "h.k", "module h { Foo }\ninline void Foo() {}\n");
        let local = write(&mut db, "local.k", "import h\ninline void main()\n{\ninline void Foo() {}\nFoo();\n}\n");
        assert_eq!(warnings(&db, &[local]).len(), 1, "the local Foo answers");
        let param = write(&mut db, "param.k", "import h\ninline void main(uint32 Foo)\n{\nuint32 x = Foo;\n}\n");
        assert_eq!(warnings(&db, &[param]).len(), 1, "a parameter answers");
        let before = write(&mut db, "before.k", "import h\ninline void main()\n{\nFoo();\ninline void Bar() {}\n}\n");
        assert!(warnings(&db, &[before]).is_empty(), "the import answers");
    }

    /// The first component of `Bar::Zero` is a bare name too: a class's
    /// earlier `enum Bar` answers it in `findInScopes`, and `[Bar, Zero]`
    /// names nothing the import exposes, so the import exporting its own
    /// `Bar` stays redundant (`enum.k::51`: the reference's seed, one
    /// `WarningF redundant-import`, `kg-oracle --tree --passes 1`). With no
    /// local `Bar`, `Bar::B1` uses it. Red with a longer path's first
    /// component still probed bare.
    #[test]
    fn a_path_whose_first_name_is_local_does_not_use_an_import() {
        let mut db = db();
        write(&mut db, "h.k", "module h { Bar }\nenum Bar : int32\n{\n    B1\n}\n");
        let local = write(
            &mut db,
            "local.k",
            "import h\nclass Foo\n{\npublic:\n    enum Bar : uint1\n    {\n        Zero\n    }\n\n    inline void Run()\n    {\n        Bar b = Bar::Zero;\n    }\n}\n",
        );
        assert_eq!(warnings(&db, &[local]).len(), 1, "the member Bar answers");
        let imported = write(&mut db, "imported.k", "import h\ninline void main()\n{\n    const auto b = Bar::B1;\n}\n");
        assert!(warnings(&db, &[imported]).is_empty(), "the import answers");
    }

    /// A member is looked up under its receiver's type or not at all
    /// (`Parser.hs:729-742,811-819,863`): `c.Go()` probes `C::Go`, never the
    /// import's free `Go`; `s.y` and `.y = 1` probe nothing; and `c.Do()`
    /// is not `Exec::Do`, a parameter's key under a function, whether the
/// receiver's type is this file's `C` or a template's `T`. Each import is
    /// redundant — read off the reference's tree (`kg-oracle --tree`, the
    /// seed's `WarningF redundant-import`, 2026-09-27) — and a free call of
    /// `Go` uses it. `static-if.k::48`'s walk emits the warning's seed.
    /// Red with a member's bare name probed (the method call and the field
    /// access), with the designator's, or with the owner not required to be a
    /// type (the parameter).
    #[test]
    fn a_member_uses_an_import_only_under_a_type() {
        let mut db = db();
        write(&mut db, "a.k", "module a { Go, y, Exec }\ninline uint32 Go() { return 1; }\nconst uint32 y = 2;\ninline void Exec(() -> void Do) { Do(); }\n");
        let method = write(&mut db, "m.k", "import a\nclass C\n{\npublic:\n    uint32 Go() { return 1; }\n}\ninline void main()\n{\n    static C c;\n    uint32 x = c.Go();\n}\n");
        assert_eq!(warnings(&db, &[method]).len(), 1, "a method call");
        let field = write(&mut db, "f.k", "import a\nstruct S\n{\n    uint32 y;\n}\ninline void main()\n{\n    S s;\n    uint32 x = s.y;\n}\n");
        assert_eq!(warnings(&db, &[field]).len(), 1, "a field access");
        let designator = write(&mut db, "d.k", "import a\nstruct S\n{\n    uint32 y;\n}\ninline void main()\n{\n    S s = {.y = 1};\n}\n");
        assert_eq!(warnings(&db, &[designator]).len(), 1, "a designator");
        let param = write(&mut db, "p.k", "import a\nclass C\n{\npublic:\n    uint32 Do() { return 1; }\n}\ninline void main()\n{\n    static C c;\n    uint32 x = c.Do();\n}\n");
        assert_eq!(warnings(&db, &[param]).len(), 1, "a parameter is no member");
        let unknown = write(&mut db, "t.k", "import a\ntemplate <typename T>\ninline uint32 F(T t)\n{\n    return t.Do();\n}\n");
        assert_eq!(warnings(&db, &[unknown]).len(), 1, "a parameter is no member, the receiver's type unknown here");
        let free = write(&mut db, "g.k", "import a\ninline void main()\n{\n    uint32 x = Go();\n}\n");
        assert!(warnings(&db, &[free]).is_empty(), "a free call uses it");
    }

    /// The receiver's type, where the file answers it: `c.Do()` over this
    /// file's `C` probes `C::Do`, so an import whose `K` also has a `Do` is
    /// not credited — `extern-class.k::1`'s shape, whose walk emits the
    /// warning's seed; `F<uint32>` probes `F::Do`, an instance's name being
    /// its template's (`extern-class.k::26`) — while `k.Do()` over the import's own `K` uses it
    /// (the reference's tree, `kg-oracle --tree`, 2026-09-27). Red with
    /// `receiver_type_here` answering nothing (the superset credits `K::Do`).
    #[test]
    fn a_method_call_is_probed_under_its_receivers_type() {
        let mut db = db();
        write(&mut db, "b.k", "module b { K }\nclass K\n{\npublic:\n    uint32 Do() { return 1; }\n}\n");
        let own = write(&mut db, "o.k", "import b\nclass C\n{\npublic:\n    uint32 Do() { return 1; }\n}\ninline void main()\n{\n    static C c;\n    uint32 x = c.Do();\n}\n");
        assert_eq!(warnings(&db, &[own]).len(), 1, "this file's C");
        let theirs = write(&mut db, "k.k", "import b\ninline void main()\n{\n    static K k;\n    uint32 x = k.Do();\n}\n");
        assert!(warnings(&db, &[theirs]).is_empty(), "the import's K");
        let instance = write(&mut db, "i.k", "import b\ntemplate <typename T>\nclass F\n{\npublic:\n    uint32 Do() { return 1; }\n}\ninline void main()\n{\n    static F<uint32> f;\n    uint32 x = f.Do();\n}\n");
        assert_eq!(warnings(&db, &[instance]).len(), 1, "this file's F<uint32>");
    }

    /// The attribution oracle, `test/syntax/imports.k:503-523`: two imports that
    /// offer the **same** declaration, and only the one written last is credited
    /// (`SymbolTable.hs:263-270`). Swapping the two lines moves the warning, so
    /// this pins the direction of `find_in_imports`' reverse iteration.
    #[test]
    fn only_the_last_import_offering_a_name_is_credited() {
        let mut db = db();
        write(&mut db, "a.k", "module a { x }\nstruct x { bool b; }\n");
        // `b` re-exposes `a`'s `x` — the same declaration, reached two ways —
        // and adds one of its own, exactly as `test/syntax/imports/a1/a8.k` does.
        write(&mut db, "b.k", "module b { x, y }\nimport a\nstruct y { bool c; }\n");

        let body = "inline void main() { static x u; static y v; }\n";
        let first = write(&mut db, "first.k", &format!("import a\nimport b\n{body}"));
        assert_eq!(warnings(&db, &[first]).len(), 1, "the earlier import is redundant");

        let second = write(&mut db, "second.k", &format!("import b\nimport a\n{body}"));
        assert!(warnings(&db, &[second]).is_empty(), "both are credited");
    }

    /// `checkSymbols` (`Parser.hs:216-223`) counts as a use. Oracle:
    /// `test/syntax/imports/a1/a3.k` re-exposes a name it imports and mentions
    /// nowhere else, and `test/syntax/imports.k:475-483` requires the *importer*
    /// of it to be the one that warns — which it cannot be if `a3.k` warned too.
    #[test]
    fn re_exposing_an_imported_symbol_counts_as_a_use() {
        let mut db = db();
        write(&mut db, "a.k", "module a { x }\nstruct x { bool b; }\n");
        let b = write(&mut db, "b.k", "module b { x }\nimport a\n");

        assert!(warnings(&db, &[b]).is_empty());
    }

    /// An aliased import is credited only through its alias
    /// (`SymbolTable.hs:276-278`), so a file that declares its own `x` and
    /// re-exposes that does **not** credit `import a as A`. Oracle:
    /// `test/syntax/imports/a1/a6.k` with `test/syntax/imports.k:457-464`.
    #[test]
    fn an_aliased_import_is_not_credited_by_the_bare_name() {
        let mut db = db();
        write(&mut db, "a.k", "module a { x }\nstruct x { bool b; }\n");
        let b = write(&mut db, "b.k", "module b { x }\nimport a as A\nstruct x { bool c; }\n");

        assert_eq!(warnings(&db, &[b]).len(), 1);
    }

    /// ... and *is* credited through it.
    #[test]
    fn an_aliased_import_is_credited_through_its_alias() {
        let mut db = db();
        write(&mut db, "a.k", "module a { x }\nstruct x { bool b; }\n");
        let b = write(
            &mut db,
            "b.k",
            "module b { y }\nimport a as A\nstruct y { A::x inner; }\n",
        );

        assert!(warnings(&db, &[b]).is_empty());
    }

    /// `addImport Needed 0 Nothing` (`Parser/IO.hs:77`) — the implicit base is
    /// seeded `Needed`, so it never warns however little of it is used.
    #[test]
    fn the_implicit_base_import_never_warns() {
        let mut db = Db::new({
            let mut s = Settings::fallback("/proj");
            s.warnings.enable(WarningKind::RedundantImport);
            s
        });
        let main = db.set_file_text(
            Utf8Path::new("/proj/main.k"),
            "inline void main() {}\n".to_owned(),
        );

        assert!(
            db.check(&[main]).iter().all(|d| d.warning != Some(WarningKind::RedundantImport))
        );
    }

    /// `updateNeeded` runs inside `findSymbol` whatever the warning switch says
    /// (`SymbolTable.hs:263-270`): with the warning off, the graph still knows
    /// which imports the file used, because the backend walk emits one
    /// `Warning` wrapper per import still `Redundant` (`Parser.hs:234-241`).
    /// Red with the replay behind the switch — every import then reads as
    /// redundant, and the walk seeded every file (2026-09-17).
    #[test]
    fn an_import_is_marked_needed_with_the_warning_off() {
        let mut settings = Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        settings.warnings.disable(WarningKind::RedundantImport);
        let mut db = Db::new(settings);

        write(&mut db, "a.k", "module a { x }\nstruct x { bool b; }\n");
        write(&mut db, "b.k", "module b { y }\nstruct y { bool b; }\n");
        let main = write(&mut db, "main.k", "import a\nimport b\ninline void main() { x v; }\n");

        let program = db.program(&[main]);
        let redundant: Vec<String> =
            program.graph.redundant_imports(program.files[program.files.len() - 1]).map(|i| format!("{:?}", i.name)).collect();
        assert_eq!(redundant, vec!["[\"b\"]"]);
        assert!(warnings(&db, &[main]).is_empty());
    }

    /// `--Wno-redundant-import` is the default for every syntax test but two
    /// (`test/syntax/CMakeLists.txt:42`), so a database that has not enabled the
    /// warning must not produce it.
    #[test]
    fn the_warning_is_off_unless_enabled() {
        let mut settings = Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        settings.warnings.disable(WarningKind::RedundantImport);
        let mut db = Db::new(settings);

        write(&mut db, "a.k", "module a { x }\nstruct x { bool b; }\n");
        let main = write(&mut db, "main.k", "import a\ninline void main() {}\n");

        assert!(warnings(&db, &[main]).is_empty());
    }
}
