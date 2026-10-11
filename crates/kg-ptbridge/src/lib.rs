//! The frontend's real output: the calls it makes into `libkanagawa-backend`
//! (`docs/spec/abi.md`). This crate walks a lowered program the way the
//! reference's `ParseTree.hs` walks its final tree — `cataM parseWithLoc`,
//! post-order, children in `ExpF` field order — and emits every call through
//! [`Backend`], a trait with three implementations:
//!
//! * [`trace::Trace`] writes `kg-pttrace` lines, so the walk is developed and
//!   graded against what `cargo xtask ptrace` recorded of the reference, with
//!   no C++ in the loop;
//! * [`record::Recording`] keeps the calls as data, to be replayed into
//!   another backend — `kg build` walks into one, and a worker process
//!   replays it into the real library ([`isolate`]);
//! * [`real::Real`] calls the library through `kg-backend-sys`, which only
//!   the worker does.
//!
//! The trait and all three implementations are generated from the headers
//! (`kg_abi_gen::backend`) into the checked-in `backend.rs`, which `cargo
//! xtask codegen` writes and `--check` compares, so a function's signature
//! here is the header's.
//!
//! What the walk declines, it declines *by name*: a node kind the reference
//! eliminates before its walk (`abi.md` §11) or a rewrite this port has not
//! made yet stops the walk with a [`walk::Declined`] naming it, and the
//! trace up to that point stands. `xtask ptdiff` counts those reasons.

pub mod cache;
pub mod isolate;
pub mod locations;
pub mod program;
pub mod real;
pub mod record;
pub mod trace;
pub mod walk;

#[allow(clippy::too_many_arguments)]
mod generated {
    use crate::record::{Replayed, f32_arg, int_arg, loc_arg, loc_ptr_arg, scope_arg, str_arg, uint_arg};
    use crate::trace::{Ret, nz};
    use kg_pttrace::Arg;
    include!("backend.rs");
}

pub use generated::{Backend, FUNCTIONS};
