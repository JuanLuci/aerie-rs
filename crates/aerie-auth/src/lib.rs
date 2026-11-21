//! Hytale account login and session-service calls (spec 04).
//!
//! Pure Rust over HTTPS (rustls), no browser and nothing desktop-only, so the
//! same code runs in a dedicated server, in a server embedded in the phone app,
//! and later in the Aerie client. Logging in uses the OAuth device flow: the
//! person opens a link on any device (the phone's own browser is fine) and
//! types a short code.

pub mod account;
pub mod jwt;
pub mod oauth;

use base64::Engine;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const OAUTH_BASE: &str = "https://oauth.accounts.hytale.com/oauth2";
pub const SESSION_SERVICE: &str = "https://sessions.hytale.com";
pub const ACCOUNT_DATA: &str = "https://account-data.hytale.com";

/// Aerie says who it is; it does not pretend to be the official server.
pub const USER_AGENT: &str = concat!("AerieServer/", env!("CARGO_PKG_VERSION"));

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{what} answered {status}: {body}")]
    Status {
        what: &'static str,
        status: u16,
        body: String,
    },
    #[error("login expired before it was completed")]
    LoginExpired,
    #[error("login was refused: {0}")]
    LoginRefused(String),
    #[error("account has no game profile")]
    NoProfile,
    #[error("token rejected: {0}")]
    BadToken(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("http client")
}

/// `x5t#S256` of a certificate: SHA-256 of its DER bytes, base64url without padding.
pub fn cert_fingerprint(der: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(der))
}

async fn check(what: &'static str, r: reqwest::Response) -> Result<reqwest::Response> {
    if r.status().is_success() {
        Ok(r)
    } else {
        let status = r.status().as_u16();
        let body = r.text().await.unwrap_or_default();
        Err(Error::Status {
            what,
            status,
            body: body.chars().take(300).collect(),
        })
    }
}
