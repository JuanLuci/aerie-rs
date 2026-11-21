//! The struct layout shared by packets and nested types (spec 02, "Struct layout"):
//!
//! ```text
//! [nullable bits][fixed fields][offset table, only with 2+ variable fields][variable block]
//! ```

use uuid::Uuid;

use crate::{Error, Result, varint};

// ---------------------------------------------------------------- writing

/// Writes one struct into `out`. Fixed fields go through the `put_*` helpers,
/// then [`begin_var`](Self::begin_var), then one [`var`](Self::var) per
/// variable field in declaration order.
pub struct StructWriter {
    start: usize,
    slots_at: usize,
    slots: usize,
    next: usize,
    var_start: usize,
}

impl StructWriter {
    pub fn new(out: &mut Vec<u8>, null_bytes: usize) -> Self {
        let start = out.len();
        out.resize(start + null_bytes, 0);
        Self {
            start,
            slots_at: 0,
            slots: 0,
            next: 0,
            var_start: 0,
        }
    }

    pub fn null_bit(&self, out: &mut [u8], bit: usize) {
        out[self.start + bit / 8] |= 1 << (bit % 8);
    }

    /// Ends the fixed part and reserves the offset table for `count` variable fields.
    pub fn begin_var(&mut self, out: &mut Vec<u8>, count: usize) {
        self.slots = count;
        self.slots_at = out.len();
        if count >= 2 {
            out.resize(out.len() + 4 * count, 0);
        }
        self.var_start = out.len();
    }

    /// Opens the next variable field. Returns `present`; when it is true the
    /// caller writes the field's bytes right after this call.
    pub fn var(&mut self, out: &mut Vec<u8>, present: bool) -> bool {
        assert!(self.next < self.slots, "more variable fields than declared");
        if self.slots >= 2 {
            let value: i32 = if present {
                (out.len() - self.var_start) as i32
            } else {
                -1
            };
            let at = self.slots_at + 4 * self.next;
            out[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        self.next += 1;
        present
    }

    /// Bytes written for this struct so far.
    pub fn len(&self, out: &[u8]) -> usize {
        out.len() - self.start
    }
}

pub fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
pub fn put_bool(out: &mut Vec<u8>, v: bool) {
    out.push(v as u8);
}
pub fn put_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_le_bytes());
}
pub fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}
pub fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}
/// UUIDs are the one big-endian scalar: 16 bytes, most significant first.
pub fn put_uuid(out: &mut Vec<u8>, v: Uuid) {
    out.extend_from_slice(v.as_bytes());
}
pub fn put_fixed_ascii(out: &mut Vec<u8>, v: &str, len: usize) {
    let b = v.as_bytes();
    let n = b.len().min(len);
    out.extend_from_slice(&b[..n]);
    out.resize(out.len() + (len - n), 0);
}
pub fn put_var_string(out: &mut Vec<u8>, field: &'static str, v: &str, max: usize) -> Result<()> {
    if v.len() > max {
        return Err(Error::FieldTooLong {
            field,
            len: v.len(),
            max,
        });
    }
    varint::write(out, v.len() as i32)?;
    out.extend_from_slice(v.as_bytes());
    Ok(())
}
pub fn put_var_bytes(out: &mut Vec<u8>, field: &'static str, v: &[u8], max: usize) -> Result<()> {
    if v.len() > max {
        return Err(Error::FieldTooLong {
            field,
            len: v.len(),
            max,
        });
    }
    varint::write(out, v.len() as i32)?;
    out.extend_from_slice(v);
    Ok(())
}

// ---------------------------------------------------------------- reading

/// Reads one struct from the start of `buf`, checking the canonical layout.
pub struct StructReader<'a> {
    buf: &'a [u8],
    null_bytes: usize,
    slots_at: usize,
    slots: usize,
    next: usize,
    var_start: usize,
    pos: usize,
}

impl<'a> StructReader<'a> {
    /// `fixed_end`: where the fixed part ends (nullable bits included).
    /// `var_count`: number of variable fields.
    pub fn new(
        buf: &'a [u8],
        null_bytes: usize,
        fixed_end: usize,
        var_count: usize,
    ) -> Result<Self> {
        let var_start = fixed_end + if var_count >= 2 { 4 * var_count } else { 0 };
        need(buf, var_start)?;
        Ok(Self {
            buf,
            null_bytes,
            slots_at: fixed_end,
            slots: var_count,
            next: 0,
            var_start,
            pos: 0,
        })
    }

    pub fn bit(&self, bit: usize) -> bool {
        debug_assert!(bit / 8 < self.null_bytes);
        self.buf[bit / 8] & (1 << (bit % 8)) != 0
    }

    pub fn u8(&self, at: usize) -> u8 {
        self.buf[at]
    }
    pub fn bool(&self, at: usize) -> bool {
        self.buf[at] != 0
    }
    pub fn i16(&self, at: usize) -> i16 {
        i16::from_le_bytes(self.buf[at..at + 2].try_into().unwrap())
    }
    pub fn i32(&self, at: usize) -> i32 {
        i32::from_le_bytes(self.buf[at..at + 4].try_into().unwrap())
    }
    pub fn i64(&self, at: usize) -> i64 {
        i64::from_le_bytes(self.buf[at..at + 8].try_into().unwrap())
    }
    pub fn uuid(&self, at: usize) -> Uuid {
        Uuid::from_bytes(self.buf[at..at + 16].try_into().unwrap())
    }
    pub fn fixed_ascii(&self, field: &'static str, at: usize, len: usize) -> Result<String> {
        let raw = &self.buf[at..at + len];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(len);
        ascii(field, &raw[..end])
    }

    /// Opens the next variable field. `present` comes from the field's
    /// nullable bit (`true` for required fields). Returns the bytes from the
    /// field's start to the end of the buffer, or `None` when absent; the
    /// caller parses them and reports how many it used with [`advance`](Self::advance).
    pub fn var(&mut self, field: &'static str, present: bool) -> Result<Option<&'a [u8]>> {
        assert!(self.next < self.slots, "more variable fields than declared");
        if self.slots >= 2 {
            let at = self.slots_at + 4 * self.next;
            let got = self.i32(at);
            let want = if present { self.pos as i32 } else { -1 };
            if got != want {
                return Err(Error::NonCanonical { field, got, want });
            }
        }
        self.next += 1;
        Ok(present.then(|| &self.buf[self.var_start + self.pos..]))
    }

    pub fn advance(&mut self, n: usize) {
        self.pos += n;
    }

    /// Total bytes this struct took.
    pub fn end(&self) -> usize {
        self.var_start + self.pos
    }
}

pub fn need(buf: &[u8], n: usize) -> Result<()> {
    if buf.len() < n {
        Err(Error::Short {
            need: n,
            have: buf.len(),
        })
    } else {
        Ok(())
    }
}

fn ascii(field: &'static str, b: &[u8]) -> Result<String> {
    if b.is_ascii() {
        Ok(String::from_utf8(b.to_vec()).unwrap())
    } else {
        Err(Error::BadText {
            field,
            what: "ASCII",
        })
    }
}

/// Varint-prefixed bytes at the start of `buf`. Returns them and the bytes used.
pub fn var_bytes<'a>(buf: &'a [u8], field: &'static str, max: usize) -> Result<(&'a [u8], usize)> {
    let (len, n) = varint::read(buf)?;
    let len = len as usize;
    if len > max {
        return Err(Error::FieldTooLong { field, len, max });
    }
    need(buf, n + len)?;
    Ok((&buf[n..n + len], n + len))
}

pub fn var_string(buf: &[u8], field: &'static str, max: usize) -> Result<(String, usize)> {
    let (b, used) = var_bytes(buf, field, max)?;
    let s = std::str::from_utf8(b).map_err(|_| Error::BadText {
        field,
        what: "UTF-8",
    })?;
    Ok((s.to_owned(), used))
}

pub fn var_ascii(buf: &[u8], field: &'static str, max: usize) -> Result<(String, usize)> {
    let (b, used) = var_bytes(buf, field, max)?;
    Ok((ascii(field, b)?, used))
}
