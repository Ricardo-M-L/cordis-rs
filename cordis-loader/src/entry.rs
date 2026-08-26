//! Runtime entry in a loader tree.

use cordis_core::CordisContext;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryConfig {
    pub name: String,
    #[serde(default)]
    pub children: Vec<EntryConfig>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub isolates: Vec<String>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default = "empty_object")]
    pub config: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

impl Default for EntryConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            children: Vec::new(),
            groups: Vec::new(),
            isolates: Vec::new(),
            disabled: false,
            config: empty_object(),
        }
    }
}

impl EntryConfig {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Self::default()
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    Pending,
    Active,
    Disabled,
    Failed,
    Unloaded,
}

pub struct Entry {
    config: EntryConfig,
    state: Mutex<EntryState>,
    error: Mutex<Option<String>>,
    context: Mutex<Option<CordisContext>>,
}

impl Entry {
    pub fn new(config: EntryConfig) -> Self {
        let state = if config.disabled {
            EntryState::Disabled
        } else {
            EntryState::Pending
        };
        Self {
            config,
            state: Mutex::new(state),
            error: Mutex::new(None),
            context: Mutex::new(None),
        }
    }

    pub fn config(&self) -> &EntryConfig {
        &self.config
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn is_disabled(&self) -> bool {
        self.config.is_disabled()
    }

    pub fn state(&self) -> EntryState {
        *lock(&self.state)
    }

    pub fn error(&self) -> Option<String> {
        lock(&self.error).clone()
    }

    pub fn context(&self) -> Option<CordisContext> {
        lock(&self.context).clone()
    }

    pub(crate) fn activate(&self, context: CordisContext) {
        *lock(&self.context) = Some(context);
        *lock(&self.error) = None;
        *lock(&self.state) = EntryState::Active;
    }

    pub(crate) fn fail(&self, error: String) {
        *lock(&self.error) = Some(error);
        *lock(&self.state) = EntryState::Failed;
    }

    pub(crate) fn unload(&self) {
        *lock(&self.context) = None;
        *lock(&self.state) = EntryState::Unloaded;
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Entry")
            .field("name", &self.name())
            .field("state", &self.state())
            .field("error", &self.error())
            .finish()
    }
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
    fn disabled_entries_have_explicit_state() {
        let mut config = EntryConfig::new("disabled");
        config.disabled = true;
        let entry = Entry::new(config);
        assert!(entry.is_disabled());
        assert_eq!(entry.state(), EntryState::Disabled);
    }
}
