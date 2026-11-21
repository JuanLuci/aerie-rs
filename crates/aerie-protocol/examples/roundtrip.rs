//! Decodes every packet of one or more `probe` capture folders with the
//! generated code and encodes it again; the bytes must be identical.
//!
//! cargo run --release -p aerie-protocol --example roundtrip -- <capture dir>...

use std::{collections::BTreeMap, fs, path::Path};

fn main() {
    let (mut ok, mut skipped) = (0, 0);
    let mut fails: BTreeMap<String, String> = BTreeMap::new();
    for dir in std::env::args().skip(1) {
        let dir = Path::new(&dir);
        let index = fs::read_to_string(dir.join("index.tsv")).expect("index.tsv");
        for line in index.lines() {
            let c: Vec<&str> = line.split('\t').collect();
            let (seq, id, name) = (
                c[0].parse::<u32>().unwrap(),
                c[1].parse::<u32>().unwrap(),
                c[2],
            );
            let data = fs::read(dir.join(format!("{seq:04}-{id}-{name}.bin"))).unwrap();
            match aerie_protocol::proto::reencode(id, &data) {
                Ok(Some(out)) if out == data => ok += 1,
                Ok(Some(out)) => {
                    let at = out
                        .iter()
                        .zip(&data)
                        .position(|(a, b)| a != b)
                        .unwrap_or(out.len().min(data.len()));
                    fails.entry(name.into()).or_insert(format!(
                        "differs at byte {at} (len {} vs {})",
                        out.len(),
                        data.len()
                    ));
                }
                Ok(None) => skipped += 1,
                Err(e) => {
                    fails.entry(name.into()).or_insert(e.to_string());
                }
            }
        }
    }
    println!(
        "round trip: {ok} identical, {} types failing, {skipped} without a generated type",
        fails.len()
    );
    for (k, v) in &fails {
        println!("  {k}: {v}");
    }
    if !fails.is_empty() {
        std::process::exit(1);
    }
}
