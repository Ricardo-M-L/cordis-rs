//! Cordis Utils — shared utility types and functions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CordisResult
// ---------------------------------------------------------------------------

pub type CordisResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// A disposable-aware list (mirrors TS List<T>).
pub struct List<T: Clone> {
    items: Arc<std::sync::Mutex<Vec<T>>>,
}

impl<T: Clone> List<T> {
    /// Create a new empty List.
    pub fn new() -> Self {
        List {
            items: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Push an item onto the list.
    pub fn push(&self, value: T) {
        let mut items = self.items.lock().unwrap();
        items.push(value);
    }

    /// Return the number of items.
    pub fn len(&self) -> usize {
        self.items.lock().unwrap().len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Filter items matching the predicate.
    pub fn filter(&self, predicate: impl Fn(&T) -> bool) -> Vec<T> {
        let items = self.items.lock().unwrap();
        items.iter().filter(|item| predicate(item)).cloned().collect()
    }

    /// Map items using the given function.
    pub fn map<U: Clone>(&self, mapper: impl Fn(&T) -> U) -> Vec<U> {
        let items = self.items.lock().unwrap();
        items.iter().map(|item| mapper(item)).collect()
    }

    /// Return all items as a vector.
    pub fn to_vec(&self) -> Vec<T> {
        self.items.lock().unwrap().clone()
    }
}

impl<T: Clone> Default for List<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Clone for List<T> {
    fn clone(&self) -> Self {
        List {
            items: Arc::clone(&self.items),
        }
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Deep clone a serde_json::Value.
pub fn clone_value(value: &serde_json::Value) -> serde_json::Value {
    value.clone()
}

/// Merge two config maps (shallow merge).
pub fn merge_configs(
    base: &mut HashMap<String, serde_json::Value>,
    override_map: &HashMap<String, serde_json::Value>,
) {
    for (key, value) in override_map {
        base.insert(key.clone(), value.clone());
    }
}

/// Format a timestamp as a human-readable date string.
pub fn format_date(ts_ms: u64) -> String {
    let secs = ts_ms / 1000;
    let h = (secs / 3600) as usize;
    let m = ((secs % 3600) / 60) as usize;
    let s = (secs % 60) as usize;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clone_value() {
        let val = serde_json::json!({"a": 1});
        let cloned = clone_value(&val);
        assert_eq!(val, cloned);
    }

    #[test]
    fn test_merge_configs() {
        let mut base = HashMap::new();
        base.insert("a".to_string(), serde_json::json!(1));
        base.insert("b".to_string(), serde_json::json!(2));

        let mut override_map = HashMap::new();
        override_map.insert("b".to_string(), serde_json::json!(99));
        override_map.insert("c".to_string(), serde_json::json!(3));

        merge_configs(&mut base, &override_map);
        assert_eq!(base["a"], serde_json::json!(1));
        assert_eq!(base["b"], serde_json::json!(99));
        assert_eq!(base["c"], serde_json::json!(3));
    }

    #[test]
    fn test_format_date() {
        let ts = 0u64;
        assert_eq!(format_date(ts), "00:00:00");
    }

    #[test]
    fn test_list() {
        let list = List::<i32>::new();
        list.push(1);
        list.push(2);
        assert_eq!(list.len(), 2);
        assert_eq!(list.filter(|x| *x > 1), vec![2]);
        assert_eq!(list.map(|x| x * 2), vec![2, 4]);
    }
}
