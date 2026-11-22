//! Prints captured packets as Rust debug text.
//!
//! One packet:
//!   cargo run -p aerie-protocol --example dump -- <packet id> <payload file>
//! A whole capture folder (its `index.tsv` and `.bin` files) in one run,
//! only the packet ids listed; each packet starts with a line
//! `### <sequence> <packet id> <name>`:
//!   cargo run -p aerie-protocol --example dump -- --capture <dir> <id,id,...>

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--capture") {
        return capture(&args[1], args.get(2).map(String::as_str).unwrap_or(""));
    }
    let id: u32 = args.first().expect("packet id").parse().unwrap();
    let data = std::fs::read(args.get(1).expect("payload file")).unwrap();
    match aerie_protocol::proto::debug(id, &data) {
        Ok(Some(text)) => println!("{text}"),
        Ok(None) => eprintln!("no generated type for packet {id}"),
        Err(e) => eprintln!("decode failed: {e}"),
    }
}

fn capture(dir: &str, ids: &str) {
    let want: Vec<u32> = ids
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let dir = std::path::Path::new(dir);
    let mut files = std::collections::HashMap::new();
    for e in std::fs::read_dir(dir).expect("capture folder").flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(seq) = name
            .strip_suffix(".bin")
            .and_then(|n| n.split('-').next())
            .and_then(|n| n.parse::<u64>().ok())
        {
            files.insert(seq, e.path());
        }
    }
    let index = std::fs::read_to_string(dir.join("index.tsv")).expect("index.tsv");
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    for line in index.lines() {
        let mut f = line.split('\t');
        let (Some(seq), Some(id)) = (
            f.next().and_then(|s| s.parse::<u64>().ok()),
            f.next().and_then(|s| s.parse::<u32>().ok()),
        ) else {
            continue;
        };
        if !want.is_empty() && !want.contains(&id) {
            continue;
        }
        let name = f.next().unwrap_or("");
        let Some(data) = files.get(&seq).and_then(|p| std::fs::read(p).ok()) else {
            continue;
        };
        let _ = writeln!(out, "### {seq} {id} {name}");
        match aerie_protocol::proto::debug(id, &data) {
            Ok(Some(text)) => {
                let _ = writeln!(out, "{text}");
            }
            Ok(None) => {}
            Err(e) => {
                let _ = writeln!(out, "decode failed: {e}");
            }
        }
    }
}
