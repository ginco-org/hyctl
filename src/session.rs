use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::auth::{LAUNCHER_BRANCH, LAUNCHER_USER_AGENT, LAUNCHER_VERSION};
const SESSIONS_BASE: &str = "https://sessions.hytale.com";

// ── Session Types ─────────────────────────────────────────────────────

/// Tokens returned when creating a new game session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTokens {
    pub session_token: String,
    pub identity_token: String,
    pub expires_at: Option<String>,
}

// ── New Session ────────────────────────────────────────────────────────

/// Create a new game session for a given profile UUID.
pub async fn create_session(
    access_token: &str,
    profile_uuid: &str,
) -> Result<SessionTokens> {
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{SESSIONS_BASE}/game-session/new"))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", LAUNCHER_USER_AGENT)
        .header("X-Hytale-Launcher-Version", LAUNCHER_VERSION)
        .header("X-Hytale-Launcher-Branch", LAUNCHER_BRANCH)
        .json(&serde_json::json!({ "uuid": profile_uuid }))
        .send()
        .await
        .context("session creation request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("session creation error ({}): {text}", status);
    }

    let tokens: SessionTokens = resp
        .json()
        .await
        .context("failed to parse session tokens response")?;

    Ok(tokens)
}
