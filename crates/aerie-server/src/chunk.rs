//! Block sections as `SetChunk` carries them (spec 08).
//!
//! A section is 32×32×32 blocks; block `(x, y, z)` (0..32 each) is at index
//! `y << 10 | z << 5 | x`. `SetChunk.data` holds three paletted arrays in a
//! row: block ids, fillers and rotations. An all-air section is sent with
//! `data` null.

use std::collections::HashMap;

use anyhow::{Result, bail};

pub const SIZE: usize = 32;
pub const VOLUME: usize = SIZE * SIZE * SIZE;

pub fn index(x: usize, y: usize, z: usize) -> usize {
    y << 10 | z << 5 | x
}

/// One section's blocks, fillers and rotations.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub blocks: Vec<i32>,
    pub fillers: Vec<i32>,
    pub rotations: Vec<i32>,
}

impl Default for Section {
    fn default() -> Self {
        Self {
            blocks: vec![0; VOLUME],
            fillers: vec![0; VOLUME],
            rotations: vec![0; VOLUME],
        }
    }
}

impl Section {
    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|&b| b == 0)
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, block: i32) {
        self.blocks[index(x, y, z)] = block;
    }

    /// `SetChunk.data`, or `None` for an all-air section.
    pub fn encode(&self) -> Option<Vec<u8>> {
        if self.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        paletted(&self.blocks, &mut out);
        paletted(&self.fillers, &mut out);
        paletted(&self.rotations, &mut out);
        Some(out)
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut p = 0;
        let blocks = unpalette(data, &mut p)?;
        let fillers = unpalette(data, &mut p)?;
        let rotations = unpalette(data, &mut p)?;
        if p != data.len() {
            bail!("{} bytes after the three arrays", data.len() - p);
        }
        Ok(Self {
            blocks,
            fillers,
            rotations,
        })
    }
}

/// Palette kinds: the type byte, then how indices are stored.
const EMPTY: u8 = 0;
const HALF_BYTE: u8 = 1;
const BYTE: u8 = 2;
/// Over 256 distinct values: i16 palette indices.
const SHORT: u8 = 3;

/// One paletted array: type byte; then, unless empty, an i16 entry count,
/// entries (u8 palette index, i32 value, i16 number of uses) and the
/// indices (two per byte, high nibble first, or one per byte). An array of
/// zeros is just the type byte `0`.
fn paletted(values: &[i32], out: &mut Vec<u8>) {
    if values.iter().all(|&v| v == 0) {
        out.push(EMPTY);
        return;
    }
    let mut ids: HashMap<i32, u8> = HashMap::new();
    let mut entries: Vec<(i32, i16)> = Vec::new();
    let mut idx = Vec::with_capacity(values.len());
    for &v in values {
        let i = *ids.entry(v).or_insert_with(|| {
            entries.push((v, 0));
            (entries.len() - 1) as u8
        });
        entries[i as usize].1 = entries[i as usize].1.wrapping_add(1);
        idx.push(i);
    }
    // More than 256 distinct values needs the short palette, not seen yet.
    assert!(
        entries.len() <= 256,
        "section with over 256 distinct values"
    );
    let kind = if entries.len() <= 16 { HALF_BYTE } else { BYTE };
    out.push(kind);
    out.extend_from_slice(&(entries.len() as i16).to_le_bytes());
    for (i, (v, n)) in entries.iter().enumerate() {
        out.push(i as u8);
        out.extend_from_slice(&v.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
    }
    if kind == HALF_BYTE {
        for pair in idx.chunks(2) {
            out.push(pair[0] << 4 | pair[1]);
        }
    } else {
        out.extend_from_slice(&idx);
    }
}

fn unpalette(d: &[u8], p: &mut usize) -> Result<Vec<i32>> {
    let need = |end: usize| -> Result<()> {
        if end > d.len() {
            bail!("section data ends early");
        }
        Ok(())
    };
    need(*p + 1)?;
    let kind = d[*p];
    *p += 1;
    if kind == EMPTY {
        return Ok(vec![0; VOLUME]);
    }
    need(*p + 2)?;
    let n = i16::from_le_bytes([d[*p], d[*p + 1]]) as usize;
    *p += 2;
    // The short palette's entries start with an i16 index, the others' with
    // a byte; then the value (i32) and its number of uses (i16).
    let key = if kind == SHORT { 2 } else { 1 };
    let mut palette = vec![0i32; if kind == SHORT { 1 << 16 } else { 256 }];
    for _ in 0..n {
        need(*p + key + 6)?;
        let i = if kind == SHORT {
            u16::from_le_bytes([d[*p], d[*p + 1]]) as usize
        } else {
            d[*p] as usize
        };
        palette[i] = i32::from_le_bytes(d[*p + key..*p + key + 4].try_into().unwrap());
        *p += key + 6;
    }
    let mut out = Vec::with_capacity(VOLUME);
    match kind {
        HALF_BYTE => {
            need(*p + VOLUME / 2)?;
            for &b in &d[*p..*p + VOLUME / 2] {
                out.push(palette[(b >> 4) as usize]);
                out.push(palette[(b & 15) as usize]);
            }
            *p += VOLUME / 2;
        }
        BYTE => {
            need(*p + VOLUME)?;
            out.extend(d[*p..*p + VOLUME].iter().map(|&b| palette[b as usize]));
            *p += VOLUME;
        }
        SHORT => {
            need(*p + VOLUME * 2)?;
            out.extend(
                d[*p..*p + VOLUME * 2]
                    .chunks(2)
                    .map(|b| palette[u16::from_le_bytes([b[0], b[1]]) as usize]),
            );
            *p += VOLUME * 2;
        }
        k => bail!("palette type {k} not known yet"),
    }
    Ok(out)
}

/// A flat world's section at section height `sy`: `layers[y]` is the block
/// of world layer `y` (missing layers are air).
pub fn flat(sy: i32, layers: &[i32]) -> Section {
    let mut s = Section::default();
    for y in 0..SIZE {
        let wy = sy as usize * SIZE + y;
        let Some(&b) = layers.get(wy) else { break };
        if b == 0 {
            continue;
        }
        for z in 0..SIZE {
            for x in 0..SIZE {
                s.set(x, y, z, b);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_section_has_no_data() {
        assert_eq!(Section::default().encode(), None);
    }

    #[test]
    fn round_trip_half_byte_and_byte() {
        let mut s = Section::default();
        s.set(0, 0, 0, 7);
        s.set(31, 31, 31, 9);
        s.rotations[index(1, 2, 3)] = 4;
        let d = s.encode().unwrap();
        assert_eq!(d[0], HALF_BYTE);
        assert_eq!(d[1 + 2 + 3 * 7 + VOLUME / 2], EMPTY, "no fillers");
        assert_eq!(Section::decode(&d).unwrap(), s);

        for i in 0..40 {
            s.set(i % 32, 5, i / 32, 100 + i as i32);
        }
        let d = s.encode().unwrap();
        assert_eq!(d[0], BYTE);
        assert_eq!(Section::decode(&d).unwrap(), s);
    }

    #[test]
    fn half_byte_puts_the_first_index_high() {
        let mut s = Section::default();
        s.set(1, 0, 0, 5); // index 1: low nibble of the first byte
        let d = s.encode().unwrap();
        // type, count 2, entries (0 → air, 1 → 5), then the indices.
        let first = d[3 + 2 * 7];
        assert_eq!(first, 0x01);
    }

    #[test]
    fn flat_layers() {
        let layers = [1, 2, 2, 3];
        let s = flat(0, &layers);
        assert_eq!(s.blocks[index(4, 0, 9)], 1);
        assert_eq!(s.blocks[index(4, 3, 9)], 3);
        assert_eq!(s.blocks[index(4, 4, 9)], 0);
        assert!(flat(1, &layers).is_empty());
    }

    #[test]
    fn reads_the_short_palette() {
        // Blocks: 300 values (i16 indices); fillers and rotations empty.
        let mut d = vec![SHORT];
        d.extend_from_slice(&300i16.to_le_bytes());
        for i in 0..300u16 {
            d.extend_from_slice(&i.to_le_bytes());
            d.extend_from_slice(&(1000 + i as i32).to_le_bytes());
            d.extend_from_slice(&1i16.to_le_bytes());
        }
        for n in 0..VOLUME {
            d.extend_from_slice(&((n % 300) as u16).to_le_bytes());
        }
        d.extend_from_slice(&[EMPTY, EMPTY]);
        let s = Section::decode(&d).unwrap();
        assert_eq!(s.blocks[0], 1000);
        assert_eq!(s.blocks[299], 1299);
        assert_eq!(s.blocks[300], 1000);
        assert!(s.fillers.iter().all(|&v| v == 0));
    }
}

/// Checks the decoder on captured chunks: `AERIE_CHUNKS=<dir of *-131-*.bin>`
/// (`cargo test -p aerie-server captured -- --ignored`). Each one must
/// decode and encode back to a section that decodes the same.
#[test]
#[ignore]
fn captured() {
    use aerie_protocol::proto;
    use aerie_protocol::wire::Wire;
    let dir = std::env::var("AERIE_CHUNKS").expect("AERIE_CHUNKS");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        if !path.to_string_lossy().contains("-131-") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let (c, _) = proto::SetChunk::decode(&bytes, 0).unwrap();
        let Some(data) = c.data else { continue };
        let data: Vec<u8> = data.iter().map(|&b| b as u8).collect();
        let s = Section::decode(&data).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let again = Section::decode(&s.encode().unwrap()).unwrap();
        assert_eq!(again, s, "{path:?}");
        n += 1;
    }
    assert!(n > 0);
    eprintln!("{n} captured sections decode and re-encode");
}
