//! Parsing and representation of `relocal.toml`, the per-repo configuration file.
//!
//! The config is intentionally minimal: only `remote` is required. Unknown keys
//! are silently ignored so that older binaries can read configs written for newer
//! versions (forward compatibility).

use crate::error::{Error, Result};
use serde::Deserialize;

/// Deserialized contents of `relocal.toml`.
///
/// All fields except `remote` have defaults, so a minimal config is just
/// `remote = "user@host"`.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub remote: String,

    #[serde(default)]
    pub exclude: Vec<String>,

    #[serde(default)]
    pub apt_packages: Vec<String>,
}

impl Config {
    pub fn parse(input: &str) -> Result<Self> {
        toml::from_str(input).map_err(|e| Error::ConfigParse {
            reason: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
