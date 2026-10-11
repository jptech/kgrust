//! Compile-time constant values and the fold's tables.
//!
//! Charter: reproduce the reference's constant folding exactly — arbitrary
//! precision throughout (`Integer` in Haskell, `cpp_int` in C++), because widths
//! legally reach `c_maxRegisterBitWidth = 1048576` bits.
//!
//! Spec: `docs/spec/types.md` §14.1 (`ConstInt`) and `docs/spec/frontend-passes.md`.
//! Nothing here may depend on `kg-ty`: constant folding runs *before* and *during*
//! type inference, so the dependency points the other way. The fold itself is
//! the IR's (`kg_hir::constexpr`, `kg_sem`'s inference), over the loop's tree;
//! the CST's evaluator, `ConstEval` over a `ConstEnv`, went with the CST
//! inference engine (sprint W3.7).
//!
//! # Module map (owners, one per file)
//!
//! | module | owns | reference |
//! |---|---|---|
//! | [`int`] | [`ConstInt`], `clog2`, the truncation helpers | `Internal.hs:34-57` |
//! | [`value`] | [`ConstValue`], [`LiteralType`], [`BinOp`], [`UnOp`] | `Parser/Syntax.hs:36-108,211-221,347-357` |
//! | [`eval`] | [`fold_binary_values`] — `evalBinary` over two literals — and [`CastTarget`] | `Constexpr.hs:171-215,307-339` |
//!
//! # Deviation from `docs/spec/types.md` §14.1
//!
//! The spec sketches `pub enum ConstInt { Small(i128), Big(Arc<BigInt>) }` with a
//! *derived* `PartialEq`/`Ord`/`Hash`. Deriving those is wrong: it makes
//! `Small(5) != Big(5)` and sorts every `Small` below every `Big` regardless of
//! sign. [`ConstInt`] is therefore a struct over a private repr with the
//! normalization invariant enforced in one constructor, and hand-written
//! `Eq`/`Ord`/`Hash`. The `i128` fast path stays an allocation optimisation and
//! is invisible through the API.

pub mod eval;
pub mod int;
pub mod value;

pub use eval::{CastTarget, fold_binary_values};
pub use int::ConstInt;
pub use value::{AggregateElem, BinOp, ConstValue, F32Bits, LiteralSign, LiteralType, UnOp};
