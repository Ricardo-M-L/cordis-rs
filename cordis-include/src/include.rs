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
    FileTooLarge(PathBuf, u64),
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
            Self::FileTooLarge(path, size) => {
                write!(
                    formatter,
                    "configuration file too large: {} ({} bytes)",
                    path.display(),
                    size
                )
            }
        }
    }
}

impl std::error::Error for IncludeError {}

impl From<std::io::Error> for IncludeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;
const DEFAULT_MAX_PATCH_DEPTH: usize = 64;

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

    /// Apply a dot-separated path.
    /// - Default mode is lenient (legacy behavior): scalar values on intermediate nodes are
    ///   replaced with objects to keep patching flexible.
    /// - Strict mode keeps intermediate scalar nodes and returns an error.
    pub fn apply(&self, config: &mut serde_json::Value) -> Result<(), IncludeError> {
        self.apply_with_options(config, false, DEFAULT_MAX_PATCH_DEPTH)
    }

    pub fn apply_with_options(
        &self,
        config: &mut serde_json::Value,
        strict: bool,
        max_depth: usize,
    ) -> Result<(), IncludeError> {
        self.apply_in_place(config, strict, max_depth)
    }

    fn path_parts(&self) -> Result<Vec<&str>, IncludeError> {
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
        Ok(parts)
    }

    fn apply_in_place(
        &self,
        config: &mut serde_json::Value,
        strict: bool,
        max_depth: usize,
    ) -> Result<(), IncludeError> {
        let parts = self.path_parts()?;
        if parts.len() > max_depth {
            return Err(IncludeError::InvalidPatch(format!(
                "patch path exceeds max depth: {} > {}",
                parts.len(),
                max_depth
            )));
        }

        let mut updated = config.clone();
        let mut current = &mut updated;
        for (index, part) in parts.iter().enumerate() {
            let last = index + 1 == parts.len();
            if last {
                set_child(current, part, self.value.clone(), strict)?;
            } else {
                current = child_mut(current, part, strict)?;
            }
        }
        *config = updated;
        Ok(())
    }
}

fn child_mut<'a>(
    current: &'a mut serde_json::Value,
    part: &str,
    strict: bool,
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
    if strict && !current.is_object() {
        return Err(IncludeError::InvalidPatch(format!(
            "strict mode disallows replacing scalar path segment: {part}"
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
    strict: bool,
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
    if strict && !current.is_object() {
        return Err(IncludeError::InvalidPatch(format!(
            "strict mode disallows replacing scalar path target: {part}"
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
    max_file_bytes: u64,
    max_patch_depth: usize,
    strict: bool,
}

impl IncludePlugin {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            patches: Vec::new(),
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_patch_depth: DEFAULT_MAX_PATCH_DEPTH,
            strict: false,
        }
    }

    pub fn with_patches(name: &str, patches: Vec<Patch>) -> Self {
        Self {
            name: name.to_string(),
            patches,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_patch_depth: DEFAULT_MAX_PATCH_DEPTH,
            strict: false,
        }
    }

    pub fn with_options(
        name: &str,
        patches: Vec<Patch>,
        max_file_bytes: u64,
        max_patch_depth: usize,
        strict: bool,
    ) -> Self {
        Self {
            name: name.to_string(),
            patches,
            max_file_bytes,
            max_patch_depth,
            strict,
        }
    }

    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }

    pub fn max_patch_depth(&self) -> usize {
        self.max_patch_depth
    }

    pub fn strict(&self) -> bool {
        self.strict
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
            patch.apply_with_options(&mut value, self.strict, self.max_patch_depth)?;
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
        let size = path.metadata()?.len();
        if size > self.max_file_bytes {
            return Err(IncludeError::FileTooLarge(path.to_path_buf(), size));
        }

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
    fn patch_replaces_scalar_intermediate_in_strict_mode() {
        let mut config = serde_json::json!({"server": 1});
        let result = Patch::new("server.port", serde_json::json!(8080)).apply_with_options(
            &mut config,
            true,
            DEFAULT_MAX_PATCH_DEPTH,
        );
        assert!(matches!(result, Err(IncludeError::InvalidPatch(_))));
    }

    #[test]
    fn patch_checks_max_depth_limit() {
        let mut config = serde_json::json!({"a": {}});
        let result =
            Patch::new("a.b.c.d", serde_json::json!(1)).apply_with_options(&mut config, false, 2);
        assert!(matches!(result, Err(IncludeError::InvalidPatch(_))));
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

    #[test]
    fn blocks_overly_large_files() {
        let path = temp_path("json");
        std::fs::write(&path, "{ \"a\": 1 }").expect("write tiny fixture");
        let plugin = IncludePlugin::with_options("json", Vec::new(), 1, DEFAULT_MAX_PATCH_DEPTH, false);
        let result = plugin.load_path(&path);
        assert!(matches!(result, Err(IncludeError::FileTooLarge(_, _))));
        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn validates_patch_depth_limit_from_plugin() {
        let plugin = IncludePlugin::with_options(
            "depth",
            vec![Patch::new("a.b.c", serde_json::json!(1))],
            DEFAULT_MAX_FILE_BYTES,
            2,
            false,
        );
        let mut config = HashMap::new();
        config.insert("a".to_string(), serde_json::json!({}));
        let result = plugin.apply_patches(&mut config);
        assert!(result.is_err());
    }

    #[test]
    fn plugin_depth_accessors() {
        let plugin = IncludePlugin::with_options("x", Vec::new(), 7, 5, true);
        assert_eq!(plugin.max_file_bytes(), 7);
        assert_eq!(plugin.max_patch_depth(), 5);
        assert!(plugin.strict());
    }
}
