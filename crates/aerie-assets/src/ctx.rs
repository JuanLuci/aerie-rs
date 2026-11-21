//! Everything registries share while they are built: the pack, the loaded
//! asset folders, and each registry's key → index map (registries refer to
//! each other by index).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::contained::{self, Contained};
use crate::{Error, Result, pack::Pack, schema, store::Store};

pub struct Ctx {
    pub pack: Pack,
    /// Folders with their file assets only.
    files: Mutex<HashMap<String, Arc<Store>>>,
    /// Contained assets by store path, found once (spec 07).
    contained: std::sync::OnceLock<HashMap<String, Vec<Contained>>>,
    stores: Mutex<HashMap<String, Arc<Store>>>,
    indexes: Mutex<HashMap<String, Arc<HashMap<String, i32>>>>,
    /// Asset tag name → index, in first-seen order unless pinned.
    tags: Mutex<(HashMap<String, i32>, i32)>,
    /// Tag names whose index was pinned.
    pinned_tags: Mutex<HashMap<String, i32>>,
    /// Captured tag lists waiting for `tags::solve` (compare runs).
    pub tag_observations: Mutex<Vec<(Vec<String>, Vec<i32>)>>,
}

impl Ctx {
    pub fn new(pack: Pack) -> Self {
        Self {
            pack,
            files: Mutex::default(),
            contained: std::sync::OnceLock::new(),
            stores: Mutex::default(),
            indexes: Mutex::default(),
            tags: Mutex::default(),
            pinned_tags: Mutex::default(),
            tag_observations: Mutex::default(),
        }
    }

    /// The assets of `Server/<path>`, loaded once, with the contained assets
    /// of that type (spec 07). A path may end in `|.ext` to load files with
    /// another extension (`Particles|.particlesystem`).
    pub fn store(&self, path: &str) -> Result<Arc<Store>> {
        if let Some(s) = self.stores.lock().unwrap().get(path) {
            return Ok(s.clone());
        }
        let base = self.file_store(path)?;
        let extra = self
            .contained()?
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let s = Arc::new(base.with_contained(extra)?);
        self.stores.lock().unwrap().insert(path.into(), s.clone());
        Ok(s)
    }

    /// A folder's file assets only.
    pub fn file_store(&self, path: &str) -> Result<Arc<Store>> {
        if let Some(s) = self.files.lock().unwrap().get(path) {
            return Ok(s.clone());
        }
        let s = Arc::new(match path.split_once('|') {
            Some((dir, ext)) => Store::load_ext(&self.pack, dir, ext)?,
            None => Store::load(&self.pack, path)?,
        });
        self.files.lock().unwrap().insert(path.into(), s.clone());
        Ok(s)
    }

    /// Every contained asset, by the store path of its type.
    pub fn contained(&self) -> Result<&HashMap<String, Vec<Contained>>> {
        if let Some(c) = self.contained.get() {
            return Ok(c);
        }
        let mut files = Vec::new();
        let mut paths = HashMap::new();
        for t in schema::get().asset_types() {
            if !t.ext.ends_with(".json") && !t.ext.starts_with(".particle") {
                continue;
            }
            let sp = t.store_path();
            let s = self.file_store(&sp)?;
            // Walked in index order: owners join the official loader's maps
            // in that order.
            let mut keys: Vec<String> = s.own.keys().cloned().collect();
            keys.sort_by_key(|k| (s.parents.contains_key(k), s.files.get(k).cloned()));
            let assets = keys
                .into_iter()
                .filter_map(|k| s.own.get(&k).map(|v| (k, v)))
                .map(|(k, v)| {
                    let k = &k;
                    let mut v = v.clone();
                    let p = s.parents.get(k).cloned();
                    if let (Value::Object(m), Some(p)) = (&mut v, &p) {
                        m.insert("Parent".into(), Value::String(p.clone()));
                    }
                    let f = s.files.get(k).cloned().unwrap_or_default();
                    (k.clone(), p, v, f)
                })
                .collect();
            paths.insert(t.name.to_owned(), sp);
            files.push((t.name.to_owned(), assets));
        }
        let mut by_path: HashMap<String, Vec<Contained>> = HashMap::new();
        let (mut found, named) = contained::discover_named(&files);
        found.extend(contained::legacy_music(
            &*self.file_store("Audio/AmbienceFX")?,
        ));
        let unknown = contained::unknown_interactions(&files, &found, &named);
        found.extend(unknown);
        for c in found {
            if let Some(p) = paths.get(&c.class) {
                by_path.entry(p.clone()).or_default().push(c);
            }
        }
        Ok(self.contained.get_or_init(|| by_path))
    }

    /// Lower-cased key → index for an indexed registry (spec 07, "Indexes").
    pub fn indexes(&self, path: &str, builtin: &[&str]) -> Result<Arc<HashMap<String, i32>>> {
        if let Some(m) = self.indexes.lock().unwrap().get(path) {
            return Ok(m.clone());
        }
        let order = self.store(path)?.index_order(builtin);
        let m: HashMap<String, i32> = order
            .iter()
            .enumerate()
            .map(|(i, k)| (k.to_lowercase(), i as i32))
            .collect();
        let m = Arc::new(m);
        self.indexes.lock().unwrap().insert(path.into(), m.clone());
        Ok(m)
    }

    /// Uses these key → index pairs for a registry instead of computing them.
    /// The compare tool pins the indexes of a capture, because the official
    /// server's own order is not stable between runs (spec 07, "Indexes").
    pub fn pin(&self, path: &str, pairs: impl IntoIterator<Item = (String, i32)>) {
        let m: HashMap<String, i32> = pairs
            .into_iter()
            .map(|(k, i)| (k.to_lowercase(), i))
            .collect();
        self.indexes
            .lock()
            .unwrap()
            .insert(path.into(), Arc::new(m));
    }

    /// Makes every pinned map a one-to-one key ↔ index map over the keys
    /// Aerie computes for that folder. Pins are partial (some assets are
    /// paired with the capture by content, some not at all), so without
    /// this a key without a pin falls back to its position, which can be an
    /// index another key was pinned to: the client refuses a registry with a
    /// duplicate key. Call once every pin is in.
    pub fn complete_pins(&self) {
        let paths: Vec<String> = self.indexes.lock().unwrap().keys().cloned().collect();
        for path in paths {
            // Pins that are not a folder's indexes (block group ids) stay.
            let Ok(store) = self.store(&path) else {
                continue;
            };
            let order = store.index_order(&[]);
            let pinned = self.indexes.lock().unwrap()[&path].clone();
            let done = complete(&order, &pinned);
            self.indexes.lock().unwrap().insert(path, Arc::new(done));
        }
    }

    /// Keys of an indexed registry in index order: the pinned order when one
    /// was pinned, else the computed one (spec 07, "Indexes").
    pub fn order(&self, path: &str, builtin: &[&str]) -> Result<Vec<String>> {
        let store = self.store(path)?;
        let computed = store.index_order(builtin);
        let idx = self.indexes(path, builtin)?;
        let mut keyed: Vec<(i32, String)> = computed
            .into_iter()
            .map(|k| (idx.get(&k.to_lowercase()).copied().unwrap_or(i32::MAX), k))
            .collect();
        keyed.sort_by_key(|(i, _)| *i);
        Ok(keyed.into_iter().map(|(_, k)| k).collect())
    }

    /// Index of an asset tag (spec 07, "Tags"): the pinned one, or the next
    /// free index the first time a name is seen.
    pub fn tag(&self, name: &str) -> i32 {
        let mut t = self.tags.lock().unwrap();
        if let Some(i) = t.0.get(name) {
            return *i;
        }
        let i = t.1;
        t.0.insert(name.to_owned(), i);
        t.1 = i + 1;
        i
    }

    /// Fixes a tag index (compare runs pin the capture's).
    pub fn pin_tag(&self, name: &str, index: i32) {
        self.pinned_tags
            .lock()
            .unwrap()
            .insert(name.to_owned(), index);
        let mut t = self.tags.lock().unwrap();
        t.0.insert(name.to_owned(), index);
        t.1 = t.1.max(index + 1);
    }

    /// A map pinned with `pin`, if any (for pins that are not a folder's
    /// indexes, like block group ids).
    pub fn pinned(&self, path: &str) -> Option<Arc<HashMap<String, i32>>> {
        self.indexes.lock().unwrap().get(path).cloned()
    }

    /// The tag indexes pinned so far.
    pub fn pinned_tags(&self) -> HashMap<String, i32> {
        self.pinned_tags.lock().unwrap().clone()
    }

    /// Index of `key` in a registry; `fallback` when the key is absent or unknown.
    pub fn index_or(
        &self,
        path: &str,
        builtin: &[&str],
        key: Option<&str>,
        fallback: i32,
    ) -> Result<i32> {
        let Some(key) = key else { return Ok(fallback) };
        Ok(self
            .indexes(path, builtin)?
            .get(&key.to_lowercase())
            .copied()
            .unwrap_or(fallback))
    }
}

/// One-to-one completion of a pinned map (see `Ctx::complete_pins`):
/// pins of keys in `order` first, in order; then pins of other keys (made up
/// at run time), by key; an index already taken loses its later claimant.
/// Keys left without an index take the free ones, lowest first.
pub fn complete(order: &[String], pinned: &HashMap<String, i32>) -> HashMap<String, i32> {
    let mut out = HashMap::new();
    let mut used = std::collections::HashSet::new();
    let lower: Vec<String> = order.iter().map(|k| k.to_lowercase()).collect();
    for k in &lower {
        if let Some(&i) = pinned.get(k) {
            if !out.contains_key(k) && used.insert(i) {
                out.insert(k.clone(), i);
            }
        }
    }
    let mut others: Vec<(&String, &i32)> = pinned
        .iter()
        .filter(|(k, _)| !out.contains_key(*k) && !lower.contains(k))
        .collect();
    others.sort();
    for (k, &i) in others {
        if used.insert(i) {
            out.insert(k.clone(), i);
        }
    }
    let mut next = 0;
    for k in &lower {
        if !out.contains_key(k) {
            while used.contains(&next) {
                next += 1;
            }
            used.insert(next);
            out.insert(k.clone(), next);
        }
    }
    out
}

#[cfg(test)]
mod complete_tests {
    use super::*;

    #[test]
    fn no_duplicate_indexes() {
        let order: Vec<String> = ["A", "B", "C", "D"].iter().map(|s| s.to_string()).collect();
        // B and C claim the same index, D has no pin, `x` is a run-time key.
        let pinned = HashMap::from([
            ("a".to_owned(), 3),
            ("b".to_owned(), 1),
            ("c".to_owned(), 1),
            ("x".to_owned(), 0),
        ]);
        let m = complete(&order, &pinned);
        assert_eq!(m["a"], 3);
        assert_eq!(m["b"], 1);
        assert_eq!(m["x"], 0);
        let mut all: Vec<i32> = m.values().copied().collect();
        all.sort();
        assert_eq!(all, [0, 1, 2, 3, 4]);
    }
}

/// Error for a required field that is missing or malformed.
pub fn invalid(asset: &str, msg: impl Into<String>) -> Error {
    Error::Invalid {
        asset: asset.into(),
        msg: msg.into(),
    }
}

/// Reads an enum field by name.
pub fn enum_or<T>(v: &Value, key: &str, from_name: fn(&str) -> Option<T>, default: T) -> T {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(from_name)
        .unwrap_or(default)
}
