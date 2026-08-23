//! Plugin registry service — manages plugins and dependency injections.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::context::CordisContext;
use crate::logger::LoggerService;

// ---------------------------------------------------------------------------
// Plugin trait
// ---------------------------------------------------------------------------

/// A Cordis plugin that can be applied to and unloaded from a context.
pub trait Plugin: Send + Sync {
    /// Apply the plugin to the given context. Returns Ok(()) on success.
    fn apply(&self, ctx: &CordisContext) -> Result<(), String>;

    /// The plugin's unique name.
    fn name(&self) -> &str;

    /// Unload the plugin from the given context. Default is a no-op.
    fn unload(&self, ctx: &CordisContext) -> Result<(), String> { Ok(()) }
}

// ---------------------------------------------------------------------------
// Inject
// ---------------------------------------------------------------------------

/// A named dependency injection entry with a JSON-serialisable config payload.
#[derive(Debug)]
pub struct Inject {
    name: String,
    config: serde_json::Value,
}

impl Inject {
    /// Create an Inject from a name and a pre-built serde_json::Value.
    pub fn new(name: &str, config: serde_json::Value) -> Self {
        Inject {
            name: name.to_string(),
            config,
        }
    }

    /// Convenience: create an Inject from a name and any serde-serialisable value.
    pub fn with_config(name: &str, config: impl serde::Serialize) -> Self {
        Inject {
            name: name.to_string(),
            config: serde_json::to_value(config).unwrap_or(serde_json::Value::Null),
        }
    }

    /// Return the injection name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return a reference to the config JSON value.
    pub fn config(&self) -> &serde_json::Value {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// RegistryService
// ---------------------------------------------------------------------------

/// RegistryService tracks plugins and dependency injections.
pub struct RegistryService {
    plugins: Arc<RwLock<HashMap<String, Box<dyn Plugin>>>>,
    injects: Arc<RwLock<HashMap<String, Arc<Inject>>>>,
    logger: Arc<LoggerService>,
}

impl RegistryService {
    /// Create a new RegistryService backed by the given logger.
    pub fn new(logger: Arc<LoggerService>) -> Self {
        RegistryService {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            injects: Arc::new(RwLock::new(HashMap::new())),
            logger,
        }
    }

    /// Register a plugin. Calls `plugin.apply(ctx)` and stores the plugin by name.
    pub fn register<T: Plugin + Send + Sync + 'static>(
        &self,
        plugin: T,
        ctx: &CordisContext,
    ) -> Result<(), String> {
        let name = plugin.name().to_string();

        plugin.apply(ctx)?;

        let mut map = self.plugins.write().unwrap();
        map.insert(name, Box::new(plugin));
        Ok(())
    }

    /// Unregister a plugin by name. Calls `plugin.unload(ctx)` before removing it.
    pub fn unregister(&self, name: &str, ctx: &CordisContext) -> Result<(), String> {
        let mut map = self.plugins.write().unwrap();
        if let Some(plugin) = map.remove(name) {
            plugin.unload(ctx)?;
        }
        Ok(())
    }

    /// Check whether a plugin with the given name is registered.
    pub fn has_plugin(&self, name: &str) -> bool {
        let map = self.plugins.read().unwrap();
        map.contains_key(name)
    }

    /// Register a named dependency injection.
    pub fn register_inject(&self, name: &str, inject: Inject) {
        let mut map = self.injects.write().unwrap();
        map.insert(name.to_string(), Arc::new(inject));
    }

    /// Look up a dependency injection by name.
    pub fn get_inject(&self, name: &str) -> Option<Arc<Inject>> {
        let map = self.injects.read().unwrap();
        map.get(name).cloned()
    }
}

impl std::fmt::Debug for RegistryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plugins = self.plugins.read().unwrap();
        let injects = self.injects.read().unwrap();
        f.debug_struct("RegistryService")
            .field("plugin_count", &plugins.len())
            .field("inject_count", &injects.len())
            .finish()
    }
}
