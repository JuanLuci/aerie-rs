//! Packets a client sends, for Aerie's probe and, later, the Aerie client.
//! Layouts are the same ones the server side decodes (spec 02, 03).

use uuid::Uuid;

use crate::layout::{self, StructWriter};
use crate::packets::{ClientType, Outgoing};
use crate::types::{self, AssetRef};
use crate::{Result, version};

/// Id 0.
pub struct ConnectOut {
    pub client_version: String,
    pub client_type: ClientType,
    pub identity_token: Option<String>,
    pub language: String,
}

impl Outgoing for ConnectOut {
    const ID: u32 = 0;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        if self.identity_token.is_some() {
            w.null_bit(out, 0);
        }
        layout::put_i32(out, version::PROTOCOL_CRC);
        layout::put_i32(out, version::PROTOCOL_BUILD);
        layout::put_fixed_ascii(out, &self.client_version, 20);
        layout::put_u8(out, self.client_type as u8);
        w.begin_var(out, 4);
        if w.var(out, self.identity_token.is_some()) {
            layout::put_var_string(
                out,
                "IdentityToken",
                self.identity_token.as_deref().unwrap(),
                8192,
            )?;
        }
        w.var(out, true);
        layout::put_var_string(out, "Language", &self.language, 100)?;
        w.var(out, false); // referral data
        w.var(out, false); // referral source
        Ok(())
    }
}

/// Id 363. Insecure-mode identity; no skin.
pub struct InsecurePlayerOptionsOut {
    pub uuid: Uuid,
    pub username: String,
}

impl Outgoing for InsecurePlayerOptionsOut {
    const ID: u32 = 363;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        layout::put_uuid(out, self.uuid);
        w.begin_var(out, 2);
        w.var(out, true);
        layout::put_var_string(out, "Username", &self.username, 16)?;
        w.var(out, false);
        Ok(())
    }
}

/// Id 23.
pub struct RequestAssetsOut {
    pub assets: Vec<AssetRef>,
}

impl Outgoing for RequestAssetsOut {
    const ID: u32 = 23;
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 1);
        w.null_bit(out, 0);
        w.begin_var(out, 1);
        w.var(out, true);
        types::encode_assets(out, &self.assets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::{Connect, InsecurePlayerOptions, RequestAssets};

    #[test]
    fn client_packets_decode_back() {
        let mut b = Vec::new();
        ConnectOut {
            client_version: "0.7.7".into(),
            client_type: ClientType::Game,
            identity_token: None,
            language: "en".into(),
        }
        .encode(&mut b)
        .unwrap();
        let c = Connect::decode(&b).unwrap();
        assert_eq!(
            (c.client_version.as_str(), c.language.as_str()),
            ("0.7.7", "en")
        );

        let mut b = Vec::new();
        InsecurePlayerOptionsOut {
            uuid: Uuid::nil(),
            username: "probe".into(),
        }
        .encode(&mut b)
        .unwrap();
        assert_eq!(InsecurePlayerOptions::decode(&b).unwrap().username, "probe");

        let mut b = Vec::new();
        RequestAssetsOut { assets: vec![] }.encode(&mut b).unwrap();
        assert_eq!(RequestAssets::decode(&b).unwrap().assets, Some(vec![]));
    }
}
