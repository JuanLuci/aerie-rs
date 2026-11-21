//! The packet table, generated at build time from `gen/packets.tsv`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    ToServer,
    ToClient,
    Both,
}

impl Direction {
    pub fn to_server(self) -> bool {
        matches!(self, Direction::ToServer | Direction::Both)
    }
    pub fn to_client(self) -> bool {
        matches!(self, Direction::ToClient | Direction::Both)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Channel {
    Default = 0,
    Chunks = 1,
    WorldMap = 2,
    Voice = 3,
}

#[derive(Clone, Copy, Debug)]
pub struct PacketInfo {
    pub id: u32,
    pub name: &'static str,
    pub direction: Direction,
    pub channel: Channel,
    /// Size of the fixed part (nullable bits + fixed fields), in bytes.
    pub fixed_size: u32,
    /// Largest payload accepted, in bytes (after decompression).
    pub max_size: u32,
    /// The payload is always one Zstandard frame.
    pub compressed: bool,
}

include!(concat!(env!("OUT_DIR"), "/packets.rs"));

/// Looks a packet up by id. The table is sorted by id.
pub fn info(id: u32) -> Option<&'static PacketInfo> {
    PACKETS
        .binary_search_by_key(&id, |p| p.id)
        .ok()
        .map(|i| &PACKETS[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_complete() {
        assert!(PACKETS.windows(2).all(|w| w[0].id < w[1].id));
        assert_eq!(PACKETS.len(), 389);
        let connect = info(0).unwrap();
        assert_eq!(connect.name, "Connect");
        assert!(connect.direction.to_server() && !connect.direction.to_client());
        assert!(info(20).unwrap().compressed);
    }
}
