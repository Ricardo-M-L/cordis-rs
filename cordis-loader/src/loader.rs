//! Loader — loads and manages entry trees.

use std::sync::{Arc, Mutex};

use crate::entry::{Entry, EntryConfig};
use serde::{Deserialize, Serialize};

/// Top-level loader config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoaderConfig {
    pub base_url: Option<String>,
}

/// The Cordis loader.
pub struct Loader {
    config: LoaderConfig,
    entries: Arc<Mutex<Vec<Arc<Entry>>>>,
}

impl Loader {
    /// Create a new loader with the given config.
    pub fn new(config: LoaderConfig) -> Self {
        Loader {
            config,
            entries: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Load an entry tree into this loader. Returns the loaded entries.
    pub fn load(&mut self, entry_config: EntryConfig) -> Vec<Arc<Entry>> {
        let entries = self.collect_entries(entry_config);
        let mut list = self.entries.lock().unwrap();
        list.extend(entries.clone());
        entries
    }

    /// Return a reference to the loader's config.
    pub fn config(&self) -> &LoaderConfig {
        &self.config
    }

    /// Return all loaded entries.
    pub fn entries(&self) -> Vec<Arc<Entry>> {
        self.entries.lock().unwrap().clone()
    }

    fn collect_entries(&self, cfg: EntryConfig) -> Vec<Arc<Entry>> {
        let children = cfg.children.clone();
        let entry = Arc::new(Entry::new(cfg));
        let mut results = vec![entry];
        for child_cfg in children {
            results.extend(self.collect_entries(child_cfg));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loader_load() {
        let config = LoaderConfig::default();
        let mut loader = Loader::new(config);

        let root = EntryConfig {
            name: "root".to_string(),
            children: vec![EntryConfig {
                name: "child".to_string(),
                children: vec![],
                groups: vec!["web".to_string()],
                isolates: vec!["i1".to_string()],
                disabled: false,
                config: serde_json::Value::Null,
            }],
            groups: vec![],
            isolates: vec![],
            disabled: false,
            config: serde_json::Value::Null,
        };

        let loaded = loader.load(root);
        assert!(loaded.len() >= 2);
        assert!(!loader.entries().is_empty());
    }
}
