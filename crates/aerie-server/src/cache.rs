//! Start-up cache: the registries (and the common asset list) built from
//! `Assets.zip` are saved and reused while nothing they depend on changes.
//! The key covers the zip (path, size, modification time), the code that
//! builds them (`aerie_assets::CODE_HASH`, `common::VERSION`) and the
//! capture used for pins, if any. Files live in `%LOCALAPPDATA%\aerie\cache` (or the temp
//! directory); `--no-cache` skips them.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use aerie_protocol::types::AssetRef;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const MAGIC: &[u8] = b"AERIE-CACHE-6\n";

fn dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("aerie")
        .join("cache")
}

fn stamp(h: &mut Sha256, p: &Path) {
    h.update(p.to_string_lossy().as_bytes());
    if let Ok(m) = std::fs::metadata(p) {
        h.update(m.len().to_le_bytes());
        if let Ok(t) = m.modified().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map_err(std::io::Error::other)
        }) {
            h.update(t.as_nanos().to_le_bytes());
        }
    }
}

/// The cache key for `kind` built from `zip` (and the capture's packets).
pub fn key(kind: &str, zip: &Path, capture: Option<&[(u32, Vec<u8>)]>) -> String {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    stamp(&mut h, zip);
    // The code that builds them, not the whole executable: server changes
    // elsewhere keep the cache.
    h.update(aerie_assets::CODE_HASH.as_bytes());
    h.update(crate::common::VERSION.to_le_bytes());
    for (id, p) in capture.unwrap_or(&[]) {
        h.update(id.to_le_bytes());
        h.update(Sha256::digest(p));
    }
    let d = h.finalize();
    format!(
        "{kind}-{}",
        d[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

struct W(Vec<u8>);
impl W {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
}

struct R<'a>(&'a [u8]);
impl R<'_> {
    fn u32(&mut self) -> Result<u32> {
        if self.0.len() < 4 {
            bail!("cache file ends early");
        }
        let v = u32::from_le_bytes(self.0[..4].try_into().unwrap());
        self.0 = &self.0[4..];
        Ok(v)
    }
    fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.u32()? as usize;
        if self.0.len() < n {
            bail!("cache file ends early");
        }
        let v = self.0[..n].to_vec();
        self.0 = &self.0[n..];
        Ok(v)
    }
    fn string(&mut self) -> Result<String> {
        Ok(String::from_utf8(self.bytes()?)?)
    }
}

fn write(key: &str, body: Vec<u8>) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = dir().join(format!("{key}.tmp"));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(MAGIC)?;
    f.write_all(&body)?;
    drop(f);
    std::fs::rename(&tmp, dir().join(key))?;
    prune(key);
    Ok(())
}

/// Keeps the newest two caches of a kind (`kind-hash`): every code or asset
/// change makes a new one, and each is tens of MB.
fn prune(key: &str) {
    let Some((kind, _)) = key.split_once('-') else {
        return;
    };
    let Ok(dir) = std::fs::read_dir(dir()) else {
        return;
    };
    let mut same: Vec<(std::time::SystemTime, PathBuf)> = dir
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with(&format!("{kind}-")) && !n.ends_with(".tmp")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    same.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in same.into_iter().skip(2) {
        let _ = std::fs::remove_file(p);
    }
}

fn read(key: &str) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    std::fs::File::open(dir().join(key))
        .ok()?
        .read_to_end(&mut v)
        .ok()?;
    v.strip_prefix(MAGIC).map(<[u8]>::to_vec)
}

/// Registry packets, block ids by lower-cased key, block keys as written,
/// and other things built from the assets, by name (see `main::build_registries`).
pub type Registries = (
    Vec<(u32, Vec<u8>)>,
    HashMap<String, i32>,
    Vec<String>,
    Vec<(String, Vec<u8>)>,
);

pub fn save_registries(key: &str, r: &Registries) -> Result<()> {
    let mut w = W(Vec::new());
    w.u32(r.0.len() as u32);
    for (id, p) in &r.0 {
        w.u32(*id);
        w.bytes(p);
    }
    w.u32(r.1.len() as u32);
    for (k, i) in &r.1 {
        w.bytes(k.as_bytes());
        w.u32(*i as u32);
    }
    w.u32(r.2.len() as u32);
    for n in &r.2 {
        w.bytes(n.as_bytes());
    }
    w.u32(r.3.len() as u32);
    for (n, b) in &r.3 {
        w.bytes(n.as_bytes());
        w.bytes(b);
    }
    write(key, w.0).context("saving the registry cache")
}

pub fn load_registries(key: &str) -> Option<Registries> {
    let body = read(key)?;
    let mut r = R(&body);
    let mut parse = || -> Result<Registries> {
        let mut regs = Vec::new();
        for _ in 0..r.u32()? {
            regs.push((r.u32()?, r.bytes()?));
        }
        let mut blocks = HashMap::new();
        for _ in 0..r.u32()? {
            blocks.insert(r.string()?, r.u32()? as i32);
        }
        let mut names = Vec::new();
        for _ in 0..r.u32()? {
            names.push(r.string()?);
        }
        let mut extras = Vec::new();
        for _ in 0..r.u32()? {
            extras.push((r.string()?, r.bytes()?));
        }
        Ok((regs, blocks, names, extras))
    };
    parse().ok()
}

pub fn save_assets(key: &str, list: &[AssetRef]) -> Result<()> {
    let mut w = W(Vec::new());
    w.u32(list.len() as u32);
    for a in list {
        w.bytes(a.hash.as_bytes());
        w.bytes(a.name.as_bytes());
    }
    write(key, w.0).context("saving the asset list cache")
}

pub fn load_assets(key: &str) -> Option<Vec<AssetRef>> {
    let body = read(key)?;
    let mut r = R(&body);
    let mut parse = || -> Result<Vec<AssetRef>> {
        let mut v = Vec::new();
        for _ in 0..r.u32()? {
            v.push(AssetRef {
                hash: r.string()?,
                name: r.string()?,
            });
        }
        Ok(v)
    };
    parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let k = format!("test-{}", std::process::id());
        let regs: Registries = (
            vec![(40, vec![1, 2, 3]), (66, vec![])],
            HashMap::from([("rock_stone".into(), 7)]),
            vec!["Rock_Stone".into()],
            vec![("player_model".into(), vec![9, 9])],
        );
        save_registries(&k, &regs).unwrap();
        assert_eq!(load_registries(&k).unwrap(), regs);
        let assets = vec![AssetRef {
            hash: "ab".repeat(32),
            name: "Music/x.ogg".into(),
        }];
        let ka = format!("{k}-a");
        save_assets(&ka, &assets).unwrap();
        assert_eq!(load_assets(&ka).unwrap(), assets);
        let _ = std::fs::remove_file(dir().join(k));
        let _ = std::fs::remove_file(dir().join(ka));
    }
}
