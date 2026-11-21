//! OAuth device flow and refresh (spec 04, "Logging in").

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{Error, OAUTH_BASE, Result, check};

/// OAuth client id and scopes a server logs in with.
pub const SERVER_CLIENT_ID: &str = "hytale-server";
pub const SERVER_SCOPES: &str = "openid offline auth:server";

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    /// What the person types on the verification page.
    pub user_code: String,
    pub verification_uri: String,
    /// Same page with the code already filled in: the link to show or open.
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// Tokens for one account. Keep them private: the refresh token is a login.
#[derive(Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds.
    pub expires_at: u64,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

impl Tokens {
    fn from_answer(a: TokenAnswer, old_refresh: Option<String>) -> Option<Self> {
        Some(Self {
            access_token: a.access_token?,
            refresh_token: a.refresh_token.or(old_refresh),
            expires_at: now() + a.expires_in.unwrap_or(3600),
        })
    }

    pub fn expires_soon(&self) -> bool {
        self.expires_at <= now() + 60
    }
}

pub struct OAuth {
    http: reqwest::Client,
    client_id: &'static str,
    scopes: &'static str,
}

impl OAuth {
    pub fn server(http: reqwest::Client) -> Self {
        Self {
            http,
            client_id: SERVER_CLIENT_ID,
            scopes: SERVER_SCOPES,
        }
    }

    /// Step 1: get a code to show the person.
    pub async fn start(&self) -> Result<DeviceCode> {
        let r = self
            .http
            .post(format!("{OAUTH_BASE}/device/auth"))
            .form(&[("client_id", self.client_id), ("scope", self.scopes)])
            .send()
            .await?;
        Ok(check("device authorization", r).await?.json().await?)
    }

    /// Step 3: wait until the person has approved the code.
    pub async fn wait(&self, code: &DeviceCode) -> Result<Tokens> {
        let deadline = now() + code.expires_in;
        let mut interval = code.interval.max(1);
        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            if now() > deadline {
                return Err(Error::LoginExpired);
            }
            let r = self
                .http
                .post(format!("{OAUTH_BASE}/token"))
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", self.client_id),
                    ("device_code", code.device_code.as_str()),
                ])
                .send()
                .await?;
            let status = r.status();
            let a: TokenAnswer = r.json().await?;
            match a.error.as_deref() {
                Some("authorization_pending") => continue,
                Some("slow_down") => interval += 5,
                Some(e) => return Err(Error::LoginRefused(e.to_owned())),
                None if status.is_success() => {
                    return Tokens::from_answer(a, None)
                        .ok_or(Error::LoginRefused("no access token".into()));
                }
                None => return Err(Error::LoginRefused(format!("HTTP {status}"))),
            }
        }
    }

    pub async fn refresh(&self, tokens: &Tokens) -> Result<Tokens> {
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or(Error::LoginRefused("no refresh token".into()))?;
        let r = self
            .http
            .post(format!("{OAUTH_BASE}/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id),
                ("refresh_token", refresh.as_str()),
            ])
            .send()
            .await?;
        let a: TokenAnswer = check("token refresh", r).await?.json().await?;
        Tokens::from_answer(a, Some(refresh)).ok_or(Error::LoginRefused("no access token".into()))
    }
}
