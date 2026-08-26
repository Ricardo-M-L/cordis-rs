//! Cordis Utils — shared utility types and functions.

use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CordisResult
// ---------------------------------------------------------------------------

pub type CordisResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// A cloneable, thread-safe list.
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
        let mut items = lock(&self.items);
        items.push(value);
    }

    /// Remove and return the item at `index` when it exists.
    pub fn remove(&self, index: usize) -> Option<T> {
        let mut items = lock(&self.items);
        (index < items.len()).then(|| items.remove(index))
    }

    pub fn clear(&self) -> Vec<T> {
        std::mem::take(&mut *lock(&self.items))
    }

    /// Return the number of items.
    pub fn len(&self) -> usize {
        lock(&self.items).len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Filter items matching the predicate.
    pub fn filter(&self, predicate: impl Fn(&T) -> bool) -> Vec<T> {
        let items = lock(&self.items);
        items
            .iter()
            .filter(|item| predicate(item))
            .cloned()
            .collect()
    }

    /// Map items using the given function.
    pub fn map<U: Clone>(&self, mapper: impl Fn(&T) -> U) -> Vec<U> {
        let items = lock(&self.items);
        items.iter().map(mapper).collect()
    }

    /// Return all items as a vector.
    pub fn to_vec(&self) -> Vec<T> {
        lock(&self.items).clone()
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

/// Recursively merge object values while replacing scalar and array values.
pub fn merge_configs(
    base: &mut HashMap<String, serde_json::Value>,
    override_map: &HashMap<String, serde_json::Value>,
) {
    for (key, value) in override_map {
        match (base.get_mut(key), value) {
            (Some(serde_json::Value::Object(base)), serde_json::Value::Object(overrides)) => {
                merge_objects(base, overrides);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

fn merge_objects(
    base: &mut serde_json::Map<String, serde_json::Value>,
    overrides: &serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in overrides {
        match (base.get_mut(key), value) {
            (Some(serde_json::Value::Object(base)), serde_json::Value::Object(overrides)) => {
                merge_objects(base, overrides);
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Format a Unix timestamp in milliseconds as UTC ISO-8601.
pub fn format_date(ts_ms: u64) -> String {
    let secs = ts_ms / 1000;
    let days = (secs / 86_400) as i64;
    let seconds_in_day = secs % 86_400;
    let hour = seconds_in_day / 3600;
    let minute = (seconds_in_day % 3600) / 60;
    let second = seconds_in_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        ts_ms % 1000
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u64, u64) {
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u64, day as u64)
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        assert_eq!(format_date(ts), "1970-01-01T00:00:00.000Z");
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
