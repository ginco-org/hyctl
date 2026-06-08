mod cli;
mod config;
mod auth;
mod download;
mod session;
mod launch;
mod wharf;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command, AuthCommand, AccountCommand, ProfileCommand, VersionCommand, ServerCommand};
use config::Account;
use tracing_subscriber::EnvFilter;
pub(crate) const BIN_NAME: &str = env!("CARGO_PKG_NAME");

#[tokio::main]
async fn main() {
    // Initialize tracing with env filter.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive("hytctl=info".parse().unwrap())
                .from_env_lossy(),
        )
        .init();

    let cli = Cli::parse();

    let result = run(cli).await;

    if let Err(e) = result {
        // Print the chain of errors.
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Auth { sub } => handle_auth(sub).await,
        Command::Account { sub } => handle_account(sub),
        Command::Profile { sub } => handle_profile(sub).await,
        Command::Version { sub } => handle_version(sub).await,
        Command::Install { version, output } => handle_install(version, output).await,
        Command::Run { profile, account, version, detach, extra_args } => handle_run(profile, account, version, detach, &extra_args).await,
        Command::Server { sub } => handle_server(sub).await,
    }
}

// ── Auth ────────────────────────────────────────────────────────────────

async fn handle_auth(cmd: AuthCommand) -> Result<()> {
    match cmd {
        AuthCommand::Login { label } => {
            let tokens = auth::launcher_login().await?;

            // Resolve label once: prefer --label, then email from id_token, then sub.
            let resolve_label = |label_opt: Option<String>| -> Result<String> {
                if let Some(l) = label_opt {
                    Ok(l)
                } else {
                    let (sub, email) = tokens.id_token.as_deref()
                        .and_then(auth::decode_id_token_claims)
                        .context("id_token missing or unparseable — use --label")?;
                    Ok(email.unwrap_or(sub))
                }
            };
            let account_label = resolve_label(label)?;

            // Fetch profiles to determine identity and populate account.
            println!("Authenticated. Fetching profiles...");
            let (profiles, default_profile) =
                match auth::fetch_launcher_data(
                    &tokens.access_token,
                    tokens.id_token.as_deref(),
                ).await {
                    Ok(profiles) => {
                        let default_profile = profiles.first().map(|p| p.uuid.clone());
                        println!("Found {} profile(s):", profiles.len());
                        for p in &profiles {
                            println!("  {} ({})", p.username, p.uuid);
                        }
                        (profiles, default_profile)
                    }
                    Err(e) => {
                        eprintln!("warning: failed to fetch profiles: {e:#}");
                        eprintln!("Run `{BIN_NAME} profile refresh {account_label}` later.");
                        (Vec::new(), None)
                    }
                };

            let account = Account {
                label: account_label.clone(),
                tokens,
                client_id: "hytale-launcher".to_string(),
                profiles,
                default_profile,
            };

            let mut config = config::load_full_config();
            config.accounts.insert(account_label.clone(), account);
            if config.default_account.is_none() {
                config.default_account = Some(account_label.clone());
            }
            config::save_full_config(&config)?;

            println!("Account '{}' configured.", account_label);
            Ok(())
        }

        AuthCommand::Refresh { account } => {
            let config = config::load_full_config();
            let acct = config
                .accounts
                .get(&account)
                .with_context(|| format!("account '{account}' not found"))?;
            let refresh_token = acct
                .tokens
                .refresh_token
                .as_deref()
                .context("no refresh token available for this account")?;

            let new_tokens = auth::refresh_token(&acct.client_id, refresh_token).await?;

            let mut config = config::load_full_config();
            if let Some(acct) = config.accounts.get_mut(&account) {
                acct.tokens = new_tokens;
                config::save_full_config(&config)?;
                println!("Token refreshed for account '{account}'.");
            }
            Ok(())
        }

        AuthCommand::Logout { account } => {
            config::remove_account(&mut config::load_full_config(), &account)?;
            println!("Account '{account}' removed.");
            Ok(())
        }

        AuthCommand::Status => {
            let config = config::load_full_config();
            if config.accounts.is_empty() {
                println!("Run `{BIN_NAME} auth login` to authenticate.");
                return Ok(());
            }

            println!("Accounts:");
            for (label, acct) in &config.accounts {
                let default_mark = if config.default_account.as_deref() == Some(label) {
                    " (default)"
                } else {
                    ""
                };
                let expires = acct
                    .tokens
                    .expires_at
                    .map(|ts| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        let remaining = ts - now;
                        if remaining > 0 {
                            format!(" (expires in {remaining}s)")
                        } else {
                            " (expired)".to_string()
                        }
                    })
                    .unwrap_or_default();
                println!(
                    "  {label}{default_mark}: client_id={}, {} profile(s), tokens{expires}",
                    acct.client_id,
                    acct.profiles.len(),
                );
            }
            Ok(())
        }

    }
}


// ── Account ─────────────────────────────────────────────────────────────

fn handle_account(cmd: AccountCommand) -> Result<()> {
    match cmd {
        AccountCommand::List => {
            let config = config::load_config();
            if config.accounts.is_empty() {
                println!("No accounts configured.");
                return Ok(());
            }
            println!("Accounts:");
            for (label, acct) in &config.accounts {
                let default_mark = if config.default_account.as_deref() == Some(label) {
                    " (default)"
                } else {
                    ""
                };
                println!("  {label}{default_mark}: {} profile(s)", acct.profiles.len());
            }
            Ok(())
        }

        AccountCommand::Default { account } => {
            let mut config = config::load_config();
            if !config.accounts.contains_key(&account) {
                anyhow::bail!("account '{account}' not found");
            }
            config.default_account = Some(account.clone());
            config::save_config(&config)?;
            println!("Default account set to '{account}'.");
            Ok(())
        }

        AccountCommand::Remove { account } => {
            config::remove_account(&mut config::load_full_config(), &account)?;
            println!("Account '{account}' removed.");
            Ok(())
        }
    }
}

// ── Profile ─────────────────────────────────────────────────────────────

async fn handle_profile(cmd: ProfileCommand) -> Result<()> {
    match cmd {
        ProfileCommand::List { account } => {
            let config = config::load_full_config();
            if let Some(label) = account {
                let acct = config.resolve_account(Some(&label))?;
                println!("Profiles for account '{label}':");
                for p in &acct.profiles {
                    let default_mark = acct.default_profile.as_deref() == Some(&p.uuid);
                    if default_mark {
                        println!("  {} ({}) ← default", p.username, p.uuid);
                    } else {
                        println!("  {} ({})", p.username, p.uuid);
                    }
                }
            } else {
                for (label, acct) in &config.accounts {
                    println!("Account '{label}':");
                    for p in &acct.profiles {
                        let default_mark = acct.default_profile.as_deref() == Some(&p.uuid);
                        if default_mark {
                            println!("  {} ({}) ← default", p.username, p.uuid);
                        } else {
                            println!("  {} ({})", p.username, p.uuid);
                        }
                    }
                }
            }
            Ok(())
        }

        ProfileCommand::Default { profile, account } => {
            let mut config = config::load_full_config();
            let (uuid, label) = {
                let acct = config.resolve_account_mut(account.as_deref())?;
                let found = acct.profiles.iter().any(|p| p.uuid == profile || p.username == profile);
                if !found {
                    anyhow::bail!("profile '{profile}' not found for account '{}'", acct.label);
                }
                let uuid = acct
                    .profiles
                    .iter()
                    .find(|p| p.uuid == profile || p.username == profile)
                    .map(|p| p.uuid.clone())
                    .unwrap();
                acct.default_profile = Some(uuid.clone());
                (uuid, acct.label.clone())
            };
            config::save_full_config(&config)?;
            println!("Default profile for account '{label}' set to '{uuid}'.");
            Ok(())
        }

        ProfileCommand::Refresh { account } => {
            // First scope: load config, fetch launcher data, then release borrows.
            let (fetch_result, label) = {
                let config = config::load_full_config();
                let acct = config.resolve_account(account.as_deref())?;
                let label = acct.label.clone();
                let fetch_result =
                    auth::fetch_launcher_data(&acct.tokens.access_token, acct.tokens.id_token.as_deref()).await;
                (fetch_result, label)
            }; // config and acct dropped here

            match fetch_result {
                Ok(profiles) => {
                    let mut config = config::load_full_config();
                    if let Some(acct) = config.accounts.get_mut(&label) {
                        acct.profiles = profiles.clone();
                        println!("Profiles refreshed for account '{}':", label);
                        for p in &profiles {
                            println!("  {} ({})", p.uuid, p.username);
                        }
                    }
                    config::save_full_config(&config)?;
                }
                Err(_e) => {
                    eprintln!("Token may be expired. Try `{BIN_NAME} auth refresh {}`.", label);
                }
            }
            Ok(())
        }
    }
}

// ── Version helpers ──────────────────────────────────────────────────────

fn is_channel(s: &str) -> bool {
    matches!(s, "release" | "pre-release")
}

fn infer_channel(version: &str) -> &'static str {
    if version.contains("-pre") { "pre-release" } else { "release" }
}

/// Resolve a version spec to (channel, build_string, numeric_build_number?).
/// Channel names fetch the latest build for that channel (includes build number).
async fn resolve_build(access_token: &str, spec: &str) -> Result<(String, String, Option<u32>)> {
    if is_channel(spec) {
        let (build, num, _) = download::fetch_build_info(access_token, spec).await?;
        // Strip "{channel}@" prefix from the API's build version for consistency
        // with direct version installs (e.g. "release@0.5.4" → "0.5.4").
        let clean = build.strip_prefix(&format!("{spec}@")).unwrap_or(&build).to_string();
        Ok((spec.to_string(), clean, Some(num)))
    } else {
        let channel = infer_channel(spec);
        // Strip "{channel}@" prefix if present (e.g. "release@0.5.4" → "0.5.4").
        let clean = spec.strip_prefix(&format!("{channel}@")).unwrap_or(spec).to_string();
        // Try to get a build number — only works if spec is the latest for this channel.
        let num = match download::fetch_build_info(access_token, channel).await {
            Ok((latest, n, _)) if latest.strip_prefix(&format!("{channel}@")).unwrap_or(&latest) == clean => Some(n),
            _ => None,
        };
        Ok((channel.to_string(), clean, num))
    }
}

// ── Version ─────────────────────────────────────────────────────────────

async fn handle_version(cmd: VersionCommand) -> Result<()> {
    match cmd {
        VersionCommand::List { channel } => {
            let config = config::load_full_config();
            // To list versions, we need any authenticated account.
            let acct = config.resolve_account(None)?;

            println!("Fetching version info for '{channel}'...");
            match download::fetch_version_url(&acct.tokens.access_token, &channel).await {
                Ok((url, manifest)) => {
                    println!("Channel: {channel}");
                    println!("Manifest URL: {url}");
                    println!("Version: {}", manifest.version);
                    println!("Download URL: {}", manifest.download_url);
                    if let Some(sha256) = &manifest.sha256 {
                        println!("SHA256: {sha256}");
                    }
                }
                Err(e) => {
                    eprintln!("Failed to fetch version info: {e:#}");
                }
            }
            Ok(())
        }

        VersionCommand::Default { version } => {
            let mut config = config::load_config();
            if !config.versions.iter().any(|v| v.build == version) {
                anyhow::bail!("version '{version}' is not installed");
            }
            config.default_version = Some(version.clone());
            config::save_config(&config)?;
            println!("Default version set to '{version}'.");
            Ok(())
        }

        VersionCommand::Installed => {
            let config = config::load_config();
            if config.versions.is_empty() {
                println!("Run `{BIN_NAME} install` to download a version.");
                return Ok(());
            }
            println!("Installed versions:");
            for v in &config.versions {
                let is_default = config.default_version.as_deref() == Some(&v.build);
                let mark = if is_default { " ← default" } else { "" };
                println!("  {} ({}) at {}{mark}", v.build, v.channel, v.install_path.display());
            }
            Ok(())
        }

        VersionCommand::Remove { version } => {
            let mut config = config::load_config();
            config.versions.retain(|v| v.build != version);
            if config.default_version.as_deref() == Some(&version) {
                config.default_version = None;
            }
            config::save_config(&config)?;
            println!("Version '{version}' removed from tracking. Files still exist on disk.");
            Ok(())
        }
    }
}

// ── Install ─────────────────────────────────────────────────────────────

async fn handle_install(version: Option<String>, output: Option<String>) -> Result<()> {
    let config = config::load_full_config();
    let acct = config.resolve_account(None)?;
    let access_token = ensure_valid_token(&acct.label).await?;

    let spec = version.as_deref().unwrap_or("release");
    let (channel, build, build_num) = resolve_build(&access_token, spec).await?;

    let install_dir = match &output {
        Some(dir) => std::path::PathBuf::from(dir),
        None => config::data_dir().join("versions").join(&channel).join(&build),
    };

    println!("Installing {build} to {}...", install_dir.display());
    let num = build_num
        .with_context(|| format!(
            "version '{spec}' is not the latest and cannot be installed.\n\
             Use a channel name (release, pre-release) to get the latest."
        ))?;
    download::install_client(&access_token, &channel, num, &install_dir).await?;

    let mut config = config::load_config();
    config.versions.retain(|v| v.build != build);
    config.versions.push(config::Version {
        channel,
        build: build.clone(),
        install_path: install_dir,
    });
    if config.default_version.is_none() {
        config.default_version = Some(build);
    }
    config::save_config(&config)?;
    println!("Done. Run `{BIN_NAME} run` to launch.");
    Ok(())
}

// ── Run ─────────────────────────────────────────────────────────────────

async fn handle_run(
    profile: Option<String>,
    account: Option<String>,
    version: Option<String>,
    detach: bool,
    extra_args: &[String],
) -> Result<()> {
    let config = config::load_full_config();

    // When a profile is named but no account is given, search all accounts.
    let (acct, prof) = if account.is_none() {
        if let Some(p) = profile.as_deref() {
            config
                .find_account_for_profile(p)
                .with_context(|| format!("profile '{p}' not found in any account"))?
        } else {
            let acct = config.resolve_account(None)?;
            let prof = config.resolve_profile(acct, None)?;
            (acct, prof)
        }
    } else {
        let acct = config.resolve_account(account.as_deref())?;
        let prof = config.resolve_profile(acct, profile.as_deref())?;
        (acct, prof)
    };

    let spec = version.as_deref()
        .or(config.default_version.as_deref())
        .context("no version specified and no default version set")?;

    // Try local cache first — avoids unnecessary API call.
    if let Some(v) = config.versions.iter().find(|v|
        v.build == spec && v.install_path.join("Client").join("HytaleClient").exists()
    ) {
        let access_token = ensure_valid_token(&acct.label).await?;
        return launch::launch_game(
            &access_token, acct, prof, &v.install_path, &v.build, extra_args, detach,
        ).await;
    }

    // Not found locally — resolve remotely and install if needed.
    let access_token = ensure_valid_token(&acct.label).await?;
    let (channel, build, build_num) = resolve_build(&access_token, spec).await?;

    let install_dir = match config.versions.iter().find(|v| {
        v.build == build && v.install_path.join("Client").join("HytaleClient").exists()
    }) {
        Some(v) => v.install_path.clone(),
        None => {
            let dir = config::data_dir().join("versions").join(&channel).join(&build);
            println!("Version '{build}' not installed, installing...");
            let num = build_num
                .with_context(|| format!(
                    "version '{spec}' is not the latest and cannot be installed.\n\
                     Use a channel name (release, pre-release) to get the latest, or\n\
                     reinstall the version that was previously downloaded."
                ))?;
            download::install_client(&access_token, &channel, num, &dir).await?;
            let mut cfg = config::load_config();
            cfg.versions.retain(|v| v.build != build);
            cfg.versions.push(config::Version {
                channel: channel.clone(),
                build: build.clone(),
                install_path: dir.clone(),
            });
            if cfg.default_version.is_none() {
                cfg.default_version = Some(build.clone());
            }
            config::save_config(&cfg)?;
            dir
        }
    };

    launch::launch_game(
        &access_token,
        acct,
        prof,
        &install_dir,
        &build,
        extra_args,
        detach,
    )
    .await
}

// ── Server ─────────────────────────────────────────────────────────────────

async fn handle_server(cmd: ServerCommand) -> Result<()> {
    match cmd {
        ServerCommand::Run { version, detach, extra_args } => {
            let config = config::load_full_config();
            let spec = version.as_deref()
                .or(config.default_version.as_deref())
                .context("no version specified and no default version set")?;
            let access_token = ensure_valid_token(
                config.default_account.as_deref()
                    .context("no default account configured")?
            ).await?;

            let (channel, build, build_num) = resolve_build(&access_token, spec).await?;
            let build_num = build_num.with_context(|| format!(
                "version '{spec}' is not the latest and cannot be installed.\n\
                 Use a channel name (release, pre-release) to get the latest."
            ))?;

            // Find or auto-install using the same patch system as client install.
            let install_dir = match config.versions.iter().find(|v| v.build == build) {
                Some(v) => v.install_path.clone(),
                None => {
                    let dir = config::data_dir().join("versions").join(&channel).join(&build);
                    println!("Version '{build}' not installed, installing...");
                    download::install_client(&access_token, &channel, build_num, &dir).await?;
                    let mut cfg = config::load_config();
                    cfg.versions.retain(|v| v.build != build);
                    cfg.versions.push(config::Version {
                        channel: channel.clone(),
                        build: build.clone(),
                        install_path: dir.clone(),
                    });
                    if cfg.default_version.is_none() {
                        cfg.default_version = Some(build.clone());
                    }
                    config::save_config(&cfg)?;
                    dir
                }
            };

            let jre = launch::ensure_jre(&install_dir).await?;
            launch::launch_server(&install_dir, &jre, &extra_args, detach)
        }
    }
}

// ── Token Refresh Helper ────────────────────────────────────────────────

/// Check if the token is near expiry and refresh if necessary.
async fn ensure_valid_token(account_label: &str) -> Result<String> {
    let mut config = config::load_full_config();

    // Borrow config mutably only inside this block; drop acct before save.
    let (access_token, needs_refresh) = {
        let acct = config
            .accounts
            .get_mut(account_label)
            .with_context(|| format!("account '{account_label}' not found"))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let needs_refresh = acct
            .tokens
            .expires_at
            .map(|exp| exp - now < 60)
            .unwrap_or(false);

        if needs_refresh {
            let rt = acct
                .tokens
                .refresh_token
                .clone()
                .context("token expired and no refresh token available")?;
            let client_id = acct.client_id.clone();

            println!("Access token expiring, refreshing...");
            let new_tokens = auth::refresh_token(&client_id, &rt).await?;
            acct.tokens = new_tokens;
            (acct.tokens.access_token.clone(), true)
        } else {
            (acct.tokens.access_token.clone(), false)
        }
    };

    if needs_refresh {
        config::save_full_config(&config)?;
    }

    Ok(access_token)
}