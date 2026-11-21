//! Aerie's implementation of the wire format described in `docs/spec`.
//!
//! - [`frame`]: length + id framing and per-packet Zstandard compression (spec 02).
//! - [`layout`]: the struct layout every packet and nested type uses (spec 02).
//! - [`packets`]: the packets Aerie speaks so far (spec 03).

pub mod client;
pub mod error;
pub mod frame;
pub mod layout;
pub mod packets;
pub mod proto;
pub mod table;
pub mod types;
pub mod varint;
pub mod wire;

pub use error::{Error, Result};
pub use frame::{Frame, FrameDecoder};
pub use table::{Channel, Direction, PacketInfo};

/// Protocol constants of the build Aerie targets (spec 03, "Version check").
pub mod version {
    /// CRC the client sends in `Connect`.
    pub const PROTOCOL_CRC: i32 = 0xE8F5_8B91_u32 as i32;
    /// Build number the client sends in `Connect`.
    pub const PROTOCOL_BUILD: i32 = 258;
    /// ALPN the server must negotiate.
    pub const ALPN: &[u8] = b"hytale/3";
    /// Default UDP port.
    pub const DEFAULT_PORT: u16 = 5520;
}
