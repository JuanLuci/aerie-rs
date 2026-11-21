//! Hashes the code the registries are built from (this crate, the protocol
//! crate and the schemas), so the server's start-up cache survives changes
//! elsewhere and is rebuilt when this code changes.

use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let tables = gen_dir(&here);
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::copy(tables.join("assets.json"), out.join("assets.json")).expect("copy assets.json");
    let inputs = [
        here.join("src"),
        here.join("../aerie-protocol/src"),
        here.join("../aerie-protocol/build_codegen.rs"),
        tables.join("protocol.json"),
        tables.join("assets.json"),
        tables.join("packets.tsv"),
    ];
    let mut all = Vec::new();
    for i in &inputs {
        println!("cargo:rerun-if-changed={}", i.display());
        if i.is_dir() {
            files(i, &mut all);
        } else {
            all.push(i.clone());
        }
    }
    all.sort();
    // FNV-1a over names and contents: a cache key, not a security hash.
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |b: &[u8]| {
        for x in b {
            h ^= *x as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    for f in &all {
        let rel = f
            .strip_prefix(&here)
            .or_else(|_| f.strip_prefix(&tables))
            .unwrap_or(f);
        eat(rel.to_string_lossy().replace('\\', "/").as_bytes());
        // Line endings may differ between checkouts.
        let text = std::fs::read(f).unwrap_or_default();
        let text: Vec<u8> = text.into_iter().filter(|&b| b != b'\r').collect();
        eat(&text);
    }
    println!("cargo:rustc-env=AERIE_ASSETS_CODE_HASH={h:016x}");
}

/// Where the generated tables are (`tools/gen.py`): `AERIE_GEN_DIR`, else
/// `gen/` at the repository root, else `docs/spec/` (older checkouts).
fn gen_dir(manifest: &std::path::Path) -> std::path::PathBuf {
    println!("cargo:rerun-if-env-changed=AERIE_GEN_DIR");
    if let Some(d) = std::env::var_os("AERIE_GEN_DIR") {
        return d.into();
    }
    let root = manifest.join("../..");
    for d in [root.join("gen"), root.join("docs/spec")] {
        if d.join("protocol.json").is_file() {
            return d;
        }
    }
    panic!(
        "

The protocol and asset tables are missing. Generate them once from your own copy of the game:

    python tools/gen.py

(see tools/gen.py for options). They go to gen/ and are not part of this repository.

"
    );
}
