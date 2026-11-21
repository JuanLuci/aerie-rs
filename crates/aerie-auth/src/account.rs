//! Game profiles, the server's game session and the join calls (spec 04).

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::oauth::Tokens;
use crate::{ACCOUNT_DATA, Error, Result, SESSION_SERVICE, check};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub uuid: Uuid,
    pub username: String,
}

#[derive(Deserialize)]
struct Profiles {
    #[serde(default)]
    profiles: Vec<Profile>,
}

/// Tokens of a server's game session.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSession {
    pub session_token: String,
    pub identity_token: String,
    pub expires_at: Option<String>,
}

impl std::fmt::Debug for GameSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameSession")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

pub async fn profiles(http: &reqwest::Client, oauth: &Tokens) -> Result<Vec<Profile>> {
    let r = http
        .get(format!("{ACCOUNT_DATA}/my-account/get-profiles"))
        .bearer_auth(&oauth.access_token)
        .send()
        .await?;
    Ok(check("profiles", r)
        .await?
        .json::<Profiles>()
        .await?
        .profiles)
}

pub async fn new_session(
    http: &reqwest::Client,
    oauth: &Tokens,
    profile: Uuid,
) -> Result<GameSession> {
    let r = http
        .post(format!("{SESSION_SERVICE}/game-session/new"))
        .bearer_auth(&oauth.access_token)
        .json(&json!({ "uuid": profile }))
        .send()
        .await?;
    Ok(check("game session", r).await?.json().await?)
}

pub async fn refresh_session(http: &reqwest::Client, session: &GameSession) -> Result<GameSession> {
    let r = http
        .post(format!("{SESSION_SERVICE}/game-session/refresh"))
        .bearer_auth(&session.session_token)
        .send()
        .await?;
    Ok(check("game session refresh", r).await?.json().await?)
}

/// `/server-join/auth-grant`: a grant the joining client will exchange.
pub async fn auth_grant(
    http: &reqwest::Client,
    session: &GameSession,
    player_identity_token: &str,
    audience: &str,
) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        authorization_grant: Option<String>,
    }
    let r = http
        .post(format!("{SESSION_SERVICE}/server-join/auth-grant"))
        .bearer_auth(&session.session_token)
        .json(&json!({ "identityToken": player_identity_token, "aud": audience }))
        .send()
        .await?;
    check("auth grant", r)
        .await?
        .json::<A>()
        .await?
        .authorization_grant
        .ok_or(Error::BadToken("empty grant".into()))
}

/// `/server-join/auth-token`: exchanges the grant the client sent for the
/// server's access token, bound to the server certificate.
pub async fn auth_token(
    http: &reqwest::Client,
    session: &GameSession,
    grant: &str,
    server_cert_fingerprint: &str,
) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct A {
        access_token: Option<String>,
    }
    let r = http
        .post(format!("{SESSION_SERVICE}/server-join/auth-token"))
        .bearer_auth(&session.session_token)
        .json(&json!({ "authorizationGrant": grant, "x509Fingerprint": server_cert_fingerprint }))
        .send()
        .await?;
    check("auth token", r)
        .await?
        .json::<A>()
        .await?
        .access_token
        .ok_or(Error::BadToken("empty access token".into()))
}

/// What a server keeps on disk between runs: the OAuth tokens and which profile it uses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    pub tokens: Tokens,
    pub profile: Profile,
}

impl Saved {
    pub async fn load(path: &std::path::Path) -> Result<Option<Self>> {
        match tokio::fs::read(path).await {
            Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn save(&self, path: &std::path::Path) -> Result<()> {
        tokio::fs::write(path, serde_json::to_vec_pretty(self)?).await?;
        Ok(())
    }
}
