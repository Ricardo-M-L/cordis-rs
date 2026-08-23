//! Include — config file loading and patch system.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Patch
// ---------------------------------------------------------------------------

/// A config patch that modifies a specific path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patch {
    path: String,
    value: serde_json::Value,
}

impl Patch {
    /// Create a new Patch targeting the given JSON path.
    pub fn new(path: &str, value: serde_json::Value) -> Self {
        Patch {
            path: path.to_string(),
            value,
        }
    }

    /// Return the patch's target path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Return the patch's value.
    pub fn value(&self) -> &serde_json::Value {
        &self.value
    }

    /// Apply this patch to the given config.
    pub fn apply(&self, config: &mut serde_json::Value) {
        let parts: Vec<&str> = self.path.split('.').collect();
        let mut current = config;

        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                current[part] = self.value.clone();
                return;
            }

            if current.get(part).is_none() {
                current[part] = serde_json::Value::Object(serde_json::Map::new());
            }
            current = &mut current[part];
        }
    }
}

// ---------------------------------------------------------------------------
// IncludePlugin
// ---------------------------------------------------------------------------

/// Plugin that loads and patches config files.
pub struct IncludePlugin {
    name: String,
    patches: Vec<Patch>,
}

impl IncludePlugin {
    /// Create a new IncludePlugin with the given name.
    pub fn new(name: &str) -> Self {
        IncludePlugin {
            name: name.to_string(),
            patches: Vec::new(),
        }
    }

    /// Create a new IncludePlugin with patches.
    pub fn with_patches(name: &str, patches: Vec<Patch>) -> Self {
        IncludePlugin {
            name: name.to_string(),
            patches,
        }
    }

    /// Apply all patches to the given config map.
    pub fn apply_patches(&self, config: &mut HashMap<String, serde_json::Value>) {
        let config_json = serde_json::to_value(&*config).unwrap_or(
            serde_json::Value::Object(serde_json::Map::new()),
        );

        let mut config_val = config_json;
        for patch in &self.patches {
            patch.apply(&mut config_val);
        }

        // Update the HashMap
        if let serde_json::Value::Object(map) = config_val {
            config.clear();
            for (k, v) in map {
                config.insert(k, v);
            }
        }
    }

    /// Return the plugin's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the patches.
    pub fn patches(&self) -> &[Patch] {
        &self.patches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_include_patch() {
        let mut config = HashMap::new();
        config.insert("server".to_string(), serde_json::json!({"port": 3000}));

        let patches = vec![Patch::new(
            "server.port",
            serde_json::json!(8080),
        )];

        let plugin = IncludePlugin::with_patches("test-include", patches);
        plugin.apply_patches(&mut config);

        let server = config.get("server").unwrap();
        assert_eq!(server, &serde_json::json!({"port": 8080}));
    }
}
