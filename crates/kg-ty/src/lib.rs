//! The Kanagawa type model: width algebra, subtyping, and the node key.
//!
//! Charter: `Ty` mirrors `Type.hs:224-277` one-for-one. Two traps are load-bearing
//! and called out in `docs/spec/types.md` §15:
//!
//! * `Eq` on a function parameter ignores its name (`Type.hs:126-129`) while `Ord`
//!   includes it — hand-write both, derive neither.
//! * Inference is a whole-program **round-based fixpoint** that re-traverses until
//!   the count of unresolved nodes stops changing. It is not unification and not
//!   bidirectional; porting it as either will diverge. It runs in
//!   `kg_sem::infer`, over the IR. The CST engine that ran it over the
//!   lossless tree, `kg-cst-infer`, was deleted in sprint W3.7.
//!
//! Spec: `docs/spec/types.md`.
//!
//! # Module map (owners, one per file)
//!
//! | module | owns | reference |
//! |---|---|---|
//! | [`ty`] | [`Ty`], [`TyRef`], [`TAttr`], [`TArg`], [`TFuncParam`], the predicates | `Type.hs:117-278`, `Type.hs:537-1031` |
//! | [`width`] | `t_width`, `t_offset_of`, the whole operator width algebra | `Type.hs:692-730`, `Type.hs:740-926` |
//! | [`subtype`] | `<:`, `tIsConvertibleTo`, `tSuper`, `tMux` | `Type.hs:306-366`, `Type.hs:899-926` |
//! | [`convert`] | the narrowing check and the `conversion` warning | `TypeCheck.hs:33-216` |
//! | [`table`] | [`TypeKey`] — a node of the text by position, kind and expansion | `Type.hs:452` |
//!
//! # Deviations from `docs/spec/types.md` §14
//!
//! * §14.2 proposes `TyRef` as "an index into an arena/interner" and then warns
//!   that `Ord` must stay structural. Those are in tension: an index's `Ord` is
//!   allocation order. [`TyRef`] is therefore an `Arc<Ty>` with structural
//!   `Eq`/`Ord`/`Hash` and an `Arc::ptr_eq` fast path; sharing an allocation
//!   never changes what equality means. See [`ty`].
//! * §14.2 writes `dims: Vec<u64>` in the code block while its own prose says to
//!   keep dimensions arbitrary-precision. The prose wins:
//!   [`Ty::Array`]`::dims` is `Vec<ConstInt>`, matching `[Integer]`
//!   (`Type.hs:261`).
//! * §14.4 splits `kg_ty::width` from `kg_ty::algebra`; they are merged into
//!   [`width`], because every row of the operator table is a statement about
//!   widths and a split invites two owners to disagree about where `tWidenInt`
//!   lives.
//! * §14.2 writes `Ty::Error(kg_base::Str)`; there is no such type. [`Ty::Error`]
//!   carries a `SmolStr`, whose `Ord` is string order — which is what
//!   `TError String`'s derived `Ord` compares.

pub mod convert;
pub mod subtype;
pub mod table;
pub mod ty;
pub mod width;

pub use ty::{
    Attribute, Flag, FunctionKind, Name, QualifiedName, TArg, TAttr, TAttrSet, TDefault, TFuncParam, Ty,
    TyRef, hash_strict,
};

pub use width::{
    MAX_REGISTER_BIT_WIDTH, expected_integer_error, infer_int_literal_type, infer_unary_op,
    invalid_operands_error, minimal_int_type, t_binary_op, t_binary_op_literal, t_narrow_int,
    t_offset_of, t_sum_widths, t_widen_int, t_width, type_of_operand,
};

pub use subtype::{
    aggregate_initializer, initialized_type, is_convertible_to, is_subtype, t_mux, t_super,
};

pub use convert::{Narrowing, invalid_conversion_error, narrowing_warning};

pub use table::TypeKey;

/// Re-exported so downstream crates get one spelling of the operator enums and of
/// arbitrary-precision integers. `kg-ty` and `kg-const` must agree on these or
/// the width algebra and the folder will disagree about what `<<` means.
pub use kg_const::{BinOp, ConstInt, ConstValue, LiteralSign, LiteralType, UnOp};
