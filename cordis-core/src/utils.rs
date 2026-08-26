//! Utility types and helpers shared across `cordis-core`.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

static GLOBAL_SN: AtomicUsize = AtomicUsize::new(0);

/// Metadata attached to a service or context value.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tracker {
    pub associate: Option<String>,
    pub property: Option<String>,
    pub no_shadow: bool,
}

struct DisposableInner<T> {
    next_serial: usize,
    values: BTreeMap<usize, T>,
}

impl<T> Default for DisposableInner<T> {
    fn default() -> Self {
        Self {
            next_serial: 0,
            values: BTreeMap::new(),
        }
    }
}

/// Handle returned by [`DisposableList::push`]. Calling [`RemoveFn::remove`]
/// or dropping the handle removes the associated entry exactly once.
pub struct RemoveFn {
    serial: usize,
    remove: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl RemoveFn {
    /// Remove the associated entry immediately.
    pub fn remove(mut self) {
        if let Some(remove) = self.remove.take() {
            remove();
        }
    }

    /// Return the stable serial number assigned to the entry.
    pub fn serial(&self) -> usize {
        self.serial
    }
}

impl Drop for RemoveFn {
    fn drop(&mut self) {
        if let Some(remove) = self.remove.take() {
            remove();
        }
    }
}

impl fmt::Debug for RemoveFn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoveFn")
            .field("serial", &self.serial)
            .field("active", &self.remove.is_some())
            .finish()
    }
}

/// Ordered, serial-numbered collection of disposable values.
///
/// Values are removed when their handle is disposed. [`DisposableList::clear`]
/// returns remaining values in deterministic last-in-first-out order.
pub struct DisposableList<T> {
    inner: Arc<Mutex<DisposableInner<T>>>,
}

impl<T> DisposableList<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DisposableInner::default())),
        }
    }

    /// Remove an entry by serial number.
    pub fn delete(&self, serial: usize) -> bool {
        lock(&self.inner).values.remove(&serial).is_some()
    }

    /// Drain all remaining values in reverse insertion order.
    pub fn clear(&self) -> Vec<T> {
        let mut inner = lock(&self.inner);
        let values = std::mem::take(&mut inner.values);
        values.into_iter().rev().map(|(_, value)| value).collect()
    }

    pub fn len(&self) -> usize {
        lock(&self.inner).values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: Send + 'static> DisposableList<T> {
    /// Insert a value and return its removal handle.
    pub fn push(&self, value: T) -> RemoveFn {
        let serial = {
            let mut inner = lock(&self.inner);
            inner.next_serial = inner.next_serial.saturating_add(1);
            let serial = inner.next_serial;
            inner.values.insert(serial, value);
            serial
        };

        let weak: Weak<Mutex<DisposableInner<T>>> = Arc::downgrade(&self.inner);
        RemoveFn {
            serial,
            remove: Some(Box::new(move || {
                if let Some(inner) = weak.upgrade() {
                    lock(&inner).values.remove(&serial);
                }
            })),
        }
    }
}

impl<T> Default for DisposableList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for DisposableList<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DisposableList")
            .field("len", &self.len())
            .finish()
    }
}

/// Capture a native Rust backtrace for error composition.
pub fn build_outer_stack() -> Vec<String> {
    vec![std::backtrace::Backtrace::force_capture().to_string()]
}

/// Run a callback while preserving native Rust panic semantics.
pub fn compose_error<F, R>(callback: F) -> R
where
    F: FnOnce() -> R,
{
    callback()
}

/// Global serial-number generator used by effects and fibers.
pub fn next_global_sn() -> usize {
    GLOBAL_SN.fetch_add(1, Ordering::SeqCst) + 1
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
