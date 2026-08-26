//! Plugin registry with validation, dependency checks, and rollback-safe unloads.

use crate::context::CordisContext;
use crate::logger::LoggerService;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

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
    plugins: Arc<RwLock<HashMap<String, Box<dyn Plugin>>>>,
    injects: Arc<RwLock<HashMap<String, Arc<Inject>>>>,
    logger: Arc<LoggerService>,
}

impl RegistryService {
    pub fn new(logger: Arc<LoggerService>) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            injects: Arc::new(RwLock::new(HashMap::new())),
            logger,
        }
    }

    /// Register a plugin exactly once. Validation and dependency checks happen before
    /// `apply`, preventing partially registered plugins.
    pub fn register<T>(&self, plugin: T, ctx: &CordisContext) -> Result<(), String>
    where
        T: Plugin + 'static,
    {
        let name = plugin.name().to_string();
        if self.has_plugin(&name) {
            return Err(format!("plugin already registered: {name}"));
        }

        plugin.validate(ctx)?;
        for dependency in plugin.dependencies() {
            if self.get_inject(dependency).is_none() {
                return Err(format!(
                    "plugin {name} requires missing injection {dependency}"
                ));
            }
        }
        plugin.apply(ctx)?;

        let mut plugins = write(&self.plugins);
        if plugins.contains_key(&name) {
            drop(plugins);
            let _ = plugin.unload(ctx);
            return Err(format!("plugin registered concurrently: {name}"));
        }
        plugins.insert(name.clone(), Box::new(plugin));
        drop(plugins);
        self.logger
            .debug("registered plugin %s", vec![Box::new(name)]);
        Ok(())
    }

    /// Unload a plugin. A failed unload is rolled back into the registry so callers can retry.
    pub fn unregister(&self, name: &str, ctx: &CordisContext) -> Result<(), String> {
        let plugin = write(&self.plugins).remove(name);
        let Some(plugin) = plugin else {
            return Ok(());
        };

        if let Err(error) = plugin.unload(ctx) {
            write(&self.plugins).insert(name.to_string(), plugin);
            return Err(error);
        }
        self.logger
            .debug("unregistered plugin %s", vec![Box::new(name.to_string())]);
        Ok(())
    }

    pub fn has_plugin(&self, name: &str) -> bool {
        read(&self.plugins).contains_key(name)
    }

    pub fn plugin_names(&self) -> Vec<String> {
        let mut names: Vec<_> = read(&self.plugins).keys().cloned().collect();
        names.sort();
        names
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
            .finish()
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
