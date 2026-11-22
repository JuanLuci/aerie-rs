//! Block changes as clients receive them (spec 10, "What clients
//! receive"): per section, the positions changed during a tick, sent at the
//! end of it in the order the official server's set gives them.
//!
//! That set is an open addressing integer hash set (the algorithm of the
//! public fastutil `IntOpenHashSet`): a power-of-two table at load factor
//! 0.75, hash `h = k * 0x9E3779B9; h ^ (h >>> 16)`, linear probing, grown
//! by rehashing (old slots taken from the top down), iterated from the last
//! slot down with key 0 first. Each section has two such sets that swap
//! every tick and are cleared but never shrunk.

/// The order the official server's integer-keyed maps list their entries
/// in a packet (inventory sections: the same open addressing table as the
/// sets below, made for 16 entries, filled in ascending slot order).
pub fn map_order(keys: impl IntoIterator<Item = i32>) -> Vec<i32> {
    let mut s = IntSet::with_capacity(32);
    for k in keys {
        s.insert(k);
    }
    s.iter()
}

/// An integer set iterated in the official server's order.
#[derive(Clone, Debug)]
pub struct IntSet {
    keys: Vec<i32>,
    has_zero: bool,
    size: usize,
    max_fill: usize,
}

fn mix(k: i32) -> u32 {
    let h = (k as u32).wrapping_mul(0x9E37_79B9);
    h ^ (h >> 16)
}

fn table_size(expected: usize) -> usize {
    let need = (expected as f64 / 0.75).ceil() as usize;
    need.next_power_of_two().max(2)
}

fn max_fill(n: usize) -> usize {
    ((n as f64 * 0.75).ceil() as usize).min(n - 1)
}

impl Default for IntSet {
    fn default() -> Self {
        let n = table_size(0);
        Self {
            keys: vec![0; n],
            has_zero: false,
            size: 0,
            max_fill: max_fill(n),
        }
    }
}

impl IntSet {
    /// The table size (slots).
    pub fn capacity(&self) -> usize {
        self.keys.len()
    }

    /// An empty set with a given table size.
    pub fn with_capacity(n: usize) -> Self {
        let n = n.next_power_of_two().max(2);
        Self {
            keys: vec![0; n],
            has_zero: false,
            size: 0,
            max_fill: max_fill(n),
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.size
    }

    pub fn insert(&mut self, k: i32) {
        if k == 0 {
            if self.has_zero {
                return;
            }
            self.has_zero = true;
        } else {
            let mask = self.keys.len() - 1;
            let mut p = mix(k) as usize & mask;
            while self.keys[p] != 0 {
                if self.keys[p] == k {
                    return;
                }
                p = (p + 1) & mask;
            }
            self.keys[p] = k;
        }
        self.size += 1;
        if self.size - 1 >= self.max_fill {
            self.rehash(table_size(self.size + 1));
        }
    }

    fn rehash(&mut self, n: usize) {
        let mask = n - 1;
        let mut new = vec![0; n];
        let mut i = self.keys.len();
        for _ in 0..self.size - self.has_zero as usize {
            i -= 1;
            while self.keys[i] == 0 {
                i -= 1;
            }
            let mut p = mix(self.keys[i]) as usize & mask;
            while new[p] != 0 {
                p = (p + 1) & mask;
            }
            new[p] = self.keys[i];
        }
        self.keys = new;
        self.max_fill = max_fill(n);
    }

    pub fn clear(&mut self) {
        if self.size == 0 {
            return;
        }
        self.size = 0;
        self.has_zero = false;
        self.keys.iter_mut().for_each(|k| *k = 0);
    }

    /// The keys in iteration order.
    pub fn iter(&self) -> Vec<i32> {
        let mut v = Vec::with_capacity(self.size);
        if self.has_zero {
            v.push(0);
        }
        v.extend(self.keys.iter().rev().filter(|&&k| k != 0));
        v
    }
}

/// One section's pair of change sets. The pair swaps every world tick
/// (counted from tick 0 for every section); a set is cleared when its turn
/// comes round again, keeping its size.
#[derive(Clone, Debug, Default)]
pub struct SectionChanges {
    sets: [IntSet; 2],
    /// The tick each set last held changes for.
    epoch: [u64; 2],
}

impl SectionChanges {
    pub fn add(&mut self, tick: u64, index: i32) {
        let c = (tick % 2) as usize;
        if self.epoch[c] != tick {
            self.sets[c].clear();
            self.epoch[c] = tick;
        }
        self.sets[c].insert(index);
    }

    /// The changes of `tick`, in sending order (empty when none).
    pub fn take(&mut self, tick: u64) -> Vec<i32> {
        let c = (tick % 2) as usize;
        if self.epoch[c] != tick {
            return Vec::new();
        }
        self.sets[c].iter()
    }
}

/// Block health changes of a section waiting to be sent: summed per
/// block, in the order of the official server's map (the same hashing as
/// [`IntSet`], one map kept per section and cleared after sending).
#[derive(Clone, Debug, Default)]
pub struct Deltas {
    keys: IntSet,
    values: std::collections::HashMap<i32, f32>,
}

impl Deltas {
    pub fn add(&mut self, index: i32, delta: f32) {
        self.keys.insert(index);
        *self.values.entry(index).or_default() += delta;
    }

    pub fn remove(&mut self, index: i32) {
        if self.values.remove(&index).is_some() {
            let left: Vec<i32> = self
                .keys
                .iter()
                .into_iter()
                .filter(|&k| k != index)
                .collect();
            // Removing from an open addressing table moves later keys; a
            // rebuild in the same order keeps the iteration order.
            let size = self.keys.capacity();
            self.keys = IntSet::with_capacity(size);
            for k in left {
                self.keys.insert(k);
            }
        }
    }

    /// The deltas in sending order, then cleared.
    pub fn take(&mut self) -> Vec<(i32, f32)> {
        let out = self
            .keys
            .iter()
            .into_iter()
            .filter_map(|k| self.values.get(&k).map(|v| (k, *v)))
            .collect();
        self.keys.clear();
        self.values.clear();
        out
    }
}

#[cfg(test)]
mod map_tests {
    #[test]
    fn inventory_sections_as_captured() {
        // Orders seen from the official server.
        assert_eq!(super::map_order(0..6), [0, 2, 4, 1, 3, 5]);
        assert_eq!(super::map_order([0, 1, 3, 4]), [0, 4, 1, 3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two batches captured from the official server (spec 10).
    #[test]
    fn captured_orders() {
        let mut s = IntSet::default();
        s.insert(5);
        s.clear();
        for k in [17362, 17363, 17394] {
            s.insert(k);
        }
        assert_eq!(s.iter(), vec![17362, 17394, 17363]);
        // A set that has grown to 8 slots before.
        let mut s = IntSet::default();
        for k in [1, 2, 3] {
            s.insert(k);
        }
        s.clear();
        for k in [17360, 18386, 19410] {
            s.insert(k);
        }
        assert_eq!(s.iter(), vec![18386, 17360, 19410]);
    }

    #[test]
    fn zero_first_and_growth() {
        let mut s = IntSet::default();
        for k in 0..100 {
            s.insert(k);
        }
        assert_eq!(s.len(), 100);
        let v = s.iter();
        assert_eq!(v[0], 0);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..100).collect::<Vec<_>>());
    }
}
