//! File-backed hot-reload event service with dependency propagation.

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
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
#[serde(default)]
pub struct HmrConfig {
    pub root: Option<String>,
    pub base: Option<String>,
    pub debounce: u64,
    pub ignored: Vec<String>,
    pub queue_capacity: usize,
}

fn default_queue_capacity() -> usize {
    1024
}

impl Default for HmrConfig {
    fn default() -> Self {
        Self {
            root: None,
            base: None,
            debounce: 100,
            ignored: Vec::new(),
            queue_capacity: default_queue_capacity(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HmrStats {
    pub total_received: u64,
    pub total_emitted: u64,
    pub total_dropped: u64,
    pub total_errors: u64,
    pub callback_panics: u64,
    pub queue_capacity: usize,
    pub queue_depth: usize,
    pub queue_depth_peak: usize,
}

#[derive(Debug)]
struct HmrStatsState {
    total_received: AtomicU64,
    total_emitted: AtomicU64,
    total_dropped: AtomicU64,
    total_errors: AtomicU64,
    callback_panics: AtomicU64,
    queue_depth_peak: AtomicUsize,
}

impl HmrStatsState {
    fn new() -> Self {
        Self {
            total_received: AtomicU64::new(0),
            total_emitted: AtomicU64::new(0),
            total_dropped: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            callback_panics: AtomicU64::new(0),
            queue_depth_peak: AtomicUsize::new(0),
        }
    }

    fn snapshot(&self, config: &HmrConfig, queue_depth: usize) -> HmrStats {
        HmrStats {
            total_received: self.total_received.load(Ordering::Acquire),
            total_emitted: self.total_emitted.load(Ordering::Acquire),
            total_dropped: self.total_dropped.load(Ordering::Acquire),
            total_errors: self.total_errors.load(Ordering::Acquire),
            callback_panics: self.callback_panics.load(Ordering::Acquire),
            queue_capacity: config.queue_capacity,
            queue_depth,
            queue_depth_peak: self
                .queue_depth_peak
                .load(Ordering::Acquire),
        }
    }

    fn record_queue_depth(&self, observed: usize) {
        let mut peak = self.queue_depth_peak.load(Ordering::Acquire);
        while observed > peak {
            match self
                .queue_depth_peak
                .compare_exchange(peak, observed, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(current) => peak = current,
            }
        }
    }
}

type Callback = Arc<dyn Fn(&HmrEvent) + Send + Sync>;
type EventSender = mpsc::SyncSender<HmrEvent>;

pub struct Hmr {
    name: String,
    config: HmrConfig,
    deps: Arc<Mutex<HashMap<String, Vec<String>>>>,
    events: Arc<Mutex<Vec<HmrEvent>>>,
    callbacks: Arc<Mutex<Vec<Callback>>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    sender: Arc<Mutex<Option<EventSender>>>,
    worker: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    recent: Arc<Mutex<HashMap<PathBuf, Instant>>>,
    queue_len: Arc<AtomicUsize>,
    stats: Arc<HmrStatsState>,
}

impl Hmr {
    pub fn new(name: &str, config: HmrConfig) -> Self {
        let mut config = config;
        config.queue_capacity = config.queue_capacity.max(1);
        Self {
            name: name.to_string(),
            config,
            deps: Arc::new(Mutex::new(HashMap::new())),
            events: Arc::new(Mutex::new(Vec::new())),
            callbacks: Arc::new(Mutex::new(Vec::new())),
            watcher: Mutex::new(None),
            sender: Arc::new(Mutex::new(None)),
            worker: Arc::new(Mutex::new(None)),
            recent: Arc::new(Mutex::new(HashMap::new())),
            queue_len: Arc::new(AtomicUsize::new(0)),
            stats: Arc::new(HmrStatsState::new()),
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

        let capacity = self.config.queue_capacity.max(1);
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let queue_len = Arc::clone(&self.queue_len);
        let stats = Arc::clone(&self.stats);
        let events = Arc::clone(&self.events);
        let deps = Arc::clone(&self.deps);
        let callbacks = Arc::clone(&self.callbacks);
        let worker = std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                queue_len.fetch_sub(1, Ordering::AcqRel);
                emit_event(event, &events, &deps, &callbacks, &stats);
            }
        });
        *lock(&self.sender) = Some(sender.clone());
        *lock(&self.worker) = Some(worker);

        let events = Arc::clone(&self.events);
        let deps = Arc::clone(&self.deps);
        let callbacks = Arc::clone(&self.callbacks);
        let sender = Arc::clone(&self.sender);
        let recent = Arc::clone(&self.recent);
        let queue_len = Arc::clone(&self.queue_len);
        let stats = Arc::clone(&self.stats);
        let ignored = self.config.ignored.clone();
        let debounce = Duration::from_millis(self.config.debounce);
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
                            EventKind::Create(_) | EventKind::Modify(_) => {
                                HmrEvent::Changed(path)
                            }
                            EventKind::Remove(_) => HmrEvent::Removed(path),
                            _ => continue,
                        };
                        dispatch_or_emit(
                            hmr_event,
                            &sender,
                            &queue_len,
                            &stats,
                            &events,
                            &deps,
                            &callbacks,
                        );
                    }
                }
                Err(error) => dispatch_or_emit(
                    HmrEvent::Error(error.to_string()),
                    &sender,
                    &queue_len,
                    &stats,
                    &events,
                    &deps,
                    &callbacks,
                ),
            },
        )
        .map_err(|error| error.to_string())?;

        watcher
            .watch(&root_path, RecursiveMode::Recursive)
            .map_err(|error| {
                stop_runtime(Arc::clone(&sender), Arc::clone(&self.worker), Arc::clone(&self.queue_len));
                error.to_string()
            })?;

        *lock(&self.watcher) = Some(watcher);
        Ok(())
    }

    pub fn stop(&self) {
        self.teardown_runtime();
        lock(&self.watcher).take();
    }

    fn teardown_runtime(&self) {
        stop_runtime(
            Arc::clone(&self.sender),
            Arc::clone(&self.worker),
            Arc::clone(&self.queue_len),
        );
    }

    pub fn is_watching(&self) -> bool {
        lock(&self.watcher).is_some()
    }

    /// Deterministic test/manual hook that uses the same dependency propagation as real events.
    pub fn simulate_change(&self, path: &str) {
        dispatch_or_emit(
            HmrEvent::Changed(normalize(path)),
            &self.sender,
            &self.queue_len,
            &self.stats,
            &self.events,
            &self.deps,
            &self.callbacks,
        );
    }

    pub fn simulate_remove(&self, path: &str) {
        dispatch_or_emit(
            HmrEvent::Removed(normalize(path)),
            &self.sender,
            &self.queue_len,
            &self.stats,
            &self.events,
            &self.deps,
            &self.callbacks,
        );
    }

    pub fn stats(&self) -> HmrStats {
        self.stats
            .snapshot(&self.config, self.queue_len.load(Ordering::Acquire))
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
        self.teardown_runtime();
        self.watcher
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

fn stop_runtime(
    sender: Arc<Mutex<Option<EventSender>>>,
    worker: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    queue_len: Arc<AtomicUsize>,
) {
    let _ = sender.lock().ok().and_then(|mut lock| lock.take());
    if let Ok(mut handle) = worker.lock() {
        if let Some(join) = handle.take() {
            let _ = join.join();
        }
    }
    queue_len.store(0, Ordering::Release);
}

fn emit_event(
    event: HmrEvent,
    events: &Arc<Mutex<Vec<HmrEvent>>>,
    deps: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    callbacks: &Arc<Mutex<Vec<Callback>>>,
    stats: &Arc<HmrStatsState>,
) {
    if matches!(event, HmrEvent::Error(_)) {
        stats.total_errors.fetch_add(1, Ordering::AcqRel);
    }
    let mut emitted = vec![event.clone()];
    let changed_path = match &event {
        HmrEvent::Changed(path) | HmrEvent::Removed(path) => Some(path.clone()),
        HmrEvent::Reload(_) | HmrEvent::Error(_) => None,
    };
    if let Some(changed_path) = changed_path {
        emitted.extend(dependents_of(&changed_path, &lock(deps)));
    }
    stats.total_emitted.fetch_add(emitted.len() as u64, Ordering::AcqRel);
    lock(events).extend(emitted.iter().cloned());
    let callbacks = lock(callbacks).clone();
    for event in &emitted {
        for callback in &callbacks {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event))).is_err() {
                stats.callback_panics.fetch_add(1, Ordering::AcqRel);
                stats.total_errors.fetch_add(1, Ordering::AcqRel);
            }
        }
    }
}

fn dispatch_or_emit(
    event: HmrEvent,
    sender: &Arc<Mutex<Option<EventSender>>>,
    queue_len: &Arc<AtomicUsize>,
    stats: &Arc<HmrStatsState>,
    events: &Arc<Mutex<Vec<HmrEvent>>>,
    deps: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    callbacks: &Arc<Mutex<Vec<Callback>>>,
) {
    stats.total_received.fetch_add(1, Ordering::AcqRel);

    if let Some(sender) = {
        let sender = lock(sender);
        sender.clone()
    } {
        match sender.try_send(event.clone()) {
            Ok(()) => {
                let new_depth = queue_len.fetch_add(1, Ordering::AcqRel) + 1;
                stats.record_queue_depth(new_depth);
            }
            Err(mpsc::TrySendError::Full(_)) => {
                stats.total_dropped.fetch_add(1, Ordering::AcqRel);
            }
            Err(mpsc::TrySendError::Disconnected(event)) => {
                emit_event(event, events, deps, callbacks, stats);
            }
        }
        return;
    }

    emit_event(event, events, deps, callbacks, stats);
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_target(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cordis-hmr-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

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
                queue_capacity: 64,
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

    #[test]
    fn queue_backpressure_is_observable() {
        let root = temp_target("queue");
        std::fs::create_dir_all(&root).expect("create watcher root");
        let hmr = Hmr::new(
            "pressure",
            HmrConfig {
                root: Some(root.to_string_lossy().into_owned()),
                base: Some(root.to_string_lossy().into_owned()),
                debounce: 0,
                ignored: Vec::new(),
                queue_capacity: 1,
            },
        );
        hmr.on_event(|_| {
            std::thread::sleep(Duration::from_millis(20));
        });
        hmr.watch().expect("start watcher");
        for _ in 0..120 {
            hmr.simulate_change("src/main.rs");
        }
        std::thread::sleep(Duration::from_millis(250));
        hmr.stop();
        let stats = hmr.stats();
        std::fs::remove_dir_all(&root).expect("cleanup queue target");
        assert_eq!(stats.total_received, 120);
        assert!(stats.total_dropped > 0);
        assert!(stats.queue_depth <= 1);
    }

    #[test]
    fn queue_capacity_is_clamped_to_minimum_one() {
        let hmr = Hmr::new(
            "clamp",
            HmrConfig {
                root: Some(std::env::temp_dir().to_string_lossy().into_owned()),
                base: None,
                debounce: 0,
                ignored: Vec::new(),
                queue_capacity: 0,
            },
        );
        assert_eq!(hmr.config().queue_capacity, 1);
    }

    #[test]
    fn hmr_config_default_has_expected_queue_capacity() {
        let config = HmrConfig::default();
        assert_eq!(config.queue_capacity, 1024);
    }
}
