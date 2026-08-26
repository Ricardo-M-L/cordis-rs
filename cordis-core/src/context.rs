//! Hierarchical dependency-injection context.

use crate::utils::lock;
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

static CONTEXT_IDS: AtomicUsize = AtomicUsize::new(1);

pub type Context = CordisContext;

type TypedValues = HashMap<TypeId, HashMap<String, Arc<dyn Any + Send + Sync>>>;
type TypedKey = (TypeId, String);

/// A hierarchical context. Cloning shares one scope; [`CordisContext::extend`]
/// creates a child overlay whose reads fall back to its parent and whose writes
/// remain local.
pub struct CordisContext {
    data: Arc<RwLock<HashMap<String, u64>>>,
    deleted_data: Arc<RwLock<HashSet<String>>>,
    typed_store: Arc<RwLock<TypedValues>>,
    deleted_typed: Arc<RwLock<HashSet<TypedKey>>>,
    timer_name: Arc<Mutex<Option<String>>>,
    context_id: usize,
    isolated: bool,
    parent: Option<Arc<CordisContext>>,
}

impl CordisContext {
    pub fn new() -> Self {
        Self::scope(None, false)
    }

    fn scope(parent: Option<Arc<CordisContext>>, isolated: bool) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            deleted_data: Arc::new(RwLock::new(HashSet::new())),
            typed_store: Arc::new(RwLock::new(HashMap::new())),
            deleted_typed: Arc::new(RwLock::new(HashSet::new())),
            timer_name: Arc::new(Mutex::new(None)),
            context_id: CONTEXT_IDS.fetch_add(1, Ordering::SeqCst),
            isolated,
            parent,
        }
    }

    pub fn set(&self, key: &str, value: u64) {
        self.deleted_data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
        self.data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_string(), value);
    }

    pub fn get(&self, key: &str) -> Option<u64> {
        if let Some(value) = self
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .copied()
        {
            return Some(value);
        }
        if self
            .deleted_data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(key)
        {
            return None;
        }
        self.parent.as_ref()?.get(key)
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Hide a local or inherited value in this scope.
    pub fn delete(&self, key: &str) -> bool {
        let removed = self
            .data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key)
            .is_some();
        let inherited = self.parent.as_ref().is_some_and(|parent| parent.has(key));
        self.deleted_data
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_string());
        removed || inherited
    }

    pub fn get_timer_name(&self) -> Option<String> {
        lock(&self.timer_name).clone().or_else(|| {
            self.parent
                .as_ref()
                .and_then(|parent| parent.get_timer_name())
        })
    }

    pub fn set_timer(&self, name: &str) {
        *lock(&self.timer_name) = Some(name.to_string());
    }

    /// Create a child scope with inherited reads and local writes.
    pub fn extend(&self) -> Self {
        Self::scope(Some(Arc::new(self.clone())), false)
    }

    /// Create an explicitly isolated child scope. The scope still inherits values,
    /// but its identity can be used by registries/loaders to prevent cross-scope reuse.
    pub fn isolate(&self) -> Self {
        Self::scope(Some(Arc::new(self.clone())), true)
    }

    pub fn parent(&self) -> Option<&Arc<CordisContext>> {
        self.parent.as_ref()
    }

    pub fn context_id(&self) -> usize {
        self.context_id
    }

    /// Backward-compatible alias for the unique context identity.
    pub fn isolate_id(&self) -> usize {
        self.context_id
    }

    pub fn is_isolated(&self) -> bool {
        self.isolated
    }

    pub fn set_typed<T: Any + Send + Sync + 'static>(&self, key: &str, value: T) {
        let type_id = TypeId::of::<T>();
        self.deleted_typed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(type_id, key.to_string()));
        self.typed_store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(type_id)
            .or_default()
            .insert(key.to_string(), Arc::new(value));
    }

    pub fn get_typed<T: Any + Send + Sync + Clone + 'static>(&self, key: &str) -> Option<T> {
        self.get_typed_arc::<T>(key).map(|value| (*value).clone())
    }

    /// Resolve a typed dependency without requiring the dependency itself to be cloneable.
    pub fn get_typed_arc<T: Any + Send + Sync + 'static>(&self, key: &str) -> Option<Arc<T>> {
        let type_id = TypeId::of::<T>();
        if let Some(value) = self
            .typed_store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&type_id)
            .and_then(|values| values.get(key))
            .cloned()
        {
            return value.downcast::<T>().ok();
        }

        if self
            .deleted_typed
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(type_id, key.to_string()))
        {
            return None;
        }
        self.parent.as_ref()?.get_typed_arc::<T>(key)
    }

    pub fn delete_typed<T: Any + Send + Sync + 'static>(&self, key: &str) -> bool {
        let type_id = TypeId::of::<T>();
        let removed = self
            .typed_store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&type_id)
            .is_some_and(|values| values.remove(key).is_some());
        let inherited = self
            .parent
            .as_ref()
            .is_some_and(|parent| parent.get_typed_arc::<T>(key).is_some());
        self.deleted_typed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((type_id, key.to_string()));
        removed || inherited
    }
}

impl Clone for CordisContext {
    fn clone(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
            deleted_data: Arc::clone(&self.deleted_data),
            typed_store: Arc::clone(&self.typed_store),
            deleted_typed: Arc::clone(&self.deleted_typed),
            timer_name: Arc::clone(&self.timer_name),
            context_id: self.context_id,
            isolated: self.isolated,
            parent: self.parent.clone(),
        }
    }
}

impl Default for CordisContext {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CordisContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CordisContext")
            .field("context_id", &self.context_id)
            .field("isolated", &self.isolated)
            .field("has_parent", &self.parent.is_some())
            .finish()
    }
}
