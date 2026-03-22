//! Configuration loading and merging for relocal.
//!
//! Config comes from two layers: a user-wide `~/.relocal/config.toml` and a
//! per-repo `relocal.toml`. Both use the same schema. The project config
//! overrides the user config on a per-field basis (no list merging).
//!
//! Unknown keys are silently ignored so that older binaries can read configs
//! written for newer versions (forward compatibility).

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use serde::Deserialize;

/// Resolved configuration with all required fields present.
///
/// This is the type that the rest of the codebase uses. Produced by
/// [`PartialConfig::resolve`] after merging user and project layers.
#[derive(Debug, Clone)]
pub struct Config {
    pub remote: String,
    pub exclude: Vec<String>,
    pub apt_packages: Vec<String>,
}

impl Config {
    /// Parse a TOML string that must contain `remote`. Convenience for
    /// call sites that have a single authoritative config source.
    pub fn parse(input: &str) -> Result<Self> {
        PartialConfig::parse(input, "relocal.toml")?.resolve()
    }
}

/// A config layer where every field is optional.
///
/// Used for deserialization of both user and project config files before
/// merging.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartialConfig {
    pub remote: Option<String>,
    pub exclude: Option<Vec<String>>,
    pub apt_packages: Option<Vec<String>>,
}

impl PartialConfig {
    pub fn parse(input: &str, path: &str) -> Result<Self> {
        toml::from_str(input).map_err(|e| Error::ConfigParse {
            path: path.to_string(),
            reason: e.to_string(),
        })
    }

    /// Overlay `over` on top of `self`. For each field, `over` wins if present.
    pub fn merge(self, over: PartialConfig) -> PartialConfig {
        PartialConfig {
            remote: over.remote.or(self.remote),
            exclude: over.exclude.or(self.exclude),
            apt_packages: over.apt_packages.or(self.apt_packages),
        }
    }

    /// Convert to a resolved [`Config`], failing if `remote` is missing.
    pub fn resolve(self) -> Result<Config> {
        let remote = self.remote.ok_or_else(|| Error::ConfigParse {
            path: "config".to_string(),
            reason: "missing field `remote` (not set in ~/.relocal/config.toml or relocal.toml)"
                .to_string(),
        })?;
        Ok(Config {
            remote,
            exclude: self.exclude.unwrap_or_default(),
            apt_packages: self.apt_packages.unwrap_or_default(),
        })
    }
}

/// Read and parse a config file. Returns `None` if the file does not exist.
/// Returns an error if the file exists but cannot be read or parsed.
fn load_optional_config(path: &Path) -> Result<Option<PartialConfig>> {
    let display = path.display().to_string();
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(PartialConfig::parse(&contents, &display)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Path to the user-level config file.
fn user_config_path(home: &Path) -> PathBuf {
    home.join(".relocal").join("config.toml")
}

/// Load and merge config from user and project layers.
///
/// The project config overrides the user config per-field. The merged result
/// must have `remote`.
pub fn load_merged_config(home: &Path, repo_root: &Path) -> Result<Config> {
    let mut base = PartialConfig::default();
    if let Some(user) = load_optional_config(&user_config_path(home))? {
        base = base.merge(user);
    }
    if let Some(project) = load_optional_config(&repo_root.join("relocal.toml"))? {
        base = base.merge(project);
    }
    base.resolve()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn minimal_config() {
        let config = Config::parse("remote = \"user@host\"").unwrap();
        assert_eq!(config.remote, "user@host");
        assert!(config.exclude.is_empty());
        assert!(config.apt_packages.is_empty());
    }

    #[test]
    fn full_config() {
        let input = r#"
remote = "user@host"
exclude = [".env", "secrets/"]
apt_packages = ["libssl-dev", "pkg-config"]
"#;
        let config = Config::parse(input).unwrap();
        assert_eq!(config.remote, "user@host");
        assert_eq!(config.exclude, vec![".env", "secrets/"]);
        assert_eq!(config.apt_packages, vec!["libssl-dev", "pkg-config"]);
    }

    #[test]
    fn missing_remote() {
        let err = Config::parse("exclude = [\".env\"]").unwrap_err();
        assert!(err.to_string().contains("remote"));
    }

    #[test]
    fn invalid_toml() {
        let err = Config::parse("not valid toml {{{}}}").unwrap_err();
        assert!(matches!(err, Error::ConfigParse { .. }));
    }

    #[test]
    fn defaults() {
        let config = Config::parse("remote = \"u@h\"").unwrap();
        assert_eq!(config.exclude, Vec::<String>::new());
        assert_eq!(config.apt_packages, Vec::<String>::new());
    }

    #[test]
    fn unknown_keys_ignored() {
        let input = r#"
remote = "user@host"
some_future_field = true
another = "value"
"#;
        let config = Config::parse(input).unwrap();
        assert_eq!(config.remote, "user@host");
    }

    // --- PartialConfig merge tests ---

    #[test]
    fn merge_override_wins() {
        let base = PartialConfig {
            remote: Some("base@host".into()),
            exclude: Some(vec!["base.txt".into()]),
            apt_packages: Some(vec!["base-pkg".into()]),
        };
        let over = PartialConfig {
            remote: Some("over@host".into()),
            exclude: Some(vec!["over.txt".into()]),
            apt_packages: None,
        };
        let merged = base.merge(over);
        assert_eq!(merged.remote.as_deref(), Some("over@host"));
        assert_eq!(merged.exclude, Some(vec!["over.txt".into()]));
        assert_eq!(merged.apt_packages, Some(vec!["base-pkg".into()]));
    }

    #[test]
    fn merge_base_used_when_override_absent() {
        let base = PartialConfig {
            remote: Some("base@host".into()),
            exclude: Some(vec![".env".into()]),
            apt_packages: None,
        };
        let over = PartialConfig::default();
        let merged = base.merge(over);
        assert_eq!(merged.remote.as_deref(), Some("base@host"));
        assert_eq!(merged.exclude, Some(vec![".env".into()]));
        assert!(merged.apt_packages.is_none());
    }

    #[test]
    fn resolve_missing_remote() {
        let partial = PartialConfig::default();
        let err = partial.resolve().unwrap_err();
        assert!(err.to_string().contains("remote"));
    }

    #[test]
    fn resolve_fills_defaults() {
        let partial = PartialConfig {
            remote: Some("u@h".into()),
            exclude: None,
            apt_packages: None,
        };
        let config = partial.resolve().unwrap();
        assert!(config.exclude.is_empty());
        assert!(config.apt_packages.is_empty());
    }

    // --- load_optional_config tests ---

    #[test]
    fn load_optional_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        assert!(load_optional_config(&path).unwrap().is_none());
    }

    #[test]
    fn load_optional_valid() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.toml");
        fs::write(&path, "remote = \"u@h\"").unwrap();
        let partial = load_optional_config(&path).unwrap().unwrap();
        assert_eq!(partial.remote.as_deref(), Some("u@h"));
    }

    #[test]
    fn load_optional_parse_error_includes_path() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("bad.toml");
        fs::write(&path, "{{invalid}}").unwrap();
        let err = load_optional_config(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("bad.toml"),
            "error should name the file: {msg}"
        );
    }

    // --- load_merged_config tests ---

    #[test]
    fn merged_project_overrides_user() {
        let home = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();

        let user_dir = home.path().join(".relocal");
        fs::create_dir(&user_dir).unwrap();
        fs::write(
            user_dir.join("config.toml"),
            "remote = \"user@default\"\nexclude = [\".env\"]",
        )
        .unwrap();

        fs::write(
            repo.path().join("relocal.toml"),
            "remote = \"user@project\"",
        )
        .unwrap();

        let config = load_merged_config(home.path(), repo.path()).unwrap();
        assert_eq!(config.remote, "user@project");
        // Project didn't specify exclude, so user's value is used
        assert_eq!(config.exclude, vec![".env"]);
    }

    #[test]
    fn merged_user_only() {
        let home = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();

        let user_dir = home.path().join(".relocal");
        fs::create_dir(&user_dir).unwrap();
        fs::write(user_dir.join("config.toml"), "remote = \"u@h\"").unwrap();

        let config = load_merged_config(home.path(), repo.path()).unwrap();
        assert_eq!(config.remote, "u@h");
    }

    #[test]
    fn merged_project_only() {
        let home = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();

        fs::write(repo.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();

        let config = load_merged_config(home.path(), repo.path()).unwrap();
        assert_eq!(config.remote, "u@h");
    }

    #[test]
    fn merged_neither_has_remote() {
        let home = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();

        let err = load_merged_config(home.path(), repo.path()).unwrap_err();
        assert!(err.to_string().contains("remote"));
    }
}
