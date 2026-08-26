use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// OAuth token pair stored per account.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub id_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub scope: Option<String>,
}

/// A single profile (in-game character) belonging to an account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub uuid: String,
    pub username: String,
}

/// A stored account with its OAuth tokens and profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Local label for this account (user-assigned).
    pub label: String,
    /// OAuth tokens (stored separately, not in config TOML).
    #[serde(skip)]
    pub tokens: Tokens,
    /// OAuth client ID used for authentication.
    pub client_id: String,
    /// Known profiles fetched from launcher data.
    pub profiles: Vec<Profile>,
    /// Default profile UUID for this account (if set).
    pub default_profile: Option<String>,
}

/// A tracked game version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    pub channel: String,
    pub build: String,
    /// Directory where game assets are installed.
    pub install_path: PathBuf,
}

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub accounts: HashMap<String, Account>,
    /// Default account label to use when none specified.
    pub default_account: Option<String>,
    /// Default version string (channel@build).
    pub default_version: Option<String>,
    /// Tracked installed versions.
    pub versions: Vec<Version>,
    /// Whether to use native keychain for token storage.
    pub use_keychain: bool,
}


impl Config {
    pub fn resolve_account(&self, label: Option<&str>) -> Result<&Account> {
        match label {
            Some(l) => self
                .accounts
                .get(l)
                .with_context(|| format!("account '{l}' not found")),
            None => {
                let default = self
                    .default_account
                    .as_deref()
                    .with_context(|| "no default account set; specify --account")?;
                self.accounts
                    .get(default)
                    .with_context(|| format!("default account '{default}' not found"))
            }
        }
    }

    pub fn resolve_account_mut(&mut self, label: Option<&str>) -> Result<&mut Account> {
        match label {
            Some(l) => self
                .accounts
                .get_mut(l)
                .with_context(|| format!("account '{l}' not found")),
            None => {
                let default = self
                    .default_account
                    .clone()
                    .with_context(|| "no default account set; specify --account")?;
                self.accounts
                    .get_mut(&default)
                    .with_context(|| format!("default account '{default}' not found"))
            }
        }
    }

    /// Find (account, profile) by profile name/UUID across all accounts.
    /// Prefers the default account when the profile exists in multiple.
    pub fn find_account_for_profile(&self, profile: &str) -> Option<(&Account, &Profile)> {
        // Prefer default account.
        let default = self.default_account.as_deref()
            .and_then(|l| self.accounts.get(l));
        if let Some(acct) = default
            && let Some(p) = acct.profiles.iter().find(|p| p.uuid == profile || p.username == profile)
        {
            return Some((acct, p));
        }
        for acct in self.accounts.values() {
            if let Some(p) = acct.profiles.iter().find(|p| p.uuid == profile || p.username == profile) {
                return Some((acct, p));
            }
        }
        None
    }

    /// Find an account configured for dedicated server authentication
    /// (client_id == "hytale-server"). Prefers the default account if it is one.
    pub fn resolve_server_account(&self) -> Option<&Account> {
        if let Some(label) = &self.default_account
            && let Some(acct) = self.accounts.get(label)
            && acct.client_id == "hytale-server"
        {
            return Some(acct);
        }
        self.accounts.values().find(|a| a.client_id == "hytale-server")
    }

    pub fn resolve_profile<'a>(&self, account: &'a Account, profile: Option<&str>) -> Result<&'a Profile> {
        match profile {
            Some(p) => {
                for prof in &account.profiles {
                    if prof.uuid == p || prof.username == p {
                        return Ok(prof);
                    }
                }
                anyhow::bail!("profile '{p}' not found for account '{}'", account.label);
            }
            None => {
                let uuid = account
                    .default_profile
                    .as_deref()
                    .with_context(|| {
                        format!(
                            "no default profile for account '{}'; specify a profile",
                            account.label
                        )
                    })?;
                account
                    .profiles
                    .iter()
                    .find(|p| p.uuid == uuid)
                    .with_context(|| {
                        format!(
                            "default profile '{uuid}' not found for account '{}'",
                            account.label
                        )
                    })
            }
        }
    }
}

// ── Paths ────────────────────────────────────────────────────────────────

const APP_NAME: &str = env!("CARGO_PKG_NAME");

/// XDG config directory for hytale CLI.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join(APP_NAME)
}

/// XDG data directory for hytale — stores downloaded game assets.
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join(APP_NAME)
}

fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Tokens path for an account. Stored separately so they can be chmod 600'd.
fn tokens_path(label: &str) -> PathBuf {
    config_dir().join(format!("tokens-{label}.json"))
}


// ── Load / Save ──────────────────────────────────────────────────────────

/// Load the configuration from disk. Returns default if no config file exists.
pub fn load_config() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

/// Save configuration to disk.
pub fn save_config(config: &Config) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir).context("failed to create config directory")?;
    let text = toml::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(config_path(), text).context("failed to write config")?;
    Ok(())
}

// ── Token-file helpers (stored alongside config, mode 600) ───────────────

use std::os::unix::fs::PermissionsExt;

fn read_token_file(label: &str) -> Result<Option<Tokens>> {
    let path = tokens_path(label);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .context("failed to parse token file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("failed to read token file"),
    }
}

fn write_token_file(label: &str, tokens: &Tokens) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir).context("failed to create config directory")?;
    let path = tokens_path(label);
    let text = serde_json::to_string_pretty(tokens).context("failed to serialize tokens")?;
    fs::write(&path, &text).context("failed to write token file")?;
    // Restrict permissions to owner-only.
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&path, perms)?;
    Ok(())
}

fn remove_token_file(label: &str) -> Result<()> {
    let path = tokens_path(label);
    if path.exists() {
        fs::remove_file(&path).context("failed to remove token file")?;
    }
    Ok(())
}

// ── High-level config helpers ───────────────────────────────────────────

/// Load config, then load and embed the account tokens from their separate files.
/// This assembles the full runtime config.
pub fn load_full_config() -> Config {
    let mut config = load_config();

    for (label, account) in &mut config.accounts {
        if let Ok(Some(tokens)) = read_token_file(label) {
            account.tokens = tokens;
        }
    }

    config
}

/// Save config and write each account's tokens to a separate restricted-permissions file.
/// Save config and write each account's tokens to a separate restricted-permissions file.
pub fn save_full_config(config: &Config) -> Result<()> {
    // Write tokens to separate files.
    for (label, account) in &config.accounts {
        write_token_file(label, &account.tokens)?;
    }

    // Tokens are #[serde(skip)] on Account, so they're auto-omitted from TOML.
    save_config(config)
}

#[allow(dead_code)]
pub fn add_account(config: &mut Config, account: Account) -> Result<()> {
    config.accounts.insert(account.label.clone(), account);
    save_full_config(config)
}

/// Remove an account and its token file.
pub fn remove_account(config: &mut Config, label: &str) -> Result<()> {
    config.accounts.remove(label);
    let _ = remove_token_file(label);
    if config.default_account.as_deref() == Some(label) {
        config.default_account = None;
    }
    save_full_config(config)
}
