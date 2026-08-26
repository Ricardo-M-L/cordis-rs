//! Rust module-factory registration and path resolution.

use crate::entry::EntryConfig;
use cordis_core::Plugin;
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

pub type ModuleFactory =
    Arc<dyn Fn(&EntryConfig) -> Result<Box<dyn Plugin>, String> + Send + Sync + 'static>;

pub struct ModuleLoader {
    jobs: Arc<Mutex<BTreeSet<String>>>,
    factories: Arc<RwLock<HashMap<String, ModuleFactory>>>,
    base: Option<PathBuf>,
}

impl ModuleLoader {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(BTreeSet::new())),
            factories: Arc::new(RwLock::new(HashMap::new())),
            base: None,
        }
    }

    pub fn with_base(base: impl Into<PathBuf>) -> Self {
        Self {
            base: Some(base.into()),
            ..Self::new()
        }
    }

    pub fn add_job(&self, url: &str) {
        lock(&self.jobs).insert(url.to_string());
    }

    pub fn remove_job(&self, url: &str) -> bool {
        lock(&self.jobs).remove(url)
    }

    pub fn jobs(&self) -> Vec<String> {
        lock(&self.jobs).iter().cloned().collect()
    }

    pub fn register(
        &self,
        name: &str,
        factory: impl Fn(&EntryConfig) -> Result<Box<dyn Plugin>, String> + Send + Sync + 'static,
    ) -> Result<(), String> {
        let mut factories = write(&self.factories);
        if factories.contains_key(name) {
            return Err(format!("module factory already registered: {name}"));
        }
        factories.insert(name.to_string(), Arc::new(factory));
        Ok(())
    }

    pub fn unregister(&self, name: &str) -> bool {
        write(&self.factories).remove(name).is_some()
    }

    pub fn has_factory(&self, name: &str) -> bool {
        read(&self.factories).contains_key(name)
    }

    pub fn instantiate(&self, config: &EntryConfig) -> Result<Box<dyn Plugin>, String> {
        let factory = read(&self.factories)
            .get(config.name())
            .cloned()
            .ok_or_else(|| format!("module factory not found: {}", config.name()))?;
        self.add_job(config.name());
        factory(config)
    }

    /// Resolve URLs unchanged and normalize local paths against the configured base.
    pub fn resolve(&self, path: &str) -> String {
        if path.contains("://") {
            return path.to_string();
        }
        let path = Path::new(path);
        if path.is_absolute() {
            return normalize_path(path).to_string_lossy().into_owned();
        }
        if let Some(base) = &self.base {
            return normalize_path(&base.join(path))
                .to_string_lossy()
                .into_owned();
        }
        format!("./{}", normalize_path(path).to_string_lossy())
    }

    pub fn has_job(&self, url: &str) -> bool {
        lock(&self.jobs).contains(url)
    }
}

impl Default for ModuleLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ModuleLoader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModuleLoader")
            .field("base", &self.base)
            .field("jobs", &lock(&self.jobs).len())
            .field("factories", &read(&self.factories).len())
            .finish()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths_and_deduplicates_jobs() {
        let loader = ModuleLoader::new();
        loader.add_job("module");
        loader.add_job("module");
        assert_eq!(loader.jobs(), vec!["module"]);
        assert_eq!(loader.resolve("a/../b"), "./b");
        assert_eq!(
            loader.resolve("https://example.com/a"),
            "https://example.com/a"
        );
    }
}
