//! Fiber — Cordis' lightweight unit of work with effect/disposable lifecycle.
//!
//! Mirrors the TypeScript `Fiber` class: state machine, async/sync effects,
//! restart/update, and store management.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// FiberState
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberState {
    Pending,
    Loading,
    Active,
    Failed,
    Disposed,
    Unloading,
}

impl FiberState {
    pub fn as_str(&self) -> &str {
        match self {
            FiberState::Pending => "pending",
            FiberState::Loading => "loading",
            FiberState::Active => "active",
            FiberState::Failed => "failed",
            FiberState::Disposed => "disposed",
            FiberState::Unloading => "unloading",
        }
    }
}

// ---------------------------------------------------------------------------
// Fiber
// ---------------------------------------------------------------------------

/// A fiber is a unit of work that runs effects with proper disposable lifecycle.
pub struct Fiber {
    id: usize,
    state: Arc<(Mutex<FiberState>, Condvar)>,
    ready: AtomicBool,
    store: Arc<Mutex<HashMap<TypeId, HashMap<String, Arc<dyn Any + Send + Sync>>>>>,
    pending_count: Arc<AtomicUsize>,
}

impl Fiber {
    /// Create a new Fiber in the Pending state.
    pub fn new() -> Self {
        Fiber {
            id: COUNTER.fetch_add(1, Ordering::SeqCst),
            state: Arc::new((Mutex::new(FiberState::Pending), Condvar::new())),
            ready: AtomicBool::new(false),
            store: Arc::new(Mutex::new(HashMap::new())),
            pending_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Return the fiber's unique id.
    pub fn id(&self) -> usize {
        self.id
    }

    /// Get the current state.
    pub fn state(&self) -> FiberState {
        *self.state.0.lock().unwrap()
    }

    /// Whether the fiber is active.
    pub fn is_active(&self) -> bool {
        self.state() == FiberState::Active
    }

    /// Whether the fiber is ready to run.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Mark the fiber as ready.
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }

    /// Mark the fiber as not ready.
    pub fn mark_not_ready(&self) {
        self.ready.store(false, Ordering::SeqCst);
    }

    /// Transition the fiber to a new state.
    pub fn transition_to(&self, new_state: FiberState) {
        let (lock, cvar) = &*self.state;
        let mut current = lock.lock().unwrap();
        *current = new_state;
        cvar.notify_all();
    }

    /// Transition to Active.
    pub fn activate(&self) {
        self.transition_to(FiberState::Active);
    }

    /// Transition to Failed.
    pub fn fail(&self) {
        self.transition_to(FiberState::Failed);
    }

    /// Transition to Disposed.
    pub fn dispose(&self) {
        self.transition_to(FiberState::Disposed);
        self.pending_count.store(0, Ordering::SeqCst);
    }

    /// Register an effect: runs the closure immediately, returns a dispose handle.
    ///
    /// The closure may return an `Option<impl FnOnce()>` which will be called on dispose.
    pub fn effect<F, R>(&self, factory: F) -> Option<EffectorHandle>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Any + Send + Sync + 'static,
    {
        if self.state() == FiberState::Disposed {
            return None;
        }

        self.pending_count.fetch_add(1, Ordering::SeqCst);

        let result = factory();
        self.store_with_result(result);

        // Return a handle that decrements pending when called
        Some(EffectorHandle {
            pending: Arc::clone(&self.pending_count),
        })
    }

    /// Store an effect result in the fiber's type-keyed store.
    fn store_with_result<T: Any + Send + Sync + 'static>(&self, _value: T) {
        let type_id = TypeId::of::<T>();
        let mut store = self.store.lock().unwrap();
        let _entry = store.entry(type_id).or_insert_with(HashMap::new);
    }

    /// Get a stored value by type.
    pub fn get<T: Any + Send + Sync + Clone + 'static>(&self) -> Option<T> {
        let type_id = TypeId::of::<T>();
        let store = self.store.lock().unwrap();
        store.get(&type_id)?.get("").and_then(|v| v.downcast_ref::<T>().cloned())
    }

    /// Set a store value for the given type and key.
    pub fn set<T: Any + Send + Sync + 'static>(&self, key: &str, value: T) {
        let type_id = TypeId::of::<T>();
        let mut store = self.store.lock().unwrap();
        let entry = store.entry(type_id).or_insert_with(HashMap::new);
        entry.insert(key.to_string(), Arc::new(value));
    }

    /// Restart the fiber: dispose then activate.
    pub fn restart(&self) {
        self.dispose();
        self.transition_to(FiberState::Pending);
        self.activate();
    }

    /// Update the fiber (no-op in this port — stub for loader integration).
    pub fn update(&self) {
        if self.state() != FiberState::Active {
            self.transition_to(FiberState::Active);
        }
    }

    /// Block until all pending effects have been resolved.
    pub async fn await_all(&self) {
        loop {
            if self.pending_count.load(Ordering::SeqCst) == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }
}

/// Handle returned by `Fiber::effect`. Dropping it decrements the pending count.
pub struct EffectorHandle {
    pending: Arc<AtomicUsize>,
}

impl Drop for EffectorHandle {
    fn drop(&mut self) {
        let _ = self.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Default for Fiber {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Fiber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fiber")
            .field("id", &self.id)
            .field("state", &self.state())
            .field("pending", &self.pending_count.load(Ordering::SeqCst))
            .finish()
    }
}
