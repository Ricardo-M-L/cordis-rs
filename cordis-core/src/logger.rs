//! Logger — structured logging service for Cordis.
//!
//! Mirrors the TypeScript `LoggerService` from cordis-core.

use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// LoggerLevel
// ---------------------------------------------------------------------------

/// Log severity levels (matches TypeScript `LoggerLevel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LoggerLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl Default for LoggerLevel {
    fn default() -> Self {
        LoggerLevel::Info
    }
}

impl std::fmt::Display for LoggerLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoggerLevel::Debug => write!(f, "Debug"),
            LoggerLevel::Info => write!(f, "Info"),
            LoggerLevel::Warn => write!(f, "Warn"),
            LoggerLevel::Error => write!(f, "Error"),
        }
    }
}

impl LoggerLevel {
    /// Return the numeric value of this level.
    pub fn as_num(&self) -> usize {
        *self as usize
    }
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// A single log message.
#[derive(Debug)]
pub struct Message {
    pub timestamp: u64,
    pub msg: String,
    pub args: Vec<Box<dyn Any + Send + Sync>>,
    pub level: LoggerLevel,
    pub name: String,
}

impl Message {
    /// Format the message for display, optionally truncating at max_length.
    pub fn to_string(&self, max_length: Option<usize>) -> String {
        let mut output = format!(
            "[{}] {} {} ",
            self.level,
            chrono_or_naive_time(self.timestamp),
            self.name,
        );
        output.push_str(&self.msg);
        if !self.args.is_empty() {
            output.push_str(" (");
            let args: Vec<String> = self.args.iter().map(|a| format!("{:?}", a)).collect();
            output.push_str(&args.join(", "));
            output.push(')');
        }
        if let Some(max_len) = max_length {
            if output.len() > max_len {
                output = output[..max_len].to_string() + "...";
            }
        }
        output
    }
}

fn chrono_or_naive_time(ts_ms: u64) -> String {
    // Simple millisecond timestamp display (no chrono dependency needed)
    let secs = ts_ms / 1000;
    let millis = (ts_ms % 1000) as usize;
    format!("{}.{:03}", secs, millis)
}

// ---------------------------------------------------------------------------
// Exporter trait
// ---------------------------------------------------------------------------

/// A log exporter handles messages (print to console, write to file, etc.).
pub trait Exporter: Send + Sync {
    fn colors(&self) -> bool;
    fn max_length(&self) -> Option<usize>;
    fn export(&self, _msg: &Message);
}

// ---------------------------------------------------------------------------
// LoggerService
// ---------------------------------------------------------------------------

/// The central logging service. Collects exporters and dispatches messages.
pub struct LoggerService {
    name: String,
    level: Mutex<LoggerLevel>,
    exporters: Mutex<Vec<Box<dyn Exporter>>>,
    buffer: Mutex<VecDeque<Message>>,
    buffer_size: usize,
}

impl LoggerService {
    /// Create a new LoggerService with the given name.
    pub fn new(name: &str) -> Self {
        LoggerService {
            name: name.to_string(),
            level: Mutex::new(LoggerLevel::Info),
            exporters: Mutex::new(Vec::new()),
            buffer: Mutex::new(VecDeque::with_capacity(1000)),
            buffer_size: 1000,
        }
    }

    /// Create a clone sharing the same internal state (Arc-based).
    pub fn with_name(name: &str) -> Arc<Self> {
        Arc::new(LoggerService::new(name))
    }

    /// Set the minimum log level.
    pub fn set_level(&self, level: LoggerLevel) {
        let mut lvl = self.level.lock().unwrap();
        *lvl = level;
    }

    /// Add an exporter.
    pub fn add_exporter(&self, exporter: Box<dyn Exporter>) {
        let mut exps = self.exporters.lock().unwrap();
        exps.push(exporter);
    }

    /// Log at info level.
    pub fn info(&self, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        self.log(LogLevelKind::Info, msg, args);
    }

    /// Log at warn level.
    pub fn warn(&self, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        self.log(LogLevelKind::Warn, msg, args);
    }

    /// Log at error level.
    pub fn error(&self, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        self.log(LogLevelKind::Error, msg, args);
    }

    /// Log at debug level.
    pub fn debug(&self, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        self.log(LogLevelKind::Debug, msg, args);
    }

    fn log(&self, kind: LogLevelKind, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        let level = *self.level.lock().unwrap();
        if kind.to_level() < level {
            return;
        }

        let message = Message {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            msg: msg.to_string(),
            args,
            level: kind.to_level(),
            name: self.name.clone(),
        };

        let exps = self.exporters.lock().unwrap();
        for exporter in exps.iter() {
            exporter.export(&message);
        }

        let mut buf = self.buffer.lock().unwrap();
        buf.push_back(message);
        while buf.len() > self.buffer_size {
            buf.pop_front();
        }
    }
}

/// Internal helper to map method names to levels.
enum LogLevelKind {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevelKind {
    fn to_level(&self) -> LoggerLevel {
        match self {
            LogLevelKind::Debug => LoggerLevel::Debug,
            LogLevelKind::Info => LoggerLevel::Info,
            LogLevelKind::Warn => LoggerLevel::Warn,
            LogLevelKind::Error => LoggerLevel::Error,
        }
    }
}

impl std::fmt::Debug for LoggerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = *self.level.lock().unwrap();
        f.debug_struct("LoggerService")
            .field("name", &self.name)
            .field("level", &level)
            .finish()
    }
}
