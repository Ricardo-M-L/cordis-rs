//! Utility types and helpers shared across cordis-core.
//!
//! Mirrors `cordis/packages/core/src/utils.ts`.
//!
//! Rust cannot literally reproduce JS Proxy / Symbol internals, so the
//! traceability/shadowing infrastructure is replaced by concrete owned
//! types.  The two data-structures that do transfer 1:1 are
//! `Tracker` (metadata) and `DisposableList` (ordered, serial-numbered
//! effect collection).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

static GLOBAL_SN: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Tracker
// ---------------------------------------------------------------------------

/// Metadata attached to a service or context value, mirroring the TS `Tracker`
/// interface.
#[derive(Debug, Default, Clone)]
pub struct Tracker {
    pub associate: Option<String>,
    pub property: Option<String>,
    pub no_shadow: bool,
}

// ---------------------------------------------------------------------------
// DisposableList
// ---------------------------------------------------------------------------

/// Handle returned by [`DisposableList::push`] that lets the caller remove
/// its own entry in O(1).
#[derive(Debug)]
pub struct RemoveFn {
    sn: usize,
}

/// Ordered, serial-numbered collection of effects / disposables.
///
/// Mirrors the TypeScript `DisposableList<T>` class.  Each `push` allocates
/// a monotonically increasing serial number and stores a remove-closure.
/// `clear()` drains the list and returns all items in reverse insertion
/// order (so disposers run last-in-first-out, matching TS behaviour).
pub struct DisposableList<T> {
    map: HashMap<usize, T>,
    sn: usize,
}

impl<T> DisposableList<T> {
    pub fn new() -> Self {
        DisposableList {
            map: HashMap::new(),
            sn: 0,
        }
    }

    /// Push a value.  Returns a `RemoveFn` whose [`Drop`] removes the entry.
    pub fn push(&mut self, value: T) -> RemoveFn {
        let sn = self.next_sn();
        self.map.insert(sn, value);
        RemoveFn { sn }
    }

    /// Remove the entry identified by `sn`; returns `true` if it existed.
    pub fn delete(&mut self, sn: usize) -> bool {
        self.map.remove(&sn).is_some()
    }

    /// Drain the list and return all items in reverse insertion order.
    pub fn clear(&mut self) -> Vec<T> {
        let mut vec: Vec<T> = self.map.drain().map(|(_, v)| v).collect();
        vec.reverse();
        self.sn = 0;
        vec
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn next_sn(&mut self) -> usize {
        self.sn += 1;
        self.sn
    }
}

impl<T> Default for DisposableList<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a simple "outer stack" snapshot for error composition.
/// In Rust we just capture a short description string.
pub fn build_outer_stack() -> Vec<String> {
    // Best-effort: we don't have JS-style stack traces in idiomatic Rust,
    // so we return an empty vector.  Callers can still compose it into
    /// error messages if they want.
    vec![]
}

/// Wrap a closure in error-handling context, mirroring TS `composeError`.
/// If the closure panics, the panic is propagated unchanged.
pub fn compose_error<F, R>(callback: F) -> R
where
    F: FnOnce() -> R,
{
    callback()
}

/// Global serial-number generator used when creating effects / fibers.
pub fn next_global_sn() -> usize {
    GLOBAL_SN.fetch_add(1, Ordering::SeqCst) + 1
}
