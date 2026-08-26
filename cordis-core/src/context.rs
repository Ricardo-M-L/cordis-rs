//! Hierarchical dependency-injection context.

use crate::events::{EventArgs, EventHandle, EventValue, EventsService};
use crate::fiber::{disposer, Fiber};
use crate::registry::{Plugin, RegistryService};
use crate::utils::lock;
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

static CONTEXT_IDS: AtomicUsize = AtomicUsize::new(1);

pub type Context = CordisContext;

type TypedValues = HashMap<TypeId, HashMap<String, Arc<dyn Any + Send + Sync>>>;
type TypedKey = (TypeId, String);
type EventFilter = Arc<dyn Fn(&ContextScope) -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum IsolationLabel {
    Generated(usize),
    Shared(usize),
}

/// Immutable listener-registration metadata used during event filtering. It deliberately
/// excludes runtime handles and the owning Fiber, preventing Events -> Context -> Fiber ->
/// disposer reference cycles while preserving per-name isolation semantics.
#[derive(Debug, Clone)]
pub struct ContextScope {
    context_id: usize,
    isolate_labels: HashMap<String, IsolationLabel>,
}

impl ContextScope {
    pub fn context_id(&self) -> usize {
        self.context_id
    }

    pub fn shares_isolate(&self, context: &CordisContext, name: &str) -> bool {
        self.isolate_labels.get(name).copied() == context.isolate_label(name)
    }
}

/// A hierarchical context. Cloning shares one scope; [`CordisContext::extend`]
/// creates a child overlay whose reads fall back to its parent and whose writes
/// remain local.
pub struct CordisContext {
    data: Arc<RwLock<HashMap<String, u64>>>,
    deleted_data: Arc<RwLock<HashSet<String>>>,
    typed_store: Arc<RwLock<TypedValues>>,
    deleted_typed: Arc<RwLock<HashSet<TypedKey>>>,
    timer_name: Arc<Mutex<Option<String>>>,
    context_id: usize,
    isolated: bool,
    parent: Option<Arc<CordisContext>>,
    /// Per-name isolation labels, mirroring upstream `Context[symbols.isolate]`:
    /// a prototype chain of `name -> label` maps. `isolate(name)` shadows only
    /// the given name; other names keep inheriting the parent's labels.
    isolate_labels: Arc<RwLock<HashMap<String, IsolationLabel>>>,
    /// Layered config overrides, mirroring upstream `Context[symbols.intercept]`:
    /// a prototype chain of `name -> config` maps. `intercept(name, config)`
    /// shadows only the given name.
    intercepts: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    /// The lifecycle owner of resources created through this context.
    fiber: Arc<Fiber>,
    /// Runtime services are weak to avoid Context -> Registry/Events -> Context cycles.
    registry: Arc<RwLock<Option<Weak<RegistryService>>>>,
    events: Arc<RwLock<Option<Weak<EventsService>>>>,
    /// Optional dispatch-side listener filter, equivalent to upstream `Context.filter`.
    event_filter: Option<EventFilter>,
}

impl CordisContext {
    pub fn new() -> Self {
        let fiber = Arc::new(Fiber::new());
        fiber.activate();
        Self::scope(None, false, fiber)
    }

    fn scope(parent: Option<Arc<CordisContext>>, isolated: bool, fiber: Arc<Fiber>) -> Self {
        let registry = parent
            .as_ref()
            .map(|parent| Arc::clone(&parent.registry))
            .unwrap_or_else(|| Arc::new(RwLock::new(None)));
        let events = parent
            .as_ref()
            .map(|parent| Arc::clone(&parent.events))
            .unwrap_or_else(|| Arc::new(RwLock::new(None)));
        let event_filter = parent
            .as_ref()
            .and_then(|parent| parent.event_filter.clone());
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            deleted_data: Arc::new(RwLock::new(HashSet::new())),
            typed_store: Arc::new(RwLock::new(HashMap::new())),
            deleted_typed: Arc::new(RwLock::new(HashSet::new())),
            timer_name: Arc::new(Mutex::new(None)),
            context_id: CONTEXT_IDS.fetch_add(1, Ordering::SeqCst),
            isolated,
            parent,
            isolate_labels: Arc::new(RwLock::new(HashMap::new())),
            intercepts: Arc::new(RwLock::new(HashMap::new())),
            fiber,
            registry,
            events,
            event_filter,
        }
    }

    pub fn set(&self, key: &str, value: u64) {
        self.deleted_data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
        self.data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_string(), value);
    }

    pub fn get(&self, key: &str) -> Option<u64> {
        if let Some(value) = self
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .copied()
        {
            return Some(value);
        }
        if self
            .deleted_data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(key)
        {
            return None;
        }
        self.parent.as_ref()?.get(key)
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Hide a local or inherited value in this scope.
    pub fn delete(&self, key: &str) -> bool {
        let removed = self
            .data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key)
            .is_some();
        let inherited = self.parent.as_ref().is_some_and(|parent| parent.has(key));
        self.deleted_data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_string());
        removed || inherited
    }

    pub fn get_timer_name(&self) -> Option<String> {
        lock(&self.timer_name).clone().or_else(|| {
            self.parent
                .as_ref()
                .and_then(|parent| parent.get_timer_name())
        })
    }

    pub fn set_timer(&self, name: &str) {
        *lock(&self.timer_name) = Some(name.to_string());
    }

    /// Create a child scope with inherited reads and local writes.
    pub fn extend(&self) -> Self {
        Self::scope(Some(Arc::new(self.clone())), false, Arc::clone(&self.fiber))
    }

    /// Create an explicitly isolated child scope. The scope still inherits values,
    /// but its identity can be used by registries/loaders to prevent cross-scope reuse.
    pub fn isolate(&self) -> Self {
        Self::scope(Some(Arc::new(self.clone())), true, Arc::clone(&self.fiber))
    }

    /// Create a child owned by a different lifecycle fiber. Registry plugin activation uses
    /// this to ensure every resource registered from `Plugin::apply` is disposed together.
    pub fn with_fiber(&self, fiber: Arc<Fiber>) -> Self {
        Self::scope(Some(Arc::new(self.clone())), false, fiber)
    }

    /// Per-name isolation, mirroring upstream `ctx.isolate(name, label?)`:
    /// returns a child scope whose `name` service registrations are keyed
    /// separately from every other scope's, while other names keep the
    /// parent's labels. Contexts derived from the same `label` share the
    /// isolated slot (see upstream "shared label" semantics).
    pub fn isolate_name(&self, name: &str, label: Option<usize>) -> Self {
        let child = self.extend();
        let isolate_key = match label {
            Some(label) => IsolationLabel::Shared(label),
            None => {
                static ISOLATE_KEYS: AtomicUsize = AtomicUsize::new(1);
                IsolationLabel::Generated(ISOLATE_KEYS.fetch_add(1, Ordering::SeqCst))
            }
        };
        child
            .isolate_labels
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_string(), isolate_key);
        child
    }

    /// Layered config override, mirroring upstream `ctx.intercept(name, config)`:
    /// returns a child scope in which `name` resolves to `config` layered on top
    /// of any inherited intercepts. Reads of other names keep the parent's layers.
    pub fn intercept(&self, name: &str, config: serde_json::Value) -> Self {
        let child = self.extend();
        child
            .intercepts
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_string(), config);
        child
    }

    /// The isolation label for `name` visible from this scope, walking the
    /// prototype chain like upstream `ctx[symbols.isolate][name]`.
    pub(crate) fn isolate_label(&self, name: &str) -> Option<IsolationLabel> {
        if let Some(label) = self
            .isolate_labels
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .copied()
        {
            return Some(label);
        }
        self.parent.as_ref()?.isolate_label(name)
    }

    /// Whether two scopes resolve `name` to the same isolated service slot,
    /// mirroring upstream `Service[symbols.filter](ctx)`:
    /// `ctx[symbols.isolate][name] === this.ctx[symbols.isolate][name]`.
    pub fn shares_isolate(&self, other: &CordisContext, name: &str) -> bool {
        self.isolate_label(name) == other.isolate_label(name)
    }

    /// Layered config resolution for `name`, mirroring upstream
    /// `Service[symbols.resolveConfig](base?, head?)`: the intercept chain is
    /// collected from outermost to innermost, then `base` is prepended and
    /// `head` appended, and everything is shallow-merged (later layers win).
    pub fn resolve_config(
        &self,
        name: &str,
        base: Option<&serde_json::Value>,
        head: Option<&serde_json::Value>,
    ) -> serde_json::Value {
        // Collect the intercept chain outermost-first.
        let mut chain: Vec<serde_json::Value> = Vec::new();
        fn collect(ctx: &CordisContext, name: &str, chain: &mut Vec<serde_json::Value>) {
            if let Some(config) = ctx
                .intercepts
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(name)
            {
                chain.push(config.clone());
            }
            if let Some(parent) = ctx.parent.as_ref() {
                collect(parent, name, chain);
            }
        }
        collect(self, name, &mut chain);
        chain.reverse(); // outermost first
        if let Some(base) = base {
            chain.insert(0, base.clone());
        }
        if let Some(head) = head {
            chain.push(head.clone());
        }
        let mut merged = serde_json::Map::new();
        for layer in chain {
            if let serde_json::Value::Object(map) = layer {
                for (key, value) in map {
                    merged.insert(key.clone(), value.clone());
                }
            }
        }
        serde_json::Value::Object(merged)
    }

    /// Attach this context family to a runtime registry and its shared event bus.
    /// Descendants created before or after binding observe the same weak runtime handles.
    pub fn bind_registry(&self, registry: &Arc<RegistryService>) {
        *self
            .registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(registry));
        let events = registry.events();
        *self
            .events
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&events));
    }

    pub fn fiber(&self) -> &Arc<Fiber> {
        &self.fiber
    }

    pub fn events(&self) -> Result<Arc<EventsService>, String> {
        self.events
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "context is not bound to an events runtime".to_string())
    }

    pub fn registry(&self) -> Result<Arc<RegistryService>, String> {
        self.registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "context is not bound to a registry runtime".to_string())
    }

    /// Derive a dispatch context whose filter decides which listener contexts are visible.
    pub fn with_event_filter(
        &self,
        filter: impl Fn(&ContextScope) -> bool + Send + Sync + 'static,
    ) -> Self {
        let mut child = self.extend();
        child.event_filter = Some(Arc::new(filter));
        child
    }

    pub(crate) fn accepts_listener(&self, listener: &ContextScope) -> bool {
        self.event_filter
            .as_ref()
            .is_none_or(|filter| filter(listener))
    }

    pub(crate) fn scope_view(&self) -> ContextScope {
        fn collect(ctx: &CordisContext, labels: &mut HashMap<String, IsolationLabel>) {
            if let Some(parent) = ctx.parent.as_ref() {
                collect(parent, labels);
            }
            labels.extend(
                ctx.isolate_labels
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .map(|(name, label)| (name.clone(), *label)),
            );
        }
        let mut isolate_labels = HashMap::new();
        collect(self, &mut isolate_labels);
        ContextScope {
            context_id: self.context_id,
            isolate_labels,
        }
    }

    /// Register a synchronous listener owned by this context's fiber.
    pub fn on(
        &self,
        name: &str,
        handler: impl Fn(EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> Result<EventHandle, String> {
        Ok(self.events()?.on_context(self, name, handler))
    }

    /// Register an asynchronous listener owned by this context's fiber.
    pub fn on_async<F, Fut>(&self, name: &str, handler: F) -> Result<EventHandle, String>
    where
        F: Fn(EventArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<EventValue>> + Send + 'static,
    {
        Ok(self.events()?.on_async_context(self, name, handler))
    }

    /// Dispatch using this context as the upstream-style `thisArg` filter source.
    pub fn emit(&self, name: &str, args: EventArgs) -> Result<(), String> {
        self.events()?.emit_context(self, name, args);
        Ok(())
    }

    pub async fn parallel(&self, name: &str, args: EventArgs) -> Result<(), String> {
        self.events()?.parallel_context(self, name, args).await;
        Ok(())
    }

    pub async fn serial(&self, name: &str, args: EventArgs) -> Result<Option<EventValue>, String> {
        Ok(self.events()?.serial_context(self, name, args).await)
    }

    pub fn bail(&self, name: &str, args: EventArgs) -> Result<Option<EventValue>, String> {
        Ok(self.events()?.bail_context(self, name, args))
    }

    pub fn waterfall_with(
        &self,
        name: &str,
        args: EventArgs,
        inner: impl Fn(&EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> Result<Option<EventValue>, String> {
        Ok(self
            .events()?
            .waterfall_with_context(self, name, args, inner))
    }

    /// Provide a typed service in the isolate slot visible from this context. The service is
    /// automatically removed when this context's fiber is disposed.
    pub fn provide_service<T>(&self, name: &str, value: Arc<T>) -> Result<(), String>
    where
        T: Any + Send + Sync + 'static,
    {
        self.registry()?.provide_service(self, name, value, None)
    }

    pub(crate) fn provide_service_checked<T>(
        &self,
        name: &str,
        value: Arc<T>,
        check: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> Result<(), String>
    where
        T: Any + Send + Sync + 'static,
    {
        self.registry()?
            .provide_service(self, name, value, Some(check))
    }

    pub fn get_service<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Any + Send + Sync + 'static,
    {
        self.registry().ok()?.resolve_service(self, name)
    }

    pub fn has_service(&self, name: &str) -> bool {
        self.registry()
            .is_ok_and(|registry| registry.has_service(self, name))
    }

    /// Register a child plugin whose runtime is owned by this context's fiber. Disposing the
    /// parent automatically unregisters the child, matching upstream nested `ctx.plugin()`.
    pub fn plugin<T>(&self, plugin: T) -> Result<(), String>
    where
        T: Plugin + 'static,
    {
        let name = plugin.name().to_string();
        let registry = self.registry()?;
        registry.register(plugin, self)?;
        let cleanup_registry = Arc::downgrade(&registry);
        let cleanup_isolate = self.isolate_label(&name);
        let cleanup_name = name.clone();
        let cleanup = disposer(move || {
            if let Some(registry) = cleanup_registry.upgrade() {
                let _ = registry.unregister_slot(&cleanup_name, cleanup_isolate);
            }
        });
        match self.fiber.effect(move || cleanup) {
            Some(handle) => {
                handle.detach();
                Ok(())
            }
            None => {
                let _ = registry.unregister(&name, self);
                Err(format!(
                    "cannot register child plugin {name} from disposed fiber"
                ))
            }
        }
    }

    pub fn parent(&self) -> Option<&Arc<CordisContext>> {
        self.parent.as_ref()
    }

    pub fn context_id(&self) -> usize {
        self.context_id
    }

    /// Backward-compatible alias for the unique context identity.
    pub fn isolate_id(&self) -> usize {
        self.context_id
    }

    pub fn is_isolated(&self) -> bool {
        self.isolated
    }

    pub fn set_typed<T: Any + Send + Sync + 'static>(&self, key: &str, value: T) {
        let type_id = TypeId::of::<T>();
        self.deleted_typed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(type_id, key.to_string()));
        self.typed_store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(type_id)
            .or_default()
            .insert(key.to_string(), Arc::new(value));
    }

    pub fn get_typed<T: Any + Send + Sync + Clone + 'static>(&self, key: &str) -> Option<T> {
        self.get_typed_arc::<T>(key).map(|value| (*value).clone())
    }

    /// Resolve a typed dependency without requiring the dependency itself to be cloneable.
    pub fn get_typed_arc<T: Any + Send + Sync + 'static>(&self, key: &str) -> Option<Arc<T>> {
        let type_id = TypeId::of::<T>();
        if let Some(value) = self
            .typed_store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&type_id)
            .and_then(|values| values.get(key))
            .cloned()
        {
            return value.downcast::<T>().ok();
        }

        if self
            .deleted_typed
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(type_id, key.to_string()))
        {
            return None;
        }
        self.parent.as_ref()?.get_typed_arc::<T>(key)
    }

    pub fn delete_typed<T: Any + Send + Sync + 'static>(&self, key: &str) -> bool {
        let type_id = TypeId::of::<T>();
        let removed = self
            .typed_store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&type_id)
            .is_some_and(|values| values.remove(key).is_some());
        let inherited = self
            .parent
            .as_ref()
            .is_some_and(|parent| parent.get_typed_arc::<T>(key).is_some());
        self.deleted_typed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((type_id, key.to_string()));
        removed || inherited
    }
}

impl Clone for CordisContext {
    fn clone(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
            deleted_data: Arc::clone(&self.deleted_data),
            typed_store: Arc::clone(&self.typed_store),
            deleted_typed: Arc::clone(&self.deleted_typed),
            timer_name: Arc::clone(&self.timer_name),
            context_id: self.context_id,
            isolated: self.isolated,
            parent: self.parent.clone(),
            isolate_labels: Arc::clone(&self.isolate_labels),
            intercepts: Arc::clone(&self.intercepts),
            fiber: Arc::clone(&self.fiber),
            registry: Arc::clone(&self.registry),
            events: Arc::clone(&self.events),
            event_filter: self.event_filter.clone(),
        }
    }
}

impl Default for CordisContext {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CordisContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CordisContext")
            .field("context_id", &self.context_id)
            .field("isolated", &self.isolated)
            .field("has_parent", &self.parent.is_some())
            .finish()
    }
}
