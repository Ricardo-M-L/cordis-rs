//! JSON, YAML, and TOML configuration loading with deterministic patch application.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum IncludeError {
    Io(std::io::Error),
    UnsupportedFormat(PathBuf),
    Parse(String),
    RootMustBeObject(PathBuf),
    InvalidPatch(String),
}

impl std::fmt::Display for IncludeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "configuration I/O error: {error}"),
            Self::UnsupportedFormat(path) => {
                write!(
                    formatter,
                    "unsupported configuration format: {}",
                    path.display()
                )
            }
            Self::Parse(error) => write!(formatter, "configuration parse error: {error}"),
            Self::RootMustBeObject(path) => {
                write!(
                    formatter,
                    "configuration root must be an object: {}",
                    path.display()
                )
            }
            Self::InvalidPatch(error) => write!(formatter, "invalid configuration patch: {error}"),
        }
    }
}

impl std::error::Error for IncludeError {}

impl From<std::io::Error> for IncludeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Patch {
    path: String,
    value: serde_json::Value,
}

impl Patch {
    pub fn new(path: &str, value: serde_json::Value) -> Self {
        Self {
            path: path.to_string(),
            value,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn value(&self) -> &serde_json::Value {
        &self.value
    }

    /// Apply a dot-separated path. Scalar intermediate values are safely replaced
    /// with objects, so paths such as `a.b` work even when `a` was previously numeric.
    pub fn apply(&self, config: &mut serde_json::Value) -> Result<(), IncludeError> {
        let mut updated = config.clone();
        self.apply_in_place(&mut updated)?;
        *config = updated;
        Ok(())
    }

    fn apply_in_place(&self, config: &mut serde_json::Value) -> Result<(), IncludeError> {
        let parts: Vec<_> = self
            .path
            .split('.')
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() {
            return Err(IncludeError::InvalidPatch(
                "patch path cannot be empty".to_string(),
            ));
        }

        let mut current = config;
        for (index, part) in parts.iter().enumerate() {
            let last = index + 1 == parts.len();
            if last {
                set_child(current, part, self.value.clone())?;
            } else {
                current = child_mut(current, part)?;
            }
        }
        Ok(())
    }
}

fn child_mut<'a>(
    current: &'a mut serde_json::Value,
    part: &str,
) -> Result<&'a mut serde_json::Value, IncludeError> {
    if current.is_array() {
        if let Ok(index) = part.parse::<usize>() {
            let array = current.as_array_mut().expect("array checked");
            if array.len() <= index {
                array.resize(index + 1, serde_json::Value::Null);
            }
            return Ok(&mut array[index]);
        }
        return Err(IncludeError::InvalidPatch(format!(
            "array segment must be an index, got {part}"
        )));
    }
    if !current.is_object() {
        *current = serde_json::Value::Object(serde_json::Map::new());
    }
    Ok(current
        .as_object_mut()
        .expect("object created")
        .entry(part.to_string())
        .or_insert(serde_json::Value::Null))
}

fn set_child(
    current: &mut serde_json::Value,
    part: &str,
    value: serde_json::Value,
) -> Result<(), IncludeError> {
    if current.is_array() {
        if let Ok(index) = part.parse::<usize>() {
            let array = current.as_array_mut().expect("array checked");
            if array.len() <= index {
                array.resize(index + 1, serde_json::Value::Null);
            }
            array[index] = value;
            return Ok(());
        }
        return Err(IncludeError::InvalidPatch(format!(
            "array segment must be an index, got {part}"
        )));
    }
    if !current.is_object() {
        *current = serde_json::Value::Object(serde_json::Map::new());
    }
    current
        .as_object_mut()
        .expect("object created")
        .insert(part.to_string(), value);
    Ok(())
}

pub struct IncludePlugin {
    name: String,
    patches: Vec<Patch>,
}

impl IncludePlugin {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            patches: Vec::new(),
        }
    }

    pub fn with_patches(name: &str, patches: Vec<Patch>) -> Self {
        Self {
            name: name.to_string(),
            patches,
        }
    }

    pub fn apply_patches(
        &self,
        config: &mut HashMap<String, serde_json::Value>,
    ) -> Result<(), IncludeError> {
        let map = config
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<serde_json::Map<String, serde_json::Value>>();
        let mut value = serde_json::Value::Object(map);
        for patch in &self.patches {
            patch.apply(&mut value)?;
        }
        if let serde_json::Value::Object(map) = value {
            config.clear();
            config.extend(map);
        }
        Ok(())
    }

    /// Load a configuration file and apply this plugin's patches.
    pub fn load_path(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<HashMap<String, serde_json::Value>, IncludeError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path)?;
        let extension = path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| IncludeError::UnsupportedFormat(path.to_path_buf()))?;

        let value = match extension.as_str() {
            "json" => serde_json::from_str(&source)
                .map_err(|error| IncludeError::Parse(error.to_string()))?,
            "yaml" | "yml" => serde_yaml::from_str(&source)
                .map_err(|error| IncludeError::Parse(error.to_string()))?,
            "toml" => {
                let value: toml::Value = toml::from_str(&source)
                    .map_err(|error| IncludeError::Parse(error.to_string()))?;
                serde_json::to_value(value)
                    .map_err(|error| IncludeError::Parse(error.to_string()))?
            }
            _ => return Err(IncludeError::UnsupportedFormat(path.to_path_buf())),
        };

        let serde_json::Value::Object(map) = value else {
            return Err(IncludeError::RootMustBeObject(path.to_path_buf()));
        };
        let mut config: HashMap<_, _> = map.into_iter().collect();
        self.apply_patches(&mut config)?;
        Ok(config)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn patches(&self) -> &[Patch] {
        &self.patches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cordis-include-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            extension
        ))
    }

    #[test]
    fn patch_replaces_scalar_intermediate_without_panicking() {
        let mut config = serde_json::json!({"server": 1});
        Patch::new("server.port", serde_json::json!(8080))
            .apply(&mut config)
            .expect("apply patch");
        assert_eq!(config, serde_json::json!({"server": {"port": 8080}}));
    }

    #[test]
    fn patch_updates_array_index() {
        let mut config = serde_json::json!({"servers": [{"port": 1}]});
        Patch::new("servers.0.port", serde_json::json!(8080))
            .apply(&mut config)
            .expect("apply patch");
        assert_eq!(config["servers"][0]["port"], 8080);
    }

    #[test]
    fn invalid_array_patch_is_atomic() {
        let original = serde_json::json!({"servers": [1]});
        let mut config = original.clone();
        assert!(Patch::new("servers.first.port", serde_json::json!(8080))
            .apply(&mut config)
            .is_err());
        assert_eq!(config, original);
    }

    #[test]
    fn loads_and_patches_yaml() {
        let path = temp_path("yaml");
        std::fs::write(&path, "server:\n  port: 3000\n").expect("write YAML fixture");
        let plugin = IncludePlugin::with_patches(
            "yaml",
            vec![Patch::new("server.port", serde_json::json!(8080))],
        );
        let config = plugin.load_path(&path).expect("load YAML");
        std::fs::remove_file(path).expect("remove YAML fixture");
        assert_eq!(config["server"]["port"], 8080);
    }

    #[test]
    fn loads_toml() {
        let path = temp_path("toml");
        std::fs::write(&path, "[server]\nport = 3000\n").expect("write TOML fixture");
        let config = IncludePlugin::new("toml")
            .load_path(&path)
            .expect("load TOML");
        std::fs::remove_file(path).expect("remove TOML fixture");
        assert_eq!(config["server"]["port"], 3000);
    }
}
