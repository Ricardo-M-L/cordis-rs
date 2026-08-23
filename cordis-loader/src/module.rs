//! ModuleLoader — module resolution and job tracking.

use std::sync::{Arc, Mutex};

/// Tracks module resolution jobs.
#[derive(Debug)]
pub struct ModuleLoader {
    jobs: Arc<Mutex<Vec<String>>>,
}

impl ModuleLoader {
    /// Create a new empty ModuleLoader.
    pub fn new() -> Self {
        ModuleLoader {
            jobs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Register a module resolution job by URL.
    pub fn add_job(&self, url: &str) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.push(url.to_string());
    }

    /// Return all registered jobs.
    pub fn jobs(&self) -> Vec<String> {
        self.jobs.lock().unwrap().clone()
    }

    /// Resolve a path relative to a base.
    pub fn resolve(&self, path: &str) -> String {
        if path.starts_with('/')
            || path.starts_with("http://")
            || path.starts_with("https://")
            || path.contains("://")
        {
            path.to_string()
        } else {
            format!("./{}", path)
        }
    }

    /// Check whether a job exists.
    pub fn has_job(&self, url: &str) -> bool {
        let jobs = self.jobs.lock().unwrap();
        jobs.iter().any(|j| j == url)
    }
}

impl Default for ModuleLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_loader() {
        let loader = ModuleLoader::new();
        loader.add_job("http://example.com/module.js");
        loader.add_job("http://example.com/other.js");

        assert_eq!(loader.jobs().len(), 2);
        assert!(loader.has_job("http://example.com/module.js"));
        assert!(!loader.has_job("http://example.com/missing.js"));
        assert_eq!(loader.resolve("relative/path.js"), "./relative/path.js");
        assert_eq!(loader.resolve("/absolute/path.js"), "/absolute/path.js");
    }
}
