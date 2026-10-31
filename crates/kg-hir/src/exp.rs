//! `Exp` — `ExpF` (`Parser/Syntax.hs:282-584`), transcribed constructor for
//! constructor, in the reference's declaration order.
//!
//! Children are [`ExpId`]s into the owning [`crate::ExpArena`]; every scalar
//! payload reuses the type another crate already owns (`kg_const::BinOp`,
//! `kg_ty::Flag`, `kg_resolve::DeclareFlag`, …) rather than growing a second
//! copy — a duplicated enum is the drift this workspace has refused twice in
//! writing.
//!
//! Two variants exist here that `ExpF` does not have, both at the bottom and
//! both deliberate:
//!
//! * [`Exp::Path`] — the reference's parser *sorts* a written name into
//!   `ScopedNameF` / `TypeIdentifierF` / `NamedValueF` / `TemplateInstanceF` by
//!   consulting the symbol table mid-parse (`Parser.hs:652-695`), which this
//!   port's parser deliberately has no access to (the first load-bearing
//!   decision, `docs/architecture.md`). Slice 1 lowers every `PATH` to the
//!   unsorted form and sorts nothing: divergence #22 records that guessing the
//!   sort invents "did you forget typename" errors on `S1<Foo>` for every
//!   struct in the corpus. The sorted variants are still declared below —
//!   resolution produces them in a later slice.
//! * [`Exp::Unsupported`] — the honest hole. `cargo xtask ir` counts these
//!   (gate 1, totality), so an unlowerable construct is a number, not a guess.

use kg_base::{SmolStr, Symbol};
use kg_const::{BinOp, ConstInt, F32Bits, LiteralType, UnOp};
use kg_resolve::DeclareFlag;
use kg_ty::{Attribute, Flag, FunctionKind, QualifiedName};

use crate::arena::ExpId;

/// `data Modifier` (`Parser/Syntax.hs:193-203`). Not in `kg-ty`: no type rule
/// ever reads it — `moveFunctions` and friends do, and they live on this IR.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Modifier {
    Inline,
    NoInline,
}

impl Modifier {
    /// The source spelling (`Parser/Syntax.hs:198-200`).
    pub fn text(self) -> &'static str {
        match self {
            Modifier::Inline => "inline",
            Modifier::NoInline => "noinline",
        }
    }
}

/// `ErrorF e`'s `e` (`Parser/Syntax.hs:583`): a located parse error in the
/// reference, flattened here to its message — and, where a pass decides by
/// the message, its family ([`kg_diag::DiagnosticKind`], sprint W8.1), set
/// where the node is made and carried wherever the node is copied. Prints
/// as its message.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ErrorNote {
    pub kind: Option<kg_diag::DiagnosticKind>,
    pub message: SmolStr,
}

impl ErrorNote {
    /// A message no pass asks about.
    pub fn new(message: impl Into<SmolStr>) -> ErrorNote {
        ErrorNote { kind: None, message: message.into() }
    }

    /// A message of the family `kind`.
    pub fn of(kind: kg_diag::DiagnosticKind, message: impl Into<SmolStr>) -> ErrorNote {
        ErrorNote { kind: Some(kind), message: message.into() }
    }

    pub fn is(&self, kind: kg_diag::DiagnosticKind) -> bool {
        self.kind == Some(kind)
    }
}

impl std::fmt::Display for ErrorNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// One segment of an unsorted path — `a`, `template a`, `a<args>`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PathSeg {
    /// The `template` disambiguator was written (`Parser.hs:646`).
    pub template_kw: bool,
    pub name: Symbol,
    /// The written `<…>` argument list, a [`Exp::Seq`] — `None` when no angle
    /// list was written at all. `Foo<>` is `Some` of an empty `Seq`
    /// (`sepBy`, `Parser.hs:697`).
    pub args: Option<ExpId>,
}

/// A written name path, **unsorted**. See the module comment and divergence
/// #22: which of the reference's five sorted node shapes this becomes is a
/// symbol-table question, and slice 1 refuses to guess it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct UnsortedPath {
    /// A leading `typename` disambiguator forced the type reading
    /// (`Parser.hs:547`). Recorded because the future sorter needs it; the
    /// reference consumes it at parse time and keeps no node.
    pub typename_kw: bool,
    pub segments: Vec<PathSeg>,
    /// What the sorter knew when it refused the path; `None` where the
    /// lowering ran with no symbol context and refused nothing.
    pub refused: Option<Refused>,
}

/// The parse-time facts about a path the sorter refused, which the
/// reference's parser rejects or reads otherwise (divergences #7, #22) and
/// pass 9 judges in its place: the name a later lookup must still find.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Refused {
    /// `getScopeQualifier` at the path (`Parser.hs:649`) — the scope every
    /// `ScopedNameF` of it would carry, which `lookupInScope` walks out from.
    pub scope: QualifiedName,
    /// Whether the parse-time table answered the head segment (`findSymbol`,
    /// `Parser.hs:657`).
    pub head_found: bool,
    /// Whether the path is written as a type — its CST parent a `PATH_TYPE`
    /// (`TypeIdentifier scope s`, `Parser.hs:777`) — or as a value.
    pub in_type: bool,
}

/// `ExpF e a` (`Parser/Syntax.hs:282-584`). Constructor order and field order
/// follow the reference; field *names* follow its record labels where it has
/// them.
///
/// The `e` parameter (a megaparsec `ParseError`, `Parser/SymbolTable.hs:46`)
/// is flattened to a message string in [`Exp::Warning`] and [`Exp::Error`]:
/// slice 1 diagnoses nothing and constructs neither, and the slice that does
/// will thread `kg_diag` through them.
/// One segment of an [`Exp::Interpolated`] — the reference's `(String, Maybe
/// (e, Bool, Maybe e, Maybe Format))` (`Parser.hs:886-899`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Segment {
    /// The source text before the hole, undecoded: the first segment's
    /// carries the opening `"`, the last one's the closing `"` (the last
    /// segment has no hole). Two adjacent holes leave an empty one between.
    pub text: SmolStr,
    pub hole: Option<Hole>,
}

/// `{expr[=][, align][: format]}`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Hole {
    pub expr: ExpId,
    /// `{e=}`: the expression's text is echoed before its value.
    pub echo: bool,
    pub align: Option<ExpId>,
    /// The text after the `:`, unparsed; pass 0 judges its grammar
    /// (`Parser.hs:900-903`).
    pub format: Option<SmolStr>,
}

impl Segment {
    /// The hole's children in `ExpF` order: the expression, then the
    /// alignment.
    fn children(&self) -> impl Iterator<Item = ExpId> + '_ {
        self.hole.iter().flat_map(|h| std::iter::once(h.expr).chain(h.align))
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Exp {
    // --- list ---
    Seq(Vec<ExpId>),
    // --- attributes ---
    FlagAttr(Flag),
    IntAttr(Attribute, ExpId),
    Attr(Attribute, ExpId),
    FunctionModifier(Modifier),
    // --- types ---
    Auto,
    Void,
    Boolean,
    Float,
    /// `StringF`. `Str` because a bare `String` variant reads as the std type.
    Str,
    /// `IntegerF Int` — `int32` is one lexer token carrying its width
    /// (`Lexer.hs:241-243`).
    Integer(u32),
    ParamInt(ExpId),
    Unsigned(u32),
    ParamUint(ExpId),
    Const(ExpId),
    /// Never written; built by the closure passes (`Closure.hs`).
    Reference(ExpId),
    /// `ArrayF attr elem (Seq dims)` (`Parser.hs:529-538`).
    Array { attr: ExpId, elem: ExpId, dims: ExpId },
    TemplateInstance { base: ExpId, args: ExpId },
    TemplateParam {
        scope: QualifiedName,
        /// `Typename`, a type, or a template-template signature
        /// (`Parser.hs:366-371`).
        kind: ExpId,
        /// A bare `Name` in the reference too — never an `Identifier` node.
        name: Symbol,
        index: u32,
        default: Option<ExpId>,
    },
    TypeParam { scope: QualifiedName, kind: ExpId, name: Symbol, index: u32 },
    Typename,
    FunctionTypeParam { attr: ExpId, ty: ExpId, name: Option<ExpId> },
    FunctionType { attr: ExpId, params: ExpId, ret: ExpId },
    // --- expressions ---
    /// Built by `captureThis` (pass 3), never by the parser.
    This { scope: QualifiedName },
    Designator { name: ExpId, value: ExpId },
    Positional { index: u32, value: ExpId },
    /// Edition 1.1's named argument, `label: value`, in a call's argument
    /// list (`docs/editions/1.1.md`). No construct of the reference's: lowered
    /// only from an `ARG_LABEL`, which only a file of a later edition parses,
    /// and gone before the walk — the call's arguments put in its
    /// parameters' order, or the call an error (`kg_sem::named`).
    Labelled { label: Symbol, value: ExpId },
    InitializerList(ExpId),
    BitOffset { ty: ExpId, field: ExpId },
    ByteOffset { ty: ExpId, field: ExpId },
    IntLiteral { ty: Option<LiteralType>, value: ConstInt },
    FloatLiteral(F32Bits),
    BoolLiteral(bool),
    /// Only an attribute argument — `[[name("s")]]` (`Parser.hs:177`). A string
    /// *expression* is [`Exp::InterpolatedString`], even without interpolations
    /// (`Parser.hs:883`). Verbatim token text, quotes included — see
    /// `InterpolatedString` for why.
    StringLiteral(SmolStr),
    /// A string with **no hole**, as the whole `"…"` token text, verbatim and
    /// undecoded — the reference's `InterpolatedStringF [(s, Nothing)]`,
    /// which `interpolatedStringPre` turns into a `StringLiteralF`
    /// (`Frontend.hs:233-234`). A string with a hole is
    /// [`Exp::Interpolated`].
    InterpolatedString(SmolStr),
    /// A string with at least one hole: `InterpolatedStringF` over its
    /// segments (`Parser.hs:883-903`), each the text before a hole and the
    /// hole — an expression, `=`, an alignment, a format. The holes are
    /// ordinary children, so every rewrite reaches them and a copy prints
    /// them substituted.
    Interpolated(Box<[Segment]>),
    Identifier(Symbol),
    ScopedName { scope: QualifiedName, prefix: Option<ExpId>, ident: ExpId },
    QualifiedIdentifier(ExpId),
    TypeIdentifier(QualifiedName, ExpId),
    /// `FunctionSpecifierF (Maybe obj) name` — a callee. `Some` receiver for a
    /// method call (`Parser.hs:730-734`), `None` for a free function.
    FunctionSpecifier { receiver: Option<ExpId>, function: ExpId },
    /// Built by `resolveEnums`, never by the parser.
    EnumValue { ty: ExpId, value: ExpId },
    NamedValue(ExpId),
    Binary(BinOp, ExpId, ExpId),
    Unary(UnOp, ExpId),
    FunctionCall { attr: ExpId, function: ExpId, args: ExpId },
    ArrayAccess { base: ExpId, index: ExpId },
    ArrayAccessLValue { base: ExpId, index: ExpId },
    MemberAccess { base: ExpId, member: ExpId },
    Cast { ty: ExpId, value: ExpId },
    /// Also the lowering of `c ? t : e` — `Mux c (Seq [e, t])`, arms reversed
    /// (`Parser.hs:968`).
    Mux { selector: ExpId, cases: ExpId },
    Concat(ExpId),
    FanOut { count: ExpId, value: ExpId },
    Static(ExpId),
    // --- statements ---
    Annotated { attr: ExpId, stmt: ExpId },
    Barrier,
    Reorder(ExpId),
    Return(Option<ExpId>),
    DoWhile { attr: ExpId, cond: ExpId, body: ExpId },
    /// Built by `staticControl` (`Constexpr.hs:54`), never by the parser.
    UnrolledFor { var: ExpId, limit: ExpId, body: ExpId },
    StaticFor { var: ExpId, limit: ExpId, body: ExpId },
    RangeFor { attr: ExpId, var: ExpId, limit: ExpId, body: ExpId },
    If { cond: ExpId, then_branch: ExpId, else_branch: Option<ExpId> },
    StaticIf { cond: ExpId, then_branch: ExpId, else_branch: Option<ExpId> },
    Switch { selector: ExpId, blocks: ExpId },
    /// The mandatory `break;` (`Parser.hs:602-603`) is consumed, not a node.
    SwitchCase { value: ExpId, body: ExpId },
    SwitchDefault(ExpId),
    Assign { lvalue: ExpId, value: ExpId },
    // --- declarations ---
    NestedScope { scope: QualifiedName, name: ExpId, body: ExpId },
    /// Components keep their written spelling; `arria-10` is not an
    /// identifier, which is why these are text and not `Symbol`s
    /// (`kg-resolve/src/lib.rs`, decision 2).
    ModuleIdentifier(Vec<SmolStr>),
    ModuleDiff(Vec<SmolStr>, Vec<SmolStr>),
    /// `exposed` is a bare list in the reference too, not a `Seq`
    /// (`Parser/Syntax.hs:465-469`).
    ModuleDecl { name: ExpId, exposed: Vec<ExpId>, body: ExpId },
    EnumConstant { scope: QualifiedName, name: ExpId, value: Option<ExpId> },
    Enum { scope: QualifiedName, name: ExpId, ty: ExpId, body: ExpId },
    Struct { scope: QualifiedName, name: ExpId, body: ExpId },
    Union { scope: QualifiedName, name: ExpId, body: ExpId },
    Class { scope: QualifiedName, name: ExpId, body: ExpId },
    DefaultInitialization(ExpId),
    Private,
    Public,
    /// `templateParams` is a bare list, not a `Seq` (`Parser/Syntax.hs:508-511`).
    Template { params: Vec<ExpId>, decl: ExpId },
    Decltype { scope: QualifiedName, expr: ExpId },
    /// Field order is the reference's: the aliased type *before* the name
    /// (`Parser/Syntax.hs:518-522`).
    Alias { scope: QualifiedName, body: ExpId, name: ExpId },
    Capture { scope: QualifiedName, name: ExpId },
    Variable {
        scope: QualifiedName,
        attr: ExpId,
        ty: ExpId,
        name: ExpId,
        init: Option<ExpId>,
        flags: DeclareFlag,
    },
    /// `default` is edition 1.1's (`docs/editions/1.1/default-arguments.md`):
    /// the value a call that leaves the parameter out passes, `None` in every
    /// reference program — the reference's parameter has none (`Parser.hs:379`).
    FuncParam { scope: QualifiedName, attr: ExpId, ty: ExpId, name: ExpId, default: Option<ExpId> },
    Function {
        scope: QualifiedName,
        kind: FunctionKind,
        attr: ExpId,
        modifier: Option<ExpId>,
        ret: ExpId,
        name: ExpId,
        params: ExpId,
        body: ExpId,
    },
    FunctionDecl { scope: QualifiedName, attr: ExpId, ret: ExpId, name: ExpId, params: ExpId },
    Lambda {
        scope: QualifiedName,
        name: ExpId,
        captures: ExpId,
        params: ExpId,
        ret: ExpId,
        body: ExpId,
    },
    ExportType { attr: ExpId, ty: ExpId },
    Extern { attr: ExpId, ty: ExpId },
    StaticAssert(ExpId),
    StaticAssertLegacy(ExpId),
    /// `WarningF WarningKind e a` (`Parser/Syntax.hs:581`). The `e` payload —
    /// a located parse error in the reference — is a bare message until the
    /// slice that emits these wires `kg_diag` through.
    Warning { kind: kg_diag::WarningKind, message: SmolStr, node: ExpId },
    Error(ErrorNote),
    // --- not in `ExpF`; see the module comment ---
    Path(UnsortedPath),
    Unsupported,
}

impl Exp {
    /// Every child id, in field order — the reference's functor traversal
    /// (`Parser/Syntax.hs:584` derives `Functor`), which every fold over this IR
    /// shares. `structure::render` enumerates the same children with labels;
    /// this is the unlabelled version a walk wants.
    ///
    /// Appended to `out` rather than returned so the caller's work list is the
    /// only allocation on a hot walk.
    #[allow(clippy::too_many_lines)]
    pub fn children(&self, out: &mut Vec<ExpId>) {
        match self {
            Exp::Seq(items) => out.extend(items.iter().copied()),
            Exp::FlagAttr(_)
            | Exp::FunctionModifier(_)
            | Exp::Auto
            | Exp::Void
            | Exp::Boolean
            | Exp::Float
            | Exp::Str
            | Exp::Integer(_)
            | Exp::Unsigned(_)
            | Exp::Typename
            | Exp::This { .. }
            | Exp::IntLiteral { .. }
            | Exp::FloatLiteral(_)
            | Exp::BoolLiteral(_)
            | Exp::StringLiteral(_)
            | Exp::InterpolatedString(_)
            | Exp::Identifier(_)
            | Exp::Barrier
            | Exp::ModuleIdentifier(_)
            | Exp::ModuleDiff(..)
            | Exp::Private
            | Exp::Public
            | Exp::Error(_)
            | Exp::Unsupported => {}
            Exp::Interpolated(segments) => out.extend(segments.iter().flat_map(Segment::children)),
            Exp::IntAttr(_, e)
            | Exp::Attr(_, e)
            | Exp::ParamInt(e)
            | Exp::ParamUint(e)
            | Exp::Const(e)
            | Exp::Reference(e)
            | Exp::InitializerList(e)
            | Exp::QualifiedIdentifier(e)
            | Exp::TypeIdentifier(_, e)
            | Exp::NamedValue(e)
            | Exp::Unary(_, e)
            | Exp::Concat(e)
            | Exp::Static(e)
            | Exp::Reorder(e)
            | Exp::SwitchDefault(e)
            | Exp::DefaultInitialization(e)
            | Exp::Decltype { expr: e, .. }
            | Exp::Capture { name: e, .. }
            | Exp::StaticAssert(e)
            | Exp::StaticAssertLegacy(e)
            | Exp::Warning { node: e, .. }
            | Exp::Positional { value: e, .. }
            | Exp::Labelled { value: e, .. } => out.push(*e),
            Exp::Array { attr, elem, dims } => out.extend([*attr, *elem, *dims]),
            Exp::TemplateInstance { base, args } => out.extend([*base, *args]),
            Exp::TemplateParam { kind, default, .. } => {
                out.push(*kind);
                out.extend(default.iter().copied());
            }
            Exp::TypeParam { kind, .. } => out.push(*kind),
            Exp::FunctionTypeParam { attr, ty, name } => {
                out.extend([*attr, *ty]);
                out.extend(name.iter().copied());
            }
            Exp::FunctionType { attr, params, ret } => out.extend([*attr, *params, *ret]),
            Exp::Designator { name, value } => out.extend([*name, *value]),
            Exp::BitOffset { ty, field } | Exp::ByteOffset { ty, field } => {
                out.extend([*ty, *field]);
            }
            Exp::ScopedName { prefix, ident, .. } => {
                out.extend(prefix.iter().copied());
                out.push(*ident);
            }
            Exp::FunctionSpecifier { receiver, function } => {
                out.extend(receiver.iter().copied());
                out.push(*function);
            }
            Exp::EnumValue { ty, value } => out.extend([*ty, *value]),
            Exp::Binary(_, a, b) => out.extend([*a, *b]),
            Exp::FunctionCall { attr, function, args } => {
                out.extend([*attr, *function, *args]);
            }
            Exp::ArrayAccess { base, index } | Exp::ArrayAccessLValue { base, index } => {
                out.extend([*base, *index]);
            }
            Exp::MemberAccess { base, member } => out.extend([*base, *member]),
            Exp::Cast { ty, value } => out.extend([*ty, *value]),
            Exp::Mux { selector, cases } => out.extend([*selector, *cases]),
            Exp::FanOut { count, value } => out.extend([*count, *value]),
            Exp::Annotated { attr, stmt } => out.extend([*attr, *stmt]),
            Exp::Return(e) => out.extend(e.iter().copied()),
            Exp::DoWhile { attr, cond, body } => out.extend([*attr, *cond, *body]),
            Exp::UnrolledFor { var, limit, body } | Exp::StaticFor { var, limit, body } => {
                out.extend([*var, *limit, *body]);
            }
            Exp::RangeFor { attr, var, limit, body } => {
                out.extend([*attr, *var, *limit, *body]);
            }
            Exp::If { cond, then_branch, else_branch }
            | Exp::StaticIf { cond, then_branch, else_branch } => {
                out.extend([*cond, *then_branch]);
                out.extend(else_branch.iter().copied());
            }
            Exp::Switch { selector, blocks } => out.extend([*selector, *blocks]),
            Exp::SwitchCase { value, body } => out.extend([*value, *body]),
            Exp::Assign { lvalue, value } => out.extend([*lvalue, *value]),
            Exp::NestedScope { name, body, .. } => out.extend([*name, *body]),
            Exp::ModuleDecl { name, exposed, body } => {
                out.push(*name);
                out.extend(exposed.iter().copied());
                out.push(*body);
            }
            Exp::EnumConstant { name, value, .. } => {
                out.push(*name);
                out.extend(value.iter().copied());
            }
            Exp::Enum { name, ty, body, .. } => out.extend([*name, *ty, *body]),
            Exp::Struct { name, body, .. }
            | Exp::Union { name, body, .. }
            | Exp::Class { name, body, .. } => out.extend([*name, *body]),
            Exp::Template { params, decl } => {
                out.extend(params.iter().copied());
                out.push(*decl);
            }
            Exp::Alias { body, name, .. } => out.extend([*body, *name]),
            Exp::Variable { attr, ty, name, init, .. } => {
                out.extend([*attr, *ty, *name]);
                out.extend(init.iter().copied());
            }
            Exp::FuncParam { attr, ty, name, default, .. } => {
                out.extend([*attr, *ty, *name]);
                out.extend(default.iter().copied());
            }
            Exp::Function { attr, modifier, ret, name, params, body, .. } => {
                out.push(*attr);
                out.extend(modifier.iter().copied());
                out.extend([*ret, *name, *params, *body]);
            }
            Exp::FunctionDecl { attr, ret, name, params, .. } => {
                out.extend([*attr, *ret, *name, *params]);
            }
            Exp::Lambda { name, captures, params, ret, body, .. } => {
                out.extend([*name, *captures, *params, *ret, *body]);
            }
            Exp::ExportType { attr, ty } | Exp::Extern { attr, ty } => {
                out.extend([*attr, *ty]);
            }
            Exp::Path(p) => {
                for seg in &p.segments {
                    out.extend(seg.args.iter().copied());
                }
            }
        }
    }

    /// Every child id slot, mutably, in the same order as [`Exp::children`] —
    /// the functor half of it. [`crate::ExpArena::deep_copy`] repoints a
    /// clone's children through this, and `crate::rewrite`'s drivers rebuild a
    /// node's children through it, which is the whole of `fmap` over `ExpF`
    /// (`Parser/Syntax.hs:584` derives `Functor`).
    ///
    /// The two traversals are kept adjacent so that a review diffs them: they
    /// enumerate the same 95 variants in the same order with no wildcard arm,
    /// so a new variant breaks the build in both.
    #[allow(clippy::too_many_lines)]
    pub fn children_mut(&mut self) -> Vec<&mut ExpId> {
        let mut out: Vec<&mut ExpId> = Vec::new();
        match self {
            Exp::Seq(items) => out.extend(items.iter_mut()),
            Exp::FlagAttr(_)
            | Exp::FunctionModifier(_)
            | Exp::Auto
            | Exp::Void
            | Exp::Boolean
            | Exp::Float
            | Exp::Str
            | Exp::Integer(_)
            | Exp::Unsigned(_)
            | Exp::Typename
            | Exp::This { .. }
            | Exp::IntLiteral { .. }
            | Exp::FloatLiteral(_)
            | Exp::BoolLiteral(_)
            | Exp::StringLiteral(_)
            | Exp::InterpolatedString(_)
            | Exp::Identifier(_)
            | Exp::Barrier
            | Exp::ModuleIdentifier(_)
            | Exp::ModuleDiff(..)
            | Exp::Private
            | Exp::Public
            | Exp::Error(_)
            | Exp::Unsupported => {}
            Exp::Interpolated(segments) => {
                for h in segments.iter_mut().filter_map(|s| s.hole.as_mut()) {
                    out.push(&mut h.expr);
                    if let Some(a) = &mut h.align {
                        out.push(a);
                    }
                }
            }
            Exp::IntAttr(_, e)
            | Exp::Attr(_, e)
            | Exp::ParamInt(e)
            | Exp::ParamUint(e)
            | Exp::Const(e)
            | Exp::Reference(e)
            | Exp::InitializerList(e)
            | Exp::QualifiedIdentifier(e)
            | Exp::TypeIdentifier(_, e)
            | Exp::NamedValue(e)
            | Exp::Unary(_, e)
            | Exp::Concat(e)
            | Exp::Static(e)
            | Exp::Reorder(e)
            | Exp::SwitchDefault(e)
            | Exp::DefaultInitialization(e)
            | Exp::Decltype { expr: e, .. }
            | Exp::Capture { name: e, .. }
            | Exp::StaticAssert(e)
            | Exp::StaticAssertLegacy(e)
            | Exp::Warning { node: e, .. }
            | Exp::Positional { value: e, .. }
            | Exp::Labelled { value: e, .. } => out.push(e),
            Exp::Array { attr, elem, dims } => out.extend([attr, elem, dims]),
            Exp::TemplateInstance { base, args } => out.extend([base, args]),
            Exp::TemplateParam { kind, default, .. } => {
                out.push(kind);
                out.extend(default.iter_mut());
            }
            Exp::TypeParam { kind, .. } => out.push(kind),
            Exp::FunctionTypeParam { attr, ty, name } => {
                out.extend([attr, ty]);
                out.extend(name.iter_mut());
            }
            Exp::FunctionType { attr, params, ret } => out.extend([attr, params, ret]),
            Exp::Designator { name, value } => out.extend([name, value]),
            Exp::BitOffset { ty, field } | Exp::ByteOffset { ty, field } => {
                out.extend([ty, field]);
            }
            Exp::ScopedName { prefix, ident, .. } => {
                out.extend(prefix.iter_mut());
                out.push(ident);
            }
            Exp::FunctionSpecifier { receiver, function } => {
                out.extend(receiver.iter_mut());
                out.push(function);
            }
            Exp::EnumValue { ty, value } => out.extend([ty, value]),
            Exp::Binary(_, a, b) => out.extend([a, b]),
            Exp::FunctionCall { attr, function, args } => {
                out.extend([attr, function, args]);
            }
            Exp::ArrayAccess { base, index } | Exp::ArrayAccessLValue { base, index } => {
                out.extend([base, index]);
            }
            Exp::MemberAccess { base, member } => out.extend([base, member]),
            Exp::Cast { ty, value } => out.extend([ty, value]),
            Exp::Mux { selector, cases } => out.extend([selector, cases]),
            Exp::FanOut { count, value } => out.extend([count, value]),
            Exp::Annotated { attr, stmt } => out.extend([attr, stmt]),
            Exp::Return(e) => out.extend(e.iter_mut()),
            Exp::DoWhile { attr, cond, body } => out.extend([attr, cond, body]),
            Exp::UnrolledFor { var, limit, body } | Exp::StaticFor { var, limit, body } => {
                out.extend([var, limit, body]);
            }
            Exp::RangeFor { attr, var, limit, body } => {
                out.extend([attr, var, limit, body]);
            }
            Exp::If { cond, then_branch, else_branch }
            | Exp::StaticIf { cond, then_branch, else_branch } => {
                out.extend([cond, then_branch]);
                out.extend(else_branch.iter_mut());
            }
            Exp::Switch { selector, blocks } => out.extend([selector, blocks]),
            Exp::SwitchCase { value, body } => out.extend([value, body]),
            Exp::Assign { lvalue, value } => out.extend([lvalue, value]),
            Exp::NestedScope { name, body, .. } => out.extend([name, body]),
            Exp::ModuleDecl { name, exposed, body } => {
                out.push(name);
                out.extend(exposed.iter_mut());
                out.push(body);
            }
            Exp::EnumConstant { name, value, .. } => {
                out.push(name);
                out.extend(value.iter_mut());
            }
            Exp::Enum { name, ty, body, .. } => out.extend([name, ty, body]),
            Exp::Struct { name, body, .. }
            | Exp::Union { name, body, .. }
            | Exp::Class { name, body, .. } => out.extend([name, body]),
            Exp::Template { params, decl } => {
                out.extend(params.iter_mut());
                out.push(decl);
            }
            Exp::Alias { body, name, .. } => out.extend([body, name]),
            Exp::Variable { attr, ty, name, init, .. } => {
                out.extend([attr, ty, name]);
                out.extend(init.iter_mut());
            }
            Exp::FuncParam { attr, ty, name, default, .. } => {
                out.extend([attr, ty, name]);
                out.extend(default.iter_mut());
            }
            Exp::Function { attr, modifier, ret, name, params, body, .. } => {
                out.push(attr);
                out.extend(modifier.iter_mut());
                out.extend([ret, name, params, body]);
            }
            Exp::FunctionDecl { attr, ret, name, params, .. } => {
                out.extend([attr, ret, name, params]);
            }
            Exp::Lambda { name, captures, params, ret, body, .. } => {
                out.extend([name, captures, params, ret, body]);
            }
            Exp::ExportType { attr, ty } | Exp::Extern { attr, ty } => {
                out.extend([attr, ty]);
            }
            Exp::Path(p) => {
                for seg in &mut p.segments {
                    out.extend(seg.args.iter_mut());
                }
            }
        }
        out
    }

    /// Whether `self`, a node of one arena, is `other`, a node of another,
    /// but for the arenas' stamps: every field equal, each child the node
    /// at the same index. Two runs of the loop number a module's arena
    /// alike where they did the same to it (`kg_sem::snapshot`'s shadow).
    pub fn same_but_stamps(&self, other: &Exp) -> bool {
        let mut theirs = Vec::new();
        other.children(&mut theirs);
        let mut copy = self.clone();
        let mine = copy.children_mut();
        if mine.len() != theirs.len() {
            return false;
        }
        for (c, t) in mine.into_iter().zip(&theirs) {
            if c.index() != t.index() {
                return false;
            }
            *c = *t;
        }
        copy == *other
    }

    /// Hashes the node's own content — everything [`Exp::same_but_stamps`]
    /// compares but the children it names, each of which hashes alike. A
    /// reader that goes on to its children hashes them on their own (the
    /// loop's read door, `kg_sem::reads`).
    pub fn hash_shape<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        let mut copy = self.clone();
        for child in copy.children_mut() {
            *child = ExpId::PLACEHOLDER;
        }
        copy.hash(state);
    }
}

/// The value of a string token with no interpolation hole — what
/// `interpolatedStringPre` makes a `StringLiteralF` of
/// (`Frontend.hs:233-234`), decoded as `stringSegments` reads it
/// (`Parser.hs:885-893`: `{{` is one brace, every other character a
/// Haskell `charLiteral`). `None` for a token with a hole: an unpaired `{`.
pub fn plain_string_value(token: &str) -> Option<String> {
    let body = token.strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(token);
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'{' if bytes.get(i + 1) == Some(&b'{') => i += 2,
            b'{' => return None,
            _ => i += 1,
        }
    }
    match kg_const::ConstValue::parse_string_literal(token) {
        kg_const::ConstValue::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// A string value spelled as a hole-less token [`plain_string_value`] reads
/// back to the same value: quotes around it, a brace doubled so no hole
/// opens, and the characters `charLiteral` would not take bare escaped.
pub fn string_token(value: &str) -> SmolStr {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("{{"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push('\\');
                out.push_str(&(c as u32).to_string());
            }
            c => out.push(c),
        }
    }
    out.push('"');
    SmolStr::new(out)
}

#[cfg(test)]
mod string_tests {
    use super::{plain_string_value, string_token};

    /// A folded string minted as a token reads back to itself — through a
    /// quote, a backslash, a brace and a control character — and a token
    /// with a hole is not a plain string.
    #[test]
    fn a_minted_string_token_reads_back_to_its_value() {
        for value in ["mock", "a\"b", "c\\d", "{x}", "tab\there", "nl\n", "\u{1}"] {
            assert_eq!(plain_string_value(&string_token(value)).as_deref(), Some(value), "{value:?}");
        }
        assert_eq!(plain_string_value("\"{{x}}\"").as_deref(), Some("{x}}"));
        assert_eq!(plain_string_value("\"a {x} b\""), None);
        assert_eq!(plain_string_value("\"\\n\""), Some("\n".to_owned()));
    }
}
