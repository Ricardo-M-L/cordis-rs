//! Typed event bus with synchronous and asynchronous dispatch strategies.

use crate::logger::LoggerService;
use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, Weak};

pub type EventValue = Arc<dyn Any + Send + Sync>;
pub type EventArgs = Vec<EventValue>;
type EventFuture = Pin<Box<dyn Future<Output = Option<EventValue>> + Send + 'static>>;
type SyncHandler = Arc<dyn Fn(EventArgs) -> Option<EventValue> + Send + Sync>;
type AsyncHandler = Arc<dyn Fn(EventArgs) -> EventFuture + Send + Sync>;

#[derive(Clone)]
enum Handler {
    Sync(SyncHandler),
    Async(AsyncHandler),
}

#[derive(Clone)]
struct HandlerEntry {
    id: usize,
    handler: Handler,
}

type HandlerMap = HashMap<String, Vec<HandlerEntry>>;

/// Explicit listener removal handle. Dropping it does not unregister the listener,
/// preserving the traditional event-emitter behavior when callers ignore `on()`'s return value.
#[derive(Clone)]
pub struct EventHandle {
    handlers: Weak<RwLock<HandlerMap>>,
    event: String,
    id: usize,
}

impl EventHandle {
    pub fn dispose(&self) -> bool {
        let Some(handlers) = self.handlers.upgrade() else {
            return false;
        };
        let mut handlers = write(&handlers);
        let Some(entries) = handlers.get_mut(&self.event) else {
            return false;
        };
        let old_len = entries.len();
        entries.retain(|entry| entry.id != self.id);
        let removed = entries.len() != old_len;
        if entries.is_empty() {
            handlers.remove(&self.event);
        }
        removed
    }

    pub fn id(&self) -> usize {
        self.id
    }
}

impl std::fmt::Debug for EventHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EventHandle")
            .field("event", &self.event)
            .field("id", &self.id)
            .finish()
    }
}

pub struct EventsService {
    handlers: Arc<RwLock<HandlerMap>>,
    next_handler: AtomicUsize,
    logger: Arc<LoggerService>,
}

impl EventsService {
    pub fn new(logger: Arc<LoggerService>) -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            next_handler: AtomicUsize::new(1),
            logger,
        }
    }

    pub fn on(
        &self,
        name: &str,
        handler: impl Fn(EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> EventHandle {
        self.register(name, Handler::Sync(Arc::new(handler)))
    }

    pub fn on_async<F, Fut>(&self, name: &str, handler: F) -> EventHandle
    where
        F: Fn(EventArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<EventValue>> + Send + 'static,
    {
        self.register(
            name,
            Handler::Async(Arc::new(move |args| Box::pin(handler(args)))),
        )
    }

    fn register(&self, name: &str, handler: Handler) -> EventHandle {
        let id = self.next_handler.fetch_add(1, Ordering::SeqCst);
        write(&self.handlers)
            .entry(name.to_string())
            .or_default()
            .push(HandlerEntry { id, handler });
        EventHandle {
            handlers: Arc::downgrade(&self.handlers),
            event: name.to_string(),
            id,
        }
    }

    pub fn off(&self, handle: &EventHandle) -> bool {
        handle.dispose()
    }

    pub fn listener_count(&self, name: &str) -> usize {
        read(&self.handlers).get(name).map_or(0, Vec::len)
    }

    /// Invoke synchronous handlers inline and schedule asynchronous handlers on the
    /// active Tokio runtime. The handler list is cloned before invocation, so handlers
    /// may safely register or remove listeners reentrantly.
    pub fn emit(&self, name: &str, args: EventArgs) {
        for entry in self.resolve(name) {
            match entry.handler {
                Handler::Sync(handler) => {
                    let _ = handler(args.clone());
                }
                Handler::Async(handler) => match tokio::runtime::Handle::try_current() {
                    Ok(runtime) => {
                        let args = args.clone();
                        runtime.spawn(async move {
                            let _ = handler(args).await;
                        });
                    }
                    Err(error) => self.logger.error(
                        "cannot emit async handler without a Tokio runtime: %s",
                        vec![Box::new(error.to_string())],
                    ),
                },
            }
        }
    }

    /// Run all handlers concurrently and wait for every handler to settle.
    pub async fn parallel(&self, name: &str, args: EventArgs) {
        let mut tasks = tokio::task::JoinSet::new();
        for entry in self.resolve(name) {
            match entry.handler {
                Handler::Sync(handler) => {
                    let args = args.clone();
                    tasks.spawn_blocking(move || handler(args));
                }
                Handler::Async(handler) => {
                    let args = args.clone();
                    tasks.spawn(async move { handler(args).await });
                }
            }
        }

        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                self.logger.error(
                    "parallel event handler failed: %s",
                    vec![Box::new(error.to_string())],
                );
            }
        }
    }

    /// Synchronous ordered dispatch. Asynchronous handlers are skipped; use
    /// [`EventsService::serial_async`] when an event can contain both kinds.
    pub fn serial(&self, name: &str, args: EventArgs) -> Option<EventValue> {
        for entry in self.resolve(name) {
            if let Handler::Sync(handler) = entry.handler {
                if let Some(result) = handler(args.clone()) {
                    return Some(result);
                }
            }
        }
        None
    }

    pub async fn serial_async(&self, name: &str, args: EventArgs) -> Option<EventValue> {
        for entry in self.resolve(name) {
            let result = match entry.handler {
                Handler::Sync(handler) => {
                    let args = args.clone();
                    match tokio::task::spawn_blocking(move || handler(args)).await {
                        Ok(result) => result,
                        Err(error) => {
                            self.logger.error(
                                "serial event handler failed: %s",
                                vec![Box::new(error.to_string())],
                            );
                            None
                        }
                    }
                }
                Handler::Async(handler) => handler(args.clone()).await,
            };
            if result.is_some() {
                return result;
            }
        }
        None
    }

    pub fn bail(&self, name: &str, args: EventArgs) -> Option<EventValue> {
        self.serial(name, args)
    }

    /// Pass each non-empty result as the first argument to the next synchronous handler.
    /// The first argument is replaced instead of repeatedly prepended.
    pub fn waterfall(&self, name: &str, args: EventArgs) -> Option<EventValue> {
        let mut current_args = args;
        let mut last_result = None;
        for entry in self.resolve(name) {
            let Handler::Sync(handler) = entry.handler else {
                continue;
            };
            if let Some(result) = handler(current_args.clone()) {
                if current_args.is_empty() {
                    current_args.push(Arc::clone(&result));
                } else {
                    current_args[0] = Arc::clone(&result);
                }
                last_result = Some(result);
            }
        }
        last_result
    }

    fn resolve(&self, name: &str) -> Vec<HandlerEntry> {
        read(&self.handlers).get(name).cloned().unwrap_or_default()
    }
}

impl std::fmt::Debug for EventsService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let handlers = read(&self.handlers);
        formatter
            .debug_struct("EventsService")
            .field("event_count", &handlers.len())
            .field(
                "listener_count",
                &handlers.values().map(Vec::len).sum::<usize>(),
            )
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
