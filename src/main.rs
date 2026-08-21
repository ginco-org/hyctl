mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{AssetCommand, AuthCommand, Cli, Command};
use hyctl::{auth, config, download, launch, BIN_NAME};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(format!("{BIN_NAME}=info").parse().unwrap())
                .from_env_lossy(),
        )
        .init();

    let cli = Cli::parse();

    if cli.no_color {
        // SAFETY: called once before any threads are spawned.
        unsafe { std::env::set_var("NO_COLOR", "1") };
    }

    if let Err(e) = run(cli).await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Launch { profile, version, background, server, world, extra_args } => {
            handle_launch(profile, version, background, server, world, &extra_args).await
        }
        Command::Serve { profile, dir, version, background, assets, extra_args } => {
            handle_serve(profile, dir, version, background, assets, &extra_args).await
        }
        Command::Auth { sub } => handle_auth(sub).await,
        Command::Asset { sub } => handle_asset(sub).await,
    }
}

// ── Launch ────────────────────────────────────────────────────────────────

async fn handle_launch(
    profile: Option<String>,
    version: Option<String>,
    background: bool,
    server: Option<String>,
    world: Option<String>,
    extra_args: &[String],

) -> Result<()> {
    let config = config::load_full_config();

    let (acct, prof) = if let Some(p) = profile.as_deref() {
        config
            .find_account_for_profile(p)
            .with_context(|| format!("profile '{p}' not found in any account"))?
    } else {
        let acct = config.resolve_account(None)?;
        let prof = config.resolve_profile(acct, None)?;
        (acct, prof)
    };

    let spec = version
        .as_deref()
        .or(config.default_version.as_deref())
        .context("no version specified and no default version set")?;

    // Try local cache first — avoids unnecessary API call.
    if let Some(v) = config.versions.iter().find(|v| {
        v.build == spec && v.install_path.join("Client").join("HytaleClient").exists()
    }) {
        let access_token = ensure_valid_token(&acct.label).await?;
        return launch::launch_game(
            &access_token,
            acct,
            prof,
            &v.install_path,
            &v.build,
            server.as_deref(),
            world.as_deref(),
            extra_args,
            background,
        )
        .await;
    }

    let access_token = ensure_valid_token(&acct.label).await?;
    let (channel, build, build_num) = resolve_build(&access_token, spec).await?;

    let install_dir = match config.versions.iter().find(|v| {
        v.build == build && v.install_path.join("Client").join("HytaleClient").exists()
    }) {
        Some(v) => v.install_path.clone(),
        None => {
            let num = build_num.with_context(|| {
                format!(
                    "version '{spec}' is not the latest and cannot be installed.\n\
                     Use a channel name (release, pre-release) to get the latest, or\n\
                     reinstall the version that was previously downloaded."
                )
            })?;
            let dir = config::data_dir().join("versions").join(&channel).join(&build);
            println!("Version '{build}' not installed, installing...");
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

    launch::launch_game(&access_token, acct, prof, &install_dir, &build, server.as_deref(), world.as_deref(), extra_args, background).await
}

// ── Serve ─────────────────────────────────────────────────────────────────

async fn handle_serve(
    profile: Option<String>,
    dir: String,
    version: Option<String>,
    background: bool,
    assets: Option<String>,
    extra_args: &[String],
) -> Result<()> {
    let assets = match assets {
        Some(p) => {
            let path = std::path::PathBuf::from(&p);
            anyhow::ensure!(path.exists(), "assets path does not exist: {p}");
            Some(
                path.canonicalize()
                    .with_context(|| format!("failed to resolve assets path: {p}"))?,
            )
        }
        None => None,
    };

    let config = config::load_full_config();

    let acct = if let Some(p) = profile.as_deref() {
        config
            .find_account_for_profile(p)
            .map(|(a, _)| a)
            .with_context(|| format!("profile '{p}' not found in any account"))?
    } else {
        config.resolve_account(None)?
    };

    let spec = version
        .as_deref()
        .or(config.default_version.as_deref())
        .context("no version specified and no default version set")?;

    let access_token = ensure_valid_token(&acct.label).await?;
    let (channel, build, build_num) = resolve_build(&access_token, spec).await?;

    let install_dir = match config.versions.iter().find(|v| v.build == build) {
        Some(v) => v.install_path.clone(),
        None => {
            let num = build_num.with_context(|| {
                format!(
                    "version '{spec}' is not the latest and cannot be installed.\n\
                     Use a channel name (release, pre-release) to get the latest."
                )
            })?;
            let dir_path = config::data_dir().join("versions").join(&channel).join(&build);
            println!("Version '{build}' not installed, installing...");
            download::install_client(&access_token, &channel, num, &dir_path).await?;
            let mut cfg = config::load_config();
            cfg.versions.retain(|v| v.build != build);
            cfg.versions.push(config::Version {
                channel: channel.clone(),
                build: build.clone(),
                install_path: dir_path.clone(),
            });
            if cfg.default_version.is_none() {
                cfg.default_version = Some(build.clone());
            }
            config::save_config(&cfg)?;
            dir_path
        }
    };

    let data_dir = std::path::PathBuf::from(&dir);
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create server directory: {dir}"))?;

    let jre = launch::ensure_jre(&install_dir).await?;
    launch::launch_server(&install_dir, &jre, &data_dir, assets.as_deref(), extra_args, background)
}

// ── Auth ──────────────────────────────────────────────────────────────────

async fn handle_auth(cmd: AuthCommand) -> Result<()> {
    match cmd {
        AuthCommand::List => {
            let config = config::load_full_config();
            if config.accounts.is_empty() {
                println!("No accounts. Run `{BIN_NAME} auth add` to add one.");
                return Ok(());
            }
            for (label, acct) in &config.accounts {
                let default_mark = if config.default_account.as_deref() == Some(label) {
                    " (default)"
                } else {
                    ""
                };
                println!("{label}{default_mark}");
                for p in &acct.profiles {
                    let mark = if acct.default_profile.as_deref() == Some(&p.uuid) {
                        " (default)"
                    } else {
                        ""
                    };
                    println!("  {} ({}){mark}", p.username, p.uuid);
                }
            }
            Ok(())
        }

        AuthCommand::Add => {
            let tokens = auth::launcher_login().await?;

            let account_label = tokens
                .id_token
                .as_deref()
                .and_then(auth::decode_id_token_claims)
                .map(|(sub, email)| email.unwrap_or(sub))
                .context("id_token missing or unparseable; authentication failed")?;

            println!("Authenticated. Fetching profiles...");
            let (profiles, default_profile) =
                match auth::fetch_launcher_data(&tokens.access_token, tokens.id_token.as_deref())
                    .await
                {
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
                        eprintln!("Re-run `{BIN_NAME} auth add` to retry.");
                        (Vec::new(), None)
                    }
                };

            let account = config::Account {
                label: account_label.clone(),
                tokens,
                client_id: "hytale-launcher".to_string(),
                profiles,
                default_profile,
            };

            let mut cfg = config::load_full_config();
            cfg.accounts.insert(account_label.clone(), account);
            if cfg.default_account.is_none() {
                cfg.default_account = Some(account_label.clone());
            }
            config::save_full_config(&cfg)?;

            println!("Account '{account_label}' added.");
            Ok(())
        }

        AuthCommand::Remove { account } => {
            let mut cfg = config::load_full_config();
            if !cfg.accounts.contains_key(&account) {
                anyhow::bail!("account '{account}' not found");
            }
            config::remove_account(&mut cfg, &account)?;
            println!("Account '{account}' removed.");
            Ok(())
        }

        AuthCommand::Default { account } => {
            let mut cfg = config::load_config();
            if !cfg.accounts.contains_key(&account) {
                anyhow::bail!("account '{account}' not found");
            }
            cfg.default_account = Some(account.clone());
            config::save_config(&cfg)?;
            println!("Default account set to '{account}'.");
            Ok(())
        }
    }
}

// ── Asset ─────────────────────────────────────────────────────────────────

async fn handle_asset(cmd: AssetCommand) -> Result<()> {
    match cmd {
        AssetCommand::List => {
            let cfg = config::load_config();
            if cfg.versions.is_empty() {
                println!("No versions installed. Run `{BIN_NAME} asset install` to download one.");
                return Ok(());
            }
            println!("Installed versions:");
            for v in &cfg.versions {
                let mark = if cfg.default_version.as_deref() == Some(&v.build) {
                    " (default)"
                } else {
                    ""
                };
                println!(
                    "  {} ({}) at {}{}",
                    v.build,
                    v.channel,
                    v.install_path.display(),
                    mark
                );
            }
            Ok(())
        }

        AssetCommand::Install { version } => {
            let config = config::load_full_config();
            let acct = config.resolve_account(None)?;
            let access_token = ensure_valid_token(&acct.label).await?;

            let spec = version.as_deref().unwrap_or("release");
            let (channel, build, build_num) = resolve_build(&access_token, spec).await?;

            let install_dir =
                config::data_dir().join("versions").join(&channel).join(&build);

            println!("Installing {build} to {}...", install_dir.display());
            let num = build_num.with_context(|| {
                format!(
                    "version '{spec}' is not the latest and cannot be installed.\n\
                     Use a channel name (release, pre-release) to get the latest."
                )
            })?;
            download::install_client(&access_token, &channel, num, &install_dir).await?;

            let mut cfg = config::load_config();
            cfg.versions.retain(|v| v.build != build);
            cfg.versions.push(config::Version {
                channel,
                build: build.clone(),
                install_path: install_dir,
            });
            if cfg.default_version.is_none() {
                cfg.default_version = Some(build);
            }
            config::save_config(&cfg)?;
            println!("Done. Run `{BIN_NAME} launch` to play.");
            Ok(())
        }

        AssetCommand::Remove { version } => {
            let mut cfg = config::load_config();
            if !cfg.versions.iter().any(|v| v.build == version) {
                anyhow::bail!("version '{version}' is not installed");
            }
            cfg.versions.retain(|v| v.build != version);
            if cfg.default_version.as_deref() == Some(&version) {
                cfg.default_version = None;
            }
            config::save_config(&cfg)?;
            println!("Version '{version}' removed from tracking. Files still exist on disk.");
            Ok(())
        }

        AssetCommand::Verify { version } => {
            let cfg = config::load_config();
            let v = cfg
                .versions
                .iter()
                .find(|v| v.build == version)
                .with_context(|| format!("version '{version}' is not installed"))?;

            println!("Verifying {}...", v.build);

            if !v.install_path.exists() {
                anyhow::bail!("install path missing: {}", v.install_path.display());
            }

            let client_bin = v.install_path.join("Client").join("HytaleClient");
            let server_jar = v.install_path.join("Server").join("HytaleServer.jar");

            let client_ok = client_bin.exists();
            let server_ok = server_jar.exists();

            println!(
                "  client binary: {}",
                if client_ok { "OK" } else { "MISSING" }
            );
            println!(
                "  server jar:    {}",
                if server_ok { "OK" } else { "MISSING" }
            );

            if !client_ok && !server_ok {
                anyhow::bail!("version '{version}' is missing both client and server files");
            }

            println!("OK");
            Ok(())
        }

        AssetCommand::Prune => {
            let mut cfg = config::load_config();
            if cfg.versions.len() <= 1 {
                println!("Nothing to prune.");
                return Ok(());
            }

            // Keep the most recently installed version; remove the rest.
            let keep = cfg.versions.last().unwrap().clone();
            let to_remove: Vec<_> = cfg.versions[..cfg.versions.len() - 1].to_vec();

            for v in &to_remove {
                print!("Removing {} ({})... ", v.build, v.channel);
                if v.install_path.exists() {
                    std::fs::remove_dir_all(&v.install_path).with_context(|| {
                        format!("failed to remove {}", v.install_path.display())
                    })?;
                    println!("done.");
                } else {
                    println!("path not found, skipping.");
                }
            }

            if cfg
                .default_version
                .as_ref()
                .map(|d| to_remove.iter().any(|v| &v.build == d))
                .unwrap_or(false)
            {
                cfg.default_version = Some(keep.build.clone());
            }
            cfg.versions = vec![keep.clone()];
            config::save_config(&cfg)?;
            println!("Kept version '{}'.", keep.build);
            Ok(())
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn is_channel(s: &str) -> bool {
    matches!(s, "release" | "pre-release")
}

fn infer_channel(version: &str) -> &'static str {
    if version.contains("-pre") {
        "pre-release"
    } else {
        "release"
    }
}

async fn resolve_build(
    access_token: &str,
    spec: &str,
) -> Result<(String, String, Option<u32>)> {
    if is_channel(spec) {
        let (build, num, _) = download::fetch_build_info(access_token, spec).await?;
        let clean = build
            .strip_prefix(&format!("{spec}@"))
            .unwrap_or(&build)
            .to_string();
        Ok((spec.to_string(), clean, Some(num)))
    } else {
        let channel = infer_channel(spec);
        let clean = spec
            .strip_prefix(&format!("{channel}@"))
            .unwrap_or(spec)
            .to_string();
        let num = match download::fetch_build_info(access_token, channel).await {
            Ok((latest, n, _))
                if latest
                    .strip_prefix(&format!("{channel}@"))
                    .unwrap_or(&latest)
                    == clean =>
            {
                Some(n)
            }
            _ => None,
        };
        Ok((channel.to_string(), clean, num))
    }
}

async fn ensure_valid_token(account_label: &str) -> Result<String> {
    let mut config = config::load_full_config();

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
