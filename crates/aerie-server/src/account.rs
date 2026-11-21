//! The server's own Hytale account: the `login` command and the game session
//! kept fresh while the server runs (spec 04).

use std::{path::Path, sync::Arc, time::Duration};

use aerie_auth::account::{self, GameSession, Saved};
use aerie_auth::jwt::TokenVerifier;
use aerie_auth::oauth::OAuth;
use anyhow::{Context, Result, bail};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// How often the game session is renewed. The official server renews 5
/// minutes before expiry; a fixed half-hour keeps Aerie well inside that.
const SESSION_REFRESH: Duration = Duration::from_secs(30 * 60);

pub struct ServerAccount {
    pub http: reqwest::Client,
    pub verifier: TokenVerifier,
    session: RwLock<GameSession>,
    /// Random per run; players' access tokens are issued for it.
    pub audience: String,
    /// `x5t#S256` of this server's TLS certificate.
    pub cert_fingerprint: String,
}

impl ServerAccount {
    /// Loads the saved login, if any, and opens a game session with it.
    pub async fn start(path: &Path, server_cert_der: &[u8]) -> Result<Option<Arc<Self>>> {
        let Some(mut saved) = Saved::load(path).await? else {
            tracing::warn!(
                "no server login at {}: release clients will refuse this server; \
                 run `aerie-server login --account {}` first",
                path.display(),
                path.display()
            );
            return Ok(None);
        };
        let http = aerie_auth::http();
        let oauth = OAuth::server(http.clone());
        if saved.tokens.expires_soon() {
            let mut tries = 0;
            saved.tokens = loop {
                tries += 1;
                match oauth.refresh(&saved.tokens).await {
                    Ok(t) => break t,
                    Err(e) if tries < 5 => {
                        tracing::warn!("refreshing the login failed ({e:#}); retrying in 5 s");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                    Err(e) => {
                        return Err(e)
                            .context("refreshing the saved login; run `aerie-server login` again");
                    }
                }
            };
            saved.save(path).await?;
        }
        // Mobile connections drop for a moment now and then: a few tries.
        let mut tries = 0;
        let session = loop {
            tries += 1;
            match account::new_session(&http, &saved.tokens, saved.profile.uuid).await {
                Ok(s) => break s,
                Err(e) if tries < 5 => {
                    tracing::warn!("opening the game session failed ({e:#}); retrying in 5 s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                Err(e) => return Err(e).context("opening the game session"),
            }
        };
        info!(
            "logged in as {} ({})",
            saved.profile.username, saved.profile.uuid
        );

        let me = Arc::new(Self {
            verifier: TokenVerifier::new(http.clone()),
            http,
            session: RwLock::new(session),
            audience: uuid::Uuid::new_v4().to_string(),
            cert_fingerprint: aerie_auth::cert_fingerprint(server_cert_der),
        });
        let weak = Arc::downgrade(&me);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SESSION_REFRESH).await;
                let Some(me) = weak.upgrade() else { return };
                let current = me.session().await;
                match account::refresh_session(&me.http, &current).await {
                    Ok(s) => *me.session.write().await = s,
                    Err(e) => warn!("game session refresh failed: {e}"),
                }
            }
        });
        Ok(Some(me))
    }

    pub async fn session(&self) -> GameSession {
        self.session.read().await.clone()
    }
}

/// `aerie-server login`: device-flow login, then picks the game profile and saves it.
pub async fn login_command(path: &Path, profile_name: Option<String>) -> Result<()> {
    let http = aerie_auth::http();
    let oauth = OAuth::server(http.clone());
    let code = oauth.start().await?;
    let link = code
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| code.verification_uri.clone());
    println!();
    println!(
        "  Open this link on any device (your phone works) and log in to your Hytale account:"
    );
    println!("    {link}");
    println!("  Code: {}", code.user_code);
    println!("  Waiting up to {} minutes...", code.expires_in / 60);
    println!();
    let tokens = oauth.wait(&code).await?;

    let profiles = account::profiles(&http, &tokens).await?;
    let profile = match &profile_name {
        Some(name) => profiles.iter().find(|p| &p.username == name).cloned(),
        None => profiles.first().cloned(),
    };
    let Some(profile) = profile else {
        if profiles.is_empty() {
            bail!("this account has no game profile");
        }
        let names: Vec<_> = profiles.iter().map(|p| p.username.as_str()).collect();
        bail!(
            "no profile named {:?}; this account has {names:?}",
            profile_name.unwrap_or_default()
        );
    };
    Saved {
        tokens,
        profile: profile.clone(),
    }
    .save(path)
    .await?;
    println!(
        "  Logged in as {}. Saved to {} (keep this file private: it is a login).",
        profile.username,
        path.display()
    );
    Ok(())
}
