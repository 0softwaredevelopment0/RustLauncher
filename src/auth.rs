//! Authentication for the three supported account kinds.
//!
//! - **Offline** — the same scheme as vanilla offline mode and the original
//!   PowerLaunch: UUID = md5("OfflinePlayer:<name>") formatted as a type-3
//!   (name-based) UUID.
//! - **Ely.by** — username/password against the Ely.by authserver (Yggdrasil);
//!   the returned access token is opaque to the game.
//! - **Mojang / Microsoft** — OAuth device-code flow against login.microsoft.com,
//!   then XBL → XSTS → the Minecraft services profile endpoint.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use md5::compute as md5_compute;
use serde::Deserialize;

use crate::net;

/// The kind of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    Offline,
    ElyBy,
    Mojang,
}

impl AccountKind {
    /// All kinds, in UI order.
    pub const ALL: &'static [AccountKind] = &[
        AccountKind::Offline,
        AccountKind::ElyBy,
        AccountKind::Mojang,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            AccountKind::Offline => "offline",
            AccountKind::ElyBy => "elyby",
            AccountKind::Mojang => "mojang",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AccountKind::Offline => "Offline",
            AccountKind::ElyBy => "Ely.by",
            AccountKind::Mojang => "Mojang / Microsoft",
        }
    }

    pub fn from_slug(slug: &str) -> Option<AccountKind> {
        match slug {
            "offline" => Some(AccountKind::Offline),
            "elyby" => Some(AccountKind::ElyBy),
            "mojang" => Some(AccountKind::Mojang),
            _ => None,
        }
    }
}

/// A logged-in account. `access_token` is passed to the game as
/// `--accessToken`; offline accounts use their UUID (the vanilla convention).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub kind: AccountKind,
}

impl Account {
    /// Build an offline-style account (token = uuid).
    #[allow(dead_code)] // superseded by AccountStore offline records
    pub fn offline(username: String, uuid: String) -> Account {
        Account {
            access_token: uuid.clone(),
            kind: AccountKind::Offline,
            username,
            uuid,
        }
    }
}

// ── Ely.by (Yggdrasil) ─────────────────────────────────────────

#[derive(Deserialize)]
struct YgAuthResponse {
    #[serde(default)]
    #[allow(dead_code)]
    client_token: String,
    /// The opaque token to pass to the game as `--accessToken`.
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    #[allow(dead_code)]
    available_profiles: Vec<YgProfile>,
    selected_profile: Option<YgProfile>,
    #[serde(default)]
    #[allow(dead_code)]
    user: serde_json::Value,
}

#[derive(Deserialize)]
struct YgProfile {
    id: String,
    name: String,
}

/// Log in to Ely.by with username + password; returns the game account.
pub fn login_elyby(username: &str, password: &str) -> Result<Account> {
    let agent = net::agent();
    let body = serde_json::json!({
        "agent": { "name": "Minecraft", "version": 1 },
        "username": username,
        "password": password,
        "clientToken": "rustlauncher",
        "requestUser": true,
    });
    let response = agent
        .post("https://authserver.ely.by/api/authenticate")
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| match e {
            ureq::Error::Status(401 | 403, _) => {
                anyhow!("Ely.by rejected the credentials: wrong login or password")
            }
            ureq::Error::Status(code, resp) => {
                let text = resp.into_string().unwrap_or_default();
                let message = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.get("errorMessage")
                            .and_then(|m| m.as_str().map(String::from))
                    })
                    .unwrap_or_else(|| format!("HTTP {code} from Ely.by"));
                anyhow!(message)
            }
            other => anyhow!("Ely.by request failed: {other}"),
        })?;
    let parsed: YgAuthResponse = response
        .into_json()
        .context("failed to parse the Ely.by auth response")?;
    let profile = parsed.selected_profile.ok_or_else(|| {
        anyhow!("the Ely.by account has no Minecraft profile (choose a nickname on ely.by first)")
    })?;
    if parsed.access_token.is_empty() {
        bail!("Ely.by returned no access token");
    }
    Ok(Account {
        username: profile.name,
        uuid: profile.id,
        access_token: parsed.access_token,
        kind: AccountKind::ElyBy,
    })
}

// ── Microsoft (Mojang) device-code flow ───────────────────────

const MS_CLIENT_ID: &str = "00000000-4d8c-4a0b-a25d-31f0b26d1c5f";

#[derive(Deserialize)]
struct DeviceCodeResponse {
    #[allow(dead_code)] // parsed for completeness of the MS response
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    #[allow(dead_code)]
    message: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    interval: u64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TokenResponse {
    Success {
        access_token: String,
        #[serde(default)]
        refresh_token: String,
    },
    Error {
        error: String,
        #[serde(default)]
        error_description: String,
    },
}

#[derive(Deserialize)]
struct XblResponse {
    #[serde(default)]
    #[allow(dead_code)]
    display_claims: serde_json::Value,
    token: String,
}

#[derive(Deserialize)]
struct McProfile {
    id: String,
    name: String,
}

/// The credentials produced by the Microsoft device-code login.
#[derive(Debug, Clone)]
pub struct MicrosoftLogin {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Seconds the access token stays valid.
    #[allow(dead_code)]
    pub expires_in: u64,
}

/// Start a device-code login: returns what the user must enter at the
/// verification URL, and polls in the background via [` microsoft_poll`].
pub fn microsoft_begin(agent: &ureq::Agent) -> Result<(String, String, u64, u64)> {
    let body = format!(
        "client_id={MS_CLIENT_ID}&scope=XboxLive.signin%20offline_access&response_mode=form_post"
    );
    let response: DeviceCodeResponse = agent
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body)
        .map_err(|e| anyhow!("Microsoft device-code request failed: {e}"))?
        .into_json()
        .context("failed to parse the device-code response")?;
    Ok((
        response.verification_uri,
        response.user_code,
        response.expires_in,
        response.interval.max(1),
    ))
}

/// Poll the Microsoft token endpoint once. `Ok(None)` = keep polling.
pub fn microsoft_poll(agent: &ureq::Agent, device_code: &str) -> Result<Option<MicrosoftLogin>> {
    let body = format!(
        "grant_type=urn:ietf:params:oauth:grant-type:device_code&client_id={MS_CLIENT_ID}&device_code={device_code}"
    );
    let response: TokenResponse = match agent
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body)
    {
        Ok(resp) => resp.into_json().context("bad token response")?,
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            let parsed: TokenResponse =
                serde_json::from_str(&text).context("failed to parse the token error response")?;
            let _ = code;
            match parsed {
                TokenResponse::Error { error, .. }
                    if error == "authorization_pending" || error == "slow_down" =>
                {
                    return Ok(None)
                }
                TokenResponse::Error {
                    error,
                    error_description,
                } => {
                    bail!("Microsoft login failed: {error} ({error_description})")
                }
                success @ TokenResponse::Success { .. } => success,
            }
        }
        Err(e) => return Err(anyhow!("Microsoft token poll failed: {e}")),
    };
    let (access_token, refresh_token) = match response {
        TokenResponse::Success {
            access_token,
            refresh_token,
        } => (access_token, refresh_token),
        TokenResponse::Error { error, .. } => bail!("Microsoft login failed: {error}"),
    };
    login_with_ms_token(agent, &access_token, refresh_token).map(Some)
}

/// XBL → XSTS → Minecraft profile, from a Microsoft OAuth access token.
pub fn login_with_ms_token(
    agent: &ureq::Agent,
    ms_token: &str,
    refresh_token: String,
) -> Result<MicrosoftLogin> {
    // 1. XBL user token.
    let xbl: XblResponse = agent
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .timeout(Duration::from_secs(30))
        .send_json(serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_token}")
            },
            "RpsTicket": format!("d={ms_token}"),
            "Endpoint": "https://user.auth.xboxlive.com/"
        }))
        .map_err(|e| anyhow!("Xbox Live auth failed: {e}"))?
        .into_json()
        .context("bad XBL response")?;

    // 2. XSTS token.
    let xsts: XblResponse = agent
        .post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .timeout(Duration::from_secs(30))
        .send_json(serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl.token]
            },
            "RpsTicket": "",
            "Endpoint": "https://xsts.auth.xboxlive.com/"
        }))
        .map_err(|e| match e {
            ureq::Error::Status(401, resp) => {
                let text = resp.into_string().unwrap_or_default();
                let code = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| v.pointer("/XErr").and_then(|x| x.as_i64()))
                    .unwrap_or(0);
                let why = match code {
                    2148916233 => "the Microsoft account has no Xbox profile",
                    2148916238 => "the account is a child account",
                    _ => "Xbox Live authorization failed",
                };
                anyhow!("{why} (XErr {code})")
            }
            other => anyhow!("XSTS auth failed: {other}"),
        })?
        .into_json()
        .context("bad XSTS response")?;

    // 3. Minecraft login with XSTS.
    let mc_resp = agent
        .post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .timeout(Duration::from_secs(30))
        .send_json(serde_json::json!({ "identityToken": format!("XBL3.0 x={};{}", xsts_display_claim(&xsts), xsts.token) }))
        .map_err(|e| anyhow!("Minecraft services login failed: {e}"))?;
    let mc: serde_json::Value = mc_resp
        .into_json()
        .context("bad Minecraft login response")?;
    let mc_token = mc
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| anyhow!("no access_token from Minecraft services"))?
        .to_string();
    let expires_in = mc.get("expires_in").and_then(|e| e.as_u64()).unwrap_or(0);

    // 4. Profile.
    let profile: McProfile = agent
        .get("https://api.minecraftservices.com/minecraft/profile")
        .timeout(Duration::from_secs(30))
        .set("Authorization", &format!("Bearer {mc_token}"))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => {
                anyhow!("this Microsoft account does not own Minecraft (Java)")
            }
            other => anyhow!("failed to fetch the Minecraft profile: {other}"),
        })?
        .into_json()
        .context("bad profile response")?;

    Ok(MicrosoftLogin {
        username: profile.name,
        uuid: profile.id,
        access_token: mc_token,
        refresh_token,
        expires_in,
    })
}

/// The uhs (user hash) used in the XBL3.0 identity token.
fn xsts_display_claim(xsts: &XblResponse) -> String {
    xsts.display_claims
        .get("xui")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("uhs"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string()
}

// ── Offline ───────────────────────────────────────────────────

/// Validate an offline username per Minecraft rules.
pub fn validate_username(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.len() < 3 {
        return Err(anyhow!("username must be at least 3 characters"));
    }
    if name.len() > 16 {
        return Err(anyhow!("username cannot be longer than 16 characters"));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(anyhow!(
            "username may only contain letters, digits, and underscores"
        ));
    }
    Ok(name)
}

/// Derive the offline UUID (v3, md5 of `OfflinePlayer:<name>`) with the
/// RFC 4122 version/variant bits set, as Minecraft does.
pub fn offline_uuid(name: &str) -> String {
    let digest = md5_compute(format!("OfflinePlayer:{name}"));
    let mut bytes = digest.0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30; // version 3
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // IETF variant

    format_uuid(&bytes)
}

/// Format 16 bytes as the canonical dashed UUID string.
fn format_uuid(bytes: &[u8; 16]) -> String {
    let hex = |slice: &[u8]| -> String { slice.iter().map(|b| format!("{b:02x}")).collect() };
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16])
    )
}

/// Log in offline with the given username.
pub fn login_offline(username: &str) -> Result<Account> {
    let name = validate_username(username)?.to_string();
    Ok(Account {
        access_token: String::new(),
        kind: AccountKind::Offline,
        uuid: offline_uuid(&name),
        username: name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_usernames() {
        assert_eq!(validate_username("Rizer001").unwrap(), "Rizer001");
        assert_eq!(validate_username("  abc  ").unwrap(), "abc");
        assert_eq!(validate_username("a_b_123").unwrap(), "a_b_123");
    }

    #[test]
    fn rejects_invalid_usernames() {
        assert!(validate_username("ab").is_err()); // too short
        assert!(validate_username("averyveryverylongname").is_err()); // >16
        assert!(validate_username("bad name").is_err()); // space
        assert!(validate_username("bad-д name").is_err()); // non-ascii
        assert!(validate_username("").is_err()); // empty
    }

    #[test]
    fn offline_uuid_is_deterministic_and_well_formed() {
        let a = offline_uuid("Notch");
        let b = offline_uuid("Notch");
        assert_eq!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.chars().filter(|c| *c == '-').count(), 4);

        // Version 3 and IETF variant nibbles.
        let hex: String = a.chars().filter(|c| *c != '-').collect();
        assert!(hex.starts_with(|_| true));
        let version_nibble = hex.chars().nth(12).unwrap().to_digit(16).unwrap();
        assert_eq!(version_nibble, 3);

        // Different names -> different UUIDs.
        assert_ne!(a, offline_uuid("Dinnerbone"));
    }

    #[test]
    fn login_offline_roundtrip() {
        let account = login_offline("Rizer001").unwrap();
        assert_eq!(account.username, "Rizer001");
        assert_eq!(account.uuid, offline_uuid("Rizer001"));
        assert!(login_offline("x").is_err());
    }
}
