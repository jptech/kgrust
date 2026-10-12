//! The program-wide loop's elaboration (`kg_sem::elaborate`,
//! `docs/history/ir-elaboration.md` §2.4) as the database holds it: run once
//! per program, and read by the backend walk, by the CST run's admission of
//! what the loop decided at each module's text (`crate::passes::FromLoop`),
//! by the parser's checks — what the parser built of each module, its
//! sorted tree ([`Looped::parsed`]), and the loop's types at its text — and
//! by the editor.
//!
//! Until sprint W3.7 it also derived a per-module type table for the CST
//! passes: the CST engine's base table with each resolved type the loop
//! gave an `Own` node of the module's text written over it, keyed by the
//! node's range and kind. The last reader, the parser's two checks, reads
//! the loop's types where the node is ([`Looped::written_at`]) and what the
//! parser built, and the table — and with it the CST engine's inference of
//! every module on every loop — is gone.

use std::sync::Arc;

use kg_base::{FileId, FxHashMap, Written};
use kg_resolve::ModuleSource;

use crate::db::Db;
use crate::text_map::TextMap;

/// One program's loop elaboration, and what the editor and the parser's
/// checks read of it.
#[derive(Debug)]
pub struct Looped {
    /// The modules the loop elaborated, in program order.
    pub modules: Vec<ModuleSource>,
    /// Shared with the editor's kept snapshot of the same run
    /// (`crate::snapshot`), which outlives this answer's memo.
    pub out: Arc<kg_sem::elaborate::Elaborated>,
    /// What the passes over the copies raised (`kg_sem::check`), filed
    /// under the module whose file each is reported in, each with the pass
    /// it is the reference's.
    diagnostics: FxHashMap<ModuleSource, Vec<(kg_sem::check::Stage, kg_diag::Diagnostic)>>,
    /// The verdicts at each module's own text of the passes the loop judges
    /// there (`kg_sem::check::Checked::text`), for the CST run to admit in
    /// those passes' places.
    text: FxHashMap<ModuleSource, Vec<kg_sem::check::TextVerdict>>,
    /// Whether the loop's own part of the program erred through pass 9
    /// (`kg_sem::check::Checked::erred`).
    erred: bool,
    /// Each module's position index over its final tree, built on first
    /// ask: the gates that read this never ask, the editor asks per module.
    indexes: Vec<std::sync::OnceLock<kg_hir::position::PositionIndex>>,
    /// Each module's copies by the text they were copied from, built on
    /// first ask, which is the editor's first question about a node the
    /// written tree leaves untyped.
    copies: Vec<std::sync::OnceLock<kg_hir::position::CopyIndex>>,
    /// Each copy's instance as a message spells it, by its expansion —
    /// built on first ask, over the whole program.
    instances: std::sync::OnceLock<FxHashMap<kg_base::Expansion, String>>,
    /// `getSymbols` over the final program, built on first use by
    /// [`Looped::instance_named`].
    symbols: std::sync::OnceLock<kg_sem::symbols::Symbols>,
    /// The parse each module was lowered from, in [`Looped::modules`]'
    /// order: the text every range in the loop is a range of.
    parses: Vec<Arc<kg_syntax::Parse>>,
    /// Each module's sorted lowering, as the loop was handed it — what the
    /// reference's parser built — in [`Looped::modules`]' order.
    sorted: Vec<Arc<kg_hir::ModuleIr>>,
    /// What each module's parser checks ask of its parse
    /// ([`crate::parsed::Parsed`]), built on first ask.
    parsed: Vec<std::sync::OnceLock<crate::parsed::Parsed>>,
    /// Each module's [`TextMap`] onto the last later parse an editor asked
    /// about — the buffer as it stands, asked again by every request until
    /// the next edit — so the text is rebuilt from the trees once per edit.
    maps: std::sync::Mutex<FxHashMap<ModuleSource, MapOnto>>,
}

/// A [`TextMap`] onto a later parse, and the parse.
type MapOnto = (Arc<kg_syntax::Parse>, Arc<TextMap>);

impl Looped {
    /// `lookupSymbolByName symbols` as `unmangle` asks it
    /// (`Type.hs:198-205`): the note at the declaration `name` names in the
    /// loop's final program — a copy's `TInstance` — or `None`. What a
    /// diagnostic spells a type by (`showUnmangled`, `TypeCheck.hs:36`).
    pub fn instance_named(&self, name: &[kg_base::Symbol], interner: &kg_base::Interner) -> Option<kg_ty::Ty> {
        kg_sem::instance::noted(&self.out, self.symbols(interner), name)
    }

    fn symbols(&self, interner: &kg_base::Interner) -> &kg_sem::symbols::Symbols {
        self.symbols.get_or_init(|| {
            kg_sem::symbols::Symbols::with(self.out.modules.iter().map(|m| &m.ir), interner, kg_sem::symbols::Config::AllSymbols)
        })
    }

    /// What a `.` reaches on a value of type `ty` in the loop's program:
    /// the fields and methods the aggregate's declaration holds — an
    /// instance's copy, for a template's instance — each with the type the
    /// loop gave it there, in the order they are written. `None` where `ty`
    /// names no aggregate the loop declares.
    ///
    /// The aggregate is the reference's: `tLookupField` reads through
    /// `TConst` and `TInstance` to a struct's, union's or class's own field
    /// list (`Type.hs:961-971`), a `TReference` — `this` — is the class its
    /// name looks up (`Type.hs:1232-1235`), and a class's methods are what
    /// `lookupSymbolByName (x ++ [y])` finds under it (`Type.hs:1359`): the
    /// declarations in its body. Only a member written in some text is
    /// listed (`Written::Own`), since a member is offered by the
    /// declaration it jumps to; one a rewrite invented has none.
    /// Where the struct, union, class or enum `ty` names was written — for
    /// a template's instance, the template's text, which the copy keeps
    /// (`Src::expanded`): the module, and the range and kind of the CST
    /// declaration. Read through `TConst`, `TInstance`, a type's own type
    /// and an array to its element, which is what "the type of this" asks
    /// for (go to type definition). `None` for a type no declaration of the
    /// loop's program names — a builtin, a function — or one no text wrote.
    pub fn type_declaration(
        &self,
        ty: &kg_ty::Ty,
        interner: &kg_base::Interner,
    ) -> Option<(ModuleSource, kg_base::TextRange, kg_syntax::SyntaxKind)> {
        use kg_ty::Ty;
        let mut ty = ty;
        let name = loop {
            ty = match ty {
                Ty::Const(inner) | Ty::Instance { concrete: inner, .. } | Ty::Type(inner) => &**inner,
                Ty::Array { elem, .. } => &**elem,
                Ty::Struct(q, _) | Ty::Union(q, _) | Ty::Class(q, _) | Ty::Reference(q) | Ty::Enum(q, _) => break q,
                _ => return None,
            };
        };
        let decl = self.symbols(interner).by_name(name)?;
        let m = self.out.modules.get(decl.module)?;
        let src = m.ir.arena[decl.id].src;
        if src.head.file != m.ir.file || src.written == Written::Donated {
            return None;
        }
        Some((*self.modules.get(decl.module)?, src.head.range, m.ir.arena[decl.id].kind))
    }

    pub fn members(&self, ty: &kg_ty::Ty, interner: &kg_base::Interner) -> Option<Vec<Member>> {
        use kg_ty::Ty;
        let mut ty = ty;
        let name = loop {
            ty = match ty {
                Ty::Const(inner) | Ty::Instance { concrete: inner, .. } => &**inner,
                Ty::Struct(q, _) | Ty::Union(q, _) | Ty::Class(q, _) | Ty::Reference(q) => break q,
                _ => return None,
            };
        };
        let decl = self.symbols(interner).by_name(name)?;
        let m = self.out.modules.get(decl.module)?;
        let module = *self.modules.get(decl.module)?;
        let body = match &m.ir.arena[decl.id].exp {
            kg_hir::Exp::Struct { body, .. } | kg_hir::Exp::Union { body, .. } | kg_hir::Exp::Class { body, .. } => *body,
            _ => return None,
        };
        let kg_hir::Exp::Seq(declared) = &m.ir.arena[body].exp else { return None };
        // A member template's instances are copies made inside the body,
        // one expansion deeper than the aggregate's own text
        // (`twice<|$uint@8|>` beside `twice`): an instance is no name a
        // user writes after the `.`. The function a rewrite keeps beside
        // the template it made (`abbreviatedFunctionTemplate`'s leftover,
        // `Function.hs:289-292`) is `Donated` and wears the template's
        // text, and no member either: the template is.
        let own = m.ir.arena[decl.id].src.via;
        let function = |id: kg_hir::ExpId| {
            matches!(m.ir.arena[id].exp, kg_hir::Exp::Function { .. } | kg_hir::Exp::FunctionDecl { .. })
        };
        let mut out: Vec<(Written, Member)> = declared
            .iter()
            .filter_map(|&id| {
                // A member function template is its function's text: one
                // written `template <…>`, which the caller's item tree
                // files as a template and declines, and one a rewrite made
                // of a function — taking a function (`higherOrderFunctions`,
                // `Function.hs:313-361`) or an `auto`
                // (`abbreviatedFunctionTemplate`, `Function.hs:288-311`) —
                // which the text declares as a function.
                let written = match &m.ir.arena[id].exp {
                    kg_hir::Exp::Variable { .. } | kg_hir::Exp::Function { .. } | kg_hir::Exp::FunctionDecl { .. } => id,
                    kg_hir::Exp::Template { decl, .. } if function(*decl) => *decl,
                    _ => return None,
                };
                let node = &m.ir.arena[written];
                if node.src.via != own || node.src.head.file != m.ir.file {
                    return None;
                }
                // A method whose body held a lambda is rebuilt round the
                // body `desugarLambdas` leaves (`Function.hs:206-214`) and
                // wears its old range, `Donated`: still the member its text
                // declares, as long as nothing `Own` claims that text (below).
                let rebuilt = node.src.written == Written::Donated && written == id && function(id);
                if node.src.written != Written::Own && !rebuilt {
                    return None;
                }
                let (_, name) = kg_sem::symbols::declared_name(&m.ir.arena, id)?;
                Some((node.src.written, Member { name, module, range: node.src.head.range, kind: node.kind, ty: m.types.get(id) }))
            })
            .collect();
        // The text's order, which the item tree's is: a rewrite moves a
        // declaration within the body without moving its text — an
        // instance's copy lands ahead of its template.
        out.sort_by_key(|(written, member)| (member.range.start(), *written != Written::Own));
        // One member per text, the `Own` one first: the function
        // `abbreviatedFunctionTemplate` keeps beside the template it made
        // is `Donated` too, and the template is the member.
        out.dedup_by(|(_, later), (_, kept)| later.name == kept.name && later.range == kept.range);
        Some(out.into_iter().map(|(_, member)| member).collect())
    }

    /// The loop's final tree and types for `module`.
    pub fn module(&self, module: ModuleSource) -> Option<&kg_sem::Module> {
        let i = self.modules.iter().position(|&m| m == module)?;
        self.out.modules.get(i)
    }

    /// The parse `module` was lowered from — the text every range of it in
    /// this loop is a range of.
    pub fn parse(&self, module: ModuleSource) -> Option<&Arc<kg_syntax::Parse>> {
        let i = self.modules.iter().position(|&m| m == module)?;
        self.parses.get(i)
    }

    /// The position index (`kg_hir::position`) over `module`'s final tree —
    /// how an editor question at an offset reaches the node the loop typed.
    pub fn index(&self, module: ModuleSource) -> Option<&kg_hir::position::PositionIndex> {
        let i = self.modules.iter().position(|&m| m == module)?;
        let ir = &self.out.modules.get(i)?.ir;
        Some(self.indexes.get(i)?.get_or_init(|| kg_hir::position::PositionIndex::build(ir)))
    }

    /// The type at the `Own` node of `module`'s written tree lowered from
    /// the CST node at `range` with `kind` — `None` where the final tree
    /// holds no such node: a module the loop did not lower, a declaration a
    /// rewrite replaced or a redefinition the loop dropped. "No answer", not
    /// [`kg_ty::Ty::Unresolved`].
    pub fn written_at(&self, module: ModuleSource, range: kg_base::TextRange, kind: kg_syntax::SyntaxKind) -> Option<kg_ty::Ty> {
        let id = self.index(module)?.node(range, kind)?;
        Some(self.module(module)?.types.get(id))
    }

    /// Each instantiated copy's node at the same text — the template body
    /// there in each instance — read through `read`, in the order the loop
    /// decided the instances (`kg_hir::position::CopyIndex`); a copy `read`
    /// declines is left out.
    fn copies_by(
        &self,
        module: ModuleSource,
        range: kg_base::TextRange,
        kind: kg_syntax::SyntaxKind,
        interner: &kg_base::Interner,
        read: &dyn Fn(&kg_sem::Module, kg_hir::ExpId) -> Option<kg_ty::Ty>,
    ) -> Vec<Copied> {
        let Some(i) = self.modules.iter().position(|&m| m == module) else { return Vec::new() };
        let Some(m) = self.out.modules.get(i) else { return Vec::new() };
        let Some(index) = self.copies.get(i) else { return Vec::new() };
        let nodes = index.get_or_init(|| kg_hir::position::CopyIndex::build(&m.ir)).nodes(range, kind);
        nodes
            .into_iter()
            .filter_map(|(via, id)| Some(Copied { ty: read(m, id)?, instance: self.instance_chain(via, interner) }))
            .collect()
    }

    /// What the loop says about the CST node at `range` with `kind`, to a
    /// reader that prints a type where `shown` says it can: the written
    /// tree's type when it has one to show, and otherwise the copies'
    /// (`IrAnswer`); `None` where neither has, and there is no type to show.
    pub fn answer_at(
        &self,
        module: ModuleSource,
        range: kg_base::TextRange,
        kind: kg_syntax::SyntaxKind,
        interner: &kg_base::Interner,
        shown: &dyn Fn(&kg_ty::Ty) -> bool,
    ) -> Option<IrAnswer> {
        self.answer_by(module, range, kind, interner, shown, &|m, id| Some(m.types.get(id)))
    }

    /// The value the loop gave the enum constant declared by the CST node
    /// at `range` with `kind`: `reify_enums` numbers an implicit constant
    /// on from the one before (`pass1.rs`) and the constant folding leaves
    /// each an integer literal, which is what the type checker reads
    /// (`Type.hs:1087-1090`). `None` where the final tree holds no such
    /// constant, or its value is not a literal.
    pub fn enum_value_at(&self, module: ModuleSource, range: kg_base::TextRange, kind: kg_syntax::SyntaxKind) -> Option<kg_const::ConstInt> {
        let id = self.index(module)?.node(range, kind)?;
        let ir = &self.module(module)?.ir;
        let kg_hir::Exp::EnumConstant { value: Some(v), .. } = ir.arena[id].exp else { return None };
        match &ir.arena[v].exp {
            kg_hir::Exp::IntLiteral { value, .. } => Some(value.clone()),
            _ => None,
        }
    }

    /// [`Looped::answer_at`] for the call at `range`: the function it
    /// invokes — `FunctionCall`'s callee, which `resolve` renamed to the
    /// instance the call compiles (`Template.hs:158-159`), typed that
    /// instance's `TInstance` — where that is a template's instance.
    /// `None` for a call to a plain function, whose written signature
    /// already says everything this would.
    pub fn callee_at(&self, module: ModuleSource, range: kg_base::TextRange, interner: &kg_base::Interner) -> Option<IrAnswer> {
        let callee = |m: &kg_sem::Module, id: kg_hir::ExpId| match m.ir.arena[id].exp {
            kg_hir::Exp::FunctionCall { function, .. } => Some(m.types.get(function)),
            _ => None,
        };
        let instance = |t: &kg_ty::Ty| matches!(t, kg_ty::Ty::Instance { concrete, .. } if concrete.is_function());
        self.answer_by(module, range, kg_syntax::SyntaxKind::CALL_EXPR, interner, &instance, &callee)
    }

    /// The instance a `TInstance` is, spelled as a message spells it
    /// (`unmangleInstanceName`, `Type.hs:176-177`): `take<uint8>`.
    pub fn instance_name(&self, ty: &kg_ty::Ty, interner: &kg_base::Interner) -> Option<String> {
        kg_sem::instance::spelled(&self.out, self.symbols(interner), ty, interner)
    }

    fn answer_by(
        &self,
        module: ModuleSource,
        range: kg_base::TextRange,
        kind: kg_syntax::SyntaxKind,
        interner: &kg_base::Interner,
        shown: &dyn Fn(&kg_ty::Ty) -> bool,
        read: &dyn Fn(&kg_sem::Module, kg_hir::ExpId) -> Option<kg_ty::Ty>,
    ) -> Option<IrAnswer> {
        let written = self.index(module)?.node(range, kind).and_then(|id| read(self.module(module)?, id));
        if let Some(ty) = written.filter(|t| shown(t)) {
            return Some(IrAnswer::Written(ty));
        }
        IrAnswer::of_copies(self.copies_by(module, range, kind, interner, read), shown, self.out.report.budget_exhausted)
    }

    /// A copy's instance spelled as a message spells it — `take<uint8>`,
    /// `Box<4>::get<uint3>` for one made inside another — from the
    /// declarations the copies made (`unmangleInstanceName`,
    /// `Type.hs:176-177`); a `static for` iteration's frame names nothing
    /// (`ExpansionInterner::copy_frames`), and a frame no declaration names
    /// is left out rather than guessed.
    pub fn instance_chain(&self, via: kg_base::Expansion, interner: &kg_base::Interner) -> String {
        let named = self.instances.get_or_init(|| self.name_instances(interner));
        kg_sem::instance::chain(&self.out.expansions, named, via)
    }

    /// Every copy's instance spelled, by the expansion its nodes carry —
    /// the `TInstance` the loop recorded as it made the copy
    /// (`kg_sem::Elaborated::copies`, `setTypeOfInstance`, `Template.hs:125`),
    /// never its mangled name (`xtask lint`'s `mangled-text`).
    fn name_instances(&self, interner: &kg_base::Interner) -> FxHashMap<kg_base::Expansion, String> {
        kg_sem::instance::spelled_copies(&self.out, self.symbols(interner), interner)
    }

    /// How `module`'s ranges in this loop move onto `now`, a later parse of
    /// it — `None` where `now` is the parse the loop read, or the loop did
    /// not read the module.
    fn map_onto(&self, module: ModuleSource, now: &Arc<kg_syntax::Parse>) -> Option<Arc<TextMap>> {
        let i = self.modules.iter().position(|&m| m == module)?;
        let then = &self.parses[i];
        if Arc::ptr_eq(then, now) {
            return None;
        }
        let mut maps = self.maps.lock().unwrap();
        if let Some((at, map)) = maps.get(&module)
            && Arc::ptr_eq(at, now)
        {
            return Some(map.clone());
        }
        let map = Arc::new(TextMap::new(&then.syntax_node().to_string(), &now.syntax_node().to_string()));
        maps.insert(module, (now.clone(), map.clone()));
        Some(map)
    }

    /// What the reference's parser built of `module`, as its own checks
    /// ask it ([`crate::parsed::Parsed`]) — `None` for a module the loop
    /// did not lower.
    pub fn parsed(&self, module: ModuleSource) -> Option<&crate::parsed::Parsed> {
        let i = self.modules.iter().position(|&m| m == module)?;
        Some(self.parsed.get(i)?.get_or_init(|| crate::parsed::Parsed::of(&self.sorted[i])))
    }

    /// The diagnostics the passes over the loop's copies raised in
    /// `module`'s text — at a template body, framed by its instantiation
    /// sites — as the route's twin carried them home, each with the pass it
    /// is the reference's (`kg_sem::check::Stage`).
    pub fn diagnostics_for(&self, module: ModuleSource) -> &[(kg_sem::check::Stage, kg_diag::Diagnostic)] {
        self.diagnostics.get(&module).map_or(&[], Vec::as_slice)
    }

    /// The verdicts at `module`'s own text of each pass the loop judges
    /// there (`kg_sem::check::TextPass`) — admitted by the CST run in each
    /// pass's place (`crate::passes::run_from_10`).
    pub fn text_for(&self, module: ModuleSource) -> &[kg_sem::check::TextVerdict] {
        self.text.get(&module).map_or(&[], Vec::as_slice)
    }

    /// Whether the loop raised an error by the end of pass 9 — a refusal,
    /// a round's error, pass 9 in a copy: its part of the reference's flag
    /// (`kg_sem::check::Checked::erred`; the whole of it is
    /// `crate::Db::program_erred_through_pass_9`).
    pub fn erred_through_pass_9(&self) -> bool {
        self.erred
    }
}

/// A program-wide loop as the editor reads it, at the text it holds now
/// (sprint W3.4): the loop of the program as it stands, or the last one
/// computed for these roots — on a buffer that no longer parses, the one
/// its last parsing state ran — with each range of the current text mapped
/// to the range the loop knows it by ([`TextMap`]).
///
/// Every question goes through the map, and text the edits made is in no
/// loop: a range whose first or last character is new answers nothing, and
/// the editor shows no type there until a loop reads it. A range enclosing an edit maps to
/// the node it was, and the answer is the one the loop had — the last good
/// answer (`docs/history/ir-elaboration.md` §2.6), not the current text's.
/// [`crate::Db::editor_loop`] and its siblings hand these out.
#[derive(Clone, Debug)]
pub struct EditorLoop {
    looped: Arc<Looped>,
    roots: Vec<FileId>,
    /// Each module whose text changed since the loop read it.
    maps: FxHashMap<ModuleSource, Arc<TextMap>>,
}

impl EditorLoop {
    /// `looped` read at the text `db` holds now: each module the loop read
    /// whose parse has moved on is mapped.
    pub(crate) fn new(db: &Db, roots: &[FileId], looped: Arc<Looped>) -> EditorLoop {
        let maps = looped
            .modules
            .iter()
            .filter_map(|&m| Some((m, looped.map_onto(m, &db.parse_module(m))?)))
            .collect();
        EditorLoop { looped, roots: roots.to_vec(), maps }
    }

    /// The loop itself.
    pub fn looped(&self) -> &Arc<Looped> {
        &self.looped
    }

    /// The roots the loop's program was loaded from.
    pub fn roots(&self) -> &[FileId] {
        &self.roots
    }

    /// Whether the loop read the text as it stands — every module's — and
    /// so answers about the program the buffer holds.
    pub fn is_current(&self) -> bool {
        self.maps.is_empty()
    }

    /// `range` of `module`'s current text as the loop knows it, or `None`
    /// where its first or last character is newer than the loop.
    pub fn to_loop(&self, module: ModuleSource, range: kg_base::TextRange) -> Option<kg_base::TextRange> {
        match self.maps.get(&module) {
            Some(map) => map.to_old(range),
            None => Some(range),
        }
    }

    /// [`EditorLoop::to_loop`] backwards: a range the loop holds, in the
    /// current text.
    pub fn to_text(&self, module: ModuleSource, range: kg_base::TextRange) -> Option<kg_base::TextRange> {
        match self.maps.get(&module) {
            Some(map) => map.to_new(range),
            None => Some(range),
        }
    }

    /// [`Looped::answer_at`] at a range of the current text.
    pub fn answer_at(
        &self,
        module: ModuleSource,
        range: kg_base::TextRange,
        kind: kg_syntax::SyntaxKind,
        interner: &kg_base::Interner,
        shown: &dyn Fn(&kg_ty::Ty) -> bool,
    ) -> Option<IrAnswer> {
        self.looped.answer_at(module, self.to_loop(module, range)?, kind, interner, shown)
    }

    /// [`Looped::enum_value_at`] at a range of the current text.
    pub fn enum_value_at(&self, module: ModuleSource, range: kg_base::TextRange, kind: kg_syntax::SyntaxKind) -> Option<kg_const::ConstInt> {
        self.looped.enum_value_at(module, self.to_loop(module, range)?, kind)
    }

    /// [`Looped::callee_at`] at a range of the current text.
    pub fn callee_at(&self, module: ModuleSource, range: kg_base::TextRange, interner: &kg_base::Interner) -> Option<IrAnswer> {
        self.looped.callee_at(module, self.to_loop(module, range)?, interner)
    }

    /// [`Looped::written_at`] at a range of the current text.
    pub fn written_at(&self, module: ModuleSource, range: kg_base::TextRange, kind: kg_syntax::SyntaxKind) -> Option<kg_ty::Ty> {
        self.looped.written_at(module, self.to_loop(module, range)?, kind)
    }

    /// [`Looped::members`], each written where the current text has it —
    /// `None`, as for a type that names no aggregate, where an edit has
    /// touched one of them: a list with a member missing would say the
    /// aggregate has fewer than it does, and a member an edit wrote is in
    /// no loop.
    pub fn members(&self, ty: &kg_ty::Ty, interner: &kg_base::Interner) -> Option<Vec<Member>> {
        self.looped
            .members(ty, interner)?
            .into_iter()
            .map(|m| Some(Member { range: self.to_text(m.module, m.range)?, ..m }))
            .collect()
    }

    /// [`Looped::instance_name`].
    pub fn instance_name(&self, ty: &kg_ty::Ty, interner: &kg_base::Interner) -> Option<String> {
        self.looped.instance_name(ty, interner)
    }

    /// [`Looped::type_declaration`], where the current text has it.
    pub fn type_declaration(
        &self,
        ty: &kg_ty::Ty,
        interner: &kg_base::Interner,
    ) -> Option<(ModuleSource, kg_base::TextRange, kg_syntax::SyntaxKind)> {
        let (module, range, kind) = self.looped.type_declaration(ty, interner)?;
        Some((module, self.to_text(module, range)?, kind))
    }
}

/// One instantiated copy's type at a node of the template's text.
#[derive(Clone, Debug, PartialEq)]
struct Copied {
    /// The instance as a message spells it (`take<uint8>`); empty for a
    /// copy no declaration names.
    instance: String,
    ty: kg_ty::Ty,
}

/// One member of an aggregate in the loop's program (`Looped::members`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: kg_base::Symbol,
    /// The text the member was written at — for a template's instance, the
    /// template's, which a copy keeps (`Src::expanded`): the module, and the
    /// range and kind of the CST declaration it was lowered from.
    pub module: ModuleSource,
    pub range: kg_base::TextRange,
    pub kind: kg_syntax::SyntaxKind,
    /// The loop's type there — the instance's own, not the template's.
    pub ty: kg_ty::Ty,
}

/// What the program-wide loop says about one node of a module's text, as
/// the editor shows it (`Looped::answer_at`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrAnswer {
    /// The written tree's own type: the node is typed where it is written.
    Written(kg_ty::Ty),
    /// The written node has no type to show — it is inside a template's
    /// body, which `inferType` visits for its condition only
    /// (`Inference.hs:63`) — and the instances' copies have: each type
    /// once, with the instances that give it, in the order the loop
    /// decided them. A body's widths depend on its arguments, so one copy's
    /// type is not the body's, and there is no single answer to give.
    Copies {
        types: Vec<(kg_ty::Ty, Vec<String>)>,
        /// Instances whose copy holds no type to show there.
        untyped: Vec<String>,
        /// The loop had not reached its fixpoint — the editor's first
        /// answer, while the fixpoint runs on (`PassBudget::Layered`): an
        /// instance it did not make yet is in neither list.
        budget_exhausted: bool,
    },
}

impl IrAnswer {
    /// This answer to a reader that prints a type where `shown` says it
    /// can: a copy's type it cannot print joins the untyped, and an answer
    /// left with nothing to print is no answer.
    pub fn shown(self, shown: &dyn Fn(&kg_ty::Ty) -> bool) -> Option<IrAnswer> {
        match self {
            IrAnswer::Written(ty) => shown(&ty).then_some(IrAnswer::Written(ty)),
            IrAnswer::Copies { types, mut untyped, budget_exhausted } => {
                let mut kept = Vec::new();
                for (ty, instances) in types {
                    if shown(&ty) {
                        kept.push((ty, instances));
                    } else {
                        untyped.extend(instances);
                    }
                }
                (!kept.is_empty()).then_some(IrAnswer::Copies { types: kept, untyped, budget_exhausted })
            }
        }
    }

    fn of_copies(copies: Vec<Copied>, shown: &dyn Fn(&kg_ty::Ty) -> bool, budget_exhausted: bool) -> Option<IrAnswer> {
        let mut types: Vec<(kg_ty::Ty, Vec<String>)> = Vec::new();
        let mut untyped = Vec::new();
        for c in copies {
            if !shown(&c.ty) {
                untyped.push(c.instance);
                continue;
            }
            match types.iter_mut().find(|(t, _)| *t == c.ty) {
                Some((_, instances)) => instances.push(c.instance),
                None => types.push((c.ty, vec![c.instance])),
            }
        }
        (!types.is_empty()).then_some(IrAnswer::Copies { types, untyped, budget_exhausted })
    }
}

/// What [`infer_identity`] found for one program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Where the runs part, if they do.
    pub differ: Option<String>,
    /// Whether the editor's first answer was taken: the loop ran past its
    /// bound, and the answer there was graded too.
    pub first_answer: bool,
}

/// The loop run over `roots`' program with inference walked whole every
/// iteration, and incrementally, and the first line where the two
/// [`kg_sem::elaborate::fingerprint`]s differ, if one does
/// (`docs/plans/performance.md` §3.6).
///
/// The incremental run is the editor's ([`PassBudget::EDITOR`], sprint
/// W7.6): layered, its first answer forked from its own state at the
/// bound — so its last answer graded against the whole walk is also the
/// proof that taking the first leaves the run as it was, and the first is
/// graded against the loop the bound alone runs, `kanagawa
/// --template-iterations=4`'s own, which is what the editor's whole answer
/// was before (`Stopped::answer`). It is recorded as the editor's is, its
/// state kept per module and round (`crate::snapshot`, sprint W7.5d): the
/// recording is graded as changing nothing.
///
/// [`PassBudget::EDITOR`]: crate::PassBudget::EDITOR
pub fn infer_identity(db: &Db, roots: &[FileId]) -> Identity {
    let sorted = crate::sorted::sorted_program(db, roots);
    let modules: Vec<ModuleSource> = sorted.files.iter().copied().filter(|m| sorted.ir(*m).is_some()).collect();
    let irs = || -> Vec<kg_hir::ModuleIr> { modules.iter().map(|m| sorted.ir(*m).expect("filtered").fork()).collect() };
    let (interner, never) = (db.interner(), kg_base::cancel::Cancel::never());
    let print = |out: &kg_sem::elaborate::Elaborated| kg_sem::elaborate::fingerprint(out, interner);
    let rounds = db.pass_budget().rounds();
    let full = print(&kg_sem::elaborate::elaborate_opts(irs(), interner, rounds, true, &never));
    let at = crate::PassBudget::EDITOR.first_answer().expect("the editor's budget is layered");
    let mut first = None;
    let names = modules.iter().map(|m| format!("{m:?}")).collect();
    let record = kg_sem::snapshot::Recorder::new(names);
    let mut take_first = |stopped: kg_sem::elaborate::Stopped<'_>| first = Some(print(&stopped.answer()));
    let incremental =
        print(&kg_sem::elaborate::elaborate_layered_recorded(irs(), interner, rounds, at, false, &never, &mut take_first, record));
    let first_answer = first.is_some();
    let differ = if full != incremental {
        Some(first_difference(("full", &full), ("incremental", &incremental)))
    } else {
        first.and_then(|first| {
            let bounded = print(&kg_sem::elaborate::elaborate_opts(irs(), interner, at, false, &never));
            (first != bounded).then(|| first_difference((&format!("bounded at {at}"), &bounded), ("first answer", &first)))
        })
    };
    Identity { differ, first_answer }
}

/// Where two fingerprints part: the module and line, and each side's line.
fn first_difference((x, a): (&str, &str), (y, b): (&str, &str)) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let at = a.iter().zip(&b).position(|(p, q)| p != q).unwrap_or(a.len().min(b.len()));
    let module = a[..at].iter().rev().find(|l| l.starts_with("== module")).copied().unwrap_or("");
    format!(
        "{module}, line {at}: {x} `{}` / {y} `{}`",
        a.get(at).map_or("<end>", |l| l.trim()),
        b.get(at).map_or("<end>", |l| l.trim())
    )
}

/// The loop's [`kg_sem::elaborate::fingerprint`] over `roots`' program,
/// as `kg check` runs it — for comparing two builds of the compiler.
pub fn fingerprint(db: &Db, roots: &[FileId]) -> String {
    let sorted = crate::sorted::sorted_program(db, roots);
    let modules: Vec<ModuleSource> = sorted.files.iter().copied().filter(|m| sorted.ir(*m).is_some()).collect();
    let irs: Vec<kg_hir::ModuleIr> = modules.iter().map(|m| sorted.ir(*m).expect("filtered").fork()).collect();
    let full = db.settings().verdicts.infer_full;
    let out = kg_sem::elaborate::elaborate_opts(irs, db.interner(), db.pass_budget().rounds(), full, &kg_base::cancel::Cancel::never());
    kg_sem::elaborate::fingerprint(&out, db.interner())
}

/// What the loop runs over: the modules the sorted lowering lowered, in
/// program order, the parse each was lowered from, and a fork of each one's
/// IR for the loop to rewrite.
pub(crate) struct Lowered {
    pub(crate) inputs: Inputs,
    pub(crate) irs: Vec<kg_hir::ModuleIr>,
}

/// What [`finish`] reads of the lowering besides the elaboration: the
/// modules in program order, the parse each was lowered from, and its
/// sorted tree — what the reference's parser built, which its checks ask
/// ([`Looped::parsed`]).
#[derive(Clone)]
pub(crate) struct Inputs {
    pub(crate) modules: Vec<ModuleSource>,
    pub(crate) parses: Vec<Arc<kg_syntax::Parse>>,
    pub(crate) sorted: Vec<Arc<kg_hir::ModuleIr>>,
}

pub(crate) fn lowered(db: &Db, roots: &[FileId]) -> Lowered {
    let sorted = crate::sorted::sorted_program(db, roots);
    let modules: Vec<ModuleSource> = sorted.files.iter().copied().filter(|m| sorted.ir(*m).is_some()).collect();
    let irs: Vec<kg_hir::ModuleIr> = modules.iter().map(|m| sorted.ir(*m).expect("filtered").fork()).collect();
    let parses = modules.iter().map(|m| sorted.parses[m].clone()).collect();
    let sorted = modules.iter().map(|&m| sorted.ir(m).expect("filtered").clone()).collect();
    Lowered { inputs: Inputs { modules, parses, sorted }, irs }
}

/// The loop run on this thread, to the budget's bound — a batch compile's
/// fixpoint, or `--template-iterations`; the editor's layered budget runs
/// its loop apart (`crate::layered`).
pub(crate) fn looped(db: &Db, roots: &[FileId]) -> Looped {
    let Lowered { inputs, irs } = lowered(db, roots);
    let out = {
        let _t = db.stats.phase(crate::stats::Phase::Loop);
        let cancel = db.cancel_token();
        // Inside the token's scope, so a walk with no token of its own —
        // `desugar`'s — stops too (`kg_base::cancel::checkpoint_strided`).
        kg_base::cancel::scope(&cancel, || {
            let full = db.settings().verdicts.infer_full;
            kg_sem::elaborate::elaborate_opts(irs, db.interner(), db.pass_budget().rounds(), full, &cancel)
        })
    };
    finish(db, inputs, Arc::new(out), false, true)
}

/// The elaboration `out` of `inputs`' modules, each lowered from its parse
/// to its sorted tree, as the database holds it: each module's table, and
/// what the passes over the copies say, placed at the text.
///
/// `truncated` where `out` is a first answer (`PassBudget::Layered`): a
/// loop stopped short of its fixpoint for latency, whose leftovers no
/// verdict may judge (`kg_sem::check::checked`). `dump` where `KG_SEM_DUMP`
/// is to see these trees.
pub(crate) fn finish(
    db: &Db,
    inputs: Inputs,
    out: Arc<kg_sem::elaborate::Elaborated>,
    truncated: bool,
    dump: bool,
) -> Looped {
    let Inputs { modules, parses, sorted } = inputs;
    // `KG_SEM_DUMP=<path>:<offset>` (`crate::sem`) over these trees.
    if dump && std::env::var_os("KG_SEM_DUMP").is_some() {
        for (&module, m) in modules.iter().zip(&out.modules) {
            let nodes: Vec<(kg_hir::ExpId, String)> = crate::sem::reachable(&m.ir)
                .into_iter()
                .map(|id| (id, crate::sem::constructor(&m.ir.arena[id].exp)))
                .collect();
            crate::sem::dump_on_request(db, module, m, &nodes, db.interner());
        }
    }
    let mut diagnostics: FxHashMap<ModuleSource, Vec<(kg_sem::check::Stage, kg_diag::Diagnostic)>> = FxHashMap::default();
    let copies = {
        let _t = db.stats.phase(crate::stats::Phase::CopiesCheck);
        let bracket = |at: kg_base::FileRange| {
            let module = modules.iter().copied().find(|m| m.diagnostic_file() == at.file)?;
            dimension_bracket(db, module, at)
        };
        kg_sem::check::checked(&out, db.interner(), db.settings().verdicts.leftovers, truncated, &bracket)
    };
    // A name a rewrite made at the text is pass 9's verdict where the text
    // spells it there, and the flag's alone where it does not
    // (`kg_sem::check::CopiedName`).
    let mut erred = copies.erred;
    let mut spelled = Vec::new();
    for c in copies.copied {
        // Every synthetic module shares one file id, so each is asked.
        let spells = |m: &&ModuleSource| {
            let text = db.parse_module(**m).text();
            text.get(usize::from(c.name.range.start())..usize::from(c.name.range.end())) == Some(c.spelling.as_str())
        };
        if modules.iter().filter(|m| m.diagnostic_file() == c.name.file).any(|m| spells(&m)) {
            spelled.push(kg_sem::check::TextVerdict {
                pass: c.pass,
                home: c.diagnostic.range.file,
                diagnostic: c.diagnostic,
                undecided: false,
            });
        } else if c.pass == kg_sem::check::TextPass::UndefinedSymbols {
            erred = true;
        }
    }
    for (stage, mut d) in copies.diagnostics {
        let Some(&home) = modules.iter().find(|m| m.diagnostic_file() == d.range.file) else { continue };
        d.range = capture_list_note(db, home, d.range);
        diagnostics.entry(home).or_default().push((stage, d));
    }
    let mut text: FxHashMap<ModuleSource, Vec<kg_sem::check::TextVerdict>> = FxHashMap::default();
    for mut verdict in copies.text.into_iter().chain(spelled) {
        let Some(&home) = modules.iter().find(|m| m.diagnostic_file() == verdict.home) else { continue };
        let d = &mut verdict.diagnostic;
        match verdict.pass {
            // An error the fold made of a parenthesized expression is
            // noted from the `(` (`Parser.hs:836,845`), as pass 10's is.
            kg_sem::check::TextPass::InferType | kg_sem::check::TextPass::TypeErrors => {
                d.range = paren_note(db, home, d.range);
            }
            kg_sem::check::TextPass::ExportableClasses => {
                let at = |r: kg_base::FileRange| modules.iter().find(|m| m.diagnostic_file() == r.file).map_or(r, |&m| token_note(db, m, r));
                d.range = at(d.range);
                for label in &mut d.labels {
                    label.range = at(label.range);
                }
            }
            // A statement's note starts at its first token (`static if`,
            // `static assert`), as the CST pass reported it; a function's
            // too, past its documentation and from its attributes (`[[async]]
            // void Pipe(() -> void cb)` at the `[[`, `kg-oracle --errors`).
            kg_sem::check::TextPass::PostDesugar | kg_sem::check::TextPass::HigherOrderFunctions => {
                d.range = token_note(db, home, d.range);
            }
            // A declaration's note — a global, a function, a variable of a
            // function type — starts at its first token, as do the notes
            // of an overridden designator.
            kg_sem::check::TextPass::ValidateProgram => {
                d.range = token_note(db, home, d.range);
                for label in &mut d.labels {
                    label.range = token_note(db, home, label.range);
                }
            }
            // The template's declaration is noted from its first token, past
            // its documentation (`println`'s at `inline`, `basics.k::3`,
            // `kg-oracle --errors`), wherever it is declared.
            // An instance `desugarLambdas` minted wears the capture list's
            // note, past its `[` (`lambdas.k::13`'s `Closure<…>`).
            kg_sem::check::TextPass::UnresolvedTemplates => {
                d.range = capture_list_note(db, home, past_template_keyword(db, home, d.range));
                for label in &mut d.labels {
                    if let Some(&m) = modules.iter().find(|m| m.diagnostic_file() == label.range.file) {
                        label.range = token_note(db, m, label.range);
                    }
                }
            }
            // A verdict at a node `desugarLambdas` minted wears the capture
            // list's note, past its `[`, as the copies' do.
            kg_sem::check::TextPass::TemplateTypeCheck => d.range = capture_list_note(db, home, d.range),
            kg_sem::check::TextPass::UndefinedSymbols
            | kg_sem::check::TextPass::ValidateExtern
            | kg_sem::check::TextPass::DeduceAuto
            | kg_sem::check::TextPass::InterpolatedStringPost
            | kg_sem::check::TextPass::CheckConversion => {}
            // The note is at the declaration shadowed, from its first token
            // (the IR wears its leading comment).
            kg_sem::check::TextPass::CheckShadowDecl => {
                for label in &mut d.labels {
                    if let Some(&m) = modules.iter().find(|m| m.diagnostic_file() == label.range.file) {
                        label.range = token_note(db, m, label.range);
                    }
                }
            }
        }
        text.entry(home).or_default().push(verdict);
    }
    let indexes = modules.iter().map(|_| std::sync::OnceLock::new()).collect();
    let copies = modules.iter().map(|_| std::sync::OnceLock::new()).collect();
    let parsed = modules.iter().map(|_| std::sync::OnceLock::new()).collect();
    Looped {
        modules,
        out,
        diagnostics,
        text,
        erred,
        indexes,
        copies,
        instances: std::sync::OnceLock::new(),
        symbols: std::sync::OnceLock::new(),
        parses,
        sorted,
        parsed,
        maps: std::sync::Mutex::default(),
    }
}

/// Where the reference notes a written array dimension, given the
/// dimension's range: the `[` of an unattributed array's bracket group, a
/// memory's expression itself (`TypeCheck.hs:189`;
/// `validate_program::dimension_note`, which the text's own check reports
/// at too, [`dimension_note`]). `None` where `at` is no dimension of an array type in
/// `module`'s text.
fn dimension_bracket(db: &Db, module: ModuleSource, at: kg_base::FileRange) -> Option<kg_base::FileRange> {
    use kg_syntax::AstNode;
    let parse = db.parse_module(module);
    let root = parse.source_file();
    // The IR has no node for parentheses and wears the inner expression's
    // range: `[(1 - 1)]`'s dimension is the parenthesised one, reported at
    // its `(` (`kg-oracle --errors`, 2026-10-10).
    let outward = |mut n: kg_syntax::SyntaxNode| {
        while let Some(p) = n.parent().filter(|p| p.kind() == kg_syntax::SyntaxKind::PAREN_EXPR) {
            n = p;
        }
        n
    };
    let dim = root
        .syntax()
        .descendants()
        .filter(|n| {
            kg_syntax::ast::Expr::can_cast(n.kind()) && (n.text_range() == at.range || kg_syntax::trimmed_range(n) == at.range)
        })
        .map(outward)
        .find(|n| n.parent().is_some_and(|p| p.kind() == kg_syntax::SyntaxKind::ARRAY_TYPE))?;
    let array = kg_syntax::ast::ArrayType::cast(dim.parent()?)?;
    if !array.exprs().any(|e| e.syntax() == &dim) {
        return None;
    }
    Some(kg_base::FileRange { range: dimension_note(&dim), ..at })
}

/// Where the reference's note on the array dimension `dim` begins, which
/// is where `arraySizeError` reports it (`TypeCheck.hs:189`).
///
/// An unattributed array's dimensions are `many (brackets expression)`
/// (`Parser.hs:535`), and that `many` notes each element itself
/// (`notedWithDocs`, `:114`), from its `[` — for a literal exactly as for
/// a name or a fold: `kg-oracle --errors` on `uint32[0] x;`, `uint2[0]
/// x;`, `int32[3.14] a;` and `uint32[n] y;` prints the `[` every time
/// (2026-09-08). A memory's one dimension is `Seq . pure <$> brackets
/// expression` (`:529`), no `many`, so it keeps the expression's own note,
/// past the `[` and any space: `static [[memory]] uint32[0] _mem;` is
/// reported at the `0` (`basics.k::173`), `[ 0]` at the `0` and `[(0)]` at
/// the `(` (2026-10-10). The memory form is the array type that carries
/// attributes (`kg_parser`'s `type1`).
fn dimension_note(dim: &kg_syntax::SyntaxNode) -> kg_base::TextRange {
    use kg_syntax::AstNode;
    let memory = dim.parent().and_then(kg_syntax::ast::ArrayType::cast).is_some_and(|a| a.attr_lists().next().is_some());
    if memory {
        return kg_syntax::trimmed_range(dim);
    }
    let mut prev = dim.prev_sibling_or_token();
    while let Some(e) = prev {
        if e.kind() == kg_syntax::SyntaxKind::L_BRACK {
            return e.text_range();
        }
        prev = e.prev_sibling_or_token();
    }
    dim.text_range()
}

/// `term = noted $ choice [… hidden (parens expr) …]` (`Parser.hs:836,845`)
/// notes the inner expression from the `(`, so the reference reports an
/// error on `(a << b)` at the `(` (`basics.k::62`, `kg-oracle --errors`). The IR has no
/// node for the parentheses either, and wears the inner expression's
/// range: pass 10's verdict moves out over every pair that encloses it.
fn paren_note(db: &Db, module: ModuleSource, at: kg_base::FileRange) -> kg_base::FileRange {
    use kg_syntax::AstNode;
    let parse = db.parse_module(module);
    let root = parse.source_file();
    let mut node = match root.syntax().covering_element(at.range) {
        kg_syntax::NodeOrToken::Node(n) => n,
        kg_syntax::NodeOrToken::Token(t) => match t.parent() {
            Some(n) => n,
            None => return at,
        },
    };
    while let Some(p) = node.parent().filter(|p| p.text_range() == at.range) {
        node = p;
    }
    while let Some(p) = node.parent().filter(|p| p.kind() == kg_syntax::SyntaxKind::PAREN_EXPR) {
        node = p;
    }
    kg_base::FileRange { range: node.text_range(), ..at }
}

/// An instance written `template Go<bool>` is noted at its name: the
/// keyword is no part of the instance (`dependent-template.k::8`, "Undefined
/// template `Go`" at the `G`, `kg-oracle --errors`), where the lowering's
/// range takes the path segment whole.
fn past_template_keyword(db: &Db, module: ModuleSource, at: kg_base::FileRange) -> kg_base::FileRange {
    use kg_syntax::AstNode;
    let parse = db.parse_module(module);
    let root = parse.source_file();
    if !root.syntax().text_range().contains_range(at.range) {
        return at;
    }
    let past_trivia = |mut t: Option<kg_syntax::SyntaxToken>| {
        while let Some(x) = t.as_ref().filter(|x| x.kind().is_trivia()) {
            t = x.next_token();
        }
        t
    };
    let first = past_trivia(root.syntax().token_at_offset(at.range.start()).right_biased());
    let Some(first) = first.filter(|t| t.kind() == kg_syntax::SyntaxKind::TEMPLATE_KW) else { return at };
    let next = past_trivia(first.next_token());
    match next {
        Some(t) if t.text_range().start() < at.range.end() => {
            kg_base::FileRange { range: kg_base::TextRange::new(t.text_range().start(), at.range.end()), ..at }
        }
        _ => at,
    }
}

/// A declaration's note starts at its first token: the lexer drops
/// comments, so `// a comment` above `inline void Go()` is in no note
/// (`basics.k::177`'s member, `kg-oracle --errors`: at `inline`). A node's
/// range here takes in the trivia the parser attached before it, and an IR
/// node wears that range: pass 13's verdicts and their notes, at a member
/// and at an `export`, are the node's tokens' range instead, as the CST
/// pass reported them.
fn token_note(db: &Db, module: ModuleSource, at: kg_base::FileRange) -> kg_base::FileRange {
    use kg_syntax::AstNode;
    let parse = db.parse_module(module);
    let root = parse.source_file();
    // Every synthetic module shares one file id (`FileId::SYNTHETIC`), so
    // `module` may not be the one whose text `at` is a range of.
    if !root.syntax().text_range().contains_range(at.range) {
        return at;
    }
    let node = match root.syntax().covering_element(at.range) {
        kg_syntax::NodeOrToken::Node(n) => n,
        kg_syntax::NodeOrToken::Token(_) => return at,
    };
    if node.text_range() != at.range {
        return at;
    }
    kg_base::FileRange { range: kg_syntax::trimmed_range(&node), ..at }
}

/// A lambda's capture list is noted from its first token after the `[` —
/// the first capture, or the `]` of an empty list — and every node
/// `desugarLambdas` mints wears that note (`new = flip newNote
/// lambdaCaptures`, `Function.hs:206-214`): `Closure<decltype(fn), fn, C>`
/// is rejected at column 18 for `[]` and 21 for `[   ]` (`lambdas.k::6`).
/// The lowering's head is the `CAPTURE_LIST` node itself; the shift is made
/// here, where the text is, as the twin's carry made it.
fn capture_list_note(db: &Db, module: ModuleSource, at: kg_base::FileRange) -> kg_base::FileRange {
    use kg_syntax::AstNode;
    let parse = db.parse_module(module);
    let root = parse.source_file();
    let Some(list) = root
        .syntax()
        .descendants()
        .find(|n| n.kind() == kg_syntax::SyntaxKind::CAPTURE_LIST && n.text_range() == at.range)
        .and_then(kg_syntax::ast::CaptureList::cast)
    else {
        return at;
    };
    let mut token = list.l_brack_token().and_then(|t| t.next_token());
    while let Some(t) = &token {
        if !t.kind().is_trivia() {
            break;
        }
        token = t.next_token();
    }
    token.map_or(at, |t| kg_base::FileRange { range: t.text_range(), ..at })
}

#[cfg(test)]
mod tests {
    /// `resolve` renames every `TemplateInstance` whose arguments are a key
    /// the round decided, in any module and inside any body
    /// (`Template.hs:158-159`): `rc<bool>` decided at `main.k`'s `other`
    /// renames the mention in `h`'s *template* `dec` that same round, so
    /// the copy `dec<1>` is made with it resolved and both deductions,
    /// `<uint3>` from the copy and `<uint14>` from `other`, fall in round
    /// 2, ordered by their arguments — `kg-oracle --passes 8`: `rc<|$bool|>
    /// <|$uint@3|>` declared before `<|$uint@14|>`, and the template's own
    /// body reading `rc<|$bool|>(…)`. Red with the round's decisions not
    /// seeded across modules (`<uint3>` a round late, after `<uint14>`).
    #[test]
    fn a_decision_renames_its_key_in_every_module() {
        use kg_base::Utf8PathBuf;
        let h = "module h\n    { rc\n    , dec\n    }\n\ntemplate <typename T>\ninline T rc(auto x)\n{\n    return cast<T>(x);\n}\n\ntemplate <auto N>\ninline uint8 dec(uint8 i)\n{\n    const auto b = rc<bool>(cast<uint3>(i));\n    return i;\n}\n";
        let main = "import h\n\ninline uint8 use()\n{\n    return dec<1>(0);\n}\n\ninline bool other(uint8 i)\n{\n    return rc<bool>(cast<uint14>(i));\n}\n";
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.implicit_base = false;
        let mut db = crate::Db::new(settings);
        db.set_file_text(&Utf8PathBuf::from("/proj/h.k"), h.to_owned());
        let root = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), main.to_owned());
        let looped = db.looped(&[root]);
        let names: Vec<String> = looped
            .out
            .modules
            .iter()
            .flat_map(|m| {
                m.ir.top_level_items()
                    .into_iter()
                    .filter_map(|id| kg_sem::symbols::declared_name(&m.ir.arena, id))
                    .map(|(_, n)| db.interner().resolve(n).to_string())
                    .collect::<Vec<_>>()
            })
            .filter(|n| n.starts_with("rc<|$bool|><"))
            .collect();
        assert_eq!(names, ["rc<|$bool|><|$uint@3|>", "rc<|$bool|><|$uint@14|>"], "{:?}", looped.out.report);
    }

    /// Each `while compiling` frame names the instance it compiles, as a
    /// message spells one (`kg_sem::instance`): the member template's
    /// copy inside `Box<4>`'s is `Box<4>::get<3>`, the class's `Box<4>`;
    /// the frame's text and the note's own message stay the reference's.
    /// Red with the frames labelled bare, or named from the innermost
    /// expansion alone.
    #[test]
    fn a_frame_names_the_instance_it_compiles() {
        let text = "template <auto N>\nclass Box\n{\n    template <auto I>\n    inline void get()\n    {\n        bool[I - 3] x;\n    }\n\n    inline void run()\n    {\n        get<3>();\n    }\n}\n\n\
                    inline void main()\n{\n    static Box<4> b;\n    b.run();\n}\n";
        let mut settings = kg_config::Settings::fallback("/proj");
        settings.implicit_base = false;
        let mut db = crate::Db::new(settings);
        let root = db.set_file_text(&kg_base::Utf8PathBuf::from("/proj/main.k"), text.to_owned());
        let out = db.check(&[root]);
        let d = out.iter().find(|d| d.message == "Array dimension must be a positive integer").expect("the copy's error");
        let said: Vec<String> = d.labels.iter().map(kg_diag::Label::said).collect();
        assert_eq!(said, ["while compiling `Box<4>::get<3>`", "while compiling `Box<4>`"], "{:?}", d.labels);
        assert!(d.labels.iter().all(|l| l.message == "while compiling"));
    }

    /// A copy's dimension error sits at the `[` of the template's text, as
    /// the reference's `brackets expression` notes it, framed by both
    /// instantiations (`template.k::30`; `kg-oracle --errors` 2026-09-27:
    /// 8:5, then 11:1 and 16:1). Red with [`super::dimension_bracket`]
    /// answering `None`.
    #[test]
    fn a_copys_dimension_is_reported_at_its_bracket() {
        let text = "template <typename T>\ninline void Go(T x)\n{\n    template <auto I>\n    inline void Do()\n    {\n        bool[I - 3] x;\n    }\n\n    Do<3>();\n}\n\ninline void main()\n{\n    Go(10);\n}\n";
        let out = crate::tests::program(&[("main.k", text)]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "Array dimension must be a positive integer");
        assert_eq!(out[0].1, "main.k:[");
        assert_eq!(out[0].2, ["main.k:Do<3>: while compiling", "main.k:Go: while compiling"]);
    }

    /// A memory's dimension in a copy is reported at its expression, as in
    /// the text (`validate_program::dimension_note`): `kg-oracle --errors`
    /// on `C<3>` over `[[memory]] uint32[N - 3] m;` prints the `N`, and the
    /// unattributed array beside it its `[` (2026-10-10).
    #[test]
    fn a_copys_memory_dimension_is_reported_at_its_expression() {
        let text = "template <auto N>\nclass C\n{\n    [[memory]] uint32[N - 3] m;\n    uint32[N - 3] p;\n}\n\n\
                    inline void main()\n{\n    static C<3> c;\n}\n";
        let out = crate::tests::program(&[("main.k", text)]);
        let at: Vec<&str> = out.iter().map(|(_, at, _)| at.as_str()).collect();
        assert_eq!(at, ["main.k:N - 3", "main.k:["], "{out:?}");
    }
}
