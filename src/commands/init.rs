//! `relocal init` — interactive creation of `relocal.toml`.
//!
//! The command prompts for configuration values and writes the file to the
//! current directory. It is the only command that does not require an existing
//! `relocal.toml`.

use std::path::Path;

use crate::error::Result;

/// Generates the TOML content for a `relocal.toml` file from collected inputs.
///
/// This is a pure function (no I/O) so it can be unit-tested independently
/// of the interactive prompts.
pub fn generate_toml(remote: &str, exclude: &[String], apt_packages: &[String]) -> String {
    let mut toml = format!("remote = \"{remote}\"\n");

    if !exclude.is_empty() {
        toml.push_str(&format!(
            "exclude = [{}]\n",
            exclude
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if !apt_packages.is_empty() {
        toml.push_str(&format!(
            "apt_packages = [{}]\n",
            apt_packages
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    toml
}

/// Runs the interactive `relocal init` command, prompting the user and writing
/// `relocal.toml` to `dir`.
pub fn run(dir: &Path) -> Result<()> {
    let toml_path = dir.join("relocal.toml");
    if toml_path.exists() {
        eprintln!("relocal.toml already exists in {}", dir.display());
        return Ok(());
    }

    let remote: String = dialoguer::Input::new()
        .with_prompt("Remote (user@host)")
        .interact_text()
        .map_err(std::io::Error::other)?;

    let exclude_input: String = dialoguer::Input::new()
        .with_prompt("Exclude patterns (comma-separated, or empty)")
        .default(String::new())
        .interact_text()
        .map_err(std::io::Error::other)?;

    let apt_input: String = dialoguer::Input::new()
        .with_prompt("APT packages (comma-separated, or empty)")
        .default(String::new())
        .interact_text()
        .map_err(std::io::Error::other)?;

    let exclude: Vec<String> = parse_comma_list(&exclude_input);
    let apt_packages: Vec<String> = parse_comma_list(&apt_input);

    let content = generate_toml(&remote, &exclude, &apt_packages);
    std::fs::write(&toml_path, &content)?;

    eprintln!("Created {}", toml_path.display());
    Ok(())
}

/// Splits a comma-separated string into a vec, trimming whitespace and
/// filtering out empty entries.
fn parse_comma_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_minimal() {
        let toml = generate_toml("user@host", &[], &[]);
        assert_eq!(toml, "remote = \"user@host\"\n");

        // Verify it parses back correctly
        let config = crate::config::Config::parse(&toml).unwrap();
        assert_eq!(config.remote, "user@host");
        assert!(config.exclude.is_empty());
        assert!(config.apt_packages.is_empty());
    }

    #[test]
    fn generate_with_exclude() {
        let toml = generate_toml("u@h", &[".env".to_string(), "secrets/".to_string()], &[]);
        assert!(toml.contains("exclude = [\".env\", \"secrets/\"]"));

        let config = crate::config::Config::parse(&toml).unwrap();
        assert_eq!(config.exclude, vec![".env", "secrets/"]);
    }

    #[test]
    fn generate_with_apt_packages() {
        let toml = generate_toml(
            "u@h",
            &[],
            &["libssl-dev".to_string(), "pkg-config".to_string()],
        );
        assert!(toml.contains("apt_packages = [\"libssl-dev\", \"pkg-config\"]"));

        let config = crate::config::Config::parse(&toml).unwrap();
        assert_eq!(config.apt_packages, vec!["libssl-dev", "pkg-config"]);
    }

    #[test]
    fn generate_full() {
        let toml = generate_toml(
            "user@host",
            &[".env".to_string()],
            &["build-essential".to_string()],
        );

        let config = crate::config::Config::parse(&toml).unwrap();
        assert_eq!(config.remote, "user@host");
        assert_eq!(config.exclude, vec![".env"]);
        assert_eq!(config.apt_packages, vec!["build-essential"]);
    }

    #[test]
    fn parse_comma_list_basic() {
        assert_eq!(
            parse_comma_list(".env, secrets/, .tmp"),
            vec![".env", "secrets/", ".tmp"]
        );
    }

    #[test]
    fn parse_comma_list_empty() {
        assert!(parse_comma_list("").is_empty());
        assert!(parse_comma_list("  ").is_empty());
    }

    #[test]
    fn parse_comma_list_trims() {
        assert_eq!(parse_comma_list("  foo ,  bar  "), vec!["foo", "bar"]);
    }
}
