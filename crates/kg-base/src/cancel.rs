//! Cancellation (sprint W7.2): a flag a computation checks at its
//! checkpoints, raised from another thread without any lock.
//!
//! A language server holds a project's database read-locked for the length
//! of a query, so an edit's write waits for whatever check is running —
//! one it has just superseded. With a [`Cancel`] raised, the next
//! checkpoint unwinds with a [`Cancelled`] payload (`resume_unwind`, which
//! runs no panic hook) out to the caller that asked, which catches it with
//! [`catch`]. Unwinding rather than a `Result` threaded through every pass
//! is salsa's choice for the same reason: a memo is written after its
//! value is computed, so a query the unwind passes through writes nothing,
//! and no signature between the checkpoint and the caller has to change. A
//! query that finished before the cancel keeps its memo; it is a correct
//! answer for the inputs it was asked about.
//!
//! An unraised token costs a relaxed atomic load per checkpoint, and an
//! uncancelled run is byte-identical to one with no token at all.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The payload a cancelled computation unwinds with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// A flag shared by whoever may cancel and whatever may be cancelled.
/// Cloning shares it; [`Cancel::never`] can never be raised.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Option<Arc<AtomicBool>>);

thread_local! {
    /// The tokens of the computations this thread is inside ([`scope`]),
    /// innermost last: a request's, which `$/cancelRequest` raises, and a
    /// database query's, which an edit raises — so a [`checkpoint`] in code
    /// that holds neither still answers to both.
    static SCOPES: RefCell<Vec<Cancel>> = const { RefCell::new(Vec::new()) };
}

impl Cancel {
    /// A token that can be raised.
    pub fn new() -> Cancel {
        Cancel(Some(Arc::new(AtomicBool::new(false))))
    }

    /// A token that never is.
    pub fn never() -> Cancel {
        Cancel(None)
    }

    /// Ask every computation holding this token to stop at its next
    /// checkpoint.
    pub fn raise(&self) {
        if let Some(flag) = &self.0 {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Lower it again, for the next computation.
    pub fn reset(&self) {
        if let Some(flag) = &self.0 {
            flag.store(false, Ordering::Relaxed);
        }
    }

    /// Whether this token, or any this thread is inside ([`scope`]), has
    /// been raised.
    pub fn is_raised(&self) -> bool {
        self.own() || SCOPES.with(|s| s.borrow().iter().any(Cancel::own))
    }

    fn own(&self) -> bool {
        self.0.as_ref().is_some_and(|f| f.load(Ordering::Relaxed))
    }

    /// A checkpoint: unwind with [`Cancelled`] if this token, or any this
    /// thread is inside, has been raised.
    pub fn check(&self) {
        if self.is_raised() {
            std::panic::resume_unwind(Box::new(Cancelled));
        }
    }
}

/// A checkpoint for code that holds no token: the ones this thread is
/// inside ([`scope`]).
pub fn checkpoint() {
    Cancel::never().check();
}

thread_local! {
    static STRIDE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// [`checkpoint`] for a walk's inner loop: checks one call in 1024, so a
/// per-node call costs a thread-local increment.
#[inline]
pub fn checkpoint_strided() {
    let n = STRIDE.with(|s| {
        let n = s.get().wrapping_add(1);
        s.set(n);
        n
    });
    if n.is_multiple_of(1024) {
        checkpoint();
    }
}

/// Run `f` inside `token`: every checkpoint in it also stops when `token`
/// is raised.
pub fn scope<T>(token: &Cancel, f: impl FnOnce() -> T) -> T {
    SCOPES.with(|s| s.borrow_mut().push(token.clone()));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            SCOPES.with(|s| s.borrow_mut().pop());
        }
    }
    let _pop = Pop;
    f()
}

/// Run `f` outside every token this thread is inside: a computation whose
/// result is shared — one a second caller is waiting on — answers only to
/// the tokens it brings into scope itself, not to the request that happened
/// to start it. A `$/cancelRequest` for that request would otherwise throw
/// away work another caller then has to redo (`kg_db::Db::looped`).
pub fn detached<T>(f: impl FnOnce() -> T) -> T {
    let outer = SCOPES.with(|s| std::mem::take(&mut *s.borrow_mut()));
    struct Restore(Vec<Cancel>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPES.with(|s| *s.borrow_mut() = std::mem::take(&mut self.0));
        }
    }
    let _restore = Restore(outer);
    f()
}

/// Run `f`, and answer `Err(Cancelled)` if a checkpoint inside it unwound;
/// any other panic goes on unwinding.
pub fn catch<T>(f: impl FnOnce() -> T) -> Result<T, Cancelled> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => Ok(value),
        Err(payload) if payload.is::<Cancelled>() => Err(Cancelled),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raised token stops the computation at its next checkpoint, and
    /// only then; `never` cannot be raised; a request's token stops a
    /// computation that carries none of its own. Red with `check` ignoring
    /// the flag, or with `scope` not consulted.
    #[test]
    fn a_raised_token_unwinds_at_the_next_checkpoint() {
        let token = Cancel::new();
        let mut reached = 0;
        let out = catch(|| {
            token.check();
            reached += 1;
            token.raise();
            token.check();
            reached += 1;
        });
        assert_eq!((out, reached), (Err(Cancelled), 1));
        token.reset();
        assert_eq!(catch(|| token.check()), Ok(()));

        let never = Cancel::never();
        never.raise();
        assert_eq!(catch(|| never.check()), Ok(()));

        let request = Cancel::new();
        request.raise();
        assert_eq!(scope(&request, || catch(|| never.check())), Err(Cancelled));
        assert_eq!(catch(|| never.check()), Ok(()), "the request's token is gone after its scope");
    }

    /// A detached computation does not answer to the request it runs in,
    /// does answer to a token it scopes itself, and leaves the request's
    /// scope as it found it. Red with `detached` leaving the stack in place.
    #[test]
    fn a_detached_computation_answers_only_to_its_own_tokens() {
        let request = Cancel::new();
        request.raise();
        let own = Cancel::new();
        scope(&request, || {
            assert_eq!(detached(|| catch(checkpoint)), Ok(()));
            own.raise();
            assert_eq!(detached(|| catch(|| scope(&own, checkpoint))), Err(Cancelled));
            assert_eq!(catch(checkpoint), Err(Cancelled), "the request's scope is back");
        });
    }

    #[test]
    fn another_panic_is_not_a_cancellation() {
        let r = std::panic::catch_unwind(|| catch(|| std::panic::resume_unwind(Box::new("a bug"))));
        assert!(r.is_err());
    }
}
