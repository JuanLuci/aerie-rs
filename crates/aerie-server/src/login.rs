//! One player's connection through login and setup (spec 03).
//!
//! Authenticated when the server has an account and the client sent an
//! identity token; insecure otherwise (development clients only).

use std::{sync::Arc, time::Duration};

use aerie_auth::account as session_service;
use aerie_auth::jwt::{self, Identity};
use aerie_protocol::packets::*;
use aerie_protocol::types::Message;
use aerie_protocol::{Frame, FrameDecoder, version};
use anyhow::{Context, Result, bail};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use uuid::Uuid;

use crate::account::ServerAccount;
use crate::tls;

/// QUIC application close codes (spec 01, "Closing").
mod close {
    pub const NO_ERROR: u32 = 0;
    pub const AUTH_FAILED: u32 = 2;
    pub const CLIENT_OUTDATED: u32 = 5;
    pub const SERVER_OUTDATED: u32 = 6;
}

pub struct Settings {
    pub name: String,
    pub motd: String,
    pub max_players: i32,
    /// Chunks (spec 25); the client is told this times 32 blocks.
    pub max_view_radius: i32,
    pub password: Option<String>,
    pub account: Option<Arc<ServerAccount>>,
    /// Registry packets built from the player's `Assets.zip` (spec 07).
    pub registries: Vec<(u32, Vec<u8>)>,
    /// Test only: setup packets captured by `probe`. Registries Aerie cannot
    /// build yet are taken from here.
    pub replay: Option<Vec<(u32, Vec<u8>)>>,
    /// Everyone in the world.
    pub hub: crate::world::Hub,
    /// The common assets for `WorldSettings` (path and SHA-256), when built.
    pub common_assets: Option<Vec<aerie_protocol::types::AssetRef>>,
    /// `--built-world`: Aerie's world even with `--replay-setup` (whose
    /// player packets it then borrows).
    pub built_world: bool,
    /// What the player's own entity is built from (built world).
    pub player: crate::join::PlayerParts,
    /// `--captured-player IDS` (test only, with `--built-world`): the built
    /// player plus only these captured player packets.
    pub captured_player: Option<String>,
    /// `--record DIR`: save samples of what clients send in the world.
    pub record: Option<std::path::PathBuf>,
    /// `--flat`: the flat test world instead of generated terrain.
    pub flat: bool,
    /// `--seed N`: the terrain's seed.
    pub seed: u64,
}

/// Loads a `probe` capture folder (`index.tsv` plus one file per packet).
pub fn load_capture(dir: &std::path::Path) -> Result<Vec<(u32, Vec<u8>)>> {
    let index = std::fs::read_to_string(dir.join("index.tsv")).context("index.tsv")?;
    let mut out = Vec::new();
    for line in index.lines() {
        let c: Vec<&str> = line.split('\t').collect();
        let (seq, id, name) = (c[0].parse::<u32>()?, c[1].parse::<u32>()?, c[2]);
        out.push((
            id,
            std::fs::read(dir.join(format!("{seq:04}-{id}-{name}.bin")))?,
        ));
    }
    Ok(out)
}

/// The game stream: frames in, packets out.
pub(crate) struct Game {
    pub send: quinn::SendStream,
    recv: quinn::RecvStream,
    dec: FrameDecoder,
    buf: Vec<u8>,
}

impl Game {
    pub async fn next(&mut self) -> Result<Frame> {
        loop {
            if let Some(f) = self.dec.next()? {
                return Ok(f);
            }
            let n = self
                .recv
                .read(&mut self.buf)
                .await?
                .context("client closed the game stream")?;
            self.dec.push(&self.buf[..n]);
        }
    }

    async fn next_packet(&mut self) -> Result<ToServer> {
        let f = self.next().await?;
        let p = ToServer::decode(&f)?;
        match &p {
            ToServer::Other { id, payload } => {
                info!(
                    "← {id} {} ({} bytes)",
                    aerie_protocol::table::info(*id).map_or("?", |i| i.name),
                    payload.len()
                )
            }
            // Never log the identity token.
            ToServer::Connect(c) => info!(
                "← Connect: client {} build {} type {:?}, language {}, identity token: {}",
                c.client_version,
                c.protocol_build,
                c.client_type,
                c.language,
                if c.identity_token.is_some() {
                    "yes"
                } else {
                    "no"
                }
            ),
            p => info!("← {p:?}"),
        }
        Ok(p)
    }

    /// Sends an already serialized packet.
    pub async fn send_raw(&mut self, id: u32, payload: &[u8]) -> Result<()> {
        let mut wire = Vec::new();
        aerie_protocol::frame::encode(id, payload, &mut wire)?;
        self.send.write_all(&wire).await?;
        Ok(())
    }

    async fn send<P: Outgoing>(&mut self, p: &P) -> Result<()> {
        let mut wire = Vec::new();
        p.write_frame(&mut wire)?;
        info!(
            "→ {} {} ({} bytes)",
            P::ID,
            aerie_protocol::table::info(P::ID).map_or("?", |i| i.name),
            wire.len() - 8
        );
        self.send.write_all(&wire).await?;
        Ok(())
    }

    async fn kick(&mut self, conn: &quinn::Connection, msg: Message, code: u32) -> Result<()> {
        self.send(&ServerDisconnect {
            reason: Some(msg),
            kind: DisconnectType::Disconnect,
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        conn.close(code.into(), b"");
        Ok(())
    }
}

/// What every player is told when the server stops (spec 25): the
/// disconnect packet, ready to be queued.
pub fn stopping_message() -> (u32, Vec<u8>) {
    let mut out = Vec::new();
    ServerDisconnect {
        reason: Some(Message::key("server.general.disconnect.stoppingServer")),
        kind: DisconnectType::Disconnect,
    }
    .encode(&mut out)
    .expect("a disconnect packet encodes");
    (ServerDisconnect::ID, out)
}

/// Who joined, after either login path.
pub(crate) struct Player {
    pub uuid: Uuid,
    pub username: String,
}

fn new_challenge(settings: &Settings) -> Option<Vec<u8>> {
    settings.password.as_ref().map(|_| {
        let mut c = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut c);
        c
    })
}

pub async fn handle(conn: quinn::Connection, settings: &Settings) -> Result<()> {
    let alpn = tls::negotiated_alpn(&conn);
    info!(
        "{} connected, ALPN {:?}",
        conn.remote_address(),
        alpn.as_deref().map(String::from_utf8_lossy)
    );

    let (send, recv) = conn.accept_bi().await?;
    let mut g = Game {
        send,
        recv,
        dec: FrameDecoder::new(true),
        buf: vec![0; 64 * 1024],
    };

    if alpn.as_deref() != Some(version::ALPN) {
        return g
            .kick(
                &conn,
                Message::key("client.general.disconnect.clientOutdated"),
                close::CLIENT_OUTDATED,
            )
            .await;
    }

    // ---- version check
    let connect = match g.next_packet().await? {
        ToServer::Connect(c) => c,
        other => {
            conn.close(close::NO_ERROR.into(), b"");
            bail!("expected Connect, got {other:?}");
        }
    };
    if connect.protocol_crc != version::PROTOCOL_CRC {
        let (key, code) = if connect.protocol_build < version::PROTOCOL_BUILD {
            (
                "client.general.disconnect.clientOutdated",
                close::CLIENT_OUTDATED,
            )
        } else {
            (
                "client.general.disconnect.serverOutdated",
                close::SERVER_OUTDATED,
            )
        };
        warn!(
            "protocol mismatch: client build {} crc {:#x}",
            connect.protocol_build, connect.protocol_crc
        );
        return g.kick(&conn, Message::key(key), code).await;
    }

    // ---- server full (spec 25)
    if settings.max_players > 0 && settings.hub.player_count() >= settings.max_players as usize {
        info!(
            "refusing a join: {} players, the most allowed",
            settings.max_players
        );
        return g
            .kick(
                &conn,
                Message::key("client.general.disconnect.serverFull"),
                close::NO_ERROR,
            )
            .await;
    }

    // ---- login
    let challenge = new_challenge(settings);
    let player = match (&settings.account, &connect.identity_token) {
        (Some(account), Some(token)) => {
            match authenticated(
                &mut g,
                &conn,
                account,
                token,
                connect.client_type,
                challenge.clone(),
            )
            .await
            {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(()),
                Err(e) => {
                    warn!("authentication failed: {e:#}");
                    return g
                        .kick(
                            &conn,
                            Message::key("client.general.disconnect.authError"),
                            close::AUTH_FAILED,
                        )
                        .await;
                }
            }
        }
        (None, _) | (_, None) => {
            if settings.account.is_none() {
                info!("no server account: offering the insecure login (release clients refuse it)");
            }
            match insecure(&mut g, &conn, challenge.clone()).await? {
                Some(p) => p,
                None => return Ok(()),
            }
        }
    };
    info!("player {} ({})", player.username, player.uuid);

    if let (Some(challenge), Some(password)) = (challenge, &settings.password) {
        if !password_exchange(&mut g, &conn, challenge, password).await? {
            return Ok(());
        }
    }

    setup(&mut g, &conn, settings, &player).await
}

/// Spec 03, "Insecure / offline login". `None` when the client left.
async fn insecure(
    g: &mut Game,
    conn: &quinn::Connection,
    challenge: Option<Vec<u8>>,
) -> Result<Option<Player>> {
    g.send(&RequestInsecurePlayerOptions).await?;
    let p = loop {
        match g.next_packet().await? {
            ToServer::InsecurePlayerOptions(p) => break p,
            ToServer::ClientDisconnect(_) => return Ok(None),
            other => bail!("expected InsecurePlayerOptions, got {other:?}"),
        }
    };
    if p.username.is_empty() {
        g.kick(
            conn,
            Message::key("client.general.disconnect.missingUsername"),
            close::NO_ERROR,
        )
        .await?;
        return Ok(None);
    }
    g.send(&ConnectAccept {
        password_challenge: challenge,
    })
    .await?;
    Ok(Some(Player {
        uuid: p.uuid,
        username: p.username,
    }))
}

/// Spec 03, "Authenticated login". `None` when the client left.
async fn authenticated(
    g: &mut Game,
    conn: &quinn::Connection,
    account: &ServerAccount,
    identity_token: &str,
    client_type: ClientType,
    challenge: Option<Vec<u8>>,
) -> Result<Option<Player>> {
    let claims = account
        .verifier
        .verify(identity_token)
        .await
        .context("identity token")?;
    let identity = Identity::from_claims(&claims)?;
    let scope = if client_type == ClientType::Editor {
        "hytale:editor"
    } else {
        "hytale:client"
    };
    if !identity.has_scope(scope) {
        bail!("identity token lacks scope {scope}");
    }
    info!(
        "identity token ok for {} ({})",
        identity.username, identity.subject
    );

    let session = account.session().await;
    let grant =
        session_service::auth_grant(&account.http, &session, identity_token, &account.audience)
            .await?;
    g.send(&AuthGrant {
        authorization_grant: Some(grant),
        server_identity_token: Some(session.identity_token.clone()),
    })
    .await?;

    let reply = loop {
        match g.next_packet().await? {
            ToServer::AuthToken(t) => break t,
            ToServer::ClientDisconnect(_) => return Ok(None),
            other => bail!("expected AuthToken, got {other:?}"),
        }
    };
    let access = reply
        .access_token
        .filter(|t| !t.is_empty())
        .context("no access token")?;
    let client_cert = tls::peer_cert_der(conn).context("no client certificate")?;
    let access_claims = account
        .verifier
        .verify(&access)
        .await
        .context("access token")?;
    jwt::check_access(
        &access_claims,
        &account.audience,
        &identity,
        &aerie_auth::cert_fingerprint(&client_cert),
    )?;

    let Some(server_grant) = reply.server_authorization_grant.filter(|t| !t.is_empty()) else {
        g.kick(
            conn,
            Message::key("client.general.disconnect.mutualAuthRequired"),
            close::AUTH_FAILED,
        )
        .await?;
        return Ok(None);
    };
    let server_access = session_service::auth_token(
        &account.http,
        &session,
        &server_grant,
        &account.cert_fingerprint,
    )
    .await?;
    g.send(&ServerAuthToken {
        server_access_token: Some(server_access),
        password_challenge: challenge,
    })
    .await?;

    let uuid = Uuid::parse_str(&identity.subject).context("subject is not a UUID")?;
    Ok(Some(Player {
        uuid,
        username: identity.username,
    }))
}

/// `false` when the player failed or left.
async fn password_exchange(
    g: &mut Game,
    conn: &quinn::Connection,
    mut challenge: Vec<u8>,
    password: &str,
) -> Result<bool> {
    let mut attempts = 3;
    loop {
        let hash = match g.next_packet().await? {
            ToServer::PasswordResponse(r) => r.hash.unwrap_or_default(),
            ToServer::ClientDisconnect(_) => return Ok(false),
            other => bail!("expected PasswordResponse, got {other:?}"),
        };
        let want = Sha256::new()
            .chain_update(&challenge)
            .chain_update(password.as_bytes())
            .finalize();
        if hash == want.as_slice() {
            g.send(&PasswordAccepted).await?;
            return Ok(true);
        }
        attempts -= 1;
        if attempts == 0 {
            g.kick(
                conn,
                Message::key("client.general.disconnect.tooManyPasswordAttempts"),
                close::NO_ERROR,
            )
            .await?;
            return Ok(false);
        }
        rand::thread_rng().fill_bytes(&mut challenge);
        g.send(&PasswordRejected {
            new_challenge: Some(challenge.clone()),
            attempts_remaining: attempts,
        })
        .await?;
    }
}

/// Spec 03, "Setup".
async fn setup(
    g: &mut Game,
    conn: &quinn::Connection,
    settings: &Settings,
    player: &Player,
) -> Result<()> {
    let replay = settings.replay.as_deref().unwrap_or(&[]);
    match replay.iter().find(|(id, _)| *id == WorldSettings::ID) {
        Some((id, payload)) => g.send_raw(*id, payload).await?,
        None => {
            g.send(&WorldSettings {
                required_assets: Some(settings.common_assets.clone().unwrap_or_default()),
            })
            .await?
        }
    }
    g.send(&ServerInfo {
        name: Some(settings.name.clone()),
        motd: Some(settings.motd.clone()),
        max_players: settings.max_players,
        fallback: None,
    })
    .await?;

    loop {
        match g.next_packet().await? {
            ToServer::RequestAssets(_) => break,
            ToServer::ViewRadius(_) => {}
            ToServer::ClientDisconnect(_) => return Ok(()),
            other => bail!("expected RequestAssets, got {other:?}"),
        }
    }
    // Registries built from the game's assets (spec 07); a test run fills the
    // ones Aerie cannot build yet from a local capture of another server.
    for (id, payload) in &settings.registries {
        g.send_raw(*id, payload).await?;
    }
    let missing: Vec<_> = replay
        .iter()
        .filter(|(id, _)| {
            (40..=99).contains(id) && !settings.registries.iter().any(|(b, _)| b == id)
        })
        .collect();
    if !missing.is_empty() {
        info!(
            "sent {} built registries; replaying {} captured ones (test only)",
            settings.registries.len(),
            missing.len()
        );
    }
    for (id, payload) in missing {
        g.send_raw(*id, payload).await?;
    }
    g.send(&WorldLoadProgress {
        status: Some(Message::key("client.general.worldLoad.loadingWorld")),
        percent: 0,
        percent_subitem: 0,
    })
    .await?;
    g.send(&WorldLoadFinished).await?;

    loop {
        match g.next_packet().await? {
            ToServer::SetupFinalize => break,
            ToServer::ViewRadius(_) | ToServer::Other { .. } => {}
            ToServer::ClientDisconnect(_) => return Ok(()),
            other => bail!("expected SetupFinalize, got {other:?}"),
        }
    }
    info!("setup finished for {}", player.username);
    crate::join::run(g, conn, settings, player).await
}
