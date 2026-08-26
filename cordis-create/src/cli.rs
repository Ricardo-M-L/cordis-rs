//! Project scaffolding implementation used by the `cordis-create` binary.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            name: String::new(),
            template: "default".to_string(),
            ref_tag: None,
            git: false,
            forced: false,
            mirror: None,
            prod: false,
            yes: false,
        }
    }
}

#[derive(Debug)]
pub enum CreateError {
    InvalidName(String),
    TargetNotEmpty(PathBuf),
    UnsupportedTemplate(String),
    Io(std::io::Error),
    Git(String),
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(name) => write!(formatter, "invalid Cargo package name: {name}"),
            Self::TargetNotEmpty(path) => {
                write!(
                    formatter,
                    "target directory is not empty: {}",
                    path.display()
                )
            }
            Self::UnsupportedTemplate(template) => {
                write!(formatter, "unsupported template: {template}")
            }
            Self::Io(error) => write!(formatter, "project generation failed: {error}"),
            Self::Git(error) => write!(formatter, "git command failed: {error}"),
        }
    }
}

impl std::error::Error for CreateError {}

impl From<std::io::Error> for CreateError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct CreateCli {
    options: CreateOptions,
}

impl CreateCli {
    pub fn new(options: CreateOptions) -> Self {
        Self { options }
    }

    /// Generate a project and return errors to the caller.
    pub fn try_generate_template(&self, target: impl AsRef<Path>) -> Result<PathBuf, CreateError> {
        validate_name(&self.options.name)?;
        let target = target.as_ref().to_path_buf();
        prepare_target(&target, self.options.forced)?;

        if let Some(remote) = self.remote_template() {
            self.clone_remote(&remote, &target)?;
        } else {
            if self.options.ref_tag.is_some() {
                return Err(CreateError::UnsupportedTemplate(
                    "--ref requires --mirror or a Git template URL".to_string(),
                ));
            }
            self.write_builtin(&target)?;
        }
        self.customize_manifest(&target)?;

        if self.options.git && !target.join(".git").exists() {
            run_git(&target, ["init"])?;
        }
        Ok(target)
    }

    /// Generate a project from a string path.
    pub fn generate_template(&self, target: &str) -> Result<PathBuf, CreateError> {
        self.try_generate_template(target)
    }

    pub fn options(&self) -> &CreateOptions {
        &self.options
    }

    fn remote_template(&self) -> Option<String> {
        self.options.mirror.clone().or_else(|| {
            (self.options.template.starts_with("https://")
                || self.options.template.starts_with("http://")
                || self.options.template.starts_with("git@"))
            .then(|| self.options.template.clone())
        })
    }

    fn clone_remote(&self, remote: &str, target: &Path) -> Result<(), CreateError> {
        // `git clone` requires the target not to exist; `prepare_target` creates it for
        // built-ins, so remove the known-empty directory first.
        std::fs::remove_dir(target)?;
        let mut command = Command::new("git");
        command.arg("clone").arg("--depth").arg("1");
        if let Some(reference) = &self.options.ref_tag {
            command.arg("--branch").arg(reference);
        }
        let output = command.arg(remote).arg(target).output()?;
        if !output.status.success() {
            return Err(CreateError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    fn write_builtin(&self, target: &Path) -> Result<(), CreateError> {
        if !matches!(
            self.options.template.as_str(),
            "default" | "minimal" | "@cordisjs/boilerplate"
        ) {
            return Err(CreateError::UnsupportedTemplate(
                self.options.template.clone(),
            ));
        }

        std::fs::create_dir_all(target.join("src"))?;
        let cargo_toml = format!(
            r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
cordis-core = {{ git = "https://github.com/Ricardo-M-L/cordis-rs", package = "cordis-core" }}
"#,
            self.options.name
        );
        std::fs::write(target.join("Cargo.toml"), cargo_toml)?;
        let main_rs = format!(
            r#"use cordis_core::CordisContext;

fn main() {{
    let context = CordisContext::new();
    println!("{} started with context {{}}", context.isolate_id());
}}
"#,
            self.options.name
        );
        std::fs::write(target.join("src/main.rs"), main_rs)?;
        std::fs::write(target.join(".gitignore"), "/target\n")?;
        Ok(())
    }

    fn customize_manifest(&self, target: &Path) -> Result<(), CreateError> {
        let manifest_path = target.join("Cargo.toml");
        let source = std::fs::read_to_string(&manifest_path)?;
        let mut manifest: toml::Value = source
            .parse::<toml::Value>()
            .map_err(|error| CreateError::UnsupportedTemplate(error.to_string()))?;
        let package = manifest
            .get_mut("package")
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| {
                CreateError::UnsupportedTemplate(
                    "template Cargo.toml is missing [package]".to_string(),
                )
            })?;
        package.insert(
            "name".to_string(),
            toml::Value::String(self.options.name.clone()),
        );
        if self.options.prod {
            let root = manifest.as_table_mut().ok_or_else(|| {
                CreateError::UnsupportedTemplate("template manifest must be a table".to_string())
            })?;
            let profile = root
                .entry("profile")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
                .as_table_mut()
                .ok_or_else(|| {
                    CreateError::UnsupportedTemplate(
                        "template [profile] must be a table".to_string(),
                    )
                })?;
            let release = profile
                .entry("release")
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
                .as_table_mut()
                .ok_or_else(|| {
                    CreateError::UnsupportedTemplate(
                        "template [profile.release] must be a table".to_string(),
                    )
                })?;
            release.insert("lto".to_string(), toml::Value::Boolean(true));
            release.insert("codegen-units".to_string(), toml::Value::Integer(1));
        }
        std::fs::write(
            manifest_path,
            toml::to_string_pretty(&manifest)
                .map_err(|error| CreateError::UnsupportedTemplate(error.to_string()))?,
        )?;
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), CreateError> {
    let valid = !name.is_empty()
        && !name.starts_with(|character: char| character.is_ascii_digit())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(CreateError::InvalidName(name.to_string()))
    }
}

fn prepare_target(target: &Path, forced: bool) -> Result<(), CreateError> {
    if target.exists() {
        let not_empty = std::fs::read_dir(target)?.next().transpose()?.is_some();
        if not_empty && !forced {
            return Err(CreateError::TargetNotEmpty(target.to_path_buf()));
        }
        if not_empty {
            std::fs::remove_dir_all(target)?;
        }
    }
    std::fs::create_dir_all(target)?;
    Ok(())
}

fn run_git<const N: usize>(directory: &Path, args: [&str; N]) -> Result<(), CreateError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CreateError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_target(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cordis-create-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn generates_a_real_cordis_project() {
        let cli = CreateCli::new(CreateOptions {
            name: "my-cordis-app".to_string(),
            template: "default".to_string(),
            prod: true,
            ..CreateOptions::default()
        });
        let target = temp_target("success");
        let result = cli
            .try_generate_template(&target)
            .expect("generate project");
        let manifest =
            std::fs::read_to_string(result.join("Cargo.toml")).expect("read generated manifest");
        assert!(manifest.contains("cordis-core"));
        assert!(manifest.contains("[profile.release]"));
        assert!(result.join("src/main.rs").exists());
        std::fs::remove_dir_all(target).expect("remove generated project");
    }

    #[test]
    fn refuses_to_overwrite_without_force() {
        let cli = CreateCli::new(CreateOptions {
            name: "safe-app".to_string(),
            ..CreateOptions::default()
        });
        let target = temp_target("existing");
        std::fs::create_dir_all(&target).expect("create target");
        std::fs::write(target.join("keep.txt"), "user data").expect("write user file");
        assert!(matches!(
            cli.try_generate_template(&target),
            Err(CreateError::TargetNotEmpty(_))
        ));
        assert!(target.join("keep.txt").exists());
        std::fs::remove_dir_all(target).expect("remove target");
    }
}
