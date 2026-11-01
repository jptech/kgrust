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
/// an in-place write is weighed by ([`unbounded`]). A subtree holding no
/// form any config files is answered without a list.
pub fn filed_everywhere(arena: &ExpArena, id: ExpId, this: Symbol, errors: Symbol) -> [Vec<(SymbolPath, ExpId)>; 3] {
    if !holds_a_filed_form(arena, id) {
        return Default::default();
    }
    Config::ALL.map(|config| contributions(arena, id, this, errors, config))
}

/// Per config, in [`Config::ALL`]'s order, whether a write in place, of
/// `old` by `new` at one node whose subtree filed `before` and files
/// `after` ([`filed_everywhere`]), can change that config's map beyond
/// what [`Filed`] finds from the node itself: the records the node heads
/// and those around them, walked again. Any other write is bounded so,
/// however it changes what the node files.
///
/// Two are not. A parent's own entry reads its name's identifier
/// (`getName`), and a template reads whether its declaration is a
/// `Typename` (`symbol (Template _ Typename) = id`, `Symbols.hs:56`): a
/// write of either shape changes an entry no record of the written node
/// holds. And a subtree that filed nothing and now files something may
/// lie below others remembered as filing nothing, which no record names.
pub fn unbounded(old: &Exp, new: &Exp, before: &[Vec<(SymbolPath, ExpId)>; 3], after: &[Vec<(SymbolPath, ExpId)>; 3]) -> [bool; 3] {
    let read_by_parent = |e: &Exp| matches!(e, Exp::Identifier(_) | Exp::Typename);
    let named = read_by_parent(old) || read_by_parent(new);
    std::array::from_fn(|c| named || (before[c].is_empty() && !after[c].is_empty()))
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
/// place ([`ExpArena::node_mut`]), which the arena logs. What a subtree
/// files is a function of its nodes alone, wherever it hangs (the tree is
/// a DAG: a folded constant shares its initializer's children). So a write
/// changes only the entries of the records whose subtrees reach the node
/// written, and a record reaches a node that files something only through
/// the records nested in it, which the store keeps in pre-order: every
/// record of a node written, and every record enclosing one, is walked
/// again, and the rest copied. A template reads its declaration without
/// walking it, so a write there walks the template's records again.
///
/// A write its writer weighed and reported [`unbounded`] for a config, and
/// any write nobody weighed ([`Filed::weighed`]), discards the module's
/// whole store under that config. The map is then assembled from each
/// module's entries in program order, first one in winning, as
/// [`Symbols::with`] assembles it from its walk.
#[derive(Default)]
pub struct Filed {
    modules: [Vec<FiledModule>; 3],
    /// Per module, the in-place writes weighed, and of those, per config,
    /// the unbounded ones.
    weighed: Vec<Weighed>,
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
            if self.weighed.len() <= module {
                self.weighed.resize(module + 1, Weighed::default());
            }
            let filer = Filer { arena: &ir.arena, this, errors, config };
            kept[module].refresh(filer, ir.root, self.weighed[module], &mut self.walked);
            for (key, id) in &kept[module].out {
                map.entry(key.clone()).or_insert(Decl { module, id: *id });
            }
        }
        Symbols { map }
    }

    /// `writes` more writes in place in `module`'s arena, each weighed by
    /// its writer, of which `unbounded[c]` were [`unbounded`] under
    /// [`Config::ALL`]`[c]`.
    pub fn weighed(&mut self, module: usize, writes: u64, unbounded: [u64; 3]) {
        if self.weighed.len() <= module {
            self.weighed.resize(module + 1, Weighed::default());
        }
        let w = &mut self.weighed[module];
        w.writes += writes;
        for (n, u) in w.unbounded.iter_mut().zip(unbounded) {
            *n += u;
        }
    }

    /// Nodes walked rather than copied, over every build so far.
    pub fn walked(&self) -> u64 {
        self.walked
    }
}

/// One module's in-place writes reported to [`Filed::weighed`].
#[derive(Copy, Clone, Default)]
struct Weighed {
    writes: u64,
    unbounded: [u64; 3],
}

/// One module's kept entries under one config.
#[derive(Default)]
struct FiledModule {
    /// What they were filed from: the arena's stamp (`0`: nothing yet), its
    /// in-place writes, the weighed ones and this config's unbounded ones
    /// then, and the root.
    stamp: u32,
    writes: u64,
    weighed: u64,
    unbounded: u64,
    root: Option<ExpId>,
    /// The module's entries, in the order the walk files them.
    out: Vec<(SymbolPath, ExpId)>,
    /// Every node whose subtree filed something, in pre-order: where its
    /// entries lie in `out`, and how many records of nodes below it follow
    /// it — so a kept subtree is copied with the records inside it, and a
    /// later build that rewrites around them still finds them.
    recs: Vec<Rec>,
    /// `recs` by node index: the first record of each node.
    at: FxHashMap<u32, u32>,
    /// Some node has more than one record: it lies in the tree twice.
    shared: bool,
    /// The nodes whose subtrees file nothing, as a bitset by node index.
    empty: Vec<u64>,
    /// Each template's declaration, to the template ([`MANY`] for one
    /// shared by two): what a template reads without walking it.
    read_by: FxHashMap<u32, u32>,
}

#[derive(Copy, Clone)]
struct Rec {
    index: u32,
    start: u32,
    len: u32,
    below: u32,
}

/// [`FiledModule::read_by`]'s template for a declaration more than one
/// reads.
const MANY: u32 = u32::MAX;

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

    /// Forget the records `written` reach — each node's own, wherever it
    /// lies, the template reading it, and every record enclosing one — so
    /// the next walk enters them; whether any was. `None` where a template
    /// reading a node written is remembered as filing nothing, which no
    /// record bounds, or where more than one does.
    fn dirty(&mut self, written: &[u32]) -> Option<bool> {
        if written.is_empty() || self.recs.is_empty() {
            return Some(false);
        }
        let mut hit: kg_base::FxHashSet<u32> = kg_base::FxHashSet::default();
        for &w in written {
            hit.insert(w);
            if let Some(&template) = self.read_by.get(&w) {
                if template == MANY || self.files_nothing(template as usize) {
                    return None;
                }
                hit.insert(template);
            }
        }
        let mut at: Vec<usize> = if self.shared {
            (0..self.recs.len()).filter(|&r| hit.contains(&self.recs[r].index)).collect()
        } else {
            hit.iter().filter_map(|i| self.at.get(i)).map(|&r| r as usize).collect()
        };
        if at.is_empty() {
            return Some(false);
        }
        at.sort_unstable();
        // One pass in pre-order, the records open at each one hit on a stack:
        // those are what encloses it.
        let mut dirty = vec![false; self.recs.len()];
        let mut open: Vec<usize> = Vec::new();
        let mut next = 0;
        for r in 0..self.recs.len() {
            if next == at.len() {
                break;
            }
            while let Some(&top) = open.last()
                && top + (self.recs[top].below as usize) < r
            {
                open.pop();
            }
            if at[next] == r {
                next += 1;
                dirty[r] = true;
                // An enclosing record already forgotten had its own
                // enclosing ones forgotten with it.
                for &o in open.iter().rev() {
                    if std::mem::replace(&mut dirty[o], true) {
                        break;
                    }
                }
            }
            open.push(r);
        }
        for (r, &d) in dirty.iter().enumerate() {
            if d {
                self.at.remove(&self.recs[r].index);
            }
        }
        Some(true)
    }

    /// Bring `out` up to the tree at `root`, walking only what nothing kept
    /// covers.
    fn refresh(&mut self, filer: Filer<'_>, root: ExpId, weighed: Weighed, walked: &mut u64) {
        let arena = filer.arena;
        let unbounded = weighed.unbounded[filer.config.slot()];
        // Another arena (a fork has its own stamp), a write in place nobody
        // weighed, or one unbounded here: nothing kept can be trusted.
        let mut trusted = self.stamp == arena.stamp()
            && arena.writes().wrapping_sub(self.writes) == weighed.writes.wrapping_sub(self.weighed)
            && unbounded == self.unbounded;
        let mut dirtied = false;
        if trusted {
            match self.dirty(arena.written_since(self.writes)) {
                Some(any) => dirtied = any,
                None => trusted = false,
            }
        }
        if !trusted {
            *self = FiledModule { stamp: arena.stamp(), ..FiledModule::default() };
        }
        self.writes = arena.writes();
        self.weighed = weighed.writes;
        self.unbounded = unbounded;
        if self.root == Some(root) && !dirtied {
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
                    if let Exp::Template { decl, .. } = &arena[id].exp {
                        let mut decl = *decl;
                        loop {
                            let reader = self.read_by.entry(decl.index() as u32).or_insert(index as u32);
                            if *reader != index as u32 {
                                *reader = MANY;
                            }
                            let Exp::Template { decl: inner, .. } = &arena[decl].exp else { break };
                            decl = *inner;
                        }
                    }
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
        self.shared = false;
        for (r, rec) in recs.iter().enumerate() {
            match self.at.entry(rec.index) {
                std::collections::hash_map::Entry::Occupied(_) => self.shared = true,
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(r as u32);
                }
            }
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

    /// The `Variable` declaring `name`.
    fn declaration(ir: &ModuleIr, interner: &Interner, name: &str) -> ExpId {
        let name = interner.intern(name);
        ir.arena
            .iter()
            .find_map(|(id, n)| match &n.exp {
                Exp::Variable { name: n, .. } if matches!(ir.arena[*n].exp, Exp::Identifier(s) if s == name) => Some(id),
                _ => None,
            })
            .expect("declared")
    }

    /// The block in the tree whose statements include `id`.
    fn block_holding(ir: &ModuleIr, id: ExpId) -> ExpId {
        crate::elaborate_tests_reachable(ir)
            .into_iter()
            .find(|&b| matches!(&ir.arena[b].exp, Exp::Seq(items) if items.contains(&id)))
            .expect("held by a block")
    }

    /// A write in place at `id` in module `module`, weighed and reported as
    /// inference weighs and reports its own (`Cx::replace`).
    fn write(filed: &mut Filed, modules: &mut [ModuleIr], module: usize, interner: &Interner, id: ExpId, new: Exp) {
        let (this, errors) = (interner.intern("$this"), interner.intern("$errors"));
        let arena = &mut modules[module].arena;
        let before = filed_everywhere(arena, id, this, errors);
        let old = std::mem::replace(&mut arena.node_mut(id).exp, new);
        let after = filed_everywhere(arena, id, this, errors);
        filed.weighed(module, 1, unbounded(&old, &arena[id].exp, &before, &after).map(u64::from));
    }

    /// `exp` declared in `scope` instead.
    fn rescoped(exp: &Exp, scope: SymbolPath) -> Exp {
        let mut exp = exp.clone();
        match &mut exp {
            Exp::Variable { scope: s, .. } | Exp::Function { scope: s, .. } => *s = scope,
            other => panic!("not a declaration with a scope: {other:?}"),
        }
        exp
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

    /// A write in place nobody weighed — a declaration's name changed under
    /// its kept entry — discards the module's store. Red when the arena's
    /// writes are not counted against the weighed ones.
    #[test]
    fn a_write_in_place_nobody_weighed_is_seen() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let a = declaration(&modules[0], &interner, "a");
        let Exp::Variable { name, .. } = modules[0].arena[a].exp else { unreachable!("a variable") };
        modules[0].arena.node_mut(name).exp = Exp::Identifier(interner.intern("moved"));
        agree(&mut filed, &modules, &interner);
    }

    /// A weighed write at a node no record holds — a literal folded, which
    /// files nothing before or after — keeps the store whole: the next
    /// builds walk nothing. Red when a weighed write discards the store.
    #[test]
    fn a_weighed_write_reaching_no_record_keeps_everything() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let one = modules[0]
            .arena
            .iter()
            .find_map(|(id, n)| matches!(n.exp, Exp::IntLiteral { .. }).then_some(id))
            .expect("a literal");
        write(&mut filed, &mut modules, 0, &interner, one, Exp::BoolLiteral(true));
        let walked = filed.walked();
        agree(&mut filed, &modules, &interner);
        assert_eq!(filed.walked(), walked);
    }

    /// A weighed write that changes what its subtree files — a block's
    /// declaration dropped, as a folded `if` drops a branch — walks again
    /// only the records that reach the node written: the next builds equal
    /// the whole walk's, and walk a few nodes of the tree. Red when the
    /// written node's own record is not forgotten, or those enclosing it
    /// (`inner` is kept), and when the write discards the store (the whole
    /// tree walked again: 631 nodes, against 24).
    #[test]
    fn a_weighed_write_walks_again_only_the_records_that_reach_it() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let whole = reachable(&modules[0]);
        let inner = declaration(&modules[0], &interner, "inner");
        let block = block_holding(&modules[0], inner);
        let Exp::Seq(items) = modules[0].arena[block].exp.clone() else { unreachable!("a block") };
        write(&mut filed, &mut modules, 0, &interner, block, Exp::Seq(items.into_iter().filter(|&i| i != inner).collect()));
        let before = filed.walked();
        agree(&mut filed, &modules, &interner);
        let walked = filed.walked() - before;
        assert!(walked * 6 < whole, "three builds walked {walked} nodes of a {whole}-node tree");
        let inner = interner.intern("inner");
        assert!(!Symbols::with(modules.iter(), &interner, Config::AllSymbols).iter().any(|(k, _)| k.last() == Some(inner)));
    }

    /// A write that makes a subtree file what it filed nothing of — an
    /// error folded in place of a literal — is unbounded: a subtree
    /// remembered as filing nothing may hold it, and no record names those.
    /// The store goes, and the next builds equal the whole walk's. Red with
    /// the rule gone from [`unbounded`], or the unbounded count not compared.
    #[test]
    fn a_write_filing_where_nothing_was_discards_the_store() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let one = modules[0]
            .arena
            .iter()
            .find_map(|(id, n)| matches!(n.exp, Exp::IntLiteral { .. }).then_some(id))
            .expect("a literal");
        write(&mut filed, &mut modules, 0, &interner, one, Exp::Error(kg_hir::ErrorNote::new("folded")));
        agree(&mut filed, &modules, &interner);
        assert!(Symbols::with(modules.iter(), &interner, Config::TemplateDecl).has_errors(&interner));
    }

    /// A template files its declaration's name without walking the
    /// declaration (under `TemplateDecl`), so a write there walks the
    /// template's records again. Red when the template is not remembered as
    /// what reads its declaration.
    #[test]
    fn a_write_at_a_templates_declaration_walks_the_template_again() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let decl = modules[0]
            .arena
            .iter()
            .find_map(|(_, n)| match n.exp {
                Exp::Template { decl, .. } => Some(decl),
                _ => None,
            })
            .expect("a template");
        let new = rescoped(&modules[0].arena[decl].exp, SymbolPath::single(interner.intern("elsewhere")));
        write(&mut filed, &mut modules, 0, &interner, decl, new);
        agree(&mut filed, &modules, &interner);
    }

    /// A node the tree holds twice — a folded constant shares its
    /// initializer's children — has a record at each place it lies, and a
    /// write at it walks again what encloses either. Red when only its first
    /// record is found.
    #[test]
    fn a_write_at_a_shared_node_walks_again_around_each_place() {
        let interner = Interner::new();
        let mut modules = vec![lower(PROGRAM, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        // `f`'s body comes to share `g`'s declaration of `a`.
        let (a, c) = (declaration(&modules[0], &interner, "a"), declaration(&modules[0], &interner, "c"));
        let body = block_holding(&modules[0], c);
        let Exp::Seq(mut items) = modules[0].arena[body].exp.clone() else { unreachable!("a block") };
        items.push(a);
        write(&mut filed, &mut modules, 0, &interner, body, Exp::Seq(items));
        agree(&mut filed, &modules, &interner);
        let new = rescoped(&modules[0].arena[a].exp, SymbolPath::single(interner.intern("elsewhere")));
        write(&mut filed, &mut modules, 0, &interner, a, new);
        agree(&mut filed, &modules, &interner);
    }

    const TEMPLATES: &str = "\
const uint8 k = 1;

template <typename T>
inline void h()
{
}

template <typename T>
inline void j()
{
}
";

    /// Each template of [`TEMPLATES`], and its declaration.
    fn templates(ir: &ModuleIr) -> Vec<(ExpId, ExpId)> {
        crate::elaborate_tests_reachable(ir)
            .into_iter()
            .filter_map(|t| match ir.arena[t].exp {
                Exp::Template { decl, .. } => Some((t, decl)),
                _ => None,
            })
            .collect()
    }

    /// A declaration two templates read is bounded by neither one's records
    /// alone: a write there discards the store. Red when only one reader is
    /// remembered.
    #[test]
    fn a_write_at_a_declaration_two_templates_read_discards_the_store() {
        let interner = Interner::new();
        let mut modules = vec![lower(TEMPLATES, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        // `j` comes to declare what `h` does, as a fold shares a subtree.
        let found = templates(&modules[0]);
        let [(_, h), (j, _)] = found[..] else { panic!("two templates: {found:?}") };
        let Exp::Template { params, .. } = modules[0].arena[j].exp.clone() else { unreachable!("a template") };
        write(&mut filed, &mut modules, 0, &interner, j, Exp::Template { params, decl: h });
        agree(&mut filed, &modules, &interner);
        let new = rescoped(&modules[0].arena[h].exp, SymbolPath::single(interner.intern("elsewhere")));
        write(&mut filed, &mut modules, 0, &interner, h, new);
        agree(&mut filed, &modules, &interner);
    }

    /// A template whose declaration holds nothing filed is remembered as
    /// filing nothing under `NoTemplate`, which enters the declaration's
    /// contents without filing it: a write there that comes to file
    /// something — an error folded in — discards the store, since no record
    /// bounds it. Red when the template's record is looked for instead.
    #[test]
    fn a_write_at_the_declaration_of_a_template_filing_nothing_discards_the_store() {
        let interner = Interner::new();
        let mut modules = vec![lower(TEMPLATES, 0, &interner)];
        let mut filed = Filed::default();
        agree(&mut filed, &modules, &interner);
        let (_, h) = templates(&modules[0])[0];
        let arena = &mut modules[0].arena;
        let error = arena.adopt(kg_hir::NodeData { exp: Exp::Error(kg_hir::ErrorNote::new("folded")), ..arena.node(h).clone() });
        write(&mut filed, &mut modules, 0, &interner, h, Exp::Seq(vec![error]));
        agree(&mut filed, &modules, &interner);
        assert!(Symbols::with(modules.iter(), &interner, Config::NoTemplate).has_errors(&interner));
    }

    /// What is unbounded: a write of a name's identifier or of a `Typename`,
    /// under every config, and one making a subtree that filed nothing file
    /// something, under the configs it does so in. A write that only changes
    /// or drops what a subtree files is bounded. Red with the name rule gone,
    /// or the lists not weighed per config.
    #[test]
    fn only_a_name_or_filing_where_nothing_was_is_unbounded() {
        let interner = Interner::new();
        let ir = lower(PROGRAM, 0, &interner);
        let (this, errors) = (interner.intern("$this"), interner.intern("$errors"));
        let none: [Vec<(SymbolPath, ExpId)>; 3] = Default::default();
        let x = Exp::Identifier(interner.intern("x"));
        let y = Exp::Identifier(interner.intern("y"));
        assert_eq!(unbounded(&x, &y, &none, &none), [true; 3]);
        assert_eq!(unbounded(&Exp::Typename, &Exp::Auto, &none, &none), [true; 3]);
        assert_eq!(unbounded(&Exp::Auto, &Exp::Void, &none, &none), [false; 3]);
        let variable = declaration(&ir, &interner, "a");
        let filed = filed_everywhere(&ir.arena, variable, this, errors);
        assert!(filed.iter().all(|list| list.len() == 1));
        assert_eq!(unbounded(&Exp::Auto, &Exp::Void, &none, &filed), [true; 3]);
        assert_eq!(unbounded(&Exp::Auto, &Exp::Void, &filed, &none), [false; 3]);
        let mut one = none.clone();
        one[1] = filed[1].clone();
        assert_eq!(unbounded(&Exp::Auto, &Exp::Void, &none, &one), [false, true, false]);
    }
}
