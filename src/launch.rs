use crate::config::{self, Account, Profile};
use crate::session;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tracing::info;

pub async fn launch_game(
    access_token: &str,
    _account: &Account,
    profile: &Profile,
    install_dir: &Path,
    version_label: &str,
    server: Option<&str>,
    world: Option<&str>,
    extra_args: &[String],
    background: bool,
) -> Result<()> {
    let client_bin = install_dir.join("Client").join("HytaleClient");

    if !client_bin.exists() {
        anyhow::bail!(
            "client binary not found at {}.\n\
             Run `{} asset install {version_label}` with a version that includes a client.\n\
             The latest release build includes the HytaleClient binary.",
            client_bin.display(),
            crate::BIN_NAME,
        )
    }

    let jre = ensure_jre(install_dir).await?;
    info!(
        "Creating game session for {} ({})",
        profile.username, profile.uuid
    );
    let session_tokens = session::create_session(access_token, &profile.uuid).await?;
    launch_client(install_dir, &client_bin, &jre, profile, &session_tokens, server, world, extra_args, background)
}

pub async fn ensure_jre(install_dir: &Path) -> Result<PathBuf> {
    if let Ok(p) = find_jre(install_dir) {
        return Ok(p);
    }
    let ver = crate::download::jre_default_version();
    let dest = config::data_dir().join("jre");
    println!("JRE not found — downloading {ver}...");
    crate::download::install_jre(ver, &dest).await?;
    find_jre(install_dir)
}
fn launch_client(
    install_dir: &Path,
    client_bin: &Path,
    java_exec: &Path,
    profile: &crate::config::Profile,
    session_tokens: &crate::session::SessionTokens,
    server: Option<&str>,
    world: Option<&str>,
    extra_args: &[String],
    background: bool,
) -> Result<()> {
    let user_dir = config::data_dir().join("userdata");
    let client_dir = client_bin.parent().unwrap_or(install_dir);

    info!("Launching {}", client_bin.display());
    info!("  --app-dir:   {}", install_dir.display());
    info!("  --user-dir:  {}", user_dir.display());
    info!("  --java-exec: {}", java_exec.display());

    // The Client/ dir has bundled .so files (libSDL3.so, etc.) with no RPATH set.
    // Prepend it to LD_LIBRARY_PATH so dlopen finds them.
    let existing_ld = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    let new_ld = if existing_ld.is_empty() {
        client_dir.display().to_string()
    } else {
        format!("{}:{existing_ld}", client_dir.display())
    };

    let mut cmd = std::process::Command::new(client_bin);
    cmd.current_dir(client_dir);
    cmd.env("LD_LIBRARY_PATH", new_ld);
    cmd.arg("--app-dir").arg(install_dir);
    cmd.arg("--user-dir").arg(&user_dir);
    cmd.arg("--java-exec").arg(java_exec);
    cmd.arg("--auth-mode").arg("authenticated");
    cmd.arg("--uuid").arg(&profile.uuid);
    cmd.arg("--name").arg(&profile.username);
    cmd.arg("--session-token").arg(&session_tokens.session_token);
    cmd.arg("--identity-token").arg(&session_tokens.identity_token);
    if let Some(server) = server {
        cmd.arg("--server").arg(server);
    }
    if let Some(world) = world {
        cmd.arg("--world").arg(world);
    }
    for arg in extra_args {
        cmd.arg(arg);
    }

    spawn_or_wait(&mut cmd, "HytaleClient", background)
}

pub fn launch_server(
    install_dir: &Path,
    java: &Path,
    data_dir: &Path,
    extra_args: &[String],
    background: bool,
) -> Result<()> {
    let start_sh = install_dir.join("start.sh");
    if start_sh.exists() {
        info!("Launching server via start.sh");
        let jre_bin = java.parent().unwrap();

        // Prepend our JRE bin to PATH so start.sh finds `java`.
        let path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{path}", jre_bin.display());

        let mut cmd = std::process::Command::new("bash");
        cmd.arg(&start_sh);
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.current_dir(data_dir);
        cmd.env("PATH", new_path);
        return spawn_or_wait(&mut cmd, "start.sh", background);
    }

    let server_jar = install_dir.join("Server").join("HytaleServer.jar");
    let assets = install_dir.join("Assets.zip");

    let mut cmd = std::process::Command::new(java);
    cmd.current_dir(data_dir);
    cmd.arg("-jar").arg(&server_jar);
    cmd.arg("--assets").arg(&assets);
    for arg in extra_args {
        cmd.arg(arg);
    }
    spawn_or_wait(&mut cmd, "server", background)
}

fn spawn_or_wait(cmd: &mut std::process::Command, label: &str, background: bool) -> Result<()> {
    if background {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        cmd.stdin(Stdio::null());
        let child = cmd.spawn().with_context(|| format!("failed to spawn {label}"))?;
        println!("Launched (pid {}).", child.id());
    } else {
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
        cmd.stdin(Stdio::inherit());
        let mut child = cmd.spawn().with_context(|| format!("failed to spawn {label}"))?;
        let status = child.wait().with_context(|| format!("{label} process wait failed"))?;
        if !status.success() {
            anyhow::bail!("{label} exited with status: {status}");
        }
    }
    Ok(())
}

fn find_jre(install_dir: &Path) -> Result<PathBuf> {
    let bundled = install_dir.join("jre").join("bin").join("java");
    if bundled.is_file() {
        return Ok(bundled);
    }

    let shared = config::data_dir().join("jre").join("bin").join("java");
    if shared.is_file() {
        return Ok(shared);
    }

    anyhow::bail!(
        "JRE not found. It will be downloaded automatically on next launch.\n\
         Looked in:\n  {}\n  {}",
        bundled.display(),
        shared.display()
    )
}
