//! Entry — a configuration entry in the loader tree.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for an Entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryConfig {
    pub name: String,
    pub children: Vec<EntryConfig>,
    pub groups: Vec<String>,
    pub isolates: Vec<String>,
    pub disabled: bool,
    pub config: serde_json::Value,
}

impl Default for EntryConfig {
    fn default() -> Self {
        EntryConfig {
            name: String::new(),
            children: Vec::new(),
            groups: Vec::new(),
            isolates: Vec::new(),
            disabled: false,
            config: serde_json::Value::Object(serde_json::Map::new()),
        }
    }
}

impl EntryConfig {
    pub fn new(name: &str) -> Self {
        EntryConfig {
            name: name.to_string(),
            children: Vec::new(),
            groups: Vec::new(),
            isolates: Vec::new(),
            disabled: false,
            config: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Check whether this entry is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Return the entry's name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// An Entry in the loader tree.
pub struct Entry {
    config: EntryConfig,
}

impl Entry {
    pub fn new(config: EntryConfig) -> Self {
        Entry { config }
    }

    pub fn config(&self) -> &EntryConfig {
        &self.config
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn is_disabled(&self) -> bool {
        self.config.is_disabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_basic() {
        let config = EntryConfig::new("root");
        let entry = Entry::new(config);
        assert_eq!(entry.name(), "root");
        assert!(!entry.is_disabled());
    }

    #[test]
    fn test_entry_disabled() {
        let mut config = EntryConfig::new("disabled");
        config.disabled = true;
        let entry = Entry::new(config);
        assert!(entry.is_disabled());
    }
}
