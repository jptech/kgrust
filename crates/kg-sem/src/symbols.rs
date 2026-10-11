//! `getSymbols' TemplateDecl` (`Symbols.hs:37-68`) over the program's IR, and
//! `lookupSymbol` / `lookupInScope` (`:73-87`) over the result.
//!
//! The map is program-wide, as the reference's is (`inferType` folds every
//! module's tree, `Type/Inference.hs:35`): a qualified name — declaration
//! scope plus name, the module namespace its first segment — to the node that
//! declares it. The first declaration in program order wins, which is
//! `HashMap.fromList . reverse` over a pre-order walk and `HashMap.unions`'
//! left bias across modules.
//!
//! The loop builds the map over and over — every `inferType` fixpoint, and
//! the wrappers', deduction's and the census's every round — while each
//! pass rewrites a little of the program. [`Filed`] keeps what each subtree
//! filed between builds, so a build walks only what changed since the last.

use kg_base::{FxHashMap, Interner, Symbol, SymbolPath};
use kg_hir::{Exp, ExpArena, ExpId, ModuleIr};

/// Where a declaration is: which module of the program, which node.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Decl {
    pub module: usize,
    pub id: ExpId,
}

#[derive(Debug, Default, PartialEq)]
pub struct Symbols {
    map: FxHashMap<SymbolPath, Decl>,
}

/// `SymbolsConfig` (`Symbols.hs:28-29`): what a template contributes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Config {
    /// Filed, not entered — inference's map.
    TemplateDecl,
    /// Not filed, its declaration's contents entered (`symbol e@Template{}
    /// | NoTemplate == config = tail`) — `addLambdaWrappers`' map.
    NoTemplate,
    /// Filed and entered — `getSymbols`, every other pass's map.
    AllSymbols,
}

impl Config {
    pub const ALL: [Config; 3] = [Config::TemplateDecl, Config::NoTemplate, Config::AllSymbols];

    /// Its place in [`Config::ALL`], and in [`filed_everywhere`]'s answer.
    pub fn slot(self) -> usize {
        self as usize
    }
}

impl Symbols {
    /// The map over `modules`, in program order.
    pub fn of<'a>(modules: impl IntoIterator<Item = &'a ModuleIr>, interner: &Interner) -> Symbols {
        Symbols::with(modules, interner, Config::TemplateDecl)
    }

    /// [`Symbols::of`] under `config`.
    pub fn with<'a>(modules: impl IntoIterator<Item = &'a ModuleIr>, interner: &Interner, config: Config) -> Symbols {
        let (this, errors) = (interner.intern("$this"), interner.intern("$errors"));
        let mut map = FxHashMap::default();
        for (module, ir) in modules.into_iter().enumerate() {
            let filer = Filer { arena: &ir.arena, this, errors, config };
            filer.collect(ir.root, &mut |key, id| {
                map.entry(key).or_insert(Decl { module, id });
            });
        }
        Symbols { map }
    }

    /// `lookupSymbolByName symbols` as `unmangle` asks it (`Type.hs:198-205`):
    /// the note `note` reads at the declaration `name` names — the map
    /// `kg_ty::subtype::show_unmangled_with` spells a message's types by.
    pub fn noted(&self, name: &[Symbol], note: impl Fn(Decl) -> Option<kg_ty::Ty>) -> Option<kg_ty::Ty> {
        note(self.by_name(&SymbolPath::new(name.iter().copied()))?)
    }

    /// `lookupSymbolByName`: exactly this key.
    pub fn by_name(&self, name: &SymbolPath) -> Option<Decl> {
        self.map.get(name).copied()
    }

    /// `lookupInScope s e` (`Symbols.hs:82-87`): `s ++ name`, then each
    /// shorter prefix of `s`, down to the empty scope.
    pub fn in_scope(&self, scope: &[Symbol], name: &[Symbol]) -> Option<Decl> {
        // One key buffer for every prefix tried, probed as a slice: this is
        // every name's lookup, and a path built per prefix was a sixth of a
        // whole-program check.
        let mut key: Vec<Symbol> = Vec::with_capacity(scope.len() + name.len());
        let mut s = scope.len();
        loop {
            key.clear();
            key.extend_from_slice(&scope[..s]);
            key.extend_from_slice(name);
            if let Some(d) = self.map.get(key.as_slice()) {
                return Some(*d);
            }
            if s == 0 {
                return None;
            }
            s -= 1;
        }
    }

    /// The keys whose declaration differs from `old`'s, added and removed
    /// included — what a lookup must meet to be answered otherwise.
    pub fn changed_keys(&self, old: &Symbols) -> Vec<SymbolPath> {
        let mut out: Vec<SymbolPath> =
            self.map.iter().filter(|(k, d)| old.map.get(*k) != Some(*d)).map(|(k, _)| k.clone()).collect();
        out.extend(old.map.keys().filter(|k| !self.map.contains_key(*k)).cloned());
        out
    }

    /// `hasErrorSymbol` (`Symbols.hs:32-33`).
    pub fn has_errors(&self, interner: &Interner) -> bool {
        self.map.contains_key(&SymbolPath::single(interner.intern("$errors")))
    }

    /// Every key and its declaration, in no particular order — the
    /// `HashMap.toList symbols` pass 9's hints rank (`Frontend.hs:128-137`).
    pub fn iter(&self) -> impl Iterator<Item = (&SymbolPath, Decl)> {
        self.map.iter().map(|(k, d)| (k, *d))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The scope and name a declaration is filed under (`getScope`, `getName`):
/// the declared name's own node for everything but a template, which is filed
/// under its declaration's.
pub fn declared_name(arena: &ExpArena, id: ExpId) -> Option<(SymbolPath, Symbol)> {
    declared_name_by(&|x| &arena[x].exp, id)
}

/// [`declared_name`] over any way of reading a node's expression.
pub fn declared_name_by<'e>(exp: &dyn Fn(ExpId) -> &'e Exp, id: ExpId) -> Option<(SymbolPath, Symbol)> {
    let (scope, name) = match exp(id) {
        Exp::Alias { scope, name, .. }
        | Exp::Capture { scope, name }
        | Exp::Struct { scope, name, .. }
        | Exp::Union { scope, name, .. }
        | Exp::Class { scope, name, .. }
        | Exp::Enum { scope, name, .. }
        | Exp::EnumConstant { scope, name, .. }
        | Exp::Variable { scope, name, .. }
        | Exp::FuncParam { scope, name, .. }
        | Exp::Function { scope, name, .. }
        | Exp::FunctionDecl { scope, name, .. } => (scope, *name),
        Exp::Template { decl, .. } => return declared_name_by(exp, *decl),
        _ => return None,
    };
    let Exp::Identifier(sym) = *exp(name) else { return None };
    Some((scope.clone(), sym))
}

/// What the subtree at `id` files, in the order [`Symbols::with`] files it:
/// a rewrite in place that leaves this list as it was leaves the map as it
/// was — the same keys to the same nodes, met in the same program order.
pub fn contributions(arena: &ExpArena, id: ExpId, this: Symbol, errors: Symbol, config: Config) -> Vec<(SymbolPath, ExpId)> {
    let mut out = Vec::new();
    Filer { arena, this, errors, config }.collect(id, &mut |key, id| out.push((key, id)));
    out
}

/// [`contributions`] under every config, in [`Config::ALL`]'s order — what
/// an in-place write is weighed by ([`harmless`]). A subtree holding no
/// form any config files is answered without a list.
pub fn filed_everywhere(arena: &ExpArena, id: ExpId, this: Symbol, errors: Symbol) -> [Vec<(SymbolPath, ExpId)>; 3] {
    if !holds_a_filed_form(arena, id) {
        return Default::default();
    }
    Config::ALL.map(|config| contributions(arena, id, this, errors, config))
}

/// Whether a write in place, of `old` by `new` at one node whose subtree
/// filed `before` and files `after` ([`filed_everywhere`]), leaves every
/// map's entries as they were — every subtree around the node filing what
/// it filed, so what [`Filed`] kept of them stays true.
///
/// A parent's own entry reads two children besides itself: its name's
/// identifier (`getName`), and a template's declaration, for its name and
/// for whether it is a `Typename` (`symbol (Template _ Typename) = id`,
/// `Symbols.hs:56`). A write of either shape is never harmless; any other
/// is read only through the subtree it heads, which `before` and `after`
/// cover.
pub fn harmless(old: &Exp, new: &Exp, before: &[Vec<(SymbolPath, ExpId)>; 3], after: &[Vec<(SymbolPath, ExpId)>; 3]) -> bool {
    let read_by_parent = |e: &Exp| matches!(e, Exp::Identifier(_) | Exp::Typename);
    !read_by_parent(old) && !read_by_parent(new) && before == after
}

/// Whether any config's walk from `root` can meet a node it files: every
/// node `Filer::file` files is one of these, and no config's walk enters a
/// node this one does not.
fn holds_a_filed_form(arena: &ExpArena, root: ExpId) -> bool {
    let mut stack = vec![root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        match &arena[id].exp {
            Exp::Error(_)
            | Exp::Template { .. }
            | Exp::Alias { .. }
            | Exp::Capture { .. }
            | Exp::Struct { .. }
            | Exp::Union { .. }
            | Exp::Class { .. }
            | Exp::Enum { .. }
            | Exp::EnumConstant { .. }
            | Exp::Variable { .. }
            | Exp::FuncParam { .. }
            | Exp::Function { .. }
            | Exp::FunctionDecl { .. } => return true,
            e => {
                kids.clear();
                e.children(&mut kids);
                stack.extend(kids.iter().copied());
            }
        }
    }
    false
}

/// `para update` (`Symbols.hs:46-50`): a template under `TemplateDecl` is
/// filed and not entered, under `NoTemplate` entered and not filed (its
/// declaration dropped, `tail`); a struct's or union's body is never entered
/// (fields are no symbols, `Symbols.hs:48-49`); everything else is entered.
#[derive(Copy, Clone)]
struct Filer<'a> {
    arena: &'a ExpArena,
    this: Symbol,
    errors: Symbol,
    config: Config,
}

impl Filer<'_> {
    fn file(&self, id: ExpId, sink: &mut impl FnMut(SymbolPath, ExpId)) {
        let arena = self.arena;
        match &arena[id].exp {
            // `symbol (Template _ Typename) = id`: a template-template
            // parameter's signature declares nothing.
            Exp::Template { decl, .. } if matches!(arena[*decl].exp, Exp::Typename) => {}
            Exp::Error(_) => sink(SymbolPath::single(self.errors), id),
            Exp::Class { .. } => {
                if let Some((scope, name)) = declared_name(arena, id) {
                    let mut key = scope.segments().to_vec();
                    key.push(name);
                    sink(SymbolPath::new(key.clone()), id);
                    key.push(self.this);
                    sink(SymbolPath::new(key), id);
                }
            }
            _ => {
                if let Some((scope, name)) = declared_name(arena, id) {
                    let mut key = scope.segments().to_vec();
                    key.push(name);
                    sink(SymbolPath::new(key), id);
                }
            }
        }
    }

    /// The walk's step at `id`: file it, and name the node whose children
    /// the walk enters next, if any.
    fn step(&self, id: ExpId, sink: &mut impl FnMut(SymbolPath, ExpId)) -> Option<ExpId> {
        let arena = self.arena;
        match &arena[id].exp {
            Exp::Template { decl, .. } if self.config == Config::NoTemplate => {
                // `tail`: the declaration's own entry goes, what it holds
                // stays. A template-template signature holds nothing.
                (!matches!(arena[*decl].exp, Exp::Typename)).then_some(*decl)
            }
            Exp::Template { .. } if self.config == Config::TemplateDecl => {
                self.file(id, sink);
                None
            }
            Exp::Struct { .. } | Exp::Union { .. } => {
                self.file(id, sink);
                None
            }
            _ => {
                self.file(id, sink);
                Some(id)
            }
        }
    }

    /// The pre-order walk, iteratively over one stack: the map is built over
    /// the whole program many times a round, and a vector of children per
    /// node was a sixth of a whole-library check.
    fn collect(&self, root: ExpId, sink: &mut impl FnMut(SymbolPath, ExpId)) {
        let mut stack = vec![root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            if let Some(parent) = self.step(id, sink) {
                kids.clear();
                self.arena[parent].exp.children(&mut kids);
                stack.extend(kids.iter().rev());
            }
        }
    }
}

/// The symbol maps of one program, built again and again as its modules are
/// rewritten (`kg_sem::Program`): what each subtree filed is kept, per
/// config and module, so a build walks only the nodes made since the last
/// one and copies the rest.
///
/// Exact because a node never changes once made — the arena is append-only,
/// and a rewrite mints a replacement and re-roots — except by a write in
/// place ([`ExpArena::node_mut`]). Every such write is counted by the
/// arena; one its writer weighed [`harmless`] and reported
/// ([`Filed::vetted`]) leaves what was kept true, and any other discards the
/// module's whole store. The map is then assembled from each module's
/// entries in program order, first one in winning, as [`Symbols::with`]
/// assembles it from its walk.
#[derive(Default)]
pub struct Filed {
    modules: [Vec<FiledModule>; 3],
    /// Per module, the in-place writes reported harmless.
    vetted: Vec<u64>,
    /// Nodes the builds walked rather than copied — for the unit tests.
    walked: u64,
}

impl std::fmt::Debug for Filed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Filed({} walked)", self.walked)
    }
}

impl Filed {
    /// [`Symbols::with`] over `modules`, which must be the program's modules
    /// in its order, every time.
    pub fn symbols<'a>(&mut self, modules: impl IntoIterator<Item = &'a ModuleIr>, interner: &Interner, config: Config) -> Symbols {
        let (this, errors) = (interner.intern("$this"), interner.intern("$errors"));
        let mut map = FxHashMap::default();
        let kept = &mut self.modules[config.slot()];
        for (module, ir) in modules.into_iter().enumerate() {
            if kept.len() <= module {
                kept.resize_with(module + 1, FiledModule::default);
            }
            if self.vetted.len() <= module {
                self.vetted.resize(module + 1, 0);
            }
            let filer = Filer { arena: &ir.arena, this, errors, config };
            kept[module].refresh(filer, ir.root, self.vetted[module], &mut self.walked);
            for (key, id) in &kept[module].out {
                map.entry(key.clone()).or_insert(Decl { module, id: *id });
            }
        }
        Symbols { map }
    }

    /// `writes` more writes in place in `module`'s arena, each weighed
    /// [`harmless`] by its writer.
    pub fn vetted(&mut self, module: usize, writes: u64) {
        if self.vetted.len() <= module {
            self.vetted.resize(module + 1, 0);
        }
        self.vetted[module] += writes;
    }

    /// Nodes walked rather than copied, over every build so far.
    pub fn walked(&self) -> u64 {
        self.walked
    }
}

/// One module's kept entries under one config.
#[derive(Default)]
struct FiledModule {
    /// What they were filed from: the arena's stamp (`0`: nothing yet), its
    /// in-place writes and the harmless ones then, and the root.
    stamp: u32,
    writes: u64,
    vetted: u64,
    root: Option<ExpId>,
    /// The module's entries, in the order the walk files them.
    out: Vec<(SymbolPath, ExpId)>,
    /// Every node whose subtree filed something, in pre-order: where its
    /// entries lie in `out`, and how many records of nodes below it follow
    /// it — so a kept subtree is copied with the records inside it, and a
    /// later build that rewrites around them still finds them.
    recs: Vec<Rec>,
    /// `recs` by node index.
    at: FxHashMap<u32, u32>,
    /// The nodes whose subtrees file nothing, as a bitset by node index.
    empty: Vec<u64>,
}

#[derive(Copy, Clone)]
struct Rec {
    index: u32,
    start: u32,
    len: u32,
    below: u32,
}

/// The walk's two visits to a node: before its children, and after.
enum Visit {
    Enter(ExpId),
    Exit { index: u32, start: u32, rec: u32 },
}

impl FiledModule {
    fn files_nothing(&self, index: usize) -> bool {
        self.empty.get(index / 64).is_some_and(|w| w >> (index % 64) & 1 == 1)
    }

    fn mark_files_nothing(&mut self, index: usize) {
        let w = index / 64;
        if self.empty.len() <= w {
            self.empty.resize(w + 1, 0);
        }
        self.empty[w] |= 1 << (index % 64);
    }

    /// Bring `out` up to the tree at `root`, walking only what nothing kept
    /// covers.
    fn refresh(&mut self, filer: Filer<'_>, root: ExpId, vetted: u64, walked: &mut u64) {
        let arena = filer.arena;
        // Another arena (a fork has its own stamp), or a write in place
        // nobody weighed: nothing kept can be trusted.
        let trusted =
            self.stamp == arena.stamp() && arena.writes().wrapping_sub(self.writes) == vetted.wrapping_sub(self.vetted);
        if !trusted {
            *self = FiledModule { stamp: arena.stamp(), ..FiledModule::default() };
        }
        self.writes = arena.writes();
        self.vetted = vetted;
        if self.root == Some(root) {
            return;
        }
        let mut out: Vec<(SymbolPath, ExpId)> = Vec::with_capacity(self.out.len());
        let mut recs: Vec<Rec> = Vec::with_capacity(self.recs.len());
        let mut stack = vec![Visit::Enter(root)];
        let mut kids = Vec::new();
        while let Some(visit) = stack.pop() {
            match visit {
                Visit::Enter(id) => {
                    let index = id.index();
                    if self.files_nothing(index) {
                        continue;
                    }
                    if let Some(&r) = self.at.get(&(index as u32)) {
                        let r = r as usize;
                        let kept = self.recs[r];
                        let start = out.len() as u32;
                        out.extend_from_slice(&self.out[kept.start as usize..(kept.start + kept.len) as usize]);
                        recs.extend(self.recs[r..=r + kept.below as usize].iter().map(|x| Rec { start: x.start - kept.start + start, ..*x }));
                        continue;
                    }
                    *walked += 1;
                    let (start, rec) = (out.len() as u32, recs.len() as u32);
                    recs.push(Rec { index: index as u32, start, len: 0, below: 0 });
                    stack.push(Visit::Exit { index: index as u32, start, rec });
                    if let Some(parent) = filer.step(id, &mut |key, id| out.push((key, id))) {
                        kids.clear();
                        arena[parent].exp.children(&mut kids);
                        stack.extend(kids.iter().rev().map(|&k| Visit::Enter(k)));
                    }
                }
                Visit::Exit { index, start, rec } => {
                    let len = out.len() as u32 - start;
                    if len == 0 {
                        // Nothing below it filed, so no record below it
                        // was kept: its own is the last.
                        recs.pop();
                        self.mark_files_nothing(index as usize);
                    } else {
                        let below = recs.len() as u32 - rec - 1;
                        recs[rec as usize] = Rec { index, start, len, below };
                    }
                }
            }
        }
        self.at.clear();
        for (r, rec) in recs.iter().enumerate() {
            self.at.entry(rec.index).or_insert(r as u32);
        }
        self.out = out;
        self.recs = recs;
        self.root = Some(root);
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_hir::rewrite::{Rewriter, desugar};
    use kg_syntax::{AstNode, ast};

    use super::*;

    fn lower(text: &str, file: u32, interner: &Interner) -> ModuleIr {
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let source = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        kg_hir::lower_sorted(&source, FileId(file), interner, &kg_hir::NoImports)
    }

    /// Every config's kept map against the whole walk's, over `modules`.
    fn agree(filed: &mut Filed, modules: &[ModuleIr], interner: &Interner) {
        for config in Config::ALL {
            let kept = filed.symbols(modules.iter(), interner, config);
            let whole = Symbols::with(modules.iter(), interner, config);
            assert!(!whole.is_empty());
            assert_eq!(kept, whole, "{config:?}");
        }
    }

    fn reachable(ir: &ModuleIr) -> u64 {
        crate::elaborate_tests_reachable(ir).len() as u64
    }

    /// `ir` with every `Variable` named `from` renamed `to`, the way a pass
    /// rewrites: replacements appended, the spine above them re-rooted.
    fn rename(ir: &mut ModuleIr, interner: &Interner, from: &str, to: &str) {
        let (from, to) = (interner.intern(from), interner.intern(to));
        let mut rw = Rewriter::new(&mut ir.arena, interner);
        ir.root = desugar(&mut rw, ir.root, &mut |rw, _, exp| match exp {
            Exp::Variable { scope, attr, ty, name, init, flags } if matches!(rw.arena[name].exp, Exp::Identifier(s) if s == from) => {
                let name = rw.replace(name, Exp::Identifier(to));
                Exp::Variable { scope, attr, ty, name, init, flags }
            }
            exp => exp,
        });
    }

    /// `ir` without the statements that declare `name`.
    fn drop_declaration(ir: &mut ModuleIr, interner: &Interner, name: &str) {
        let name = interner.intern(name);
        let mut rw = Rewriter::new(&mut ir.arena, interner);
        ir.root = desugar(&mut rw, ir.root, &mut |rw, _, exp| match exp {
            Exp::Seq(items) => Exp::Seq(
                items
                    .into_iter()
                    .filter(|&i| !matches!(&rw.arena[i].exp, Exp::Variable { name: n, .. } if matches!(rw.arena[*n].exp, Exp::Identifier(s) if s == name)))
                    .collect(),
            ),
            exp => exp,
        });
    }

    const PROGRAM: &str = "\
inline uint8 g(uint8 p)
{
    uint8 doomed = p;
    uint8 a = doomed;
    return a;
}

class C
{
public:
    uint8 Run(uint8 q)
    {
        uint8 b = q;
        if (b == 0)
        {
            uint8 inner = 1;
            b = inner;
        }
        return b;
    }
}

template <typename T>
inline T id(T x)
{
    T y = x;
    return y;
}

inline uint8 f(uint8 r)
{
    uint8 c = r;
    if (c == 1)
    {
        uint8 deep = 2;
        c = deep;
    }
    return c;
}
";

    /// A build after a rewrite equals the whole walk's under every config,
    /// and walks a fraction of the tree: the subtrees the rewrite left
    /// alone are copied. Red when a changed root is not walked again (the
    /// rename unseen), when nothing is kept (the walk is whole), or when
    /// the subtrees that file nothing are not remembered as such (84 nodes
    /// walked, against 15).
    #[test]
    fn a_kept_map_after_a_rewrite_is_the_whole_walks_and_walks_only_what_changed() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let whole = reachable(&modules[0]);
        rename(&mut modules[0], &interner, "a", "renamed");
        let before = filed.walked();
        agree(&mut filed, &modules, &interner);
        let walked = filed.walked() - before;
        assert!(walked * 6 < whole, "three builds walked {walked} nodes of a {whole}-node tree");
        assert!(
            Symbols::with(modules.iter(), &interner, Config::AllSymbols).iter().any(|(k, _)| k.last() == Some(interner.intern("renamed")))
        );
    }

    /// A rewrite that shortens what comes first moves every kept entry
    /// after it, and the records of the subtrees inside them: a second
    /// rewrite inside one of those subtrees copies the records it did not
    /// touch from their moved places. Red when a copied record keeps its old
    /// offset.
    #[test]
    fn records_copied_with_a_kept_subtree_follow_it_when_it_moves() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        drop_declaration(&mut modules[0], &interner, "doomed");
        agree(&mut filed, &modules, &interner);
        rename(&mut modules[0], &interner, "c", "c2");
        agree(&mut filed, &modules, &interner);
        rename(&mut modules[0], &interner, "b", "b2");
        agree(&mut filed, &modules, &interner);
    }

    /// The first declaration in program order wins, across modules and
    /// after either is rewritten. Red when a module's entries go in after
    /// a later module's.
    #[test]
    fn the_first_declaration_in_program_order_wins_as_modules_are_rewritten() {
        let interner = Interner::new();
        let mut modules = vec![
            lower("const uint8 x = 1;\nconst uint8 z = 1;\n", 0, &interner),
            lower("const uint8 x = 2;\nconst uint8 z = 2;\n", 1, &interner),
        ];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let x = SymbolPath::single(interner.intern("x"));
        assert_eq!(filed.symbols(modules.iter(), &interner, Config::AllSymbols).by_name(&x).map(|d| d.module), Some(0));
        rename(&mut modules[0], &interner, "x", "w");
        agree(&mut filed, &modules, &interner);
        assert_eq!(filed.symbols(modules.iter(), &interner, Config::AllSymbols).by_name(&x).map(|d| d.module), Some(1));
        rename(&mut modules[1], &interner, "z", "v");
        agree(&mut filed, &modules, &interner);
    }

    /// A write in place nobody vetted — a declaration's name changed under
    /// its kept entry — discards the module's store. Red when the arena's
    /// writes are not counted against the vetted ones.
    #[test]
    fn a_write_in_place_nobody_vetted_is_seen() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let a = interner.intern("a");
        let name = modules[0]
            .arena
            .iter()
            .find_map(|(_, n)| match &n.exp {
                Exp::Variable { name, .. } if matches!(modules[0].arena[*name].exp, Exp::Identifier(s) if s == a) => Some(*name),
                _ => None,
            })
            .expect("a is declared");
        modules[0].arena.node_mut(name).exp = Exp::Identifier(interner.intern("moved"));
        agree(&mut filed, &modules, &interner);
    }

    /// A write in place weighed harmless and reported keeps the store
    /// whole: the next build walks nothing. Red when a vetted write
    /// discards it.
    #[test]
    fn a_vetted_harmless_write_keeps_everything() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let (this, errors) = (interner.intern("$this"), interner.intern("$errors"));
        let one = modules[0]
            .arena
            .iter()
            .find_map(|(id, n)| matches!(n.exp, Exp::IntLiteral { .. }).then_some(id))
            .expect("a literal");
        let arena = &mut modules[0].arena;
        let before = filed_everywhere(arena, one, this, errors);
        let new = Exp::BoolLiteral(true);
        let old = std::mem::replace(&mut arena.node_mut(one).exp, new.clone());
        let after = filed_everywhere(arena, one, this, errors);
        assert!(harmless(&old, &new, &before, &after));
        filed.vetted(0, 1);
        let walked = filed.walked();
        agree(&mut filed, &modules, &interner);
        assert_eq!(filed.walked(), walked);
    }

    /// What is never harmless: a name's identifier, a `Typename`, and a
    /// write that changes what the subtree files. Red with the name rule
    /// gone, or with the lists not compared.
    #[test]
    fn a_name_or_a_change_in_what_is_filed_is_not_harmless() {
        let interner = Interner::new();
        let ir = lower(PROGRAM, 0, &interner);
        let (this, errors) = (interner.intern("$this"), interner.intern("$errors"));
        let none: [Vec<(SymbolPath, ExpId)>; 3] = Default::default();
        let x = Exp::Identifier(interner.intern("x"));
        let y = Exp::Identifier(interner.intern("y"));
        assert!(!harmless(&x, &y, &none, &none));
        assert!(!harmless(&Exp::Typename, &Exp::Auto, &none, &none));
        assert!(harmless(&Exp::Auto, &Exp::Void, &none, &none));
        let variable = ir.arena.iter().find_map(|(id, n)| matches!(n.exp, Exp::Variable { .. }).then_some(id)).expect("a variable");
        let filed = filed_everywhere(&ir.arena, variable, this, errors);
        assert!(filed.iter().all(|list| list.len() == 1));
        assert!(!harmless(&Exp::Auto, &Exp::Void, &none, &filed));
    }
}
