//! Frames: `u32 LE length`, `u32 LE id`, payload (spec 02, "Frames").

use bytes::{Buf, BytesMut};

use crate::{Error, Result, table};

/// Hard cap on a frame's wire length, whatever the packet.
pub const MAX_WIRE_LEN: u32 = 1_677_721_600;

/// One decoded frame, payload already decompressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: u32,
    pub payload: Vec<u8>,
}

/// Encodes a frame. `payload` is the packet's serialized form; it is
/// compressed here when the packet type says so.
pub fn encode(id: u32, payload: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let info = table::info(id).ok_or(Error::UnknownId(id))?;
    if payload.len() > info.max_size as usize {
        return Err(Error::TooBig {
            name: info.name,
            len: payload.len(),
            max: info.max_size as usize,
        });
    }
    if info.compressed && !payload.is_empty() {
        let body = zstd::bulk::compress(payload, 0).map_err(|e| Error::Zstd(e.to_string()))?;
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&body);
    } else {
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(payload);
    }
    Ok(())
}

/// Splits a byte stream into frames.
///
/// `incoming` picks the direction the frames travel in: `true` when this side
/// is the server reading what the client sent.
pub struct FrameDecoder {
    buf: BytesMut,
    to_server: bool,
}

impl FrameDecoder {
    pub fn new(to_server: bool) -> Self {
        Self {
            buf: BytesMut::new(),
            to_server,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Next complete frame, `Ok(None)` when more bytes are needed.
    pub fn next(&mut self) -> Result<Option<Frame>> {
        if self.buf.len() < 8 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.buf[0..4].try_into().unwrap());
        let id = u32::from_le_bytes(self.buf[4..8].try_into().unwrap());
        if len > MAX_WIRE_LEN {
            return Err(Error::BadLength(len as i64));
        }
        let info = table::info(id).ok_or(Error::UnknownId(id))?;
        let right_way = if self.to_server {
            info.direction.to_server()
        } else {
            info.direction.to_client()
        };
        if !right_way {
            return Err(Error::UnknownId(id));
        }
        if len > info.max_size {
            return Err(Error::TooBig {
                name: info.name,
                len: len as usize,
                max: info.max_size as usize,
            });
        }
        let len = len as usize;
        if self.buf.len() < 8 + len {
            return Ok(None);
        }
        self.buf.advance(8);
        let body = self.buf.split_to(len);
        let payload = if info.compressed && len > 0 {
            decompress(&body, info.max_size as usize)?
        } else {
            body.to_vec()
        };
        Ok(Some(Frame { id, payload }))
    }
}

fn decompress(body: &[u8], max: usize) -> Result<Vec<u8>> {
    let size = zstd::zstd_safe::get_frame_content_size(body)
        .map_err(|_| Error::Zstd("bad frame".into()))?
        .ok_or_else(|| Error::Zstd("frame does not declare its content size".into()))?;
    if size as usize > max {
        return Err(Error::Zstd(format!("content size {size} over max {max}")));
    }
    zstd::bulk::decompress(body, size as usize).map_err(|e| Error::Zstd(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_compressed_round_trip() {
        let mut wire = Vec::new();
        encode(4, &[7u8; 19], &mut wire).unwrap(); // Pong, plain, to server
        encode(23, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9], &mut wire).unwrap(); // RequestAssets, zstd, to server
        encode(33, &[], &mut wire).unwrap(); // SetupFinalize, empty

        let mut d = FrameDecoder::new(true);
        // Feed byte by byte: frames must come out whole regardless of chunking.
        let mut got = Vec::new();
        for b in &wire {
            d.push(&[*b]);
            while let Some(f) = d.next().unwrap() {
                got.push(f);
            }
        }
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0],
            Frame {
                id: 4,
                payload: vec![7; 19]
            }
        );
        assert_eq!(got[1].payload, [1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9]);
        assert_eq!(
            got[2],
            Frame {
                id: 33,
                payload: vec![]
            }
        );
    }

    #[test]
    fn rejects_wrong_direction_and_oversize() {
        let mut wire = Vec::new();
        encode(3, &[0; 28], &mut wire).unwrap(); // Ping goes to the client
        let mut d = FrameDecoder::new(true);
        d.push(&wire);
        assert!(d.next().is_err());

        let mut d = FrameDecoder::new(true);
        d.push(&100u32.to_le_bytes());
        d.push(&4u32.to_le_bytes()); // Pong max is 19
        assert!(matches!(d.next(), Err(Error::TooBig { .. })));
    }
}
