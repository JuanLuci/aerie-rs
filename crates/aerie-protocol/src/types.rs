//! Nested types used by the login and setup packets.

use crate::Result;
use crate::layout::{self, StructReader, StructWriter};

/// Limit the server applies to most strings.
pub const BIG: usize = 4_096_000;

/// Rich text shown by the client. Aerie writes the two plain forms: literal
/// text, or a translation key the client resolves in its own language.
///
/// Layout: 2 bytes of nullable bits, 6 bools, 8 variable fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub raw_text: Option<String>,
    pub translation_key: Option<String>,
}

impl Message {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            raw_text: Some(s.into()),
            translation_key: None,
        }
    }
    pub fn key(k: impl Into<String>) -> Self {
        Self {
            raw_text: None,
            translation_key: Some(k.into()),
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 2);
        // Bits, first byte: bold 0, italic 1, monospace 2, underlined 3,
        // strikethrough 4, raw text 5, translation key 6, children 7.
        // Second byte: params 8, message params 9, color 10, link 11, image 12.
        if self.raw_text.is_some() {
            w.null_bit(out, 5);
        }
        if self.translation_key.is_some() {
            w.null_bit(out, 6);
        }
        // bold, italic, monospace, underlined, strikethrough (absent = 0), markup enabled.
        out.extend_from_slice(&[0; 6]);
        w.begin_var(out, 8);
        if w.var(out, self.raw_text.is_some()) {
            layout::put_var_string(out, "RawText", self.raw_text.as_deref().unwrap(), BIG)?;
        }
        if w.var(out, self.translation_key.is_some()) {
            layout::put_var_string(
                out,
                "MessageId",
                self.translation_key.as_deref().unwrap(),
                BIG,
            )?;
        }
        for _ in 0..6 {
            w.var(out, false); // children, params, message params, color, link, image
        }
        Ok(())
    }
}

/// A host and port, as used for referrals and the fallback server.
///
/// Layout: port (i16) then the host string (≤ 256 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostAddress {
    pub host: String,
    pub port: u16,
}

impl HostAddress {
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 0);
        layout::put_i16(out, self.port as i16);
        w.begin_var(out, 1);
        w.var(out, true);
        layout::put_var_string(out, "Host", &self.host, 256)
    }

    pub fn decode(buf: &[u8]) -> Result<(Self, usize)> {
        let mut r = StructReader::new(buf, 0, 2, 1)?;
        let port = r.i16(0) as u16;
        let (host, n) = layout::var_string(r.var("Host", true)?.unwrap(), "Host", 256)?;
        r.advance(n);
        Ok((Self { host, port }, r.end()))
    }
}

/// A common asset the client may need to download: content hash and path.
///
/// Layout: hash (64 ASCII bytes, fixed) then the name (≤ 512 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRef {
    pub hash: String,
    pub name: String,
}

impl AssetRef {
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let mut w = StructWriter::new(out, 0);
        layout::put_fixed_ascii(out, &self.hash, 64);
        w.begin_var(out, 1);
        w.var(out, true);
        layout::put_var_string(out, "Name", &self.name, 512)
    }

    pub fn decode(buf: &[u8]) -> Result<(Self, usize)> {
        let mut r = StructReader::new(buf, 0, 64, 1)?;
        let hash = r.fixed_ascii("Hash", 0, 64)?;
        let (name, n) = layout::var_string(r.var("Name", true)?.unwrap(), "Name", 512)?;
        r.advance(n);
        Ok((Self { hash, name }, r.end()))
    }
}

/// A list of [`AssetRef`]s: varint count, then each one in turn.
pub fn encode_assets(out: &mut Vec<u8>, assets: &[AssetRef]) -> Result<()> {
    crate::varint::write(out, assets.len() as i32)?;
    for a in assets {
        a.encode(out)?;
    }
    Ok(())
}

pub fn decode_assets(buf: &[u8]) -> Result<(Vec<AssetRef>, usize)> {
    let (count, mut used) = crate::varint::read(buf)?;
    let count = count as usize;
    if count > BIG {
        return Err(crate::Error::FieldTooLong {
            field: "Assets",
            len: count,
            max: BIG,
        });
    }
    let mut v = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let (a, n) = AssetRef::decode(&buf[used..])?;
        used += n;
        v.push(a);
    }
    Ok((v, used))
}

/// Seconds and nanoseconds, 12 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Instant {
    pub seconds: i64,
    pub nanos: i32,
}

impl Instant {
    pub fn encode(&self, out: &mut Vec<u8>) {
        layout::put_i64(out, self.seconds);
        layout::put_i32(out, self.nanos);
    }
}
