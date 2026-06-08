use crate::config::{Tokens, Profile};
use anyhow::{Context, Result};
use base64::Engine;
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use tokio::net::TcpListener;
use tracing::info;

const OAUTH_BASE: &str = "https://oauth.accounts.hytale.com/oauth2";
const ACCOUNT_DATA_BASE: &str = "https://account-data.hytale.com";
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    id_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    token_type: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

// Official launcher identifiers — the API checks these to verify the caller.
pub(crate) const LAUNCHER_VERSION: &str = "2026.05.29-125f35f";
pub(crate) const LAUNCHER_USER_AGENT: &str = "hytale-launcher/2026.05.29-125f35f";
pub(crate) const LAUNCHER_BRANCH: &str = "release";

/// Build a request with the standard launcher headers.
fn launcher_request(client: &reqwest::Client, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .header("User-Agent", LAUNCHER_USER_AGENT)
        .header("X-Hytale-Launcher-Version", LAUNCHER_VERSION)
        .header("X-Hytale-Launcher-Branch", LAUNCHER_BRANCH)
        .header("Accept", "application/json")
}
// ── PKCE Flow with Local Server ──────────────────────────────────────

/// Generate a PKCE code verifier (base64url-encoded random bytes).
pub fn pkce_verifier() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = std::array::from_fn(|_| rng.r#gen());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Compute the S256 code challenge from a verifier string.
/// challenge = BASE64URL-ENCODE(SHA256(ASCII(verifier)))
pub fn pkce_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}

/// Perform the full PKCE login flow: start local server, open browser,
/// wait for callback, exchange code for tokens.
pub async fn launcher_login() -> Result<Tokens> {
    // Bind to a random available port on loopback.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let code_verifier = pkce_verifier();
    let code_challenge = pkce_challenge(&code_verifier);

    // Random inner state for CSRF protection.
    let inner_state = {
        let mut rng = rand::thread_rng();
        let bytes: Vec<u8> = (0..16).map(|_| rng.r#gen()).collect();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
    };

    // The launcher encodes state as base64({"state":"...","port":"..."}).
    // The consent page parses this and redirects the browser to http://127.0.0.1:{port}/authorization-callback.
    let state_json = serde_json::json!({ "state": inner_state, "port": port.to_string() });
    let state_param = base64::engine::general_purpose::STANDARD.encode(state_json.to_string());

    const REDIRECT_URI: &str = "https://accounts.hytale.com/consent/client";

    let auth_url = format!(
        "{OAUTH_BASE}/auth?\
         access_type=offline&\
         client_id=hytale-launcher&\
         scope=openid+offline+auth%3Alauncher&\
         response_type=code&\
         redirect_uri={REDIRECT_URI}&\
         code_challenge={code_challenge}&\
         code_challenge_method=S256&\
         state={state_param}"
    );

    println!("\nOpen this URL in your browser (you must be logged in to your Hytale account):\n  {auth_url}\n");

    info!("Waiting for OAuth callback on port {port}");


    // Open browser.
    let _ = open::that(&auth_url);

    // Accept one connection — the consent page redirects here.
    let (mut stream, _) = listener.accept().await?;
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse "GET /authorization-callback?code=...&state=... HTTP/1.1"
    let path = request
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    let query = path.split('?').nth(1).unwrap_or("");

    let mut code = None;
    let mut state = None;

    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("code"), Some(v)) => code = Some(url_decode(v)?),
            (Some("state"), Some(v)) => state = Some(url_decode(v)?),
            _ => {}
        }
    }

    // Send a success response to the browser.
    let response_body = if code.is_some() {
        "<html><body><h1>Authentication successful!</h1><p>You can close this window.</p></body></html>"
    } else {
        "<html><body><h1>Authentication failed.</h1><p>No code received.</p></body></html>"
    };
    let http_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
        response_body.len(),
        response_body
    );
    let _ = stream.write_all(http_response.as_bytes()).await;

    // Validate state.
    if state.as_deref() != Some(&inner_state) {
        anyhow::bail!(
            "OAuth state mismatch (got {:?}, expected {:?})",
            state,
            inner_state
        );
    }

    let code = code.context("no authorization code in callback")?;

    // Exchange the code for tokens (redirect_uri MUST match the auth request).
    pkce_token_exchange(&code, &code_verifier, REDIRECT_URI).await
}

async fn pkce_token_exchange(code: &str, code_verifier: &str, redirect_uri: &str) -> Result<Tokens> {
    let client = reqwest::Client::new();

    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", "hytale-launcher"),
        ("code", code),
        ("code_verifier", code_verifier),
        ("redirect_uri", redirect_uri),
    ];

    let resp = client
        .post(format!("{OAUTH_BASE}/token"))
        .form(&params)
        .send()
        .await
        .context("token exchange request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("token exchange error ({status}): {text}");
    }


    let tr: TokenResponse = resp
        .json()
        .await
        .context("failed to parse token exchange response")?;

    let expires_at = tr.expires_in.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + secs as i64
    });

    Ok(Tokens {
        access_token: tr.access_token.context("missing access_token in token response")?,
        id_token: tr.id_token,
        refresh_token: tr.refresh_token,
        expires_at,
        scope: tr.scope,
    })
}

/// Minimal URL-decoding for values from the query string.
/// Decode URL percent-encoding for query string values.
fn url_decode(s: &str) -> Result<String> {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '+' => result.push(' '),
            '%' => {
                let hi = chars.next().and_then(|c| c.to_digit(16))
                    .with_context(|| format!("invalid percent-encoding in query value: {s}"))?;
                let lo = chars.next().and_then(|c| c.to_digit(16))
                    .with_context(|| format!("invalid percent-encoding in query value: {s}"))?;
                result.push(char::from(hi as u8 * 16 + lo as u8));
            }
            _ => result.push(c),
        }
    }
    Ok(result)
}
// ── JWT Claim Helpers ──────────────────────────────────────────────────

/// Decode the payload of a JWT and return (sub, email) without signature verification.
pub fn decode_id_token_claims(id_token: &str) -> Option<(String, Option<String>)> {
    let payload = id_token.split('.').nth(1)?;
    // JWT uses URL_SAFE_NO_PAD; tolerate missing padding.
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let sub = claims.get("sub")?.as_str()?.to_string();
    let email = claims.get("email").and_then(|v| v.as_str()).map(str::to_string);
    Some((sub, email))
}

// ── Token Refresh ──────────────────────────────────────────────────────

/// Refresh an access token using its refresh token.
pub async fn refresh_token(client_id: &str, refresh_token: &str) -> Result<Tokens> {
    let client = reqwest::Client::new();

    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
    ];

    let resp = client
        .post(format!("{OAUTH_BASE}/token"))
        .form(&params)
        .send()
        .await
        .context("token refresh request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("token refresh error ({status}): {text}");
    }

    let tr: TokenResponse = resp
        .json()
        .await
        .context("failed to parse refresh response")?;

    let expires_at = tr.expires_in.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + secs as i64
    });

    Ok(Tokens {
        access_token: tr.access_token.context("missing access_token in refresh response")?,
        id_token: tr.id_token,
        refresh_token: tr.refresh_token.or(Some(refresh_token.to_string())),
        expires_at,
        scope: tr.scope,
    })
}

// ── Launcher Data ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LauncherDataResponse {
    profiles: Vec<LauncherProfile>,
}

#[derive(Debug, Deserialize)]
struct LauncherProfile {
    uuid: String,
    username: String,
}

pub async fn fetch_launcher_data(access_token: &str, id_token: Option<&str>) -> Result<Vec<Profile>> {
    let client = reqwest::Client::new();

    let tokens: [(&str, &str); 2] = [
        ("access_token", access_token),
        ("id_token", id_token.unwrap_or("")),
    ];

    for &(label, token) in &tokens {
        if token.is_empty() && label == "id_token" {
            continue;
        }

        let resp = launcher_request(&client, reqwest::Method::GET, &format!("{ACCOUNT_DATA_BASE}/my-account/get-launcher-data?arch=amd64&os=linux"))
            .bearer_auth(token)
            .send()
            .await
            .context("launcher data request failed")?;

        if resp.status().is_success() {
            let ld: LauncherDataResponse = resp
                .json()
                .await
                .context("failed to parse launcher data")?;
            return Ok(ld
                .profiles
                .into_iter()
                .map(|p| Profile {
                    uuid: p.uuid,
                    username: p.username,
                })
                .collect());
        }

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        eprintln!("  {label} returned {status}: {text}");
        if label == "access_token" {
            continue;
        }
        anyhow::bail!("launcher data error ({status}): {text}");
    }

    anyhow::bail!("launcher data request failed with all available tokens");
}
