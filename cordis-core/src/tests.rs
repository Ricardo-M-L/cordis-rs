//! Integration tests for the cordis-core crate.

use crate::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// -------------------------
// Fiber
// -------------------------

#[test]
fn test_fiber_basic() {
    let f = crate::fiber::Fiber::new();
    assert_eq!(f.is_ready(), false);
}

#[test]
fn test_fiber_state_machine() {
    let fiber = crate::fiber::Fiber::new();
    assert_eq!(fiber.state(), FiberState::Pending);
    fiber.activate();
    assert_eq!(fiber.state(), FiberState::Active);
    fiber.fail();
    assert_eq!(fiber.state(), FiberState::Failed);
    fiber.restart();
    assert_eq!(fiber.state(), FiberState::Active);
}

// -------------------------
// Events
// -------------------------

#[test]
fn test_events_emit() {
    let logger = Arc::new(logger::LoggerService::new("events"));
    let events = events::EventsService::new(logger);
    let c = Arc::new(AtomicUsize::new(0));
    let c1 = c.clone();
    events.on("test", move |_| {
        c1.fetch_add(1, Ordering::SeqCst);
        None
    });
    let c2 = c.clone();
    events.on("test", move |_| {
        c2.fetch_add(1, Ordering::SeqCst);
        None
    });
    events.emit("test", vec![]);
    assert_eq!(c.load(Ordering::SeqCst), 2);
}

#[test]
fn test_events_serial() {
    let logger = Arc::new(logger::LoggerService::new("events-serial"));
    let events = events::EventsService::new(logger);
    events.on("ser", move |_| {
        Some(Arc::new(42i32) as Arc<dyn std::any::Any + Send + Sync>)
    });
    assert!(events.serial("ser", vec![]).is_some());
}

#[test]
fn test_events_bail() {
    let logger = Arc::new(logger::LoggerService::new("events-bail"));
    let events = events::EventsService::new(logger);
    let c = Arc::new(AtomicUsize::new(0));
    let c1 = c.clone();
    events.on("bail", move |_| {
        c1.fetch_add(1, Ordering::SeqCst);
        Some(Arc::new("stopped") as Arc<dyn std::any::Any + Send + Sync>)
    });
    let c2 = c.clone();
    events.on("bail", move |_| {
        c2.fetch_add(1, Ordering::SeqCst);
        None
    });
    assert!(events.bail("bail", vec![]).is_some());
    assert_eq!(c.load(Ordering::SeqCst), 1);
}

// -------------------------
// Registry
// -------------------------

struct TestPlugin {
    name: String,
    applied: AtomicBool,
}
impl TestPlugin {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            applied: AtomicBool::new(false),
        }
    }
}
impl registry::Plugin for TestPlugin {
    fn apply(&self, _ctx: &CordisContext) -> Result<(), String> {
        self.applied.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn name(&self) -> &str {
        &self.name
    }
}

#[test]
fn test_registry_register_plugin() {
    let logger = Arc::new(logger::LoggerService::new("registry"));
    let registry = registry::RegistryService::new(logger);
    let ctx = CordisContext::new();
    let plugin = TestPlugin::new("test-plugin");
    registry.register(plugin, &ctx).unwrap();
    assert!(registry.has_plugin("test-plugin"));
}

#[test]
fn test_registry_unregister_plugin() {
    let logger = Arc::new(logger::LoggerService::new("registry-unreg"));
    let registry = registry::RegistryService::new(logger);
    let ctx = CordisContext::new();
    registry.register(TestPlugin::new("remove-me"), &ctx).unwrap();
    registry.unregister("remove-me", &ctx).unwrap();
    assert!(!registry.has_plugin("remove-me"));
}

#[test]
fn test_registry_inject() {
    let logger = Arc::new(logger::LoggerService::new("registry-inject"));
    let registry = registry::RegistryService::new(logger);
    registry.register_inject("db", registry::Inject::with_config("db", 42i32));
    assert!(registry.get_inject("db").is_some());
}

// -------------------------
// Reflect
// -------------------------

#[test]
fn test_reflect_provide_get() {
    let mut reflect = reflect::Reflect::new();
    reflect.provide::<String>("key1", "value1".to_string());
    assert_eq!(reflect.get::<String>("key1"), Some("value1".to_string()));
}

#[test]
fn test_reflect_delete() {
    let mut reflect = reflect::Reflect::new();
    reflect.provide::<String>("k", "v".to_string());
    reflect.delete::<String>("k");
    assert!(reflect.get::<String>("k").is_none());
}

// -------------------------
// Logger
// -------------------------

#[test]
fn test_logger_message() {
    use logger::Exporter;
    let received = Arc::new(AtomicUsize::new(0));
    #[derive(Debug)]
    struct CountingExporter {
        count: Arc<AtomicUsize>,
    }
    impl Exporter for CountingExporter {
        fn colors(&self) -> bool {
            false
        }
        fn max_length(&self) -> Option<usize> {
            None
        }
        fn export(&self, _msg: &logger::Message) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }
    let logger = logger::LoggerService::new("msg-test");
    logger.set_level(logger::LoggerLevel::Debug);
    logger.add_exporter(Box::new(CountingExporter {
        count: received.clone(),
    }));
    logger.info("hello", vec![Box::new("world")]);
    logger.warn("warn", vec![]);
    logger.error("err", vec![]);
    assert_eq!(received.load(Ordering::SeqCst), 3);
}

// -------------------------
// Service
// -------------------------

struct MyService {
    name: String,
}
impl std::fmt::Debug for MyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MyService").field("name", &self.name).finish()
    }
}
impl service::Service for MyService {
    fn name(&self) -> &str {
        &self.name
    }
    fn init(&self) -> Result<(), String> {
        Ok(())
    }
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn test_service_init() {
    let svc = MyService { name: "my-service".to_string() };
    assert_eq!(svc.name(), "my-service");
    assert!(svc.init().is_ok());
    assert!(svc.check().is_ok());
}

// -------------------------
// Utils
// -------------------------

#[test]
fn test_disposable_list() {
    use utils::DisposableList;
    let mut list: DisposableList<i32> = DisposableList::new();
    list.push(1);
    list.push(2);
    list.push(3);
    assert_eq!(list.len(), 3);
    let values = list.clear();
    assert_eq!(values.len(), 3);
    assert!(values.contains(&1));
    assert!(values.contains(&2));
    assert!(values.contains(&3));
    assert_eq!(list.len(), 0);
}

#[test]
fn test_tracker() {
    let tracker = utils::Tracker {
        associate: Some("service-name".to_string()),
        property: Some("ctx".to_string()),
        no_shadow: false,
    };
    assert_eq!(tracker.associate, Some("service-name".to_string()));
}

#[test]
fn test_context_basic() {
    let mut ctx = CordisContext::new();
    ctx.set("a", 1);
    assert_eq!(ctx.get("a"), Some(1));
    assert!(ctx.has("a"));
    ctx.delete("a");
    assert!(!ctx.has("a"));
    ctx.set_timer("t1");
    assert_eq!(ctx.get_timer_name(), Some("t1".to_string()));
}

// -------------------------
// Cross-crate smoke tests
// -------------------------

#[test]
fn test_timer_timeout() {
    let timer = cordis_timer::TimerService::new("test-timer");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let timer_clone = timer.clone();
    runtime.block_on(async move {
        let done = Arc::new(AtomicBool::new(false));
        let d = done.clone();
        timer_clone.timeout(
            move || {
                d.store(true, Ordering::SeqCst);
            },
            100,
        );
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(done.load(Ordering::SeqCst));
    });
}

#[test]
fn test_timer_interval_stop() {
    let timer = cordis_timer::TimerService::new("interval-test");
    let done = Arc::new(AtomicUsize::new(0));
    let timer_clone = timer.clone();
    let done_clone = done.clone();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async move {
        let d = done_clone.clone();
        let handle = timer_clone.interval(
            move || {
                d.fetch_add(1, Ordering::SeqCst);
            },
            50,
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        handle.stop();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });
    let count = done.load(Ordering::SeqCst);
    assert!(count >= 2 && count <= 5);
}

#[test]
fn test_loader_entry_tree() {
    use cordis_loader::{Entry, EntryConfig, Loader, LoaderConfig};
    let root = EntryConfig {
        name: "app".to_string(),
        children: vec![],
        groups: vec![],
        isolates: vec![],
        disabled: false,
        config: serde_json::Value::Null,
    };
    let mut loader = Loader::new(LoaderConfig { base_url: None });
    let entry = Arc::new(Entry::new(root.clone()));
    let _ = entry;
    loader.load(root);
    assert_eq!(loader.config().base_url, None);
}

#[test]
fn test_module_loader() {
    use cordis_loader::ModuleLoader;
    let loader = ModuleLoader::new();
    loader.add_job("file:///app/main.js");
    loader.add_job("file:///app/util.js");
    assert_eq!(loader.jobs().len(), 2);
    assert_eq!(loader.resolve("file:///test"), "file:///test");
}

#[test]
fn test_hmr_register_dep() {
    use cordis_hmr::{Hmr, HmrConfig};
    let hmr = Hmr::new("test-hmr", HmrConfig::default());
    hmr.register_dep("main.js", "util.js");
    hmr.register_dep("main.js", "config.js");
    assert_eq!(hmr.deps("main.js").len(), 2);
}

#[test]
fn test_hmr_events() {
    use cordis_hmr::{Hmr, HmrConfig, HmrEvent};
    let hmr = Hmr::new("test-hmr", HmrConfig::default());
    hmr.simulate_change("src/app.ts");
    let events = hmr.events();
    assert_eq!(events.len(), 1);
    match &events[0] {
        HmrEvent::Changed(p) => assert_eq!(p, "src/app.ts"),
        _ => panic!("expected Changed event"),
    }
}

#[test]
fn test_console_exporter_config() {
    use cordis_logger_console::ConsoleExporter;
    let exporter = ConsoleExporter::default();
    assert_eq!(exporter.config.colors, true);
    assert_eq!(exporter.config.max_length, Some(1024));
}

#[test]
fn test_include_patch() {
    use std::collections::HashMap;
    use cordis_include::{IncludePlugin, Patch};
    let patch = Patch::new("db.host", serde_json::json!("localhost"));
    let include = IncludePlugin::with_patches("test-include", vec![patch]);
    let mut config: HashMap<String, serde_json::Value> = HashMap::new();
    include.apply_patches(&mut config);
    assert!(config.contains_key("db"));
}

#[test]
fn test_group_plugin() {
    use cordis_group::Group;
    let mut group = Group::new("test-group");
    group.add_entry("entry-1");
    group.add_entry("entry-2");
    assert_eq!(group.name(), "test-group");
    assert_eq!(group.entries().len(), 2);
}
