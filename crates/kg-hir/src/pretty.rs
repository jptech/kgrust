//! IR → parseable Kanagawa source.
//!
//! This is **not** a formatter — `kg-fmt` owns layout. It exists for one gate:
//! `lower(parse(pretty(ir)))` must be structurally equal to `ir`
//! (`cargo xtask ir`, gate 2), which is what catches a lowering that silently
//! drops a field. So the printer optimises for *reparse fidelity*, not looks:
//!
//! * Every `Binary`/`Unary` operand is parenthesised, so no precedence table
//!   exists here to disagree with the parser's — and a parenthesised
//!   sub-expression is also what re-enables `<`/`>`/`<<`/`>>` inside a
//!   template argument list (`Parser.hs:910-912`).
//! * Desugared forms print in their *lowered* spelling — a ternary as
//!   `mux(c, e, t)`, `x++` as `x = x + 1;` — because both lower to the same
//!   IR, so the round trip is a fixpoint even though the surface text is not.
//! * Synthesized names (`?lambdaN`, `$quasiN`) are never printed; the reparse
//!   re-mints them, and the scope walk being deterministic is what makes that
//!   an identity.
//! * A **sorted** name ([`pretty_with_source`]) prints the *file text at its
//!   own head* rather than a reconstruction. The sorter turns a written
//!   `A::b` into a prefix chain that interleaves written segments with
//!   materialised qualifier segments (`setQualifier`, `Desugar.hs:193-201`),
//!   and no per-node field separates the two: `written == Own` does not —
//!   divergence #24's demotion donates a *written* path's inner links too, so
//!   an `Own`-guarded prefix printer suppresses every written prefix — and
//!   lexability of the segment text does not either, since a sanitised `$`
//!   name lexes while `stage0.__kg_m2::execute(…)` still cannot parse in
//!   member position. Both were measured, not argued (`docs/journal.md`,
//!   "Print and reparse works"). What *is* exact: the outermost sorted node is
//!   `Own` at the CST slot the whole written path occupied (`Parser.hs:
//!   652-695`; `finish_sorted`), so the file text at its head range **is** the
//!   written spelling — qualifiers included where the user wrote them, absent
//!   where syntax cannot carry them (a method callee's donor is the `NAME_REF`
//!   itself, `Parser.hs:730-734`, so its slice is the bare member name).

use kg_base::{Expansion, Interner, Symbol, Written};
use kg_const::{BinOp, ConstInt};
use kg_resolve::DeclareFlag;
use kg_ty::Flag;

use crate::arena::ExpId;
use crate::exp::{Exp, PathSeg, UnsortedPath};
use crate::ModuleIr;

pub fn pretty(ir: &ModuleIr, interner: &Interner) -> String {
    print(ir, interner, None, &[])
}

/// [`pretty`] over the text `ir` was lowered from, so sorted names round-trip:
/// `lower_sorted(parse(pretty_with_source(ir, text)))` is structurally equal
/// to `ir` where plain `pretty` is not (`xtask ir`'s `sorted trip:` line).
/// With a `source` that is not `ir`'s own file text the slices are garbage.
pub fn pretty_with_source(ir: &ModuleIr, interner: &Interner, source: &str) -> String {
    print(ir, interner, Some(source), &[])
}

/// [`pretty_with_source`] with the module's import spellings: `(namespace,
/// alias)` pairs, one per namespace an aliased import makes visible — the
/// import's own and every module it re-exports. A resolved qualifier names
/// the *declaring* namespace (`findSymbol`'s, `Parser.hs:642-645`), which is
/// not the written alias when the symbol arrived through a re-export:
/// `unit::assert` over `import test.unit as unit` resolves to
/// `@test@internal@assert`, and a copy past `lowered_len` cannot slice its
/// spelling. Without the table the copy printed `assert(tag, cond)`, which
/// the twin read as the base intrinsic of that name
/// (`test/library/data/hash_table.k`, 2026-09-10).
pub fn pretty_with_spellings(
    ir: &ModuleIr,
    interner: &Interner,
    source: &str,
    spellings: &[(Symbol, Symbol)],
) -> String {
    print(ir, interner, Some(source), spellings)
}

fn print(ir: &ModuleIr, interner: &Interner, source: Option<&str>, spellings: &[(Symbol, Symbol)]) -> String {
    let p = Printer { ir, interner, source, spellings, fn_decls: std::cell::OnceCell::new() };
    let mut out = String::new();

    // Root shape per `program` (`Parser.hs:111,189`): a header wraps the
    // declarations, imports follow the header (`Parser.hs:194,243`).
    let root = &ir.arena.node(ir.root).exp;
    let (header, items): (Option<ExpId>, Vec<ExpId>) = match root {
        Exp::Seq(items) => match items.as_slice() {
            [only] if matches!(ir.arena.node(*only).exp, Exp::ModuleDecl { .. }) => {
                (Some(*only), Vec::new())
            }
            _ => (None, items.clone()),
        },
        _ => (None, Vec::new()),
    };

    let body = if let Some(decl) = header {
        let Exp::ModuleDecl { name, exposed, body } = &ir.arena.node(decl).exp else {
            unreachable!("checked above")
        };
        out.push_str("module ");
        out.push_str(&p.module_name(*name));
        out.push_str(" { ");
        let entries: Vec<String> = exposed.iter().map(|e| p.export_entry(*e)).collect();
        out.push_str(&entries.join(", "));
        out.push_str(" }\n");
        match &ir.arena.node(*body).exp {
            Exp::Seq(items) => items.clone(),
            _ => Vec::new(),
        }
    } else {
        items
    };

    for import in &ir.imports {
        out.push_str("import ");
        out.push_str(&import.name.join("."));
        if let Some(alias) = import.alias {
            out.push_str(" as ");
            out.push_str(&interner.resolve(alias));
        }
        out.push('\n');
    }

    for item in body {
        out.push_str(&p.item(item));
        out.push('\n');
    }
    out
}

struct Printer<'a> {
    ir: &'a ModuleIr,
    interner: &'a Interner,
    source: Option<&'a str>,
    /// See [`pretty_with_spellings`].
    spellings: &'a [(Symbol, Symbol)],
    /// The names this IR declares as a function — built on first use, for
    /// [`Printer::names_a_function_decl`].
    fn_decls: std::cell::OnceCell<kg_base::FxHashSet<Symbol>>,
}

impl<'a> Printer<'a> {
    fn exp(&self, id: ExpId) -> &Exp {
        &self.ir.arena.node(id).exp
    }

    /// The file text at `id`'s own head — the written spelling of a sorted
    /// name, and `None` whenever that claim would not hold: without source
    /// text; for a `Donated` node, whose head is some *other* node's slot; or
    /// inside an instantiation frame, where the head names the template
    /// body's pre-substitution text.
    ///
    /// One more refusal, and it is load-bearing: an `Own` descendant whose own
    /// head pokes *outside* this range is written material the slice would
    /// drop. A method callee is the shape that has one — `field_seg_view`
    /// donors the `NAME_REF` alone, so `fixture.Run<-10, 10, 3>(…)`'s callee
    /// is `Own` at `Run` while the written `<…>` list is `Own` at its own
    /// `TEMPLATE_ARG_LIST` beside it. Sliced, the list vanishes and the
    /// reparse resolves a bare function name to an instance over `Seq []`
    /// (`templateInstance`, `Parser.hs:690-695`).
    fn slice(&self, id: ExpId) -> Option<String> {
        let source = self.source?;
        // A rewrite-minted node never slices, however its `Src` reads:
        // `Rewriter::replace` hands a replacement the replaced node's `Src`
        // *verbatim* (`rewrite/mod.rs:101-104`, the reference's `renote`), so
        // a resolved mention wears the written occurrence's `Own` head while
        // its leaf spells the mangled name — sliced, it would print the
        // unresolved spelling and mask the rewrite. `lowered_len` is the
        // exact boundary, and it bounds the whole subtree: a rewrite's node
        // gets a fresh index the moment any descendant changed, so an index
        // below it is the lowering's own tree.
        if id.index() >= self.ir.lowered_len as usize {
            return None;
        }
        // The claim's own precondition: `Own` says this node is the lowering
        // of exactly that `(range, kind)` slot, which is what makes the text
        // there its spelling. The `via`/`file` clauses restate for `Src` what
        // `lowered_len` already enforces — an instantiated copy is appended
        // past the boundary, and a lowering wears its own file throughout —
        // so today they cannot fire (measured, 2026-08-14: disabling any of
        // the three moves neither the corpus count nor a test). They stay
        // because this function's contract is per-node, and must not quietly
        // come to depend on the boundary argument staying true as rewrites
        // evolve.
        let src = &self.ir.arena.node(id).src;
        if src.written != Written::Own
            || src.via != Expansion::ROOT
            || src.head.file != self.ir.file
        {
            return None;
        }
        let range = src.head.range;
        let mut stack = Vec::new();
        let mut kids = Vec::new();
        self.exp(id).children(&mut stack);
        while let Some(d) = stack.pop() {
            let node = self.ir.arena.node(d);
            if node.src.written == Written::Own
                && node.src.via == Expansion::ROOT
                && node.src.head.file == self.ir.file
                && !range.contains_range(node.src.head.range)
            {
                return None;
            }
            node.exp.children(&mut kids);
            stack.append(&mut kids);
        }
        source.get(usize::from(range.start())..usize::from(range.end())).map(str::to_string)
    }

    /// The alias to print for a prefix the sorter resolved to an imported
    /// module's namespace — `@test.unit`, which `import test.unit as unit`
    /// spells `unit`. `None` for anything else, an unaliased import included:
    /// a bare namespace cannot be written as a qualifier.
    fn import_alias(&self, id: ExpId) -> Option<Symbol> {
        let Exp::ScopedName { ident, prefix: None, .. } = self.exp(id) else { return None };
        let Exp::Identifier(sym) = self.exp(*ident) else { return None };
        if let Some((_, alias)) = self.spellings.iter().find(|(ns, _)| ns == sym) {
            return Some(*alias);
        }
        let text = self.interner.resolve(*sym);
        self.ir
            .imports
            .iter()
            .find(|i| i.alias.is_some() && kg_resolve::Namespace::mangle(&i.name) == text)
            .and_then(|i| i.alias)
    }

    /// Whether `id` is the `ScopedName` `resolve` leaves for a decided
    /// instance: its identifier spells a mangled name (`<|…|>`,
    /// `Mangle.hs:29`) or the alias the rename layer gave one.
    /// Whether a `ScopedName`'s leaf names a function this IR declares — a
    /// function copy, whose name qualifies nothing: a parameter mentioned in
    /// its body is `x`, not `__kg_m0::x`, which resolves (`Parser.hs:378`)
    /// but reads as no runtime value to any pass that looks for a bare
    /// name. A type copy's name — this IR's or another twin's — stays a
    /// qualifier: its members are nothing without it.
    fn names_a_function_decl(&self, id: ExpId) -> bool {
        let Exp::ScopedName { ident, .. } = self.exp(id) else { return false };
        let Exp::Identifier(sym) = self.exp(*ident) else { return false };
        let decls = self.fn_decls.get_or_init(|| {
            let mut out = kg_base::FxHashSet::default();
            for (_, node) in self.ir.arena.iter() {
                let name = match &node.exp {
                    Exp::Function { name, .. } | Exp::FunctionDecl { name, .. } => *name,
                    _ => continue,
                };
                if let Exp::Identifier(s) = self.exp(name) {
                    out.insert(*s);
                }
            }
            out
        });
        decls.contains(sym)
    }

    fn is_resolved_instance(&self, id: ExpId) -> bool {
        let Exp::ScopedName { ident, .. } = self.exp(id) else { return false };
        let Exp::Identifier(sym) = self.exp(*ident) else { return false };
        let text = self.interner.resolve(*sym);
        kg_base::mentions_instance(&text)
    }

    /// Whether `id` is a qualifier a twin can spell as written: one bare
    /// `ScopedName` whose identifier lexes as a plain name — a class, not a
    /// materialised scope component (`$`, `?lambdaN`, a namespace). What a
    /// member template's copy is printed under; anything else would print
    /// a qualifier the parser rejects (measured: 137 of 353 twins,
    /// 2026-09-11).
    fn is_bare_class_prefix(&self, id: ExpId) -> bool {
        let Exp::ScopedName { ident, prefix: None, .. } = self.exp(id) else { return false };
        let Exp::Identifier(sym) = self.exp(*ident) else { return false };
        let text = self.interner.resolve(*sym);
        let mut chars = text.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    /// Whether `id` is a qualifier chain the text *wrote*: bare `ScopedName`s
    /// whose own head ranges spell their identifiers — `C` of `C::W`, a class
    /// naming its member alias — as against the chain `makeQualified` minted
    /// for a declaration's scope (`Desugar.hs:188-201`), which wears a
    /// donor's range and spells a namespace no source can. A written
    /// qualifier survives a copy: `deep_copy` keeps every head.
    /// With `next` the range of the segment this one qualifies: a written
    /// `C::W` has `C` *just before* `W`, `::` between
    /// them, where a decl-scope prefix `makeQualified` minted wears the
    /// declaring class's name — which spells itself too, from across the
    /// file (`return Class::__kg_m0::x[0]` printed for a copy's `x[0]`).
    fn written_qualifier_before(&self, id: ExpId, next: Option<kg_base::TextRange>) -> bool {
        let Some(source) = self.source else { return false };
        let Exp::ScopedName { prefix, ident, .. } = self.exp(id) else { return false };
        if !self.is_bare_class_prefix(id) && prefix.is_none() {
            return false;
        }
        let Exp::Identifier(sym) = self.exp(*ident) else { return false };
        let range = self.ir.arena.node(id).src.head.range;
        let spelled = source.get(usize::from(range.start())..usize::from(range.end()));
        if spelled != Some(self.interner.resolve(*sym).as_ref()) {
            return false;
        }
        if let Some(next) = next {
            if range.end() > next.start() {
                return false;
            }
            let between = source.get(usize::from(range.end())..usize::from(next.start()));
            if between.is_none_or(|b| b.trim() != "::") {
                return false;
            }
        }
        // The qualifier's own prefix need not be written: printed, it
        // follows its own rule — a chain `makeQualified` minted is dropped,
        // a resolved instance spelled. `Bar::Zero` in a class template's
        // copy qualifies `Bar` with the class the parser's `makeQualified`
        // named (`Parser.hs:667-676`), and demanding that chain be written
        // too printed the constant as a bare `Zero`, which names nothing;
        // `Foo<uint32>::Bar::Zero` lost its whole qualifier the same way
        // (`enum.k::51`, the backend's "Unknown symbol: Zero", 2026-09-23).
        true
    }

    fn seq(&self, id: ExpId) -> Vec<ExpId> {
        match self.exp(id) {
            Exp::Seq(items) => items.clone(),
            _ => vec![id],
        }
    }

    fn sym(&self, s: kg_base::Symbol) -> String {
        self.interner.resolve(s).to_string()
    }

    fn name_of(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::Identifier(s) => self.sym(*s),
            _ => self.expr(id),
        }
    }

    fn module_name(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::ModuleIdentifier(parts) => parts.join("."),
            _ => String::new(),
        }
    }

    fn export_entry(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::ModuleIdentifier(parts) => format!("module {}", parts.join(".")),
            Exp::ModuleDiff(lhs, rhs) => {
                format!("module {} \\ {}", lhs.join("."), rhs.join("."))
            }
            Exp::Identifier(s) => self.sym(*s),
            _ => self.expr(id),
        }
    }

    // --- attributes ---------------------------------------------------------

    /// `""` for an empty set, otherwise `[[a, b]] ` with the trailing space.
    fn attrs(&self, id: ExpId) -> String {
        let items = self.seq(id);
        if items.is_empty() {
            return String::new();
        }
        let rendered: Vec<String> = items.iter().map(|a| self.attr(*a)).collect();
        format!("[[{}]] ", rendered.join(", "))
    }

    fn attr(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::FlagAttr(f) => f.text().to_string(),
            Exp::IntAttr(a, e) => format!("{}({})", a.text(), self.expr(*e)),
            Exp::Attr(a, e) => format!("{}({})", a.text(), self.expr(*e)),
            _ => self.expr(id),
        }
    }

    /// The one attribute `Seq` that must *not* print as `[[…]]`: the
    /// desugared `atomic` statement's (`Parser.hs:559-561`), whose flag has no
    /// written spelling.
    fn is_atomic_attr(&self, id: ExpId) -> bool {
        let items = self.seq(id);
        items.len() == 1 && matches!(self.exp(items[0]), Exp::FlagAttr(Flag::Atomic))
    }

    // --- declarations -------------------------------------------------------

    fn item(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::Variable { .. } => self.variable(id),
            Exp::Function { attr, modifier, ret, name, params, body, .. } => {
                let mut out = self.attrs(*attr);
                if let Some(m) = modifier
                    && let Exp::FunctionModifier(m) = self.exp(*m)
                {
                    out.push_str(m.text());
                    out.push(' ');
                }
                out.push_str(&self.ty(*ret));
                out.push(' ');
                out.push_str(&self.name_of(*name));
                out.push('(');
                out.push_str(&self.params(*params));
                out.push_str(") {\n");
                out.push_str(&self.stmts(*body));
                out.push_str("}\n");
                out
            }
            Exp::FunctionDecl { attr, ret, name, params, .. } => {
                format!(
                    "{}{} {}({});",
                    self.attrs(*attr),
                    self.ty(*ret),
                    self.name_of(*name),
                    self.params(*params)
                )
            }
            Exp::Class { name, body, .. } => {
                let mut out = format!("class {} {{\n", self.name_of(*name));
                for member in self.seq(*body) {
                    out.push_str(&self.item(member));
                    out.push('\n');
                }
                out.push_str("}\n");
                out
            }
            Exp::Struct { name, body, .. } => self.aggregate("struct", *name, *body),
            Exp::Union { name, body, .. } => self.aggregate("union", *name, *body),
            Exp::Enum { name, ty, body, .. } => {
                let consts: Vec<String> = self
                    .seq(*body)
                    .iter()
                    .map(|c| match self.exp(*c) {
                        Exp::EnumConstant { name, value, .. } => {
                            let mut s = self.name_of(*name);
                            if let Some(v) = value {
                                s.push_str(" = ");
                                s.push_str(&self.operand(*v));
                            }
                            s
                        }
                        _ => self.expr(*c),
                    })
                    .collect();
                format!(
                    "enum {} : {} {{ {} }}\n",
                    self.name_of(*name),
                    self.ty(*ty),
                    consts.join(", ")
                )
            }
            Exp::Alias { body, name, .. } => {
                format!("using {} = {};", self.name_of(*name), self.ty(*body))
            }
            Exp::Template { params, decl } => {
                let rendered: Vec<String> =
                    params.iter().map(|p| self.template_param(*p)).collect();
                format!("template <{}> {}", rendered.join(", "), self.item(*decl))
            }
            Exp::ExportType { attr, ty } => {
                format!("{}export {};", self.attrs(*attr), self.ty(*ty))
            }
            Exp::Extern { attr, ty } => format!("{}extern {};", self.attrs(*attr), self.ty(*ty)),
            Exp::StaticAssert(e) => format!("static assert({});", self.expr(*e)),
            Exp::StaticAssertLegacy(e) => format!("static_assert({});", self.expr(*e)),
            Exp::Private => "private:".to_string(),
            Exp::Public => "public:".to_string(),
            Exp::DefaultInitialization(e) => format!("default = {};", self.expr(*e)),
            Exp::StaticIf { cond, then_branch, else_branch } => {
                let mut out = format!(
                    "static if ({}) {}",
                    self.expr(*cond),
                    self.static_if_branch_item(*then_branch)
                );
                if let Some(e) = else_branch {
                    out.push_str(" else ");
                    out.push_str(&self.static_if_branch_item(*e));
                }
                out
            }
            Exp::NestedScope { body, .. } => {
                let mut out = String::from("{\n");
                for member in self.seq(*body) {
                    out.push_str(&self.item(member));
                    out.push('\n');
                }
                out.push('}');
                out
            }
            _ => self.stmt(id),
        }
    }

    fn aggregate(&self, kw: &str, name: ExpId, body: ExpId) -> String {
        let mut out = format!("{kw} {} {{\n", self.name_of(name));
        for member in self.seq(body) {
            out.push_str(&self.item(member));
            out.push('\n');
        }
        out.push_str("}\n");
        out
    }

    fn variable(&self, id: ExpId) -> String {
        let Exp::Variable { attr, ty, name, init, flags, .. } = self.exp(id) else {
            return self.stmt(id);
        };
        let mut out = self.attrs(*attr);
        if *flags == DeclareFlag::DeclareStatic {
            out.push_str("static ");
        }
        out.push_str(&self.ty(*ty));
        out.push(' ');
        out.push_str(&self.name_of(*name));
        if let Some(init) = init {
            out.push_str(" = ");
            out.push_str(&self.operand(*init));
        }
        // The `for` head prints its own induction variable without the `;`.
        if *flags == DeclareFlag::DeclareUninitConst { out } else { format!("{out};") }
    }

    fn template_param(&self, id: ExpId) -> String {
        let Exp::TemplateParam { kind, name, default, .. } = self.exp(id) else {
            return self.expr(id);
        };
        let mut out = match self.exp(*kind) {
            Exp::Typename => format!("typename {}", self.sym(*name)),
            // The template-template signature (`Parser.hs:370-371`).
            Exp::Template { params, .. } => {
                let kinds: Vec<String> = params
                    .iter()
                    .map(|k| match self.exp(*k) {
                        Exp::Typename => "typename".to_string(),
                        _ => self.ty(*k),
                    })
                    .collect();
                format!("template <{}> typename {}", kinds.join(", "), self.sym(*name))
            }
            _ => format!("{} {}", self.ty(*kind), self.sym(*name)),
        };
        if let Some(dflt) = default {
            out.push_str(" = ");
            out.push_str(&self.operand(*dflt));
        }
        out
    }

    fn params(&self, id: ExpId) -> String {
        let rendered: Vec<String> = self
            .seq(id)
            .iter()
            .map(|p| match self.exp(*p) {
                Exp::FuncParam { attr, ty, name, .. } => {
                    format!("{}{} {}", self.attrs(*attr), self.ty(*ty), self.name_of(*name))
                }
                _ => self.expr(*p),
            })
            .collect();
        rendered.join(", ")
    }

    fn static_if_branch_item(&self, id: ExpId) -> String {
        self.item(id)
    }

    // --- statements ---------------------------------------------------------

    fn stmts(&self, body: ExpId) -> String {
        let mut out = String::new();
        for s in self.seq(body) {
            out.push_str(&self.stmt(s));
            out.push('\n');
        }
        out
    }

    fn stmt(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::Seq(items) if items.is_empty() => ";".to_string(),
            Exp::Seq(items) => {
                // Never produced in statement position by the lowering; print a
                // block so the result still parses.
                let mut out = String::from("{\n");
                for s in items {
                    out.push_str(&self.stmt(*s));
                    out.push('\n');
                }
                out.push('}');
                out
            }
            Exp::Assign { lvalue, value } => {
                // `a[i]++` is `Assign lval (Binary Add lval 1)` with the
                // *same* lvalue reading in the sum (`Parser.hs:612-614`);
                // printed as `a[i] = a[i] + 1` it would reparse to an rvalue
                // there, so it is printed as written.
                if let Exp::Binary(op @ (BinOp::Add | BinOp::Sub), x, one) = &self.ir.arena[*value].exp
                    && matches!(&self.ir.arena[*one].exp, Exp::IntLiteral { ty: None, value } if *value == ConstInt::ONE)
                    && self.reads_as_lvalue(*x)
                {
                    let suffix = if *op == BinOp::Add { "++" } else { "--" };
                    return format!("{}{suffix};", self.expr(*lvalue));
                }
                format!("{} = {};", self.expr(*lvalue), self.operand(*value))
            }
            Exp::Return(None) => "return;".to_string(),
            Exp::Return(Some(e)) => format!("return {};", self.operand(*e)),
            Exp::If { cond, then_branch, else_branch } => {
                let mut out =
                    format!("if ({}) {}", self.expr(*cond), self.stmt(*then_branch));
                if let Some(e) = else_branch {
                    out.push_str(" else ");
                    out.push_str(&self.stmt(*e));
                }
                out
            }
            Exp::StaticIf { cond, then_branch, else_branch } => {
                let mut out =
                    format!("static if ({}) {}", self.expr(*cond), self.stmt(*then_branch));
                if let Some(e) = else_branch {
                    out.push_str(" else ");
                    out.push_str(&self.stmt(*e));
                }
                out
            }
            Exp::NestedScope { body, .. } => {
                let mut out = String::from("{\n");
                out.push_str(&self.stmts(*body));
                out.push('}');
                out
            }
            Exp::DoWhile { attr, cond, body } => {
                format!(
                    "{}do {} while ({});",
                    self.attrs(*attr),
                    self.stmt(*body),
                    self.expr(*cond)
                )
            }
            Exp::RangeFor { attr, var, limit, body } => {
                format!(
                    "{}for ({} : {}) {}",
                    self.attrs(*attr),
                    self.variable(*var),
                    self.expr(*limit),
                    self.stmt(*body)
                )
            }
            // `PrettyPrint.hs:175-176`: an `UnrolledFor` — the `static for`
            // `staticControl` found no constant use of — prints as one.
            Exp::StaticFor { var, limit, body } | Exp::UnrolledFor { var, limit, body } => {
                format!(
                    "static for ({} : {}) {}",
                    self.variable(*var),
                    self.expr(*limit),
                    self.stmt(*body)
                )
            }
            Exp::Switch { selector, blocks } => {
                let mut out = format!("switch ({}) {{\n", self.expr(*selector));
                for b in self.seq(*blocks) {
                    out.push_str(&self.switch_block(b));
                    out.push('\n');
                }
                out.push('}');
                out
            }
            Exp::Barrier => "barrier;".to_string(),
            Exp::Reorder(s) => format!("reorder {}", self.stmt(*s)),
            Exp::Annotated { attr, stmt } => {
                if self.is_atomic_attr(*attr) {
                    format!("atomic {}", self.stmt(*stmt))
                } else {
                    format!("{}{}", self.attrs(*attr), self.stmt(*stmt))
                }
            }
            // An unrolled `static for`'s induction variable keeps the loop
            // head's flag (`renote inductionVar a`, `Constexpr.hs:98`) and
            // stands as a statement of its iteration's scope.
            Exp::Variable { flags: DeclareFlag::DeclareUninitConst, .. } => format!("{};", self.variable(id)),
            Exp::Variable { .. } => self.variable(id),
            Exp::Function { .. }
            | Exp::FunctionDecl { .. }
            | Exp::Class { .. }
            | Exp::Struct { .. }
            | Exp::Union { .. }
            | Exp::Enum { .. }
            | Exp::Alias { .. }
            | Exp::Template { .. }
            | Exp::ExportType { .. }
            | Exp::Extern { .. }
            | Exp::StaticAssert(_)
            | Exp::StaticAssertLegacy(_)
            | Exp::DefaultInitialization(_) => self.item(id),
            // An expression in statement position.
            _ => format!("{};", self.expr(id)),
        }
    }

    fn switch_block(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::SwitchCase { value, body } => {
                format!("case {}:\n{}break;", self.expr(*value), self.stmts(*body))
            }
            Exp::SwitchDefault(body) => format!("default:\n{}break;", self.stmts(*body)),
            Exp::StaticIf { cond, then_branch, else_branch } => {
                let mut out = format!(
                    "static if ({}) {}",
                    self.expr(*cond),
                    self.switch_branch(*then_branch)
                );
                if let Some(e) = else_branch {
                    out.push_str(" else ");
                    out.push_str(&self.switch_branch(*e));
                }
                out
            }
            _ => self.stmt(id),
        }
    }

    fn switch_branch(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::NestedScope { body, .. } => {
                let mut out = String::from("{\n");
                for b in self.seq(*body) {
                    out.push_str(&self.switch_block(b));
                    out.push('\n');
                }
                out.push('}');
                out
            }
            _ => self.switch_block(id),
        }
    }

    // --- expressions --------------------------------------------------------

    /// An expression in operand position: parenthesised iff its top is an
    /// operator, so no precedence table exists here — see the module comment.
    /// Whether `id` is an lvalue reading — an `ArrayAccessLValue` under the
    /// member accesses that wrap it — which only an increment's sum holds
    /// in value position.
    fn reads_as_lvalue(&self, id: ExpId) -> bool {
        match &self.ir.arena[id].exp {
            Exp::ArrayAccessLValue { .. } => true,
            Exp::MemberAccess { base, .. } => self.reads_as_lvalue(*base),
            _ => false,
        }
    }

    fn operand(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::Binary(op, ..) if op.text().is_some() => format!("({})", self.expr(id)),
            Exp::Unary(..) => format!("({})", self.expr(id)),
            _ => self.expr(id),
        }
    }

    fn expr(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::IntLiteral { ty, value } => {
                let suffix = ty.map(|t| t.to_string()).unwrap_or_default();
                format!("{value}{suffix}")
            }
            Exp::FloatLiteral(bits) => {
                let v = bits.to_f32();
                if v.is_finite() {
                    // `{:?}` is shortest-round-trip, and the lexer accepts
                    // every shape it emits (`probe_float`, exponent included).
                    format!("{v:?}")
                } else {
                    // An overflowing literal parses to `inf`, which does not
                    // lex; any overflowing spelling reparses to the same bits.
                    "1e40".to_string()
                }
            }
            Exp::BoolLiteral(true) => "true".to_string(),
            Exp::BoolLiteral(false) => "false".to_string(),
            Exp::StringLiteral(s) | Exp::InterpolatedString(s) => s.to_string(),
            // The written text around each hole, and the hole printed from
            // the tree — so a copy's hole reads its substituted arguments.
            Exp::Interpolated(segments) => {
                let mut out = String::new();
                for segment in segments.iter() {
                    out.push_str(&segment.text);
                    let Some(hole) = &segment.hole else { continue };
                    out.push('{');
                    out.push_str(&self.expr(hole.expr));
                    if hole.echo {
                        out.push('=');
                    }
                    if let Some(align) = hole.align {
                        out.push_str(", ");
                        out.push_str(&self.expr(align));
                    }
                    if let Some(format) = &hole.format {
                        out.push(':');
                        out.push_str(format);
                    }
                    out.push('}');
                }
                out
            }
            Exp::Identifier(s) => self.sym(*s),
            Exp::Path(p) => self.path(p),
            Exp::Binary(BinOp::LutMul, lhs, rhs) => {
                format!("lutmul({}, {})", self.operand(*lhs), self.operand(*rhs))
            }
            Exp::Binary(op, lhs, rhs) => {
                let text = op.text().expect("LutMul handled above");
                format!("{} {} {}", self.operand(*lhs), text, self.operand(*rhs))
            }
            Exp::Unary(op, e) => {
                let inner = self.operand(*e);
                match op {
                    kg_const::UnOp::BitSizeof
                    | kg_const::UnOp::ByteSizeof
                    | kg_const::UnOp::Clog2 => format!("{} {inner}", op.text()),
                    _ => format!("{}{inner}", op.text()),
                }
            }
            Exp::Mux { selector, cases } => {
                let mut parts = vec![self.operand(*selector)];
                parts.extend(self.seq(*cases).iter().map(|c| self.operand(*c)));
                format!("mux({})", parts.join(", "))
            }
            // Sliced for the one call shape no reconstruction can print: a
            // const-function callee is the *initializer's copy* (`Parser.hs:
            // 776-777`), an expression-position `NamedValue`, and its leaf
            // re-sorts by callee position into the bare `TypeParam` of
            // `Parser.hs:774-775` — one wrapper apart, and the written callee
            // name exists nowhere in the IR. The call's own slice is that
            // name, and the reparse repeats the reference's substitution.
            Exp::FunctionCall { attr, function, args } => self.slice(id).unwrap_or_else(|| {
                let rendered: Vec<String> =
                    self.seq(*args).iter().map(|a| self.operand(*a)).collect();
                format!(
                    "{}{}({})",
                    self.attrs(*attr),
                    self.callee(*function),
                    rendered.join(", ")
                )
            }),
            Exp::ArrayAccess { base, index } | Exp::ArrayAccessLValue { base, index } => {
                format!("{}[{}]", self.operand(*base), self.operand(*index))
            }
            Exp::MemberAccess { base, member } => {
                format!("{}.{}", self.operand(*base), self.expr(*member))
            }
            Exp::Cast { ty, value } => {
                format!("cast<{}>({})", self.ty(*ty), self.operand(*value))
            }
            Exp::Concat(items) => {
                let rendered: Vec<String> =
                    self.seq(*items).iter().map(|c| self.operand(*c)).collect();
                format!("concat({})", rendered.join(", "))
            }
            Exp::FanOut { count, value } => {
                format!("fan_out<{}>({})", self.operand(*count), self.operand(*value))
            }
            Exp::Static(e) => format!("static({})", self.operand(*e)),
            Exp::BitOffset { ty, field } => {
                format!("bitoffsetof({}, {})", self.ty(*ty), self.expr(*field))
            }
            Exp::ByteOffset { ty, field } => {
                format!("byteoffsetof({}, {})", self.ty(*ty), self.expr(*field))
            }
            Exp::InitializerList(items) => {
                let rendered: Vec<String> = self
                    .seq(*items)
                    .iter()
                    .map(|i| match self.exp(*i) {
                        Exp::Positional { value, .. } => self.operand(*value),
                        Exp::Designator { name, value } => {
                            format!(".{} = {}", self.expr(*name), self.operand(*value))
                        }
                        _ => self.expr(*i),
                    })
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
            Exp::Lambda { captures, params, ret, body, .. } => {
                let caps: Vec<String> = self
                    .seq(*captures)
                    .iter()
                    .map(|c| match self.exp(*c) {
                        Exp::Capture { name, .. } => self.name_of(*name),
                        _ => self.expr(*c),
                    })
                    .collect();
                format!(
                    "[{}]({}) -> {} {{\n{}}}",
                    caps.join(", "),
                    self.params(*params),
                    self.ty(*ret),
                    self.stmts(*body)
                )
            }
            Exp::Designator { name, value } => {
                format!(".{} = {}", self.expr(*name), self.operand(*value))
            }
            Exp::Positional { value, .. } => self.operand(*value),
            Exp::FunctionSpecifier { .. } => self.callee(id),
            // The sorted name shapes. Each prints its own source slice where
            // it has one (see the module comment); the structural fallback is
            // leaf-only, which is all a shape whose head is not its own — a
            // materialised copy's, a rewrite product's — can honestly claim.
            Exp::NamedValue(e) | Exp::QualifiedIdentifier(e) => {
                self.slice(id).unwrap_or_else(|| self.expr(*e))
            }
            // A rebuilt scoped name — one a rewrite below it made not its
            // own — prints leaf-only, as the module comment says it must,
            // with one exception that is exact: a prefix `resolve` produced.
            // `A<uint32>::A1_t` becomes `A<|$uint@32|>::A1_t`, and the
            // qualifier is the instance's mangled (then aliased) name, which
            // no materialised qualifier ever is. Suppressing it would print
            // the copy's member as a bare `A1_t` and type it against nothing.
            // The other exact exception is an *import alias*: `unit::assert`
            // inside a copy printed as `assert`, which the twin resolved to
            // the base intrinsic of that name — one parameter to the
            // module's two — and the copy's every call was "Invalid number
            // of arguments" (`test/library/data/hash_table.k`, 2026-09-10).
            // The module's aliases are known (`ir.imports`), and an alias is
            // never a materialised qualifier.
            Exp::ScopedName { prefix, ident, .. } => self.slice(id).unwrap_or_else(|| {
                let alias = prefix.and_then(|p| self.import_alias(p));
                match prefix {
                    Some(_) if alias.is_some() => {
                        let alias = alias.expect("checked");
                        format!("{}::{}", self.interner.resolve(alias), self.expr(*ident))
                    }
                    // A resolved instance as the qualifier (`A<|…|>::A1_t`),
                    // or as the leaf under a class qualifier — a member
                    // template's copy, `C::B_t<|@2|>`, which is declared
                    // inside `C` and is nothing without it (2026-09-11).
                    Some(p) if self.is_resolved_instance(*p) && !self.names_a_function_decl(*p) => {
                        format!("{}::{}", self.expr(*p), self.expr(*ident))
                    }
                    Some(p) if self.is_resolved_instance(id) && self.is_bare_class_prefix(*p) => {
                        format!("{}::{}", self.expr(*p), self.expr(*ident))
                    }
                    // A member *instance* as the qualifier — `B<|…|>::A<bool>::A1_t`,
                    // the copy's member template mentioned with its own list —
                    // spells its base and arguments; dropping it printed
                    // `A1_t` alone, a name nothing declares (2026-09-12).
                    Some(p) if matches!(self.exp(*p), Exp::TemplateInstance { .. }) => {
                        format!("{}::{}", self.expr(*p), self.expr(*ident))
                    }
                    // A type substituted for a template parameter as the
                    // qualifier — `T::B2` in `C<Bar>`'s copy — is that type:
                    // `removeNestedTypeIdentifier` keeps it there
                    // (`Desugar.hs:122-127`), and leaf-only printed `B2`,
                    // which names nothing (`enum.k::56`, the backend's
                    // "Unknown symbol", 2026-09-23). The substituted chain
                    // is the site's argument and keeps the site's expansion,
                    // where every chain a rewrite made inside the template
                    // is copied with it and shares the copy's.
                    Some(p)
                        if matches!(self.exp(*p), Exp::TypeIdentifier(..))
                            || self.ir.arena.node(*p).src.via != self.ir.arena.node(*ident).src.via =>
                    {
                        format!("{}::{}", self.expr(*p), self.expr(*ident))
                    }
                    // A qualifier the text wrote — `C::W`, a class's member
                    // alias given as an argument — copied into a copy's body,
                    // where `W` alone names nothing (2026-09-13).
                    Some(p) if self.written_qualifier_before(*p, Some(self.ir.arena.node(*ident).src.head.range)) => {
                        format!("{}::{}", self.expr(*p), self.expr(*ident))
                    }
                    _ => self.expr(*ident),
                }
            }),
            Exp::TypeIdentifier(_, e) => self.slice(id).unwrap_or_else(|| self.expr(*e)),
            Exp::TemplateInstance { base, args } => self.slice(id).unwrap_or_else(|| {
                let rendered: Vec<String> =
                    self.seq(*args).iter().map(|a| self.operand(*a)).collect();
                format!("{}<{}>", self.expr(*base), rendered.join(", "))
            }),
            // `src (EnumValue a _) = pretty a` (`PrettyPrint.hs:98`): the
            // constant by its name — an enum constant is written qualified
            // by its enum, `Base::RV32I`; the namespace ahead of it is one
            // the printer never spells. The only `EnumValue` this port
            // builds is a copy's enum-valued argument
            // (`elaborate::mint_value_tree`), whose name chain is minted and
            // would otherwise print leaf-only, naming nothing.
            Exp::EnumValue { ty, value } => {
                let mut segs = Vec::new();
                let mut at = match self.exp(*ty) {
                    Exp::QualifiedIdentifier(e) | Exp::NamedValue(e) => Some(*e),
                    _ => None,
                };
                while let Some(id) = at {
                    let Exp::ScopedName { prefix, ident, .. } = self.exp(id) else { break };
                    segs.push(self.expr(*ident));
                    at = *prefix;
                }
                match segs.as_slice() {
                    [constant, enumeration, ..] => format!("{enumeration}::{constant}"),
                    _ => self.expr(*value),
                }
            }
            // `src This{} = "this"` (`PrettyPrint.hs:165`). The `$this` spelling
            // is the *symbol* the node resolves through (`Symbols.hs:66`), not
            // the text the reference prints for it.
            Exp::This { .. } => "this".to_string(),
            // The type variants; everything else has no expression spelling
            // and must not bounce back through `ty` (which falls through to
            // `expr` — a two-arm cycle otherwise).
            Exp::Auto
            | Exp::Void
            | Exp::Boolean
            | Exp::Float
            | Exp::Str
            | Exp::Integer(_)
            | Exp::Unsigned(_)
            | Exp::ParamInt(_)
            | Exp::ParamUint(_)
            | Exp::Const(_)
            | Exp::Array { .. }
            | Exp::FunctionType { .. }
            | Exp::Decltype { .. }
            | Exp::Typename
            | Exp::Reference(_)
            | Exp::TypeParam { .. } => self.ty(id),
            _ => "0".to_string(),
        }
    }

    fn callee(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::FunctionSpecifier { receiver: Some(r), function } => {
                format!("{}.{}", self.expr(*r), self.receiver_method(*function))
            }
            Exp::FunctionSpecifier { receiver: None, function } => self.expr(*function),
            _ => self.expr(id),
        }
    }

    /// The member half of a method callee, when its own slice declined — which
    /// is exactly the written-argument-list case ([`Printer::slice`]'s
    /// containment refusal). The structural fallback restores the list, and it
    /// must restore a `template` keyword with it: the sorted instance keeps no
    /// record of the written keyword, but without one an *unresolved* member —
    /// `obj.template Go<uint32>()` under a dependent receiver — re-sorts down
    /// `templateArgs`' non-template arm and the instance collapses
    /// (`Parser.hs:676-678` against `:673-674`). Printing the keyword
    /// unconditionally here is sound because every arm that can have produced
    /// a sorted method instance sorts identically under `forceTemplate`
    /// (`Parser.hs:658-672`), and the keyword itself is not represented in the
    /// sorted IR for the reparse to disagree with.
    /// A method called *through a receiver* is spelled by its leaf. The
    /// qualifier `moveFunctions` leaves on the callee — `thisMethod b =
    /// functionSpecifier (Just $ thisValue b) b` reuses the node whole
    /// (`Function.hs:67`), and `qualifyName` (`:72-77`) had already given it
    /// the class's — is a spelling no parser reads back: `$this.Foo<|$bool|>::Do()`
    /// cost the twin of every class template with a wrapped member two parse
    /// errors and the round that would have decided its capture
    /// (`callbacks.k::63`). The receiver's type is what resolves the member.
    fn receiver_method(&self, function: ExpId) -> String {
        if let Exp::QualifiedIdentifier(inner) = self.exp(function)
            && let Exp::ScopedName { prefix: Some(_), ident, .. } = self.exp(*inner)
        {
            // A member template's own list survives the trimming, keyword
            // and all (`x.template Foo<uint32>()`).
            if let Exp::TemplateInstance { args, .. } = self.exp(*ident)
                && self.ir.arena.node(*args).src.written == Written::Own
            {
                return format!("template {}", self.expr(*ident));
            }
            return self.expr(*ident);
        }
        self.method_name(function)
    }

    fn method_name(&self, id: ExpId) -> String {
        if let Some(s) = self.slice(id) {
            return s;
        }
        if let Exp::QualifiedIdentifier(payload) = self.exp(id)
            && let Exp::TemplateInstance { args, .. } = self.exp(*payload)
            && self.ir.arena.node(*args).src.written == Written::Own
        {
            return format!("template {}", self.expr(id));
        }
        self.expr(id)
    }

    fn path(&self, p: &UnsortedPath) -> String {
        let mut out = String::new();
        if p.typename_kw {
            out.push_str("typename ");
        }
        let segs: Vec<String> = p
            .segments
            .iter()
            .map(|PathSeg { template_kw, name, args }| {
                let mut s = String::new();
                if *template_kw {
                    s.push_str("template ");
                }
                s.push_str(&self.sym(*name));
                if let Some(args) = args {
                    let rendered: Vec<String> =
                        self.seq(*args).iter().map(|a| self.operand(*a)).collect();
                    s.push('<');
                    s.push_str(&rendered.join(", "));
                    s.push('>');
                }
                s
            })
            .collect();
        out.push_str(&segs.join("::"));
        out
    }

    // --- types --------------------------------------------------------------

    fn ty(&self, id: ExpId) -> String {
        match self.exp(id) {
            Exp::Auto => "auto".to_string(),
            Exp::Void => "void".to_string(),
            Exp::Boolean => "bool".to_string(),
            Exp::Float => "float32".to_string(),
            Exp::Str => "string".to_string(),
            Exp::Integer(w) => format!("int{w}"),
            Exp::Unsigned(w) => format!("uint{w}"),
            Exp::ParamInt(e) => format!("int<{}>", self.operand(*e)),
            Exp::ParamUint(e) => format!("uint<{}>", self.operand(*e)),
            Exp::Const(e) => format!("const {}", self.ty(*e)),
            Exp::Array { attr, elem, dims } => {
                // A written `uint4[9][4]` is one node with dims `[9, 4]` —
                // outer first, `type''`'s `many (brackets expression)`
                // (`Parser.hs:533-538`). An array *nested* as the element
                // arises only from substitution — `T[10]` under `T =
                // uint4[4]` — and the reference's type there is `TArray uint4
                // [10, 4]`: the outer array's dimensions, then the element's.
                // Printed that way, so the twin reads back as the reference
                // types it and not as `uint4[4][10]`.
                let mut dims: Vec<ExpId> = self.seq(*dims).to_vec();
                let mut elem = *elem;
                while let Exp::Array { attr: inner_attr, elem: inner, dims: inner_dims } = self.exp(elem)
                    && self.seq(*inner_attr).is_empty()
                {
                    dims.extend(self.seq(*inner_dims).iter().copied());
                    elem = *inner;
                }
                let mut out = format!("{}{}", self.attrs(*attr), self.ty(elem));
                for dim in dims {
                    out.push('[');
                    out.push_str(&self.operand(dim));
                    out.push(']');
                }
                out
            }
            Exp::FunctionType { attr, params, ret } => {
                let rendered: Vec<String> = self
                    .seq(*params)
                    .iter()
                    // A parameter whose type substituted to `void` — `(T) -> R`
                    // under `closure<void, uint32>` — is no parameter:
                    // `params` drops it from the type (`typeOf p == TVoid =
                    // Nothing`, `Type.hs:1352-1353`), and neither parser
                    // accepts it written, so a twin that printed it did not
                    // reparse and its module lost its twin
                    // (`higher-order-functions.k::88`, 2026-09-23).
                    .filter(|p| {
                        !matches!(self.exp(**p), Exp::FunctionTypeParam { ty, .. } if matches!(self.exp(*ty), Exp::Void))
                    })
                    .map(|p| match self.exp(*p) {
                        Exp::FunctionTypeParam { attr, ty, name } => {
                            let mut s = format!("{}{}", self.attrs(*attr), self.ty(*ty));
                            if let Some(n) = name {
                                s.push(' ');
                                s.push_str(&self.name_of(*n));
                            }
                            s
                        }
                        _ => self.ty(*p),
                    })
                    .collect();
                format!("{}({}) -> {}", self.attrs(*attr), rendered.join(", "), self.ty(*ret))
            }
            Exp::Decltype { expr, .. } => format!("decltype({})", self.operand(*expr)),
            Exp::Path(p) => self.path(p),
            Exp::Typename => "typename".to_string(),
            Exp::Reference(e) => self.ty(*e),
            Exp::TypeParam { name, .. } => self.sym(*name),
            _ => self.expr(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{ExpansionInterner, FileId, InstanceId, Interner};
    use kg_syntax::{AstNode, ast};

    use crate::exp::Exp;
    use crate::rewrite::{
        DecidedInstances, DecidedSite, InstanceSpec, TemplateInstances, instantiate_templates,
        resolve_instances,
    };
    use crate::sort::NoImports;
    use crate::{ExpId, ModuleIr};

    fn sorted(text: &str) -> (ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(
            parse.errors().is_empty(),
            "fixture must parse clean: {:?}",
            parse.errors().first()
        );
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let ir = crate::lower_sorted(&file, FileId(0), &interner, &NoImports);
        (ir, interner)
    }

    /// The graded equality (`xtask ir`, `sorted trip:`): the printed text
    /// parses, and its sorted relowering renders the same structure.
    fn round_trips(ir: &ModuleIr, interner: &Interner, printed: &str) {
        let reparse = kg_syntax::parse_file(printed);
        assert!(
            reparse.errors().is_empty(),
            "printed output must parse: {:?}\n---\n{printed}",
            reparse.errors().first()
        );
        let file = ast::SourceFile::cast(reparse.syntax_node()).expect("root");
        let again = crate::lower_sorted(&file, FileId(0), interner, &NoImports);
        assert_eq!(
            ir.structure_string(interner),
            again.structure_string(interner),
            "printed:\n{printed}"
        );
    }

    fn walk(ir: &ModuleIr, mut f: impl FnMut(&ModuleIr, ExpId)) {
        let mut seen = vec![false; ir.arena.len()];
        let mut stack = vec![ir.root];
        let mut kids = Vec::new();
        while let Some(id) = stack.pop() {
            if std::mem::replace(&mut seen[id.index()], true) {
                continue;
            }
            f(ir, id);
            ir.arena[id].exp.children(&mut kids);
            stack.append(&mut kids);
        }
    }

    /// Read off `library/data/vector.k:186` (`unsigned::decrement(v.size)`)
    /// and the class-member constant and alias uses beside it: a **written**
    /// qualifier must print as written in every position that can carry one —
    /// type (`TypeIdentifier`'s slice), callee (the call's own slice), and
    /// value (`NamedValue`'s). Leaf-only printing here was the dominant
    /// failure of the 188/394 baseline: the reparse cannot see `count_t`,
    /// `decrement` or `LIMIT` from `use`'s scope, so the sorted shapes
    /// collapse — the value read keeps its `NamedValue` wrapper but loses the
    /// prefix chain (`prefix: none` against the written tree's
    /// `prefix: ScopedName`), 56 corpus files on its own.
    /// A string's holes are children of the tree (`Parser.hs:883-903`) and
    /// print from it — the text around them verbatim, then each hole's
    /// expression, `=`, alignment and format — so a copy's hole reads its
    /// substituted arguments where the written text would name a parameter
    /// the copy no longer has. Red with a hole printed as anything but its
    /// node.
    #[test]
    fn a_strings_holes_print_from_the_tree() {
        let source = "inline void f(uint8 x, uint8 w)\n{\n    print(\"a{{{x + 1:x2}b{x, w}{x=}\\n\");\n}\n";
        let (ir, interner) = sorted(source);
        let s = ir.structure_string(&interner);
        assert!(s.contains("hole: Binary"), "the hole is a child:\n{s}");
        assert!(s.contains("echo: NamedValue"), "the echoed hole is a child:\n{s}");
        assert!(s.contains("align: NamedValue"), "the alignment is a child:\n{s}");
        let printed = super::pretty(&ir, &interner);
        assert!(printed.contains("print(\"a{{{x + 1:x2}b{x, w}{x=}\\n\")"), "{printed}");
        round_trips(&ir, &interner, &printed);
    }

    /// An increment's sum holds the lvalue reading (`Parser.hs:612-614`),
    /// which `a[0] = a[0] + 1` would reparse as an rvalue: it prints as the
    /// increment it is; a compound assignment's re-read is an rvalue already.
    #[test]
    fn an_increment_prints_as_an_increment() {
        let source = "inline void f()\n{\n    uint8[4] a;\n    a[0]++;\n    a[1]--;\n    a[2] += 1;\n}\n";
        let (ir, interner) = sorted(source);
        let printed = super::pretty_with_source(&ir, &interner, source);
        assert!(printed.contains("a[0]++;"), "{printed}");
        assert!(printed.contains("a[1]--;"), "{printed}");
        assert!(printed.contains("a[2] = (a[2] + 1);"), "{printed}");
    }

    #[test]
    fn a_written_qualifier_prints_as_written() {
        let source = "class helpers\n{\npublic:\n    using count_t = uint32;\n\n\
             \x20   const uint32 LIMIT = 3;\n\n\
             \x20   inline uint32 decrement(uint32 x)\n    {\n        return x - 1;\n    }\n}\n\n\
             inline uint32 use(uint32 v)\n{\n    helpers::count_t c = helpers::decrement(v);\n\
             \x20   return c + helpers::LIMIT;\n}\n";
        let (ir, interner) = sorted(source);
        let s = ir.structure_string(&interner);
        assert!(s.contains("prefix: ScopedName"), "the qualifier sorts to a prefix:\n{s}");
        let printed = super::pretty_with_source(&ir, &interner, source);
        assert!(printed.contains("helpers::count_t"), "type position:\n{printed}");
        assert!(printed.contains("helpers::decrement(v)"), "callee position:\n{printed}");
        assert!(printed.contains("helpers::LIMIT"), "value position:\n{printed}");
        round_trips(&ir, &interner, &printed);
    }

    /// The delayed-error shape (`Parser.hs:720-721`, `Error.hs:136-137`): a
    /// template name missing its arguments stays a bare spine, the one
    /// position where the outermost sorted node is the `ScopedName` itself —
    /// a written qualifier on it has no wrapper to ride, only the spine's own
    /// slice. Not a corpus shape, and pinned here *because* the corpus cannot
    /// pin it: the `sorted trip:` line reads 403/403 with the `ScopedName`
    /// arm's slice disabled.
    #[test]
    fn a_qualified_template_name_missing_its_arguments_prints_as_written() {
        let source = "class helpers\n{\npublic:\n    template <typename T>\n\
             \x20   struct Box\n    {\n        T v;\n    }\n}\n\n\
             inline uint32 use()\n{\n    helpers::Box b;\n    return 0;\n}\n";
        let (ir, interner) = sorted(source);
        let printed = super::pretty_with_source(&ir, &interner, source);
        assert!(printed.contains("helpers::Box b"), "{printed}");
        round_trips(&ir, &interner, &printed);
    }

    /// The inverse pin, off the same shape written *unqualified*: the sorter
    /// grafts `counter` onto the sibling callee (`setQualifier`,
    /// `Desugar.hs:193-201`), and the printer must not print the graft. No
    /// per-node field can tell that prefix from a written one — `written ==
    /// Own` cannot (divergence #24's demotion donates written inner links
    /// too), lexability cannot (a sanitised `$` lexes) — which is why the
    /// printer prints *slices* rather than reconstructing prefixes, and why
    /// its no-source fallback stays leaf-only. Do not retry either guard;
    /// both reproduce the 188/394 baseline (`docs/journal.md`, "Print and
    /// reparse works").
    #[test]
    fn a_materialised_qualifier_is_never_printed() {
        let (ir, interner) = sorted(
            "class counter\n{\npublic:\n    inline uint32 decrement(uint32 x)\n    {\n\
             \x20       return x - 1;\n    }\n\n    inline uint32 twice(uint32 x)\n    {\n\
             \x20       return decrement(decrement(x));\n    }\n}\n",
        );
        let s = ir.structure_string(&interner);
        assert!(s.contains("prefix: ScopedName"), "the graft exists to suppress:\n{s}");
        let printed = super::pretty(&ir, &interner);
        assert!(!printed.contains("counter::"), "the graft printed:\n{printed}");
        round_trips(&ir, &interner, &printed);
    }

    /// Read off `test/syntax/helper/dependent_template.k:27`
    /// (`obj.template Go<uint32>()`): the written argument list and the
    /// written `template` keyword survive printing. Sliced at the call, the
    /// text is verbatim; without the keyword the reparse of an unresolved
    /// member sorts down `Parser.hs:673-674` instead of `:676-678` and the
    /// instance collapses to a bare `ScopedName`.
    #[test]
    fn a_method_callee_keeps_written_arguments_and_keyword() {
        let source = "class B\n{\npublic:\n    template <typename T>\n    T Go()\n    {\n\
             \x20       return {};\n    }\n}\n\ntemplate <auto Cond>\nclass C\n{\n\
             \x20   inline void Run()\n    {\n        obj.template Go<uint32>();\n    }\n\n\
             \x20   static if (Cond)\n    {\n        B obj;\n    }\n}\n";
        let (ir, interner) = sorted(source);
        let printed = super::pretty_with_source(&ir, &interner, source);
        assert!(printed.contains("obj.template Go<uint32>()"), "{printed}");
        round_trips(&ir, &interner, &printed);
    }

    /// Read off `test/logic/test_cases_18.k:42` (`stage0.execute(data)`):
    /// member syntax cannot carry a qualifier, and the sorted callee's own
    /// head is the `NAME_REF` alone (`field_seg_view`), so both the sliced
    /// and the structural printings are leaf-only. The no-source half pins
    /// the structural path; `$unresolved` is `methodSpecifier`'s placeholder
    /// prefix (`Parser.hs:730-734`) and must never leak into text.
    #[test]
    fn a_method_callee_prints_leaf_only_in_member_position() {
        let source = "class Stage\n{\npublic:\n    inline uint32 execute(uint32 input)\n    {\n\
             \x20       return input + 1;\n    }\n}\n\nclass Unit\n{\npublic:\n    Stage stage0;\n\n\
             \x20   inline uint32 go(uint32 x)\n    {\n        return stage0.execute(x);\n    }\n}\n";
        let (ir, interner) = sorted(source);
        for printed in [
            super::pretty_with_source(&ir, &interner, source),
            super::pretty(&ir, &interner),
        ] {
            assert!(printed.contains("stage0.execute(x)"), "{printed}");
            assert!(!printed.contains("::execute"), "{printed}");
            assert!(!printed.contains("$unresolved"), "{printed}");
            round_trips(&ir, &interner, &printed);
        }
    }

    /// Read off `test/syntax/helper/function_constants.k` (`C1::Run`): a
    /// const-function callee is the initializer's *copy* (`Parser.hs:
    /// 776-777`), so the written name `fn` exists nowhere in the IR and the
    /// copy's leaf spells `Fn` — which re-sorts in callee position to the
    /// bare `TypeParam` of `Parser.hs:774-775`, one `NamedValue` wrapper
    /// away from the copy. Only the call's own slice can print `fn(false)`;
    /// this is the test that keeps `FunctionCall` in the sliced set.
    #[test]
    fn a_const_function_callee_prints_the_written_call() {
        let source = "template <(bool) -> uint32 Fn>\nclass C1\n{\npublic:\n\
             \x20   const auto fn = Fn;\n\n    inline void Run()\n    {\n        fn(false);\n    }\n}\n";
        let (ir, interner) = sorted(source);
        let s = ir.structure_string(&interner);
        assert!(s.contains("function: NamedValue"), "the copied callee shape:\n{s}");
        let printed = super::pretty_with_source(&ir, &interner, source);
        assert!(printed.contains("fn(false)"), "{printed}");
        assert!(!printed.contains("Fn(false)"), "the copy's leaf printed:\n{printed}");
        round_trips(&ir, &interner, &printed);
    }

    /// The slice's containment contract, pinned directly because the print
    /// path shadows it: a method callee's own head is the `NAME_REF` alone,
    /// so `fixture.Run<-10, 10, 3>(…)`'s callee slice would drop the written
    /// list beside it (`test/library/control/loop.k`'s shape — this exact
    /// drop was 4 of the 6 residual files before the refusal existed). The
    /// enclosing call slices instead, and its range holds everything.
    #[test]
    fn a_slice_refuses_a_head_that_does_not_contain_its_written_material() {
        let source = "class Fixture\n{\npublic:\n    template <auto A, auto B, auto S>\n\
             \x20   inline uint32 Run()\n    {\n        return 0;\n    }\n}\n\nclass Use\n{\n\
             public:\n    Fixture fixture;\n\n    inline uint32 go()\n    {\n\
             \x20       return fixture.Run<-10, 10, 3>();\n    }\n}\n";
        let (ir, interner) = sorted(source);
        let p = super::Printer {
            ir: &ir,
            interner: &interner,
            source: Some(source),
            spellings: &[],
            fn_decls: std::cell::OnceCell::new(),
        };
        let mut checked = false;
        walk(&ir, |ir, id| {
            let Exp::FunctionCall { function, .. } = &ir.arena[id].exp else { return };
            let Exp::FunctionSpecifier { receiver: Some(_), function: callee } =
                &ir.arena[*function].exp
            else {
                return;
            };
            assert_eq!(
                p.slice(*callee),
                None,
                "the callee's head is the NAME_REF; its slice would drop `<-10, 10, 3>`"
            );
            assert_eq!(
                p.slice(id).as_deref(),
                Some("fixture.Run<-10, 10, 3>()"),
                "the call's head contains everything written"
            );
            checked = true;
        });
        assert!(checked, "the fixture holds exactly this shape");
    }

    const TWO_TEMPLATES: &str = "template <typename T>\nstruct Inner\n{\n    T x;\n}\n\n\
         template <typename T>\nstruct A\n{\n    Inner<T> inner;\n}\n\n\
         inline uint32 use()\n{\n    A<uint8> a;\n    return 0;\n}\n";

    /// The declaration a `Template` wraps, its qualified path
    /// (`template_path`'s reading), and the last written `name<…>` occurrence
    /// — enough to hand `instantiate_templates` one decided instance without
    /// `kg_db`'s census.
    fn decide_one(
        ir: &ModuleIr,
        interner: &Interner,
        name: &str,
        key: &str,
    ) -> TemplateInstances {
        let mut path = None;
        let mut site = None;
        walk(ir, |ir, id| {
            match &ir.arena[id].exp {
                Exp::Template { decl, .. } => {
                    if let Exp::Struct { scope, name: n, .. } | Exp::Class { scope, name: n, .. } =
                        &ir.arena[*decl].exp
                        && let Exp::Identifier(sym) = &ir.arena[*n].exp
                        && interner.resolve(*sym) == name
                    {
                        let mut segs = scope.segments().to_vec();
                        segs.push(*sym);
                        path = Some(segs);
                    }
                }
                Exp::TemplateInstance { base, args } => {
                    let mut leaf = *base;
                    while let Exp::ScopedName { ident, .. } = &ir.arena[leaf].exp {
                        leaf = *ident;
                    }
                    if let Exp::Identifier(sym) = &ir.arena[leaf].exp
                        && interner.resolve(*sym) == name
                        && site.is_none_or(|(r, _): (kg_base::FileRange, ExpId)| {
                            r.range.start() < ir.arena[id].src.head.range.start()
                        })
                    {
                        site = Some((ir.arena[id].src.head, *args));
                    }
                }
                _ => {}
            }
        });
        let (site, args) = site.expect("the fixture writes a use site");
        let mut map = TemplateInstances::default();
        map.insert(
            path.expect("the fixture declares the template"),
            vec![InstanceSpec { id: InstanceId::of(name, key), site, args, stamps: Vec::new(), round: 1 }],
        );
        map
    }

    /// A materialised copy prints its **substituted** shape, never the
    /// template's own text: the copy's nodes sit past `lowered_len` and
    /// carry an instantiation frame, and either fact alone refuses the
    /// slice — the head still names the body's pre-substitution range, where
    /// the written text says `Inner<T>`. The surviving template keeps
    /// printing as written.
    #[test]
    fn a_materialised_copy_prints_substituted_not_template_text() {
        let (ir, interner) = sorted(TWO_TEMPLATES);
        let map = decide_one(&ir, &interner, "A", "<uint8>");
        let exps = ExpansionInterner::new();
        let (out, report) = instantiate_templates(&ir, &interner, &exps, &map, None);
        assert_eq!(report.made, 1, "the fixture decides one instance");
        let printed = super::pretty_with_source(&out, &interner, TWO_TEMPLATES);
        assert!(printed.contains("Inner<uint8>"), "the copy's substituted member:\n{printed}");
        assert!(printed.contains("Inner<T>"), "the surviving template body:\n{printed}");
    }

    /// A rewrite is never masked by the written spelling its node still
    /// wears: `resolve_instances` replaces the decided mention with the
    /// mangled `ScopedName`, and the replacement carries the occurrence's
    /// `Src` verbatim (`Rewriter::replace`; the reference's `renote`,
    /// `Mangle.hs:27`) — `Own`, at ROOT, at the range that still spells
    /// `Holder<uint8>`. Only `lowered_len` tells the printer the node is not
    /// the lowering of that text.
    #[test]
    fn a_resolved_mention_is_not_masked_by_its_written_spelling() {
        let source = "template <typename T>\nstruct Holder\n{\n    T value;\n}\n\n\
             inline uint32 use()\n{\n    Holder<uint8> h;\n    return 0;\n}\n";
        let (ir, interner) = sorted(source);
        let mut decided = DecidedInstances::default();
        walk(&ir, |ir, id| {
            if matches!(ir.arena[id].exp, Exp::TemplateInstance { .. }) {
                decided.insert(
                    crate::rewrite::Site::from(ir.arena[id].src.head),
                    DecidedSite { template: "Holder".to_string(), stamps: Vec::new() },
                );
            }
        });
        assert_eq!(decided.len(), 1);
        let out = resolve_instances(&ir, &interner, &decided);
        let printed = super::pretty_with_source(&out, &interner, source);
        assert!(printed.contains("Holder<|$uint@8|>"), "the mangle is visible:\n{printed}");
        assert!(
            !printed.contains("Holder<uint8>"),
            "the written spelling would mask the rewrite:\n{printed}"
        );
    }

    /// Inside a materialised copy the whole-call slice is refused, so the
    /// method-callee fallback is what prints — and it must restore the
    /// written `template` keyword and argument list from the copy's own
    /// nodes ([`Printer::method_name`]). Off `dependent_template.k`'s `C`,
    /// the copy's call must come out `obj.template Go<uint32>()`, not
    /// `obj.Go()`.
    #[test]
    fn a_copied_method_call_restores_its_keyword_and_arguments() {
        let source = "class B\n{\npublic:\n    template <typename T>\n    T Go()\n    {\n\
             \x20       return {};\n    }\n}\n\ntemplate <auto Cond>\nclass C\n{\n\
             \x20   inline void Run()\n    {\n        obj.template Go<uint32>();\n    }\n\n\
             \x20   static if (Cond)\n    {\n        B obj;\n    }\n}\n\n\
             inline uint32 use()\n{\n    C<true> c;\n    return 0;\n}\n";
        let (ir, interner) = sorted(source);
        let map = decide_one(&ir, &interner, "C", "<true>");
        let exps = ExpansionInterner::new();
        let (out, report) = instantiate_templates(&ir, &interner, &exps, &map, None);
        assert_eq!(report.made, 1);
        let printed = super::pretty_with_source(&out, &interner, source);
        assert_eq!(
            printed.matches("obj.template Go<uint32>()").count(),
            2,
            "the template's own body and the copy's:\n{printed}"
        );
    }
}
