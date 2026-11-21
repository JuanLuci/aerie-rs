//! One asset folder (spec 07, "Loading").
//!
//! - Every `.json` under `Server/<path>/` is an asset; its key is the file name
//!   without extension.
//! - A `Parent` key names another asset of the same folder to inherit from.
//!   **Only some fields inherit**, and which ones depends on the asset type
//!   (most do not). `assets` holds every field merged down the parent chain;
//!   `own` holds what each file says itself; `inherited` merges just the
//!   fields a registry names. Merging: the child's value replaces the
//!   parent's, nested objects merge field by field.
//! - Indexed registries number their assets: built-in entries first, then
//!   the assets without a parent, then those with one (see `index_order`).

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{Error, Result, chm::Chm, pack::Pack};

/// A folder's assets, by key.
#[derive(Clone)]
pub struct Store {
    pub path: String,
    /// Every field merged down the parent chain.
    pub assets: BTreeMap<String, Value>,
    /// Each file's own fields, without inheritance (`Parent` removed).
    pub own: BTreeMap<String, Value>,
    /// File path inside the pack, by key.
    pub files: BTreeMap<String, String>,
    /// Parent key, for assets that name one.
    pub parents: BTreeMap<String, String>,
    /// Keys of the contained assets (no file), in the order found.
    pub contained: Vec<String>,
    /// For each contained asset, the type and key of the asset it was
    /// written in.
    pub contained_files: BTreeMap<String, (String, String, String)>,
}

/// Parses Hytale's JSON, which tolerates comments and trailing commas.
pub fn parse(file: &str, bytes: &[u8]) -> Result<Value> {
    let text = std::str::from_utf8(bytes).map_err(|e| Error::Json {
        file: file.into(),
        msg: e.to_string(),
    })?;
    let text = text.trim_start_matches('\u{feff}');
    serde_json::from_str(text)
        .or_else(|_| json5::from_str(text))
        .map_err(|e| Error::Json {
            file: file.into(),
            msg: e.to_string(),
        })
}

/// Child over parent: objects merge key by key, everything else is replaced.
pub fn merge(parent: &Value, child: &Value) -> Value {
    match (parent, child) {
        (Value::Object(p), Value::Object(c)) => {
            let mut out: Map<String, Value> = p.clone();
            for (k, v) in c {
                let merged = match p.get(k) {
                    Some(pv) => merge(pv, v),
                    None => v.clone(),
                };
                out.insert(k.clone(), merged);
            }
            out.into()
        }
        _ => child.clone(),
    }
}

impl Store {
    /// A folder with no assets (for built-in entries).
    pub fn empty(path: &str) -> Self {
        Self {
            path: path.into(),
            assets: BTreeMap::new(),
            own: BTreeMap::new(),
            files: BTreeMap::new(),
            parents: BTreeMap::new(),
            contained: Vec::new(),
            contained_files: BTreeMap::new(),
        }
    }

    pub fn load(pack: &Pack, path: &str) -> Result<Self> {
        Self::load_ext(pack, path, ".json")
    }

    /// Assets of `Server/<path>` whose files end in `ext` (most use `.json`).
    pub fn load_ext(pack: &Pack, path: &str, ext: &str) -> Result<Self> {
        let mut raw: BTreeMap<String, Value> = BTreeMap::new();
        let mut files = BTreeMap::new();
        for name in pack.list(&format!("Server/{path}"), ext) {
            let key = name
                .rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(ext)
                .to_owned();
            raw.insert(key.clone(), parse(name, &pack.read(name)?)?);
            files.insert(key, name.to_owned());
        }
        Self::build(path, raw, files, Vec::new(), BTreeMap::new(), false)
    }

    /// This folder's file assets plus the contained assets of its type
    /// (spec 07, "Contained assets"), which come after the files in index
    /// order. A contained asset whose parent is missing inherits nothing.
    pub fn with_contained(&self, extra: &[crate::contained::Contained]) -> Result<Self> {
        if extra.is_empty() {
            return Ok(self.clone());
        }
        let mut raw = self.own.clone();
        for (k, p) in &self.parents {
            if let (Some(Value::Object(m)), false) = (raw.get_mut(k), self.contained.contains(k)) {
                m.insert("Parent".into(), Value::String(p.clone()));
            }
        }
        let mut order = Vec::new();
        let mut cparents = BTreeMap::new();
        let mut cfiles = BTreeMap::new();
        for c in extra {
            if raw.contains_key(&c.key) {
                continue;
            }
            let mut v = c.value.clone();
            if let Value::Object(m) = &mut v {
                m.remove("Parent");
            }
            raw.insert(c.key.clone(), v);
            cfiles.insert(
                c.key.clone(),
                (c.owner_class.clone(), c.owner.clone(), c.root_class.clone()),
            );
            if let Some(p) = &c.parent {
                cparents.insert(c.key.clone(), p.clone());
            }
            order.push(c.key.clone());
        }
        let mut s = Self::build(&self.path, raw, self.files.clone(), order, cparents, true)?;
        s.contained_files = cfiles;
        Ok(s)
    }

    /// `raw` holds every asset as written, file assets with their `Parent`;
    /// `contained` lists the contained keys in order, with their parents in
    /// `cparents` (their own `Parent` already applied or removed).
    fn build(
        path: &str,
        mut raw: BTreeMap<String, Value>,
        files: BTreeMap<String, String>,
        contained: Vec<String>,
        cparents: BTreeMap<String, String>,
        lenient: bool,
    ) -> Result<Self> {
        let lower: std::collections::HashMap<String, String> =
            raw.keys().map(|k| (k.to_lowercase(), k.clone())).collect();
        let mut parents = BTreeMap::new();
        for (key, v) in &raw {
            let named = v
                .get("Parent")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| cparents.get(key).cloned());
            if let Some(p) = named {
                let p = lower.get(&p.to_lowercase()).cloned().unwrap_or(p);
                if lenient && !raw.contains_key(&p) {
                    continue;
                }
                parents.insert(key.clone(), p);
            }
        }
        for v in raw.values_mut() {
            if let Value::Object(m) = v {
                m.remove("Parent");
            }
        }
        let mut assets = BTreeMap::new();
        for key in raw.keys() {
            assets.insert(key.clone(), resolve(&raw, &parents, key, 0)?);
        }
        Ok(Self {
            path: path.into(),
            assets,
            own: raw,
            files,
            parents,
            contained,
            contained_files: BTreeMap::new(),
        })
    }

    /// The asset's own fields, plus each of `keys` taken from the nearest of
    /// the asset and its ancestors that sets it.
    pub fn inherited(&self, key: &str, keys: &[&str]) -> Value {
        let mut out = self.own.get(key).cloned().unwrap_or(Value::Null);
        let mut chain = Vec::new();
        let mut k = self.parents.get(key);
        while let Some(p) = k {
            if chain.len() > 32 || !self.own.contains_key(p) {
                break;
            }
            chain.push(p.clone());
            k = self.parents.get(p);
        }
        if let Value::Object(m) = &mut out {
            for name in keys {
                let mut acc: Option<Value> = None;
                for p in chain
                    .iter()
                    .rev()
                    .map(String::as_str)
                    .chain(std::iter::once(key))
                {
                    if let Some(v) = self.own.get(p).and_then(|a| a.get(*name)) {
                        // The nearest value wins whole: inside nested objects the
                        // fields almost never inherit (see `nested` for those that do).
                        acc = Some(v.clone());
                    }
                }
                if let Some(v) = acc {
                    m.insert((*name).to_owned(), v);
                }
            }
        }
        out
    }

    /// The asset resolved down its parent chain by the rules of its type's
    /// schema (spec 07, "Inheritance"): which fields inherit, and inside
    /// object fields which of their fields do. `class` is the schema name of
    /// the folder's type (`EntityEffect`).
    pub fn resolved(&self, key: &str, class: &str) -> Value {
        let mut chain = vec![key.to_owned()];
        let mut k = self.parents.get(key);
        while let Some(p) = k {
            if chain.len() > 32 || !self.own.contains_key(p) || chain.contains(p) {
                break;
            }
            chain.push(p.clone());
            k = self.parents.get(p);
        }
        let values: Vec<&Value> = chain.iter().rev().filter_map(|k| self.own.get(k)).collect();
        crate::schema::get().inherit_chain(class, &values)
    }

    /// A map field whose entries combine down the parent chain: the child's
    /// entries replace the parent's entries with the same key.
    pub fn combined_map(&self, key: &str, field: &str) -> Option<Map<String, Value>> {
        let parent = self
            .parents
            .get(key)
            .filter(|p| self.own.contains_key(*p) && *p != key);
        let mut out = parent.and_then(|p| self.combined_map(p, field));
        if let Some(Value::Object(own)) = self.own.get(key).and_then(|a| a.get(field)) {
            let m = out.get_or_insert_with(Map::new);
            for (k, v) in own {
                m.insert(k.clone(), v.clone());
            }
        }
        out
    }

    /// A nested object field that inherits as a whole, but inside which only
    /// `subkeys` inherit: when the asset sets `field` itself, its object is
    /// used with just `subkeys` filled in from the parent's; otherwise the
    /// parent's object is used as it is.
    pub fn nested(&self, key: &str, field: &str, subkeys: &[&str]) -> Option<Value> {
        let parent = self
            .parents
            .get(key)
            .filter(|p| self.own.contains_key(*p) && *p != key);
        let from_parent = parent.and_then(|p| self.nested(p, field, subkeys));
        match self.own.get(key).and_then(|a| a.get(field)) {
            Some(own) => {
                let mut out = own.clone();
                if let (Value::Object(m), Some(Value::Object(pm))) = (&mut out, &from_parent) {
                    for k in subkeys {
                        if !m.contains_key(*k) {
                            if let Some(v) = pm.get(*k) {
                                m.insert((*k).to_owned(), v.clone());
                            }
                        }
                    }
                }
                Some(out)
            }
            None => from_parent,
        }
    }

    /// Keys in index order (spec 07, "Indexes"): `builtin` first; then the
    /// assets without a parent, in reverse alphabetical order of their paths;
    /// then the assets with a parent, in passes (each pass takes those whose
    /// parent is already loaded) in the iteration order of the official
    /// loader's waiting map. That last order is not stable even on the
    /// official server: keys in the same hash bucket can swap between runs.
    pub fn index_order(&self, builtin: &[&str]) -> Vec<String> {
        // Keys are case-insensitive: a file named like a built-in entry
        // replaces it and keeps its index, under the file's spelling.
        let is_builtin = |k: &str| builtin.iter().any(|b| b.eq_ignore_ascii_case(k));
        let mut by_path: Vec<(&String, &String)> = self
            .files
            .iter()
            .filter(|(k, _)| !is_builtin(k))
            .map(|(k, p)| (p, k))
            .collect();
        by_path.sort();
        by_path.reverse();

        let mut out: Vec<String> = builtin
            .iter()
            .map(|b| {
                self.files
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case(b))
                    .cloned()
                    .unwrap_or_else(|| b.to_string())
            })
            .collect();
        let mut loaded: std::collections::HashSet<String> = out.iter().cloned().collect();
        let mut waiting = Chm::default();
        for (_, k) in &by_path {
            if self.parents.contains_key(*k) {
                waiting.insert(k);
            } else {
                out.push((*k).clone());
                loaded.insert((*k).clone());
            }
        }
        while !waiting.is_empty() {
            let keys = waiting.keys();
            let now: Vec<String> = keys
                .iter()
                .filter(|k| {
                    let p = &self.parents[*k];
                    loaded.contains(p) || !keys.contains(p)
                })
                .cloned()
                .collect();
            if now.is_empty() {
                break;
            }
            for k in &now {
                waiting.remove(k);
                loaded.insert(k.clone());
            }
            out.extend(now);
        }
        // Contained assets after the files (spec 07, "Contained assets"). The
        // official loader adds them in batches, one per owner type and nesting
        // level, in the order the batches are found. In a batch the owners come
        // in the iteration order of a `HashMap` keyed by owner, each owner's
        // assets in the order written; those with a parent wait, like file
        // assets, in passes.
        // Batches: (outermost owner type, owner type, nesting level). Those
        // written inside this folder's own assets load with the folder; the
        // others load later with their owner's folder.
        let mut batches: Vec<((String, String, usize), Vec<&String>)> = Vec::new();
        for k in &self.contained {
            let (oc, ok, rc) = &self.contained_files[k];
            let id = (
                rc.clone(),
                oc.clone(),
                ok.chars().take_while(|c| *c == '*').count(),
            );
            match batches.iter_mut().find(|(b, _)| *b == id) {
                Some((_, v)) => v.push(k),
                None => batches.push((id, vec![k])),
            }
        }
        let own_class = crate::schema::get()
            .asset_types()
            .into_iter()
            .find(|t| t.store_path() == self.path)
            .map(|t| t.name.to_owned());
        let first_root: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for (b, _) in &batches {
                if !v.contains(&b.0) {
                    v.push(b.0.clone());
                }
            }
            v
        };
        batches.sort_by_key(|((rc, _, depth), _)| {
            let own = Some(rc) != own_class.as_ref();
            let pos = first_root
                .iter()
                .position(|r| r == rc)
                .unwrap_or(usize::MAX);
            (own, pos, *depth)
        });
        for (_, keys) in batches {
            let owners: Vec<String> = keys
                .iter()
                .map(|k| self.contained_files[*k].1.clone())
                .collect();
            let mut waiting = Chm::default();
            for o in crate::chm::hashmap_order(&owners) {
                for k in keys.iter().filter(|k| self.contained_files[**k].1 == o) {
                    if self.parents.contains_key(*k) {
                        waiting.insert(k);
                    } else {
                        out.push((*k).clone());
                        loaded.insert((*k).clone());
                    }
                }
            }
            while !waiting.is_empty() {
                let wk = waiting.keys();
                let mut now: Vec<String> = wk
                    .iter()
                    .filter(|k| {
                        let p = &self.parents[*k];
                        loaded.contains(p) || !wk.contains(p)
                    })
                    .cloned()
                    .collect();
                if now.is_empty() {
                    now = wk;
                }
                for k in &now {
                    waiting.remove(k);
                    loaded.insert(k.clone());
                }
                out.extend(now);
            }
        }
        out
    }
}

/// `raw` without `Parent` fields; `parents` holds the resolved parent keys.
fn resolve(
    raw: &BTreeMap<String, Value>,
    parents: &BTreeMap<String, String>,
    key: &str,
    depth: usize,
) -> Result<Value> {
    let v = &raw[key];
    let Some(parent) = parents.get(key) else {
        return Ok(v.clone());
    };
    if depth > 32 {
        return Err(Error::Invalid {
            asset: key.into(),
            msg: "Parent chain too deep".into(),
        });
    }
    if !raw.contains_key(parent) {
        return Err(Error::MissingParent {
            asset: key.into(),
            parent: parent.clone(),
        });
    }
    Ok(merge(&resolve(raw, parents, parent, depth + 1)?, v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn child_overrides_and_nested_objects_merge() {
        let p = json!({"A": 1, "N": {"x": 1, "y": 2}, "L": [1, 2]});
        let c = json!({"A": 5, "N": {"y": 9}, "L": [3]});
        assert_eq!(
            merge(&p, &c),
            json!({"A": 5, "N": {"x": 1, "y": 9}, "L": [3]})
        );
    }

    #[test]
    fn lenient_json() {
        assert_eq!(
            parse("t", b"{ // c\n \"a\": 1, }").unwrap(),
            json!({"a": 1})
        );
    }
}
