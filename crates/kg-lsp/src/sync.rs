//! Lock acquisition that survives a panic elsewhere.
//!
//! A worker that panics while holding one of the server's locks poisons it,
//! and `lock().unwrap()` then turns that one bad request into a panic on every
//! later acquisition — including the main loop's, on the next keystroke, which
//! ends the process. Every lock here guards data that each critical section
//! leaves consistent at every step (an insert, a push, a swap), so the data
//! behind a poisoned lock is still the data, and the poison is only news.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}
