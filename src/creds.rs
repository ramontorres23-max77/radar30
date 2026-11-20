//! Credential resolution: process env → `.env` files → macOS Keychain (lowest priority).
//!
//! Secrets are never printed. `status()` reports only presence and origin.
//! Keychain items use service name `radar30-<KEY>` for the current user, matching
//! the `last30days` convention. Writes go through `security -i` stdin so the
//! secret never appears in process argv.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::process;
use crate::setup;

/// Keys radar30 knows how to resolve, with the source lane they unlock.
pub const KNOWN_KEYS: &[(&str, &str)] = &[
    ("GITHUB_TOKEN", "github (higher rate limits)"),
    ("BLUESKY_HANDLE", "bluesky (authenticated AppView)"),
    ("BLUESKY_APP_PASSWORD", "bluesky (authenticated AppView)"),
    ("X_BEARER_TOKEN", "x (official X API recent search)"),
    (
        "SCRAPECREATORS_API_KEY",
        "tiktok, instagram, threads, linkedin, pinterest",
    ),
    ("PERPLEXITY_API_KEY", "perplexity (Sonar synthesis)"),
    ("BRAVE_API_KEY", "web (Brave Search)"),
];

/// Origin of a resolved credential (never the value itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Env,
    DotEnv,
    Keychain,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Env => "env",
            Origin::DotEnv => ".env",
            Origin::Keychain => "keychain",
        }
    }
}

/// `.env` search paths: project-local first, then user config.
pub fn dotenv_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(".env")];
    if let Ok(home) = setup::home() {
        paths.push(home.join(".config").join("radar30").join(".env"));
    }
    paths
}

/// Parse `KEY=VALUE` lines (comments and blank lines ignored, optional quotes).
pub fn parse_dotenv(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_start_matches("export ").to_string();
        let mut value = value.trim().to_string();
        if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            value = value[1..value.len() - 1].to_string();
        }
        if !key.is_empty() && !value.is_empty() {
            out.insert(key, value);
        }
    }
    out
}

/// Testable core: resolve `key` against an explicit env lookup, `.env` files,
/// and an optional keychain lookup.
pub fn resolve_with(
    key: &str,
    env_lookup: &dyn Fn(&str) -> Option<String>,
    dotenv_files: &[PathBuf],
    keychain_lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, Origin)> {
    if let Some(v) = env_lookup(key).filter(|s| !s.trim().is_empty()) {
        return Some((v, Origin::Env));
    }
    for path in dotenv_files {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Some(v) = parse_dotenv(&text).get(key).cloned()
        {
            return Some((v, Origin::DotEnv));
        }
    }
    keychain_lookup(key).map(|v| (v, Origin::Keychain))
}

/// Resolve a credential through the full chain (env → .env → Keychain).
pub fn resolve(key: &str) -> Option<String> {
    resolve_with(
        key,
        &|k| std::env::var(k).ok(),
        &dotenv_paths(),
        &keychain_read,
    )
    .map(|(v, _)| v)
}

/// Resolve with origin information (for `creds` status and preflight).
pub fn resolve_origin(key: &str) -> Option<Origin> {
    resolve_with(key, &|k| std::env::var(k).ok(), &dotenv_paths(), &|k| {
        keychain_read(k)
    })
    .map(|(_, o)| o)
}

fn keychain_service(key: &str) -> String {
    format!("radar30-{key}")
}

fn current_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "user".to_string())
}

/// Read a Keychain item via `security find-generic-password` (macOS only).
pub fn keychain_read(key: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let user = current_user();
    let service = keychain_service(key);
    let out = process::run_capture(
        "security",
        &["find-generic-password", "-a", &user, "-s", &service, "-w"],
        Duration::from_secs(5),
    )
    .ok()?;
    let trimmed = out.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Store a secret in the Keychain through `security -i` stdin (never argv).
pub fn keychain_set(key: &str, secret: &str) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("Keychain storage is only available on macOS; use a .env file instead");
    }
    if secret.trim().is_empty() {
        bail!("refusing to store an empty secret");
    }
    let user = current_user();
    let service = keychain_service(key);
    let command = format!(
        "add-generic-password -a {user} -s {service} -w {} -U\n",
        shell_quote_security(secret)
    );
    process::run_with_stdin("security", &["-i"], &command, Duration::from_secs(10))
        .map_err(|e| anyhow::anyhow!("keychain write failed: {e}"))?;
    Ok(())
}

/// Delete a Keychain item. Returns `true` when an item was removed.
pub fn keychain_delete(key: &str) -> Result<bool> {
    if !cfg!(target_os = "macos") {
        return Ok(false);
    }
    let user = current_user();
    let service = keychain_service(key);
    match process::run_capture(
        "security",
        &["delete-generic-password", "-a", &user, "-s", &service],
        Duration::from_secs(5),
    ) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Quote a value for the `security -i` command language (double quotes, escape backslash/quote).
fn shell_quote_security(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// One key's presence report for `radar30 creds`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct KeyStatus {
    pub key: &'static str,
    pub unlocks: &'static str,
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<&'static str>,
}

/// Presence-only status of every known key (values are never included).
pub fn status() -> Vec<KeyStatus> {
    KNOWN_KEYS
        .iter()
        .map(|(key, unlocks)| {
            let origin = resolve_origin(key);
            KeyStatus {
                key,
                unlocks,
                present: origin.is_some(),
                origin: origin.map(Origin::label),
            }
        })
        .collect()
}

/// Whether a `.env` file exists at any search path.
pub fn dotenv_present() -> Option<PathBuf> {
    dotenv_paths().into_iter().find(|p| Path::new(p).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_parsing_handles_quotes_comments_and_export() {
        let parsed = parse_dotenv(
            "# comment\nexport X_BEARER_TOKEN=\"abc123\"\nBRAVE_API_KEY='xyz'\nEMPTY=\n",
        );
        assert_eq!(parsed.get("X_BEARER_TOKEN").unwrap(), "abc123");
        assert_eq!(parsed.get("BRAVE_API_KEY").unwrap(), "xyz");
        assert!(!parsed.contains_key("EMPTY"));
    }

    #[test]
    fn resolution_precedence_env_over_dotenv_over_keychain() {
        let tmp = tempfile::tempdir().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, "MY_KEY=from-dotenv\n").unwrap();

        let env = |k: &str| (k == "MY_KEY").then(|| "from-env".to_string());
        let kc = |_: &str| Some("from-keychain".to_string());

        let (v, o) = resolve_with("MY_KEY", &env, std::slice::from_ref(&env_file), &kc).unwrap();
        assert_eq!((v.as_str(), o), ("from-env", Origin::Env));

        let no_env = |_: &str| None;
        let (v, o) = resolve_with("MY_KEY", &no_env, std::slice::from_ref(&env_file), &kc).unwrap();
        assert_eq!((v.as_str(), o), ("from-dotenv", Origin::DotEnv));

        let (v, o) = resolve_with("MY_KEY", &no_env, &[], &kc).unwrap();
        assert_eq!((v.as_str(), o), ("from-keychain", Origin::Keychain));

        assert!(resolve_with("MY_KEY", &no_env, &[], &|_| None).is_none());
    }
}
