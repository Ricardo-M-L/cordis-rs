//! Console log exporter — ANSI-colored output with labels and formatting.

use cordis_core::logger::{Exporter, LoggerLevel, Message};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ConsoleExporterConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleExporterConfig {
    pub colors: bool,
    pub max_length: Option<usize>,
    pub show_time: bool,
    pub label: Option<String>,
}

impl Default for ConsoleExporterConfig {
    fn default() -> Self {
        ConsoleExporterConfig {
            colors: true,
            max_length: Some(1024),
            show_time: true,
            label: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ConsoleExporter
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct ConsoleExporter {
    pub config: ConsoleExporterConfig,
}

impl ConsoleExporter {
    pub fn new(config: ConsoleExporterConfig) -> Self {
        ConsoleExporter { config }
    }

    fn level_color(&self, level: LoggerLevel) -> &'static str {
        if !self.config.colors {
            return "";
        }
        match level {
            LoggerLevel::Debug => "\x1b[36m", // cyan
            LoggerLevel::Info => "\x1b[32m",  // green
            LoggerLevel::Warn => "\x1b[33m",  // yellow
            LoggerLevel::Error => "\x1b[31m", // red
        }
    }
}

impl Exporter for ConsoleExporter {
    fn colors(&self) -> bool {
        self.config.colors
    }

    fn max_length(&self) -> Option<usize> {
        self.config.max_length
    }

    fn export(&self, msg: &Message) {
        let reset = if self.config.colors { "\x1b[0m" } else { "" };
        let color = self.level_color(msg.level);
        let label = self.config.label.as_deref().unwrap_or(&msg.name);
        let mut output = if self.config.show_time {
            format!(
                "[{}.{:03}] [{}] {} {}",
                msg.timestamp / 1000,
                msg.timestamp % 1000,
                msg.level,
                label,
                msg.formatted_body()
            )
        } else {
            format!("[{}] {} {}", msg.level, label, msg.formatted_body())
        };
        if let Some(max_length) = self.config.max_length {
            if output.chars().count() > max_length {
                output = output.chars().take(max_length).collect::<String>() + "...";
            }
        }
        eprintln!("{color}{output}{reset}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exporter_exposes_configured_capabilities() {
        let exporter = ConsoleExporter::new(ConsoleExporterConfig {
            colors: false,
            max_length: Some(12),
            show_time: false,
            label: Some("app".to_string()),
        });
        assert!(!exporter.colors());
        assert_eq!(exporter.max_length(), Some(12));
        assert_eq!(exporter.level_color(LoggerLevel::Error), "");
    }
}
