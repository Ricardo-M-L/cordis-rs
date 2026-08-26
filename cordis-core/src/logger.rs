//! Structured logging with safe formatting and reentrant exporters.

use crate::utils::lock;
use std::any::Any;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LoggerLevel {
    Debug = 0,
    #[default]
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl std::fmt::Display for LoggerLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Debug => "Debug",
            Self::Info => "Info",
            Self::Warn => "Warn",
            Self::Error => "Error",
        })
    }
}

impl LoggerLevel {
    pub fn as_num(&self) -> usize {
        *self as usize
    }
}

#[derive(Debug)]
pub struct Message {
    pub timestamp: u64,
    pub msg: String,
    pub args: Vec<Box<dyn Any + Send + Sync>>,
    pub level: LoggerLevel,
    pub name: String,
}

impl Message {
    /// Format `%s`, `%d`, `%o`, and `%%` placeholders. Unknown argument types are
    /// represented as `<opaque>` instead of leaking implementation-dependent `Any` output.
    pub fn formatted_body(&self) -> String {
        let mut output = String::new();
        let mut chars = self.msg.chars().peekable();
        let mut argument_index = 0;

        while let Some(ch) = chars.next() {
            if ch != '%' {
                output.push(ch);
                continue;
            }
            let Some(specifier) = chars.peek().copied() else {
                output.push('%');
                break;
            };
            if specifier == '%' {
                chars.next();
                output.push('%');
                continue;
            }
            if !matches!(specifier, 's' | 'd' | 'o') || argument_index >= self.args.len() {
                output.push('%');
                continue;
            }

            chars.next();
            output.push_str(&format_argument(
                self.args[argument_index].as_ref(),
                specifier,
            ));
            argument_index += 1;
        }

        if argument_index < self.args.len() {
            let remaining = self.args[argument_index..]
                .iter()
                .map(|value| format_argument(value.as_ref(), 'o'))
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(" (");
            output.push_str(&remaining);
            output.push(')');
        }
        output
    }

    pub fn to_string(&self, max_length: Option<usize>) -> String {
        let output = format!(
            "[{}] {} {} {}",
            self.level,
            format_timestamp(self.timestamp),
            self.name,
            self.formatted_body()
        );
        truncate_chars(output, max_length)
    }
}

fn format_timestamp(timestamp_ms: u64) -> String {
    format!("{}.{:03}", timestamp_ms / 1000, timestamp_ms % 1000)
}

fn truncate_chars(output: String, max_length: Option<usize>) -> String {
    let Some(max_length) = max_length else {
        return output;
    };
    if output.chars().count() <= max_length {
        return output;
    }
    let mut truncated: String = output.chars().take(max_length).collect();
    truncated.push_str("...");
    truncated
}

fn format_argument(value: &(dyn Any + Send + Sync), specifier: char) -> String {
    macro_rules! numeric {
        ($($kind:ty),+ $(,)?) => {
            $(if let Some(value) = value.downcast_ref::<$kind>() {
                return value.to_string();
            })+
        };
    }

    if let Some(value) = value.downcast_ref::<String>() {
        return value.clone();
    }
    if let Some(value) = value.downcast_ref::<&str>() {
        return (*value).to_string();
    }
    numeric!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64);
    if let Some(value) = value.downcast_ref::<bool>() {
        return value.to_string();
    }
    if let Some(value) = value.downcast_ref::<char>() {
        return value.to_string();
    }
    if let Some(value) = value.downcast_ref::<serde_json::Value>() {
        return match specifier {
            's' => value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_string),
            _ => value.to_string(),
        };
    }
    "<opaque>".to_string()
}

pub trait Exporter: Send + Sync {
    fn colors(&self) -> bool;
    fn max_length(&self) -> Option<usize>;
    fn export(&self, msg: &Message);
}

pub struct LoggerService {
    name: String,
    level: Mutex<LoggerLevel>,
    exporters: Mutex<Vec<Arc<dyn Exporter>>>,
    buffer: Mutex<VecDeque<Arc<Message>>>,
    buffer_size: usize,
}

impl LoggerService {
    pub fn new(name: &str) -> Self {
        Self::with_buffer_size(name, 1000)
    }

    pub fn with_buffer_size(name: &str, buffer_size: usize) -> Self {
        Self {
            name: name.to_string(),
            level: Mutex::new(LoggerLevel::Info),
            exporters: Mutex::new(Vec::new()),
            buffer: Mutex::new(VecDeque::with_capacity(buffer_size)),
            buffer_size,
        }
    }

    pub fn with_name(name: &str) -> Arc<Self> {
        Arc::new(Self::new(name))
    }

    pub fn set_level(&self, level: LoggerLevel) {
        *lock(&self.level) = level;
    }

    pub fn add_exporter(&self, exporter: Box<dyn Exporter>) {
        lock(&self.exporters).push(Arc::from(exporter));
    }

    pub fn clear_exporters(&self) {
        lock(&self.exporters).clear();
    }

    pub fn messages(&self) -> Vec<Arc<Message>> {
        lock(&self.buffer).iter().cloned().collect()
    }

    pub fn info(&self, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        self.log(LoggerLevel::Info, msg, args);
    }

    pub fn warn(&self, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        self.log(LoggerLevel::Warn, msg, args);
    }

    pub fn error(&self, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        self.log(LoggerLevel::Error, msg, args);
    }

    pub fn debug(&self, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        self.log(LoggerLevel::Debug, msg, args);
    }

    fn log(&self, level: LoggerLevel, msg: &str, args: Vec<Box<dyn Any + Send + Sync>>) {
        if level < *lock(&self.level) {
            return;
        }

        let message = Arc::new(Message {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            msg: msg.to_string(),
            args,
            level,
            name: self.name.clone(),
        });

        // Clone exporter Arcs before callbacks so an exporter can safely reenter the logger.
        let exporters = lock(&self.exporters).clone();
        for exporter in exporters {
            let message = Arc::clone(&message);
            let _ = catch_unwind(AssertUnwindSafe(|| exporter.export(&message)));
        }

        let mut buffer = lock(&self.buffer);
        if self.buffer_size == 0 {
            return;
        }
        buffer.push_back(message);
        while buffer.len() > self.buffer_size {
            buffer.pop_front();
        }
    }
}

impl std::fmt::Debug for LoggerService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoggerService")
            .field("name", &self.name)
            .field("level", &*lock(&self.level))
            .field("exporters", &lock(&self.exporters).len())
            .field("buffered", &lock(&self.buffer).len())
            .finish()
    }
}
