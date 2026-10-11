//! The walk: `parse` of `compiler/hs/app/ParseTree.hs:63-175`, arm for arm,
//! over [`kg_hir::Exp`], which mirrors `ExpF` constructor for constructor.
//!
//! Order is `cataM parseWithLoc` (`ParseTree.hs:40`, `Recursion.hs:79-83`):
//! every child first, in `ExpF` field order — [`Exp::children`]'s order —
//! then, for the node itself, `setSrcLocation`, the constructor call, and
//! `SetNodeType` when its type is an integer or an enum (`:50-55`). A node
//! with no call of its own (`Positional`, `Warning`, `ModuleDiff`) still
//! sets its location, because `parseWithLoc` does.
//!
//! What is declined, and why, is in [`Declined`]: the eighteen `undefined`
//! arms (`abi.md` §11), and the rewrites the reference made before its walk
//! that this port has not — each named, so `xtask ptdiff` can count them.

use kg_base::{Interner, LineIndex, Src, Written};
use kg_hir::{Exp, ExpId, ModuleIr};
use kg_pttrace::Location;
use kg_ty::{Attribute, Flag, Ty};

use crate::Backend;

/// A node the walk cannot emit, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declined {
    /// A short, stable name for the class: `auto`, `template`, `fold:clog2`.
    pub reason: String,
    pub at: Src,
}

/// One module as the walk reads it.
#[derive(Clone, Copy)]
pub struct Module<'a> {
    pub module: kg_resolve::ModuleSource,
    pub ir: &'a ModuleIr,
    /// Index into the program's file list — `Location._fileIndex`.
    pub file_index: usize,
    pub lines: &'a LineIndex,
    /// `Variable` nodes by their name's range — what an `Item`'s range
    /// names — for a constant's initializer looked up across modules.
    pub variables: &'a rustc_hash::FxHashMap<kg_base::TextRange, ExpId>,
    /// Imports still `Redundant` at end of file — each one wraps the
    /// parser's seed in a `Warning` node (`Parser.hs:234-241`).
    pub redundant: usize,
    /// The loop's inference over this very tree (`kg_sem`, `docs/plans/
    /// ir-elaboration.md` M3): every node's type by its id, minted nodes
    /// included, and the tree as `evaluateConstexpr` rewrote it — the
    /// walk's only source of types and folded values since 2026-10-10
    /// (sprint W3.6; the CST engine's table and folder are no input).
    pub sem: &'a kg_sem::Module,
    /// `kg_config::Verdicts::ptbridge_seed`.
    pub seed: bool,
}

/// Everything the walk needs of one module, owned; [`ModuleCtx::view`] is
/// the [`Module`] the walk reads.
pub struct ModuleCtx {
    pub module: kg_resolve::ModuleSource,
    pub ir: std::sync::Arc<ModuleIr>,
    pub lines: LineIndex,
    pub file_index: usize,
    pub variables: rustc_hash::FxHashMap<kg_base::TextRange, ExpId>,
    pub redundant: usize,
    pub sem: std::sync::Arc<kg_sem::Module>,
    pub seed: bool,
}

impl std::fmt::Debug for ModuleCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleCtx").field("module", &self.module).finish_non_exhaustive()
    }
}

impl ModuleCtx {
    pub fn new(
        module: kg_resolve::ModuleSource,
        ir: std::sync::Arc<ModuleIr>,
        sem: std::sync::Arc<kg_sem::Module>,
        lines: LineIndex,
        file_index: usize,
    ) -> ModuleCtx {
        let mut variables = rustc_hash::FxHashMap::default();
        for (id, n) in ir.arena.iter() {
            if let Exp::Variable { name, .. } = &n.exp {
                let name = ir.arena.node(*name);
                if name.src.written == Written::Own {
                    variables.insert(name.src.head.range, id);
                }
            }
        }
        ModuleCtx {
            module,
            ir,
            lines,
            file_index,
            variables,
            redundant: 0,
            sem,
            seed: true,
        }
    }

    /// The walk's verdict switch, from the settings
    /// (`kg_config::Verdicts::ptbridge_seed`).
    pub fn with_seed(mut self, seed: bool) -> ModuleCtx {
        self.seed = seed;
        self
    }

    pub fn with_redundant_imports(mut self, redundant: usize) -> ModuleCtx {
        self.redundant = redundant;
        self
    }

    pub fn view(&self) -> Module<'_> {
        Module {
            module: self.module,
            ir: &self.ir,
            file_index: self.file_index,
            lines: &self.lines,
            variables: &self.variables,
            redundant: self.redundant,
            sem: &self.sem,
            seed: self.seed,
        }
    }
}

impl std::fmt::Debug for Module<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Module").field("file_index", &self.file_index).finish_non_exhaustive()
    }
}

/// See [`Walker::conversions`].
#[derive(Default)]
struct Conversions {
    /// The nodes an enum value is walked as its literal at.
    ints: Vec<ExpId>,
    /// The list elements a narrowing warning types a second time.
    warned: Vec<ExpId>,
}

pub struct Walker<'a, B: Backend> {
    pub backend: &'a mut B,
    pub interner: &'a Interner,
    /// `getSymbols` over the whole program (`Frontend.hs:40`), for
    /// `resolveNames` and `redundantNameScope`.
    pub symbols: &'a kg_db::SymbolMap,
    /// The import graph, for `findInImports` (`SymbolTable.hs:263-270`)
    /// on a name the walk reads as text — a hole's.
    pub graph: &'a kg_resolve::module::ModuleGraph,
    pub items: kg_resolve::lower::Program<'a>,
    /// Every module of the program, for a subtree the reference copies
    /// from another module (a constant's initializer).
    pub modules: &'a [ModuleCtx],
    /// A note to give the next node walked instead of its own — `NotedExp n
    /// $ eval e`, where a substituted subtree's top node wears the use
    /// site's note.
    pending_src: Option<Src>,
    /// `reifyEnums` (`Frontend.hs:195-208`): the value of every enum
    /// constant written without one, computed when its `Enum` is entered.
    enum_values: rustc_hash::FxHashMap<ExpId, kg_const::ConstInt>,
    /// The declarations of each enclosing function, innermost last: its
    /// parameters and every local of its body, by name — what a name the
    /// table left untyped is typed by ([`Walker::local_type`]).
    frames: Vec<Vec<(kg_base::Symbol, ExpId)>>,
    /// The `declScopeName` of each enclosing declaration, innermost last —
    /// where a hole's name is looked up when no local declares it.
    scopes: Vec<kg_base::SymbolPath>,
    /// `getSymbols` over the tree the walk reads (`postDesugar symbols`,
    /// `Frontend.hs:63,172-179`): see [`post_symbols`].
    post: PostSymbols,
    /// What the conversion check around the node being walked does inside
    /// it — see [`Walker::conversions`].
    conversions: Conversions,
    /// `checkReturn`'s conversions (`TypeCheck.hs:76,84`), by the `Return`
    /// statement they belong to — see [`Walker::return_conversions`].
    returns: Vec<(ExpId, Conversions)>,
    /// The leaf of every function template some module declares — what
    /// `removeTemplate` takes out of `post` (see [`template_functions`]).
    template_functions: rustc_hash::FxHashSet<kg_base::Symbol>,
    /// Every copy a walked tree declares as a type — see [`declared_copies`].
    declared_copies: rustc_hash::FxHashSet<kg_base::Symbol>,
}

/// The leaf of every function template in the walked trees, by original
/// name. A call whose callee is one of these and which `post` does not
/// declare is a call no deduction rewrote: the reference names an instance
/// there or reports the call (`deduceTemplateArgs`, `Type.hs:1450-1461`),
/// and the backend has neither — its "Undefined function" is ours alone
/// (`Go(callback)` in a class template's lambda, `this-capture.k::18`,
/// whose member wrapper this route makes a round late).
fn template_functions(modules: &[ModuleCtx]) -> rustc_hash::FxHashSet<kg_base::Symbol> {
    let mut out = rustc_hash::FxHashSet::default();
    for ctx in modules {
        let m = ctx.view();
        for (_, n) in m.ir.arena.iter() {
            if let Exp::Template { decl, .. } = &n.exp
                && let Exp::Function { name, .. } = &m.ir.arena.node(*decl).exp
                && let Exp::Identifier(sym) = &m.ir.arena.node(*name).exp
            {
                out.insert(*sym);
            }
        }
    }
    out
}

/// The name of every type declaration in the walked trees that is a copy
/// (`<|`, the mangler's argument bracket), by original name. A type named
/// as a copy that none of them declares is a mention whose instance no
/// round materialised — `Computer<32, 6>` over an `extern` class template
/// with a string default (`test/interface/external_class_template.k`,
/// 2026-09-23) — and the backend meets the variable declared with it as
/// "Left of . in a method call must be an object", an error on a program
/// the reference compiles. Declined instead: silence, not a false verdict.
fn declared_copies(modules: &[ModuleCtx], interner: &Interner) -> rustc_hash::FxHashSet<kg_base::Symbol> {
    let mut out = rustc_hash::FxHashSet::default();
    for ctx in modules {
        let m = ctx.view();
        for (_, n) in m.ir.arena.iter() {
            let name = match &n.exp {
                Exp::Class { name, .. }
                | Exp::Struct { name, .. }
                | Exp::Union { name, .. }
                | Exp::Enum { name, .. }
                | Exp::Alias { name, .. } => *name,
                _ => continue,
            };
            if let Exp::Identifier(sym) = &m.ir.arena.node(name).exp {
                let original = *sym;
                if kg_base::mentions_instance(&interner.resolve(original)) {
                    out.insert(original);
                }
            }
        }
    }
    out
}

/// A declaration of the walked program: its module (an index into
/// [`Walker::modules`]) and its node there.
type PostSymbols = rustc_hash::FxHashMap<Vec<kg_base::Symbol>, (usize, ExpId)>;

/// `getSymbols'' AllSymbols` (`Symbols.hs:43-71`) over every module's walked
/// tree, in program order, keys spelled in original names (the route's
/// aliases mapped back) — the map `resolveNames` reads (`Frontend.hs:273-291`).
///
/// It is not the program's item map: `postDesugar`'s symbols are taken after
/// `instantiateTemplates` and `removeTemplate` (`Frontend.hs:62-64`, the list
/// runs bottom-up), so a copy is declared in it — `Fn_T<|$bool|>` beside its
/// template, which the reference then spells with its module's namespace at
/// every call (`return-type-polymorphism.k::10`) — and a template is not.
/// Pre-order with the first key kept is `fromList . reverse` (`:44`); a
/// `Struct` or `Union` claims its own key and stops (`:48-49`); a `Class`
/// also claims `[…, name, $this]` (`:66`). An abbreviated function template
/// is deleted with the templates (`Frontend.hs:300-301`).
fn post_symbols(modules: &[ModuleCtx], interner: &Interner) -> PostSymbols {
    let this_sym = interner.intern(kg_resolve::names::THIS);
    let mut map = PostSymbols::default();
    for (index, ctx) in modules.iter().enumerate() {
        let m = ctx.view();
        let ident = |id: ExpId| match &m.ir.arena.node(id).exp {
            Exp::Identifier(sym) => Some(*sym),
            _ => None,
        };
        let mut stack = vec![m.ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            let exp = &m.ir.arena.node(id).exp;
            let mut insert = |scope: &kg_base::SymbolPath, name: Option<kg_base::Symbol>, this: bool| {
                let Some(name) = name else { return };
                let mut key: Vec<kg_base::Symbol> = scope.segments().to_vec();
                key.push(name);
                if this {
                    key.push(this_sym);
                }
                map.entry(key).or_insert((index, id));
            };
            match exp {
                Exp::Template { .. } => continue,
                Exp::Function { params, .. } | Exp::FunctionDecl { params, .. }
                    if kg_hir::rewrite::is_abbreviated_template(&m.ir.arena, *params) =>
                {
                    continue;
                }
                Exp::Struct { scope, name, .. } | Exp::Union { scope, name, .. } => {
                    insert(scope, ident(*name), false);
                    continue;
                }
                Exp::Class { scope, name, .. } => {
                    insert(scope, ident(*name), false);
                    insert(scope, ident(*name), true);
                }
                Exp::Alias { scope, name, .. }
                | Exp::Capture { scope, name }
                | Exp::Function { scope, name, .. }
                | Exp::FunctionDecl { scope, name, .. }
                | Exp::Enum { scope, name, .. }
                | Exp::EnumConstant { scope, name, .. }
                | Exp::Variable { scope, name, .. }
                | Exp::FuncParam { scope, name, .. } => insert(scope, ident(*name), false),
                _ => {}
            }
            kids.clear();
            exp.children(&mut kids);
            // Pre-order, earlier sibling first: the stack pops the last push.
            stack.extend(kids.iter().rev());
        }
    }
    map
}

impl<B: Backend> std::fmt::Debug for Walker<'_, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Walker").finish_non_exhaustive()
    }
}

type Walked<N> = Result<N, Declined>;

/// A type with the `TInstance` note around a class, struct, union or
/// function copy taken off, through `TType` and `const` — the convention
/// the twin's tables held, which the walk's shape tests read (`expFromType`
/// and `tReturn` look through the note, `Type.hs:1638`, `:577`). An alias
/// or closure copy's note stays: the twin carried those.
fn copy_notes_stripped(t: &Ty) -> Ty {
    match t {
        Ty::Instance { concrete, .. }
            if matches!(**concrete, Ty::Class(..) | Ty::Struct(..) | Ty::Union(..) | Ty::Function { .. }) =>
        {
            (**concrete).clone()
        }
        Ty::Type(x) => Ty::Type(kg_ty::TyRef::new(copy_notes_stripped(x))),
        Ty::Const(x) => Ty::Const(kg_ty::TyRef::new(copy_notes_stripped(x))),
        other => other.clone(),
    }
}

fn integer_type(ty: &Ty) -> Option<(bool, u32)> {
    match ty {
        // `tIsInt` and `tEnumBase` look through `TInstance` (`maybeInner`,
        // `Type.hs:596-601`): an alias copy's `index_t<32>` is `uint5`.
        Ty::Const(inner) | Ty::Instance { concrete: inner, .. } => integer_type(inner),
        Ty::Signed(w) => Some((true, *w)),
        Ty::Unsigned(w) => Some((false, *w)),
        Ty::Enum(_, base) => integer_type(base),
        _ => None,
    }
}


/// One Haskell-style character escape (`Text.Megaparsec.Char.Lexer.charLiteral`)
/// at `i`, which points just past the backslash: the byte and the index
/// after it. The escapes the corpus uses; anything else is kept verbatim
/// with its backslash, which is what a mismatch would show as.
fn char_escape(bytes: &[u8], i: usize, out: &mut Vec<u8>) -> usize {
    let Some(&c) = bytes.get(i) else {
        out.push(b'\\');
        return i;
    };
    match c {
        b'n' => out.push(b'\n'),
        b't' => out.push(b'\t'),
        b'r' => out.push(b'\r'),
        b'0' if !bytes.get(i + 1).is_some_and(u8::is_ascii_digit) => out.push(0),
        b'\\' => out.push(b'\\'),
        b'"' => out.push(b'"'),
        b'\'' => out.push(b'\''),
        b'a' => out.push(7),
        b'b' => out.push(8),
        b'f' => out.push(12),
        b'v' => out.push(11),
        b'x' => {
            let mut j = i + 1;
            let mut v: u32 = 0;
            while let Some(d) = bytes.get(j).and_then(|b| (*b as char).to_digit(16)) {
                v = v * 16 + d;
                j += 1;
            }
            push_char(v, out);
            return j;
        }
        b'0'..=b'9' => {
            let mut j = i;
            let mut v: u32 = 0;
            while let Some(d) = bytes.get(j).and_then(|b| (*b as char).to_digit(10)) {
                v = v * 10 + d;
                j += 1;
            }
            push_char(v, out);
            return j;
        }
        other => {
            out.push(b'\\');
            out.push(other);
        }
    }
    i + 1
}

fn push_char(v: u32, out: &mut Vec<u8>) {
    match char::from_u32(v) {
        Some(c) => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        None => out.push(b'?'),
    }
}

/// A string's text before a hole, decoded (`charLiteral`, `Parser.hs:892`):
/// the opening quote dropped from the first piece and the closing one from
/// the last, escapes read, and `{{` one brace.
pub fn decode_piece(raw: &str, first: bool, last: bool) -> Vec<u8> {
    let mut bytes = raw.as_bytes();
    if first {
        bytes = bytes.strip_prefix(b"\"").unwrap_or(bytes);
    }
    if last {
        bytes = bytes.strip_suffix(b"\"").unwrap_or(bytes);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = char_escape(bytes, i + 1, &mut out),
            b'{' if bytes.get(i + 1) == Some(&b'{') => {
                out.push(b'{');
                i += 2;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// `format` (`Parser.hs:900-903`) over a hole's text after the `:` — `Format
/// <$> choiceViaShow [Bin, Oct, Dec, Hex, HexUpper] <*> optional L.decimal`,
/// then the lexeme's trailing space — as `(ParseTreeFormatSpecifier,
/// precision)`, the enum as `parse_tree.h:152-160` numbers it. `None` for
/// anything the grammar rejects; pass 0 has reported that as a parse error.
pub fn parse_format(text: &str) -> Option<(u32, Option<u32>)> {
    let text = text.trim();
    let mut chars = text.chars();
    let spec = match chars.next()? {
        'b' => 0,
        'o' => 1,
        'd' => 2,
        'x' => 3,
        'X' => 4,
        _ => return None,
    };
    let digits = chars.as_str();
    if digits.is_empty() {
        return Some((spec, None));
    }
    Some((spec, Some(digits.parse::<u32>().ok()?)))
}

/// `stringLiteral` for an attribute argument (`Parser.hs:821`): escapes
/// decoded, no holes.
pub fn plain_string(raw: &str) -> Vec<u8> {
    let bytes = raw.as_bytes();
    let inner = if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        &bytes[1..bytes.len() - 1]
    } else {
        bytes
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < inner.len() {
        if inner[i] == b'\\' {
            i = char_escape(inner, i + 1, &mut out);
        } else {
            out.push(inner[i]);
            i += 1;
        }
    }
    out
}

/// The constructor name of a type, for a decline reason.
fn ty_head(ty: &Ty) -> String {
    let text = format!("{ty:?}");
    text.split(['(', ' ', '{']).next().unwrap_or("?").to_owned()
}

impl<'a, B: Backend> Walker<'a, B> {
    pub fn new(
        backend: &'a mut B,
        interner: &'a Interner,
        symbols: &'a kg_db::SymbolMap,
        graph: &'a kg_resolve::module::ModuleGraph,
        items: kg_resolve::lower::Program<'a>,
        modules: &'a [ModuleCtx],
    ) -> Walker<'a, B> {
        let post = post_symbols(modules, interner);
        Walker {
            backend,
            interner,
            symbols,
            graph,
            items,
            modules,
            pending_src: None,
            enum_values: rustc_hash::FxHashMap::default(),
            frames: Vec::new(),
            scopes: Vec::new(),
            conversions: Conversions::default(),
            returns: Vec::new(),
            post,
            template_functions: template_functions(modules),
            declared_copies: declared_copies(modules, interner),
        }
    }

    /// `lookupInScope` (`Symbols.hs:82-87`) over [`post_symbols`]: the scope's
    /// prefixes, longest first.
    fn lookup_post(&self, scope: &kg_base::SymbolPath, name: &[kg_base::Symbol]) -> Option<(Module<'a>, ExpId)> {
        let mut prefix: Vec<kg_base::Symbol> = scope.segments().to_vec();
        loop {
            let mut key = prefix.clone();
            key.extend_from_slice(name);
            if let Some(&(index, id)) = self.post.get(&key) {
                return Some((self.modules[index].view(), id));
            }
            prefix.pop()?;
        }
    }

    /// The type of the declaration the walked tree files under `path` — the
    /// symbol map `unmangleQualifiedName` reads (`Type.hs:180-187`),
    /// original names throughout.
    ///
    /// A copy's is the note `setTypeOfInstance` wrote at it — the merged
    /// table's entry at the copy's own key, where the twin's reading of
    /// the declaration is its function or class type.
    /// What `lookupSymbolByName` answers for `unmangleQualifiedName`
    /// (`Type.hs:198-205`): the declaration's instance note, read off the
    /// loop's own notes by node ([`Self::loop_note`]) — a function hoisted
    /// out of a lambda shares its head with the `Closure` copy made over it,
    /// and a note read by head would be the copy's — else its type.
    fn declared_type(&self, path: &[kg_base::Symbol]) -> Option<Ty> {
        let &(index, id) = self.post.get(path)?;
        let dm = self.modules[index].view();
        match Self::loop_note(dm, id) {
            Some(t @ Ty::Instance { .. }) => Some(t),
            _ => Self::type_of(dm, id),
        }
    }

    fn module_ctx(&self, module: kg_resolve::ModuleSource) -> Option<&'a ModuleCtx> {
        self.modules.iter().find(|c| c.module == module)
    }

    /// A `const` variable's initializer, in its own module, for the
    /// `NamedValueF` arm of `evaluateConstexpr` (`Constexpr.hs:252-260`):
    /// the qualified identifier's definition, if it is a variable with an
    /// initializer, as `(module, variable, initializer)`.
    fn const_init(&self, m: Module<'_>, qi: ExpId) -> Option<(Module<'a>, ExpId, ExpId)> {
        let Exp::QualifiedIdentifier(inner) = &m.ir.arena.node(qi).exp else { return None };
        let (scope, qualifier, ident) = Self::written_path(m, *inner)?;
        let Exp::Identifier(sym) = &m.ir.arena.node(ident).exp else { return None };
        let scope = scope.clone();
        let name = kg_base::SymbolPath::new(qualifier.iter().copied().chain([*sym]));
        let def = self.symbols.lookup_in_scope(&scope, &name)?;
        let item = self.items.item(def)?;
        if !matches!(item.kind, kg_resolve::DefKind::Variable(_)) {
            return None;
        }
        let ctx = self.module_ctx(def.module)?;
        let var = *ctx.variables.get(&item.range.range)?;
        let Exp::Variable { init: Some(init), .. } = &ctx.ir.arena.node(var).exp else { return None };
        Some((ctx.view(), var, *init))
    }

    /// Walk `id` in module `m` with its top node wearing `src` instead of
    /// its own.
    fn node_with_src(&mut self, m: Module<'_>, id: ExpId, src: Src) -> Walked<B::Node> {
        self.pending_src = Some(src);
        self.node(m, id)
    }

    /// `SetLocation2`/`UnknownLocation` for the node's own span: 1-based,
    /// end exclusive, as megaparsec's `SourcePos` pair reaches the wire
    /// (`Types.hsc:31-37`).
    ///
    /// A synthesized node is located at its donor, whose note the
    /// reference's rewrites copy onto what they make (`copyNote`): the
    /// closure cast around a wrapped argument stands at the argument
    /// (`higher-order-functions.k::0`'s recorded trace), and an
    /// assertion's call trace names the file of a wrapper's call. Measured
    /// 2026-09-24: `cargo xtask output` 156 → 166 identical, location
    /// comments agreeing 31% → 38%, nothing newly different. Only a donor
    /// in this module's own file — this walk's line index — is answerable.
    fn set_location(&mut self, m: Module<'_>, src: &Src) {
        // `Reread` is its own text, so its position is answerable — the
        // reference gives `x++`'s second reading the same span as the first.
        if src.written == Written::Donated && m.module.file() != Some(src.head.file) {
            self.backend.UnknownLocation();
            return;
        }
        let begin = m.lines.line_col(src.head.range.start());
        let end = m.lines.line_col(src.head.range.end());
        let loc = Location {
            begin_line: begin.line as usize + 1,
            begin_column: begin.col as usize + 1,
            end_line: end.line as usize + 1,
            end_column: end.col as usize + 1,
            file: m.file_index,
            valid: true,
        };
        self.backend.SetLocation2(Some(&loc));
    }

    /// `typeOf e`: the loop's type at the node (`kg_sem`'s inference over the
    /// loop's own tree), unresolved and undefined included. Until 2026-10-10
    /// an unresolved answer fell through to the CST engine's table, which
    /// decided nothing (`ptdiff` and `backend` identical without it, W3.6).
    fn type_of(m: Module<'_>, id: ExpId) -> Option<Ty> {
        let t = m.sem.types.get(id);
        Some(match t {
            Ty::Unresolved | Ty::Undefined => t,
            _ => copy_notes_stripped(&t),
        })
    }

    /// The loop's own note at `id` — its `TypeMap`, keyed by node, not by
    /// `TypeKey`, whose head a function hoisted out of a lambda shares with
    /// the `Closure` copy made over it. `None` where the loop wrote none; a
    /// `TType` unwrapped.
    fn loop_note(m: Module<'_>, id: ExpId) -> Option<Ty> {
        if !m.sem.types.is_set(id) {
            return None;
        }
        match m.sem.types.get(id) {
            Ty::Type(t) => Some((*t).clone()),
            other => Some(other),
        }
    }

    /// The note at a declaration as written — `setTypeOfInstance`'s
    /// `TInstance` included, which [`Self::type_of`] presents stripped: the
    /// loop's, which holds the note of every copy (the CST engine's table
    /// was asked first until 2026-10-10, and decided nothing the loop did
    /// not, W3.6).
    fn note(m: Module<'_>, id: ExpId) -> Ty {
        if m.sem.types.is_set(id) { m.sem.types.get(id) } else { Ty::Unresolved }
    }

    /// `parseWithLoc`'s two `setNodeType` lines.
    fn set_node_type(&mut self, m: Module<'_>, id: ExpId, node: B::Node) {
        // The loop types the node (`table`); every fallback below answers
        // only where it did not. None fired over `ptdiff`'s 2422 programs
        // (instrumented, 2026-10-10): they are kept as insurance, since a
        // node left untyped aborts the backend on a program the reference
        // compiles (`single-source-of-truth.md`, the register).
        let table = Self::type_of(m, id);
        let ty = table
            .as_ref()
            .and_then(integer_type)
            // A declaration whose written type is `int<e>` with `e` the
            // table never folded — `int<16 - bitsizeof(S1_t)> q;` in a
            // struct — is typed by that type all the same (`inferType` on
            // a `Variable` is its declared type), and the walk has just
            // spelled it from the fold: read it off the written type.
            .or_else(|| match &m.ir.arena.node(id).exp {
                // An `auto` one the table could not type whose initializer
                // folds: the literal's type, as [`Self::deduced_type`]
                // spells it (`constexpr.k::174`).
                Exp::Variable { ty, .. } => Self::written_integer_type(m, *ty)
                    .or_else(|| Self::folded_initializer_type(m, id).as_ref().and_then(integer_type)),
                Exp::FuncParam { ty, .. } => Self::written_integer_type(m, *ty),
                // A literal no table typed: `inferIntLiteralType Nothing i`
                // (`Type.hs:485-491`) — pass 6's unrolled induction
                // variable, `IntLiteralF Nothing i` (`Constexpr.hs:106`),
                // which the next inference round types by its value
                // (`static-for.k::0`) — or one the table left unresolved,
                // in a declaration of a spliced top-level `static if` block
                // (`static-if.k::30`, whose untyped literal aborted the
                // backend). The reference types every such literal by its
                // value.
                Exp::IntLiteral { ty: None, value } if !table.as_ref().is_some_and(Ty::resolved) => {
                    integer_type(&kg_ty::width::minimal_int_type(value))
                }
                // A typed literal is its type (`inferIntLiteralType (Just
                // t)`, `Type.hs:485`): the value argument a copy substitutes
                // for `T X` with `T = int32` — `validateArg` re-stamps it
                // (`TypeCheck.hs:281-283`) — whose node the copy's table
                // never typed (`type-check.k::22`, which aborted the backend).
                Exp::IntLiteral { ty: Some(lt), value } => integer_type(&kg_ty::width::infer_int_literal_type(
                    &kg_const::ConstValue::Int { ty: Some(*lt), value: value.clone() },
                )),
                // A binary operator the *lowering* synthesized — `x++` is
                // `Assign(x, x + 1)` and the `+` stands at the lvalue's range
                // with the lvalue's kind, so no `TypeKey` is its own and
                // inference over the CST can never have typed it. The
                // reference's tree has a node there and `inferBinaryOp` typed
                // it (`Type.hs:746-771`), so the operands' types are read back
                // and the same rule run: `uint32 + uint1` is `uint33`
                // (`static-if.k::17`'s `x++`, the recorded walk).
                // A local's name no table typed: a copy's live `static if`
                // branch the twin's inference could not decide — `print(auto
                // x)`'s `__print("{x}")` under `decltype(x) == string` — which
                // the reference decides and types (`staticControl`,
                // `Constexpr.hs:44-47`). `infer (NamedValueF a) = typeOf a`,
                // and a local's type is its declaration's.
                Exp::NamedValue(_) if !table.as_ref().is_some_and(Ty::resolved) => self.local_type(m, id),
                // …and one the table has a key for but left unresolved: the
                // product in a hoisted local function's `return i * i`
                // (`decltype.k::10`), which the reference types `uint10`.
                Exp::Binary(..) if !table.as_ref().is_some_and(Ty::resolved) => {
                    self.operand_type(m, id).as_ref().and_then(integer_type)
                }
                // `infer (ReturnF (Just a)) = tUnConst $ typeOf a`
                // (`Type.hs:1250-1251`), where the table left the statement
                // unresolved because it left the value so.
                Exp::Return(Some(value)) if !table.as_ref().is_some_and(Ty::resolved) => {
                    self.operand_type(m, *value).map(|t| t.un_const()).as_ref().and_then(integer_type)
                }
                _ => None,
            });
        if let Some((signed, width)) = ty {
            let ty = self.integer_node(signed, width);
            self.backend.SetNodeType(node, ty);
        }
    }

    /// The type the reference gives `id`, for [`Walker::set_node_type`]'s
    /// fallbacks: the table's when it resolved one, else the reference's own
    /// rule — the literal rules, a folded name's literal, a local's declared
    /// type, and `inferBinaryOp` over the operands' (`Type.hs:746-771`),
    /// recursively.
    fn operand_type(&self, m: Module<'_>, id: ExpId) -> Option<Ty> {
        if let Some(t) = Self::type_of(m, id)
            && t.resolved()
        {
            return Some(t);
        }
        if let Exp::Binary(op, lhs, rhs) = &m.ir.arena.node(id).exp {
            let (op, lhs, rhs) = (*op, *lhs, *rhs);
            let lt = self.operand_type(m, lhs)?;
            let rt = self.operand_type(m, rhs)?;
            return Some(kg_ty::width::t_binary_op(
                op,
                kg_ty::width::type_of_operand(&lt),
                kg_ty::width::type_of_operand(&rt),
                Some(self.interner),
            ));
        }
        // A name the fold replaced: the walk passes the literal, which the
        // reference types by its value (`x += y` over `const uint32 y = 10`
        // is `uint32 + uint4`, `static-if.k::50`).
        if let Some(v @ kg_const::ConstValue::Int { .. }) = Self::folded(m, id) {
            return Some(kg_ty::width::infer_int_literal_type(&v));
        }
        match &m.ir.arena.node(id).exp {
            Exp::IntLiteral { ty: None, value } => Some(kg_ty::width::minimal_int_type(value)),
            // The local's declared type, as [`Walker::set_node_type`]'s
            // `NamedValue` arm reads it: `x++` over a `static uint32 x`
            // declared in a spliced `static if` block, which the table
            // leaves untyped and the reference types `uint32`, so its `+`
            // is `uint33` (`static-if.k::20` and ten more that aborted the
            // backend on the untyped operator).
            Exp::NamedValue(_) => {
                self.local_type(m, id).map(|(signed, n)| if signed { Ty::Signed(n) } else { Ty::Unsigned(n) })
            }
            _ => None,
        }
    }

    /// A written type that is an integer type this walk can spell: a
    /// fixed width, or `int<e>`/`uint<e>` with `e` folded, through `const`.
    fn written_integer_type(m: Module<'_>, ty: ExpId) -> Option<(bool, u32)> {
        match &m.ir.arena.node(ty).exp {
            Exp::Integer(n) => Some((true, *n)),
            Exp::Unsigned(n) => Some((false, *n)),
            Exp::Const(inner) => Self::written_integer_type(m, *inner),
            Exp::ParamInt(e) | Exp::ParamUint(e) => {
                let signed = matches!(m.ir.arena.node(ty).exp, Exp::ParamInt(_));
                let value = match Self::folded(m, *e) {
                    Some(kg_const::ConstValue::Int { value, .. }) => value,
                    _ => match &m.ir.arena.node(*e).exp {
                        Exp::IntLiteral { value, .. } => value.clone(),
                        _ => return None,
                    },
                };
                Some((signed, value.to_u32()?))
            }
            _ => None,
        }
    }

    fn integer_node(&mut self, signed: bool, width: u32) -> B::Node {
        if signed { self.backend.ParseIntType(width as i32) } else { self.backend.ParseUintType(width as i32) }
    }

    /// A name as the reference spells it.
    fn name(&self, sym: kg_base::Symbol) -> String {
        self.interner.resolve(sym).to_string()
    }

    /// `withQualifiedName`: the scope's segments, outer to inner, as
    /// `toString` spells each `Name`.
    fn scope(&self, path: &kg_base::SymbolPath) -> Vec<String> {
        path.segments().iter().map(|s| self.name(*s)).collect()
    }

    /// The function type a class variable's written name stands for —
    /// see the `Variable` arm.
    fn aliased_callback(m: Module<'_>, ty: ExpId, flags: kg_resolve::DeclareFlag) -> Option<Ty> {
        if flags != kg_resolve::DeclareFlag::DeclareClass || !matches!(m.ir.arena.node(ty).exp, Exp::TypeIdentifier(..)) {
            return None;
        }
        let t = match Self::type_of(m, ty)? {
            Ty::Type(t) => (*t).clone(),
            other => other,
        };
        // `tIsFunction` looks through an alias copy's `TInstance`, and
        // `expFromType (TInstance t _ _) = typ t` spells what it wraps
        // (`Type.hs:1637`): `callback_T_to_R_t<uint32, uint32>` is the
        // function type (`callbacks.k::23`, the recorded walk).
        let t = match t {
            Ty::Instance { concrete, .. } => (*concrete).clone(),
            t => t,
        };
        matches!(t.un_const(), Ty::Function { .. }).then_some(t)
    }

    /// `withUnmangledName` (`ParseTree.hs:176-179`): a declaration whose
    /// note is an instance is named `show (unmangleInstanceName symbols
    /// …)` of it (`Type.hs:193-205`) — the instance's template and its
    /// arguments, each through `unmangle`, never its mangled name; any other
    /// declaration, `NULL`. What the note holds is `setTypeOfInstance`'s
    /// (`kg_hir::rewrite::instantiate`), the first copy's for a copy of a
    /// copy.
    fn unmangled(&self, m: Module<'_>, decl: ExpId) -> Option<String> {
        let note @ Ty::Instance { .. } = Self::loop_note(m, decl)? else { return None };
        kg_ty::subtype::unmangle_instance_name_with(&note, Some(self.interner), &|path| self.declared_type(path))
    }

    /// `unmangleInstanceName` of an alias whose type is an instance
    /// (`tIsInstance $ theType expr`, `ParseTree.hs:178`; `Type.hs:665`
    /// looks through nothing, not even `const`).
    ///
    /// The table types a mention of a copy as the copy's own declaration —
    /// `Class([@helper@constexpr, C<|@2|>], …)` for `C<2>` — where the
    /// reference holds the `TInstance` `setTypeOfInstance` wrote: so the
    /// named declaration's own note decides ([`Self::declared_type`]), as
    /// `unmangleQualifiedName` looks the name up (`Type.hs:198-205`) — an
    /// instance is displayed from its arguments, anything else is no
    /// instance (`callbacks.k::38`: `using foo1 =
    /// Foo<callback_uint32_to_uint32_t>` is `Foo<(uint32) -> uint32>`).
    fn aliased_instance(&self, m: Module<'_>, id: ExpId) -> Option<String> {
        let ty = match (Self::type_of(m, id)?).clone() {
            Ty::Type(t) => (*t).clone(),
            other => other,
        };
        let instance = match &ty {
            Ty::Instance { .. } => Some(ty.clone()),
            Ty::Class(name, _) | Ty::Struct(name, _) | Ty::Union(name, _) => self.declared_type(name.segments()),
            // An alias of an alias template's copy — `using type_t =
            // count_t<4>;` — is typed through to what the copy aliases
            // (`uint3`) here, and as the copy's instance in the reference:
            // the mention names the copy, whose note it reads
            // (`higher-order-functions.k::0`, `"count_t<4>"`).
            _ => {
                let Exp::Alias { body, .. } = &m.ir.arena.node(id).exp else { return None };
                let Exp::TypeIdentifier(_, name) = &m.ir.arena.node(*body).exp else { return None };
                let (_, mut written, ident) = Self::written_path(m, *name)?;
                let Exp::Identifier(leaf) = &m.ir.arena.node(ident).exp else { return None };
                written.push(*leaf);
                let path: Vec<kg_base::Symbol> = written.into_iter().collect();
                self.declared_type(&path)
            }
        };
        let instance @ Ty::Instance { .. } = instance? else { return None };
        kg_ty::subtype::unmangle_instance_name_with(&instance, Some(self.interner), &|path| self.declared_type(path))
    }

    fn decline(reason: impl Into<String>, src: &Src) -> Declined {
        Declined { reason: reason.into(), at: *src }
    }

    /// `parseInstanceType` for a copy named `Template<|…|>`: the template
    /// found beside the copy by its simple name, its parameters' names
    /// against the arguments read back off the mangle. `Ok(None)` for a
    /// class that is no copy — and for a copy whose template or arguments
    /// this cannot read back (an array-typed argument, `$array…`): the
    /// backend takes a NULL instance type where it took one before this
    /// arm existed, and `ptdiff` counts the divergence, which is the
    /// honest instrument for the walk.
    /// `parseInstanceType` (`ParseTree.hs:237-253`) off the copy's own type,
    /// as the reference reads it: `tInstanceTemplate` by its qualified name,
    /// and per `tInstanceArgs` a struct, enum or union by its qualified
    /// name, an integer type by its width, an integer or a string by its
    /// literal, and anything else — a function type
    /// (`same_types<[[max_threads(2)]] (uint3) -> void, …>`,
    /// `type-constexpr.k::16`), a class, `bool` — NULL. `None` for a class
    /// that is no instance. The mangled name was read back here once; it
    /// cannot split a `$fun` argument from the next, and with the note read
    /// it moved no program (2026-09-19).
    fn typed_instance(&mut self, m: Module<'_>, decl: ExpId) -> Option<B::Node> {
        // The note `setTypeOfInstance` wrote at the copy's root, as
        // [`Walker::declared_type`] reads it: the twin types the copy as the
        // class its text declares.
        let ty = match Self::note(m, decl) {
            Ty::Type(t) => (*t).clone(),
            other => other,
        };
        let Ty::Instance { template, args, .. } = (ty).clone() else { return None };
        let template = self.scope(&template);
        let args: Vec<(String, kg_ty::TArg)> = args.iter().map(|(n, a)| (self.name(*n), a.clone())).collect();
        let refs: Vec<&str> = template.iter().map(String::as_str).collect();
        let name_node = self.backend.ParseQualifiedName(&refs);
        let null = self.backend.null();
        let list = self.backend.ParseBaseList(null);
        for (param, arg) in &args {
            let node = match arg {
                // An argument naming a copy is typed as the copy's note
                // (`setTypeOfInstance`'s `TInstance`) in the IR inference's
                // tree; `parseInstanceType` reads the concrete type under it.
                kg_ty::TArg::Type(t) => match t.concrete() {
                    Ty::Struct(q, _) | Ty::Enum(q, _) | Ty::Union(q, _) => {
                        let segments = self.scope(q);
                        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
                        self.backend.ParseQualifiedName(&refs)
                    }
                    Ty::Unsigned(n) => self.backend.ParseUintType(*n as i32),
                    Ty::Signed(n) => self.backend.ParseIntType(*n as i32),
                    _ => null,
                },
                kg_ty::TArg::Int(v) => self.backend.ParseDecimalLiteral(Some(&v.to_string())),
                kg_ty::TArg::Str(v) => self.backend.ParseStringLiteral(Some(v)),
                _ => null,
            };
            let arg_node = self.backend.ParseTemplateArg(Some(param), node);
            self.backend.ParseAppendList(list, arg_node);
        }
        Some(self.backend.ParseTemplateInstance(name_node, list))
    }

    /// A function's parameters and every `Variable` declared in its body,
    /// by name — the declarations a hole in one of its strings can name.
    /// The integer type of the one local of the enclosing function a
    /// `NamedValue` names, by its declaration — `None` when the name is not a
    /// local, or two locals of the function share it (which one is in scope
    /// is a question this flat list cannot answer).
    fn local_type(&self, m: Module<'_>, id: ExpId) -> Option<(bool, u32)> {
        let mut at = id;
        let name = loop {
            match &m.ir.arena.node(at).exp {
                Exp::NamedValue(e) | Exp::QualifiedIdentifier(e) => at = *e,
                Exp::ScopedName { ident, .. } => at = *ident,
                Exp::Identifier(sym) => break *sym,
                _ => return None,
            }
        };
        let mut decls = self.frames.last()?.iter().filter(|(sym, _)| *sym == name);
        let (_, decl) = decls.next()?;
        if decls.next().is_some() {
            return None;
        }
        Self::type_of(m, *decl).as_ref().and_then(integer_type).or_else(|| match &m.ir.arena.node(*decl).exp {
            Exp::Variable { ty, .. } | Exp::FuncParam { ty, .. } => Self::written_integer_type(m, *ty),
            _ => None,
        })
    }

    fn frame(m: Module<'_>, params: ExpId, body: ExpId) -> Vec<(kg_base::Symbol, ExpId)> {
        let mut out = Vec::new();
        let ident = |id: ExpId| match &m.ir.arena.node(id).exp {
            Exp::Identifier(sym) => Some(*sym),
            _ => None,
        };
        if let Exp::Seq(items) = &m.ir.arena.node(params).exp {
            for &p in items {
                if let Exp::FuncParam { name, .. } = &m.ir.arena.node(p).exp
                    && let Some(sym) = ident(*name)
                {
                    out.push((sym, p));
                }
            }
        }
        let mut stack = vec![body];
        let mut children = Vec::new();
        while let Some(id) = stack.pop() {
            if let Exp::Variable { name, .. } = &m.ir.arena.node(id).exp
                && let Some(sym) = ident(*name)
            {
                out.push((sym, id));
            }
            m.ir.arena.node(id).exp.children(&mut children);
            stack.append(&mut children);
        }
        out
    }

    /// `InterpolatedString a -> parseList parseSegment a >>=
    /// parseInterpolatedString` (`ParseTree.hs:125,217-226`), with
    /// `interpolatedStringPre`'s echo (`Frontend.hs:236-237`) and
    /// `interpolatedStringPost`'s default precision (`:245-254`) applied on
    /// the way.
    ///
    /// In `cataM` order every hole's expression, then its alignment, is
    /// walked before the list is opened — they are the children of
    /// `InterpolatedStringF` — and each is an ordinary node of the tree.
    fn interpolated_string(&mut self, m: Module<'_>, segments: &[kg_hir::Segment], src: &Src) -> Walked<B::Node> {
        let mut walked: Vec<(B::Node, Option<B::Node>)> = Vec::new();
        for hole in segments.iter().filter_map(|s| s.hole.as_ref()) {
            let value = self.node(m, hole.expr)?;
            let align = match hole.align {
                Some(a) => Some(self.node(m, a)?),
                None => None,
            };
            walked.push((value, align));
        }
        let last = segments.len() - 1;
        // Each segment's decoded text, and its hole's `(specifier, precision)`.
        type Piece = (Vec<u8>, Option<(u32, u32)>);
        let mut texts: Vec<Piece> = Vec::with_capacity(segments.len());
        for (k, segment) in segments.iter().enumerate() {
            let mut text = decode_piece(&segment.text, k == 0, k == last);
            let Some(hole) = &segment.hole else {
                // `xs <- (lookAhead (char '"') >> pure []) <|> stringSegments`
                // (`Parser.hs:888`): a string that ends right after a hole has
                // no trailing segment.
                if !text.is_empty() {
                    texts.push((text, None));
                }
                continue;
            };
            // `update (s, Just (a, True, b, f)) = (s ++ showPretty a ++ " = ", …)`.
            if hole.echo {
                let Some(shown) = self.echo_text(m, hole.expr) else {
                    return Err(Self::decline("interpolation hole: echo", src));
                };
                text.extend_from_slice(shown.as_bytes());
                text.extend_from_slice(b" = ");
            }
            let (spec, precision) = match &hole.format {
                None => (5, 0),
                Some(format) => {
                    let Some((spec, precision)) = parse_format(format) else {
                        return Err(Self::decline("interpolation hole: format", src));
                    };
                    // `updatePrecision`: a missing precision from `sizeOf a`
                    // — `n` bits for `b`, `(n+2)/3` for `o`, `(n+3)/4` for `x`
                    // and `X`, none for `d` — then `fromMaybe 0`.
                    let precision = match precision {
                        Some(p) => p,
                        None if spec == 2 => 0,
                        None => {
                            let width = Self::type_of(m, hole.expr)
                                .as_ref()
                                .and_then(kg_ty::subtype::bit_sizeof)
                                .and_then(|n| u32::try_from(n).ok());
                            let Some(n) = width else {
                                return Err(Self::decline("interpolation hole: precision", src));
                            };
                            match spec {
                                0 => n,
                                1 => n.div_ceil(3),
                                _ => n.div_ceil(4),
                            }
                        }
                    };
                    (spec, precision)
                }
            };
            texts.push((text, Some((spec, precision))));
        }
        // `parseList`, one segment at a time (`ParseTree.hs:217`).
        let null = self.backend.null();
        self.set_location(m, src);
        let root = self.backend.ParseBaseList(null);
        let mut next_hole = walked.into_iter();
        for (text, hole) in texts {
            let text = String::from_utf8_lossy(&text).into_owned();
            let s = self.backend.ParseStringLiteral(Some(&text));
            let e = match hole {
                Some((spec, precision)) => {
                    let (value, align) = next_hole.next().expect("one walked hole per hole");
                    self.backend.ParseInterpolationExpression(value, align.unwrap_or(null), spec, precision)
                }
                None => null,
            };
            let segment = self.backend.ParseInterpolatedStringSegment(s, e);
            self.backend.ParseAppendList(root, segment);
        }
        Ok(self.backend.ParseInterpolatedString(root))
    }

    /// `showPretty a` for an echoed hole, for the shapes whose rendering is
    /// fixed by the text: a name, as `resolveNames` leaves a local or a
    /// constant. Anything else declines rather than guess the pretty
    /// printer's spacing.
    fn echo_text(&self, m: Module<'_>, id: ExpId) -> Option<String> {
        let mut id = id;
        loop {
            match &m.ir.arena.node(id).exp {
                Exp::NamedValue(e) | Exp::QualifiedIdentifier(e) => id = *e,
                Exp::ScopedName { ident, .. } => id = *ident,
                Exp::Identifier(name) => return Some(self.name(*name)),
                _ => return None,
            }
        }
    }

    /// `parseList`: a fresh list, then one append per element.
    fn list(&mut self, items: &[B::Node]) -> B::Node {
        let null = self.backend.null();
        let root = self.backend.ParseBaseList(null);
        for &item in items {
            self.backend.ParseAppendList(root, item);
        }
        root
    }

    /// The items of a module's root `Seq`, walked; the program's root list is
    /// the concatenation over modules (`Main.hs:80,106`).
    pub fn module_items(&mut self, m: Module<'_>) -> Walked<Vec<B::Node>> {
        let root = m.ir.arena.node(m.ir.root);
        let Exp::Seq(items) = &root.exp else {
            return Err(Self::decline("root is not a Seq", &root.src));
        };
        let mut out = Vec::new();
        // The parser seeds every file's `Seq` with an empty `Seq` wearing
        // the file's note, one `Warning` around it per redundant import
        // (`checkRedundantImports`, `Parser.hs:234-241`). Bare, it is
        // flattened away at pass 15; wrapped, it survives, and each wrapper
        // is a `parseWithLoc` with no call of its own. A file with a header
        // is `Seq [ModuleDecl …]` (`moduleDecl`, `Parser.hs:188`), which
        // the `Seq` clause seeds (`:235`) — never the `ModuleDecl` one
        // (`:234`) — so the seed stands before the module in the file's own
        // list and is walked first, before the namespace's name
        // (`imports.k::43`); its note is the body's, which the parser noted
        // from the same offset.
        let header = match items.as_slice() {
            [only] => match &m.ir.arena.node(*only).exp {
                Exp::ModuleDecl { body, .. } => Some(*body),
                _ => None,
            },
            _ => None,
        };
        // `KG_PTBRIDGE_SEED=0` (`Verdicts::ptbridge_seed`) leaves the seed
        // out — the reading to fall back to if the loader's
        // `redundant_imports` ever disagrees with the reference again (it
        // did until `updateNeeded` was replayed whatever the warning switch
        // says; see the journal, 2026-09-17).
        if m.redundant > 0 && m.seed {
            let src = header.map_or(&root.src, |body| &m.ir.arena.node(body).src);
            self.seed(m, src, m.redundant, &mut out);
        }
        self.seq_items(m, items, &mut out)?;
        Ok(out)
    }

    fn seed(&mut self, m: Module<'_>, src: &Src, wrappers: usize, out: &mut Vec<B::Node>) {
        self.set_location(m, src);
        out.push(self.list(&[]));
        for _ in 0..wrappers {
            self.set_location(m, src);
        }
    }

    /// `reifyValues` (`Frontend.hs:199-207`) over an enum's constants: a
    /// constant without a value gets the previous one's plus one, `0`
    /// first, each `increment` folded to the literal the reference's fold
    /// leaves. An explicit value that does not fold leaves the constants
    /// after it undecided.
    fn reify_enum(&mut self, m: Module<'_>, body: ExpId) {
        let Exp::Seq(items) = &m.ir.arena.node(body).exp else { return };
        let mut next: Option<kg_const::ConstInt> = Some(kg_const::ConstInt::ZERO);
        for &item in items {
            let Exp::EnumConstant { value, .. } = &m.ir.arena.node(item).exp else { continue };
            let this = match value {
                Some(v) => match Self::folded(m, *v).or_else(|| match &m.ir.arena.node(*v).exp {
                    Exp::IntLiteral { value, ty } => {
                        Some(kg_const::ConstValue::Int { ty: *ty, value: value.clone() })
                    }
                    _ => None,
                }) {
                    Some(kg_const::ConstValue::Int { value, .. }) => Some(value),
                    Some(kg_const::ConstValue::Enum { value, .. }) => Some(value),
                    _ => None,
                },
                None => {
                    if let Some(v) = &next {
                        self.enum_values.insert(item, v.clone());
                    }
                    next.clone()
                }
            };
            next = this.map(|v| v.add(&kg_const::ConstInt::ONE));
        }
    }

    /// The elements of a `Seq`, with what the reference's passes did to a
    /// sequence before the walk: a template declaration is gone
    /// (`removeTemplate`, `Frontend.hs:298-302`, `SeqF` of its diagnostics —
    /// none, for a program that compiles), an abbreviated function template
    /// with it — a declaration with no body as much as a definition (`void
    /// Do(auto x);` in an `extern` class, `extern-class.k::15,37`) — and a
    /// nested `Seq` is spliced into its parent
    /// (`removeNestedSeq`, `Desugar.hs:132-137`, applied by `removeTemplate`
    /// and by the folder's default arm).
    fn seq_items(&mut self, m: Module<'_>, items: &[ExpId], out: &mut Vec<B::Node>) -> Walked<()> {
        for &item in items {
            match &m.ir.arena.node(item).exp {
                Exp::Template { .. } => {}
                Exp::Function { params, .. } | Exp::FunctionDecl { params, .. }
                    if kg_hir::rewrite::is_abbreviated_template(&m.ir.arena, *params) => {}
                // `checkVarDecl`'s last clause (`TypeCheck.hs:139`): a `const`
                // of a function type, not a class's, is `SeqF []` — every
                // use of it was replaced by its initializer
                // (`Constexpr.hs:290`), `const auto fn1 = Foo;` among them
                // — and that empty sequence stays an element, as a true
                // `static assert`'s does: the recorded walk appends an empty
                // list where the declaration stood (`function-constants.k::
                // 0,2`).
                Exp::Variable { flags, .. }
                    if *flags != kg_resolve::DeclareFlag::DeclareClass && Self::a_function_constant(m, item) =>
                {
                    let src = m.ir.arena.node(item).src;
                    self.set_location(m, &src);
                    out.push(self.list(&[]));
                }
                Exp::Seq(inner) => self.seq_items(m, inner, out)?,
                // `staticAssert` (`Frontend.hs:393-398`, pass 16): a true
                // one becomes `SeqF []` wearing the assertion's note — and
                // stays an element, since `removeNestedSeq` ran at pass 15.
                // Anything else is an error the frontend already reported.
                Exp::StaticAssert(cond) => match Self::folded(m, *cond) {
                    Some(kg_const::ConstValue::Bool(true)) => {
                        let src = m.ir.arena.node(item).src;
                        self.set_location(m, &src);
                        out.push(self.list(&[]));
                    }
                    _ => return Err(Self::decline("fold:static assert", &m.ir.arena.node(item).src)),
                },
                // A `static if` is decided by the loop, at the node, as
                // inference reaches it (`staticControl`, `Constexpr.hs:41-47`;
                // `kg_hir::rewrite::static_control_node`); one still standing
                // is pass 16's error (`Frontend.hs:406`), which the reference's
                // backend never meets, and the walk declines it with every
                // other node it cannot lower (`single-source-of-truth.md`
                // D2.4).
                _ => out.push(self.node(m, item)?),
            }
        }
        Ok(())
    }

    /// The root `Seq` itself, after every module's items: its location is
    /// the last module's root note (`append` keeps the right operand's,
    /// `Main.hs:106`), then `parseList`.
    pub fn program_root(&mut self, last: Module<'_>, items: &[B::Node]) -> B::Node {
        let src = last.ir.arena.node(last.ir.root).src;
        self.set_location(last, &src);
        self.list(items)
    }


    /// `isAuto`/`isConstAuto` (`Type.hs:1600-1606`): a written type
    /// `deduceAuto` replaces.
    fn is_auto(m: Module<'_>, ty: ExpId) -> bool {
        match &m.ir.arena.node(ty).exp {
            Exp::Auto => true,
            Exp::Const(inner) => matches!(m.ir.arena.node(*inner).exp, Exp::Auto),
            _ => false,
        }
    }

    /// `typeIdentifier [] s e` (`Desugar.hs:206-207`): the qualified name as
    /// a left-nested `ScopedName` chain under a `TypeIdentifier` with an
    /// empty scope, every node wearing the donor's note.
    fn type_identifier(&mut self, m: Module<'_>, name: &kg_base::SymbolPath, donor: &Src) -> B::Node {
        self.type_identifier_in(m, name, donor, &[])
    }

    /// [`Self::type_identifier`] under the `TypeIdentifier`'s own `scope`.
    fn type_identifier_in(&mut self, m: Module<'_>, name: &kg_base::SymbolPath, donor: &Src, scope: &[&str]) -> B::Node {
        let mut list: Option<B::Node> = None;
        for &seg in name.segments() {
            self.set_location(m, donor);
            let text = self.name(seg);
            let ident = self.backend.ParseIdentifier(Some(&text));
            self.set_location(m, donor);
            list = Some(match list {
                Some(prev) => self.backend.ParseAppendList(prev, ident),
                None => self.backend.ParseBaseList(ident),
            });
        }
        let list = list.unwrap_or_else(|| self.backend.null());
        self.set_location(m, donor);
        self.backend.ParseNamedType(list, scope)
    }

    /// An integer literal the reference minted (`IntLiteralF Nothing v`) and
    /// typed with `inferIntLiteralType Nothing` — so it gets a `SetNodeType`.
    fn minted_literal(&mut self, m: Module<'_>, value: &kg_const::ConstInt, donor: &Src) -> B::Node {
        self.minted_literal_typed(m, value, donor, None)
    }

    /// [`Walker::minted_literal`], typed `(signed, width)` where the caller
    /// knows the note it wears.
    fn minted_literal_typed(
        &mut self,
        m: Module<'_>,
        value: &kg_const::ConstInt,
        donor: &Src,
        typed: Option<(bool, u32)>,
    ) -> B::Node {
        self.set_location(m, donor);
        let text = value.to_string();
        let lit = self.backend.ParseDecimalLiteral(Some(&text));
        if let Some((signed, width)) = typed.or_else(|| integer_type(&kg_ty::width::minimal_int_type(value))) {
            let ty = self.integer_node(signed, width);
            self.backend.SetNodeType(lit, ty);
        }
        lit
    }

    /// `expFromTAttr` (`Type.hs:1662-1668`) as a `Seq`, in `Set.toList`
    /// order — `TAttr`'s derived `Ord`, which [`kg_ty::TAttr`] mirrors.
    fn attribute_list(&mut self, m: Module<'_>, attrs: &kg_ty::TAttrSet, donor: &Src) -> Walked<B::Node> {
        let mut nodes = Vec::with_capacity(attrs.len());
        for attr in attrs {
            let node = match attr {
                kg_ty::TAttr::Flag(flag) => {
                    self.set_location(m, donor);
                    self.flag_attribute(*flag)
                }
                kg_ty::TAttr::Int(attr, value) => {
                    let lit = self.minted_literal(m, value, donor);
                    self.set_location(m, donor);
                    let code = Self::int_attribute_code(*attr).ok_or_else(|| Self::decline(format!("int attribute {attr:?}"), donor))?;
                    self.backend.ParseIntAttribute(code, lit)
                }
                kg_ty::TAttr::Named(attr, name, _) => {
                    // `qualifiedIdentifier [] b e`: the name as a scoped chain
                    // under a `QualifiedIdentifier`.
                    let mut list: Option<B::Node> = None;
                    for &seg in name.segments() {
                        self.set_location(m, donor);
                        let text = self.name(seg);
                        let ident = self.backend.ParseIdentifier(Some(&text));
                        self.set_location(m, donor);
                        list = Some(match list {
                            Some(prev) => self.backend.ParseAppendList(prev, ident),
                            None => self.backend.ParseBaseList(ident),
                        });
                    }
                    let list = list.unwrap_or_else(|| self.backend.null());
                    self.set_location(m, donor);
                    let qualified = self.backend.ParseScopedIdentifier(list);
                    self.set_location(m, donor);
                    let code = match attr {
                        Attribute::Ecc => 3,
                        Attribute::Rename => 8,
                        other => return Err(Self::decline(format!("attribute {other:?}"), donor)),
                    };
                    self.backend.ParseAttribute(code, qualified)
                }
            };
            nodes.push(node);
        }
        self.set_location(m, donor);
        Ok(self.list(&nodes))
    }

    fn int_attribute_code(attr: Attribute) -> Option<u32> {
        Some(match attr {
            Attribute::CallRate => 0,
            Attribute::FifoDepth => 2,
            Attribute::Latency => 3,
            Attribute::MaxThreads => 4,
            Attribute::Schedule => 5,
            Attribute::ThreadRate => 6,
            Attribute::TransactionSize => 7,
            _ => return None,
        })
    }

    /// `FlagAttr` (`ParseTree.hs:95-107`): a function modifier, a memory
    /// flag, or `atomic` as `[[schedule(1)]]`.
    fn flag_attribute(&mut self, flag: Flag) -> B::Node {
        match flag {
            Flag::Async => self.backend.ParseFunctionModifier(2),
            Flag::Atomic => {
                let one = self.backend.ParseDecimalLiteral(Some("1"));
                self.backend.ParseIntAttribute(5, one)
            }
            Flag::EndTransaction => self.backend.ParseFlagAttribute(0x20),
            Flag::Initialize => self.backend.ParseFlagAttribute(4),
            Flag::Memory => self.backend.ParseFlagAttribute(0),
            Flag::NoBackPressure => self.backend.ParseFunctionModifier(256),
            Flag::Pure => self.backend.ParseFunctionModifier(32768),
            Flag::NonReplicated => self.backend.ParseFlagAttribute(1),
            Flag::Pipelined => self.backend.ParseFunctionModifier(8),
            Flag::QuadPort => self.backend.ParseFlagAttribute(2),
            Flag::ReorderByLooping => self.backend.ParseFunctionModifier(512),
            Flag::Reset => self.backend.ParseFunctionModifier(8192),
            Flag::Unordered => self.backend.ParseFunctionModifier(32),
        }
    }

    /// `expFromType` (`Type.hs:1612-1660`): a type spelled back as the tree
    /// the parser would have built for it, walked — every node at the
    /// donor's location, none typed except the literals it mints.
    fn type_expression(&mut self, m: Module<'_>, ty: &Ty, donor: &Src) -> Walked<B::Node> {
        let node = match ty {
            Ty::Type(inner) | Ty::Dependent(inner) => return self.type_expression(m, inner, donor),
            Ty::Void => {
                self.set_location(m, donor);
                self.backend.ParseVoidType()
            }
            Ty::Boolean => {
                self.set_location(m, donor);
                self.backend.ParseBoolType()
            }
            Ty::Float => {
                self.set_location(m, donor);
                self.backend.ParseFloatType()
            }
            Ty::Str => {
                self.set_location(m, donor);
                self.backend.ParseStringType()
            }
            Ty::Signed(n) => {
                self.set_location(m, donor);
                self.backend.ParseIntType(*n as i32)
            }
            Ty::Unsigned(n) => {
                self.set_location(m, donor);
                self.backend.ParseUintType(*n as i32)
            }
            Ty::Const(inner) => {
                let e = self.type_expression(m, inner, donor)?;
                self.set_location(m, donor);
                self.backend.ParseConst(e)
            }
            Ty::Reference(name) => {
                let t = self.type_identifier(m, name, donor);
                self.set_location(m, donor);
                self.backend.ParseReference(t)
            }
            Ty::Array { attrs, elem, dims } => {
                if dims.is_empty() {
                    return Err(Self::decline("deduce: array with no dimensions", donor));
                }
                let a = self.attribute_list(m, attrs, donor)?;
                let e = self.type_expression(m, elem, donor)?;
                let mut ds = Vec::with_capacity(dims.len());
                for d in dims {
                    ds.push(self.minted_literal(m, d, donor));
                }
                self.set_location(m, donor);
                let d = self.list(&ds);
                self.set_location(m, donor);
                self.backend.ParseArrayType(a, e, d)
            }
            Ty::Function { attrs, ret, params, .. } => {
                let a = self.attribute_list(m, attrs, donor)?;
                let mut ps = Vec::with_capacity(params.len());
                for p in params {
                    let pa = self.attribute_list(m, &p.attrs, donor)?;
                    let pt = self.type_expression(m, &p.ty, donor)?;
                    let pn = match p.name {
                        Some(name) => {
                            self.set_location(m, donor);
                            let text = self.name(name);
                            self.backend.ParseIdentifier(Some(&text))
                        }
                        None => self.backend.null(),
                    };
                    self.set_location(m, donor);
                    let param = self.backend.ParseFunctionTypeParam(pa, pt, pn);
                    // `params = mapSeq typeOfParam param` (`Type.hs:1655`):
                    // each parameter is noted with its own type, which an
                    // integer's node carries (`ParseTree.hs:53-54`).
                    if let Some((signed, width)) = integer_type(&p.ty) {
                        let ty = self.integer_node(signed, width);
                        self.backend.SetNodeType(param, ty);
                    }
                    ps.push(param);
                }
                self.set_location(m, donor);
                let p = self.list(&ps);
                let r = self.type_expression(m, ret, donor)?;
                self.set_location(m, donor);
                self.backend.ParseFunctionType(a, p, r)
            }
            Ty::Closure { alias, .. } => self.type_identifier(m, alias, donor),
            // `typ (TInstance t _ _) = typ t` (`Type.hs:1638`): an instance is
            // spelled as the copy it stands for.
            Ty::Instance { concrete, .. } => return self.type_expression(m, concrete, donor),
            Ty::Enum(name, _) | Ty::Struct(name, _) | Ty::Union(name, _) | Ty::Class(name, _) => {
                self.type_identifier(m, name, donor)
            }
            other => return Err(Self::decline(format!("deduce: {}", ty_head(other)), donor)),
        };
        Ok(node)
    }

    /// `deduceAuto` (`Type.hs:1595-1610`): the written `auto` of a variable
    /// or a function's return, replaced by its inferred type spelled out.
    fn deduced_type(&mut self, m: Module<'_>, decl: ExpId, written: ExpId, function: bool) -> Walked<B::Node> {
        let donor = m.ir.arena.node(written).src;
        let mut ty = Self::type_of(m, decl).unwrap_or(Ty::Unresolved);
        if function {
            ty = match ty {
                Ty::Function { ret, .. } => (*ret).clone(),
                other => other,
            };
        } else if !ty.resolved()
            && let Some(literal) = Self::folded_initializer_type(m, decl)
        {
            // `evaluateConstexpr` replaced the initializer with its literal
            // at pass 6, and `deduceAuto` reads *that* type: `const auto x =
            // clog2(a)` over a `const a` is `const uint2` (`constexpr.k::174`),
            // `Bar::B1 * 2` over an enum its literal (`enum.k::18`) — where
            // this table, which types a `clog2` `Unresolved` and folds
            // nothing, had no answer. `infer (VariableF _ _ (Const Auto) _ a
            // _) = try (tConst . typeOf) a` (`Type.hs:1285`).
            ty = match &m.ir.arena.node(written).exp {
                Exp::Const(_) => Ty::Const(kg_ty::TyRef::new(literal)),
                _ => literal,
            };
        }
        self.type_expression(m, &ty, &donor)
    }


    /// The written qualifier and name of a `ScopedName` chain: its scope,
    /// `getQualifier` outer to inner, and the outermost link's `Identifier`
    /// node — `getName` (`Syntax.hs:746-760`).
    /// Whether `e` names an unrolled iteration's induction variable
    /// anywhere: `fixScopedName` qualifies each mention by the iteration's
    /// scope, `…::$i0::i` (`Constexpr.hs:105-108`).
    fn mentions_induction_var(&self, m: Module<'_>, e: ExpId) -> bool {
        if let Exp::NamedValue(inner) = &m.ir.arena.node(e).exp
            && let Exp::QualifiedIdentifier(name) = &m.ir.arena.node(*inner).exp
            && let Some((_, written, ident)) = Self::written_path(m, *name)
            && let Exp::Identifier(leaf) = &m.ir.arena.node(ident).exp
            && let Some(iteration) = written.last()
        {
            let leaf = self.interner.resolve(*leaf);
            let iteration = self.interner.resolve(*iteration);
            if iteration
                .strip_prefix('$')
                .and_then(|r| r.strip_prefix(&*leaf))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            {
                return true;
            }
        }
        let mut kids = Vec::new();
        m.ir.arena.node(e).exp.children(&mut kids);
        kids.into_iter().any(|k| self.mentions_induction_var(m, k))
    }

    fn written_path(m: Module<'_>, outer: ExpId) -> Option<(kg_base::SymbolPath, Vec<kg_base::Symbol>, ExpId)> {
        let Exp::ScopedName { scope, ident: outer_ident, .. } = &m.ir.arena.node(outer).exp else { return None };
        let mut names = Vec::new();
        let mut id = outer;
        loop {
            let Exp::ScopedName { prefix, ident, .. } = &m.ir.arena.node(id).exp else { return None };
            let Exp::Identifier(sym) = &m.ir.arena.node(*ident).exp else { return None };
            names.push(*sym);
            match prefix {
                Some(p) => id = *p,
                None => break,
            }
        }
        names.reverse();
        names.pop();
        Some((scope.clone(), names, *outer_ident))
    }

    /// A `QualifiedIdentifier` after `resolveNames` (`Frontend.hs:273-291`,
    /// pass 16), emitted as the chain it leaves: a function, an enum
    /// constant or a global variable is spelled with its declaring scope in
    /// full; anything else whose qualifier is a prefix of the use site's
    /// scope is spelled bare. `force_bare` is `makeUnqualified` applied from
    /// outside (a method's name after its receiver).
    ///
    /// Pass 1's `redundantNameScope` (`Frontend.hs:160-167`) is not redone
    /// here: it ran once, on the IR before the loop
    /// (`kg_sem::pass1::redundant_name_scope`), as the reference runs it
    /// before `instantiateTemplates`. Asked again at walk time inside a
    /// copy, it asked the parse-time table about the copy's scope, which
    /// that table has never held, so the lookup fell back to the enclosing
    /// scope: `risc-v.k`'s `ProgramTest` initializes `.custom_decode =
    /// CustomDecode` above its own `CustomDecode`, the parser binds the
    /// enclosing `Tests::CustomDecode` (`Parser.hs:686-688`), and the
    /// qualifier was dropped as redundant and re-resolved to the copy's
    /// member.
    fn qualified_identifier(&mut self, m: Module<'_>, id: ExpId, force_bare: bool, typed_as: Option<ExpId>) -> Walked<B::Node> {
        let data = m.ir.arena.node(id);
        let src = data.src;
        let Exp::QualifiedIdentifier(inner) = &data.exp else {
            return self.node(m, id);
        };
        let Some((scope, written, ident)) = Self::written_path(m, *inner) else {
            // A template instance or anything else: walked as written.
            let e = self.node(m, *inner)?;
            self.set_location(m, &src);
            let node = self.backend.ParseScopedIdentifier(e);
            self.set_node_type(m, typed_as.unwrap_or(id), node);
            return Ok(node);
        };
        let (written, ident_sym, expanded) = match &m.ir.arena.node(ident).exp {
            Exp::Identifier(sym) => (written.clone(), *sym, false),
            _ => return Err(Self::decline("qualified identifier without a name", &src)),
        };
        let scope = scope.clone();
        let mut qualifier: Vec<kg_base::Symbol> = written.clone();
        if force_bare {
            qualifier.clear();
        } else {
            // `resolveNames` (`Frontend.hs:273-291`) over the walked tree's
            // own symbols, copies included: a function, an enum constant or
            // a global variable is spelled with its declaring scope in full,
            // anything else whose qualifier prefixes the use site's scope
            // bare, and a name nothing declares as written.
            let name: Vec<kg_base::Symbol> = qualifier.iter().copied().chain([ident_sym]).collect();
            if let Some((dm, decl)) = self.lookup_post(&scope, &name) {
                let exp = &dm.ir.arena.node(decl).exp;
                let global = match exp {
                    Exp::Function { .. } | Exp::FunctionDecl { .. } | Exp::Lambda { .. } | Exp::EnumConstant { .. } => true,
                    Exp::Variable { flags, .. } => *flags == kg_resolve::DeclareFlag::DeclareGlobal,
                    _ => false,
                };
                if global {
                    let declared = match exp {
                        Exp::Function { scope, .. }
                        | Exp::FunctionDecl { scope, .. }
                        | Exp::Lambda { scope, .. }
                        | Exp::EnumConstant { scope, .. }
                        | Exp::Variable { scope, .. } => scope,
                        _ => unreachable!("the arms above"),
                    };
                    qualifier = declared.segments().to_vec();
                } else if scope.starts_with(&qualifier) {
                    qualifier.clear();
                }
            }
        }
        if qualifier == written && !expanded {
            let e = self.node(m, *inner)?;
            self.set_location(m, &src);
            let node = self.backend.ParseScopedIdentifier(e);
            self.set_node_type(m, typed_as.unwrap_or(id), node);
            return Ok(node);
        }
        // A rewritten chain: every minted segment wears the name's note
        // (`scopedQualifier … b`, `copyNote`), the name keeps its own.
        let ident_src = m.ir.arena.node(ident).src;
        let mut list: Option<B::Node> = None;
        for &seg in &qualifier {
            self.set_location(m, &ident_src);
            let text = self.name(seg);
            let node = self.backend.ParseIdentifier(Some(&text));
            self.set_location(m, &ident_src);
            list = Some(match list {
                Some(prev) => self.backend.ParseAppendList(prev, node),
                None => self.backend.ParseBaseList(node),
            });
        }
        let name_node = if expanded {
            self.set_location(m, &ident_src);
            let text = self.name(ident_sym);
            self.backend.ParseIdentifier(Some(&text))
        } else {
            self.node(m, ident)?
        };
        self.set_location(m, &m.ir.arena.node(*inner).src);
        let chain = match list {
            Some(prev) => self.backend.ParseAppendList(prev, name_node),
            None => self.backend.ParseBaseList(name_node),
        };
        self.set_location(m, &src);
        let node = self.backend.ParseScopedIdentifier(chain);
        self.set_node_type(m, typed_as.unwrap_or(id), node);
        Ok(node)
    }


    /// `checkArguments` (`TypeCheck.hs:90-97`): every argument is checked
    /// against its parameter's type, and one the parameter does not accept
    /// as a subtype but can convert from is wrapped in a warning
    /// (`warningNarrowingConversions`, `:101`). A `Warning`'s own parse is
    /// its child's (`Warning _ _ a -> return a`, `ParseTree.hs:173`), so
    /// the backend is told the node's type twice — `Do(x + 1)` into a
    /// `uint32` parameter is `uint33` stamped twice
    /// (`higher-order-functions.k::3`, the recorded walk). This port
    /// reports the warning as a diagnostic and has no node for it, so the
    /// second stamp is emitted here. A `[[pipelined]]` callee's first
    /// argument is not checked (`:92-93`) — kept for fidelity though no
    /// corpus call reaches it (removing it moved no program of 2,422,
    /// measured 2026-09-19): that argument is the thread count.
    fn call_arguments(&mut self, m: Module<'_>, function: ExpId, args: ExpId) -> Walked<B::Node> {
        let src = m.ir.arena.node(args).src;
        let Exp::Seq(items) = m.ir.arena.node(args).exp.clone() else { return self.node(m, args) };
        // `typeOf callFunction`: the specifier's own node is synthesized
        // here and typed by no table, so its callee answers.
        let callee = match &m.ir.arena.node(function).exp {
            Exp::FunctionSpecifier { function: f, .. } => *f,
            _ => function,
        };
        let formals = match Self::type_of(m, callee).map(|t| t.un_const()) {
            Some(Ty::Function { attrs, params, .. }) if params.len() == items.len() => {
                let pipelined = attrs.contains(&kg_ty::TAttr::Flag(kg_ty::Flag::Pipelined));
                params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (!(pipelined && i == 0)).then(|| (*p.ty).clone()))
                    .collect::<Vec<_>>()
            }
            _ => return self.node(m, args),
        };
        let mut nodes = Vec::with_capacity(items.len());
        for (&item, formal) in items.iter().zip(&formals) {
            // `checkArg` is `checkConversion warningNarrowingConversions
            // (typeOfParam tParam)` (`TypeCheck.hs:97`), and its `EnumValue`
            // clause turns an enum value into its own value when the parameter
            // is an integer. The walk has the parameter here and nowhere else.
            let mut targets = Conversions::default();
            if let Some(formal) = formal {
                Self::conversions(m, item, formal, true, false, &mut targets);
            }
            let saved = std::mem::replace(&mut self.conversions, targets);
            let node = self.node(m, item);
            self.conversions = saved;
            let node = node?;
            // An argument `evaluateConstexpr` folded is a literal by the time
            // `checkArg` sees it, an enum value converted to an integer one
            // included, and is judged by `inferIntLiteralType Nothing` of its
            // value (`TypeCheck.hs:154-156`): `Do(Foo::One)` over a `uint16`
            // enum is `uint1` into `uint3`, no narrowing
            // (`conversion-warning.k::15`, the recorded walk).
            if let Some(formal) = formal
                && match Self::folded(m, item) {
                    Some(kg_const::ConstValue::Int { value, .. }) => Self::literal_narrows(value, formal),
                    Some(kg_const::ConstValue::Enum { value, .. }) if formal.un_const().is_int() => {
                        Self::literal_narrows(value, formal)
                    }
                    _ => Self::narrows(m, item, formal),
                }
            {
                self.set_node_type(m, item, node);
            }
            nodes.push(node);
        }
        self.set_location(m, &src);
        Ok(self.list(&nodes))
    }

    /// What `checkConversion n t e` (`TypeCheck.hs:152-177`) does inside `e`
    /// that the walk spells: where its `EnumValue` clause meets an integer
    /// `t` (`:155-156`) and turns the value into its literal — `e` itself,
    /// or, through an initializer list that is no subtype of `t`, whose
    /// elements `checkConversion'` checks one by one against the type each
    /// initializes (`:167-175`, `tInitializedType`, `Type.hs:293-304`), an
    /// element at any depth; and which of those elements a narrowing
    /// *warning* wraps (`warningNarrowingConversions`, `:101`), typing it a
    /// second time. A designated element is checked with
    /// `errorNarrowingConversions` (`:157`), which rejects the program
    /// instead. `Foo({E::X, false})` on a `Foo(S)` whose first field is an
    /// `int2` walks the bare `3`, stamped `uint2` twice
    /// (`conversion-warning.k::6`, the recorded walk). The element nodes
    /// carry no type here; each one's `TPositional`/`TDesignator` is the
    /// list's (`Type.hs:1201`). A list the table left untyped is left.
    fn conversions(m: Module<'_>, e: ExpId, t: &Ty, warning: bool, element: bool, out: &mut Conversions) {
        match &m.ir.arena.node(e).exp {
            Exp::InitializerList(seq) => {
                let Some(s) = Self::type_of(m, e) else { return };
                if kg_ty::subtype::is_subtype(&s, t) {
                    return;
                }
                let (Ty::Initializer(elements), Exp::Seq(items)) = (&s, &m.ir.arena.node(*seq).exp) else { return };
                if elements.len() != items.len() {
                    return;
                }
                for (&item, ty) in items.iter().zip(elements.iter()) {
                    let target = kg_ty::subtype::initialized_type(t, ty);
                    match &m.ir.arena.node(item).exp {
                        Exp::Positional { value, .. } => Self::conversions(m, *value, &target, warning, true, out),
                        Exp::Designator { value, .. } => Self::conversions(m, *value, &target, false, true, out),
                        _ => {}
                    }
                }
            }
            _ => {
                let int = t.un_const().is_int();
                if int {
                    out.ints.push(e);
                }
                if element && warning {
                    // `checkConversion`'s literal arm judges a literal by
                    // `inferIntLiteralType Nothing` of its value (`:154`),
                    // an enum value turned into one included.
                    let narrows = match Self::folded(m, e) {
                        Some(kg_const::ConstValue::Int { value, .. }) => Self::literal_narrows(value, t),
                        Some(kg_const::ConstValue::Enum { value, .. }) if int => Self::literal_narrows(value, t),
                        _ => Self::narrows(m, e, t),
                    };
                    if narrows {
                        out.warned.push(e);
                    }
                }
            }
        }
    }

    /// `checkFunctionType` (`TypeCheck.hs:73-85`): a function whose type has
    /// no `[[reset]]` has `checkReturn` applied to each statement *directly*
    /// in its body — `renote (fmap $ renote checkReturn) $ body e`, one
    /// level, so a `return` inside an `if` is not converted — and a
    /// `return x` there is `checkConversion allowNarrowingConversions
    /// (tReturn t) x`: `return Foo::Off;` from a `uint1` function walks the
    /// bare `1` (`enum.k::52`, the recorded walk).
    fn return_conversions(m: Module<'_>, function: ExpId, body: ExpId, out: &mut Vec<(ExpId, Conversions)>) {
        let Some(Ty::Function { attrs, ret, .. }) = Self::type_of(m, function) else { return };
        if attrs.contains(&kg_ty::TAttr::Flag(kg_ty::Flag::Reset)) {
            return;
        }
        let Exp::Seq(items) = &m.ir.arena.node(body).exp else { return };
        for &item in items {
            if let Exp::Return(Some(value)) = m.ir.arena.node(item).exp {
                let mut targets = Conversions::default();
                Self::conversions(m, value, &ret, false, false, &mut targets);
                out.push((item, targets));
            }
        }
    }

    /// `Just (InitializerList (Seq (Designator{}:_)))` (`TypeCheck.hs:130`).
    fn designated_first(m: Module<'_>, init: ExpId) -> bool {
        let Exp::InitializerList(seq) = &m.ir.arena.node(init).exp else { return false };
        let Exp::Seq(items) = &m.ir.arena.node(*seq).exp else { return false };
        items.first().is_some_and(|&i| matches!(m.ir.arena.node(i).exp, Exp::Designator { .. }))
    }

    fn literal_narrows(value: kg_const::ConstInt, t: &Ty) -> bool {
        let lit = kg_ty::width::infer_int_literal_type(&kg_const::ConstValue::Int { ty: None, value });
        !kg_ty::subtype::is_subtype(&lit, t) && kg_ty::subtype::is_convertible_to(&lit, t)
    }

    /// `checkConversion'`'s warning clause (`TypeCheck.hs:176-177`): the
    /// argument's type is no subtype of the parameter's and converts to it.
    /// A literal is judged by `inferIntLiteralType` of its value (`:154`),
    /// which is what the table holds for it.
    fn narrows(m: Module<'_>, arg: ExpId, formal: &Ty) -> bool {
        let Some(actual) = Self::type_of(m, arg) else { return false };
        let actual = match &actual {
            Ty::Type(_) | Ty::Unresolved | Ty::Undefined | Ty::Template => return false,
            other => other,
        };
        !kg_ty::subtype::is_subtype(actual, formal) && kg_ty::subtype::is_convertible_to(actual, formal)
    }

    /// A free call's callee, a plain name, that only a function template
    /// declares — see [`template_functions`] — given something callable:
    /// a function as it is, unwrapped, the argument `addLambdaWrappers`
    /// would have made a closure of; a template's name; or a closure, to a
    /// higher-order member template called without its receiver
    /// (`higher-order-functions.k::42`) — each a deduction this route left
    /// without a verdict. An argument the table left unresolved counts too:
    /// it is no evidence of a value, and a member read through a hoisted
    /// lambda's `$this` is typed nothing here (`Go10($this.callback)`,
    /// `higher-order-functions.k::193`). A template the backend knows by
    /// its own name (`println`) is given no such argument.
    fn calls_an_uninstantiated_template(&self, m: Module<'_>, function: ExpId, args: ExpId) -> bool {
        fn callable(t: &Ty) -> bool {
            match t.un_const() {
                Ty::Function { .. } | Ty::Closure { .. } | Ty::Template | Ty::Unresolved => true,
                Ty::Instance { concrete, .. } => callable(&concrete),
                _ => false,
            }
        }
        let Exp::Seq(items) = &m.ir.arena.node(args).exp else { return false };
        if !items.iter().any(|&a| Self::type_of(m, a).is_some_and(|t| callable(&t))) {
            return false;
        }
        let Exp::FunctionSpecifier { receiver: None, function: callee } = &m.ir.arena.node(function).exp else {
            return false;
        };
        let Exp::QualifiedIdentifier(inner) = &m.ir.arena.node(*callee).exp else { return false };
        let Some((scope, written, ident)) = Self::written_path(m, *inner) else { return false };
        let Exp::Identifier(sym) = &m.ir.arena.node(ident).exp else { return false };
        let leaf = *sym;
        if !self.template_functions.contains(&leaf) {
            return false;
        }
        let scope = scope.clone();
        let name: Vec<kg_base::Symbol> = written.iter().copied().chain([leaf]).collect();
        self.lookup_post(&scope, &name).is_none()
    }

    /// The class a qualifier names through a type alias, by its own path:
    /// `bar_t::foo_t` under `using bar_t = Foo<uint32>` is
    /// `Foo<|$uint@32|>::foo_t` to the backend, and `Bbool::A<uint32>::A3_t`
    /// under `using Bbool = B<bool>` is `@helper@…::B<|$bool|>::A<…>::A3_t`
    /// (`using.k::39`, `template-instance-identifiers.k::35`, the recorded
    /// walks, 2026-09-23). The parser keeps the alias as the written
    /// qualifier and resolves through it only for lookups (`Parser.hs:
    /// 667-668`); the reference's tree has the alias's body there by the
    /// first instantiation round (`kg-oracle --tree --passes 8`, iteration 0
    /// against 1), and `removeNestedTypeIdentifier` (`Desugar.hs:122-127`)
    /// is the shape it leaves. Handed the alias, the backend cannot find a
    /// scope by that name ("Error 24", "Error 62").
    fn aliased_class_path(&self, m: Module<'_>, prefix: ExpId) -> Option<Vec<String>> {
        let (scope, mut written, ident) = Self::written_path(m, prefix)?;
        let Exp::Identifier(leaf) = &m.ir.arena.node(ident).exp else { return None };
        written.push(*leaf);
        let name: Vec<kg_base::Symbol> = written.into_iter().collect();
        let scope = kg_base::SymbolPath::new(scope.segments().iter().copied());
        let (dm, decl) = self.lookup_post(&scope, &name)?;
        if !matches!(dm.ir.arena.node(decl).exp, Exp::Alias { .. }) {
            return None;
        }
        let mut ty = (Self::type_of(dm, decl)?).clone();
        loop {
            ty = match ty {
                Ty::Type(t) => (*t).clone(),
                Ty::Instance { concrete, .. } => (*concrete).clone(),
                Ty::Class(q, _) | Ty::Struct(q, _) | Ty::Union(q, _) => {
                    return Some(q.segments().iter().map(|s| self.interner.resolve(*s).to_string()).collect());
                }
                _ => return None,
            };
        }
    }

    /// Whether `member` of `base` names a method of `base`'s class — a
    /// function its walked class declares under that name, or a member
    /// template of it.
    fn names_a_method(&self, m: Module<'_>, base: ExpId, member: ExpId) -> bool {
        let Exp::Identifier(sym) = &m.ir.arena.node(member).exp else { return false };
        let leaf = *sym;
        let Some(ty) = Self::type_of(m, base) else { return false };
        let mut ty = (ty).clone();
        let class = loop {
            ty = match ty {
                Ty::Const(t) | Ty::Type(t) => (*t).clone(),
                Ty::Instance { concrete, .. } => (*concrete).clone(),
                Ty::Class(q, _) | Ty::Reference(q) => break q,
                _ => return false,
            };
        };
        let mut key: Vec<kg_base::Symbol> = class.segments().to_vec();
        key.push(leaf);
        match self.post.get(&key) {
            Some(&(index, id)) => {
                let dm = self.modules[index].view();
                matches!(dm.ir.arena.node(id).exp, Exp::Function { .. })
            }
            None => self.template_functions.contains(&leaf) && {
                key.pop();
                self.post.contains_key(&key)
            },
        }
    }

    /// A class's member variable named through the class — `main@Foo::x`.
    /// The reference never hands the backend one: inside a method
    /// `resolveNames` drops the qualifier (`Frontend.hs:277`), and in a
    /// function hoisted out of the class `moveFunctions`' `memberAccess`
    /// makes it `$this.x` (`Function.hs:52-57`). One left here is a rewrite
    /// that missed — the lambda of a local class's template method copy
    /// (`local-classes.k::15,16`, 2026-09-23) — and the backend's "Unknown
    /// symbol" would be an error on a program the reference compiles.
    fn names_a_member_through_its_class(&self, m: Module<'_>, value: ExpId) -> bool {
        let Exp::QualifiedIdentifier(inner) = &m.ir.arena.node(value).exp else { return false };
        let Some((scope, written, ident)) = Self::written_path(m, *inner) else { return false };
        if written.is_empty() {
            return false;
        }
        let Exp::Identifier(leaf) = &m.ir.arena.node(ident).exp else { return false };
        let name: Vec<kg_base::Symbol> = written.iter().copied().chain([*leaf]).collect();
        let scope = kg_base::SymbolPath::new(scope.segments().iter().copied());
        // `getQualifier a `isPrefixOf` getScope a` (`Frontend.hs:277`): a
        // use inside the class is unqualified by `resolveNames`, as the
        // walk's `qualified_identifier` does; only a use whose scope is
        // outside the class — a hoisted function's — is a missed rewrite.
        if scope.segments().starts_with(&name[..name.len() - 1]) {
            return false;
        }
        let Some((dm, decl)) = self.lookup_post(&scope, &name) else { return false };
        matches!(dm.ir.arena.node(decl).exp, Exp::Variable { flags: kg_resolve::DeclareFlag::DeclareClass, .. })
    }

    /// A method call whose method the receiver's class declares only as a
    /// template — no instance of it in the walked class. `higherOrderFunctions`
    /// makes a method with a function-typed parameter a template
    /// (`Function.hs:180-184`), and the reference calls the instance its
    /// deduction made (`foo.Go<Capture, main@$wrapper$Do>(…)`,
    /// `closures.k::25`, 2026-09-23); a call to the bare name is one the
    /// backend answers "Undefined function" on a program the reference
    /// compiles. `post` holds every non-template member under its class's
    /// path (`removeTemplate` takes the templates out), so a name in
    /// [`template_functions`] that the class does not declare there is
    /// such a call.
    fn calls_a_member_template(&self, m: Module<'_>, function: ExpId) -> bool {
        let Exp::FunctionSpecifier { receiver: Some(receiver), function: callee } = &m.ir.arena.node(function).exp else {
            return false;
        };
        let Exp::QualifiedIdentifier(inner) = &m.ir.arena.node(*callee).exp else { return false };
        let Some((_, _, ident)) = Self::written_path(m, *inner) else { return false };
        let Exp::Identifier(sym) = &m.ir.arena.node(ident).exp else { return false };
        let leaf = *sym;
        if !self.template_functions.contains(&leaf) {
            return false;
        }
        let Some(ty) = Self::type_of(m, *receiver) else { return false };
        let Ty::Class(class, _) = (ty.un_const()).clone() else { return false };
        let mut key: Vec<kg_base::Symbol> = class.segments().to_vec();
        if !self.post.contains_key(&key) {
            return false;
        }
        key.push(leaf);
        !self.post.contains_key(&key)
    }

    /// A call of a *mangled* function name — a copy's, a hoisted lambda's
    /// (`@`, `mangleName`'s join) — that no walked tree declares where the
    /// call can reach it. `closureCalls` names the closure's function by
    /// the path its type carries; one that lost its module namespace on the
    /// way (`BypassTest@Test@?lambda0`, called from `control.async`'s copy
    /// in `test/logic/device/quad_port_tests.k`, 2026-09-23) is a name the
    /// backend calls "Undefined function" on a program the reference
    /// compiles. Declined instead. Its witnesses are corpus programs —
    /// `test/logic/device/quad_port_tests.k`, `test/library/data/bits.k`,
    /// `test/library/data/representation.k` (2026-09-23) — for which no
    /// hermetic reduction has yet been found; the shapes it was written
    /// against now compile.
    fn calls_an_undeclared_function(&self, m: Module<'_>, function: ExpId) -> bool {
        let Exp::FunctionSpecifier { receiver: None, function: callee } = &m.ir.arena.node(function).exp else {
            return false;
        };
        let callee = match &m.ir.arena.node(*callee).exp {
            Exp::NamedValue(e) => *e,
            _ => *callee,
        };
        let Exp::QualifiedIdentifier(inner) = &m.ir.arena.node(callee).exp else { return false };
        let named = match &m.ir.arena.node(*inner).exp {
            Exp::TemplateInstance { base, args }
                if matches!(&m.ir.arena.node(*args).exp, Exp::Seq(items) if items.is_empty()) =>
            {
                *base
            }
            _ => *inner,
        };
        let Some((scope, written, ident)) = Self::written_path(m, named) else { return false };
        let Exp::Identifier(sym) = &m.ir.arena.node(ident).exp else { return false };
        let leaf = *sym;
        // A hoisted lambda's function only — `?lambda`, or the `$wrapper$`
        // `addLambdaWrappers` makes of a named function: `post` is not a
        // complete index of every copy a call may name, and asked about a
        // function template's copy it declined 17 programs the backend
        // compiles.
        let text = self.interner.resolve(leaf);
        if !text.contains("?lambda") && !text.contains("$wrapper$") {
            return false;
        }
        let scope = scope.clone();
        let name: Vec<kg_base::Symbol> = written.iter().copied().chain([leaf]).collect();
        self.lookup_post(&scope, &name).is_none()
    }

    /// A call whose callee is a parameter or a local of the enclosing
    /// function. The reference hands the backend none: `closureCalls`
    /// rewrote every call of a closure into a call of its function with the
    /// capture first (`Closure.hs:19-26`), and a function-typed parameter
    /// is a template's by then (`higherOrderFunctions`). One left here is a
    /// call no rewrite reached — `task()` in `control.async`'s
    /// `async_then_void` copy's local class (`test/logic/atomic_do.k`,
    /// 2026-09-23) — and the backend's "Undefined function" is an error on a
    /// program the reference compiles.
    fn calls_a_variable(&self, m: Module<'_>, function: ExpId) -> bool {
        let Exp::FunctionSpecifier { receiver: None, function: callee } = &m.ir.arena.node(function).exp else {
            return false;
        };
        // A call through a variable wears the lowering's value wrapper.
        let callee = match &m.ir.arena.node(*callee).exp {
            Exp::NamedValue(e) => *e,
            _ => *callee,
        };
        let Exp::QualifiedIdentifier(inner) = &m.ir.arena.node(callee).exp else { return false };
        // And every callee the lowering wraps as a template's, no arguments
        // written (`Parser.hs:694`).
        let named = match &m.ir.arena.node(*inner).exp {
            Exp::TemplateInstance { base, args }
                if matches!(&m.ir.arena.node(*args).exp, Exp::Seq(items) if items.is_empty()) =>
            {
                *base
            }
            _ => *inner,
        };
        // Resolution qualified a local by its scope (`resolveNames`,
        // `Frontend.hs:273-291`); the leaf is the name.
        let Exp::ScopedName { ident, .. } = &m.ir.arena.node(named).exp else { return false };
        let Exp::Identifier(sym) = &m.ir.arena.node(*ident).exp else { return false };
        self.frames.last().is_some_and(|frame| frame.iter().any(|(s, _)| s == sym))
    }

    /// The type of the literal a variable's initializer folds to —
    /// `inferIntLiteralType` of its width (`Type.hs`), `bool` for a boolean.
    fn folded_initializer_type(m: Module<'_>, decl: ExpId) -> Option<Ty> {
        let Exp::Variable { init: Some(init), .. } = &m.ir.arena.node(decl).exp else { return None };
        match Self::folded(m, *init)? {
            kg_const::ConstValue::Bool(_) => Some(Ty::Boolean),
            v @ kg_const::ConstValue::Int { .. } => Some(kg_ty::width::infer_int_literal_type(&v)),
            _ => None,
        }
    }

    /// `evaluateConstexpr` at this node (`Constexpr.hs:117-283`): the
    /// literal it left in the loop's tree, if it folded — a copy's
    /// substituted parameters included. Until 2026-10-10 the CST folder
    /// answered where the loop's tree held no literal; two of its five
    /// answers were the loop's own miss (`x & 0`, `Constexpr.hs:231-232`),
    /// three an enum constant read below, and without it `ptdiff` and
    /// `backend` are identical (W3.6).
    fn folded(m: Module<'_>, id: ExpId) -> Option<kg_const::ConstValue> {
        let arena = &m.sem.ir.arena;
        if let Some(v) = kg_hir::constexpr::value(arena, id, &kg_hir::constexpr::Closed) {
            return Some(v);
        }
        // `EnumValue a x` (`Constexpr.hs:292`): an enum constant folded to
        // its value, nested where it names another (`G = F`) — what the
        // reference's `checkConversion` judges as a literal (`uint3 x =
        // Foo::One;`, `conversion-warning.k::14`).
        let Exp::EnumValue { ty, value } = &arena[id].exp else { return None };
        let mut inner = *value;
        while let Exp::EnumValue { value, .. } = &arena[inner].exp {
            inner = *value;
        }
        let Exp::IntLiteral { value, .. } = &arena[inner].exp else { return None };
        let name = kg_base::SymbolPath::new(kg_hir::sort::spine_qualname(arena, *ty));
        Some(kg_const::ConstValue::Enum { name, value: value.clone() })
    }

    /// `tIsClosure s` with `tWidth (tClosureCapture s) == Just 0`
    /// (`TypeCheck.hs:129,147`) for the node's type, `const` stripped.
    fn captures_nothing(m: Module<'_>, id: ExpId) -> bool {
        let Some(ty) = Self::type_of(m, id) else { return false };
        let mut t = &ty;
        loop {
            match t {
                Ty::Type(inner) | Ty::Const(inner) => t = inner,
                Ty::Instance { concrete, .. } => t = concrete,
                Ty::Closure { capture, .. } => return kg_ty::width::t_width(capture) == Some(0),
                _ => return false,
            }
        }
    }

    /// [`Self::constant`], and positive: `checkArraySize` rejects anything
    /// else before the walk (`TypeCheck.hs:187-196`) — `bool[I - 3]` in
    /// `Do<3>`'s copy (`template.k::30`), which this frontend does not yet
    /// report inside a copy, and the backend compiles as `bool[0]`.
    fn positive(m: Module<'_>, id: ExpId) -> bool {
        Self::natural(m, id).is_some()
    }

    /// `fromNaturalConstexpr` (`Type.hs:1102-1103`): a positive integer, an
    /// enum constant's included.
    fn natural(m: Module<'_>, id: ExpId) -> Option<kg_const::ConstInt> {
        let value = match &m.ir.arena.node(id).exp {
            Exp::IntLiteral { value, .. } => Some(value.clone()),
            // `fromIntegralConstexpr (EnumValue _ x)` (`Type.hs:1099`): an
            // enum constant a copy's `auto` parameter was bound to, standing
            // folded in the copy (`constexpr.k::121`'s `bool[N]` under
            // `Foo<E5::Count>`).
            Exp::EnumValue { value, .. } => match &m.ir.arena.node(*value).exp {
                Exp::IntLiteral { value, .. } => Some(value.clone()),
                _ => None,
            },
            // An enum constant is a dimension too (`bool[Bar::B4]`,
            // `enum.k::39`): `as_int` reads `Int` and `Enum` alike, as
            // `fromIntegralConstexpr` does (`Type.hs:1099`).
            _ => Self::dimension_value(m, id).and_then(|v| v.as_int().cloned()),
        };
        value.filter(kg_const::ConstInt::is_positive)
    }

    /// `isEnum` (`Constexpr.hs:372-375`) of a dimension as the fold leaves
    /// it: an enum constant, written or folded.
    fn enum_dimension(m: Module<'_>, id: ExpId) -> bool {
        matches!(m.ir.arena.node(id).exp, Exp::EnumValue { .. })
            || matches!(Self::dimension_value(m, id), Some(kg_const::ConstValue::Enum { .. }))
    }

    /// The value the reference's tree holds at a dimension or a width: the
    /// fold's, or — where the fold has none — literal arithmetic over the
    /// IR itself. A copy's substituted `T[N+1]` is `uint4[(10 + 1)]`, which
    /// `evaluateConstexpr` has made `11` before the walk
    /// (`template-argument-deduction.k::53`, the recorded walk's
    /// `ParseDecimalLiteral "11"`), and the twin's fold keys its values by
    /// ranges a copy's printed parenthesis does not line up with. Only
    /// here: elsewhere `2 + 1` is walked as the operation the reference
    /// walks (`staticControl`'s module comment).
    fn dimension_value(m: Module<'_>, id: ExpId) -> Option<kg_const::ConstValue> {
        Self::folded(m, id).or_else(|| literal_arithmetic(m, id))
    }

    /// A `const` variable of a function type (`tIsConst t`, `tIsFunction`,
    /// `TypeCheck.hs:139`), read off its declared type.
    fn a_function_constant(m: Module<'_>, var: ExpId) -> bool {
        let ty = match Self::type_of(m, var) {
            Some(Ty::Type(t)) => (*t).clone(),
            Some(other) => other,
            None => return false,
        };
        matches!(&ty, Ty::Const(inner) if inner.is_function())
    }

    /// Whether the reference's tree holds a literal here by the time it is
    /// walked — written as one, or folded to one. An array dimension and an
    /// integer attribute always do: the backend reads them as numbers, and a
    /// name the fold missed there — `max_threads(C_t::x)` over an import
    /// (`constexpr.k::126`), a dimension the frontend should have rejected
    /// (`constexpr.k::166`) — is a crash, not a verdict.
    fn constant(m: Module<'_>, id: ExpId) -> bool {
        matches!(m.ir.arena.node(id).exp, Exp::IntLiteral { .. }) || Self::folded(m, id).is_some()
    }

    /// The literal a folded node becomes, in place: the node's own
    /// location and its own type (`NotedExp n $ eval e` keeps the note).
    fn literal(&mut self, m: Module<'_>, id: ExpId, value: &kg_const::ConstValue) -> Walked<Option<B::Node>> {
        let data = m.ir.arena.node(id);
        let src = data.src;
        let node = match value {
            kg_const::ConstValue::Int { value: int, .. } => {
                self.set_location(m, &src);
                let text = int.to_string();
                let lit = self.backend.ParseDecimalLiteral(Some(&text));
                // The fold's literal is typed by the next inference round
                // (`inferIntLiteralType`, `Type.hs:485-491`) when the note it
                // keeps had no type: `byteoffsetof(S4<S3>, y)` as an array
                // dimension is `2 : uint2` in the reference, and an untyped
                // literal there aborts the backend (`constexpr.k::106`).
                if Self::type_of(m, id).is_some_and(|t| t.resolved()) {
                    self.set_node_type(m, id, lit);
                } else if let Some((signed, width)) = integer_type(&kg_ty::width::infer_int_literal_type(value)) {
                    let ty = self.integer_node(signed, width);
                    self.backend.SetNodeType(lit, ty);
                }
                return Ok(Some(lit));
            }
            kg_const::ConstValue::Bool(b) => {
                self.set_location(m, &src);
                self.backend.ParseBoolLiteral(u32::from(*b))
            }
            kg_const::ConstValue::Float(f) => {
                self.set_location(m, &src);
                self.backend.ParseFloatLiteral(f.to_f32())
            }
            kg_const::ConstValue::Str(s) => {
                self.set_location(m, &src);
                self.backend.ParseStringLiteral(Some(s))
            }
            kg_const::ConstValue::Enum { value, .. } => {
                let Exp::NamedValue(inner) = &data.exp else { return Ok(None) };
                // `checkConversion n t (EnumValue _ x) | tIsInt t =
                // checkConversion n t x` (`TypeCheck.hs:155-156`): converted to
                // an integer, an enum value **is** its value — `Foo(E::X)` on a
                // `Foo(uint2)` walks the bare literal where `T x = T::One` on an
                // enum-typed `T` keeps the `EnumValue`
                // (`conversion-warning.k::4` against `type-identifier.k::9`).
                if self.conversions.ints.contains(&id) {
                    self.set_location(m, &src);
                    let text = value.to_string();
                    self.backend.ParseDecimalLiteral(Some(&text))
                } else {
                // A constant holding an enum value is replaced by its
                // initializer's own subtree (`unNote $ unfix v`,
                // `Constexpr.hs:256`), children keeping the notes of the
                // module that declared it, the top node taking this one's.
                if let Some((dm, _, init)) = self.const_init(m, *inner) {
                    return self.node_with_src(dm, init, src).map(Some);
                }
                // `EnumValueF x v` (`Constexpr.hs:258-259`): the name as
                // written, then the constant's literal, both wearing the
                // node's note.
                let x = match &m.ir.arena.node(*inner).exp {
                    Exp::QualifiedIdentifier(_) => self.qualified_identifier(m, *inner, false, Some(id))?,
                    _ => self.node(m, *inner)?,
                };
                self.set_location(m, &src);
                let text = value.to_string();
                let lit = self.backend.ParseDecimalLiteral(Some(&text));
                self.set_node_type(m, id, lit);
                self.set_location(m, &src);
                self.backend.ParseEnumValue(x, lit)
                }
            }
            kg_const::ConstValue::Aggregate(_) => return Ok(None),
        };
        self.set_node_type(m, id, node);
        Ok(Some(node))
    }

    /// One node, `cataM parseWithLoc`.
    #[allow(clippy::too_many_lines)]
    pub fn node(&mut self, m: Module<'_>, id: ExpId) -> Walked<B::Node> {
        let data = m.ir.arena.node(id);
        let src = self.pending_src.take().unwrap_or(data.src);
        let null = self.backend.null();
        // The folder first: what the reference reduced to a literal before
        // its walk never reaches the backend as an expression.
        if matches!(
            data.exp,
            Exp::Binary(..)
                | Exp::Unary(..)
                | Exp::NamedValue(_)
                | Exp::Cast { .. }
                | Exp::Mux { .. }
                | Exp::Static(_)
                | Exp::BitOffset { .. }
                | Exp::ByteOffset { .. }
        ) && let Some(value) = Self::folded(m, id)
            && let Some(node) = self.literal(m, id, &value)?
        {
            return Ok(node);
        }
        macro_rules! w {
            ($e:expr) => {
                self.node(m, $e)?
            };
        }
        macro_rules! opt {
            ($e:expr) => {
                match $e {
                    Some(e) => Some(self.node(m, e)?),
                    None => None,
                }
            };
        }
        let result: B::Node = match &data.exp {
            Exp::Seq(items) => {
                let mut nodes = Vec::with_capacity(items.len());
                self.seq_items(m, items, &mut nodes)?;
                self.set_location(m, &src);
                self.list(&nodes)
            }
            Exp::FlagAttr(flag) => {
                self.set_location(m, &src);
                self.flag_attribute(*flag)
            }
            Exp::IntAttr(attr, e) => {
                if !Self::constant(m, *e) {
                    return Err(Self::decline("fold:attribute", &src));
                }
                let e = w!(*e);
                self.set_location(m, &src);
                let code = Self::int_attribute_code(*attr)
                    .ok_or_else(|| Self::decline(format!("int attribute {attr:?}"), &src))?;
                self.backend.ParseIntAttribute(code, e)
            }
            Exp::Attr(attr, e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                let code = match attr {
                    Attribute::Ecc => 3,
                    Attribute::Rename => 8,
                    other => return Err(Self::decline(format!("attribute {other:?}"), &src)),
                };
                self.backend.ParseAttribute(code, e)
            }
            Exp::FunctionModifier(modifier) => {
                self.set_location(m, &src);
                self.backend.ParseFunctionModifier(match modifier {
                    kg_hir::Modifier::Inline => 1,
                    kg_hir::Modifier::NoInline => 1024,
                })
            }
            Exp::Auto => return Err(Self::decline("auto", &src)),
            Exp::Void => {
                self.set_location(m, &src);
                self.backend.ParseVoidType()
            }
            Exp::Boolean => {
                self.set_location(m, &src);
                self.backend.ParseBoolType()
            }
            Exp::Float => {
                self.set_location(m, &src);
                self.backend.ParseFloatType()
            }
            Exp::Str => {
                self.set_location(m, &src);
                self.backend.ParseStringType()
            }
            Exp::Integer(n) => {
                self.set_location(m, &src);
                self.backend.ParseIntType(*n as i32)
            }
            Exp::Unsigned(n) => {
                self.set_location(m, &src);
                self.backend.ParseUintType(*n as i32)
            }
            // `int<N>`/`uint<N>` with `N` folded (`Constexpr.hs:298-303`).
            Exp::ParamInt(e) | Exp::ParamUint(e) => {
                let Some(kg_const::ConstValue::Int { value, .. }) = Self::dimension_value(m, *e).or_else(|| {
                    match &m.ir.arena.node(*e).exp {
                        Exp::IntLiteral { value, ty } => Some(kg_const::ConstValue::Int { ty: *ty, value: value.clone() }),
                        _ => None,
                    }
                }) else {
                    return Err(Self::decline("fold:int<N>", &src));
                };
                let Some(n) = value.to_u32() else { return Err(Self::decline("fold:int<N>", &src)) };
                self.set_location(m, &src);
                if matches!(data.exp, Exp::ParamInt(_)) {
                    self.backend.ParseIntType(n as i32)
                } else {
                    self.backend.ParseUintType(n as i32)
                }
            }
            Exp::Const(e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                self.backend.ParseConst(e)
            }
            Exp::Reference(e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                self.backend.ParseReference(e)
            }
            Exp::Array { attr, elem, dims } => {
                if let Exp::Seq(ds) = &m.ir.arena.node(*dims).exp
                    && !ds.iter().all(|d| Self::positive(m, *d))
                {
                    return Err(Self::decline("fold:array dimension", &src));
                }
                let a = w!(*attr);
                let b = w!(*elem);
                let c = match &m.ir.arena.node(*dims).exp {
                    Exp::Seq(ds) => {
                        // `eval (ArrayF x y z) | any isEnum $ unfix z = ArrayF x
                        // y $ renote (fmap resolveDim) z`, `resolveDim` making
                        // each natural dimension an untyped literal at its own
                        // note (`Constexpr.hs:154-157`), which inference then
                        // types as a bare literal: `bool[Bar::B4]` is `4 :
                        // uint3` (`enum.k::39`, the recorded walk).
                        let enums = ds.iter().any(|&d| Self::enum_dimension(m, d));
                        let mut nodes = Vec::with_capacity(ds.len());
                        for &d in ds {
                            if enums && let Some(n) = Self::natural(m, d) {
                                let donor = m.ir.arena.node(d).src;
                                nodes.push(self.minted_literal(m, &n, &donor));
                                continue;
                            }
                            let folded = match &m.ir.arena.node(d).exp {
                                Exp::IntLiteral { .. } => None,
                                _ => Self::dimension_value(m, d),
                            };
                            nodes.push(match folded {
                                // A dimension over an unrolled iteration's
                                // induction variable was unresolved when
                                // `staticControl` copied it — the loop is
                                // unrolled at its own node, before the
                                // `const auto` it declares is typed — so the
                                // fold's untyped literal (`Constexpr.hs:76`)
                                // is typed as a bare literal: `uint32[i+1]` in
                                // `$i0` is `1 : uint1`, not the `uint2` of
                                // `i + 1` (`static-for.k::0`, the recorded walk).
                                Some(kg_const::ConstValue::Int { value: v, .. }) if self.mentions_induction_var(m, d) => {
                                    let donor = m.ir.arena.node(d).src;
                                    self.minted_literal(m, &v, &donor)
                                }
                                Some(value) => match self.literal(m, d, &value)? {
                                    Some(node) => node,
                                    None => w!(d),
                                },
                                None => w!(d),
                            });
                        }
                        self.set_location(m, &m.ir.arena.node(*dims).src);
                        self.list(&nodes)
                    }
                    _ => w!(*dims),
                };
                self.set_location(m, &src);
                self.backend.ParseArrayType(a, b, c)
            }
            Exp::TemplateInstance { .. } => return Err(Self::decline("template instance", &src)),
            Exp::TemplateParam { .. } => return Err(Self::decline("template parameter", &src)),
            Exp::TypeParam { .. } => return Err(Self::decline("type parameter", &src)),
            Exp::Typename => return Err(Self::decline("typename", &src)),
            Exp::FunctionTypeParam { attr, ty, name } => {
                let a = w!(*attr);
                let b = w!(*ty);
                let c = opt!(*name);
                self.set_location(m, &src);
                self.backend.ParseFunctionTypeParam(a, b, c.unwrap_or(null))
            }
            Exp::FunctionType { attr, params, ret } => {
                let a = w!(*attr);
                let b = w!(*params);
                let c = w!(*ret);
                self.set_location(m, &src);
                self.backend.ParseFunctionType(a, b, c)
            }
            Exp::This { scope } => {
                // `thisReference` (`Frontend.hs:453-456`, pass 16): a `this`
                // whose `$this` resolves to a variable — the local
                // `desugarLambdas` declares in a hoisted lambda, which a
                // wrapper's capture reads (`this-capture.k::9`) — is
                // `NamedValueF $ localIdentifier n a`: the variable's name,
                // at its note, under the `this`'s own.
                let this = self.interner.intern(kg_resolve::names::THIS);
                if let Some((dm, decl)) = self.lookup_post(scope, &[this])
                    && let Exp::Variable { name, .. } = &dm.ir.arena.node(decl).exp
                {
                    let name_src = dm.ir.arena.node(*name).src;
                    self.set_location(dm, &name_src);
                    let ident = self.backend.ParseIdentifier(Some(kg_resolve::names::THIS));
                    self.set_location(dm, &name_src);
                    let list = self.backend.ParseBaseList(ident);
                    self.set_location(dm, &name_src);
                    let scoped = self.backend.ParseScopedIdentifier(list);
                    self.set_node_type(m, id, scoped);
                    self.set_location(m, &src);
                    return Ok(self.backend.ParseNamedVariable(scoped));
                }
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                self.backend.ParseThis(&refs)
            }
            Exp::Designator { name, value } => {
                let a = w!(*name);
                let b = w!(*value);
                self.set_location(m, &src);
                self.backend.ParseDesignator(a, b)
            }
            Exp::Positional { value, .. } => {
                let a = w!(*value);
                // The narrowing warning's second stamp, at the element's own
                // note (`Walker::conversions`).
                if self.conversions.warned.contains(value) {
                    let at = m.ir.arena.node(*value).src;
                    self.set_location(m, &at);
                    self.set_node_type(m, *value, a);
                }
                self.set_location(m, &src);
                a
            }
            Exp::InitializerList(e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                self.backend.ParseInitializerList(e)
            }
            Exp::BitOffset { .. } => return Err(Self::decline("fold:bitoffsetof", &src)),
            Exp::ByteOffset { .. } => return Err(Self::decline("fold:byteoffsetof", &src)),
            Exp::IntLiteral { value, .. } => {
                self.set_location(m, &src);
                let text = value.to_string();
                self.backend.ParseDecimalLiteral(Some(&text))
            }
            Exp::FloatLiteral(bits) => {
                self.set_location(m, &src);
                self.backend.ParseFloatLiteral(bits.to_f32())
            }
            Exp::BoolLiteral(b) => {
                self.set_location(m, &src);
                self.backend.ParseBoolLiteral(u32::from(*b))
            }
            Exp::StringLiteral(s) => {
                self.set_location(m, &src);
                let text = String::from_utf8_lossy(&plain_string(s)).into_owned();
                self.backend.ParseStringLiteral(Some(&text))
            }
            // `interpolatedStringPre` (`Frontend.hs:233-234`): a string with
            // no hole is a `StringLiteral`.
            Exp::InterpolatedString(raw) => {
                self.set_location(m, &src);
                let text = String::from_utf8_lossy(&decode_piece(raw, true, true)).into_owned();
                self.backend.ParseStringLiteral(Some(&text))
            }
            Exp::Interpolated(segments) => return self.interpolated_string(m, segments, &src),
            Exp::Identifier(name) => {
                self.set_location(m, &src);
                let text = self.name(*name);
                self.backend.ParseIdentifier(Some(&text))
            }
            Exp::ScopedName { prefix, ident, .. } => {
                let p = match prefix {
                    Some(p) => match self.aliased_class_path(m, *p) {
                        Some(path) => {
                            let at = m.ir.arena.node(*p).src;
                            let mut list = None;
                            for segment in path {
                                self.set_location(m, &at);
                                let i = self.backend.ParseIdentifier(Some(&segment));
                                self.set_location(m, &at);
                                list = Some(match list {
                                    Some(l) => self.backend.ParseAppendList(l, i),
                                    None => self.backend.ParseBaseList(i),
                                });
                            }
                            list
                        }
                        None => Some(self.node(m, *p)?),
                    },
                    None => None,
                };
                let i = w!(*ident);
                self.set_location(m, &src);
                match p {
                    Some(p) => self.backend.ParseAppendList(p, i),
                    None => self.backend.ParseBaseList(i),
                }
            }
            Exp::QualifiedIdentifier(_) => return self.qualified_identifier(m, id, false, None),
            Exp::TypeIdentifier(scope, e) => {
                if let Exp::ScopedName { ident, .. } = &m.ir.arena.node(*e).exp
                    && let Exp::Identifier(sym) = &m.ir.arena.node(*ident).exp
                {
                    let original = *sym;
                    if kg_base::mentions_instance(&self.interner.resolve(original)) && !self.declared_copies.contains(&original) {
                        return Err(Self::decline("undeclared copy", &src));
                    }
                }
                let e = w!(*e);
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                self.backend.ParseNamedType(e, &refs)
            }
            Exp::FunctionSpecifier { receiver, function } => {
                // `resolveNames` (`Frontend.hs:284-285`): a method's name is
                // bare after its receiver; a call through a function-typed
                // variable loses its `NamedValue` wrapper.
                let r = opt!(*receiver);
                let f = match (receiver, &m.ir.arena.node(*function).exp) {
                    (Some(_), Exp::QualifiedIdentifier(_)) => self.qualified_identifier(m, *function, true, None)?,
                    (None, Exp::NamedValue(inner)) => {
                        // A constant holding a function is its initializer
                        // (`Constexpr.hs:290`), the fixpoint following a
                        // chain of them to the function itself: `fn(true)`
                        // over `const auto fn = fn1; const auto fn1 = Foo;`
                        // calls `Foo` (`function-constants.k::2`).
                        let (mut dm, mut inner) = (m, *inner);
                        while let Some((cm, _, init)) = self.const_init(dm, inner)
                            && Self::type_of(cm, init).is_some_and(|t| t.is_function())
                            && let Exp::NamedValue(next) = &cm.ir.arena.node(init).exp
                        {
                            (dm, inner) = (cm, *next);
                        }
                        let n = self.node(dm, inner)?;
                        self.set_location(m, &m.ir.arena.node(*function).src);
                        n
                    }
                    _ => w!(*function),
                };
                self.set_location(m, &src);
                self.backend.ParseFunctionSpecifier(r.unwrap_or(null), f)
            }
            Exp::EnumValue { ty, value } => {
                // `checkConversion n t (EnumValue _ x) | tIsInt t =
                // checkConversion n t x` (`TypeCheck.hs:155-156`), for the
                // enum value `evaluateConstexpr` already wrote into the tree
                // (the IR inference's tree, `KG_LOOP`): converted to an
                // integer, it is its literal — as the `NamedValue` arm below
                // walks a constant that folds to one.
                // The clause recurses (`checkConversion n t x`), so an enum
                // value of an enum value is its innermost literal.
                let mut inner = *value;
                while let Exp::EnumValue { value, .. } = &m.ir.arena.node(inner).exp {
                    inner = *value;
                }
                if self.conversions.ints.contains(&id)
                    && let Exp::IntLiteral { value: v, .. } = &m.ir.arena.node(inner).exp
                {
                    self.set_location(m, &src);
                    let text = v.to_string();
                    self.backend.ParseDecimalLiteral(Some(&text))
                } else {
                    let a = w!(*ty);
                    let b = w!(*value);
                    self.set_location(m, &src);
                    self.backend.ParseEnumValue(a, b)
                }
            }
            Exp::NamedValue(e) => {
                if self.names_a_member_through_its_class(m, *e) {
                    return Err(Self::decline("member named through its class", &src));
                }
                // A constant holding a function (`tIsFunction $ typeOf v`,
                // `Constexpr.hs:290`) is replaced by its initializer — a
                // *constant*: the lookup's pattern is `Variable _ _ (Const
                // typ) _ (Just v) _` (`:287`), so a plain member callback,
                // `(uint32)->bool callback_b = callback_a;`, keeps the name
                // (`logic/helper/callback_tests.k`, the recorded walk).
                if let Some((dm, var, init)) = self.const_init(m, *e)
                    && matches!(&dm.ir.arena.node(var).exp, Exp::Variable { ty, .. }
                        if matches!(dm.ir.arena.node(*ty).exp, Exp::Const(_)))
                    && Self::type_of(dm, init).is_some_and(|t| matches!(t, Ty::Function { .. } | Ty::Closure { .. }))
                {
                    return self.node_with_src(dm, init, src);
                }
                // `infer (NamedValueF a) = typeOf a` (`Type.hs`): the name
                // inside is typed as the value is.
                let e = match &m.ir.arena.node(*e).exp {
                    Exp::QualifiedIdentifier(_) => self.qualified_identifier(m, *e, false, Some(id))?,
                    _ => w!(*e),
                };
                self.set_location(m, &src);
                self.backend.ParseNamedVariable(e)
            }
            Exp::Binary(op, a, b) => {
                let a = w!(*a);
                let b = w!(*b);
                self.set_location(m, &src);
                self.backend.ParseBinaryOp(*op as u32, a, b)
            }
            Exp::Unary(op, e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                match op {
                    kg_const::UnOp::BitSizeof => self.backend.ParseSizeOf(0, e),
                    kg_const::UnOp::ByteSizeof => self.backend.ParseSizeOf(1, e),
                    kg_const::UnOp::Clog2 => return Err(Self::decline("fold:clog2", &src)),
                    op => self.backend.ParseUnaryOp(*op as u32, e),
                }
            }
            Exp::FunctionCall { attr, function, args } => {
                if self.calls_an_uninstantiated_template(m, *function, *args) {
                    return Err(Self::decline("call of a template", &src));
                }
                if self.calls_a_variable(m, *function) {
                    return Err(Self::decline("call of a variable", &src));
                }
                if self.calls_a_member_template(m, *function) {
                    return Err(Self::decline("call of a member template", &src));
                }
                if self.calls_an_undeclared_function(m, *function) {
                    return Err(Self::decline("call of an undeclared function", &src));
                }
                let a = w!(*attr);
                let f = w!(*function);
                let g = self.call_arguments(m, *function, *args)?;
                self.set_location(m, &src);
                self.backend.ParseFunctionCall(f, g, a)
            }
            Exp::ArrayAccess { base, index } => {
                let a = w!(*base);
                let b = w!(*index);
                self.set_location(m, &src);
                self.backend.ParseAccessArray(a, b)
            }
            Exp::ArrayAccessLValue { base, index } => {
                let a = w!(*base);
                let b = w!(*index);
                self.set_location(m, &src);
                self.backend.ParseAccessArrayLValue(a, b)
            }
            Exp::MemberAccess { base, member } => {
                let method = self.names_a_method(m, *base, *member);
                let a = w!(*base);
                let b = w!(*member);
                if method {
                    // `deduceTemplateArgs`' first clause (`Type.hs:1390-1391`):
                    // a member access that names a method of a class-typed
                    // object — typed `TUndefined`, being no field — is a
                    // method reference, `FunctionSpecifierF (Just a)`
                    // `(Class::b)`, whose qualifier `resolveNames` then drops
                    // (`Frontend.hs:284`). A callback initialized with
                    // another object's method is the shape
                    // (`auto callback = d.c[1].u32_F_u32;`,
                    // `callbacks.k::5`); as a member access the backend
                    // answered "Type error" on a program the reference
                    // compiles.
                    self.set_location(m, &src);
                    let list = self.backend.ParseBaseList(b);
                    self.set_location(m, &src);
                    let scoped = self.backend.ParseScopedIdentifier(list);
                    self.set_location(m, &src);
                    return Ok(self.backend.ParseFunctionSpecifier(a, scoped));
                }
                self.set_location(m, &src);
                self.backend.ParseAccessMember(a, b)
            }
            Exp::Cast { ty, value } => {
                let a = w!(*ty);
                let b = w!(*value);
                self.set_location(m, &src);
                self.backend.ParseCast(a, b)
            }
            Exp::Mux { selector, cases } => {
                let a = w!(*selector);
                let b = w!(*cases);
                self.set_location(m, &src);
                self.backend.ParseMux(a, b)
            }
            Exp::Concat(e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                self.backend.ParseConcat(e)
            }
            Exp::FanOut { count, value } => {
                let a = w!(*count);
                let b = w!(*value);
                self.set_location(m, &src);
                self.backend.ParseFanOut(a, b)
            }
            Exp::Static(e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                self.backend.ParseStatic(e)
            }
            Exp::Annotated { attr, stmt } => {
                let a = w!(*attr);
                let b = w!(*stmt);
                self.set_location(m, &src);
                self.backend.ParseAnnotatedStatement(a, b)
            }
            Exp::Barrier => {
                self.set_location(m, &src);
                self.backend.ParseBarrier()
            }
            Exp::Reorder(e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                self.backend.ParseReorder(e)
            }
            Exp::Return(e) => {
                // `returnVoid` (`Frontend.hs:293-296`): `return f();` of a
                // void call is `SeqF [f(), return]`, the bare return wearing
                // the call's note and the `Seq` the return's — and, run after
                // `removeTemplate`, nothing flattens it.
                if let Some(value) = e
                    && Self::type_of(m, *value).is_some_and(|t| matches!(t, Ty::Void))
                {
                    let value_src = m.ir.arena.node(*value).src;
                    let call = self.node(m, *value)?;
                    self.set_location(m, &value_src);
                    let ret = self.backend.ParseReturn();
                    self.set_location(m, &src);
                    self.list(&[call, ret])
                } else {
                    let targets = self
                        .returns
                        .iter()
                        .position(|(r, _)| *r == id)
                        .map(|i| std::mem::take(&mut self.returns[i].1))
                        .unwrap_or_default();
                    let saved = std::mem::replace(&mut self.conversions, targets);
                    let e = e.map(|e| self.node(m, e)).transpose();
                    self.conversions = saved;
                    let e = e?;
                    self.set_location(m, &src);
                    match e {
                        Some(e) => self.backend.ParseReturnExpression(e),
                        None => self.backend.ParseReturn(),
                    }
                }
            }
            Exp::DoWhile { attr, cond, body } => {
                let a = w!(*attr);
                let b = w!(*cond);
                let c = w!(*body);
                self.set_location(m, &src);
                self.backend.ParseDoWhile(a, b, c)
            }
            Exp::UnrolledFor { var, limit, body } | Exp::StaticFor { var, limit, body } => {
                let a = w!(*var);
                let b = w!(*limit);
                let c = w!(*body);
                self.set_location(m, &src);
                self.backend.ParseUnrolledFor(a, b, c)
            }
            Exp::RangeFor { attr, var, limit, body } => {
                let a = w!(*attr);
                let b = w!(*var);
                let c = w!(*limit);
                let d = w!(*body);
                self.set_location(m, &src);
                self.backend.ParseRangeFor(a, b, c, d)
            }
            Exp::If { cond, then_branch, else_branch } => {
                let a = w!(*cond);
                let b = w!(*then_branch);
                let c = opt!(*else_branch);
                self.set_location(m, &src);
                self.backend.ParseIf(a, b, c.unwrap_or(null))
            }
            Exp::StaticIf { .. } => return Err(Self::decline("fold:static if", &src)),
            Exp::Switch { selector, blocks } => {
                let a = w!(*selector);
                let b = w!(*blocks);
                self.set_location(m, &src);
                self.backend.ParseSwitch(a, b)
            }
            Exp::SwitchCase { value, body } => {
                let a = w!(*value);
                let b = w!(*body);
                self.set_location(m, &src);
                self.backend.ParseSwitchBlock(a, b)
            }
            Exp::SwitchDefault(body) => {
                let b = w!(*body);
                self.set_location(m, &src);
                self.backend.ParseSwitchBlock(null, b)
            }
            Exp::Assign { lvalue, value } => {
                let a = w!(*lvalue);
                let b = w!(*value);
                self.set_location(m, &src);
                self.backend.ParseAssign(a, b)
            }
            Exp::NestedScope { name, body, .. } => {
                let _n = w!(*name);
                let b = w!(*body);
                self.set_location(m, &src);
                self.backend.ParseNestedScope(b)
            }
            Exp::ModuleIdentifier(parts) => {
                self.set_location(m, &src);
                // `moduleNamespace`: `concatMap ('@':)` (`Syntax.hs:273-274`).
                let text: String = parts.iter().map(|p| format!("@{p}")).collect();
                self.backend.ParseIdentifier(Some(&text))
            }
            Exp::ModuleDiff(..) => {
                self.set_location(m, &src);
                null
            }
            Exp::ModuleDecl { name, exposed, body } => {
                let n = w!(*name);
                for &e in exposed {
                    self.node(m, e)?;
                }
                let b = w!(*body);
                self.set_location(m, &src);
                self.backend.ParseNamespace(n, b)
            }
            Exp::EnumConstant { scope, name, value } => {
                let n = w!(*name);
                let v = match value {
                    // A written value the fold reduces is the literal the
                    // fold leaves (`IntLiteralF Nothing`,
                    // `Constexpr.hs:117-283`) under the expression's own
                    // note, typed as the expression was: `-128` is the
                    // negation of a `uint8`, an `int9`, and `-1` an `int2`
                    // (`enum.k::27,33`, the recorded walk). An expression
                    // the table left untyped is typed as a bare literal. A
                    // suffixed literal keeps its written type through the
                    // ordinary walk.
                    Some(v) => match Self::folded(m, *v) {
                        Some(kg_const::ConstValue::Int { ty: None, value: folded })
                            if !matches!(m.ir.arena.node(*v).exp, Exp::IntLiteral { .. }) =>
                        {
                            let donor = m.ir.arena.node(*v).src;
                            let typed = Self::type_of(m, *v).and_then(|t| integer_type(&t));
                            self.minted_literal_typed(m, &folded, &donor, typed)
                        }
                        _ => w!(*v),
                    },
                    None => match self.enum_values.get(&id).cloned() {
                        // `NotedExp n i`: the minted literal wears the
                        // constant's note and is typed as a bare literal.
                        Some(v) => self.minted_literal(m, &v, &src),
                        None => return Err(Self::decline("reifyEnums", &src)),
                    },
                };
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                self.backend.ParseEnumConstant(n, v, &refs)
            }
            Exp::Enum { scope, name, ty, body } => {
                self.reify_enum(m, *body);
                let a = w!(*name);
                // `checkEnumBaseType a | tIsInt $ theType a = copyNote
                // (fromJust $ expFromType a $ theType a) a`
                // (`TypeCheck.hs:201-203`): pass 19 respells the base as
                // its integer type, so `enum Foo : DWORD` over `using DWORD
                // = uint32` reaches the backend as `ParseUintType 32` — an
                // identifier there aborts it (`enum.k::24,25`). A base the
                // table did not type is walked as written.
                let base = Self::type_of(m, *ty).map(|t| match t {
                    Ty::Type(inner) => (*inner).clone(),
                    t => t,
                });
                let b = match base {
                    Some(t @ (Ty::Signed(_) | Ty::Unsigned(_))) => {
                        let at = m.ir.arena.node(*ty).src;
                        self.type_expression(m, &t, &at)?
                    }
                    _ => w!(*ty),
                };
                let c = w!(*body);
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                self.backend.ParseEnum(a, b, c, &refs)
            }
            Exp::Struct { scope, name, body } => {
                let a = w!(*name);
                let b = w!(*body);
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                let unmangled = self.unmangled(m, id);
                self.backend.ParseStruct(a, b, &refs, unmangled.as_deref())
            }
            Exp::Union { scope, name, body } => {
                let a = w!(*name);
                let b = w!(*body);
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                let unmangled = self.unmangled(m, id);
                self.backend.ParseUnion(a, b, &refs, unmangled.as_deref())
            }
            Exp::Class { scope, name, body } => {
                let a = w!(*name);
                let b = w!(*body);
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                // `parseInstanceType (theType expr)` (`ParseTree.hs:237-253`):
                // a copy's type is `TInstance template args`
                // (`setTypeOfInstance`, `Type.hs:426-440`), spelled as the
                // template's qualified name and one named argument per
                // parameter; NULL for a class that is no instance.
                let instance = self.typed_instance(m, id);
                let unmangled = self.unmangled(m, id);
                self.backend.ParseClass(a, b, instance.unwrap_or(null), &refs, unmangled.as_deref())
            }
            Exp::DefaultInitialization(e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                self.backend.ParseDefaultInitialization(e)
            }
            Exp::Private => {
                self.set_location(m, &src);
                self.backend.ParseMemberModifier(1)
            }
            Exp::Public => {
                self.set_location(m, &src);
                self.backend.ParseMemberModifier(0)
            }
            Exp::Template { .. } => return Err(Self::decline("template", &src)),
            // `eval (DecltypeF _ x) = expFromType x (typeOf x)`
            // (`Constexpr.hs:304-305`): the operand's type spelled out, its
            // nodes wearing the operand's note.
            Exp::Decltype { expr, .. } => {
                let operand = m.ir.arena.node(*expr).src;
                let Some(ty) = Self::type_of(m, *expr) else {
                    return Err(Self::decline("fold:decltype", &src));
                };
                return self.type_expression(m, &ty, &operand);
            }
            Exp::Alias { scope, body, name } => {
                let a = w!(*body);
                let b = w!(*name);
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                // `withUnmangledName` asks `theType expr` first
                // (`ParseTree.hs:64,177-178`) — the note `setTypeOfInstance`
                // wrote at the declaration, which an alias template's copy
                // has: `using Pair = Tuple2<T1, T2>`'s copy is `Pair<bool,
                // uint4>`, not the `Tuple2` it aliases
                // (`template-argument-deduction-from-instance.k::1`, the
                // recorded walk). `typeOf expr` is the second arm: `using
                // C_t = C<2>` names the instance it aliases
                // (`constexpr.k:131`).
                let unmangled = self.unmangled(m, id).or_else(|| self.aliased_instance(m, id));
                self.backend.ParseTypedef(a, b, &refs, unmangled.as_deref())
            }
            Exp::Capture { .. } => return Err(Self::decline("capture", &src)),
            Exp::Variable { scope, attr, ty, name, init, flags } => {
                let a = w!(*attr);
                let b = if Self::is_auto(m, *ty) {
                    self.deduced_type(m, id, *ty, false)?
                } else if let Some(function) = Self::aliased_callback(m, *ty, *flags) {
                    // `checkVarDecl`'s last clause (`TypeCheck.hs:141-142`,
                    // pass 19): "middle-end doesn't handle function type
                    // aliases" — a class's callback written as a name is the
                    // function type `expFromType` spells, under the name's
                    // note (`callbacks.k::0`'s `callback_uint32_to_uint32_t
                    // callback1`).
                    let at = m.ir.arena.node(*ty).src;
                    self.type_expression(m, &function, &at)?
                } else {
                    w!(*ty)
                };
                let c = w!(*name);
                let d = match init {
                    // `checkVarDecl`'s `reduceClosure` (`TypeCheck.hs:129,
                    // 146-148`): a global initialized with a closure that
                    // captures nothing is initialized with `{}`, both nodes
                    // wearing the initializer's note — `const auto
                    // add_and_increment = compose2(…)` at a module's top
                    // level (`higher-order-functions.k::0`). A capture is
                    // pass 19's error and never reaches the walk.
                    Some(i) if *flags == kg_resolve::DeclareFlag::DeclareGlobal && Self::captures_nothing(m, *i) => {
                        let at = m.ir.arena.node(*i).src;
                        self.set_location(m, &at);
                        let empty = self.list(&[]);
                        self.set_location(m, &at);
                        Some(self.backend.ParseInitializerList(empty))
                    }
                    // `methodSpecifier` (`Parser.hs:730-735`): the reference's
                    // parser resolves `c.void_F_void` against the receiver's
                    // type and builds a `FunctionSpecifier`, where this CST
                    // has only a member access — `() -> void callback =
                    // c.void_F_void;` (`callbacks.k::2`, the recorded walk).
                    // The grammar decides it, not the type: a callback's
                    // initializer is a function specifier wherever it is a
                    // member of something.
                    Some(i)
                        if matches!(m.ir.arena.node(*i).exp, Exp::MemberAccess { .. })
                            && Self::type_of(m, *ty)
                                .is_some_and(|t| matches!(&t, Ty::Type(inner) if inner.un_const().is_function())) =>
                    {
                        let Exp::MemberAccess { base, member } = m.ir.arena.node(*i).exp else {
                            unreachable!("matched above")
                        };
                        self.scopes.push(scope.clone());
                        let r = self.node(m, base);
                        self.scopes.pop();
                        let r = r?;
                        let name = self.node(m, member)?;
                        let at = m.ir.arena.node(member).src;
                        self.set_location(m, &at);
                        let list = self.backend.ParseBaseList(name);
                        self.set_location(m, &at);
                        let scoped = self.backend.ParseScopedIdentifier(list);
                        self.set_location(m, &m.ir.arena.node(*i).src);
                        Some(self.backend.ParseFunctionSpecifier(r, scoped))
                    }
                    _ => {
                        self.scopes.push(scope.clone());
                        // `checkVarDecl`'s `checkConversion` against the
                        // declared type (`TypeCheck.hs:123-144`), whose
                        // `EnumValue` clause is the one `call_arguments`
                        // applies — `uint8 u = E::X;` is the bare literal.
                        // A list whose first element is designated is
                        // checked with `errorNarrowingConversions`
                        // (`TypeCheck.hs:130`).
                        let mut targets = Conversions::default();
                        if let (Some(i), Some(Ty::Type(t))) = (*init, Self::type_of(m, *ty))
                            && t.resolved()
                        {
                            Self::conversions(m, i, &t, !Self::designated_first(m, i), false, &mut targets);
                        }
                        let saved = std::mem::replace(&mut self.conversions, targets);
                        let d = init.map(|i| self.node(m, i)).transpose();
                        self.conversions = saved;
                        self.scopes.pop();
                        let d = d?;
                        // `checkVarDecl`'s narrowing clause
                        // (`TypeCheck.hs:123-144`, `:176-177`): the
                        // initializer is stamped a second time against the
                        // declared type, as `checkArg` stamps an argument
                        // against the parameter's — `uint2 x = Fn_T_T(10);`
                        // over a `uint4` call (`return-type-polymorphism.k::6`,
                        // the recorded walk).
                        if let (Some(node), Some(i)) = (d, *init)
                            && let Some(Ty::Type(t)) = Self::type_of(m, *ty)
                            // `checkVar`'s guard (`TypeCheck.hs:130`): both
                            // types resolved and the declared one no function
                            // — an `auto` declaration is `TUnresolved` and
                            // takes no `initValueCheck` at all.
                            && t.resolved()
                            && !t.un_const().is_function()
                            && !matches!(m.ir.arena.node(i).exp, Exp::InitializerList(_))
                            // An initializer `evaluateConstexpr` folds is a
                            // *literal* by the time `checkVarDecl` sees it
                            // (`Inference.hs:47`), and `checkConversion`'s
                            // literal arm judges it by `inferIntLiteralType
                            // Nothing` of the value (`TypeCheck.hs:154`) — the
                            // table's type for the written expression is not
                            // what the reference compares (`helper/
                            // constexpr.k`'s `C<2>` members, 72 programs,
                            // measured 2026-09-20). `const uint<5> x = 78;` is
                            // `uint7` into `uint5` (`constexpr.k::82`, the
                            // recorded walk).
                            // An enum value an integer declaration turns
                            // into its literal is judged the same way:
                            // `const uint8 count = Foo::B;` over `B = -1`
                            // is `int1` into `uint8` (`enum.k::44`).
                            && match Self::folded(m, i) {
                                None => Self::narrows(m, i, &t),
                                Some(kg_const::ConstValue::Int { value, .. }) => Self::literal_narrows(value, &t),
                                Some(kg_const::ConstValue::Enum { value, .. }) if t.un_const().is_int() => {
                                    Self::literal_narrows(value, &t)
                                }
                                Some(_) => false,
                            }
                        {
                            let at = m.ir.arena.node(i).src;
                            self.set_location(m, &at);
                            self.set_node_type(m, i, node);
                        }
                        d
                    }
                };
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                // `declareFlag` (`ParseTree.hs:182-188`, `Types.hsc:44-49`).
                let flags = match flags {
                    kg_resolve::DeclareFlag::DeclareGlobal => 0x02 | 0x04,
                    kg_resolve::DeclareFlag::DeclareClass => 0x02 | 0x04 | 0x80,
                    kg_resolve::DeclareFlag::DeclareLocal => 0x04,
                    kg_resolve::DeclareFlag::DeclareStatic => 0x02 | 0x04 | 0x40,
                    kg_resolve::DeclareFlag::DeclareMember => 0,
                    kg_resolve::DeclareFlag::DeclareUninitConst => 0x100,
                };
                self.backend.ParseDeclare(a, b, c, d.unwrap_or(null), flags, &refs)
            }
            Exp::FuncParam { scope, attr, ty, name } => {
                let a = w!(*attr);
                let b = w!(*ty);
                let c = w!(*name);
                self.set_location(m, &src);
                let scope = self.scope(scope);
                let refs: Vec<&str> = scope.iter().map(String::as_str).collect();
                self.backend.ParseFunctionParam(a, b, c, &refs)
            }
            Exp::Function { scope, attr, modifier, ret, name, params, body, .. } => {
                let a = w!(*attr);
                let mo = opt!(*modifier);
                let r = if Self::is_auto(m, *ret) { self.deduced_type(m, id, *ret, true)? } else { w!(*ret) };
                let n = w!(*name);
                let p = w!(*params);
                // An extern class's functions were trimmed by pass 7 on the
                // loop (`trimExternClasses`, `Frontend.hs:315-327`;
                // `kg_sem::pass7`).
                let saved = self.returns.len();
                Self::return_conversions(m, id, *body, &mut self.returns);
                self.frames.push(Self::frame(m, *params, *body));
                self.scopes.push(scope.clone());
                let b = self.node(m, *body);
                self.scopes.pop();
                self.frames.pop();
                self.returns.truncate(saved);
                let b = b?;
                self.set_location(m, &src);
                let unmangled = self.unmangled(m, id);
                self.backend.ParseFunction(a, mo.unwrap_or(null), r, n, p, b, unmangled.as_deref())
            }
            Exp::FunctionDecl { attr, ret, name, params, .. } => {
                let a = w!(*attr);
                let r = w!(*ret);
                let n = w!(*name);
                let p = w!(*params);
                self.set_location(m, &src);
                self.backend.ParseExternalFunction(a, r, n, p)
            }
            Exp::Lambda { .. } => return Err(Self::decline("lambda", &src)),
            Exp::ExportType { attr, ty } => {
                let a = w!(*attr);
                let b = w!(*ty);
                self.set_location(m, &src);
                self.backend.ParseExportType(a, b)
            }
            Exp::Extern { attr, ty } => {
                let a = w!(*attr);
                let b = w!(*ty);
                self.set_location(m, &src);
                self.backend.ParseExtern(a, b)
            }
            Exp::StaticAssert(_) => return Err(Self::decline("fold:static assert", &src)),
            Exp::StaticAssertLegacy(e) => {
                let e = w!(*e);
                self.set_location(m, &src);
                self.backend.ParseStaticAssert(e)
            }
            Exp::Warning { node, .. } => {
                let a = w!(*node);
                self.set_location(m, &src);
                a
            }
            Exp::Error(_) => return Err(Self::decline("error node", &src)),
            Exp::Path(_) => return Err(Self::decline("unsorted path", &src)),
            Exp::Unsupported => return Err(Self::decline("unsupported", &src)),
        };
        self.set_node_type(m, id, result);
        Ok(result)
    }
}

/// `Foo<bool, Do::Bar>` read back as `Foo<bool, Do, Bar>` when the note says
/// three arguments: the mangle writes a name argument as a run of `@`
/// segments and the next argument's run follows with no separator, so two
/// bare names join into one qualified one. Only a part whose own components
/// are plain leaves is split — a namespaced name (`helper.memory::read_mem`)
/// and a nested copy's display are left whole — and only until the count the
/// note gives is reached. Never padded: an invented argument is worse than a
/// missing one.
/// Literal arithmetic over the IR — `(10 + 1)`, `-2`, `!true` — as
/// `evaluateConstexpr` folds it; `None` for anything with a name in it.
fn literal_arithmetic(m: Module<'_>, id: ExpId) -> Option<kg_const::ConstValue> {
    kg_hir::constexpr::value(&m.ir.arena, id, &kg_hir::constexpr::Closed)
}
#[cfg(test)]
mod tests {
    use super::{decode_piece, parse_format};

    /// `formatedExpr` (`Parser.hs:894-903`) on the shapes the walk emits, and
    /// the ones it declines: an alignment, a member access, a literal.
    #[test]
    fn a_hole_is_a_bare_name_with_an_optional_format() {
        assert_eq!(parse_format("x8"), Some((3, Some(8))));
        assert_eq!(parse_format("b"), Some((0, None)));
        assert_eq!(parse_format(" d10 "), Some((2, Some(10))));
        assert_eq!(parse_format("X"), Some((4, None)));
        assert_eq!(parse_format("q"), None);
        assert_eq!(parse_format("d 10"), None);
        assert_eq!(decode_piece("\"a\\n{{", true, false), b"a\n{".to_vec());
        assert_eq!(decode_piece("}x\"", false, true), b"}x".to_vec());
    }
}
