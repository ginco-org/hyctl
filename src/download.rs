use anyhow::{Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::path::Path;
use tracing::info;

const JRE_DEFAULT_VERSION: &str = "25.0.2_10";
const JRE_BASE_URL: &str = "https://launcher.hytale.com/redist/jre";

const ACCOUNT_DATA_BASE: &str = "https://account-data.hytale.com";

/// ── Launcher Build Info ───────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct PatchlineInfo {
    #[serde(rename = "buildVersion")]
    build_version: String,
    newest: u32,
}

#[derive(Debug, serde::Deserialize)]
struct LauncherDataResponse {
    owner: String,
    patchlines: std::collections::HashMap<String, PatchlineInfo>,
}

/// Fetch the current build version and build number for a channel.
/// Returns `(version_string, build_number, owner_uuid)`.
pub async fn fetch_build_info(
    access_token: &str,
    channel: &str,
) -> Result<(String, u32, String)> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{ACCOUNT_DATA_BASE}/my-account/get-launcher-data"))
        .query(&[("arch", "amd64"), ("os", "linux")])
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .context("launcher data request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("launcher data error ({}): {text}", status);
    }

    let data: LauncherDataResponse = resp
        .json()
        .await
        .context("failed to parse launcher data")?;

    let pl = data
        .patchlines
        .get(channel)
        .with_context(|| format!("channel '{channel}' not in launcher data"))?;

    Ok((pl.build_version.clone(), pl.newest, data.owner))
}

// ── Patch Set ──────────────────────────────────────────────────────────
#[derive(Debug, serde::Deserialize)]
pub struct PatchStep {
    pub from: u32,
    pub to: u32,
    pub pwr: String,
}

/// Fetch the list of patch steps needed to reach `build_number`.
pub async fn fetch_patch_set(
    access_token: &str,
    os: &str,
    arch: &str,
    channel: &str,
    build_number: u32,
) -> Result<Vec<PatchStep>> {
    let client = reqwest::Client::new();
    let url = format!("{ACCOUNT_DATA_BASE}/patches/{os}/{arch}/{channel}/{build_number}");
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .context("patch set request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("patch set error ({}): {text}", status);
    }

    #[derive(serde::Deserialize)]
    struct PatchSetResp {
        steps: Vec<PatchStep>,
    }

    let ps: PatchSetResp = resp.json().await.context("failed to parse patch set")?;
    Ok(ps.steps)
}

/// Download and apply the full-install wharf patch for the given build.
pub async fn install_client(
    access_token: &str,
    channel: &str,
    build_number: u32,
    install_dir: &Path,
) -> Result<()> {
    let steps = fetch_patch_set(access_token, "linux", "amd64", channel, build_number).await?;

    let step = steps
        .into_iter()
        .find(|s| s.from == 0)
        .with_context(|| {
            format!("no full-install patch step (from=0) found for build {build_number}")
        })?;

    let tmp_pwr = install_dir
        .parent()
        .unwrap_or(install_dir)
        .join(format!("{channel}-{build_number}.pwr"));

    info!("Downloading client patch (0 → {})", step.to);
    download_file(
        &step.pwr,
        &tmp_pwr,
        &format!("{channel} client 0→{}", step.to),
    )
    .await?;

    info!("Applying wharf patch to {}", install_dir.display());
    let install_dir_buf = install_dir.to_path_buf();
    let tmp_clone = tmp_pwr.clone();
    tokio::task::spawn_blocking(move || crate::wharf::apply_patch(&tmp_clone, &install_dir_buf))
        .await
        .context("patch apply task panicked")??;

    tokio::fs::remove_file(&tmp_pwr)
        .await
        .context("failed to remove temporary patch file")?;

    Ok(())
}

// ── Version Info ──────────────────────────────────────────────────────


/// Version JSON returned by the version endpoint.
/// Contains a signed URL to the actual version manifest.
#[derive(Debug, Deserialize)]
struct VersionResponse {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    target: Option<String>, // sometimes the signed URL is in a "target" field
}

/// Version manifest returned by the signed manifest URL.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildManifest {
    pub version: String,
    pub download_url: String,
    pub sha256: Option<String>,
}

/// Fetch the signed URL for a given patchline's version manifest.
pub async fn fetch_version_url(
    access_token: &str,
    patchline: &str,
) -> Result<(String, BuildManifest)> {
    let client = reqwest::Client::new();

    let resp = client
        .get(format!(
            "{ACCOUNT_DATA_BASE}/game-assets/version/{patchline}.json"
        ))
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .context("version info request failed")?;

    if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("version info error ({}): {text}", status);
        }

    let vr: VersionResponse = resp
        .json()
        .await
        .context("failed to parse version response")?;

    let url = vr
        .url
        .or(vr.target)
        .context("version response has no URL field")?;

    // Fetch the actual build manifest from the signed URL.
    let manifest_resp = client
        .get(&url)
        .send()
        .await
        .context("failed to fetch build manifest from signed URL")?;

    if !manifest_resp.status().is_success() {
        anyhow::bail!(
            "build manifest fetch error ({}): {}",
            manifest_resp.status(),
            manifest_resp.text().await.unwrap_or_default()
        );
    }

    let manifest: BuildManifest = manifest_resp
        .json()
        .await
        .context("failed to parse build manifest")?;

    Ok((url, manifest))
}


// ── Download ──────────────────────────────────────────────────────────

/// Download a file from `url` to `dest` with a progress bar.
pub async fn download_file(url: &str, dest: &Path, label: &str) -> Result<()> {
    let client = reqwest::Client::new();

    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("download request failed for {label}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("download failed ({}): {label}", resp.status());
    }

    let total_size = resp
        .content_length()
        .unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}\n{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
            .context("failed to set progress style")?
            .progress_chars("=> "),
    );
    pb.set_message(format!("Downloading {label}"));

    // Ensure parent directory exists.
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create download directory")?;
    }

    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("failed to create file: {}", dest.display()))?;

    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("error reading download stream")?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .context("error writing download to file")?;
        downloaded += chunk.len() as u64;
        pb.set_position(downloaded);
    }

    pb.finish_with_message(format!("Downloaded {label}"));
    Ok(())
}


/// Download and install a JRE to `dest_dir`.
///
/// Downloads from `launcher.hytale.com/redist/jre/{os}/{arch}/jre-{version}.tar.gz`
/// and extracts it so that `dest_dir/bin/java` is the executable.
pub async fn install_jre(version: &str, dest_dir: &Path) -> Result<()> {
    let os = "linux";
    let arch = "amd64";
    let filename = format!("jre-{version}.tar.gz");
    let url = format!("{JRE_BASE_URL}/{os}/{arch}/{filename}");

    info!("Downloading JRE {version} from {url}");

    let tmp = dest_dir
        .parent()
        .unwrap_or(dest_dir)
        .join(format!("hyctl-jre-{version}.tar.gz"));

    download_file(&url, &tmp, &format!("JRE {version}")).await?;

    info!("Extracting JRE to {}", dest_dir.display());
    tokio::fs::create_dir_all(dest_dir)
        .await
        .context("failed to create JRE directory")?;

    let tmp_clone = tmp.clone();
    let dest_clone = dest_dir.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::open(&tmp_clone)
            .context("failed to open JRE tarball")?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);
        // Strip the top-level directory from the tarball.
        for entry in archive.entries().context("failed to read tar entries")? {
            let mut entry = entry.context("failed to read tar entry")?;
            let path = entry.path().context("failed to read entry path")?;
            let stripped = path
                .components()
                .skip(1)
                .collect::<std::path::PathBuf>();
            if stripped.as_os_str().is_empty() {
                continue;
            }
            let out = dest_clone.join(&stripped);
            entry.unpack(&out)
                .with_context(|| format!("failed to extract {}", stripped.display()))?;
        }
        Ok(())
    })
    .await
    .context("JRE extraction task panicked")??;

    tokio::fs::remove_file(&tmp)
        .await
        .context("failed to remove JRE tarball")?;

    // Verify the result.
    let java_bin = dest_dir.join("bin").join("java");
    if !java_bin.is_file() {
        anyhow::bail!(
            "JRE extraction succeeded but bin/java not found at {}",
            java_bin.display()
        );
    }

    info!("JRE installed at {}", dest_dir.display());
    Ok(())
}

pub fn jre_default_version() -> &'static str {
    JRE_DEFAULT_VERSION
}
