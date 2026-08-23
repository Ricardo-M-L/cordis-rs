//! Cordis Context — the dependency-injection container.
//!
//! Mirrors the TypeScript `Context` class with extend/isolate/intercept patterns.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// Alias used by other modules that expect `context::Context`.
pub type Context = CordisContext;

/// CordisContext holds configuration values, per-type stores, and isolation markers.
pub struct CordisContext {
    data: Arc<Mutex<HashMap<String, u64>>>,
    typed_store: Arc<RwLock<HashMap<TypeId, HashMap<String, Arc<dyn Any + Send + Sync>>>>>,
    timer_name: Arc<Mutex<Option<String>>>,
    isolate_id: usize,
    parent: Option<Arc<CordisContext>>,
}

impl CordisContext {
    /// Create a new empty context.
    pub fn new() -> Self {
        CordisContext {
            data: Arc::new(Mutex::new(HashMap::new())),
            typed_store: Arc::new(RwLock::new(HashMap::new())),
            timer_name: Arc::new(Mutex::new(None)),
            isolate_id: 0,
            parent: None,
        }
    }

    /// Set a numeric value for the given key.
    pub fn set(&mut self, key: &str, value: u64) {
        let mut map = self.data.lock().unwrap();
        map.insert(key.to_string(), value);
    }

    /// Get the value for the given key.
    pub fn get(&self, key: &str) -> Option<u64> {
        let map = self.data.lock().unwrap();
        map.get(key).copied()
    }

    /// Check whether a key exists.
    pub fn has(&self, key: &str) -> bool {
        let map = self.data.lock().unwrap();
        map.contains_key(key)
    }

    /// Delete the key and its value.
    pub fn delete(&mut self, key: &str) {
        let mut map = self.data.lock().unwrap();
        map.remove(key);
    }

    /// Get the current timer name, if any.
    pub fn get_timer_name(&self) -> Option<String> {
        let timer = self.timer_name.lock().unwrap();
        timer.clone()
    }

    /// Set the current timer name.
    pub fn set_timer(&mut self, name: &str) {
        let mut timer = self.timer_name.lock().unwrap();
        *timer = Some(name.to_string());
    }

    /// Create an extended context that inherits from this one.
    pub fn extend(&self) -> Self {
        CordisContext {
            data: Arc::clone(&self.data),
            typed_store: Arc::clone(&self.typed_store),
            timer_name: Arc::clone(&self.timer_name),
            isolate_id: self.isolate_id + 1,
            parent: Some(Arc::new(self.clone())),
        }
    }

    /// Return the parent context, if this context was created via extend/isolate.
    pub fn parent(&self) -> Option<&Arc<CordisContext>> {
        self.parent.as_ref()
    }

    /// Get the isolate id.
    pub fn isolate_id(&self) -> usize {
        self.isolate_id
    }

    /// Store a typed value for the given type and key.
    pub fn set_typed<T: Any + Send + Sync + 'static>(&self, key: &str, value: T) {
        let type_id = TypeId::of::<T>();
        let mut store = self.typed_store.write().unwrap();
        let entry = store.entry(type_id).or_insert_with(HashMap::new);
        entry.insert(key.to_string(), Arc::new(value));
    }

    /// Get a typed value by type and key.
    pub fn get_typed<T: Any + Send + Sync + Clone + 'static>(&self, key: &str) -> Option<T> {
        let type_id = TypeId::of::<T>();
        let store = self.typed_store.read().unwrap();
        store.get(&type_id)?.get(key).and_then(|v| v.downcast_ref::<T>().cloned())
    }
}

impl Clone for CordisContext {
    fn clone(&self) -> Self {
        CordisContext {
            data: Arc::clone(&self.data),
            typed_store: Arc::clone(&self.typed_store),
            timer_name: Arc::clone(&self.timer_name),
            isolate_id: self.isolate_id,
            parent: self.parent.clone(),
        }
    }
}
