//! File-backed hot-reload event service with dependency propagation.

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HmrEvent {
    Changed(String),
    Removed(String),
    Reload(String),
    Error(String),
}

impl std::fmt::Display for HmrEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changed(path) => write!(formatter, "HMR change: {path}"),
            Self::Removed(path) => write!(formatter, "HMR remove: {path}"),
            Self::Reload(path) => write!(formatter, "HMR reload: {path}"),
            Self::Error(error) => write!(formatter, "HMR error: {error}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HmrConfig {
    pub root: Option<String>,
    pub base: Option<String>,
    pub debounce: u64,
    pub ignored: Vec<String>,
}

impl Default for HmrConfig {
    fn default() -> Self {
        Self {
            root: None,
            base: None,
            debounce: 100,
            ignored: Vec::new(),
        }
    }
}

type Callback = Arc<dyn Fn(&HmrEvent) + Send + Sync>;

pub struct Hmr {
    name: String,
    config: HmrConfig,
    deps: Arc<Mutex<HashMap<String, Vec<String>>>>,
    events: Arc<Mutex<Vec<HmrEvent>>>,
    callbacks: Arc<Mutex<Vec<Callback>>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    recent: Arc<Mutex<HashMap<PathBuf, Instant>>>,
}

impl Hmr {
    pub fn new(name: &str, config: HmrConfig) -> Self {
        Self {
            name: name.to_string(),
            config,
            deps: Arc::new(Mutex::new(HashMap::new())),
            events: Arc::new(Mutex::new(Vec::new())),
            callbacks: Arc::new(Mutex::new(Vec::new())),
            watcher: Mutex::new(None),
            recent: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_dep(&self, file: &str, dep: &str) {
        let mut deps = lock(&self.deps);
        let values = deps.entry(normalize(file)).or_default();
        let dependency = normalize(dep);
        if !values.contains(&dependency) {
            values.push(dependency);
        }
    }

    pub fn remove_dep(&self, file: &str, dep: &str) -> bool {
        let mut deps = lock(&self.deps);
        let Some(values) = deps.get_mut(&normalize(file)) else {
            return false;
        };
        let before = values.len();
        values.retain(|value| value != &normalize(dep));
        before != values.len()
    }

    pub fn deps(&self, file: &str) -> Vec<String> {
        lock(&self.deps)
            .get(&normalize(file))
            .cloned()
            .unwrap_or_default()
    }

    pub fn on_event(&self, callback: impl Fn(&HmrEvent) + Send + Sync + 'static) {
        lock(&self.callbacks).push(Arc::new(callback));
    }

    /// Begin recursively watching `config.root` using the operating system's file watcher.
    pub fn watch(&self) -> Result<(), String> {
        let root = self
            .config
            .root
            .as_deref()
            .ok_or_else(|| "HMR root is required before watch()".to_string())?;
        let root_path = PathBuf::from(root);
        if !root_path.exists() {
            return Err(format!("HMR root does not exist: {}", root_path.display()));
        }
        if lock(&self.watcher).is_some() {
            return Ok(());
        }

        let events = Arc::clone(&self.events);
        let deps = Arc::clone(&self.deps);
        let callbacks = Arc::clone(&self.callbacks);
        let recent = Arc::clone(&self.recent);
        let debounce = Duration::from_millis(self.config.debounce);
        let ignored = self.config.ignored.clone();
        let base = self.config.base.as_ref().map(PathBuf::from);

        let mut watcher = notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) => {
                    for path in event.paths {
                        if is_ignored(&path, &ignored) || is_debounced(&path, debounce, &recent) {
                            continue;
                        }
                        let path = display_path(&path, base.as_deref());
                        let hmr_event = match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) => HmrEvent::Changed(path),
                            EventKind::Remove(_) => HmrEvent::Removed(path),
                            _ => continue,
                        };
                        record_event(hmr_event, &events, &deps, &callbacks);
                    }
                }
                Err(error) => record_event(
                    HmrEvent::Error(error.to_string()),
                    &events,
                    &deps,
                    &callbacks,
                ),
            },
        )
        .map_err(|error| error.to_string())?;

        watcher
            .watch(&root_path, RecursiveMode::Recursive)
            .map_err(|error| error.to_string())?;
        *lock(&self.watcher) = Some(watcher);
        Ok(())
    }

    pub fn stop(&self) {
        lock(&self.watcher).take();
    }

    pub fn is_watching(&self) -> bool {
        lock(&self.watcher).is_some()
    }

    /// Deterministic test/manual hook that uses the same dependency propagation as real events.
    pub fn simulate_change(&self, path: &str) {
        record_event(
            HmrEvent::Changed(normalize(path)),
            &self.events,
            &self.deps,
            &self.callbacks,
        );
    }

    pub fn simulate_remove(&self, path: &str) {
        record_event(
            HmrEvent::Removed(normalize(path)),
            &self.events,
            &self.deps,
            &self.callbacks,
        );
    }

    pub fn events(&self) -> Vec<HmrEvent> {
        lock(&self.events).clone()
    }

    pub fn take_events(&self) -> Vec<HmrEvent> {
        std::mem::take(&mut *lock(&self.events))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn config(&self) -> &HmrConfig {
        &self.config
    }
}

impl Drop for Hmr {
    fn drop(&mut self) {
        self.watcher
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

fn record_event(
    event: HmrEvent,
    events: &Arc<Mutex<Vec<HmrEvent>>>,
    deps: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    callbacks: &Arc<Mutex<Vec<Callback>>>,
) {
    let mut emitted = vec![event.clone()];
    let changed_path = match &event {
        HmrEvent::Changed(path) | HmrEvent::Removed(path) => Some(path.clone()),
        HmrEvent::Reload(_) | HmrEvent::Error(_) => None,
    };
    if let Some(changed_path) = changed_path {
        emitted.extend(dependents_of(&changed_path, &lock(deps)));
    }
    lock(events).extend(emitted.iter().cloned());
    let callbacks = lock(callbacks).clone();
    for event in &emitted {
        for callback in &callbacks {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event)));
        }
    }
}

fn dependents_of(changed: &str, deps: &HashMap<String, Vec<String>>) -> Vec<HmrEvent> {
    let mut queue = VecDeque::from([changed.to_string()]);
    let mut visited = HashSet::from([changed.to_string()]);
    let mut reloads = Vec::new();
    while let Some(dependency) = queue.pop_front() {
        for (file, dependencies) in deps {
            if dependencies.contains(&dependency) && visited.insert(file.clone()) {
                reloads.push(HmrEvent::Reload(file.clone()));
                queue.push_back(file.clone());
            }
        }
    }
    reloads
}

fn is_debounced(
    path: &Path,
    debounce: Duration,
    recent: &Arc<Mutex<HashMap<PathBuf, Instant>>>,
) -> bool {
    if debounce.is_zero() {
        return false;
    }
    let now = Instant::now();
    let mut recent = lock(recent);
    recent.retain(|_, previous| now.duration_since(*previous) < debounce.saturating_mul(2));
    let duplicate = recent
        .get(path)
        .is_some_and(|previous| now.duration_since(*previous) < debounce);
    recent.insert(path.to_path_buf(), now);
    duplicate
}

fn is_ignored(path: &Path, ignored: &[String]) -> bool {
    let display = path.to_string_lossy();
    ignored.iter().any(|pattern| display.contains(pattern))
}

fn display_path(path: &Path, base: Option<&Path>) -> String {
    base.and_then(|base| path.strip_prefix(base).ok())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propagates_transitive_dependencies() {
        let hmr = Hmr::new("deps", HmrConfig::default());
        hmr.register_dep("app.rs", "lib.rs");
        hmr.register_dep("root.rs", "app.rs");
        hmr.simulate_change("lib.rs");
        assert_eq!(
            hmr.events(),
            vec![
                HmrEvent::Changed("lib.rs".to_string()),
                HmrEvent::Reload("app.rs".to_string()),
                HmrEvent::Reload("root.rs".to_string())
            ]
        );
    }

    #[test]
    fn watches_real_files() {
        let root = std::env::temp_dir().join(format!(
            "cordis-hmr-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create watcher root");
        let hmr = Hmr::new(
            "watch",
            HmrConfig {
                root: Some(root.to_string_lossy().into_owned()),
                base: Some(root.to_string_lossy().into_owned()),
                debounce: 0,
                ignored: Vec::new(),
            },
        );
        hmr.watch().expect("start watcher");
        std::fs::write(root.join("sample.txt"), "hello").expect("write watched file");
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && hmr.events().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        hmr.stop();
        std::fs::remove_dir_all(&root).expect("remove watcher root");
        assert!(hmr
            .events()
            .iter()
            .any(|event| matches!(event, HmrEvent::Changed(path) if path.ends_with("sample.txt"))));
    }
}
