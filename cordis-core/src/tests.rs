//! Integration tests for the cordis-core crate.

use crate::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

// -------------------------
// Fiber
// -------------------------

#[test]
fn test_fiber_basic() {
    let f = crate::fiber::Fiber::new();
    assert!(!f.is_ready());
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
    registry
        .register(TestPlugin::new("remove-me"), &ctx)
        .unwrap();
    registry.unregister("remove-me", &ctx).unwrap();
    assert!(!registry.has_plugin("remove-me"));
}

struct FailingUnloadPlugin;

impl registry::Plugin for FailingUnloadPlugin {
    fn apply(&self, _ctx: &CordisContext) -> Result<(), String> {
        Ok(())
    }

    fn name(&self) -> &str {
        "failing-unload"
    }

    fn unload(&self, _ctx: &CordisContext) -> Result<(), String> {
        Err("still busy".to_string())
    }
}

#[test]
fn test_registry_rolls_back_failed_unload() {
    let registry = registry::RegistryService::new(Arc::new(logger::LoggerService::new("registry")));
    let ctx = CordisContext::new();
    registry
        .register(FailingUnloadPlugin, &ctx)
        .expect("register plugin");
    assert!(registry.unregister("failing-unload", &ctx).is_err());
    assert!(registry.has_plugin("failing-unload"));
}

#[test]
fn test_registry_inject() {
    let logger = Arc::new(logger::LoggerService::new("registry-inject"));
    let registry = registry::RegistryService::new(logger);
    registry.register_inject(
        "db",
        registry::Inject::with_config("db", 42_i32).expect("serialize injection"),
    );
    assert!(registry.get_inject("db").is_some());
}

#[test]
fn test_registry_validates_injections() {
    let registry = registry::RegistryService::new(Arc::new(logger::LoggerService::new("registry")));
    let inject = registry::Inject::new("port", serde_json::json!(-1)).with_validator(|value| {
        value
            .as_u64()
            .filter(|port| *port > 0)
            .map(|_| ())
            .ok_or_else(|| "port must be positive".to_string())
    });
    assert!(registry.try_register_inject("port", inject).is_err());
    assert!(registry.get_inject("port").is_none());
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
        f.debug_struct("MyService")
            .field("name", &self.name)
            .finish()
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
    let svc = MyService {
        name: "my-service".to_string(),
    };
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
    let list: DisposableList<i32> = DisposableList::new();
    let first = list.push(1);
    let second = list.push(2);
    let third = list.push(3);
    assert_eq!(list.len(), 3);
    drop(second);
    assert_eq!(list.len(), 2);
    let values = list.clear();
    assert_eq!(values, vec![3, 1]);
    assert_eq!(list.len(), 0);
    drop((first, third));
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
    let ctx = CordisContext::new();
    ctx.set("a", 1);
    assert_eq!(ctx.get("a"), Some(1));
    assert!(ctx.has("a"));
    ctx.delete("a");
    assert!(!ctx.has("a"));
    ctx.set_timer("t1");
    assert_eq!(ctx.get_timer_name(), Some("t1".to_string()));
}

#[test]
fn test_context_child_scope_shadows_parent() {
    let parent = CordisContext::new();
    parent.set("port", 3000);
    parent.set_typed("name", "parent".to_string());
    let child = parent.extend();
    assert_eq!(child.get("port"), Some(3000));
    child.set("port", 8080);
    child.set_typed("name", "child".to_string());
    assert_eq!(child.get("port"), Some(8080));
    assert_eq!(parent.get("port"), Some(3000));
    assert_eq!(child.get_typed::<String>("name").as_deref(), Some("child"));
    assert_eq!(
        parent.get_typed::<String>("name").as_deref(),
        Some("parent")
    );
    assert_ne!(child.context_id(), parent.context_id());
}

#[test]
fn test_fiber_effect_value_and_cleanup() {
    let fiber = Fiber::new();
    let value_handle = fiber.effect(|| 42_u64).expect("value effect");
    assert_eq!(fiber.get::<u64>(), Some(42));
    drop(value_handle);

    let cleaned = Arc::new(AtomicUsize::new(0));
    let cleanup_counter = Arc::clone(&cleaned);
    let cleanup_handle = fiber
        .effect(move || {
            disposer(move || {
                cleanup_counter.fetch_add(1, Ordering::SeqCst);
            })
        })
        .expect("cleanup effect");
    assert_eq!(fiber.active_effects(), 1);
    fiber.dispose();
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    drop(cleanup_handle);
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
}

#[test]
fn test_fiber_update_notifies_hooks() {
    let fiber = Fiber::new();
    fiber.activate();
    let observed = Arc::new(AtomicUsize::new(0));
    let hook_observed = Arc::clone(&observed);
    let hook = fiber.on_update(move |revision| {
        hook_observed.store(revision, Ordering::SeqCst);
    });
    fiber.update();
    assert_eq!(fiber.revision(), 1);
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    assert!(fiber.remove_update_hook(hook));
}

#[test]
fn test_logger_unicode_truncation_and_placeholders() {
    let message = logger::Message {
        timestamp: 0,
        msg: "你好 %s %d %o".to_string(),
        args: vec![
            Box::new("世界".to_string()),
            Box::new(7_u64),
            Box::new(serde_json::json!({"ok": true})),
        ],
        level: logger::LoggerLevel::Info,
        name: "test".to_string(),
    };
    assert_eq!(message.formatted_body(), "你好 世界 7 {\"ok\":true}");
    let truncated = message.to_string(Some(5));
    assert_eq!(truncated.chars().count(), 8);
    assert!(truncated.ends_with("..."));
}

#[test]
fn test_events_parallel_is_concurrent() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let logger = Arc::new(logger::LoggerService::new("events-parallel"));
        let events = events::EventsService::new(logger);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let first_barrier = Arc::clone(&barrier);
        events.on("slow", move |_| {
            first_barrier.wait();
            None
        });
        let second_barrier = Arc::clone(&barrier);
        events.on("slow", move |_| {
            second_barrier.wait();
            None
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            events.parallel("slow", vec![]),
        )
        .await
        .expect("parallel handlers must run concurrently");
    });
}

#[test]
fn test_event_handle_unregisters_listener() {
    let events = events::EventsService::new(Arc::new(logger::LoggerService::new("events-off")));
    let calls = Arc::new(AtomicUsize::new(0));
    let listener_calls = Arc::clone(&calls);
    let handle = events.on("event", move |_| {
        listener_calls.fetch_add(1, Ordering::SeqCst);
        None
    });
    events.emit("event", vec![]);
    assert!(handle.dispose());
    events.emit("event", vec![]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn test_async_event_handlers() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let events = events::EventsService::new(Arc::new(logger::LoggerService::new("async")));
        let calls = Arc::new(AtomicUsize::new(0));
        let async_calls = Arc::clone(&calls);
        events.on_async("event", move |_| {
            let async_calls = Arc::clone(&async_calls);
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                async_calls.fetch_add(1, Ordering::SeqCst);
                Some(Arc::new(42_u64) as events::EventValue)
            }
        });
        let result = events.serial_async("event", vec![]).await;
        assert_eq!(
            result.and_then(|value| value.downcast_ref::<u64>().copied()),
            Some(42)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    });
}
