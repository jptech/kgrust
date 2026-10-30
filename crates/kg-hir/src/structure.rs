//! The canonical structural rendering of a [`ModuleIr`].
//!
//! One writer, two levels of detail. [`Detail::Structure`] prints exactly the
//! IR's *shape* — variant, scalar payloads, labelled children in order, and
//! the import list — and nothing position-dependent, so it is the equality the
//! round-trip gate compares: `lower(parse(pretty(ir)))` must render to the
//! same string as `ir`, even though every `Src` range differs between the two
//! texts and desugared forms reparse under different CST kinds (`x++` prints
//! as `x = x + 1;`, a ternary as `mux(…)`). [`Detail::Annotated`] adds the
//! per-node provenance — kind, range, `own`/`donated` — and is what
//! `kg dump ir` shows a human.
//!
//! Comparing renderings instead of walking two arenas in lockstep is a
//! deliberate trade: the string is unambiguous (every child is labelled, every
//! scalar printed), the diff between two of them is the *diagnostic* a failing
//! gate wants to show anyway, and there is one traversal to keep correct
//! instead of two.

use kg_base::Interner;
use kg_ty::QualifiedName;

use crate::arena::{ExpArena, ExpId};
use crate::exp::{Exp, PathSeg, UnsortedPath};
use crate::ModuleIr;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Detail {
    Structure,
    Annotated,
}

/// The structural rendering of one node's subtree — what two claimants of a
/// single `(range, kind)` `TypeKey` are compared by in `xtask ir`'s
/// provenance gate.
pub fn render_subtree(ir: &ModuleIr, interner: &Interner, id: ExpId) -> String {
    render_exp(&ir.arena, interner, id)
}

/// [`render_subtree`] straight off an arena — for callers that hold no
/// [`ModuleIr`] yet, like the sorter's `nub` fingerprint mid-lowering.
pub fn render_exp(arena: &ExpArena, interner: &Interner, id: ExpId) -> String {
    let mut w = Writer { arena, interner, detail: Detail::Structure, out: String::new() };
    w.node("", id, 0);
    w.out
}

pub fn render(ir: &ModuleIr, interner: &Interner, detail: Detail) -> String {
    let mut w = Writer { arena: &ir.arena, interner, detail, out: String::new() };
    for import in &ir.imports {
        w.out.push_str("import ");
        w.out.push_str(&import.name.join("."));
        if let Some(alias) = import.alias {
            w.out.push_str(" as ");
            w.out.push_str(&interner.resolve(alias));
        }
        w.out.push('\n');
    }
    w.node("", ir.root, 0);
    w.out
}

struct Writer<'a> {
    arena: &'a ExpArena,
    interner: &'a Interner,
    detail: Detail,
    out: String,
}

impl<'a> Writer<'a> {
    fn sym(&self, s: kg_base::Symbol) -> String {
        self.interner.resolve(s).to_string()
    }

    fn scope(&self, q: &QualifiedName) -> String {
        let parts: Vec<String> =
            q.segments().iter().map(|s| self.interner.resolve(*s).to_string()).collect();
        format!("[{}]", parts.join(","))
    }

    fn line(&mut self, label: &str, id: ExpId, depth: usize, head: &str) {
        let indent = "  ".repeat(depth);
        self.out.push_str(&indent);
        if !label.is_empty() {
            self.out.push_str(label);
            self.out.push_str(": ");
        }
        self.out.push_str(head);
        if self.detail == Detail::Annotated {
            let node = self.arena.node(id);
            let range = node.src.head.range;
            let written = match node.src.written {
                kg_base::Written::Own => "own",
                kg_base::Written::Donated => "donated",
                kg_base::Written::Reread => "reread",
            };
            self.out.push_str(&format!(
                "  @{}..{} {:?} {written}",
                u32::from(range.start()),
                u32::from(range.end()),
                node.kind
            ));
        }
        self.out.push('\n');
    }

    fn opt(&mut self, label: &str, id: Option<ExpId>, depth: usize) {
        match id {
            Some(id) => self.node(label, id, depth),
            None => {
                let indent = "  ".repeat(depth);
                self.out.push_str(&indent);
                self.out.push_str(label);
                self.out.push_str(": none\n");
            }
        }
    }

    /// A bare `Vec<ExpId>` field — `exposed`, `templateParams` — which has no
    /// node (and so no provenance line) of its own.
    fn list(&mut self, label: &str, ids: &[ExpId], depth: usize) {
        let indent = "  ".repeat(depth);
        self.out.push_str(&indent);
        self.out.push_str(label);
        self.out.push_str(&format!(": {}\n", ids.len()));
        for id in ids {
            self.node("", *id, depth + 1);
        }
    }

    fn path(&mut self, p: &UnsortedPath, id: ExpId, depth: usize, label: &str) {
        let mut head = String::from("Path");
        if p.typename_kw {
            head.push_str(" typename");
        }
        self.line(label, id, depth, &head);
        for PathSeg { template_kw, name, args } in &p.segments {
            let indent = "  ".repeat(depth + 1);
            self.out.push_str(&indent);
            self.out.push_str("seg: ");
            if *template_kw {
                self.out.push_str("template ");
            }
            self.out.push_str(&self.sym(*name));
            self.out.push('\n');
            if let Some(args) = args {
                self.node("args", *args, depth + 2);
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn node(&mut self, label: &str, id: ExpId, depth: usize) {
        let node = self.arena.node(id);
        let d = depth + 1;
        // Clone the small payloads out so `self` is free for recursion; the
        // children are `Copy` ids either way.
        match node.exp.clone() {
            Exp::Seq(items) => {
                self.line(label, id, depth, &format!("Seq {}", items.len()));
                for item in items {
                    self.node("", item, d);
                }
            }
            Exp::FlagAttr(f) => self.line(label, id, depth, &format!("FlagAttr {}", f.text())),
            Exp::IntAttr(a, e) => {
                self.line(label, id, depth, &format!("IntAttr {}", a.text()));
                self.node("value", e, d);
            }
            Exp::Attr(a, e) => {
                self.line(label, id, depth, &format!("Attr {}", a.text()));
                self.node("value", e, d);
            }
            Exp::FunctionModifier(m) => {
                self.line(label, id, depth, &format!("FunctionModifier {}", m.text()));
            }
            Exp::Auto => self.line(label, id, depth, "Auto"),
            Exp::Void => self.line(label, id, depth, "Void"),
            Exp::Boolean => self.line(label, id, depth, "Boolean"),
            Exp::Float => self.line(label, id, depth, "Float"),
            Exp::Str => self.line(label, id, depth, "String"),
            Exp::Integer(w) => self.line(label, id, depth, &format!("Integer {w}")),
            Exp::ParamInt(e) => {
                self.line(label, id, depth, "ParamInt");
                self.node("width", e, d);
            }
            Exp::Unsigned(w) => self.line(label, id, depth, &format!("Unsigned {w}")),
            Exp::ParamUint(e) => {
                self.line(label, id, depth, "ParamUint");
                self.node("width", e, d);
            }
            Exp::Const(e) => {
                self.line(label, id, depth, "Const");
                self.node("ty", e, d);
            }
            Exp::Reference(e) => {
                self.line(label, id, depth, "Reference");
                self.node("ty", e, d);
            }
            Exp::Array { attr, elem, dims } => {
                self.line(label, id, depth, "Array");
                self.node("attr", attr, d);
                self.node("elem", elem, d);
                self.node("dims", dims, d);
            }
            Exp::TemplateInstance { base, args } => {
                self.line(label, id, depth, "TemplateInstance");
                self.node("base", base, d);
                self.node("args", args, d);
            }
            Exp::TemplateParam { scope, kind, name, index, default } => {
                self.line(
                    label,
                    id,
                    depth,
                    &format!(
                        "TemplateParam {} #{index} scope={}",
                        self.sym(name),
                        self.scope(&scope)
                    ),
                );
                self.node("kind", kind, d);
                self.opt("default", default, d);
            }
            Exp::TypeParam { scope, kind, name, index } => {
                self.line(
                    label,
                    id,
                    depth,
                    &format!("TypeParam {} #{index} scope={}", self.sym(name), self.scope(&scope)),
                );
                self.node("kind", kind, d);
            }
            Exp::Typename => self.line(label, id, depth, "Typename"),
            Exp::FunctionTypeParam { attr, ty, name } => {
                self.line(label, id, depth, "FunctionTypeParam");
                self.node("attr", attr, d);
                self.node("ty", ty, d);
                self.opt("name", name, d);
            }
            Exp::FunctionType { attr, params, ret } => {
                self.line(label, id, depth, "FunctionType");
                self.node("attr", attr, d);
                self.node("params", params, d);
                self.node("ret", ret, d);
            }
            Exp::This { scope } => {
                self.line(label, id, depth, &format!("This scope={}", self.scope(&scope)));
            }
            Exp::Designator { name, value } => {
                self.line(label, id, depth, "Designator");
                self.node("name", name, d);
                self.node("value", value, d);
            }
            Exp::Positional { index, value } => {
                self.line(label, id, depth, &format!("Positional {index}"));
                self.node("value", value, d);
            }
            Exp::Labelled { label: name, value } => {
                self.line(label, id, depth, &format!("Labelled {}", self.sym(name)));
                self.node("value", value, d);
            }
            Exp::InitializerList(e) => {
                self.line(label, id, depth, "InitializerList");
                self.node("items", e, d);
            }
            Exp::BitOffset { ty, field } => {
                self.line(label, id, depth, "BitOffset");
                self.node("ty", ty, d);
                self.node("field", field, d);
            }
            Exp::ByteOffset { ty, field } => {
                self.line(label, id, depth, "ByteOffset");
                self.node("ty", ty, d);
                self.node("field", field, d);
            }
            Exp::IntLiteral { ty, value } => {
                let suffix = ty.map(|t| t.to_string()).unwrap_or_default();
                self.line(label, id, depth, &format!("IntLiteral {value}{suffix}"));
            }
            Exp::FloatLiteral(bits) => {
                self.line(label, id, depth, &format!("FloatLiteral {:?}", bits.to_f32()));
            }
            Exp::BoolLiteral(b) => self.line(label, id, depth, &format!("BoolLiteral {b}")),
            Exp::StringLiteral(s) => {
                self.line(label, id, depth, &format!("StringLiteral {s}"));
            }
            Exp::InterpolatedString(s) => {
                self.line(label, id, depth, &format!("InterpolatedString {s}"));
            }
            Exp::Interpolated(segments) => {
                let texts: Vec<&str> = segments.iter().map(|s| s.text.as_str()).collect();
                self.line(label, id, depth, &format!("InterpolatedString {}", texts.join(" {…} ")));
                for hole in segments.iter().filter_map(|s| s.hole.as_ref()) {
                    self.node(if hole.echo { "echo" } else { "hole" }, hole.expr, d);
                    self.opt("align", hole.align, d);
                }
            }
            Exp::Identifier(s) => {
                self.line(label, id, depth, &format!("Identifier {}", self.sym(s)));
            }
            Exp::ScopedName { scope, prefix, ident } => {
                self.line(label, id, depth, &format!("ScopedName scope={}", self.scope(&scope)));
                self.opt("prefix", prefix, d);
                self.node("ident", ident, d);
            }
            Exp::QualifiedIdentifier(e) => {
                self.line(label, id, depth, "QualifiedIdentifier");
                self.node("name", e, d);
            }
            Exp::TypeIdentifier(scope, e) => {
                self.line(label, id, depth, &format!("TypeIdentifier scope={}", self.scope(&scope)));
                self.node("name", e, d);
            }
            Exp::FunctionSpecifier { receiver, function } => {
                self.line(label, id, depth, "FunctionSpecifier");
                self.opt("receiver", receiver, d);
                self.node("function", function, d);
            }
            Exp::EnumValue { ty, value } => {
                self.line(label, id, depth, "EnumValue");
                self.node("ty", ty, d);
                self.node("value", value, d);
            }
            Exp::NamedValue(e) => {
                self.line(label, id, depth, "NamedValue");
                self.node("name", e, d);
            }
            Exp::Binary(op, lhs, rhs) => {
                self.line(label, id, depth, &format!("Binary {op:?}"));
                self.node("lhs", lhs, d);
                self.node("rhs", rhs, d);
            }
            Exp::Unary(op, e) => {
                self.line(label, id, depth, &format!("Unary {op:?}"));
                self.node("operand", e, d);
            }
            Exp::FunctionCall { attr, function, args } => {
                self.line(label, id, depth, "FunctionCall");
                self.node("attr", attr, d);
                self.node("function", function, d);
                self.node("args", args, d);
            }
            Exp::ArrayAccess { base, index } => {
                self.line(label, id, depth, "ArrayAccess");
                self.node("base", base, d);
                self.node("index", index, d);
            }
            Exp::ArrayAccessLValue { base, index } => {
                self.line(label, id, depth, "ArrayAccessLValue");
                self.node("base", base, d);
                self.node("index", index, d);
            }
            Exp::MemberAccess { base, member } => {
                self.line(label, id, depth, "MemberAccess");
                self.node("base", base, d);
                self.node("member", member, d);
            }
            Exp::Cast { ty, value } => {
                self.line(label, id, depth, "Cast");
                self.node("ty", ty, d);
                self.node("value", value, d);
            }
            Exp::Mux { selector, cases } => {
                self.line(label, id, depth, "Mux");
                self.node("selector", selector, d);
                self.node("cases", cases, d);
            }
            Exp::Concat(e) => {
                self.line(label, id, depth, "Concat");
                self.node("items", e, d);
            }
            Exp::FanOut { count, value } => {
                self.line(label, id, depth, "FanOut");
                self.node("count", count, d);
                self.node("value", value, d);
            }
            Exp::Static(e) => {
                self.line(label, id, depth, "Static");
                self.node("value", e, d);
            }
            Exp::Annotated { attr, stmt } => {
                self.line(label, id, depth, "Annotated");
                self.node("attr", attr, d);
                self.node("stmt", stmt, d);
            }
            Exp::Barrier => self.line(label, id, depth, "Barrier"),
            Exp::Reorder(e) => {
                self.line(label, id, depth, "Reorder");
                self.node("stmt", e, d);
            }
            Exp::Return(e) => {
                self.line(label, id, depth, "Return");
                self.opt("value", e, d);
            }
            Exp::DoWhile { attr, cond, body } => {
                self.line(label, id, depth, "DoWhile");
                self.node("attr", attr, d);
                self.node("cond", cond, d);
                self.node("body", body, d);
            }
            Exp::UnrolledFor { var, limit, body } => {
                self.line(label, id, depth, "UnrolledFor");
                self.node("var", var, d);
                self.node("limit", limit, d);
                self.node("body", body, d);
            }
            Exp::StaticFor { var, limit, body } => {
                self.line(label, id, depth, "StaticFor");
                self.node("var", var, d);
                self.node("limit", limit, d);
                self.node("body", body, d);
            }
            Exp::RangeFor { attr, var, limit, body } => {
                self.line(label, id, depth, "RangeFor");
                self.node("attr", attr, d);
                self.node("var", var, d);
                self.node("limit", limit, d);
                self.node("body", body, d);
            }
            Exp::If { cond, then_branch, else_branch } => {
                self.line(label, id, depth, "If");
                self.node("cond", cond, d);
                self.node("then", then_branch, d);
                self.opt("else", else_branch, d);
            }
            Exp::StaticIf { cond, then_branch, else_branch } => {
                self.line(label, id, depth, "StaticIf");
                self.node("cond", cond, d);
                self.node("then", then_branch, d);
                self.opt("else", else_branch, d);
            }
            Exp::Switch { selector, blocks } => {
                self.line(label, id, depth, "Switch");
                self.node("selector", selector, d);
                self.node("blocks", blocks, d);
            }
            Exp::SwitchCase { value, body } => {
                self.line(label, id, depth, "SwitchCase");
                self.node("value", value, d);
                self.node("body", body, d);
            }
            Exp::SwitchDefault(e) => {
                self.line(label, id, depth, "SwitchDefault");
                self.node("body", e, d);
            }
            Exp::Assign { lvalue, value } => {
                self.line(label, id, depth, "Assign");
                self.node("lvalue", lvalue, d);
                self.node("value", value, d);
            }
            Exp::NestedScope { scope, name, body } => {
                self.line(label, id, depth, &format!("NestedScope scope={}", self.scope(&scope)));
                self.node("name", name, d);
                self.node("body", body, d);
            }
            Exp::ModuleIdentifier(parts) => {
                self.line(label, id, depth, &format!("ModuleIdentifier {}", parts.join(".")));
            }
            Exp::ModuleDiff(lhs, rhs) => {
                self.line(
                    label,
                    id,
                    depth,
                    &format!("ModuleDiff {} \\ {}", lhs.join("."), rhs.join(".")),
                );
            }
            Exp::ModuleDecl { name, exposed, body } => {
                self.line(label, id, depth, "ModuleDecl");
                self.node("name", name, d);
                self.list("exposed", &exposed, d);
                self.node("body", body, d);
            }
            Exp::EnumConstant { scope, name, value } => {
                self.line(label, id, depth, &format!("EnumConstant scope={}", self.scope(&scope)));
                self.node("name", name, d);
                self.opt("value", value, d);
            }
            Exp::Enum { scope, name, ty, body } => {
                self.line(label, id, depth, &format!("Enum scope={}", self.scope(&scope)));
                self.node("name", name, d);
                self.node("ty", ty, d);
                self.node("body", body, d);
            }
            Exp::Struct { scope, name, body } => {
                self.line(label, id, depth, &format!("Struct scope={}", self.scope(&scope)));
                self.node("name", name, d);
                self.node("body", body, d);
            }
            Exp::Union { scope, name, body } => {
                self.line(label, id, depth, &format!("Union scope={}", self.scope(&scope)));
                self.node("name", name, d);
                self.node("body", body, d);
            }
            Exp::Class { scope, name, body } => {
                self.line(label, id, depth, &format!("Class scope={}", self.scope(&scope)));
                self.node("name", name, d);
                self.node("body", body, d);
            }
            Exp::DefaultInitialization(e) => {
                self.line(label, id, depth, "DefaultInitialization");
                self.node("value", e, d);
            }
            Exp::Private => self.line(label, id, depth, "Private"),
            Exp::Public => self.line(label, id, depth, "Public"),
            Exp::Template { params, decl } => {
                self.line(label, id, depth, "Template");
                self.list("params", &params, d);
                self.node("decl", decl, d);
            }
            Exp::Decltype { scope, expr } => {
                self.line(label, id, depth, &format!("Decltype scope={}", self.scope(&scope)));
                self.node("expr", expr, d);
            }
            Exp::Alias { scope, body, name } => {
                self.line(label, id, depth, &format!("Alias scope={}", self.scope(&scope)));
                self.node("body", body, d);
                self.node("name", name, d);
            }
            Exp::Capture { scope, name } => {
                self.line(label, id, depth, &format!("Capture scope={}", self.scope(&scope)));
                self.node("name", name, d);
            }
            Exp::Variable { scope, attr, ty, name, init, flags } => {
                self.line(
                    label,
                    id,
                    depth,
                    &format!("Variable {flags:?} scope={}", self.scope(&scope)),
                );
                self.node("attr", attr, d);
                self.node("ty", ty, d);
                self.node("name", name, d);
                self.opt("init", init, d);
            }
            Exp::FuncParam { scope, attr, ty, name } => {
                self.line(label, id, depth, &format!("FuncParam scope={}", self.scope(&scope)));
                self.node("attr", attr, d);
                self.node("ty", ty, d);
                self.node("name", name, d);
            }
            Exp::Function { scope, kind, attr, modifier, ret, name, params, body } => {
                self.line(
                    label,
                    id,
                    depth,
                    &format!("Function {kind:?} scope={}", self.scope(&scope)),
                );
                self.node("attr", attr, d);
                self.opt("modifier", modifier, d);
                self.node("ret", ret, d);
                self.node("name", name, d);
                self.node("params", params, d);
                self.node("body", body, d);
            }
            Exp::FunctionDecl { scope, attr, ret, name, params } => {
                self.line(label, id, depth, &format!("FunctionDecl scope={}", self.scope(&scope)));
                self.node("attr", attr, d);
                self.node("ret", ret, d);
                self.node("name", name, d);
                self.node("params", params, d);
            }
            Exp::Lambda { scope, name, captures, params, ret, body } => {
                self.line(label, id, depth, &format!("Lambda scope={}", self.scope(&scope)));
                self.node("name", name, d);
                self.node("captures", captures, d);
                self.node("params", params, d);
                self.node("ret", ret, d);
                self.node("body", body, d);
            }
            Exp::ExportType { attr, ty } => {
                self.line(label, id, depth, "ExportType");
                self.node("attr", attr, d);
                self.node("ty", ty, d);
            }
            Exp::Extern { attr, ty } => {
                self.line(label, id, depth, "Extern");
                self.node("attr", attr, d);
                self.node("ty", ty, d);
            }
            Exp::StaticAssert(e) => {
                self.line(label, id, depth, "StaticAssert");
                self.node("cond", e, d);
            }
            Exp::StaticAssertLegacy(e) => {
                self.line(label, id, depth, "StaticAssertLegacy");
                self.node("cond", e, d);
            }
            Exp::Warning { kind, message, node } => {
                self.line(label, id, depth, &format!("Warning {kind:?} {message}"));
                self.node("node", node, d);
            }
            Exp::Error(message) => self.line(label, id, depth, &format!("Error {message}")),
            Exp::Path(p) => self.path(&p, id, depth, label),
            Exp::Unsupported => self.line(label, id, depth, "Unsupported"),
        }
    }
}
