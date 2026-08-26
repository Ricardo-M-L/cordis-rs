//! Entry-tree loader backed by registered Rust module factories.

use crate::entry::{Entry, EntryConfig, EntryState};
use crate::module::ModuleLoader;
use cordis_core::{CordisContext, RegistryService};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoaderConfig {
    pub base_url: Option<String>,
}

#[derive(Clone)]
struct LoaderRuntime {
    root: CordisContext,
    registry: Arc<RegistryService>,
    modules: Arc<ModuleLoader>,
}

pub struct Loader {
    config: LoaderConfig,
    entries: Arc<Mutex<Vec<Arc<Entry>>>>,
    runtime: Option<LoaderRuntime>,
}

impl Loader {
    /// Create a configuration-only loader. Use [`Loader::with_runtime`] to instantiate plugins.
    pub fn new(config: LoaderConfig) -> Self {
        Self {
            config,
            entries: Arc::new(Mutex::new(Vec::new())),
            runtime: None,
        }
    }

    pub fn with_runtime(
        config: LoaderConfig,
        root: CordisContext,
        registry: Arc<RegistryService>,
        modules: Arc<ModuleLoader>,
    ) -> Self {
        Self {
            config,
            entries: Arc::new(Mutex::new(Vec::new())),
            runtime: Some(LoaderRuntime {
                root,
                registry,
                modules,
            }),
        }
    }

    /// Backward-compatible load operation. Runtime failures are recorded on the failed entry;
    /// use [`Loader::try_load`] when the caller needs the error and atomic rollback guarantee.
    pub fn load(&mut self, entry_config: EntryConfig) -> Vec<Arc<Entry>> {
        match self.try_load(entry_config.clone()) {
            Ok(entries) => entries,
            Err(error) => {
                let entry = Arc::new(Entry::new(entry_config));
                entry.fail(error);
                lock(&self.entries).push(Arc::clone(&entry));
                vec![entry]
            }
        }
    }

    /// Load a tree atomically. If any factory, validation, or plugin apply step fails,
    /// plugins activated by this call are unloaded in reverse order.
    pub fn try_load(&self, entry_config: EntryConfig) -> Result<Vec<Arc<Entry>>, String> {
        let mut loaded = Vec::new();
        let mut activated = Vec::new();
        let result = if let Some(runtime) = &self.runtime {
            Self::load_runtime_tree(
                entry_config,
                &runtime.root,
                true,
                runtime,
                &mut loaded,
                &mut activated,
            )
        } else {
            collect_tree(entry_config, true, &mut loaded);
            Ok(())
        };

        if let Err(error) = result {
            if let Some(runtime) = &self.runtime {
                for (name, context, entry) in activated.into_iter().rev() {
                    let _ = runtime.registry.unregister(&name, &context);
                    entry.unload();
                }
            }
            return Err(error);
        }
        lock(&self.entries).extend(loaded.iter().cloned());
        Ok(loaded)
    }

    #[allow(clippy::too_many_arguments)]
    fn load_runtime_tree(
        config: EntryConfig,
        parent: &CordisContext,
        parent_enabled: bool,
        runtime: &LoaderRuntime,
        loaded: &mut Vec<Arc<Entry>>,
        activated: &mut Vec<(String, CordisContext, Arc<Entry>)>,
    ) -> Result<(), String> {
        let children = config.children.clone();
        let enabled = parent_enabled && !config.disabled;
        let entry = Arc::new(Entry::new(config));
        loaded.push(Arc::clone(&entry));

        let context = if entry.config().isolates.is_empty() {
            parent.extend()
        } else {
            parent.isolate()
        };
        context.set_typed("loader.config", entry.config().config.clone());
        context.set_typed("loader.groups", entry.config().groups.clone());
        context.set_typed("loader.isolates", entry.config().isolates.clone());

        if enabled {
            let plugin = match runtime.modules.instantiate(entry.config()) {
                Ok(plugin) => plugin,
                Err(error) => {
                    entry.fail(error.clone());
                    return Err(error);
                }
            };
            if plugin.name() != entry.name() {
                let error = format!(
                    "module {} created plugin with mismatched name {}",
                    entry.name(),
                    plugin.name()
                );
                entry.fail(error.clone());
                return Err(error);
            }
            if let Err(error) = runtime.registry.register(plugin, &context) {
                entry.fail(error.clone());
                return Err(error);
            }
            entry.activate(context.clone());
            activated.push((
                entry.name().to_string(),
                context.clone(),
                Arc::clone(&entry),
            ));
        }

        for child in children {
            Self::load_runtime_tree(child, &context, enabled, runtime, loaded, activated)?;
        }
        Ok(())
    }

    pub fn config(&self) -> &LoaderConfig {
        &self.config
    }

    pub fn entries(&self) -> Vec<Arc<Entry>> {
        lock(&self.entries).clone()
    }

    pub fn active_entries(&self) -> Vec<Arc<Entry>> {
        self.entries()
            .into_iter()
            .filter(|entry| entry.state() == EntryState::Active)
            .collect()
    }

    pub fn unload(&self, name: &str) -> Result<bool, String> {
        let Some(runtime) = &self.runtime else {
            return Err("loader has no runtime".to_string());
        };
        let entry = self
            .entries()
            .into_iter()
            .find(|entry| entry.name() == name && entry.state() == EntryState::Active);
        let Some(entry) = entry else {
            return Ok(false);
        };
        let context = entry
            .context()
            .ok_or_else(|| format!("active entry {name} has no context"))?;
        runtime.registry.unregister(name, &context)?;
        entry.unload();
        Ok(true)
    }

    pub fn reload(&self, name: &str) -> Result<(), String> {
        let Some(runtime) = &self.runtime else {
            return Err("loader has no runtime".to_string());
        };
        let entry = self
            .entries()
            .into_iter()
            .find(|entry| entry.name() == name)
            .ok_or_else(|| format!("entry not found: {name}"))?;
        let context = entry
            .context()
            .ok_or_else(|| format!("entry is not active: {name}"))?;
        let plugin = runtime.modules.instantiate(entry.config())?;
        runtime.registry.unregister(name, &context)?;
        if let Err(error) = runtime.registry.register(plugin, &context) {
            let rollback_result = runtime
                .modules
                .instantiate(entry.config())
                .and_then(|plugin| runtime.registry.register(plugin, &context));
            if rollback_result.is_ok() {
                entry.activate(context);
            } else {
                entry.fail(format!("reload failed: {error}; rollback failed"));
            }
            return Err(error);
        }
        entry.activate(context);
        Ok(())
    }
}

fn collect_tree(config: EntryConfig, parent_enabled: bool, output: &mut Vec<Arc<Entry>>) {
    let children = config.children.clone();
    let enabled = parent_enabled && !config.disabled;
    let mut entry_config = config;
    if !enabled {
        entry_config.disabled = true;
    }
    output.push(Arc::new(Entry::new(entry_config)));
    for child in children {
        collect_tree(child, enabled, output);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_core::{LoggerService, Plugin};

    struct TestPlugin(String);

    impl Plugin for TestPlugin {
        fn apply(&self, _ctx: &CordisContext) -> Result<(), String> {
            Ok(())
        }

        fn name(&self) -> &str {
            &self.0
        }
    }

    struct FailingPlugin(String);

    impl Plugin for FailingPlugin {
        fn apply(&self, _ctx: &CordisContext) -> Result<(), String> {
            Err("apply failed".to_string())
        }

        fn name(&self) -> &str {
            &self.0
        }
    }

    #[test]
    fn runtime_loader_instantiates_and_unloads_plugins() {
        let modules = Arc::new(ModuleLoader::new());
        modules
            .register("app", |config| {
                Ok(Box::new(TestPlugin(config.name.clone())))
            })
            .expect("register module");
        let registry = Arc::new(RegistryService::new(Arc::new(LoggerService::new("test"))));
        let loader = Loader::with_runtime(
            LoaderConfig::default(),
            CordisContext::new(),
            Arc::clone(&registry),
            modules,
        );
        let loaded = loader
            .try_load(EntryConfig::new("app"))
            .expect("load plugin");
        assert_eq!(loaded[0].state(), EntryState::Active);
        assert!(registry.has_plugin("app"));
        assert!(loader.unload("app").expect("unload plugin"));
        assert!(!registry.has_plugin("app"));
    }

    #[test]
    fn disabled_parent_disables_children() {
        let mut root = EntryConfig::new("root");
        root.disabled = true;
        root.children.push(EntryConfig::new("child"));
        let loader = Loader::new(LoaderConfig::default());
        let loaded = loader.try_load(root).expect("load config tree");
        assert!(loaded.iter().all(|entry| entry.is_disabled()));
    }

    #[test]
    fn failed_child_rolls_back_activated_parent() {
        let modules = Arc::new(ModuleLoader::new());
        modules
            .register("root", |config| {
                Ok(Box::new(TestPlugin(config.name.clone())))
            })
            .expect("register root");
        modules
            .register("bad", |config| {
                Ok(Box::new(FailingPlugin(config.name.clone())))
            })
            .expect("register failing child");
        let registry = Arc::new(RegistryService::new(Arc::new(LoggerService::new("test"))));
        let loader = Loader::with_runtime(
            LoaderConfig::default(),
            CordisContext::new(),
            Arc::clone(&registry),
            modules,
        );
        let mut root = EntryConfig::new("root");
        root.children.push(EntryConfig::new("bad"));
        assert!(loader.try_load(root).is_err());
        assert!(registry.plugin_names().is_empty());
        assert!(loader.entries().is_empty());
    }
}
