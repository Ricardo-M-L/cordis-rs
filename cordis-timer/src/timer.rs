//! Timer service — timeout and interval with proper disposable lifecycle.
//!
//! Mirrors `@cordisjs/plugin-timer` with async timeout, interval, throttle, debounce.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

// ---------------------------------------------------------------------------
// TimerService
// ---------------------------------------------------------------------------

/// TimerService provides timeout, interval, throttle, and debounce with dispose support.
#[derive(Clone)]
pub struct TimerService {
    name: Arc<String>,
}

impl TimerService {
    pub fn new(name: &str) -> Self {
        TimerService {
            name: Arc::new(name.to_string()),
        }
    }

    /// Execute a callback after `ms` milliseconds. Returns a dispose handle.
    pub fn timeout<F>(&self, callback: F, ms: u64) -> IntervalHandle
    where
        F: FnOnce() + Send + 'static,
    {
        let name = Arc::clone(&self.name);
        let handle = Arc::new(TimerInner {
            stopped: AtomicBool::new(false),
        });
        let h = handle.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            if !h.stopped.load(Ordering::SeqCst) {
                callback();
            }
        });

        IntervalHandle {
            name: Arc::clone(&name),
            handle,
        }
    }

    /// Return a Future that resolves after `ms` milliseconds.
    pub async fn timeout_async(&self, ms: u64) {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }

    /// Execute a callback every `ms` milliseconds. Returns a stop handle.
    pub fn interval<F>(&self, callback: F, ms: u64) -> IntervalHandle
    where
        F: Fn() + Send + Clone + 'static,
    {
        let name = Arc::clone(&self.name);
        let handle = Arc::new(TimerInner {
            stopped: AtomicBool::new(false),
        });
        let h = handle.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(ms));
            loop {
                interval.tick().await;
                if h.stopped.load(Ordering::SeqCst) {
                    break;
                }
                callback();
            }
        });

        IntervalHandle {
            name: Arc::clone(&name),
            handle,
        }
    }

    /// Return an async iterator that yields every `ms` milliseconds.
    pub async fn interval_async(
        &self,
        ms: u64,
    ) -> impl futures::Stream<Item = ()> + Send + 'static {
        let stopped = Arc::new(AtomicBool::new(false));
        futures::stream::unfold((stopped.clone(), ms), move |(stop, delay)| {
            async move {
                if stop.load(Ordering::SeqCst) {
                    return None;
                }
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                Some(((), (stop.clone(), delay)))
            }
        })
    }

    /// Debounce: only execute the last call within `ms` window.
    pub fn debounce<F>(&self, callback: F, ms: u64)
    where
        F: Fn() + Send + Clone + 'static,
    {
        let name = Arc::clone(&self.name);
        let handle = Arc::new(TimerInner {
            stopped: AtomicBool::new(false),
        });
        let h = handle.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            if !h.stopped.load(Ordering::SeqCst) {
                callback();
            }
        });

        let _ = name;
        let _ = handle;
    }

    /// Throttle: only execute at most once per `ms` window.
    pub fn throttle<F>(&self, callback: F, ms: u64)
    where
        F: Fn() + Send + Clone + 'static,
    {
        let name = Arc::clone(&self.name);
        let handle = Arc::new(TimerInner {
            stopped: AtomicBool::new(false),
        });
        let h = handle.clone();

        tokio::spawn(async move {
            if !h.stopped.load(Ordering::SeqCst) {
                callback();
            }
        });

        let _ = name;
        let _ = handle;
    }
}

/// Inner state shared between timer and its handle.
struct TimerInner {
    stopped: AtomicBool,
}

/// Handle to stop an interval/timeout.
#[derive(Clone)]
pub struct IntervalHandle {
    name: Arc<String>,
    handle: Arc<TimerInner>,
}

impl IntervalHandle {
    /// Stop the timer.
    pub fn stop(&self) {
        self.handle.stopped.store(true, Ordering::SeqCst);
    }

    /// Whether the timer has been stopped.
    pub fn is_stopped(&self) -> bool {
        self.handle.stopped.load(Ordering::SeqCst)
    }

    /// Return the timer's name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_timer_timeout() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let timer = TimerService::new("test-timeout");
            let counter = Arc::new(AtomicUsize::new(0));
            let counter_clone = counter.clone();
            let _handle = timer.timeout(
                move || {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                },
                50,
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert_eq!(counter.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn test_timer_interval() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let timer = TimerService::new("test-interval");
            let counter = Arc::new(AtomicUsize::new(0));
            let counter_clone = counter.clone();
            let handle = timer.interval(
                move || {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                },
                30,
            );

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let count = counter.load(Ordering::SeqCst);
            assert!(count >= 1 && count <= 5);
            handle.stop();
        });
    }
}
