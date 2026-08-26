//! Fiber lifecycle and effect management.

use crate::utils::lock;
use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};

static COUNTER: AtomicUsize = AtomicUsize::new(1);

type FiberStore = HashMap<TypeId, HashMap<String, Arc<dyn Any + Send + Sync>>>;
type UpdateHook = Arc<dyn Fn(usize) + Send + Sync + 'static>;

/// Cleanup callback retained by a [`Fiber`] until the effect handle or fiber is disposed.
pub type Disposer = Box<dyn FnOnce() + Send + Sync + 'static>;

/// Convert a closure into the concrete cleanup type accepted by [`Fiber::effect`].
pub fn disposer(callback: impl FnOnce() + Send + Sync + 'static) -> Disposer {
    Box::new(callback)
}

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
            Self::Pending => "pending",
            Self::Loading => "loading",
            Self::Active => "active",
            Self::Failed => "failed",
            Self::Disposed => "disposed",
            Self::Unloading => "unloading",
        }
    }

    fn can_transition_to(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (
                    Self::Pending,
                    Self::Loading | Self::Active | Self::Failed | Self::Disposed
                ) | (
                    Self::Loading,
                    Self::Active | Self::Failed | Self::Unloading | Self::Disposed
                ) | (
                    Self::Active,
                    Self::Failed | Self::Unloading | Self::Disposed
                ) | (
                    Self::Failed,
                    Self::Pending | Self::Unloading | Self::Disposed
                ) | (Self::Unloading, Self::Disposed | Self::Failed)
                    | (Self::Disposed, Self::Pending)
            )
    }
}

struct FiberInner {
    state: (Mutex<FiberState>, Condvar),
    ready: AtomicBool,
    store: Mutex<FiberStore>,
    effects: Mutex<BTreeMap<usize, Disposer>>,
    next_effect: AtomicUsize,
    pending: AtomicUsize,
    pending_notify: tokio::sync::Notify,
    revision: AtomicUsize,
    update_hooks: Mutex<BTreeMap<usize, UpdateHook>>,
    next_update_hook: AtomicUsize,
}

/// A unit of work with deterministic effect cleanup and a validated lifecycle.
pub struct Fiber {
    id: usize,
    inner: Arc<FiberInner>,
}

impl Fiber {
    pub fn new() -> Self {
        Self {
            id: COUNTER.fetch_add(1, Ordering::SeqCst),
            inner: Arc::new(FiberInner {
                state: (Mutex::new(FiberState::Pending), Condvar::new()),
                ready: AtomicBool::new(false),
                store: Mutex::new(HashMap::new()),
                effects: Mutex::new(BTreeMap::new()),
                next_effect: AtomicUsize::new(1),
                pending: AtomicUsize::new(0),
                pending_notify: tokio::sync::Notify::new(),
                revision: AtomicUsize::new(0),
                update_hooks: Mutex::new(BTreeMap::new()),
                next_update_hook: AtomicUsize::new(1),
            }),
        }
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn state(&self) -> FiberState {
        *lock(&self.inner.state.0)
    }

    pub fn is_active(&self) -> bool {
        self.state() == FiberState::Active
    }

    pub fn is_ready(&self) -> bool {
        self.inner.ready.load(Ordering::SeqCst)
    }

    pub fn mark_ready(&self) {
        self.inner.ready.store(true, Ordering::SeqCst);
    }

    pub fn mark_not_ready(&self) {
        self.inner.ready.store(false, Ordering::SeqCst);
    }

    /// Attempt a lifecycle transition and reject invalid state changes.
    pub fn try_transition_to(&self, next: FiberState) -> Result<(), String> {
        let mut current = lock(&self.inner.state.0);
        if !current.can_transition_to(next) {
            return Err(format!(
                "invalid fiber transition: {} -> {}",
                current.as_str(),
                next.as_str()
            ));
        }
        *current = next;
        self.inner.state.1.notify_all();
        Ok(())
    }

    /// Backward-compatible transition helper. Invalid transitions leave the state unchanged.
    pub fn transition_to(&self, next: FiberState) {
        let _ = self.try_transition_to(next);
    }

    pub fn activate(&self) {
        self.transition_to(FiberState::Active);
    }

    pub fn fail(&self) {
        self.transition_to(FiberState::Failed);
    }

    /// Run an effect factory.
    ///
    /// Ordinary return values are stored under the empty key and can be read with
    /// [`Fiber::get`]. Return [`Disposer`] to register deterministic cleanup:
    /// `fiber.effect(|| disposer(|| release_resource()))`.
    pub fn effect<F, R>(&self, factory: F) -> Option<EffectorHandle>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Any + Send + Sync + 'static,
    {
        if matches!(self.state(), FiberState::Disposed | FiberState::Unloading) {
            return None;
        }

        let _pending = PendingGuard::new(Arc::clone(&self.inner));
        let result = factory();
        Some(self.store_effect_result(result))
    }

    /// Await an asynchronous factory and register/store its result using the same rules as
    /// [`Fiber::effect`].
    pub async fn effect_async<F, R>(&self, future: F) -> Option<EffectorHandle>
    where
        F: std::future::Future<Output = R> + Send,
        R: Any + Send + Sync + 'static,
    {
        if matches!(self.state(), FiberState::Disposed | FiberState::Unloading) {
            return None;
        }
        let _pending = PendingGuard::new(Arc::clone(&self.inner));
        let result = future.await;
        Some(self.store_effect_result(result))
    }

    fn store_effect_result<R>(&self, result: R) -> EffectorHandle
    where
        R: Any + Send + Sync + 'static,
    {
        if matches!(self.state(), FiberState::Disposed | FiberState::Unloading) {
            if TypeId::of::<R>() == TypeId::of::<Disposer>() {
                let value: Box<dyn Any + Send + Sync> = Box::new(result);
                let cleanup = *value
                    .downcast::<Disposer>()
                    .expect("type id checked before downcast");
                let _ = catch_unwind(AssertUnwindSafe(cleanup));
            }
            return EffectorHandle {
                fiber: Arc::downgrade(&self.inner),
                effect_id: None,
            };
        }
        if TypeId::of::<R>() == TypeId::of::<Disposer>() {
            let value: Box<dyn Any + Send + Sync> = Box::new(result);
            let disposer = *value
                .downcast::<Disposer>()
                .expect("type id checked before downcast");
            let effect_id = self.inner.next_effect.fetch_add(1, Ordering::SeqCst);
            lock(&self.inner.effects).insert(effect_id, disposer);
            EffectorHandle {
                fiber: Arc::downgrade(&self.inner),
                effect_id: Some(effect_id),
            }
        } else {
            let type_id = TypeId::of::<R>();
            lock(&self.inner.store)
                .entry(type_id)
                .or_default()
                .insert(String::new(), Arc::new(result));
            EffectorHandle {
                fiber: Arc::downgrade(&self.inner),
                effect_id: None,
            }
        }
    }

    pub fn get<T: Any + Send + Sync + Clone + 'static>(&self) -> Option<T> {
        self.get_key("")
    }

    pub fn get_key<T: Any + Send + Sync + Clone + 'static>(&self, key: &str) -> Option<T> {
        lock(&self.inner.store)
            .get(&TypeId::of::<T>())?
            .get(key)
            .and_then(|value| value.downcast_ref::<T>().cloned())
    }

    pub fn set<T: Any + Send + Sync + 'static>(&self, key: &str, value: T) {
        lock(&self.inner.store)
            .entry(TypeId::of::<T>())
            .or_default()
            .insert(key.to_string(), Arc::new(value));
    }

    pub fn remove<T: Any + Send + Sync + 'static>(&self, key: &str) -> bool {
        lock(&self.inner.store)
            .get_mut(&TypeId::of::<T>())
            .is_some_and(|values| values.remove(key).is_some())
    }

    /// Dispose all registered effects in reverse registration order.
    pub fn dispose(&self) {
        if self.state() == FiberState::Disposed {
            return;
        }
        self.transition_to(FiberState::Unloading);
        let effects = std::mem::take(&mut *lock(&self.inner.effects));
        for (_, effect) in effects.into_iter().rev() {
            let _ = catch_unwind(AssertUnwindSafe(effect));
        }
        lock(&self.inner.store).clear();
        lock(&self.inner.update_hooks).clear();
        self.inner.ready.store(false, Ordering::SeqCst);
        self.inner.pending.store(0, Ordering::SeqCst);
        self.inner.pending_notify.notify_waiters();
        self.transition_to(FiberState::Disposed);
    }

    /// Reset a disposed/failed fiber and activate a fresh lifecycle generation.
    pub fn restart(&self) {
        self.dispose();
        self.transition_to(FiberState::Pending);
        self.activate();
        self.inner.revision.fetch_add(1, Ordering::SeqCst);
    }

    /// Register a lifecycle-bound update hook. Returns a stable hook id.
    pub fn on_update(&self, callback: impl Fn(usize) + Send + Sync + 'static) -> usize {
        let id = self.inner.next_update_hook.fetch_add(1, Ordering::SeqCst);
        lock(&self.inner.update_hooks).insert(id, Arc::new(callback));
        id
    }

    pub fn remove_update_hook(&self, id: usize) -> bool {
        lock(&self.inner.update_hooks).remove(&id).is_some()
    }

    /// Advance the observable revision and invoke lifecycle-bound update hooks.
    pub fn update(&self) {
        if self.state() == FiberState::Pending {
            self.activate();
        }
        if self.is_active() {
            let revision = self.inner.revision.fetch_add(1, Ordering::SeqCst) + 1;
            let hooks: Vec<_> = lock(&self.inner.update_hooks).values().cloned().collect();
            for hook in hooks {
                let _ = catch_unwind(AssertUnwindSafe(|| hook(revision)));
            }
        }
    }

    pub fn revision(&self) -> usize {
        self.inner.revision.load(Ordering::SeqCst)
    }

    pub fn active_effects(&self) -> usize {
        lock(&self.inner.effects).len()
    }

    /// Wait until all synchronous or asynchronous effect factories have completed.
    pub async fn await_all(&self) {
        loop {
            let notified = self.inner.pending_notify.notified();
            if self.inner.pending.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }
}

struct PendingGuard {
    inner: Arc<FiberInner>,
}

impl PendingGuard {
    fn new(inner: Arc<FiberInner>) -> Self {
        inner.pending.fetch_add(1, Ordering::SeqCst);
        Self { inner }
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let previous = self.inner.pending.fetch_sub(1, Ordering::SeqCst);
        if previous <= 1 {
            self.inner.pending.store(0, Ordering::SeqCst);
            self.inner.pending_notify.notify_waiters();
        }
    }
}

/// RAII handle for a registered fiber effect.
pub struct EffectorHandle {
    fiber: Weak<FiberInner>,
    effect_id: Option<usize>,
}

impl EffectorHandle {
    pub fn dispose(mut self) {
        self.dispose_inner();
    }

    pub fn is_cleanup(&self) -> bool {
        self.effect_id.is_some()
    }

    /// Leave the cleanup registered with the owning fiber without leaking this handle.
    ///
    /// Dropping an ordinary [`EffectorHandle`] disposes its effect immediately.  Runtime
    /// facilities such as context-bound services and event listeners instead need the
    /// fiber to retain the cleanup until the whole scope is disposed.  `detach()` transfers
    /// that responsibility to the fiber while still allowing the handle's `Weak` reference
    /// to be released normally.
    pub fn detach(mut self) {
        self.effect_id = None;
    }

    fn dispose_inner(&mut self) {
        let Some(effect_id) = self.effect_id.take() else {
            return;
        };
        if let Some(fiber) = self.fiber.upgrade() {
            if let Some(effect) = lock(&fiber.effects).remove(&effect_id) {
                let _ = catch_unwind(AssertUnwindSafe(effect));
            }
        }
    }
}

impl Drop for EffectorHandle {
    fn drop(&mut self) {
        self.dispose_inner();
    }
}

impl fmt::Debug for EffectorHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EffectorHandle")
            .field("effect_id", &self.effect_id)
            .finish()
    }
}

impl Default for Fiber {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Fiber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Fiber")
            .field("id", &self.id)
            .field("state", &self.state())
            .field("effects", &self.active_effects())
            .field("pending", &self.inner.pending.load(Ordering::SeqCst))
            .field("revision", &self.revision())
            .finish()
    }
}
