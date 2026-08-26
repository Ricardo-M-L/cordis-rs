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
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let logger = Arc::new(logger::LoggerService::new("events-serial"));
        let events = events::EventsService::new(logger);
        events.on("ser", move |_| {
            Some(Arc::new(42i32) as Arc<dyn std::any::Any + Send + Sync>)
        });
        assert!(events.serial("ser", vec![]).await.is_some());
    });
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
    // Not a callable service: the invoke protocol is opt-in.
    assert!(svc.invoke(&[]).is_none());
}

/// A callable service, mirroring upstream services that implement `symbols.invoke`
/// (e.g. `Logger` being callable as `logger('name')`).
struct CallableLogger {
    base_name: String,
}

impl service::Service for CallableLogger {
    fn name(&self) -> &str {
        &self.base_name
    }

    fn invoke(
        &self,
        args: &[Box<dyn std::any::Any + Send + Sync>],
    ) -> Option<Result<Box<dyn std::any::Any + Send + Sync>, String>> {
        // Called like `logger("child")`: derive a child logger name.
        let child = args
            .first()
            .and_then(|arg| arg.downcast_ref::<String>())
            .cloned()
            .unwrap_or_default();
        Some(Ok(Box::new(format!("{}/{}", self.base_name, child))))
    }
}

#[test]
fn test_callable_service_protocol() {
    let logger = CallableLogger {
        base_name: "app".to_string(),
    };
    let result = logger
        .invoke(&[Box::new("db".to_string())])
        .expect("callable service returns Some")
        .expect("invoke succeeds");
    assert_eq!(
        result.downcast_ref::<String>().map(String::as_str),
        Some("app/db")
    );
}

// -------------------------
// Context isolate / intercept
// -------------------------

#[test]
fn test_context_isolate_per_name() {
    // Mirrors upstream isolate.spec.ts "isolated context": two scopes isolated
    // under the same name do NOT see each other's provides, but the root does.
    let root = CordisContext::new();
    let ctx1 = root.isolate_name("foo", None);
    let ctx2 = root.isolate_name("foo", None);
    // Distinct auto labels -> distinct service slots.
    assert!(!ctx1.shares_isolate(&ctx2, "foo"));
    // Both still share the root's slot for other names.
    assert!(ctx1.shares_isolate(&ctx2, "bar"));
    assert!(ctx1.shares_isolate(&root, "bar"));
    // Values are still inherited through extend().
    root.set("inherited", 100);
    assert_eq!(ctx1.get("inherited"), Some(100));
}

#[test]
fn test_context_isolate_shared_label() {
    // Mirrors upstream isolate.spec.ts "shared label": two scopes isolated with
    // the SAME label share one service slot for that name.
    let root = CordisContext::new();
    let ctx1 = root.isolate_name("foo", Some(7));
    let ctx2 = root.isolate_name("foo", Some(7));
    assert!(ctx1.shares_isolate(&ctx2, "foo"));
    assert!(!ctx1.shares_isolate(&root, "foo"));
}

#[test]
fn test_generated_isolate_never_collides_with_shared_label() {
    let root = CordisContext::new();
    let generated = root.isolate_name("foo", None);
    let explicit = root.isolate_name("foo", Some(1));
    assert!(!generated.shares_isolate(&explicit, "foo"));
}

#[test]
fn test_context_intercept_layering() {
    // Mirrors upstream Service[symbols.resolveConfig]: intercepts layer
    // outermost-first; later (inner) layers override earlier keys; `head`
    // wins over intercepts and `base` is overridden by everything.
    let root = CordisContext::new();
    let mid = root.intercept(
        "logger",
        serde_json::json!({ "level": "info", "name": "root" }),
    );
    let leaf = mid.intercept("logger", serde_json::json!({ "level": "debug" }));
    let resolved = leaf.resolve_config(
        "logger",
        Some(&serde_json::json!({ "level": "error", "baseOnly": true })),
        Some(&serde_json::json!({ "headOnly": true })),
    );
    assert_eq!(resolved["level"], serde_json::json!("debug"));
    assert_eq!(resolved["name"], serde_json::json!("root"));
    assert_eq!(resolved["baseOnly"], serde_json::json!(true));
    assert_eq!(resolved["headOnly"], serde_json::json!(true));
    // An un-intercepted name resolves to just base + head.
    let plain = leaf.resolve_config("other", Some(&serde_json::json!({ "a": 1 })), None);
    assert_eq!(plain, serde_json::json!({ "a": 1 }));
}

struct ScopedRuntimePlugin {
    value: u64,
    calls: Arc<AtomicUsize>,
}

impl registry::Plugin for ScopedRuntimePlugin {
    fn apply(&self, ctx: &CordisContext) -> Result<(), String> {
        ctx.provide_service("worker", Arc::new(self.value))?;
        let calls = Arc::clone(&self.calls);
        ctx.on("runtime/tick", move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        })?;
        Ok(())
    }

    fn name(&self) -> &str {
        "worker"
    }
}

#[test]
fn test_context_registry_fiber_events_runtime_chain() {
    let logger = Arc::new(logger::LoggerService::new("runtime-chain"));
    let registry = Arc::new(registry::RegistryService::new(logger));
    let root = CordisContext::new();
    root.bind_registry(&registry);

    let scope_a = root.isolate_name("worker", None);
    let scope_b = root.isolate_name("worker", None);
    let calls_a = Arc::new(AtomicUsize::new(0));
    let calls_b = Arc::new(AtomicUsize::new(0));
    registry
        .register(
            ScopedRuntimePlugin {
                value: 1,
                calls: Arc::clone(&calls_a),
            },
            &scope_a,
        )
        .expect("register isolated worker A");
    registry
        .register(
            ScopedRuntimePlugin {
                value: 2,
                calls: Arc::clone(&calls_b),
            },
            &scope_b,
        )
        .expect("register isolated worker B");

    assert_eq!(scope_a.get_service::<u64>("worker").as_deref(), Some(&1));
    assert_eq!(scope_b.get_service::<u64>("worker").as_deref(), Some(&2));
    assert!(root.get_service::<u64>("worker").is_none());

    let a_filter = scope_a.clone();
    let dispatch_a =
        root.with_event_filter(move |listener| listener.shares_isolate(&a_filter, "worker"));
    dispatch_a
        .emit("runtime/tick", vec![])
        .expect("dispatch filtered event");
    assert_eq!(calls_a.load(Ordering::SeqCst), 1);
    assert_eq!(calls_b.load(Ordering::SeqCst), 0);

    root.emit("runtime/tick", vec![])
        .expect("dispatch unfiltered event");
    assert_eq!(calls_a.load(Ordering::SeqCst), 2);
    assert_eq!(calls_b.load(Ordering::SeqCst), 1);

    registry
        .unregister("worker", &scope_a)
        .expect("unregister isolated worker A");
    assert!(scope_a.get_service::<u64>("worker").is_none());
    assert_eq!(scope_b.get_service::<u64>("worker").as_deref(), Some(&2));
    assert!(!registry.has_plugin_in("worker", &scope_a));
    assert!(registry.has_plugin_in("worker", &scope_b));
    assert_eq!(registry.events().listener_count("runtime/tick"), 1);
}

#[test]
fn test_context_filter_applies_to_every_dispatch_mode() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let registry = Arc::new(registry::RegistryService::new(Arc::new(
            logger::LoggerService::new("dispatch-filter"),
        )));
        let root = CordisContext::new();
        root.bind_registry(&registry);
        let scope_a = root.isolate_name("worker", None);
        let scope_b = root.isolate_name("worker", None);
        let selected = scope_a.clone();
        let dispatch =
            root.with_event_filter(move |listener| listener.shares_isolate(&selected, "worker"));

        let parallel_a = Arc::new(AtomicUsize::new(0));
        let parallel_b = Arc::new(AtomicUsize::new(0));
        let parallel_global = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&parallel_a);
        scope_a
            .on("filtered/parallel", move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                None
            })
            .expect("register parallel listener A");
        let calls = Arc::clone(&parallel_b);
        scope_b
            .on("filtered/parallel", move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                None
            })
            .expect("register parallel listener B");
        let calls = Arc::clone(&parallel_global);
        registry.events().on("filtered/parallel", move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        });
        dispatch
            .parallel("filtered/parallel", vec![])
            .await
            .expect("parallel dispatch");
        assert_eq!(parallel_a.load(Ordering::SeqCst), 1);
        assert_eq!(parallel_b.load(Ordering::SeqCst), 0);
        assert_eq!(parallel_global.load(Ordering::SeqCst), 1);

        let serial_a = Arc::new(AtomicUsize::new(0));
        let serial_b = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&serial_a);
        scope_a
            .on_async("filtered/serial", move |_| {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Some(Arc::new(11_u64) as events::EventValue)
                }
            })
            .expect("register serial listener A");
        let calls = Arc::clone(&serial_b);
        scope_b
            .on_async("filtered/serial", move |_| {
                let calls = Arc::clone(&calls);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Some(Arc::new(22_u64) as events::EventValue)
                }
            })
            .expect("register serial listener B");
        let serial = dispatch
            .serial("filtered/serial", vec![])
            .await
            .expect("serial dispatch");
        assert_eq!(
            serial.and_then(|value| value.downcast_ref::<u64>().copied()),
            Some(11)
        );
        assert_eq!(serial_a.load(Ordering::SeqCst), 1);
        assert_eq!(serial_b.load(Ordering::SeqCst), 0);

        let bail_a = Arc::new(AtomicUsize::new(0));
        let bail_b = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&bail_a);
        scope_a
            .on("filtered/bail", move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Some(Arc::new(31_u64) as events::EventValue)
            })
            .expect("register bail listener A");
        let calls = Arc::clone(&bail_b);
        scope_b
            .on("filtered/bail", move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Some(Arc::new(32_u64) as events::EventValue)
            })
            .expect("register bail listener B");
        let bail = dispatch
            .bail("filtered/bail", vec![])
            .expect("bail dispatch");
        assert_eq!(
            bail.and_then(|value| value.downcast_ref::<u64>().copied()),
            Some(31)
        );
        assert_eq!(bail_a.load(Ordering::SeqCst), 1);
        assert_eq!(bail_b.load(Ordering::SeqCst), 0);

        let waterfall_a = Arc::new(AtomicUsize::new(0));
        let waterfall_b = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&waterfall_a);
        scope_a
            .on("filtered/waterfall", move |args| {
                calls.fetch_add(1, Ordering::SeqCst);
                let downstream = args
                    .iter()
                    .find_map(|value| value.downcast_ref::<events::NextHandle>())
                    .and_then(events::NextHandle::invoke)
                    .and_then(|value| value.downcast_ref::<u64>().copied())
                    .unwrap_or_default();
                Some(Arc::new(downstream + 10) as events::EventValue)
            })
            .expect("register waterfall listener A");
        let calls = Arc::clone(&waterfall_b);
        scope_b
            .on("filtered/waterfall", move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Some(Arc::new(99_u64) as events::EventValue)
            })
            .expect("register waterfall listener B");
        let waterfall = dispatch
            .waterfall_with("filtered/waterfall", vec![], |_| {
                Some(Arc::new(1_u64) as events::EventValue)
            })
            .expect("waterfall dispatch");
        assert_eq!(
            waterfall.and_then(|value| value.downcast_ref::<u64>().copied()),
            Some(11)
        );
        assert_eq!(waterfall_a.load(Ordering::SeqCst), 1);
        assert_eq!(waterfall_b.load(Ordering::SeqCst), 0);
    });
}

struct ChildPlugin;

impl registry::Plugin for ChildPlugin {
    fn apply(&self, _ctx: &CordisContext) -> Result<(), String> {
        Ok(())
    }

    fn name(&self) -> &str {
        "child"
    }
}

struct ParentPlugin {
    fail: bool,
}

impl registry::Plugin for ParentPlugin {
    fn apply(&self, ctx: &CordisContext) -> Result<(), String> {
        ctx.plugin(ChildPlugin)?;
        if self.fail {
            return Err("parent failed after child registration".to_string());
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "parent"
    }
}

#[test]
fn test_nested_plugin_registration_is_reentrant_and_fiber_owned() {
    let registry = Arc::new(registry::RegistryService::new(Arc::new(
        logger::LoggerService::new("nested-runtime"),
    )));
    let root = CordisContext::new();
    root.bind_registry(&registry);

    assert!(registry
        .register(ParentPlugin { fail: true }, &root)
        .is_err());
    assert!(!registry.has_plugin("parent"));
    assert!(!registry.has_plugin("child"));

    registry
        .register(ParentPlugin { fail: false }, &root)
        .expect("register nested plugin tree");
    assert!(registry.has_plugin("parent"));
    assert!(registry.has_plugin("child"));
    registry
        .unregister("parent", &root)
        .expect("dispose nested plugin tree");
    assert!(!registry.has_plugin("parent"));
    assert!(!registry.has_plugin("child"));
}

struct LifecycleService {
    initialized: Arc<AtomicUsize>,
    healthy: Arc<AtomicBool>,
}

impl service::Service for LifecycleService {
    fn name(&self) -> &str {
        "lifecycle"
    }

    fn init(&self) -> Result<(), String> {
        self.initialized.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn check(&self) -> Result<(), String> {
        self.healthy
            .load(Ordering::SeqCst)
            .then_some(())
            .ok_or_else(|| "unhealthy".to_string())
    }
}

#[test]
fn test_service_lifecycle_is_driven_by_registry_fiber() {
    let registry = Arc::new(registry::RegistryService::new(Arc::new(
        logger::LoggerService::new("service-runtime"),
    )));
    let root = CordisContext::new();
    root.bind_registry(&registry);
    let initialized = Arc::new(AtomicUsize::new(0));
    let healthy = Arc::new(AtomicBool::new(true));
    registry
        .register_service(
            LifecycleService {
                initialized: Arc::clone(&initialized),
                healthy: Arc::clone(&healthy),
            },
            &root,
        )
        .expect("register service");
    assert_eq!(initialized.load(Ordering::SeqCst), 1);
    assert!(root.get_service::<LifecycleService>("lifecycle").is_some());
    healthy.store(false, Ordering::SeqCst);
    assert!(root.get_service::<LifecycleService>("lifecycle").is_none());
    healthy.store(true, Ordering::SeqCst);
    registry
        .unregister("lifecycle", &root)
        .expect("unregister service");
    assert!(root.get_service::<LifecycleService>("lifecycle").is_none());
}

struct DependencyConsumer {
    applied: Arc<AtomicUsize>,
}

impl registry::Plugin for DependencyConsumer {
    fn apply(&self, ctx: &CordisContext) -> Result<(), String> {
        if ctx.get_service::<LifecycleService>("lifecycle").is_none() {
            return Err("provider disappeared during apply".to_string());
        }
        self.applied.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn name(&self) -> &str {
        "consumer"
    }

    fn dependencies(&self) -> Vec<&str> {
        vec!["lifecycle"]
    }
}

#[test]
fn test_plugin_dependencies_follow_service_health() {
    let registry = Arc::new(registry::RegistryService::new(Arc::new(
        logger::LoggerService::new("dependency-runtime"),
    )));
    let root = CordisContext::new();
    root.bind_registry(&registry);
    let applied = Arc::new(AtomicUsize::new(0));

    let missing = registry.register(
        DependencyConsumer {
            applied: Arc::clone(&applied),
        },
        &root,
    );
    assert!(missing.is_err());
    assert_eq!(applied.load(Ordering::SeqCst), 0);

    let initialized = Arc::new(AtomicUsize::new(0));
    let healthy = Arc::new(AtomicBool::new(false));
    registry
        .register_service(
            LifecycleService {
                initialized: Arc::clone(&initialized),
                healthy: Arc::clone(&healthy),
            },
            &root,
        )
        .expect("register provider");
    let unhealthy = registry.register(
        DependencyConsumer {
            applied: Arc::clone(&applied),
        },
        &root,
    );
    assert!(unhealthy.is_err());
    assert_eq!(applied.load(Ordering::SeqCst), 0);

    healthy.store(true, Ordering::SeqCst);
    registry
        .register(
            DependencyConsumer {
                applied: Arc::clone(&applied),
            },
            &root,
        )
        .expect("healthy provider satisfies dependency");
    assert_eq!(initialized.load(Ordering::SeqCst), 1);
    assert_eq!(applied.load(Ordering::SeqCst), 1);
    registry
        .unregister("consumer", &root)
        .expect("unregister consumer");
    registry
        .unregister("lifecycle", &root)
        .expect("unregister provider");
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
        let result = events.serial("event", vec![]).await;
        assert_eq!(
            result.and_then(|value| value.downcast_ref::<u64>().copied()),
            Some(42)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn test_waterfall_middleware_chain() {
    // Mirrors upstream events.spec.ts 'ctx.waterfall()':
    // (value, next) => value + next() composed over an innermost callback
    // returning 2 => 1 + (1 + 2) = 4.
    let events = events::EventsService::new(Arc::new(logger::LoggerService::new("wf")));
    let make_handler = || {
        move |args: events::EventArgs| -> Option<events::EventValue> {
            let value = args
                .first()
                .and_then(|v| v.downcast_ref::<i32>().copied())
                .unwrap_or(0);
            let next_value = args
                .iter()
                .find_map(|v| {
                    v.downcast_ref::<events::NextHandle>()
                        .map(|next| next.invoke())
                })
                .flatten()
                .and_then(|v| v.downcast_ref::<i32>().copied());
            Some(Arc::new(value + next_value.unwrap_or_default()) as events::EventValue)
        }
    };
    events.on("wf", make_handler());
    events.on("wf", make_handler());
    let result = events.waterfall_with("wf", vec![Arc::new(1_i32) as events::EventValue], |_| {
        Some(Arc::new(2_i32) as events::EventValue)
    });
    assert_eq!(
        result.and_then(|v| v.downcast_ref::<i32>().copied()),
        Some(4)
    );
}

#[test]
fn test_waterfall_short_circuits_without_next() {
    // Upstream: cb3 returns value without calling next, so cb4 is never invoked.
    let events = events::EventsService::new(Arc::new(logger::LoggerService::new("wf2")));
    let first_called = Arc::new(AtomicBool::new(false));
    let cb1 = Arc::clone(&first_called);
    events.on("wf2", move |args| {
        cb1.store(true, Ordering::SeqCst);
        // calls next, chain continues
        args.iter()
            .find_map(|v| {
                v.downcast_ref::<events::NextHandle>()
                    .map(|next| next.invoke())
            })
            .flatten()
    });
    let second_called = Arc::new(AtomicBool::new(false));
    let cb2 = Arc::clone(&second_called);
    events.on("wf2", move |args| {
        cb2.store(true, Ordering::SeqCst);
        // short-circuit: return value without calling next
        args.first().map(Arc::clone)
    });
    let third_called = Arc::new(AtomicBool::new(false));
    let cb3 = Arc::clone(&third_called);
    events.on("wf2", move |args| {
        cb3.store(true, Ordering::SeqCst);
        args.first().map(Arc::clone)
    });
    let result = events.waterfall_with("wf2", vec![Arc::new(9_i32) as events::EventValue], |_| {
        Some(Arc::new(0_i32) as events::EventValue)
    });
    assert_eq!(
        result.and_then(|v| v.downcast_ref::<i32>().copied()),
        Some(9)
    );
    assert!(first_called.load(Ordering::SeqCst));
    assert!(second_called.load(Ordering::SeqCst));
    assert!(!third_called.load(Ordering::SeqCst));
}

#[test]
fn test_fiber_bound_listener_auto_removed() {
    let events = events::EventsService::new(Arc::new(logger::LoggerService::new("scope")));
    let fiber = Arc::new(Fiber::new());
    fiber.activate();
    let calls = Arc::new(AtomicUsize::new(0));
    let listener_calls = Arc::clone(&calls);
    events.on_fiber(&fiber, "scoped", move |_| {
        listener_calls.fetch_add(1, Ordering::SeqCst);
        None
    });
    events.emit("scoped", vec![]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(events.listener_count("scoped"), 1);

    fiber.dispose();
    // Listener was removed automatically with its scope.
    assert_eq!(events.listener_count("scoped"), 0);
    events.emit("scoped", vec![]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn test_disposed_scope_listener_not_dispatched() {
    let events = events::EventsService::new(Arc::new(logger::LoggerService::new("scope2")));
    let fiber = Arc::new(Fiber::new());
    fiber.activate();
    let calls = Arc::new(AtomicUsize::new(0));
    let listener_calls = Arc::clone(&calls);
    events.on_fiber(&fiber, "scoped", move |_| {
        listener_calls.fetch_add(1, Ordering::SeqCst);
        None
    });
    fiber.dispose();
    // Even if a stale entry lingered (it cannot), dispatch filters dead scopes.
    events.emit("scoped", vec![]);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
