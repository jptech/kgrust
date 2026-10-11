//! Owner-topic: **the `Type` algebraic data type** and everything that is pure
//! structure over it — the auxiliary types, the predicates, and the accessors
//! that walk through `TConst`/`TInstance`.
//!
//! Spec: `docs/spec/types.md` §1 (the type model), §14.2 (the Rust
//! representation), §15 traps 8, 12-15.
//! Reference: `Type.hs:117-215` (auxiliaries), `Type.hs:224-278` (`Type`),
//! `Type.hs:537-690` (predicates and accessors), `Type.hs:928-998` (element and
//! field access), `Type.hs:1000-1031` (`tResolved`).
//!
//! # `Ord` is structural, and that is deliberate
//!
//! Haskell derives `Eq, Ord, Show` on `Type` (`Type.hs:278`) with no interning.
//! `tSuper`, `<:`, `Data.Set TAttr` and the template-instance `Map` all depend on
//! that order. [`TyRef`] is therefore an `Arc<Ty>` whose `Eq`/`Ord`/`Hash`
//! delegate to the pointee — structural, exactly as the reference — with an
//! `Arc::ptr_eq` fast path for the common case where two references share an
//! allocation (a clone, or a walker that keeps an untouched `TyRef`). Sharing
//! buys a fast path; it does **not** turn equality into pointer equality,
//! because it cannot: see [`TFuncParam`].

use std::sync::Arc;

use kg_base::{Interner, SmolStr, Symbol, SymbolPath};
use kg_const::ConstInt;

/// `type Name = kg_base::Symbol` — Haskell `newtype Name = Name (Hashed String)`
/// (`Parser/Syntax.hs:51`).
pub type Name = Symbol;

/// Haskell `type QualifiedName = [Name]` (`Parser/Syntax.hs:69`).
pub type QualifiedName = SymbolPath;

/// `data FunctionKind` (`Parser/Syntax.hs:205-208`).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FunctionKind {
    FreeFunction,
    MemberFunction,
    LambdaFunction,
}

/// `data Attribute` — the nine `[[…(x)]]` attributes (`Parser/Syntax.hs:124-133`).
///
/// Variant order is the Haskell declaration order, which is what the derived
/// `Ord` — and therefore [`TAttrSet`]'s iteration order — uses.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Attribute {
    CallRate,
    Ecc,
    FifoDepth,
    Latency,
    MaxThreads,
    /// Spelled `name`, not `rename` (`Parser/Syntax.hs:142`).
    Rename,
    Schedule,
    ThreadRate,
    TransactionSize,
}

impl Attribute {
    /// The source spelling (`Parser/Syntax.hs:137-145`).
    pub fn text(self) -> &'static str {
        match self {
            Attribute::CallRate => "call_rate",
            Attribute::Ecc => "ecc",
            Attribute::FifoDepth => "fifo_depth",
            Attribute::Latency => "latency",
            Attribute::MaxThreads => "max_threads",
            Attribute::Rename => "name",
            Attribute::Schedule => "schedule",
            Attribute::ThreadRate => "thread_rate",
            Attribute::TransactionSize => "transaction_size",
        }
    }
}

/// `data Flag` — the thirteen valueless `[[…]]` attributes
/// (`Parser/Syntax.hs:150-164`). Declaration order again drives `Ord`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Flag {
    Async,
    Atomic,
    /// Spelled `last` (`Parser/Syntax.hs:169`).
    EndTransaction,
    Initialize,
    Memory,
    NoBackPressure,
    Pure,
    NonReplicated,
    Pipelined,
    QuadPort,
    ReorderByLooping,
    Reset,
    Unordered,
}

impl Flag {
    /// The source spelling (`Parser/Syntax.hs:167-179`).
    pub fn text(self) -> &'static str {
        match self {
            Flag::Async => "async",
            Flag::Atomic => "atomic",
            Flag::EndTransaction => "last",
            Flag::Initialize => "initialize",
            Flag::Memory => "memory",
            Flag::NoBackPressure => "no_backpressure",
            Flag::Pure => "pure",
            Flag::NonReplicated => "non_replicated",
            Flag::Pipelined => "pipelined",
            Flag::QuadPort => "quad_port",
            Flag::ReorderByLooping => "reorder_by_looping",
            Flag::Reset => "reset",
            Flag::Unordered => "unordered",
        }
    }
}

/// `data TAttr` (`Type.hs:188-195`).
///
/// Haskell names the third constructor `TAttr` as well; here it is [`TAttr::Named`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TAttr {
    /// `TFlagAttr Flag`
    Flag(Flag),
    /// `TIntAttr Attribute Integer`
    Int(Attribute, ConstInt),
    /// `TAttr Attribute QualifiedName Type` — the `[[ecc(f)]]` shape, where the
    /// payload is a *function* whose return type replaces the element type on the
    /// last dimension (`Type.hs:930-935`).
    Named(Attribute, QualifiedName, TyRef),
}

impl TAttr {
    /// `attrResolved` (`Type.hs:1029-1031`): only the `Named` form can be
    /// unresolved.
    pub fn resolved(&self) -> bool {
        match self {
            TAttr::Named(_, _, ty) => ty.resolved(),
            _ => true,
        }
    }
}

/// `Set TAttr`. **Must** be ordered and deduplicated: `<:` compares attribute
/// sets by subset (`Type.hs:333`, `Type.hs:344`) and the derived `Ord` on `Ty`
/// walks them in order. A `HashSet` would break both.
pub type TAttrSet = std::collections::BTreeSet<TAttr>;

/// `data TArg` (`Type.hs:206-215`) — a bound template argument.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TArg {
    /// `TArg String` — the fallback: the pretty-printed expression
    /// (`Type.hs:437`).
    Expr(SmolStr),
    /// `TStrArg String`
    Str(SmolStr),
    /// `TTypeArg Type`
    Type(TyRef),
    /// `TIntArg Integer`
    Int(ConstInt),
    /// `TNameArg Type QualifiedName`
    Name(TyRef, QualifiedName),
}

/// `data TFuncParam` (`Type.hs:117-124`).
///
/// # The `Eq`/`Ord` inconsistency is reproduced on purpose
///
/// `instance Eq TFuncParam` compares only the type and the attributes and
/// **ignores the name** (`Type.hs:126-129`), while `Ord` is *derived* and does
/// include it (`Type.hs:123`). Rust's `Ord` contract asks for consistency with
/// `Eq`; violating it here is deliberate, because function-type equality is
/// name-insensitive (that is what makes a declaration match its definition) while
/// ordered containers must not silently merge two parameters that differ only by
/// name. Haskell's `Data.Set`/`Data.Map` behave the same way, so reproducing the
/// inconsistency reproduces the reference.
///
/// `Hash` follows `Eq` (name excluded), as Rust requires.
///
/// **Consequence for interning**: a hash-consing interner keyed on `Eq`/`Hash`
/// would collapse `(uint8 x)` and `(uint8 y)` into one type and lose the
/// parameter name. One that is ever written must key on `Ord`, which includes
/// the name.
#[derive(Clone, Debug)]
pub struct TFuncParam {
    pub attrs: TAttrSet,
    pub ty: TyRef,
    pub name: Option<Name>,
}

impl PartialEq for TFuncParam {
    /// `Type.hs:126-129` — name-insensitive.
    fn eq(&self, other: &TFuncParam) -> bool {
        self.ty == other.ty && self.attrs == other.attrs
    }
}

impl Eq for TFuncParam {}

impl std::hash::Hash for TFuncParam {
    /// Consistent with [`PartialEq`]: the name is excluded — but inside
    /// [`hash_strict`], which hashes as `Ord` compares.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.attrs.hash(state);
        self.ty.hash(state);
        if STRICT.with(std::cell::Cell::get) {
            self.name.hash(state);
        }
    }
}

thread_local! {
    /// Whether [`hash_strict`] is hashing on this thread.
    static STRICT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `ty` hashed as `Ord` compares it — a parameter's name included, which
/// `Hash` (consistent with `==`) leaves out: two types `Ord` calls equal
/// hash alike, and two that differ only in a parameter's name almost never
/// do. For a key that must tell `(uint8 x)` from `(uint8 y)` (the loop's
/// read door, `kg_sem::reads`).
pub fn hash_strict<H: std::hash::Hasher>(ty: &Ty, state: &mut H) {
    use std::hash::Hash;
    let outer = STRICT.with(|s| s.replace(true));
    ty.hash(state);
    STRICT.with(|s| s.set(outer));
}

impl PartialOrd for TFuncParam {
    fn partial_cmp(&self, other: &TFuncParam) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TFuncParam {
    /// Derived field order in Haskell is `attrOfParam, typeOfParam, nameOfParam`
    /// (`Type.hs:119-123`), and the name **is** part of it.
    fn cmp(&self, other: &TFuncParam) -> std::cmp::Ordering {
        self.attrs
            .cmp(&other.attrs)
            .then_with(|| self.ty.cmp(&other.ty))
            .then_with(|| self.name.cmp(&other.name))
    }
}

/// The Kanagawa type. Mirrors `Type.hs:224-277` one-for-one, **in declaration
/// order**, because `Ord` is derived on both sides and the order of the variants
/// is the order of the comparison.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ty {
    /// `TAuto` (`Type.hs:225`). Counts as *resolved* (`Type.hs:1001`) — trap.
    Auto,
    /// `TTemplate` (`Type.hs:227`) — a template *name*, not an instance.
    Template,
    /// `TType Type` (`Type.hs:229`) — "this expression is a type".
    Type(TyRef),
    /// `TUndefined` (`Type.hs:231`) — symbol lookup failed.
    Undefined,
    /// `TUntyped` (`Type.hs:233`) — no type by construction (statements, attrs,
    /// identifiers; complete list in `docs/spec/types.md` §1.5).
    Untyped,
    /// `TUnresolved` (`Type.hs:235`) — not known *this round*. The initial note
    /// on every node (`Type.hs:970-972`).
    Unresolved,
    /// `TInitializer [Type]` (`Type.hs:237`).
    Initializer(Vec<TyRef>),
    /// `TDesignator Name Type` (`Type.hs:239`) — one `.f = e`.
    Designator(Name, TyRef),
    /// `TPositional Int Type` (`Type.hs:241`).
    Positional(u32, TyRef),
    /// `TVoid` (`Type.hs:243`).
    Void,
    /// `TBoolean` (`Type.hs:245`), width 1.
    Boolean,
    /// `TFloat` (`Type.hs:247`), always 32-bit.
    Float,
    /// `TString` (`Type.hs:249`). **Width 0** (`Type.hs:705`) — trap.
    Str,
    /// `TSigned Int` (`Type.hs:251`) — `intN`.
    Signed(u32),
    /// `TUnsigned Int` (`Type.hs:253`) — `uintN`.
    Unsigned(u32),
    /// `TDependent Type` (`Type.hs:255`) — the return type of a `[[pipelined]]`
    /// function, elaborated at the call site (`Type.hs:1368-1378`).
    Dependent(TyRef),
    /// `TConst Type` (`Type.hs:257`).
    Const(TyRef),
    /// `TReference QualifiedName` (`Type.hs:259`) — only ever produced for
    /// `this`. **Width 1** (`Type.hs:710`) — trap.
    Reference(QualifiedName),
    /// `TArray (Set TAttr) Type [Integer]` (`Type.hs:261`).
    ///
    /// Dimensions are `[Integer]` in the reference, so they stay arbitrary
    /// precision here — `docs/spec/types.md` §14.2 says "keep them wide" and then
    /// writes `Vec<u64>`; the prose wins.
    Array { attrs: TAttrSet, elem: TyRef, dims: Vec<ConstInt> },
    /// `TFunction FunctionKind (Set TAttr) Type [TFuncParam]` (`Type.hs:263`).
    Function { kind: FunctionKind, attrs: TAttrSet, ret: TyRef, params: Vec<TFuncParam> },
    /// `TClosure QualifiedName Type Type QualifiedName` (`Type.hs:265`):
    /// lambda-function name, function type, capture type, closure-alias name.
    Closure { func: QualifiedName, func_ty: TyRef, capture: TyRef, alias: QualifiedName },
    /// `TEnum QualifiedName Type` (`Type.hs:267`).
    Enum(QualifiedName, TyRef),
    /// `TStruct QualifiedName [(Name, Type)]` (`Type.hs:269`).
    Struct(QualifiedName, Vec<(Name, TyRef)>),
    /// `TUnion QualifiedName [(Name, Type)]` (`Type.hs:271`).
    Union(QualifiedName, Vec<(Name, TyRef)>),
    /// `TClass QualifiedName [(Name, Type)]` (`Type.hs:273`) — member *variables*
    /// only; methods live in the symbol map (`Type.hs:1358-1362`).
    Class(QualifiedName, Vec<(Name, TyRef)>),
    /// `TInstance Type QualifiedName [(Name, TArg)]` (`Type.hs:275`).
    Instance { concrete: TyRef, template: QualifiedName, args: Vec<(Name, TArg)> },
    /// `TError String` (`Type.hs:277`). Counts as **resolved** (`Type.hs:1015`),
    /// so an error terminates the fixpoint instead of looping — trap 26.
    Error(SmolStr),
}

/// The `fn` argument of `tLookupField` (`Type.hs:961`) — `lookup name` or
/// `maybeAt index` over an aggregate's ordered field list.
type FieldSelector<'a> = dyn Fn(&[(Name, TyRef)]) -> Option<Ty> + 'a;

/// A shared, structurally-compared handle on a [`Ty`].
///
/// `Eq`, `Ord` and `Hash` all delegate to the pointee. Cloning is one atomic
/// increment.
#[derive(Clone)]
pub struct TyRef(Arc<Ty>);

impl TyRef {
    pub fn new(ty: Ty) -> TyRef {
        TyRef(Arc::new(ty))
    }

    #[inline]
    pub fn get(&self) -> &Ty {
        &self.0
    }
}

impl std::ops::Deref for TyRef {
    type Target = Ty;

    #[inline]
    fn deref(&self) -> &Ty {
        &self.0
    }
}

impl From<Ty> for TyRef {
    fn from(ty: Ty) -> TyRef {
        TyRef::new(ty)
    }
}

impl PartialEq for TyRef {
    #[inline]
    fn eq(&self, other: &TyRef) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl Eq for TyRef {}

impl PartialOrd for TyRef {
    fn partial_cmp(&self, other: &TyRef) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TyRef {
    #[inline]
    fn cmp(&self, other: &TyRef) -> std::cmp::Ordering {
        if Arc::ptr_eq(&self.0, &other.0) {
            std::cmp::Ordering::Equal
        } else {
            self.0.cmp(&other.0)
        }
    }
}

impl std::hash::Hash for TyRef {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl std::fmt::Debug for TyRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// --- predicates --------------------------------------------------------------

impl Ty {
    /// `tInner` (`Type.hs:598-601`): the one level `maybeInner` looks through.
    /// **Only** `TConst` and `TInstance` — not `TType`, not `TEnum`.
    pub fn inner(&self) -> Option<&Ty> {
        match self {
            Ty::Const(x) => Some(x),
            Ty::Instance { concrete, .. } => Some(concrete),
            _ => None,
        }
    }

    fn maybe_inner(&self, f: impl Fn(&Ty) -> bool) -> bool {
        self.inner().is_some_and(f)
    }

    /// `tIsUndefined` (`Type.hs:610`) — no `maybeInner`.
    pub fn is_undefined(&self) -> bool {
        matches!(self, Ty::Undefined)
    }

    /// `tIsConst` (`Type.hs:613`) — no `maybeInner`.
    pub fn is_const(&self) -> bool {
        matches!(self, Ty::Const(_))
    }

    /// `tIsTemplate` (`Type.hs:616`) — no `maybeInner`.
    pub fn is_template(&self) -> bool {
        matches!(self, Ty::Template)
    }

    /// `tIsInstance` (`Type.hs:661`) — no `maybeInner`.
    pub fn is_instance(&self) -> bool {
        matches!(self, Ty::Instance { .. })
    }

    /// `tIsDesignator` (`Type.hs:673`) — no `maybeInner`.
    pub fn is_designator(&self) -> bool {
        matches!(self, Ty::Designator(..))
    }

    pub fn is_type(&self) -> bool {
        matches!(self, Ty::Type(_)) || self.maybe_inner(Ty::is_type)
    }

    pub fn is_reference(&self) -> bool {
        matches!(self, Ty::Reference(_)) || self.maybe_inner(Ty::is_reference)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Ty::Error(_)) || self.maybe_inner(Ty::is_error)
    }

    pub fn is_void(&self) -> bool {
        matches!(self, Ty::Void) || self.maybe_inner(Ty::is_void)
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Ty::Str) || self.maybe_inner(Ty::is_string)
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Ty::Float) || self.maybe_inner(Ty::is_float)
    }

    pub fn is_class(&self) -> bool {
        matches!(self, Ty::Class(..)) || self.maybe_inner(Ty::is_class)
    }

    pub fn is_struct(&self) -> bool {
        matches!(self, Ty::Struct(..)) || self.maybe_inner(Ty::is_struct)
    }

    pub fn is_union(&self) -> bool {
        matches!(self, Ty::Union(..)) || self.maybe_inner(Ty::is_union)
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Ty::Boolean) || self.maybe_inner(Ty::is_bool)
    }

    pub fn is_enum(&self) -> bool {
        matches!(self, Ty::Enum(..)) || self.maybe_inner(Ty::is_enum)
    }

    pub fn is_signed(&self) -> bool {
        matches!(self, Ty::Signed(_)) || self.maybe_inner(Ty::is_signed)
    }

    pub fn is_unsigned(&self) -> bool {
        matches!(self, Ty::Unsigned(_)) || self.maybe_inner(Ty::is_unsigned)
    }

    /// `tIsInt = tIsUnsigned || tIsSigned` (`Type.hs:663`). Note that `TBoolean`
    /// is **not** an integer here.
    pub fn is_int(&self) -> bool {
        self.is_unsigned() || self.is_signed()
    }

    pub fn is_function(&self) -> bool {
        matches!(self, Ty::Function { .. }) || self.maybe_inner(Ty::is_function)
    }

    /// `tIsMethod` (`Type.hs:658-659`).
    pub fn is_method(&self) -> bool {
        match self {
            Ty::Function { kind, .. } => *kind == FunctionKind::MemberFunction,
            _ => self.maybe_inner(Ty::is_method),
        }
    }

    /// `tIsArrayOf f` (`Type.hs:687-689`) — note it recurses into the element
    /// type as well as through `tInner`.
    pub fn is_array_of(&self, f: &impl Fn(&Ty) -> bool) -> bool {
        match self {
            Ty::Array { elem, .. } => f(elem) || elem.is_array_of(f),
            _ => self.inner().is_some_and(|i| i.is_array_of(f)),
        }
    }

    /// `tIsArray = tIsArrayOf (const True)` (`Type.hs:671`).
    pub fn is_array(&self) -> bool {
        self.is_array_of(&|_| true)
    }

    /// `tIsEccMem` (`Type.hs:665-669`).
    pub fn is_ecc_mem(&self) -> bool {
        match self {
            Ty::Array { attrs, .. } => {
                attrs.iter().any(|a| matches!(a, TAttr::Named(Attribute::Ecc, _, _)))
            }
            _ => self.maybe_inner(Ty::is_ecc_mem),
        }
    }

    /// `tIsDesignatedInitializer` (`Type.hs:676-678`).
    pub fn is_designated_initializer(&self) -> bool {
        match self {
            Ty::Initializer(ts) => ts.iter().all(|t| t.is_designator()),
            _ => false,
        }
    }

    /// `tIsValueType` (`Type.hs:686`). A closure — an instance of
    /// `closure_template`, [`Ty::closure_template_path`] — is one: a hole
    /// may interpolate a lambda (`test/logic/string.k:802`).
    pub fn is_value_type(&self, closure_template: &QualifiedName) -> bool {
        self.is_closure(closure_template)
            || self.is_bool()
            || self.is_int()
            || self.is_float()
            || self.is_enum()
            || self.is_string()
            || self.is_struct()
            || self.is_union()
            || self.is_array_of(&|t: &Ty| t.is_value_type(closure_template))
    }

    /// The mangled qualified name of `data.closure.core::Closure`
    /// (`Type.hs:115-119`, `library/data/closure/core.k:9-10`). Interned on
    /// demand because a [`SymbolPath`] needs an [`Interner`].
    pub fn closure_template_path(interner: &Interner) -> QualifiedName {
        SymbolPath::new([interner.intern("@data@closure@core"), interner.intern("Closure")])
    }

    /// `tIsClosure` (`Type.hs:618-619`): a `TInstance` whose *template* is
    /// `data.closure.core::Closure`. Takes the template path rather than an
    /// interner so it stays allocation-free in a hot loop.
    pub fn is_closure(&self, closure_template: &QualifiedName) -> bool {
        match self {
            Ty::Instance { template, .. } => template == closure_template,
            _ => self.inner().is_some_and(|i| i.is_closure(closure_template)),
        }
    }

    /// `tResolved` (`Type.hs:1000-1027`) — the fixpoint's termination measure.
    ///
    /// Two traps live here: [`Ty::Auto`] is resolved and [`Ty::Error`] is
    /// resolved (`docs/spec/types.md` §15 trap 26).
    pub fn resolved(&self) -> bool {
        match self {
            Ty::Auto | Ty::Untyped | Ty::Void | Ty::Boolean | Ty::Float | Ty::Str => true,
            Ty::Signed(_) | Ty::Unsigned(_) | Ty::Error(_) | Ty::Reference(_) => true,
            Ty::Template | Ty::Undefined | Ty::Unresolved => false,
            Ty::Initializer(xs) => xs.iter().all(|x| x.resolved()),
            Ty::Designator(_, x) | Ty::Positional(_, x) => x.resolved(),
            Ty::Type(x) | Ty::Dependent(x) | Ty::Const(x) => x.resolved(),
            Ty::Array { attrs, elem, dims } => {
                elem.resolved() && !dims.is_empty() && attrs.iter().all(TAttr::resolved)
            }
            Ty::Function { ret, params, .. } => {
                ret.resolved() && params.iter().all(|p| p.ty.resolved())
            }
            Ty::Closure { func_ty, capture, .. } => func_ty.resolved() && capture.resolved(),
            Ty::Enum(_, x) => x.resolved(),
            Ty::Struct(_, fs) | Ty::Union(_, fs) | Ty::Class(_, fs) => {
                fs.iter().all(|(_, t)| t.resolved())
            }
            Ty::Instance { concrete, .. } => concrete.resolved(),
        }
    }
}

// --- accessors ---------------------------------------------------------------
//
// Each of these is partial in the reference (`undefined` on a mismatch). The
// Rust versions return `Option`, and callers reproduce the reference's behaviour
// by treating `None` the way the surrounding clause guard does. Where the
// reference produces a `TError` rather than `undefined`, so does this.

impl Ty {
    /// `tConcrete` (`Type.hs:594-595`): strip `TInstance` wrappers.
    pub fn concrete(&self) -> &Ty {
        match self {
            Ty::Instance { concrete, .. } => concrete.concrete(),
            other => other,
        }
    }

    /// `tUnConst` (`Type.hs:588-592`). Note it descends through `TType` and
    /// `TInstance` but is **not** recursive on `TConst`.
    pub fn un_const(&self) -> Ty {
        match self {
            Ty::Const(x) => (**x).clone(),
            Ty::Type(x) => Ty::Type(TyRef::new(x.un_const())),
            Ty::Instance { concrete, template, args } => Ty::Instance {
                concrete: TyRef::new(concrete.un_const()),
                template: template.clone(),
                args: args.clone(),
            },
            other => other.clone(),
        }
    }

    /// `tQualifiedName` (`Type.hs:504-513`). `undefined` in the reference for
    /// anything else.
    pub fn qualified_name(&self) -> Option<&QualifiedName> {
        match self {
            Ty::Closure { alias, .. } => Some(alias),
            Ty::Class(x, _) | Ty::Struct(x, _) | Ty::Union(x, _) => Some(x),
            Ty::Enum(x, _) | Ty::Reference(x) => Some(x),
            Ty::Instance { concrete, .. } => concrete.qualified_name(),
            Ty::Const(x) => x.qualified_name(),
            _ => None,
        }
    }

    /// `tDimension` (`Type.hs:537-541`) — the *first* dimension.
    pub fn dimension(&self) -> Option<&ConstInt> {
        match self {
            Ty::Instance { .. } => self.concrete().dimension(),
            Ty::Const(x) => x.dimension(),
            Ty::Array { dims, .. } => dims.first(),
            _ => None,
        }
    }

    /// `tFunctionParams` (`Type.hs:526-531`). A closure drops the **first**
    /// parameter (the capture) — `tail` at `Type.hs:528`, which throws on an
    /// empty list; an empty slice is returned instead.
    pub fn function_params(&self) -> Option<&[TFuncParam]> {
        match self {
            Ty::Function { params, .. } => Some(params),
            Ty::Closure { func_ty, .. } => {
                func_ty.function_params().map(|p| p.get(1..).unwrap_or(&[]))
            }
            Ty::Instance { concrete, .. } => concrete.function_params(),
            Ty::Const(x) => x.function_params(),
            _ => None,
        }
    }

    /// `tFunctionAttr` (`Type.hs:520-524`).
    pub fn function_attrs(&self) -> Option<&TAttrSet> {
        match self {
            Ty::Function { attrs, .. } => Some(attrs),
            Ty::Instance { concrete, .. } => concrete.function_attrs(),
            Ty::Const(x) => x.function_attrs(),
            _ => None,
        }
    }

    /// `tEnumBase` (`Type.hs:557-561`).
    pub fn enum_base(&self) -> Ty {
        match self {
            Ty::Enum(_, x) => (**x).clone(),
            Ty::Const(x) | Ty::Instance { concrete: x, .. } => x.enum_base(),
            _ => Ty::Error(SmolStr::new_static("Expected an enum")),
        }
    }

    /// `tReturn` (`Type.hs:578-586`). A function returning `TAuto` reports
    /// `TUnresolved`, not `TAuto`.
    pub fn return_ty(&self) -> Ty {
        match self {
            Ty::Function { ret, .. } => {
                if matches!(**ret, Ty::Auto) { Ty::Unresolved } else { (**ret).clone() }
            }
            Ty::Closure { func_ty: x, .. } => x.return_ty(),
            Ty::Instance { concrete: x, .. } | Ty::Const(x) => x.return_ty(),
            x @ Ty::Error(_) => x.clone(),
            x if !x.resolved() => Ty::Unresolved,
            _ => Ty::Error(SmolStr::new_static("Undefined function")),
        }
    }

    /// `tSigned` (`Type.hs:551-555`): the signed type that can hold this one.
    /// `uintN` widens to `int(N+1)`.
    pub fn to_signed(&self) -> Ty {
        match self {
            Ty::Signed(_) => self.clone(),
            Ty::Unsigned(n) => Ty::Signed(n + 1),
            x if !x.resolved() => Ty::Unresolved,
            _ => Ty::Error(SmolStr::new_static("Expected an integer")),
        }
    }

    /// `tConst` (`Type.hs:571-576`) — the smart constructor, which refuses to
    /// double-wrap and pushes through `TType`.
    pub fn to_const(&self) -> Ty {
        match self {
            Ty::Const(_) | Ty::Error(_) => self.clone(),
            Ty::Type(x) => Ty::Type(TyRef::new(x.to_const())),
            x if !x.resolved() => Ty::Unresolved,
            x => Ty::Const(TyRef::new(x.clone())),
        }
    }

    /// `theType` (`Type.hs:453-460`): unwrap a `TType`, or explain why not.
    pub fn the_type(&self) -> Ty {
        match self {
            Ty::Type(x) => (**x).clone(),
            x if !x.resolved() => Ty::Unresolved,
            _ => Ty::Error(SmolStr::new_static("Expected a type")),
        }
    }

    /// `tElement` (`Type.hs:928-943`) — indexing an array.
    ///
    /// Trap 14: on the **last** dimension of an ECC memory the result is the ECC
    /// function's *return* type, not the element type (`Type.hs:930-935`).
    pub fn element(&self, names: Option<&Interner>) -> Ty {
        match self {
            Ty::Array { elem, dims, .. } if dims.is_empty() => (**elem).clone(),
            Ty::Array { attrs, elem, dims } if dims.len() == 1 => attrs
                .iter()
                .find_map(|a| match a {
                    TAttr::Named(Attribute::Ecc, _, f) => Some(f.return_ty()),
                    _ => None,
                })
                .unwrap_or_else(|| (**elem).clone()),
            Ty::Array { attrs, elem, dims } => Ty::Array {
                attrs: attrs.clone(),
                elem: elem.clone(),
                dims: dims[1..].to_vec(),
            },
            Ty::Const(a) => Ty::Const(TyRef::new(a.element(names))),
            Ty::Instance { concrete, .. } => concrete.element(names),
            Ty::Type(_) => {
                Ty::Error(SmolStr::new_static("Invalid element access; did you forget `typename`"))
            }
            x if !x.resolved() => Ty::Unresolved,
            x if !x.is_array() => Ty::Error(SmolStr::new(format!(
                "Invalid element access on non-array type `{}`",
                crate::subtype::show_pretty(x, names)
            ))),
            _ => Ty::Error(SmolStr::new_static("Invalid element access")),
        }
    }

    /// `tElementLValue` (`Type.hs:945-957`).
    ///
    /// Trap 15: the `TConst` clause delegates to [`Ty::element`], **not** to
    /// itself, so the ECC substitution leaks onto the lvalue path for `const`
    /// memories (`Type.hs:949`). Reproduced verbatim.
    pub fn element_lvalue(&self, names: Option<&Interner>) -> Ty {
        match self {
            Ty::Array { elem, dims, .. } if dims.len() <= 1 => (**elem).clone(),
            Ty::Array { attrs, elem, dims } => Ty::Array {
                attrs: attrs.clone(),
                elem: elem.clone(),
                dims: dims[1..].to_vec(),
            },
            // `tElementLValue (TConst a) = TConst $ tElement a` — Type.hs:949.
            Ty::Const(a) => Ty::Const(TyRef::new(a.element(names))),
            Ty::Instance { concrete, .. } => concrete.element_lvalue(names),
            Ty::Type(_) => {
                Ty::Error(SmolStr::new_static("Invalid element access; did you forget `typename`"))
            }
            x if !x.resolved() => Ty::Unresolved,
            x if !x.is_array() => Ty::Error(SmolStr::new(format!(
                "Invalid element access on non-array type `{}`",
                crate::subtype::show_pretty(x, names)
            ))),
            _ => Ty::Error(SmolStr::new_static("Invalid element access")),
        }
    }

    /// The field list of a struct / union / class, looking through `TConst` and
    /// `TInstance` — the shape `tLookupField` walks (`Type.hs:961-971`).
    ///
    /// `tLookupField (TReference _) _ = undefined` (`Type.hs:965`) — trap 12.
    /// Here that is `None`, and a caller that can actually reach it should
    /// dereference through the symbol map first, exactly as `MemberAccessF` does
    /// (`Type.hs:1232-1235`).
    pub fn fields(&self) -> Option<&[(Name, TyRef)]> {
        match self {
            Ty::Struct(_, fs) | Ty::Union(_, fs) | Ty::Class(_, fs) => Some(fs),
            Ty::Const(a) | Ty::Instance { concrete: a, .. } => a.fields(),
            _ => None,
        }
    }

    /// `tLookupField fn` (`Type.hs:961-971`), parameterised by how a field is
    /// selected from the list — `lookup name` for [`Ty::field`], `maybeAt index`
    /// for [`Ty::field_at`].
    ///
    /// The `TConst` clause is `TConst <$> member a x`: a *miss* stays a miss, and
    /// a hit is re-wrapped in `TConst`. Reproducing that shape is why this returns
    /// `Option<Ty>` rather than folding the default in.
    fn lookup_field(&self, select: &FieldSelector<'_>, names: Option<&Interner>) -> Option<Ty> {
        match self {
            Ty::Struct(_, fs) | Ty::Union(_, fs) | Ty::Class(_, fs) => select(fs),
            Ty::Const(a) => a.lookup_field(select, names).map(|t| Ty::Const(TyRef::new(t))),
            // `tLookupField (TReference _) _ = undefined` (Type.hs:965) — trap 12.
            Ty::Reference(_) => None,
            Ty::Instance { concrete, .. } => concrete.lookup_field(select, names),
            a if !a.resolved() => Some(Ty::Unresolved),
            a => Some(Ty::Error(SmolStr::new(format!(
                "Invalid member access on type `{}`",
                crate::subtype::show_pretty(a, names)
            )))),
        }
    }

    /// `tField` (`Type.hs:959`): the type of member `name`, or `TUndefined` when
    /// there is no such member.
    pub fn field(&self, name: Name, names: Option<&Interner>) -> Ty {
        self.lookup_field(&|fs| fs.iter().find(|(n, _)| *n == name).map(|(_, t)| (**t).clone()), names)
            .unwrap_or(Ty::Undefined)
    }

    /// `tFieldAt` (`Type.hs:281-283`): the *n*-th field of an aggregate, or
    /// ``TError "Excess element in initializer of type t"`` — no backticks
    /// around the type there, unlike every other message in this file.
    pub fn field_at(&self, index: usize, names: Option<&Interner>) -> Ty {
        self.lookup_field(&|fs| fs.get(index).map(|(_, t)| (**t).clone()), names)
            .unwrap_or_else(|| Ty::Error(SmolStr::new(format!(
                "Excess element in initializer of type {}",
                crate::subtype::show_pretty(self, names)
            ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(t: Ty) -> TyRef {
        TyRef::new(t)
    }

    /// `tIsValueType` (`Type.hs:686`) takes a closure, `const` or in an
    /// array: `"{fn}"` over a lambda compiles (`test/logic/string.k:802`).
    /// Red with the `tIsClosure` arm left out.
    #[test]
    fn a_closure_is_a_value_type() {
        let i = Interner::new();
        let closure = Ty::closure_template_path(&i);
        let c = Ty::Instance { concrete: r(Ty::Class(SymbolPath::new([i.intern("C")]), Vec::new())), template: closure.clone(), args: Vec::new() };
        assert!(c.is_value_type(&closure));
        assert!(Ty::Const(r(c.clone())).is_value_type(&closure));
        let array = Ty::Array { attrs: TAttrSet::new(), elem: r(c.clone()), dims: vec![ConstInt::from_i128(2)] };
        assert!(array.is_value_type(&closure));
        let other = SymbolPath::new([i.intern("Box")]);
        let boxed = Ty::Instance { concrete: r(Ty::Class(SymbolPath::new([i.intern("C")]), Vec::new())), template: other, args: Vec::new() };
        assert!(!boxed.is_value_type(&closure));
    }

    #[test]
    fn eq_on_a_func_param_ignores_the_name() {
        // Type.hs:126-129.
        let i = Interner::new();
        let a = TFuncParam { attrs: TAttrSet::new(), ty: r(Ty::Unsigned(8)), name: Some(i.intern("x")) };
        let b = TFuncParam { attrs: TAttrSet::new(), ty: r(Ty::Unsigned(8)), name: Some(i.intern("y")) };
        assert_eq!(a, b);
        // ... while Ord does not (Type.hs:123).
        assert_ne!(a.cmp(&b), std::cmp::Ordering::Equal);
    }

    /// [`hash_strict`] hashes as `Ord` compares: a parameter's name in,
    /// where `Hash` leaves it out; and the flag is off again after it.
    /// Red with the name left out of the strict hash, or the flag left on.
    #[test]
    fn a_strict_hash_tells_a_parameter_s_name() {
        use std::hash::{Hash, Hasher};
        let i = Interner::new();
        let mk = |n: &str| Ty::Function {
            kind: FunctionKind::FreeFunction,
            attrs: TAttrSet::new(),
            ret: r(Ty::Void),
            params: vec![TFuncParam { attrs: TAttrSet::new(), ty: r(Ty::Unsigned(8)), name: Some(i.intern(n)) }],
        };
        let plain = |t: &Ty| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            t.hash(&mut h);
            h.finish()
        };
        let strict = |t: &Ty| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            hash_strict(t, &mut h);
            h.finish()
        };
        let (x, y) = (mk("x"), mk("y"));
        assert_eq!(plain(&x), plain(&y));
        assert_ne!(strict(&x), strict(&y));
        assert_eq!(strict(&x), strict(&mk("x")));
        assert_eq!(plain(&x), plain(&y), "the flag is off after a strict hash");
    }

    #[test]
    fn equal_function_types_keep_their_parameter_names() {
        // Why an interner keyed on `Eq` would lose `y`: the two compare equal
        // (names ignored), and each still carries its own name; only `Ord`
        // tells them apart.
        let i = Interner::new();
        let mk = |n: &str| {
            r(Ty::Function {
                kind: FunctionKind::FreeFunction,
                attrs: TAttrSet::new(),
                ret: r(Ty::Void),
                params: vec![TFuncParam {
                    attrs: TAttrSet::new(),
                    ty: r(Ty::Unsigned(8)),
                    name: Some(i.intern(n)),
                }],
            })
        };
        let (fx, fy) = (mk("x"), mk("y"));
        assert_eq!(fx, fy, "function types compare equal (names ignored)");
        assert_eq!(fx.function_params().unwrap()[0].name, Some(i.intern("x")));
        assert_eq!(fy.function_params().unwrap()[0].name, Some(i.intern("y")));
    }

    #[test]
    fn auto_and_error_count_as_resolved() {
        // Type.hs:1001, Type.hs:1015 — traps that terminate the fixpoint.
        assert!(Ty::Auto.resolved());
        assert!(Ty::Error(SmolStr::new("boom")).resolved());
        assert!(!Ty::Unresolved.resolved());
        assert!(!Ty::Template.resolved());
        assert!(!Ty::Undefined.resolved());
        assert!(Ty::Untyped.resolved());
    }

    #[test]
    fn empty_array_dims_are_unresolved() {
        // Type.hs:1022: `not (null xs)` is part of tResolved.
        let t = Ty::Array { attrs: TAttrSet::new(), elem: r(Ty::Unsigned(8)), dims: vec![] };
        assert!(!t.resolved());
    }

    #[test]
    fn predicates_look_through_const_and_instance_only() {
        // Type.hs:598-601: tInner is TConst | TInstance, nothing else.
        let inner = r(Ty::Unsigned(4));
        assert!(Ty::Const(inner.clone()).is_unsigned());
        assert!(
            Ty::Instance {
                concrete: r(Ty::Const(inner.clone())),
                template: SymbolPath::empty(),
                args: vec![],
            }
            .is_unsigned()
        );
        // `TType uint4` is not an unsigned type.
        assert!(!Ty::Type(inner).is_unsigned());
    }

    #[test]
    fn ecc_element_of_last_dimension_is_the_ecc_return_type() {
        // Type.hs:930-935 (trap 14).
        let ecc_fn = r(Ty::Function {
            kind: FunctionKind::FreeFunction,
            attrs: TAttrSet::new(),
            ret: r(Ty::Unsigned(37)),
            params: vec![],
        });
        let mut attrs = TAttrSet::new();
        attrs.insert(TAttr::Named(Attribute::Ecc, SymbolPath::empty(), ecc_fn));
        let mem = Ty::Array {
            attrs: attrs.clone(),
            elem: r(Ty::Unsigned(32)),
            dims: vec![ConstInt::from_i128(16)],
        };
        assert_eq!(mem.element(None), Ty::Unsigned(37));
        // ... but the lvalue path returns the element type (Type.hs:947).
        assert_eq!(mem.element_lvalue(None), Ty::Unsigned(32));
        // Trap 15: `const` memories leak the ECC substitution onto the lvalue path.
        let const_mem = Ty::Const(r(mem));
        assert_eq!(const_mem.element_lvalue(None), Ty::Const(r(Ty::Unsigned(37))));
    }

    #[test]
    fn variant_order_matches_type_hs_224_277() {
        // Ord is derived on both sides, so declaration order *is* the ordering.
        assert!(Ty::Auto < Ty::Template);
        assert!(Ty::Template < Ty::Undefined);
        assert!(Ty::Void < Ty::Boolean);
        assert!(Ty::Boolean < Ty::Float);
        assert!(Ty::Float < Ty::Str);
        assert!(Ty::Str < Ty::Signed(0));
        assert!(Ty::Signed(u32::MAX) < Ty::Unsigned(0));
        assert!(Ty::Unsigned(0) < Ty::Error(SmolStr::new("")));
    }
}
