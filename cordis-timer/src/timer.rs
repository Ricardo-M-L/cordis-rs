//! Disposable timeout, interval, debounce, and throttle primitives.

use futures::Stream;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct TimerService {
    name: Arc<String>,
}

impl TimerService {
    pub fn new(name: &str) -> Self {
        Self {
            name: Arc::new(name.to_string()),
        }
    }

    pub fn timeout<F>(&self, callback: F, ms: u64) -> IntervalHandle
    where
        F: FnOnce() + Send + 'static,
    {
        let inner = Arc::new(TimerInner::default());
        let task_inner = Arc::clone(&inner);
        spawn_after(ms, move || {
            if !task_inner.stopped.swap(true, Ordering::SeqCst) {
                callback();
            }
        });
        IntervalHandle {
            name: Arc::clone(&self.name),
            inner,
        }
    }

    pub async fn timeout_async(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    pub fn interval<F>(&self, callback: F, ms: u64) -> IntervalHandle
    where
        F: Fn() + Send + 'static,
    {
        let delay = ms.max(1);
        let inner = Arc::new(TimerInner::default());
        let task_inner = Arc::clone(&inner);
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    let period = Duration::from_millis(delay);
                    let start = tokio::time::Instant::now() + period;
                    let mut interval = tokio::time::interval_at(start, period);
                    loop {
                        interval.tick().await;
                        if task_inner.stopped.load(Ordering::SeqCst) {
                            return;
                        }
                        callback();
                    }
                });
            }
            Err(_) => {
                std::thread::spawn(move || {
                    let period = Duration::from_millis(delay);
                    while !task_inner.stopped.load(Ordering::SeqCst) {
                        std::thread::sleep(period);
                        if !task_inner.stopped.load(Ordering::SeqCst) {
                            callback();
                        }
                    }
                });
            }
        }
        IntervalHandle {
            name: Arc::clone(&self.name),
            inner,
        }
    }

    pub async fn interval_async(&self, ms: u64) -> IntervalStream {
        IntervalStream::new(ms.max(1))
    }

    /// Create a reusable debounced callback. Calls within the delay window supersede
    /// older calls; only the last scheduled call executes.
    pub fn debounce<F>(&self, callback: F, ms: u64) -> DebounceHandle
    where
        F: Fn() + Send + Sync + 'static,
    {
        DebounceHandle::new(callback, ms)
    }

    /// Create a reusable leading-edge throttled callback.
    pub fn throttle<F>(&self, callback: F, ms: u64) -> ThrottleHandle
    where
        F: Fn() + Send + Sync + 'static,
    {
        ThrottleHandle::new(callback, ms)
    }
}

#[derive(Default)]
struct TimerInner {
    stopped: AtomicBool,
}

#[derive(Clone)]
pub struct IntervalHandle {
    name: Arc<String>,
    inner: Arc<TimerInner>,
}

impl IntervalHandle {
    pub fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl std::fmt::Debug for IntervalHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IntervalHandle")
            .field("name", &self.name)
            .field("stopped", &self.is_stopped())
            .finish()
    }
}

/// A stoppable stream returned by [`TimerService::interval_async`].
pub struct IntervalStream {
    stream: Pin<Box<dyn Stream<Item = ()> + Send>>,
    stopped: Arc<AtomicBool>,
}

impl IntervalStream {
    fn new(ms: u64) -> Self {
        let stopped = Arc::new(AtomicBool::new(false));
        let stream_stopped = Arc::clone(&stopped);
        let stream = futures::stream::unfold(stream_stopped, move |stopped| async move {
            if stopped.load(Ordering::SeqCst) {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(ms)).await;
            if stopped.load(Ordering::SeqCst) {
                None
            } else {
                Some(((), stopped))
            }
        });
        Self {
            stream: Box::pin(stream),
            stopped,
        }
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

impl Stream for IntervalStream {
    type Item = ();

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(context)
    }
}

type SharedCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub struct DebounceHandle {
    callback: SharedCallback,
    delay_ms: u64,
    generation: Arc<AtomicU64>,
    stopped: Arc<AtomicBool>,
}

impl DebounceHandle {
    fn new(callback: impl Fn() + Send + Sync + 'static, delay_ms: u64) -> Self {
        Self {
            callback: Arc::new(callback),
            delay_ms,
            generation: Arc::new(AtomicU64::new(0)),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn call(&self) -> bool {
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let current_generation = Arc::clone(&self.generation);
        let stopped = Arc::clone(&self.stopped);
        let callback = Arc::clone(&self.callback);
        spawn_after(self.delay_ms, move || {
            if !stopped.load(Ordering::SeqCst)
                && current_generation.load(Ordering::SeqCst) == generation
            {
                callback();
            }
        });
        true
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for DebounceHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DebounceHandle")
            .field("delay_ms", &self.delay_ms)
            .field("stopped", &self.is_stopped())
            .finish()
    }
}

#[derive(Clone)]
pub struct ThrottleHandle {
    callback: SharedCallback,
    delay: Duration,
    last_run: Arc<Mutex<Option<Instant>>>,
    stopped: Arc<AtomicBool>,
}

impl ThrottleHandle {
    fn new(callback: impl Fn() + Send + Sync + 'static, delay_ms: u64) -> Self {
        Self {
            callback: Arc::new(callback),
            delay: Duration::from_millis(delay_ms),
            last_run: Arc::new(Mutex::new(None)),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Execute on the leading edge. Returns `true` when the callback ran.
    pub fn call(&self) -> bool {
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        let now = Instant::now();
        let should_run = {
            let mut last_run = self
                .last_run
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if last_run.is_some_and(|last| now.duration_since(last) < self.delay) {
                false
            } else {
                *last_run = Some(now);
                true
            }
        };
        if should_run {
            (self.callback)();
        }
        should_run
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        *self
            .last_run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

impl std::fmt::Debug for ThrottleHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ThrottleHandle")
            .field("delay", &self.delay)
            .field("stopped", &self.is_stopped())
            .finish()
    }
}

fn spawn_after(callback_delay_ms: u64, callback: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn(async move {
                tokio::time::sleep(Duration::from_millis(callback_delay_ms)).await;
                callback();
            });
        }
        Err(_) => {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(callback_delay_ms));
                callback();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn timeout_runs_once() {
        let timer = TimerService::new("test-timeout");
        let counter = Arc::new(AtomicUsize::new(0));
        let callback_counter = Arc::clone(&counter);
        let handle = timer.timeout(
            move || {
                callback_counter.fetch_add(1, Ordering::SeqCst);
            },
            20,
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert!(handle.is_stopped());
    }

    #[tokio::test]
    async fn interval_waits_and_stops() {
        let timer = TimerService::new("test-interval");
        let counter = Arc::new(AtomicUsize::new(0));
        let callback_counter = Arc::clone(&counter);
        let handle = timer.interval(
            move || {
                callback_counter.fetch_add(1, Ordering::SeqCst);
            },
            20,
        );
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        tokio::time::sleep(Duration::from_millis(65)).await;
        handle.stop();
        let count = counter.load(Ordering::SeqCst);
        assert!((2..=4).contains(&count));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(counter.load(Ordering::SeqCst), count);
    }

    #[tokio::test]
    async fn debounce_keeps_only_last_call() {
        let timer = TimerService::new("debounce");
        let counter = Arc::new(AtomicUsize::new(0));
        let callback_counter = Arc::clone(&counter);
        let debounced = timer.debounce(
            move || {
                callback_counter.fetch_add(1, Ordering::SeqCst);
            },
            30,
        );
        debounced.call();
        debounced.call();
        debounced.call();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn throttle_uses_delay_window() {
        let timer = TimerService::new("throttle");
        let counter = Arc::new(AtomicUsize::new(0));
        let callback_counter = Arc::clone(&counter);
        let throttled = timer.throttle(
            move || {
                callback_counter.fetch_add(1, Ordering::SeqCst);
            },
            30,
        );
        assert!(throttled.call());
        assert!(!throttled.call());
        std::thread::sleep(Duration::from_millis(35));
        assert!(throttled.call());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn async_interval_can_be_stopped() {
        let timer = TimerService::new("stream");
        let mut interval = timer.interval_async(10).await;
        assert_eq!(interval.next().await, Some(()));
        interval.stop();
        assert_eq!(interval.next().await, None);
    }
}
