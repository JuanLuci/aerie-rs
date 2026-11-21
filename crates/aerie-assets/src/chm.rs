//! Iteration order of a concurrent hash map keyed by strings, as the official
//! server's asset loader uses one for assets that wait for their parent
//! (spec 07, "Indexes"). Single-threaded model: inserts append to a bucket,
//! the table doubles at 3/4 load, and a doubling splits each bucket keeping
//! its trailing run in place and putting the nodes before it in reverse order.

/// `String.hashCode` over UTF-16 code units.
pub fn java_hash(s: &str) -> u32 {
    s.encode_utf16()
        .fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32))
}

fn spread(h: u32) -> u32 {
    (h ^ (h >> 16)) & 0x7fff_ffff
}

pub struct Chm {
    table: Vec<Vec<String>>,
    count: usize,
}

impl Default for Chm {
    fn default() -> Self {
        Self {
            table: vec![Vec::new(); 16],
            count: 0,
        }
    }
}

impl Chm {
    fn bucket(&self, k: &str) -> usize {
        spread(java_hash(k)) as usize & (self.table.len() - 1)
    }

    pub fn insert(&mut self, k: &str) {
        let b = self.bucket(k);
        if self.table[b].iter().any(|x| x == k) {
            return;
        }
        self.table[b].push(k.to_owned());
        self.count += 1;
        let n = self.table.len();
        if self.count >= n - (n >> 2) {
            self.grow();
        }
    }

    fn grow(&mut self) {
        let n = self.table.len();
        let mut next = vec![Vec::new(); n * 2];
        for i in (0..n).rev() {
            let bin = std::mem::take(&mut self.table[i]);
            if bin.is_empty() {
                continue;
            }
            let high = |k: &String| spread(java_hash(k)) as usize & n != 0;
            let mut last_run = bin.len() - 1;
            while last_run > 0 && high(&bin[last_run - 1]) == high(&bin[last_run]) {
                last_run -= 1;
            }
            let (mut lo, mut hi): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
            if high(&bin[last_run]) {
                hi.extend_from_slice(&bin[last_run..])
            } else {
                lo.extend_from_slice(&bin[last_run..])
            }
            for k in &bin[..last_run] {
                if high(k) {
                    hi.insert(0, k.clone())
                } else {
                    lo.insert(0, k.clone())
                }
            }
            next[i] = lo;
            next[i + n] = hi;
        }
        self.table = next;
    }

    pub fn remove(&mut self, k: &str) {
        let b = self.bucket(k);
        if let Some(p) = self.table[b].iter().position(|x| x == k) {
            self.table[b].remove(p);
            self.count -= 1;
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn keys(&self) -> Vec<String> {
        self.table.iter().flatten().cloned().collect()
    }
}

/// Iteration order of a `java.util.HashMap<String, _>` filled with `keys` in
/// this order: by bucket of the final table (16 slots, doubling past 3/4
/// load), then by insertion. (Bins long enough to become trees are not
/// modelled.)
pub fn hashmap_order(keys: &[String]) -> Vec<String> {
    hashmap_order_from(keys, 16)
}

/// Like `hashmap_order`, for a map created with another initial capacity
/// (`new HashMap<>(0)` starts at 1).
pub fn hashmap_order_from(keys: &[String], initial: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let uniq: Vec<&String> = keys.iter().filter(|k| seen.insert(k.as_str())).collect();
    let mut cap = initial.max(1);
    while uniq.len() > cap * 3 / 4 {
        cap *= 2;
    }
    let mut v: Vec<(usize, usize, &String)> = uniq
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let h = java_hash(k);
            (((h ^ (h >> 16)) as usize) & (cap - 1), i, *k)
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, _, k)| k.clone()).collect()
}

/// Iteration order of a fastutil `IntOpenHashSet` (load factor 0.75) filled
/// with `values` in this order, as `toIntArray` returns it: 0 first when
/// present, then the table from its last slot down. Open addressing with
/// linear probing over `mix(k) & mask`; the table grows by rehashing from
/// the top slot down.
pub fn int_set_order(values: &[i32], expected: usize) -> Vec<i32> {
    fn mix(x: i32) -> u32 {
        let h = (x as u32).wrapping_mul(0x9E37_79B9);
        h ^ (h >> 16)
    }
    fn array_size(expected: usize) -> usize {
        let need = ((expected as f32) / 0.75f32) as f64;
        let need = need.ceil() as u64;
        need.max(1).next_power_of_two().max(2) as usize
    }
    fn max_fill(n: usize) -> usize {
        (((n as f32) * 0.75f32) as f64).ceil().min((n - 1) as f64) as usize
    }
    fn place(table: &mut [i32], mask: usize, k: i32) {
        let mut pos = mix(k) as usize & mask;
        while table[pos] != 0 {
            pos = (pos + 1) & mask;
        }
        table[pos] = k;
    }
    let mut n = array_size(expected);
    let mut table = vec![0i32; n];
    let mut size = 0usize;
    let mut has_zero = false;
    for &k in values {
        if k == 0 {
            if has_zero {
                continue;
            }
            has_zero = true;
        } else {
            let mask = n - 1;
            let mut pos = mix(k) as usize & mask;
            let mut dup = false;
            while table[pos] != 0 {
                if table[pos] == k {
                    dup = true;
                    break;
                }
                pos = (pos + 1) & mask;
            }
            if dup {
                continue;
            }
            table[pos] = k;
        }
        let grow = size >= max_fill(n);
        size += 1;
        if grow {
            let new_n = array_size(size + 1);
            let mut new_table = vec![0i32; new_n];
            for &k in table.iter().rev().filter(|k| **k != 0) {
                place(&mut new_table, new_n - 1, k);
            }
            table = new_table;
            n = new_n;
        }
    }
    let mut out = Vec::with_capacity(size);
    if has_zero {
        out.push(0);
    }
    out.extend(table.iter().rev().filter(|k| **k != 0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_string_hash() {
        assert_eq!(java_hash("hello") as i32, 99162322);
        assert_eq!(
            java_hash("SFX_Attn_Loud") as i32,
            "SFX_Attn_Loud"
                .encode_utf16()
                .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
        );
    }
}
