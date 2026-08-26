//! Typed event bus with synchronous and asynchronous dispatch strategies.
//!
//! Mirrors the upstream Cordis `EventsService`:
//!
//! - Listeners registered through [`EventsService::on_fiber`] are bound to a
//!   [`crate::fiber::Fiber`] scope and removed automatically when that scope
//!   disposes (the counterpart of `ctx.on()` registering via `fiber.effect()`).
//! - Dispatch skips listeners whose owning scope is disposed.
//! - `waterfall` is a koa-style middleware chain where the innermost callback
//!   is passed to [`EventsService::waterfall_with`] as a separate closure.

use crate::context::{ContextScope, CordisContext};
use crate::fiber::Fiber;
use crate::logger::LoggerService;
use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

pub type EventValue = Arc<dyn Any + Send + Sync>;
pub type EventArgs = Vec<EventValue>;
type EventFuture = Pin<Box<dyn Future<Output = Option<EventValue>> + Send + 'static>>;
type SyncHandler = Arc<dyn Fn(EventArgs) -> Option<EventValue> + Send + Sync>;
type AsyncHandler = Arc<dyn Fn(EventArgs) -> EventFuture + Send + Sync>;

/// The innermost callback of a waterfall chain, mirroring upstream's trailing
/// `(...args) => value` argument.
pub type WaterfallInner = Box<dyn Fn(&EventArgs) -> Option<EventValue> + Send + Sync>;

#[derive(Clone)]
enum Handler {
    Sync(SyncHandler),
    Async(AsyncHandler),
}

#[derive(Clone)]
struct HandlerEntry {
    id: usize,
    handler: Handler,
    /// Owning scope. `None` means the listener is global and never scope-filtered.
    /// Stored weakly so fiber -> disposer -> handler-map cannot form a cycle.
    scope: Option<Weak<Fiber>>,
    /// Listener registration context used by dispatch-side `Context.filter` logic.
    context: Option<ContextScope>,
    /// Global listeners are dispatched regardless of any scope filter.
    /// Retained for parity with upstream `EventOptions.global`; scoped
    /// filtering itself is expressed by presence/absence of `scope`.
    #[allow(dead_code)]
    global: bool,
}

impl HandlerEntry {
    fn is_dispatchable(&self) -> bool {
        match &self.scope {
            None => true,
            Some(weak) => weak.upgrade().is_some_and(|fiber| fiber.is_active()),
        }
    }

    fn is_visible_from(&self, dispatch: Option<&CordisContext>) -> bool {
        if self.global {
            return true;
        }
        match (dispatch, self.context.as_ref()) {
            (Some(dispatch), Some(listener)) => dispatch.accepts_listener(listener),
            (Some(_), None) => false,
            (None, _) => true,
        }
    }
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

/// Handler-map handle captured by fiber-bound disposers. Holding this strongly in
/// the disposer is safe: [`HandlerEntry::scope`] is a `Weak`, so no reference
/// cycle is created.
struct SharedHandlers {
    handlers: Arc<RwLock<HandlerMap>>,
}

impl SharedHandlers {
    fn retain_owned(&self, id: usize, event: &str) {
        let mut handlers = write(&self.handlers);
        if let Some(entries) = handlers.get_mut(event) {
            entries.retain(|entry| entry.id != id);
            if entries.is_empty() {
                handlers.remove(event);
            }
        }
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

    /// Register a global listener, mirroring `ctx.on(name, listener, { global: true })`.
    pub fn on(
        &self,
        name: &str,
        handler: impl Fn(EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> EventHandle {
        self.register(name, Handler::Sync(Arc::new(handler)), None, None, true)
    }

    /// Register a listener owned by a fiber scope. When the fiber disposes, the
    /// listener is removed automatically - the Rust counterpart of `ctx.on()`,
    /// which registers its cleanup through `ctx.fiber.effect()`.
    pub fn on_fiber(
        &self,
        fiber: &Arc<Fiber>,
        name: &str,
        handler: impl Fn(EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> EventHandle {
        let handle = self.register(
            name,
            Handler::Sync(Arc::new(handler)),
            Some(Arc::downgrade(fiber)),
            None,
            false,
        );
        self.bind_scope(fiber, &handle);
        handle
    }

    pub fn on_async<F, Fut>(&self, name: &str, handler: F) -> EventHandle
    where
        F: Fn(EventArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<EventValue>> + Send + 'static,
    {
        self.register(
            name,
            Handler::Async(Arc::new(move |args| Box::pin(handler(args)))),
            None,
            None,
            true,
        )
    }

    /// Async listener bound to a fiber scope; removed when the scope disposes.
    pub fn on_async_fiber<F, Fut>(&self, fiber: &Arc<Fiber>, name: &str, handler: F) -> EventHandle
    where
        F: Fn(EventArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<EventValue>> + Send + 'static,
    {
        let handle = self.register(
            name,
            Handler::Async(Arc::new(move |args| Box::pin(handler(args)))),
            Some(Arc::downgrade(fiber)),
            None,
            false,
        );
        self.bind_scope(fiber, &handle);
        handle
    }

    /// Register a listener owned by a concrete Cordis context. Its fiber controls lifetime,
    /// while its context participates in dispatch-side filtering.
    pub fn on_context(
        &self,
        context: &CordisContext,
        name: &str,
        handler: impl Fn(EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> EventHandle {
        let fiber = context.fiber();
        let handle = self.register(
            name,
            Handler::Sync(Arc::new(handler)),
            Some(Arc::downgrade(fiber)),
            Some(context.scope_view()),
            false,
        );
        self.bind_scope(fiber, &handle);
        handle
    }

    pub fn on_async_context<F, Fut>(
        &self,
        context: &CordisContext,
        name: &str,
        handler: F,
    ) -> EventHandle
    where
        F: Fn(EventArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<EventValue>> + Send + 'static,
    {
        let fiber = context.fiber();
        let handle = self.register(
            name,
            Handler::Async(Arc::new(move |args| Box::pin(handler(args)))),
            Some(Arc::downgrade(fiber)),
            Some(context.scope_view()),
            false,
        );
        self.bind_scope(fiber, &handle);
        handle
    }

    /// Tie a registered listener to `fiber`: when the fiber disposes, the listener
    /// is unregistered by a fiber effect.
    fn bind_scope(&self, fiber: &Arc<Fiber>, handle: &EventHandle) {
        let shared = Arc::new(SharedHandlers {
            handlers: Arc::clone(&self.handlers),
        });
        let event = handle.event.clone();
        let id = handle.id;
        let disposer = crate::fiber::disposer(move || {
            shared.retain_owned(id, &event);
        });
        match fiber.effect(move || disposer) {
            Some(effector) => effector.detach(),
            None => {
                // Fiber already disposed: unregister immediately.
                SharedHandlers {
                    handlers: Arc::clone(&self.handlers),
                }
                .retain_owned(handle.id, &handle.event);
            }
        }
    }

    fn register(
        &self,
        name: &str,
        handler: Handler,
        scope: Option<Weak<Fiber>>,
        context: Option<ContextScope>,
        global: bool,
    ) -> EventHandle {
        let id = self.next_handler.fetch_add(1, Ordering::SeqCst);
        write(&self.handlers)
            .entry(name.to_string())
            .or_default()
            .push(HandlerEntry {
                id,
                handler,
                scope,
                context,
                global,
            });
        EventHandle {
            handlers: Arc::downgrade(&self.handlers),
            event: name.to_string(),
            id,
        }
    }

    pub fn off(&self, handle: &EventHandle) -> bool {
        handle.dispose()
    }

    /// Number of live (non-disposed-scope) listeners for `name`.
    pub fn listener_count(&self, name: &str) -> usize {
        self.resolve(name, None).len()
    }

    /// Invoke synchronous handlers inline and schedule asynchronous handlers on the
    /// active Tokio runtime. The handler list is cloned before invocation, so handlers
    /// may safely register or remove listeners reentrantly.
    pub fn emit(&self, name: &str, args: EventArgs) {
        self.emit_entries(self.resolve(name, None), args);
    }

    /// Dispatch with a context filter. Global listeners always run; scoped listeners run only
    /// when `dispatch` accepts their registration context.
    pub fn emit_context(&self, dispatch: &CordisContext, name: &str, args: EventArgs) {
        self.emit_entries(self.resolve(name, Some(dispatch)), args);
    }

    fn emit_entries(&self, entries: Vec<HandlerEntry>, args: EventArgs) {
        for entry in entries {
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
        self.parallel_entries(self.resolve(name, None), args).await;
    }

    pub async fn parallel_context(&self, dispatch: &CordisContext, name: &str, args: EventArgs) {
        self.parallel_entries(self.resolve(name, Some(dispatch)), args)
            .await;
    }

    async fn parallel_entries(&self, entries: Vec<HandlerEntry>, args: EventArgs) {
        let mut tasks = tokio::task::JoinSet::new();
        for entry in entries {
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

    /// Synchronous ordered dispatch over sync handlers only, mirroring upstream `bail`.
    /// Asynchronous handlers are skipped; use [`EventsService::serial`] (async fn)
    /// when an event can contain both kinds.
    pub fn serial_sync(&self, name: &str, args: EventArgs) -> Option<EventValue> {
        self.serial_sync_entries(self.resolve(name, None), args)
    }

    pub fn serial_sync_context(
        &self,
        dispatch: &CordisContext,
        name: &str,
        args: EventArgs,
    ) -> Option<EventValue> {
        self.serial_sync_entries(self.resolve(name, Some(dispatch)), args)
    }

    fn serial_sync_entries(
        &self,
        entries: Vec<HandlerEntry>,
        args: EventArgs,
    ) -> Option<EventValue> {
        for entry in entries {
            if let Handler::Sync(handler) = entry.handler {
                if let Some(result) = handler(args.clone()) {
                    return Some(result);
                }
            }
        }
        None
    }

    /// Ordered dispatch awaiting both sync and async handlers, mirroring the upstream
    /// `serial()` contract: run each listener in order, stop at the first non-empty
    /// (`bailed`) result.
    pub async fn serial(&self, name: &str, args: EventArgs) -> Option<EventValue> {
        self.serial_entries(self.resolve(name, None), args).await
    }

    pub async fn serial_context(
        &self,
        dispatch: &CordisContext,
        name: &str,
        args: EventArgs,
    ) -> Option<EventValue> {
        self.serial_entries(self.resolve(name, Some(dispatch)), args)
            .await
    }

    async fn serial_entries(
        &self,
        entries: Vec<HandlerEntry>,
        args: EventArgs,
    ) -> Option<EventValue> {
        for entry in entries {
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

    /// Synchronous first-non-empty dispatch, mirroring upstream `bail()`.
    pub fn bail(&self, name: &str, args: EventArgs) -> Option<EventValue> {
        self.serial_sync(name, args)
    }

    pub fn bail_context(
        &self,
        dispatch: &CordisContext,
        name: &str,
        args: EventArgs,
    ) -> Option<EventValue> {
        self.serial_sync_context(dispatch, name, args)
    }

    /// Koa-style middleware chain over synchronous handlers with no innermost
    /// callback; equivalent to [`EventsService::waterfall_with`] with an inner
    /// callback that returns `None`.
    pub fn waterfall(&self, name: &str, args: EventArgs) -> Option<EventValue> {
        self.waterfall_with(name, args, |_| None)
    }

    /// Koa-style middleware chain, mirroring upstream `waterfall(name, ...args, inner)`.
    ///
    /// Each handler receives the original arguments plus a [`NextHandle`] appended.
    /// Calling `next.invoke()` runs the remaining handlers; if every handler calls
    /// `next`, `inner` is invoked with the original arguments and its value becomes
    /// the chain's result. A handler that returns `Some` without invoking `next`
    /// short-circuits the chain.
    pub fn waterfall_with(
        &self,
        name: &str,
        args: EventArgs,
        inner: impl Fn(&EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> Option<EventValue> {
        self.waterfall_entries(self.resolve(name, None), args, inner)
    }

    pub fn waterfall_with_context(
        &self,
        dispatch: &CordisContext,
        name: &str,
        args: EventArgs,
        inner: impl Fn(&EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> Option<EventValue> {
        self.waterfall_entries(self.resolve(name, Some(dispatch)), args, inner)
    }

    fn waterfall_entries(
        &self,
        entries: Vec<HandlerEntry>,
        args: EventArgs,
        inner: impl Fn(&EventArgs) -> Option<EventValue> + Send + Sync + 'static,
    ) -> Option<EventValue> {
        let sync_handlers: Vec<SyncHandler> = entries
            .into_iter()
            .filter_map(|entry| match entry.handler {
                Handler::Sync(handler) => Some(handler),
                Handler::Async(_) => None,
            })
            .collect();
        run_waterfall(sync_handlers, args, Arc::new(Box::new(inner)))
    }

    fn resolve(&self, name: &str, dispatch: Option<&CordisContext>) -> Vec<HandlerEntry> {
        let handlers = read(&self.handlers);
        match handlers.get(name) {
            Some(entries) => entries
                .iter()
                .filter(|entry| entry.is_dispatchable() && entry.is_visible_from(dispatch))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }
}

/// The `next` handle passed to waterfall handlers.
pub struct NextHandle {
    chain: Arc<Mutex<WaterfallChain>>,
}

impl NextHandle {
    /// Run the remaining handlers in the chain. Returns the chain's final value:
    /// either a later handler's short-circuit value or the innermost callback's.
    pub fn invoke(&self) -> Option<EventValue> {
        let (handlers, args, next_index) = {
            let chain = lock_chain(&self.chain);
            (chain.handlers.clone(), chain.args.clone(), chain.next_index)
        };
        run_chain_from(&handlers, args, next_index, &self.chain)
    }
}

impl std::fmt::Debug for NextHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("NextHandle").finish()
    }
}

struct WaterfallChain {
    handlers: Vec<SyncHandler>,
    args: EventArgs,
    next_index: usize,
    inner: Arc<WaterfallInner>,
}

fn run_waterfall(
    handlers: Vec<SyncHandler>,
    args: EventArgs,
    inner: Arc<WaterfallInner>,
) -> Option<EventValue> {
    let chain = Arc::new(Mutex::new(WaterfallChain {
        handlers,
        args,
        next_index: 0,
        inner,
    }));
    let handlers = lock_chain(&chain).handlers.clone();
    let args = lock_chain(&chain).args.clone();
    run_chain_from(&handlers, args, 0, &chain)
}

fn run_chain_from(
    handlers: &[SyncHandler],
    args: EventArgs,
    start: usize,
    chain: &Arc<Mutex<WaterfallChain>>,
) -> Option<EventValue> {
    let Some(handler) = handlers.get(start) else {
        // Chain exhausted: run the innermost callback with the original arguments.
        let inner = lock_chain(chain).inner.clone();
        return inner(&args);
    };

    // Record progress so a later next() continues after this handler.
    {
        let mut state = lock_chain(chain);
        state.next_index = start + 1;
    }

    let mut call_args = args;
    let next_handle = Arc::new(NextHandle {
        chain: Arc::clone(chain),
    });
    call_args.push(next_handle as EventValue);
    handler(call_args)
}

fn lock_chain(chain: &Arc<Mutex<WaterfallChain>>) -> std::sync::MutexGuard<'_, WaterfallChain> {
    chain
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
