//! Unsigned LEB128, at most 5 bytes (spec 02, "Scalars").

use crate::{Error, Result};

pub fn write(out: &mut Vec<u8>, value: i32) -> Result<()> {
    if value < 0 {
        return Err(Error::VarIntNegative);
    }
    let mut v = value as u32;
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return Ok(());
        }
        out.push(byte | 0x80);
    }
}

pub fn len(value: u32) -> usize {
    match value {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0x0fff_ffff => 4,
        _ => 5,
    }
}

/// Reads a varint at the start of `buf`. Returns the value and how many bytes it took.
pub fn read(buf: &[u8]) -> Result<(i32, usize)> {
    let mut value: u32 = 0;
    for i in 0..5 {
        let byte = *buf.get(i).ok_or(Error::Short {
            need: i + 1,
            have: buf.len(),
        })?;
        value |= ((byte & 0x7f) as u32) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value as i32, i + 1));
        }
    }
    Err(Error::VarIntTooLong)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for v in [
            0,
            1,
            127,
            128,
            300,
            16_383,
            16_384,
            2_097_151,
            2_097_152,
            268_435_455,
            268_435_456,
            i32::MAX,
        ] {
            let mut out = Vec::new();
            write(&mut out, v).unwrap();
            assert_eq!(out.len(), len(v as u32), "len of {v}");
            assert_eq!(read(&out).unwrap(), (v, out.len()));
        }
    }

    #[test]
    fn known_bytes() {
        let mut out = Vec::new();
        write(&mut out, 300).unwrap();
        assert_eq!(out, [0xac, 0x02]);
        assert!(write(&mut out, -1).is_err());
        assert!(matches!(
            read(&[0x80, 0x80, 0x80, 0x80, 0x80]),
            Err(Error::VarIntTooLong)
        ));
    }
}
