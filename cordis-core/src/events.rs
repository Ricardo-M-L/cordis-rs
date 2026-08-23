//! Events service — pub/sub with serial, parallel, waterfall, and bail execution strategies.

use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::logger::LoggerService;

/// Type alias for boxed event handlers (Arc so they're Clone-able across the async boundary).
type Handler =
    Arc<dyn Fn(Vec<Arc<dyn Any + Send + Sync>>) -> Option<Arc<dyn Any + Send + Sync>> + Send + Sync>;

/// EventsService is a pub/sub event bus that supports four invocation strategies:
///   - **emit**: fire-and-forget, handlers run inline
///   - **serial**: run handlers in order, return the first non-None result
///   - **parallel**: run all handlers concurrently, wait for all to finish
///   - **waterfall**: run handlers in order, passing each result as the first arg to the next
///   - **bail**: like serial but stops at the first handler that returns Some
pub struct EventsService {
    handlers: Arc<RwLock<HashMap<String, Vec<Handler>>>>,
    logger: Arc<LoggerService>,
}

impl EventsService {
    /// Create a new EventsService backed by the given logger.
    pub fn new(logger: Arc<LoggerService>) -> Self {
        EventsService {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            logger,
        }
    }

    /// Register a handler for the given event name.
    pub fn on(
        &self,
        name: &str,
        handler: impl Fn(Vec<Arc<dyn Any + Send + Sync>>) -> Option<Arc<dyn Any + Send + Sync>> + Send + Sync + 'static,
    ) {
        let mut map = self.handlers.write().unwrap();
        let event_name = name.to_string();
        let _ = &self.logger;
        map.entry(event_name)
            .or_insert_with(Vec::new)
            .push(Arc::new(handler));
    }

    /// Fire-and-forget: run all handlers for `name` synchronously and inline.
    pub fn emit(&self, name: &str, args: Vec<Arc<dyn Any + Send + Sync>>) {
        let map = self.handlers.read().unwrap();
        if let Some(handlers) = map.get(name) {
            for handler in handlers {
                let _ = handler(args.clone());
            }
        }
    }

    /// Run all handlers for `name` in parallel, await all of them.
    pub fn parallel(
        &self,
        name: &str,
        args: Vec<Arc<dyn Any + Send + Sync>>,
    ) -> impl Future<Output = ()> {
        let handlers: Vec<Handler> = {
            let map = self.handlers.read().unwrap();
            map.get(name).cloned().unwrap_or_default()
        };

        async move {
            for handler in handlers {
                let _ = handler(args.clone());
            }
        }
    }

    /// Run handlers serially and return the first non-None result.
    pub fn serial(
        &self,
        name: &str,
        args: Vec<Arc<dyn Any + Send + Sync>>,
    ) -> Option<Arc<dyn Any + Send + Sync>> {
        let handlers: Vec<Handler> = {
            let map = self.handlers.read().unwrap();
            map.get(name).cloned().unwrap_or_default()
        };

        for handler in handlers {
            if let Some(result) = handler(args.clone()) {
                return Some(result);
            }
        }
        None
    }

    /// Run handlers serially; stop at the first handler that returns Some.
    pub fn bail(
        &self,
        name: &str,
        args: Vec<Arc<dyn Any + Send + Sync>>,
    ) -> Option<Arc<dyn Any + Send + Sync>> {
        let handlers: Vec<Handler> = {
            let map = self.handlers.read().unwrap();
            map.get(name).cloned().unwrap_or_default()
        };

        for handler in handlers {
            let result = handler(args.clone());
            if result.is_some() {
                return result;
            }
        }
        None
    }

    /// Run handlers in a waterfall: each handler's return value becomes the first
    /// argument to the next handler. Returns the last non-None value.
    pub fn waterfall(
        &self,
        name: &str,
        args: Vec<Arc<dyn Any + Send + Sync>>,
    ) -> Option<Arc<dyn Any + Send + Sync>> {
        let handlers: Vec<Handler> = {
            let map = self.handlers.read().unwrap();
            map.get(name).cloned().unwrap_or_default()
        };

        let mut current_args = args;
        let mut last_result: Option<Arc<dyn Any + Send + Sync>> = None;

        for handler in handlers {
            last_result = handler(current_args.clone());
            if let Some(ref val) = last_result {
                let mut next_args = Vec::with_capacity(current_args.len() + 1);
                next_args.push(Arc::clone(val));
                for a in current_args {
                    next_args.push(a);
                }
                current_args = next_args;
            }
        }
        last_result
    }
}

impl std::fmt::Debug for EventsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let map = self.handlers.read().unwrap();
        f.debug_struct("EventsService")
            .field("event_count", &map.len())
            .finish()
    }
}
