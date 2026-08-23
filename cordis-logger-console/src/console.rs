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
    pub fn default() -> Self {
        ConsoleExporter {
            config: ConsoleExporterConfig::default(),
        }
    }

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

        let mut output = String::new();
        if self.config.show_time {
            output.push_str(&format!(
                "[{}:{}]",
                msg.timestamp / 1000,
                (msg.timestamp % 1000) as usize
            ));
        }
        output.push_str(&format!(
            "{}[{}]{} {} {}{}",
            color, msg.level, reset, msg.name, msg.msg, reset
        ));

        eprintln!("{}", output);
    }
}
