//! Plugin registry with validation, dependency checks, and rollback-safe unloads.

use crate::context::{CordisContext, IsolationLabel};
use crate::events::EventsService;
use crate::fiber::{disposer, Fiber};
use crate::logger::LoggerService;
use crate::service::Service;
use std::any::Any;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock, Weak};

pub trait Plugin: Send + Sync {
    fn apply(&self, ctx: &CordisContext) -> Result<(), String>;

    fn name(&self) -> &str;

    fn unload(&self, _ctx: &CordisContext) -> Result<(), String> {
        Ok(())
    }

    /// Validate plugin configuration before side effects are applied.
    fn validate(&self, _ctx: &CordisContext) -> Result<(), String> {
        Ok(())
    }

    /// Names of injections that must exist before this plugin can be applied.
    fn dependencies(&self) -> Vec<&str> {
        Vec::new()
    }
}

impl<T: Plugin + ?Sized> Plugin for Box<T> {
    fn apply(&self, ctx: &CordisContext) -> Result<(), String> {
        (**self).apply(ctx)
    }

    fn name(&self) -> &str {
        (**self).name()
    }

    fn unload(&self, ctx: &CordisContext) -> Result<(), String> {
        (**self).unload(ctx)
    }

    fn validate(&self, ctx: &CordisContext) -> Result<(), String> {
        (**self).validate(ctx)
    }

    fn dependencies(&self) -> Vec<&str> {
        (**self).dependencies()
    }
}

type Validator = Arc<dyn Fn(&serde_json::Value) -> Result<(), String> + Send + Sync>;

/// Named dependency-injection descriptor with optional schema validation.
pub struct Inject {
    name: String,
    config: serde_json::Value,
    validator: Option<Validator>,
}

impl Inject {
    pub fn new(name: &str, config: serde_json::Value) -> Self {
        Self {
            name: name.to_string(),
            config,
            validator: None,
        }
    }

    pub fn with_config(
        name: &str,
        config: impl serde::Serialize,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self::new(name, serde_json::to_value(config)?))
    }

    pub fn try_with_config(
        name: &str,
        config: impl serde::Serialize,
    ) -> Result<Self, serde_json::Error> {
        Self::with_config(name, config)
    }

    pub fn with_validator(
        mut self,
        validator: impl Fn(&serde_json::Value) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.validator = Some(Arc::new(validator));
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(validator) = &self.validator {
            validator(&self.config)?;
        }
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn config(&self) -> &serde_json::Value {
        &self.config
    }
}

impl std::fmt::Debug for Inject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Inject")
            .field("name", &self.name)
            .field("config", &self.config)
            .field("validated", &self.validator.is_some())
            .finish()
    }
}

pub struct RegistryService {
    plugins: Arc<RwLock<HashMap<ScopedKey, PluginRuntime>>>,
    injects: Arc<RwLock<HashMap<String, Arc<Inject>>>>,
    services: Arc<RwLock<HashMap<ScopedKey, Vec<ServiceRecord>>>>,
    logger: Arc<LoggerService>,
    events: Arc<EventsService>,
    pending_operations: Mutex<HashSet<ScopedKey>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ScopedKey {
    name: String,
    isolate: Option<IsolationLabel>,
}

struct PluginRuntime {
    plugin: Box<dyn Plugin>,
    context: CordisContext,
    fiber: Arc<Fiber>,
}

struct ServiceRecord {
    owner_id: usize,
    owner: Weak<Fiber>,
    value: Arc<dyn Any + Send + Sync>,
    check: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
}

struct OperationGuard<'a> {
    pending: &'a Mutex<HashSet<ScopedKey>>,
    key: ScopedKey,
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        mutex_lock(self.pending).remove(&self.key);
    }
}

struct ServicePlugin<T: Service + 'static> {
    service: Arc<T>,
}

impl<T: Service + 'static> Plugin for ServicePlugin<T> {
    fn apply(&self, ctx: &CordisContext) -> Result<(), String> {
        self.service.init()?;
        let checked = Arc::clone(&self.service);
        ctx.provide_service_checked(
            self.service.name(),
            Arc::clone(&self.service),
            Arc::new(move || checked.check()),
        )
    }

    fn name(&self) -> &str {
        self.service.name()
    }
}

impl RegistryService {
    pub fn new(logger: Arc<LoggerService>) -> Self {
        let events = Arc::new(EventsService::new(Arc::clone(&logger)));
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            injects: Arc::new(RwLock::new(HashMap::new())),
            services: Arc::new(RwLock::new(HashMap::new())),
            logger,
            events,
            pending_operations: Mutex::new(HashSet::new()),
        }
    }

    pub fn events(&self) -> Arc<EventsService> {
        Arc::clone(&self.events)
    }

    fn key(ctx: &CordisContext, name: &str) -> ScopedKey {
        ScopedKey {
            name: name.to_string(),
            isolate: ctx.isolate_label(name),
        }
    }

    fn reserve(&self, key: ScopedKey) -> Result<OperationGuard<'_>, String> {
        let mut pending = mutex_lock(&self.pending_operations);
        if !pending.insert(key.clone()) {
            return Err(format!(
                "plugin operation already in progress: {}",
                key.name
            ));
        }
        Ok(OperationGuard {
            pending: &self.pending_operations,
            key,
        })
    }

    /// Register a plugin exactly once. Validation and dependency checks happen before
    /// `apply`. Effects created by a failed apply are disposed before the error returns.
    pub fn register<T>(&self, plugin: T, ctx: &CordisContext) -> Result<(), String>
    where
        T: Plugin + 'static,
    {
        let name = plugin.name().to_string();
        let key = Self::key(ctx, &name);
        let _operation = self.reserve(key.clone())?;
        if read(&self.plugins).contains_key(&key) {
            return Err(format!("plugin already registered: {name}"));
        }
        let runtime = self.prepare(Box::new(plugin), ctx)?;
        let fiber = Arc::clone(&runtime.fiber);
        let mut plugins = write(&self.plugins);
        plugins.insert(key, runtime);
        // Publish the plugin runtime before its services and listeners become visible.
        // Readers of the plugin map are blocked by `plugins` until activation completes.
        fiber.activate();
        drop(plugins);
        self.logger
            .debug("registered plugin %s", vec![Box::new(name)]);
        Ok(())
    }

    /// Register a Cordis service through the same plugin/fiber lifecycle as ordinary plugins.
    /// `Service::init` runs during preparation, `check` gates resolution, and the provided value
    /// disappears automatically when the owning fiber unloads.
    pub fn register_service<T>(&self, service: T, ctx: &CordisContext) -> Result<(), String>
    where
        T: Service + 'static,
    {
        self.register(
            ServicePlugin {
                service: Arc::new(service),
            },
            ctx,
        )
    }

    fn prepare(
        &self,
        plugin: Box<dyn Plugin>,
        parent: &CordisContext,
    ) -> Result<PluginRuntime, String> {
        let name = plugin.name().to_string();
        let fiber = Arc::new(Fiber::new());
        fiber.try_transition_to(crate::fiber::FiberState::Loading)?;
        let context = parent.with_fiber(Arc::clone(&fiber));

        let prepare_result = (|| {
            plugin.validate(&context)?;
            for dependency in plugin.dependencies() {
                if !context.has_service(dependency) {
                    return Err(format!(
                        "plugin {name} requires missing service {dependency}"
                    ));
                }
            }
            plugin.apply(&context)
        })();

        if let Err(error) = prepare_result {
            let _ = plugin.unload(&context);
            fiber.dispose();
            return Err(error);
        }
        Ok(PluginRuntime {
            plugin,
            context,
            fiber,
        })
    }

    /// Stage a replacement while the old runtime remains active. The staged fiber is invisible
    /// to other contexts until apply succeeds. Then the old runtime is unloaded and disposed
    /// before the new one becomes active. Failed preparation leaves the old runtime untouched.
    pub fn replace<T>(&self, plugin: T, ctx: &CordisContext) -> Result<(), String>
    where
        T: Plugin + 'static,
    {
        let name = plugin.name().to_string();
        let key = Self::key(ctx, &name);
        let _operation = self.reserve(key.clone())?;
        if !read(&self.plugins).contains_key(&key) {
            return Err(format!("plugin not registered: {name}"));
        }

        let staged = self.prepare(Box::new(plugin), ctx)?;
        let old = write(&self.plugins)
            .remove(&key)
            .expect("plugin presence checked under operation lock");
        if let Err(error) = old.plugin.unload(&old.context) {
            let _ = staged.plugin.unload(&staged.context);
            staged.fiber.dispose();
            write(&self.plugins).insert(key, old);
            return Err(format!("failed to unload previous {name}: {error}"));
        }
        old.fiber.dispose();
        let fiber = Arc::clone(&staged.fiber);
        let mut plugins = write(&self.plugins);
        plugins.insert(key, staged);
        // Keep plugin-map visibility and Fiber-backed resource visibility in commit order.
        fiber.activate();
        drop(plugins);
        self.logger
            .debug("replaced plugin %s", vec![Box::new(name)]);
        Ok(())
    }

    /// Unload a plugin. A failed unload is rolled back into the registry so callers can retry.
    pub fn unregister(&self, name: &str, ctx: &CordisContext) -> Result<(), String> {
        self.unregister_key(Self::key(ctx, name))
    }

    pub(crate) fn unregister_slot(
        &self,
        name: &str,
        isolate: Option<IsolationLabel>,
    ) -> Result<(), String> {
        self.unregister_key(ScopedKey {
            name: name.to_string(),
            isolate,
        })
    }

    fn unregister_key(&self, key: ScopedKey) -> Result<(), String> {
        let name = key.name.clone();
        let _operation = self.reserve(key.clone())?;
        let runtime = write(&self.plugins).remove(&key);
        let Some(runtime) = runtime else {
            return Ok(());
        };

        if let Err(error) = runtime.plugin.unload(&runtime.context) {
            write(&self.plugins).insert(key, runtime);
            return Err(error);
        }
        runtime.fiber.dispose();
        self.logger
            .debug("unregistered plugin %s", vec![Box::new(name)]);
        Ok(())
    }

    pub fn has_plugin(&self, name: &str) -> bool {
        read(&self.plugins).keys().any(|key| key.name == name)
    }

    pub fn has_plugin_in(&self, name: &str, ctx: &CordisContext) -> bool {
        read(&self.plugins).contains_key(&Self::key(ctx, name))
    }

    pub fn plugin_names(&self) -> Vec<String> {
        read(&self.plugins)
            .keys()
            .map(|key| key.name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn provide_service<T>(
        &self,
        ctx: &CordisContext,
        name: &str,
        value: Arc<T>,
        check: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    ) -> Result<(), String>
    where
        T: Any + Send + Sync + 'static,
    {
        let key = Self::key(ctx, name);
        let fiber = Arc::clone(ctx.fiber());
        let owner_id = fiber.id();
        {
            let mut services = write(&self.services);
            let records = services.entry(key.clone()).or_default();
            if records.iter().any(|record| record.owner_id == owner_id) {
                return Err(format!("service already provided by this fiber: {name}"));
            }
            records.push(ServiceRecord {
                owner_id,
                owner: Arc::downgrade(&fiber),
                value,
                check,
            });
        }

        let services = Arc::clone(&self.services);
        let cleanup_key = key.clone();
        let cleanup = disposer(move || {
            let mut services = write(&services);
            if let Some(records) = services.get_mut(&cleanup_key) {
                records.retain(|record| record.owner_id != owner_id);
                if records.is_empty() {
                    services.remove(&cleanup_key);
                }
            }
        });
        match fiber.effect(move || cleanup) {
            Some(handle) => {
                handle.detach();
                Ok(())
            }
            None => {
                let mut services = write(&self.services);
                if let Some(records) = services.get_mut(&key) {
                    records.retain(|record| record.owner_id != owner_id);
                    if records.is_empty() {
                        services.remove(&key);
                    }
                }
                Err(format!("cannot provide service {name} from disposed fiber"))
            }
        }
    }

    pub fn resolve_service<T>(&self, ctx: &CordisContext, name: &str) -> Option<Arc<T>>
    where
        T: Any + Send + Sync + 'static,
    {
        let key = Self::key(ctx, name);
        let current_owner = ctx.fiber().id();
        let services = read(&self.services);
        let records = services.get(&key)?;
        records.iter().rev().find_map(|record| {
            let owner = record.owner.upgrade()?;
            if record.owner_id != current_owner && !owner.is_active() {
                return None;
            }
            if record.check.as_ref().is_some_and(|check| check().is_err()) {
                return None;
            }
            Arc::clone(&record.value).downcast::<T>().ok()
        })
    }

    pub fn has_service(&self, ctx: &CordisContext, name: &str) -> bool {
        let key = Self::key(ctx, name);
        let current_owner = ctx.fiber().id();
        read(&self.services).get(&key).is_some_and(|records| {
            records.iter().rev().any(|record| {
                let Some(owner) = record.owner.upgrade() else {
                    return false;
                };
                (record.owner_id == current_owner || owner.is_active())
                    && record.check.as_ref().is_none_or(|check| check().is_ok())
            })
        })
    }

    pub fn try_register_inject(&self, name: &str, inject: Inject) -> Result<(), String> {
        if name != inject.name() {
            return Err(format!(
                "injection key {name} does not match descriptor name {}",
                inject.name()
            ));
        }
        inject.validate()?;
        write(&self.injects).insert(name.to_string(), Arc::new(inject));
        Ok(())
    }

    /// Backward-compatible registration helper. Invalid descriptors are rejected and logged.
    pub fn register_inject(&self, name: &str, inject: Inject) {
        if let Err(error) = self.try_register_inject(name, inject) {
            self.logger
                .error("failed to register injection: %s", vec![Box::new(error)]);
        }
    }

    pub fn get_inject(&self, name: &str) -> Option<Arc<Inject>> {
        read(&self.injects).get(name).cloned()
    }

    pub fn unregister_inject(&self, name: &str) -> Option<Arc<Inject>> {
        write(&self.injects).remove(name)
    }
}

impl std::fmt::Debug for RegistryService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegistryService")
            .field("plugin_count", &read(&self.plugins).len())
            .field("inject_count", &read(&self.injects).len())
            .field("service_slots", &read(&self.services).len())
            .finish()
    }
}

impl Drop for RegistryService {
    fn drop(&mut self) {
        let plugins = self
            .plugins
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|(_, runtime)| runtime)
            .collect::<Vec<_>>();
        for runtime in plugins {
            let _ = runtime.plugin.unload(&runtime.context);
            runtime.fiber.dispose();
        }
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn mutex_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
