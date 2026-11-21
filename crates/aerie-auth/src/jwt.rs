//! Checking the session service's tokens: JWTs signed with Ed25519 (spec 04, "Tokens").

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::{Error, Result, SESSION_SERVICE, check};

const CLOCK_SKEW: i64 = 300;

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: Option<String>,
    crv: Option<String>,
    x: Option<String>,
}

/// Keeps the session service's public keys and checks tokens against them.
pub struct TokenVerifier {
    http: reqwest::Client,
    keys: RwLock<HashMap<String, VerifyingKey>>,
}

fn b64(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|_| Error::BadToken("bad base64".into()))
}

impl TokenVerifier {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            keys: RwLock::new(HashMap::new()),
        }
    }

    async fn fetch_keys(&self) -> Result<()> {
        let r = self
            .http
            .get(format!("{SESSION_SERVICE}/.well-known/jwks.json"))
            .send()
            .await?;
        let jwks: Jwks = check("jwks", r).await?.json().await?;
        let mut map = HashMap::new();
        for k in jwks.keys {
            if k.kty.as_deref() != Some("OKP") || k.crv.as_deref() != Some("Ed25519") {
                continue;
            }
            let (Some(kid), Some(x)) = (k.kid, k.x) else {
                continue;
            };
            let bytes: [u8; 32] = b64(&x)?
                .try_into()
                .map_err(|_| Error::BadToken("bad key length".into()))?;
            if let Ok(key) = VerifyingKey::from_bytes(&bytes) {
                map.insert(kid, key);
            }
        }
        *self.keys.write().await = map;
        Ok(())
    }

    async fn key(&self, kid: &str) -> Result<VerifyingKey> {
        if let Some(k) = self.keys.read().await.get(kid) {
            return Ok(*k);
        }
        // Unknown key id: the service may have rotated keys.
        self.fetch_keys().await?;
        self.keys
            .read()
            .await
            .get(kid)
            .copied()
            .ok_or_else(|| Error::BadToken(format!("unknown key {kid}")))
    }

    /// Checks signature, issuer and time window. Returns the claims.
    pub async fn verify(&self, token: &str) -> Result<Value> {
        let mut parts = token.split('.');
        let (Some(h), Some(p), Some(s), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Error::BadToken("not a JWT".into()));
        };
        let header: Value = serde_json::from_slice(&b64(h)?)?;
        if header["alg"] != "EdDSA" {
            return Err(Error::BadToken(format!("algorithm {}", header["alg"])));
        }
        let kid = header["kid"]
            .as_str()
            .ok_or(Error::BadToken("no kid".into()))?;
        let sig =
            Signature::from_slice(&b64(s)?).map_err(|_| Error::BadToken("bad signature".into()))?;
        self.key(kid)
            .await?
            .verify(format!("{h}.{p}").as_bytes(), &sig)
            .map_err(|_| Error::BadToken("signature does not verify".into()))?;

        let claims: Value = serde_json::from_slice(&b64(p)?)?;
        if claims["iss"] != SESSION_SERVICE {
            return Err(Error::BadToken(format!("issuer {}", claims["iss"])));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        if claims["exp"]
            .as_i64()
            .is_some_and(|exp| now > exp + CLOCK_SKEW)
        {
            return Err(Error::BadToken("expired".into()));
        }
        if claims["nbf"]
            .as_i64()
            .is_some_and(|nbf| now + CLOCK_SKEW < nbf)
        {
            return Err(Error::BadToken("not valid yet".into()));
        }
        Ok(claims)
    }
}

/// The claims of a player's identity token that the server uses.
#[derive(Debug, Clone)]
pub struct Identity {
    pub subject: String,
    pub username: String,
    pub skin: Option<String>,
    pub scopes: Vec<String>,
}

impl Identity {
    pub fn from_claims(c: &Value) -> Result<Self> {
        let subject = c["sub"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(Error::BadToken("no subject".into()))?;
        let username = c["profile"]["username"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(Error::BadToken("no username".into()))?;
        Ok(Self {
            subject: subject.to_owned(),
            username: username.to_owned(),
            skin: c["profile"]["skin"].as_str().map(str::to_owned),
            scopes: c["scope"]
                .as_str()
                .unwrap_or("")
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        })
    }

    pub fn has_scope(&self, s: &str) -> bool {
        self.scopes.iter().any(|x| x == s)
    }
}

/// Checks an access token's audience, subject, username and certificate binding.
pub fn check_access(
    c: &Value,
    audience: &str,
    identity: &Identity,
    client_cert_fingerprint: &str,
) -> Result<()> {
    let aud_ok = match &c["aud"] {
        Value::String(a) => a == audience,
        Value::Array(v) => v.first().and_then(Value::as_str) == Some(audience),
        _ => false,
    };
    if !aud_ok {
        return Err(Error::BadToken("wrong audience".into()));
    }
    if c["sub"].as_str() != Some(identity.subject.as_str()) {
        return Err(Error::BadToken(
            "subject differs from identity token".into(),
        ));
    }
    if c["username"].as_str() != Some(identity.username.as_str()) {
        return Err(Error::BadToken(
            "username differs from identity token".into(),
        ));
    }
    if c["cnf"]["x5t#S256"].as_str() != Some(client_cert_fingerprint) {
        return Err(Error::BadToken(
            "not bound to this connection's certificate".into(),
        ));
    }
    Ok(())
}
