//! The common assets a world needs, as `WorldSettings` lists them (spec 05):
//! every file under `Common/` in `Assets.zip`, by its path without that
//! prefix (the key the client looks up, such as
//! `Music/Zone4/Z4_Mage_Tower_Fire.ogg`) and the SHA-256 of its bytes.
//! The client asks only for the ones it lacks; Aerie does not serve files
//! yet, so the hashes have to match the client's own copy.

use std::path::Path;

use aerie_assets::pack::Pack;
use aerie_protocol::types::AssetRef;
use anyhow::Result;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

const PREFIX: &str = "Common/";

/// Bump when `list` changes, to drop cached lists.
pub const VERSION: u32 = 1;

pub fn list(zip: &Path) -> Result<Vec<AssetRef>> {
    let t = std::time::Instant::now();
    let pack = Pack::open(zip)?;
    let names: Vec<String> = pack
        .list("Common", "")
        .into_iter()
        .filter(|n| !n.ends_with('/'))
        .map(str::to_owned)
        .collect();
    let mut out = Vec::with_capacity(names.len());
    for n in &names {
        let bytes = pack.read(n)?;
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        out.push(AssetRef {
            hash,
            name: n[PREFIX.len()..].to_owned(),
        });
    }
    info!("hashed {} common assets in {:.1?}", out.len(), t.elapsed());
    Ok(out)
}

/// Compares the built list with a captured `WorldSettings` payload and logs
/// what differs. Returns the number of differences.
pub fn compare(built: &[AssetRef], captured_payload: &[u8]) -> usize {
    use std::collections::HashMap;
    // WorldSettings: one nullable-bits byte, then the varint-counted list.
    let Some(rest) = captured_payload.get(1..) else {
        warn!("captured WorldSettings is empty");
        return 1;
    };
    let captured = match aerie_protocol::types::decode_assets(rest) {
        Ok((a, _)) => a,
        Err(e) => {
            warn!("captured WorldSettings does not decode: {e}");
            return 1;
        }
    };
    if let Some(a) = captured.first() {
        info!(
            "captured WorldSettings: {} assets; first: {} {}",
            captured.len(),
            a.hash,
            a.name
        );
    }
    if let Some(a) = built.first() {
        info!(
            "built WorldSettings: {} assets; first: {} {}",
            built.len(),
            a.hash,
            a.name
        );
    }
    let theirs: HashMap<&str, &str> = captured
        .iter()
        .map(|a| (a.name.as_str(), a.hash.as_str()))
        .collect();
    let ours: HashMap<&str, &str> = built
        .iter()
        .map(|a| (a.name.as_str(), a.hash.as_str()))
        .collect();
    let mut n = 0;
    let mut shown = 0;
    let mut show = |what: &str, name: &str| {
        if shown < 10 {
            warn!("WorldSettings: {what}: {name}");
            shown += 1;
        }
    };
    for (k, h) in &theirs {
        match ours.get(k) {
            None => {
                n += 1;
                show("only in the capture", k);
            }
            Some(o) if o != h => {
                n += 1;
                show("hash differs", k);
            }
            _ => {}
        }
    }
    for k in ours.keys() {
        if !theirs.contains_key(k) {
            n += 1;
            show("only built", k);
        }
    }
    if n == 0 {
        info!(
            "WorldSettings: all {} assets match the capture",
            built.len()
        );
    } else {
        warn!("WorldSettings: {n} differences with the capture");
    }
    n
}
