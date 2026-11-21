//! Build-time generation from the tables `tools/gen.py` writes (`gen/`):
//! - packets.tsv → the packet table (`table`),
//! - protocol.json → every protocol type with its codec (`proto`).

#[path = "build_codegen.rs"]
mod codegen;

use std::{env, fs, path::PathBuf};

fn main() {
    let root = gen_dir(&PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()));
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let packets = root.join("packets.tsv");
    let protocol = root.join("protocol.json");
    println!("cargo:rerun-if-changed={}", packets.display());
    println!("cargo:rerun-if-changed={}", protocol.display());
    println!("cargo:rerun-if-changed=build_codegen.rs");

    let text = fs::read_to_string(&packets).expect("read packets.tsv");
    let mut table = String::from("pub static PACKETS: &[PacketInfo] = &[\n");
    let mut names = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("id\t"))
    {
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() < 7 {
            continue;
        }
        let dir = match c[2] {
            "ToServer" => "Direction::ToServer",
            "ToClient" => "Direction::ToClient",
            _ => "Direction::Both",
        };
        table += &format!(
            "    PacketInfo {{ id: {}, name: {:?}, direction: {dir}, channel: Channel::{}, fixed_size: {}, max_size: {}, compressed: {} }},\n",
            c[0],
            c[1],
            c[3],
            c[4],
            c[5],
            c[6] == "True"
        );
        names.push((c[0].parse::<u32>().unwrap(), c[1].to_owned()));
    }
    table += "];\n";
    fs::write(out_dir.join("packets.rs"), table).unwrap();

    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&protocol).expect("read protocol.json")).unwrap();
    fs::write(
        out_dir.join("proto.rs"),
        codegen::Gen::new(&doc).generate(&names),
    )
    .unwrap();
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
