//! Builds each registry from the local Assets.zip and compares it with a
//! `probe` capture of the official server.
//!
//! cargo run --release -p aerie-assets --example compare -- <capture dir> [Assets.zip]

use std::{fs, path::PathBuf};

use aerie_assets::{ctx::Ctx, pack::Pack, registries};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("capture dir"));
    let zip = args
        .next()
        .map(PathBuf::from)
        .or_else(Pack::default_path)
        .expect("Assets.zip path");
    let ctx = Ctx::new(Pack::open(&zip).expect("open Assets.zip"));

    let index = fs::read_to_string(dir.join("index.tsv")).unwrap();
    // A registry sent more than once (an update before the full list) is
    // compared by its largest packet.
    let captured = |id: u32| -> Option<Vec<u8>> {
        index
            .lines()
            .filter(|l| l.split('\t').nth(1) == Some(&id.to_string()))
            .filter_map(|line| {
                let c: Vec<&str> = line.split('\t').collect();
                fs::read(dir.join(format!(
                    "{:04}-{}-{}.bin",
                    c[0].parse::<u32>().unwrap(),
                    c[1],
                    c[2]
                )))
                .ok()
            })
            .max_by_key(Vec::len)
    };
    // Pin the capture's indexes where the official order is not stable.
    registries::pin_from_capture(&ctx, captured).unwrap();
    let mut failed = 0;
    for reg in registries::all() {
        let Some(captured) = captured(reg.id) else {
            println!("{:<34} no capture", reg.name);
            continue;
        };
        let built = match (reg.build)(&ctx) {
            Ok(b) => b,
            Err(e) => {
                println!("{:<34} BUILD FAILED: {e}", reg.name);
                failed += 1;
                continue;
            }
        };
        let (a, b) = (
            (reg.canonical)(&built).unwrap(),
            (reg.canonical)(&captured).unwrap(),
        );
        if a == b {
            println!(
                "{:<34} same ({} bytes built, {} captured)",
                reg.name,
                built.len(),
                captured.len()
            );
        } else {
            failed += 1;
            let (la, lb): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
            let at = la
                .iter()
                .zip(&lb)
                .position(|(x, y)| x != y)
                .unwrap_or(la.len().min(lb.len()));
            println!("{:<34} DIFFERS at line {at}:", reg.name);
            for (k, l) in lb.iter().enumerate().skip(at.saturating_sub(6)).take(10) {
                println!("    captured {k:>5} {l}");
            }
            for (k, l) in la.iter().enumerate().skip(at.saturating_sub(6)).take(10) {
                println!("    built    {k:>5} {l}");
            }
        }
    }
    std::process::exit(failed);
}
