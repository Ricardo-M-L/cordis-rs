//! Cordis Create — CLI scaffolding for new Cordis projects.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// CreateOptions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateOptions {
    pub name: String,
    pub template: String,
    pub ref_tag: Option<String>,
    pub git: bool,
    pub forced: bool,
    pub mirror: Option<String>,
    pub prod: bool,
    pub yes: bool,
}

// ---------------------------------------------------------------------------
// CreateCli
// ---------------------------------------------------------------------------

/// CLI entry point for creating new Cordis projects.
pub struct CreateCli {
    options: CreateOptions,
    target_dir: PathBuf,
}

impl CreateCli {
    /// Create a new CreateCli with the given options.
    pub fn new(options: CreateOptions) -> Self {
        CreateCli {
            options,
            target_dir: PathBuf::new(),
        }
    }

    /// Generate a project template at the given target directory.
    pub fn generate_template(&self, target: &str) -> PathBuf {
        let target_path = PathBuf::from(target);

        // Create the target directory structure
        std::fs::create_dir_all(&target_path).ok();

        // Write a basic Cargo.toml
        let cargo_toml = format!(
            r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"
"#,
            self.options.name
        );
        let _ = std::fs::write(target_path.join("Cargo.toml"), cargo_toml);

        // Create src directory
        let _ = std::fs::create_dir_all(target_path.join("src"));

        // Write a basic main.rs
        let main_rs = "fn main() {\n    println!(\"Hello, {}!\");\n}\n".to_string();
        let _ = std::fs::write(target_path.join("src/main.rs"), main_rs);

        target_path
    }

    /// Return the options.
    pub fn options(&self) -> &CreateOptions {
        &self.options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_template() {
        let options = CreateOptions {
            name: "my-cordis-app".to_string(),
            template: "@cordisjs/boilerplate".to_string(),
            ..Default::default()
        };

        let cli = CreateCli::new(options);
        let target = std::env::temp_dir().join("cordis-template-test");
        let _ = std::fs::remove_dir_all(&target);

        let result = cli.generate_template(target.to_str().unwrap());
        assert!(result.exists());
        assert!(result.join("Cargo.toml").exists());
        assert!(result.join("src/main.rs").exists());

        let _ = std::fs::remove_dir_all(&target);
    }
}
