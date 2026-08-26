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
        root.bind_registry(&registry);
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

        let mut context = parent.extend();
        for name in &entry.config().isolates {
            context = context.isolate_name(name, None);
        }
        context.set_typed("loader.groups", entry.config().groups.clone());
        context.set_typed("loader.isolates", entry.config().isolates.clone());

        if enabled {
            let resolved_config = resolve_entry_config(entry.config(), &context);
            context.set_typed("loader.config", resolved_config.config.clone());
            let plugin = match runtime.modules.instantiate(&resolved_config) {
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

        let resolved_config = resolve_entry_config(entry.config(), &context);
        context.set_typed("loader.config", resolved_config.config.clone());
        let new_plugin = runtime.modules.instantiate(&resolved_config)?;
        if new_plugin.name() != entry.name() {
            return Err(format!(
                "module {} created plugin with mismatched name {}",
                entry.name(),
                new_plugin.name()
            ));
        }
        runtime.registry.replace(new_plugin, &context)?;
        entry.activate(context);
        Ok(())
    }
}

/// Apply the Context intercept chain to the entry's base configuration before the
/// module factory or plugin sees it. This is the explicit Rust counterpart of
/// Cordis' `Service.resolveConfig()` proxy path.
fn resolve_entry_config(config: &EntryConfig, context: &CordisContext) -> EntryConfig {
    let mut resolved = config.clone();
    resolved.config = context.resolve_config(config.name(), Some(&config.config), None);
    resolved
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
    fn runtime_chain_resolves_intercepts_isolation_events_and_fiber_cleanup() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct IntegratedPlugin {
            value: u64,
            calls: Arc<AtomicUsize>,
        }

        impl Plugin for IntegratedPlugin {
            fn apply(&self, ctx: &CordisContext) -> Result<(), String> {
                let config = ctx
                    .get_typed::<serde_json::Value>("loader.config")
                    .ok_or_else(|| "resolved loader config is missing".to_string())?;
                if config.get("value").and_then(serde_json::Value::as_u64) != Some(self.value) {
                    return Err("factory and plugin contexts saw different configs".to_string());
                }
                ctx.provide_service("worker", Arc::new(self.value))?;
                let calls = Arc::clone(&self.calls);
                ctx.on("worker/tick", move |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    None
                })?;
                Ok(())
            }

            fn name(&self) -> &str {
                "worker"
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let factory_calls = Arc::clone(&calls);
        let modules = Arc::new(ModuleLoader::new());
        modules
            .register("worker", move |config| {
                let value = config
                    .config
                    .get("value")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| "resolved value is missing".to_string())?;
                if config.config.get("baseOnly") != Some(&serde_json::json!(true))
                    || config.config.get("intercepted") != Some(&serde_json::json!(true))
                {
                    return Err("entry base and Context intercept were not merged".to_string());
                }
                Ok(Box::new(IntegratedPlugin {
                    value,
                    calls: Arc::clone(&factory_calls),
                }))
            })
            .expect("register integrated module");

        let registry = Arc::new(RegistryService::new(Arc::new(LoggerService::new(
            "integration",
        ))));
        let root = CordisContext::new().intercept(
            "worker",
            serde_json::json!({ "value": 7, "intercepted": true }),
        );
        let loader = Loader::with_runtime(
            LoaderConfig::default(),
            root.clone(),
            Arc::clone(&registry),
            modules,
        );
        let mut config = EntryConfig::new("worker");
        config.config = serde_json::json!({ "value": 1, "baseOnly": true });
        config.isolates.push("worker".to_string());
        let loaded = loader.try_load(config).expect("load integrated runtime");
        let context = loaded[0].context().expect("active entry context");

        assert!(root.get_service::<u64>("worker").is_none());
        assert_eq!(context.get_service::<u64>("worker").as_deref(), Some(&7));
        assert!(registry.has_plugin_in("worker", &context));
        let selected = context.clone();
        let dispatch =
            context.with_event_filter(move |listener| listener.shares_isolate(&selected, "worker"));
        dispatch
            .emit("worker/tick", vec![])
            .expect("dispatch scoped plugin event");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(registry.events().listener_count("worker/tick"), 1);

        assert!(loader.unload("worker").expect("unload integrated runtime"));
        assert!(context.get_service::<u64>("worker").is_none());
        assert!(!registry.has_plugin_in("worker", &context));
        assert_eq!(registry.events().listener_count("worker/tick"), 0);
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

    #[test]
    fn failed_reload_restores_original_instance() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Plugin that counts instantiations and can fail on demand.
        struct ReloadablePlugin {
            name: String,
            fail_next: Arc<std::sync::atomic::AtomicBool>,
        }

        impl Plugin for ReloadablePlugin {
            fn apply(&self, _ctx: &CordisContext) -> Result<(), String> {
                if self.fail_next.swap(false, Ordering::SeqCst) {
                    return Err("reload apply failed".to_string());
                }
                Ok(())
            }

            fn name(&self) -> &str {
                &self.name
            }
        }

        let fail_next = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let instances = Arc::new(AtomicUsize::new(0));
        let modules = Arc::new(ModuleLoader::new());
        let fail_for_factory = Arc::clone(&fail_next);
        let instances_for_factory = Arc::clone(&instances);
        modules
            .register("app", move |_config| {
                instances_for_factory.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(ReloadablePlugin {
                    name: "app".to_string(),
                    fail_next: Arc::clone(&fail_for_factory),
                }) as Box<dyn Plugin>)
            })
            .expect("register module");

        let registry = Arc::new(RegistryService::new(Arc::new(LoggerService::new("test"))));
        let loader = Loader::with_runtime(
            LoaderConfig::default(),
            CordisContext::new(),
            Arc::clone(&registry),
            modules,
        );
        loader
            .try_load(EntryConfig::new("app"))
            .expect("initial load");
        assert_eq!(instances.load(Ordering::SeqCst), 1);

        // Make the NEXT instantiation fail at apply time.
        fail_next.store(true, Ordering::SeqCst);
        assert!(loader.reload("app").is_err());
        // The factory ran (for the failed attempt) but the registry must still
        // hold the ORIGINAL instance: reload failure must not lose state.
        assert_eq!(instances.load(Ordering::SeqCst), 2);
        assert!(registry.has_plugin("app"));

        // A subsequent successful reload works.
        assert!(loader.reload("app").is_ok());
        assert_eq!(instances.load(Ordering::SeqCst), 3);
        assert!(registry.has_plugin("app"));
    }

    #[test]
    fn reload_transaction_cleans_staged_effects_and_switches_services() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        struct RuntimePlugin {
            generation: usize,
            fail_after_apply: bool,
            fail_unload: Arc<AtomicBool>,
        }

        impl Plugin for RuntimePlugin {
            fn apply(&self, ctx: &CordisContext) -> Result<(), String> {
                ctx.provide_service("app", Arc::new(self.generation))?;
                ctx.on("app/tick", |_| None)?;
                if self.fail_after_apply {
                    return Err("failed after registering effects".to_string());
                }
                Ok(())
            }

            fn name(&self) -> &str {
                "app"
            }

            fn unload(&self, _ctx: &CordisContext) -> Result<(), String> {
                if self.fail_unload.swap(false, Ordering::SeqCst) {
                    return Err("old runtime refused to unload".to_string());
                }
                Ok(())
            }
        }

        let generation = Arc::new(AtomicUsize::new(0));
        let fail_next = Arc::new(AtomicBool::new(false));
        let fail_unload = Arc::new(AtomicBool::new(false));
        let modules = Arc::new(ModuleLoader::new());
        let factory_generation = Arc::clone(&generation);
        let factory_failure = Arc::clone(&fail_next);
        let factory_unload_failure = Arc::clone(&fail_unload);
        modules
            .register("app", move |_config| {
                Ok(Box::new(RuntimePlugin {
                    generation: factory_generation.fetch_add(1, Ordering::SeqCst) + 1,
                    fail_after_apply: factory_failure.swap(false, Ordering::SeqCst),
                    fail_unload: Arc::clone(&factory_unload_failure),
                }))
            })
            .expect("register runtime factory");

        let registry = Arc::new(RegistryService::new(Arc::new(LoggerService::new("test"))));
        let root = CordisContext::new();
        let loader = Loader::with_runtime(
            LoaderConfig::default(),
            root.clone(),
            Arc::clone(&registry),
            modules,
        );
        loader
            .try_load(EntryConfig::new("app"))
            .expect("initial load");
        assert_eq!(root.get_service::<usize>("app").as_deref(), Some(&1));
        assert_eq!(registry.events().listener_count("app/tick"), 1);

        fail_unload.store(true, Ordering::SeqCst);
        assert!(loader.reload("app").is_err());
        assert_eq!(root.get_service::<usize>("app").as_deref(), Some(&1));
        assert_eq!(registry.events().listener_count("app/tick"), 1);

        fail_next.store(true, Ordering::SeqCst);
        assert!(loader.reload("app").is_err());
        assert_eq!(root.get_service::<usize>("app").as_deref(), Some(&1));
        assert_eq!(registry.events().listener_count("app/tick"), 1);

        loader.reload("app").expect("successful replacement");
        assert_eq!(root.get_service::<usize>("app").as_deref(), Some(&4));
        assert_eq!(registry.events().listener_count("app/tick"), 1);
    }
}
