//! HMR — Hot Module Replacement service.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// HmrEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum HmrEvent {
    Changed(String),
    Removed(String),
    Reload(String),
}

impl std::fmt::Display for HmrEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HmrEvent::Changed(path) => write!(f, "HMR change: {}", path),
            HmrEvent::Removed(path) => write!(f, "HMR remove: {}", path),
            HmrEvent::Reload(path) => write!(f, "HMR reload: {}", path),
        }
    }
}

// ---------------------------------------------------------------------------
// HmrConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HmrConfig {
    pub root: Option<String>,
    pub base: Option<String>,
    pub debounce: u64,
    pub ignored: Vec<String>,
}

// ---------------------------------------------------------------------------
// Hmr
// ---------------------------------------------------------------------------

/// HMR service for hot module replacement.
pub struct Hmr {
    name: String,
    config: HmrConfig,
    deps: Mutex<HashMap<String, Vec<String>>>,
    events: Mutex<Vec<HmrEvent>>,
}

impl Hmr {
    /// Create a new Hmr instance.
    pub fn new(name: &str, config: HmrConfig) -> Self {
        Hmr {
            name: name.to_string(),
            config,
            deps: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
        }
    }

    /// Register a dependency relationship.
    pub fn register_dep(&self, file: &str, dep: &str) {
        let mut deps = self.deps.lock().unwrap();
        deps.entry(file.to_string())
            .or_insert_with(Vec::new)
            .push(dep.to_string());
    }

    /// Return all dependencies for a file.
    pub fn deps(&self, file: &str) -> Vec<String> {
        let deps = self.deps.lock().unwrap();
        deps.get(file).cloned().unwrap_or_default()
    }

    /// Simulate a file change event.
    pub fn simulate_change(&self, path: &str) {
        let mut events = self.events.lock().unwrap();
        events.push(HmrEvent::Changed(path.to_string()));

        // Also emit reload for all files that depend on this one
        let deps = self.deps.lock().unwrap();
        for (file, dep_list) in deps.iter() {
            if dep_list.iter().any(|d| d == path) {
                events.push(HmrEvent::Reload(file.clone()));
            }
        }
    }

    /// Return the event history.
    pub fn events(&self) -> Vec<HmrEvent> {
        self.events.lock().unwrap().clone()
    }

    /// Return the HMR name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the config.
    pub fn config(&self) -> &HmrConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hmr_events() {
        let hmr = Hmr::new("test-hmr", HmrConfig::default());
        assert_eq!(hmr.events().len(), 0);

        hmr.simulate_change("/src/app.ts");
        assert_eq!(hmr.events().len(), 1);
        assert!(matches!(hmr.events()[0], HmrEvent::Changed(_)));
    }

    #[test]
    fn test_hmr_register_deps() {
        let hmr = Hmr::new("deps", HmrConfig::default());
        hmr.register_dep("/src/app.ts", "/src/lib.ts");
        hmr.register_dep("/src/app.ts", "/src/util.ts");

        let deps = hmr.deps("/src/app.ts");
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"/src/lib.ts".to_string()));
    }
}
