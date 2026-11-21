//! Building blocks the generated protocol code (`proto`) is made of.
//! Decoders take the buffer and a position and return the value and the
//! position right after it.

use uuid::Uuid;

use crate::{Error, Result, varint};

/// A type with a wire form.
pub trait Wire: Sized {
    fn encode(&self, out: &mut Vec<u8>) -> Result<()>;
    /// Decodes at `p`; returns the value and where it ends.
    fn decode(b: &[u8], p: usize) -> Result<(Self, usize)>;
}

#[inline]
pub fn need(b: &[u8], end: usize) -> Result<()> {
    if end > b.len() {
        Err(Error::Short {
            need: end,
            have: b.len(),
        })
    } else {
        Ok(())
    }
}

/// Little-endian scalars.
pub trait Scalar: Sized + Copy {
    const SIZE: usize;
    fn put(self, out: &mut Vec<u8>);
    fn at(b: &[u8], p: usize) -> Result<Self>;
}

macro_rules! scalar {
    ($t:ty, $n:expr) => {
        impl Scalar for $t {
            const SIZE: usize = $n;
            #[inline]
            fn put(self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            #[inline]
            fn at(b: &[u8], p: usize) -> Result<Self> {
                need(b, p + $n)?;
                Ok(<$t>::from_le_bytes(b[p..p + $n].try_into().unwrap()))
            }
        }
    };
}
scalar!(i8, 1);
scalar!(i16, 2);
scalar!(i32, 4);
scalar!(i64, 8);
scalar!(f32, 4);
scalar!(f64, 8);

impl Scalar for bool {
    const SIZE: usize = 1;
    fn put(self, out: &mut Vec<u8>) {
        out.push(self as u8);
    }
    fn at(b: &[u8], p: usize) -> Result<Self> {
        need(b, p + 1)?;
        Ok(b[p] != 0)
    }
}

pub fn get<T: Scalar>(b: &[u8], p: usize) -> Result<(T, usize)> {
    Ok((T::at(b, p)?, p + T::SIZE))
}

pub fn put<T: Scalar>(out: &mut Vec<u8>, v: T) -> Result<()> {
    v.put(out);
    Ok(())
}

pub fn get_varint(b: &[u8], p: usize) -> Result<(usize, usize)> {
    let (v, n) = varint::read(b.get(p..).ok_or(Error::Short {
        need: p,
        have: b.len(),
    })?)?;
    Ok((v as usize, p + n))
}

pub fn put_len(out: &mut Vec<u8>, n: usize) -> Result<()> {
    varint::write(out, i32::try_from(n).map_err(|_| Error::VarIntNegative)?)
}

pub fn get_string(b: &[u8], p: usize) -> Result<(String, usize)> {
    let (n, p) = get_varint(b, p)?;
    need(b, p + n)?;
    let s = std::str::from_utf8(&b[p..p + n]).map_err(|_| Error::BadText {
        field: "string",
        what: "UTF-8",
    })?;
    Ok((s.to_owned(), p + n))
}

pub fn put_string(out: &mut Vec<u8>, s: &str) -> Result<()> {
    put_len(out, s.len())?;
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

pub fn get_fixed_string(b: &[u8], p: usize, len: usize) -> Result<(String, usize)> {
    need(b, p + len)?;
    let raw = &b[p..p + len];
    let end = raw.iter().position(|&c| c == 0).unwrap_or(len);
    let s = std::str::from_utf8(&raw[..end]).map_err(|_| Error::BadText {
        field: "string",
        what: "UTF-8",
    })?;
    Ok((s.to_owned(), p + len))
}

pub fn put_fixed_string(out: &mut Vec<u8>, s: &str, len: usize) -> Result<()> {
    let b = s.as_bytes();
    let n = b.len().min(len);
    out.extend_from_slice(&b[..n]);
    out.resize(out.len() + len - n, 0);
    Ok(())
}

pub fn get_uuid(b: &[u8], p: usize) -> Result<(Uuid, usize)> {
    need(b, p + 16)?;
    Ok((Uuid::from_bytes(b[p..p + 16].try_into().unwrap()), p + 16))
}

pub fn put_uuid(out: &mut Vec<u8>, v: &Uuid) -> Result<()> {
    out.extend_from_slice(v.as_bytes());
    Ok(())
}

pub fn get_floats<const N: usize>(b: &[u8], mut p: usize) -> Result<([f32; N], usize)> {
    let mut v = [0f32; N];
    for x in &mut v {
        *x = f32::at(b, p)?;
        p += 4;
    }
    Ok((v, p))
}

pub fn put_floats<const N: usize>(out: &mut Vec<u8>, v: &[f32; N]) -> Result<()> {
    for x in v {
        x.put(out);
    }
    Ok(())
}

/// A fixed number of scalars with no count in front.
pub fn get_fixed_scalars<T: Scalar>(b: &[u8], mut p: usize, n: usize) -> Result<(Vec<T>, usize)> {
    need(b, p + n * T::SIZE)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(T::at(b, p)?);
        p += T::SIZE;
    }
    Ok((v, p))
}

pub fn put_fixed_scalars<T: Scalar>(out: &mut Vec<u8>, v: &[T], n: usize) -> Result<()> {
    for i in 0..n {
        match v.get(i) {
            Some(x) => x.put(out),
            None => out.resize(out.len() + T::SIZE, 0),
        }
    }
    Ok(())
}

/// Count, then each element.
pub fn get_vec<T>(
    b: &[u8],
    p: usize,
    f: impl Fn(&[u8], usize) -> Result<(T, usize)>,
) -> Result<(Vec<T>, usize)> {
    let (n, mut p) = get_varint(b, p)?;
    if n > b.len().saturating_sub(p).saturating_add(1) * 8 {
        return Err(Error::FieldTooLong {
            field: "array",
            len: n,
            max: b.len(),
        });
    }
    let mut v = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let (x, q) = f(b, p)?;
        v.push(x);
        p = q;
    }
    Ok((v, p))
}

pub fn put_vec<T>(
    out: &mut Vec<u8>,
    v: &[T],
    f: impl Fn(&mut Vec<u8>, &T) -> Result<()>,
) -> Result<()> {
    put_len(out, v.len())?;
    for x in v {
        f(out, x)?;
    }
    Ok(())
}

/// Count, presence bits, then the present elements.
pub fn get_opt_vec<T>(
    b: &[u8],
    p: usize,
    f: impl Fn(&[u8], usize) -> Result<(T, usize)>,
) -> Result<(Vec<Option<T>>, usize)> {
    let (n, p) = get_varint(b, p)?;
    let bits = n.div_ceil(8);
    need(b, p + bits)?;
    let flags = &b[p..p + bits];
    let mut p = p + bits;
    let mut v = Vec::with_capacity(n.min(1 << 16));
    for i in 0..n {
        if flags[i / 8] >> (i % 8) & 1 != 0 {
            let (x, q) = f(b, p)?;
            v.push(Some(x));
            p = q;
        } else {
            v.push(None);
        }
    }
    Ok((v, p))
}

pub fn put_opt_vec<T>(
    out: &mut Vec<u8>,
    v: &[Option<T>],
    f: impl Fn(&mut Vec<u8>, &T) -> Result<()>,
) -> Result<()> {
    put_len(out, v.len())?;
    let mut bits = vec![0u8; v.len().div_ceil(8)];
    for (i, x) in v.iter().enumerate() {
        if x.is_some() {
            bits[i / 8] |= 1 << (i % 8);
        }
    }
    out.extend_from_slice(&bits);
    for x in v.iter().flatten() {
        f(out, x)?;
    }
    Ok(())
}

/// Count, then key and value pairs. Kept as a list so the order survives a round trip.
pub fn get_map<K, V>(
    b: &[u8],
    p: usize,
    fk: impl Fn(&[u8], usize) -> Result<(K, usize)>,
    fv: impl Fn(&[u8], usize) -> Result<(V, usize)>,
) -> Result<(Vec<(K, V)>, usize)> {
    let (n, mut p) = get_varint(b, p)?;
    let mut v = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let (k, q) = fk(b, p)?;
        let (x, q) = fv(b, q)?;
        v.push((k, x));
        p = q;
    }
    Ok((v, p))
}

pub fn put_map<K, V>(
    out: &mut Vec<u8>,
    v: &[(K, V)],
    fk: impl Fn(&mut Vec<u8>, &K) -> Result<()>,
    fv: impl Fn(&mut Vec<u8>, &V) -> Result<()>,
) -> Result<()> {
    put_len(out, v.len())?;
    for (k, x) in v {
        fk(out, k)?;
        fv(out, x)?;
    }
    Ok(())
}

/// Writes `bytes` over `out` at `at` (fixed fields go into space reserved up front).
pub fn place(out: &mut [u8], at: usize, bytes: &[u8]) {
    out[at..at + bytes.len()].copy_from_slice(bytes);
}

/// Checks a variable-field slot against the canonical position.
pub fn slot(b: &[u8], at: usize, present: bool, rel: usize, field: &'static str) -> Result<()> {
    let got = i32::at(b, at)?;
    let want = if present { rel as i32 } else { -1 };
    if got != want {
        return Err(Error::NonCanonical { field, got, want });
    }
    Ok(())
}

/// Puts every map of a value in key order (maps arrive in the sender's hash order).
pub trait Canon {
    fn canon(&mut self);
}

/// Sorts map entries by key. Keys are never NaN in practice; ties keep their order.
pub fn sort_pairs<K: PartialOrd, V>(v: &mut [(K, V)]) {
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
}

/// Finds what the client refuses in a value it otherwise decodes: the same
/// key twice in a map ("duplicate key"), registry indexes out of range.
pub trait Check {
    fn check(&self, errs: &mut Vec<String>);
}

/// Most errors kept per check, so one bad registry does not flood the log.
const MAX_ERRS: usize = 20;

/// Reports keys that appear more than once in a map.
pub fn check_unique_keys<K: PartialOrd + std::fmt::Debug, V>(
    v: &[(K, V)],
    at: &str,
    errs: &mut Vec<String>,
) {
    let mut keys: Vec<&K> = v.iter().map(|(k, _)| k).collect();
    keys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    for w in keys.windows(2) {
        if w[0] == w[1] && errs.len() < MAX_ERRS {
            errs.push(format!("{at}: duplicate key {:?}", w[0]));
        }
    }
}

/// Reports registry indexes outside `0..max_id`.
pub fn check_index_range<V>(v: &[(i32, V)], max_id: i32, at: &str, errs: &mut Vec<String>) {
    for (k, _) in v {
        if (*k < 0 || *k >= max_id) && errs.len() < MAX_ERRS {
            errs.push(format!("{at}: index {k} outside 0..{max_id}"));
        }
    }
}
