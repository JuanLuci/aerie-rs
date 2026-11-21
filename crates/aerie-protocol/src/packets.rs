//! Packets of the login and setup (spec 03). Field order and limits follow the
//! layout tables in `docs/spec`.

use uuid::Uuid;

use crate::layout::{self, StructReader, StructWriter};
use crate::types::{self, AssetRef, BIG, HostAddress, Instant, Message};
use crate::{Error, Result, frame};

/// A packet the server sends.
pub trait Outgoing {
    const ID: u32;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()>;

    /// Serializes and frames the packet, appending to `wire`.
    fn write_frame(&self, wire: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::new();
        self.encode(&mut payload)?;
        frame::encode(Self::ID, &payload, wire)
    }
}

fn enum_u8<T>(field: &'static str, v: u8, f: impl Fn(u8) -> Option<T>) -> Result<T> {
    f(v).ok_or(Error::BadEnum {
        field,
        value: v as i64,
    })
}

// ================================================================ to server

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientType {
    Game,
    Editor,
}

/// Id 0. First packet on the game stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connect {
    pub protocol_crc: i32,
    pub protocol_build: i32,
    pub client_version: String,
    pub client_type: ClientType,
    pub identity_token: Option<String>,
    pub language: String,
    pub referral_data: Option<Vec<u8>>,
    pub referral_source: Option<HostAddress>,
}

impl Connect {
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let mut r = StructReader::new(buf, 1, 30, 4)?;
        let client_type = enum_u8("ClientType", r.u8(29), |v| match v {
            0 => Some(ClientType::Game),
            1 => Some(ClientType::Editor),
            _ => None,
        })?;
        let (has_token, has_ref_data, has_ref_src) = (r.bit(0), r.bit(1), r.bit(2));

        let identity_token = match r.var("IdentityToken", has_token)? {
            Some(b) => {
                let (s, n) = layout::var_string(b, "IdentityToken", 8192)?;
                r.advance(n);
                Some(s)
            }
            None => None,
        };
        let (language, n) = layout::var_ascii(r.var("Language", true)?.unwrap(), "Language", 100)?;
        r.advance(n);
        let referral_data = match r.var("ReferralData", has_ref_data)? {
            Some(b) => {
                let (d, n) = layout::var_bytes(b, "ReferralData", 4096)?;
                r.advance(n);
                Some(d.to_vec())
            }
            None => None,
        };
        let referral_source = match r.var("ReferralSource", has_ref_src)? {
            Some(b) => {
                let (h, n) = HostAddress::decode(b)?;
                r.advance(n);
                Some(h)
            }
            None => None,
        };
        Ok(Self {
            protocol_crc: r.i32(1),
            protocol_build: r.i32(5),
            client_version: r.fixed_ascii("ClientVersion", 9, 20)?,
            client_type,
            identity_token,
            language,
            referral_data,
            referral_source,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientDisconnectReason {
    PlayerLeave,
    PlayerAbort,
    UserLeave,
    Crash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectType {
    Disconnect = 0,
    Crash = 1,
}

/// Id 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientDisconnect {
    pub reason: ClientDisconnectReason,
    pub kind: DisconnectType,
}

impl ClientDisconnect {
    pub fn decode(buf: &[u8]) -> Result<Self> {
        layout::need(buf, 2)?;
        Ok(Self {
            reason: enum_u8("Reason", buf[0], |v| {
                use ClientDisconnectReason::*;
                [PlayerLeave, PlayerAbort, UserLeave, Crash]
                    .get(v as usize)
                    .copied()
            })?,
            kind: enum_u8("Type", buf[1], |v| {
                [DisconnectType::Disconnect, DisconnectType::Crash]
                    .get(v as usize)
                    .copied()
            })?,
        })
    }
}

/// Id 363. The player's identity in insecure and offline mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsecurePlayerOptions {
    pub uuid: Uuid,
    pub username: String,
    /// The skin, kept as the raw bytes of its struct (Aerie does not read it yet).
    pub skin_raw: Option<Vec<u8>>,
}

impl InsecurePlayerOptions {
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let mut r = StructReader::new(buf, 1, 17, 2)?;
        let has_skin = r.bit(0);
        let (username, n) = layout::var_ascii(r.var("Username", true)?.unwrap(), "Username", 16)?;
        r.advance(n);
        // The skin is the last field: whatever follows is its struct.
        let skin_raw = r.var("Skin", has_skin)?.map(|b| b.to_vec());
        Ok(Self {
            uuid: r.uuid(1),
            username,
            skin_raw,
        })
    }
}

/// Id 15. `SHA-256(challenge ‖ password)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordResponse {
    pub hash: Option<Vec<u8>>,
}

impl PasswordResponse {
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let mut r = StructReader::new(buf, 1, 1, 1)?;
        let has = r.bit(0);
        let hash = r
            .var("Hash", has)?
            .map(|b| layout::var_bytes(b, "Hash", 64).map(|(d, _)| d.to_vec()))
            .transpose()?;
        Ok(Self { hash })
    }
}

/// Id 23. The common assets the client does not have yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestAssets {
    pub assets: Option<Vec<AssetRef>>,
}

impl RequestAssets {
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let mut r = StructReader::new(buf, 1, 1, 1)?;
        let has = r.bit(0);
        let assets = r
            .var("Assets", has)?
            .map(|b| types::decode_assets(b).map(|(v, _)| v))
            .transpose()?;
        Ok(Self { assets })
    }
}

/// Id 32 (both ways). View distance in blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewRadius {
    pub blocks: i32,
}

impl ViewRadius {
    pub fn decode(buf: &[u8]) -> Result<Self> {
        layout::need(buf, 4)?;
        Ok(Self {
            blocks: i32::from_le_bytes(buf[0..4].try_into().unwrap()),
        })
    }
}

/// Everything the server can receive during login and setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToServer {
    Connect(Connect),
    ClientDisconnect(ClientDisconnect),
    InsecurePlayerOptions(InsecurePlayerOptions),
    PasswordResponse(PasswordResponse),
    RequestAssets(RequestAssets),
    ViewRadius(ViewRadius),
    SetupFinalize,
    AuthToken(AuthToken),
    /// Any packet Aerie does not read yet.
    Other {
        id: u32,
        payload: Vec<u8>,
    },
}

impl ToServer {
    pub fn decode(f: &frame::Frame) -> Result<Self> {
        let p = &f.payload;
        Ok(match f.id {
            0 => Self::Connect(Connect::decode(p)?),
            1 => Self::ClientDisconnect(ClientDisconnect::decode(p)?),
            12 => Self::AuthToken(AuthToken::decode(p)?),
            15 => Self::PasswordResponse(PasswordResponse::decode(p)?),
            23 => Self::RequestAssets(RequestAssets::decode(p)?),
            32 => Self::ViewRadius(ViewRadius::decode(p)?),
            33 => Self::SetupFinalize,
            363 => Self::InsecurePlayerOptions(InsecurePlayerOptions::decode(p)?),
            id => Self::Other {
                id,
                payload: p.clone(),
            },
        })
    }
}

/// Id 12. The client's access token and the grant the server exchanges for its own token.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthToken {
    pub access_token: Option<String>,
    pub server_authorization_grant: Option<String>,
}

impl std::fmt::Debug for AuthToken {
    /// Tokens stay out of logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthToken")
            .field("access_token", &self.access_token.as_ref().map(|t| t.len()))
            .field(
                "server_authorization_grant",
                &self.server_authorization_grant.as_ref().map(|t| t.len()),
            )
            .finish()
    }
}

impl AuthToken {
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let mut r = StructReader::new(buf, 1, 1, 2)?;
        let (a, b) = (r.bit(0), r.bit(1));
        let access_token = match r.var("AccessToken", a)? {
            Some(x) => {
                let (s, n) = layout::var_string(x, "AccessToken", 8192)?;
                r.advance(n);
                Some(s)
            }
            None => None,
        };
        let server_authorization_grant = match r.var("ServerAuthorizationGrant", b)? {
            Some(x) => {
                let (s, n) = layout::var_string(x, "ServerAuthorizationGrant", 4096)?;
                r.advance(n);
                Some(s)
            }
            None => None,
        };
        Ok(Self {
            access_token,
            server_authorization_grant,
        })
    }
}

// ================================================================ to client

/// Id 11. The grant the client exchanges, plus the server's identity token.
pub struct AuthGrant {
    pub authorization_grant: Option<String>,
    pub server_identity_token: Option<String>,
}

impl Outgoing for AuthGrant {
    const ID: u32 = 11;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        if self.authorization_grant.is_some() {
            w.null_bit(out, 0);
        }
        if self.server_identity_token.is_some() {
            w.null_bit(out, 1);
        }
        w.begin_var(out, 2);
        if w.var(out, self.authorization_grant.is_some()) {
            layout::put_var_string(
                out,
                "AuthorizationGrant",
                self.authorization_grant.as_deref().unwrap(),
                4096,
            )?;
        }
        if w.var(out, self.server_identity_token.is_some()) {
            layout::put_var_string(
                out,
                "ServerIdentityToken",
                self.server_identity_token.as_deref().unwrap(),
                8192,
            )?;
        }
        Ok(())
    }
}

/// Id 13. The server's access token, and a password challenge when the server has one.
pub struct ServerAuthToken {
    pub server_access_token: Option<String>,
    pub password_challenge: Option<Vec<u8>>,
}

impl Outgoing for ServerAuthToken {
    const ID: u32 = 13;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        if self.server_access_token.is_some() {
            w.null_bit(out, 0);
        }
        if self.password_challenge.is_some() {
            w.null_bit(out, 1);
        }
        w.begin_var(out, 2);
        if w.var(out, self.server_access_token.is_some()) {
            layout::put_var_string(
                out,
                "ServerAccessToken",
                self.server_access_token.as_deref().unwrap(),
                8192,
            )?;
        }
        if w.var(out, self.password_challenge.is_some()) {
            layout::put_var_bytes(
                out,
                "PasswordChallenge",
                self.password_challenge.as_ref().unwrap(),
                64,
            )?;
        }
        Ok(())
    }
}

/// Id 2. Sent before closing the connection.
pub struct ServerDisconnect {
    pub reason: Option<Message>,
    pub kind: DisconnectType,
}

impl Outgoing for ServerDisconnect {
    const ID: u32 = 2;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        if self.reason.is_some() {
            w.null_bit(out, 0);
        }
        layout::put_u8(out, self.kind as u8);
        w.begin_var(out, 1);
        if w.var(out, self.reason.is_some()) {
            self.reason.as_ref().unwrap().encode(out)?;
        }
        Ok(())
    }
}

/// Id 3. Latency probe; the client answers with `Pong` (4).
pub struct Ping {
    pub id: i32,
    pub time: Instant,
    pub last_raw: i32,
    pub last_direct: i32,
    pub last_tick: i32,
}

impl Outgoing for Ping {
    const ID: u32 = 3;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        layout::put_i32(out, self.id);
        self.time.encode(out);
        layout::put_i32(out, self.last_raw);
        layout::put_i32(out, self.last_direct);
        layout::put_i32(out, self.last_tick);
        Ok(())
    }
}

/// Id 14. Login accepted; carries a password challenge when the server has one.
pub struct ConnectAccept {
    pub password_challenge: Option<Vec<u8>>,
}

impl Outgoing for ConnectAccept {
    const ID: u32 = 14;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        encode_optional_bytes(out, &self.password_challenge, "PasswordChallenge", 64, &[])
    }
}

/// Id 16.
pub struct PasswordAccepted;

impl Outgoing for PasswordAccepted {
    const ID: u32 = 16;
    fn encode(&self, _: &mut Vec<u8>) -> Result<()> {
        Ok(())
    }
}

/// Id 17. Wrong password: a fresh challenge and the attempts left.
pub struct PasswordRejected {
    pub new_challenge: Option<Vec<u8>>,
    pub attempts_remaining: i32,
}

impl Outgoing for PasswordRejected {
    const ID: u32 = 17;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        encode_optional_bytes(
            out,
            &self.new_challenge,
            "NewChallenge",
            64,
            &self.attempts_remaining.to_le_bytes(),
        )
    }
}

/// One nullable bit, some fixed bytes, then one optional byte array.
fn encode_optional_bytes(
    out: &mut Vec<u8>,
    v: &Option<Vec<u8>>,
    field: &'static str,
    max: usize,
    fixed: &[u8],
) -> Result<()> {
    let mut w = StructWriter::new(out, 1);
    if v.is_some() {
        w.null_bit(out, 0);
    }
    out.extend_from_slice(fixed);
    w.begin_var(out, 1);
    if w.var(out, v.is_some()) {
        layout::put_var_bytes(out, field, v.as_ref().unwrap(), max)?;
    }
    Ok(())
}

/// Id 20 (zstd). First packet of setup: the common assets the world needs.
pub struct WorldSettings {
    pub required_assets: Option<Vec<AssetRef>>,
}

impl Outgoing for WorldSettings {
    const ID: u32 = 20;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        if self.required_assets.is_some() {
            w.null_bit(out, 0);
        }
        w.begin_var(out, 1);
        if w.var(out, self.required_assets.is_some()) {
            types::encode_assets(out, self.required_assets.as_ref().unwrap())?;
        }
        Ok(())
    }
}

/// Id 21. Loading screen progress.
pub struct WorldLoadProgress {
    pub status: Option<Message>,
    pub percent: i32,
    pub percent_subitem: i32,
}

impl Outgoing for WorldLoadProgress {
    const ID: u32 = 21;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        if self.status.is_some() {
            w.null_bit(out, 0);
        }
        layout::put_i32(out, self.percent);
        layout::put_i32(out, self.percent_subitem);
        w.begin_var(out, 1);
        if w.var(out, self.status.is_some()) {
            self.status.as_ref().unwrap().encode(out)?;
        }
        Ok(())
    }
}

/// Id 22.
pub struct WorldLoadFinished;

impl Outgoing for WorldLoadFinished {
    const ID: u32 = 22;
    fn encode(&self, _: &mut Vec<u8>) -> Result<()> {
        Ok(())
    }
}

/// Id 223. Name, MOTD, player limit and fallback server.
pub struct ServerInfo {
    pub name: Option<String>,
    pub motd: Option<String>,
    pub max_players: i32,
    pub fallback: Option<HostAddress>,
}

impl Outgoing for ServerInfo {
    const ID: u32 = 223;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        for (bit, set) in [
            self.name.is_some(),
            self.motd.is_some(),
            self.fallback.is_some(),
        ]
        .into_iter()
        .enumerate()
        {
            if set {
                w.null_bit(out, bit);
            }
        }
        layout::put_i32(out, self.max_players);
        w.begin_var(out, 3);
        if w.var(out, self.name.is_some()) {
            layout::put_var_string(out, "ServerName", self.name.as_deref().unwrap(), BIG)?;
        }
        if w.var(out, self.motd.is_some()) {
            layout::put_var_string(out, "Motd", self.motd.as_deref().unwrap(), BIG)?;
        }
        if w.var(out, self.fallback.is_some()) {
            self.fallback.as_ref().unwrap().encode(out)?;
        }
        Ok(())
    }
}

/// Id 364. Asks the client for its identity (insecure and offline mode).
pub struct RequestInsecurePlayerOptions;

impl Outgoing for RequestInsecurePlayerOptions {
    const ID: u32 = 364;
    fn encode(&self, _: &mut Vec<u8>) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a Connect by hand, byte by byte, following the table in spec 02.
    fn connect_bytes(token: Option<&str>) -> Vec<u8> {
        let mut b = vec![if token.is_some() { 1 } else { 0 }];
        b.extend_from_slice(&crate::version::PROTOCOL_CRC.to_le_bytes());
        b.extend_from_slice(&258i32.to_le_bytes());
        let mut ver = b"0.7.7".to_vec();
        ver.resize(20, 0);
        b.extend_from_slice(&ver);
        b.push(0); // Game
        let mut var = Vec::new();
        let token_slot = match token {
            Some(t) => {
                var.push(t.len() as u8);
                var.extend_from_slice(t.as_bytes());
                0
            }
            None => -1,
        };
        let lang_slot = var.len() as i32;
        var.push(5);
        var.extend_from_slice(b"pt-BR");
        for slot in [token_slot, lang_slot, -1, -1] {
            b.extend_from_slice(&slot.to_le_bytes());
        }
        b.extend_from_slice(&var);
        b
    }

    #[test]
    fn decode_connect() {
        let c = Connect::decode(&connect_bytes(Some("tok"))).unwrap();
        assert_eq!(c.protocol_crc, crate::version::PROTOCOL_CRC);
        assert_eq!(c.client_version, "0.7.7");
        assert_eq!(c.identity_token.as_deref(), Some("tok"));
        assert_eq!(c.language, "pt-BR");
        assert_eq!(c.referral_source, None);

        let c = Connect::decode(&connect_bytes(None)).unwrap();
        assert_eq!(c.identity_token, None);
        assert_eq!(c.language, "pt-BR");
    }

    #[test]
    fn connect_rejects_non_canonical_offsets() {
        let mut b = connect_bytes(None);
        b[34] = 1; // language slot no longer points at the start of the block
        assert!(matches!(
            Connect::decode(&b),
            Err(Error::NonCanonical { .. })
        ));
    }

    #[test]
    fn decode_insecure_options() {
        let id = Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").unwrap();
        let mut b = vec![0];
        b.extend_from_slice(id.as_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&(-1i32).to_le_bytes());
        b.push(4);
        b.extend_from_slice(b"Joao");
        let p = InsecurePlayerOptions::decode(&b).unwrap();
        assert_eq!(p.uuid, id);
        assert_eq!(p.username, "Joao");
        assert_eq!(p.skin_raw, None);
    }

    #[test]
    fn encode_server_info_layout() {
        let mut out = Vec::new();
        ServerInfo {
            name: Some("Aerie".into()),
            motd: None,
            max_players: 8,
            fallback: None,
        }
        .encode(&mut out)
        .unwrap();
        assert_eq!(out[0], 0b001);
        assert_eq!(&out[1..5], &8i32.to_le_bytes());
        assert_eq!(&out[5..9], &0i32.to_le_bytes()); // name at the start of the block
        assert_eq!(&out[9..13], &(-1i32).to_le_bytes());
        assert_eq!(&out[13..17], &(-1i32).to_le_bytes());
        assert_eq!(&out[17..], b"\x05Aerie");
    }

    #[test]
    fn encode_message_text() {
        let mut out = Vec::new();
        Message::text("hi").encode(&mut out).unwrap();
        assert_eq!(out[0], 1 << 5);
        assert_eq!(out[1], 0);
        assert_eq!(out.len(), 40 + 3);
        assert_eq!(&out[8..12], &0i32.to_le_bytes());
        assert_eq!(&out[12..16], &(-1i32).to_le_bytes());
        assert_eq!(&out[40..], b"\x02hi");
    }

    #[test]
    fn asset_list_round_trip() {
        let assets = vec![AssetRef {
            hash: "a".repeat(64),
            name: "Common/x.png".into(),
        }];
        let mut out = Vec::new();
        WorldSettings {
            required_assets: Some(assets.clone()),
        }
        .encode(&mut out)
        .unwrap();
        assert_eq!(RequestAssets::decode(&out).unwrap().assets, Some(assets));
    }
}
