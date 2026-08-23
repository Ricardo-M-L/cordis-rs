//! Reflect — type-aware metadata storage with provide/get/set.
//!
//! Mirrors the TypeScript `ReflectService`.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// Property descriptor kinds.
#[derive(Debug, Clone)]
pub enum PropertyDescriptor {
    Service { singleton: bool },
    Accessor,
}

/// ReflectService provides type-safe metadata look-up and storage.
pub struct Reflect {
    store: HashMap<TypeId, HashMap<String, Arc<dyn Any + Send + Sync>>>,
    descriptors: HashMap<String, PropertyDescriptor>,
}

impl Reflect {
    /// Create a new empty Reflect.
    pub fn new() -> Self {
        Reflect {
            store: HashMap::new(),
            descriptors: HashMap::new(),
        }
    }

    /// Provide a value under `key` with its concrete type.
    pub fn provide<T: Any + Send + Sync + 'static>(&mut self, key: &str, value: T) {
        let type_id = TypeId::of::<T>();
        let entry = self.store.entry(type_id).or_insert_with(HashMap::new);
        entry.insert(key.to_string(), Arc::new(value));
    }

    /// Register a property descriptor.
    pub fn descriptor(&mut self, key: &str, desc: PropertyDescriptor) {
        self.descriptors.insert(key.to_string(), desc);
    }

    /// Retrieve a value by `key` and expected type `T`.
    pub fn get<T: Any + Send + Sync + Clone + 'static>(&self, key: &str) -> Option<T> {
        let type_id = TypeId::of::<T>();
        self.store
            .get(&type_id)?
            .get(key)
            .and_then(|v| v.downcast_ref::<T>().cloned())
    }

    /// Delete a value by `key` and type `T`.
    pub fn delete<T: Any + Send + Sync + 'static>(&mut self, key: &str) {
        let type_id = TypeId::of::<T>();
        if let Some(map) = self.store.get_mut(&type_id) {
            map.remove(key);
        }
    }

    /// Check whether a descriptor exists for the given key.
    pub fn has_descriptor(&self, key: &str) -> bool {
        self.descriptors.contains_key(key)
    }
}

impl Default for Reflect {
    fn default() -> Self {
        Self::new()
    }
}
